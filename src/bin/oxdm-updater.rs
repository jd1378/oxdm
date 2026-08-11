//! oxdm self-update worker (artifact mode).
//!
//! Spawned by the running oxdm GUI **after** it has already fetched the
//! update artifact through the regular download pipeline. The helper's
//! sole job is the dangerous half: verify the SHA-256, wait for the
//! parent to exit, atomically replace the running executable, and
//! relaunch it.
//!
//! ## Protocol
//!
//! Stdout: one JSON message per line, matching `UpdaterEvent` in the
//! data layer. Stages: `started → verified → ready → (await stdin
//! "go") → installing → done` (or `error`).
//!
//! Stdin: a single `go\n` from the parent greenlights the swap.
//!
//! ## CLI
//!
//! ```text
//! oxdm-updater --exe <PATH> --pid <PID> --artifact <PATH> --sha256 <HEX>
//! ```

use std::io::{self, BufRead, Read, Write};
use std::path::PathBuf;
use std::time::Duration;

use sha2::{Digest, Sha256};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            emit(&Event::Error { message: e });
            std::process::exit(2);
        }
    };
    if let Err(e) = run(args).await {
        emit(&Event::Error { message: e });
        std::process::exit(1);
    }
}

struct Args {
    exe: PathBuf,
    pid: u32,
    artifact: PathBuf,
    sha256: String,
}

fn parse_args() -> Result<Args, String> {
    let mut argv = std::env::args().skip(1);
    let mut exe: Option<PathBuf> = None;
    let mut pid: Option<u32> = None;
    let mut artifact: Option<PathBuf> = None;
    let mut sha256: Option<String> = None;
    while let Some(flag) = argv.next() {
        let val = argv
            .next()
            .ok_or_else(|| format!("missing value for {flag}"))?;
        match flag.as_str() {
            "--exe" => exe = Some(PathBuf::from(val)),
            "--pid" => pid = Some(val.parse().map_err(|_| "invalid pid".to_string())?),
            "--artifact" => artifact = Some(PathBuf::from(val)),
            "--sha256" => sha256 = Some(val),
            other => return Err(format!("unknown flag: {other}")),
        }
    }
    Ok(Args {
        exe: exe.ok_or_else(|| "missing --exe".to_string())?,
        pid: pid.ok_or_else(|| "missing --pid".to_string())?,
        artifact: artifact.ok_or_else(|| "missing --artifact".to_string())?,
        sha256: sha256.ok_or_else(|| "missing --sha256".to_string())?,
    })
}

async fn run(args: Args) -> Result<(), String> {
    emit(&Event::Started);

    // 1. Verify. The artifact lives on disk already; stream-hash it on
    // a blocking thread so the tokio runtime can keep handling stdin.
    let path = args.artifact.clone();
    let actual = tokio::task::spawn_blocking(move || hash_file(&path))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("hash: {e}"))?;
    if !ct_eq(&actual, &args.sha256.to_lowercase()) {
        return Err(format!(
            "checksum mismatch: expected {}, got {}",
            args.sha256, actual
        ));
    }
    emit(&Event::Verified);

    // 2. Ready — wait for the parent to confirm + exit.
    emit(&Event::Ready);
    wait_for_go().await?;

    emit(&Event::Installing);
    wait_for_pid_exit(args.pid).await;

    // 3. Swap. Move the artifact into place atomically. On Windows
    // a brief retry tides over handle release.
    swap_executable(&args.artifact, &args.exe)?;

    // 4. Relaunch from the same path. Detach so this updater can exit.
    spawn_detached(&args.exe).map_err(|e| format!("relaunch: {e}"))?;

    emit(&Event::Done);
    Ok(())
}

fn hash_file(path: &PathBuf) -> io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

async fn wait_for_go() -> Result<(), String> {
    let line = tokio::task::spawn_blocking(|| {
        let mut buf = String::new();
        let stdin = io::stdin();
        let mut h = stdin.lock();
        h.read_line(&mut buf).map(|_| buf)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    if line.trim() == "go" {
        Ok(())
    } else {
        Err("install cancelled".into())
    }
}

async fn wait_for_pid_exit(pid: u32) {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if !pid_alive(pid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    unsafe {
        let r = libc::kill(pid as libc::pid_t, 0);
        r == 0
    }
}

#[cfg(windows)]
fn pid_alive(pid: u32) -> bool {
    let _ = pid;
    std::thread::sleep(Duration::from_millis(500));
    false
}

#[cfg(not(any(unix, windows)))]
fn pid_alive(_pid: u32) -> bool {
    std::thread::sleep(Duration::from_millis(500));
    false
}

fn swap_executable(staged: &PathBuf, target: &PathBuf) -> Result<(), String> {
    // The staging folder is under the user's data dir and the install
    // may be on another filesystem, where rename fails with EXDEV. Copy
    // next to the target first so the final step is still a rename
    // within one filesystem, and so a half-written executable is never
    // what the user is left with.
    let staged = match std::fs::rename(staged, target.with_extension("oxdm-new")) {
        Ok(()) => target.with_extension("oxdm-new"),
        Err(_) => {
            let beside = target.with_extension("oxdm-new");
            std::fs::copy(staged, &beside)
                .map_err(|e| format!("staging next to the install failed: {e}"))?;
            beside
        }
    };
    let staged = &staged;
    let mut last_err = None;
    for _ in 0..40 {
        match std::fs::rename(staged, target) {
            Ok(()) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(meta) = std::fs::metadata(target) {
                        let mut perm = meta.permissions();
                        perm.set_mode(0o755);
                        let _ = std::fs::set_permissions(target, perm);
                    }
                }
                return Ok(());
            }
            Err(e) => {
                last_err = Some(e.to_string());
                std::thread::sleep(Duration::from_millis(150));
            }
        }
    }
    Err(format!(
        "swap failed: {}",
        last_err.unwrap_or_else(|| "unknown".into())
    ))
}

fn spawn_detached(exe: &PathBuf) -> io::Result<()> {
    use std::process::{Command, Stdio};
    let mut cmd = Command::new(exe);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    cmd.spawn().map(|_| ())
}

#[derive(Debug, serde::Serialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
enum Event {
    Started,
    Verified,
    Ready,
    Installing,
    Done,
    Error { message: String },
}

fn emit(ev: &Event) {
    let line = serde_json::to_string(ev).unwrap_or_else(|_| "{}".into());
    let mut out = io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(&mut s, "{b:02x}");
    }
    s
}

fn ct_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.as_bytes().iter().zip(b.as_bytes().iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

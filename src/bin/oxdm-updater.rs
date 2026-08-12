//! oxdm self-update worker (artifact mode).
//!
//! Spawned by the running oxdm GUI **after** it has already fetched the
//! update artifact through the regular download pipeline. The helper's
//! sole job is the dangerous half: wait for the parent to exit,
//! atomically replace the installed programs, and relaunch the app.
//!
//! An installed build is three programs, not one. `oxdm-native-host` is
//! what the browser launches, and this helper is what will perform the
//! *next* update — leaving either behind means a machine running one
//! version of the app and older copies of the two programs it depends
//! on. `--payload` names a directory of replacements; each is placed
//! beside the app, and the one matching the app itself takes the path
//! given by `--exe`, whatever the user has named it. An AppImage is
//! one file and updates through `--artifact` instead.
//!
//! Every update replaces this helper too, so the daemon and the helper
//! are always the same build talking to each other: the flags below can
//! change with the code that passes them, and nothing has to be kept
//! around for an older caller.
//!
//! Replacing a program that is *running* — this helper, or a native
//! host the browser still has open — cannot be done by writing over it
//! on Windows: the target has to be unlinked, and a file backing a
//! loaded image cannot be. It *can* be renamed. Windows draws the line
//! between the two, allowing a running executable to move and refusing
//! to let it disappear, which is what every self-updater on the
//! platform is built on.
//!
//! So when the direct swap is refused, the old program is renamed
//! aside and the new one takes the name. On Windows that is the normal
//! path for this helper and for a native host in use, not an
//! exception; the app itself is already gone by then and swaps
//! directly. The displaced file cannot be deleted while it is still
//! running, so it is left for the next launch to sweep up.
//!
//! It does not hash the artifact. The digest the feed published is
//! attached to the download as an ordinary checksum, so the download
//! manager verifies it the same way it verifies anything else — and a
//! mismatch fails the download, which is reported as a failed update
//! rather than reaching this helper at all. Re-hashing here would only
//! re-answer a question already answered, and only for the sliver of
//! time between the two checks: the artifact sits in a 0700 directory
//! under the user's own data dir, so anything able to swap it there
//! could replace the executable outright.
//!
//! ## Protocol
//!
//! Stdout: one JSON message per line, matching `UpdaterEvent` in the
//! data layer. Stages: `started → ready → (await stdin "go") →
//! installing → done` (or `error`).
//!
//! Stdin: a single `go\n` from the parent greenlights the swap.
//!
//! ## CLI
//!
//! ```text
//! oxdm-updater --exe <PATH> --pid <PID> --payload <DIR>
//! oxdm-updater --exe <PATH> --pid <PID> --artifact <PATH>   # AppImage
//! ```

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

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
    /// What to install: a directory of programs, or the single file an
    /// AppImage build replaces itself with.
    source: Source,
}

enum Source {
    Payload(PathBuf),
    Artifact(PathBuf),
}

fn parse_args() -> Result<Args, String> {
    let mut argv = std::env::args().skip(1);
    let mut exe: Option<PathBuf> = None;
    let mut pid: Option<u32> = None;
    let mut artifact: Option<PathBuf> = None;
    let mut payload: Option<PathBuf> = None;
    while let Some(flag) = argv.next() {
        let val = argv
            .next()
            .ok_or_else(|| format!("missing value for {flag}"))?;
        match flag.as_str() {
            "--exe" => exe = Some(PathBuf::from(val)),
            "--pid" => pid = Some(val.parse().map_err(|_| "invalid pid".to_string())?),
            "--artifact" => artifact = Some(PathBuf::from(val)),
            "--payload" => payload = Some(PathBuf::from(val)),
            other => return Err(format!("unknown flag: {other}")),
        }
    }
    let source = match (payload, artifact) {
        (Some(dir), _) => Source::Payload(dir),
        (None, Some(file)) => Source::Artifact(file),
        (None, None) => return Err("missing --payload or --artifact".into()),
    };
    Ok(Args {
        exe: exe.ok_or_else(|| "missing --exe".to_string())?,
        pid: pid.ok_or_else(|| "missing --pid".to_string())?,
        source,
    })
}

async fn run(args: Args) -> Result<(), String> {
    emit(&Event::Started);

    // 1. Ready — wait for the parent to confirm + exit. The artifact
    // arrived through the download manager, which checked it against
    // the digest the feed published before it ever got here.
    emit(&Event::Ready);
    wait_for_go().await?;

    emit(&Event::Installing);
    wait_for_pid_exit(args.pid).await;

    // 2. Swap. Each program is moved into place atomically; on Windows
    // a brief retry tides over handle release.
    match &args.source {
        Source::Artifact(file) => swap_executable(file, &args.exe)?,
        Source::Payload(dir) => install_payload(dir, &args.exe)?,
    }

    // 3. Relaunch from the same path. Detach so this updater can exit.
    spawn_detached(&args.exe).map_err(|e| format!("relaunch: {e}"))?;

    emit(&Event::Done);
    Ok(())
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

/// Put every program in `dir` beside the app.
///
/// The entry named like the app itself goes to `exe` — a user who
/// renamed the binary keeps their name — and the rest go to their own
/// names in the same directory. A failure on any one of them stops the
/// install: a half-updated set is the state this whole exercise exists
/// to avoid.
fn install_payload(dir: &PathBuf, exe: &Path) -> Result<(), String> {
    let home = exe
        .parent()
        .ok_or_else(|| format!("{} has no directory to install into", exe.display()))?;
    let entries = std::fs::read_dir(dir).map_err(|e| format!("update payload: {e}"))?;
    let mut installed = 0usize;
    for entry in entries.flatten() {
        let from = entry.path();
        if !from.is_file() {
            continue;
        }
        let name = entry.file_name();
        let stem = std::path::Path::new(&name)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        // The app is identified by being the app, not by its filename.
        let target = if stem.eq_ignore_ascii_case("oxdm") {
            exe.to_path_buf()
        } else {
            home.join(&name)
        };
        swap_executable(&from, &target)?;
        installed += 1;
    }
    if installed == 0 {
        return Err("the update payload is empty".into());
    }
    Ok(())
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
    // Still refused: on Windows a running program cannot be written
    // over, because replacing it means unlinking it and a file backing
    // a loaded image will not unlink. Renaming one is allowed, which
    // is the whole trick.
    let displaced = target.with_extension("oxdm-old");
    let _ = std::fs::remove_file(&displaced);
    if std::fs::rename(target, &displaced).is_ok() {
        match std::fs::rename(staged, target) {
            Ok(()) => {
                // Fails while the displaced copy is still running; the
                // next launch sweeps it up.
                let _ = std::fs::remove_file(&displaced);
                return Ok(());
            }
            Err(e) => {
                // Put back what was there rather than leave a hole
                // where a program used to be.
                let _ = std::fs::rename(&displaced, target);
                last_err = Some(e.to_string());
            }
        }
    }
    Err(format!(
        "could not replace {}: {}",
        target.display(),
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

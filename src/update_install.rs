//! Installing an update: the half that cannot run inside the app.
//!
//! Replacing a program means unlinking the file behind it, and Windows
//! will not unlink a file backing a loaded image — so `oxdm.exe` cannot
//! be the process that replaces `oxdm.exe`. Something else has to
//! outlive it, do the swap, and start the new build.
//!
//! That something is oxdm itself, copied elsewhere and re-run as
//! `oxdm --install-update`. A copy running from a temp directory is
//! not the installed file, so every installed program is free to be
//! replaced, and there is no second executable to ship, install,
//! uninstall, or keep in step with the app it updates.
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
//! oxdm --install-update --exe <PATH> --pid <PID> --payload <DIR>
//! oxdm --install-update --exe <PATH> --pid <PID> --artifact <PATH>
//! ```
//!
//! The artifact form is for an AppImage, which is one file holding
//! everything; the payload form is a directory of programs.
//!
//! Nothing here hashes the artifact. The download manager checked it
//! against the digest the feed published before any of this ran.
//!
//! Replacing a program that is *running* — a native host the browser
//! still has open — is the case the rename dance covers: the old file
//! is moved aside, the new one takes the name, and the displaced copy
//! is swept up at the next launch because it cannot be deleted while
//! it runs.

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Entry point for `oxdm --install-update`. Never returns.
pub fn main(argv: impl Iterator<Item = String>) -> ! {
    let args = match parse_args(argv) {
        Ok(a) => a,
        Err(e) => {
            emit(&Event::Error { message: e });
            std::process::exit(2);
        }
    };
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            emit(&Event::Error {
                message: format!("runtime: {e}"),
            });
            std::process::exit(1);
        }
    };
    match rt.block_on(run(args)) {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            emit(&Event::Error { message: e });
            std::process::exit(1);
        }
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

fn parse_args(argv: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut argv = argv;
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

/// Windows has no `kill(pid, 0)`; opening the process for a query is
/// the equivalent, and a handle that cannot be opened — or a process
/// whose exit code is no longer `STILL_ACTIVE` — means it is gone.
///
/// Answering "not running" without asking, as this used to, meant the
/// updater started replacing binaries while the old app might still
/// have them open, and then relaunched into a single-instance guard
/// the old process had not released yet.
#[cfg(windows)]
fn pid_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: a failed open returns null, which is checked before use;
    // the handle is closed on every path out.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code: u32 = 0;
        let ok = GetExitCodeProcess(handle, &mut code) != 0;
        CloseHandle(handle);
        ok && code == STILL_ACTIVE as u32
    }
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

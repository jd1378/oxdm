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
    /// Run as an administrator to do the replacing, and stop there.
    /// Relaunching oxdm is the unprivileged half's job: an app started
    /// from here would run as root, own every file it touched, and be
    /// a worse problem than the one being solved.
    swap_only: bool,
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
    let mut swap_only = false;
    while let Some(flag) = argv.next() {
        // The one flag that takes no value: it says "you are the
        // elevated half, do the swap and nothing else".
        if flag == "--swap-only" {
            swap_only = true;
            continue;
        }
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
        // An elevated run is handed the swap alone; there is no parent
        // left to wait for by then.
        pid: pid.unwrap_or(0),
        source,
        swap_only,
    })
}

async fn run(args: Args) -> Result<(), String> {
    // The elevated half: no handshake, no waiting, no relaunch. It was
    // started by the unprivileged half, which is still there and will
    // do the rest.
    if args.swap_only {
        return swap(&args);
    }
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
    if let Err(e) = swap(&args) {
        // A system-wide install belongs to root, and this process does
        // not. Ask, once, through the desktop's own prompt.
        let elevated = if needs_rights(&e) {
            emit(&Event::Elevating);
            elevated_swap(&args)
        } else {
            Err(e.clone())
        };
        if let Err(why) = elevated {
            // oxdm has already exited, which is what let its own file
            // be replaced, so failing here leaves the user with no app
            // at all and this process's output goes nowhere they will
            // look. Put the old one back on screen and leave the
            // reason on disk beside the staged update.
            let reason = if why == e {
                e
            } else {
                format!("{e}. Installing as an administrator did not work either: {why}")
            };
            record_failure(&args.exe, &reason);
            let _ = spawn_detached(&args.exe);
            return Err(reason);
        }
    }

    // 3. The launcher icon, which lives in the user's icon theme rather
    // than beside the programs. Deliberately after the swap and outside
    // it: it is not part of "the install either happened or it did
    // not", it is the user's own directory even when the programs are
    // root's, and it must never be the reason an update reports failure.
    if let Source::Payload(dir) = &args.source {
        refresh_icon(dir);
    }

    // 4. Relaunch from the same path. Detach so this updater can exit.
    spawn_detached(&args.exe).map_err(|e| format!("relaunch: {e}"))?;

    emit(&Event::Done);
    Ok(())
}

fn swap(args: &Args) -> Result<(), String> {
    match &args.source {
        Source::Artifact(file) => swap_executable(file, &args.exe),
        Source::Payload(dir) => install_payload(dir, &args.exe),
    }
}

/// Is this the kind of failure administrator rights would fix?
///
/// Matched on the message rather than on an error kind because the two
/// swap paths fold several IO calls into one string; a false positive
/// costs one prompt the user can dismiss, a false negative costs them
/// the update.
fn needs_rights(reason: &str) -> bool {
    let reason = reason.to_ascii_lowercase();
    reason.contains("permission denied")
        || reason.contains("access is denied")
        || reason.contains("operation not permitted")
        || reason.contains("read-only file system")
        || reason.contains("os error 13")
        || reason.contains("os error 1")
}

/// Do the same swap again, as an administrator.
///
/// The elevated process is this same program with `--swap-only`, so
/// there is one implementation of "replace these files" and the
/// privileged half does nothing else: it never relaunches the app,
/// never touches the user's data directory, and exits as soon as the
/// files are in place.
fn elevated_swap(args: &Args) -> Result<(), String> {
    if !crate::platform::elevate::available() {
        return Err("this system has no way to ask for administrator rights".into());
    }
    let me = std::env::current_exe().map_err(|e| format!("cannot find the installer: {e}"))?;
    let argv = elevated_args(args);
    let borrowed: Vec<&str> = argv.iter().map(String::as_str).collect();
    crate::platform::elevate::run(&me, &borrowed)
}

/// What the privileged half is asked to do. Built here so it can be
/// read without a password prompt: `--swap-only` and no `--pid`, so it
/// waits for nothing and relaunches nothing.
fn elevated_args(args: &Args) -> Vec<String> {
    let (flag, value) = match &args.source {
        Source::Artifact(file) => ("--artifact", file.display().to_string()),
        Source::Payload(dir) => ("--payload", dir.display().to_string()),
    };
    vec![
        "--install-update".to_owned(),
        "--swap-only".to_owned(),
        "--exe".to_owned(),
        args.exe.display().to_string(),
        flag.to_owned(),
        value,
    ]
}

/// Leave the reason somewhere a person can find it.
///
/// The parent that would have shown this in a window is gone by the
/// time an install can fail, and stderr goes to a pipe nobody is
/// reading any more. The next launch does not read this file either —
/// it is for the user and for a bug report.
fn record_failure(exe: &Path, reason: &str) {
    let Some(dir) = dirs::data_dir().map(|d| d.join("oxdm")) else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let body = format!(
        "oxdm could not install an update.\n\n\
         target: {}\n\
         reason: {reason}\n\n\
         The previous version has been left in place and restarted.\n",
        exe.display(),
    );
    let _ = std::fs::write(dir.join("update-failed.txt"), body);
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
        // The payload carries more than programs. `oxdm.png` has the
        // same file stem as the app, so anything less exact than this
        // would install a picture over the executable.
        if !crate::data::update_bundle::is_ours(&name.to_string_lossy()) {
            continue;
        }
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

/// Replace the launcher icon the installer put in the user's icon
/// theme, if there is one.
///
/// The archive carries `oxdm.png` and `tools/install.sh` copies it to
/// `~/.local/share/icons/hicolor/512x512/apps/oxdm.png`, where the
/// `.desktop` entry's `Icon=oxdm` resolves it. Nothing was refreshing
/// that afterwards, so a release that changed the icon reached new
/// installs only.
///
/// Only ever *overwrites*: an install with no icon there is an
/// AppImage, a portable copy, or someone who set `OXDM_NO_DESKTOP`, and
/// an updater is not the place to start creating desktop integration
/// the user declined. Every failure is silent for the same reason the
/// call site is outside the swap — a stale picture is not a failed
/// update.
#[cfg(target_os = "linux")]
fn refresh_icon(payload: &Path) {
    refresh_icon_in(payload, &icon_theme_dirs())
}

/// The body of [`refresh_icon`], with the icon theme roots handed in.
///
/// Split so a test can point it at a temporary directory by argument.
/// Doing it by environment variable would mutate process-global state
/// that every other test in the binary shares, and the failure mode of
/// getting it wrong is overwriting the icon in the developer's own
/// `~/.local/share/icons`.
#[cfg(target_os = "linux")]
fn refresh_icon_in(payload: &Path, themes: &[PathBuf]) {
    let staged = payload.join(crate::data::update_bundle::ICON);
    if !staged.is_file() {
        return;
    }
    for theme in themes {
        let target = theme.join("512x512/apps").join("oxdm.png");
        if !target.is_file() {
            continue;
        }
        // Beside the target so the final step is a rename within one
        // filesystem, and so a half-copied PNG is never what a launcher
        // reads.
        let tmp = target.with_extension("oxdm-new");
        if std::fs::copy(&staged, &tmp).is_err() {
            continue;
        }
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = std::fs::metadata(&tmp) {
                let mut perm = meta.permissions();
                perm.set_mode(0o644);
                let _ = std::fs::set_permissions(&tmp, perm);
            }
        }
        if std::fs::rename(&tmp, &target).is_err() {
            let _ = std::fs::remove_file(&tmp);
            continue;
        }
        // Desktops read the icon through a cache that keeps serving the
        // old picture until it is told otherwise. Absent on plenty of
        // systems, which is fine: without a cache there is nothing
        // stale to invalidate.
        let _ = std::process::Command::new("gtk-update-icon-cache")
            .args(["-q", "-t", "-f"])
            .arg(theme)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

/// Where an installed icon could be.
///
/// `install.sh` writes to `$HOME/.local/share` literally while the spec
/// says `$XDG_DATA_HOME`, and the two differ on a machine that sets the
/// variable. Both are checked rather than one guessed at: the caller
/// only touches a file that already exists, so the extra candidate
/// costs a `stat` and covers the mismatch either way round.
#[cfg(target_os = "linux")]
fn icon_theme_dirs() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let candidates = [
        dirs::data_dir(),
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")),
    ];
    for base in candidates.into_iter().flatten() {
        let theme = base.join("icons/hicolor");
        if !out.contains(&theme) {
            out.push(theme);
        }
    }
    out
}

#[cfg(not(target_os = "linux"))]
fn refresh_icon(_payload: &Path) {}

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
    // Nothing of this updater's descriptor table travels into the app
    // it is starting. Whatever this process inherited from the daemon
    // that spawned it would otherwise be held open by the very process
    // that has to start cleanly.
    crate::platform::attach_close_high_fds(&mut cmd);
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
    /// The install needs rights this process does not have, and the
    /// system's own prompt is now in front of the user.
    Elevating,
    Done,
    Error {
        message: String,
    },
}

fn emit(ev: &Event) {
    let line = serde_json::to_string(ev).unwrap_or_else(|_| "{}".into());
    let mut out = io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only failures administrator rights would actually fix are worth
    /// a prompt. A wrong guess either way costs the user something: a
    /// pointless password dialog, or an update that stops with a fixable
    /// error nobody was asked about.
    #[test]
    fn only_permission_failures_ask_for_rights() {
        for yes in [
            "staging next to the install failed: Permission denied (os error 13)",
            "install /usr/local/bin/oxdm: Access is denied. (os error 5)",
            "rename: Operation not permitted",
            "write: Read-only file system",
        ] {
            assert!(needs_rights(yes), "{yes}");
        }
        for no in [
            "update payload: No such file or directory",
            "the update archive holds none of oxdm's programs",
            "relaunch: Exec format error",
        ] {
            assert!(!needs_rights(no), "{no}");
        }
    }

    /// An install missing the browser host gets one. A user who copied
    /// only the app out of the archive, or who installed before the
    /// host existed, should not be left without browser integration
    /// forever: the payload carries both programs, and each one is put
    /// where it belongs whether or not something is already there.
    #[cfg(unix)]
    #[test]
    fn a_missing_program_is_added_rather_than_skipped() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let install = dir.path().join("bin");
        let payload = dir.path().join("payload");
        std::fs::create_dir_all(&install).unwrap();
        std::fs::create_dir_all(&payload).unwrap();

        let exe = install.join("oxdm");
        std::fs::write(&exe, b"old app").unwrap();
        std::fs::write(payload.join("oxdm"), b"new app").unwrap();
        std::fs::write(payload.join("oxdm-native-host"), b"new host").unwrap();
        assert!(!install.join("oxdm-native-host").exists());

        install_payload(&payload, &exe).unwrap();

        assert_eq!(std::fs::read(&exe).unwrap(), b"new app");
        let host = install.join("oxdm-native-host");
        assert_eq!(std::fs::read(&host).unwrap(), b"new host");
        // And it has to be runnable, or the browser cannot launch it.
        let mode = std::fs::metadata(&host).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "{mode:o}");
    }

    /// A binary the user renamed keeps its name: the app is identified
    /// by being the app, not by what it is called.
    #[cfg(unix)]
    #[test]
    fn a_renamed_app_is_replaced_under_its_own_name() {
        let dir = tempfile::tempdir().unwrap();
        let install = dir.path().join("bin");
        let payload = dir.path().join("payload");
        std::fs::create_dir_all(&install).unwrap();
        std::fs::create_dir_all(&payload).unwrap();

        let exe = install.join("oxdm-nightly");
        std::fs::write(&exe, b"old app").unwrap();
        std::fs::write(payload.join("oxdm"), b"new app").unwrap();
        std::fs::write(payload.join("oxdm-native-host"), b"new host").unwrap();

        install_payload(&payload, &exe).unwrap();

        assert_eq!(std::fs::read(&exe).unwrap(), b"new app");
        assert!(!install.join("oxdm").exists(), "no second copy appeared");
        assert!(install.join("oxdm-native-host").exists());
    }

    /// The payload carries the launcher icon, and `oxdm.png` shares a
    /// file stem with the app. Installing payload entries by stem alone
    /// would put a picture where the executable goes, which is the
    /// worst outcome this whole file exists to avoid.
    #[cfg(unix)]
    #[test]
    fn the_icon_is_never_installed_as_a_program() {
        let dir = tempfile::tempdir().unwrap();
        let install = dir.path().join("bin");
        let payload = dir.path().join("payload");
        std::fs::create_dir_all(&install).unwrap();
        std::fs::create_dir_all(&payload).unwrap();

        let exe = install.join("oxdm");
        std::fs::write(&exe, b"old app").unwrap();
        std::fs::write(payload.join("oxdm"), b"new app").unwrap();
        std::fs::write(payload.join("oxdm-native-host"), b"new host").unwrap();
        std::fs::write(payload.join("oxdm.png"), b"PNG").unwrap();

        install_payload(&payload, &exe).unwrap();

        assert_eq!(std::fs::read(&exe).unwrap(), b"new app", "still the app");
        assert!(
            !install.join("oxdm.png").exists(),
            "an icon has no business beside the binaries"
        );
    }

    /// An install whose icon the user can see gets the one this version
    /// ships. Overwritten in place, so a launcher never reads a
    /// half-copied file.
    #[cfg(target_os = "linux")]
    #[test]
    fn an_installed_icon_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let payload = dir.path().join("payload");
        std::fs::create_dir_all(&payload).unwrap();
        std::fs::write(payload.join("oxdm.png"), b"new icon").unwrap();

        let theme = dir.path().join("icons/hicolor");
        let apps = theme.join("512x512/apps");
        std::fs::create_dir_all(&apps).unwrap();
        let icon = apps.join("oxdm.png");
        std::fs::write(&icon, b"old icon").unwrap();

        refresh_icon_in(&payload, &[theme]);

        assert_eq!(std::fs::read(&icon).unwrap(), b"new icon");
        assert!(
            !apps.join("oxdm.oxdm-new").exists(),
            "and nothing left staged"
        );
    }

    /// No icon installed means no desktop integration to refresh. An
    /// updater that created one would be adding a launcher entry to an
    /// AppImage, a portable copy, or an install that passed
    /// `OXDM_NO_DESKTOP` precisely to avoid it.
    #[cfg(target_os = "linux")]
    #[test]
    fn an_install_without_an_icon_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let payload = dir.path().join("payload");
        std::fs::create_dir_all(&payload).unwrap();
        std::fs::write(payload.join("oxdm.png"), b"new icon").unwrap();
        let theme = dir.path().join("icons/hicolor");

        refresh_icon_in(&payload, std::slice::from_ref(&theme));

        assert!(
            !theme.join("512x512/apps/oxdm.png").exists(),
            "nothing was there, so nothing was made"
        );
        assert!(!theme.exists(), "not even the directory");
    }

    /// The privileged half replaces files and does nothing else. No
    /// pid to wait for, and no relaunch, because an oxdm started from
    /// there would run as root and own every file it touched.
    #[test]
    fn the_elevated_half_is_asked_only_to_swap() {
        let args = Args {
            exe: PathBuf::from("/usr/local/bin/oxdm"),
            pid: 4321,
            source: Source::Payload(PathBuf::from("/tmp/staged")),
            swap_only: false,
        };
        let argv = elevated_args(&args);
        assert_eq!(
            argv,
            vec![
                "--install-update",
                "--swap-only",
                "--exe",
                "/usr/local/bin/oxdm",
                "--payload",
                "/tmp/staged",
            ]
        );
        assert!(!argv.iter().any(|a| a == "--pid"));

        // And it round-trips: what is sent is what the other side
        // parses back.
        let parsed = parse_args(argv.into_iter().skip(1)).unwrap();
        assert!(parsed.swap_only);
        assert_eq!(parsed.pid, 0);
        assert_eq!(parsed.exe, PathBuf::from("/usr/local/bin/oxdm"));
    }
}

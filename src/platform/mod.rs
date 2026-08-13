//! Platform helpers: opening a path or a URL, desktop entries,
//! notifications, and the inotify-watch ceiling on Linux.

pub mod watch_limit;

pub fn open_path(path: &std::path::Path) {
    #[cfg(target_os = "linux")]
    let prog = "xdg-open";
    #[cfg(target_os = "macos")]
    let prog = "open";
    #[cfg(target_os = "windows")]
    let prog = "explorer";
    if let Err(e) = std::process::Command::new(prog).arg(path).spawn() {
        tracing::warn!(path = %path.display(), error = %e, "failed to open path");
    }
}

/// Platform-native label for the [`reveal_in_folder`] action, so menu
/// items / buttons read naturally per OS instead of the macOS-only
/// "Reveal in Finder" everywhere.
pub fn reveal_label() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "Reveal in Finder"
    }
    #[cfg(target_os = "windows")]
    {
        "Show in Explorer"
    }
    #[cfg(target_os = "linux")]
    {
        "Open Containing Folder"
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        "Open Containing Folder"
    }
}

/// Reveal `path` in the system file manager. On Windows + macOS the
/// file manager highlights the file itself; on Linux there is no
/// portable "select" verb so we open the parent directory instead.
pub fn reveal_in_folder(path: &std::path::Path) {
    #[cfg(target_os = "windows")]
    {
        let arg = format!("/select,{}", path.display());
        if let Err(e) = std::process::Command::new("explorer").arg(arg).spawn() {
            tracing::warn!(path = %path.display(), error = %e, "explorer reveal failed");
        }
        return;
    }
    #[cfg(target_os = "macos")]
    {
        if let Err(e) = std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn()
        {
            tracing::warn!(path = %path.display(), error = %e, "finder reveal failed");
        }
        return;
    }
    #[cfg(target_os = "linux")]
    {
        // Try the freedesktop FileManager1 D-Bus interface first — every
        // major Linux file manager (Nautilus, Dolphin, Nemo, Caja,
        // Thunar) implements it and it's the only portable way to
        // highlight a specific item. Fall back to opening the parent
        // directory if the call fails (no FM running, no D-Bus, etc.).
        let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let uri = format!("file://{}", abs.display());
        // GVariant string-literal escaping: backslash + double-quote.
        let escaped = uri.replace('\\', "\\\\").replace('"', "\\\"");
        let dbus_ok = std::process::Command::new("gdbus")
            .args([
                "call",
                "--session",
                "--dest",
                "org.freedesktop.FileManager1",
                "--object-path",
                "/org/freedesktop/FileManager1",
                "--method",
                "org.freedesktop.FileManager1.ShowItems",
                &format!("[\"{}\"]", escaped),
                "",
            ])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if dbus_ok {
            return;
        }
        let target = path.parent().unwrap_or(path);
        if let Err(e) = std::process::Command::new("xdg-open").arg(target).spawn() {
            tracing::warn!(path = %target.display(), error = %e, "xdg-open reveal failed");
        }
    }
}

pub fn open_url(url: &str) {
    #[cfg(target_os = "linux")]
    let cmd_args: (&str, &[&str]) = ("xdg-open", &[url]);
    #[cfg(target_os = "macos")]
    let cmd_args: (&str, &[&str]) = ("open", &[url]);
    #[cfg(target_os = "windows")]
    let cmd_args: (&str, &[&str]) = ("cmd", &["/C", "start", "", url]);

    if let Err(e) = std::process::Command::new(cmd_args.0)
        .args(cmd_args.1)
        .spawn()
    {
        tracing::warn!(url = %url, error = %e, "failed to open url");
    }
}

#[cfg(target_os = "linux")]
pub fn install_desktop_entry() -> Result<std::path::PathBuf, String> {
    use std::io::Write;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe_str = exe.to_string_lossy();
    let dir = dirs::data_dir()
        .ok_or_else(|| "no data dir".to_string())?
        .join("applications");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("oxdm.desktop");
    let body = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=oxdm\n\
         Comment=Cross-platform download manager\n\
         Exec={} %U\n\
         Terminal=false\n\
         Categories=Network;FileTransfer;\n\
         StartupNotify=true\n",
        exe_str
    );
    let mut f = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    f.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
    // Best-effort xdg refresh; ignore failures.
    let _ = std::process::Command::new("update-desktop-database")
        .arg(&dir)
        .spawn();
    Ok(path)
}

#[cfg(not(target_os = "linux"))]
#[allow(dead_code)]
pub fn install_desktop_entry() -> Result<std::path::PathBuf, String> {
    Err("Create Desktop Entry is Linux-only".into())
}

/// Write (or remove) the XDG autostart entry for `exe` under `dir`.
/// Split out from [`set_autostart`] so the entry contents are testable
/// without touching the real `~/.config/autostart`.
#[cfg(target_os = "linux")]
fn write_xdg_autostart(
    dir: &std::path::Path,
    exe: &std::path::Path,
    enabled: bool,
) -> Result<(), String> {
    use std::io::Write;
    let path = dir.join("oxdm.desktop");
    if !enabled {
        let _ = std::fs::remove_file(&path);
        return Ok(());
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let body = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=oxdm\n\
         Comment=Cross-platform download manager\n\
         Exec={}\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n\
         Categories=Network;FileTransfer;\n",
        exe.to_string_lossy()
    );
    let mut f = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    f.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
    Ok(())
}

/// Install or remove a system autostart entry for oxdm.
///
/// - Linux: writes `~/.config/autostart/oxdm.desktop` (XDG autostart).
/// - macOS: writes `~/Library/LaunchAgents/com.oxdm.app.plist`.
/// - Windows: writes / clears
///   `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\oxdm`.
///
/// The entry launches oxdm with no arguments so the separate
/// "start to tray" setting stays in charge of whether the main window
/// opens; hard-coding `--tray` here would silently override it.
pub fn set_autostart(enabled: bool) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let dir = dirs::config_dir()
            .ok_or_else(|| "no config dir".to_string())?
            .join("autostart");
        write_xdg_autostart(&dir, &exe, enabled)
    }
    #[cfg(target_os = "macos")]
    {
        use std::io::Write;
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let dir = dirs::home_dir()
            .ok_or_else(|| "no home dir".to_string())?
            .join("Library/LaunchAgents");
        let path = dir.join("com.oxdm.app.plist");
        if !enabled {
            let _ = std::process::Command::new("launchctl")
                .args(["unload", path.to_string_lossy().as_ref()])
                .status();
            let _ = std::fs::remove_file(&path);
            return Ok(());
        }
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let body = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>com.oxdm.app</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
    </array>
    <key>RunAtLoad</key><true/>
</dict>
</plist>
"#,
            exe.to_string_lossy()
        );
        let mut f = std::fs::File::create(&path).map_err(|e| e.to_string())?;
        f.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
        let _ = std::process::Command::new("launchctl")
            .args(["load", path.to_string_lossy().as_ref()])
            .status();
        Ok(())
    }
    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        let key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
        if !enabled {
            let _ = Command::new("reg")
                .args(["delete", key, "/v", "oxdm", "/f"])
                .status();
            return Ok(());
        }
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let value = format!("\"{}\"", exe.to_string_lossy());
        let status = Command::new("reg")
            .args(["add", key, "/v", "oxdm", "/t", "REG_SZ", "/d", &value, "/f"])
            .status()
            .map_err(|e| e.to_string())?;
        if !status.success() {
            return Err(format!("reg add returned {status}"));
        }
        Ok(())
    }
}

/// The running executable's path, usable for re-spawning ourselves.
///
/// Plain [`std::env::current_exe`] is not: it reads `/proc/self/exe`,
/// and once the binary has been replaced on disk — an in-place update,
/// or a rebuild while the daemon is running — the kernel reports the
/// original path with a literal `" (deleted)"` appended. Spawning that
/// path fails with `ENOENT`, so a long-running daemon loses the ability
/// to open any window until it is restarted.
///
/// When that happens the replacement usually sits at the original path,
/// so fall back to the suffix-stripped path — but only when the
/// suffixed one is really gone and the stripped one is really there,
/// so a file genuinely named `"… (deleted)"` is left alone.
pub fn current_exe() -> std::io::Result<std::path::PathBuf> {
    Ok(undelete_exe_path(std::env::current_exe()?))
}

/// The path half of [`current_exe`], split out so it is testable without
/// replacing the test binary on disk.
fn undelete_exe_path(exe: std::path::PathBuf) -> std::path::PathBuf {
    const DELETED: &str = " (deleted)";
    if exe.exists() {
        return exe;
    }
    exe.file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_suffix(DELETED))
        .map(|n| exe.with_file_name(n))
        .filter(|live| live.exists())
        .unwrap_or(exe)
}

/// Highest descriptor number the child bothers to inspect. Descriptors
/// are handed out lowest-free-first, so a process holding a few dozen
/// files never reaches this; it only bounds the scan when `RLIMIT_NOFILE`
/// is enormous (or unlimited), where walking the real ceiling would mean
/// millions of `fcntl` calls between `fork` and `exec`.
#[cfg(unix)]
const MAX_FD_SCAN: libc::c_int = 4096;

/// `pre_exec` hook closing inherited fds ≥ 3 in spawned subprocesses
/// (daemon / GUI windows) so sockets don't leak across exec on Unix.
/// No-op on non-Unix platforms — Windows handles use explicit
/// `bInheritHandle = FALSE` by default.
///
/// Descriptors already marked `FD_CLOEXEC` are left alone. The kernel
/// drops those at `exec` anyway, and one of them is the pipe `std` keeps
/// open to report a failed `exec` back to the parent. Closing that pipe
/// (which a blanket `close_range(3, ..)` does) breaks spawning twice
/// over: the parent reads EOF, concludes the exec succeeded and returns
/// `Ok(child)` for a window that will never appear, and the child's
/// attempt to write its errno into the closed fd trips
/// `fatal runtime error: assertion failed: output.write(&bytes).is_ok()`
/// and aborts. What actually needs dropping is the opposite set — the
/// descriptors *without* `FD_CLOEXEC`, such as the single-instance
/// socket, whose binding an inheriting child would pin alive.
///
/// The closure runs in the forked child, so it may only make syscalls:
/// no allocation, no locks. `std` always takes the `fork` + `exec` path
/// once any `pre_exec` hook is registered, so the child's descriptor
/// table is already a private copy and needs no `CLOSE_RANGE_UNSHARE`.
#[allow(unused_variables)]
pub fn attach_close_high_fds(cmd: &mut std::process::Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                let mut lim: libc::rlimit = std::mem::zeroed();
                let max = if libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) == 0 {
                    (lim.rlim_cur.min(MAX_FD_SCAN as libc::rlim_t)) as libc::c_int
                } else {
                    MAX_FD_SCAN
                };
                for fd in 3..max {
                    let flags = libc::fcntl(fd, libc::F_GETFD);
                    if flags >= 0 && flags & libc::FD_CLOEXEC == 0 {
                        libc::close(fd);
                    }
                }
                Ok(())
            });
        }
    }
}

/// Show a desktop notification from a plain thread.
///
/// notify-rust's sync `show()` drives zbus with an internal
/// `block_on` that panics on tokio runtime threads ("Cannot start a
/// runtime from within a runtime"); every daemon call site runs on
/// the runtime, so the blocking dance is isolated here.
pub fn show_notification(summary: String, body: String) {
    std::thread::spawn(move || {
        let mut n = notify_rust::Notification::new();
        n.summary(&summary).body(&body).appname("oxdm");
        #[cfg(target_os = "windows")]
        {
            n.app_id("oxdm");
        }
        if let Err(e) = n.show() {
            tracing::debug!(error = %e, "notification failed (no daemon?)");
        }
    });
}

/// A notification the user can press, running `on_press` when they do.
///
/// Freedesktop notifications carry named actions and report which one
/// was chosen, so `label` becomes a button and the body itself is
/// clickable. Elsewhere this degrades to a plain notification: the
/// action is a shortcut to something the user can still reach from the
/// tray, so an unclickable report is a smaller loss than no report.
///
/// The waiting thread lives until the notification is dismissed or
/// pressed, which is why this is a thread and not a task.
#[cfg(target_os = "linux")]
pub fn show_notification_with_action(
    summary: String,
    body: String,
    label: String,
    on_press: impl FnOnce() + Send + 'static,
) {
    std::thread::spawn(move || {
        let mut n = notify_rust::Notification::new();
        n.summary(&summary)
            .body(&body)
            .appname("oxdm")
            // "default" is the action a press on the notification body
            // itself invokes; the named one draws a button for desktops
            // that show them.
            .action("default", &label)
            .action("show", &label);
        match n.show() {
            Ok(handle) => handle.wait_for_action(|action| {
                if action == "default" || action == "show" {
                    on_press();
                }
            }),
            Err(e) => tracing::debug!(error = %e, "notification failed (no daemon?)"),
        }
    });
}

#[cfg(not(target_os = "linux"))]
pub fn show_notification_with_action(
    summary: String,
    body: String,
    _label: String,
    _on_press: impl FnOnce() + Send + 'static,
) {
    show_notification(summary, body);
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn autostart_entry_is_written_then_removed() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("autostart");
        let exe = std::path::Path::new("/opt/oxdm/oxdm");

        write_xdg_autostart(&dir, exe, true).unwrap();
        let body = std::fs::read_to_string(dir.join("oxdm.desktop")).unwrap();
        assert!(body.contains("Exec=/opt/oxdm/oxdm\n"));
        // The entry must not force tray mode — `start_to_tray` owns that.
        assert!(!body.contains("--tray"));

        write_xdg_autostart(&dir, exe, false).unwrap();
        assert!(!dir.join("oxdm.desktop").exists());
    }

    /// A binary replaced on disk (in-place update, or a rebuild while
    /// the daemon runs) makes `/proc/self/exe` report `"… (deleted)"`,
    /// which no longer spawns. Callers re-spawning oxdm need the live
    /// path that took its place.
    #[test]
    fn undelete_exe_path_recovers_a_replaced_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let live = tmp.path().join("oxdm");
        std::fs::write(&live, b"").unwrap();

        assert_eq!(undelete_exe_path(tmp.path().join("oxdm (deleted)")), live);
        // An existing path is returned untouched, suffix or not.
        assert_eq!(undelete_exe_path(live.clone()), live);
        let odd = tmp.path().join("real (deleted)");
        std::fs::write(&odd, b"").unwrap();
        assert_eq!(undelete_exe_path(odd.clone()), odd);
        // Nothing to fall back to: hand the original back so the caller
        // reports the real spawn error rather than a silent wrong path.
        let gone = tmp.path().join("absent (deleted)");
        assert_eq!(undelete_exe_path(gone.clone()), gone);
    }

    /// A blanket `close_range(3, ..)` in the `pre_exec` hook also closed
    /// the pipe `std` reports a failed `exec` on, so the parent read EOF,
    /// reported `Ok(child)` for a process that never started, and the
    /// child aborted with `fatal runtime error: assertion failed:
    /// output.write(&bytes).is_ok()`. A failed spawn must surface as
    /// `Err` so the caller can log it and fall back.
    #[test]
    fn close_high_fds_keeps_the_exec_error_pipe_open() {
        let mut cmd = std::process::Command::new("/nonexistent/oxdm-spawn-probe");
        attach_close_high_fds(&mut cmd);
        let err = cmd
            .spawn()
            .expect_err("spawning a missing binary must fail");
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn disabling_autostart_when_absent_is_ok() {
        let tmp = tempfile::tempdir().unwrap();
        write_xdg_autostart(
            &tmp.path().join("nope"),
            std::path::Path::new("/opt/oxdm/oxdm"),
            false,
        )
        .unwrap();
    }
}

//! Platform helpers: opening a path or a URL, desktop entries,
//! notifications, and the inotify-watch ceiling on Linux.

pub mod elevate;
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
    let exe = launch_target()?;
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
        desktop_exec_arg(&exe)
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

/// Is this process running from an AppImage, and if so where is the
/// bundle?
///
/// The AppImage runtime exports `APPIMAGE` with the bundle's own path.
/// It matters wherever we record a path to run later: [`current_exe`]
/// points *inside* the bundle's mount (`/tmp/.mount_oxdmXXXX/usr/bin/
/// oxdm`), which is unmounted the moment the app exits.
pub fn bundle_path() -> Option<std::path::PathBuf> {
    std::env::var_os("APPIMAGE")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
}

/// The path an autostart entry, launcher or Run key should name.
///
/// The bundle when there is one, so the entry survives the mount going
/// away; otherwise the running binary, by way of [`current_exe`] so a
/// binary replaced under a running daemon does not get recorded with
/// `" (deleted)"` glued to its name.
fn launch_target() -> Result<std::path::PathBuf, String> {
    if let Some(bundle) = bundle_path() {
        return Ok(bundle);
    }
    current_exe().map_err(|e| e.to_string())
}

/// A path as the program token of a desktop entry's `Exec=`.
///
/// The value is parsed with shell-like quoting, so an unquoted path
/// holding a space is read as several arguments and the entry silently
/// launches nothing. Two layers of escaping stack here: the argument is
/// double-quoted with `"`, `` ` ``, `$` and `\` backslash-escaped inside
/// (Desktop Entry Spec, "Exec variables"), and then every backslash is
/// doubled because the file format unescapes `\\` to `\` before the
/// value is ever split (same spec, "String values").
#[cfg(target_os = "linux")]
fn desktop_exec_arg(path: &std::path::Path) -> String {
    let mut quoted = String::from("\"");
    for c in path.to_string_lossy().chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    quoted.replace('\\', "\\\\")
}

/// A path as XML character data, for the launch agent's plist. `&` and
/// `<` are legal in a macOS filename and would otherwise make the plist
/// unparseable, at which point `launchctl` refuses the whole agent.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn xml_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
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
        desktop_exec_arg(exe)
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
        let exe = launch_target()?;
        let dir = dirs::config_dir()
            .ok_or_else(|| "no config dir".to_string())?
            .join("autostart");
        write_xdg_autostart(&dir, &exe, enabled)
    }
    #[cfg(target_os = "macos")]
    {
        use std::io::Write;
        let exe = launch_target()?;
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
            xml_text(&exe.to_string_lossy())
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
        let exe = launch_target()?;
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

/// Rewrite the autostart entry when it no longer names this binary.
///
/// The entry records an absolute path, and the path can stop being
/// right without the setting ever being touched: oxdm was moved, or
/// reinstalled elsewhere, or an AppImage was renamed. The setting then
/// reads "on" while nothing starts at login. Same reasoning as the
/// browser manifests in [`crate::ipc::manifest_check`], and the same
/// deliberate limit: an entry the user edited by hand keeps whatever
/// arguments it was given, because only the program path is compared.
///
/// Never fails loudly. A login entry that could not be repaired is
/// worth a log line, not a startup error.
pub fn refresh_autostart(enabled: bool) {
    if !enabled {
        return;
    }
    let target = match launch_target() {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(error = %e, "autostart: cannot resolve this binary's path");
            return;
        }
    };
    if autostart_names(&target) {
        return;
    }
    match set_autostart(true) {
        Ok(()) => tracing::info!(target = %target.display(), "autostart entry repointed"),
        Err(e) => tracing::warn!(error = %e, "autostart entry could not be repointed"),
    }
}

/// Does the installed autostart entry launch `target`?
fn autostart_names(target: &std::path::Path) -> bool {
    #[cfg(target_os = "linux")]
    {
        let Some(dir) = dirs::config_dir() else {
            return true; // nowhere to look: leave it alone
        };
        let Ok(body) = std::fs::read_to_string(dir.join("autostart").join("oxdm.desktop")) else {
            return false;
        };
        body.contains(&desktop_exec_arg(target))
    }
    #[cfg(target_os = "macos")]
    {
        let Some(home) = dirs::home_dir() else {
            return true;
        };
        let path = home.join("Library/LaunchAgents/com.oxdm.app.plist");
        let Ok(body) = std::fs::read_to_string(path) else {
            return false;
        };
        body.contains(&format!(
            "<string>{}</string>",
            xml_text(&target.to_string_lossy())
        ))
    }
    #[cfg(target_os = "windows")]
    {
        let out = std::process::Command::new("reg")
            .args([
                r"query",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                "oxdm",
            ])
            .output();
        match out {
            Ok(out) if out.status.success() => {
                String::from_utf8_lossy(&out.stdout).contains(target.to_string_lossy().as_ref())
            }
            // No value, or `reg` itself unavailable: writing one back is
            // the safe move either way.
            _ => false,
        }
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
/// Hand this process's right to come to the front over to whoever the
/// daemon is about to start.
///
/// Windows only lets the *foreground* process decide what may take the
/// foreground next. A window opened from a click here is opened by the
/// daemon, which is a background process with no such right to give,
/// so its `AllowSetForegroundWindow` for the child fails and the new
/// window lands behind this one, blinking in the taskbar.
///
/// Called from the window the user just clicked in, which is the
/// foreground process at that moment and is therefore allowed to say
/// "the next window to ask may have it". The permission covers a
/// single activation and expires by itself.
///
/// Nothing to do elsewhere: X11 and Wayland let the new window raise
/// itself, and on macOS the window asks for focus when it opens.
pub fn allow_foreground_handoff() {
    #[cfg(windows)]
    {
        // ASFW_ANY: any process may take the foreground next. The
        // alternative needs the child's pid, which does not exist yet
        // when this is called.
        const ASFW_ANY: u32 = u32::MAX;
        // SAFETY: FFI call with no preconditions. It fails when this
        // process is not in the foreground, which is exactly when the
        // permission would not have been ours to give.
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(ASFW_ANY);
        }
    }
}

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
        assert!(body.contains("Exec=\"/opt/oxdm/oxdm\"\n"));
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

    /// Nothing says a user keeps oxdm in a path without spaces. An
    /// unquoted `Exec=` splits on them, so the session launches
    /// `/home/me/My` at login and the setting quietly does nothing.
    #[test]
    fn a_path_with_spaces_still_launches() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("autostart");
        let exe = std::path::Path::new("/home/me/My Apps/oxdm");

        write_xdg_autostart(&dir, exe, true).unwrap();
        let body = std::fs::read_to_string(dir.join("oxdm.desktop")).unwrap();
        assert!(
            body.contains("Exec=\"/home/me/My Apps/oxdm\"\n"),
            "got: {body}"
        );
    }

    /// The characters the two escaping layers disagree about: `$` and a
    /// backslash mean something to the argument splitter, and the
    /// backslash means something to the file format on top of that.
    #[test]
    fn exec_escapes_what_the_shell_would_eat() {
        assert_eq!(
            desktop_exec_arg(std::path::Path::new("/opt/$HOME/oxdm")),
            "\"/opt/\\\\$HOME/oxdm\""
        );
        // One literal backslash: escaped for the splitter (\\), then
        // both of those doubled for the file format.
        assert_eq!(
            desktop_exec_arg(std::path::Path::new("/opt/a\\b/oxdm")),
            "\"/opt/a\\\\\\\\b/oxdm\""
        );
        assert_eq!(
            desktop_exec_arg(std::path::Path::new("/opt/plain/oxdm")),
            "\"/opt/plain/oxdm\""
        );
    }

    /// A launch agent whose plist does not parse is refused whole, so
    /// an `&` in the path would cost the user the setting.
    #[test]
    fn the_launch_agent_path_is_xml_escaped() {
        assert_eq!(
            xml_text("/Users/me/Apps & Tools/oxdm"),
            "/Users/me/Apps &amp; Tools/oxdm"
        );
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

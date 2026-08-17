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
        // The shell's own verb first. `explorer /select,` spawns a
        // fresh window every time, even when the folder is already
        // open, and leaves it behind whichever window had focus;
        // `SHOpenFolderAndSelectItems` reuses a window already showing
        // the folder, selects the item in it, and brings it forward.
        if let Err(e) = shell_reveal(path) {
            tracing::debug!(path = %path.display(), error = %e, "shell reveal failed; falling back");
            // Quoting is the whole reason this is `raw_arg`. Rust
            // quotes an argument containing spaces as a unit, so a path
            // with a space became `"/select,C:\a b\f.zip"`, which
            // explorer does not parse: it ignored the switch and opened
            // the default folder with nothing selected. The quotes have
            // to sit around the path alone.
            use std::os::windows::process::CommandExt;
            let arg = format!("/select,\"{}\"", path.display());
            if let Err(e) = std::process::Command::new("explorer").raw_arg(arg).spawn() {
                tracing::warn!(path = %path.display(), error = %e, "explorer reveal failed");
            }
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

/// Select `path` in Explorer through the shell, rather than by asking
/// `explorer.exe` to do it on a command line.
///
/// Passing the file's own item id as the *folder* with no selection
/// list is the documented way to say "show me this one thing": the
/// shell opens its parent, reuses a window already displaying that
/// folder instead of adding another, selects the item, and activates
/// the window.
///
/// COM is initialised per call and never uninitialised. This runs on
/// whichever thread the UI happens to call it from, and that thread
/// keeps running afterwards; tearing the apartment down under it would
/// break any other shell call it makes later. A second init on an
/// already-initialised thread returns `RPC_E_CHANGED_MODE`, which is
/// not fatal here and is why the result is ignored.
#[cfg(target_os = "windows")]
fn shell_reveal(path: &std::path::Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

    use windows::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, IBindCtx,
    };
    use windows::Win32::UI::Shell::{SHOpenFolderAndSelectItems, SHParseDisplayName};
    use windows::core::PCWSTR;

    // Explorer will not select something it cannot address. A relative
    // path resolves against Explorer's idea of the current directory,
    // not ours.
    let full = path
        .canonicalize()
        .map_err(|e| format!("resolve {}: {e}", path.display()))?;
    // `canonicalize` hands back a `\\?\` extended-length path, which
    // the shell namespace does not parse.
    let display = full.to_string_lossy();
    let plain = display.strip_prefix(r"\\?\").unwrap_or(&display);
    let wide: Vec<u16> = std::ffi::OsStr::new(plain)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let mut pidl = std::ptr::null_mut();
        // Spelled out rather than a bare `None`: the parameter is
        // generic over anything convertible to an `IBindCtx`, so the
        // literal alone gives inference nothing to work with.
        SHParseDisplayName(
            PCWSTR(wide.as_ptr()),
            Option::<&IBindCtx>::None,
            &mut pidl,
            0,
            None,
        )
        .map_err(|e| format!("parse {plain}: {e}"))?;
        let opened = SHOpenFolderAndSelectItems(pidl, None, 0);
        CoTaskMemFree(Some(pidl as *const std::ffi::c_void));
        opened.map_err(|e| e.to_string())
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

/// The icon name every entry oxdm writes carries, and the Wayland
/// `app_id` / X11 `WM_CLASS` its windows are given (see
/// `gui::chrome::window_settings`). One string for all three on
/// purpose: a launcher resolves the picture by icon name, and a
/// taskbar finds the same entry by matching `app_id` against the
/// entry's own basename, so `oxdm.desktop` naming `Icon=oxdm` is what
/// makes both work. `tools/install.sh` writes the matching
/// `~/.local/share/icons/hicolor/512x512/apps/oxdm.png`.
#[cfg(target_os = "linux")]
const DESKTOP_ICON: &str = "oxdm";

/// Where the launcher looks for `Icon=oxdm`.
///
/// The size directory is the one `tools/install.sh` writes and
/// `update_install::refresh_icon` replaces, and 512 is the size of the
/// asset itself — the theme scales down from there.
#[cfg(target_os = "linux")]
const ICON_REL_DIR: &str = "icons/hicolor/512x512/apps";

/// What writing a desktop entry ended up doing.
///
/// The icon is reported separately because the entry is still worth
/// having without it: a launcher with the stock download glyph is a
/// launcher, and telling the user it all worked when half of it did not
/// is how a bug report becomes "it says it installed".
#[derive(Debug, Clone)]
pub struct DesktopEntry {
    pub path: std::path::PathBuf,
    pub icon_installed: bool,
}

/// Write the launcher icon `Icon=oxdm` resolves to.
///
/// The same PNG the release archive carries, at the same path
/// `tools/install.sh` uses, so repairing from inside the app and
/// installing from the script leave the machine in one state rather
/// than two that disagree.
#[cfg(target_os = "linux")]
fn install_launcher_icon() -> Result<std::path::PathBuf, String> {
    use std::io::Write;
    let dir = dirs::data_dir()
        .ok_or_else(|| "no data dir".to_string())?
        .join(ICON_REL_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("oxdm.png");
    // Beside the target and renamed into place, so a launcher reading
    // the directory mid-write never gets half a PNG.
    let tmp = path.with_extension("oxdm-new");
    let write = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(crate::gui::app_icon::LAUNCHER_PNG)?;
        f.sync_all()?;
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perm = f.metadata()?.permissions();
            perm.set_mode(0o644);
            f.set_permissions(perm)?;
        }
        Ok(())
    })();
    if let Err(e) = write {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.to_string());
    }
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })?;
    Ok(path)
}

/// The user's own launcher entry, if there is one.
///
/// Only their own: an entry under `/usr/share/applications` belongs to
/// whatever package manager put it there, and overwriting one from in
/// here is not oxdm's business.
#[cfg(target_os = "linux")]
pub fn desktop_entry_path() -> Option<std::path::PathBuf> {
    let path = dirs::data_dir()?.join("applications/oxdm.desktop");
    path.is_file().then_some(path)
}

#[cfg(not(target_os = "linux"))]
pub fn desktop_entry_path() -> Option<std::path::PathBuf> {
    None
}

/// Tell the desktop that `apps` and the icon theme changed.
///
/// Every one of these is best-effort and none is required: the files
/// are on disk either way, and a desktop that keeps no cache — or that
/// notices the write itself, as Plasma and GNOME do for the
/// applications directory — needs none of them.
///
/// What they cover:
/// - `update-desktop-database` rebuilds `mimeinfo.cache`.
/// - `kbuildsycoca6`/`5` rebuild KDE's service database.
/// - `gtk-update-icon-cache` rebuilds GTK's `icon-theme.cache`, which
///   is the one that can genuinely hide a newly written icon: readers
///   compare it against the theme root's mtime, not the size directory
///   the PNG landed in. The tool ships with GTK, so a KDE-only machine
///   may not have it — and does not need it, since nothing there reads
///   that cache.
///
/// Nothing here can fail in a way the caller has to handle: a missing
/// tool, one that exits non-zero, and one that hangs are all the same
/// outcome, which is that a cache somewhere is stale.
/// How long the reaper waits on all of the cache tools together before
/// killing what is left.
///
/// These finish in well under a second on a normal machine; the budget
/// is for the one that never finishes. Nothing the user can see is lost
/// by cutting one short — the entry and its icon are already written,
/// and the tool only rebuilds a cache that gets rebuilt again on the
/// desktop's own schedule.
#[cfg(target_os = "linux")]
const CACHE_TOOL_BUDGET: std::time::Duration = std::time::Duration::from_secs(5);

#[cfg(target_os = "linux")]
fn refresh_desktop_caches(apps: &std::path::Path) {
    use std::ffi::OsStr;

    let mut running: Vec<std::process::Child> = Vec::new();
    running.extend(run_detached("update-desktop-database", &[apps.as_os_str()]));
    // 6 before 5, and only one of them: a machine with `kbuildsycoca6`
    // is on KDE 6, where running the 5 build would rebuild a database
    // nothing reads.
    for kde in ["kbuildsycoca6", "kbuildsycoca5"] {
        if let Some(child) = run_detached(kde, &[OsStr::new("--noincremental")]) {
            running.push(child);
            break;
        }
    }
    if let Some(theme) = dirs::data_dir().map(|d| d.join("icons/hicolor")) {
        running.extend(run_detached(
            "gtk-update-icon-cache",
            &[
                OsStr::new("-q"),
                OsStr::new("-t"),
                OsStr::new("-f"),
                theme.as_os_str(),
            ],
        ));
    }

    if running.is_empty() {
        return;
    }
    // Reaped off-thread, on a deadline. Waiting inline would hand a
    // tool that blocks — `kbuildsycoca` with no KDE session to talk to,
    // say — the power to freeze the window that called this, and
    // dropping a `Child` unwaited leaves a zombie for as long as the
    // process lives. The budget covers all of them together: past it,
    // whatever is still running is not going to finish usefully, so it
    // is killed and reaped rather than left behind.
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + CACHE_TOOL_BUDGET;
        for mut child in running {
            loop {
                match child.try_wait() {
                    // Exited, or is no longer ours to wait for. Either
                    // way there is nothing left to reap.
                    Ok(Some(_)) | Err(_) => break,
                    Ok(None) if std::time::Instant::now() >= deadline => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                    Ok(None) => std::thread::sleep(std::time::Duration::from_millis(100)),
                }
            }
        }
    });
}

/// Start `prog` with its output discarded, or report why it did not
/// start.
///
/// `None` covers both "not installed" and "would not run": these are
/// conveniences, and the files they describe are already written.
#[cfg(target_os = "linux")]
fn run_detached(prog: &str, args: &[&std::ffi::OsStr]) -> Option<std::process::Child> {
    use std::process::Stdio;
    match std::process::Command::new(prog)
        .args(args)
        // Inherited by default, which would print a tool's chatter into
        // whatever terminal the app was started from.
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => Some(child),
        Err(e) => {
            tracing::debug!(prog, error = %e, "desktop cache tool not started");
            None
        }
    }
}

/// Write (or overwrite) the launcher entry and the icon it names.
///
/// Overwriting on purpose: this is the repair for an entry that is
/// missing, or one left by an older install pointing at a binary that
/// has since moved. `Exec=` is taken from the running executable, so
/// the entry always names the copy the user actually launched.
#[cfg(target_os = "linux")]
pub fn install_desktop_entry() -> Result<DesktopEntry, String> {
    use std::io::Write;
    let exe = launch_target()?;
    let dir = dirs::data_dir()
        .ok_or_else(|| "no data dir".to_string())?
        .join("applications");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("oxdm.desktop");

    // An icon that could not be written must not leave the entry
    // naming one: `Icon=` pointing at nothing shows as a blank slot on
    // some desktops, which is worse than the theme's own download
    // glyph. Same fallback the install script makes.
    let icon = install_launcher_icon()
        .map_err(|e| tracing::warn!(error = %e, "install launcher icon"))
        .ok();
    let icon_name = if icon.is_some() {
        DESKTOP_ICON
    } else {
        "folder-download"
    };

    let body = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=oxdm\n\
         Comment=Cross-platform download manager\n\
         Exec={} %U\n\
         Icon={icon_name}\n\
         Terminal=false\n\
         Categories=Network;FileTransfer;\n\
         StartupNotify=true\n\
         StartupWMClass={DESKTOP_ICON}\n",
        desktop_exec_arg(&exe)
    );
    let mut f = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    f.write_all(body.as_bytes()).map_err(|e| e.to_string())?;

    refresh_desktop_caches(&dir);

    Ok(DesktopEntry {
        path,
        icon_installed: icon.is_some(),
    })
}

#[cfg(not(target_os = "linux"))]
#[allow(dead_code)]
pub fn install_desktop_entry() -> Result<DesktopEntry, String> {
    Err("a desktop entry is a Linux thing".into())
}

/// The path an autostart entry, launcher or Run key should name.
///
/// The running binary, by way of [`current_exe`] so a binary replaced
/// under a running daemon does not get recorded with `" (deleted)"`
/// glued to its name.
fn launch_target() -> Result<std::path::PathBuf, String> {
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
         Icon={DESKTOP_ICON}\n\
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
/// reinstalled elsewhere, or renamed. The setting then
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

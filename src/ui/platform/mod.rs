//! Platform helpers: menu actions (open URL, install desktop entry)
//! and per-OS window integration (e.g. Windows borderless resize).

#[cfg(target_os = "windows")]
pub mod windows;

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

/// Install or remove a system autostart entry for oxdm.
///
/// - Linux: writes `~/.config/autostart/oxdm.desktop` (XDG autostart).
/// - macOS: writes `~/Library/LaunchAgents/com.oxdm.app.plist`.
/// - Windows: writes / clears
///   `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\oxdm`.
pub fn set_autostart(enabled: bool) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        use std::io::Write;
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let dir = dirs::config_dir()
            .ok_or_else(|| "no config dir".to_string())?
            .join("autostart");
        let path = dir.join("oxdm.desktop");
        if !enabled {
            let _ = std::fs::remove_file(&path);
            return Ok(());
        }
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let body = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=oxdm\n\
             Comment=Cross-platform download manager\n\
             Exec={} --tray\n\
             Terminal=false\n\
             X-GNOME-Autostart-enabled=true\n\
             Categories=Network;FileTransfer;\n",
            exe.to_string_lossy()
        );
        let mut f = std::fs::File::create(&path).map_err(|e| e.to_string())?;
        f.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
        Ok(())
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
        <string>--tray</string>
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
        let value = format!("\"{}\" --tray", exe.to_string_lossy());
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

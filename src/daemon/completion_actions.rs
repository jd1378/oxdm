//! On-completion action runner.
//!
//! Subscribes `DomainEvent::JobCompleted` and applies each job's
//! `OnCompletion` preferences. The "show dialog" flag suppresses every
//! other automatic action — same UX as IDM.
//!
//! Also raises the per-job window when a download the user started by
//! hand *fails*: that gesture was aimed at one download, and the window
//! is the only place showing the error and offering a retry. Automated
//! runs (a queue, Resume all, the scheduler, a capture) stay silent —
//! a batch can fail many jobs, and a stack of windows buries the
//! queue-finished summary that already reports them.

use std::process::Command;
use std::sync::Arc;

use crate::data::{AppState, DomainEvent};
use crate::domain::ShutdownAction;

pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut rx = state.subscribe();
        while let Ok(ev) = rx.recv().await {
            if let DomainEvent::JobFailed { id, error } = &ev {
                // A conflict parks the job pending an answer and has its
                // own dialog; only real failures raise this one.
                let conflict = matches!(error, crate::domain::JobError::ConflictPending(_));
                // Only a hand-started run gets a window; automation
                // reports its failures elsewhere.
                let manual = state.is_manual_run(*id).await;
                if !conflict && manual && state.settings().await.show_failed_dialog {
                    crate::daemon::tray::spawn_download_gui(*id);
                }
                continue;
            }
            if let DomainEvent::JobCompleted { id, .. } = ev {
                let Some(entry) = state.job_entry(id).await else {
                    continue;
                };
                let prefs = match entry.on_completion.read() {
                    Ok(g) => g.clone(),
                    Err(_) => continue,
                };
                // Honour IDM's "show dialog wins" rule. Per-job opt-in
                // *and* the global "Show download-complete dialog"
                // setting must both be on; either off skips the dialog
                // and falls through to unattended actions below.
                let show_global = state.settings().await.show_complete_dialog;
                if prefs.show_dialog && show_global {
                    crate::daemon::tray::spawn_download_gui(id);
                    continue;
                }
                // Destructive power actions go through the shared
                // grace timer (feature #9) instead of firing
                // immediately — the GUI shows a cancellable countdown.
                let mut power_armed = false;
                if let Some(action) = prefs.shutdown {
                    let force = prefs.force_shutdown;
                    power_armed = state.arm_power_action(action.into(), move || {
                        run_shutdown(action, force).map_err(|e| e.to_string())
                    });
                }
                // A power action already takes the link down; arming
                // disconnect too would only lose the race for the
                // one-slot guard and drop the shutdown countdown.
                if prefs.disconnect {
                    if power_armed || state.pending_shutdown().is_some() {
                        tracing::info!(
                            "skipping disconnect completion action: a power action is pending"
                        );
                    } else {
                        state.arm_power_action(crate::domain::PowerAction::Disconnect, || {
                            run_disconnect().map_err(|e| e.to_string())
                        });
                    }
                }
                if prefs.exit_app {
                    if power_armed || state.pending_shutdown().is_some() {
                        // Exiting now would kill the daemon-side grace
                        // task and silently drop the promised power
                        // action; the action takes the whole system
                        // down anyway.
                        tracing::warn!(
                            "skipping exit-app completion action while a power action is pending"
                        );
                        continue;
                    }
                    // Give pending notifications + IO a beat.
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    std::process::exit(0);
                }
            }
        }
    });
}

#[cfg(target_os = "linux")]
fn run_shutdown(action: ShutdownAction, _force: bool) -> std::io::Result<()> {
    // systemd-friendly. Falls back through PolicyKit; will fail if the
    // user lacks permission — log + continue.
    match action {
        ShutdownAction::ShutDown => Command::new("systemctl").arg("poweroff").status(),
        ShutdownAction::Restart => Command::new("systemctl").arg("reboot").status(),
        ShutdownAction::Sleep => Command::new("systemctl").arg("suspend").status(),
    }
    .map(|_| ())
}

#[cfg(target_os = "macos")]
fn run_shutdown(action: ShutdownAction, _force: bool) -> std::io::Result<()> {
    let cmd = match action {
        ShutdownAction::ShutDown => "tell app \"System Events\" to shut down",
        ShutdownAction::Restart => "tell app \"System Events\" to restart",
        ShutdownAction::Sleep => "tell app \"System Events\" to sleep",
    };
    Command::new("osascript")
        .arg("-e")
        .arg(cmd)
        .status()
        .map(|_| ())
}

#[cfg(target_os = "windows")]
fn run_shutdown(action: ShutdownAction, force: bool) -> std::io::Result<()> {
    let mut cmd = Command::new("shutdown");
    match action {
        ShutdownAction::ShutDown => cmd.arg("/s"),
        ShutdownAction::Restart => cmd.arg("/r"),
        ShutdownAction::Sleep => cmd.arg("/h"),
    };
    cmd.arg("/t").arg("0");
    if force {
        cmd.arg("/f");
    }
    cmd.status().map(|_| ())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn run_shutdown(_action: ShutdownAction, _force: bool) -> std::io::Result<()> {
    Err(std::io::Error::other("unsupported platform"))
}

/// Take the machine's network down. Each platform's command needs
/// rights the daemon may not have (PolicyKit on Linux, an elevated
/// process on Windows); a refusal surfaces as a non-zero exit that we
/// turn into an error, so the GUI reports it instead of silently
/// pretending the link went down.
#[cfg(target_os = "linux")]
fn run_disconnect() -> std::io::Result<()> {
    // NetworkManager is the only cross-distro handle we can rely on;
    // `networking off` deactivates every managed device and, unlike
    // `nmcli radio all off`, also covers wired links.
    check(Command::new("nmcli").args(["networking", "off"]).status()?)
}

#[cfg(target_os = "macos")]
fn run_disconnect() -> std::io::Result<()> {
    // "airport" is networksetup's alias for every Wi-Fi device, so this
    // does not hard-code en0. Wired links are left alone: disabling a
    // service by name would need the localized service name.
    check(
        Command::new("networksetup")
            .args(["-setairportpower", "airport", "off"])
            .status()?,
    )
}

#[cfg(target_os = "windows")]
fn run_disconnect() -> std::io::Result<()> {
    check(
        Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Disable-NetAdapter -Name * -Confirm:$false",
            ])
            .status()?,
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn run_disconnect() -> std::io::Result<()> {
    Err(std::io::Error::other("unsupported platform"))
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn check(status: std::process::ExitStatus) -> std::io::Result<()> {
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "disconnect command failed ({status})"
        )))
    }
}

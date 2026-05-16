//! On-completion action runner.
//!
//! Subscribes `DomainEvent::JobCompleted` and applies each job's
//! `OnCompletion` preferences. The "show dialog" flag suppresses every
//! other automatic action — same UX as IDM.

use std::process::Command;
use std::sync::Arc;

use crate::data::{AppState, DomainEvent};
use crate::domain::ShutdownAction;

pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut rx = state.subscribe();
        while let Ok(ev) = rx.recv().await {
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
                if let Some(action) = prefs.shutdown
                    && let Err(e) = run_shutdown(action, prefs.force_terminate)
                {
                    tracing::warn!(error = %e, "shutdown command failed");
                }
                if prefs.exit_app {
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

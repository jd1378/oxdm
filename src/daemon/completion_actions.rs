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
use crate::domain::{Settings, ShutdownAction};

/// Does this stopped download get a window?
///
/// Only a run the user started by hand. A queue run is work they set
/// going and walked away from: a window over whatever they are doing
/// now is an interruption they did not ask for, and a batch that fails
/// would be a stack of them. Conflicts included — the row says "needs
/// your answer" and the queue's finish summary reports it.
fn wants_window(manual: bool, conflict: bool, s: &Settings) -> bool {
    manual
        && if conflict {
            s.show_conflict_dialog
        } else {
            s.show_failed_dialog
        }
}

pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut rx = state.subscribe();
        while let Some(ev) = crate::data::next_event(&mut rx, "completion actions").await {
            if let DomainEvent::JobFailed { id, error } = &ev {
                let conflict = matches!(error, crate::domain::JobError::ConflictPending(_));
                // Only a hand-started run gets a window; automation
                // reports its failures elsewhere.
                let manual = state.is_manual_run(*id).await;
                // Surfacing means evict-and-respawn (focusing is
                // unreliable across window managers), which would tear
                // down a window the user is already reading. If it is
                // focused it is already surfaced — it refreshes itself
                // off the same event.
                let already_watching = crate::ipc_local::server::is_focused(
                    crate::ipc_local::protocol::GuiKind::Download(*id),
                );
                // Nothing a queue started raises a window, conflict
                // included. A queue run is work the user set going and
                // walked away from; a window in front of what they are
                // doing now is an interruption they did not ask for,
                // and a batch of failures would be a stack of them. The
                // row still says what happened, and the queue's own
                // finish summary reports it.
                let settings = state.settings().await;
                if wants_window(manual, conflict, &settings) && !already_watching {
                    crate::daemon::tray::spawn_download_gui(*id);
                }
                continue;
            }
            if let DomainEvent::JobCompleted { id, ref path, .. } = ev {
                // The self-update artifact is a download like any
                // other until it lands; from here it is the executable
                // oxdm is about to become, so it goes to the helper
                // that checks its digest rather than to the folder-
                // opening, notification-raising path below.
                if state.pending_update().await.is_some_and(|p| p.job == id) {
                    state.stage_update(path.clone()).await;
                    continue;
                }
                let Some(entry) = state.job_entry(id).await else {
                    continue;
                };
                let prefs = match entry.on_completion.read() {
                    Ok(g) => g.clone(),
                    Err(_) => continue,
                };
                // Same rule as a failure: only a download the user
                // started by hand gets a window. The rest of the
                // completion actions below still run — a queue whose
                // job asks for shutdown means it.
                let manual = state.is_manual_run(id).await;
                // The per-job toggle is the answer. It starts out as a
                // copy of the global "Show download-complete dialog"
                // setting, so leaving it alone follows the global; a
                // user who changed it for this download meant it, and
                // ANDing the global back in would silently ignore them.
                if manual && prefs.show_dialog {
                    crate::daemon::tray::spawn_download_gui(id);
                    continue;
                }
                // Destructive power actions go through the shared
                // grace timer (feature #9) instead of firing
                // immediately — the GUI shows a cancellable countdown.
                let mut power_armed = false;
                if let Some(action) = prefs.shutdown {
                    let force = prefs.force_shutdown;
                    // Both, in the order they make sense in: the link
                    // goes down first, then the machine. They ride one
                    // armed action rather than two, so there is still
                    // one countdown and one thing to cancel — as two
                    // they would race for the guard's single slot and
                    // the loser would be dropped.
                    let disconnect_first = prefs.disconnect;
                    power_armed = state.arm_power_action(action.into(), move || {
                        if disconnect_first && let Err(e) = run_disconnect() {
                            // The machine still goes down: the user
                            // asked for that, and a link that would not
                            // drop is not a reason to keep it running.
                            tracing::warn!(error = %e, "disconnect before power action failed");
                        }
                        run_shutdown(action, force).map_err(|e| e.to_string())
                    });
                }
                // On its own, the disconnect is the armed action.
                if prefs.disconnect && !power_armed {
                    if state.pending_shutdown().is_some() {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(failed: bool, conflict: bool) -> Settings {
        Settings {
            show_failed_dialog: failed,
            show_conflict_dialog: conflict,
            ..Settings::default()
        }
    }

    #[test]
    fn a_queue_run_never_raises_a_window() {
        let loud = settings(true, true);
        assert!(
            !wants_window(false, false, &loud),
            "a queued failure is silent"
        );
        assert!(!wants_window(false, true, &loud), "so is a queued conflict");
    }

    #[test]
    fn a_hand_started_run_follows_the_settings() {
        assert!(wants_window(true, false, &settings(true, true)));
        assert!(wants_window(true, true, &settings(false, true)));
        assert!(!wants_window(true, false, &settings(false, true)));
        assert!(!wants_window(true, true, &settings(true, false)));
    }
}

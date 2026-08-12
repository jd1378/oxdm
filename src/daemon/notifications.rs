//! System-notification bridge.
//!
//! Subscribes to `DomainEvent` and surfaces lifecycle terminals
//! (Completed / Failed / parked Conflict / finished queue) as desktop
//! notifications. Tray + queue UI surface the same info; notifications
//! matter most when the window is hidden.
//!
//! Each event is opt-out from Settings → Notifications. A conflict is
//! not covered: it is a question for the user, not a report, and the
//! download stays parked until it is answered.
//!
//! `notify-rust` is fire-and-forget on Linux/Mac; on Windows it
//! requires an AppUserModelID — we set one matching the binary name.

use std::sync::Arc;

use crate::data::{AppState, DomainEvent};
use crate::domain::Settings;

/// Does this stopped download get a desktop notification?
///
/// The per-download notification settings describe downloads the user
/// started. A queue running twenty of them is one piece of work, and
/// it reports itself through the queue's own finish hook rather than
/// twenty notifications.
fn wants_notification(manual: bool, conflict: bool, s: &Settings) -> bool {
    manual
        && if conflict {
            s.notify_conflict
        } else {
            s.notify_failed
        }
}

pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut rx = state.subscribe();
        while let Some(ev) = crate::data::next_event(&mut rx, "notifications").await {
            // A hidden job is machinery, not a download the user
            // started: the self-update artifact is the only one today,
            // and "Download complete — oxdm-update-0.2.0" is not news
            // anybody asked for. The update flow reports itself.
            if let DomainEvent::JobCompleted { id, .. } | DomainEvent::JobFailed { id, .. } = ev
                && state.is_hidden(id).await
            {
                continue;
            }
            match ev {
                DomainEvent::JobCompleted { id, path, .. } => {
                    // The per-download notification settings describe
                    // downloads the user started. A queue running
                    // twenty of them is one piece of work, and it
                    // reports itself through the queue's own finish
                    // hook rather than twenty desktop notifications.
                    if !state.is_manual_run(id).await {
                        continue;
                    }
                    if !state.settings().await.notify_complete {
                        continue;
                    }
                    let body = format!(
                        "Saved to {}",
                        path.file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.display().to_string())
                    );
                    notify("Download complete", &body);
                }
                DomainEvent::JobFailed { id, error } => {
                    // A conflict has its own pair of toggles: it is not
                    // a failure, and it is the one stopped state that
                    // never resolves itself, so it defaults to loud.
                    let conflict = matches!(error, crate::domain::JobError::ConflictPending(_));
                    let manual = state.is_manual_run(id).await;
                    if !wants_notification(manual, conflict, &state.settings().await) {
                        continue;
                    }
                    // The body is the cause in its own words — the
                    // title already says it is waiting, and repeating
                    // "needs your answer" there costs the one line the
                    // user has to work out *which* question it is.
                    let (title, body) = match error {
                        crate::domain::JobError::ConflictPending(cause) => {
                            ("Download needs your answer", cause.to_string())
                        }
                        other => ("Download failed", other.to_string()),
                    };
                    notify(title, &body);
                }
                _ => {}
            }
        }
    });
}

fn notify(summary: &str, body: &str) {
    // Logged because the delivery itself is fire-and-forget: without
    // this there is no way to tell "the setting suppressed it" from
    // "the desktop dropped it".
    tracing::debug!(summary, "notifying");
    crate::platform::show_notification(summary.to_owned(), body.to_owned());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(failed: bool, conflict: bool) -> Settings {
        Settings {
            notify_failed: failed,
            notify_conflict: conflict,
            ..Settings::default()
        }
    }

    #[test]
    fn a_queue_run_notifies_about_itself_not_its_downloads() {
        let loud = settings(true, true);
        assert!(!wants_notification(false, false, &loud));
        assert!(!wants_notification(false, true, &loud));
    }

    #[test]
    fn a_hand_started_run_follows_the_settings() {
        assert!(wants_notification(true, false, &settings(true, false)));
        assert!(wants_notification(true, true, &settings(false, true)));
        assert!(!wants_notification(true, false, &settings(false, true)));
    }
}

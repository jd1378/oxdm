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

pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut rx = state.subscribe();
        while let Some(ev) = crate::data::next_event(&mut rx, "notifications").await {
            match ev {
                DomainEvent::JobCompleted { path, .. } => {
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
                DomainEvent::JobFailed { error, .. } => {
                    // A conflict has its own pair of toggles: it is not
                    // a failure, and it is the one stopped state that
                    // never resolves itself, so it defaults to loud.
                    let conflict = matches!(error, crate::domain::JobError::ConflictPending(_));
                    let settings = state.settings().await;
                    let wanted = if conflict {
                        settings.notify_conflict
                    } else {
                        settings.notify_failed
                    };
                    if !wanted {
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

//! System-notification bridge.
//!
//! Subscribes to `DomainEvent` and surfaces lifecycle terminals
//! (Completed / Failed / parked Conflict) as desktop notifications.
//! Tray + queue UI surface the same info; notifications matter most
//! when the window is hidden.
//!
//! `notify-rust` is fire-and-forget on Linux/Mac; on Windows it
//! requires an AppUserModelID — we set one matching the binary name.

use std::sync::Arc;

use crate::data::{AppState, DomainEvent};

pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut rx = state.subscribe();
        while let Ok(ev) = rx.recv().await {
            match ev {
                DomainEvent::JobCompleted { path, .. } => {
                    let body = format!(
                        "Saved to {}",
                        path.file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.display().to_string())
                    );
                    notify("Download complete", &body);
                }
                DomainEvent::JobFailed { error, .. } => {
                    let title = match &error {
                        crate::domain::JobError::ConflictPending(_) => "Download paused",
                        _ => "Download failed",
                    };
                    notify(title, &error.to_string());
                }
                _ => {}
            }
        }
    });
}

fn notify(summary: &str, body: &str) {
    let mut n = notify_rust::Notification::new();
    n.summary(summary).body(body).appname("oxdm");
    #[cfg(target_os = "windows")]
    {
        n.app_id("oxdm");
    }
    if let Err(e) = n.show() {
        tracing::debug!(error = %e, "notification failed (no daemon?)");
    }
}

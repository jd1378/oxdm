//! Announcing a new version.
//!
//! One listener for `DomainEvent::UpdateAvailable`, because the two
//! surfaces answer the same question and only one of them may be on:
//! a window that offers the install, or a notification that opens that
//! same window when pressed. Splitting them across the notification and
//! window services would have put one user-facing decision in two
//! files, each unable to see what the other did.
//!
//! The window is About: it already holds the whole update flow (notes,
//! download, verify, install), and a second dialog that could only hand
//! off to it would be a click in the way.

use std::sync::Arc;

use crate::data::{AppState, DomainEvent};
use crate::domain::UpdateSurface;

pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut rx = state.subscribe();
        while let Some(ev) = crate::data::next_event(&mut rx, "update alerts").await {
            let DomainEvent::UpdateAvailable { info } = ev else {
                continue;
            };
            match state.settings().await.update_surface() {
                UpdateSurface::Silent => {}
                UpdateSurface::Dialog => open_about(),
                UpdateSurface::Notification => {
                    crate::platform::show_notification_with_action(
                        "New version available".to_owned(),
                        format!("oxdm {} is ready to download.", info.version),
                        "Show".to_owned(),
                        open_about,
                    );
                }
            }
        }
    });
}

/// Surface About on the update. An already-focused window is left
/// alone: surfacing is evict-and-respawn (focusing is unreliable across
/// window managers), which would tear down the window the user is
/// reading — and it picks the same news up from the event anyway.
fn open_about() {
    if crate::ipc_local::server::is_focused(crate::ipc_local::protocol::GuiKind::About) {
        return;
    }
    crate::daemon::tray::spawn_about_gui();
}

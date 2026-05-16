//! Per-download window state types.
//!
//! Download windows always run as standalone subprocesses
//! (`oxdm gui download <id>`); see `app::download_window`. This
//! module just keeps the small UI-state structs they share with the
//! rest of the codebase, so callers don't have to import the bin-only
//! module path directly.

use crate::domain::OnCompletion;

#[derive(Clone)]
pub struct DownloadState {
    pub tab: Tab,
    pub speed_enabled_draft: Option<bool>,
    pub speed_kbs_draft: String,
    pub remember_speed: bool,
    pub on_completion_draft: Option<OnCompletion>,
    pub show_parts: bool,
    /// Per-job parallel-connection draft. `None` = inherit global.
    /// Capped to 16 by the daemon.
    pub max_conn_draft: String,
    /// Whether we have refreshed the on-demand fields
    /// (`on_completion`, `session_speed_override`) from the daemon
    /// for this dialog open. Cache only carries the lifecycle bits.
    pub fetched_extras: bool,
}

impl Default for DownloadState {
    fn default() -> Self {
        Self {
            tab: Tab::default(),
            speed_enabled_draft: None,
            speed_kbs_draft: String::new(),
            remember_speed: false,
            on_completion_draft: None,
            show_parts: true,
            max_conn_draft: String::new(),
            fetched_extras: false,
        }
    }
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Info,
    Speed,
    OnCompletion,
}

/// Ask the daemon to surface a per-job download window. The daemon
/// either focuses an existing GUI subprocess or spawns a new one;
/// callers do not need to track child PIDs.
pub fn spawn(app: &crate::ui::AppShell, id: crate::domain::JobId) {
    let c = app.client.clone();
    app.spawn(async move {
        let _ = c.open_download_window(id).await;
    });
}

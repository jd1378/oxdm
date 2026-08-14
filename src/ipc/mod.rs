//! Browser-extension bridge.
//!
//! Two transports converge on the same accept path
//! ([`accept_capture`]):
//! - [`ws`] — `ws://127.0.0.1:<port>` for dev / first-party
//!   extensions.
//! - **Native messaging** via the standalone `oxdm-native-host` binary
//!   (`src/bin/oxdm-native-host.rs`). The browser launches the binary
//!   per its native-messaging manifest; the binary opens a fresh WS
//!   session against `127.0.0.1:<port>` and shuttles framed JSON in
//!   both directions. There is no in-process listener here for that
//!   transport — it intentionally lives outside the daemon so its
//!   lifecycle is owned by the browser.
//!
//! Auth: first frame must be `{"token":"…"}` matching the value in
//! `AppState::ext_token`. Mismatches close the socket immediately.

pub mod evaluator;
pub mod manifest_check;
pub mod staged;
mod ws;

use std::sync::Arc;

use crate::data::AppState;
use crate::domain::CaptureRequest;

/// Public entry point. Spawns the WebSocket listener on the configured
/// port and returns when shutdown is requested or the listener errors.
pub async fn serve(state: Arc<AppState>) -> Result<(), IpcError> {
    let settings = state.settings().await;
    ws::run(state.clone(), settings.ipc_port).await
}

/// What a capture turned into. An interactive one has no job id to
/// report: the job does not exist until the user presses a button in
/// the dialog, and may never exist at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureOutcome {
    Added(crate::domain::JobId),
    /// Handed to the Add dialog. Nothing was added, nothing started.
    Staged,
}

/// Centralized accept path called by every transport. Decoupled so
/// unit tests can drive it without a socket.
///
/// Behavior is controlled entirely by `CaptureRequest::interactive`:
///   - `true`  → stage the request and surface the Add-Download dialog
///     (`spawn_add_gui_staged`). The user picks Download now /
///     Add to queue / Cancel, and **only then** is a job created. No
///     global override exists — the per-request flag is authoritative.
///   - `false` → add the job, start it, and surface a per-job
///     download window so the user sees progress.
///
/// The extension chooses per request: context-menu / pinned-button /
/// selection captures all set `interactive: true` so destination URLs
/// are visible before any cookies travel; download-interception sets
/// `interactive: false` because the user already clicked the real
/// download link in the browser, where intent is explicit.
///
/// Interactive used to add the job first and open the dialog on it in
/// *edit* mode, which meant Cancel left a job the user had just
/// declined sitting in the list, and the dialog's queue picker edited a
/// routing decision that had already been made. Staging keeps the whole
/// decision in the dialog: it is the mirror of the batch flow, where
/// `interactive: true` has always staged rather than added.
pub async fn accept_capture(
    state: &Arc<AppState>,
    req: CaptureRequest,
) -> Result<CaptureOutcome, String> {
    // First-line defence: only accept http/https URLs over the wire.
    // The dialog / runner happily consume `url::Url` for many schemes
    // (file://, ftp://, magnet:…) but the extension contract is
    // browser downloads, which are http(s). Reject everything else
    // so the surface stays narrow.
    let scheme = req.url.scheme();
    if !matches!(scheme, "http" | "https") {
        return Err(format!("rejected scheme: {scheme}"));
    }
    let target_queue = resolve_queue(state, req.queue, req.queue_name.as_deref()).await;

    if req.interactive {
        // The dialog has no daemon state to resolve `queue_name`
        // against precedence-for-precedence, so the answer travels
        // already resolved. `auto_start_queue` is dropped on this path:
        // the dialog's own buttons are the start decision, and starting
        // a queue behind a dialog the user has not answered yet is the
        // pre-emptive behavior this path exists to stop.
        let mut req = req;
        req.queue = target_queue.map(|q| q.0);
        req.queue_name = None;
        let path = crate::ipc::staged::stage_capture(&req).map_err(|e| e.to_string())?;
        crate::daemon::tray::spawn_add_gui_staged(&path);
        return Ok(CaptureOutcome::Staged);
    }

    let auto_start_queue = req.auto_start_queue;
    let id = state
        .add_from_capture(req)
        .await
        .map_err(|e| e.to_string())?;

    if let Some(qid) = target_queue
        && let Err(e) = state.set_job_queue(id, qid).await
    {
        tracing::warn!(job = %id, queue = %qid, error = %e, "capture: set_job_queue failed");
    }

    // Browser capture, not a click in oxdm — the window opens here
    // either way, so a failure does not need to raise a second one.
    state.mark_run_intent(id, false).await;
    let _ = state.start_job(id).await;
    crate::daemon::tray::spawn_download_gui(id);

    if auto_start_queue && let Some(qid) = target_queue {
        let _ = state.start_queue(qid).await;
    }
    Ok(CaptureOutcome::Added(id))
}

/// Resolve `(queue_id, queue_name)` → an existing queue id.
/// Precedence: id wins; then name (case-insensitive); else `None`,
/// which leaves the job in whatever queue `add_from_capture` chose
/// (typically Main).
pub(crate) async fn resolve_queue(
    state: &Arc<AppState>,
    by_id: Option<uuid::Uuid>,
    by_name: Option<&str>,
) -> Option<crate::domain::QueueId> {
    use crate::domain::QueueId;
    let queues = state.queues_snapshot().await;
    if let Some(uuid) = by_id {
        let qid = QueueId(uuid);
        if queues.iter().any(|q| q.id == qid) {
            return Some(qid);
        }
    }
    if let Some(name) = by_name {
        let lower = name.trim().to_lowercase();
        if let Some(q) = queues.iter().find(|q| q.name.to_lowercase() == lower) {
            return Some(q.id);
        }
    }
    None
}

/// Scheme guard for the WS bridge. Only `http(s)` allowed.
///
/// LAN / loopback URLs are *not* rejected — downloading from a NAS,
/// internal mirror, or a developer's own dev server is legitimate
/// usage, and the WS bridge already requires the per-user auth token.
/// Network-policy decisions belong with the caller (the extension's
/// `isPublicHttpUrl` is what protects against attacker-page-driven
/// captures). Scripts that authenticate with the token are trusted to
/// know what they're pointing oxdm at.
pub fn guard_public_http_url(url: &url::Url) -> Result<(), String> {
    let scheme = url.scheme();
    if !matches!(scheme, "http" | "https") {
        return Err(format!("rejected scheme: {scheme}"));
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

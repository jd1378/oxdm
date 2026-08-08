//! `odl` conflict resolvers driven by oxdm UI.
//!
//! ODL drives the resolver synchronously via `async_trait`, but oxdm
//! cannot answer until the user clicks. We bridge with a `oneshot`:
//! the resolver emits a `ConflictRequested` event carrying a token,
//! then awaits the matching `oneshot::Receiver`. The UI calls
//! `AppState::resolve_*` which sends the answer.
//!
//! For non-interactive captures (extension-driven, no UI in front), the
//! resolver falls back to *sensible defaults* matching IDM:
//! - duplicate filename on disk → add number suffix
//! - same in-progress download exists → resume
//! - file changed on server → restart
//! - server doesn't support range → restart (single-connection)

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, oneshot};

use odl::Download;
use odl::conflict::{
    FileChangedResolution, FinalFileExistsResolution, NotResumableResolution,
    SameDownloadExistsResolution, SaveConflictResolver, ServerConflictResolver,
};

use crate::data::events::{ConflictKind, DomainEvent};
use crate::domain::{JobId, LiveCounters};

/// Picks resolutions for a single job. Construct one per `evaluate`/`download`
/// call; do not share across jobs.
pub struct UiResolver {
    job_id: JobId,
    events: broadcast::Sender<DomainEvent>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Resolution>>>,
    next_token: std::sync::atomic::AtomicU64,
    /// If set to `false`, the UI is hidden and `conflict_while_hidden` is
    /// `NotifyAndPark` — the resolver returns the abort variant instead of
    /// awaiting user input. The caller (runner) handles the park.
    interactive: bool,
    /// The job's live byte count — what a restart would throw away.
    /// A conflict that costs the user nothing is not worth a dialog.
    progress: Arc<LiveCounters>,
    /// Called when the server proves it will not resume. The headers
    /// go on advertising `accept-ranges` and odl only writes the
    /// correction to disk, so the run in front of the user would keep
    /// promising a resume it cannot deliver.
    observed_not_resumable: Box<dyn Fn() + Send + Sync>,
}

#[derive(Debug, Clone, Copy)]
pub enum Resolution {
    FileChanged(FileChangedResolution),
    NotResumable(NotResumableResolution),
    SameDownload(SameDownloadExistsResolution),
    FinalFile(FinalFileExistsResolution),
}

impl UiResolver {
    pub fn new(
        job_id: JobId,
        events: broadcast::Sender<DomainEvent>,
        interactive: bool,
        progress: Arc<LiveCounters>,
        observed_not_resumable: Box<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self {
            job_id,
            events,
            pending: Mutex::new(HashMap::new()),
            next_token: std::sync::atomic::AtomicU64::new(1),
            interactive,
            progress,
            observed_not_resumable,
        }
    }

    /// Whether this job has bytes a restart would discard.
    fn has_bytes_to_lose(&self) -> bool {
        self.progress.downloaded() > 0
    }

    /// Used by `AppState::resolve_*` to satisfy a pending request.
    pub fn answer(&self, token: u64, resolution: Resolution) -> bool {
        if let Some(tx) = self.pending.lock().unwrap().remove(&token) {
            tx.send(resolution).is_ok()
        } else {
            false
        }
    }

    fn emit_and_wait(&self, kind: ConflictKind) -> Option<oneshot::Receiver<Resolution>> {
        if !self.interactive {
            return None;
        }
        let token = self
            .next_token
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(token, tx);
        let _ = self.events.send(DomainEvent::ConflictRequested {
            id: self.job_id,
            kind,
            token,
        });
        Some(rx)
    }
}

#[async_trait]
impl ServerConflictResolver for UiResolver {
    async fn resolve_file_changed(&self, _i: &Download) -> FileChangedResolution {
        match self.emit_and_wait(ConflictKind::FileChanged) {
            Some(rx) => match rx.await {
                Ok(Resolution::FileChanged(r)) => r,
                _ => FileChangedResolution::Abort,
            },
            None => FileChangedResolution::Abort,
        }
    }

    async fn resolve_not_resumable(&self, _i: &Download) -> NotResumableResolution {
        // Whatever is decided below, the server has answered the
        // question the banner asks.
        (self.observed_not_resumable)();
        // With nothing downloaded, "the server will not resume" is not
        // a question: there is nothing to resume, and the answer is the
        // single-connection download the user already asked for. The
        // window says so in its banner. Only bytes already on disk make
        // this a decision worth interrupting for.
        if !self.has_bytes_to_lose() {
            return NotResumableResolution::Restart;
        }
        // Sensible default: restart (treat as single-connection download).
        // IDM behavior; matches user expectation.
        match self.emit_and_wait(ConflictKind::NotResumable) {
            Some(rx) => match rx.await {
                Ok(Resolution::NotResumable(r)) => r,
                _ => NotResumableResolution::Restart,
            },
            None => NotResumableResolution::Restart,
        }
    }
}

#[async_trait]
impl SaveConflictResolver for UiResolver {
    async fn same_download_exists(&self, _i: &Download) -> SameDownloadExistsResolution {
        match self.emit_and_wait(ConflictKind::SameDownloadExists) {
            Some(rx) => match rx.await {
                Ok(Resolution::SameDownload(r)) => r,
                _ => SameDownloadExistsResolution::Resume,
            },
            None => SameDownloadExistsResolution::Resume,
        }
    }

    async fn final_file_exists(&self, _i: &Download) -> FinalFileExistsResolution {
        match self.emit_and_wait(ConflictKind::FinalFileExists) {
            Some(rx) => match rx.await {
                Ok(Resolution::FinalFile(r)) => r,
                _ => FinalFileExistsResolution::AddNumberToNameAndContinue,
            },
            None => FinalFileExistsResolution::AddNumberToNameAndContinue,
        }
    }
}

/// Probe-only resolver. Aborts every conflict so `evaluate` never
/// proceeds past the resolution stage. Used by `AppState::probe` —
/// we want metadata, not a queued download.
pub struct ProbeResolver;

#[async_trait]
impl ServerConflictResolver for ProbeResolver {
    async fn resolve_file_changed(&self, _i: &Download) -> FileChangedResolution {
        FileChangedResolution::Abort
    }
    async fn resolve_not_resumable(&self, _i: &Download) -> NotResumableResolution {
        NotResumableResolution::Abort
    }
}

#[async_trait]
impl SaveConflictResolver for ProbeResolver {
    async fn same_download_exists(&self, _i: &Download) -> SameDownloadExistsResolution {
        // `Resume` keeps evaluate from erroring out, so probe still
        // returns useful metadata when a prior partial exists.
        SameDownloadExistsResolution::Resume
    }
    async fn final_file_exists(&self, _i: &Download) -> FinalFileExistsResolution {
        // Same reasoning — surface metadata, do not actually start.
        FinalFileExistsResolution::AddNumberToNameAndContinue
    }
}

use crate::domain::{JobError, JobId, Phase, QueueId};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Coarse-grained events the UI subscribes to via a `tokio::sync::broadcast`.
///
/// Hot per-byte progress is **not** emitted here — UI samples
/// `LiveCounters` on a render tick instead. Only state transitions that
/// change which row/dialog should re-render flow through here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DomainEvent {
    JobAdded {
        id: JobId,
    },
    JobUpdated {
        id: JobId,
        phase: Phase,
    },
    JobFilenameResolved {
        id: JobId,
        filename: String,
    },
    JobPartAdded {
        id: JobId,
        ulid: String,
        offset: u64,
        size: u64,
    },
    JobPartFinished {
        id: JobId,
        ulid: String,
    },
    JobCompleted {
        id: JobId,
        path: PathBuf,
        already_complete: bool,
    },
    JobFailed {
        id: JobId,
        error: JobError,
    },
    JobRemoved {
        id: JobId,
    },
    SettingsChanged,
    /// User must answer a server-side conflict (etag changed, not resumable, …).
    /// Carries a token the UI uses when calling `state.resolve_*`.
    ConflictRequested {
        id: JobId,
        kind: ConflictKind,
        token: u64,
    },
    /// Out-of-band request (e.g. tray menu click) to open the
    /// download dialog for `id`. UI listens for this and updates its
    /// `Signal<Option<JobId>>`.
    OpenDownloadDialog {
        id: JobId,
    },
    /// Request from tray ("Open oxdm") to surface the main window.
    ShowMainWindow,
    /// First job of a queue transitioned to running. Hook executor
    /// observes this to fire `Queue::on_start` actions.
    QueueStarted {
        id: QueueId,
    },
    /// Every job of a queue reached a terminal phase and at least one
    /// job had been running. Drives `Queue::on_finish` (shutdown, …).
    QueueFinished {
        id: QueueId,
    },
    /// Queue mutation (created / renamed / schedule edit / deleted).
    /// UI re-snapshots the queue list.
    QueuesChanged,
    /// Host overrides mutated. UI re-snapshots the per-host list.
    HostSettingsChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictKind {
    FileChanged,
    NotResumable,
    UrlBroken,
    CredentialsInvalid,
    SameDownloadExists,
    FinalFileExists,
}

use crate::domain::{JobError, JobId, Phase, PowerAction, QueueId};
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
    /// A retry is scheduled (odl `RetryScheduled`): the next attempt
    /// starts after `delay_ms`. `ulid` is `None` for a whole-download
    /// step such as the initial probe. The wait is interruptible, so
    /// this is the current plan rather than a promise.
    JobRetryScheduled {
        id: JobId,
        ulid: Option<String>,
        attempt: u32,
        max_attempts: u32,
        delay_ms: u64,
        server_requested: bool,
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
    /// A hash check could not be carried out — the saved file has moved
    /// or cannot be read. Distinct from a mismatch: nothing was
    /// disproved, so no row's verdict changes.
    JobVerifyFailed {
        id: JobId,
        message: String,
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
    /// The counts cover *this run* only — a queue keeps jobs from
    /// earlier runs, so they cannot be recovered from the job list.
    QueueFinished {
        id: QueueId,
        completed: u32,
        failed: u32,
    },
    /// Queue mutation (created / renamed / schedule edit / deleted).
    /// UI re-snapshots the queue list.
    QueuesChanged,
    /// A destructive power action (shutdown / restart / sleep /
    /// hibernate) was armed and will execute at `deadline_ms` (epoch
    /// milliseconds) unless cancelled. UI shows a countdown banner and
    /// derives the remaining time from the deadline — no timer state
    /// travels on the wire.
    ShutdownPending {
        action: PowerAction,
        deadline_ms: i64,
    },
    /// The pending power action was cancelled before its deadline.
    ShutdownCancelled,
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

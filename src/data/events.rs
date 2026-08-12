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
    /// A queue's run was ended by the user rather than by running out
    /// of work. Carries no tally because there is nothing to report:
    /// on-finish hooks belong to a queue that finished.
    QueueStopped {
        id: QueueId,
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
        /// Downloads left waiting on a question the user can settle.
        /// Separate from `failed`: nothing went wrong with them, and
        /// the queue will run them once they are answered.
        needs_answer: u32,
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
    /// The filesystem watcher hit — or stopped hitting — a kernel
    /// limit. Carries nothing: the UI asks for the detail, so a state
    /// that changes twice in a row cannot leave a stale copy behind.
    WatchLimitChanged,
    /// An update artifact has been fetched and its SHA-256 checked
    /// against the feed. Nothing is replaced until the user says so.
    UpdateStaged {
        version: String,
    },
    /// The update did not get as far as being installable.
    UpdateFailed {
        message: String,
    },
    /// Try to start the filesystem watcher again. Sent after the user
    /// raises the limit, so the fix takes effect now rather than at
    /// the next launch.
    FileWatchRetry,
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

/// The next event, riding out a lag instead of ending the subscription.
///
/// `broadcast::Receiver::recv` reports `Lagged` when a consumer falls
/// behind the channel's buffer. It is recoverable — the next `recv`
/// succeeds — but the obvious `while let Ok(ev) = rx.recv().await`
/// treats it as the end of the stream, and the daemon has no
/// supervision: a task that exits that way is gone for the rest of the
/// process, silently. That is how queue hooks, notifications,
/// completion actions and the power prompt could all stop working at
/// once after a single burst.
///
/// Returns `None` only when the sender is gone, which for the domain
/// bus means the daemon is going down.
pub async fn next_event(
    rx: &mut tokio::sync::broadcast::Receiver<DomainEvent>,
    who: &'static str,
) -> Option<DomainEvent> {
    loop {
        match rx.recv().await {
            Ok(ev) => return Some(ev),
            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                tracing::warn!(who, skipped, "fell behind the event bus");
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::JobId;

    /// A consumer that falls behind used to lose its subscription for
    /// the life of the process.
    #[tokio::test]
    async fn falling_behind_costs_events_not_the_subscription() {
        let (tx, mut rx) = tokio::sync::broadcast::channel::<DomainEvent>(2);
        for _ in 0..5 {
            tx.send(DomainEvent::JobAdded { id: JobId::new() }).unwrap();
        }
        let survivor = JobId::new();
        tx.send(DomainEvent::JobAdded { id: survivor }).unwrap();

        // The oldest events are gone, but the stream goes on.
        let ev = next_event(&mut rx, "test").await.expect("still subscribed");
        assert!(matches!(ev, DomainEvent::JobAdded { .. }));

        // And it keeps delivering until the sender is dropped.
        assert!(next_event(&mut rx, "test").await.is_some());
        drop(tx);
        assert!(next_event(&mut rx, "test").await.is_none());
    }
}

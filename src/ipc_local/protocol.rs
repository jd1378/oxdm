//! Wire protocol between the daemon and GUI subprocesses.

use std::collections::HashSet;
use std::path::PathBuf;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::data::ConflictKind;
use crate::data::ProbeResult;
use crate::data::RemoveOpts;
use crate::data::UpdateInfo;
use crate::data::UpdaterEvent;
use crate::domain::{
    Advanced, Category, Checksum, HostSetting, Job, JobError, JobId, OnCompletion, Phase,
    PowerAction, Queue, QueueId, Settings,
};

/// Top-level frame on the wire. Each frame is one length-prefixed
/// JSON document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Frame {
    /// Client → daemon. `req_id` echoed in the matching `Reply`.
    Request(u64, Request),
    /// Daemon → client.
    Reply(u64, Reply),
    /// Daemon → client. Pushed asynchronously after `Subscribe`.
    Event(Event),
}

/// Subscription filter. Clients pick exactly one.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum SubFilter {
    /// Receive every event + per-tick counter dumps for every job.
    /// Used by the main window.
    All,
    /// Receive events only for the named job + per-tick counters
    /// scoped to that job. Used by per-download windows.
    Job(JobId),
    /// Receive lifecycle events only (no counter pumps). Used by
    /// transient dialogs (settings/about/queues/etc) that don't render
    /// progress bars.
    Lifecycle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GuiKind {
    /// The main queue / table window (one per running daemon).
    Main,
    /// A per-job download window. Multiple may coexist, keyed by id.
    Download(JobId),
    /// A per-job Properties window. One per job (re-triggering evicts
    /// and re-spawns, like `Download`).
    Properties(JobId),
    /// Settings window (singleton).
    Settings,
    /// Queues & scheduling window (singleton).
    Queues,
    /// Add Download window (singleton — re-triggering focuses the
    /// existing window rather than opening a second one).
    Add,
    /// Batch-capture triage window. Singleton — a second batch
    /// request while one is still on screen merges into the same
    /// window (current impl: focuses the existing dialog).
    Batch,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    // ── connection management ──────────────────────────────────────
    Ping,
    Subscribe(SubFilter),
    /// Identify the calling GUI subprocess so the daemon can de-dup
    /// future spawn requests (tray "Open" / `OpenDownloadDialog`)
    /// into a `Focus` event on the existing connection instead.
    Hello(GuiKind),
    /// Tell the daemon to terminate after the current tick. Reserved
    /// for the tray "Quit" action and CLI `--quit`.
    DaemonQuit,

    // ── snapshot reads ─────────────────────────────────────────────
    /// Full state used by the main window on connect.
    Snapshot,
    /// A single job's state as needed by the per-download window.
    JobEntry(JobId),
    /// Just the host-settings list; main window refreshes via this on
    /// the Per Host Settings dialog open.
    HostList,

    // ── job lifecycle ──────────────────────────────────────────────
    AddJob(AddJobReq),
    AddUpdateJob {
        url: Url,
        filename: Option<String>,
    },
    StartJob(JobId),
    Pause(JobId),
    Resume(JobId),
    CancelToQueued(JobId),
    RestartJob(JobId),
    Remove(JobId, RemoveOpts),
    SetJobQueue(JobId, QueueId),
    /// Set a job's category explicitly.
    SetJobCategory(JobId, Category),
    /// Edit a job's URL and final destination. The per-job working
    /// dir lives under `Settings::work_dir` and is keyed only by job
    /// id, so changing these fields does not invalidate any in-flight
    /// `.part` data — the runner just assembles into the new
    /// destination at completion. URL changes flow through to odl's
    /// next `evaluate`; conflicts (size / Last-Modified mismatch with
    /// existing partial) surface via the standard `FileChanged`
    /// resolver path.
    UpdateJobLocation(JobId, JobEdit),

    // ── queues ─────────────────────────────────────────────────────
    StartQueue(QueueId),
    StopQueue(QueueId),
    PauseAll,
    ResumeAll,
    UpsertQueue(Queue),
    DeleteQueue(QueueId),

    // ── settings / hosts ───────────────────────────────────────────
    /// Boxed: `Settings` is by far the largest payload and would bloat
    /// every stack-passed `Request` (clippy `large_enum_variant`).
    /// `Box<T>` serializes identically to `T` — wire shape unchanged.
    UpdateSettings(Box<Settings>),
    RegenerateExtToken,
    UpsertHost(HostSetting),
    DeleteHost(String),
    /// Look up the OS-keyring password for a host. `Reply::HostPassword`
    /// carries `Some(secret)` when one is stored, `None` when no entry
    /// exists. The reply travels over the per-user local socket — same
    /// trust boundary as the keyring itself.
    HostPassword(String),
    /// Inspect the daemon's secrets-encryption state. Used by the GUI
    /// at boot to decide whether to surface the "master key missing"
    /// wipe-confirmation dialog. Reply is `Reply::SecretsStatus`.
    SecretsStatus,
    /// User acknowledged the missing-key dialog. NULL every encrypted
    /// secret column, generate a fresh master key, unlock the daemon.
    /// Reply is `Reply::Ok` or `Reply::Err`.
    WipeJobSecrets,
    /// Decrypt every per-job secret (auth password, proxy password,
    /// cookies) for the Add/Edit dialog. The daemon never broadcasts
    /// plaintext secrets — the UI pulls them explicitly when the
    /// dialog opens. Reply is `Reply::JobSecretsPlaintext`; each
    /// field is `None` when no ciphertext is present.
    JobSecretsPlaintext(JobId),
    /// Boot-time health check for the on-disk SQLite store. Reply is
    /// `Reply::DbStatus(None)` when the original `Store::open` call
    /// succeeded, or `Reply::DbStatus(Some(msg))` when the daemon
    /// fell back to an in-memory store. The GUI uses this to drive
    /// the Exit / Reset recovery modal.
    DbStatus,
    /// User acknowledged the "database broken" recovery dialog and
    /// picked Reset. Daemon renames the corrupt file to a `.bak`
    /// sibling and exits — a fresh daemon on next launch creates a
    /// clean DB.
    ResetDatabase,

    // ── per-job overrides ──────────────────────────────────────────
    SetSessionSpeedLimit(JobId, Option<u64>),
    SetPersistentSpeedLimit(JobId, Option<u64>),
    SetMaxConnections(JobId, Option<u64>),
    SetOnCompletion(JobId, OnCompletion),
    /// Replace the per-job Advanced bundle (Properties dialog Apply).
    SetJobAdvanced(JobId, Advanced),
    /// Replace the per-job checksum list (Properties dialog Apply).
    SetJobChecksums(JobId, Vec<Checksum>),
    /// Replace only the source URL + destination (save_dir + filename) of a
    /// non-running job (Properties dialog Apply). Narrower than
    /// `UpdateJobLocation` so it can't disturb the job's secrets/headers.
    SetJobSource(JobId, Url, PathBuf, Option<String>),

    // ── conflict resolution ────────────────────────────────────────
    PeekConflict,
    PopConflict,
    ResolveFileChanged(JobId, u64, FileChangedRes),
    ResolveNotResumable(JobId, u64, NotResumableRes),
    ResolveSameDownload(JobId, u64, SameDownloadRes),
    ResolveFinalFile(JobId, u64, FinalFileRes),

    // ── power actions ──────────────────────────────────────────────
    /// Cancel the pending destructive power action (countdown banner's
    /// Cancel button). Idempotent: replies `Ok` even when nothing is
    /// pending (e.g. the timer fired a beat earlier).
    CancelPendingShutdown,

    // ── update channel ─────────────────────────────────────────────
    UpdateCheck,

    // ── one-shot helpers ───────────────────────────────────────────
    Probe(Url),

    /// Ask the daemon to surface a per-job download window: focus an
    /// existing GUI subprocess if one is registered, otherwise spawn
    /// a fresh `oxdm gui download <id>`.
    OpenDownloadWindow(JobId),
    /// Open (evict + re-spawn) the per-job Properties window.
    OpenPropertiesWindow(JobId),
    /// Same idea for the main window.
    OpenMainWindow,
    /// Open or focus the Settings window. `tab` selects the initial
    /// tab ("general" / "downloads" / "network" / "appearance" /
    /// "advanced"); `highlight_proxy` jumps to and highlights the
    /// proxy URL field on open.
    OpenSettingsWindow {
        tab: Option<String>,
        highlight_proxy: bool,
    },
    /// Open or focus the Queues & scheduling window.
    OpenQueuesWindow,
    /// Open or focus the Add Download window. `edit_id` carries the
    /// capture-review path; `prefill_url` is the clipboard-resolved
    /// URL the caller (main GUI) read on the user's behalf, since the
    /// daemon process has no clipboard access of its own.
    OpenAddWindow {
        edit_id: Option<JobId>,
        prefill_url: Option<String>,
    },

    /// Look up a job id by filename via the persistent store. Returns
    /// `Reply::JobIdOpt` with the first match (sqlite query, no
    /// in-memory scan).
    FindJobByFilename(String),
}

/// Editable fields for `UpdateJobLocation`. Mirrors the subset of
/// `Job` the Add/Edit dialog exposes. Secrets (Basic auth password,
/// proxy password, cookies) travel as plain `Option<String>` —
/// `None` or empty ⇒ clear the encrypted column, otherwise the
/// daemon re-encrypts with the master key on every save. The dialog
/// pre-fetches the current plaintext via `JobSecretsPlaintext` at
/// open time so the user always edits real values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobEdit {
    pub url: Url,
    pub save_dir: PathBuf,
    pub filename: Option<String>,
    pub referrer: Option<Url>,
    pub headers: IndexMap<String, String>,
    pub max_connections: Option<u64>,
    /// Per-job proxy URL. Format: `scheme://[user@]host:port` — no
    /// password embedded; that comes from `proxy_password`. `None`
    /// inherits the global proxy.
    #[serde(default)]
    pub proxy: Option<String>,
    #[serde(default)]
    pub auth_user: Option<String>,
    #[serde(default)]
    pub auth_password: Option<String>,
    #[serde(default)]
    pub proxy_password: Option<String>,
    #[serde(default)]
    pub cookies: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddJobReq {
    pub url: Url,
    pub save_dir: PathBuf,
    pub filename: Option<String>,
    pub referrer: Option<Url>,
    pub headers: IndexMap<String, String>,
    pub max_connections: Option<u64>,
    /// Per-job proxy URL. Format: `scheme://[user@]host:port` — no
    /// password embedded; that comes from `proxy_password`. `None`
    /// inherits the global `Settings::proxy`.
    #[serde(default)]
    pub proxy: Option<String>,
    #[serde(default)]
    pub auth_user: Option<String>,
    /// HTTP Basic password (plaintext). Daemon encrypts with the
    /// master key before storing. `None` / empty ⇒ no password stored.
    #[serde(default)]
    pub auth_password: Option<String>,
    /// Proxy password (plaintext). Daemon encrypts before storing.
    #[serde(default)]
    pub proxy_password: Option<String>,
    /// Cookie jar (plaintext, raw `Cookie:` header value). Daemon
    /// encrypts before storing.
    #[serde(default)]
    pub cookies: Option<String>,
    /// Explicit category chosen in the Add dialog. `None` lets the
    /// daemon detect it from the filename + user settings.
    #[serde(default)]
    pub category: Option<Category>,
}

// Resolution mirrors of `odl::conflict::*` so the wire is independent
// of the `odl` crate (kept inside `data`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum FileChangedRes {
    Abort,
    Restart,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum NotResumableRes {
    Abort,
    Restart,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum SameDownloadRes {
    Abort,
    AddNumberAndContinue,
    Resume,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum FinalFileRes {
    Abort,
    Replace,
    AddNumberAndContinue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Reply {
    Ok,
    Err(String),
    Snapshot(SnapshotData),
    JobEntry(Option<JobEntryView>),
    JobAdded(JobId),
    JobIdOpt(Option<JobId>),
    HostList(Vec<HostSetting>),
    HostPassword(Option<String>),
    /// Structured probe outcome: the error side carries the full
    /// `JobError` so the Add dialog can render a typed error panel
    /// instead of a flattened string.
    ProbeResult(Result<ProbeResult, JobError>),
    UpdateInfo(Option<UpdateInfo>),
    ConflictHead(Option<(JobId, ConflictKind, u64)>),
    ConflictLen(usize),
    SecretsStatus {
        locked: bool,
    },
    JobSecretsPlaintext {
        auth_password: Option<String>,
        proxy_password: Option<String>,
        cookies: Option<String>,
    },
    DbStatus(Option<String>),
}

/// Daemon → client async events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Event {
    /// Per-tick (250 ms) counter dump for every job in scope.
    Counters(Vec<JobCounters>),
    /// Job list / phase / metadata changed; client should re-fetch
    /// `Snapshot` (or for a per-job window, `JobEntry(id)`).
    JobsChanged,
    QueuesChanged,
    SettingsChanged,
    ActiveQueuesChanged,
    HostListChanged,
    ConflictChanged,
    JobCompleted {
        id: JobId,
        path: PathBuf,
    },
    JobFailed {
        id: JobId,
        error: JobError,
    },
    Updater(UpdaterEvent),
    /// A destructive power action was armed; it executes at
    /// `deadline_ms` (epoch milliseconds) unless cancelled via
    /// `Request::CancelPendingShutdown`. GUIs derive the remaining
    /// seconds from the deadline — no timer state on the wire.
    ShutdownPending {
        action: PowerAction,
        deadline_ms: i64,
    },
    /// The pending power action was cancelled before its deadline.
    ShutdownCancelled,
    /// Daemon asks the GUI process to spawn a per-download window for
    /// the given job (e.g. when capture flow opens one). The main
    /// window owns the spawn decision.
    OpenDownloadDialog(JobId),
    /// Daemon asks the GUI to surface its window (single-instance
    /// re-launch, tray "Open" while a stale GUI is alive).
    ShowMainWindow,
    /// Daemon asks this specific GUI process (matched by `Hello` kind)
    /// to raise its window. Sent in lieu of spawning a duplicate.
    Focus,
    /// Daemon asks this specific GUI process to exit. Sent when the
    /// daemon prefers to spawn a fresh subprocess over surfacing the
    /// existing one (per-download window re-open from main).
    Close,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotData {
    pub jobs: Vec<Job>,
    pub queues: Vec<Queue>,
    pub settings: Settings,
    pub active_queues: HashSet<QueueId>,
    pub conflict_head: Option<(JobId, ConflictKind, u64)>,
    pub conflict_len: usize,
    pub counters: Vec<JobCounters>,
    /// Pending destructive power action `(action, deadline_ms)`, so a
    /// GUI connecting mid-countdown still shows the banner.
    #[serde(default)]
    pub pending_shutdown: Option<(PowerAction, i64)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobCounters {
    pub id: JobId,
    pub phase: Phase,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub speed_bps: f64,
    /// 0 = unknown, 1 = yes, -1 = no.
    pub is_resumable: i8,
    pub running: bool,
    /// Live count of `PartRetrying` events this run. Lets an in-progress
    /// download show the running retry tally; completion reads the
    /// persisted `Job::retries` instead.
    pub retries: u32,
    pub parts: Vec<PartView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartView {
    pub ulid: String,
    pub offset: u64,
    pub size: u64,
    pub downloaded: u64,
    pub speed_bps: f64,
    pub finished: bool,
}

/// Per-download window snapshot. Adds metadata + per-job overrides
/// the main-window `JobCounters` does not need.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobEntryView {
    pub job: Job,
    pub counters: JobCounters,
    pub on_completion: OnCompletion,
    pub session_speed_override: u64,
}

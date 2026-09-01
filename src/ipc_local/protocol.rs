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
    Advanced, Category, Checksum, CondKind, Job, JobError, JobId, OnCompletion, Phase, PowerAction,
    Queue, QueueId, Settings,
};

/// Top-level frame on the wire. Each frame is one length-prefixed
/// JSON document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Frame {
    /// Client → daemon. `req_id` echoed in the matching `Reply`.
    /// Boxed for the same reason as the reply below: an `AddJob`
    /// carries a whole form, and every ping would otherwise be as big
    /// as the largest request anyone can send.
    Request(u64, Box<Request>),
    /// Daemon → client. Boxed: a reply carrying a full snapshot is
    /// twice the size of anything else on the wire, and every frame —
    /// including a one-word event — would otherwise be that big.
    Reply(u64, Box<Reply>),
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
    /// Shutdown/sleep grace-countdown window (singleton). Spawned by
    /// the daemon when a destructive power action arms; offers instant
    /// Cancel / Confirm.
    Power,
    /// About window (singleton): identity, update check, build facts.
    About,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    // ── connection management ──────────────────────────────────────
    /// First frame on every connection: the token from the daemon's
    /// 0600 token file. Until it arrives and matches, the daemon
    /// answers nothing else — see `ipc_local::auth`.
    Auth(String),
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

    // ── job lifecycle ──────────────────────────────────────────────
    AddJob(AddJobReq),
    /// Fetch an update artifact as a hidden download. The daemon holds
    /// on to the digest from the feed and checks the artifact against
    /// it before anything is offered for installing.
    AddUpdateJob(UpdateInfo),
    /// Replace the running executable with the staged update and
    /// relaunch. Only answered once an update has reached
    /// `Event::UpdateStaged`.
    InstallUpdate,
    /// Start a job. `manual` marks a gesture aimed at this one
    /// download (a row's Start, Add → Download now): only those raise
    /// the failure window. Bulk senders (batch triage) pass `false`.
    StartJob {
        id: JobId,
        manual: bool,
    },
    Pause(JobId),
    Resume(JobId),
    CancelToQueued(JobId),
    RestartJob(JobId),
    /// Delete the assembled file for a completed job, leaving the job
    /// itself in the list. `Remove` is the one that forgets the
    /// download; this only reclaims the bytes on disk.
    DeleteFinalFile(JobId),
    /// Hash the saved file and record the verdict on the job's checksum
    /// rows. Runs in the daemon: a hash of a large file outlives the
    /// window that asked for it.
    VerifyChecksums(JobId),
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
    /// The order pending downloads should run in, as the queue window
    /// left it. Ids outside `queue` are ignored; anything in the queue
    /// the caller did not list keeps its place behind them.
    ReorderQueue {
        queue: QueueId,
        ids: Vec<JobId>,
    },
    PauseAll,
    /// Pause everything *and* end every queue run — the toolbar's Stop
    /// all, as opposed to the tray's Pause all.
    StopAll,
    ResumeAll,
    UpsertQueue(Queue),
    DeleteQueue(QueueId),

    // ── settings / hosts ───────────────────────────────────────────
    /// Boxed: `Settings` is by far the largest payload and would bloat
    /// every stack-passed `Request` (clippy `large_enum_variant`).
    /// `Box<T>` serializes identically to `T` — wire shape unchanged.
    UpdateSettings(Box<Settings>),
    /// Save only the named fields, merged onto whatever the daemon
    /// currently holds. What the Settings window's Apply sends, so it
    /// cannot revert a change made somewhere else while it was open.
    UpdateSettingsFields {
        settings: Box<Settings>,
        keys: Vec<String>,
    },
    /// Mint a pairing code without storing it: the Settings window
    /// stages it and saves it with the rest of the page.
    MintExtToken,
    /// Register oxdm as a native-messaging host with every browser on
    /// this machine, and report what happened per browser. Runs in the
    /// daemon because it writes to the user's home and the windows are
    /// short-lived. Reply is `Reply::NativeHost`.
    InstallNativeHost,
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
    /// How many downloads still hold partly-fetched data under a cache
    /// folder that is no longer the configured one, and how many bytes
    /// that is. Reply is `Reply::StrandedPartials`.
    StrandedPartials,
    /// Delete that data and set those downloads up to start over.
    /// Reply is `Reply::Count`.
    DiscardStrandedPartials,
    /// Boot-time health check for the on-disk SQLite store. Reply is
    /// `Reply::DbStatus`: `error` is set when the original
    /// `Store::open` failed and the daemon fell back to an in-memory
    /// store (the GUI drives its Exit / Reset recovery modal off it),
    /// `warning` when the store opened but something inside it could
    /// not be read — settings, or individual job rows.
    DbStatus,
    /// User acknowledged the "database broken" recovery dialog and
    /// picked Reset. Daemon renames the corrupt file to a `.bak`
    /// sibling and exits — a fresh daemon on next launch creates a
    /// clean DB.
    ResetDatabase,
    /// What kernel limit, if any, is stopping the filesystem watcher.
    /// Reply is `Reply::WatchLimit`. Drives the warning dialog that
    /// offers to raise it.
    WatchLimit,
    /// The user raised the limit — start the watcher again, without
    /// waiting for the next launch.
    RetryFileWatch,

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
    /// Execute the pending destructive power action immediately
    /// ("Shut down now" in the countdown window). Idempotent — Ok even
    /// when nothing is pending.
    ConfirmPendingShutdown,

    // ── update channel ─────────────────────────────────────────────
    UpdateCheck,
    /// Where the update flow currently stands, without touching the
    /// network. A window opened after the fact — by an update alert, or
    /// by the user a minute later — picks the flow up where it is
    /// instead of offering to start it again.
    UpdateState,
    /// Abandon the update in flight: stop the download, throw away what
    /// it fetched, and forget the installer waiting on it. What the
    /// check found is kept, so the offer stands.
    CancelUpdate,

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
    /// Open or focus the Queues & scheduling window. `select` picks
    /// the queue it opens on; `delete` asks it for that queue's delete
    /// confirmation, so the main window's queue menu hands the
    /// destructive step to the dialog that already owns it rather than
    /// keeping a second one in step.
    OpenQueuesWindow {
        select: Option<QueueId>,
        delete: bool,
    },
    /// Open or focus the About window.
    OpenAboutWindow,
    /// This connection's window gained or lost keyboard focus. Lets the
    /// daemon skip surfacing a window the user is already looking at.
    WindowFocused(bool),
    /// Open or focus the Add Download window. `edit_id` carries the
    /// capture-review path; `prefill_url` is the clipboard-resolved
    /// URL the caller (main GUI) read on the user's behalf, since the
    /// daemon process has no clipboard access of its own.
    OpenAddWindow {
        edit_id: Option<JobId>,
        prefill_url: Option<String>,
    },

    /// Open the batch-triage window for a list of links the user
    /// pasted or dropped. The daemon stages them the same way the
    /// browser bridge does — the dialog reads a file, not argv, because
    /// a hundred URLs do not fit on a command line.
    OpenBatchWindow(Vec<Url>),

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
    /// Credential changes to apply along with the edit, or `None` to
    /// leave the job's proxy and site auth exactly as they are.
    ///
    /// Properties sends `None` from its Headers/Cookies Apply — that
    /// tab is not where credentials are edited, and rewriting them
    /// from a form it never showed would be a way to lose a stored
    /// secret. The Add window in edit mode does show them, and sends
    /// what it showed.
    #[serde(default)]
    pub creds: Option<crate::domain::Creds>,
    #[serde(default)]
    pub cookies: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddJobReq {
    pub url: Url,
    /// Which queue the job belongs to. `None` is Main. Part of the add
    /// rather than a move afterwards, so the job is never briefly in a
    /// queue it was not meant for.
    #[serde(default)]
    pub queue: Option<QueueId>,
    pub save_dir: PathBuf,
    pub filename: Option<String>,
    pub referrer: Option<Url>,
    pub headers: IndexMap<String, String>,
    pub max_connections: Option<u64>,
    /// Proxy and site-authentication choices, exactly as the Add
    /// dialog's Advanced pane and Properties → Connection both express
    /// them. Secrets travel in plaintext over the local socket and the
    /// daemon encrypts them before they reach the database.
    #[serde(default)]
    pub creds: crate::domain::Creds,
    /// Cookie jar (plaintext, raw `Cookie:` header value). Daemon
    /// encrypts before storing.
    #[serde(default)]
    pub cookies: Option<String>,
    /// Explicit category chosen in the Add dialog. `None` lets the
    /// daemon detect it from the filename + user settings.
    #[serde(default)]
    pub category: Option<Category>,
    /// Size the Add dialog's probe reported, if it got one. Carried so a
    /// queued job knows how big it is before it has ever run — the
    /// window and the list can show a size and a percentage instead of
    /// waiting for the first progress event to tell them what the probe
    /// already knew.
    #[serde(default)]
    pub size: Option<u64>,
    /// Digests the probe read out of the server's headers, so they are
    /// on the job before it first runs — visible in Properties, and
    /// checked even if the very first attempt is what completes it.
    #[serde(default)]
    pub checksums: Vec<crate::domain::Checksum>,
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
    /// The request was carried out, but something inside it went wrong
    /// in a way the user should hear about — the entry was removed and
    /// the file it was meant to take with it is still on disk. Distinct
    /// from `Err`, which means nothing happened.
    Warning(String),
    Snapshot(SnapshotData),
    JobEntry(Option<JobEntryView>),
    JobAdded(JobId),
    JobIdOpt(Option<JobId>),
    /// Structured probe outcome: the error side carries the full
    /// `JobError` so the Add dialog can render a typed error panel
    /// instead of a flattened string.
    ProbeResult(Result<ProbeResult, JobError>),
    UpdateInfo(Option<UpdateInfo>),
    UpdateState(UpdateState),
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
    StrandedPartials {
        count: u64,
        bytes: u64,
    },
    /// How many items an action affected.
    Count(u64),
    DbStatus {
        /// The store is unusable; the recovery modal is the answer.
        error: Option<String>,
        /// The store is fine but something in it would not read.
        warning: Option<String>,
    },
    WatchLimit(Option<crate::domain::WatchLimit>),
    /// A freshly minted pairing code, not yet saved anywhere.
    ExtToken(String),
    NativeHost(Box<crate::domain::HostReport>),
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
    /// The filesystem watcher's health changed; ask for the detail
    /// with `Request::WatchLimit`.
    WatchLimitChanged,
    ConflictChanged,
    JobCompleted {
        id: JobId,
        path: PathBuf,
    },
    JobFailed {
        id: JobId,
        error: JobError,
    },
    /// A hash check could not run: the saved file has moved or cannot
    /// be read. No checksum row changed — nothing was disproved.
    VerifyFailed {
        id: JobId,
        message: String,
    },
    /// odl scheduled a retry: the next attempt starts in `delay_ms`.
    /// `ulid` names the part it belongs to, or `None` for a
    /// whole-download step such as the probe. The wait is
    /// interruptible, so a UI should treat this as the current plan.
    RetryScheduled {
        id: JobId,
        ulid: Option<String>,
        attempt: u32,
        max_attempts: u32,
        delay_ms: u64,
        server_requested: bool,
    },
    Updater(UpdaterEvent),
    /// A newer release exists. Raised by the automatic checker once the
    /// user is at the machine; a window that is already open switches
    /// to it, and About shows the same thing when the alert spawns it.
    UpdateAvailable {
        info: UpdateInfo,
    },
    /// An update artifact has been fetched and verified against the
    /// digest the feed published. Installing it is the user's call.
    UpdateStaged {
        version: String,
    },
    /// The update stopped short of being installable.
    UpdateFailed {
        message: String,
    },
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

/// How far along an update is, as the daemon sees it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum UpdateState {
    /// Nothing found, or nothing looked for yet.
    Idle,
    /// A newer version exists and nothing has been fetched.
    Found { info: UpdateInfo },
    /// Its artifact is downloading, as this job.
    Downloading { info: UpdateInfo, job: JobId },
    /// Fetched, checked, and waiting on the user to say install.
    Staged { version: String },
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
    /// Schedule conditions this host can evaluate right now (runtime
    /// capability, e.g. AC power only when a battery exists). The
    /// queues GUI hides the rest; the scheduler ignores them.
    #[serde(default)]
    pub cond_available: Vec<CondKind>,
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
    /// odl is transferring this part right now, as opposed to having
    /// allocated it and not started it yet.
    ///
    /// Sent rather than inferred from `speed_bps`: a part on a slow
    /// link reports no bytes for whole sampling windows, and a reader
    /// that reads that as "not started" flickers the segment between
    /// Active and Pending.
    #[serde(default)]
    pub active: bool,
}

/// Per-download window snapshot. Adds metadata + per-job overrides
/// the main-window `JobCounters` does not need.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobEntryView {
    pub job: Job,
    pub counters: JobCounters,
    pub on_completion: OnCompletion,
    pub session_speed_override: u64,
    /// A hash of the saved file is running in the daemon right now.
    /// Lives here rather than in a window so every window agrees, and
    /// so closing the one that started it changes nothing.
    #[serde(default)]
    pub verifying: bool,
}

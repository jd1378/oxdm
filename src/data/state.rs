//! [`AppState`] — single shared object that the UI, IPC bridge, and the
//! per-job runners all hold an `Arc` of.
//!
//! Responsibilities:
//! - own the SQLite [`Store`]
//! - own the current `Settings` and a derived `odl::DownloadManager`
//! - own the in-memory job registry (`IndexMap<JobId, JobEntry>`)
//! - publish `DomainEvent`s on a `broadcast` channel
//! - expose intent-level methods (`add_job`, `start_job`, `pause`, …)
//!   that the UI and IPC drive without ever touching `odl` directly

use indexmap::IndexMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicU32, AtomicU64, Ordering};
use tokio::sync::{RwLock, broadcast};
use tokio_util::sync::CancellationToken;

use odl::download_manager::DownloadManager;
use odl::progress::ProgressEvent as OdlProgressEvent;

use crate::data::events::DomainEvent;
use crate::data::mapping::settings_to_odl_config;
use crate::data::pause::{CancelResumeStrategy, DynPauseStrategy, JobHandle, ResumeContext};
use crate::data::resolvers::{ProbeResolver, Resolution, UiResolver};
use crate::data::runner::{JobRunner, LiveBridge, PartCounters};
use crate::data::store::{Store, default_db_path};
use crate::data::update_channel::{NoopUpdateChannel, UpdateChannel};
use crate::domain::{
    CaptureRequest, Category, Job, JobError, JobId, JobStatus, LiveCounters, Phase, Queue, QueueId,
    Settings, classify,
};

/// In-memory record per job. Lives in `AppState::jobs`.
///
/// `parts` uses `std::sync::RwLock` (not tokio's) because UI render code
/// reads it from a sync context. The runner only ever does
/// `try_read`/`try_write` on it, so contention is bounded.
pub struct JobEntry {
    pub job: Job,
    /// Authoritative live phase. `job.status.phase` is the load-time
    /// snapshot from the store and never mutates after construction;
    /// the runner / pause path bumps this atomic instead. UI reads this
    /// for the visible status pill.
    pub live_phase: AtomicU8,
    pub counters: Arc<LiveCounters>,
    /// First `Downloading` transition of the current run, as epoch
    /// milliseconds. `0` = None (not yet started). Set-once per run by
    /// the live bridge; reset on restart / cancel-to-queued. Spliced
    /// onto `Job::started_at` for the UI.
    pub started_at_ms: AtomicI64,
    /// Milliseconds this run has spent in `Downloading`, excluding the
    /// stretch it is in right now (see `active_ms`). Wall clock between
    /// start and finish is a different number: it counts pauses, retry
    /// waits and the time a queued job sat behind others, and dividing
    /// bytes by it reports a speed the transfer never ran at.
    pub active_ms: AtomicI64,
    /// When the current `Downloading` stretch began, epoch
    /// milliseconds. `0` = not downloading.
    pub downloading_since_ms: AtomicI64,
    /// `Completed` transition timestamp, epoch milliseconds. `0` = None.
    /// Spliced onto `Job::finished_at`.
    pub finished_at_ms: AtomicI64,
    /// Cumulative count of `PartRetrying` events this run. Spliced onto
    /// `Job::retries`.
    pub retries: AtomicU32,
    /// The parts map still describes the *previous* run. odl
    /// re-announces every part when a run starts, under fresh ulids, so
    /// the old rows are dropped as the first new one arrives rather
    /// than when the run begins — otherwise the table blanks between
    /// the two.
    pub parts_stale: AtomicBool,
    /// A daemon-side hash of the saved file is running for this job.
    /// The window that asked can close; the work and its result stay
    /// here.
    pub verifying: AtomicBool,
    /// Live mirror of `Job::interruptions` — part retries plus explicit
    /// resumes, the one number the completed view reports.
    pub interruptions: AtomicU32,
    /// ULIDs of parts currently mid-retry. Drives the `Reconnecting`
    /// phase: non-empty ⇒ at least one part is retrying. Keyed by ulid
    /// (rather than a bare counter) so a sibling part's progress tick
    /// can't spuriously clear a still-retrying part — debounces the
    /// banner. Cleared on restart / cancel-to-queued.
    pub retrying_parts: std::sync::Mutex<std::collections::HashSet<String>>,
    pub parts: std::sync::RwLock<IndexMap<String, Arc<PartCounters>>>,
    pub cancel: std::sync::Mutex<CancellationToken>,
    pub running: AtomicBool,
    /// Why the last run of this job failed. Lives here because the
    /// registry's `Job` is immutable and `JobStatus.error` has no
    /// column in the store: without it the failure exists only as a
    /// fired event, so a window opened *after* the failure — which is
    /// every window the daemon spawns in response to one — sees a job
    /// that is `Failed` for no stated reason. Cleared when the job
    /// starts again.
    pub last_error: std::sync::RwLock<Option<crate::domain::JobError>>,
    /// The user started *this* download by hand — a row's Resume, the
    /// download window's Resume, Add → Download now, Retry. Only such
    /// a run raises the failure window: a batch reports its failures in
    /// the queue-finished summary instead of stacking one window per
    /// failed job. Set by [`AppState::mark_run_intent`] at every entry
    /// point that starts a job, so it always describes the current run.
    pub manual_run: AtomicBool,
    /// The concurrency cap sent this one back to Queued rather than
    /// starting it. Set by [`AppState::start_job`] when it refuses an
    /// automatic start, cleared when the job actually starts. It is
    /// what tells [`AppState::fill_deferred_slots`] which queued jobs
    /// are waiting on a slot as opposed to waiting on a person.
    pub deferred_by_cap: AtomicBool,
    /// `0` = unknown, `1` = yes, `-1` = no. Set by the runner after
    /// evaluate succeeds. UI exposes the value as
    /// "Resume support: Yes / No / Unknown".
    pub is_resumable: std::sync::atomic::AtomicI8,
    /// Response headers from this session's evaluate probe. Overlays
    /// `Job::captured_response` (the load-time snapshot from the store)
    /// via `splice_live`, so a fresh probe supersedes an older capture
    /// and the same splice persists it back through `persist_job`.
    pub captured_response: std::sync::RwLock<Option<crate::domain::CapturedResponse>>,
    /// Session-scoped per-job speed cap, in bytes/sec. `0` = inherit
    /// the global `Settings::speed_limit`. Lives only in memory; the
    /// Speed tab's "Remember" checkbox writes the value to
    /// `Job::speed_limit_override` for cross-restart persistence.
    pub session_speed_override: std::sync::atomic::AtomicU64,
    /// Per-job completion actions (IDM-style "Options on completion").
    /// Defaults to showing the system notification only.
    pub on_completion: std::sync::RwLock<crate::domain::OnCompletion>,
    /// Digests already computed for the saved file, and which file they
    /// were computed from. Hashing a finished download is minutes of
    /// disk for a large one, and Properties asks for it again every
    /// time a row is added or removed — nearly always about the same
    /// bytes as last time.
    pub hashed: std::sync::Mutex<Option<HashedFile>>,
    /// Active resolver for the in-flight runner, if any. The UI calls
    /// into it via `AppState::resolve_*`.
    pub resolver: RwLock<Option<Arc<UiResolver>>>,
    /// Final on-disk path captured from the runner's `JobCompleted`
    /// outcome. `Job::status::final_path` is the load-time snapshot
    /// from the store (always `None` for jobs that completed in this
    /// session). UI layers splice this into the Job view so the
    /// "Download complete" dialog has a real path to open / reveal.
    pub final_path: std::sync::RwLock<Option<PathBuf>>,
    /// Live knobs handed to odl through `DownloadContext::with_live` on
    /// run. Stays attached to the entry so concurrent control paths
    /// (Apply button in the Speed tab, queue rebalancer, etc.) can call
    /// `set_max_connections` mid-flight without going through the
    /// runner's future.
    pub live_controls: odl::progress::LiveControls,
}

/// Digests computed from one particular file.
///
/// Tied to the file's length and modification time, not just its path:
/// a file replaced on disk is a different file, and handing out a digest
/// from the old one would let new bytes pass a check they never took.
#[derive(Debug, Clone)]
pub struct HashedFile {
    pub len: u64,
    pub mtime_ms: i64,
    pub digests: std::collections::HashMap<crate::domain::Algo, String>,
}

/// Length + modification time of `path`, or `None` if it cannot be
/// asked — in which case nothing is cached and nothing is reused.
async fn file_identity(path: &std::path::Path) -> Option<(u64, i64)> {
    let meta = tokio::fs::metadata(path).await.ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis() as i64;
    Some((meta.len(), mtime))
}

impl JobEntry {
    /// Digests already known for the file `ident` describes.
    fn known_digests(
        &self,
        ident: Option<(u64, i64)>,
    ) -> std::collections::HashMap<crate::domain::Algo, String> {
        let Some((len, mtime_ms)) = ident else {
            return Default::default();
        };
        self.hashed
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .filter(|h| h.len == len && h.mtime_ms == mtime_ms)
            .map(|h| h.digests)
            .unwrap_or_default()
    }

    /// Remember what this file hashed to, replacing any record of an
    /// older version of it.
    fn remember_digests(
        &self,
        ident: Option<(u64, i64)>,
        digests: std::collections::HashMap<crate::domain::Algo, String>,
    ) {
        let Some((len, mtime_ms)) = ident else {
            return;
        };
        if digests.is_empty() {
            return;
        }
        if let Ok(mut g) = self.hashed.lock() {
            *g = Some(HashedFile {
                len,
                mtime_ms,
                digests,
            });
        }
    }

    /// Where this job's assembled file is, live value first.
    ///
    /// The runner writes the path it actually produced into the atomic
    /// slot as soon as the download lands; `job.status` catches up on
    /// the next persist. Reading only the persisted side misses a file
    /// that exists — which, for anything that deletes, means silently
    /// leaving it behind.
    pub fn saved_file(&self) -> Option<std::path::PathBuf> {
        self.final_path
            .read()
            .ok()
            .and_then(|g| g.clone())
            .or_else(|| self.job.status.final_path.clone())
    }

    /// Per-job completion prefs are passed in rather than defaulted:
    /// they live in memory only, so every boot re-seeds them from the
    /// current global setting, and from then on the job's own answer is
    /// what the completion handler reads.
    fn with_completion(job: Job, on_completion: crate::domain::OnCompletion) -> Self {
        let phase = job.status.phase;
        let job_final_path = job.status.final_path.clone();
        // Seed live counters from the persisted snapshot so a freshly
        // loaded entry already reports its last-known progress. Without
        // this, `splice_live` would clobber the stored `downloaded` /
        // `total` columns with zeros on the first persist after boot.
        let counters = LiveCounters::new();
        counters.set_downloaded(job.status.downloaded);
        counters.set_total(job.status.total);
        // Seed the run-stat atomics from the persisted snapshot so a
        // freshly loaded entry already reports its last-known
        // started_at / finished_at / retries. `0` = None.
        let job_active_ms = job.active_ms.unwrap_or(0) as i64;
        let started_at_ms = job.started_at.map(|d| d.timestamp_millis()).unwrap_or(0);
        let finished_at_ms = job.finished_at.map(|d| d.timestamp_millis()).unwrap_or(0);
        let retries = job.retries;
        let interruptions = job.interruptions;
        // Same reason: the entry is the source of truth `splice_live`
        // writes back from, so it starts out holding whatever the store
        // recorded — otherwise the first persist after boot would clear
        // the column for a job that is still `Failed`.
        let last_error = job.status.error.clone();
        Self {
            job,
            live_phase: AtomicU8::new(encode_phase(phase)),
            counters,
            active_ms: AtomicI64::new(job_active_ms),
            downloading_since_ms: AtomicI64::new(0),
            started_at_ms: AtomicI64::new(started_at_ms),
            finished_at_ms: AtomicI64::new(finished_at_ms),
            retries: AtomicU32::new(retries),
            interruptions: AtomicU32::new(interruptions),
            parts_stale: AtomicBool::new(false),
            verifying: AtomicBool::new(false),
            retrying_parts: std::sync::Mutex::new(std::collections::HashSet::new()),
            parts: std::sync::RwLock::new(IndexMap::new()),
            cancel: std::sync::Mutex::new(CancellationToken::new()),
            running: AtomicBool::new(false),
            last_error: std::sync::RwLock::new(last_error),
            manual_run: AtomicBool::new(false),
            deferred_by_cap: AtomicBool::new(false),
            is_resumable: std::sync::atomic::AtomicI8::new(0),
            captured_response: std::sync::RwLock::new(None),
            session_speed_override: std::sync::atomic::AtomicU64::new(0),
            on_completion: std::sync::RwLock::new(on_completion),
            hashed: std::sync::Mutex::new(None),
            resolver: RwLock::new(None),
            final_path: std::sync::RwLock::new(job_final_path),
            live_controls: odl::progress::LiveControls::new(),
        }
    }

    pub fn phase(&self) -> Phase {
        decode_phase(self.live_phase.load(Ordering::Acquire))
    }

    pub fn set_phase(&self, p: Phase) {
        // Time in `Downloading` is banked as it is left, so every path
        // out of it counts the same: pausing, failing, finishing, or a
        // connection dropping the job back to a wait.
        let was_downloading = self.phase() == Phase::Downloading;
        if p == Phase::Downloading {
            if !was_downloading {
                self.downloading_since_ms.store(now_ms(), Ordering::Release);
            }
        } else if was_downloading {
            let since = self.downloading_since_ms.swap(0, Ordering::AcqRel);
            if since > 0 {
                self.active_ms
                    .fetch_add((now_ms() - since).max(0), Ordering::AcqRel);
            }
        }
        self.live_phase.store(encode_phase(p), Ordering::Release);
    }

    /// Milliseconds this run has actually spent downloading, the
    /// stretch in progress included.
    pub fn active_ms(&self) -> i64 {
        let banked = self.active_ms.load(Ordering::Acquire);
        let since = self.downloading_since_ms.load(Ordering::Acquire);
        if since > 0 {
            banked + (now_ms() - since).max(0)
        } else {
            banked
        }
    }

    /// Clear the per-run stats (started_at / finished_at / retries /
    /// in-flight retrying parts). Called when a job re-enters a clean
    /// pre-run state (restart, cancel-to-queued) so the next run starts
    /// its timing and retry tally from scratch. Set-once within a run,
    /// cleared on re-run (plan W4).
    pub fn reset_run_stats(&self) {
        self.active_ms.store(0, Ordering::Release);
        self.downloading_since_ms.store(0, Ordering::Release);
        self.started_at_ms.store(0, Ordering::Release);
        self.finished_at_ms.store(0, Ordering::Release);
        self.retries.store(0, Ordering::Release);
        self.interruptions.store(0, Ordering::Release);
        if let Ok(mut g) = self.retrying_parts.lock() {
            g.clear();
        }
    }

    /// Zero the live speed counters. Called on pause so the dialog and
    /// queue do not keep showing the last in-flight rate / ETA.
    pub fn reset_live_speed(&self) {
        self.counters.set_speed(0.0);
        if let Ok(parts) = self.parts.read() {
            for p in parts.values() {
                p.speed_bps_bits.store(0u64, Ordering::Relaxed);
            }
        }
    }
}

pub(crate) fn encode_phase(p: Phase) -> u8 {
    match p {
        Phase::Queued => 0,
        Phase::Evaluating => 1,
        Phase::ResolvingConflicts => 2,
        Phase::Downloading => 3,
        Phase::Assembling => 4,
        Phase::Flushing => 5,
        Phase::Verifying => 6,
        Phase::Paused => 7,
        Phase::Completed => 8,
        Phase::Failed => 9,
        Phase::Cancelled => 10,
        Phase::Reconnecting => 11,
        Phase::Conflict => 12,
    }
}

pub(crate) fn decode_phase(v: u8) -> Phase {
    match v {
        1 => Phase::Evaluating,
        2 => Phase::ResolvingConflicts,
        3 => Phase::Downloading,
        4 => Phase::Assembling,
        5 => Phase::Flushing,
        6 => Phase::Verifying,
        7 => Phase::Paused,
        8 => Phase::Completed,
        9 => Phase::Failed,
        10 => Phase::Cancelled,
        11 => Phase::Reconnecting,
        12 => Phase::Conflict,
        _ => Phase::Queued,
    }
}

pub struct AppState {
    store: Store,
    settings: RwLock<Settings>,
    manager: RwLock<Arc<DownloadManager>>,
    jobs: RwLock<IndexMap<JobId, Arc<JobEntry>>>,
    events: broadcast::Sender<DomainEvent>,
    pause_strategy: DynPauseStrategy,
    #[allow(dead_code)]
    update_channel: Arc<dyn UpdateChannel>,
    /// Browser-extension auth token. Loaded from / generated into the DB.
    ext_token: RwLock<String>,
    /// Job whose download dialog is currently visible. UI updates this
    /// whenever its `Signal<Option<JobId>>` changes; the runner reads
    /// it to decide whether conflicts should drive UI dialogs or the
    /// notify-and-park path.
    pub dialog_visible_for: RwLock<Option<JobId>>,
    /// Job ids that should not appear in the user-facing queue list.
    /// Currently used for the self-update artifact download — we
    /// reuse the regular runner + download window for it but want the
    /// queue page and bulk operations to ignore it.
    hidden_jobs: RwLock<std::collections::HashSet<JobId>>,
    /// Fired when a run task ends, whatever its outcome. The shutdown
    /// waits on this instead of polling: a run finishes when it
    /// finishes, and only the run knows when that is.
    run_finished: tokio::sync::Notify,
    /// Held across "count what is running, then claim a slot" so two
    /// starts landing together cannot both see the last free slot.
    admission: tokio::sync::Mutex<()>,
    /// The update being fetched, and the digest its artifact must
    /// have. Cleared when it installs or fails.
    pending_update: RwLock<Option<PendingUpdate>>,
    /// The `oxdm-updater` helper, once it holds a verified artifact and
    /// is waiting to be told to go ahead.
    updater: tokio::sync::Mutex<Option<tokio::process::Child>>,
    /// Set once the daemon has been asked to quit and never cleared:
    /// the process is on its way out, waiting only for whatever cannot
    /// be interrupted. Everything that would start new work checks it,
    /// and a second quit request is a no-op rather than a second
    /// shutdown racing the first.
    exiting: AtomicBool,
    /// Cached id of the built-in Main queue. Resolved once at boot so
    /// `add_job` does not have to round-trip the DB.
    main_queue_id: QueueId,
    /// In-memory cache of every Queue. Authoritative copy is the DB;
    /// this avoids hitting SQLite on every UI refresh.
    queues: RwLock<IndexMap<QueueId, Queue>>,
    /// In-memory cache of host overrides keyed by lowercased host.
    /// Queues currently in "active" state — at least one job has been
    /// started by `start_queue` and no `QueueFinished` event has fired
    /// yet. Used to gate `QueueStarted` / `QueueFinished` emission so
    /// hooks fire exactly once per run. The value tallies *this run's*
    /// outcomes so the finish event can report them; a queue's own job
    /// list cannot, since it also holds results from earlier runs.
    active_queues: RwLock<std::collections::HashMap<QueueId, QueueRunTally>>,
    /// FIFO of unresolved conflict prompts. The conflict window pops
    /// from here; its presence on `AppState` lets a freshly opened
    /// window observe pending items even when the dispatching event
    /// landed before the window subscribed.
    conflict_queue: RwLock<std::collections::VecDeque<(JobId, crate::data::ConflictKind, u64)>>,
    /// Master AES-GCM key for per-job secret encryption. `None` while
    /// the daemon is in the "secrets locked" startup state — the DB
    /// has ciphertext but the OS keyring has no key, so we cannot
    /// decrypt anything until the user acknowledges the wipe.
    master_key: RwLock<Option<crate::data::crypto::MasterKey>>,
    /// Sticky error from the original `Store::open` call. `Some(msg)`
    /// means the on-disk DB could not be opened (file corruption,
    /// incompatible schema, sqlite IO error) and we are running
    /// against an in-memory fallback. The GUI surfaces this via the
    /// `DbStatus` IPC + a recovery modal that offers Exit / Reset.
    db_error: RwLock<Option<String>>,
    /// Something in the store could not be read, but the store itself
    /// is fine: unparsable settings, a job row that would not hydrate.
    /// Deliberately separate from `db_error` — the recovery modal's
    /// only remedy is deleting the database, which is far too much to
    /// offer for one bad row. The GUI raises this as a warning toast.
    db_warning: RwLock<Option<String>>,
    /// The kernel limit that stopped the filesystem watcher, if one
    /// did. Set by `file_watch`, read by the UI's warning dialog, and
    /// cleared the moment a watcher starts.
    watch_limit: RwLock<Option<crate::domain::WatchLimit>>,
    /// Single-slot grace timer for destructive power actions (queue
    /// hooks + per-job completion actions both go through it).
    power: Arc<crate::data::power::PowerGuard>,
    /// Probes in flight and recently finished, keyed by URL.
    ///
    /// The Add dialog probes through the daemon, and the same URL is
    /// usually asked about twice within a second or two — once for the
    /// dialog, once for the job it turns into. One request answers
    /// both.
    probes: tokio::sync::Mutex<std::collections::HashMap<String, ProbeSlot>>,
}

/// A probe's state: running (with everyone waiting on it) or finished
/// (with what it found, until it goes stale).
enum ProbeSlot {
    /// Subscribers get the result the moment the leader has it.
    Running(broadcast::Sender<Arc<Result<ProbeResult, JobError>>>),
    Done {
        at: std::time::Instant,
        result: Arc<Result<ProbeResult, JobError>>,
    },
}

/// How long a finished probe answers for the next caller. Long enough
/// to cover "paste, look at it, press Add", short enough that a link
/// re-added later in the session is asked about again — sizes and
/// signed URLs both go stale.
const PROBE_FRESH_FOR: std::time::Duration = std::time::Duration::from_secs(120);

impl AppState {
    /// Boot oxdm: open the DB, load settings, hydrate the queue, build
    /// an `odl::DownloadManager`. Any I/O failure logs and falls back to
    /// in-memory defaults so the UI still launches.
    pub async fn load() -> Arc<Self> {
        let (store, db_error) = match Store::open(default_db_path()).await {
            Ok(s) => (s, None),
            Err(e) => {
                tracing::error!(error = %e, "failed to open store; running ephemerally");
                // Fall back to an in-memory DB so the IPC layer can still
                // come up and serve the recovery dialog. `db_error`
                // carries the original message — the GUI probes it on
                // boot and surfaces an Exit-or-Reset modal so the user
                // never silently runs against an ephemeral store.
                let s = Store::open(PathBuf::from(":memory:"))
                    .await
                    .expect("memory db");
                (s, Some(e.to_string()))
            }
        };

        // A settings row that will not parse is a read failure, and a
        // read failure must never become a write: `save_settings` is
        // DELETE + re-INSERT, so persisting the defaults on top would
        // destroy the user's folders, proxy, limits and pairing token
        // rather than merely ignoring them for this run.
        let mut db_warning: Option<String> = None;
        let (mut settings, settings_are_readable) = match store.load_settings().await {
            Ok(s) => (s, true),
            Err(e) => {
                tracing::error!(error = %e, "failed to load settings; running on defaults without saving");
                db_warning = Some(format!(
                    "Your settings could not be read ({e}). oxdm is running on defaults \
                     and will not overwrite them."
                ));
                (Settings::default(), false)
            }
        };

        // Generate ext token on first launch and persist it. Token is
        // used by browser extensions to authenticate against the local
        // WebSocket bridge — see `ipc::ws`.
        if settings.ext_token.is_empty() {
            settings.ext_token = generate_token();
            if settings_are_readable && let Err(e) = store.save_settings(&settings).await {
                tracing::warn!(error = %e, "failed to persist generated ext token");
            }
        }

        // Decide secret-encryption mode before loading jobs so the UI
        // can render Locked state immediately on first paint.
        let any_ct = store.any_job_has_ciphertext().await.unwrap_or(false);
        let master_key = match crate::data::crypto::MasterKey::bootstrap(any_ct) {
            Ok(crate::data::crypto::BootOutcome::Ready(k)) => Some(k),
            Ok(crate::data::crypto::BootOutcome::Locked) => {
                tracing::warn!(
                    "master key missing from OS keyring but DB holds encrypted job \
                     secrets — entering Locked mode; the GUI will prompt the user \
                     to wipe ciphertext before any download with secrets can run"
                );
                None
            }
            Err(e) => {
                tracing::error!(error = %e, "crypto bootstrap failed; running without secret encryption");
                None
            }
        };

        // Boot builds the manager before `AppState` exists, so decrypt
        // inline with the key we just bootstrapped.
        let boot_proxy_password = match (&master_key, &settings.enc_proxy_password) {
            (Some(key), Some(blob)) => key
                .decrypt(
                    GLOBAL_SECRET_ID,
                    crate::data::crypto::Field::ProxyPassword,
                    blob,
                )
                .ok()
                .flatten(),
            _ => None,
        };
        let manager = build_manager(&settings, boot_proxy_password.as_deref());
        // A failed read here is not "you have no downloads" — the rows
        // are still on disk. Say so instead of showing an empty list.
        let stored_jobs = match store.list_jobs().await {
            Ok(loaded) => {
                if loaded.skipped > 0 {
                    let msg = format!(
                        "{} download{} could not be read and {} left out of the list.",
                        loaded.skipped,
                        if loaded.skipped == 1 { "" } else { "s" },
                        if loaded.skipped == 1 { "was" } else { "were" },
                    );
                    tracing::error!("{msg}");
                    db_warning = db_warning.or(Some(msg));
                }
                loaded.jobs
            }
            Err(e) => {
                tracing::error!(error = %e, "failed to read the download list");
                db_warning = db_warning.or(Some(format!(
                    "The download list could not be read ({e}). Your downloads are still \
                     on disk — do not add new ones until this is sorted out."
                )));
                Vec::new()
            }
        };
        let mut jobs = IndexMap::new();
        let completion = seeded_completion(&settings);
        for j in stored_jobs {
            // A job left mid-run by a crash arrives here already
            // demoted to Paused — `Store::list_jobs` does it, since no
            // runner survives a restart. That is what keeps a phantom
            // Downloading out of the queue's concurrency count, and
            // what lets the user resume it: the parts and `metadata.pb`
            // are still on disk, so a job caught during assembly
            // assembles and verifies rather than fetching again.
            jobs.insert(
                j.id,
                Arc::new(JobEntry::with_completion(j, completion.clone())),
            );
        }

        let main_queue_id = store
            .main_queue_id()
            .await
            .expect("main queue must exist after migrate()");
        let queue_list = store.list_queues().await.unwrap_or_default();
        let mut queues = IndexMap::new();
        for q in queue_list {
            queues.insert(q.id, q);
        }

        let (tx, _rx) = broadcast::channel(1024);
        let token = settings.ext_token.clone();
        let power = Arc::new(crate::data::power::PowerGuard::new(tx.clone()));

        Arc::new(Self {
            store,
            settings: RwLock::new(settings),
            manager: RwLock::new(Arc::new(manager)),
            jobs: RwLock::new(jobs),
            events: tx,
            pause_strategy: Arc::new(CancelResumeStrategy),
            update_channel: Arc::new(NoopUpdateChannel),
            ext_token: RwLock::new(token),
            dialog_visible_for: RwLock::new(None),
            hidden_jobs: RwLock::new(std::collections::HashSet::new()),
            run_finished: tokio::sync::Notify::new(),
            admission: tokio::sync::Mutex::new(()),
            pending_update: RwLock::new(None),
            updater: tokio::sync::Mutex::new(None),
            exiting: AtomicBool::new(false),
            main_queue_id,
            queues: RwLock::new(queues),
            active_queues: RwLock::new(std::collections::HashMap::new()),
            conflict_queue: RwLock::new(std::collections::VecDeque::new()),
            master_key: RwLock::new(master_key),
            db_error: RwLock::new(db_error),
            db_warning: RwLock::new(db_warning),
            watch_limit: RwLock::new(None),
            power,
            probes: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        })
    }

    /// Claim the shutdown. `false` means one is already under way and
    /// this request should be dropped — the user cannot ask twice, and
    /// two shutdown sequences racing each other is how a half-written
    /// file gets left behind.
    pub fn begin_exit(&self) -> bool {
        !self.exiting.swap(true, Ordering::AcqRel)
    }

    /// Whether the daemon is on its way out. Anything that starts work
    /// — the queue scheduler, a resume, a fresh job — refuses while
    /// this is set.
    pub fn is_exiting(&self) -> bool {
        self.exiting.load(Ordering::Acquire)
    }

    /// Wait for the next run to end.
    ///
    /// Arm it *before* reading the state you are waiting on — a run
    /// that ends in the gap between the two would otherwise notify
    /// nobody and leave the caller waiting for a signal that has
    /// already been sent.
    pub fn run_finished(&self) -> tokio::sync::futures::Notified<'_> {
        self.run_finished.notified()
    }

    /// Jobs currently writing their final file. These are the only
    /// reason a shutdown waits.
    pub async fn assembling_jobs(&self) -> Vec<JobId> {
        self.jobs
            .read()
            .await
            .values()
            .filter(|e| e.phase() == Phase::Assembling)
            .map(|e| e.job.id)
            .collect()
    }

    /// Stop every queue from starting anything else, and pause the jobs
    /// already running. Assembly refuses to pause and is left alone.
    pub async fn halt_for_exit(self: &Arc<Self>) {
        let queues: Vec<QueueId> = self.active_queue_ids().await.into_iter().collect();
        for q in queues {
            let _ = self.stop_queue(q).await;
        }
        self.pause_all().await;
    }

    pub async fn push_conflict(&self, id: JobId, kind: crate::data::ConflictKind, token: u64) {
        self.conflict_queue
            .write()
            .await
            .push_back((id, kind, token));
    }

    pub async fn peek_conflict(&self) -> Option<(JobId, crate::data::ConflictKind, u64)> {
        self.conflict_queue.read().await.front().cloned()
    }

    pub async fn pop_conflict(&self) {
        self.conflict_queue.write().await.pop_front();
    }

    pub async fn conflict_len(&self) -> usize {
        self.conflict_queue.read().await.len()
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn main_queue_id(&self) -> QueueId {
        self.main_queue_id
    }

    // ── queues ───────────────────────────────────────────────────────

    pub async fn queues_snapshot(&self) -> Vec<Queue> {
        self.queues.read().await.values().cloned().collect()
    }

    pub async fn queue(&self, id: QueueId) -> Option<Queue> {
        self.queues.read().await.get(&id).cloned()
    }

    /// Insert or update a queue. Built-in Main queue's `builtin` flag is
    /// preserved regardless of caller intent.
    pub async fn upsert_queue(self: &Arc<Self>, mut queue: Queue) -> Result<(), String> {
        if queue.id == self.main_queue_id {
            queue.builtin = true;
        }
        self.store
            .upsert_queue(&queue)
            .await
            .map_err(|e| e.to_string())?;
        self.queues.write().await.insert(queue.id, queue);
        let _ = self.events.send(DomainEvent::QueuesChanged);
        Ok(())
    }

    pub async fn delete_queue(self: &Arc<Self>, id: QueueId) -> Result<(), String> {
        if id == self.main_queue_id {
            return Err("cannot delete the built-in Main queue".into());
        }
        // Reassign every job that belonged to this queue to Main. Every
        // job must always have a queue; orphaning (queue_id = None) is
        // not allowed. Persist each reassignment before deleting the
        // queue so the FK ON DELETE SET NULL action has no rows to act
        // on, keeping DB + memory consistent.
        let main_id = self.main_queue_id;
        let job_ids: Vec<JobId> = self
            .jobs
            .read()
            .await
            .values()
            .filter(|e| e.job.queue_id == id)
            .map(|e| e.job.id)
            .collect();
        for jid in &job_ids {
            let entry_opt = self.jobs.read().await.get(jid).cloned();
            if let Some(entry) = entry_opt {
                let mut new_job = entry.job.clone();
                new_job.queue_id = main_id;
                self.store
                    .upsert_job(&new_job)
                    .await
                    .map_err(|e| e.to_string())?;
                let new_entry = clone_entry_with_job(&entry, new_job).await;
                self.jobs.write().await.insert(*jid, new_entry);
            }
        }
        self.store
            .delete_queue(id)
            .await
            .map_err(|e| e.to_string())?;
        self.queues.write().await.shift_remove(&id);
        self.active_queues.write().await.remove(&id);
        let _ = self.events.send(DomainEvent::QueuesChanged);
        Ok(())
    }

    /// Move a job to a different queue. Persisted; idempotent.
    pub async fn set_job_queue(
        self: &Arc<Self>,
        id: JobId,
        queue_id: QueueId,
    ) -> Result<(), JobError> {
        if !self.queues.read().await.contains_key(&queue_id) {
            return Err(JobError::Other("queue not found".into()));
        }
        let mut jobs = self.jobs.write().await;
        let Some(old) = jobs.get(&id).cloned() else {
            return Err(JobError::Other("job not found".into()));
        };
        if old.job.queue_id == queue_id {
            return Ok(());
        }
        let mut new_job = old.job.clone();
        new_job.queue_id = queue_id;
        let new_entry = clone_entry_with_job(&old, new_job.clone()).await;
        jobs.insert(id, new_entry);
        drop(jobs);
        self.store
            .upsert_job(&new_job)
            .await
            .map_err(|e| JobError::Io(e.to_string()))?;
        let _ = self.events.send(DomainEvent::JobUpdated {
            id,
            phase: old.phase(),
        });
        Ok(())
    }

    /// Set a job's category explicitly. Persisted; idempotent.
    pub async fn set_job_category(
        self: &Arc<Self>,
        id: JobId,
        category: Category,
    ) -> Result<(), JobError> {
        let mut jobs = self.jobs.write().await;
        let Some(old) = jobs.get(&id).cloned() else {
            return Err(JobError::Other("job not found".into()));
        };
        if old.job.category == category {
            return Ok(());
        }
        let mut new_job = old.job.clone();
        new_job.category = category;
        let new_entry = clone_entry_with_job(&old, new_job.clone()).await;
        jobs.insert(id, new_entry);
        drop(jobs);
        self.store
            .upsert_job(&new_job)
            .await
            .map_err(|e| JobError::Io(e.to_string()))?;
        let _ = self.events.send(DomainEvent::JobUpdated {
            id,
            phase: old.phase(),
        });
        Ok(())
    }

    /// Start every Queued / Paused job in the queue, respecting the
    /// queue's `max_concurrent` cap (falling back to the global setting).
    /// Idempotent: jobs already running stay running.
    pub async fn start_queue(self: &Arc<Self>, id: QueueId) -> Result<(), String> {
        let queue = self
            .queue(id)
            .await
            .ok_or_else(|| "queue not found".to_string())?;
        let global = self.settings.read().await.max_concurrent_downloads;
        let cap = queue.max_concurrent.unwrap_or(global).max(1);

        // Same rule as `resume_all`: a failed integrity check is not
        // work this queue can carry on with, it is a file the user has
        // to decide about.
        let snapshot: Vec<(JobId, Phase)> = self
            .jobs
            .read()
            .await
            .values()
            .filter(|e| e.job.queue_id == id && !e.job.integrity_failed())
            .map(|e| (e.job.id, e.phase()))
            .collect();
        let running_now = snapshot.iter().filter(|(_, p)| p.is_running()).count();

        // A queue is asked about as a whole: its downloads land on the
        // same disks one after another, and starting the first two of
        // ten only to run out on the third helps nobody. Jobs whose
        // size nobody knows sit this out — see `data::space`.
        let work_dir = self.settings.read().await.work_dir.clone();
        let needs: Vec<crate::data::space::Need> = {
            let jobs = self.jobs.read().await;
            snapshot
                .iter()
                .filter(|(_, p)| p.is_startable())
                .filter_map(|(jid, _)| jobs.get(jid))
                .map(|e| self.space_need(e, &work_dir))
                .collect()
        };
        self.refuse_if_short_on_space(needs)
            .await
            .map_err(|e| e.to_string())?;

        // The run is declared before the first job starts, not after the
        // loop: a job that reaches its epilogue while the loop is still
        // going asks whether its queue is running, and a "no" there
        // stopped the queue feeding itself for good. Rolled back below
        // if nothing could be started at all.
        //
        // A fresh run gets a fresh tally, so what the finish
        // notification reports is this run — but only when the queue was
        // not already running, or restarting a running queue would
        // discard the counts it has accumulated.
        let was_running = {
            let mut active = self.active_queues.write().await;
            match active.get_mut(&id) {
                Some(tally) if tally.queue_run => true,
                // A hand-started download in this queue may have opened
                // a tally already; this run takes it over rather than
                // dropping the outcome it is holding.
                Some(tally) => {
                    tally.queue_run = true;
                    false
                }
                None => {
                    active.insert(
                        id,
                        QueueRunTally {
                            queue_run: true,
                            ..QueueRunTally::default()
                        },
                    );
                    false
                }
            }
        };

        let mut budget = cap.saturating_sub(running_now);
        let mut started_any = false;
        for (jid, phase) in snapshot {
            if budget == 0 {
                break;
            }
            // Only what this queue actually starts is marked as the
            // queue's: a download already running because the user
            // pressed Resume stays theirs, and a later Stop queue
            // leaves it alone.
            if !phase.is_startable() {
                continue;
            }
            self.mark_run_intent(jid, false).await;
            match self.start_job(jid).await {
                Ok(()) => {
                    started_any = true;
                    budget -= 1;
                }
                // Every slot is busy. The job is queued and starts when
                // one frees, so the queue run is real either way.
                Err(JobError::Deferred) => started_any = true,
                Err(_) => {}
            }
        }

        if !started_any {
            // Nothing to run: undo the declaration rather than leaving a
            // queue that shows Stop with nothing to stop.
            if !was_running {
                self.active_queues.write().await.remove(&id);
            }
            return Ok(());
        }
        if !was_running {
            let _ = self.events.send(DomainEvent::QueueStarted { id });
        }
        Ok(())
    }

    /// True when the queue itself is running: someone pressed Start
    /// queue, or the scheduler did, and the run has not ended.
    ///
    /// A download the user started by hand does not put its queue in
    /// this state. It joins the tally so its outcome is counted, but
    /// "this one download is running" and "the queue is working
    /// through its list" are different things, and the toolbar offers
    /// Stop queue on the strength of the second.
    pub async fn is_queue_active(&self, id: QueueId) -> bool {
        self.active_queues
            .read()
            .await
            .get(&id)
            .is_some_and(|t| t.queue_run)
    }

    /// Snapshot of currently running queues. Same rule as
    /// [`Self::is_queue_active`].
    pub async fn active_queue_ids(&self) -> std::collections::HashSet<QueueId> {
        self.active_queues
            .read()
            .await
            .iter()
            .filter(|(_, t)| t.queue_run)
            .map(|(id, _)| *id)
            .collect()
    }

    /// End the queue's run: pause what the queue started, and leave
    /// what the user started alone.
    pub async fn stop_queue(self: &Arc<Self>, id: QueueId) -> Result<(), String> {
        let ids = queue_stop_targets(&*self.jobs.read().await, id);
        for jid in ids {
            let _ = self.pause(jid).await;
        }
        // The run ends, but the tally entry stays as long as a
        // hand-started download in this queue is still going: it is
        // what counts that download's outcome and, once it ends, lets
        // the finish watcher close the entry.
        let manual_running = self.jobs.read().await.values().any(|e| {
            e.job.queue_id == id && e.phase().is_running() && e.manual_run.load(Ordering::Acquire)
        });
        let mut active = self.active_queues.write().await;
        match active.get_mut(&id) {
            Some(tally) if manual_running => tally.queue_run = false,
            Some(_) => {
                active.remove(&id);
            }
            None => return Ok(()),
        }
        drop(active);
        // Stopped, not finished: on-finish hooks belong to a queue that
        // ran out of work, and arming a shutdown because someone
        // pressed Stop queue is a decision oxdm does not get to make.
        let _ = self.events.send(DomainEvent::QueueStopped { id });
        Ok(())
    }

    /// Start queued jobs in `queue_id` until its slots are full.
    ///
    /// Two caps apply and both are real: the queue's own
    /// `max_concurrent`, and the global `max_concurrent_downloads`
    /// across every queue — a per-queue limit that let three queues run
    /// three each would be a setting that means nothing.
    ///
    /// Only for a queue that is actually running. A queue nobody
    /// started has no slots to fill, and a job finishing in it (started
    /// by hand) must not quietly start its neighbours.
    ///
    /// Boxed because this is a cycle: filling a slot starts a job, and
    /// that job finishing fills the next. An `async fn` calling itself
    /// through that loop has a future whose type contains itself, which
    /// the compiler cannot size — or prove `Send`.
    fn fill_queue_slots(
        self: &Arc<Self>,
        queue_id: QueueId,
        // The job whose ending freed the slot. It is skipped for this
        // pass: a user pausing the only running download in a queue
        // would otherwise watch the queue start it again a moment
        // later. The queue comes back to it on the next pass, which is
        // what "paused, not failed" is supposed to mean.
        just_ended: Option<JobId>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        let state = Arc::clone(self);
        Box::pin(async move {
            let (queue_run, failed_now) = {
                let active = state.active_queues.read().await;
                match active.get(&queue_id) {
                    Some(t) => (t.queue_run, t.failed_now.clone()),
                    None => (false, std::collections::HashSet::new()),
                }
            };
            if state.is_exiting() || !queue_run {
                return;
            }
            let global_cap = state.settings().await.max_concurrent_downloads.max(1);
            let queue_cap = state
                .queue(queue_id)
                .await
                .and_then(|q| q.max_concurrent)
                .unwrap_or(global_cap)
                .max(1);
            // A job that refuses to start — out of disk, secrets locked
            // — stays queued, so without this the loop would keep
            // picking the same one.
            let mut tried: std::collections::HashSet<JobId> = just_ended.into_iter().collect();
            loop {
                let next = {
                    let jobs = state.jobs.read().await;
                    let running_global = jobs.values().filter(|e| e.phase().is_running()).count();
                    let running_here = jobs
                        .values()
                        .filter(|e| e.job.queue_id == queue_id && e.phase().is_running())
                        .count();
                    if running_global >= global_cap || running_here >= queue_cap {
                        return;
                    }
                    // Queued and paused always; failed only if the
                    // failure is older than this run. A pause is "not
                    // now" and the queue comes back to it. A failure
                    // from this run has a reason that is still true, and
                    // retrying it every pass would spin the queue on one
                    // broken download forever — but a queue full of
                    // failures from *yesterday* is exactly what Start
                    // queue was pressed for, and stopping after the
                    // first of them helps nobody.
                    jobs.values()
                        .find(|e| {
                            let phase = e.phase();
                            let eligible = match phase {
                                Phase::Queued | Phase::Paused => true,
                                Phase::Failed => !failed_now.contains(&e.job.id),
                                _ => false,
                            };
                            e.job.queue_id == queue_id
                                && eligible
                                && !e.job.integrity_failed()
                                && !tried.contains(&e.job.id)
                        })
                        .map(|e| e.job.id)
                };
                let Some(id) = next else {
                    return;
                };
                tried.insert(id);
                state.mark_run_intent(id, false).await;
                if let Err(e) = state.start_job(id).await {
                    tracing::info!(id = %id, error = %e, "queue could not start the next download");
                }
            }
        })
    }

    /// Watcher: after a job leaves running state, if its queue has no
    /// more running or queued jobs, close the run. Called from the
    /// runner outcome handler.
    ///
    /// `QueueFinished` — and with it the on-finish hooks, which can
    /// shut the machine down — only for a queue that was running. A
    /// download the user started by hand is one download: the last one
    /// in the queue finishing means their download finished, not that
    /// the queue worked through its list.
    async fn maybe_finish_queue(self: &Arc<Self>, queue_id: QueueId) {
        let still_busy = self.jobs.read().await.values().any(|e| {
            e.job.queue_id == queue_id
                && (e.running.load(Ordering::Acquire)
                    || matches!(e.phase(), Phase::Queued | Phase::Evaluating))
        });
        if still_busy {
            return;
        }
        let mut active = self.active_queues.write().await;
        let Some(tally) = active.remove(&queue_id) else {
            return;
        };
        drop(active);
        if !tally.queue_run {
            return;
        }
        let _ = self.events.send(DomainEvent::QueueFinished {
            id: queue_id,
            completed: tally.completed,
            failed: tally.failed,
            needs_answer: tally.needs_answer,
        });
    }

    /// Record one job's terminal outcome against its queue's current
    /// run. Called before the finish watcher runs, so the job that
    /// drains the queue is counted in the event it triggers.
    async fn tally_queue_outcome(&self, queue_id: QueueId, id: JobId, outcome: JobOutcome) {
        let mut active = self.active_queues.write().await;
        let Some(tally) = active.get_mut(&queue_id) else {
            return; // job outside a queue run (single Start of a paused job)
        };
        match outcome {
            JobOutcome::Completed => {
                tally.completed += 1;
                tally.failed_now.remove(&id);
            }
            JobOutcome::Failed => {
                tally.failed += 1;
                tally.failed_now.insert(id);
            }
            // Counted apart from failures, and left out of
            // `failed_now`: the queue does not retry it this run — the
            // question is still unanswered — but it is not something
            // that went wrong either.
            JobOutcome::NeedsAnswer => {
                tally.needs_answer += 1;
                tally.failed_now.insert(id);
            }
            JobOutcome::Cancelled => {
                tally.failed_now.remove(&id);
            }
        }
    }

    /// Queue + start a hidden artifact download for self-update. Reuses
    /// every piece of regular download machinery (multi-part fetcher,
    /// progress bar, pause / cancel) but stays out of the queue list.
    /// Caller is expected to subscribe to `DomainEvent::JobCompleted`
    /// to learn the final artifact path, then hand it to the updater
    /// helper for verification + swap + relaunch.
    pub async fn add_update_job(
        self: &Arc<Self>,
        info: crate::data::UpdateInfo,
    ) -> Result<JobId, JobError> {
        // Checked before a byte is fetched: an artifact nobody can
        // verify is one that must never reach the swap, and finding
        // that out after the download has run is finding it out too
        // late to mean anything.
        if !is_sha256_hex(&info.sha256) {
            return Err(JobError::Other(
                "this update has no usable SHA-256, so it cannot be verified".into(),
            ));
        }
        if info.url.scheme() != "https" {
            return Err(JobError::Other(
                "updates are only fetched over https".into(),
            ));
        }
        let save_dir = update_staging_dir().map_err(|e| JobError::Io(e.to_string()))?;
        let suggested_filename = Some(format!("oxdm-update-{}", info.version));
        let url = info.url.clone();
        let id = self
            .add_job(
                url,
                save_dir,
                suggested_filename,
                None,
                indexmap::IndexMap::new(),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                // The updater probes nothing up front.
                ProbeFacts::default(),
            )
            .await?;
        self.hidden_jobs.write().await.insert(id);
        // Remembered so the completion can be recognised: the digest to
        // check the artifact against belongs to this download and no
        // other.
        *self.pending_update.write().await = Some(PendingUpdate { job: id, info });
        // Aimed at by a person pressing a button, so it runs now rather
        // than queueing behind the download list.
        self.mark_run_intent(id, true).await;
        self.start_job(id).await?;
        Ok(id)
    }

    /// The update download in flight, if there is one.
    pub async fn pending_update(&self) -> Option<PendingUpdate> {
        self.pending_update.read().await.clone()
    }

    /// Hand the finished artifact to `oxdm-updater`, which verifies it
    /// and reports back.
    ///
    /// Nothing is replaced here: the helper stops at `ready` and waits
    /// for [`Self::install_update`]. The daemon only learns whether the
    /// bytes it fetched are the bytes the feed promised.
    pub async fn stage_update(self: &Arc<Self>, artifact: std::path::PathBuf) {
        let Some(pending) = self.pending_update().await else {
            return;
        };
        let running = match crate::platform::current_exe() {
            Ok(p) => p,
            Err(e) => {
                self.fail_update(format!("cannot find the running oxdm: {e}"))
                    .await;
                return;
            }
        };
        // What gets replaced. Inside an AppImage, `current_exe` is a
        // path in a read-only mount that vanishes when the app exits —
        // replacing it would update nothing. The bundle is the file the
        // user launched and the file the feed's artifact is a new
        // version of.
        let exe = crate::data::update_channel::running_as_appimage().unwrap_or(running.clone());
        // The helper, on the other hand, is found beside the *running*
        // binary: inside the bundle when that is where we came from.
        let updater = running.with_file_name(if cfg!(windows) {
            "oxdm-updater.exe"
        } else {
            "oxdm-updater"
        });
        if !updater.exists() {
            self.fail_update(format!(
                "the updater helper is missing from this install ({})",
                updater.display()
            ))
            .await;
            return;
        }

        let mut child = match tokio::process::Command::new(&updater)
            .arg("--exe")
            .arg(&exe)
            .arg("--pid")
            .arg(std::process::id().to_string())
            .arg("--artifact")
            .arg(&artifact)
            .arg("--sha256")
            .arg(&pending.info.sha256)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                self.fail_update(format!("could not run the updater: {e}"))
                    .await;
                return;
            }
        };

        let stdout = child.stdout.take();
        *self.updater.lock().await = Some(child);
        let Some(stdout) = stdout else {
            self.fail_update("the updater said nothing".into()).await;
            return;
        };

        let state = Arc::clone(self);
        let version = pending.info.version.clone();
        tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let mut lines = tokio::io::BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(ev) = serde_json::from_str::<crate::data::UpdaterEvent>(&line) else {
                    continue;
                };
                match ev {
                    // Verified and waiting: this is the only point at
                    // which offering to install is honest.
                    crate::data::UpdaterEvent::Ready => {
                        let _ = state.events.send(DomainEvent::UpdateStaged {
                            version: version.clone(),
                        });
                    }
                    crate::data::UpdaterEvent::Error { message } => {
                        state.fail_update(message).await;
                        return;
                    }
                    _ => {}
                }
            }
        });
    }

    /// Greenlight the swap: the helper replaces the executable once
    /// this process is gone, then relaunches it.
    pub async fn install_update(self: &Arc<Self>) -> Result<(), String> {
        use tokio::io::AsyncWriteExt;
        let mut guard = self.updater.lock().await;
        let child = guard
            .as_mut()
            .ok_or_else(|| "no update is ready to install".to_string())?;
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| "the updater is not listening".to_string())?;
        stdin
            .write_all(b"go\n")
            .await
            .map_err(|e| format!("could not tell the updater to go ahead: {e}"))?;
        stdin
            .flush()
            .await
            .map_err(|e| format!("could not tell the updater to go ahead: {e}"))?;
        Ok(())
    }

    async fn fail_update(&self, message: String) {
        tracing::warn!(%message, "update did not install");
        *self.pending_update.write().await = None;
        let _ = self.events.send(DomainEvent::UpdateFailed { message });
    }

    pub async fn is_hidden(&self, id: JobId) -> bool {
        self.hidden_jobs.read().await.contains(&id)
    }

    /// Update the session-only per-job speed cap. `None` clears it.
    pub async fn set_session_speed_limit(
        &self,
        id: JobId,
        bps: Option<u64>,
    ) -> Result<(), JobError> {
        let entry = self
            .job_entry(id)
            .await
            .ok_or_else(|| JobError::Other("job not found".into()))?;
        entry
            .session_speed_override
            .store(bps.unwrap_or(0), Ordering::Release);
        Ok(())
    }

    /// Persist (or clear) the per-job speed cap on the `Job` itself
    /// so it survives restarts. Mirrors IDM's "Remember settings for
    /// this file" checkbox.
    pub async fn set_persistent_speed_limit(
        &self,
        id: JobId,
        bps: Option<u64>,
    ) -> Result<(), JobError> {
        // `JobEntry::job` is owned by an `Arc<JobEntry>`; we can't
        // mutate it in place because clones are shared with active
        // runners. Instead rebuild the entry, transferring every
        // sticky field (counters, cancel token, parts map, …) and
        // swap it into the IndexMap.
        let mut jobs = self.jobs.write().await;
        let Some(old) = jobs.get(&id).cloned() else {
            return Err(JobError::Other("job not found".into()));
        };
        let mut new_job = old.job.clone();
        new_job.speed_limit_override = bps;

        let new_entry = clone_entry_with_job(&old, new_job.clone()).await;
        jobs.insert(id, new_entry);
        drop(jobs);

        self.store
            .upsert_job(&new_job)
            .await
            .map_err(|e| JobError::Io(e.to_string()))?;
        Ok(())
    }

    /// Update per-job parallel-connection cap. `None` clears the
    /// override (job inherits global). Values above 16 are rejected;
    /// past that point throughput stops scaling and the file just
    /// thrashes the server.
    pub async fn set_max_connections(&self, id: JobId, n: Option<u64>) -> Result<(), JobError> {
        if let Some(v) = n
            && v > 16
        {
            return Err(JobError::Other("max connections capped at 16".into()));
        }
        let mut jobs = self.jobs.write().await;
        let Some(old) = jobs.get(&id).cloned() else {
            return Err(JobError::Other("job not found".into()));
        };
        let mut new_job = old.job.clone();
        new_job.max_connections = n;
        let new_entry = clone_entry_with_job(&old, new_job.clone()).await;
        // Push the change into the live odl run loop (no-op if the job
        // is not currently running — odl will pick the value up the
        // next time it starts). `None` falls back to the global default
        // baked into the manager's config.
        let target = n.map(|v| v as usize).unwrap_or_else(|| {
            // Mirror odl's own behaviour: 0 = unset, downloader will
            // re-seed from the per-job options on the next iteration.
            0
        });
        if target > 0 {
            new_entry.live_controls.set_max_connections(target);
        }
        jobs.insert(id, new_entry);
        drop(jobs);
        self.store
            .upsert_job(&new_job)
            .await
            .map_err(|e| JobError::Io(e.to_string()))?;
        Ok(())
    }

    /// Persist the per-job Advanced bundle (Properties dialog →
    /// Advanced / Connection / Cookies / Headers tabs). Stores the
    /// blob as JSON on the `jobs` row and updates the in-memory entry.
    ///
    /// Secrets never land in `advanced_json` (guardian F1): the
    /// proxy password, Basic password and Bearer token are stripped
    /// from the blob and re-routed onto the encrypted columns — same
    /// rails as `add_from_capture`'s Authorization extraction. The
    /// Basic username rides the legacy `Job.auth_user` field, which
    /// stays the single source of truth the runner reads (F2);
    /// `advanced.auth` keeps only the scheme selection. Empty incoming
    /// secret fields leave the stored ciphertext untouched, so an
    /// Apply with untouched (blank) password boxes cannot silently
    /// wipe stored secrets.
    pub async fn set_job_advanced(
        &self,
        id: JobId,
        mut advanced: crate::domain::Advanced,
    ) -> Result<(), JobError> {
        use crate::domain::AuthScheme;
        let proxy_password = std::mem::take(&mut advanced.proxy.password);
        // Consumed here: a persisted `true` would re-clear the secret on
        // every later Apply that never touched the field.
        let clear_proxy_password = std::mem::take(&mut advanced.proxy.clear_password);
        let auth_username = std::mem::take(&mut advanced.auth.username);
        let auth_password = std::mem::take(&mut advanced.auth.password);
        let auth_token = std::mem::take(&mut advanced.auth.token);
        let clear_auth_secret = std::mem::take(&mut advanced.auth.clear_secret);
        // Cookie text is a secret too — never persisted in the blob;
        // routed onto `enc_cookies` like the passwords above.
        let cookie_jar = std::mem::take(&mut advanced.cookie_jar);
        let clear_cookie_jar = std::mem::take(&mut advanced.clear_cookie_jar);

        // Encrypt before taking the jobs lock — `encrypt_field` awaits
        // on the master key and must not run under the registry lock.
        let enc_proxy_password = if proxy_password.is_empty() {
            None
        } else {
            self.encrypt_field(
                id,
                crate::data::crypto::Field::ProxyPassword,
                Some(&proxy_password),
            )
            .await?
        };
        let auth_secret = match advanced.auth.scheme {
            AuthScheme::Basic => auth_password,
            AuthScheme::Bearer => auth_token,
            AuthScheme::None | AuthScheme::Digest => String::new(),
        };
        let enc_auth_secret = if auth_secret.is_empty() {
            None
        } else {
            self.encrypt_field(
                id,
                crate::data::crypto::Field::AuthPassword,
                Some(&auth_secret),
            )
            .await?
        };
        let enc_cookie_jar = if cookie_jar.trim().is_empty() {
            None
        } else {
            self.encrypt_field(id, crate::data::crypto::Field::Cookies, Some(&cookie_jar))
                .await?
        };

        let mut jobs = self.jobs.write().await;
        let Some(old) = jobs.get(&id).cloned() else {
            return Err(JobError::Other("job not found".into()));
        };
        let mut new_job = old.job.clone();
        new_job.advanced = advanced;
        if let Some(enc) = enc_proxy_password {
            new_job.enc_proxy_password = Some(enc);
        } else if clear_proxy_password {
            new_job.enc_proxy_password = None;
        }
        if let Some(enc) = enc_auth_secret {
            new_job.enc_auth_password = Some(enc);
        } else if clear_auth_secret {
            new_job.enc_auth_password = None;
        }
        if let Some(enc) = enc_cookie_jar {
            new_job.enc_cookies = Some(enc);
        } else if clear_cookie_jar {
            new_job.enc_cookies = None;
        }
        match new_job.advanced.auth.scheme {
            AuthScheme::Basic if !auth_username.is_empty() => {
                new_job.auth_user = Some(auth_username);
            }
            // Scheme "None" must actually stop Basic credentials from
            // being sent: the runner builds them off `auth_user`, so
            // clearing it is what makes the selection honest (F2/F4).
            // The stored secret goes with it — without `auth_user` it
            // could never be used again, and keeping the ciphertext
            // leaves a secret at rest with no UI left to remove it.
            AuthScheme::None => {
                new_job.auth_user = None;
                new_job.enc_auth_password = None;
            }
            _ => {}
        }
        let new_entry = clone_entry_with_job(&old, new_job.clone()).await;
        jobs.insert(id, new_entry);
        drop(jobs);
        self.store
            .upsert_job(&new_job)
            .await
            .map_err(|e| JobError::Io(e.to_string()))?;
        Ok(())
    }

    /// Persist the per-job checksum list (Properties dialog →
    /// Checksums tab).
    /// Replace the job's checksum list.
    ///
    /// The caller owns *which* hashes exist; the daemon owns what was
    /// proven about them. So a row that survives the edit keeps its
    /// verdict, matched on algorithm and value: a window sends the list
    /// it last hydrated, and adding one hash to a stale copy would
    /// otherwise reset every other row to unverified — throwing away a
    /// check that actually happened.
    pub async fn set_job_checksums(
        &self,
        id: JobId,
        checksums: Vec<crate::domain::Checksum>,
    ) -> Result<(), JobError> {
        let mut jobs = self.jobs.write().await;
        let Some(old) = jobs.get(&id).cloned() else {
            return Err(JobError::Other("job not found".into()));
        };
        let mut checksums = checksums;
        for c in &mut checksums {
            if let Some(known) = old
                .job
                .checksums
                .iter()
                .find(|k| k.algo == c.algo && k.hash.eq_ignore_ascii_case(&c.hash))
            {
                c.status = known.status;
                c.expected = known.expected.clone();
            }
        }
        let mut new_job = old.job.clone();
        new_job.checksums = checksums;
        let new_entry = clone_entry_with_job(&old, new_job).await;
        clear_settled_mismatch(&new_entry);
        jobs.insert(id, new_entry);
        drop(jobs);
        // Through `persist_job` rather than the job built above: the
        // phase and the error were just edited on the entry, and the
        // splice is what carries them to the store.
        self.persist_job(id).await;
        let phase = self.job_entry(id).await.map(|e| e.phase());
        if let Some(phase) = phase {
            let _ = self.events.send(DomainEvent::JobUpdated { id, phase });
        }
        Ok(())
    }

    /// Record checksums the server advertised, keeping every row the
    /// job already carries.
    ///
    /// Unlike `set_job_checksums` this is additive: the daemon learns
    /// these from a response header, not from a user editing a list,
    /// and must not drop hashes typed in the Properties dialog.
    pub async fn merge_server_checksums(&self, id: JobId, checksums: Vec<crate::domain::Checksum>) {
        let mut jobs = self.jobs.write().await;
        let Some(old) = jobs.get(&id).cloned() else {
            return;
        };
        let mut new_job = old.job.clone();
        if !crate::data::mapping::merge_checksums(&mut new_job.checksums, checksums) {
            return;
        }
        let phase = old.phase();
        let new_entry = clone_entry_with_job(&old, new_job.clone()).await;
        jobs.insert(id, new_entry);
        drop(jobs);
        if let Err(e) = self.store.upsert_job(&new_job).await {
            tracing::warn!(id = %id, error = %e, "could not store server checksums");
        }
        let _ = self.events.send(DomainEvent::JobUpdated { id, phase });
    }

    /// Where a job should be saved once its category is known, or
    /// `None` to leave it where it is.
    ///
    /// Only a folder the app chose is moved. A job sitting in some
    /// category's default folder was routed there by a guess at its
    /// name; a job sitting anywhere else is sitting where the user put
    /// it, and no classification outranks that.
    fn retarget_dir(
        settings: &Settings,
        current: &std::path::Path,
        category: Category,
    ) -> Option<std::path::PathBuf> {
        let wanted = settings.category_folder(category);
        if wanted == current {
            return None;
        }
        let app_chose_it = Category::ALL_ASSIGNABLE
            .iter()
            .any(|c| settings.category_folder(*c) == current);
        app_chose_it.then_some(wanted)
    }

    /// Record the name a run resolved for a job that was added without
    /// one, and classify it now that there is something to classify.
    ///
    /// Only fills a blank: a name the user typed is theirs, and a
    /// second run of a renamed job must not undo the rename.
    ///
    /// The category follows the name but the folder does not. The bytes
    /// are already being written to the folder chosen when the job was
    /// added, and a category is what a file *is* — moving a file behind
    /// the user's back to make the two agree is the worse answer.
    pub async fn apply_resolved_filename(
        &self,
        id: JobId,
        filename: String,
    ) -> Option<std::path::PathBuf> {
        // odl took this from the server's `Content-Disposition` or the
        // URL; it is a suggestion, not a path.
        let name = crate::domain::filename::sanitize(&filename)?;
        let name = name.as_str();
        let settings = self.settings.read().await.clone();
        let mut jobs = self.jobs.write().await;
        let old = jobs.get(&id).cloned()?;
        if old.job.filename.as_deref().is_some_and(|n| !n.is_empty()) {
            return None;
        }
        let mut new_job = old.job.clone();
        // The run learned a name the table may already hold — two
        // links to `setup.exe` are the common case. Numbering it here
        // is the last point where it can be done without a user
        // waiting on an answer.
        new_job.filename = Some(free_name(&jobs, name, Some(id)));
        let mut moved_to = None;
        let stored_name = new_job.filename.clone().unwrap_or_default();
        if new_job.category == Category::Other {
            new_job.category = classify(&stored_name, &settings.category_extensions);
            // The folder follows the category while it still can. The
            // parts live in the per-job work dir and the file is
            // assembled at the end, so this run's destination is still
            // a decision rather than a fact — and the caller applies it
            // to the instruction before the download starts.
            if let Some(dir) = Self::retarget_dir(&settings, &new_job.save_dir, new_job.category) {
                new_job.save_dir = dir.clone();
                moved_to = Some(dir);
            }
        }
        let phase = old.phase();
        let new_entry = clone_entry_with_job(&old, new_job.clone()).await;
        jobs.insert(id, new_entry);
        drop(jobs);
        if let Err(e) = self.store.upsert_job(&new_job).await {
            tracing::warn!(id = %id, error = %e, "could not store the resolved filename");
        }
        let _ = self.events.send(DomainEvent::JobFilenameResolved {
            id,
            filename: stored_name,
        });
        let _ = self.events.send(DomainEvent::JobUpdated { id, phase });
        moved_to
    }

    /// Replace only the source URL + destination (save_dir + filename) of
    /// a job. Refused while the job is running — the Properties UI only
    /// offers these fields in paused/queued/cancelled/failed states, and
    /// mutating the destination mid-transfer would strand the partial
    /// file. Unlike `update_job_location` this leaves headers, proxy, auth
    /// and cookies untouched, so a URL edit can't wipe stored secrets.
    pub async fn set_job_source(
        self: &Arc<Self>,
        id: JobId,
        url: url::Url,
        save_dir: std::path::PathBuf,
        filename: Option<String>,
    ) -> Result<(), JobError> {
        let mut jobs = self.jobs.write().await;
        let Some(old) = jobs.get(&id).cloned() else {
            return Err(JobError::Other("job not found".into()));
        };
        if old.phase().is_running() {
            return Err(JobError::Other(
                "cannot change source while the download is running".into(),
            ));
        }
        let filename = filename.and_then(|n| crate::domain::filename::sanitize(&n));
        if let Some(name) = filename.as_deref().filter(|n| !n.trim().is_empty())
            && name_is_taken(&jobs, name, Some(id))
        {
            return Err(JobError::NameTaken {
                filename: name.trim().to_owned(),
            });
        }
        let mut new_job = old.job.clone();
        new_job.url = url;
        new_job.save_dir = save_dir;
        new_job.filename = filename;
        let new_entry = clone_entry_with_job(&old, new_job.clone()).await;
        jobs.insert(id, new_entry);
        drop(jobs);
        self.store
            .upsert_job(&new_job)
            .await
            .map_err(|e| JobError::Io(e.to_string()))?;
        let _ = self.events.send(DomainEvent::JobUpdated {
            id,
            phase: old.phase(),
        });
        Ok(())
    }

    /// Update per-job completion actions. UI binds the OnCompletion
    /// tab to this.
    pub async fn set_on_completion(
        &self,
        id: JobId,
        new: crate::domain::OnCompletion,
    ) -> Result<(), JobError> {
        let entry = self
            .job_entry(id)
            .await
            .ok_or_else(|| JobError::Other("job not found".into()))?;
        if let Ok(mut g) = entry.on_completion.write() {
            *g = new;
        }
        Ok(())
    }

    /// Called by the UI whenever the active download dialog changes.
    pub async fn set_dialog_visible_for(&self, id: Option<JobId>) {
        *self.dialog_visible_for.write().await = id;
    }

    /// Put a queue's pending downloads in the order the user just gave.
    ///
    /// Order is all this touches. Nothing is started, nothing is
    /// paused: a queue already running keeps running exactly what it
    /// was running, and the new order decides what goes next when a
    /// slot comes free.
    ///
    /// Ids not in `queue` are ignored, and jobs in the queue the caller
    /// left out keep their place after the listed ones — the window
    /// lists only what is waiting, and a download that finished while
    /// the user was dragging must not be dropped from the order.
    pub async fn reorder_queue(self: &Arc<Self>, queue: QueueId, ids: Vec<JobId>) {
        let ordered: Vec<JobId> = {
            let jobs = self.jobs.read().await;
            let mut seen = std::collections::HashSet::new();
            let mut out: Vec<JobId> = ids
                .into_iter()
                .filter(|id| {
                    jobs.get(id).is_some_and(|e| e.job.queue_id == queue) && seen.insert(*id)
                })
                .collect();
            out.extend(
                jobs.values()
                    .filter(|e| e.job.queue_id == queue && !seen.contains(&e.job.id))
                    .map(|e| e.job.id),
            );
            out
        };
        if ordered.is_empty() {
            return;
        }
        {
            // Re-inserting moves a key to the back of the map, so
            // walking the new order front to back leaves the queue's
            // jobs in exactly that order behind everyone else's.
            let mut jobs = self.jobs.write().await;
            for id in &ordered {
                if let Some(entry) = jobs.shift_remove(id) {
                    jobs.insert(*id, entry);
                }
            }
        }
        if let Err(e) = self.store.set_queue_order(&ordered).await {
            tracing::warn!(queue = %queue, error = %e, "could not store the queue order");
        }
        let _ = self.events.send(DomainEvent::QueuesChanged);
    }

    /// Send a job to the back of its queue.
    ///
    /// A download that stops — the user paused it, or it failed — is no
    /// longer the one the queue is working on, and leaving it at the
    /// front means everything behind it waits for a decision nobody is
    /// making. At the back it is still there, still resumable, and the
    /// queue carries on with what it can do.
    ///
    /// Both halves matter: the in-memory order is what every window
    /// lists, and the stored `queue_position` is what survives a
    /// restart.
    async fn move_to_queue_end(&self, id: JobId) {
        {
            let mut jobs = self.jobs.write().await;
            if let Some(entry) = jobs.shift_remove(&id) {
                jobs.insert(id, entry);
            }
        }
        if let Err(e) = self.store.move_job_to_end(id).await {
            tracing::warn!(id = %id, error = %e, "could not record the new queue position");
        }
    }

    /// Park a job at the end of the queue, mark it `Failed` with a
    /// `ConflictPending` payload, and send a notification. Used by the
    /// runner when a conflict comes up and the job's dialog is not the
    /// window on screen to host the question.
    ///
    /// "No auto-retry" is implicit: oxdm never auto-retries after a
    /// terminal phase. The user explicitly Resumes from the queue row.
    pub async fn park_with_conflict(self: &Arc<Self>, id: JobId, cause: JobError) {
        self.move_to_queue_end(id).await;
        let err = JobError::ConflictPending(Box::new(cause));
        if let Some(entry) = self.jobs.read().await.get(&id) {
            entry.set_phase(Phase::Conflict);
            entry.reset_live_speed();
            if let Ok(mut g) = entry.last_error.write() {
                *g = Some(err.clone());
            }
        }
        // Written down: the caller persisted the job as `Failed` a
        // moment ago, and a restart in between would lose the fact that
        // this one is waiting on a person rather than broken.
        self.persist_job(id).await;
        let _ = self.events.send(DomainEvent::JobFailed { id, error: err });
    }

    /// A fresh pairing code, minted and handed back — *not* stored.
    ///
    /// Saving it here would unpair the extension the instant the button
    /// was pressed, before the user has copied the new code or decided
    /// to keep it. It becomes real when the settings that carry it are
    /// saved, like every other field on that page.
    pub fn mint_ext_token(&self) -> String {
        generate_token()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<DomainEvent> {
        self.events.subscribe()
    }

    // ── destructive power actions (shutdown grace) ──────────────────

    /// Arm a destructive power action behind the shared grace timer.
    /// `execute` is the platform command, supplied by the call site
    /// (queue hooks / completion actions keep their own platform
    /// integrations). Returns `false` when another action is already
    /// pending — the first countdown keeps running.
    pub fn arm_power_action<F>(&self, action: crate::domain::PowerAction, execute: F) -> bool
    where
        F: FnOnce() -> Result<(), String> + Send + 'static,
    {
        self.power.arm(action, execute)
    }

    /// Cancel the pending power action, if any. Idempotent.
    pub fn cancel_pending_shutdown(&self) {
        self.power.cancel();
    }

    /// Execute the pending power action immediately ("confirm now"
    /// from the countdown window). Idempotent.
    pub fn confirm_pending_shutdown(&self) {
        self.power.confirm();
    }

    /// `(action, deadline_ms)` of the pending power action, for
    /// snapshots to late-connecting GUIs.
    pub fn pending_shutdown(&self) -> Option<(crate::domain::PowerAction, i64)> {
        self.power.pending()
    }

    pub async fn settings(&self) -> Settings {
        self.settings.read().await.clone()
    }

    // ── secrets (master-key encrypted) ──────────────────────────────

    // ── database health ─────────────────────────────────────────────

    /// `None` when the on-disk store opened cleanly; `Some(msg)` when
    /// we fell back to in-memory because the original `Store::open`
    /// failed. The GUI uses this to gate the recovery modal.
    pub async fn db_error(&self) -> Option<String> {
        self.db_error.read().await.clone()
    }

    /// Something the store could not read while the store itself stayed
    /// usable. Surfaced as a warning, never as the recovery modal.
    pub async fn db_warning(&self) -> Option<String> {
        self.db_warning.read().await.clone()
    }

    // ── filesystem watcher health ───────────────────────────────────

    /// The limit currently stopping the watcher, if any.
    pub async fn watch_limit(&self) -> Option<crate::domain::WatchLimit> {
        self.watch_limit.read().await.clone()
    }

    /// Record what the kernel refused, or that it no longer refuses.
    /// Only announced when it actually changed: the watcher retries on
    /// its own schedule, and re-announcing the same refusal would put
    /// the same dialog in front of the user again.
    pub async fn set_watch_limit(&self, limit: Option<crate::domain::WatchLimit>) {
        {
            let mut cur = self.watch_limit.write().await;
            if *cur == limit {
                return;
            }
            *cur = limit;
        }
        let _ = self.events.send(DomainEvent::WatchLimitChanged);
    }

    /// Ask the watcher to start again — after the user has raised the
    /// limit, so the repair lands now instead of at the next launch.
    pub async fn retry_file_watch(&self) {
        let _ = self.events.send(DomainEvent::FileWatchRetry);
    }

    /// User chose "Reset" — from the Advanced danger section, or from
    /// the recovery dialog after `Store::open` failed. Wipe every
    /// per-job working dir, drop the DB, and exit the daemon process.
    /// The next daemon spawn re-runs `Store::open`, gets a fresh DB,
    /// and boots normally.
    ///
    /// The DB is only kept (renamed to a timestamped `.bak`) when it
    /// failed to open: there a copy is the user's one shot at recovering
    /// their job list out of a corrupt file. A healthy DB is deleted
    /// outright — the user asked for a clean slate, and a backup they
    /// cannot restore from the UI is just a stale file that outlives its
    /// purpose.
    ///
    /// Partials are always deleted, never backed up: they belong to the
    /// jobs this reset destroys, nothing would ever collect them again,
    /// and they can run to gigabytes.
    ///
    /// We do not try to hot-swap the `Store` in place — too many live
    /// references (queues / runners / scheduler / IPC handlers) hold
    /// pointers into the existing one. A clean exit + re-spawn is the
    /// safer reset path.
    pub async fn reset_database_and_exit(&self) -> Result<(), String> {
        // Prefix scan rather than "walk the job registry": it is the
        // only form that works on the corrupt-DB path (no rows to walk)
        // and it also reaps dirs orphaned by past crashes.
        let work_dir = self.settings().await.work_dir;
        let purged = purge_work_dir_partials(&work_dir);
        if purged > 0 {
            tracing::warn!(
                work_dir = %work_dir.display(),
                dirs = purged,
                "reset: deleted per-job working dirs",
            );
        }

        let keep_backup = self.db_error().await.is_some();
        let path = crate::data::store::default_db_path();
        if path.exists() {
            if keep_backup {
                let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S");
                let backup = path.with_extension(format!("db.bak-{ts}"));
                if let Err(e) = std::fs::rename(&path, &backup) {
                    return Err(format!("could not back up corrupt DB: {e}"));
                }
                // WAL + shm carry committed pages the main file does not
                // — a forensic copy without them is not the same DB.
                for suffix in DB_SIDECARS {
                    let from = sidecar(&path, suffix);
                    if from.exists() {
                        let _ = std::fs::rename(&from, sidecar(&backup, suffix));
                    }
                }
                tracing::warn!(
                    original = %path.display(),
                    backup = %backup.display(),
                    "DB reset: corrupt file renamed for forensics",
                );
            } else {
                if let Err(e) = std::fs::remove_file(&path) {
                    return Err(format!("could not delete database: {e}"));
                }
                // Leaving a `-wal` behind next to a freshly created DB
                // invites SQLite into recovering pages from the store we
                // just erased.
                for suffix in DB_SIDECARS {
                    let _ = std::fs::remove_file(sidecar(&path, suffix));
                }
            }
        }
        // Spawn the replacement daemon ourselves, then exit. The new
        // daemon's normal boot path calls `tray::spawn_main_gui`, so
        // the user gets their window back without the GUI side having
        // to relaunch itself (which previously caused a "two windows"
        // race against the daemon-side GUI spawn). Close every fd
        // >= 3 in the child so it does not inherit the dying daemon's
        // single-instance abstract socket — that fd would pin the
        // binding alive and force the replacement into
        // `AlreadyRunning` mode.
        let exe = crate::platform::current_exe().map_err(|e| e.to_string())?;
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let mut cmd = std::process::Command::new(&exe);
            crate::platform::attach_close_high_fds(&mut cmd);
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                unsafe {
                    cmd.pre_exec(|| {
                        libc::setsid();
                        Ok(())
                    });
                }
            }
            if let Err(e) = cmd.spawn() {
                tracing::error!(error = %e, "failed to spawn replacement daemon");
            }
            #[cfg(unix)]
            unsafe {
                libc::_exit(0);
            }
            #[cfg(not(unix))]
            std::process::exit(0);
        });
        Ok(())
    }

    /// `true` while the daemon is in startup "secrets locked" mode —
    /// DB has ciphertext, OS keyring has no key, no decryption is
    /// possible until the user acknowledges a wipe via
    /// [`unlock_via_wipe`].
    pub async fn is_secrets_locked(&self) -> bool {
        self.master_key.read().await.is_none()
    }

    /// User has acknowledged the missing-key dialog. NULL every
    /// `*_enc` column, refresh the in-memory job entries to reflect
    /// the wipe, then generate and store a fresh master key so future
    /// downloads can persist secrets again.
    pub async fn unlock_via_wipe(self: &Arc<Self>) -> Result<(), String> {
        self.store
            .wipe_all_job_secrets()
            .await
            .map_err(|e| e.to_string())?;
        // Mutate the cached `Job` on every entry so the UI doesn't
        // keep displaying stale "(stored)" hints.
        let ids: Vec<JobId> = self.jobs.read().await.keys().copied().collect();
        for id in &ids {
            let entry = match self.job_entry(*id).await {
                Some(e) => e,
                None => continue,
            };
            let mut new_job = entry.job.clone();
            new_job.enc_auth_password = None;
            new_job.enc_proxy_password = None;
            new_job.enc_cookies = None;
            let new_entry = clone_entry_with_job(&entry, new_job).await;
            self.jobs.write().await.insert(*id, new_entry);
            let _ = self.events.send(DomainEvent::JobUpdated {
                id: *id,
                phase: entry.phase(),
            });
        }
        let key = crate::data::crypto::MasterKey::generate().map_err(|e| e.to_string())?;
        *self.master_key.write().await = Some(key);
        Ok(())
    }

    /// Encrypt a per-job secret for at-rest storage. `None`/empty
    /// plaintext is a no-op (returns `Ok(None)` so callers can store
    /// NULL in the DB column).
    /// Plaintext of the global proxy password, or `None` when unset or
    /// undecryptable (Locked mode) — the proxy then goes out without
    /// credentials and the server answers 407, which is legible.
    async fn global_proxy_password(&self, s: &Settings) -> Option<String> {
        self.decrypt_field(
            GLOBAL_SECRET_ID,
            crate::data::crypto::Field::ProxyPassword,
            s.enc_proxy_password.as_deref(),
        )
        .await
    }

    pub(crate) async fn encrypt_field(
        &self,
        id: JobId,
        field: crate::data::crypto::Field,
        plaintext: Option<&str>,
    ) -> Result<Option<String>, JobError> {
        let Some(pt) = plaintext.filter(|s| !s.is_empty()) else {
            return Ok(None);
        };
        let key = self
            .master_key
            .read()
            .await
            .clone()
            .ok_or_else(|| JobError::Other("secrets locked: master key unavailable".into()))?;
        key.encrypt(id, field, pt)
            .map(Some)
            .map_err(|e| JobError::Other(format!("encrypt failed: {e}")))
    }

    /// Decrypt a per-job secret. Returns `None` on absent column or
    /// any error (Locked mode, AAD mismatch, tampered blob). Errors
    /// are logged once but do not abort the caller — typical use is
    /// the runner asking "what password should I hand to odl?" and a
    /// missing/broken secret simply means "send the request without
    /// it and let the server respond".
    pub(crate) async fn decrypt_field(
        &self,
        id: JobId,
        field: crate::data::crypto::Field,
        blob: Option<&str>,
    ) -> Option<String> {
        let blob = blob?.trim();
        if blob.is_empty() {
            return None;
        }
        let key = self.master_key.read().await.clone()?;
        match key.decrypt(id, field, blob) {
            Ok(Some(s)) => Some(s),
            Ok(None) => None,
            Err(e) => {
                tracing::warn!(job = %id, ?field, error = %e, "decrypt failed");
                None
            }
        }
    }

    /// Save only the fields the caller actually edited.
    ///
    /// The Settings window sends the whole page, which meant an Apply
    /// wrote back every value as it stood when the window opened —
    /// silently reverting anything changed elsewhere in the meantime
    /// (dismissing the inotify warning with "don't warn again" in the
    /// main window, for instance). Merging by key keeps an Apply to
    /// what the user touched.
    pub async fn update_settings_fields(
        &self,
        edited: Settings,
        keys: &[String],
    ) -> Result<(), String> {
        if keys.is_empty() {
            return Ok(());
        }
        let current = self.settings().await;
        let merged = merge_settings_fields(&current, &edited, keys)?;
        self.update_settings(merged).await
    }

    pub async fn update_settings(&self, mut new: Settings) -> Result<(), String> {
        // Checked here rather than in the Settings window, because the
        // window is not the only caller: this is reachable over IPC.
        // A blank or relative cache folder is not a small mistake —
        // `.part` files land relative to wherever the daemon was
        // started, and the free-space check has no volume to measure,
        // so it silently passes everything.
        validate_work_dir(&new.work_dir)?;
        // The proxy password arrives in the clear and leaves as
        // ciphertext; the plaintext never reaches the settings table.
        let typed = std::mem::take(&mut new.proxy.password);
        let clear = std::mem::take(&mut new.proxy.clear_password);
        new.enc_proxy_password = if clear {
            None
        } else if typed.is_empty() {
            // Untouched field: keep whatever is already stored.
            self.settings.read().await.enc_proxy_password.clone()
        } else {
            self.encrypt_field(
                GLOBAL_SECRET_ID,
                crate::data::crypto::Field::ProxyPassword,
                Some(&typed),
            )
            .await
            .map_err(|e| e.to_string())?
        };
        // Autostart lives outside the DB (XDG autostart entry / launch
        // agent / Run key), so the flag has to be reconciled with the
        // OS whenever it flips. A failure there must not lose the rest
        // of the save, so we keep the persisted flag matching what the
        // OS actually has instead of writing a promise we didn't keep.
        let was = self.settings.read().await.start_at_login;
        if new.start_at_login != was {
            let want = new.start_at_login;
            let applied = tokio::task::spawn_blocking(move || crate::platform::set_autostart(want))
                .await
                .map_err(|e| e.to_string())?;
            if let Err(e) = applied {
                tracing::warn!(error = %e, enabled = want, "set autostart failed");
                new.start_at_login = was;
            }
        }

        let proxy_password = self.global_proxy_password(&new).await;
        let manager = build_manager(&new, proxy_password.as_deref());
        self.store
            .save_settings(&new)
            .await
            .map_err(|e| e.to_string())?;
        *self.manager.write().await = Arc::new(manager);
        // The extension authenticates against this copy, so a saved
        // pairing code has to land here too — otherwise the code the
        // window shows only starts working after a restart.
        *self.ext_token.write().await = new.ext_token.clone();
        *self.settings.write().await = new;
        let _ = self.events.send(DomainEvent::SettingsChanged);
        Ok(())
    }

    /// What one job will want from the disks, if anyone knows its size.
    fn space_need(&self, entry: &JobEntry, work_dir: &std::path::Path) -> crate::data::space::Need {
        crate::data::space::Need {
            // The configured cache folder, not this job's subfolder
            // inside it: they are the same volume, and only one of them
            // is a folder the user has ever seen.
            work_dir: work_dir.to_path_buf(),
            save_dir: entry.job.save_dir.clone(),
            // The live counter first: a job part-way through a run knows
            // more than the row last written to the database.
            total: entry.counters.total().or(entry.job.status.total),
            downloaded: entry.counters.downloaded().max(entry.job.status.downloaded),
        }
    }

    /// Refuse before starting anything that plainly cannot fit.
    ///
    /// The error names one volume, what it needs and what it has; the
    /// windows show it verbatim, because the numbers *are* the
    /// explanation.
    async fn refuse_if_short_on_space(
        &self,
        needs: Vec<crate::data::space::Need>,
    ) -> Result<(), JobError> {
        use crate::data::space;
        if needs.is_empty() {
            return Ok(());
        }
        // Every syscall here — statvfs per volume, plus the metadata
        // walk that finds it — is blocking, and there may be one per
        // job in a queue.
        let short = tokio::task::spawn_blocking(move || {
            let required = space::required_by_volume(&needs, space::volume_key);
            space::shortfall(required, space::free_space)
        })
        .await
        .ok()
        .flatten();
        match short {
            None => Ok(()),
            Some(s) => Err(JobError::InsufficientSpace {
                path: s.path.display().to_string(),
                needed: s.needed,
                available: s.available,
            }),
        }
    }

    /// Snapshot every user-visible job. Hidden jobs (e.g. self-update
    /// artifact downloads) are filtered out here so every callsite —
    /// queue UI, tray menu, clear-completed — gets the same view.
    pub async fn list_jobs(&self) -> Vec<Job> {
        let hidden = self.hidden_jobs.read().await.clone();
        self.jobs
            .read()
            .await
            .values()
            .filter(|e| !hidden.contains(&e.job.id))
            .map(|e| splice_live(e))
            .collect()
    }

    pub async fn job_entry(&self, id: JobId) -> Option<Arc<JobEntry>> {
        self.jobs.read().await.get(&id).cloned()
    }

    /// Persist the live state of `id` into the store. Captures phase,
    /// downloaded, total, and final_path — the fields the queue UI and
    /// Download-Complete dialog need to keep showing accurate data
    /// after a restart. Called on every terminal phase transition
    /// (completed / paused / failed / cancelled / requeued); we
    /// deliberately don't write on every Progress tick to keep SQLite
    /// out of the hot path.
    async fn persist_job(&self, id: JobId) {
        let Some(entry) = self.jobs.read().await.get(&id).cloned() else {
            return;
        };
        let job = splice_live(&entry);
        if let Err(e) = self.store.upsert_job(&job).await {
            tracing::warn!(id = %id, error = %e, "persist job state failed");
        }
    }

    /// Hash the saved file and record the verdict, in the daemon.
    ///
    /// Returns as soon as the work is scheduled: hashing a large file
    /// takes minutes, and the caller is a window that must stay
    /// responsive — and may well be closed before it finishes. Progress
    /// is published as `JobEntryView::verifying` plus a `JobUpdated`
    /// at each end of the run, so any window open at the time follows
    /// along and one opened later sees the result.
    pub async fn verify_checksums(self: &Arc<Self>, id: JobId) -> Result<(), JobError> {
        let entry = self
            .job_entry(id)
            .await
            .ok_or_else(|| JobError::Other("job not found".into()))?;
        Self::refuse_while_assembling(&entry)?;
        let path = entry
            .saved_file()
            .ok_or_else(|| JobError::Other("this download has no saved file".into()))?;
        if crate::data::mapping::checksum_digests(&entry.job).is_empty() {
            return Err(JobError::Other("nothing to check this file against".into()));
        }
        // Answer "the file is gone" to the caller's face rather than in
        // a log line half a second later — the window asking is still
        // open, and this is the common failure: the user moved it.
        if tokio::fs::metadata(&path).await.is_err() {
            return Err(JobError::Other(format!(
                "the saved file is no longer at {}",
                path.display()
            )));
        }
        // One run per job: a second click while the first is hashing
        // would read the same file twice to the same answer.
        if entry.verifying.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        // Written down before the work starts: a hash cannot be resumed
        // — half a digest is worth nothing — but a daemon that dies
        // mid-check should know on the next launch that it owes one.
        self.set_verify_pending(id, true).await;

        let state = self.clone();
        let phase = entry.phase();
        tokio::spawn(async move {
            let _ = state.events.send(DomainEvent::JobUpdated { id, phase });
            // Adding or deleting a row does not change the file, so a
            // digest this job already computed from these exact bytes
            // answers the new row too — the common case, since the
            // dialog asks for a check after every edit.
            let ident = file_identity(&path).await;
            let known = entry.known_digests(ident);
            let results = hash_against_rows(&path, &entry.job.checksums, known).await;
            if let Ok((_, computed)) = &results {
                entry.remember_digests(ident, computed.clone());
            }
            let results = results.map(|(rows, _)| rows);
            if let Some(e) = state.job_entry(id).await {
                e.verifying.store(false, Ordering::Release);
            }
            match results {
                Ok(rows) => state.apply_checksum_results(id, rows).await,
                // An unreadable file says nothing about the hashes, so
                // no row's verdict changes — but a window that asked
                // deserves to hear why nothing happened. It can vanish
                // between the ask and the answer, hence an event rather
                // than a reply.
                Err(e) => {
                    tracing::warn!(id = %id, error = %e, "checksum verification failed");
                    let _ = state.events.send(DomainEvent::JobVerifyFailed {
                        id,
                        message: e.clone(),
                    });
                }
            }
            // Cleared either way: a file we cannot read is not a check
            // worth retrying on every launch.
            state.set_verify_pending(id, false).await;
            let _ = state.events.send(DomainEvent::JobUpdated { id, phase });
        });
        Ok(())
    }

    /// Flip the persisted "a check is owed" marker.
    async fn set_verify_pending(self: &Arc<Self>, id: JobId, pending: bool) {
        let Some(entry) = self.job_entry(id).await else {
            return;
        };
        if entry.job.verify_pending == pending {
            return;
        }
        let mut job = entry.job.clone();
        job.verify_pending = pending;
        let fresh = clone_entry_with_job(&entry, job).await;
        self.jobs.write().await.insert(id, fresh);
        self.persist_job(id).await;
    }

    /// Re-run every hash check that was interrupted by a daemon exit.
    ///
    /// Bounded to jobs that were actually mid-check — normally none, at
    /// most a handful — rather than re-hashing every completed download
    /// on the chance that one of them is stale.
    pub async fn resume_pending_verifications(self: &Arc<Self>) {
        let owed: Vec<JobId> = self
            .jobs
            .read()
            .await
            .values()
            .filter(|e| e.job.verify_pending)
            .map(|e| e.job.id)
            .collect();
        for id in owed {
            if let Err(e) = self.verify_checksums(id).await {
                // The file is gone, or there is nothing to check it
                // against: clear the marker rather than carrying it
                // forward into every future launch.
                tracing::info!(id = %id, reason = %e, "dropping an owed checksum check");
                self.set_verify_pending(id, false).await;
            }
        }
    }

    /// Write per-row verdicts from a hash run. A mismatched row keeps
    /// the computed digest in `expected` — that is the "got" side the
    /// completion page and the Checksums tab both show against it.
    async fn apply_checksum_results(
        self: &Arc<Self>,
        id: JobId,
        results: Vec<(usize, crate::domain::CsStatus, Option<String>)>,
    ) {
        let Some(entry) = self.job_entry(id).await else {
            return;
        };
        let mut job = entry.job.clone();
        for (i, status, computed) in results {
            let Some(c) = job.checksums.get_mut(i) else {
                continue;
            };
            c.status = status;
            c.expected = computed;
        }
        let fresh = clone_entry_with_job(&entry, job).await;
        clear_settled_mismatch(&fresh);
        let phase = fresh.phase();
        self.jobs.write().await.insert(id, fresh);
        self.persist_job(id).await;
        // A verdict changes what every window says about the job —
        // whether it is tampered, whether it can be resumed. Silence
        // here leaves them rendering the answer from before the check.
        let _ = self.events.send(DomainEvent::JobUpdated { id, phase });
    }

    /// Insert a new job in `Queued` state. Caller decides whether to
    /// also `start_job` (Download Now) or leave it (Download Later).
    ///
    /// The name is made unique against the whole table before the job
    /// is stored: one name identifies one download, whatever folder
    /// each saves into. Adds are numbered rather than refused — a
    /// capture or a batch has nobody to ask — while a *rename* of an
    /// existing job is refused (`JobError::NameTaken`), because there
    /// the name is one someone just typed. The runner-level
    /// `SaveConflictResolver` still handles files already on disk.
    #[allow(clippy::too_many_arguments)]
    pub async fn add_job(
        self: &Arc<Self>,
        url: url::Url,
        save_dir: PathBuf,
        filename: Option<String>,
        referrer: Option<url::Url>,
        headers: indexmap::IndexMap<String, String>,
        max_connections: Option<u64>,
        proxy: Option<String>,
        auth_user: Option<String>,
        auth_password: Option<String>,
        proxy_password: Option<String>,
        cookies: Option<String>,
        category: Option<Category>,
        // What the caller's probe found. Recorded now so the job knows
        // its size and its expected digests while it is still queued,
        // instead of the first run teaching the UI things the Add
        // dialog already displayed.
        probe: ProbeFacts,
    ) -> Result<JobId, JobError> {
        let id = JobId::new();
        // Nobody downstream re-checks this: `save_dir.join(filename)` is
        // where the bytes land and what "delete file" removes. Most
        // names here were written by the server (`Content-Disposition`,
        // or the URL's last path segment), so this is the boundary.
        let filename = filename.and_then(|n| crate::domain::filename::sanitize(&n));
        // Whether this job arrives knowing anything about the file. A
        // caller that probed says so by passing what it found; one that
        // did not gets the answer filled in behind it.
        let named = filename.as_deref().is_some_and(|n| !n.trim().is_empty());
        let probe_was_empty = probe.size.is_none() && probe.checksums.is_empty();
        // Detect the category once at creation when the caller did not
        // supply an explicit choice. `classify` falls back to
        // `Category::Other` when nothing matches.
        let category = match category {
            Some(c) => c,
            None => {
                let overrides = self.settings.read().await.category_extensions.clone();
                classify(filename.as_deref().unwrap_or(""), &overrides)
            }
        };
        let enc_auth_password = self
            .encrypt_field(
                id,
                crate::data::crypto::Field::AuthPassword,
                auth_password.as_deref(),
            )
            .await?;
        let enc_proxy_password = self
            .encrypt_field(
                id,
                crate::data::crypto::Field::ProxyPassword,
                proxy_password.as_deref(),
            )
            .await?;
        let enc_cookies = self
            .encrypt_field(id, crate::data::crypto::Field::Cookies, cookies.as_deref())
            .await?;
        let mut job = Job {
            id,
            url,
            save_dir,
            filename,
            referrer,
            headers,
            max_connections,
            proxy,
            auth_user,
            enc_auth_password,
            enc_proxy_password,
            enc_cookies,
            speed_limit_override: None,
            queue_id: self.main_queue_id,
            // Recorded when the first run actually creates the folder,
            // not now: a job that never runs should not pin a cache
            // folder the user is still free to change.
            work_root: None,
            created_at: chrono::Utc::now(),
            started_at: None,
            active_ms: None,
            finished_at: None,
            retries: 0,
            interruptions: 0,
            verify_pending: false,
            status: JobStatus {
                total: probe.size,
                ..JobStatus::default()
            },
            advanced: crate::domain::Advanced::default(),
            checksums: probe.checksums,
            category,
            captured_response: None,
        };
        let completion = seeded_completion(&self.settings().await);
        let url = job.url.clone();
        // A job with credentials is not one a bare probe can describe:
        // the server answers a sign-in page, and recording its name and
        // size on the job would be worse than knowing nothing. Those
        // jobs learn from their own run, which carries the secrets.
        let probe_worth_it = !named
            && probe_was_empty
            && job.auth_user.is_none()
            && job.enc_auth_password.is_none()
            && job.enc_cookies.is_none();
        // The name is made unique and the job goes in under the same
        // lock: two adds landing together must not both decide the
        // same name is free. The store write stays inside it too, so
        // nothing is announced that failed to persist.
        let mut jobs = self.jobs.write().await;
        if let Some(name) = job.filename.as_deref() {
            job.filename = Some(free_name(&jobs, name, None));
        }
        if let Err(e) = self.store.upsert_job(&job).await {
            return Err(JobError::Io(e.to_string()));
        }
        jobs.insert(id, Arc::new(JobEntry::with_completion(job, completion)));
        drop(jobs);
        let _ = self.events.send(DomainEvent::JobAdded { id });
        if probe_worth_it {
            self.probe_in_background(id, url);
        }
        Ok(id)
    }

    /// Edit an existing job's URL / final destination. The per-job
    /// working dir lives under the global `Settings::work_dir` and is
    /// keyed by job id only, so retargeting `save_dir` or renaming the
    /// final file does not invalidate any in-flight `.part` data — the
    /// runner just assembles into the new location at completion.
    ///
    /// URL changes flow through to odl's next `evaluate`; if the new
    /// URL points at different content (size or `Last-Modified`
    /// mismatch with the existing partial), odl's `FileChanged`
    /// resolver fires and the UI's conflict prompt surfaces. No need
    /// to pre-wipe state here — let odl decide.
    pub async fn update_job_location(
        self: &Arc<Self>,
        id: JobId,
        edit: crate::ipc_local::protocol::JobEdit,
    ) -> Result<(), JobError> {
        let entry = self
            .job_entry(id)
            .await
            .ok_or_else(|| JobError::Other("job not found".into()))?;

        let edit_filename = edit
            .filename
            .and_then(|n| crate::domain::filename::sanitize(&n));
        if let Some(name) = edit_filename.as_deref().filter(|n| !n.trim().is_empty())
            && name_is_taken(&*self.jobs.read().await, name, Some(id))
        {
            return Err(JobError::NameTaken {
                filename: name.trim().to_owned(),
            });
        }
        let mut new_job = entry.job.clone();
        new_job.url = edit.url;
        new_job.save_dir = edit.save_dir;
        new_job.filename = edit_filename;
        new_job.referrer = edit.referrer;
        new_job.headers = edit.headers;
        new_job.max_connections = edit.max_connections;
        new_job.proxy = edit.proxy;
        new_job.auth_user = edit.auth_user;
        // Absent/empty secrets keep the stored ciphertext (same rule as
        // `set_job_advanced`): a header/cookie-only Apply from a client
        // that never round-trips secrets must not wipe them. Explicit
        // clearing is not expressible through this path (documented in
        // features-impl-plan F1/F2).
        if let Some(pw) = edit.auth_password.as_deref().filter(|s| !s.is_empty()) {
            new_job.enc_auth_password = self
                .encrypt_field(id, crate::data::crypto::Field::AuthPassword, Some(pw))
                .await?;
        }
        if let Some(pw) = edit.proxy_password.as_deref().filter(|s| !s.is_empty()) {
            new_job.enc_proxy_password = self
                .encrypt_field(id, crate::data::crypto::Field::ProxyPassword, Some(pw))
                .await?;
        }
        if let Some(ck) = edit.cookies.as_deref().filter(|s| !s.is_empty()) {
            new_job.enc_cookies = self
                .encrypt_field(id, crate::data::crypto::Field::Cookies, Some(ck))
                .await?;
        }

        // Rebuild the JobEntry; it holds runtime atomics behind shared
        // refs, so spawning a fresh one keeps things consistent.
        // Counters / final_path are preserved by `clone_entry_with_job`.
        let new_entry = clone_entry_with_job(&entry, new_job.clone()).await;
        self.jobs.write().await.insert(id, new_entry);
        self.store
            .upsert_job(&new_job)
            .await
            .map_err(|e| JobError::Io(e.to_string()))?;
        let _ = self.events.send(DomainEvent::JobUpdated {
            id,
            phase: entry.phase(),
        });
        Ok(())
    }

    /// Run an HTTP probe (HEAD) to discover filename / size / resume
    /// support, without queueing or starting a download.
    ///
    /// Used by the Add-Download dialog to fill detected fields live.
    /// Internally goes through `DownloadManager::evaluate` with a
    /// `ProbeResolver` that aborts on every conflict, so the call
    /// either returns metadata or a clean error — never side-effects.
    /// Probe `url`, sharing one request between everyone who asks.
    ///
    /// The Add dialog asks, and a moment later the job it created asks
    /// again; a second request would ask the same server the same
    /// question for the same answer. Callers that arrive while a probe
    /// is running wait for it, and one that arrives just after gets
    /// what it found (see [`PROBE_FRESH_FOR`]).
    ///
    /// Failures are shared too, and deliberately not cached: a server
    /// that was unreachable a second ago is worth asking again.
    pub async fn probe_shared(&self, url: url::Url) -> Result<ProbeResult, JobError> {
        let key = url.as_str().to_owned();
        let mut rx = {
            let mut slots = self.probes.lock().await;
            // Swept whenever the map is held anyway: entries past
            // freshness answer nobody, and a long session pasting
            // links kept every one of them.
            slots.retain(|_, slot| match slot {
                ProbeSlot::Done { at, .. } => at.elapsed() < PROBE_FRESH_FOR,
                ProbeSlot::Running(_) => true,
            });
            match slots.get(&key) {
                Some(ProbeSlot::Done { at, result }) if at.elapsed() < PROBE_FRESH_FOR => {
                    return (**result).clone();
                }
                Some(ProbeSlot::Running(tx)) => Some(tx.subscribe()),
                _ => {
                    let (tx, _) = broadcast::channel(1);
                    slots.insert(key.clone(), ProbeSlot::Running(tx));
                    None
                }
            }
        };
        if let Some(rx) = rx.as_mut() {
            // The leader dropping its sender without a value would only
            // happen if its task was cancelled mid-probe; asking again
            // is better than reporting an error nobody caused.
            return match rx.recv().await {
                Ok(result) => (*result).clone(),
                Err(_) => self.probe(url).await,
            };
        }

        let result = Arc::new(self.probe(url).await);
        {
            let mut slots = self.probes.lock().await;
            let waiters = match slots.remove(&key) {
                Some(ProbeSlot::Running(tx)) => Some(tx),
                _ => None,
            };
            if result.is_ok() {
                slots.insert(
                    key,
                    ProbeSlot::Done {
                        at: std::time::Instant::now(),
                        result: result.clone(),
                    },
                );
            }
            // Sent with the map unlocked in mind: `send` only fails when
            // nobody is waiting, which is the common case.
            if let Some(tx) = waiters {
                let _ = tx.send(result.clone());
            }
        }
        (*result).clone()
    }

    /// Probe a job's URL in the background and record what comes back.
    ///
    /// For a job added before its probe finished — the user pasted a
    /// link and pressed Add. The answer is worth having even though
    /// nobody is waiting for it: the row can show its real name and
    /// size while it sits in the queue, rather than a URL and a dash
    /// until someone starts it.
    ///
    /// Failure is silent by design. Nothing the user asked for has
    /// failed yet; if the link really is dead they find out when they
    /// start the download, with the error panel that flow already has.
    fn probe_in_background(self: &Arc<Self>, id: JobId, url: url::Url) {
        let state = self.clone();
        tokio::spawn(async move {
            match state.probe_shared(url).await {
                Ok(probe) => state.apply_probe_result(id, probe).await,
                Err(e) => {
                    tracing::debug!(id = %id, error = %e, "background probe found nothing")
                }
            }
        });
    }

    /// Fill in what a job does not know yet from a probe that landed
    /// after it was added.
    ///
    /// Only ever fills blanks. By the time this runs the job may have
    /// been started, renamed or finished, and a probe is the *older*
    /// piece of information in every one of those races — the run
    /// talked to the same server later and for real.
    pub async fn apply_probe_result(self: &Arc<Self>, id: JobId, probe: ProbeResult) {
        let settings = self.settings.read().await.clone();
        let mut jobs = self.jobs.write().await;
        let Some(old) = jobs.get(&id).cloned() else {
            return;
        };
        // A run of its own outranks anything a probe can say.
        if old.running.load(Ordering::Acquire) || old.phase() != Phase::Queued {
            return;
        }
        let mut new_job = old.job.clone();
        let mut changed = false;
        let named = new_job.filename.as_deref().is_some_and(|n| !n.is_empty());
        if !named && !probe.filename.trim().is_empty() {
            // Numbered against the table, the same as every other way
            // a job gets its name: three links to the same `clip.mkv`
            // are three downloads, and the list has to be able to say
            // which is which.
            let name = free_name(&jobs, &probe.filename, Some(id));
            new_job.filename = Some(name.clone());
            if new_job.category == Category::Other {
                new_job.category = classify(&name, &settings.category_extensions);
                // Nothing has run yet, so the destination is still
                // nobody's decision but the app's own guess — which
                // this just improved on.
                if let Some(dir) =
                    Self::retarget_dir(&settings, &new_job.save_dir, new_job.category)
                {
                    new_job.save_dir = dir;
                }
            }
            changed = true;
        }
        if new_job.status.total.is_none() && probe.size.is_some() {
            new_job.status.total = probe.size;
            changed = true;
        }
        if crate::data::mapping::merge_checksums(&mut new_job.checksums, probe.checksums.clone()) {
            changed = true;
        }
        if !changed {
            return;
        }
        let phase = old.phase();
        let new_entry = clone_entry_with_job(&old, new_job.clone()).await;
        new_entry
            .is_resumable
            .store(if probe.is_resumable { 1 } else { -1 }, Ordering::Release);
        // The counters are what the list reads for its size column, so
        // a total that only reached the stored job would not show.
        new_entry.counters.set_total(probe.size);
        jobs.insert(id, new_entry);
        drop(jobs);
        if let Err(e) = self.store.upsert_job(&new_job).await {
            tracing::warn!(id = %id, error = %e, "could not store what the probe found");
        }
        if let Some(name) = new_job.filename.clone() {
            let _ = self
                .events
                .send(DomainEvent::JobFilenameResolved { id, filename: name });
        }
        let _ = self.events.send(DomainEvent::JobUpdated { id, phase });
    }

    pub async fn probe(&self, url: url::Url) -> Result<ProbeResult, JobError> {
        let manager = self.manager.read().await.clone();
        let settings = self.settings.read().await.clone();
        let resolver = ProbeResolver;
        let instr = manager
            .evaluate(
                odl::download_manager::EvaluateRequest::new(
                    url,
                    settings.fallback_dir(),
                    &resolver,
                )
                // Same engine the run will use, or the probe would
                // describe a file the download never fetches.
                .engine(crate::data::runner::FORCED_ENGINE),
            )
            .await
            .map_err(|e| crate::data::mapping::job_error_from_odl(&e))?;
        Ok(ProbeResult {
            checksums: crate::data::mapping::server_checksums(&instr),
            filename: instr.filename().to_string(),
            size: instr.size(),
            is_resumable: instr.is_resumable(),
            etag: instr.etag().clone(),
            last_modified: instr.last_modified(),
            requires_auth: instr.requires_auth(),
        })
    }

    /// Convenience for IPC: build a Job from a `CaptureRequest` and add it.
    /// Does not auto-start; caller (UI) decides per `interactive` flag.
    pub async fn add_from_capture(
        self: &Arc<Self>,
        req: CaptureRequest,
    ) -> Result<JobId, JobError> {
        let settings = self.settings().await;
        // Pull `Cookie` (and a stray `Authorization` if present) out of
        // the captured header bag so they ride the encrypted-secret
        // path instead of being persisted as plaintext headers.
        let mut headers = req.headers.clone();
        let captured_cookie = headers
            .shift_remove("Cookie")
            .or_else(|| headers.shift_remove("cookie"));
        if let Some(ua) = req.user_agent.as_deref()
            && !headers.contains_key("User-Agent")
        {
            headers.insert("User-Agent".into(), ua.into());
        }
        // The referrer is *not* copied into the header bag: it rides
        // `Job::referrer`, and `mapping::job_overlay_options` splices
        // it in at request time. Two copies would show up as two rows
        // in Properties and drift the moment one is edited.
        let cookies = req.cookies.clone().or(captured_cookie);
        // A capture has nobody to ask, so a name the table already
        // holds is numbered by `add_job` rather than refused.
        let filename = req.filename;
        // Per-category routing (feature #10) applies only on this
        // non-interactive path (guardian F5) — the Add dialog prefills
        // client-side instead, so an explicit user choice always wins.
        // Known caveat: classification here uses the captured filename;
        // a later FilenameResolved may land in a different category —
        // no re-routing this pass.
        let category = classify(
            filename.as_deref().unwrap_or(""),
            &settings.category_extensions,
        );
        let save_dir = settings.category_folder(category);
        let id = self
            .add_job(
                req.url,
                save_dir,
                filename,
                req.referrer,
                headers,
                None,
                None,
                None,
                None,
                None,
                cookies,
                Some(category),
                // A capture carries no probe of its own; the run
                // reports the size.
                ProbeFacts::default(),
            )
            .await?;
        if let Some(qid) = settings.category_queues.get(&category).copied()
            && qid != self.main_queue_id
            && let Err(e) = self.set_job_queue(id, qid).await
        {
            // Stale id (queue deleted since the mapping was saved) —
            // the job stays in Main rather than failing the capture.
            tracing::warn!(job = %id, queue = %qid, error = %e, "category default queue not applied");
        }
        Ok(id)
    }

    /// Spawn a runner for a queued / paused job. Idempotent on a
    /// running job (no-op).
    /// Record who asked for the run that is about to start: a user
    /// gesture aimed at this one download, or automation (a queue run,
    /// Resume all, the scheduler, a browser capture). Every entry point
    /// that starts a job states this, so the flag always describes the
    /// current run rather than an earlier one.
    pub async fn mark_run_intent(&self, id: JobId, manual: bool) {
        if let Some(entry) = self.job_entry(id).await {
            entry.manual_run.store(manual, Ordering::Release);
        }
    }

    /// Did the user start this download by hand?
    pub async fn is_manual_run(&self, id: JobId) -> bool {
        match self.job_entry(id).await {
            Some(entry) => entry.manual_run.load(Ordering::Acquire),
            None => false,
        }
    }

    /// Take the name and folder of the file that was actually written.
    ///
    /// A save conflict is resolved by odl, not by oxdm: with no window
    /// open to ask in, `AddNumberToNameAndContinue` renames the output
    /// and carries on. Nothing wrote that name back, so the row, its
    /// window, and Open all pointed at the name that was taken — an
    /// unrelated file already sitting in the folder.
    async fn adopt_final_path(&self, id: JobId, path: std::path::PathBuf) {
        let Some(name) = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| !n.is_empty())
        else {
            return;
        };
        let mut jobs = self.jobs.write().await;
        let Some(old) = jobs.get(&id).cloned() else {
            return;
        };
        let dir = path.parent().map(|p| p.to_path_buf());
        let name_differs = old.job.filename.as_deref() != Some(name.as_str());
        let dir_differs = dir.as_ref().is_some_and(|d| *d != old.job.save_dir);
        if !name_differs && !dir_differs {
            return;
        }
        let mut job = old.job.clone();
        job.filename = Some(name.clone());
        if let Some(dir) = dir {
            job.save_dir = dir;
        }
        let phase = old.phase();
        let entry = clone_entry_with_job(&old, job.clone()).await;
        jobs.insert(id, entry);
        drop(jobs);
        if let Err(e) = self.store.upsert_job(&job).await {
            tracing::warn!(id = %id, error = %e, "could not record the name the file was saved under");
        }
        tracing::info!(id = %id, %name, "the saved file was renamed; the download follows it");
        let _ = self
            .events
            .send(DomainEvent::JobFilenameResolved { id, filename: name });
        let _ = self.events.send(DomainEvent::JobUpdated { id, phase });
    }

    /// Remember the folder this job's partials went into, so a later
    /// change to the cache-folder setting cannot strand or re-fetch
    /// them. Best effort: failing to persist it only means the job
    /// falls back to the live setting, which is where it just wrote.
    async fn record_work_root(&self, id: JobId, root: &std::path::Path) {
        let mut jobs = self.jobs.write().await;
        let Some(old) = jobs.get(&id).cloned() else {
            return;
        };
        if old.job.work_root.is_some() {
            return;
        }
        let mut job = old.job.clone();
        job.work_root = Some(root.to_path_buf());
        let entry = clone_entry_with_job(&old, job.clone()).await;
        jobs.insert(id, entry);
        drop(jobs);
        if let Err(e) = self.store.upsert_job(&job).await {
            tracing::warn!(id = %id, error = %e, "could not record the cache folder");
        }
    }

    /// Downloads whose partly-fetched data sits under a cache folder
    /// that is no longer the configured one, with how many bytes are
    /// down there.
    ///
    /// Changing the cache folder leaves them behind by design — they go
    /// on resuming from where they were written — but the user cannot
    /// see that folder in the UI, so the window offers to clear it.
    pub async fn stranded_partials(&self) -> (usize, u64) {
        let current = self.settings().await.work_dir;
        let jobs = self.jobs.read().await;
        let mut count = 0;
        let mut bytes = 0;
        for entry in jobs.values() {
            let Some(root) = entry.job.work_root.as_ref() else {
                continue;
            };
            let downloaded = entry.counters.downloaded().max(entry.job.status.downloaded);
            if *root != current && downloaded > 0 && !entry.phase().is_running() {
                count += 1;
                bytes += downloaded;
            }
        }
        (count, bytes)
    }

    /// Delete what those downloads have already fetched and set them up
    /// to start over under the current cache folder.
    ///
    /// Only ever from an explicit "yes, delete it": every byte here was
    /// paid for once already.
    pub async fn discard_stranded_partials(self: &Arc<Self>) -> Result<usize, String> {
        let current = self.settings().await.work_dir;
        let targets: Vec<(JobId, std::path::PathBuf)> = self
            .jobs
            .read()
            .await
            .values()
            .filter(|e| !e.phase().is_running())
            .filter_map(|e| {
                let root = e.job.work_root.clone()?;
                (root != current).then_some((e.job.id, root))
            })
            .collect();

        let mut discarded = 0;
        for (id, root) in targets {
            let dir = per_job_dir(&root, id);
            if let Err(e) = tokio::fs::remove_dir_all(&dir).await
                && e.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!(id = %id, dir = %dir.display(), error = %e,
                    "could not delete the old partial data");
                continue;
            }
            let mut jobs = self.jobs.write().await;
            let Some(old) = jobs.get(&id).cloned() else {
                continue;
            };
            let mut job = old.job.clone();
            // Back to "never run": the next start records the folder it
            // writes into, which is now the configured one.
            job.work_root = None;
            job.status.downloaded = 0;
            let entry = clone_entry_with_job(&old, job.clone()).await;
            entry.counters.reset_progress();
            entry.reset_run_stats();
            entry.set_phase(Phase::Queued);
            jobs.insert(id, entry);
            drop(jobs);
            if let Err(e) = self.store.upsert_job(&job).await {
                tracing::warn!(id = %id, error = %e, "could not record the restart");
            }
            let _ = self.events.send(DomainEvent::JobUpdated {
                id,
                phase: Phase::Queued,
            });
            discarded += 1;
        }
        Ok(discarded)
    }

    /// The folder holding this job's partials: the one it wrote into,
    /// or the current setting for a job that has never run.
    async fn work_root_of(&self, id: JobId) -> std::path::PathBuf {
        let recorded = self
            .job_entry(id)
            .await
            .and_then(|entry| entry.job.work_root.clone());
        match recorded {
            Some(root) => root,
            None => self.settings().await.work_dir,
        }
    }

    /// Wait out a run that has been told to stop but has not finished
    /// writing its outcome yet, and hand back the entry to start from.
    ///
    /// Bounded: a job whose phase still says it is running is genuinely
    /// running and returns immediately, and the wait gives up rather
    /// than blocking a request forever if an epilogue never lands.
    async fn settle_previous_run(&self, id: JobId, entry: Arc<JobEntry>) -> Arc<JobEntry> {
        const GRACE: std::time::Duration = std::time::Duration::from_secs(3);
        let deadline = std::time::Instant::now() + GRACE;
        let mut entry = entry;
        while entry.running.load(Ordering::Acquire) && !entry.phase().is_running() {
            if std::time::Instant::now() >= deadline {
                tracing::warn!(id = %id, "previous run has not released the job; starting anyway");
                break;
            }
            let finished = self.run_finished.notified();
            tokio::select! {
                _ = finished => {}
                _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
            }
            match self.job_entry(id).await {
                Some(e) => entry = e,
                None => break,
            }
        }
        entry
    }

    /// Is there room for one more download in `queue`?
    ///
    /// Two caps, and both are real: the queue's own `max_concurrent`
    /// and the global `max_concurrent_downloads` across every queue.
    /// The global one is shared, so two queues allowing three each
    /// still run five between them when five is the global limit —
    /// otherwise a global cap would mean nothing as soon as a second
    /// queue existed.
    async fn slots_full_for(&self, queue: QueueId) -> bool {
        let global = self.settings().await.max_concurrent_downloads;
        let per_queue = self.queue(queue).await.and_then(|q| q.max_concurrent);
        slots_full(&*self.jobs.read().await, global, queue, per_queue)
    }

    /// Start whatever the cap sent back to Queued, oldest first, until
    /// the slots are full again.
    ///
    /// The queue filler only runs for a queue the user actually
    /// started, so without this a job deferred by the global cap during
    /// Resume all would sit Queued with nothing ever coming back for it.
    /// Boxed for the same reason as [`Self::fill_queue_slots`]: filling
    /// a slot starts a job, and that job's ending fills the next, so the
    /// future's type would contain itself.
    pub fn fill_deferred_slots(
        self: &Arc<Self>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        let state = Arc::clone(self);
        Box::pin(async move { state.fill_deferred_slots_inner().await })
    }

    async fn fill_deferred_slots_inner(self: &Arc<Self>) {
        // A job that will not start — its own queue is full, it is out
        // of disk — must not be picked again on the next turn of this
        // loop, or the loop never ends.
        let mut tried: std::collections::HashSet<JobId> = std::collections::HashSet::new();
        loop {
            if self.is_exiting() {
                return;
            }
            let Some(id) = next_deferred(&*self.jobs.read().await, &tried) else {
                return;
            };
            tried.insert(id);
            // The queue this job belongs to may be at its own limit
            // even while the global one has room; it keeps waiting.
            if self.slots_full_for(self.queue_of(id).await).await {
                continue;
            }
            self.mark_run_intent(id, false).await;
            match self.start_job(id).await {
                Ok(()) => {}
                // Still no room — the mark stays so the next slot to
                // free comes back to it.
                Err(JobError::Deferred) => {}
                Err(e) => {
                    // A refusal of its own (out of disk, secrets
                    // locked). Stop calling it a cap problem, or every
                    // freed slot would retry it forever.
                    if let Some(entry) = self.job_entry(id).await {
                        entry.deferred_by_cap.store(false, Ordering::Release);
                    }
                    tracing::info!(id = %id, error = %e, "a deferred download could not start");
                }
            }
        }
    }

    /// Which queue a job belongs to; the main queue if it has gone.
    async fn queue_of(&self, id: JobId) -> QueueId {
        match self.job_entry(id).await {
            Some(entry) => entry.job.queue_id,
            None => self.main_queue_id,
        }
    }

    pub async fn start_job(self: &Arc<Self>, id: JobId) -> Result<(), JobError> {
        // The daemon is winding down; starting a transfer now would
        // either be paused a moment later or hold the exit open.
        if self.is_exiting() {
            return Err(JobError::Other("oxdm is shutting down".into()));
        }
        let entry = self
            .job_entry(id)
            .await
            .ok_or_else(|| JobError::Other("job not found".into()))?;
        // Asked before the job is claimed, so a refusal leaves it
        // exactly as it was: still queued, still startable once there
        // is room.
        let work_dir = self.settings.read().await.work_dir.clone();
        self.refuse_if_short_on_space(vec![self.space_need(&entry, &work_dir)])
            .await?;
        // A run that was cancelled a moment ago still holds `running`
        // until its task has written the outcome. Starting into that gap
        // returned Ok having spawned nothing, so Pause → Resume in quick
        // succession reported success and left the download stopped.
        let entry = self.settle_previous_run(id, entry).await;
        // Admission: the global cap governs automatic starts (a queue,
        // Resume all, the scheduler, a capture). A start the user aimed
        // at this one download runs regardless — being told "no" by a
        // number they set for background work is not what pressing play
        // means.
        let manual = entry.manual_run.load(Ordering::Acquire);
        let _admission = self.admission.lock().await;
        if !manual && self.slots_full_for(entry.job.queue_id).await {
            entry.deferred_by_cap.store(true, Ordering::Release);
            if entry.phase() != Phase::Queued {
                entry.set_phase(Phase::Queued);
                let _ = self.events.send(DomainEvent::JobUpdated {
                    id,
                    phase: Phase::Queued,
                });
            }
            return Err(JobError::Deferred);
        }
        if entry.running.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        entry.deferred_by_cap.store(false, Ordering::Release);
        drop(_admission);
        // Fresh cancel token per run. The previous run's token may be
        // already cancelled (pause trips it); installing a new one lets
        // resume actually run instead of returning Cancelled instantly.
        let token = CancellationToken::new();
        *entry.cancel.lock().unwrap_or_else(|e| e.into_inner()) = token.clone();
        // Whatever the last run concluded is no longer about this job.
        let entry = self.supersede_last_run(id, entry).await;
        // The previous run's segments are stale, but not worthless:
        // they are what the table shows until this run says otherwise.
        // Clearing here would blank it for the second or so before odl
        // announces the new parts. Marked instead, and swapped out by
        // the first `PartAdded` — see the bridge.
        entry.parts_stale.store(true, Ordering::Release);
        if let Ok(mut retrying) = entry.retrying_parts.lock() {
            retrying.clear();
        }
        let manager = self.manager.read().await.clone();
        let events = self.events.clone();
        let bridge: Arc<dyn LiveBridge> = Arc::new(StateLiveBridge {
            state: Arc::downgrade(self),
        });

        let settings = self.settings.read().await.clone();
        // Where this job's partials live. A job that has run before is
        // held to the folder it wrote into, whatever the setting says
        // now; a job running for the first time takes the current one
        // and records it.
        let work_root = entry
            .job
            .work_root
            .clone()
            .unwrap_or_else(|| settings.work_dir.clone());
        if let Err(e) = tokio::fs::create_dir_all(&work_root).await {
            // A cache folder that cannot be created is not something to
            // discover part-way through a transfer: odl would write its
            // parts relative to wherever the daemon happens to be
            // running, and the free-space check has nothing to measure.
            entry.running.store(false, Ordering::Release);
            return Err(JobError::Io(format!(
                "cannot use the cache folder {}: {e}",
                work_root.display()
            )));
        }
        if entry.job.work_root.is_none() {
            self.record_work_root(id, &work_root).await;
        }
        let per_job_dir = Some(per_job_dir(&work_root, id));
        let interactive = dialog_open_for(self, id).await;
        // No window to ask in means the download stops and waits. It
        // never opens one of its own: a background download raising a
        // dialog over whatever the user is doing puts a question under
        // their keystrokes, and the answer is not urgent — nothing
        // expires while it waits. Surfacing is left to the same rules
        // every other stopped download follows (`show_failed_dialog`,
        // `notify_failed`).
        let park_on_conflict = !interactive;

        // Effective settings overlay:
        //   global Settings → per-job overrides.
        // When any layer changes the manager-level config, build a
        // fresh `DownloadManager` off a settings copy — odl applies
        // `speed_limit` / `max_connections` / `user_agent` per Manager,
        // not per call.
        let session_override = entry.session_speed_override.load(Ordering::Acquire);
        let job_override = entry.job.speed_limit_override;
        let effective_speed = if session_override != 0 {
            Some(session_override)
        } else if let Some(o) = job_override {
            Some(o)
        } else {
            settings.speed_limit
        };

        // Per-job headers captured from the browser extension (or
        // CLI). Merge into the per-run config so odl sends them on
        // every request. A per-job `User-Agent` header is promoted to
        // `Settings::user_agent` so reqwest's builder applies it via
        // the dedicated UA setter (otherwise `default_headers` may not
        // win against the global setting).
        let job_headers = entry.job.headers.clone();
        let job_ua = job_headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("user-agent"))
            .map(|(_, v)| v.clone());

        let needs_rebuild =
            effective_speed != settings.speed_limit || !job_headers.is_empty() || job_ua.is_some();
        let runner_manager = if needs_rebuild {
            let mut s = settings.clone();
            s.speed_limit = effective_speed;
            for (k, v) in job_headers.iter() {
                if k.eq_ignore_ascii_case("user-agent") {
                    continue;
                }
                s.headers.insert(k.clone(), v.clone());
            }
            if let Some(ua) = job_ua {
                s.user_agent = Some(ua);
            }
            Arc::new(build_manager(
                &s,
                self.global_proxy_password(&s).await.as_deref(),
            ))
        } else {
            manager
        };

        // Decrypt per-job secrets just before the runner spawns.
        // Failures (Locked mode, tampered ciphertext, AAD mismatch)
        // degrade to `None` rather than abort: the download proceeds
        // without the secret and the server will surface a normal
        // 401/407 error if it actually needed one.
        let auth_password = self
            .decrypt_field(
                id,
                crate::data::crypto::Field::AuthPassword,
                entry.job.enc_auth_password.as_deref(),
            )
            .await;
        let proxy_password = self
            .decrypt_field(
                id,
                crate::data::crypto::Field::ProxyPassword,
                entry.job.enc_proxy_password.as_deref(),
            )
            .await;
        // "Send cookies" toggle is honored here — stored cookies stay
        // encrypted at rest but are only injected when enabled.
        let cookies = if entry.job.advanced.cookies_enabled {
            self.decrypt_field(
                id,
                crate::data::crypto::Field::Cookies,
                entry.job.enc_cookies.as_deref(),
            )
            .await
        } else {
            None
        };

        // Built here, not inside the runner: the UI answers conflicts
        // through `JobEntry::resolver`, so the same instance odl asks
        // has to be the one the dialog can reach.
        let resolver = Arc::new(UiResolver::new(
            id,
            events.clone(),
            interactive,
            entry.counters.clone(),
            {
                let entry = entry.clone();
                let events = events.clone();
                Box::new(move || {
                    entry.is_resumable.store(-1, Ordering::Release);
                    let _ = events.send(DomainEvent::JobUpdated {
                        id,
                        phase: entry.phase(),
                    });
                })
            },
        ));
        *entry.resolver.write().await = Some(resolver.clone());

        let runner = JobRunner {
            job_id: id,
            manager: runner_manager,
            events: events.clone(),
            cancel: token.clone(),
            bridge,
            per_job_dir,
            live_controls: entry.live_controls.clone(),
            auth_password,
            proxy_password,
            cookies,
            resolver,
        };

        let job_clone = entry.job.clone();
        let queue_id = entry.job.queue_id;
        let state = Arc::clone(self);
        entry.set_phase(Phase::Evaluating);
        let _ = self.events.send(DomainEvent::JobUpdated {
            id,
            phase: Phase::Evaluating,
        });
        // Join the queue's run, never replace it. `insert` here
        // overwrote the tally on every start — which threw away the
        // completed/failed counts the finish notification reports, and
        // cleared the flag that says this is a queue run, so the queue
        // stopped feeding itself after the first job it started this
        // way.
        //
        // No `QueueStarted`: on-start hooks belong to the queue
        // starting, and `start_queue` sends that itself. One download
        // the user pressed play on is not the queue starting, and the
        // entry made here only exists so that download's outcome is
        // counted.
        self.active_queues
            .write()
            .await
            .entry(queue_id)
            .or_default();
        tokio::spawn(async move {
            // Caught rather than allowed to unwind out of the task: the
            // epilogue below is what clears `running`, writes the phase
            // and frees the queue slot. Without it a panicked run left
            // the job flagged running for the life of the daemon —
            // `remove` refused it, `start_job` returned Ok without
            // starting anything, and the slot was never given back.
            let outcome = match futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(
                runner.run(job_clone),
            ))
            .await
            {
                Ok(outcome) => outcome,
                Err(_) => {
                    // The hook has already logged what and where.
                    tracing::error!(id = %id, "the download task panicked");
                    Err(JobError::Other(
                        "oxdm hit an internal error running this download".into(),
                    ))
                }
            };
            // Re-read the entry: anything that rebuilds a job — a
            // checksum merge, a settings edit, a queue move — replaces
            // the `Arc` in the map, and the one captured at spawn time
            // is then an orphan. Writing the outcome into it left the
            // job stuck on its last live phase, with the failure
            // recorded nowhere the UI reads.
            let entry = state.job_entry(id).await.unwrap_or(entry);
            // The run is over; a conflict answer arriving now belongs
            // to nothing. Dropped so the next run publishes its own.
            *entry.resolver.write().await = None;
            // Tally before the finish watcher: the job that empties the
            // queue must be part of the counts its own completion
            // reports.
            state
                .tally_queue_outcome(
                    queue_id,
                    id,
                    match &outcome {
                        Ok(_) => JobOutcome::Completed,
                        Err(JobError::Cancelled) => JobOutcome::Cancelled,
                        Err(e) if is_conflict(e) => JobOutcome::NeedsAnswer,
                        Err(_) => JobOutcome::Failed,
                    },
                )
                .await;
            match outcome {
                Ok(o) => {
                    // Row verdicts came from the run itself, which
                    // checked each one and said so — nothing to stamp
                    // here.
                    entry.set_phase(Phase::Completed);
                    // Stamp the completion time once. `splice_live`
                    // reads this back inside `persist_job` below, so the
                    // `finished_at` column is written in the same flush.
                    entry.finished_at_ms.store(now_ms(), Ordering::Relaxed);
                    entry.reset_live_speed();
                    // Pin counters to "100%": odl may not emit a final
                    // Progress(downloaded == total) before the Completed
                    // event, leaving the dialog and queue showing the
                    // last in-flight number.
                    match entry.counters.total() {
                        Some(total) => entry.counters.set_downloaded(total),
                        // A server that declared no length left the size
                        // unknown for the whole transfer, and the list
                        // showed "—" against a file that is now sitting
                        // on disk. What arrived is how big it is.
                        None => {
                            let got = entry.counters.downloaded();
                            if got > 0 {
                                entry.counters.set_total(Some(got));
                            }
                        }
                    }
                    let final_path = o.final_path.clone().unwrap_or_default();
                    if let Ok(mut g) = entry.final_path.write() {
                        *g = o.final_path.clone();
                    }
                    state.persist_job(id).await;
                    let _ = state.events.send(DomainEvent::JobCompleted {
                        id,
                        path: final_path,
                        already_complete: o.already_complete,
                    });
                    // Also fan out a JobUpdated so subscribers refresh
                    // their snapshot — `list_jobs` splices final_path /
                    // downloaded into the Job view, but only a fresh
                    // snapshot pulls those values into the cache.
                    let _ = state.events.send(DomainEvent::JobUpdated {
                        id,
                        phase: Phase::Completed,
                    });
                }
                Err(JobError::Cancelled) => {
                    entry.set_phase(Phase::Paused);
                    entry.reset_live_speed();
                    state.persist_job(id).await;
                    let _ = state.events.send(DomainEvent::JobUpdated {
                        id,
                        phase: Phase::Paused,
                    });
                }
                Err(err) => {
                    entry.set_phase(Phase::Failed);
                    entry.reset_live_speed();
                    // Same as a pause: it stops being the download the
                    // queue is working on. Unlike a pause, the queue
                    // does not pick it up again when it comes round —
                    // whatever went wrong is still wrong, and a queue
                    // that retries a failure every pass is a loop.
                    state.move_to_queue_end(id).await;
                    // A failed integrity check is a failure *after* the
                    // last byte arrived — the file is whole, it is just
                    // not the file that was promised. So the run has an
                    // end, and the count is the whole of it. Without
                    // both, the page that explains the failure sits
                    // above three dashes where the speed it managed,
                    // the time it took and the moment it finished
                    // should be — facts the run collected and then
                    // threw away for want of a timestamp.
                    if matches!(err, JobError::ChecksumMismatch { .. }) {
                        entry.finished_at_ms.store(now_ms(), Ordering::Relaxed);
                        if let Some(total) = entry.counters.total() {
                            entry.counters.set_downloaded(total);
                        }
                    }
                    // No blanket verdict here: the run already recorded
                    // one per row. A job with a good MD5 and a bad
                    // SHA-1 has one of each, and painting them both
                    // with the failure said the MD5 was wrong when it
                    // was the only thing that matched.
                    if let Ok(mut g) = entry.last_error.write() {
                        *g = Some(err.clone());
                    }
                    state.persist_job(id).await;
                    // Questions, not failures: each of these stops the
                    // download on something a person can settle, and
                    // answering it lets the same run continue.
                    //
                    // A checksum mismatch is deliberately not among
                    // them. Nothing about it is answerable — the file
                    // on disk is not the promised one, `resume` refuses
                    // it outright, and the only way forward is starting
                    // over. Calling that "needs your answer" would
                    // offer a decision that does not exist.
                    if park_on_conflict && is_conflict(&err) {
                        state.park_with_conflict(id, err).await;
                    } else {
                        let _ = state.events.send(DomainEvent::JobFailed { id, error: err });
                    }
                }
            }
            // Released only now, with the outcome written. Cleared
            // first, this was a lie in the gap: `start_job`'s admission
            // check saw a free job whose phase still said Downloading,
            // and `pause` → `resume` inside that gap returned Ok having
            // started nothing — the UI reported success and the
            // download stayed stopped. It is also the flag `remove`
            // refuses on, so it must outlive the phase write.
            entry.running.store(false, Ordering::Release);
            // Only now — with the phase written — does the queue get
            // asked what to do next. Run before it, this raced the
            // outcome: the job whose run had just ended still counted
            // as running, so a queue with one slot saw no room, started
            // nothing, and stopped for good the moment the phase
            // flipped underneath it.
            {
                let finish_state = state.clone();
                tokio::spawn(async move {
                    // A slot just came free: give it to the next job
                    // waiting in this queue before asking whether the
                    // queue is done, or a queue with ten downloads and
                    // room for three would run three and stop.
                    finish_state.fill_queue_slots(queue_id, Some(id)).await;
                    // Then to anything the cap itself deferred, whatever
                    // queue it belongs to — the queue filler only serves
                    // a queue the user started.
                    finish_state.fill_deferred_slots().await;
                    finish_state.maybe_finish_queue(queue_id).await;
                });
            }
            let _ = token; // keep alive for the run
            // Last thing the task does: whoever is waiting for runs to
            // drain — the shutdown — hears it here rather than
            // discovering it on a timer.
            state.run_finished.notify_waiters();
        });

        Ok(())
    }

    /// Retire everything the previous run concluded: the error it
    /// failed with, the file it produced, and every verdict about that
    /// file.
    ///
    /// A new run replaces all three. Left behind, a checksum that
    /// failed two runs ago keeps describing a file this run is busy
    /// overwriting — the list reads "Integrity check failed" over a
    /// download in flight, and Resume refuses a job with nothing yet
    /// wrong with it. Returns the entry to use from here on, which is a
    /// fresh one whenever the verdicts had to be rewritten.
    async fn supersede_last_run(&self, id: JobId, entry: Arc<JobEntry>) -> Arc<JobEntry> {
        // Including when it started and ended. These are per-run by
        // definition — `started_at` is the first `Downloading` of *this*
        // run — but only restart cleared them, so a job that was paused
        // and picked up again measured itself from the first attempt:
        // "time taken 03:10:09" for half a minute of transferring, with
        // the average speed divided by the hours it spent idle.
        entry.started_at_ms.store(0, Ordering::Release);
        entry.finished_at_ms.store(0, Ordering::Release);
        entry.retries.store(0, Ordering::Release);
        if let Ok(mut g) = entry.last_error.write() {
            *g = None;
        }
        if let Ok(mut g) = entry.final_path.write() {
            *g = None;
        }
        let stale_verdicts = entry
            .job
            .checksums
            .iter()
            .any(|c| c.status != crate::domain::CsStatus::Unverified);
        if !stale_verdicts {
            return entry;
        }
        let mut job = entry.job.clone();
        for c in &mut job.checksums {
            c.status = crate::domain::CsStatus::Unverified;
            c.expected = None;
        }
        job.status.final_path = None;
        let fresh = clone_entry_with_job(&entry, job).await;
        self.jobs.write().await.insert(id, fresh.clone());
        fresh
    }

    /// Refuse anything that would interrupt, delete or re-read a final
    /// file while it is being written.
    ///
    /// Assembly copies the parts into the finished file. Cut it short
    /// and what is left looks finished and is not — the right length,
    /// the wrong contents, and nothing on screen to say so. There is
    /// nothing to gain either: the network is already idle and the copy
    /// ends on its own. Refused in the daemon rather than only in the
    /// UI, so no path — window, list, tray, IPC — can do it.
    fn refuse_while_assembling(entry: &JobEntry) -> Result<(), JobError> {
        if entry.phase() == Phase::Assembling {
            return Err(JobError::Other(
                "the final file is being assembled; this finishes on its own".into(),
            ));
        }
        Ok(())
    }

    pub async fn pause(self: &Arc<Self>, id: JobId) -> Result<(), JobError> {
        let entry = self
            .job_entry(id)
            .await
            .ok_or_else(|| JobError::Other("job not found".into()))?;
        Self::refuse_while_assembling(&entry)?;
        let handle = JobHandle {
            id,
            cancel: entry
                .cancel
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        };
        let res = self.pause_strategy.pause(&handle).await;
        // Whatever this download was waiting for, it is not waiting any
        // more. Left set, a job the cap had deferred would be started
        // again by the slot filler moments after the user paused it —
        // and Pause all would end with a download running.
        entry.deferred_by_cap.store(false, Ordering::Release);
        // The runner outcome handler also sets phase + zeros counters on
        // Cancelled, but the runner may take a tick to wind down. Flip
        // the visible state immediately so the dialog footer button and
        // the speed/ETA cells switch the moment the user clicks Pause.
        entry.set_phase(Phase::Paused);
        entry.reset_live_speed();
        // Whoever else is waiting in this queue should not be waiting on
        // a download the user has just stopped. It keeps its place in
        // line — at the back of it.
        self.move_to_queue_end(id).await;
        self.persist_job(id).await;
        let _ = self.events.send(DomainEvent::JobUpdated {
            id,
            phase: Phase::Paused,
        });
        res
    }

    pub async fn resume(self: &Arc<Self>, id: JobId) -> Result<(), JobError> {
        let entry = self
            .job_entry(id)
            .await
            .ok_or_else(|| JobError::Other("job not found".into()))?;
        // Nothing to resume: the transfer finished and the file it
        // produced is not the promised one. Fetching the missing bytes
        // is not the answer, because none are missing — the whole file
        // has to come again, which is Restart.
        if entry.job.integrity_failed() {
            return Err(JobError::Other(
                "this file failed its integrity check; restart the download instead".into(),
            ));
        }
        // Picking a stopped transfer back up is an interruption too —
        // whether the user paused it or a failure did. A job that never
        // started has nothing to interrupt.
        if entry.counters.downloaded() > 0 {
            entry.interruptions.fetch_add(1, Ordering::Relaxed);
        }
        let handle = JobHandle {
            id,
            cancel: entry
                .cancel
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        };
        let ctx = StateResumeContext {
            state: Arc::downgrade(self),
        };
        self.pause_strategy.resume(&handle, &ctx).await
    }

    /// Stop the active transfer (if any). Does not touch files; does
    /// not remove from queue. Same on-disk state as a Pause.
    pub async fn cancel_active(self: &Arc<Self>, id: JobId) -> Result<(), JobError> {
        self.pause(id).await
    }

    /// Cancel the active transfer and reset the visible phase to
    /// `Queued` so the job leaves the tray's running/paused list and
    /// returns to its default look in the queue. Used by the download
    /// window's Cancel button — distinct from Pause (which keeps the
    /// job pinned at Paused so the user can Resume).
    ///
    /// On-disk state mirrors a Pause: `.part` files survive and the
    /// stored counters are preserved so a later Resume re-uses the
    /// existing offsets. Use `restart_job` to wipe progress.
    pub async fn cancel_to_queued(self: &Arc<Self>, id: JobId) -> Result<(), JobError> {
        self.pause(id).await?;
        let entry = self
            .job_entry(id)
            .await
            .ok_or_else(|| JobError::Other("job not found".into()))?;
        entry.reset_run_stats();
        // The abandoned run's failure goes with it: the job is back to
        // Queued, and a stale reason would still be rendered by the
        // download window, which keys the error block on the reason
        // rather than the phase.
        if let Ok(mut g) = entry.last_error.write() {
            *g = None;
        }
        entry.set_phase(Phase::Queued);
        self.persist_job(id).await;
        let _ = self.events.send(DomainEvent::JobUpdated {
            id,
            phase: Phase::Queued,
        });
        Ok(())
    }

    /// Restart a job from scratch: stop any running transfer, wipe the
    /// per-job working dir (metadata.pb + every `.part`), reset live
    /// counters, and start fresh. The assembled final file (if the job
    /// had completed) is left alone — caller decides via Remove if they
    /// want it gone.
    pub async fn restart_job(self: &Arc<Self>, id: JobId) -> Result<(), JobError> {
        let entry = self
            .job_entry(id)
            .await
            .ok_or_else(|| JobError::Other("job not found".into()))?;
        // Before the pause, whose error this path deliberately ignores:
        // restarting deletes the work directory, and doing that under a
        // running assembly pulls the parts out from under the copy.
        Self::refuse_while_assembling(&entry)?;
        // Best-effort pause; ignore "not running" errors.
        let _ = self.pause(id).await;

        // The folder this job actually wrote into, which is not
        // necessarily the one the setting points at today.
        let dir = per_job_dir(&self.work_root_of(id).await, id);
        let _ = tokio::fs::remove_dir_all(&dir).await;

        entry.reset_live_speed();
        entry.counters.reset_progress();
        entry.reset_run_stats();
        // The rest of the previous run's conclusions — its error, its
        // file, its verdicts — are retired by `start_job` below, which
        // every path into a run goes through.
        entry.set_phase(Phase::Queued);
        self.persist_job(id).await;
        let _ = self.events.send(DomainEvent::JobUpdated {
            id,
            phase: Phase::Queued,
        });
        self.start_job(id).await
    }

    /// Delete the assembled file of a completed job, leaving the job in
    /// the list. The record keeps pointing at where the file *was*: the
    /// user asked to reclaim the bytes, not to forget the download, and
    /// a history row that suddenly claims no path is a worse lie than
    /// one naming a path that is now empty.
    pub async fn delete_final_file(self: &Arc<Self>, id: JobId) -> Result<(), JobError> {
        let entry = self
            .job_entry(id)
            .await
            .ok_or_else(|| JobError::Other("job not found".into()))?;
        // A job that ran before has a path on record; deleting it now
        // would delete the file this run is in the middle of writing.
        Self::refuse_while_assembling(&entry)?;
        let path = entry
            .saved_file()
            .ok_or_else(|| JobError::Other("this download has no saved file".into()))?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {}
            // Already gone is the state the user asked for.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(JobError::Io(e.to_string())),
        }
        let _ = self.events.send(DomainEvent::JobUpdated {
            id,
            phase: entry.phase(),
        });
        Ok(())
    }

    /// Remove a job. `opts` decides whether to also wipe `metadata.pb`
    /// + `.part` + (if completed) the assembled file.
    ///
    /// `Ok(Some(msg))` means the entry is gone but the file the caller
    /// asked to delete is still on disk — read-only, in use, on a
    /// mount that vanished. The list has to lose the row either way (a
    /// removal that half-happens is worse), but a delete that quietly
    /// did not delete is exactly the kind of thing the user finds out
    /// about a month later, so it is handed back to be shown.
    pub async fn remove(
        self: &Arc<Self>,
        id: JobId,
        opts: RemoveOpts,
    ) -> Result<Option<String>, JobError> {
        let entry = self
            .job_entry(id)
            .await
            .ok_or_else(|| JobError::Other("job not found".into()))?;
        if entry.running.load(Ordering::Acquire) {
            return Err(JobError::Other(
                "cannot remove while download is in progress".into(),
            ));
        }

        if opts.purge_partial {
            // Per-job working dir holds metadata.pb + every .part +
            // lockfile. Recursive remove wipes them all without
            // touching other jobs' folders.
            let dir = per_job_dir(&self.work_root_of(id).await, id);
            let _ = tokio::fs::remove_dir_all(&dir).await;
        }
        let mut warning = None;
        if opts.delete_final_file
            && let Some(p) = entry.saved_file()
        {
            match tokio::fs::remove_file(&p).await {
                Ok(()) => {}
                // Already gone is the state the user asked for.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    tracing::warn!(id = %id, path = %p.display(), error = %e,
                        "could not delete the file while removing the job");
                    warning = Some(format!("{}: {e}", p.display()));
                }
            }
        }

        self.store
            .delete_job(id)
            .await
            .map_err(|e| JobError::Io(e.to_string()))?;
        // Per-job secrets live on the deleted row — nothing else to
        // clean up. The master key in the keyring stays put, valid
        // for the rest of the user's jobs.
        let _ = entry;
        self.jobs.write().await.shift_remove(&id);
        let _ = self.events.send(DomainEvent::JobRemoved { id });
        Ok(warning)
    }

    // ── conflict resolution callbacks (driven from UI) ─────────────

    pub async fn resolve_file_changed(
        &self,
        id: JobId,
        token: u64,
        resolution: odl::conflict::FileChangedResolution,
    ) {
        if let Some(r) = self.active_resolver(id).await {
            r.answer(token, Resolution::FileChanged(resolution));
        }
    }
    pub async fn resolve_not_resumable(
        &self,
        id: JobId,
        token: u64,
        resolution: odl::conflict::NotResumableResolution,
    ) {
        if let Some(r) = self.active_resolver(id).await {
            r.answer(token, Resolution::NotResumable(resolution));
        }
    }
    pub async fn resolve_same_download(
        &self,
        id: JobId,
        token: u64,
        resolution: odl::conflict::SameDownloadExistsResolution,
    ) {
        if let Some(r) = self.active_resolver(id).await {
            r.answer(token, Resolution::SameDownload(resolution));
        }
    }
    pub async fn resolve_final_file(
        &self,
        id: JobId,
        token: u64,
        resolution: odl::conflict::FinalFileExistsResolution,
    ) {
        if let Some(r) = self.active_resolver(id).await {
            r.answer(token, Resolution::FinalFile(resolution));
        }
    }

    async fn active_resolver(&self, id: JobId) -> Option<Arc<UiResolver>> {
        let entry = self.jobs.read().await.get(&id).cloned()?;
        entry.resolver.read().await.clone()
    }

    /// Remove every job currently in a terminal state. Any per-row
    /// "delete file on disk" decision must already have been made by
    /// the caller — bulk clear never deletes assembled files (see
    /// PLAN §4.5).
    pub async fn remove_completed_all(self: &Arc<Self>) -> usize {
        let ids: Vec<JobId> = self
            .jobs
            .read()
            .await
            .values()
            .filter(|e| e.phase() == Phase::Completed)
            .map(|e| e.job.id)
            .collect();
        let mut removed = 0;
        for id in ids {
            if self
                .remove(
                    id,
                    RemoveOpts {
                        purge_partial: false,
                        delete_final_file: false,
                    },
                )
                .await
                .is_ok()
            {
                removed += 1;
            }
        }
        removed
    }

    /// Pause every running job. Used by the tray "Pause all" item.
    /// Inject an event into the broadcast — used by tray dispatcher
    /// to ask the UI to open a download dialog for a specific job
    /// without giving the tray module direct access to the dioxus
    /// signal layer.
    pub fn events_emit(&self, event: DomainEvent) {
        let _ = self.events.send(event);
    }

    /// Synchronously trip every active job's cancellation token. Used
    /// by the tray Quit handler, which runs inside a muda callback and
    /// cannot await. CancellationToken::cancel is itself sync and
    /// non-blocking; we sidestep the async `jobs` RwLock with a
    /// best-effort `try_read`, which is fine because Quit happens at
    /// most once and any contention just means the process exits a
    /// hair sooner without checkpointing.
    pub fn cancel_all_runners(&self) {
        if let Ok(jobs) = self.jobs.try_read() {
            for entry in jobs.values() {
                // Cancelling mid-assembly leaves a final file of the
                // right length and the wrong contents. Nothing is
                // gained either: the copy is local and ends on its own.
                if entry.phase() == Phase::Assembling {
                    continue;
                }
                if let Ok(token) = entry.cancel.lock() {
                    token.cancel();
                }
            }
        }
    }

    /// Ids of every job currently running. Used by supervisors that
    /// need to act on the live set without holding the jobs lock.
    pub async fn running_job_ids(&self) -> Vec<JobId> {
        self.jobs
            .read()
            .await
            .values()
            .filter(|e| e.running.load(Ordering::Acquire))
            .map(|e| e.job.id)
            .collect()
    }

    /// Forget every "waiting for a free slot" mark.
    ///
    /// Pause all and Stop all mean *stop*: a queued download that was
    /// only waiting on the cap must not be started by the slot filler a
    /// moment later. It stays queued for the user to start again.
    async fn clear_deferrals(&self) {
        for entry in self.jobs.read().await.values() {
            entry.deferred_by_cap.store(false, Ordering::Release);
        }
    }

    pub async fn pause_all(self: &Arc<Self>) {
        self.clear_deferrals().await;
        // A job writing its final file is left alone: `pause` refuses
        // it anyway, and asking is how a shutdown ends up logging an
        // error for doing the right thing.
        let ids: Vec<JobId> = self
            .jobs
            .read()
            .await
            .values()
            .filter(|e| e.running.load(Ordering::Acquire) && e.phase() != Phase::Assembling)
            .map(|e| e.job.id)
            .collect();
        for id in ids {
            let _ = self.pause(id).await;
        }
    }

    /// Stop everything: pause every running download, and hand the
    /// queues back to the user.
    ///
    /// `pause_all` on its own leaves every queue marked as running,
    /// which is not a cosmetic detail — the toolbar goes on offering
    /// "Stop queue" for a queue with nothing in flight, and the run
    /// stays open forever because nothing will ever drain it.
    ///
    /// No `QueueFinished`: the queues did not finish, they were
    /// stopped, and an on-finish hook that fires when the user presses
    /// Stop all is a shutdown nobody asked for.
    pub async fn stop_all(self: &Arc<Self>) {
        // Queues first, and not for tidiness: while a queue is still
        // marked as running it is something that starts downloads, so
        // pausing first leaves a window in which the queue machinery
        // can put back what was just stopped. Ending the runs first
        // means everything still transferring afterwards is a download
        // somebody started by hand.
        let stopped: Vec<QueueId> = {
            let mut active = self.active_queues.write().await;
            let ids = active.keys().copied().collect();
            active.clear();
            ids
        };
        for id in stopped {
            tracing::info!(queue = %id, "queue run ended by Stop all");
            // The list still has to repaint: its Start/Stop button is
            // keyed on whether the queue is active.
            let _ = self.events.send(DomainEvent::QueueStopped { id });
        }
        self.pause_all().await;
    }

    /// Resume every job that is not already running or done — failed
    /// ones included, same rule as `start_queue`: "resume everything"
    /// that silently skips the failures is not what it says.
    pub async fn resume_all(self: &Arc<Self>) {
        // A download whose checksum failed is skipped: it has every
        // byte already, so "resume" would silently fetch the whole file
        // again — a decision the user makes with Restart, not one a
        // bulk command makes for them.
        let ids: Vec<JobId> = self
            .jobs
            .read()
            .await
            .values()
            .filter(|e| e.phase().is_startable() && !e.job.integrity_failed())
            .map(|e| e.job.id)
            .collect();
        for id in ids {
            self.mark_run_intent(id, false).await;
            let _ = self.resume(id).await;
        }
    }

    pub async fn update_channel(&self) -> Arc<dyn UpdateChannel> {
        // Re-derive on each call so a settings change reaches the next
        // user-triggered "Check for updates". UpdateChannel itself is
        // cheap to construct (no real I/O until called).
        let s = self.settings.read().await.clone();
        crate::data::update_channel::from_settings(&s)
    }

    pub async fn ext_token(&self) -> String {
        self.ext_token.read().await.clone()
    }
}

/// Result of `AppState::probe`. All fields are best-effort — anything
/// the server did not advertise stays `None` / empty.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProbeResult {
    pub filename: String,
    pub size: Option<u64>,
    pub is_resumable: bool,
    pub etag: Option<String>,
    pub last_modified: Option<i64>,
    pub requires_auth: bool,
    /// Digests the server published in its headers. Carried so a job
    /// added from a probe already knows what it will be checked
    /// against, instead of learning it on the first run.
    pub checksums: Vec<crate::domain::Checksum>,
}

/// What a caller's probe learned about the file, for `add_job`.
///
/// Grouped rather than passed as loose arguments: these all answer the
/// same question — "what did the probe already tell us?" — and callers
/// that made no probe say so once with `default()`.
#[derive(Debug, Clone, Default)]
pub struct ProbeFacts {
    pub size: Option<u64>,
    pub checksums: Vec<crate::domain::Checksum>,
}

#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
pub struct RemoveOpts {
    /// Wipe `metadata.pb` + `.part` files for an incomplete job.
    pub purge_partial: bool,
    /// Delete the assembled final file (only meaningful for completed).
    pub delete_final_file: bool,
}

/// Hash `path` once per algorithm the rows name, and judge each row
/// against it. Returns the per-row verdicts — `(row index, verdict,
/// computed digest)`, the digest only where it disagrees, since a
/// matching row has nothing to show beside itself — and every digest
/// computed along the way, so the caller can spare the next check.
///
/// `known` seeds that map: an algorithm already answered for this file
/// is not read from disk again.
///
/// Rows whose hash is malformed are left alone rather than failed:
/// "this is not a hash" is not the same claim as "this file is wrong".
#[allow(clippy::type_complexity)]
async fn hash_against_rows(
    path: &std::path::Path,
    rows: &[crate::domain::Checksum],
    known: std::collections::HashMap<crate::domain::Algo, String>,
) -> Result<
    (
        Vec<(usize, crate::domain::CsStatus, Option<String>)>,
        std::collections::HashMap<crate::domain::Algo, String>,
    ),
    String,
> {
    use crate::domain::CsStatus;
    let mut computed = known;
    let mut out = Vec::new();
    for (i, c) in rows.iter().enumerate() {
        if c.hash.trim().len() != c.algo.hex_len() {
            continue;
        }
        let digest = match computed.get(&c.algo) {
            Some(d) => d.clone(),
            None => {
                let d = odl::hash::HashDigest::from_path(
                    path,
                    crate::data::mapping::odl_algorithm(c.algo),
                    odl::hash::HashEncoding::Hex,
                )
                .await
                .map_err(|e| e.to_string())?
                .digest()
                .to_ascii_lowercase();
                computed.insert(c.algo, d.clone());
                d
            }
        };
        if digest.eq_ignore_ascii_case(c.hash.trim()) {
            out.push((i, CsStatus::Verified, None));
        } else {
            out.push((i, CsStatus::Mismatch, Some(digest)));
        }
    }
    Ok((out, computed))
}

/// Materialise a Job view that reflects live (in-memory) state on top
/// of the load-time snapshot held by `JobEntry::job`. The `Job` struct
/// is immutable in the registry, so progress and completion data live
/// elsewhere — atomic counters, the live phase byte, and the
/// completion-only `final_path` cell. UI consumers want a single Job
/// value with all of those folded in.
pub(crate) fn splice_live(entry: &JobEntry) -> Job {
    let mut j = entry.job.clone();
    j.status.phase = entry.phase();
    // Why the last run failed, for any window that opens after it did
    // — including after a restart, since this round-trips to the store
    // through `persist_job`.
    if let Ok(g) = entry.last_error.read() {
        j.status.error = g.clone();
    }
    j.status.downloaded = entry.counters.downloaded();
    if let Some(t) = entry.counters.total() {
        j.status.total = Some(t);
    }
    if let Ok(g) = entry.final_path.read()
        && let Some(p) = g.clone()
    {
        j.status.final_path = Some(p);
    }
    // A probe from this session supersedes the stored capture; with no
    // probe yet the persisted one (possibly from a past run) stands.
    if let Ok(g) = entry.captured_response.read()
        && let Some(c) = g.clone()
    {
        j.captured_response = Some(c);
    }
    // Overlay live run-stats from the entry atomics (epoch-ms → option,
    // `0` = None). These are the authoritative in-session values;
    // `persist_job` round-trips them back to the store via this same
    // splice, so the columns stay in sync.
    j.started_at = ms_to_datetime(entry.started_at_ms.load(Ordering::Relaxed));
    // Zero means the run never reached `Downloading`, which is not the
    // same fact as "nothing recorded" and would read as an instant
    // transfer.
    let active = entry.active_ms();
    j.active_ms = (active > 0).then_some(active as u64);
    j.finished_at = ms_to_datetime(entry.finished_at_ms.load(Ordering::Relaxed));
    j.retries = entry.retries.load(Ordering::Relaxed);
    j.interruptions = entry.interruptions.load(Ordering::Relaxed);
    j
}

/// Drop a recorded integrity failure once no row disagrees with the
/// file any more.
///
/// The verdict is about a hash, and the hashes are the user's list:
/// deleting the one that did not match, or replacing it with one that
/// does, answers the question the failure was asking. Leaving it
/// recorded would keep a file condemned by a line that is no longer
/// there — and keep it unresumable, since `integrity_failed` reads the
/// error as well as the rows.
///
/// Only the checksum failure is cleared. Every other one is about bytes
/// that never arrived, and no edit to a hash list fetches them.
fn clear_settled_mismatch(entry: &JobEntry) {
    if entry
        .job
        .checksums
        .iter()
        .any(|c| c.status == crate::domain::CsStatus::Mismatch)
    {
        return;
    }
    let Ok(mut last) = entry.last_error.write() else {
        return;
    };
    if !matches!(*last, Some(JobError::ChecksumMismatch { .. })) {
        return;
    }
    *last = None;
    // The transfer had reached its end — the file is whole and
    // assembled, and the mismatch was the only reason the run counted
    // as a failure at all.
    if entry.phase() == Phase::Failed && entry.saved_file().is_some() {
        entry.set_phase(Phase::Completed);
    }
}

/// Current wall-clock as epoch milliseconds — matches the `0 = None`
/// encoding used by the run-stat atomics.
fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Epoch-milliseconds → `Option<DateTime<Utc>>`, treating `0` as None.
fn ms_to_datetime(ms: i64) -> Option<chrono::DateTime<chrono::Utc>> {
    if ms == 0 {
        None
    } else {
        chrono::DateTime::from_timestamp_millis(ms)
    }
}

/// Per-job working directory for `metadata.pb` + every `.part`. Lives
/// under the user's configured `work_dir` (app data dir by default),
/// completely independent of the job's `save_dir` final destination.
/// Stable across renames / save-dir retargets so an in-flight job does
/// not lose its partial state when the user edits its destination.
pub fn per_job_dir(work_dir: &std::path::Path, id: JobId) -> std::path::PathBuf {
    work_dir.join(per_job_dir_name(id))
}

/// Per-job completion prefs for a newly-tracked job: defaults, with the
/// dialog opt-in taking the global setting as its starting point. The
/// per-job toggle then overrides the global for that download.
fn seeded_completion(settings: &Settings) -> crate::domain::OnCompletion {
    crate::domain::OnCompletion {
        show_dialog: settings.show_complete_dialog,
        ..Default::default()
    }
}

/// Directory-name prefix every per-job working dir carries. The reset
/// sweep keys off it, so the two must not drift apart.
const PER_JOB_PREFIX: &str = ".oxdm-";

fn per_job_dir_name(id: JobId) -> String {
    format!("{PER_JOB_PREFIX}{}", id.0.simple())
}

/// SQLite sidecars that must travel with (or die with) the main DB file.
const DB_SIDECARS: [&str; 3] = ["-wal", "-shm", "-journal"];

fn sidecar(db: &std::path::Path, suffix: &str) -> std::path::PathBuf {
    let mut s = db.as_os_str().to_os_string();
    s.push(suffix);
    std::path::PathBuf::from(s)
}

/// Delete every per-job working dir under `work_dir`, returning how many
/// went. Matching is by directory-name prefix so a `work_dir` the user
/// shares with their own files keeps everything that is not ours; a
/// symlink pointing outside is unlinked, never followed.
///
/// Best-effort: an unreadable dir or a failed removal is logged and
/// skipped rather than aborting the reset, which must still get to the
/// point of dropping the DB.
fn purge_work_dir_partials(work_dir: &std::path::Path) -> usize {
    let entries = match std::fs::read_dir(work_dir) {
        Ok(e) => e,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(dir = %work_dir.display(), error = %e, "reset: cannot scan work dir");
            }
            return 0;
        }
    };
    let mut purged = 0;
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with(PER_JOB_PREFIX)
        {
            continue;
        }
        let path = entry.path();
        // `file_type` does not follow symlinks: a symlinked `.oxdm-*` is
        // removed as a link, so we never recurse into whatever it aims at.
        let removed = match entry.file_type() {
            Ok(t) if t.is_dir() => std::fs::remove_dir_all(&path),
            _ => std::fs::remove_file(&path),
        };
        match removed {
            Ok(()) => purged += 1,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "reset: cannot remove partial")
            }
        }
    }
    purged
}

/// Is another job already called `name`?
///
/// Every job in the table counts, whatever folder it saves to and
/// whatever state it is in: the point is that one name identifies one
/// download, in the list as much as on disk. `except` is the job doing
/// the asking, so an edit that leaves the name alone is not a clash
/// with itself.
fn name_is_taken(jobs: &IndexMap<JobId, Arc<JobEntry>>, name: &str, except: Option<JobId>) -> bool {
    let key = crate::domain::name_key(name);
    if key.is_empty() {
        return false;
    }
    jobs.iter()
        .filter(|(id, _)| Some(**id) != except)
        .filter_map(|(_, e)| e.job.filename.as_deref())
        .any(|n| crate::domain::name_key(n) == key)
}

/// The downloads "Stop queue" pauses: the ones the queue is running of
/// its own accord.
///
/// `current` with `keys` taken from `edited`.
///
/// By key on the serialized form rather than field by field, so a new
/// setting cannot be forgotten here and silently stop being saveable.
fn merge_settings_fields(
    current: &Settings,
    edited: &Settings,
    keys: &[String],
) -> Result<Settings, String> {
    let mut base = match serde_json::to_value(current) {
        Ok(serde_json::Value::Object(map)) => map,
        _ => return Err("settings are not an object".into()),
    };
    let from = match serde_json::to_value(edited) {
        Ok(serde_json::Value::Object(map)) => map,
        _ => return Err("settings are not an object".into()),
    };
    for key in keys {
        match from.get(key) {
            Some(value) => {
                base.insert(key.clone(), value.clone());
            }
            // A key the sender named but does not have: newer client,
            // older daemon. Ignored rather than failing the save.
            None => tracing::warn!(%key, "unknown setting in an update; ignoring it"),
        }
    }
    serde_json::from_value(serde_json::Value::Object(base))
        .map_err(|e| format!("could not apply the settings: {e}"))
}

/// An update download in flight: which job is fetching it, and what
/// the feed said it should hash to.
#[derive(Debug, Clone)]
pub struct PendingUpdate {
    pub job: JobId,
    pub info: crate::data::UpdateInfo,
}

/// Is this a SHA-256 digest at all? An artifact whose digest is
/// missing or malformed cannot be checked, and an unverifiable update
/// is one oxdm has no business running.
fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Where update artifacts are staged: a per-user 0700 folder, never
/// the shared temp dir — the file becomes the executable oxdm replaces
/// itself with, so anyone able to write it owns the next launch.
fn update_staging_dir() -> std::io::Result<std::path::PathBuf> {
    let dir = dirs::data_local_dir()
        .or_else(dirs::data_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join("oxdm")
        .join("updates");
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}

/// A cache folder oxdm can actually write partials into.
///
/// Blank and relative are both refused: neither names a place, and the
/// consequences are silent — parts written next to whatever the daemon
/// was launched from, and a free-space precheck with no volume to
/// measure, which then treats every download as fitting.
fn validate_work_dir(dir: &std::path::Path) -> Result<(), String> {
    if dir.as_os_str().is_empty() {
        return Err("the cache folder cannot be blank".into());
    }
    if !dir.is_absolute() {
        return Err(format!(
            "the cache folder must be a full path — `{}` is relative",
            dir.display()
        ));
    }
    Ok(())
}

/// Is there room for one more download in `queue`?
///
/// `global` is shared across every queue and `per_queue` is this
/// queue's own limit, so a queue that allows ten still runs one at a
/// time under a global cap of one, and two queues allowing three each
/// run five between them under a global cap of five. `None` for
/// `per_queue` means the queue defers entirely to the global limit.
///
/// Counted by phase rather than by the `running` flag: the phase is
/// what every other surface calls "running", and the flag lags it at
/// both ends of a run.
fn slots_full(
    jobs: &IndexMap<JobId, Arc<JobEntry>>,
    global: usize,
    queue: QueueId,
    per_queue: Option<usize>,
) -> bool {
    let running_global = jobs.values().filter(|e| e.phase().is_running()).count();
    if running_global >= global.max(1) {
        return true;
    }
    match per_queue {
        Some(cap) => {
            let running_here = jobs
                .values()
                .filter(|e| e.job.queue_id == queue && e.phase().is_running())
                .count();
            running_here >= cap.max(1)
        }
        None => false,
    }
}

/// The next download the cap sent back to the queue, in list order,
/// skipping any this pass has already picked up.
fn next_deferred(
    jobs: &IndexMap<JobId, Arc<JobEntry>>,
    skip: &std::collections::HashSet<JobId>,
) -> Option<JobId> {
    jobs.values()
        .find(|e| {
            e.deferred_by_cap.load(Ordering::Acquire)
                && e.phase() == Phase::Queued
                && !e.running.load(Ordering::Acquire)
                && !skip.contains(&e.job.id)
        })
        .map(|e| e.job.id)
}

/// Membership is by phase, not by the `running` flag: a job that is
/// evaluating or reconnecting is a download this queue is doing, and
/// the flag lags the phase by a moment at both ends of a run.
///
/// Assembly is excluded rather than asked and refused: writing the
/// final file is not a transfer a queue can stop, and a pause it will
/// reject is not worth sending.
///
/// A hand-started download is excluded because the user asked for that
/// download, not for the queue. Stopping the queue takes back what the
/// queue decided to run; taking back their press as well would make
/// Resume on a row mean "until the queue is next stopped".
fn queue_stop_targets(jobs: &IndexMap<JobId, Arc<JobEntry>>, queue: QueueId) -> Vec<JobId> {
    jobs.values()
        .filter(|e| {
            e.job.queue_id == queue
                && e.phase().is_running()
                && e.phase() != Phase::Assembling
                && !e.manual_run.load(Ordering::Acquire)
        })
        .map(|e| e.job.id)
        .collect()
}

/// `name`, or the numbered variant of it that no other job holds.
fn free_name(jobs: &IndexMap<JobId, Arc<JobEntry>>, name: &str, except: Option<JobId>) -> String {
    crate::domain::unique_name(name, |candidate| name_is_taken(jobs, candidate, except))
}

/// Rebuild a `JobEntry` carrying every sticky field forward but with a
/// new `Job` value. `JobEntry` is held inside an `Arc` shared with
/// active runners, so we cannot mutate in place — produce a fresh `Arc`
/// and let the caller swap it into the registry. Used wherever the
/// stored `Job` mutates (queue move, persisted speed cap, …).
async fn clone_entry_with_job(old: &Arc<JobEntry>, new_job: Job) -> Arc<JobEntry> {
    // Pre-collect every sync-locked field before the await so no
    // !Send guard is held across it.
    let parts = old.parts.read().map(|g| g.clone()).unwrap_or_default();
    let cancel_token = old.cancel.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let on_completion = old
        .on_completion
        .read()
        .map(|g| g.clone())
        .unwrap_or_default();
    let final_path = old.final_path.read().map(|g| g.clone()).unwrap_or(None);
    let captured_response = old
        .captured_response
        .read()
        .map(|g| g.clone())
        .unwrap_or(None);
    let retrying_parts = old
        .retrying_parts
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default();
    let resolver = old.resolver.read().await.clone();
    Arc::new(JobEntry {
        job: new_job,
        live_phase: AtomicU8::new(old.live_phase.load(Ordering::Acquire)),
        counters: old.counters.clone(),
        active_ms: AtomicI64::new(old.active_ms.load(Ordering::Acquire)),
        downloading_since_ms: AtomicI64::new(old.downloading_since_ms.load(Ordering::Acquire)),
        started_at_ms: AtomicI64::new(old.started_at_ms.load(Ordering::Acquire)),
        finished_at_ms: AtomicI64::new(old.finished_at_ms.load(Ordering::Acquire)),
        retries: AtomicU32::new(old.retries.load(Ordering::Acquire)),
        interruptions: AtomicU32::new(old.interruptions.load(Ordering::Acquire)),
        parts_stale: AtomicBool::new(old.parts_stale.load(Ordering::Acquire)),
        verifying: AtomicBool::new(old.verifying.load(Ordering::Acquire)),
        retrying_parts: std::sync::Mutex::new(retrying_parts),
        parts: std::sync::RwLock::new(parts),
        cancel: std::sync::Mutex::new(cancel_token),
        running: AtomicBool::new(old.running.load(Ordering::Acquire)),
        last_error: std::sync::RwLock::new(old.last_error.read().ok().and_then(|g| g.clone())),
        manual_run: AtomicBool::new(old.manual_run.load(Ordering::Acquire)),
        deferred_by_cap: AtomicBool::new(old.deferred_by_cap.load(Ordering::Acquire)),
        is_resumable: std::sync::atomic::AtomicI8::new(old.is_resumable.load(Ordering::Acquire)),
        captured_response: std::sync::RwLock::new(captured_response),
        session_speed_override: std::sync::atomic::AtomicU64::new(
            old.session_speed_override.load(Ordering::Acquire),
        ),
        on_completion: std::sync::RwLock::new(on_completion),
        // Carried over: the bytes on disk did not change because a row
        // was added to the list describing them.
        hashed: std::sync::Mutex::new(old.hashed.lock().ok().and_then(|g| g.clone())),
        resolver: RwLock::new(resolver),
        final_path: std::sync::RwLock::new(final_path),
        live_controls: old.live_controls.clone(),
    })
}

/// Helper used by `start_job` to decide whether the runner should drive
/// conflicts through the UI resolver (interactive) or the headless
/// AbortAll path. A job is "interactive" iff its download dialog is
/// currently the one on screen — see milestone 9 / `PLAN §9 q4`.
async fn dialog_open_for(state: &Arc<AppState>, id: JobId) -> bool {
    *state.dialog_visible_for.read().await == Some(id)
}

/// Cryptographically-random URL-safe token. 256 bits, base64url.
fn generate_token() -> String {
    use rand::RngCore;
    let mut buf = [0u8; 32];
    rand::rng().fill_bytes(&mut buf);
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

/// Build a single copy-pasteable pairing code that bundles the IPC
/// port and the auth token. Format: `oxdm1.<base64url(port_be_u16 ||
/// 32_token_bytes)>`. Users paste one string into the extension
/// Options page instead of copying port + token separately.
pub fn encode_pairing_code(port: u16, token_b64: &str) -> String {
    use base64::Engine;
    let token_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token_b64)
        .unwrap_or_default();
    if token_bytes.len() != 32 {
        // Fallback: encode the displayed string anyway — round-trip
        // still works, the bundle is just a bit longer.
        let mut buf = port.to_be_bytes().to_vec();
        buf.extend_from_slice(token_b64.as_bytes());
        return format!(
            "oxdm1.{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
        );
    }
    let mut buf = Vec::with_capacity(34);
    buf.extend_from_slice(&port.to_be_bytes());
    buf.extend_from_slice(&token_bytes);
    format!(
        "oxdm1.{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
    )
}

/// Inverse of [`encode_pairing_code`]. Accepts the canonical form
/// `oxdm1.<base64url(port_be_u16 || 32_token_bytes)>` and the
/// fallback form where the token was preserved as the original
/// base64url string. Returns `(port, token_as_base64url)`.
pub fn decode_pairing_code(code: &str) -> Option<(u16, String)> {
    use base64::Engine;
    let rest = code.strip_prefix("oxdm1.")?;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(rest.trim())
        .ok()?;
    if raw.len() < 3 {
        return None;
    }
    let port = u16::from_be_bytes([raw[0], raw[1]]);
    if raw.len() == 34 {
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&raw[2..]);
        return Some((port, token));
    }
    // Fallback form — token was stored as its base64url string.
    let token = String::from_utf8(raw[2..].to_vec()).ok()?;
    Some((port, token))
}

fn build_manager(settings: &Settings, proxy_password: Option<&str>) -> DownloadManager {
    match settings_to_odl_config(settings, proxy_password) {
        Ok(cfg) => DownloadManager::new(cfg),
        Err(e) => {
            tracing::warn!(error = %e, "settings → odl config failed; using odl defaults");
            DownloadManager::new(odl::config::Config::default())
        }
    }
}

/// One queue run's terminal outcomes. Reset every time the queue goes
/// active, because "how did this run go" is the only question the
/// finish notification can answer honestly — the queue's job list still
/// holds whatever earlier runs left behind.
#[derive(Default, Clone)]
struct QueueRunTally {
    completed: u32,
    failed: u32,
    /// Stopped waiting for the user to answer something.
    needs_answer: u32,
    /// Jobs that failed *during this run*.
    ///
    /// A queue does not retry these when it comes back round — the
    /// reason they failed is still true, and a queue that tries them
    /// every pass spins forever. A job that was already failed when the
    /// run started is not in here: starting a queue full of failures is
    /// how the user asks for them to be tried again.
    failed_now: std::collections::HashSet<JobId>,
    /// The whole queue was started, rather than one job in it. Only a
    /// queue run feeds itself: someone who started a single download by
    /// hand asked for that download, and answering by starting the
    /// other forty behind it would be oxdm deciding for them.
    queue_run: bool,
}

/// Did the run stop on a question the user can settle, rather than on
/// something that went wrong?
///
/// A checksum mismatch is deliberately not among these: nothing about
/// it is answerable — the file on disk is not the promised one and the
/// only way forward is starting over.
fn is_conflict(e: &JobError) -> bool {
    matches!(
        e,
        JobError::ServerConflict(_)
            | JobError::NotResumable(_)
            | JobError::FileChanged(_)
            | JobError::SaveConflict(_)
    )
}

/// A finished job's contribution to its queue's tally. Cancelled (the
/// user paused or stopped it) is neither a success nor a failure.
#[derive(Clone, Copy)]
enum JobOutcome {
    Completed,
    Failed,
    /// Stopped on a question — the file changed, the name is taken,
    /// the server refused to resume. Not a failure: nothing is broken
    /// and the user can settle it, but the download will not move
    /// until they do.
    NeedsAnswer,
    Cancelled,
}

/// Stand-in key for a retry that belongs to no single part.
const WHOLE_JOB_RETRY: &str = "\u{0}whole-job";

/// AAD identity for secrets that belong to the app rather than to a
/// job. The nil UUID is never a real `JobId`, so a global ciphertext
/// cannot be replayed as a job's and vice versa.
const GLOBAL_SECRET_ID: JobId = JobId(uuid::Uuid::nil());

/// `LiveBridge` impl that walks back to `AppState` via a weak ref.
struct StateLiveBridge {
    state: std::sync::Weak<AppState>,
}

#[async_trait::async_trait]
impl LiveBridge for StateLiveBridge {
    fn on_evaluated(&self, id: JobId, is_resumable: bool) {
        let Some(state) = self.state.upgrade() else {
            return;
        };
        if let Ok(jobs) = state.jobs.try_read()
            && let Some(entry) = jobs.get(&id)
        {
            entry
                .is_resumable
                .store(if is_resumable { 1 } else { -1 }, Ordering::Release);
        }
    }

    fn on_response_headers(&self, id: JobId, captured: crate::domain::CapturedResponse) {
        let Some(state) = self.state.upgrade() else {
            return;
        };
        if let Ok(jobs) = state.jobs.try_read()
            && let Some(entry) = jobs.get(&id)
            && let Ok(mut slot) = entry.captured_response.write()
        {
            *slot = Some(captured);
        }
    }

    async fn on_checksum_results(
        &self,
        id: JobId,
        results: Vec<(usize, crate::domain::CsStatus, Option<String>)>,
    ) {
        let Some(state) = self.state.upgrade() else {
            return;
        };
        state.apply_checksum_results(id, results).await;
    }

    fn on_final_path(&self, id: JobId, path: std::path::PathBuf) {
        let Some(state) = self.state.upgrade() else {
            return;
        };
        if let Ok(jobs) = state.jobs.try_read()
            && let Some(entry) = jobs.get(&id)
            && let Ok(mut slot) = entry.final_path.write()
        {
            *slot = Some(path.clone());
        }
        // odl may have chosen a different name than the job carries —
        // a file of that name was already in the folder, so it wrote
        // `setup (1).exe`. Only the runtime slot used to learn that, so
        // the row, and everything keyed off it, went on naming a file
        // belonging to somebody else.
        tokio::spawn(async move {
            state.adopt_final_path(id, path).await;
        });
    }

    async fn on_server_checksums(&self, id: JobId, checksums: Vec<crate::domain::Checksum>) {
        let Some(state) = self.state.upgrade() else {
            return;
        };
        state.merge_server_checksums(id, checksums).await;
    }

    async fn on_filename_resolved(
        &self,
        id: JobId,
        filename: String,
    ) -> Option<std::path::PathBuf> {
        let state = self.state.upgrade()?;
        state.apply_resolved_filename(id, filename).await
    }

    fn on_event(&self, id: JobId, event: &OdlProgressEvent) {
        let Some(state) = self.state.upgrade() else {
            return;
        };
        // Update counters synchronously (cheap atomics) — we cannot await
        // here. RwLocks on `parts` are accessed via try_read/try_write to
        // avoid blocking the reporter. Drops on contention are fine: the
        // UI is sampling at 8 Hz from the same atomics anyway.
        match event {
            OdlProgressEvent::Progress { downloaded, total } => {
                if let Ok(jobs) = state.jobs.try_read()
                    && let Some(entry) = jobs.get(&id)
                {
                    // odl's sample is what this run has; it replaces
                    // whatever was on screen. A monotonic rule kept the
                    // *previous* run's byte count — a download that
                    // finished, failed its check and was started again
                    // showed 100% beside segments at 40%.
                    //
                    // Since 2.2 the aggregate belongs to the transfer
                    // alone — assembly and verification report on their
                    // own rows — and the downloader closes it with one
                    // final sample at the full size. Only the `0` odl
                    // emits while re-evaluating a resume, before it has
                    // read the part offsets back off disk, is still
                    // dropped.
                    if *downloaded > 0 {
                        entry.counters.set_downloaded(*downloaded);
                    }
                    entry.counters.set_total(*total);
                }
            }
            OdlProgressEvent::Speed { bytes_per_second } => {
                if let Ok(jobs) = state.jobs.try_read()
                    && let Some(entry) = jobs.get(&id)
                {
                    entry.counters.set_speed(*bytes_per_second);
                }
            }
            OdlProgressEvent::PartAdded { ulid, offset, size } => {
                if let Ok(jobs) = state.jobs.try_read()
                    && let Some(entry) = jobs.get(&id)
                    && let Ok(mut parts) = entry.parts.try_write()
                {
                    // First part of a new run: the rows still on screen
                    // belong to the last one. Swap, rather than leaving
                    // both sets to pile up.
                    if entry.parts_stale.swap(false, Ordering::AcqRel) {
                        parts.clear();
                        // A run that cannot resume starts at byte zero:
                        // whatever the last attempt fetched went with
                        // it. The monotonic guard on `Progress` exists
                        // for the transient zero odl emits while
                        // re-evaluating a resume, and would otherwise
                        // hold the old figure here — leaving the window
                        // claiming 176 MB while its only segment reads
                        // 52 MB and the file on disk is empty.
                        if entry.is_resumable.load(Ordering::Acquire) < 0 {
                            entry.counters.set_downloaded(0);
                        }
                    }
                    parts.insert(
                        ulid.clone(),
                        Arc::new(PartCounters {
                            ulid: ulid.clone(),
                            offset: *offset,
                            size: AtomicU64::new(crate::data::runner::part_size(*size)),
                            downloaded: AtomicU64::new(0),
                            speed_bps_bits: AtomicU64::new(0),
                            finished: AtomicBool::new(false),
                            sampled_at_ms: std::sync::atomic::AtomicI64::new(0),
                        }),
                    );
                }
            }
            // A restart re-split the download: the ulids announced so
            // far name nothing, and no `PartFinished` is coming for
            // them. Their bytes went with them, so the job's own count
            // goes back to zero rather than counting a transfer that
            // was thrown away.
            OdlProgressEvent::PartsCleared => {
                if let Ok(jobs) = state.jobs.try_read()
                    && let Some(entry) = jobs.get(&id)
                {
                    if let Ok(mut parts) = entry.parts.try_write() {
                        parts.clear();
                    }
                    entry.parts_stale.store(false, Ordering::Release);
                    entry.counters.set_downloaded(0);
                }
            }
            OdlProgressEvent::PartProgress {
                ulid,
                downloaded,
                total,
            } => {
                if let Ok(jobs) = state.jobs.try_read()
                    && let Some(entry) = jobs.get(&id)
                {
                    if let Ok(parts) = entry.parts.try_read()
                        && let Some(p) = parts.get(ulid)
                    {
                        p.apply_progress(*downloaded, *total);
                        // odl samples every part it has in flight on a
                        // fixed cadence, so this arriving is the part
                        // saying it still holds a connection — even in
                        // a tick where no bytes landed.
                        p.mark_sampled(now_ms());
                    }
                    // This part is making progress again — drop it from
                    // the retrying set. Removing by ulid (not a blanket
                    // counter decrement) means a sibling part's tick
                    // can't clear a still-retrying part. When the last
                    // retrying part clears, restore Downloading — but
                    // only if we are still in Reconnecting, so we never
                    // clobber a later Assembling / Verifying transition.
                    if let Ok(mut retrying) = entry.retrying_parts.lock()
                        && retrying.remove(ulid)
                        && retrying.is_empty()
                        && entry.phase() == Phase::Reconnecting
                    {
                        entry.set_phase(Phase::Downloading);
                    }
                }
            }
            // The wait before the next attempt. odl announces it when
            // the wait *starts*, and `PartRetrying` only when the retry
            // fires — so without this the row said "Downloading" for
            // the whole wait and flickered through Reconnecting for an
            // instant afterwards. A download waiting to try again is
            // the state the user wants named.
            OdlProgressEvent::RetryScheduled { ulid, .. } => {
                if let Ok(jobs) = state.jobs.try_read()
                    && let Some(entry) = jobs.get(&id)
                {
                    if let Ok(mut retrying) = entry.retrying_parts.lock() {
                        // A whole-download retry (the initial probe) has
                        // no part to key on; it clears when the next
                        // phase change lands.
                        retrying.insert(ulid.clone().unwrap_or_else(|| WHOLE_JOB_RETRY.to_owned()));
                    }
                    entry.set_phase(Phase::Reconnecting);
                }
            }
            OdlProgressEvent::PartRetrying { ulid, .. } => {
                if let Ok(jobs) = state.jobs.try_read()
                    && let Some(entry) = jobs.get(&id)
                {
                    // Count every retry event (plan W2). Mark this part
                    // as retrying and surface the Reconnecting banner
                    // while ≥1 part is mid-retry (plan W1).
                    entry.retries.fetch_add(1, Ordering::Relaxed);
                    // A dropped connection is an interruption whether or
                    // not the retry succeeds.
                    entry.interruptions.fetch_add(1, Ordering::Relaxed);
                    if let Ok(mut retrying) = entry.retrying_parts.lock() {
                        retrying.insert(ulid.clone());
                    }
                    entry.set_phase(Phase::Reconnecting);
                }
            }
            OdlProgressEvent::PartSpeed {
                ulid,
                bytes_per_second,
            } => {
                if let Ok(jobs) = state.jobs.try_read()
                    && let Some(entry) = jobs.get(&id)
                    && let Ok(parts) = entry.parts.try_read()
                    && let Some(p) = parts.get(ulid)
                {
                    p.speed_bps_bits
                        .store(bytes_per_second.to_bits(), Ordering::Relaxed);
                    p.mark_sampled(now_ms());
                }
            }
            OdlProgressEvent::PartFinished { ulid } => {
                if let Ok(jobs) = state.jobs.try_read()
                    && let Some(entry) = jobs.get(&id)
                    && let Ok(parts) = entry.parts.try_read()
                    && let Some(p) = parts.get(ulid)
                {
                    p.mark_finished();
                }
            }
            OdlProgressEvent::PhaseChanged(p) => {
                if let Ok(jobs) = state.jobs.try_read()
                    && let Some(entry) = jobs.get(&id)
                {
                    let phase = crate::data::mapping::phase_from_odl(*p);
                    // First Downloading transition of the run stamps
                    // started_at (set-once; `0` = unset).
                    if phase == Phase::Downloading {
                        let _ = entry.started_at_ms.compare_exchange(
                            0,
                            now_ms(),
                            Ordering::AcqRel,
                            Ordering::Relaxed,
                        );
                    }
                    entry.set_phase(phase);
                }
            }
            OdlProgressEvent::Cancelled => {
                if let Ok(jobs) = state.jobs.try_read()
                    && let Some(entry) = jobs.get(&id)
                {
                    entry.set_phase(Phase::Paused);
                    entry.reset_live_speed();
                }
            }
            _ => {}
        }
    }
}

struct StateResumeContext {
    state: std::sync::Weak<AppState>,
}

#[async_trait::async_trait]
impl ResumeContext for StateResumeContext {
    async fn relaunch(&self, id: JobId) -> Result<(), JobError> {
        let Some(state) = self.state.upgrade() else {
            return Err(JobError::Other("app state dropped".into()));
        };
        state.start_job(id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry_in(phase: Phase) -> JobEntry {
        let mut job = Job {
            id: JobId::new(),
            url: url::Url::parse("http://example.invalid/f.bin").unwrap(),
            save_dir: std::path::PathBuf::from("/tmp"),
            filename: Some("f.bin".into()),
            referrer: None,
            headers: indexmap::IndexMap::new(),
            max_connections: None,
            proxy: None,
            auth_user: None,
            enc_auth_password: None,
            enc_proxy_password: None,
            enc_cookies: None,
            speed_limit_override: None,
            queue_id: crate::domain::QueueId::new(),
            work_root: None,
            created_at: chrono::Utc::now(),
            started_at: None,
            active_ms: None,
            finished_at: None,
            retries: 0,
            interruptions: 0,
            verify_pending: false,
            status: JobStatus::default(),
            advanced: crate::domain::Advanced::default(),
            checksums: Vec::new(),
            category: crate::domain::Category::Other,
            captured_response: None,
        };
        job.status.phase = phase;
        JobEntry::with_completion(job, crate::domain::OnCompletion::default())
    }

    fn table(names: &[Option<&str>]) -> IndexMap<JobId, Arc<JobEntry>> {
        names
            .iter()
            .map(|n| {
                let mut entry = entry_in(Phase::Queued);
                entry.job.filename = n.map(|n| n.to_owned());
                (entry.job.id, Arc::new(entry))
            })
            .collect()
    }

    /// One name, one download — whatever folder each saves into and
    /// whatever state it is in.
    #[test]
    fn a_name_another_job_holds_is_taken() {
        let jobs = table(&[Some("foo.zip"), None]);
        assert!(name_is_taken(&jobs, "foo.zip", None));
        assert!(name_is_taken(&jobs, "  FOO.ZIP ", None), "case and padding");
        assert!(!name_is_taken(&jobs, "bar.zip", None));
        // A job with no name yet claims nothing, and neither does a
        // blank enquiry.
        assert!(!name_is_taken(&jobs, "   ", None));
    }

    /// Stop queue ends the queue's run. A download the user started by
    /// hand is not part of that run and keeps going.
    #[test]
    fn stopping_a_queue_leaves_hand_started_downloads_alone() {
        let queue = crate::domain::QueueId::new();
        let mut jobs: IndexMap<JobId, Arc<JobEntry>> = IndexMap::new();
        let mut add = |phase: Phase, manual: bool| -> JobId {
            let mut entry = entry_in(phase);
            entry.job.queue_id = queue;
            entry.manual_run.store(manual, Ordering::Release);
            let id = entry.job.id;
            jobs.insert(id, Arc::new(entry));
            id
        };
        let by_queue = add(Phase::Downloading, false);
        let by_hand = add(Phase::Downloading, true);
        let connecting = add(Phase::Evaluating, false);
        let assembling = add(Phase::Assembling, false);
        let paused = add(Phase::Paused, false);

        let targets = queue_stop_targets(&jobs, queue);
        assert!(targets.contains(&by_queue));
        assert!(targets.contains(&connecting), "reconnecting is still a run");
        assert!(!targets.contains(&by_hand), "the user asked for this one");
        assert!(!targets.contains(&assembling), "not a transfer to stop");
        assert!(!targets.contains(&paused), "nothing to stop");
    }

    /// A stale Settings window used to write back its whole page,
    /// reverting whatever had changed elsewhere while it was open.
    #[test]
    fn only_the_named_fields_are_taken_from_an_edit() {
        let current = Settings {
            max_concurrent_downloads: 3,
            notify_complete: true,
            ..Settings::default()
        };
        let edited = Settings {
            max_concurrent_downloads: 7,
            // What the window had at open, now out of date.
            notify_complete: false,
            ..Settings::default()
        };
        let merged =
            merge_settings_fields(&current, &edited, &["max_concurrent_downloads".to_owned()])
                .unwrap();
        assert_eq!(merged.max_concurrent_downloads, 7);
        assert!(merged.notify_complete, "an untouched field is left alone");
    }

    #[test]
    fn a_setting_the_daemon_does_not_know_is_ignored() {
        let current = Settings::default();
        let merged =
            merge_settings_fields(&current, &current, &["from_a_newer_build".to_owned()]).unwrap();
        assert_eq!(
            merged.max_concurrent_downloads,
            current.max_concurrent_downloads
        );
    }

    #[test]
    fn an_update_digest_has_to_be_a_digest() {
        assert!(is_sha256_hex(&"a".repeat(64)));
        assert!(is_sha256_hex(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        ));
        // Nothing to check the artifact against.
        assert!(!is_sha256_hex(""));
        assert!(!is_sha256_hex(&"a".repeat(63)));
        assert!(!is_sha256_hex(&"a".repeat(65)));
        assert!(!is_sha256_hex(&"z".repeat(64)));
    }

    /// What separates "this needs you" from "this broke": the first
    /// is counted apart in the queue's summary, and the second is not
    /// answerable.
    #[test]
    fn a_question_is_not_a_failure() {
        assert!(is_conflict(&JobError::FileChanged("changed".into())));
        assert!(is_conflict(&JobError::NotResumable("no ranges".into())));
        assert!(is_conflict(&JobError::SaveConflict("name taken".into())));
        assert!(is_conflict(&JobError::ServerConflict("mismatch".into())));

        assert!(!is_conflict(&JobError::Network("reset".into())));
        assert!(!is_conflict(&JobError::Cancelled));
        // Nothing to answer: the file is not the promised one and the
        // only way forward is starting over.
        assert!(!is_conflict(&JobError::ChecksumMismatch {
            expected: "a".into(),
            actual: "b".into(),
        }));
    }

    #[test]
    fn a_cache_folder_has_to_name_a_place() {
        assert!(validate_work_dir(std::path::Path::new("/var/tmp/oxdm")).is_ok());
        assert!(validate_work_dir(std::path::Path::new("")).is_err());
        // Relative: the parts would land wherever the daemon was
        // started, and the free-space check would have no volume.
        assert!(validate_work_dir(std::path::Path::new("cache")).is_err());
        assert!(validate_work_dir(std::path::Path::new("./cache")).is_err());
    }

    #[test]
    fn the_cap_counts_what_is_running_not_what_is_waiting() {
        let queue = crate::domain::QueueId::new();
        let mut jobs: IndexMap<JobId, Arc<JobEntry>> = IndexMap::new();
        let mut add = |phase: Phase| {
            let mut entry = entry_in(phase);
            entry.job.queue_id = queue;
            jobs.insert(entry.job.id, Arc::new(entry));
        };
        add(Phase::Downloading);
        add(Phase::Evaluating);
        add(Phase::Queued);
        add(Phase::Paused);
        add(Phase::Completed);

        assert!(slots_full(&jobs, 2, queue, None), "two running, global two");
        assert!(!slots_full(&jobs, 3, queue, None), "a third slot is free");
        // A cap of zero would mean nothing may ever run.
        assert!(slots_full(&jobs, 0, queue, None));
        assert!(!slots_full(&IndexMap::new(), 1, queue, None));
    }

    /// The global limit is shared: a queue allowing ten still runs one
    /// at a time when the global limit is one.
    #[test]
    fn the_global_limit_beats_a_roomier_queue() {
        let queue = crate::domain::QueueId::new();
        let mut jobs: IndexMap<JobId, Arc<JobEntry>> = IndexMap::new();
        let mut entry = entry_in(Phase::Downloading);
        entry.job.queue_id = queue;
        jobs.insert(entry.job.id, Arc::new(entry));

        assert!(slots_full(&jobs, 1, queue, Some(10)));
        assert!(!slots_full(&jobs, 2, queue, Some(10)));
    }

    /// And it is shared *between* queues: two queues allowing three
    /// each run five between them under a global five.
    #[test]
    fn two_queues_share_the_global_limit() {
        let (a, b) = (crate::domain::QueueId::new(), crate::domain::QueueId::new());
        let mut jobs: IndexMap<JobId, Arc<JobEntry>> = IndexMap::new();
        let mut add = |queue: QueueId, phase: Phase| {
            let mut entry = entry_in(phase);
            entry.job.queue_id = queue;
            jobs.insert(entry.job.id, Arc::new(entry));
        };
        for _ in 0..3 {
            add(a, Phase::Downloading);
        }
        add(b, Phase::Downloading);
        add(b, Phase::Downloading);

        // Five running, global five: neither queue may add a sixth,
        // even though queue B is one under its own limit of three.
        assert!(
            slots_full(&jobs, 5, b, Some(3)),
            "the global limit is shared"
        );
        assert!(slots_full(&jobs, 5, a, Some(3)));
        // Queue A is at its own limit whatever the global one allows.
        assert!(
            slots_full(&jobs, 99, a, Some(3)),
            "the queue's own limit still holds"
        );
        // Queue B has room on both counts once the global one does.
        assert!(!slots_full(&jobs, 6, b, Some(3)));
    }

    #[test]
    fn a_deferred_download_is_picked_up_in_list_order() {
        let mut jobs: IndexMap<JobId, Arc<JobEntry>> = IndexMap::new();
        let mut add = |phase: Phase, deferred: bool| -> JobId {
            let entry = entry_in(phase);
            entry.deferred_by_cap.store(deferred, Ordering::Release);
            let id = entry.job.id;
            jobs.insert(id, Arc::new(entry));
            id
        };
        // Queued because a person left it there, not because of the cap.
        add(Phase::Queued, false);
        let first = add(Phase::Queued, true);
        let second = add(Phase::Queued, true);
        // Deferred once, but running now.
        add(Phase::Downloading, true);

        let mut tried = std::collections::HashSet::new();
        assert_eq!(next_deferred(&jobs, &tried), Some(first));
        // One this pass has already picked up is not picked again —
        // without that the filler would spin on a job whose own queue
        // is full.
        tried.insert(first);
        assert_eq!(next_deferred(&jobs, &tried), Some(second));
        tried.insert(second);
        assert_eq!(next_deferred(&jobs, &tried), None);
    }

    #[test]
    fn nothing_is_waiting_on_a_slot_when_nothing_was_deferred() {
        let jobs = table(&[Some("a.zip"), Some("b.zip")]);
        assert_eq!(
            next_deferred(&jobs, &std::collections::HashSet::new()),
            None
        );
    }

    #[test]
    fn another_queues_downloads_are_not_touched() {
        let mut entry = entry_in(Phase::Downloading);
        entry.job.queue_id = crate::domain::QueueId::new();
        let jobs: IndexMap<JobId, Arc<JobEntry>> =
            IndexMap::from([(entry.job.id, Arc::new(entry))]);
        assert!(queue_stop_targets(&jobs, crate::domain::QueueId::new()).is_empty());
    }

    #[test]
    fn a_job_does_not_clash_with_its_own_name() {
        let jobs = table(&[Some("foo.zip")]);
        let id = *jobs.keys().next().unwrap();
        assert!(!name_is_taken(&jobs, "foo.zip", Some(id)));
    }

    #[test]
    fn a_free_name_is_found_around_what_the_table_holds() {
        let jobs = table(&[Some("foo.zip"), Some("foo_1.zip")]);
        assert_eq!(free_name(&jobs, "foo.zip", None), "foo_2.zip");
        assert_eq!(free_name(&jobs, "other.zip", None), "other.zip");
    }

    /// The completion page divides bytes by this, so it has to be the
    /// time the transfer was running and nothing else: a job that was
    /// paused for an hour did not average a byte a second.
    #[test]
    fn time_is_banked_only_while_downloading() {
        let entry = entry_in(Phase::Queued);
        assert_eq!(entry.active_ms(), 0, "nothing before it starts");

        entry.set_phase(Phase::Downloading);
        std::thread::sleep(std::time::Duration::from_millis(12));
        assert!(entry.active_ms() >= 10, "the stretch in progress counts");

        entry.set_phase(Phase::Paused);
        let banked = entry.active_ms();
        std::thread::sleep(std::time::Duration::from_millis(12));
        assert_eq!(entry.active_ms(), banked, "a pause adds nothing");

        entry.set_phase(Phase::Downloading);
        std::thread::sleep(std::time::Duration::from_millis(12));
        assert!(
            entry.active_ms() > banked,
            "resuming picks the tally back up"
        );

        entry.reset_run_stats();
        assert_eq!(entry.active_ms(), 0, "a fresh run starts from zero");
    }

    /// Writing the final file is the one thing no command may cut
    /// short: what survives is the right length and the wrong contents,
    /// with nothing on screen to say so. Pause, cancel, restart, delete
    /// and re-verify all go through this.
    #[test]
    fn nothing_may_interrupt_an_assembly() {
        let assembling = entry_in(Phase::Assembling);
        assert!(AppState::refuse_while_assembling(&assembling).is_err());

        for phase in [
            Phase::Downloading,
            Phase::Paused,
            Phase::Queued,
            Phase::Completed,
            Phase::Failed,
            Phase::Verifying,
        ] {
            assert!(
                AppState::refuse_while_assembling(&entry_in(phase)).is_ok(),
                "{phase:?} is interruptible"
            );
        }
    }

    /// Hashing a file once per algorithm, then judging each row against
    /// it: a row that matches is verified with nothing to show beside
    /// it, one that does not keeps the digest as its "got" side, and a
    /// malformed row is left alone — "this is not a hash" is a
    /// different claim from "this file is wrong".
    #[tokio::test]
    async fn hash_rows_judges_each_against_the_file() {
        use crate::domain::{Algo, Checksum, CsSource, CsStatus};
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f.bin");
        std::fs::write(&path, b"hello world").unwrap();
        const GOOD: &str = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        let row = |hash: &str| Checksum {
            algo: Algo::Sha256,
            hash: hash.to_owned(),
            source: CsSource::User,
            status: CsStatus::Unverified,
            expected: None,
        };
        let rows = vec![row(GOOD), row(&GOOD.replace('b', "a")), row("not-a-hash")];

        let (out, computed) = hash_against_rows(&path, &rows, Default::default())
            .await
            .unwrap();
        assert_eq!(computed.get(&Algo::Sha256).map(String::as_str), Some(GOOD));

        assert_eq!(out.len(), 2, "the malformed row is skipped, not judged");
        assert_eq!(out[0], (0, CsStatus::Verified, None));
        assert_eq!(out[1].0, 1);
        assert_eq!(out[1].1, CsStatus::Mismatch);
        assert_eq!(
            out[1].2.as_deref(),
            Some(GOOD),
            "a mismatch carries what the file actually hashes to",
        );
    }

    /// A digest already computed from these bytes answers the next row
    /// too. Proven by asking about a file that is not there: reading it
    /// would fail, so a verdict at all means nothing was read.
    #[tokio::test]
    async fn a_known_digest_is_not_computed_again() {
        use crate::domain::{Algo, Checksum, CsSource, CsStatus};
        const GOOD: &str = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        let rows = vec![Checksum {
            algo: Algo::Sha256,
            hash: GOOD.to_owned(),
            source: CsSource::User,
            status: CsStatus::Unverified,
            expected: None,
        }];
        let known = std::collections::HashMap::from([(Algo::Sha256, GOOD.to_owned())]);

        let (out, _) = hash_against_rows(std::path::Path::new("/nonexistent/f.bin"), &rows, known)
            .await
            .expect("no file is read");

        assert_eq!(out, vec![(0, CsStatus::Verified, None)]);
    }

    /// A file replaced on disk is a different file: its length or its
    /// modification time moves, and the digests taken from the old one
    /// must not answer for the new bytes.
    #[test]
    fn a_changed_file_forgets_what_it_hashed_to() {
        use crate::domain::Algo;
        let entry = entry_in(Phase::Completed);
        let digests = std::collections::HashMap::from([(Algo::Sha256, "abc".to_owned())]);
        entry.remember_digests(Some((10, 1_000)), digests);

        assert!(!entry.known_digests(Some((10, 1_000))).is_empty());
        assert!(
            entry.known_digests(Some((10, 2_000))).is_empty(),
            "rewritten in place"
        );
        assert!(
            entry.known_digests(Some((11, 1_000))).is_empty(),
            "a different length"
        );
        assert!(
            entry.known_digests(None).is_empty(),
            "a file we cannot stat is not a file we know"
        );
    }

    /// The reset sweep must take every per-job dir — including ones
    /// orphaned by a past crash — and nothing else the user parked in
    /// their work dir.
    #[test]
    fn purge_takes_only_per_job_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let mine = per_job_dir(root, JobId::new());
        std::fs::create_dir_all(mine.join("nested")).unwrap();
        std::fs::write(mine.join("metadata.pb"), b"x").unwrap();
        let orphan = root.join(".oxdm-deadbeef");
        std::fs::create_dir_all(&orphan).unwrap();

        let keep_dir = root.join("keepme");
        std::fs::create_dir_all(&keep_dir).unwrap();
        let keep_file = root.join("notes.txt");
        std::fs::write(&keep_file, b"x").unwrap();
        // Prefix-adjacent, not ours.
        let keep_lookalike = root.join(".oxdm");
        std::fs::create_dir_all(&keep_lookalike).unwrap();

        assert_eq!(purge_work_dir_partials(root), 2);
        assert!(!mine.exists());
        assert!(!orphan.exists());
        assert!(keep_dir.exists());
        assert!(keep_file.exists());
        assert!(keep_lookalike.exists());
    }

    /// A missing work dir is a normal state (nothing downloaded yet) —
    /// the reset has to carry on to dropping the DB regardless.
    #[test]
    fn purge_tolerates_missing_work_dir() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(purge_work_dir_partials(&tmp.path().join("nope")), 0);
    }

    /// A symlinked `.oxdm-*` is unlinked, not followed — otherwise a
    /// reset could delete whatever the link aims at.
    #[cfg(unix)]
    #[test]
    fn purge_does_not_follow_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let outside = root.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("precious"), b"x").unwrap();
        std::os::unix::fs::symlink(&outside, root.join(".oxdm-link")).unwrap();

        assert_eq!(purge_work_dir_partials(root), 1);
        assert!(!root.join(".oxdm-link").exists());
        assert!(outside.join("precious").exists());
    }

    #[test]
    fn sidecar_appends_suffix_to_full_filename() {
        let p = std::path::Path::new("/data/oxdm/oxdm.db");
        assert_eq!(
            sidecar(p, "-wal"),
            std::path::Path::new("/data/oxdm/oxdm.db-wal")
        );
    }
}

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
    CaptureRequest, Category, ConflictWhileHidden, HostSetting, Job, JobError, JobId, JobStatus,
    LiveCounters, Phase, Queue, QueueId, Settings, classify,
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
    /// `Completed` transition timestamp, epoch milliseconds. `0` = None.
    /// Spliced onto `Job::finished_at`.
    pub finished_at_ms: AtomicI64,
    /// Cumulative count of `PartRetrying` events this run. Spliced onto
    /// `Job::retries`.
    pub retries: AtomicU32,
    /// ULIDs of parts currently mid-retry. Drives the `Reconnecting`
    /// phase: non-empty ⇒ at least one part is retrying. Keyed by ulid
    /// (rather than a bare counter) so a sibling part's progress tick
    /// can't spuriously clear a still-retrying part — debounces the
    /// banner. Cleared on restart / cancel-to-queued.
    pub retrying_parts: std::sync::Mutex<std::collections::HashSet<String>>,
    pub parts: std::sync::RwLock<IndexMap<String, Arc<PartCounters>>>,
    pub cancel: std::sync::Mutex<CancellationToken>,
    pub running: AtomicBool,
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

impl JobEntry {
    fn new(job: Job) -> Self {
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
        let started_at_ms = job.started_at.map(|d| d.timestamp_millis()).unwrap_or(0);
        let finished_at_ms = job.finished_at.map(|d| d.timestamp_millis()).unwrap_or(0);
        let retries = job.retries;
        Self {
            job,
            live_phase: AtomicU8::new(encode_phase(phase)),
            counters,
            started_at_ms: AtomicI64::new(started_at_ms),
            finished_at_ms: AtomicI64::new(finished_at_ms),
            retries: AtomicU32::new(retries),
            retrying_parts: std::sync::Mutex::new(std::collections::HashSet::new()),
            parts: std::sync::RwLock::new(IndexMap::new()),
            cancel: std::sync::Mutex::new(CancellationToken::new()),
            running: AtomicBool::new(false),
            is_resumable: std::sync::atomic::AtomicI8::new(0),
            captured_response: std::sync::RwLock::new(None),
            session_speed_override: std::sync::atomic::AtomicU64::new(0),
            on_completion: std::sync::RwLock::new(crate::domain::OnCompletion::default()),
            resolver: RwLock::new(None),
            final_path: std::sync::RwLock::new(job_final_path),
            live_controls: odl::progress::LiveControls::new(),
        }
    }

    pub fn phase(&self) -> Phase {
        decode_phase(self.live_phase.load(Ordering::Acquire))
    }

    pub fn set_phase(&self, p: Phase) {
        self.live_phase.store(encode_phase(p), Ordering::Release);
    }

    /// Clear the per-run stats (started_at / finished_at / retries /
    /// in-flight retrying parts). Called when a job re-enters a clean
    /// pre-run state (restart, cancel-to-queued) so the next run starts
    /// its timing and retry tally from scratch. Set-once within a run,
    /// cleared on re-run (plan W4).
    pub fn reset_run_stats(&self) {
        self.started_at_ms.store(0, Ordering::Release);
        self.finished_at_ms.store(0, Ordering::Release);
        self.retries.store(0, Ordering::Release);
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
    /// Cached id of the built-in Main queue. Resolved once at boot so
    /// `add_job` does not have to round-trip the DB.
    main_queue_id: QueueId,
    /// In-memory cache of every Queue. Authoritative copy is the DB;
    /// this avoids hitting SQLite on every UI refresh.
    queues: RwLock<IndexMap<QueueId, Queue>>,
    /// In-memory cache of host overrides keyed by lowercased host.
    host_settings: RwLock<IndexMap<String, HostSetting>>,
    /// Queues currently in "active" state — at least one job has been
    /// started by `start_queue` and no `QueueFinished` event has fired
    /// yet. Used to gate `QueueStarted` / `QueueFinished` emission so
    /// hooks fire exactly once per run.
    active_queues: RwLock<std::collections::HashSet<QueueId>>,
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
    /// Single-slot grace timer for destructive power actions (queue
    /// hooks + per-job completion actions both go through it).
    power: Arc<crate::data::power::PowerGuard>,
}

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

        let mut settings = store.load_settings().await.unwrap_or_else(|e| {
            tracing::warn!(error = %e, "failed to load settings; using defaults");
            Settings::default()
        });

        // Generate ext token on first launch and persist it. Token is
        // used by browser extensions to authenticate against the local
        // WebSocket bridge — see `ipc::ws`.
        if settings.ext_token.is_empty() {
            settings.ext_token = generate_token();
            if let Err(e) = store.save_settings(&settings).await {
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
        let stored_jobs = store.list_jobs().await.unwrap_or_default();
        let mut jobs = IndexMap::new();
        for j in stored_jobs {
            jobs.insert(j.id, Arc::new(JobEntry::new(j)));
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

        let host_list = store.list_host_settings().await.unwrap_or_default();
        let mut host_settings = IndexMap::new();
        for h in host_list {
            host_settings.insert(HostSetting::host_key(&h.host), h);
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
            main_queue_id,
            queues: RwLock::new(queues),
            host_settings: RwLock::new(host_settings),
            active_queues: RwLock::new(std::collections::HashSet::new()),
            conflict_queue: RwLock::new(std::collections::VecDeque::new()),
            master_key: RwLock::new(master_key),
            db_error: RwLock::new(db_error),
            power,
        })
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

        let snapshot: Vec<(JobId, Phase)> = self
            .jobs
            .read()
            .await
            .values()
            .filter(|e| e.job.queue_id == id)
            .map(|e| (e.job.id, e.phase()))
            .collect();
        let running_now = snapshot.iter().filter(|(_, p)| p.is_running()).count();
        let mut budget = cap.saturating_sub(running_now);
        let mut started_any = false;
        for (jid, phase) in snapshot {
            if budget == 0 {
                break;
            }
            match phase {
                Phase::Queued | Phase::Paused if self.start_job(jid).await.is_ok() => {
                    started_any = true;
                    budget -= 1;
                }
                _ => {}
            }
        }

        let mut active = self.active_queues.write().await;
        if started_any && active.insert(id) {
            let _ = self.events.send(DomainEvent::QueueStarted { id });
        }
        Ok(())
    }

    /// True when the queue has been started (via `start_queue` or
    /// scheduler) and has not yet emitted `QueueFinished`.
    pub async fn is_queue_active(&self, id: QueueId) -> bool {
        self.active_queues.read().await.contains(&id)
    }

    /// Snapshot of currently active queue ids.
    pub async fn active_queue_ids(&self) -> std::collections::HashSet<QueueId> {
        self.active_queues.read().await.clone()
    }

    /// Pause every running job in the queue. Emits `QueueFinished` on
    /// the active→inactive transition.
    pub async fn stop_queue(self: &Arc<Self>, id: QueueId) -> Result<(), String> {
        let ids: Vec<JobId> = self
            .jobs
            .read()
            .await
            .values()
            .filter(|e| e.job.queue_id == id && e.running.load(Ordering::Acquire))
            .map(|e| e.job.id)
            .collect();
        for jid in ids {
            let _ = self.pause(jid).await;
        }
        let mut active = self.active_queues.write().await;
        if active.remove(&id) {
            let _ = self.events.send(DomainEvent::QueueFinished { id });
        }
        Ok(())
    }

    /// Watcher: after a job leaves running state, if its queue has no
    /// more running or queued jobs, fire `QueueFinished` and clear the
    /// active flag. Called from the runner outcome handler.
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
        if active.remove(&queue_id) {
            let _ = self
                .events
                .send(DomainEvent::QueueFinished { id: queue_id });
        }
    }

    // ── host settings ────────────────────────────────────────────────

    pub async fn host_settings_snapshot(&self) -> Vec<HostSetting> {
        self.host_settings.read().await.values().cloned().collect()
    }

    pub async fn host_settings_for(&self, url: &url::Url) -> Option<HostSetting> {
        let host = url.host_str()?;
        self.host_settings
            .read()
            .await
            .get(&HostSetting::host_key(host))
            .cloned()
    }

    pub async fn upsert_host_setting(self: &Arc<Self>, h: HostSetting) -> Result<(), String> {
        self.store
            .upsert_host_setting(&h)
            .await
            .map_err(|e| e.to_string())?;
        self.host_settings
            .write()
            .await
            .insert(HostSetting::host_key(&h.host), h);
        let _ = self.events.send(DomainEvent::HostSettingsChanged);
        Ok(())
    }

    pub async fn delete_host_setting(self: &Arc<Self>, host: &str) -> Result<(), String> {
        self.store
            .delete_host_setting(host)
            .await
            .map_err(|e| e.to_string())?;
        // The row is gone; leaving its keyring entry behind would strand
        // a credential no UI can reach. Best-effort — a locked keyring
        // must not block the delete.
        if let Err(e) = crate::data::keyring::delete_password(host) {
            tracing::warn!(host, error = %e, "could not delete host password from the keyring");
        }
        self.host_settings
            .write()
            .await
            .shift_remove(&HostSetting::host_key(host));
        let _ = self.events.send(DomainEvent::HostSettingsChanged);
        Ok(())
    }

    /// Queue + start a hidden artifact download for self-update. Reuses
    /// every piece of regular download machinery (multi-part fetcher,
    /// progress bar, pause / cancel) but stays out of the queue list.
    /// Caller is expected to subscribe to `DomainEvent::JobCompleted`
    /// to learn the final artifact path, then hand it to the updater
    /// helper for verification + swap + relaunch.
    pub async fn add_update_job(
        self: &Arc<Self>,
        url: url::Url,
        suggested_filename: Option<String>,
    ) -> Result<JobId, JobError> {
        let save_dir = std::env::temp_dir().join("oxdm-updates");
        let _ = tokio::fs::create_dir_all(&save_dir).await;
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
            )
            .await?;
        self.hidden_jobs.write().await.insert(id);
        self.start_job(id).await?;
        Ok(id)
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
    pub async fn set_job_checksums(
        &self,
        id: JobId,
        checksums: Vec<crate::domain::Checksum>,
    ) -> Result<(), JobError> {
        let mut jobs = self.jobs.write().await;
        let Some(old) = jobs.get(&id).cloned() else {
            return Err(JobError::Other("job not found".into()));
        };
        let mut new_job = old.job.clone();
        new_job.checksums = checksums;
        let new_entry = clone_entry_with_job(&old, new_job.clone()).await;
        jobs.insert(id, new_entry);
        drop(jobs);
        self.store
            .upsert_job(&new_job)
            .await
            .map_err(|e| JobError::Io(e.to_string()))?;
        Ok(())
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

    /// Park a job at the end of the queue, mark it `Failed` with a
    /// `ConflictPending` payload, and send a notification. Used by the
    /// runner when `conflict_while_hidden = NotifyAndPark` and the
    /// job's dialog is not visible.
    ///
    /// "No auto-retry" is implicit: oxdm never auto-retries after a
    /// terminal phase. The user explicitly Resumes from the queue row.
    pub async fn park_with_conflict(self: &Arc<Self>, id: JobId, msg: String) {
        // Re-insert at the tail of the IndexMap.
        let mut jobs = self.jobs.write().await;
        if let Some(entry) = jobs.shift_remove(&id) {
            jobs.insert(id, entry);
        }
        drop(jobs);
        if let Some(entry) = self.jobs.read().await.get(&id) {
            entry.set_phase(Phase::Failed);
            entry.reset_live_speed();
        }
        let _ = self.events.send(DomainEvent::JobFailed {
            id,
            error: JobError::ConflictPending(msg),
        });
    }

    /// Replace the extension token with a freshly generated one and
    /// persist. Existing extension WebSocket sessions stay open until
    /// they reconnect; new sessions must use the new value.
    pub async fn regenerate_ext_token(self: &Arc<Self>) -> Result<String, String> {
        let new = generate_token();
        let mut settings = self.settings.read().await.clone();
        settings.ext_token = new.clone();
        self.store
            .save_settings(&settings)
            .await
            .map_err(|e| e.to_string())?;
        *self.ext_token.write().await = new.clone();
        *self.settings.write().await = settings;
        let _ = self.events.send(DomainEvent::SettingsChanged);
        Ok(new)
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

    /// User chose "Reset" in the recovery dialog. Move the broken DB
    /// aside (timestamped `.bak`) and exit the daemon process. The
    /// next daemon spawn re-runs `Store::open`, gets a fresh DB, and
    /// boots normally.
    ///
    /// We do not try to hot-swap the `Store` in place — too many live
    /// references (queues / runners / scheduler / IPC handlers) hold
    /// pointers into the existing one. A clean exit + re-spawn is the
    /// safer reset path.
    pub fn reset_database_and_exit(&self) -> Result<(), String> {
        let path = crate::data::store::default_db_path();
        if path.exists() {
            let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S");
            let backup = path.with_extension(format!("db.bak-{ts}"));
            if let Err(e) = std::fs::rename(&path, &backup) {
                return Err(format!("could not back up corrupt DB: {e}"));
            }
            tracing::warn!(
                original = %path.display(),
                backup = %backup.display(),
                "DB reset: original file renamed for forensics",
            );
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

    pub async fn update_settings(&self, mut new: Settings) -> Result<(), String> {
        // The proxy password arrives in the clear and leaves as
        // ciphertext; the plaintext never reaches the settings table.
        let typed = std::mem::take(&mut new.proxy_password);
        let clear = std::mem::take(&mut new.clear_proxy_password);
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
        *self.settings.write().await = new;
        let _ = self.events.send(DomainEvent::SettingsChanged);
        Ok(())
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

    /// Insert a new job in `Queued` state. Caller decides whether to
    /// also `start_job` (Download Now) or leave it (Download Later).
    ///
    /// Filename collisions are no longer pre-checked here — the UI
    /// asks the user (Add dialog overwrite overlay) before this is
    /// called. The runner-level `SaveConflictResolver` still handles
    /// on-disk collisions if one slips through.
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
    ) -> Result<JobId, JobError> {
        let id = JobId::new();
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
        let job = Job {
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
            created_at: chrono::Utc::now(),
            started_at: None,
            finished_at: None,
            retries: 0,
            status: JobStatus::default(),
            advanced: crate::domain::Advanced::default(),
            checksums: Vec::new(),
            category,
            captured_response: None,
        };
        self.store
            .upsert_job(&job)
            .await
            .map_err(|e| JobError::Io(e.to_string()))?;
        self.jobs
            .write()
            .await
            .insert(id, Arc::new(JobEntry::new(job)));
        let _ = self.events.send(DomainEvent::JobAdded { id });
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

        let mut new_job = entry.job.clone();
        new_job.url = edit.url;
        new_job.save_dir = edit.save_dir;
        new_job.filename = edit.filename;
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
    pub async fn probe(&self, url: url::Url) -> Result<ProbeResult, JobError> {
        let manager = self.manager.read().await.clone();
        let settings = self.settings.read().await.clone();
        let resolver = ProbeResolver;
        let instr = manager
            .evaluate(odl::download_manager::EvaluateRequest::new(
                url,
                settings.download_dir,
                &resolver,
            ))
            .await
            .map_err(|e| crate::data::mapping::job_error_from_odl(&e))?;
        Ok(ProbeResult {
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
        if let Some(ref r) = req.referrer
            && !headers.contains_key("Referer")
        {
            headers.insert("Referer".into(), r.to_string());
        }
        let cookies = req.cookies.clone().or(captured_cookie);
        // Capture flow keeps the original filename even on collision.
        // Confirm window (`confirm_window.rs`) detects the dup against
        // the live snapshot and offers an overwrite confirmation
        // overlay; deferring the decision to the user matches the
        // manual Add dialog behaviour.
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
        let save_dir = settings
            .category_folders
            .get(&category)
            .filter(|p| !p.as_os_str().is_empty())
            .cloned()
            .unwrap_or(settings.download_dir);
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
    pub async fn start_job(self: &Arc<Self>, id: JobId) -> Result<(), JobError> {
        let entry = self
            .job_entry(id)
            .await
            .ok_or_else(|| JobError::Other("job not found".into()))?;
        if entry.running.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        // Fresh cancel token per run. The previous run's token may be
        // already cancelled (pause trips it); installing a new one lets
        // resume actually run instead of returning Cancelled instantly.
        let token = CancellationToken::new();
        *entry.cancel.lock().expect("cancel mutex poisoned") = token.clone();
        let manager = self.manager.read().await.clone();
        let events = self.events.clone();
        let bridge: Arc<dyn LiveBridge> = Arc::new(StateLiveBridge {
            state: Arc::downgrade(self),
        });

        let settings = self.settings.read().await.clone();
        let _ = tokio::fs::create_dir_all(&settings.work_dir).await;
        let per_job_dir = Some(per_job_dir(&settings.work_dir, id));
        let interactive = dialog_open_for(self, id).await;
        let park_on_conflict =
            !interactive && settings.conflict_while_hidden == ConflictWhileHidden::NotifyAndPark;

        // Effective settings overlay:
        //   global Settings → per-host overrides → per-job overrides.
        // When any layer changes the manager-level config, build a
        // fresh `DownloadManager` off a settings copy — odl applies
        // `speed_limit` / `max_connections` / `user_agent` per Manager,
        // not per call.
        let host_override = self.host_settings_for(&entry.job.url).await;
        // Resolve credentials: username from DB, password from OS keyring
        // (sentinel `has_password` decides whether to look it up).
        let host_credentials: Option<(String, Option<String>)> = host_override
            .as_ref()
            .and_then(|h| h.username.as_ref().map(|u| (u.clone(), h.has_password)))
            .map(|(u, has_pw)| {
                let pw = if has_pw {
                    let host = entry.job.url.host_str().unwrap_or("").to_string();
                    match crate::data::keyring::get_password(&host) {
                        Ok(p) => p,
                        Err(e) => {
                            tracing::warn!(host = %host, error = %e, "keyring read failed");
                            None
                        }
                    }
                } else {
                    None
                };
                (u, pw)
            });
        let _ = host_credentials; // odl integration of basic auth lands with milestone 12.
        let session_override = entry.session_speed_override.load(Ordering::Acquire);
        let job_override = entry.job.speed_limit_override;
        let effective_speed = if session_override != 0 {
            Some(session_override)
        } else if let Some(o) = job_override {
            Some(o)
        } else {
            host_override.as_ref().and_then(|h| h.speed_limit)
        };
        let host_threads = host_override.as_ref().and_then(|h| h.thread_count);
        let host_ua = host_override
            .as_ref()
            .and_then(|h| h.default_user_agent.clone());

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

        let needs_rebuild = effective_speed != settings.speed_limit
            || host_threads.is_some()
            || host_ua.is_some()
            || !job_headers.is_empty()
            || job_ua.is_some();
        let runner_manager = if needs_rebuild {
            let mut s = settings.clone();
            s.speed_limit = effective_speed;
            if let Some(t) = host_threads {
                s.max_connections = Some(t);
            }
            if let Some(ua) = host_ua {
                s.user_agent = Some(ua);
            }
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

        let runner = JobRunner {
            job_id: id,
            manager: runner_manager,
            events: events.clone(),
            cancel: token.clone(),
            bridge,
            interactive,
            per_job_dir,
            live_controls: entry.live_controls.clone(),
            auth_password,
            proxy_password,
            cookies,
        };

        let job_clone = entry.job.clone();
        let queue_id = entry.job.queue_id;
        let state = Arc::clone(self);
        entry.set_phase(Phase::Evaluating);
        let _ = self.events.send(DomainEvent::JobUpdated {
            id,
            phase: Phase::Evaluating,
        });
        // First started job in a queue also fires QueueStarted, so a
        // manual single-job Start surfaces on_start hooks the same as a
        // schedule-driven start_queue.
        if self.active_queues.write().await.insert(queue_id) {
            let _ = self.events.send(DomainEvent::QueueStarted { id: queue_id });
        }
        tokio::spawn(async move {
            let outcome = runner.run(job_clone).await;
            entry.running.store(false, Ordering::Release);
            // After every terminal outcome, ask the watcher whether this
            // job's queue has now drained completely; if so it emits
            // QueueFinished. Doing it here avoids a second subscriber
            // task with a different view of "still running".
            {
                let finish_state = state.clone();
                tokio::spawn(async move {
                    finish_state.maybe_finish_queue(queue_id).await;
                });
            }
            match outcome {
                Ok(o) => {
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
                    if let Some(total) = entry.counters.total() {
                        entry.counters.set_downloaded(total);
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
                    state.persist_job(id).await;
                    // NotifyAndPark path: surface the conflict failure
                    // as a parked job (end of queue, no auto-retry)
                    // plus a desktop notification, instead of leaving
                    // the user staring at a `Failed` row with no clue
                    // what happened.
                    let is_conflict = matches!(
                        &err,
                        JobError::ServerConflict(_)
                            | JobError::SaveConflict(_)
                            | JobError::ChecksumMismatch { .. }
                    );
                    if park_on_conflict && is_conflict {
                        state.park_with_conflict(id, err.to_string()).await;
                    } else {
                        let _ = state.events.send(DomainEvent::JobFailed { id, error: err });
                    }
                }
            }
            let _ = token; // keep alive for the run
        });

        Ok(())
    }

    pub async fn pause(self: &Arc<Self>, id: JobId) -> Result<(), JobError> {
        let entry = self
            .job_entry(id)
            .await
            .ok_or_else(|| JobError::Other("job not found".into()))?;
        let handle = JobHandle {
            id,
            cancel: entry.cancel.lock().expect("cancel mutex poisoned").clone(),
        };
        let res = self.pause_strategy.pause(&handle).await;
        // The runner outcome handler also sets phase + zeros counters on
        // Cancelled, but the runner may take a tick to wind down. Flip
        // the visible state immediately so the dialog footer button and
        // the speed/ETA cells switch the moment the user clicks Pause.
        entry.set_phase(Phase::Paused);
        entry.reset_live_speed();
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
        let handle = JobHandle {
            id,
            cancel: entry.cancel.lock().expect("cancel mutex poisoned").clone(),
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
        // Best-effort pause; ignore "not running" errors.
        let _ = self.pause(id).await;
        let entry = self
            .job_entry(id)
            .await
            .ok_or_else(|| JobError::Other("job not found".into()))?;

        let settings = self.settings.read().await.clone();
        let dir = per_job_dir(&settings.work_dir, id);
        let _ = tokio::fs::remove_dir_all(&dir).await;

        entry.reset_live_speed();
        entry.counters.reset_progress();
        entry.reset_run_stats();
        if let Ok(mut g) = entry.final_path.write() {
            *g = None;
        }
        entry.set_phase(Phase::Queued);
        self.persist_job(id).await;
        let _ = self.events.send(DomainEvent::JobUpdated {
            id,
            phase: Phase::Queued,
        });
        self.start_job(id).await
    }

    /// Remove a job. `delete_files` decides whether to also wipe
    /// `metadata.pb` + `.part` + (if completed) the assembled file.
    pub async fn remove(self: &Arc<Self>, id: JobId, opts: RemoveOpts) -> Result<(), JobError> {
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
            let settings = self.settings().await;
            // Per-job working dir holds metadata.pb + every .part +
            // lockfile. Recursive remove wipes them all without
            // touching other jobs' folders.
            let dir = per_job_dir(&settings.work_dir, id);
            let _ = tokio::fs::remove_dir_all(&dir).await;
        }
        if opts.delete_final_file
            && let Some(p) = entry.job.status.final_path.as_ref()
        {
            let _ = tokio::fs::remove_file(p).await;
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
        Ok(())
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

    pub async fn pause_all(self: &Arc<Self>) {
        let ids: Vec<JobId> = self
            .jobs
            .read()
            .await
            .values()
            .filter(|e| e.running.load(Ordering::Acquire))
            .map(|e| e.job.id)
            .collect();
        for id in ids {
            let _ = self.pause(id).await;
        }
    }

    /// Resume every paused / queued job.
    pub async fn resume_all(self: &Arc<Self>) {
        let ids: Vec<JobId> = self
            .jobs
            .read()
            .await
            .values()
            .filter(|e| matches!(e.phase(), Phase::Paused | Phase::Queued))
            .map(|e| e.job.id)
            .collect();
        for id in ids {
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
}

#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
pub struct RemoveOpts {
    /// Wipe `metadata.pb` + `.part` files for an incomplete job.
    pub purge_partial: bool,
    /// Delete the assembled final file (only meaningful for completed).
    pub delete_final_file: bool,
}

/// Per-job working directory under the configured download dir. Holds
/// `metadata.pb`, `odl.lock`, and every `<ulid>.part` for this job.
/// Materialise a Job view that reflects live (in-memory) state on top
/// of the load-time snapshot held by `JobEntry::job`. The `Job` struct
/// is immutable in the registry, so progress and completion data live
/// elsewhere — atomic counters, the live phase byte, and the
/// completion-only `final_path` cell. UI consumers want a single Job
/// value with all of those folded in.
pub(crate) fn splice_live(entry: &JobEntry) -> Job {
    let mut j = entry.job.clone();
    j.status.phase = entry.phase();
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
    j.finished_at = ms_to_datetime(entry.finished_at_ms.load(Ordering::Relaxed));
    j.retries = entry.retries.load(Ordering::Relaxed);
    j
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
    work_dir.join(format!(".oxdm-{}", id.0.simple()))
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
    let cancel_token = old.cancel.lock().expect("cancel mutex poisoned").clone();
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
        started_at_ms: AtomicI64::new(old.started_at_ms.load(Ordering::Acquire)),
        finished_at_ms: AtomicI64::new(old.finished_at_ms.load(Ordering::Acquire)),
        retries: AtomicU32::new(old.retries.load(Ordering::Acquire)),
        retrying_parts: std::sync::Mutex::new(retrying_parts),
        parts: std::sync::RwLock::new(parts),
        cancel: std::sync::Mutex::new(cancel_token),
        running: AtomicBool::new(old.running.load(Ordering::Acquire)),
        is_resumable: std::sync::atomic::AtomicI8::new(old.is_resumable.load(Ordering::Acquire)),
        captured_response: std::sync::RwLock::new(captured_response),
        session_speed_override: std::sync::atomic::AtomicU64::new(
            old.session_speed_override.load(Ordering::Acquire),
        ),
        on_completion: std::sync::RwLock::new(on_completion),
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

/// AAD identity for secrets that belong to the app rather than to a
/// job. The nil UUID is never a real `JobId`, so a global ciphertext
/// cannot be replayed as a job's and vice versa.
const GLOBAL_SECRET_ID: JobId = JobId(uuid::Uuid::nil());

/// `LiveBridge` impl that walks back to `AppState` via a weak ref.
struct StateLiveBridge {
    state: std::sync::Weak<AppState>,
}

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
                    // Monotonic: skips the brief Progress(0, total) odl
                    // can emit while re-evaluating an in-flight resume.
                    entry.counters.advance_downloaded(*downloaded);
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
                    parts.insert(
                        ulid.clone(),
                        Arc::new(PartCounters {
                            ulid: ulid.clone(),
                            offset: *offset,
                            size: *size,
                            downloaded: AtomicU64::new(0),
                            speed_bps_bits: AtomicU64::new(0),
                            finished: AtomicBool::new(false),
                        }),
                    );
                }
            }
            OdlProgressEvent::PartProgress {
                ulid, downloaded, ..
            } => {
                if let Ok(jobs) = state.jobs.try_read()
                    && let Some(entry) = jobs.get(&id)
                {
                    if let Ok(parts) = entry.parts.try_read()
                        && let Some(p) = parts.get(ulid)
                    {
                        p.downloaded.store(*downloaded, Ordering::Relaxed);
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
            OdlProgressEvent::PartRetrying { ulid, .. } => {
                if let Ok(jobs) = state.jobs.try_read()
                    && let Some(entry) = jobs.get(&id)
                {
                    // Count every retry event (plan W2). Mark this part
                    // as retrying and surface the Reconnecting banner
                    // while ≥1 part is mid-retry (plan W1).
                    entry.retries.fetch_add(1, Ordering::Relaxed);
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
                }
            }
            OdlProgressEvent::PartFinished { ulid } => {
                if let Ok(jobs) = state.jobs.try_read()
                    && let Some(entry) = jobs.get(&id)
                    && let Ok(parts) = entry.parts.try_read()
                    && let Some(p) = parts.get(ulid)
                {
                    p.finished.store(true, Ordering::Release);
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

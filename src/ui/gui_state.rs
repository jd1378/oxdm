//! Snapshot cache the GUI process maintains in front of the daemon.
//!
//! The egui main thread reads from `Cache` synchronously. A
//! background tokio task drains `ipc_local::Event`s from the daemon
//! and mutates the cache; lifecycle events trigger `Snapshot` /
//! `JobEntry` re-fetches, counter ticks update the per-job map in
//! place. A `Notify` lets the UI request an immediate refresh
//! (e.g. right after a mutation succeeds).

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use tokio::runtime::Handle;

use crate::data::ConflictKind;
use crate::domain::{HostSetting, Job, JobId, Queue, QueueId, Settings};
use crate::ipc_local::Client;
use crate::ipc_local::protocol::{
    Event, JobCounters, JobEntryView, PartView, SnapshotData, SubFilter,
};

/// Live cache of everything the GUI reads from the daemon.
pub struct Cache {
    snap: RwLock<SnapshotData>,
    /// Per-job latest counters dump. Updated on every `Event::Counters`.
    counters: RwLock<indexmap::IndexMap<JobId, JobCounters>>,
    /// Per-host settings list. Refreshed on `HostListChanged` and on
    /// explicit `refresh_host_list`.
    host_list: RwLock<Vec<HostSetting>>,
    /// Built-in Main queue id, derived from the first builtin queue.
    main_queue_id: RwLock<QueueId>,
}

impl Cache {
    pub fn from_snapshot(snap: SnapshotData) -> Self {
        let main = snap
            .queues
            .iter()
            .find(|q| q.builtin)
            .map(|q| q.id)
            .unwrap_or_else(QueueId::new);
        let mut counters_map = indexmap::IndexMap::with_capacity(snap.counters.len());
        for c in &snap.counters {
            counters_map.insert(c.id, c.clone());
        }
        Self {
            snap: RwLock::new(snap),
            counters: RwLock::new(counters_map),
            host_list: RwLock::new(Vec::new()),
            main_queue_id: RwLock::new(main),
        }
    }

    pub fn snapshot(&self) -> SnapshotData {
        self.snap.read().unwrap().clone()
    }

    pub fn jobs(&self) -> Vec<Job> {
        self.snap.read().unwrap().jobs.clone()
    }

    pub fn queues(&self) -> Vec<Queue> {
        self.snap.read().unwrap().queues.clone()
    }

    pub fn settings(&self) -> Settings {
        self.snap.read().unwrap().settings.clone()
    }

    pub fn active_queues(&self) -> HashSet<QueueId> {
        self.snap.read().unwrap().active_queues.clone()
    }

    pub fn conflict_head(&self) -> Option<(JobId, ConflictKind, u64)> {
        self.snap.read().unwrap().conflict_head
    }

    pub fn conflict_len(&self) -> usize {
        self.snap.read().unwrap().conflict_len
    }

    pub fn main_queue_id(&self) -> QueueId {
        *self.main_queue_id.read().unwrap()
    }

    pub fn host_list(&self) -> Vec<HostSetting> {
        self.host_list.read().unwrap().clone()
    }

    pub fn job_counters(&self, id: JobId) -> Option<JobCounters> {
        self.counters.read().unwrap().get(&id).cloned()
    }

    /// View used by the per-job download window: combines the job
    /// metadata from the snapshot with the latest counters and the
    /// daemon-cached per-job overrides (on_completion, session
    /// speed). When the daemon hasn't replied to `JobEntry` yet,
    /// returns `None`.
    pub fn job_entry_cached(&self, id: JobId) -> Option<JobEntryView> {
        let snap = self.snap.read().unwrap();
        let job = snap.jobs.iter().find(|j| j.id == id)?.clone();
        let counters = self
            .counters
            .read()
            .unwrap()
            .get(&id)
            .cloned()
            .unwrap_or_else(|| counters_from_job(&job));
        // OnCompletion + session speed live in the daemon. UI calls
        // `Client::job_entry` to refresh this on demand.
        Some(JobEntryView {
            job,
            counters,
            on_completion: Default::default(),
            session_speed_override: 0,
        })
    }
}

fn counters_from_job(job: &Job) -> JobCounters {
    JobCounters {
        id: job.id,
        phase: job.status.phase,
        downloaded: job.status.downloaded,
        total: job.status.total,
        speed_bps: 0.0,
        is_resumable: 0,
        running: false,
        parts: Vec::<PartView>::new(),
    }
}

/// Spawn the background task that drains daemon events and keeps
/// `Cache` fresh. `wake` is called whenever the cache changes so the
/// egui thread requests a repaint.
pub fn spawn_event_loop(
    rt: &Handle,
    client: Arc<Client>,
    cache: Arc<Cache>,
    filter: SubFilter,
    wake: impl Fn() + Send + Sync + 'static,
) {
    let _g = rt.enter();
    tokio::spawn(async move {
        if let Err(e) = client.subscribe(filter).await {
            tracing::warn!(error = %e, "ipc_local subscribe failed");
            return;
        }
        let Some(mut rx) = client.take_events().await else {
            tracing::error!("ipc_local event receiver already taken");
            return;
        };
        while let Some(ev) = rx.recv().await {
            handle_event(&client, &cache, ev).await;
            wake();
        }
        tracing::info!("ipc_local event stream ended");
        DAEMON_LOST.store(true, Ordering::Relaxed);
        wake();
    });
}

async fn handle_event(client: &Arc<Client>, cache: &Arc<Cache>, ev: Event) {
    match ev {
        Event::Counters(list) => {
            let mut map = cache.counters.write().unwrap();
            for c in list {
                map.insert(c.id, c);
            }
        }
        Event::JobsChanged
        | Event::QueuesChanged
        | Event::SettingsChanged
        | Event::ActiveQueuesChanged
        | Event::ConflictChanged => {
            refresh_snapshot(client, cache).await;
        }
        Event::HostListChanged => {
            refresh_host_list(client, cache).await;
        }
        Event::Focus => {
            FOCUS_REQUESTED.store(true, Ordering::Relaxed);
        }
        Event::Close => {
            CLOSE_REQUESTED.store(true, Ordering::Relaxed);
        }
        Event::JobCompleted { .. }
        | Event::JobFailed { .. }
        | Event::Updater(_)
        | Event::OpenDownloadDialog(_)
        | Event::ShowMainWindow => {
            // Forwarded to the UI bus by the caller of this loop, not
            // handled here.
        }
    }
}

static FOCUS_REQUESTED: AtomicBool = AtomicBool::new(false);
static CLOSE_REQUESTED: AtomicBool = AtomicBool::new(false);
static DAEMON_LOST: AtomicBool = AtomicBool::new(false);

/// Returns `true` once after the daemon sends `Event::Close` to this
/// process (e.g. when the daemon evicts the old per-download window
/// before spawning a fresh one).
pub fn close_requested() -> bool {
    CLOSE_REQUESTED.load(Ordering::Relaxed)
}

/// Returns `true` once for each `Event::Focus` the daemon sends.
/// Shells call this per frame and surface the window when set.
pub fn take_focus_request() -> bool {
    FOCUS_REQUESTED.swap(false, Ordering::Relaxed)
}

/// Set by the cache loop when the IPC stream ends (daemon died /
/// restarted). Shells poll this per frame and exit gracefully.
pub fn daemon_lost() -> bool {
    DAEMON_LOST.load(Ordering::Relaxed)
}

/// Send all the viewport commands needed to surface a hidden /
/// minimized / behind-other-apps window.
pub fn surface_window(ctx: &eframe::egui::Context) {
    use eframe::egui::{ViewportCommand, WindowLevel};
    ctx.send_viewport_cmd(ViewportCommand::Visible(true));
    // Toggle AlwaysOnTop to coerce the WM into raising the window above
    // siblings. ViewportCommand::Focus alone is a no-op on most Linux
    // WMs (focus-stealing prevention) and on Windows when another app
    // owns the foreground. The toggle pulses the X11/Wayland stacking
    // order without leaving the window pinned.
    ctx.send_viewport_cmd(ViewportCommand::WindowLevel(WindowLevel::AlwaysOnTop));
    ctx.send_viewport_cmd(ViewportCommand::Focus);
    ctx.send_viewport_cmd(ViewportCommand::WindowLevel(WindowLevel::Normal));
    ctx.send_viewport_cmd(ViewportCommand::RequestUserAttention(
        eframe::egui::UserAttentionType::Critical,
    ));
}

pub async fn refresh_snapshot(client: &Arc<Client>, cache: &Arc<Cache>) {
    match client.snapshot().await {
        Ok(snap) => {
            let main = snap.queues.iter().find(|q| q.builtin).map(|q| q.id);
            *cache.snap.write().unwrap() = snap.clone();
            let mut counters_map = indexmap::IndexMap::with_capacity(snap.counters.len());
            for c in snap.counters {
                counters_map.insert(c.id, c);
            }
            *cache.counters.write().unwrap() = counters_map;
            if let Some(m) = main {
                *cache.main_queue_id.write().unwrap() = m;
            }
        }
        Err(e) => tracing::warn!(error = %e, "snapshot refresh failed"),
    }
}

pub async fn refresh_host_list(client: &Arc<Client>, cache: &Arc<Cache>) {
    match client.host_list().await {
        Ok(v) => *cache.host_list.write().unwrap() = v,
        Err(e) => tracing::warn!(error = %e, "host list refresh failed"),
    }
}

//! Daemon-side IPC server.
//!
//! Binds the per-user local socket and accepts connections. Each
//! connection runs a per-conn task that reads `Frame::Request`s,
//! dispatches against `AppState`, and writes back `Frame::Reply`s.
//!
//! After the client sends `Request::Subscribe`, two background tasks
//! start: one forwards relevant `DomainEvent`s as `Frame::Event`s,
//! the other ticks at 250 ms and pushes a `Frame::Event::Counters`
//! dump scoped to the subscription filter. Both share a write-side
//! mutex with the request loop so writes never interleave.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::Ordering as AtomicOrd;
use std::time::Duration;

use interprocess::local_socket::{
    GenericNamespaced, ListenerOptions, ToNsName, tokio::Stream as IpcStream,
    traits::tokio::Listener as _,
};
use tokio::sync::{Mutex, mpsc};

use super::codec::{CodecError, read_frame, write_frame};
use super::protocol::{
    AddJobReq, Event, FileChangedRes, FinalFileRes, Frame, GuiKind, JobCounters, JobEntryView,
    NotResumableRes, PartView, Reply, Request, SameDownloadRes, SnapshotData, SubFilter,
};
use crate::data::{AppState, ConflictKind, DomainEvent, JobEntry, RemoveOpts};
use crate::domain::{JobError, JobId};

/// Per-`GuiKind` registry of live event-pump senders. Updated as
/// connections call `Request::Hello` and torn down on disconnect.
type FocusRegistry = std::sync::Mutex<HashMap<GuiKind, mpsc::Sender<Event>>>;
fn registry() -> &'static FocusRegistry {
    static REG: OnceLock<FocusRegistry> = OnceLock::new();
    REG.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// Which GUI windows currently hold keyboard focus, as reported by the
/// windows themselves (`Request::WindowFocused`). Cleared with the rest
/// of a connection's state on disconnect.
type FocusState = std::sync::Mutex<HashMap<GuiKind, bool>>;
fn focus_state() -> &'static FocusState {
    static FOCUS: OnceLock<FocusState> = OnceLock::new();
    FOCUS.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// Is this window open *and* focused? A window the user is already
/// looking at does not need to be replaced to be seen.
pub fn is_focused(kind: GuiKind) -> bool {
    focus_state()
        .lock()
        .map(|f| f.get(&kind).copied().unwrap_or(false))
        .unwrap_or(false)
}

/// Try to surface an existing GUI process matching `kind`. Returns
/// `true` when a `Focus` event was queued onto its connection — the
/// caller should then *not* spawn a duplicate subprocess.
pub fn try_focus(kind: GuiKind) -> bool {
    let Ok(reg) = registry().lock() else {
        return false;
    };
    let Some(tx) = reg.get(&kind).cloned() else {
        return false;
    };
    drop(reg);
    tx.try_send(Event::Focus).is_ok()
}

/// Ask an existing GUI subprocess (matched by `Hello` kind) to exit.
/// Used when the caller prefers to spawn a fresh window over raising
/// the old one. Returns `true` if a `Close` event was queued.
pub fn try_close(kind: GuiKind) -> bool {
    let Ok(mut reg) = registry().lock() else {
        return false;
    };
    let Some(tx) = reg.remove(&kind) else {
        return false;
    };
    drop(reg);
    tx.try_send(Event::Close).is_ok()
}

/// Bind the IPC socket and run the accept loop until the daemon
/// terminates. Designed to be `tokio::spawn`ed by the daemon main.
pub async fn serve(state: Arc<AppState>) -> std::io::Result<()> {
    let name = super::socket_name();
    tracing::info!(socket = %name, "ipc_local listening");
    let ns = name.as_str().to_ns_name::<GenericNamespaced>()?;
    let listener = match ListenerOptions::new().name(ns).create_tokio() {
        Ok(l) => l,
        Err(e) => return Err(e),
    };

    loop {
        match listener.accept().await {
            Ok(stream) => {
                let state = state.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_conn(stream, state).await {
                        tracing::debug!(error = %e, "ipc_local conn ended");
                    }
                });
            }
            Err(e) => {
                tracing::warn!(error = %e, "ipc_local accept failed");
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    }
}

#[derive(Default)]
struct ConnState {
    sub: Option<SubFilter>,
    /// `Hello` kind, if announced. Used to deregister from the focus
    /// registry on disconnect.
    kind: Option<GuiKind>,
    pump_handles: Vec<tokio::task::JoinHandle<()>>,
    /// Sender into the per-conn event pump. Cloned into the focus
    /// registry so `try_focus` can poke this connection from outside.
    event_tx: Option<mpsc::Sender<Event>>,
}

/// Atomic "child is fully ready" promotion.
///
/// Both prerequisites must hold:
///   - `Hello(kind)` arrived → `conn_state.kind` is `Some`.
///   - `Subscribe` arrived → `conn_state.event_tx` is `Some` (the
///     focus-registry uses this `tx` to deliver `Event::Focus` /
///     `Event::Close`; without it, `try_close` is a no-op even if
///     the registry has the kind).
///
/// Until both land, the tray spawn state stays `Spawning` and
/// `try_spawn` keeps blocking duplicate triggers — fixes the
/// double-click race where Hello completes fast but Subscribe is
/// delayed by eframe / GL init.
fn register_if_ready(conn_state: &ConnState) {
    let (Some(kind), Some(tx)) = (conn_state.kind, conn_state.event_tx.clone()) else {
        return;
    };
    if let Ok(mut r) = registry().lock() {
        r.insert(kind, tx);
    }
    crate::daemon::tray::mark_registered(kind);
}

impl Drop for ConnState {
    fn drop(&mut self) {
        if let Some(k) = self.kind
            && let Ok(mut r) = registry().lock()
        {
            // Only deregister if the entry still points at *this*
            // connection's sender. A re-spawn (try_close + spawn)
            // can race the old connection's Drop and reinstall a
            // new sender under the same key — we must not evict it.
            let our_tx = self.event_tx.as_ref();
            let same = match (r.get(&k), our_tx) {
                (Some(reg_tx), Some(ours)) => reg_tx.same_channel(ours),
                (Some(_), None) => false,
                _ => false,
            };
            if same {
                r.remove(&k);
                if let Ok(mut f) = focus_state().lock() {
                    f.remove(&k);
                }
                // The Spawning/Registered tracking owned by the
                // tray module is keyed by the *kind*, not by this
                // connection — only clear it on a clean disconnect
                // (`same`), so a re-spawn that already raced past
                // us doesn't get its `Registered` entry wiped.
                crate::daemon::tray::clear_pending(k);
            }
        }
        for h in self.pump_handles.drain(..) {
            h.abort();
        }
    }
}

async fn handle_conn(stream: IpcStream, state: Arc<AppState>) -> Result<(), CodecError> {
    let stream = Arc::new(stream);
    let writer = Arc::new(Mutex::new(()));
    let mut conn_state = ConnState::default();

    loop {
        let frame: Frame = {
            let mut r = &*stream;
            match read_frame(&mut r).await {
                Ok(f) => f,
                Err(CodecError::Closed) => return Ok(()),
                Err(e) => return Err(e),
            }
        };
        let Frame::Request(req_id, req) = frame else {
            continue;
        };

        // Subscribe is special: it spawns the event pump tasks rather
        // than producing a payload reply.
        if let Request::Subscribe(filter) = req {
            for h in conn_state.pump_handles.drain(..) {
                h.abort();
            }
            conn_state.sub = Some(filter);
            let event_tx = spawn_event_pump(
                state.clone(),
                stream.clone(),
                writer.clone(),
                filter,
                &mut conn_state.pump_handles,
            );
            conn_state.event_tx = Some(event_tx);
            // Now that event_tx is known, register in the focus
            // registry if Hello already arrived.
            register_if_ready(&conn_state);
            let _g = writer.lock().await;
            let mut w = &*stream;
            write_frame(&mut w, &Frame::Reply(req_id, Reply::Ok)).await?;
            continue;
        }

        if let Request::WindowFocused(focused) = req {
            if let Some(kind) = conn_state.kind
                && let Ok(mut f) = focus_state().lock()
            {
                f.insert(kind, focused);
            }
            let _g = writer.lock().await;
            let mut w = &*stream;
            write_frame(&mut w, &Frame::Reply(req_id, Reply::Ok)).await?;
            continue;
        }

        if let Request::Hello(kind) = req {
            conn_state.kind = Some(kind);
            // Defer registry insert + tray "Registered" until both
            // Hello *and* Subscribe have landed — until then, the
            // server registry has no usable tx for `try_close` and a
            // click 2 would fail to evict the in-flight child. The
            // tray spawn state stays `Spawning`, blocking duplicates.
            register_if_ready(&conn_state);
            let _g = writer.lock().await;
            let mut w = &*stream;
            write_frame(&mut w, &Frame::Reply(req_id, Reply::Ok)).await?;
            continue;
        }

        let reply = dispatch(&state, req).await;
        let _g = writer.lock().await;
        let mut w = &*stream;
        write_frame(&mut w, &Frame::Reply(req_id, reply)).await?;
    }
}

fn spawn_event_pump(
    state: Arc<AppState>,
    stream: Arc<IpcStream>,
    writer: Arc<Mutex<()>>,
    filter: SubFilter,
    handles: &mut Vec<tokio::task::JoinHandle<()>>,
) -> mpsc::Sender<Event> {
    // Bridge that sequences both "domain event" and "tick" producers
    // through a single mpsc; the consumer writes them out under the
    // shared write lock. This keeps writes serial without forcing the
    // request loop to coordinate with two sibling tasks.
    let (tx, mut rx) = mpsc::channel::<Event>(512);
    let tx_returned = tx.clone();

    // Domain-event forwarder.
    {
        let state = state.clone();
        let tx = tx.clone();
        handles.push(tokio::spawn(async move {
            let mut bus = state.subscribe();
            loop {
                let ev = match bus.recv().await {
                    Ok(e) => e,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                };
                let Some(out) = map_domain_event(filter, ev) else {
                    continue;
                };
                if tx.send(out).await.is_err() {
                    break;
                }
            }
        }));
    }

    // Counter pump (skipped for `Lifecycle` subscribers).
    if !matches!(filter, SubFilter::Lifecycle) {
        let state = state.clone();
        let tx = tx.clone();
        handles.push(tokio::spawn(async move {
            let mut iv = tokio::time::interval(Duration::from_millis(250));
            iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            iv.tick().await;
            loop {
                iv.tick().await;
                let counters = collect_counters(&state, filter).await;
                if counters.is_empty() {
                    continue;
                }
                if tx.send(Event::Counters(counters)).await.is_err() {
                    break;
                }
            }
        }));
    }

    // Writer task — single owner of the conn for outbound events.
    handles.push(tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            let _g = writer.lock().await;
            let mut w = &*stream;
            if write_frame(&mut w, &Frame::Event(ev)).await.is_err() {
                break;
            }
        }
    }));

    tx_returned
}

fn matches_filter(filter: SubFilter, id: JobId) -> bool {
    match filter {
        SubFilter::All | SubFilter::Lifecycle => true,
        SubFilter::Job(j) => j == id,
    }
}

fn map_domain_event(filter: SubFilter, ev: DomainEvent) -> Option<Event> {
    match ev {
        DomainEvent::JobAdded { .. }
        | DomainEvent::JobRemoved { .. }
        | DomainEvent::JobUpdated { .. }
        | DomainEvent::JobFilenameResolved { .. }
        | DomainEvent::JobPartAdded { .. }
        | DomainEvent::JobPartFinished { .. } => Some(Event::JobsChanged),
        // The download window renders the countdown from the part rows
        // it already polls, so this only has to reach the window
        // watching that job.
        DomainEvent::JobRetryScheduled {
            id,
            ulid,
            attempt,
            max_attempts,
            delay_ms,
            server_requested,
        } if matches_filter(filter, id) => Some(Event::RetryScheduled {
            id,
            ulid,
            attempt,
            max_attempts,
            delay_ms,
            server_requested,
        }),
        DomainEvent::JobRetryScheduled { .. } => None,
        DomainEvent::JobVerifyFailed { id, message } if matches_filter(filter, id) => {
            Some(Event::VerifyFailed { id, message })
        }
        DomainEvent::JobVerifyFailed { .. } => None,
        DomainEvent::JobCompleted { id, path, .. } if matches_filter(filter, id) => {
            Some(Event::JobCompleted { id, path })
        }
        DomainEvent::JobCompleted { .. } => None,
        DomainEvent::JobFailed { id, error } if matches_filter(filter, id) => {
            Some(Event::JobFailed { id, error })
        }
        DomainEvent::JobFailed { .. } => None,
        DomainEvent::SettingsChanged => Some(Event::SettingsChanged),
        DomainEvent::ConflictRequested { .. } => Some(Event::ConflictChanged),
        DomainEvent::OpenDownloadDialog { id } => Some(Event::OpenDownloadDialog(id)),
        DomainEvent::ShowMainWindow => Some(Event::ShowMainWindow),
        DomainEvent::QueueStarted { .. } | DomainEvent::QueueFinished { .. } => {
            Some(Event::ActiveQueuesChanged)
        }
        DomainEvent::QueuesChanged => Some(Event::QueuesChanged),
        DomainEvent::ShutdownPending {
            action,
            deadline_ms,
        } => Some(Event::ShutdownPending {
            action,
            deadline_ms,
        }),
        DomainEvent::ShutdownCancelled => Some(Event::ShutdownCancelled),
    }
}

async fn collect_counters(state: &Arc<AppState>, filter: SubFilter) -> Vec<JobCounters> {
    let jobs = state.list_jobs().await;
    let mut out = Vec::with_capacity(jobs.len());
    for j in jobs {
        if !matches_filter(filter, j.id) {
            continue;
        }
        let Some(entry) = state.job_entry(j.id).await else {
            continue;
        };
        out.push(snapshot_counters(j.id, &entry));
        if matches!(filter, SubFilter::Job(_)) {
            // Single-job filter — we already have the one we wanted.
            break;
        }
    }
    out
}

fn snapshot_counters(id: JobId, entry: &JobEntry) -> JobCounters {
    let phase = entry.phase();
    let downloaded = entry.counters.downloaded();
    let total = entry.counters.total();
    let speed_bps = if phase.is_running() {
        entry.counters.speed_bps()
    } else {
        0.0
    };
    let is_resumable = entry.is_resumable.load(AtomicOrd::Acquire);
    let running = entry.running.load(AtomicOrd::Acquire);
    let retries = entry.retries.load(AtomicOrd::Relaxed);
    let parts = entry
        .parts
        .read()
        .map(|g| {
            g.values()
                .map(|p| PartView {
                    ulid: p.ulid.clone(),
                    offset: p.offset,
                    size: p.size.load(AtomicOrd::Relaxed),
                    downloaded: p.downloaded.load(AtomicOrd::Relaxed),
                    speed_bps: f64::from_bits(p.speed_bps_bits.load(AtomicOrd::Relaxed)),
                    finished: p.finished.load(AtomicOrd::Relaxed),
                })
                .collect()
        })
        .unwrap_or_default();
    JobCounters {
        id,
        phase,
        downloaded,
        total,
        speed_bps,
        is_resumable,
        running,
        retries,
        parts,
    }
}

fn job_err_string(e: JobError) -> String {
    e.to_string()
}

async fn dispatch(state: &Arc<AppState>, req: Request) -> Reply {
    match req {
        Request::Ping => Reply::Ok,
        Request::Subscribe(_) => unreachable!("subscribe handled in the conn loop"),
        Request::Hello(_) => unreachable!("hello handled in the conn loop"),
        Request::DaemonQuit => {
            // Reply Ok first so the requesting client gets a clean
            // ack; then run the orderly shutdown (pause downloads →
            // close GUIs via IPC drop → exit) on the runtime.
            crate::daemon::tray::quit_daemon(&tokio::runtime::Handle::current(), state);
            Reply::Ok
        }
        Request::Snapshot => {
            let counters = collect_counters(state, SubFilter::All).await;
            let snap = SnapshotData {
                jobs: state.list_jobs().await,
                queues: state.queues_snapshot().await,
                settings: state.settings().await,
                active_queues: state.active_queue_ids().await,
                conflict_head: state.peek_conflict().await,
                conflict_len: state.conflict_len().await,
                counters,
                pending_shutdown: state.pending_shutdown(),
                cond_available: crate::data::available_conditions(),
            };
            Reply::Snapshot(snap)
        }
        Request::JobEntry(id) => {
            let Some(entry) = state.job_entry(id).await else {
                return Reply::JobEntry(None);
            };
            let counters = snapshot_counters(id, &entry);
            let on_completion = entry
                .on_completion
                .read()
                .ok()
                .map(|g| g.clone())
                .unwrap_or_default();
            Reply::JobEntry(Some(JobEntryView {
                job: crate::data::state::splice_live(&entry),
                counters,
                on_completion,
                session_speed_override: entry.session_speed_override.load(AtomicOrd::Acquire),
                verifying: entry.verifying.load(AtomicOrd::Acquire),
            }))
        }
        Request::AddJob(AddJobReq {
            url,
            save_dir,
            filename,
            referrer,
            headers,
            max_connections,
            proxy,
            auth_user,
            auth_password,
            proxy_password,
            cookies,
            category,
            size,
            checksums,
        }) => match state
            .add_job(
                url,
                save_dir,
                filename,
                referrer,
                headers,
                max_connections,
                proxy,
                auth_user,
                auth_password,
                proxy_password,
                cookies,
                category,
                crate::data::state::ProbeFacts { size, checksums },
            )
            .await
        {
            Ok(id) => Reply::JobAdded(id),
            Err(e) => Reply::Err(job_err_string(e)),
        },
        Request::AddUpdateJob { url, filename } => {
            match state.add_update_job(url, filename).await {
                Ok(id) => Reply::JobAdded(id),
                Err(e) => Reply::Err(job_err_string(e)),
            }
        }
        Request::StartJob { id, manual } => {
            state.mark_run_intent(id, manual).await;
            match state.start_job(id).await {
                Ok(()) => Reply::Ok,
                Err(e) => Reply::Err(job_err_string(e)),
            }
        }
        Request::Pause(id) => match state.pause(id).await {
            Ok(()) => Reply::Ok,
            Err(e) => Reply::Err(job_err_string(e)),
        },
        // Resume / Restart reach the daemon only from a GUI gesture
        // aimed at one download, so their failures are worth a window.
        Request::Resume(id) => {
            state.mark_run_intent(id, true).await;
            match state.resume(id).await {
                Ok(()) => Reply::Ok,
                Err(e) => Reply::Err(job_err_string(e)),
            }
        }
        Request::CancelToQueued(id) => match state.cancel_to_queued(id).await {
            Ok(()) => Reply::Ok,
            Err(e) => Reply::Err(job_err_string(e)),
        },
        Request::VerifyChecksums(id) => match state.verify_checksums(id).await {
            Ok(()) => Reply::Ok,
            Err(e) => Reply::Err(job_err_string(e)),
        },
        Request::DeleteFinalFile(id) => match state.delete_final_file(id).await {
            Ok(()) => Reply::Ok,
            Err(e) => Reply::Err(job_err_string(e)),
        },
        Request::RestartJob(id) => {
            state.mark_run_intent(id, true).await;
            match state.restart_job(id).await {
                Ok(()) => Reply::Ok,
                Err(e) => Reply::Err(job_err_string(e)),
            }
        }
        Request::Remove(id, opts) => {
            let opts = RemoveOpts {
                purge_partial: opts.purge_partial,
                delete_final_file: opts.delete_final_file,
            };
            match state.remove(id, opts).await {
                Ok(None) => Reply::Ok,
                Ok(Some(w)) => Reply::Warning(w),
                Err(e) => Reply::Err(job_err_string(e)),
            }
        }
        Request::SetJobQueue(jid, qid) => match state.set_job_queue(jid, qid).await {
            Ok(()) => Reply::Ok,
            Err(e) => Reply::Err(job_err_string(e)),
        },
        Request::SetJobCategory(jid, cat) => match state.set_job_category(jid, cat).await {
            Ok(()) => Reply::Ok,
            Err(e) => Reply::Err(job_err_string(e)),
        },
        Request::UpdateJobLocation(id, edit) => match state.update_job_location(id, edit).await {
            Ok(()) => Reply::Ok,
            Err(e) => Reply::Err(job_err_string(e)),
        },
        Request::StartQueue(id) => match state.start_queue(id).await {
            Ok(()) => Reply::Ok,
            Err(e) => Reply::Err(e),
        },
        Request::StopQueue(id) => match state.stop_queue(id).await {
            Ok(()) => Reply::Ok,
            Err(e) => Reply::Err(e),
        },
        Request::PauseAll => {
            state.pause_all().await;
            Reply::Ok
        }
        Request::ResumeAll => {
            state.resume_all().await;
            Reply::Ok
        }
        Request::UpsertQueue(q) => match state.upsert_queue(q).await {
            Ok(()) => Reply::Ok,
            Err(e) => Reply::Err(e),
        },
        Request::DeleteQueue(id) => match state.delete_queue(id).await {
            Ok(()) => Reply::Ok,
            Err(e) => Reply::Err(e),
        },
        Request::UpdateSettings(s) => match state.update_settings(*s).await {
            Ok(()) => Reply::Ok,
            Err(e) => Reply::Err(e),
        },
        Request::RegenerateExtToken => match state.regenerate_ext_token().await {
            Ok(_) => Reply::Ok,
            Err(e) => Reply::Err(e),
        },
        Request::SecretsStatus => Reply::SecretsStatus {
            locked: state.is_secrets_locked().await,
        },
        Request::WipeJobSecrets => match state.unlock_via_wipe().await {
            Ok(()) => Reply::Ok,
            Err(e) => Reply::Err(e),
        },
        Request::DbStatus => Reply::DbStatus(state.db_error().await),
        Request::ResetDatabase => match state.reset_database_and_exit().await {
            Ok(()) => Reply::Ok,
            Err(e) => Reply::Err(e),
        },
        Request::JobSecretsPlaintext(id) => {
            let entry = state.job_entry(id).await;
            let (auth_blob, proxy_blob, cookie_blob) = match entry {
                Some(e) => (
                    e.job.enc_auth_password.clone(),
                    e.job.enc_proxy_password.clone(),
                    e.job.enc_cookies.clone(),
                ),
                None => (None, None, None),
            };
            let auth_password = state
                .decrypt_field(
                    id,
                    crate::data::crypto::Field::AuthPassword,
                    auth_blob.as_deref(),
                )
                .await;
            let proxy_password = state
                .decrypt_field(
                    id,
                    crate::data::crypto::Field::ProxyPassword,
                    proxy_blob.as_deref(),
                )
                .await;
            let cookies = state
                .decrypt_field(
                    id,
                    crate::data::crypto::Field::Cookies,
                    cookie_blob.as_deref(),
                )
                .await;
            Reply::JobSecretsPlaintext {
                auth_password,
                proxy_password,
                cookies,
            }
        }
        Request::SetSessionSpeedLimit(id, bps) => {
            match state.set_session_speed_limit(id, bps).await {
                Ok(()) => Reply::Ok,
                Err(e) => Reply::Err(job_err_string(e)),
            }
        }
        Request::SetPersistentSpeedLimit(id, bps) => {
            match state.set_persistent_speed_limit(id, bps).await {
                Ok(()) => Reply::Ok,
                Err(e) => Reply::Err(job_err_string(e)),
            }
        }
        Request::SetMaxConnections(id, n) => match state.set_max_connections(id, n).await {
            Ok(()) => Reply::Ok,
            Err(e) => Reply::Err(job_err_string(e)),
        },
        Request::SetOnCompletion(id, prefs) => match state.set_on_completion(id, prefs).await {
            Ok(()) => Reply::Ok,
            Err(e) => Reply::Err(job_err_string(e)),
        },
        Request::SetJobAdvanced(id, adv) => match state.set_job_advanced(id, adv).await {
            Ok(()) => Reply::Ok,
            Err(e) => Reply::Err(job_err_string(e)),
        },
        Request::SetJobChecksums(id, cs) => match state.set_job_checksums(id, cs).await {
            Ok(()) => Reply::Ok,
            Err(e) => Reply::Err(job_err_string(e)),
        },
        Request::SetJobSource(id, url, save_dir, filename) => {
            match state.set_job_source(id, url, save_dir, filename).await {
                Ok(()) => Reply::Ok,
                Err(e) => Reply::Err(job_err_string(e)),
            }
        }
        Request::PeekConflict => Reply::ConflictHead(state.peek_conflict().await),
        Request::PopConflict => {
            state.pop_conflict().await;
            Reply::Ok
        }
        Request::ResolveFileChanged(id, token, kind) => {
            use odl::conflict::FileChangedResolution as R;
            let r = match kind {
                FileChangedRes::Abort => R::Abort,
                FileChangedRes::Restart => R::Restart,
            };
            state.resolve_file_changed(id, token, r).await;
            Reply::Ok
        }
        Request::ResolveNotResumable(id, token, kind) => {
            use odl::conflict::NotResumableResolution as R;
            let r = match kind {
                NotResumableRes::Abort => R::Abort,
                NotResumableRes::Restart => R::Restart,
            };
            state.resolve_not_resumable(id, token, r).await;
            Reply::Ok
        }
        Request::ResolveSameDownload(id, token, kind) => {
            use odl::conflict::SameDownloadExistsResolution as R;
            let r = match kind {
                SameDownloadRes::Abort => R::Abort,
                SameDownloadRes::AddNumberAndContinue => R::AddNumberToNameAndContinue,
                SameDownloadRes::Resume => R::Resume,
            };
            state.resolve_same_download(id, token, r).await;
            Reply::Ok
        }
        Request::ResolveFinalFile(id, token, kind) => {
            use odl::conflict::FinalFileExistsResolution as R;
            let r = match kind {
                FinalFileRes::Abort => R::Abort,
                FinalFileRes::Replace => R::ReplaceAndContinue,
                FinalFileRes::AddNumberAndContinue => R::AddNumberToNameAndContinue,
            };
            state.resolve_final_file(id, token, r).await;
            Reply::Ok
        }
        Request::UpdateCheck => {
            let ch = state.update_channel().await;
            match ch.check().await {
                Ok(info) => Reply::UpdateInfo(info),
                Err(e) => Reply::Err(e),
            }
        }
        Request::CancelPendingShutdown => {
            state.cancel_pending_shutdown();
            Reply::Ok
        }
        Request::ConfirmPendingShutdown => {
            state.confirm_pending_shutdown();
            Reply::Ok
        }
        Request::Probe(url) => Reply::ProbeResult(state.probe(url).await),
        Request::OpenDownloadWindow(id) => {
            crate::daemon::tray::spawn_download_gui(id);
            Reply::Ok
        }
        Request::OpenPropertiesWindow(id) => {
            crate::daemon::tray::spawn_properties_gui(id);
            Reply::Ok
        }
        Request::OpenMainWindow => {
            crate::daemon::tray::spawn_main_gui();
            Reply::Ok
        }
        Request::OpenSettingsWindow {
            tab,
            highlight_proxy,
        } => {
            crate::daemon::tray::spawn_settings_gui(tab.as_deref(), highlight_proxy);
            Reply::Ok
        }
        Request::OpenQueuesWindow => {
            crate::daemon::tray::spawn_queues_gui();
            Reply::Ok
        }
        // Handled before dispatch (it needs the connection's kind);
        // reaching here means a caller sent it on a connection that
        // never said Hello, which is a no-op rather than an error.
        Request::WindowFocused(_) => Reply::Ok,
        Request::OpenAboutWindow => {
            crate::daemon::tray::spawn_about_gui();
            Reply::Ok
        }
        Request::OpenAddWindow {
            edit_id,
            prefill_url,
        } => {
            crate::daemon::tray::spawn_add_gui(edit_id, prefill_url.as_deref());
            Reply::Ok
        }
        Request::FindJobByFilename(name) => {
            match state.store().find_job_id_by_filename(&name).await {
                Ok(v) => Reply::JobIdOpt(v),
                Err(e) => Reply::Err(e.to_string()),
            }
        }
    }
}

// `ConflictKind` is referenced by `protocol::Reply::ConflictHead` so we
// need it visible here even when the dispatcher doesn't otherwise touch
// it.
const _: fn(ConflictKind) -> ConflictKind = |k| k;

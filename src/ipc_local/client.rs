//! GUI-side IPC client.
//!
//! Connects to the daemon's local socket and offers a request/reply
//! API plus an event subscription stream. One client owns one
//! connection.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use interprocess::local_socket::{
    GenericNamespaced, ToNsName,
    tokio::{Stream as IpcStream, prelude::*},
};
use tokio::sync::{Mutex, mpsc, oneshot};

use super::codec::{CodecError, read_frame, write_frame};
use super::protocol::{
    AddJobReq, Event, FileChangedRes, FinalFileRes, Frame, GuiKind, JobEntryView, NotResumableRes,
    Reply, Request, SameDownloadRes, SnapshotData, SubFilter,
};
use crate::data::{ProbeResult, RemoveOpts, UpdateInfo};
use crate::domain::{Advanced, Checksum, JobError, JobId, OnCompletion, Queue, QueueId, Settings};

type Pending = std::collections::HashMap<u64, oneshot::Sender<Reply>>;

/// Background-task-driven IPC client.
/// Plaintext secrets for a single job, returned in one round-trip by
/// `Client::job_secrets_plaintext` so the Add/Edit dialog can prefill
/// every field at open without N IPC calls. Each field is `None`
/// when the underlying ciphertext column was NULL.
#[derive(Debug, Default, Clone)]
pub struct JobSecretsPlaintext {
    pub auth_password: Option<String>,
    pub proxy_password: Option<String>,
    pub cookies: Option<String>,
}

pub struct Client {
    next_id: AtomicU64,
    pending: Arc<Mutex<Pending>>,
    stream: Arc<IpcStream>,
    write_lock: Arc<Mutex<()>>,
    /// Receiver of unsolicited events. The GUI polls / awaits this
    /// channel from its own task.
    events: Mutex<Option<mpsc::Receiver<Event>>>,
}

impl Client {
    pub async fn connect() -> Result<Arc<Self>, CodecError> {
        let name_owned = super::socket_name();
        let name = name_owned
            .as_str()
            .to_ns_name::<GenericNamespaced>()
            .map_err(CodecError::Io)?;
        let stream = IpcStream::connect(name).await.map_err(CodecError::Io)?;
        let stream = Arc::new(stream);

        let (ev_tx, ev_rx) = mpsc::channel::<Event>(256);
        let pending: Arc<Mutex<Pending>> = Arc::new(Mutex::new(std::collections::HashMap::new()));

        let pending_for_task = pending.clone();
        let stream_for_task = stream.clone();
        tokio::spawn(async move {
            loop {
                let mut r = &*stream_for_task;
                let frame: Frame = match read_frame(&mut r).await {
                    Ok(f) => f,
                    Err(CodecError::Closed) => break,
                    Err(e) => {
                        tracing::warn!(error = %e, "ipc_local client read");
                        break;
                    }
                };
                match frame {
                    Frame::Reply(id, r) => {
                        if let Some(tx) = pending_for_task.lock().await.remove(&id) {
                            let _ = tx.send(r);
                        }
                    }
                    Frame::Event(ev) => {
                        if ev_tx.send(ev).await.is_err() {
                            break;
                        }
                    }
                    Frame::Request(_, _) => {
                        // Unexpected from server; ignore.
                    }
                }
            }
        });

        Ok(Arc::new(Self {
            next_id: AtomicU64::new(1),
            pending,
            stream,
            write_lock: Arc::new(Mutex::new(())),
            events: Mutex::new(Some(ev_rx)),
        }))
    }

    /// Connect with retry — the daemon may still be wiring its socket
    /// when the GUI launches as a child process.
    pub async fn connect_retry(deadline: Duration) -> Result<Arc<Self>, CodecError> {
        let start = std::time::Instant::now();
        loop {
            match Self::connect().await {
                Ok(c) => return Ok(c),
                Err(e) => {
                    if start.elapsed() >= deadline {
                        return Err(e);
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }

    pub async fn request(&self, req: Request) -> Result<Reply, CodecError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        {
            let _g = self.write_lock.lock().await;
            let mut w = &*self.stream;
            write_frame(&mut w, &Frame::Request(id, req)).await?;
        }
        rx.await.map_err(|_| CodecError::Closed)
    }

    /// Take the event receiver (one-consumer model). Returns `None`
    /// after the first call.
    pub async fn take_events(&self) -> Option<mpsc::Receiver<Event>> {
        self.events.lock().await.take()
    }

    // ── ergonomic helpers ──────────────────────────────────────────
    //
    // Each method mirrors a method on `data::AppState`. They translate
    // a `Reply::Err(_)` into a `String` so callers can `?` cleanly,
    // and `unreachable!` on protocol mismatch (would indicate a daemon
    // bug, not a recoverable runtime error).

    pub async fn ping(&self) -> Result<(), String> {
        match self
            .request(Request::Ping)
            .await
            .map_err(|e| e.to_string())?
        {
            Reply::Ok => Ok(()),
            Reply::Err(e) => Err(e),
            _ => unreachable!("ping reply"),
        }
    }

    pub async fn subscribe(&self, filter: SubFilter) -> Result<(), String> {
        self.expect_ok(Request::Subscribe(filter)).await
    }

    pub async fn hello(&self, kind: GuiKind) -> Result<(), String> {
        self.expect_ok(Request::Hello(kind)).await
    }

    pub async fn snapshot(&self) -> Result<SnapshotData, String> {
        match self
            .request(Request::Snapshot)
            .await
            .map_err(|e| e.to_string())?
        {
            Reply::Snapshot(s) => Ok(s),
            Reply::Err(e) => Err(e),
            _ => unreachable!("snapshot reply"),
        }
    }

    pub async fn job_entry(&self, id: JobId) -> Result<Option<JobEntryView>, String> {
        match self
            .request(Request::JobEntry(id))
            .await
            .map_err(|e| e.to_string())?
        {
            Reply::JobEntry(v) => Ok(v),
            Reply::Err(e) => Err(e),
            _ => unreachable!("job entry reply"),
        }
    }

    pub async fn add_job(&self, req: AddJobReq) -> Result<JobId, String> {
        self.expect_added(Request::AddJob(req)).await
    }

    pub async fn add_update_job(
        &self,
        url: url::Url,
        filename: Option<String>,
    ) -> Result<JobId, String> {
        self.expect_added(Request::AddUpdateJob { url, filename })
            .await
    }

    pub async fn start_job(&self, id: JobId) -> Result<(), String> {
        self.expect_ok(Request::StartJob(id)).await
    }
    pub async fn pause(&self, id: JobId) -> Result<(), String> {
        self.expect_ok(Request::Pause(id)).await
    }
    pub async fn resume(&self, id: JobId) -> Result<(), String> {
        self.expect_ok(Request::Resume(id)).await
    }
    pub async fn cancel_to_queued(&self, id: JobId) -> Result<(), String> {
        self.expect_ok(Request::CancelToQueued(id)).await
    }
    pub async fn restart_job(&self, id: JobId) -> Result<(), String> {
        self.expect_ok(Request::RestartJob(id)).await
    }
    pub async fn remove(&self, id: JobId, opts: RemoveOpts) -> Result<(), String> {
        self.expect_ok(Request::Remove(id, opts)).await
    }
    pub async fn set_job_queue(&self, id: JobId, qid: QueueId) -> Result<(), String> {
        self.expect_ok(Request::SetJobQueue(id, qid)).await
    }
    pub async fn set_job_category(
        &self,
        id: JobId,
        category: crate::domain::Category,
    ) -> Result<(), String> {
        self.expect_ok(Request::SetJobCategory(id, category)).await
    }

    pub async fn update_job_location(
        &self,
        id: JobId,
        edit: crate::ipc_local::protocol::JobEdit,
    ) -> Result<(), String> {
        self.expect_ok(Request::UpdateJobLocation(id, edit)).await
    }

    pub async fn start_queue(&self, id: QueueId) -> Result<(), String> {
        self.expect_ok(Request::StartQueue(id)).await
    }
    pub async fn stop_queue(&self, id: QueueId) -> Result<(), String> {
        self.expect_ok(Request::StopQueue(id)).await
    }
    pub async fn pause_all(&self) -> Result<(), String> {
        self.expect_ok(Request::PauseAll).await
    }
    pub async fn resume_all(&self) -> Result<(), String> {
        self.expect_ok(Request::ResumeAll).await
    }
    pub async fn upsert_queue(&self, q: Queue) -> Result<(), String> {
        self.expect_ok(Request::UpsertQueue(q)).await
    }
    pub async fn delete_queue(&self, id: QueueId) -> Result<(), String> {
        self.expect_ok(Request::DeleteQueue(id)).await
    }

    pub async fn update_settings(&self, s: Settings) -> Result<(), String> {
        self.expect_ok(Request::UpdateSettings(Box::new(s))).await
    }
    pub async fn regenerate_ext_token(&self) -> Result<(), String> {
        self.expect_ok(Request::RegenerateExtToken).await
    }
    pub async fn secrets_status(&self) -> Result<bool, String> {
        match self
            .request(Request::SecretsStatus)
            .await
            .map_err(|e| e.to_string())?
        {
            Reply::SecretsStatus { locked } => Ok(locked),
            Reply::Err(e) => Err(e),
            _ => unreachable!("secrets status reply"),
        }
    }

    pub async fn wipe_job_secrets(&self) -> Result<(), String> {
        self.expect_ok(Request::WipeJobSecrets).await
    }

    pub async fn db_status(&self) -> Result<Option<String>, String> {
        match self
            .request(Request::DbStatus)
            .await
            .map_err(|e| e.to_string())?
        {
            Reply::DbStatus(v) => Ok(v),
            Reply::Err(e) => Err(e),
            _ => unreachable!("db status reply"),
        }
    }

    pub async fn reset_database(&self) -> Result<(), String> {
        self.expect_ok(Request::ResetDatabase).await
    }

    pub async fn job_secrets_plaintext(&self, id: JobId) -> Result<JobSecretsPlaintext, String> {
        match self
            .request(Request::JobSecretsPlaintext(id))
            .await
            .map_err(|e| e.to_string())?
        {
            Reply::JobSecretsPlaintext {
                auth_password,
                proxy_password,
                cookies,
            } => Ok(JobSecretsPlaintext {
                auth_password,
                proxy_password,
                cookies,
            }),
            Reply::Err(e) => Err(e),
            _ => unreachable!("job secrets reply"),
        }
    }

    pub async fn set_session_speed_limit(&self, id: JobId, bps: Option<u64>) -> Result<(), String> {
        self.expect_ok(Request::SetSessionSpeedLimit(id, bps)).await
    }
    pub async fn set_persistent_speed_limit(
        &self,
        id: JobId,
        bps: Option<u64>,
    ) -> Result<(), String> {
        self.expect_ok(Request::SetPersistentSpeedLimit(id, bps))
            .await
    }
    pub async fn set_max_connections(&self, id: JobId, n: Option<u64>) -> Result<(), String> {
        self.expect_ok(Request::SetMaxConnections(id, n)).await
    }
    pub async fn set_on_completion(&self, id: JobId, prefs: OnCompletion) -> Result<(), String> {
        self.expect_ok(Request::SetOnCompletion(id, prefs)).await
    }
    pub async fn set_job_advanced(&self, id: JobId, adv: Advanced) -> Result<(), String> {
        self.expect_ok(Request::SetJobAdvanced(id, adv)).await
    }
    pub async fn set_job_checksums(&self, id: JobId, cs: Vec<Checksum>) -> Result<(), String> {
        self.expect_ok(Request::SetJobChecksums(id, cs)).await
    }
    pub async fn set_job_source(
        &self,
        id: JobId,
        url: url::Url,
        save_dir: std::path::PathBuf,
        filename: Option<String>,
    ) -> Result<(), String> {
        self.expect_ok(Request::SetJobSource(id, url, save_dir, filename))
            .await
    }

    pub async fn pop_conflict(&self) -> Result<(), String> {
        self.expect_ok(Request::PopConflict).await
    }
    pub async fn resolve_file_changed(
        &self,
        id: JobId,
        token: u64,
        kind: FileChangedRes,
    ) -> Result<(), String> {
        self.expect_ok(Request::ResolveFileChanged(id, token, kind))
            .await
    }
    pub async fn resolve_not_resumable(
        &self,
        id: JobId,
        token: u64,
        kind: NotResumableRes,
    ) -> Result<(), String> {
        self.expect_ok(Request::ResolveNotResumable(id, token, kind))
            .await
    }
    pub async fn resolve_same_download(
        &self,
        id: JobId,
        token: u64,
        kind: SameDownloadRes,
    ) -> Result<(), String> {
        self.expect_ok(Request::ResolveSameDownload(id, token, kind))
            .await
    }
    pub async fn resolve_final_file(
        &self,
        id: JobId,
        token: u64,
        kind: FinalFileRes,
    ) -> Result<(), String> {
        self.expect_ok(Request::ResolveFinalFile(id, token, kind))
            .await
    }

    pub async fn update_check(&self) -> Result<Option<UpdateInfo>, String> {
        match self
            .request(Request::UpdateCheck)
            .await
            .map_err(|e| e.to_string())?
        {
            Reply::UpdateInfo(v) => Ok(v),
            Reply::Err(e) => Err(e),
            _ => unreachable!("update check reply"),
        }
    }

    /// Outer `Err` = transport failure; inner `Err` = structured probe
    /// error from the daemon (`JobError`).
    pub async fn probe(&self, url: url::Url) -> Result<Result<ProbeResult, JobError>, String> {
        match self
            .request(Request::Probe(url))
            .await
            .map_err(|e| e.to_string())?
        {
            Reply::ProbeResult(v) => Ok(v),
            Reply::Err(e) => Err(e),
            _ => unreachable!("probe reply"),
        }
    }

    pub async fn cancel_pending_shutdown(&self) -> Result<(), String> {
        self.expect_ok(Request::CancelPendingShutdown).await
    }

    pub async fn confirm_pending_shutdown(&self) -> Result<(), String> {
        self.expect_ok(Request::ConfirmPendingShutdown).await
    }

    pub async fn daemon_quit(&self) -> Result<(), String> {
        self.expect_ok(Request::DaemonQuit).await
    }

    pub async fn open_download_window(&self, id: crate::domain::JobId) -> Result<(), String> {
        self.expect_ok(Request::OpenDownloadWindow(id)).await
    }
    pub async fn open_properties_window(&self, id: crate::domain::JobId) -> Result<(), String> {
        self.expect_ok(Request::OpenPropertiesWindow(id)).await
    }
    pub async fn open_main_window(&self) -> Result<(), String> {
        self.expect_ok(Request::OpenMainWindow).await
    }
    pub async fn open_settings_window(
        &self,
        tab: Option<String>,
        highlight_proxy: bool,
    ) -> Result<(), String> {
        self.expect_ok(Request::OpenSettingsWindow {
            tab,
            highlight_proxy,
        })
        .await
    }
    pub async fn open_queues_window(&self) -> Result<(), String> {
        self.expect_ok(Request::OpenQueuesWindow).await
    }
    pub async fn open_about_window(&self) -> Result<(), String> {
        self.expect_ok(Request::OpenAboutWindow).await
    }
    pub async fn open_add_window(
        &self,
        edit_id: Option<JobId>,
        prefill_url: Option<String>,
    ) -> Result<(), String> {
        self.expect_ok(Request::OpenAddWindow {
            edit_id,
            prefill_url,
        })
        .await
    }

    pub async fn find_job_by_filename(&self, name: String) -> Result<Option<JobId>, String> {
        match self
            .request(Request::FindJobByFilename(name))
            .await
            .map_err(|e| e.to_string())?
        {
            Reply::JobIdOpt(v) => Ok(v),
            Reply::Err(e) => Err(e),
            _ => unreachable!("find job by filename reply"),
        }
    }

    async fn expect_ok(&self, req: Request) -> Result<(), String> {
        match self.request(req).await.map_err(|e| e.to_string())? {
            Reply::Ok => Ok(()),
            Reply::Err(e) => Err(e),
            _ => unreachable!("ok reply"),
        }
    }

    async fn expect_added(&self, req: Request) -> Result<JobId, String> {
        match self.request(req).await.map_err(|e| e.to_string())? {
            Reply::JobAdded(id) => Ok(id),
            Reply::Err(e) => Err(e),
            _ => unreachable!("added reply"),
        }
    }
}

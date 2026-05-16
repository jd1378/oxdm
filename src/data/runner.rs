//! Per-job runner. Drives one `Job` through `evaluate → download` while
//! translating ODL progress events to oxdm `DomainEvent`s and updating
//! `LiveCounters` in place.

use std::sync::Arc;

use odl::download_manager::{DownloadManager, DownloadRequest, EvaluateRequest};
use odl::progress::{
    AsyncReporter, DownloadContext, ProgressEvent as OdlProgressEvent, ProgressReporter,
};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::data::events::DomainEvent;
use crate::data::mapping::{job_error_from_odl, phase_from_odl};
use crate::data::resolvers::UiResolver;
use crate::domain::{Job, JobError, JobId, Phase};

/// Live counters per part. The aggregate `LiveCounters` lives on the
/// `JobEntry`; this struct is one entry in `JobEntry::parts`.
#[derive(Debug)]
pub struct PartCounters {
    pub ulid: String,
    pub offset: u64,
    pub size: u64,
    pub downloaded: std::sync::atomic::AtomicU64,
    pub speed_bps_bits: std::sync::atomic::AtomicU64,
    pub finished: std::sync::atomic::AtomicBool,
}

/// Output of running a job to completion (or failure).
pub struct RunOutcome {
    pub final_path: Option<std::path::PathBuf>,
    pub already_complete: bool,
}

/// Drives a single job. One instance per spawn; the spawning task owns
/// the `JoinHandle`.
pub struct JobRunner {
    pub job_id: JobId,
    pub manager: Arc<DownloadManager>,
    pub events: broadcast::Sender<DomainEvent>,
    pub cancel: CancellationToken,
    /// Bridge that forwards `odl::ProgressEvent` to oxdm.
    pub bridge: Arc<dyn LiveBridge>,
    pub interactive: bool,
    /// Per-job override for the metadata / parts directory. `None` ⇒
    /// use the manager's global `download_dir`.
    pub per_job_dir: Option<std::path::PathBuf>,
    /// Shared live knobs (max_connections) lifted from `JobEntry`. We
    /// hand a clone to odl via `DownloadContext::with_live` so the GUI
    /// can mutate the running job's connection count mid-flight.
    pub live_controls: odl::progress::LiveControls,
    /// HTTP Basic password decrypted from the DB just before spawning
    /// the runner. Combined with `Job.auth_user` to build an
    /// `odl::Credentials`. `None` ⇒ no per-job Basic auth.
    pub auth_password: Option<String>,
    /// Proxy password decrypted from the DB just before spawning the
    /// runner. Merged into `Job.proxy` (which stores no password
    /// itself) by `job_overlay_options`. `None` ⇒ no proxy password.
    pub proxy_password: Option<String>,
    /// Cookie jar decrypted from the DB just before spawning the
    /// runner. Injected as a `Cookie` header in the per-run overlay;
    /// never persisted in `Job.headers`. `None` ⇒ no cookies.
    pub cookies: Option<String>,
}

/// Sink the runner uses to push hot per-byte progress to `LiveCounters`.
/// Defined as a trait so `state.rs` (which owns the counters) is the
/// only file that knows their layout.
pub trait LiveBridge: Send + Sync + 'static {
    fn on_event(&self, id: JobId, event: &OdlProgressEvent);
    /// Called once after `evaluate` succeeds with the resume-support
    /// flag the server advertised. Used by the UI's Info tab.
    fn on_evaluated(&self, id: JobId, is_resumable: bool) {
        let _ = (id, is_resumable);
    }
}

impl JobRunner {
    /// Run the job. The caller has already inserted a `JobEntry` for
    /// `job_id` and stored `cancel`; we just drive ODL.
    pub async fn run(self, job: Job) -> Result<RunOutcome, JobError> {
        let url = job.url.clone();
        let save_dir = job.save_dir.clone();

        let reporter = Arc::new(BridgeReporter {
            id: self.job_id,
            inner: self.bridge.clone(),
            events: self.events.clone(),
        });
        let async_reporter: Arc<AsyncReporter> = AsyncReporter::spawn(BridgeReporter {
            id: self.job_id,
            inner: self.bridge.clone(),
            events: self.events.clone(),
        });
        let _ = reporter; // kept for clarity — AsyncReporter wraps a clone

        // Seed the live cap with the job's persisted override (if any)
        // so the first run-loop iteration honours it before any user
        // edit. Subsequent UI edits hit `live_controls.set_max_connections`
        // directly and odl picks them up on the next loop iteration.
        if let Some(n) = job.max_connections {
            self.live_controls.set_max_connections(n as usize);
        }
        let ctx = DownloadContext::new()
            .with_reporter(async_reporter)
            .with_cancel(self.cancel.clone())
            .with_url(url.clone())
            .with_live(self.live_controls.clone());

        let resolver = UiResolver::new(self.job_id, self.events.clone(), self.interactive);

        // Build per-job options overlay (proxy / headers / max_connections
        // from `Job`) on top of the manager's defaults.
        let overlay = crate::data::mapping::job_overlay_options(
            self.manager.config().download(),
            &job,
            self.proxy_password.as_deref(),
            self.cookies.as_deref(),
        )
        .map_err(JobError::Other)?;

        let mut eval_req = EvaluateRequest::new(url, save_dir, &resolver)
            .ctx(&ctx)
            .options(&overlay);
        if let Some(creds) = build_credentials(&job, self.auth_password.as_deref()) {
            eval_req = eval_req.credentials(creds);
        }
        let mut instruction = self
            .manager
            .evaluate(eval_req)
            .await
            .map_err(|e| job_error_from_odl(&e))?;

        self.bridge
            .on_evaluated(self.job_id, instruction.is_resumable());

        // Per-job working directory inside the configured download_dir.
        // Keeps `metadata.pb` / `.part` files isolated so Remove can
        // clean up just this job without touching others. See PLAN §4.5.
        if let Some(per_job) = self.per_job_dir.clone() {
            instruction.set_download_dir(per_job);
        }

        let dl_req = DownloadRequest::new(instruction, &resolver)
            .ctx(&ctx)
            .options(&overlay);
        let path = self
            .manager
            .download(dl_req)
            .await
            .map_err(|e| job_error_from_odl(&e))?;

        Ok(RunOutcome {
            final_path: Some(path),
            already_complete: false,
        })
    }
}

/// Build HTTP Basic credentials from the job's structured fields:
/// `auth_user` from `Job` (persisted), `auth_password` from the OS
/// keyring (loaded by `state::start_job`). Returns `None` when no per-
/// job Basic auth is configured. Other `Authorization` schemes (Bearer,
/// captured tokens, …) flow through `Job.headers` unchanged.
fn build_credentials(
    job: &Job,
    auth_password: Option<&str>,
) -> Option<odl::credentials::Credentials> {
    let user = job.auth_user.as_deref()?;
    Some(odl::credentials::Credentials::new(user, auth_password))
}

/// `ProgressReporter` that fans events out to (a) the shared `LiveBridge`
/// for per-byte updates and (b) the `DomainEvent` broadcast for coarse
/// state changes the UI cares about.
pub struct BridgeReporter {
    id: JobId,
    inner: Arc<dyn LiveBridge>,
    events: broadcast::Sender<DomainEvent>,
}

impl ProgressReporter for BridgeReporter {
    fn on_event(&self, event: OdlProgressEvent) {
        self.inner.on_event(self.id, &event);
        match &event {
            OdlProgressEvent::PhaseChanged(p) => {
                let _ = self.events.send(DomainEvent::JobUpdated {
                    id: self.id,
                    phase: phase_from_odl(*p),
                });
            }
            OdlProgressEvent::FilenameResolved(name) => {
                let _ = self.events.send(DomainEvent::JobFilenameResolved {
                    id: self.id,
                    filename: name.clone(),
                });
            }
            OdlProgressEvent::PartAdded { ulid, offset, size } => {
                let _ = self.events.send(DomainEvent::JobPartAdded {
                    id: self.id,
                    ulid: ulid.clone(),
                    offset: *offset,
                    size: *size,
                });
            }
            OdlProgressEvent::PartFinished { ulid } => {
                let _ = self.events.send(DomainEvent::JobPartFinished {
                    id: self.id,
                    ulid: ulid.clone(),
                });
            }
            OdlProgressEvent::Completed { .. } => {
                // Canonical `JobCompleted` is emitted by the outcome
                // handler in `state.rs` once the runner future returns.
                // Forwarding it here as well caused subscribers
                // (`completion_actions`, `notifications`) to fire twice
                // — including spawning a second download-complete
                // dialog when the focus registry hadn't yet picked up
                // the first spawn.
            }
            OdlProgressEvent::Failed { message } => {
                let _ = self.events.send(DomainEvent::JobFailed {
                    id: self.id,
                    error: JobError::Other(message.clone()),
                });
            }
            OdlProgressEvent::Cancelled => {
                let _ = self.events.send(DomainEvent::JobUpdated {
                    id: self.id,
                    phase: Phase::Paused,
                });
            }
            OdlProgressEvent::Progress { .. }
            | OdlProgressEvent::Speed { .. }
            | OdlProgressEvent::PartProgress { .. }
            | OdlProgressEvent::PartSpeed { .. }
            | OdlProgressEvent::PartRetrying { .. }
            | OdlProgressEvent::Message(_) => {
                // Hot path / UI pulls these from LiveCounters.
            }
        }
    }
}

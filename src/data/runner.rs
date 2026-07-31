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
#[derive(Debug)]
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
    /// Called once after `evaluate` succeeds with the headers the
    /// server sent on that probe (already stripped of credential-
    /// bearing entries). Feeds Properties → Headers → captured
    /// response. Not called when the probe returned no headers.
    fn on_response_headers(&self, id: JobId, captured: crate::domain::CapturedResponse) {
        let _ = (id, captured);
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
            self.auth_password.as_deref(),
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
        if let Some(captured) = crate::data::mapping::captured_response(&instruction) {
            self.bridge.on_response_headers(self.job_id, captured);
        }

        // Feature #14: hand the job's expected checksums to odl so the
        // Verifying phase actually compares — a mismatch surfaces as
        // `JobError::ChecksumMismatch` through the existing error path.
        // Gating (auto_verify) + source filtering (Server/User only)
        // live in `mapping::job_expected_digests`.
        let digests = crate::data::mapping::job_expected_digests(&job);
        if !digests.is_empty() {
            instruction.add_checksums(digests);
        }

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
/// `auth_user` from `Job` (persisted), `auth_password` decrypted from
/// the store (loaded by `state::start_job`). Returns `None` when no
/// per-job Basic auth is configured. When the job's advanced scheme is
/// Bearer, the same decrypted secret is a token and travels as an
/// `Authorization` header via `job_overlay_options` instead (F2) —
/// never as Basic credentials.
fn build_credentials(
    job: &Job,
    auth_password: Option<&str>,
) -> Option<odl::credentials::Credentials> {
    if job.advanced.auth.scheme == crate::domain::AuthScheme::Bearer {
        return None;
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Algo, Checksum, CsSource, CsStatus, JobStatus};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const BODY: &[u8] = b"hello world";
    /// SHA-256 of `BODY`.
    const GOOD_SHA256: &str = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
    /// Same length/charset, wrong value.
    const BAD_SHA256: &str = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde0";

    /// Minimal HTTP/1.1 server on an ephemeral loopback port. Serves
    /// `BODY` for every GET (headers only for HEAD), one request per
    /// connection. No Range support → odl treats it as non-resumable
    /// and downloads in a single part.
    async fn spawn_http_server() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 1024];
                    loop {
                        match sock.read(&mut chunk).await {
                            Ok(0) => return,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                            Err(_) => return,
                        }
                        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                    let head_only = buf.starts_with(b"HEAD ");
                    // `Set-Cookie` is here on purpose: the capture path
                    // must drop it before it can reach the store or UI.
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: \
                         application/octet-stream\r\nSet-Cookie: sid=secret; \
                         Path=/\r\nConnection: close\r\n\r\n",
                        BODY.len()
                    );
                    let _ = sock.write_all(header.as_bytes()).await;
                    if !head_only {
                        let _ = sock.write_all(BODY).await;
                    }
                    let _ = sock.shutdown().await;
                });
            }
        });
        format!("http://{addr}/file.bin")
    }

    /// Records what the runner hands back after `evaluate`.
    #[derive(Default)]
    struct RecordingBridge {
        captured: std::sync::Mutex<Option<crate::domain::CapturedResponse>>,
    }
    impl LiveBridge for RecordingBridge {
        fn on_event(&self, _id: JobId, _event: &OdlProgressEvent) {}
        fn on_response_headers(&self, _id: JobId, captured: crate::domain::CapturedResponse) {
            *self.captured.lock().unwrap() = Some(captured);
        }
    }

    fn test_job(url: &str, save_dir: std::path::PathBuf, sha256: &str) -> Job {
        Job {
            id: JobId::new(),
            url: url::Url::parse(url).unwrap(),
            save_dir,
            filename: Some("file.bin".into()),
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
            created_at: chrono::Utc::now(),
            started_at: None,
            finished_at: None,
            retries: 0,
            status: JobStatus::default(),
            // `auto_verify` defaults to true — the gate under test.
            advanced: crate::domain::Advanced::default(),
            checksums: vec![Checksum {
                algo: Algo::Sha256,
                hash: sha256.into(),
                source: CsSource::User,
                status: CsStatus::Unverified,
                expected: None,
            }],
            category: crate::domain::Category::Other,
            captured_response: None,
        }
    }

    /// Mirrors the real layout: `save_dir` = final destination,
    /// `work_dir` = metadata + `.part` files, per-job subdir under it.
    async fn run_job(job: Job, work_dir: &std::path::Path) -> Result<RunOutcome, JobError> {
        run_job_with(job, work_dir, Arc::new(RecordingBridge::default())).await
    }

    async fn run_job_with(
        job: Job,
        work_dir: &std::path::Path,
        bridge: Arc<dyn LiveBridge>,
    ) -> Result<RunOutcome, JobError> {
        let per_job = crate::data::state::per_job_dir(work_dir, job.id);
        tokio::fs::create_dir_all(&per_job).await.expect("mkdir");
        let cfg = odl::config::ConfigBuilder::default()
            .download_dir(work_dir.to_path_buf())
            .build()
            .expect("odl config");
        let manager = Arc::new(DownloadManager::new(cfg));
        let (events, _rx) = broadcast::channel(64);
        let runner = JobRunner {
            job_id: job.id,
            manager,
            events,
            cancel: CancellationToken::new(),
            bridge,
            interactive: false,
            per_job_dir: Some(per_job),
            live_controls: odl::progress::LiveControls::new(),
            auth_password: None,
            proxy_password: None,
            cookies: None,
        };
        tokio::time::timeout(Duration::from_secs(60), runner.run(job))
            .await
            .expect("runner timed out")
    }

    #[tokio::test]
    async fn user_checksum_mismatch_fails_the_job() {
        let url = spawn_http_server().await;
        let dir = tempfile::tempdir().unwrap();
        let job = test_job(&url, dir.path().join("save"), BAD_SHA256);
        let err = run_job(job, &dir.path().join("work"))
            .await
            .expect_err("mismatch must fail");
        assert!(
            matches!(&err, JobError::ChecksumMismatch { expected, .. }
                if expected.contains(BAD_SHA256)),
            "expected ChecksumMismatch, got {err:?}"
        );
    }

    #[tokio::test]
    async fn evaluate_captures_response_headers_without_secrets() {
        let url = spawn_http_server().await;
        let dir = tempfile::tempdir().unwrap();
        let job = test_job(&url, dir.path().join("save"), GOOD_SHA256);
        let bridge = Arc::new(RecordingBridge::default());
        run_job_with(job, &dir.path().join("work"), bridge.clone())
            .await
            .expect("download should verify");

        let captured = bridge
            .captured
            .lock()
            .unwrap()
            .clone()
            .expect("evaluate must report response headers");
        let names: Vec<_> = captured
            .headers
            .iter()
            .map(|h| h.name.as_str())
            .collect::<Vec<_>>();
        assert!(
            names.contains(&"content-type"),
            "expected content-type in {names:?}"
        );
        assert!(
            !names.contains(&"set-cookie"),
            "Set-Cookie must never be captured: {names:?}"
        );
        assert!(captured.probed_at > 0);
    }

    #[tokio::test]
    async fn user_checksum_match_completes() {
        let url = spawn_http_server().await;
        let dir = tempfile::tempdir().unwrap();
        let job = test_job(&url, dir.path().join("save"), GOOD_SHA256);
        let outcome = run_job(job, &dir.path().join("work"))
            .await
            .expect("download should verify");
        let path = outcome.final_path.expect("final path");
        assert_eq!(std::fs::read(path).unwrap(), BODY);
    }
}

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

/// The engine every oxdm download uses. odl 2.0 can hand media links to
/// yt-dlp, but oxdm has no UI for format choice, and an engine chosen
/// per-URL changes what every window is describing — the segment table
/// and the connection controls mean nothing for a delegated download.
/// Pinned until the design covers it.
pub const FORCED_ENGINE: odl::engine::EnginePreference =
    odl::engine::EnginePreference::Engine(odl::engine::Engine::HttpMultipart);

/// Live counters per part. The aggregate `LiveCounters` lives on the
/// `JobEntry`; this struct is one entry in `JobEntry::parts`.
#[derive(Debug)]
pub struct PartCounters {
    pub ulid: String,
    pub offset: u64,
    /// The part's *current* byte range, not the one it was created
    /// with. odl splits a live part by handing its tail to a new one,
    /// which shortens this — and then reports the shortened part
    /// finished. Held at the original value, a split part reads as
    /// "Complete" at 56%.
    pub size: std::sync::atomic::AtomicU64,
    pub downloaded: std::sync::atomic::AtomicU64,
    pub speed_bps_bits: std::sync::atomic::AtomicU64,
    pub finished: std::sync::atomic::AtomicBool,
}

/// A part's size as reported by odl, with its "no end in sight"
/// sentinel folded into oxdm's own marker for the same thing.
///
/// A server that sends no `Content-Length` gives odl no end to aim
/// for; it stores `Download::UNKNOWN_PART_SIZE` and streams until EOF.
/// Rendered literally that is a segment 16777216 TB long sitting at
/// 0%, so it becomes `0` here — which every reader already treats as
/// "size not known yet".
pub fn part_size(reported: u64) -> u64 {
    if reported == odl::Download::UNKNOWN_PART_SIZE {
        0
    } else {
        reported
    }
}

impl PartCounters {
    /// A progress sample: bytes so far, and the range the part is
    /// *currently* responsible for. odl shortens that range when it
    /// splits a live part, so the total travels with every sample
    /// rather than being fixed at creation.
    pub fn apply_progress(&self, downloaded: u64, total: u64) {
        use std::sync::atomic::Ordering;
        self.downloaded.store(downloaded, Ordering::Relaxed);
        self.size.store(part_size(total), Ordering::Relaxed);
    }

    /// odl finished this part.
    ///
    /// Nothing to reconcile: since 2.0.3 odl emits a final
    /// `PartProgress` with `downloaded == total` immediately before
    /// every `PartFinished`, and emits one when a split shrinks a
    /// part — so the counters already agree by the time this lands.
    pub fn mark_finished(&self) {
        self.finished
            .store(true, std::sync::atomic::Ordering::Release);
    }
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
    /// Answers odl's conflict questions. Built by the caller, which
    /// also publishes it on the `JobEntry` — a resolver the UI cannot
    /// reach leaves every dialog answering into the void, and the run
    /// waiting for a reply that can never arrive.
    pub resolver: Arc<UiResolver>,
}

/// Sink the runner uses to push hot per-byte progress to `LiveCounters`.
/// Defined as a trait so `state.rs` (which owns the counters) is the
/// only file that knows their layout.
#[async_trait::async_trait]
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
    /// Called the moment the assembled file exists, before anything is
    /// checked against it. A verification failure is still a failure
    /// with a file on disk, and a job that does not know where that
    /// file is cannot offer to delete it.
    fn on_final_path(&self, id: JobId, path: std::path::PathBuf) {
        let _ = (id, path);
    }
    /// Called after `evaluate` for a job that was added without one —
    /// the name odl derived from `Content-Disposition` or the URL.
    /// Awaited: every window reads `Job::filename`, and the name has to
    /// be there before the download it belongs to starts.
    async fn on_filename_resolved(&self, id: JobId, filename: String) {
        let _ = (id, filename);
    }
    /// Called after `evaluate` with the checksums the server advertised
    /// in its headers, for the daemon to record on the job. Awaited —
    /// the digests must be on the job (and in the DB) before the
    /// download that will be checked against them starts, or a small
    /// file can finish first and be marked verified against a list that
    /// does not yet include them.
    async fn on_server_checksums(&self, id: JobId, checksums: Vec<crate::domain::Checksum>) {
        let _ = (id, checksums);
    }
    /// Called with one verdict per checksum row the run checked, so the
    /// job records what was true of each rather than one answer for all
    /// of them.
    async fn on_checksum_results(
        &self,
        id: JobId,
        results: Vec<(usize, crate::domain::CsStatus, Option<String>)>,
    ) {
        let _ = (id, results);
    }
}

impl JobRunner {
    /// Run the job. The caller has already inserted a `JobEntry` for
    /// `job_id` and stored `cancel`; we just drive ODL.
    pub async fn run(self, mut job: Job) -> Result<RunOutcome, JobError> {
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

        let resolver = self.resolver.clone();

        // Build per-job options overlay (proxy / headers /
        // max_connections from `Job`) on top of the manager's defaults.
        let overlay = crate::data::mapping::job_overlay_options(
            self.manager.config().download(),
            &job,
            self.proxy_password.as_deref(),
            self.cookies.as_deref(),
            self.auth_password.as_deref(),
        )
        .map_err(JobError::Other)?;

        let mut eval_req = EvaluateRequest::new(url, save_dir, &*resolver)
            .ctx(&ctx)
            .options(&overlay)
            .engine(FORCED_ENGINE);
        if let Some(creds) = build_credentials(&job, self.auth_password.as_deref()) {
            eval_req = eval_req.credentials(creds);
        }
        let mut instruction = self
            .manager
            .evaluate(eval_req)
            .await
            .map_err(|e| job_error_from_odl(&e))?;

        // odl 2.1 keeps `is_resumable: false` on a download that watched
        // the server ignore `Range`, outranking the `accept-ranges` the
        // headers go on advertising — so this is the server's own
        // record, not the header's claim.
        self.bridge
            .on_evaluated(self.job_id, instruction.is_resumable());
        if let Some(captured) = crate::data::mapping::captured_response(&instruction) {
            self.bridge.on_response_headers(self.job_id, captured);
        }

        // What the server advertised in its headers becomes part of the
        // job before anything is checked against it: odl parses those
        // digests during `evaluate` and would otherwise drop them on the
        // floor, since oxdm does its own verification from
        // `Job::checksums`. Recorded first, then merged into this run's
        // copy so this download is checked against them too — not only
        // the next one.
        let advertised = crate::data::mapping::server_checksums(&instruction);
        if !advertised.is_empty() {
            self.bridge
                .on_server_checksums(self.job_id, advertised.clone())
                .await;
            crate::data::mapping::merge_checksums(&mut job.checksums, advertised);
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

        // The name the user chose. odl derives one from the URL or
        // `Content-Disposition`, which is right for a job that was never
        // renamed and wrong for every one that was: the list, the
        // window and Properties all say `Job::filename`, and without
        // this the bytes land under a different name entirely.
        if let Some(name) = job.filename.as_deref().filter(|n| !n.trim().is_empty()) {
            instruction.set_filename(name.to_owned());
        } else {
            // Added without a probe: the job has been sitting in the
            // list under its URL, and this is the first moment anything
            // knows what it is actually called. Awaited so the name is
            // on the job before the first byte lands — a row that
            // renames itself halfway through a download reads as a
            // different download.
            self.bridge
                .on_filename_resolved(self.job_id, instruction.filename().to_owned())
                .await;
        }

        // Per-job working directory inside the configured download_dir.
        // Keeps `metadata.pb` / `.part` files isolated so Remove can
        // clean up just this job without touching others. See PLAN §4.5.
        if let Some(per_job) = self.per_job_dir.clone() {
            instruction.set_download_dir(per_job);
        }

        // A server that stops honouring `Range` mid-download is odl's
        // problem to recover from since 2.1: it asks the resolver, and
        // on `Restart` discards the parts and re-fetches the file whole
        // on one connection. oxdm used to catch the failure, wipe the
        // work directory and re-run — that is gone.
        let dl_req = DownloadRequest::new(instruction, &*resolver)
            .ctx(&ctx)
            .options(&overlay);
        let path = self
            .manager
            .download(dl_req)
            .await
            .map_err(|e| job_error_from_odl(&e))?;
        self.bridge.on_final_path(self.job_id, path.clone());

        // Hashing is oxdm's, not odl's (`verify_checksums(false)`): the
        // file exists by now, so a mismatch can be reported against a
        // download the user still has, and the digests we compute are
        // worth keeping rather than being thrown away inside a verify
        // step. Reported as its own phase — a large file takes a while
        // and a silent pause after 100% reads as a hang.
        let rows = crate::data::mapping::checksum_rows_to_verify(&job);
        if !rows.is_empty() {
            self.bridge.on_event(
                self.job_id,
                &OdlProgressEvent::PhaseChanged(odl::progress::Phase::Verifying),
            );
            let _ = self.events.send(DomainEvent::JobUpdated {
                id: self.job_id,
                phase: Phase::Verifying,
            });
            // Hashing a gigabyte takes seconds. odl 2.2 reports it per
            // block, and oxdm forwards that as a row of its own — the
            // same shape assembly already uses — so the bar moves
            // instead of sitting at 100% hoping.
            let size = tokio::fs::metadata(&path)
                .await
                .map(|m| m.len())
                .unwrap_or(0);
            if size > 0 {
                self.bridge.on_event(
                    self.job_id,
                    &OdlProgressEvent::PartAdded {
                        ulid: odl::progress::VERIFY_ULID.to_string(),
                        offset: 0,
                        size,
                    },
                );
            }
            let outcome = verify_rows(
                &path,
                &job.checksums,
                &rows,
                &self.cancel,
                size,
                |done, total| {
                    self.bridge.on_event(
                        self.job_id,
                        &OdlProgressEvent::PartProgress {
                            ulid: odl::progress::VERIFY_ULID.to_string(),
                            downloaded: done,
                            total,
                        },
                    );
                },
            )
            .await;
            self.bridge.on_event(
                self.job_id,
                &OdlProgressEvent::PartFinished {
                    ulid: odl::progress::VERIFY_ULID.to_string(),
                },
            );
            let results = outcome?;
            // Every row's own verdict, before the run's outcome: a job
            // with a good MD5 and a bad SHA-1 has one of each, and
            // painting them both with the failure told the user their
            // MD5 was wrong when it was the only thing that matched.
            let failed = results.iter().find_map(|(i, status, computed)| {
                (*status == crate::domain::CsStatus::Mismatch).then(|| {
                    (
                        job.checksums[*i].hash.to_ascii_lowercase(),
                        computed.clone().unwrap_or_default(),
                    )
                })
            });
            self.bridge.on_checksum_results(self.job_id, results).await;
            if let Some((expected, actual)) = failed {
                return Err(JobError::ChecksumMismatch { expected, actual });
            }
        }

        Ok(RunOutcome {
            final_path: Some(path),
            already_complete: false,
        })
    }
}

/// Hash `path` once per algorithm and judge every row against it.
///
/// Returns `(row index, verdict, computed digest)` — the shape
/// `AppState::apply_checksum_results` records — so a job carrying two
/// checksums gets two answers instead of one verdict painted over
/// both. Stopping at the first failure is what made a correct MD5
/// beside a wrong SHA-1 read as two failures.
///
/// odl reads the file in 256 KiB blocks off the async runtime, so a
/// multi-gigabyte hash yields between blocks instead of holding a
/// worker — the UI keeps painting while `Checking integrity` is on
/// screen. `on_progress(done, total)` spans every *algorithm*: two
/// checksums of different algorithms means reading the file twice, and
/// a bar that restarted halfway would be measuring the wrong thing.
async fn verify_rows(
    path: &std::path::Path,
    rows: &[crate::domain::Checksum],
    to_check: &[usize],
    cancel: &CancellationToken,
    size: u64,
    mut on_progress: impl FnMut(u64, u64) + Send,
) -> Result<Vec<(usize, crate::domain::CsStatus, Option<String>)>, JobError> {
    use crate::domain::CsStatus;

    let mut algos: Vec<crate::domain::Algo> = Vec::new();
    for i in to_check {
        let algo = rows[*i].algo;
        if !algos.contains(&algo) {
            algos.push(algo);
        }
    }
    let total = size.saturating_mul(algos.len() as u64);

    let mut digests: std::collections::HashMap<crate::domain::Algo, String> =
        std::collections::HashMap::new();
    let mut done_before = 0u64;
    for algo in &algos {
        // Between files, not inside one: odl's reader has no cancel
        // hook, and a pause the user asked for should not wait on the
        // whole list.
        if cancel.is_cancelled() {
            return Err(JobError::Cancelled);
        }
        let mut read_so_far = 0u64;
        let got = odl::hash::HashDigest::from_path_with_progress(
            path,
            crate::data::mapping::odl_algorithm(*algo),
            odl::hash::HashEncoding::Hex,
            |n| {
                read_so_far = read_so_far.saturating_add(n);
                on_progress(done_before.saturating_add(read_so_far), total);
            },
        )
        .await
        .map_err(|e| JobError::Io(e.to_string()))?;
        digests.insert(*algo, got.digest().to_ascii_lowercase());
        done_before = done_before.saturating_add(size);
    }

    Ok(to_check
        .iter()
        .map(|i| {
            let row = &rows[*i];
            let got = digests.get(&row.algo).cloned().unwrap_or_default();
            if got.eq_ignore_ascii_case(row.hash.trim()) {
                (*i, CsStatus::Verified, None)
            } else {
                (*i, CsStatus::Mismatch, Some(got))
            }
        })
        .collect())
}

/// Build HTTP Basic credentials from the job's structured fields:/// Build HTTP Basic credentials from the job's structured fields:
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
            OdlProgressEvent::RetryScheduled {
                ulid,
                attempt,
                max_attempts,
                delay,
                server_requested,
            } => {
                let _ = self.events.send(DomainEvent::JobRetryScheduled {
                    id: self.id,
                    ulid: ulid.clone(),
                    attempt: *attempt,
                    max_attempts: *max_attempts,
                    delay_ms: delay.as_millis() as u64,
                    server_requested: *server_requested,
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
            // Hot path / UI pulls these from LiveCounters. The wildcard
            // also absorbs whatever a future odl engine reports:
            // `ProgressEvent` is `non_exhaustive`, and an event this
            // build has no notion of is not one it can render.
            _ => {}
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

    fn part(size: u64) -> PartCounters {
        PartCounters {
            ulid: "01ULID".into(),
            offset: 0,
            size: std::sync::atomic::AtomicU64::new(size),
            downloaded: std::sync::atomic::AtomicU64::new(0),
            speed_bps_bits: std::sync::atomic::AtomicU64::new(0),
            finished: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn seen(p: &PartCounters) -> (u64, u64) {
        use std::sync::atomic::Ordering;
        (
            p.downloaded.load(Ordering::Relaxed),
            p.size.load(Ordering::Relaxed),
        )
    }

    /// A part that is split keeps the range it still owns, not the one
    /// it was created with — otherwise its bar reads against bytes that
    /// now belong to another part.
    #[test]
    fn a_split_part_follows_its_new_range() {
        let p = part(256 * 1024);
        p.apply_progress(100 * 1024, 256 * 1024);
        assert_eq!(seen(&p), (100 * 1024, 256 * 1024));

        // odl hands the tail to a new part: same bytes downloaded, less
        // of the file to answer for.
        p.apply_progress(100 * 1024, 144 * 1024);
        assert_eq!(seen(&p), (100 * 1024, 144 * 1024));
    }

    /// A server that declares no length leaves odl with a part whose
    /// range has no end; the size arrives as `u64::MAX` and the table
    /// drew a 16777216 TB segment stuck at 0%.
    #[test]
    fn a_part_with_no_declared_end_reads_as_unknown() {
        let p = part(0);
        p.apply_progress(142 * 1024 * 1024, odl::Download::UNKNOWN_PART_SIZE);
        let (downloaded, size) = seen(&p);
        assert_eq!(downloaded, 142 * 1024 * 1024);
        assert_eq!(size, 0, "no end declared is no size, not an exabyte");

        // A real range is still a real range.
        p.apply_progress(1024, 4096);
        assert_eq!(seen(&p), (1024, 4096));
    }

    /// odl closes a part with a full sample before saying it finished
    /// — including a part whose bytes were already on disk, which
    /// otherwise reported nothing at all. The row renders what arrived
    /// rather than second-guessing it.
    #[test]
    fn a_finished_part_reads_as_full() {
        let p = part(256 * 1024);
        // The split shortened it, then the closing sample squared the
        // two counters.
        p.apply_progress(144 * 1024, 144 * 1024);
        p.mark_finished();

        let (downloaded, size) = seen(&p);
        assert_eq!((downloaded, size), (144 * 1024, 144 * 1024));
        assert!(p.finished.load(std::sync::atomic::Ordering::Acquire));
    }

    /// Server that answers every GET with `503` + a long `Retry-After`,
    /// counting the attempts. HEAD still succeeds, so the download gets
    /// as far as scheduling retries. Returns the URL and the counter.
    async fn spawn_retry_server() -> (String, Arc<std::sync::atomic::AtomicUsize>) {
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let counter = hits.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let counter = counter.clone();
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
                    let head = if buf.starts_with(b"HEAD ") {
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: \
                             bytes\r\nConnection: close\r\n\r\n",
                            BODY.len()
                        )
                    } else {
                        counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        // Long enough that a test finishing quickly can
                        // only mean the wait was skipped, not waited out.
                        "HTTP/1.1 503 Service Unavailable\r\nRetry-After: 30\r\nContent-Length: \
                         0\r\nConnection: close\r\n\r\n"
                            .to_owned()
                    };
                    let _ = sock.write_all(head.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        (format!("http://{addr}/file.bin"), hits)
    }

    /// MD5 of `BODY`, as a server would advertise it.
    const GOOD_MD5: &str = "5eb63bbbe01eeed093cb22bb8f5acdc3";
    /// Same length/charset, wrong value.
    const BAD_MD5: &str = "5eb63bbbe01eeed093cb22bb8f5acdc0";

    /// Serves `BODY` with an `X-Checksum-Md5` header carrying whatever
    /// digest the test names — odl parses it during `evaluate`, which
    /// is where oxdm has to pick it up.
    async fn spawn_checksum_server(md5_hex: &'static str) -> String {
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
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: \
                         application/octet-stream\r\nX-Checksum-Md5: \
                         {md5_hex}\r\nConnection: close\r\n\r\n",
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
        server_checksums: std::sync::Mutex<Vec<Checksum>>,
    }
    #[async_trait::async_trait]
    impl LiveBridge for RecordingBridge {
        fn on_event(&self, _id: JobId, _event: &OdlProgressEvent) {}
        fn on_response_headers(&self, _id: JobId, captured: crate::domain::CapturedResponse) {
            *self.captured.lock().unwrap() = Some(captured);
        }
        async fn on_server_checksums(&self, _id: JobId, checksums: Vec<Checksum>) {
            *self.server_checksums.lock().unwrap() = checksums;
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
            interruptions: 0,
            verify_pending: false,
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
        run_job_cancellable(job, work_dir, bridge, CancellationToken::new()).await
    }

    async fn run_job_cancellable(
        job: Job,
        work_dir: &std::path::Path,
        bridge: Arc<dyn LiveBridge>,
        cancel: CancellationToken,
    ) -> Result<RunOutcome, JobError> {
        let per_job = crate::data::state::per_job_dir(work_dir, job.id);
        tokio::fs::create_dir_all(&per_job).await.expect("mkdir");
        // Mirror production: retries enabled (so a 503 schedules a
        // wait rather than failing outright) and hashing left to oxdm.
        let opts = odl::config::DownloadOptionsBuilder::default()
            .max_retries(5)
            .verify_checksums(false)
            .build()
            .expect("odl options");
        let cfg = odl::config::ConfigBuilder::default()
            .download_dir(work_dir.to_path_buf())
            .download(opts)
            .build()
            .expect("odl config");
        let manager = Arc::new(DownloadManager::new(cfg));
        let (events, _rx) = broadcast::channel(64);
        let events2 = events.clone();
        let runner = JobRunner {
            job_id: job.id,
            manager,
            events,
            cancel,
            bridge,
            per_job_dir: Some(per_job),
            live_controls: odl::progress::LiveControls::new(),
            auth_password: None,
            proxy_password: None,
            cookies: None,
            resolver: Arc::new(UiResolver::new(
                job.id,
                events2,
                false,
                crate::domain::LiveCounters::new(),
                Box::new(|| {}),
            )),
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

    /// A digest the server advertised is worth nothing if it is not
    /// written down: oxdm verifies from `Job::checksums`, so a checksum
    /// that stays inside odl's instruction is never checked and never
    /// shown in Properties.
    #[tokio::test]
    async fn a_server_advertised_checksum_reaches_the_job() {
        let url = spawn_checksum_server(GOOD_MD5).await;
        let dir = tempfile::tempdir().unwrap();
        let job = test_job(&url, dir.path().join("save"), GOOD_SHA256);
        let bridge = Arc::new(RecordingBridge::default());
        run_job_with(job, &dir.path().join("work"), bridge.clone())
            .await
            .expect("download should verify");

        let rows = bridge.server_checksums.lock().unwrap().clone();
        assert_eq!(rows.len(), 1, "expected one server row, got {rows:?}");
        assert_eq!(rows[0].algo, Algo::Md5);
        assert_eq!(rows[0].hash, GOOD_MD5);
        assert_eq!(rows[0].source, CsSource::Server);
    }

    /// And it is checked against *this* run, not merely stored for the
    /// next one — a small file can finish before anything else looks at
    /// the list.
    #[tokio::test]
    async fn a_wrong_server_checksum_fails_the_same_run() {
        let url = spawn_checksum_server(BAD_MD5).await;
        let dir = tempfile::tempdir().unwrap();
        let job = test_job(&url, dir.path().join("save"), GOOD_SHA256);
        let err = run_job(job, &dir.path().join("work"))
            .await
            .expect_err("a wrong server digest must fail the job");
        assert!(
            matches!(&err, JobError::ChecksumMismatch { expected, .. }
                if expected.contains(BAD_MD5)),
            "expected ChecksumMismatch on the server digest, got {err:?}"
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

    /// Pausing during a scheduled retry stops the download then, not
    /// when the server's `Retry-After` expires. The wait is 30s; a run
    /// that ends in under two is one that was interrupted.
    #[tokio::test]
    async fn cancel_during_a_retry_wait_stops_now() {
        let (url, hits) = spawn_retry_server().await;
        let dir = tempfile::tempdir().unwrap();
        let job = test_job(&url, dir.path().to_path_buf(), GOOD_SHA256);
        let cancel = CancellationToken::new();
        let stopper = cancel.clone();
        tokio::spawn(async move {
            // Long enough for the first GET to be refused and the wait
            // to start; far short of the 30s it asks for.
            tokio::time::sleep(Duration::from_millis(700)).await;
            stopper.cancel();
        });

        let started = std::time::Instant::now();
        let outcome = run_job_cancellable(
            job,
            dir.path(),
            Arc::new(RecordingBridge::default()),
            cancel,
        )
        .await;
        let elapsed = started.elapsed();

        assert!(
            matches!(outcome, Err(JobError::Cancelled)),
            "a cancelled run reports as cancelled, got {outcome:?}",
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "cancel waited out the retry delay ({elapsed:?})",
        );
        assert!(
            hits.load(std::sync::atomic::Ordering::Relaxed) >= 1,
            "the server was never asked",
        );
    }

    /// Resuming after that pause tries again immediately rather than
    /// serving out the delay the previous attempt was told to wait: a
    /// fresh run carries no memory of it.
    #[tokio::test]
    async fn resume_after_a_retry_wait_asks_again_immediately() {
        let (url, hits) = spawn_retry_server().await;
        let dir = tempfile::tempdir().unwrap();
        let job = test_job(&url, dir.path().to_path_buf(), GOOD_SHA256);

        let cancel = CancellationToken::new();
        let stopper = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(700)).await;
            stopper.cancel();
        });
        let _ = run_job_cancellable(
            job.clone(),
            dir.path(),
            Arc::new(RecordingBridge::default()),
            cancel,
        )
        .await;
        let before = hits.load(std::sync::atomic::Ordering::Relaxed);

        // Second run, stopped just as quickly. If it had inherited the
        // 30s wait it would make no request at all in that window.
        let cancel = CancellationToken::new();
        let stopper = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(700)).await;
            stopper.cancel();
        });
        let _ = run_job_cancellable(
            job,
            dir.path(),
            Arc::new(RecordingBridge::default()),
            cancel,
        )
        .await;

        assert!(
            hits.load(std::sync::atomic::Ordering::Relaxed) > before,
            "the resumed run served out the old delay instead of asking again",
        );
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

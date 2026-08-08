use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use uuid::Uuid;

use crate::domain::{Category, QueueId};

/// Stable identifier for a download job. Independent of URL — a user can
/// queue the same URL twice on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct JobId(pub Uuid);

impl JobId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for JobId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for JobId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for JobId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(JobId)
    }
}

/// Lifecycle phase mirrored from `odl::progress::Phase` plus oxdm-only
/// states (`Queued`, `Paused`). Kept as an oxdm domain enum so the UI
/// never imports `odl` types directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Phase {
    Queued,
    Evaluating,
    ResolvingConflicts,
    Downloading,
    Assembling,
    Flushing,
    Verifying,
    Paused,
    /// oxdm-synth: at least one part is mid-retry (odl emits
    /// `PartRetrying`). The transfer is still live — the runner has not
    /// failed — so this counts as a running phase. Restored to
    /// `Downloading` once every retrying part resumes.
    Reconnecting,
    Completed,
    Failed,
    Cancelled,
}

impl Phase {
    /// Single source of the user-facing phase wording. The pre-transfer
    /// and post-transfer steps read as "Downloading" because they are
    /// invisible plumbing to the user, and every surface (list pill,
    /// window title) must agree on one vocabulary.
    pub fn label(self) -> &'static str {
        match self {
            Self::Evaluating
            | Self::ResolvingConflicts
            | Self::Downloading
            | Self::Assembling
            | Self::Flushing
            | Self::Verifying => "Downloading",
            Self::Reconnecting => "Reconnecting",
            Self::Queued => "Queued",
            Self::Paused => "Paused",
            Self::Cancelled => "Cancelled",
            Self::Completed => "Complete",
            Self::Failed => "Failed",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }

    /// Can a queue run pick this job up? Everything that is not
    /// already running and not already done — a failed job is a
    /// retry, not a reason to refuse to start the queue.
    pub fn is_startable(self) -> bool {
        matches!(
            self,
            Self::Queued | Self::Paused | Self::Failed | Self::Cancelled
        )
    }

    pub fn is_running(self) -> bool {
        matches!(
            self,
            Self::Evaluating
                | Self::ResolvingConflicts
                | Self::Downloading
                | Self::Assembling
                | Self::Flushing
                | Self::Verifying
                | Self::Reconnecting
        )
    }
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobError {
    #[error("network error: {0}")]
    Network(String),
    #[error("dns resolution failed{}: {message}", host.as_ref().map(|h| format!(" for `{h}`")).unwrap_or_default())]
    Dns {
        host: Option<String>,
        message: String,
    },
    /// The server answered, but with a refusal. Kept structured
    /// because the status class decides what the user should do —
    /// 403 is a permissions problem, 404 a wrong address, 429 a
    /// waiting game.
    #[error("HTTP {code}{}", reason.as_ref().map(|r| format!(" {r}")).unwrap_or_default())]
    HttpStatus {
        code: u16,
        reason: Option<String>,
        url: Option<String>,
    },
    #[error("server conflict: {0}")]
    ServerConflict(String),
    /// The server refused a ranged request, so the bytes already on
    /// disk cannot be continued — only discarded and re-fetched.
    /// (odl `ServerConflict::NotResumable`.)
    #[error("server refused to resume: {0}")]
    NotResumable(String),
    /// The remote file changed since this run started (size / ETag /
    /// Last-Modified). Continuing would splice two different files.
    /// (odl `ServerConflict::FileChanged`.)
    #[error("the file on the server changed: {0}")]
    FileChanged(String),
    #[error("save conflict: {0}")]
    SaveConflict(String),
    #[error("a download with filename `{filename}` is already in progress in {save_dir}")]
    DuplicateActive { filename: String, save_dir: String },
    #[error("checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },
    #[error("cancelled")]
    Cancelled,
    #[error("io error: {0}")]
    Io(String),
    /// The destination drive ran out of space. Split from `Io` because
    /// the user can act on it — free space, or save elsewhere — and
    /// the partial download survives either way.
    #[error("out of disk space: {0}")]
    DiskFull(String),
    /// The destination folder rejected the write.
    #[error("can't write to the destination: {0}")]
    PermissionDenied(String),
    /// Conflict surfaced while the job was running in background mode
    /// and the user has set `conflict_while_hidden = NotifyAndPark`.
    /// The job is parked at the end of the queue; user must explicitly
    /// resume to retry.
    #[error("paused due to conflict: {0}")]
    ConflictPending(String),
    #[error("{0}")]
    Other(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeedSample {
    pub bytes_per_second: f64,
    pub at: DateTime<Utc>,
}

/// Live status of a job. Held inside `Job` behind atomics where possible
/// so the UI can poll cheaply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobStatus {
    pub phase: Phase,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub speed_bps: f64,
    pub eta_secs: Option<u64>,
    pub error: Option<JobError>,
    pub final_path: Option<PathBuf>,
}

impl Default for JobStatus {
    fn default() -> Self {
        Self {
            phase: Phase::Queued,
            downloaded: 0,
            total: None,
            speed_bps: 0.0,
            eta_secs: None,
            error: None,
            final_path: None,
        }
    }
}

/// User-facing description of a download.
///
/// `Job` holds the *intent* (url, save path, per-job overrides). Live
/// status comes from `JobStatus` which the runner mutates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub id: JobId,
    pub url: url::Url,
    pub save_dir: PathBuf,
    /// Suggested filename. May be `None` until evaluation resolves it.
    pub filename: Option<String>,
    pub referrer: Option<url::Url>,
    pub headers: indexmap::IndexMap<String, String>,
    /// Per-job override for max parallel parts. `None` = use global config.
    pub max_connections: Option<u64>,
    /// Per-job proxy override. URL form `scheme://[user@]host:port` — the
    /// password (if any) is **not** persisted here; it lives in the OS
    /// keyring under `job-proxy:<id>` and is merged in by the runner.
    /// `None` = use global `Settings::proxy`.
    #[serde(default)]
    pub proxy: Option<String>,
    /// HTTP Basic auth username. The matching password is stored
    /// encrypted at rest in the DB (`enc_auth_password`) using the
    /// AES-GCM master key from the OS keyring. The runner decrypts at
    /// start time and hands `odl::Credentials` to the downloader.
    /// `None` = no per-job Basic auth.
    #[serde(default)]
    pub auth_user: Option<String>,
    /// Encrypted HTTP Basic password (base64 of
    /// `version ‖ nonce ‖ ct+tag`). Travels over IPC unchanged; the
    /// UI treats `is_some()` as the "(stored)" sentinel and can never
    /// decrypt it (no master key in the GUI process).
    #[serde(default)]
    pub enc_auth_password: Option<String>,
    #[serde(default)]
    pub enc_proxy_password: Option<String>,
    #[serde(default)]
    pub enc_cookies: Option<String>,
    /// Persisted per-job speed cap in bytes/sec. `None` = inherit
    /// global `Settings::speed_limit`. Set by the Speed tab when the
    /// user checks "Remember Speed Limiter settings for this file".
    #[serde(default)]
    pub speed_limit_override: Option<u64>,
    /// Queue this job belongs to. Every job belongs to exactly one
    /// queue; deleting a queue reassigns its jobs to the built-in Main
    /// queue.
    pub queue_id: QueueId,
    pub created_at: DateTime<Utc>,
    /// First `Downloading` transition of the current run. `None` until
    /// the job actually starts transferring (a queued-then-removed job
    /// never gets one). Set-once per run; cleared on restart /
    /// cancel-to-queued. Persisted in the `started_at` column.
    #[serde(default)]
    pub started_at: Option<DateTime<Utc>>,
    /// `Completed` transition timestamp. `None` until completion.
    /// Persisted in the `finished_at` column.
    #[serde(default)]
    pub finished_at: Option<DateTime<Utc>>,
    /// Cumulative count of `PartRetrying` events observed during the
    /// current run. Reset on restart / cancel-to-queued. Persisted in
    /// the `retries` column.
    #[serde(default)]
    pub retries: u32,
    /// How many times this download was knocked off course and had to
    /// pick up again: every part retry (a dropped or refused
    /// connection) plus every explicit resume. One number because the
    /// user's question is "did this go cleanly", not "which of the
    /// three mechanisms fired". Reset with the other run stats when a
    /// job restarts from zero.
    #[serde(default)]
    pub interruptions: u32,
    /// A hash check was asked for and has not finished. Persisted so an
    /// interrupted check is one the daemon knows to redo: the work
    /// itself cannot be resumed — a partial hash is worth nothing — but
    /// the *intent* is worth keeping, and re-running it is bounded to
    /// the jobs that were actually mid-check.
    #[serde(default)]
    pub verify_pending: bool,
    pub status: JobStatus,
    /// Per-job advanced settings (Properties dialog → Advanced /
    /// Connection / Cookies / Headers tabs). Persisted as JSON in the
    /// `advanced_json` column.
    #[serde(default)]
    pub advanced: super::Advanced,
    /// Per-job integrity hashes (Properties dialog → Checksums tab).
    /// Persisted as JSON in the `checksums_json` column.
    #[serde(default)]
    pub checksums: Vec<super::Checksum>,
    /// File-type category. Detected once at creation (Add dialog /
    /// captured download) via [`classify`], or set explicitly by the
    /// "Move To Category" menu. Always concrete — defaults to
    /// [`Category::Other`] when nothing matches.
    #[serde(default = "default_category")]
    pub category: Category,
    /// Headers the server sent on the most recent evaluate probe
    /// (Properties → Headers, "captured response"). `None` until the
    /// job has been evaluated at least once. Persisted as JSON in the
    /// `response_headers_json` column.
    #[serde(default)]
    pub captured_response: Option<CapturedResponse>,
}

impl Job {
    /// Every byte is here and the file is not what was promised: the
    /// download finished, then failed its checksum.
    ///
    /// Worth telling apart from every other failure. Those are missing
    /// part of the file and can be resumed, so what is on disk is
    /// progress; this one has nothing left to fetch and a finished
    /// file to deal with.
    pub fn integrity_failed(&self) -> bool {
        matches!(self.status.error, Some(JobError::ChecksumMismatch { .. }))
            || self
                .checksums
                .iter()
                .any(|c| c.status == crate::domain::CsStatus::Mismatch)
    }

    /// Whether the job left an assembled file behind — the thing a
    /// removal can offer to delete.
    pub fn has_saved_file(&self) -> bool {
        self.status.final_path.is_some()
    }
}

fn default_category() -> Category {
    Category::Other
}

/// Per-job actions taken when `Phase::Completed` fires. Maps to the
/// IDM "Options on completion" tab. `show_dialog` suppresses every
/// other option when set — matches IDM's UX where the dialog gives
/// the user the final say, and unattended actions only fire when the
/// dialog is disabled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnCompletion {
    pub show_dialog: bool,
    pub exit_app: bool,
    pub shutdown: Option<ShutdownAction>,
    /// Windows only: pass `/f` to `shutdown`, closing open applications
    /// without waiting for them to save. Ignored elsewhere — it is
    /// picked as part of the power action, not as a separate option.
    /// The alias is the pre-rename wire name — an older GUI talking to
    /// a newer daemon still lands on this field, which always carried
    /// exactly this meaning despite the misleading old label.
    #[serde(alias = "force_terminate")]
    pub force_shutdown: bool,
    /// Turn the machine's network off once the download finishes (IDM's
    /// "disconnect when done"). Runs through the same cancellable grace
    /// timer as the power actions; suppressed when a power action is
    /// already armed, since that takes the link down anyway.
    #[serde(default)]
    pub disconnect: bool,
}

impl Default for OnCompletion {
    fn default() -> Self {
        Self {
            show_dialog: true,
            exit_app: false,
            shutdown: None,
            force_shutdown: false,
            disconnect: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShutdownAction {
    ShutDown,
    Restart,
    Sleep,
}

/// Grace period before a destructive power action (shut down / restart /
/// sleep / hibernate) actually fires, so the user gets a cancellable
/// countdown instead of an instant power-off. 60 s per the queue-hook
/// copy in the design handoff (§3.6 — wins over the §3.3 mock's 30 s).
/// Debug/test override: `OXDM_SHUTDOWN_GRACE_SECS` env var (data layer).
pub const SHUTDOWN_GRACE_SECS: u64 = 60;

/// A destructive power action waiting out the shutdown grace timer.
/// Superset of [`ShutdownAction`]: queue hooks can also hibernate, and
/// the per-job completion tab can drop the network connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerAction {
    ShutDown,
    Restart,
    Sleep,
    Hibernate,
    Disconnect,
}

impl From<ShutdownAction> for PowerAction {
    fn from(a: ShutdownAction) -> Self {
        match a {
            ShutdownAction::ShutDown => PowerAction::ShutDown,
            ShutdownAction::Restart => PowerAction::Restart,
            ShutdownAction::Sleep => PowerAction::Sleep,
        }
    }
}

/// Cheap immutable view of `(Job, JobStatus)` used by the UI layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSnapshot {
    pub job: Job,
}

/// Atomic counters the runner updates on the hot path. Held alongside
/// `Job` in `data::AppState`. UI samples by reading these — no clone of
/// `JobStatus` per progress tick.
#[derive(Debug, Default)]
pub struct LiveCounters {
    pub downloaded: AtomicU64,
    /// `0` means unknown.
    pub total: AtomicU64,
    /// Bytes/sec as `f64` bits, last-window sample.
    pub speed_bps_bits: AtomicU64,
}

impl LiveCounters {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn set_downloaded(&self, v: u64) {
        self.downloaded.store(v, Ordering::Relaxed);
    }

    /// Hard reset: zero progress + total. Called when a job leaves the
    /// completed/in-flight state and goes back to a clean Queued
    /// (cancel-to-queued, remove), so nothing of the old run is left on
    /// screen before the new one reports anything.
    pub fn reset_progress(&self) {
        self.downloaded.store(0, Ordering::Relaxed);
        self.total.store(0, Ordering::Relaxed);
    }
    pub fn set_total(&self, v: Option<u64>) {
        self.total.store(v.unwrap_or(0), Ordering::Relaxed);
    }
    pub fn set_speed(&self, bps: f64) {
        self.speed_bps_bits.store(bps.to_bits(), Ordering::Relaxed);
    }

    pub fn downloaded(&self) -> u64 {
        self.downloaded.load(Ordering::Relaxed)
    }
    pub fn total(&self) -> Option<u64> {
        match self.total.load(Ordering::Relaxed) {
            0 => None,
            v => Some(v),
        }
    }
    pub fn speed_bps(&self) -> f64 {
        f64::from_bits(self.speed_bps_bits.load(Ordering::Relaxed))
    }
}

// ---------------------------------------------------------------- captured response

/// One response header as the server sent it. Repeated names are kept
/// (a response may carry several `Vary` / `Link` lines), so this is a
/// list rather than a map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseHeader {
    pub name: String,
    pub value: String,
}

/// Response headers observed on one `evaluate` probe.
///
/// The values describe **that** probe, not the server's current state —
/// `probed_at` is shown next to them so a stale capture reads as stale.
/// Credential-bearing headers are dropped before the capture reaches
/// this type (`data::mapping::captured_response`); nothing secret is
/// ever persisted or displayed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapturedResponse {
    pub headers: Vec<ResponseHeader>,
    /// When the probe happened, in unix seconds.
    pub probed_at: i64,
}

// ---------------------------------------------------------------- will-send headers

/// One row of the read-only "Request headers (will send)" table
/// (Properties → Headers, feature #7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WillSendHeader {
    pub name: String,
    /// Display value. For `masked` rows this is a placeholder — the
    /// encrypted store is never decrypted for display.
    pub value: String,
    /// Row comes from this job (custom header / cookie jar / auth)
    /// rather than the global settings; the UI accents these.
    pub custom: bool,
    /// The real value lives on an encrypted column; `value` is a
    /// `"(stored)"`-style placeholder.
    pub masked: bool,
}

/// Placeholder value for secret-backed rows.
const STORED_PLACEHOLDER: &str = "(stored)";

/// Header keys the per-run overlay injects from encrypted columns.
/// Exact-case strings — they mirror the literals in
/// `data::mapping::job_overlay_options`.
const COOKIE_KEY: &str = "Cookie";
const AUTHORIZATION_KEY: &str = "Authorization";

/// Compute the request headers oxdm will send for `job`, for display.
///
/// KEEP IN SYNC with the real merge in `data::mapping` (this fn cannot
/// import it: `domain` must stay engine-agnostic, and `mapping` is the
/// data layer):
/// - base headers = `Settings::headers`
///   (`mapping::settings_to_download_options`);
/// - per-job `Job::headers` override base keys (exact-key insert,
///   `mapping::job_overlay_options`);
/// - the decrypted cookie jar is injected as `Cookie` at run time when
///   `enc_cookies` is stored — shown masked here;
/// - Bearer auth travels as `Authorization: Bearer <token>`
///   (`mapping::bearer_header`) — shown masked;
/// - Basic auth rides `odl::Credentials` (`runner::build_credentials`,
///   legacy `Job::auth_user` + `enc_auth_password`) and reaches the
///   wire as an `Authorization: Basic …` header — shown masked;
/// - the User-Agent is an odl *option*, not a headers-map entry:
///   `Settings::user_agent` wins; with no explicit UA and
///   `randomize_user_agent` on, odl picks a random UA per request; the
///   per-job `Advanced::user_agent` field is dead and NEVER shown.
pub fn will_send_headers(settings: &super::Settings, job: &Job) -> Vec<WillSendHeader> {
    let mut rows = Vec::new();

    let ua = match (&settings.user_agent, settings.randomize_user_agent) {
        (Some(ua), _) => ua.clone(),
        (None, true) => "randomized per request".to_owned(),
        (None, false) => "(not set)".to_owned(),
    };
    rows.push(WillSendHeader {
        name: "User-Agent".to_owned(),
        value: ua,
        custom: false,
        masked: false,
    });

    // Stored cookies are only injected while "Send cookies" is on
    // (`start_job` gates the decryption on `cookies_enabled`).
    let cookie_stored = job.enc_cookies.is_some() && job.advanced.cookies_enabled;
    let bearer_stored = matches!(job.advanced.auth.scheme, super::AuthScheme::Bearer)
        && job.enc_auth_password.is_some();
    // Basic credentials reach the wire as Authorization too (reqwest
    // `basic_auth` overrides a same-name custom header) — suppress the
    // duplicate custom row when they will be sent.
    let basic_sent = matches!(
        job.advanced.auth.scheme,
        super::AuthScheme::None | super::AuthScheme::Basic
    ) && job.auth_user.is_some();
    let auth_sent = bearer_stored || basic_sent;

    // Base (global) headers, skipping keys the job overrides.
    for (k, v) in settings.headers.iter() {
        // Case-insensitively, as the wire resolves them: a job's
        // `x-api-key` overrides a global `X-API-Key`.
        if super::has_header(&job.headers, k) {
            continue;
        }
        if (super::header_name_eq(k, COOKIE_KEY) && cookie_stored)
            || (super::header_name_eq(k, AUTHORIZATION_KEY) && auth_sent)
        {
            continue; // replaced by the stored-secret row below
        }
        rows.push(WillSendHeader {
            name: k.clone(),
            value: v.clone(),
            custom: false,
            masked: false,
        });
    }
    // Per-job custom headers (win over base, mirror of the overlay
    // merge). A stored cookie jar / bearer token / Basic credentials
    // replace same-name entries at run time, exactly like
    // `job_overlay_options` + `Credentials` do.
    for (k, v) in job.headers.iter() {
        if (super::header_name_eq(k, COOKIE_KEY) && cookie_stored)
            || (super::header_name_eq(k, AUTHORIZATION_KEY) && auth_sent)
        {
            continue;
        }
        rows.push(WillSendHeader {
            name: k.clone(),
            value: v.clone(),
            custom: true,
            masked: false,
        });
    }

    if cookie_stored {
        rows.push(WillSendHeader {
            name: COOKIE_KEY.to_owned(),
            value: STORED_PLACEHOLDER.to_owned(),
            custom: true,
            masked: true,
        });
    }
    if bearer_stored {
        rows.push(WillSendHeader {
            name: AUTHORIZATION_KEY.to_owned(),
            value: format!("Bearer {STORED_PLACEHOLDER}"),
            custom: true,
            masked: true,
        });
    } else if basic_sent {
        // Legacy/Basic credentials become an Authorization header on
        // the wire via odl::Credentials.
        rows.push(WillSendHeader {
            name: AUTHORIZATION_KEY.to_owned(),
            value: format!("Basic {STORED_PLACEHOLDER}"),
            custom: true,
            masked: true,
        });
    }

    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_job() -> Job {
        Job {
            id: JobId::new(),
            url: url::Url::parse("https://example.com/file.zip").unwrap(),
            save_dir: std::path::PathBuf::from("/tmp/oxdm-test"),
            filename: None,
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
            advanced: crate::domain::Advanced::default(),
            checksums: Vec::new(),
            category: Category::Other,
            captured_response: None,
        }
    }

    #[test]
    fn failed_jobs_are_startable_so_a_queue_of_failures_can_run_again() {
        assert!(Phase::Failed.is_startable());
        assert!(Phase::Queued.is_startable());
        assert!(Phase::Paused.is_startable());
        assert!(Phase::Cancelled.is_startable());
        // Done stays done, and a running job is not re-launched.
        assert!(!Phase::Completed.is_startable());
        assert!(!Phase::Downloading.is_startable());
        assert!(!Phase::Evaluating.is_startable());
    }

    #[test]
    fn will_send_hides_a_global_header_the_job_overrides_in_another_case() {
        let mut settings = crate::domain::Settings::default();
        settings
            .headers
            .insert("X-API-Key".to_owned(), "global".to_owned());
        let mut headers = indexmap::IndexMap::new();
        headers.insert("x-api-key".to_owned(), "per-job".to_owned());
        let job = Job {
            headers,
            ..sample_job()
        };

        let rows = will_send_headers(&settings, &job);
        let keys: Vec<&str> = rows
            .iter()
            .filter(|r| r.name.eq_ignore_ascii_case("x-api-key"))
            .map(|r| r.value.as_str())
            .collect();
        assert_eq!(
            keys,
            vec!["per-job"],
            "the preview must show what the wire sends: the override alone"
        );
    }

    #[test]
    fn on_completion_accepts_pre_rename_payload() {
        // An older GUI still sends `force_terminate` and omits
        // `disconnect`; both must land on the renamed fields.
        let legacy = r#"{"show_dialog":false,"exit_app":false,
            "shutdown":"shut_down","force_terminate":true}"#;
        let oc: OnCompletion = serde_json::from_str(legacy).unwrap();
        assert!(oc.force_shutdown);
        assert!(!oc.disconnect);
        assert_eq!(oc.shutdown, Some(ShutdownAction::ShutDown));
    }
}

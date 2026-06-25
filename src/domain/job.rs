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
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
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
    #[error("server conflict: {0}")]
    ServerConflict(String),
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
    pub force_terminate: bool,
}

impl Default for OnCompletion {
    fn default() -> Self {
        Self {
            show_dialog: true,
            exit_app: false,
            shutdown: None,
            force_terminate: false,
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

    /// Monotonic update: only advance forward. Used by the runner's
    /// progress reporter so a transient `Progress(downloaded = 0)` odl
    /// emits during resume re-evaluation does not roll the UI back to
    /// 0% before it discovers the existing `.part` offsets. Explicit
    /// resets (cancel-to-queued, remove) bypass this via
    /// `reset_progress` / `set_downloaded`.
    pub fn advance_downloaded(&self, v: u64) {
        let mut cur = self.downloaded.load(Ordering::Relaxed);
        while v > cur {
            match self.downloaded.compare_exchange_weak(
                cur,
                v,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return,
                Err(actual) => cur = actual,
            }
        }
    }

    /// Hard reset: zero progress + total. Called when a job leaves the
    /// completed/in-flight state and goes back to a clean Queued
    /// (cancel-to-queued, remove). Without this, monotonic
    /// `advance_downloaded` would keep the old number visible after a
    /// genuine restart of the same job.
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

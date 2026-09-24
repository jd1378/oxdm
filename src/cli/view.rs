//! What the command line prints, as types.
//!
//! The JSON shapes here are the machine contract, so they are built from
//! the domain rather than serialised from it: a field renamed inside the
//! daemon must not change what a script reads, and nothing secret on a
//! `Job` (headers, stored credentials) may ever reach the output.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::failure::{Kind, classify, describe};
use crate::domain::{Job, JobError, Phase, Queue};
use crate::gui::format::{format_bytes, format_speed};
use crate::ipc_local::protocol::JobCounters;

/// Where a download stands, coarsely: the one field a script should
/// branch on. `phase` beside it says exactly which step it is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Queued,
    Active,
    Paused,
    Completed,
    Failed,
    Conflict,
    Cancelled,
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            State::Queued => "queued",
            State::Active => "active",
            State::Paused => "paused",
            State::Completed => "complete",
            State::Failed => "failed",
            State::Conflict => "conflict",
            State::Cancelled => "cancelled",
        }
    }

    pub fn of(phase: Phase) -> Self {
        match phase {
            Phase::Queued => State::Queued,
            Phase::Paused => State::Paused,
            Phase::Completed => State::Completed,
            Phase::Failed => State::Failed,
            Phase::Conflict => State::Conflict,
            Phase::Cancelled => State::Cancelled,
            Phase::Evaluating
            | Phase::ResolvingConflicts
            | Phase::Downloading
            | Phase::Assembling
            | Phase::Flushing
            | Phase::Verifying
            | Phase::Reconnecting => State::Active,
        }
    }
}

pub fn phase_slug(phase: Phase) -> &'static str {
    match phase {
        Phase::Queued => "queued",
        Phase::Evaluating => "evaluating",
        Phase::ResolvingConflicts => "resolving_conflicts",
        Phase::Downloading => "downloading",
        Phase::Assembling => "assembling",
        Phase::Flushing => "flushing",
        Phase::Verifying => "verifying",
        Phase::Paused => "paused",
        Phase::Conflict => "conflict",
        Phase::Reconnecting => "reconnecting",
        Phase::Completed => "completed",
        Phase::Failed => "failed",
        Phase::Cancelled => "cancelled",
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ErrorView {
    pub kind: &'static str,
    pub message: String,
    /// Trying again unchanged (`oxdm resume`) can be expected to help.
    pub retryable: bool,
}

impl ErrorView {
    pub fn of(e: &JobError) -> Self {
        let (kind, retryable) = classify(e);
        Self {
            kind: kind.slug(),
            message: describe(e),
            retryable,
        }
    }

    pub fn from_failure(kind: Kind, message: &str) -> Self {
        Self {
            kind: kind.slug(),
            message: message.to_owned(),
            retryable: false,
        }
    }
}

fn lossy(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// One download, as `list` and `status` print it.
#[derive(Debug, Clone, Serialize)]
pub struct DownloadView {
    pub id: String,
    pub url: String,
    pub filename: Option<String>,
    pub save_dir: String,
    /// The finished file, or where it will be written once the name is
    /// known.
    pub path: Option<String>,
    pub file_exists: bool,
    pub queue: Option<String>,
    pub category: &'static str,
    pub state: State,
    pub phase: &'static str,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub percent: Option<f64>,
    pub speed_bps: u64,
    pub resumable: Option<bool>,
    pub error: Option<ErrorView>,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

/// The file a job is about: the one it wrote, else the one it will.
pub fn target_path(job: &Job) -> Option<PathBuf> {
    job.status.final_path.clone().or_else(|| {
        job.filename
            .as_deref()
            .filter(|n| !n.is_empty())
            .map(|n| job.save_dir.join(n))
    })
}

impl DownloadView {
    pub fn new(job: &Job, counters: Option<&JobCounters>, queues: &[Queue]) -> Self {
        let phase = job.status.phase;
        let path = target_path(job);
        let total = job.status.total;
        let downloaded = match (phase, total) {
            // A finished job's counter may have been reset by a restart
            // of the daemon; the file is all there.
            (Phase::Completed, Some(t)) => t,
            _ => job.status.downloaded,
        };
        let percent = match (phase, total) {
            (Phase::Completed, _) => Some(100.0),
            (_, Some(t)) if t > 0 => Some((downloaded as f64 / t as f64 * 1000.0).round() / 10.0),
            _ => None,
        };
        Self {
            id: job.id.to_string(),
            url: job.url.to_string(),
            filename: job.filename.clone(),
            save_dir: lossy(&job.save_dir),
            file_exists: path.as_deref().is_some_and(Path::is_file),
            path: path.as_deref().map(lossy),
            queue: queues
                .iter()
                .find(|q| q.id == job.queue_id)
                .map(|q| q.name.clone()),
            category: job.category.slug(),
            state: State::of(phase),
            phase: phase_slug(phase),
            downloaded,
            total,
            percent,
            speed_bps: counters.map(|c| c.speed_bps.max(0.0) as u64).unwrap_or(0),
            resumable: counters.and_then(|c| match c.is_resumable {
                1 => Some(true),
                -1 => Some(false),
                _ => None,
            }),
            error: match phase {
                // A finished or waiting job may still carry the reason an
                // earlier run stopped; it is not this one's.
                Phase::Failed | Phase::Conflict => job.status.error.as_ref().map(ErrorView::of),
                _ => None,
            },
            created_at: job.created_at,
            finished_at: job.finished_at,
        }
    }

    pub fn text(&self) -> String {
        let name = self.filename.as_deref().unwrap_or(self.url.as_str());
        let progress = match (self.percent, self.total) {
            (Some(p), Some(t)) => format!("{p:>5.1}% of {}", format_bytes(t)),
            _ if self.downloaded > 0 => format_bytes(self.downloaded),
            _ => String::new(),
        };
        let mut line = format!(
            "{}  {:<9}  {:<20}  {name}",
            short(&self.id),
            self.state.label(),
            progress
        );
        if self.speed_bps > 0 {
            line.push_str(&format!("  {}", format_speed(self.speed_bps as f64)));
        }
        if let Some(e) = &self.error {
            line.push_str(&format!("\n          {}", e.message));
        }
        line
    }
}

/// The first eight characters of an id: enough to type, and accepted
/// back as a prefix.
pub fn short(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

#[derive(Debug, Clone, Serialize)]
pub struct QueueView {
    pub id: String,
    pub name: String,
    pub builtin: bool,
    pub running: bool,
    pub downloads: usize,
}

/// A checksum the server published, as `probe` reports it.
#[derive(Debug, Clone, Serialize)]
pub struct DigestView {
    pub algorithm: &'static str,
    pub digest: String,
}

pub fn algo_slug(a: crate::domain::Algo) -> &'static str {
    use crate::domain::Algo;
    match a {
        Algo::Md5 => "md5",
        Algo::Sha1 => "sha1",
        Algo::Sha256 => "sha256",
        Algo::Sha384 => "sha384",
        Algo::Sha512 => "sha512",
    }
}

/// Everything the command line prints on stdout, one JSON object per
/// line. `type` names the shape.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Line {
    /// `add` took this URL: a new download, or one already in the list.
    Added {
        id: String,
        url: String,
        queue: Option<String>,
        save_dir: String,
        filename: Option<String>,
        existing: bool,
        state: Action,
    },
    /// This item of the command was not done; the rest may have been.
    Rejected {
        id: Option<String>,
        url: Option<String>,
        error: ErrorView,
    },
    Paused {
        id: String,
        state: State,
    },
    Resumed {
        id: String,
        state: Action,
    },
    Restarted {
        id: String,
        state: Action,
    },
    Removed {
        id: String,
        warning: Option<String>,
    },
    Phase {
        id: String,
        phase: &'static str,
    },
    Filename {
        id: String,
        filename: String,
    },
    Progress {
        id: String,
        downloaded: u64,
        total: Option<u64>,
        speed_bps: u64,
        #[serde(skip)]
        name: Option<String>,
    },
    RetryScheduled {
        id: String,
        part: Option<String>,
        attempt: u32,
        max_attempts: u32,
        delay_ms: u64,
        server_requested: bool,
    },
    Completed {
        id: String,
        path: Option<String>,
        already_complete: bool,
    },
    Failed {
        id: String,
        phase: &'static str,
        error: ErrorView,
    },
    Stopped {
        id: String,
        phase: &'static str,
    },
    Downloads {
        count: usize,
        downloads: Vec<DownloadView>,
    },
    Queues {
        queues: Vec<QueueView>,
    },
    /// Where agent downloads go now (`restore-agent`).
    Agent {
        category: Option<&'static str>,
        save_dir: String,
        queue: String,
    },
    Probe {
        url: String,
        filename: String,
        size: Option<u64>,
        resumable: bool,
        etag: Option<String>,
        last_modified: Option<i64>,
        requires_auth: bool,
        checksums: Vec<DigestView>,
    },
}

/// What a command did to a download's run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Its run started.
    Started,
    /// Every download slot is taken; it starts by itself when one frees.
    Queued,
    /// Added only, as asked (`--no-start`).
    NotStarted,
    /// Already downloading; left alone.
    Running,
    /// Already finished and the file is there; nothing to do.
    Complete,
}

impl Action {
    fn text(self) -> &'static str {
        match self {
            Action::Started => "started",
            Action::Queued => "queued (waiting for a free download slot)",
            Action::NotStarted => "added, not started",
            Action::Running => "already downloading",
            Action::Complete => "already complete",
        }
    }
}

impl Line {
    /// The human rendering. Empty means "nothing worth a line".
    pub fn text(&self) -> String {
        match self {
            Line::Added {
                id,
                url,
                existing,
                state,
                ..
            } => {
                let what = if *existing { "existing" } else { "added" };
                format!("{}  {what}, {}  {url}", short(id), state.text())
            }
            Line::Rejected { id, url, error } => {
                let who = id.as_deref().map(short).or(url.as_deref()).unwrap_or("");
                format!("{who}  not done: {}", error.message)
            }
            Line::Paused { id, state } => format!("{}  {}", short(id), state.label()),
            Line::Resumed { id, state } => format!("{}  resumed, {}", short(id), state.text()),
            Line::Restarted { id, state } => {
                format!("{}  restarted, {}", short(id), state.text())
            }
            Line::Removed { id, warning } => match warning {
                Some(w) => format!("{}  removed; could not delete {w}", short(id)),
                None => format!("{}  removed", short(id)),
            },
            Line::Phase { id, phase } => format!("{}  {phase}", short(id)),
            Line::Filename { id, filename } => format!("{}  saving as {filename}", short(id)),
            Line::Progress {
                id,
                downloaded,
                total,
                speed_bps,
                name,
            } => {
                let amount = match total {
                    Some(t) if *t > 0 => format!(
                        "{:.1}% ({} of {})",
                        *downloaded as f64 / *t as f64 * 100.0,
                        format_bytes(*downloaded),
                        format_bytes(*t)
                    ),
                    _ => format_bytes(*downloaded),
                };
                let speed = if *speed_bps > 0 {
                    format!("  {}", format_speed(*speed_bps as f64))
                } else {
                    String::new()
                };
                format!(
                    "{}  {}  {amount}{speed}",
                    short(id),
                    name.as_deref().unwrap_or("")
                )
            }
            Line::RetryScheduled {
                id,
                attempt,
                max_attempts,
                delay_ms,
                ..
            } => format!(
                "{}  retrying in {:.0}s (attempt {attempt} of {max_attempts})",
                short(id),
                *delay_ms as f64 / 1000.0
            ),
            Line::Completed {
                id,
                path,
                already_complete,
            } => {
                let path = path.as_deref().unwrap_or("");
                if *already_complete {
                    format!("{}  already complete: {path}", short(id))
                } else {
                    format!("{}  complete: {path}", short(id))
                }
            }
            Line::Failed { id, phase, error } => {
                format!("{}  {phase}: {}", short(id), error.message)
            }
            Line::Stopped { id, phase } => format!("{}  stopped ({phase})", short(id)),
            Line::Downloads { downloads, .. } => {
                if downloads.is_empty() {
                    return "no downloads".to_owned();
                }
                downloads
                    .iter()
                    .map(DownloadView::text)
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            Line::Queues { queues } => queues
                .iter()
                .map(|q| {
                    let running = if q.running { "  running" } else { "" };
                    format!(
                        "{}  {} ({} downloads){running}",
                        short(&q.id),
                        q.name,
                        q.downloads
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
            Line::Agent {
                category,
                save_dir,
                queue,
            } => match category {
                Some(_) => {
                    format!("Agent downloads: category Agent, saved to {save_dir}, queue {queue}")
                }
                None => format!("Agent downloads: filed by type, queue {queue}"),
            },
            Line::Probe {
                filename,
                size,
                resumable,
                requires_auth,
                ..
            } => {
                let size = size
                    .map(format_bytes)
                    .unwrap_or_else(|| "unknown size".into());
                let resume = if *resumable {
                    "resumable"
                } else {
                    "not resumable"
                };
                let auth = if *requires_auth {
                    ", needs sign-in"
                } else {
                    ""
                };
                format!("{filename}  {size}, {resume}{auth}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(phase: Phase) -> Job {
        let mut job = crate::cli::fixtures::job();
        job.enc_cookies = Some("ciphertext".into());
        job.headers
            .insert("Authorization".into(), "Bearer secret".into());
        job.status.phase = phase;
        job
    }

    /// Nothing the daemon keeps secret may reach a terminal or a log.
    #[test]
    fn a_view_carries_no_headers_or_stored_secrets() {
        let v = DownloadView::new(&job(Phase::Downloading), None, &[]);
        let json = serde_json::to_string(&v).unwrap();
        assert!(!json.contains("secret"), "{json}");
        assert!(!json.contains("ciphertext"), "{json}");
        assert!(!json.to_lowercase().contains("authorization"), "{json}");
    }

    #[test]
    fn a_waiting_download_does_not_show_an_old_runs_error() {
        let mut j = job(Phase::Queued);
        j.status.error = Some(JobError::Network("reset".into()));
        assert!(DownloadView::new(&j, None, &[]).error.is_none());
        j.status.phase = Phase::Failed;
        let v = DownloadView::new(&j, None, &[]);
        assert_eq!(v.error.as_ref().map(|e| e.kind), Some("network"));
        assert_eq!(v.state, State::Failed);
    }

    #[test]
    fn progress_is_a_share_of_the_known_size() {
        let mut j = job(Phase::Downloading);
        j.status.downloaded = 250;
        j.status.total = Some(1000);
        let v = DownloadView::new(&j, None, &[]);
        assert_eq!(v.percent, Some(25.0));
        assert_eq!(v.state, State::Active);
        assert_eq!(v.path.as_deref(), Some("/nonexistent-oxdm-cli/dl/a.zip"));
        assert!(!v.file_exists);

        j.status.total = None;
        assert_eq!(DownloadView::new(&j, None, &[]).percent, None);
    }

    #[test]
    fn lines_are_tagged_by_type() {
        let line = Line::Completed {
            id: "x".into(),
            path: Some("/dl/a.zip".into()),
            already_complete: false,
        };
        let v = serde_json::to_value(&line).unwrap();
        assert_eq!(v["type"], "completed");
        assert_eq!(v["path"], "/dl/a.zip");

        let progress = Line::Progress {
            id: "x".into(),
            downloaded: 1,
            total: None,
            speed_bps: 0,
            name: Some("a.zip".into()),
        };
        let v = serde_json::to_value(&progress).unwrap();
        assert_eq!(v["type"], "progress");
        assert!(v.get("name").is_none(), "text-only field leaked: {v}");
    }
}

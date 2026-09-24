//! Following downloads to their end.
//!
//! [`Tracker`] decides what each observation means and what to print;
//! [`follow`] feeds it from the daemon. The split keeps every rule about
//! when a download counts as finished testable without a daemon.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::failure::{Failure, Kind, classify};
use super::output::Out;
use super::view::{ErrorView, Line, phase_slug, short};
use crate::domain::{JobError, JobId, Phase};
use crate::ipc_local::Client;
use crate::ipc_local::protocol::{Event, JobCounters, JobEntryView, SubFilter};

/// What one look at a download shows.
#[derive(Debug, Clone, PartialEq)]
pub struct JobState {
    pub phase: Phase,
    pub filename: Option<String>,
    pub path: Option<PathBuf>,
    pub error: Option<JobError>,
}

impl JobState {
    pub fn of(v: &JobEntryView) -> Self {
        Self {
            phase: v.counters.phase,
            filename: v.job.filename.clone(),
            path: v.job.status.final_path.clone(),
            error: v.job.status.error.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Outcome {
    Completed,
    Ended(Kind, String),
}

#[derive(Debug, Default)]
struct Track {
    phase: Option<Phase>,
    filename: Option<String>,
    last_progress: Option<Instant>,
    last_downloaded: u64,
    outcome: Option<Outcome>,
}

pub struct Tracker {
    order: Vec<JobId>,
    tracks: HashMap<JobId, Track>,
    /// Finished before this command started, so nothing was downloaded
    /// on its account.
    already: HashSet<JobId>,
    progress_every: Duration,
}

impl Tracker {
    pub fn new(ids: Vec<JobId>, already: HashSet<JobId>, progress_every: Duration) -> Self {
        Self {
            tracks: ids.iter().map(|id| (*id, Track::default())).collect(),
            order: ids,
            already,
            progress_every,
        }
    }

    pub fn is_done(&self) -> bool {
        self.tracks.values().all(|t| t.outcome.is_some())
    }

    pub fn pending(&self) -> Vec<JobId> {
        self.order
            .iter()
            .filter(|id| self.tracks.get(id).is_some_and(|t| t.outcome.is_none()))
            .copied()
            .collect()
    }

    /// A download's state as the daemon reports it; `None` when it is no
    /// longer in the list.
    pub fn observe(&mut self, id: JobId, state: Option<&JobState>) -> Vec<Line> {
        let already = self.already.contains(&id);
        let Some(t) = self.tracks.get_mut(&id) else {
            return Vec::new();
        };
        if t.outcome.is_some() {
            return Vec::new();
        }
        let ids = id.to_string();
        let Some(s) = state else {
            t.outcome = Some(Outcome::Ended(Kind::Stopped, "it was removed".into()));
            return vec![Line::Stopped {
                id: ids,
                phase: "removed",
            }];
        };
        let mut out = Vec::new();
        // A finished download's own line names its file; "saving as"
        // after the fact would only be noise.
        let finished = matches!(
            s.phase,
            Phase::Completed | Phase::Failed | Phase::Conflict | Phase::Paused | Phase::Cancelled
        );
        if let Some(name) = s.filename.as_ref().filter(|n| !n.is_empty())
            && t.filename.as_ref() != Some(name)
            && !finished
        {
            t.filename = Some(name.clone());
            out.push(Line::Filename {
                id: ids.clone(),
                filename: name.clone(),
            });
        }
        let changed = t.phase != Some(s.phase);
        t.phase = Some(s.phase);
        match s.phase {
            Phase::Completed => {
                t.outcome = Some(Outcome::Completed);
                out.push(Line::Completed {
                    id: ids,
                    path: s.path.as_ref().map(|p| p.to_string_lossy().into_owned()),
                    already_complete: already,
                });
            }
            Phase::Failed | Phase::Conflict => {
                let error = match &s.error {
                    Some(e) => ErrorView::of(e),
                    None => ErrorView::from_failure(
                        Kind::Other,
                        "the download stopped without giving a reason",
                    ),
                };
                let kind = match (&s.phase, &s.error) {
                    (Phase::Conflict, _) => Kind::Conflict,
                    (_, Some(e)) => classify(e).0,
                    (_, None) => Kind::Other,
                };
                t.outcome = Some(Outcome::Ended(kind, error.message.clone()));
                out.push(Line::Failed {
                    id: ids,
                    phase: phase_slug(s.phase),
                    error,
                });
            }
            // Someone else stopped it: a person in the window, or the
            // metered-connection / low-battery guard. Waiting on would be
            // waiting for a decision this command cannot make.
            Phase::Paused | Phase::Cancelled => {
                t.outcome = Some(Outcome::Ended(
                    Kind::Stopped,
                    format!("it was {} before it finished", phase_slug(s.phase)),
                ));
                out.push(Line::Stopped {
                    id: ids,
                    phase: phase_slug(s.phase),
                });
            }
            // Queued (waiting for a slot) or running: keep waiting.
            _ if changed => out.push(Line::Phase {
                id: ids,
                phase: phase_slug(s.phase),
            }),
            _ => {}
        }
        out
    }

    /// A tick of live counters. Returns a progress line when one is due,
    /// and whether the phase moved (the caller then fetches the full
    /// state, which counters do not carry).
    pub fn counters(&mut self, c: &JobCounters, now: Instant) -> (Option<Line>, bool) {
        let Some(t) = self.tracks.get_mut(&c.id) else {
            return (None, false);
        };
        if t.outcome.is_some() {
            return (None, false);
        }
        if t.phase != Some(c.phase) {
            return (None, true);
        }
        // Past the transfer the counter measures assembly, not bytes
        // arriving, and restarts from zero.
        let transferring = c.phase.is_running() && !c.phase.is_post_transfer();
        let due = t
            .last_progress
            .is_none_or(|at| now.duration_since(at) >= self.progress_every);
        if !transferring || !due || c.downloaded == t.last_downloaded {
            return (None, false);
        }
        t.last_progress = Some(now);
        t.last_downloaded = c.downloaded;
        let line = Line::Progress {
            id: c.id.to_string(),
            downloaded: c.downloaded,
            total: c.total,
            speed_bps: c.speed_bps.max(0.0) as u64,
            name: t.filename.clone(),
        };
        (Some(line), false)
    }

    /// Why the command should exit non-zero: the first download, in the
    /// order they were named, that ended without its file.
    pub fn failure(&self) -> Option<Failure> {
        self.order
            .iter()
            .find_map(|id| match &self.tracks[id].outcome {
                Some(Outcome::Ended(kind, why)) => Some(Failure::new(
                    *kind,
                    format!("download {} did not finish: {why}", short(&id.to_string())),
                )),
                _ => None,
            })
    }
}

/// Print the progress of `ids` until each has finished, stopped or
/// failed. The downloads belong to the daemon: a timeout or Ctrl-C ends
/// the waiting, never the download.
pub async fn follow(
    client: &Arc<Client>,
    ids: Vec<JobId>,
    already: HashSet<JobId>,
    timeout: Option<Duration>,
    out: Out,
) -> Result<(), Failure> {
    if ids.is_empty() {
        return Ok(());
    }
    // Events on a connection of their own. Sharing one with requests
    // would stall a reply behind a full event queue, and nothing here
    // reads events while it waits on a reply.
    let sub = Client::connect()
        .await
        .map_err(|e| Failure::daemon(format!("cannot reach oxdm: {e}")))?;
    sub.subscribe(SubFilter::All)
        .await
        .map_err(Failure::daemon)?;
    let mut events = sub
        .take_events()
        .await
        .ok_or_else(|| Failure::daemon("no event stream from oxdm"))?;

    let every = if out.is_json() {
        Duration::from_secs(1)
    } else {
        Duration::from_secs(5)
    };
    let mut tracker = Tracker::new(ids.clone(), already, every);
    // Read after subscribing, so a change in between is seen twice rather
    // than not at all.
    for id in &ids {
        refresh(client, &mut tracker, *id, out).await?;
    }

    let deadline = timeout.map(|t| tokio::time::Instant::now() + t);
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);
    while !tracker.is_done() {
        let expiry = async {
            match deadline {
                Some(d) => tokio::time::sleep_until(d).await,
                None => std::future::pending().await,
            }
        };
        let first = tokio::select! {
            ev = events.recv() => ev,
            _ = expiry => return Err(gave_up(Kind::Timeout, &tracker)),
            _ = &mut ctrl_c => return Err(gave_up(Kind::Interrupted, &tracker)),
        };
        let Some(first) = first else {
            return Err(Failure::daemon(
                "lost the connection to oxdm; it may have quit, pausing its downloads",
            ));
        };
        // Everything already queued behind it, so a burst of changes
        // costs one look per download rather than one per change.
        let mut stale: Vec<JobId> = Vec::new();
        let mut next = Some(first);
        while let Some(ev) = next {
            handle(ev, &mut tracker, &mut stale, out);
            next = events.try_recv().ok();
        }
        for id in stale {
            refresh(client, &mut tracker, id, out).await?;
        }
    }
    match tracker.failure() {
        Some(f) => Err(f),
        None => Ok(()),
    }
}

fn handle(ev: Event, tracker: &mut Tracker, stale: &mut Vec<JobId>, out: Out) {
    let mut mark = |id: JobId| {
        if !stale.contains(&id) {
            stale.push(id);
        }
    };
    match ev {
        Event::Counters(list) => {
            let now = Instant::now();
            for c in &list {
                let (line, moved) = tracker.counters(c, now);
                if let Some(line) = line {
                    out.line(&line);
                }
                if moved {
                    mark(c.id);
                }
            }
        }
        Event::JobCompleted { id, .. } | Event::JobFailed { id, .. } => mark(id),
        Event::JobsChanged | Event::ConflictChanged => {
            for id in tracker.pending() {
                mark(id);
            }
        }
        Event::RetryScheduled {
            id,
            ulid,
            attempt,
            max_attempts,
            delay_ms,
            server_requested,
        } if tracker.pending().contains(&id) => out.line(&Line::RetryScheduled {
            id: id.to_string(),
            part: ulid,
            attempt,
            max_attempts,
            delay_ms,
            server_requested,
        }),
        _ => {}
    }
}

async fn refresh(
    client: &Arc<Client>,
    tracker: &mut Tracker,
    id: JobId,
    out: Out,
) -> Result<(), Failure> {
    let entry = client.job_entry(id).await.map_err(Failure::daemon)?;
    let state = entry.as_ref().map(JobState::of);
    for line in tracker.observe(id, state.as_ref()) {
        out.line(&line);
    }
    Ok(())
}

fn gave_up(kind: Kind, tracker: &Tracker) -> Failure {
    let ids: Vec<String> = tracker
        .pending()
        .iter()
        .map(|id| short(&id.to_string()).to_owned())
        .collect();
    let why = match kind {
        Kind::Timeout => "timed out",
        _ => "stopped",
    };
    Failure::new(
        kind,
        format!(
            "{why} waiting for {}; the downloads carry on in oxdm (`oxdm wait` picks them up again)",
            ids.join(", ")
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(phase: Phase) -> JobState {
        JobState {
            phase,
            filename: None,
            path: None,
            error: None,
        }
    }

    fn counters(id: JobId, phase: Phase, downloaded: u64) -> JobCounters {
        JobCounters {
            id,
            phase,
            downloaded,
            total: Some(100),
            speed_bps: 10.0,
            is_resumable: 1,
            running: phase.is_running(),
            retries: 0,
            parts: Vec::new(),
        }
    }

    fn types(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| {
                serde_json::to_value(l).unwrap()["type"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn a_finished_download_ends_the_wait_with_its_path() {
        let id = JobId::new();
        let mut t = Tracker::new(vec![id], HashSet::new(), Duration::ZERO);
        let lines = t.observe(
            id,
            Some(&JobState {
                phase: Phase::Completed,
                filename: Some("a.zip".into()),
                path: Some("/dl/a.zip".into()),
                error: None,
            }),
        );
        assert_eq!(types(&lines), ["completed"]);
        let v = serde_json::to_value(&lines[0]).unwrap();
        assert_eq!(v["path"], "/dl/a.zip");
        assert_eq!(v["already_complete"], false);
        assert!(t.is_done());
        assert!(t.failure().is_none());
    }

    #[test]
    fn a_failure_exits_with_its_kind() {
        let id = JobId::new();
        let mut t = Tracker::new(vec![id], HashSet::new(), Duration::ZERO);
        let lines = t.observe(
            id,
            Some(&JobState {
                error: Some(JobError::Network("reset".into())),
                ..state(Phase::Failed)
            }),
        );
        let v = serde_json::to_value(&lines[0]).unwrap();
        assert_eq!(v["type"], "failed");
        assert_eq!(v["error"]["kind"], "network");
        assert_eq!(v["error"]["retryable"], true);
        assert_eq!(t.failure().unwrap().kind, Kind::Network);
    }

    #[test]
    fn a_parked_download_is_a_conflict_whatever_its_error() {
        let id = JobId::new();
        let mut t = Tracker::new(vec![id], HashSet::new(), Duration::ZERO);
        t.observe(id, Some(&state(Phase::Conflict)));
        assert_eq!(t.failure().unwrap().kind, Kind::Conflict);
    }

    /// Paused by a person (or a guard) is their decision; the wait ends
    /// rather than sitting there until a timeout.
    #[test]
    fn a_pause_or_a_removal_ends_the_wait_as_stopped() {
        let (a, b) = (JobId::new(), JobId::new());
        let mut t = Tracker::new(vec![a, b], HashSet::new(), Duration::ZERO);
        assert_eq!(
            types(&t.observe(a, Some(&state(Phase::Paused)))),
            ["stopped"]
        );
        assert!(!t.is_done());
        assert_eq!(types(&t.observe(b, None)), ["stopped"]);
        assert!(t.is_done());
        assert_eq!(t.failure().unwrap().kind, Kind::Stopped);
    }

    #[test]
    fn the_name_is_announced_once_while_the_download_runs() {
        let id = JobId::new();
        let mut t = Tracker::new(vec![id], HashSet::new(), Duration::ZERO);
        let named = JobState {
            filename: Some("a.zip".into()),
            ..state(Phase::Downloading)
        };
        assert_eq!(types(&t.observe(id, Some(&named))), ["filename", "phase"]);
        assert!(t.observe(id, Some(&named)).is_empty());
    }

    /// Waiting for a free slot is part of the journey, not the end of it.
    #[test]
    fn queued_and_running_keep_waiting_and_report_each_move_once() {
        let id = JobId::new();
        let mut t = Tracker::new(vec![id], HashSet::new(), Duration::ZERO);
        assert_eq!(
            types(&t.observe(id, Some(&state(Phase::Queued)))),
            ["phase"]
        );
        assert!(t.observe(id, Some(&state(Phase::Queued))).is_empty());
        assert_eq!(
            types(&t.observe(id, Some(&state(Phase::Downloading)))),
            ["phase"]
        );
        assert!(!t.is_done());
    }

    #[test]
    fn the_first_download_to_fail_names_the_exit_code() {
        let (a, b) = (JobId::new(), JobId::new());
        let mut t = Tracker::new(vec![a, b], HashSet::new(), Duration::ZERO);
        t.observe(
            b,
            Some(&JobState {
                error: Some(JobError::DiskFull("x".into())),
                ..state(Phase::Failed)
            }),
        );
        t.observe(a, Some(&state(Phase::Completed)));
        assert_eq!(t.failure().unwrap().kind, Kind::Io);
    }

    #[test]
    fn already_finished_downloads_say_so() {
        let id = JobId::new();
        let mut t = Tracker::new(vec![id], HashSet::from([id]), Duration::ZERO);
        let lines = t.observe(id, Some(&state(Phase::Completed)));
        let v = serde_json::to_value(&lines[0]).unwrap();
        assert_eq!(v["already_complete"], true);
    }

    #[test]
    fn progress_is_throttled_and_only_counts_the_transfer() {
        let id = JobId::new();
        let mut t = Tracker::new(vec![id], HashSet::new(), Duration::from_secs(1));
        t.observe(id, Some(&state(Phase::Downloading)));
        let now = Instant::now();

        let (line, moved) = t.counters(&counters(id, Phase::Downloading, 10), now);
        assert!(line.is_some() && !moved);
        // Too soon for another.
        let (line, _) = t.counters(
            &counters(id, Phase::Downloading, 20),
            now + Duration::from_millis(300),
        );
        assert!(line.is_none());
        let (line, _) = t.counters(
            &counters(id, Phase::Downloading, 30),
            now + Duration::from_secs(2),
        );
        assert!(line.is_some());
        // Nothing new arrived.
        let (line, _) = t.counters(
            &counters(id, Phase::Downloading, 30),
            now + Duration::from_secs(4),
        );
        assert!(line.is_none());

        // A new phase asks for a fresh look instead of printing.
        let (line, moved) = t.counters(
            &counters(id, Phase::Assembling, 5),
            now + Duration::from_secs(6),
        );
        assert!(line.is_none() && moved);
        t.observe(id, Some(&state(Phase::Assembling)));
        let (line, moved) = t.counters(
            &counters(id, Phase::Assembling, 50),
            now + Duration::from_secs(8),
        );
        assert!(
            line.is_none() && !moved,
            "assembly is not download progress"
        );
    }

    #[test]
    fn nothing_is_said_about_downloads_nobody_asked_about() {
        let mut t = Tracker::new(vec![JobId::new()], HashSet::new(), Duration::ZERO);
        let other = JobId::new();
        assert!(t.observe(other, Some(&state(Phase::Completed))).is_empty());
        assert!(matches!(
            t.counters(&counters(other, Phase::Downloading, 1), Instant::now()),
            (None, false)
        ));
    }
}

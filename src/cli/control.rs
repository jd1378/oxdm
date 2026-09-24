//! Pause, resume, restart, remove: acting on downloads already in the
//! list. Each id is handled on its own; one refusal does not stop the
//! rest, and the first one decides the exit code.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use super::args::{JobsArgs, RemoveArgs, RestartArgs, ResumeArgs, WaitOpts};
use super::daemon::lost;
use super::failure::{Failure, Kind, describe};
use super::output::Out;
use super::select;
use super::view::{Action, ErrorView, Line, State, short};
use super::wait;
use crate::data::RemoveOpts;
use crate::domain::{Job, JobError, JobId, Phase};
use crate::ipc_local::Client;
use crate::ipc_local::protocol::SnapshotData;

/// Start or resume a download as automation: its failures do not raise
/// windows, and it waits its turn behind the concurrent-download cap.
pub async fn run_job(client: &Client, id: JobId, fresh: bool) -> Result<Action, Failure> {
    match client.run(id, false, fresh).await.map_err(lost)? {
        Ok(()) => Ok(Action::Started),
        Err(JobError::Deferred) => Ok(Action::Queued),
        Err(e) => Err(Failure::from_job_error(&e)),
    }
}

/// A download stopped on a question only a person can answer. Resuming
/// it would ask the same question again, with nobody there to answer.
pub fn needs_decision(job: &Job) -> Option<Failure> {
    if job.status.phase != Phase::Conflict && !job.integrity_failed() {
        return None;
    }
    // A check of the saved file records its verdict on the row, not as
    // the run's error.
    let failed_row = || {
        job.checksums
            .iter()
            .find(|c| c.status == crate::domain::CsStatus::Mismatch)
            .map(|c| {
                describe(&JobError::ChecksumMismatch {
                    expected: c.hash.clone(),
                    actual: c.expected.clone().unwrap_or_default(),
                })
            })
    };
    let cause = job
        .status
        .error
        .as_ref()
        .map(describe)
        .or_else(failed_row)
        .unwrap_or_else(|| "it stopped on a conflict".to_owned());
    Some(Failure::new(
        Kind::Conflict,
        format!(
            "{cause}; `oxdm restart {}` downloads it again from the start",
            short(&job.id.to_string())
        ),
    ))
}

pub async fn snapshot(client: &Client) -> Result<SnapshotData, Failure> {
    client.snapshot().await.map_err(lost)
}

pub fn job(snap: &SnapshotData, id: JobId) -> Option<&Job> {
    snap.jobs.iter().find(|j| j.id == id)
}

/// Collects per-item refusals: printed as they happen, the first one
/// kept for the exit code.
#[derive(Default)]
pub struct Refusals(Option<Failure>);

impl Refusals {
    pub fn add(&mut self, out: Out, id: Option<JobId>, url: Option<&str>, f: Failure) {
        out.line(&Line::Rejected {
            id: id.map(|i| i.to_string()),
            url: url.map(str::to_owned),
            error: ErrorView::from_failure(f.kind, &f.message),
        });
        self.0.get_or_insert(f);
    }

    /// The command's result: the first refusal, else what followed.
    pub fn finish(self, then: Result<(), Failure>) -> Result<(), Failure> {
        match self.0 {
            Some(f) => Err(f),
            None => then,
        }
    }
}

/// Wait on what was acted on, if asked to.
pub async fn maybe_wait(
    client: &Arc<Client>,
    opts: &WaitOpts,
    ids: Vec<JobId>,
    already: HashSet<JobId>,
    out: Out,
) -> Result<(), Failure> {
    if !opts.wait {
        return Ok(());
    }
    wait::follow(client, ids, already, opts.timeout, out).await
}

pub async fn pause(client: &Arc<Client>, args: JobsArgs, out: Out) -> Result<(), Failure> {
    let snap = snapshot(client).await?;
    let ids = select::resolve_all(&snap.jobs, &args.jobs)?;
    let mut refusals = Refusals::default();
    for id in ids {
        let Some(job) = job(&snap, id) else { continue };
        let phase = job.status.phase;
        // Only what is running or about to run has anything to pause;
        // pausing a finished download would unfinish it.
        if !(phase.is_running() || phase == Phase::Queued) {
            out.line(&Line::Paused {
                id: id.to_string(),
                state: State::of(phase),
            });
            continue;
        }
        match client.pause(id).await {
            Ok(()) => out.line(&Line::Paused {
                id: id.to_string(),
                state: State::Paused,
            }),
            Err(e) => refusals.add(out, Some(id), None, Failure::new(Kind::Other, e)),
        }
    }
    refusals.finish(Ok(()))
}

pub async fn resume(client: &Arc<Client>, args: ResumeArgs, out: Out) -> Result<(), Failure> {
    let snap = snapshot(client).await?;
    let ids = if args.failed {
        let queue = match &args.queue {
            Some(name) => Some(super::query::queue_named(&snap, name)?.id),
            None => None,
        };
        snap.jobs
            .iter()
            .filter(|j| j.status.phase == Phase::Failed)
            .filter(|j| queue.is_none_or(|q| j.queue_id == q))
            .map(|j| j.id)
            .collect()
    } else {
        select::resolve_all(&snap.jobs, &args.jobs)?
    };
    let mut refusals = Refusals::default();
    let mut followed = Vec::new();
    let mut already = HashSet::new();
    for id in ids {
        let Some(job) = job(&snap, id) else { continue };
        let phase = job.status.phase;
        let result = if let Some(f) = needs_decision(job) {
            Err(f)
        } else if phase == Phase::Completed {
            already.insert(id);
            Ok(Action::Complete)
        } else if phase.is_running() {
            Ok(Action::Running)
        } else {
            run_job(client, id, false).await
        };
        match result {
            Ok(state) => {
                out.line(&Line::Resumed {
                    id: id.to_string(),
                    state,
                });
                followed.push(id);
            }
            Err(f) => refusals.add(out, Some(id), None, f),
        }
    }
    let waited = maybe_wait(client, &args.wait, followed, already, out).await;
    refusals.finish(waited)
}

pub async fn restart(client: &Arc<Client>, args: RestartArgs, out: Out) -> Result<(), Failure> {
    let snap = snapshot(client).await?;
    let ids = select::resolve_all(&snap.jobs, &args.jobs)?;
    let mut refusals = Refusals::default();
    let mut followed = Vec::new();
    for id in ids {
        let Some(job) = job(&snap, id) else { continue };
        if args.delete_file
            && let Some(path) = saved_file(job)
            && let Err(e) = to_trash(path).await
        {
            refusals.add(out, Some(id), None, e);
            continue;
        }
        match run_job(client, id, true).await {
            Ok(state) => {
                out.line(&Line::Restarted {
                    id: id.to_string(),
                    state,
                });
                followed.push(id);
            }
            Err(f) => refusals.add(out, Some(id), None, f),
        }
    }
    let waited = maybe_wait(client, &args.wait, followed, HashSet::new(), out).await;
    refusals.finish(waited)
}

pub async fn remove(client: &Arc<Client>, args: RemoveArgs, out: Out) -> Result<(), Failure> {
    let snap = snapshot(client).await?;
    let ids = select::resolve_all(&snap.jobs, &args.jobs)?;
    let mut refusals = Refusals::default();
    for id in ids {
        let Some(job) = job(&snap, id) else { continue };
        let completed = job.status.phase == Phase::Completed;
        // The daemon will not remove a download mid-transfer; asking to
        // remove it is asking to stop it first.
        if let Err(f) = stop(client, id).await {
            refusals.add(out, Some(id), None, f);
            continue;
        }
        // Trashed here, as the main window does, so a removal the user
        // regrets can still be undone from the desktop's trash.
        let mut warning = None;
        if args.delete_file
            && let Some(path) = saved_file(job)
            && let Err(e) = to_trash(path).await
        {
            warning = Some(e.message);
        }
        let opts = RemoveOpts {
            purge_partial: !completed,
            delete_final_file: false,
        };
        match client.remove(id, opts).await {
            Ok(w) => out.line(&Line::Removed {
                id: id.to_string(),
                warning: warning.or(w),
            }),
            Err(e) => refusals.add(out, Some(id), None, Failure::new(Kind::Other, e)),
        }
    }
    refusals.finish(Ok(()))
}

/// Pause `id` if it is running and wait for its runner to let go.
async fn stop(client: &Client, id: JobId) -> Result<(), Failure> {
    const SETTLE: Duration = Duration::from_secs(10);
    if !is_running(client, id).await? {
        return Ok(());
    }
    client
        .pause(id)
        .await
        .map_err(|e| Failure::new(Kind::Other, e))?;
    let start = std::time::Instant::now();
    while is_running(client, id).await? {
        if start.elapsed() > SETTLE {
            return Err(Failure::new(
                Kind::Other,
                "the download did not stop in time; try again",
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}

async fn is_running(client: &Client, id: JobId) -> Result<bool, Failure> {
    client
        .job_entry(id)
        .await
        .map(|e| e.is_some_and(|e| e.counters.running))
        .map_err(lost)
}

/// The file a finished (or failed-verification) download left, if it
/// is still there.
pub fn saved_file(job: &Job) -> Option<PathBuf> {
    job.status.final_path.clone().filter(|p| p.is_file())
}

async fn to_trash(path: PathBuf) -> Result<(), Failure> {
    let shown = path.display().to_string();
    tokio::task::spawn_blocking(move || trash::delete(&path))
        .await
        .map_err(|e| Failure::new(Kind::Io, format!("{shown}: {e}")))?
        .map_err(|e| {
            Failure::new(
                Kind::Io,
                format!("could not move {shown} to the trash: {e}"),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_conflict_or_a_bad_file_needs_a_decision() {
        let mut j = crate::cli::fixtures::job();
        j.status.phase = Phase::Failed;
        j.status.error = Some(JobError::Network("reset".into()));
        assert!(needs_decision(&j).is_none(), "a network failure resumes");

        j.status.phase = Phase::Conflict;
        j.status.error = Some(JobError::ConflictPending(Box::new(JobError::FileChanged(
            "size".into(),
        ))));
        let f = needs_decision(&j).unwrap();
        assert_eq!(f.kind, Kind::Conflict);
        assert!(
            f.message.contains("the file on the server changed"),
            "{}",
            f.message
        );
        assert!(f.message.contains("oxdm restart"), "{}", f.message);

        j.status.phase = Phase::Failed;
        j.status.error = Some(JobError::ChecksumMismatch {
            expected: "a".into(),
            actual: "b".into(),
        });
        assert!(
            needs_decision(&j).is_some(),
            "every byte is there and wrong"
        );

        // Condemned by a later check of the saved file: the row says why.
        j.status.phase = Phase::Completed;
        j.status.error = None;
        j.status.final_path = Some("/dl/a.zip".into());
        j.checksums.push(crate::domain::Checksum {
            algo: crate::domain::Algo::Sha1,
            hash: "0".repeat(40),
            source: crate::domain::CsSource::User,
            status: crate::domain::CsStatus::Mismatch,
            expected: Some("2a49".into()),
        });
        let f = needs_decision(&j).unwrap();
        assert!(f.message.contains("expected 0000"), "{}", f.message);
        assert!(f.message.contains("got 2a49"), "{}", f.message);
    }
}

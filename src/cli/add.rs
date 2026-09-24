//! `oxdm add`: hand URLs to the daemon.
//!
//! Re-running the same command is safe. A URL already in the list is
//! reported rather than added again, and resumed if it had stopped, so
//! "try again" after an interruption is the same command, not a new one.

use std::collections::HashSet;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use super::args::AddArgs;
use super::control::{Refusals, maybe_wait, needs_decision, run_job, saved_file, snapshot};
use super::daemon::lost;
use super::failure::Failure;
use super::input::{self, Dest};
use super::output::Out;
use super::query::queue_named;
use super::view::{Action, Line};
use crate::domain::{Creds, Job, JobId, Phase, Queue, QueueId};
use crate::ipc_local::Client;
use crate::ipc_local::protocol::AddJobReq;

/// Text behind `-i FILE` or `@FILE`; `-` is stdin.
fn read_source(path: &str) -> std::io::Result<String> {
    if path == "-" {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        Ok(s)
    } else {
        std::fs::read_to_string(path)
    }
}

/// An `add` whose input has been read and checked, ready for the
/// daemon.
pub struct Plan {
    args: AddArgs,
    urls: Vec<url::Url>,
    extras: input::Extras,
    checksums: Vec<crate::domain::Checksum>,
    referrer: Option<url::Url>,
    output: Option<std::path::PathBuf>,
}

/// Everything that can be wrong with the command, found before the
/// daemon is asked to do anything (or started to be asked).
pub fn plan(args: AddArgs) -> Result<Plan, Failure> {
    let urls = input::collect_urls(&args.urls, args.input.as_deref(), &read_source)?;
    if urls.is_empty() {
        return Err(Failure::usage("no URLs to add"));
    }
    let extras = input::parse_headers(&args.headers, &read_source)?;
    let checksums = args
        .checksum
        .iter()
        .map(|c| input::parse_checksum(c))
        .collect::<Result<Vec<_>, _>>()?;
    let referrer = args.referrer.as_deref().map(input::parse_url).transpose()?;
    let output = args
        .output
        .as_deref()
        .map(std::path::absolute)
        .transpose()
        .map_err(|e| Failure::usage(format!("--output: {e}")))?;
    Ok(Plan {
        args,
        urls,
        extras,
        checksums,
        referrer,
        output,
    })
}

pub async fn add(client: &Arc<Client>, plan: Plan, out: Out) -> Result<(), Failure> {
    let Plan {
        args,
        urls,
        extras,
        checksums,
        referrer,
        output,
    } = plan;
    let snap = snapshot(client).await?;
    // An explicit queue is checked before anything is set up on the
    // agent's behalf.
    let explicit = match &args.queue {
        Some(name) => Some(queue_named(&snap, name)?),
        None => None,
    };
    let route = client.agent_route().await.map_err(lost)?;
    let (queue_id, queue_name) = match explicit {
        Some(q) => (Some(q.id), q.name.clone()),
        None => (route.queue, queue_label(&snap, route.queue)),
    };
    let dest = input::destination(
        output.as_deref(),
        urls.len() > 1,
        &route.dir,
        &snap.settings,
        &Path::is_dir,
    )?;
    let fresh = Fresh {
        category: route.category,
        queue_id,
        queue_name: &queue_name,
        dest: &dest,
        extras: &extras,
        checksums: &checksums,
        referrer: referrer.as_ref(),
        connections: args.connections,
        no_start: args.no_start,
    };
    let mut refusals = Refusals::default();
    let mut followed = Vec::new();
    let mut already = HashSet::new();
    for url in urls {
        let existing = (!args.new)
            .then(|| find_existing(&snap.jobs, &url, &dest))
            .flatten();
        let outcome = match existing {
            Some(job) => reuse(client, &snap, job, &url, args.no_start).await,
            None => create(client, &fresh, &url).await,
        };
        match outcome {
            Ok((id, line)) => {
                if matches!(
                    line,
                    Line::Added {
                        state: Action::Complete,
                        ..
                    }
                ) {
                    already.insert(id);
                }
                out.line(&line);
                followed.push(id);
                if args.show
                    && let Err(e) = client.open_download_window(id).await
                {
                    tracing::warn!(error = %e, "could not open the download window");
                }
            }
            Err((id, f)) => refusals.add(out, id, Some(url.as_str()), f),
        }
    }
    let waited = maybe_wait(client, &args.wait, followed, already, out).await;
    refusals.finish(waited)
}

/// What every new download in one `add` shares.
struct Fresh<'a> {
    category: Option<crate::domain::Category>,
    queue_id: Option<crate::domain::QueueId>,
    queue_name: &'a str,
    dest: &'a Dest,
    extras: &'a input::Extras,
    checksums: &'a [crate::domain::Checksum],
    referrer: Option<&'a url::Url>,
    connections: Option<u64>,
    no_start: bool,
}

/// A refusal, with the download it concerns when one exists.
type Refused = (Option<JobId>, Failure);

async fn create(client: &Client, f: &Fresh<'_>, url: &url::Url) -> Result<(JobId, Line), Refused> {
    let req = AddJobReq {
        url: url.clone(),
        queue: f.queue_id,
        save_dir: f.dest.dir.clone(),
        filename: f.dest.filename.clone(),
        referrer: f.referrer.cloned(),
        headers: f.extras.headers.clone(),
        max_connections: f.connections,
        creds: Creds {
            auth: f.extras.auth.clone(),
            ..Creds::default()
        },
        cookies: f.extras.cookies.clone(),
        // `Agent`, or (once the user deleted that category) left to the
        // daemon to file by the name the server gives.
        category: f.category,
        size: None,
        checksums: f.checksums.to_vec(),
        run_follows: !f.no_start,
    };
    let id = client.add_job(req).await.map_err(|e| (None, lost(e)))?;
    let state = if f.no_start {
        Action::NotStarted
    } else {
        run_job(client, id, false)
            .await
            .map_err(|e| (Some(id), e))?
    };
    let line = Line::Added {
        id: id.to_string(),
        url: url.to_string(),
        queue: Some(f.queue_name.to_owned()),
        save_dir: f.dest.dir.to_string_lossy().into_owned(),
        filename: f.dest.filename.clone(),
        existing: false,
        state,
    };
    Ok((id, line))
}

async fn reuse(
    client: &Client,
    snap: &crate::ipc_local::protocol::SnapshotData,
    job: &Job,
    url: &url::Url,
    no_start: bool,
) -> Result<(JobId, Line), Refused> {
    let state = revive(client, job, no_start)
        .await
        .map_err(|e| (Some(job.id), e))?;
    let line = Line::Added {
        id: job.id.to_string(),
        url: url.to_string(),
        queue: snap
            .queues
            .iter()
            .find(|q| q.id == job.queue_id)
            .map(|q| q.name.clone()),
        save_dir: job.save_dir.to_string_lossy().into_owned(),
        filename: job.filename.clone(),
        existing: true,
        state,
    };
    Ok((job.id, line))
}

/// A queue's name for the output. The Agent queue may be younger than
/// the snapshot: `agent_route` makes it on first use.
fn queue_label(snap: &crate::ipc_local::protocol::SnapshotData, id: Option<QueueId>) -> String {
    let named = |id: QueueId| {
        snap.queues
            .iter()
            .find(|q| q.id == id)
            .map(|q| q.name.clone())
    };
    match id {
        Some(id) => named(id).unwrap_or_else(|| Queue::AGENT_NAME.to_owned()),
        None => snap
            .queues
            .iter()
            .find(|q| q.builtin)
            .map(|q| q.name.clone())
            .unwrap_or_else(|| Queue::MAIN_NAME.to_owned()),
    }
}

/// The download this URL already is, if the list has one: same link,
/// and, when `-o` named a place, the same place. The newest wins.
fn find_existing<'a>(jobs: &'a [Job], url: &url::Url, dest: &Dest) -> Option<&'a Job> {
    jobs.iter()
        .filter(|j| j.url == *url)
        .filter(|j| {
            !dest.explicit
                || (j.save_dir == dest.dir
                    && dest
                        .filename
                        .as_ref()
                        .is_none_or(|f| j.filename.as_ref() == Some(f)))
        })
        .max_by_key(|j| j.created_at)
}

/// Bring an existing download back to life, as far as it needs.
async fn revive(client: &Client, job: &Job, no_start: bool) -> Result<Action, Failure> {
    let phase = job.status.phase;
    if phase == Phase::Completed {
        if saved_file(job).is_some() {
            return Ok(Action::Complete);
        }
        // Finished once, but the file has gone: this is a fresh fetch.
        if no_start {
            return Ok(Action::NotStarted);
        }
        return run_job(client, job.id, true).await;
    }
    if let Some(f) = needs_decision(job) {
        return Err(f);
    }
    if phase.is_running() {
        return Ok(Action::Running);
    }
    if no_start {
        return Ok(Action::NotStarted);
    }
    run_job(client, job.id, false).await
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn job_at(url: &str, dir: &str, name: Option<&str>, age_secs: i64) -> Job {
        let mut j = crate::cli::fixtures::job();
        j.url = url::Url::parse(url).unwrap();
        j.save_dir = PathBuf::from(dir);
        j.filename = name.map(str::to_owned);
        j.created_at = chrono::Utc::now() - chrono::Duration::seconds(age_secs);
        j
    }

    fn dest(dir: &str, name: Option<&str>, explicit: bool) -> Dest {
        Dest {
            dir: PathBuf::from(dir),
            filename: name.map(str::to_owned),
            explicit,
        }
    }

    #[test]
    fn the_same_link_is_the_same_download_and_the_newest_wins() {
        let url = url::Url::parse("https://example.com/a.iso").unwrap();
        let jobs = [
            job_at("https://example.com/a.iso", "/dl/Programs", None, 100),
            job_at("https://example.com/a.iso", "/dl/Other", None, 10),
            job_at("https://example.com/b.iso", "/dl/Other", None, 1),
        ];
        let hit = find_existing(&jobs, &url, &dest("/dl", None, false)).unwrap();
        assert_eq!(hit.id, jobs[1].id);
    }

    /// A different place asked for by name is a different download.
    #[test]
    fn a_named_destination_must_match_too() {
        let url = url::Url::parse("https://example.com/a.iso").unwrap();
        let jobs = [job_at("https://example.com/a.iso", "/dl", Some("a.iso"), 5)];
        assert!(find_existing(&jobs, &url, &dest("/elsewhere", None, true)).is_none());
        assert!(find_existing(&jobs, &url, &dest("/dl", Some("b.iso"), true)).is_none());
        assert!(find_existing(&jobs, &url, &dest("/dl", Some("a.iso"), true)).is_some());
        assert!(find_existing(&jobs, &url, &dest("/dl", None, true)).is_some());
    }
}

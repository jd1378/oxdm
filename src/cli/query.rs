//! Read-only commands: list, status, queues, probe.

use std::sync::Arc;

use super::args::{JobsArgs, ListArgs, ProbeArgs, StateArg};
use super::control::snapshot;
use super::daemon::lost;
use super::failure::Failure;
use super::input::parse_url;
use super::output::Out;
use super::select;
use super::view::{DigestView, DownloadView, Line, QueueView, State, algo_slug};
use crate::domain::{Category, Job, Queue};
use crate::ipc_local::Client;
use crate::ipc_local::protocol::SnapshotData;

/// The queue a name refers to, or a usage error naming the ones there are.
pub fn queue_named<'a>(snap: &'a SnapshotData, name: &str) -> Result<&'a Queue, Failure> {
    snap.queues
        .iter()
        .find(|q| q.is_named(name))
        .ok_or_else(|| {
            let names: Vec<&str> = snap.queues.iter().map(|q| q.name.as_str()).collect();
            Failure::usage(format!(
                "no queue named {name:?} (queues: {})",
                names.join(", ")
            ))
        })
}

/// A category by the name `list` prints for it.
fn category_named(name: &str) -> Result<Category, Failure> {
    Category::from_slug(&name.trim().to_lowercase()).ok_or_else(|| {
        let names: Vec<&str> = Category::ALL_ASSIGNABLE.iter().map(|c| c.slug()).collect();
        Failure::usage(format!(
            "no category named {name:?} (categories: {})",
            names.join(", ")
        ))
    })
}

fn view(snap: &SnapshotData, job: &Job) -> DownloadView {
    let counters = snap.counters.iter().find(|c| c.id == job.id);
    DownloadView::new(job, counters, &snap.queues)
}

fn wanted_state(s: StateArg) -> State {
    match s {
        StateArg::Queued => State::Queued,
        StateArg::Active => State::Active,
        StateArg::Paused => State::Paused,
        StateArg::Completed => State::Completed,
        StateArg::Failed => State::Failed,
        StateArg::Conflict => State::Conflict,
        StateArg::Cancelled => State::Cancelled,
    }
}

/// Case-insensitive: the URL or the file name contains `text`, or the
/// id starts with it.
fn matches_text(job: &Job, text: &str) -> bool {
    let text = text.to_lowercase();
    job.id.to_string().starts_with(&text)
        || job.url.as_str().to_lowercase().contains(&text)
        || job
            .filename
            .as_deref()
            .is_some_and(|n| n.to_lowercase().contains(&text))
}

pub async fn list(client: &Arc<Client>, args: ListArgs, out: Out) -> Result<(), Failure> {
    let snap = snapshot(client).await?;
    let queue = match &args.queue {
        Some(name) => Some(queue_named(&snap, name)?.id),
        None => None,
    };
    let category = match &args.category {
        Some(name) => Some(category_named(name)?),
        None => None,
    };
    let states: Vec<State> = args.state.iter().copied().map(wanted_state).collect();
    let downloads: Vec<DownloadView> = snap
        .jobs
        .iter()
        .filter(|j| queue.is_none_or(|q| j.queue_id == q))
        .filter(|j| category.is_none_or(|c| j.category == c))
        .filter(|j| states.is_empty() || states.contains(&State::of(j.status.phase)))
        .filter(|j| args.filter.as_deref().is_none_or(|t| matches_text(j, t)))
        .map(|j| view(&snap, j))
        .collect();
    out.line(&Line::Downloads {
        count: downloads.len(),
        downloads,
    });
    Ok(())
}

pub async fn status(client: &Arc<Client>, args: JobsArgs, out: Out) -> Result<(), Failure> {
    let snap = snapshot(client).await?;
    let ids = select::resolve_all(&snap.jobs, &args.jobs)?;
    let downloads: Vec<DownloadView> = ids
        .iter()
        .filter_map(|id| snap.jobs.iter().find(|j| j.id == *id))
        .map(|j| view(&snap, j))
        .collect();
    out.line(&Line::Downloads {
        count: downloads.len(),
        downloads,
    });
    Ok(())
}

pub async fn queues(client: &Arc<Client>, out: Out) -> Result<(), Failure> {
    let snap = snapshot(client).await?;
    let queues = snap
        .queues
        .iter()
        .map(|q| QueueView {
            id: q.id.to_string(),
            name: q.name.clone(),
            builtin: q.builtin,
            running: snap.active_queues.contains(&q.id),
            downloads: snap.jobs.iter().filter(|j| j.queue_id == q.id).count(),
        })
        .collect();
    out.line(&Line::Queues { queues });
    Ok(())
}

pub async fn probe(client: &Arc<Client>, args: ProbeArgs, out: Out) -> Result<(), Failure> {
    let url = parse_url(&args.url)?;
    let p = client
        .probe(url.clone())
        .await
        .map_err(lost)?
        .map_err(|e| Failure::from_job_error(&e))?;
    out.line(&Line::Probe {
        url: url.to_string(),
        filename: p.filename,
        size: p.size,
        resumable: p.is_resumable,
        etag: p.etag,
        last_modified: p.last_modified,
        requires_auth: p.requires_auth,
        checksums: p
            .checksums
            .iter()
            .map(|c| DigestView {
                algorithm: algo_slug(c.algo),
                digest: c.hash.clone(),
            })
            .collect(),
    });
    Ok(())
}

pub async fn restore_agent(client: &Arc<Client>, out: Out) -> Result<(), Failure> {
    let route = client.restore_agent().await.map_err(lost)?;
    let snap = snapshot(client).await?;
    let queue = route
        .queue
        .and_then(|id| snap.queues.iter().find(|q| q.id == id))
        .or_else(|| snap.queues.iter().find(|q| q.builtin))
        .map(|q| q.name.clone())
        .unwrap_or_default();
    out.line(&Line::Agent {
        category: route.category.map(Category::slug),
        save_dir: route.dir.to_string_lossy().into_owned(),
        queue,
    });
    Ok(())
}

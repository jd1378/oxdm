//! Drop entries whose data is no longer there
//! (`Settings::forget_moved_files`).
//!
//! Two ways for that to happen. A finished download's file is moved,
//! renamed or deleted, and the row points at nothing. Or an unfinished
//! one's work folder — its `metadata.pb` and every `.part` — is deleted
//! out from under it, and the row is a progress bar over bytes that no
//! longer exist.
//!
//! The OS reports the move: one watch per folder that holds a completed
//! download, and a file leaving it wakes this task within milliseconds.
//! A sweep — one `stat` per completed job — then decides what actually
//! went, because an event says a folder changed, not which row is now
//! wrong.
//!
//! The sweep also runs at startup, which is what catches everything
//! moved while oxdm was not running.
//!
//! Nothing is on a timer: an idle oxdm with a watch on a folder nobody
//! touches costs nothing, and a periodic stat of every completed
//! download would spin up disks and wake network shares for an answer
//! that is almost always "still there". The cost is that a folder whose
//! OS reports nothing — network shares, some FUSE mounts, a drive
//! unplugged and plugged back in — is only reconciled at the next
//! startup or the next event elsewhere.
//!
//! The check cannot distinguish a move from a rename or a delete. All
//! three mean the same thing to the list: the row points at nothing.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::{RecursiveMode, Watcher};

use crate::data::RemoveOpts;
use crate::data::state::AppState;
use crate::domain::Phase;

/// A move is a delete plus a create, and a file manager working in the
/// folder emits a burst. Settling first turns a burst into one sweep,
/// and gives a rename-in-place time to land before we call it gone.
const SETTLE: Duration = Duration::from_millis(750);

/// Start one watcher, and say what stopped it if one did not start.
///
/// Every backend can fail: inotify's two kernel limits, or a sandbox
/// with no filesystem notification at all. Without a watcher the
/// startup sweep is all there is, so the failure is recorded rather
/// than degraded into a setting that looks on and does nothing.
async fn start_watcher(
    state: &Arc<AppState>,
    tx: tokio::sync::mpsc::UnboundedSender<()>,
) -> Option<notify::RecommendedWatcher> {
    // Test scaffolding: pretend the kernel refused, so the warning and
    // its repair path can be exercised without filling the machine's
    // real limit tables. `instances` or `watches`, either with a
    // `:once` suffix that refuses the first watcher only — which is
    // what raising the limit looks like from in here.
    if let Ok(spec) = std::env::var("OXDM_SIMULATE_WATCH_LIMIT") {
        static SPENT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        let (kind, once) = match spec.split_once(':') {
            Some((k, "once")) => (k, true),
            _ => (spec.as_str(), false),
        };
        let kind = match kind {
            "watches" => crate::domain::WatchLimitKind::Watches,
            _ => crate::domain::WatchLimitKind::Instances,
        };
        if !(once && SPENT.swap(true, std::sync::atomic::Ordering::Relaxed)) {
            tracing::warn!(
                ?kind,
                "OXDM_SIMULATE_WATCH_LIMIT: pretending the kernel refused"
            );
            state.set_watch_limit(Some(watch_limit_now(kind))).await;
            return None;
        }
    }
    match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res
            && interesting(&ev.kind)
        {
            let _ = tx.send(());
        }
    }) {
        Ok(w) => {
            state.set_watch_limit(None).await;
            Some(w)
        }
        Err(e) => {
            tracing::warn!(error = %e, "no filesystem watcher: moved files will only be noticed at startup");
            state.set_watch_limit(limit_from(&e)).await;
            None
        }
    }
}

/// The limit behind a `notify` error, if it is one. `notify` wraps the
/// OS error, so the io error underneath is what carries the errno.
fn limit_from(e: &notify::Error) -> Option<crate::domain::WatchLimit> {
    let io = match &e.kind {
        notify::ErrorKind::Io(io) => io,
        _ => return None,
    };
    crate::domain::watch_limit::classify(io).map(watch_limit_now)
}

/// A limit paired with the value in force right now.
fn watch_limit_now(kind: crate::domain::WatchLimitKind) -> crate::domain::WatchLimit {
    crate::domain::WatchLimit::new(kind, crate::domain::watch_limit::read_limit(kind))
}

pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        let (tx, mut fs_rx) = tokio::sync::mpsc::unbounded_channel();
        // The watcher calls back on its own thread; the channel is the
        // only thing it touches.
        let mut watcher = start_watcher(&state, tx.clone()).await;
        let mut watched: HashSet<PathBuf> = HashSet::new();
        let mut events = state.subscribe();

        loop {
            if state.is_exiting() {
                return;
            }
            if state.settings().await.forget_moved_files {
                sweep(&state).await;
                if let Some(limit) = resync(&state, watcher.as_mut(), &mut watched).await {
                    // A folder that could not be watched because the
                    // watch table is full: the same news as a watcher
                    // that never started, one folder down.
                    state.set_watch_limit(Some(limit)).await;
                }
            } else if !watched.is_empty() {
                // Turned off: drop the watches rather than keep waking
                // up to discard their events.
                unwatch_all(watcher.as_mut(), &mut watched);
            }

            tokio::select! {
                _ = fs_rx.recv() => {
                    // Drain the rest of the burst, then let it settle.
                    while fs_rx.try_recv().is_ok() {}
                    tokio::time::sleep(SETTLE).await;
                }
                ev = events.recv() => {
                    use tokio::sync::broadcast::error::RecvError;
                    match ev {
                        // A finished download adds a folder to watch; a
                        // removed one may take the last row in one.
                        Ok(crate::data::DomainEvent::JobCompleted { .. })
                        | Ok(crate::data::DomainEvent::JobRemoved { .. })
                        | Ok(crate::data::DomainEvent::SettingsChanged) => {}
                        // The user raised the limit. Try again now:
                        // the whole point of offering the fix is that
                        // it works without restarting the daemon.
                        Ok(crate::data::DomainEvent::FileWatchRetry) => {
                            if watcher.is_none() {
                                watcher = start_watcher(&state, tx.clone()).await;
                                watched.clear();
                            }
                        }
                        // Lagged means events were missed, which is
                        // exactly when a full sweep is worth running.
                        Err(RecvError::Lagged(_)) => {}
                        Err(RecvError::Closed) => return,
                        _ => continue,
                    }
                }
            }
        }
    });
}

/// Events worth a sweep. A file being created is not one: assembly
/// lands new files in watched folders all the time, and nothing about
/// that means an older download went anywhere.
fn interesting(kind: &notify::EventKind) -> bool {
    use notify::EventKind;
    matches!(kind, EventKind::Remove(_) | EventKind::Modify(_))
}

/// Watch exactly the folders that currently hold a completed download.
///
/// Returns the kernel limit that turned a folder away, when one did —
/// the watcher is alive but has no room left, which degrades the same
/// feature and has the same fix.
async fn resync(
    state: &Arc<AppState>,
    watcher: Option<&mut notify::RecommendedWatcher>,
    watched: &mut HashSet<PathBuf>,
) -> Option<crate::domain::WatchLimit> {
    let mut hit = None;
    let watcher = watcher?;
    // Every folder holding something a job depends on: the folders its
    // finished files are in, and the cache root whose children are the
    // work folders. One watch on the root covers every job in it.
    let mut wanted: HashSet<PathBuf> = completed_files(state)
        .await
        .into_iter()
        .filter_map(|(_, p)| p.parent().map(Path::to_path_buf))
        .collect();
    if !partial_work_dirs(state).await.is_empty() {
        wanted.insert(state.settings().await.work_dir);
    }

    for dir in watched.difference(&wanted).cloned().collect::<Vec<_>>() {
        let _ = watcher.unwatch(&dir);
        watched.remove(&dir);
    }
    for dir in wanted.difference(watched).cloned().collect::<Vec<_>>() {
        // Non-recursive: the file sits directly in this folder, and a
        // recursive watch on a downloads folder full of extracted
        // archives is a lot of kernel watches for nothing.
        match watcher.watch(&dir, RecursiveMode::NonRecursive) {
            Ok(()) => {
                watched.insert(dir);
            }
            // A folder that cannot be watched is usually a mount that
            // went away, which is the case this deliberately does not
            // act on — debug, not a warning. A full watch table is the
            // exception: that one the user can fix.
            Err(e) => {
                tracing::debug!(dir = %dir.display(), error = %e, "cannot watch folder");
                hit = hit.or_else(|| limit_from(&e));
            }
        }
    }
    hit
}

fn unwatch_all(watcher: Option<&mut notify::RecommendedWatcher>, watched: &mut HashSet<PathBuf>) {
    if let Some(w) = watcher {
        for dir in watched.iter() {
            let _ = w.unwatch(dir);
        }
    }
    watched.clear();
}

async fn completed_files(state: &Arc<AppState>) -> Vec<(crate::domain::JobId, PathBuf)> {
    state
        .list_jobs()
        .await
        .into_iter()
        .filter(|j| j.status.phase == Phase::Completed)
        .filter_map(|j| j.status.final_path.clone().map(|p| (j.id, p)))
        .collect()
}

/// Work folders that hold something worth losing, and the job they
/// belong to.
///
/// Only downloads that are stopped part-way. A running one is writing
/// into its folder as we look; a finished one has everything in the
/// saved file and does not care what is left in the cache; and one that
/// has never fetched a byte loses nothing to a folder that was never
/// created.
async fn partial_work_dirs(state: &Arc<AppState>) -> Vec<(crate::domain::JobId, PathBuf)> {
    let work_root = state.settings().await.work_dir;
    state
        .list_jobs()
        .await
        .into_iter()
        .filter(|j| {
            !j.status.phase.is_running()
                && j.status.phase != Phase::Completed
                && j.status.downloaded > 0
        })
        .map(|j| (j.id, crate::data::state::per_job_dir(&work_root, j.id)))
        .collect()
}

async fn sweep(state: &Arc<AppState>) {
    let mut gone = Vec::new();
    for (id, path) in completed_files(state).await {
        if is_gone(&path).await {
            gone.push((id, path));
        }
    }
    for (id, dir) in partial_work_dirs(state).await {
        if is_gone(&dir).await {
            gone.push((id, dir));
        }
    }

    for (id, path) in gone {
        tracing::info!(id = %id, path = %path.display(),
            "what this download had is no longer there — forgetting it");
        // The partial state goes with it: keeping a work dir for a job
        // that is no longer listed leaves bytes nobody can reach. The
        // file itself is never touched — it is not there to touch.
        let opts = RemoveOpts {
            purge_partial: true,
            delete_final_file: false,
        };
        if let Err(e) = state.remove(id, opts).await {
            tracing::warn!(id = %id, error = %e, "could not forget the download");
        }
    }
}

/// Whether the file at `path` has gone away, as opposed to being
/// temporarily unreachable.
async fn is_gone(path: &Path) -> bool {
    let here = tokio::fs::try_exists(path).await.unwrap_or(true);
    let parent = match path.parent() {
        Some(dir) => tokio::fs::try_exists(dir).await.unwrap_or(true),
        // A path with no parent is not something we can reason about.
        None => return false,
    };
    moved_away(here, parent)
}

/// An unmounted drive and a moved file look identical from the file's
/// own path: nothing is there. They are told apart by the folder — an
/// external disk takes its whole tree with it, so a missing file inside
/// a folder that is still there is the only case that means the user
/// moved something.
///
/// Erring towards keeping the row is deliberate. A row that outlives
/// its file is a stale entry the user can remove; a row deleted because
/// a NAS was asleep is history they cannot get back.
fn moved_away(file_exists: bool, parent_exists: bool) -> bool {
    !file_exists && parent_exists
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_missing_file_in_a_folder_that_is_still_there_counts() {
        assert!(moved_away(false, true));
        assert!(!moved_away(true, true));
        // The whole volume went away — the download is not lost, the
        // disk is elsewhere.
        assert!(!moved_away(false, false));
    }

    #[tokio::test]
    async fn a_file_that_exists_is_never_gone() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("saved.bin");
        std::fs::write(&f, b"x").unwrap();
        assert!(!is_gone(&f).await);

        std::fs::remove_file(&f).unwrap();
        assert!(is_gone(&f).await);
    }

    /// The folder going with it is the unmounted-volume case.
    #[tokio::test]
    async fn a_file_under_a_vanished_folder_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("volume");
        std::fs::create_dir(&sub).unwrap();
        let f = sub.join("saved.bin");
        std::fs::write(&f, b"x").unwrap();
        std::fs::remove_dir_all(&sub).unwrap();
        assert!(!is_gone(&f).await);
    }

    /// The watcher is what makes this immediate, so its wiring is worth
    /// a test of its own: a file leaving a watched folder has to reach
    /// the channel the task selects on.
    #[tokio::test]
    async fn a_moved_file_wakes_the_watcher() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("saved.bin");
        std::fs::write(&f, b"x").unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res
                && interesting(&ev.kind)
            {
                let _ = tx.send(());
            }
        })
        .unwrap();
        w.watch(dir.path(), RecursiveMode::NonRecursive).unwrap();

        std::fs::rename(&f, dir.path().join("filed-away.bin")).unwrap();
        let woke = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
        assert!(woke.is_ok(), "the move never reached the watcher");
    }
}

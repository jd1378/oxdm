//! Drop completed entries whose saved file is no longer where oxdm put
//! it (`Settings::forget_moved_files`).
//!
//! The OS reports the move: one watch per folder that holds a completed
//! download, and a file leaving it wakes this task within milliseconds.
//! A sweep — one `stat` per completed job — then decides what actually
//! went, because an event says a folder changed, not which row is now
//! wrong.
//!
//! Watches cannot be the whole story, so the sweep also runs:
//!
//! * at startup, for everything moved while oxdm was not running;
//! * on a slow tick, because network shares and some FUSE mounts never
//!   deliver events at all, and a watch on an unplugged drive dies with
//!   it.
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

/// Safety net for folders whose OS never reports anything.
const SWEEP_EVERY: Duration = Duration::from_secs(300);
/// A move is a delete plus a create, and a file manager working in the
/// folder emits a burst. Settling first turns a burst into one sweep,
/// and gives a rename-in-place time to land before we call it gone.
const SETTLE: Duration = Duration::from_millis(750);

pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        let (tx, mut fs_rx) = tokio::sync::mpsc::unbounded_channel();
        // The watcher calls back on its own thread; the channel is the
        // only thing it touches.
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res
                && interesting(&ev.kind)
            {
                let _ = tx.send(());
            }
        });
        let mut watcher = match watcher {
            Ok(w) => Some(w),
            Err(e) => {
                // Every backend can fail to start — inotify watch
                // limits, a sandbox with no FS notification at all.
                // The slow sweep still does the job.
                tracing::warn!(error = %e, "no filesystem watcher; falling back to periodic checks");
                None
            }
        };
        let mut watched: HashSet<PathBuf> = HashSet::new();
        let mut events = state.subscribe();
        let mut slow = tokio::time::interval(SWEEP_EVERY);

        loop {
            if state.is_exiting() {
                return;
            }
            if state.settings().await.forget_moved_files {
                sweep(&state).await;
                resync(&state, watcher.as_mut(), &mut watched).await;
            } else if !watched.is_empty() {
                // Turned off: drop the watches rather than keep waking
                // up to discard their events.
                unwatch_all(watcher.as_mut(), &mut watched);
            }

            tokio::select! {
                _ = slow.tick() => {}
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
async fn resync(
    state: &Arc<AppState>,
    watcher: Option<&mut notify::RecommendedWatcher>,
    watched: &mut HashSet<PathBuf>,
) {
    let Some(watcher) = watcher else {
        return;
    };
    let wanted: HashSet<PathBuf> = completed_files(state)
        .await
        .into_iter()
        .filter_map(|(_, p)| p.parent().map(Path::to_path_buf))
        .collect();

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
            // A folder that cannot be watched is not worth a warning
            // every tick — it is usually a mount that went away, and
            // the sweep covers it.
            Err(e) => tracing::debug!(dir = %dir.display(), error = %e, "cannot watch folder"),
        }
    }
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

async fn sweep(state: &Arc<AppState>) {
    let mut gone = Vec::new();
    for (id, path) in completed_files(state).await {
        if is_gone(&path).await {
            gone.push((id, path));
        }
    }

    for (id, path) in gone {
        tracing::info!(id = %id, path = %path.display(),
            "file is no longer at its saved path — forgetting the download");
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

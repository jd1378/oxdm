//! The kernel refused to watch the filesystem, and why.
//!
//! On Linux every app that watches files spends from two small, shared
//! pools: `fs.inotify.max_user_instances` (how many watchers one user
//! may hold — a browser takes one per renderer) and
//! `fs.inotify.max_user_watches` (how many paths, in total). When
//! either runs out, oxdm's watcher cannot start, and the only thing
//! that notices a finished download being moved or deleted is the
//! sweep at the next startup.
//!
//! That degradation is invisible: the setting still reads "on", the
//! list still looks right, and rows quietly stop keeping up with the
//! disk. So the failure is named, carried to the UI, and — because the
//! fix is one sysctl the user cannot be expected to know — offered.
//!
//! Nothing here elevates anything. This module decides *what* is
//! wrong and *what would fix it*; asking is the dialog's job and doing
//! it is the user's.

use serde::{Deserialize, Serialize};

/// Which pool ran out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchLimitKind {
    /// `inotify_init` refused: this user holds as many watchers as the
    /// kernel allows. Nothing oxdm does with its own fds helps — it
    /// asks for exactly one.
    Instances,
    /// A watcher exists but has no room for another path.
    Watches,
}

impl WatchLimitKind {
    pub fn sysctl_key(self) -> &'static str {
        match self {
            Self::Instances => "fs.inotify.max_user_instances",
            Self::Watches => "fs.inotify.max_user_watches",
        }
    }

    /// A value that is comfortable rather than merely enough. Both
    /// pools cost a little kernel memory per entry and nothing else,
    /// and a limit raised to exactly today's need runs out again the
    /// next time a browser opens a few more tabs.
    fn floor(self) -> u64 {
        match self {
            Self::Instances => 1024,
            Self::Watches => 524_288,
        }
    }

    /// What the dialog says the download manager can no longer do.
    pub fn consequence(self) -> &'static str {
        match self {
            Self::Instances => {
                "oxdm could not start watching your download folders, so a finished \
                 download that is moved, renamed or deleted elsewhere will keep its \
                 row until the next time oxdm starts."
            }
            Self::Watches => {
                "oxdm could not watch all of your download folders, so files moved out \
                 of some of them will keep their rows until the next time oxdm starts."
            }
        }
    }
}

/// A refusal, with the numbers needed to explain and to fix it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchLimit {
    pub kind: WatchLimitKind,
    /// The limit in force, when it could be read. `None` on a system
    /// that does not publish it — the explanation survives without the
    /// number, but there is nothing to propose.
    pub current: Option<u64>,
    /// What to raise it to. `None` when `current` is unknown.
    pub suggested: Option<u64>,
}

impl WatchLimit {
    pub fn new(kind: WatchLimitKind, current: Option<u64>) -> Self {
        Self {
            kind,
            current,
            suggested: current.map(|c| suggested_for(kind, c)),
        }
    }

    /// The one-line change. Shown verbatim in the dialog and copyable,
    /// so a user who would rather not be asked for a password — or
    /// whose system has no way to ask — has the whole fix in hand.
    pub fn sysctl_line(&self) -> Option<String> {
        self.suggested
            .map(|n| format!("{} = {n}", self.kind.sysctl_key()))
    }
}

/// Double it, but never propose less than a comfortable floor.
fn suggested_for(kind: WatchLimitKind, current: u64) -> u64 {
    current.saturating_mul(2).max(kind.floor())
}

/// Read a limit the kernel publishes. Linux-only by construction:
/// `/proc/sys` is where these live, and nowhere else has them.
pub fn read_limit(kind: WatchLimitKind) -> Option<u64> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let path = format!("/proc/sys/{}", kind.sysctl_key().replace('.', "/"));
    std::fs::read_to_string(path)
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
}

/// Is this error one of the two pools running out?
///
/// `EMFILE` / `ENFILE` come back from `inotify_init` when the per-user
/// or system-wide instance table is full; `ENOSPC` is how the kernel
/// reports a full watch table, which is a famously misleading "No
/// space left on device" that has nothing to do with disks.
///
/// Everything else — a sandbox with no filesystem notification, a
/// backend that is simply absent — is a different problem with a
/// different answer, and raising a limit would not touch it.
pub fn classify(err: &std::io::Error) -> Option<WatchLimitKind> {
    match err.raw_os_error() {
        // ENFILE (23), EMFILE (24) — system-wide and per-user.
        Some(23 | 24) => Some(WatchLimitKind::Instances),
        // ENOSPC (28).
        Some(28) => Some(WatchLimitKind::Watches),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_instance_table_is_told_apart_from_a_full_watch_table() {
        // EMFILE, ENFILE, ENOSPC — the three the kernel actually uses.
        assert_eq!(
            classify(&std::io::Error::from_raw_os_error(24)),
            Some(WatchLimitKind::Instances)
        );
        assert_eq!(
            classify(&std::io::Error::from_raw_os_error(23)),
            Some(WatchLimitKind::Instances)
        );
        assert_eq!(
            classify(&std::io::Error::from_raw_os_error(28)),
            Some(WatchLimitKind::Watches)
        );
    }

    /// A missing backend is not a limit, and offering to raise one
    /// would send the user to change a setting that changes nothing.
    #[test]
    fn an_unrelated_failure_proposes_nothing() {
        assert_eq!(
            classify(&std::io::Error::from_raw_os_error(2)), // ENOENT
            None
        );
        assert_eq!(
            classify(&std::io::Error::other("no backend on this platform")),
            None
        );
    }

    #[test]
    fn the_proposal_doubles_the_limit_but_not_below_a_workable_floor() {
        let low = WatchLimit::new(WatchLimitKind::Instances, Some(128));
        assert_eq!(low.suggested, Some(1024), "doubling 128 is still too few");
        let high = WatchLimit::new(WatchLimitKind::Instances, Some(1024));
        assert_eq!(high.suggested, Some(2048));
        assert_eq!(
            high.sysctl_line().as_deref(),
            Some("fs.inotify.max_user_instances = 2048")
        );
    }

    /// Without a number to raise, the dialog can still say what broke
    /// — it just has nothing to propose, and must not invent one.
    #[test]
    fn an_unreadable_limit_proposes_nothing_to_run() {
        let unknown = WatchLimit::new(WatchLimitKind::Watches, None);
        assert_eq!(unknown.suggested, None);
        assert_eq!(unknown.sysctl_line(), None);
    }
}

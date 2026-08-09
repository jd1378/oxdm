//! Will this fit? Asked before a download starts, and before a queue
//! does.
//!
//! Two places need room, and they are not always the same disk: the
//! parts are written to the work directory, and the finished file is
//! assembled into the save folder. At the moment of assembly both
//! exist, so a job whose two folders share a volume needs room for the
//! file twice over — the single most common way a "there was plenty of
//! space" download dies at 100%.
//!
//! Only what is known counts. A download of unknown length, or one
//! nobody has probed yet, contributes nothing to the tally rather than
//! a guess: refusing to start on an invented number would be worse
//! than running out honestly.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Space one job wants, and where.
#[derive(Debug, Clone)]
pub struct Need {
    /// Where the `.part` files go — the per-job work directory.
    pub work_dir: PathBuf,
    /// Where the assembled file lands.
    pub save_dir: PathBuf,
    /// The file's full size, or `None` when nothing knows it yet.
    pub total: Option<u64>,
    /// Bytes already on disk in the work directory, which this run does
    /// not have to fetch again.
    pub downloaded: u64,
}

/// One volume that cannot hold what is planned for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shortfall {
    /// A folder on the volume, for naming it to the user.
    pub path: PathBuf,
    pub needed: u64,
    pub available: u64,
}

/// Left unspoken for: a volume with nothing left is a broken desktop —
/// no logs, no temp files, no saving whatever else is open. oxdm filling
/// it to the last byte is not a download that succeeded.
pub const RESERVE: u64 = 64 * 1024 * 1024;

/// Total bytes wanted per volume, keyed by whatever `volume` calls the
/// volume a path is on.
///
/// The work and save folders of one job are counted separately, so they
/// add up when they share a volume and stand alone when they do not.
pub fn required_by_volume<K: std::hash::Hash + Eq>(
    needs: &[Need],
    volume: impl Fn(&Path) -> K,
) -> HashMap<K, (PathBuf, u64)> {
    let mut out: HashMap<K, (PathBuf, u64)> = HashMap::new();
    for need in needs {
        let Some(total) = need.total.filter(|t| *t > 0) else {
            continue;
        };
        // The parts only need what is left to fetch; the assembled file
        // needs the whole thing however far along the run is.
        let remaining = total.saturating_sub(need.downloaded);
        for (dir, bytes) in [(&need.work_dir, remaining), (&need.save_dir, total)] {
            if bytes == 0 {
                continue;
            }
            let entry = out
                .entry(volume(dir))
                .or_insert_with(|| (dir.clone(), 0u64));
            entry.1 = entry.1.saturating_add(bytes);
        }
    }
    out
}

/// The worst volume that comes up short, or `None` if everything fits.
///
/// Worst rather than first: with two volumes short, the one missing the
/// most is the one worth naming, and the check runs again on the next
/// attempt anyway.
pub fn shortfall<K: std::hash::Hash + Eq>(
    required: HashMap<K, (PathBuf, u64)>,
    free: impl Fn(&Path) -> Option<u64>,
) -> Option<Shortfall> {
    required
        .into_values()
        .filter_map(|(path, needed)| {
            // A volume we cannot measure is not a volume we refuse to
            // write to. Being unable to ask is not an answer of "no".
            let available = free(&path)?;
            let budget = available.saturating_sub(RESERVE);
            (needed > budget).then_some(Shortfall {
                path,
                needed,
                available,
            })
        })
        .max_by_key(|s| s.needed.saturating_sub(s.available))
}

/// Which volume a path is on, for paths that may not exist yet.
///
/// A save folder is created on demand, so the question is asked of the
/// nearest ancestor that does exist — which is on the same volume as
/// the folder about to be made inside it.
pub fn volume_key(path: &Path) -> String {
    let existing = nearest_existing(path);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let Ok(meta) = std::fs::metadata(&existing) {
            return format!("dev:{}", meta.dev());
        }
    }
    // Windows has no device id in `Metadata`; the volume is the prefix
    // — a drive letter or a UNC share — and paths on one are on one
    // filesystem.
    let prefix = existing
        .components()
        .next()
        .map(|c| c.as_os_str().to_string_lossy().to_uppercase())
        .unwrap_or_default();
    format!("root:{prefix}")
}

/// Free bytes on the volume holding `path`, asked of the nearest
/// ancestor that exists.
pub fn free_space(path: &Path) -> Option<u64> {
    fs4::available_space(nearest_existing(path)).ok()
}

fn nearest_existing(path: &Path) -> PathBuf {
    let mut probe = path;
    loop {
        if probe.exists() {
            return probe.to_path_buf();
        }
        match probe.parent() {
            Some(p) => probe = p,
            None => return path.to_path_buf(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn need(work: &str, save: &str, total: Option<u64>, downloaded: u64) -> Need {
        Need {
            work_dir: PathBuf::from(work),
            save_dir: PathBuf::from(save),
            total,
            downloaded,
        }
    }

    /// One volume for both folders is the common desktop case, and the
    /// one that used to die at 100%: the parts and the file it is
    /// assembled from are on disk at the same time.
    #[test]
    fn one_volume_holding_both_needs_the_file_twice() {
        let needs = [need("/work", "/home/dl", Some(1_000), 0)];
        let by_vol = required_by_volume(&needs, |_| "same");
        assert_eq!(by_vol["same"].1, 2_000);
    }

    /// Two volumes: the parts on one, the file on the other, neither
    /// paying for the other's copy.
    #[test]
    fn separate_volumes_each_carry_their_own() {
        let needs = [need("/work", "/mnt/big/dl", Some(1_000), 0)];
        let by_vol = required_by_volume(
            &needs,
            |p: &Path| {
                if p.starts_with("/mnt") { "big" } else { "root" }
            },
        );
        assert_eq!(by_vol["root"].1, 1_000);
        assert_eq!(by_vol["big"].1, 1_000);
    }

    /// A queue is the sum of what its downloads still need. Bytes
    /// already fetched are not fetched twice, but the assembled file is
    /// still the whole file.
    #[test]
    fn a_queue_sums_and_a_resume_counts_only_what_is_left() {
        let needs = [
            need("/work", "/home/dl", Some(1_000), 400),
            need("/work", "/home/dl", Some(500), 0),
        ];
        let by_vol = required_by_volume(&needs, |_| "same");
        // parts: 600 + 500, file: 1000 + 500
        assert_eq!(by_vol["same"].1, 2_600);
    }

    /// Unknown length and never probed contribute nothing — a refusal
    /// built on a guess is worse than running out honestly.
    #[test]
    fn what_nobody_knows_is_not_counted() {
        let needs = [
            need("/work", "/home/dl", None, 0),
            need("/work", "/home/dl", Some(0), 0),
        ];
        assert!(required_by_volume(&needs, |_| "same").is_empty());
    }

    #[test]
    fn the_worst_short_volume_is_the_one_reported() {
        let mut required = HashMap::new();
        required.insert("a", (PathBuf::from("/a"), 10_000 + RESERVE));
        required.insert("b", (PathBuf::from("/b"), 100_000 + RESERVE));
        let short = shortfall(required, |_| Some(1_000)).expect("both are short");
        assert_eq!(short.path, PathBuf::from("/b"));

        let mut fits = HashMap::new();
        fits.insert("a", (PathBuf::from("/a"), 1_000));
        assert!(shortfall(fits, |_| Some(1_000 + RESERVE)).is_none());
    }

    /// A volume that cannot be measured is not a refusal: being unable
    /// to ask is not an answer of "no".
    #[test]
    fn an_unmeasurable_volume_does_not_refuse() {
        let mut required = HashMap::new();
        required.insert("a", (PathBuf::from("/a"), u64::MAX));
        assert!(shortfall(required, |_| None).is_none());
    }
}

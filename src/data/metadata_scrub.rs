//! Clears the credentials odl before 3.3 left in each download's
//! `metadata.pb`.
//!
//! odl kept a job's request headers in that file verbatim, so the cookies
//! and tokens the store keeps encrypted sat beside the parts in plaintext.
//! 3.3 filters them whenever it writes the file, but it only writes it for
//! a download that runs again: a finished one, or one removed from the
//! list with its partial data kept, holds them for good.

use std::io::Write;
use std::path::{Path, PathBuf};

use odl::download_metadata::DownloadMetadata;
use prost::Message;

use crate::data::state::PER_JOB_PREFIX;

const METADATA: &str = "metadata.pb";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Scrubbed {
    pub rewritten: usize,
    /// Files or folders that could not be read or rewritten. Non-zero
    /// means the pass has to run again.
    pub failed: usize,
}

/// Scrub every per-job folder directly under each of `work_roots`.
///
/// Must run while no download does: odl rewrites the same file as a
/// transfer progresses, and a stale copy swapped in over it would roll the
/// parts' offsets back.
pub fn scrub_credentials(work_roots: &[PathBuf]) -> Scrubbed {
    let mut out = Scrubbed::default();
    for root in work_roots {
        let entries = match std::fs::read_dir(root) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                tracing::warn!(dir = %root.display(), error = %e, "cannot scan work dir for stored credentials");
                out.failed += 1;
                continue;
            }
        };
        for entry in entries.flatten() {
            // `file_type` does not follow symlinks: a linked folder is not
            // ours to rewrite through.
            let ours = entry
                .file_name()
                .to_string_lossy()
                .starts_with(PER_JOB_PREFIX)
                && entry.file_type().is_ok_and(|t| t.is_dir());
            if !ours {
                continue;
            }
            let path = entry.path().join(METADATA);
            match scrub_file(&path) {
                Ok(true) => out.rewritten += 1,
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "cannot clear stored credentials");
                    out.failed += 1;
                }
            }
        }
    }
    out
}

/// `Ok(true)` when the file was rewritten without them.
fn scrub_file(path: &Path) -> std::io::Result<bool> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    // odl frames it length-delimited. A file that does not decode is one
    // odl will not use either, and has its own answer for.
    let Ok(mut stored) = DownloadMetadata::decode_length_delimited(bytes.as_slice()) else {
        return Ok(false);
    };
    // odl's own filter, through the same round trip it makes on a write.
    // A file it cannot rebuild a download from keeps no headers at all:
    // nothing reads stored request headers back for a request.
    let dir = path.parent().unwrap_or(path).to_path_buf();
    let kept = odl::Download::from_metadata(dir, stored.clone())
        .map(|d| d.as_metadata().headers)
        .unwrap_or_default();
    if kept == stored.headers {
        return Ok(false);
    }
    stored.headers = kept;
    replace(path, &stored.encode_length_delimited_to_vec())?;
    Ok(true)
}

/// Swap `bytes` in for `path` in one step, so a crash leaves the old file
/// or the new one, never half of either. Owner-only, as odl writes it.
fn replace(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("pb.scrub");
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let write = || -> std::io::Result<()> {
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    };
    write().inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::state::per_job_dir;
    use crate::domain::JobId;

    /// A file as odl 3.2 wrote it for a download captured with a cookie.
    fn written_by_old_odl(root: &Path) -> PathBuf {
        let dir = per_job_dir(root, JobId::new());
        std::fs::create_dir_all(&dir).unwrap();
        let metadata = DownloadMetadata {
            url: "https://cdn.example/file.bin?sig=x".into(),
            filename: "file.bin".into(),
            size: Some(10),
            headers: [
                ("cookie", "session=s3cret"),
                ("authorization", "Bearer t0ken"),
                ("user-agent", "Mozilla/5.0"),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect(),
            ..Default::default()
        };
        let path = dir.join(METADATA);
        std::fs::write(&path, metadata.encode_length_delimited_to_vec()).unwrap();
        path
    }

    fn read(path: &Path) -> DownloadMetadata {
        DownloadMetadata::decode_length_delimited(std::fs::read(path).unwrap().as_slice()).unwrap()
    }

    #[test]
    fn drops_credentials_and_keeps_the_rest() {
        let root = tempfile::tempdir().unwrap();
        let path = written_by_old_odl(root.path());

        let scrub = scrub_credentials(&[root.path().to_path_buf()]);

        assert_eq!(
            scrub,
            Scrubbed {
                rewritten: 1,
                failed: 0
            }
        );
        let after = read(&path);
        let mut names: Vec<_> = after.headers.keys().map(String::as_str).collect();
        names.sort_unstable();
        assert_eq!(names, ["user-agent"]);
        assert_eq!(after.url, "https://cdn.example/file.bin?sig=x");
        assert_eq!(after.size, Some(10));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    /// The marker that stops it running again can fail to save; a second
    /// pass must then change nothing.
    #[test]
    fn a_second_pass_rewrites_nothing() {
        let root = tempfile::tempdir().unwrap();
        written_by_old_odl(root.path());
        let roots = [root.path().to_path_buf()];
        scrub_credentials(&roots);

        assert_eq!(scrub_credentials(&roots), Scrubbed::default());
    }

    #[test]
    fn leaves_folders_that_are_not_downloads_alone() {
        let root = tempfile::tempdir().unwrap();
        let theirs = root.path().join("not-oxdm");
        std::fs::create_dir_all(&theirs).unwrap();
        let path = written_by_old_odl(root.path());
        std::fs::rename(&path, theirs.join(METADATA)).unwrap();
        let before = std::fs::read(theirs.join(METADATA)).unwrap();

        let scrub = scrub_credentials(&[root.path().to_path_buf(), root.path().join("gone")]);

        assert_eq!(scrub, Scrubbed::default());
        assert_eq!(std::fs::read(theirs.join(METADATA)).unwrap(), before);
    }
}

//! Helpers for the capture flows that hand their request to a dialog
//! instead of adding a job.
//!
//! Two callers stage: `batch_capture` with `interactive: true` (many
//! items, `oxdm gui batch <path>`), and a single `capture` with
//! `interactive: true` (one item, `oxdm gui add --staged <path>`).
//! Neither can travel on the command line — the requests carry cookies,
//! headers and a referrer — so they are serialized to a single-use JSON
//! file and only the path is passed. The dialog reads the file once,
//! deletes it, and renders from it. Nothing is added until the user
//! confirms in that dialog.
//!
//! The items carry cookies and whatever headers the page was using, so
//! the file lives in the per-user runtime dir (0700) as a 0600 file,
//! not in the shared temp dir where it used to be created world-
//! readable. It is deleted by the dialog, by the failure paths here,
//! and — for the case where no dialog ever ran — by [`sweep_stale`] at
//! daemon start.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::domain::CaptureRequest;

/// Prefixes are per-flow so a stale file says which dialog never ran.
/// [`sweep_stale`] cleans both.
const BATCH_PREFIX: &str = "oxdm-batch-";
const CAPTURE_PREFIX: &str = "oxdm-capture-";
const PREFIXES: [&str; 2] = [BATCH_PREFIX, CAPTURE_PREFIX];

/// Stage the items of an interactive `batch_capture` for `oxdm gui batch`.
pub fn stage_batch(items: &[CaptureRequest]) -> std::io::Result<PathBuf> {
    stage(BATCH_PREFIX, items)
}

/// Stage a single interactive `capture` for `oxdm gui add --staged`.
pub fn stage_capture(req: &CaptureRequest) -> std::io::Result<PathBuf> {
    stage(CAPTURE_PREFIX, req)
}

/// Load + delete a staged batch file. Used by the dialog subprocess.
pub fn load_batch(path: &Path) -> std::io::Result<Vec<CaptureRequest>> {
    load_and_consume(path)
}

/// Load + delete a staged single-capture file. Used by the Add dialog.
pub fn load_capture(path: &Path) -> std::io::Result<CaptureRequest> {
    load_and_consume(path)
}

fn stage<T: serde::Serialize + ?Sized>(prefix: &str, value: &T) -> std::io::Result<PathBuf> {
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%.f");
    let path = crate::ipc_local::auth::runtime_dir()?
        .join(format!("{prefix}{stamp}-{}.json", std::process::id()));
    let json = serde_json::to_vec(value).map_err(std::io::Error::other)?;

    let mut opts = std::fs::OpenOptions::new();
    // `create_new` so a name we did not create is never written
    // through; the timestamp+pid makes a collision our own bug.
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let write = || -> std::io::Result<()> {
        let mut f = opts.open(&path)?;
        f.write_all(&json)?;
        f.sync_all()
    };
    if let Err(e) = write() {
        // Half a file of cookies is worse than none.
        let _ = std::fs::remove_file(&path);
        return Err(e);
    }
    Ok(path)
}

fn load_and_consume<T: serde::de::DeserializeOwned>(path: &Path) -> std::io::Result<T> {
    let bytes = std::fs::read(path)?;
    let parsed = serde_json::from_slice(&bytes).map_err(std::io::Error::other);
    // Deleted whether or not it parsed: a file we could not read is one
    // nobody will read, and it is still full of cookies.
    let _ = std::fs::remove_file(path);
    parsed
}

/// Delete staged files no dialog consumed — a window that was killed, or
/// a daemon that died between staging and spawning, would otherwise
/// leave captured cookies on disk until the next logout.
pub fn sweep_stale() {
    let Ok(dir) = crate::ipc_local::auth::runtime_dir() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if PREFIXES.iter().any(|p| name.starts_with(p)) && name.ends_with(".json") {
            let path = entry.path();
            match std::fs::remove_file(&path) {
                Ok(()) => tracing::info!(path = %path.display(), "removed a stale staged file"),
                Err(e) => tracing::warn!(path = %path.display(), error = %e, "stale staged file"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One test, not three: `runtime_dir` follows the process-wide
    /// `XDG_RUNTIME_DIR`, so parallel tests setting it would race each
    /// other's staging directory.
    #[cfg(unix)]
    #[test]
    fn staged_files_are_owner_only_and_never_outlive_their_reader() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        // The harness owns the runtime dir for the duration of this test.
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", tmp.path()) };
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;

        let batch = stage_batch(&[]).unwrap();
        assert_eq!(mode(&batch), 0o600, "staged batch file is world-readable");
        load_batch(&batch).unwrap();
        assert!(!batch.exists());

        let mut req = CaptureRequest::from_url("https://example.com/f.zip".parse().unwrap());
        req.cookies = Some("session=secret".to_owned());
        let capture = stage_capture(&req).unwrap();
        assert_eq!(mode(&capture), 0o600, "staged capture is world-readable");
        let back = load_capture(&capture).unwrap();
        assert_eq!(back.cookies.as_deref(), Some("session=secret"));
        assert!(!capture.exists(), "the cookies outlived their reader");

        // And a dialog that never ran leaves nothing behind either.
        let batch = stage_batch(&[]).unwrap();
        let capture = stage_capture(&req).unwrap();
        sweep_stale();
        assert!(!batch.exists(), "stale batch file survived the sweep");
        assert!(!capture.exists(), "stale capture file survived the sweep");
    }
}

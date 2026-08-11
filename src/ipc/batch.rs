//! Helpers for the interactive batch-capture flow.
//!
//! When the WS bridge receives `batch_capture` with `interactive: true`,
//! the items are too large + structured to pass on the command line.
//! Instead, we serialize them to a single-use JSON file, then hand the
//! path to a fresh `oxdm gui batch <path>` subprocess. The dialog reads
//! the file once, deletes it, and renders the table.
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

const PREFIX: &str = "oxdm-batch-";

pub fn stage_for_dialog(items: &[CaptureRequest]) -> std::io::Result<PathBuf> {
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%.f");
    let path = crate::ipc_local::auth::runtime_dir()?
        .join(format!("{PREFIX}{stamp}-{}.json", std::process::id()));
    let json = serde_json::to_vec(items).map_err(std::io::Error::other)?;

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

/// Load + delete a staged batch file. Used by the dialog subprocess.
pub fn load_and_consume(path: &Path) -> std::io::Result<Vec<CaptureRequest>> {
    let bytes = std::fs::read(path)?;
    let parsed = serde_json::from_slice(&bytes).map_err(std::io::Error::other);
    // Deleted whether or not it parsed: a file we could not read is one
    // nobody will read, and it is still full of cookies.
    let _ = std::fs::remove_file(path);
    parsed
}

/// Delete batch files no dialog consumed — a window that was killed, or
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
        if name.starts_with(PREFIX) && name.ends_with(".json") {
            let path = entry.path();
            match std::fs::remove_file(&path) {
                Ok(()) => tracing::info!(path = %path.display(), "removed a stale batch file"),
                Err(e) => tracing::warn!(path = %path.display(), error = %e, "stale batch file"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn a_staged_batch_is_only_readable_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        // `runtime_dir` follows XDG_RUNTIME_DIR, which the harness owns
        // for the duration of this test.
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", tmp.path()) };

        let path = stage_for_dialog(&[]).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "staged batch file is world-readable");

        // And it does not outlive its reader.
        load_and_consume(&path).unwrap();
        assert!(!path.exists());
    }
}

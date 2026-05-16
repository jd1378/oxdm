//! Helpers for the interactive batch-capture flow.
//!
//! When the WS bridge receives `batch_capture` with `interactive: true`,
//! the items are too large + structured to pass on the command line.
//! Instead, we serialize them to a single-use JSON file in the user's
//! temp dir, then hand the path to a fresh `oxdm gui batch <path>`
//! subprocess. The dialog reads the file once, deletes it, and renders
//! the table.

use std::io::Write;
use std::path::PathBuf;

use crate::domain::CaptureRequest;

pub fn stage_for_dialog(items: &[CaptureRequest]) -> std::io::Result<PathBuf> {
    let mut path = std::env::temp_dir();
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%.f");
    path.push(format!("oxdm-batch-{stamp}-{}.json", std::process::id()));
    let json = serde_json::to_vec(items).map_err(std::io::Error::other)?;
    let mut f = std::fs::File::create(&path)?;
    f.write_all(&json)?;
    f.sync_all()?;
    Ok(path)
}

/// Load + delete a staged batch file. Used by the dialog subprocess.
pub fn load_and_consume(path: &std::path::Path) -> std::io::Result<Vec<CaptureRequest>> {
    let bytes = std::fs::read(path)?;
    let items: Vec<CaptureRequest> =
        serde_json::from_slice(&bytes).map_err(std::io::Error::other)?;
    let _ = std::fs::remove_file(path);
    Ok(items)
}

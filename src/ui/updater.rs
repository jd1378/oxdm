//! Driver for the `oxdm-updater` child process.
//!
//! Spawns the helper binary, streams its stdout JSON events to a
//! tokio mpsc, and exposes a `confirm()` method that writes `go\n` to
//! its stdin. After confirm, the GUI is expected to quit so the helper
//! can replace the running executable and relaunch it.

use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, mpsc};

use crate::data::UpdaterEvent;

/// Owns the running child + a sender into its stdin. Drop = kill (the
/// child's wait closure handles cleanup).
pub struct UpdaterHandle {
    child: Mutex<Child>,
    stdin: Mutex<Option<ChildStdin>>,
}

impl UpdaterHandle {
    /// Send the `go` line that greenlights the swap-and-relaunch step.
    /// Idempotent — second call is a no-op.
    pub async fn confirm(&self) -> Result<(), String> {
        let mut guard = self.stdin.lock().await;
        let stdin = guard
            .as_mut()
            .ok_or_else(|| "already confirmed".to_string())?;
        stdin
            .write_all(b"go\n")
            .await
            .map_err(|e| format!("confirm write: {e}"))?;
        stdin.flush().await.map_err(|e| e.to_string())?;
        // Drop the handle so the child sees EOF if it ever switches
        // to line-loop reads.
        guard.take();
        Ok(())
    }

    /// Best-effort kill. Used when the user closes the About page
    /// before reaching `Ready`.
    pub async fn abort(&self) {
        let _ = self.child.lock().await.kill().await;
    }
}

/// Launch `oxdm-updater` next to the running binary. The GUI has
/// already downloaded the artifact and hands its on-disk path + the
/// feed-published SHA-256 to the helper, which verifies and performs
/// the swap.
pub fn spawn(
    artifact: std::path::PathBuf,
    sha256: String,
) -> Result<(std::sync::Arc<UpdaterHandle>, mpsc::Receiver<UpdaterEvent>), String> {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let updater = exe
        .parent()
        .ok_or_else(|| "exe has no parent dir".to_string())?
        .join(if cfg!(windows) {
            "oxdm-updater.exe"
        } else {
            "oxdm-updater"
        });
    if !updater.exists() {
        return Err(format!("updater binary not found at {}", updater.display()));
    }

    let mut child = Command::new(&updater)
        .arg("--exe")
        .arg(&exe)
        .arg("--pid")
        .arg(std::process::id().to_string())
        .arg("--artifact")
        .arg(&artifact)
        .arg("--sha256")
        .arg(&sha256)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn updater: {e}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "no updater stdout".to_string())?;
    let stdin = child.stdin.take();

    let (tx, rx) = mpsc::channel::<UpdaterEvent>(64);
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            match serde_json::from_str::<UpdaterEvent>(&line) {
                Ok(ev) => {
                    if tx.send(ev).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = tx
                        .send(UpdaterEvent::Error {
                            message: format!("bad updater frame: {e}: {line}"),
                        })
                        .await;
                }
            }
        }
    });

    Ok((
        std::sync::Arc::new(UpdaterHandle {
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
        }),
        rx,
    ))
}

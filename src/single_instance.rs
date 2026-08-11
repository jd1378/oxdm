//! Single-instance guard.
//!
//! Two cooperating mechanisms:
//! - `single-instance` crate holds a per-user named lock (POSIX file
//!   lock on Unix, `CreateMutex` on Windows). It cheaply answers
//!   "am I the first?".
//! - `interprocess` local socket (a 0600 filesystem socket in the
//!   per-user runtime dir on Unix, named pipe on Windows) carries a
//!   one-line `SHOW` ping from secondary launches to the primary. The
//!   peer's credentials are checked on accept: `SHOW` pops a window on
//!   the user's screen, and only the user may ask for that.
//!
//! On launch:
//! 1. Take the named lock. If we are not single, connect to the local
//!    socket, send `SHOW\n`, and exit. The primary surfaces its main
//!    window in response.
//! 2. If we are single, return a guard. The app boot hook converts
//!    the guard into a tokio listener and spawns the accept loop.
//!
//! Per-user separation: the lock name + socket name include the uid
//! on Unix so two different users on the same machine don't collide.

use std::io::{Read, Write};
use std::sync::Arc;
use std::time::Duration;

use interprocess::local_socket::{
    ListenerOptions, Stream, prelude::*, traits::tokio::Listener as _,
};
use single_instance::SingleInstance;

const SHOW_CMD: &str = "SHOW\n";
const SHOW_OK: &[u8] = b"OK\n";

fn user_suffix() -> String {
    // Env override lets developers (and the visual-test harness) run a
    // sandboxed second instance alongside the host daemon. Value is
    // appended so it cannot collide with another user's uid.
    let extra = std::env::var("OXDM_INSTANCE_SUFFIX")
        .ok()
        .filter(|s| !s.is_empty());
    #[cfg(unix)]
    let base = format!("{}", unsafe { libc::getuid() });
    #[cfg(not(unix))]
    let base = "user".to_string();
    match extra {
        Some(s) => format!("{base}-{s}"),
        None => base,
    }
}

fn lock_name() -> String {
    format!("oxdm-lock-{}", user_suffix())
}

/// Windows named-pipe name. Unix uses a filesystem socket in the 0700
/// runtime dir instead — see [`crate::ipc_local::auth`].
#[cfg(not(unix))]
fn socket_name() -> String {
    format!("oxdm-{}.sock", user_suffix())
}

pub enum InstanceOutcome {
    Primary(InstanceGuard),
    AlreadyRunning,
}

pub struct InstanceGuard {
    lock: SingleInstance,
}

pub fn acquire() -> std::io::Result<InstanceOutcome> {
    let lock =
        SingleInstance::new(&lock_name()).map_err(|e| std::io::Error::other(e.to_string()))?;

    if !lock.is_single() {
        // Lock failed → primary is alive. Signal it and exit.
        // Retry briefly: primary may still be wiring its listener.
        for _ in 0..20 {
            if signal_show().is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        return Ok(InstanceOutcome::AlreadyRunning);
    }

    Ok(InstanceOutcome::Primary(InstanceGuard { lock }))
}

#[cfg(unix)]
fn show_socket_path() -> std::io::Result<std::path::PathBuf> {
    Ok(crate::ipc_local::auth::runtime_dir()?.join("show.sock"))
}

fn signal_show() -> std::io::Result<()> {
    #[cfg(unix)]
    let mut conn = {
        use interprocess::local_socket::{GenericFilePath, ToFsName};
        let name = show_socket_path()?.to_fs_name::<GenericFilePath>()?;
        Stream::connect(name)?
    };
    #[cfg(not(unix))]
    let mut conn = {
        use interprocess::local_socket::{GenericNamespaced, ToNsName};
        let name = socket_name().to_ns_name::<GenericNamespaced>()?;
        Stream::connect(name)?
    };
    conn.write_all(SHOW_CMD.as_bytes())?;
    let mut buf = [0u8; 8];
    let _ = conn.read(&mut buf);
    Ok(())
}

impl InstanceGuard {
    /// Bind the local-socket listener (must run inside a tokio
    /// runtime) and spawn the accept loop. The lock is held for the
    /// process lifetime via `mem::forget` — the OS releases it on
    /// exit, so a crash cannot leave us permanently locked out.
    pub fn spawn_listener(self, state: Arc<crate::data::AppState>) -> std::io::Result<()> {
        let Self { lock } = self;
        std::mem::forget(lock);

        #[cfg(unix)]
        let listener = {
            use interprocess::local_socket::{GenericFilePath, ToFsName};
            use interprocess::os::unix::local_socket::ListenerOptionsExt as _;
            let name = show_socket_path()?.to_fs_name::<GenericFilePath>()?;
            ListenerOptions::new()
                .name(name)
                .mode(0o600)
                .try_overwrite(true)
                .max_spin_time(Duration::from_secs(2))
                .create_tokio()?
        };
        #[cfg(not(unix))]
        let listener = {
            use interprocess::local_socket::{GenericNamespaced, ToNsName};
            let name = socket_name().to_ns_name::<GenericNamespaced>()?;
            ListenerOptions::new().name(name).create_tokio()?
        };

        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok(stream) => {
                        if !crate::ipc_local::auth::peer_is_self(&stream) {
                            tracing::warn!("single-instance rejected a foreign peer");
                            continue;
                        }
                        let state = state.clone();
                        tokio::spawn(handle_signal(stream, state));
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "single-instance accept");
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    }
                }
            }
        });
        Ok(())
    }
}

async fn handle_signal(
    mut stream: interprocess::local_socket::tokio::Stream,
    state: Arc<crate::data::AppState>,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut buf = [0u8; 32];
    let n = match tokio::time::timeout(Duration::from_millis(500), stream.read(&mut buf)).await {
        Ok(Ok(n)) => n,
        _ => return,
    };
    let line = match std::str::from_utf8(&buf[..n]) {
        Ok(s) => s.trim_end_matches(['\r', '\n']),
        Err(_) => return,
    };
    if line == "SHOW" {
        state.events_emit(crate::data::DomainEvent::ShowMainWindow);
        let _ = stream.write_all(SHOW_OK).await;
    }
}

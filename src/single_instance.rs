//! Single-instance guard.
//!
//! Two cooperating mechanisms:
//! - A per-user named lock answers "am I the first?" cheaply. On Linux
//!   that is an abstract unix socket bound here (see [`Lock`]);
//!   elsewhere it is the `single-instance` crate's own — a file locked
//!   with `flock` on macOS (at the absolute path [`lock_path`] gives),
//!   `CreateMutex` on Windows.
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
#[cfg(not(target_os = "linux"))]
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

/// The file macOS locks, as an absolute path.
///
/// The `single-instance` crate takes one `&str` for every platform and
/// means something different by it on each: a mutex name on Windows, an
/// abstract socket name on Linux, and on macOS a *filesystem path* it
/// hands straight to `File::create`. Passing the bare name there made
/// the lock relative to the working directory, so two launches from
/// different directories locked two different files and each concluded
/// it was the only instance.
///
/// It lives beside the database rather than in the runtime dir that
/// holds the socket: on macOS that dir is under `$TMPDIR`, which the
/// system prunes by last-access time. A daemon left running for days
/// would have kept its lock while the file it locks was deleted from
/// under it, and the next launch would have created a fresh file,
/// locked that instead, and started a second daemon.
#[cfg(target_os = "macos")]
fn lock_path() -> std::io::Result<std::path::PathBuf> {
    let dir = dirs::data_dir()
        .ok_or_else(|| std::io::Error::other("no data directory to lock in"))?
        .join("oxdm");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(format!("{}.lock", lock_name())))
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
    lock: Lock,
}

/// The Linux lock, bound here rather than through the `single-instance`
/// crate, for one reason: the crate creates its socket with
/// `SockFlag::empty()`, so the descriptor has no `FD_CLOEXEC` and every
/// child the daemon spawns inherits it.
///
/// An abstract socket's name stays taken while *any* process holds a
/// descriptor bound to it. So a child outliving the daemon by a moment
/// keeps the name taken, and the next oxdm to start gets `EADDRINUSE`,
/// decides another instance is running, and exits. That is what broke
/// the self-update: the updater is a child, and it is alive across
/// exactly the window in which the replacement app starts.
///
/// `SOCK_CLOEXEC` ends the whole class at the source — no spawn site
/// has to remember anything.
#[cfg(target_os = "linux")]
struct Lock {
    /// `None` when the name was already taken.
    fd: Option<std::os::fd::RawFd>,
}

#[cfg(target_os = "linux")]
impl Lock {
    fn new(name: &str) -> std::io::Result<Self> {
        // "\0name": the leading NUL is what makes it abstract, so it
        // lives in the network namespace rather than the filesystem and
        // needs no cleanup after a crash.
        let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
        let bytes = name.as_bytes();
        if bytes.len() + 1 > addr.sun_path.len() {
            return Err(std::io::Error::other("single-instance name too long"));
        }
        for (slot, b) in addr.sun_path.iter_mut().skip(1).zip(bytes) {
            *slot = *b as libc::c_char;
        }
        // Only the bytes actually used, or the kernel takes the
        // trailing NULs as part of the name.
        let len = (std::mem::size_of::<libc::sa_family_t>() + 1 + bytes.len()) as libc::socklen_t;

        // SAFETY: a plain socket call; the fd is checked before use and
        // owned by this struct from here on.
        let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `addr` outlives the call and `len` describes it.
        let bound = unsafe { libc::bind(fd, std::ptr::addr_of!(addr).cast(), len) };
        if bound == 0 {
            return Ok(Self { fd: Some(fd) });
        }
        let err = std::io::Error::last_os_error();
        // SAFETY: `fd` came from a successful `socket` and is not
        // stored anywhere.
        unsafe { libc::close(fd) };
        if err.raw_os_error() == Some(libc::EADDRINUSE) {
            Ok(Self { fd: None })
        } else {
            Err(err)
        }
    }

    fn is_single(&self) -> bool {
        self.fd.is_some()
    }
}

#[cfg(target_os = "linux")]
impl Drop for Lock {
    /// Only reached when a guard is dropped without being handed to
    /// [`InstanceGuard::spawn_listener`] — a boot that failed after
    /// taking the lock. The live daemon forgets the guard instead and
    /// lets the kernel free the name at exit.
    fn drop(&mut self) {
        if let Some(fd) = self.fd.take() {
            // SAFETY: the fd is ours, taken so it cannot close twice.
            unsafe { libc::close(fd) };
        }
    }
}

#[cfg(not(target_os = "linux"))]
struct Lock {
    inner: SingleInstance,
}

#[cfg(not(target_os = "linux"))]
impl Lock {
    fn new(name: &str) -> std::io::Result<Self> {
        SingleInstance::new(name)
            .map(|inner| Self { inner })
            .map_err(|e| std::io::Error::other(e.to_string()))
    }

    fn is_single(&self) -> bool {
        self.inner.is_single()
    }
}

pub fn acquire() -> std::io::Result<InstanceOutcome> {
    // macOS locks a path; the others are handed a name.
    #[cfg(target_os = "macos")]
    let lock = {
        let path = lock_path()?;
        // Refused rather than converted lossily: a path that is not
        // UTF-8 would come out of `to_string_lossy` as a *different*
        // path, and locking the wrong file is the bug this exists to
        // fix.
        let path = path
            .to_str()
            .ok_or_else(|| std::io::Error::other("lock path is not valid UTF-8"))?;
        Lock::new(path)?
    };
    #[cfg(not(target_os = "macos"))]
    let lock = Lock::new(&lock_name())?;

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
        // Held for the process lifetime: never closed, never dropped.
        // The OS releases it on exit, so a crash cannot leave us
        // permanently locked out.
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

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    fn name(tag: &str) -> String {
        format!("oxdm-test-{}-{}-{tag}", std::process::id(), unsafe {
            libc::getuid()
        })
    }

    #[test]
    fn the_second_instance_is_not_single() {
        let n = name("second");
        let first = Lock::new(&n).unwrap();
        assert!(first.is_single());
        assert!(!Lock::new(&n).unwrap().is_single());
        drop(first);
        // Freed with the last descriptor, so a later launch is primary.
        assert!(Lock::new(&n).unwrap().is_single());
    }

    /// The regression this file exists for.
    ///
    /// A child that inherited the lock descriptor kept the name taken
    /// after the daemon holding it had gone, and the oxdm the updater
    /// started next concluded another instance was already running and
    /// exited. `SOCK_CLOEXEC` means the child never receives it, so the
    /// name frees the moment the holder does — even while the child is
    /// still alive.
    #[test]
    fn a_surviving_child_does_not_keep_the_name_taken() {
        let n = name("child");
        let lock = Lock::new(&n).unwrap();
        assert!(lock.is_single());

        // No `attach_close_high_fds`: the point is that the descriptor
        // does not travel even to a child that takes no precautions.
        let mut child = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg("sleep 5")
            .spawn()
            .expect("spawn a child that outlives the lock");

        drop(lock);
        let taken_over = Lock::new(&n).unwrap();
        let single = taken_over.is_single();

        let _ = child.kill();
        let _ = child.wait();
        assert!(single, "the child pinned the abstract name");
    }
}

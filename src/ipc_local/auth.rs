//! Who is allowed to talk to the daemon's local IPC socket.
//!
//! The socket carries the daemon's whole control surface: plaintext job
//! secrets, queue hooks that run shell commands, `ResetDatabase`. It is
//! a per-user socket, so "may this peer speak" means "is this peer the
//! same user".
//!
//! Two independent gates, because neither is enough alone on every
//! platform we ship:
//!
//! - **Transport.** On Unix the socket is a filesystem socket inside a
//!   0700 directory with mode 0600, and every accepted connection has
//!   its peer credentials checked against our own uid. The abstract
//!   namespace this used to live in has no permissions at all — any
//!   local process could derive the name (it is just the uid) and
//!   connect.
//! - **Handshake.** Every connection must present a token read from a
//!   0600 file before any request is dispatched. On Windows, where the
//!   transport is a named pipe whose default DACL is more generous than
//!   we want and whose peer credentials carry no user identity, this is
//!   the gate that holds.
//!
//! The token is regenerated on every daemon start: it authenticates
//! "you can read a file only I can read", nothing longer-lived, so it
//! never needs to survive a restart.

use std::io;
use std::path::PathBuf;

/// Directory holding the socket and the token, one per user (plus
/// `OXDM_INSTANCE_SUFFIX`, so a sandboxed second instance is separate).
///
/// `$XDG_RUNTIME_DIR` is the right home for both: it is already
/// per-user, 0700, and cleared at logout. Without it (macOS, or a
/// stripped-down session) we fall back to a uid-tagged directory under
/// the temp dir, created 0700 ourselves.
pub fn runtime_dir() -> io::Result<PathBuf> {
    let dir = base_dir().join(dir_name());
    create_private_dir(&dir)?;
    Ok(dir)
}

fn base_dir() -> PathBuf {
    #[cfg(unix)]
    {
        match std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) {
            Some(d) => PathBuf::from(d),
            None => std::env::temp_dir(),
        }
    }
    #[cfg(not(unix))]
    {
        dirs::data_local_dir().unwrap_or_else(std::env::temp_dir)
    }
}

/// `oxdm-<uid>[-<suffix>]` — uid-tagged because the temp-dir fallback is
/// world-writable, so the directory name must not be guessable-and-
/// squattable by another user under a name we would then trust.
fn dir_name() -> String {
    #[cfg(unix)]
    let base = format!("oxdm-{}", unsafe { libc::getuid() });
    #[cfg(not(unix))]
    let base = "oxdm".to_string();
    match instance_suffix() {
        Some(s) => format!("{base}-{s}"),
        None => base,
    }
}

pub(crate) fn instance_suffix() -> Option<String> {
    std::env::var("OXDM_INSTANCE_SUFFIX")
        .ok()
        .filter(|s| !s.is_empty())
}

/// Create `dir` 0700, and refuse to use one that is not ours.
///
/// The refusal matters only for the temp-dir fallback, where another
/// user could have pre-created the path and be waiting for us to bind a
/// socket inside it.
fn create_private_dir(dir: &std::path::Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt};
        match std::fs::DirBuilder::new().mode(0o700).create(dir) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        let md = std::fs::metadata(dir)?;
        if !md.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} exists and is not a directory", dir.display()),
            ));
        }
        if md.uid() != unsafe { libc::getuid() } {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{} is owned by another user", dir.display()),
            ));
        }
        // A pre-existing dir may be more permissive than we want (an
        // older oxdm created it with the umask default).
        if md.mode() & 0o077 != 0 {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

/// Filesystem path of the GUI IPC socket (Unix only — on Windows the
/// transport is a named pipe addressed by name, see
/// [`super::socket_name`]).
#[cfg(unix)]
pub fn socket_path() -> io::Result<PathBuf> {
    Ok(runtime_dir()?.join("gui.sock"))
}

fn token_path() -> io::Result<PathBuf> {
    Ok(runtime_dir()?.join("ipc-token"))
}

/// Mint a fresh token and write it 0600, replacing any previous one.
/// Called once by the daemon before it binds the socket.
pub fn install_token() -> io::Result<String> {
    install_token_at(&token_path()?)
}

fn install_token_at(path: &std::path::Path) -> io::Result<String> {
    use rand::Rng;
    let mut raw = [0u8; 32];
    rand::rng().fill_bytes(&mut raw);
    let token = raw.iter().map(|b| format!("{b:02x}")).collect::<String>();

    // Replace rather than truncate-in-place: a stale token file could
    // be a symlink planted by whoever got there first.
    let _ = std::fs::remove_file(path);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    {
        use std::io::Write;
        let mut f = opts.open(path)?;
        f.write_all(token.as_bytes())?;
        f.flush()?;
    }
    Ok(token)
}

/// Read the token a client must present. Fails when no daemon has run,
/// which the caller reports the same way as a failed connect.
pub fn read_token() -> io::Result<String> {
    let token = std::fs::read_to_string(token_path()?)?;
    Ok(token.trim().to_string())
}

/// Constant-time comparison — the token is a secret and the server
/// answers on an unauthenticated connection, so a timing oracle here
/// would be free to probe.
pub fn token_matches(expected: &str, got: &str) -> bool {
    let (a, b) = (expected.as_bytes(), got.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Is the peer on the other end of this connection the user we are
/// running as?
///
/// Unix: `SO_PEERCRED` (or the platform equivalent), which the kernel
/// stamps at connect time and no process can forge. Windows: named-pipe
/// peer credentials carry a pid but no user identity, so the token
/// handshake is the gate there and this reports `true`.
pub fn peer_is_self<S>(stream: &S) -> bool
where
    S: interprocess::local_socket::traits::tokio::Stream,
{
    #[cfg(unix)]
    {
        match stream.peer_creds() {
            Ok(creds) => match creds.euid() {
                Some(uid) => uid == unsafe { libc::getuid() },
                // No euid on this platform: fall back to the token.
                None => true,
            },
            Err(e) => {
                tracing::warn!(error = %e, "peer credentials unavailable; refusing connection");
                false
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = stream;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_only_matches_itself() {
        assert!(token_matches("abc", "abc"));
        assert!(!token_matches("abc", "abd"));
        assert!(!token_matches("abc", "ab"));
        assert!(!token_matches("abc", ""));
        assert!(!token_matches("", "abc"));
    }

    #[test]
    fn minted_tokens_are_long_and_distinct() {
        // Guards against a refactor that hands out a constant.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ipc-token");
        let a = install_token_at(&path).unwrap();
        let b = install_token_at(&path).unwrap();
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), b);
    }

    #[cfg(unix)]
    #[test]
    fn the_token_file_is_not_readable_by_others() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ipc-token");
        install_token_at(&path).unwrap();
        let md = std::fs::metadata(&path).unwrap();
        assert_eq!(md.permissions().mode() & 0o777, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn a_private_dir_is_created_0700_and_tightened_if_loose() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("oxdm-run");
        create_private_dir(&dir).unwrap();
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );

        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        create_private_dir(&dir).unwrap();
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_non_directory_in_the_way_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("oxdm-run");
        std::fs::write(&path, b"not a dir").unwrap();
        assert!(create_private_dir(&path).is_err());
    }
}

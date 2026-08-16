//! Where the database lives.
//!
//! Compiled into two programs from this one file: the app gets it as
//! part of `data`, and `oxdm-native-host` includes the same source with
//! `#[path]` rather than linking the whole library into a shim that has
//! to stay small. The alternative is two copies of a path, and this
//! module exists because that is exactly what happened once — the
//! wrapper handed Flatpak browsers a location the daemon had never
//! used, and browser capture was broken for as long as nobody thought
//! to compare the two.
//!
//! Nothing here touches the database. It answers *where*, so that
//! everything asking gets the same answer.

use std::path::PathBuf;

/// `…/oxdm`: the app's own data directory, holding the database, the
/// per-job working directories and staged updates.
fn base() -> PathBuf {
    dirs::data_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("oxdm")
}

/// The directory the database has to itself.
///
/// Its own, and not shared with `work/` or `updates/`, because a
/// Flatpak browser has to be granted read access to *a directory* to
/// read the database at all: SQLite in WAL mode needs the `-wal` and
/// `-shm` sidecars beside the file, and those come and go, so naming
/// the three files in the grant would bind whichever happened to exist
/// when the sandbox started. A directory grant is the workable unit,
/// which makes the directory's contents the thing worth keeping small.
pub fn db_dir() -> PathBuf {
    base().join("db")
}

/// Where a database is created when there is not one already.
pub fn default_db_path() -> PathBuf {
    db_dir().join("oxdm.db")
}

/// Where the database lived before it had a directory of its own,
/// beside everything else under `…/oxdm`.
pub fn legacy_db_path() -> PathBuf {
    base().join("oxdm.db")
}

/// The database this install actually has, whichever place it is in.
///
/// Reads nothing and moves nothing: an install that has not been
/// migrated yet, or one where the move failed, still has to be
/// findable — by the shim, which only ever reads, and by the wrapper
/// that tells it where to look.
pub fn current_db_path() -> PathBuf {
    let current = default_db_path();
    if current.exists() {
        return current;
    }
    let legacy = legacy_db_path();
    if legacy.exists() {
        return legacy;
    }
    current
}

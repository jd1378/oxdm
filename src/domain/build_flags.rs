//! What this particular build is allowed to do, decided when it was
//! compiled rather than by anything the user can set.
//!
//! There is one flag, and it exists because "who owns these files" is
//! not oxdm's to decide. See `build.rs`.

/// May this build replace its own files?
///
/// `false` in a build compiled with `OXDM_NO_SELF_UPDATE=1` — a distro
/// package, a Flatpak, anything whose files a package manager owns.
/// Such a build does not check for updates, does not download one, does
/// not announce one, and shows no way to install one: the packaging is
/// what updates it, and an app that overwrote packaged files would
/// leave the package database describing something that is no longer
/// on disk.
///
/// Read as a constant so every layer answers the same way and the
/// unreachable half compiles out. Deliberately *not* a setting: a
/// packaged build must not be able to talk itself into self-updating,
/// and there is nothing here a user could sensibly choose.
pub const SELF_UPDATE: bool = env!("OXDM_SELF_UPDATE").as_bytes()[0] == b'1';

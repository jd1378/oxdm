//! Unpacking a self-update payload.
//!
//! An installed build is two programs: `oxdm` is the app, and
//! `oxdm-native-host` is what the browser launches to hand downloads
//! over. Replacing only the app leaves a machine running one version
//! beside a native host from another, talking a protocol that may have
//! moved on.
//!
//! So the artifact for an installed build is the release archive
//! itself — the same tar.gz a person downloads — and this takes the
//! two programs out of it. Everything else in there (the README, the
//! licence, the icon, the install script) is ignored, which is what
//! lets one archive serve both purposes instead of publishing a
//! near-duplicate beside it.
//!
//! Nothing is trusted about the archive's own paths. Only entries whose
//! *file name* is one of the binaries we ship are written, into a
//! directory of our choosing — an entry claiming to be `../../.bashrc`
//! never gets the chance to say so. The artifact is already checked
//! against the digest the feed published, so this is the second lock on
//! the same door rather than the only one.

use std::io;
use std::path::{Path, PathBuf};

/// What an installed build consists of. An archive entry named
/// anything else is ignored.
const BINARIES: [&str; 2] = ["oxdm", "oxdm-native-host"];

/// Is this the name of one of our binaries, with or without the
/// Windows suffix? Compared case-insensitively because Windows file
/// names are.
fn is_ours(name: &str) -> bool {
    let stem = name
        .strip_suffix(".exe")
        .or_else(|| name.strip_suffix(".EXE"))
        .unwrap_or(name);
    BINARIES.iter().any(|b| b.eq_ignore_ascii_case(stem))
}

/// Unpack the binaries from `archive` into `dest`, returning what was
/// written. `dest` is created if missing.
///
/// An archive with none of our binaries in it is an error rather than
/// an empty success: it means the release published something this
/// build does not understand, and installing nothing while reporting
/// success would leave the user on the old version believing they had
/// updated.
pub fn extract(archive: &Path, dest: &Path) -> io::Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dest)?;
    let file = std::fs::File::open(archive)?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
    let mut written = Vec::new();
    for entry in tar.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        // `file_name` alone: whatever directories the entry claims to
        // live in are discarded rather than followed.
        let name = match entry
            .path()?
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
        {
            Some(n) if is_ours(&n) => n,
            _ => continue,
        };
        let out = dest.join(&name);
        entry.unpack(&out)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perm = std::fs::metadata(&out)?.permissions();
            perm.set_mode(0o755);
            std::fs::set_permissions(&out, perm)?;
        }
        written.push(out);
    }
    if written.is_empty() {
        return Err(io::Error::other(
            "the update archive holds none of oxdm's programs",
        ));
    }
    Ok(written)
}

/// Extension left on a program that had to be renamed out of the way
/// because it was running when the update replaced it.
const DISPLACED: &str = "oxdm-old";

/// Delete anything the last update could not.
///
/// Replacing a running program on Windows means renaming it aside, and
/// the rename cannot be followed by a delete while the old copy is
/// still executing — the updater itself is always in exactly that
/// position. By the next launch nothing is holding them, so this is
/// where they go. Failure is not worth reporting: an undeletable
/// leftover is untidy, not broken, and it will be tried again.
pub fn sweep_displaced(beside: &Path) {
    let Some(dir) = beside.parent() else { return };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == DISPLACED)
            && let Some(stem) = path.file_stem().map(|s| s.to_string_lossy().into_owned())
            && is_ours(&stem)
            && std::fs::remove_file(&path).is_ok()
        {
            tracing::debug!(path = %path.display(), "removed a displaced program from an earlier update");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Build a gzipped tar from `(name, contents)` pairs.
    ///
    /// The name is written into the raw header rather than through
    /// `append_data`, which refuses to *write* a path containing `..` —
    /// a courtesy no hostile archive would extend us, and the reason
    /// this helper exists in this shape.
    fn tarball(dir: &Path, entries: &[(&str, &[u8])]) -> PathBuf {
        let path = dir.join("payload.tar.gz");
        let gz = flate2::write::GzEncoder::new(
            std::fs::File::create(&path).unwrap(),
            flate2::Compression::fast(),
        );
        let mut builder = tar::Builder::new(gz);
        for (name, body) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o755);
            header.set_entry_type(tar::EntryType::Regular);
            {
                let raw = header.as_gnu_mut().unwrap();
                let bytes = name.as_bytes();
                assert!(bytes.len() < raw.name.len());
                raw.name[..bytes.len()].copy_from_slice(bytes);
            }
            header.set_cksum();
            builder.append(&header, *body).unwrap();
        }
        builder
            .into_inner()
            .unwrap()
            .finish()
            .unwrap()
            .flush()
            .unwrap();
        path
    }

    #[test]
    fn only_our_binaries_come_out() {
        let dir = tempfile::tempdir().unwrap();
        let archive = tarball(
            dir.path(),
            &[
                ("oxdm", b"app"),
                ("oxdm-native-host", b"host"),
                ("README.md", b"docs"),
                ("oxdm.png", b"icon"),
            ],
        );
        let dest = dir.path().join("out");
        let mut written = extract(&archive, &dest).unwrap();
        written.sort();
        assert_eq!(written.len(), 2, "{written:?}");
        assert!(dest.join("oxdm").exists());
        assert!(dest.join("oxdm-native-host").exists());
        assert!(!dest.join("README.md").exists(), "no passengers");
    }

    /// The digest check already stands between a hostile archive and
    /// this code. It is not the only thing that has to.
    #[test]
    fn an_entry_cannot_escape_the_destination() {
        let dir = tempfile::tempdir().unwrap();
        let archive = tarball(
            dir.path(),
            &[
                ("../../oxdm", b"escapee"),
                ("nested/dir/oxdm-native-host", b"host"),
            ],
        );
        let dest = dir.path().join("out");
        let written = extract(&archive, &dest).unwrap();

        for path in &written {
            assert_eq!(path.parent(), Some(dest.as_path()), "{path:?} left the pen");
        }
        assert!(dest.join("oxdm").exists(), "written by its name alone");
        assert!(dest.join("oxdm-native-host").exists());
        assert!(!dir.path().join("../../oxdm").exists());
    }

    /// The shape the release workflow actually produces: everything
    /// inside one versioned directory, with documentation beside the
    /// programs. This is the archive the updater downloads, so if the
    /// packaging changed under it the update would fail at the last
    /// step, after the download, on the user's machine.
    #[test]
    fn the_release_archive_gives_up_its_programs() {
        let dir = tempfile::tempdir().unwrap();
        let root = "oxdm-v1.2.3-x86_64-unknown-linux-gnu";
        let archive = tarball(
            dir.path(),
            &[
                (&format!("{root}/oxdm"), b"app"),
                (&format!("{root}/oxdm-native-host"), b"host"),
                (&format!("{root}/README.md"), b"docs"),
                (&format!("{root}/LICENSE"), b"agpl"),
                (&format!("{root}/oxdm.png"), b"icon"),
                (&format!("{root}/install-native-host.sh"), b"script"),
            ],
        );
        let dest = dir.path().join("out");
        let mut written = extract(&archive, &dest).unwrap();
        written.sort();
        assert_eq!(written.len(), 2, "{written:?}");
        assert!(dest.join("oxdm").exists());
        assert!(dest.join("oxdm-native-host").exists());
        assert!(!dest.join("README.md").exists(), "no passengers");
        assert!(!dest.join(root).exists(), "and no directory to walk");
    }

    /// The same archive as Windows builds it.
    #[test]
    fn the_windows_release_archive_works_too() {
        let dir = tempfile::tempdir().unwrap();
        let root = "oxdm-v1.2.3-x86_64-pc-windows-msvc";
        let archive = tarball(
            dir.path(),
            &[
                (&format!("{root}/oxdm.exe"), b"app"),
                (&format!("{root}/oxdm-native-host.exe"), b"host"),
                (&format!("{root}/README.md"), b"docs"),
            ],
        );
        let dest = dir.path().join("out");
        let written = extract(&archive, &dest).unwrap();
        assert_eq!(written.len(), 2, "{written:?}");
        assert!(dest.join("oxdm.exe").exists());
        assert!(dest.join("oxdm-native-host.exe").exists());
    }

    #[test]
    fn an_archive_without_our_programs_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let archive = tarball(dir.path(), &[("README.md", b"docs")]);
        assert!(extract(&archive, &dir.path().join("out")).is_err());
    }

    /// Only ours, and only the displaced ones: a sweep that took its
    /// cue from the extension alone would delete a neighbour's file.
    #[test]
    fn the_sweep_takes_only_what_an_update_left() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("oxdm");
        for name in [
            "oxdm.oxdm-old",
            "oxdm-native-host.oxdm-old",
            "notes.oxdm-old",
            "oxdm",
            "oxdm-native-host",
        ] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        sweep_displaced(&exe);
        assert!(!dir.path().join("oxdm.oxdm-old").exists());
        assert!(!dir.path().join("oxdm-native-host.oxdm-old").exists());
        assert!(dir.path().join("notes.oxdm-old").exists(), "not ours");
        assert!(dir.path().join("oxdm").exists(), "still the app");
        assert!(dir.path().join("oxdm-native-host").exists());
    }

    #[test]
    fn the_windows_suffix_is_recognised() {
        assert!(is_ours("oxdm.exe"));
        assert!(is_ours("oxdm-native-host.EXE"));
        assert!(is_ours("oxdm"));
        assert!(!is_ours("oxdm-testserver"));
        assert!(!is_ours("notoxdm"));
    }
}

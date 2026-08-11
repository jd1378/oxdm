//! What a typed "Save to" path actually means.
//!
//! The field holds one string but carries two values: the folder and
//! the file name. Splitting it on the last separator is right only
//! while the string still ends in a file name. The moment someone
//! deletes the name to retarget the folder — the common way to do it —
//! a naive split hands the *folder's own name* back as the file name,
//! and the download goes to `~/Downloads` as a file called `Downloads`
//! rather than into it.
//!
//! So the split asks whether the tail names a folder, and takes the
//! known name when it does. The answer needs the filesystem and the
//! user's settings, which do not belong here, so both arrive as
//! borrows on [`Resolver`] and the decision itself stays pure.

use std::path::{Path, PathBuf};

/// A save path split the way the daemon wants it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    pub dir: PathBuf,
    /// `None` only when nothing suggested a name — the daemon then
    /// derives one from the URL or the response.
    pub filename: Option<String>,
}

impl Destination {
    /// The path as the field should spell it: what the user will get.
    pub fn display_path(&self) -> PathBuf {
        match &self.filename {
            Some(name) => self.dir.join(name),
            None => self.dir.clone(),
        }
    }
}

/// The context a save path is read in.
pub struct Resolver<'a> {
    /// Where a path with no folder of its own lands.
    pub fallback_dir: &'a Path,
    /// Folders oxdm already knows are folders — the category targets.
    /// Consulted so a category folder that has not been created yet is
    /// still read as a folder rather than as a file name.
    pub known_dirs: &'a [PathBuf],
    /// Filesystem probe, injected so the domain stays pure and the
    /// tests do not need a real tree.
    pub is_dir: &'a dyn Fn(&Path) -> bool,
}

impl Resolver<'_> {
    /// Split `input` into a folder and a file name, keeping
    /// `current_name` whenever the input turns out to name a folder.
    pub fn resolve(&self, input: &str, current_name: Option<&str>) -> Destination {
        let name = current_name
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_owned);
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Destination {
                dir: self.fallback_dir.to_path_buf(),
                filename: name,
            };
        }
        let p = PathBuf::from(trimmed);
        if self.names_a_dir(trimmed, &p) {
            return Destination {
                dir: p,
                filename: name,
            };
        }
        // A bare file name has no folder to keep; anything else brings
        // its own.
        let dir = match p.parent() {
            Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
            _ => self.fallback_dir.to_path_buf(),
        };
        Destination {
            dir,
            // Falls back to the known name for the shapes with no tail
            // to take: `/`, `..`, and the like.
            filename: p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .or(name),
        }
    }

    fn names_a_dir(&self, trimmed: &str, p: &Path) -> bool {
        // A trailing separator is the one unambiguous statement of
        // intent the field can carry, and it survives a folder that
        // does not exist yet.
        trimmed.ends_with(std::path::is_separator)
            // Nothing left after the folder part: `/`, `.`, `..`.
            || p.file_name().is_none()
            || self.known_dirs.iter().any(|d| d == p)
            || (self.is_dir)(p)
    }
}

/// How the name being written differs from the name the file came
/// with, in the one way that changes what happens when it is opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionChange {
    Dropped(String),
    Replaced { from: String, to: String },
}

/// The extension `known` carries and `chosen` does not, if any.
///
/// Renaming `report.pdf` to `report` or to `report.txt` is a legal
/// thing to want and oxdm writes what it is told, but on every desktop
/// the extension is what decides which application opens the file, and
/// either edit is as easily a slip as a decision. Reporting is not
/// refusing: the note says what will happen, and the user goes on.
pub fn extension_change(known: &str, chosen: &str) -> Option<ExtensionChange> {
    let ext = |p: &str| {
        Path::new(p)
            .extension()
            .map(|e| e.to_string_lossy().into_owned())
            .filter(|e| !e.is_empty())
    };
    let from = ext(known)?;
    match ext(chosen) {
        None => Some(ExtensionChange::Dropped(from)),
        Some(to) if to != from => Some(ExtensionChange::Replaced { from, to }),
        Some(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver<'a>(known: &'a [PathBuf], dirs: &'a dyn Fn(&Path) -> bool) -> Resolver<'a> {
        Resolver {
            fallback_dir: Path::new("/home/u/Downloads"),
            known_dirs: known,
            is_dir: dirs,
        }
    }

    #[test]
    fn an_existing_folder_keeps_the_name_it_was_given() {
        let none: [PathBuf; 0] = [];
        let is_dir = |p: &Path| p == Path::new("/home/u/Videos");
        let d = resolver(&none, &is_dir).resolve("/home/u/Videos", Some("clip.mkv"));
        assert_eq!(d.dir, Path::new("/home/u/Videos"));
        assert_eq!(d.filename.as_deref(), Some("clip.mkv"));
    }

    /// The case a plain split gets wrong: the folder's own name comes
    /// back as the file name and the download overwrites the folder.
    #[test]
    fn a_folder_is_never_read_as_the_file_to_write() {
        let none: [PathBuf; 0] = [];
        let is_dir = |p: &Path| p == Path::new("/home/u/Downloads");
        let d = resolver(&none, &is_dir).resolve("/home/u/Downloads", Some("clip.mkv"));
        assert_eq!(d.display_path(), Path::new("/home/u/Downloads/clip.mkv"));
    }

    /// Category folders are created on first use, so "does not exist
    /// yet" must not mean "is a file name".
    #[test]
    fn a_category_folder_counts_before_it_exists() {
        let known = [PathBuf::from("/home/u/Downloads/Video")];
        let is_dir = |_: &Path| false;
        let d = resolver(&known, &is_dir).resolve("/home/u/Downloads/Video", Some("clip.mkv"));
        assert_eq!(d.dir, Path::new("/home/u/Downloads/Video"));
        assert_eq!(d.filename.as_deref(), Some("clip.mkv"));
    }

    #[test]
    fn a_trailing_separator_means_a_folder_whatever_is_on_disk() {
        let none: [PathBuf; 0] = [];
        let is_dir = |_: &Path| false;
        let d = resolver(&none, &is_dir).resolve("/home/u/new-place/", Some("clip.mkv"));
        assert_eq!(d.dir, Path::new("/home/u/new-place"));
        assert_eq!(d.filename.as_deref(), Some("clip.mkv"));
    }

    /// A folder whose name has a dot in it is still a folder — the
    /// extension says nothing about what is on disk.
    #[test]
    fn a_dotted_folder_is_still_a_folder() {
        let none: [PathBuf; 0] = [];
        let is_dir = |p: &Path| p == Path::new("/home/u/my.files");
        let d = resolver(&none, &is_dir).resolve("/home/u/my.files", Some("clip.mkv"));
        assert_eq!(d.display_path(), Path::new("/home/u/my.files/clip.mkv"));
    }

    #[test]
    fn a_full_path_keeps_the_name_that_was_typed() {
        let none: [PathBuf; 0] = [];
        let is_dir = |_: &Path| false;
        let d = resolver(&none, &is_dir).resolve("/home/u/Videos/other.mkv", Some("clip.mkv"));
        assert_eq!(d.dir, Path::new("/home/u/Videos"));
        assert_eq!(d.filename.as_deref(), Some("other.mkv"));
    }

    #[test]
    fn an_empty_field_is_the_fallback_folder() {
        let none: [PathBuf; 0] = [];
        let is_dir = |_: &Path| false;
        let d = resolver(&none, &is_dir).resolve("   ", Some("clip.mkv"));
        assert_eq!(d.display_path(), Path::new("/home/u/Downloads/clip.mkv"));
    }

    #[test]
    fn a_bare_name_lands_in_the_fallback_folder() {
        let none: [PathBuf; 0] = [];
        let is_dir = |_: &Path| false;
        let d = resolver(&none, &is_dir).resolve("clip.mkv", None);
        assert_eq!(d.display_path(), Path::new("/home/u/Downloads/clip.mkv"));
    }

    /// With no name anywhere the daemon derives one; the split must not
    /// invent `Downloads` as the file.
    #[test]
    fn a_folder_with_no_name_to_keep_stays_nameless() {
        let none: [PathBuf; 0] = [];
        let is_dir = |p: &Path| p == Path::new("/home/u/Downloads");
        let d = resolver(&none, &is_dir).resolve("/home/u/Downloads", None);
        assert_eq!(d.dir, Path::new("/home/u/Downloads"));
        assert_eq!(d.filename, None);
    }

    #[test]
    fn an_extension_that_goes_missing_or_changes_is_reported() {
        assert_eq!(
            extension_change("clip.mkv", "clip"),
            Some(ExtensionChange::Dropped("mkv".into()))
        );
        assert_eq!(
            extension_change("clip.mkv", "clip.mp4"),
            Some(ExtensionChange::Replaced {
                from: "mkv".into(),
                to: "mp4".into()
            })
        );
        // The name changed, the extension did not: nothing to say.
        assert_eq!(extension_change("clip.mkv", "holiday.mkv"), None);
        // Nothing to lose.
        assert_eq!(extension_change("clip", "anything"), None);
        // A dotfile is a name, not an extension it just lost.
        assert_eq!(extension_change(".bashrc", "bashrc"), None);
    }

    #[test]
    fn the_root_is_a_folder() {
        let none: [PathBuf; 0] = [];
        let is_dir = |_: &Path| false;
        let d = resolver(&none, &is_dir).resolve("/", Some("clip.mkv"));
        assert_eq!(d.display_path(), Path::new("/clip.mkv"));
    }
}

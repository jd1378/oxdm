//! The "Save to" field, bound to the real disk.
//!
//! [`crate::domain::save_path`] decides what a typed path means; this
//! hands it the two things it will not look up for itself — the user's
//! folders and whether a path is a directory — so the Add dialog and
//! the Properties dialog read the same field the same way.

use std::path::Path;

use crate::domain::{Destination, SavePathResolver, Settings};

/// Split what the user typed, keeping `current_name` when the path
/// turns out to name a folder.
pub fn destination(settings: &Settings, input: &str, current_name: Option<&str>) -> Destination {
    let fallback = settings.fallback_dir();
    let known = settings.known_dirs();
    let is_dir = |p: &Path| p.is_dir();
    SavePathResolver {
        fallback_dir: &fallback,
        known_dirs: &known,
        is_dir: &is_dir,
    }
    .resolve(input, current_name)
}

/// What to show under the field when the download will not land where
/// the text literally reads — the user deleted the file name, or typed
/// a bare one. `None` when the text already says it.
pub fn note(input: &str, dest: &Destination) -> Option<String> {
    let resolved = dest.display_path();
    (Path::new(input.trim()) != resolved).then(|| format!("Saves as {}", resolved.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dest(dir: &str, name: Option<&str>) -> Destination {
        Destination {
            dir: dir.into(),
            filename: name.map(str::to_owned),
        }
    }

    #[test]
    fn the_note_appears_only_when_the_text_understates_where_it_goes() {
        let d = dest("/home/u/Videos", Some("clip.mkv"));
        assert_eq!(
            note("/home/u/Videos", &d).as_deref(),
            Some("Saves as /home/u/Videos/clip.mkv")
        );
        assert_eq!(note("/home/u/Videos/clip.mkv", &d), None);
        // Trailing separators and stray spaces are spelling, not a
        // different destination.
        assert_eq!(note("  /home/u/Videos/clip.mkv  ", &d), None);
    }
}

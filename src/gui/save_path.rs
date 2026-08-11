//! The "Save to" field, bound to the real disk.
//!
//! [`crate::domain::save_path`] decides what a typed path means; this
//! hands it the two things it will not look up for itself — the user's
//! folders and whether a path is a directory — so the Add dialog and
//! the Properties dialog read the same field the same way.

use std::path::Path;

use crate::domain::save_path::ExtensionChange;
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

/// A line under the field, when the field alone leaves something out.
pub struct Note {
    pub text: String,
    /// The file will land where the user said, but not under the name
    /// their system knows how to open.
    pub warning: bool,
}

/// What to show under the field: where the file actually lands when the
/// text does not say it, else the extension the typed name drops.
/// `None` when the text already tells the whole story.
///
/// One line, not two: the first case resolves to the known name, so it
/// answers the extension question by showing the answer.
pub fn note(input: &str, dest: &Destination, known_name: Option<&str>) -> Option<Note> {
    let resolved = dest.display_path();
    if Path::new(input.trim()) != resolved {
        return Some(Note {
            text: format!("Will save to {}", resolved.display()),
            warning: false,
        });
    }
    let change = crate::domain::save_path::extension_change(
        known_name?,
        dest.filename.as_deref().unwrap_or_default(),
    )?;
    Some(Note {
        text: match change {
            ExtensionChange::Dropped(ext) => {
                format!("No .{ext} extension. Apps may not know how to open it")
            }
            ExtensionChange::Replaced { from, to } => {
                format!("Saved as .{to}, not .{from}. Apps may open it with the wrong program")
            }
        },
        warning: true,
    })
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
        let n = note("/home/u/Videos", &d, Some("clip.mkv")).expect("a note");
        assert_eq!(n.text, "Will save to /home/u/Videos/clip.mkv");
        assert!(!n.warning);
        assert!(note("/home/u/Videos/clip.mkv", &d, Some("clip.mkv")).is_none());
        // Trailing separators and stray spaces are spelling, not a
        // different destination.
        assert!(note("  /home/u/Videos/clip.mkv  ", &d, Some("clip.mkv")).is_none());
    }

    #[test]
    fn a_name_typed_without_its_extension_is_flagged() {
        let d = dest("/home/u/Videos", Some("clip"));
        let n = note("/home/u/Videos/clip", &d, Some("clip.mkv")).expect("a note");
        assert_eq!(
            n.text,
            "No .mkv extension. Apps may not know how to open it"
        );
        assert!(n.warning);
    }

    #[test]
    fn a_swapped_extension_is_flagged_too() {
        let d = dest("/home/u/Videos", Some("clip.mp4"));
        let n = note("/home/u/Videos/clip.mp4", &d, Some("clip.mkv")).expect("a note");
        assert_eq!(
            n.text,
            "Saved as .mp4, not .mkv. Apps may open it with the wrong program"
        );
        assert!(n.warning);
    }

    /// The destination line already spells the full name out, so it
    /// wins rather than stacking a second line under the first.
    #[test]
    fn only_one_line_at_a_time() {
        let d = dest("/home/u/Videos", Some("clip.mkv"));
        let n = note("/home/u/Videos", &d, Some("clip.mkv")).expect("a note");
        assert!(n.text.starts_with("Will save to "));
    }
}

//! File-type categories shown in the sidebar tree.
//!
//! Categories are derived from the filename extension at render time; the
//! `Job` does not store a category. The default extension lists below
//! mirror the AB Download Manager categories. Users can extend them via
//! `Settings::category_extensions`.
//!
//! Every category but `Other` can be deleted from Settings, which drops
//! it from `ALL_VISIBLE` for that user (see
//! `Settings::deleted_categories`). `Other` is the catch-all everything
//! else falls into, so it has nowhere to fall to and always stays.

use serde::{Deserialize, Serialize};

use super::Settings;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Compressed,
    Programs,
    Videos,
    Music,
    Pictures,
    Documents,
    Other,
}

impl Category {
    pub const ALL_VISIBLE: &'static [Category] = &[
        Category::Compressed,
        Category::Programs,
        Category::Videos,
        Category::Music,
        Category::Pictures,
        Category::Documents,
    ];

    /// Every category a user can explicitly assign — the visible set
    /// plus the `Other` catch-all.
    pub const ALL_ASSIGNABLE: &'static [Category] = &[
        Category::Compressed,
        Category::Programs,
        Category::Videos,
        Category::Music,
        Category::Pictures,
        Category::Documents,
        Category::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Compressed => "Compressed",
            Category::Programs => "Programs",
            Category::Videos => "Videos",
            Category::Music => "Music",
            Category::Pictures => "Pictures",
            Category::Documents => "Documents",
            Category::Other => "Other",
        }
    }

    /// Stable lowercase name: the serialized form, and what the GUI
    /// subprocesses are launched with (`--category videos`).
    pub fn slug(self) -> &'static str {
        match self {
            Category::Compressed => "compressed",
            Category::Programs => "programs",
            Category::Videos => "videos",
            Category::Music => "music",
            Category::Pictures => "pictures",
            Category::Documents => "documents",
            Category::Other => "other",
        }
    }

    pub fn from_slug(s: &str) -> Option<Category> {
        Category::ALL_ASSIGNABLE
            .iter()
            .copied()
            .find(|c| c.slug() == s)
    }

    pub fn default_extensions(self) -> &'static [&'static str] {
        match self {
            Category::Compressed => &[
                "zip", "rar", "7z", "tar", "gz", "bz2", "xz", "zst", "lz", "lzma", "tgz", "tbz2",
                "txz",
            ],
            Category::Programs => &[
                "exe", "msi", "msix", "appx", "dmg", "pkg", "deb", "rpm", "apk", "appimage",
                "snap", "flatpak", "bin", "run", "sh",
            ],
            Category::Videos => &[
                "mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "mpg", "mpeg", "ts",
                "vob", "ogv",
            ],
            Category::Music => &[
                "mp3", "flac", "wav", "aac", "m4a", "ogg", "opus", "wma", "ape", "alac", "aiff",
            ],
            Category::Pictures => &[
                "jpg", "jpeg", "png", "gif", "webp", "bmp", "tiff", "tif", "svg", "heic", "heif",
                "ico", "raw", "cr2", "nef", "arw",
            ],
            Category::Documents => &[
                "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp", "rtf",
                "txt", "md", "epub", "mobi", "azw", "azw3", "csv",
            ],
            Category::Other => &[],
        }
    }
}

/// Classify a filename into a category using the user's extension lists,
/// falling back to the built-in defaults for any category the user has
/// not customised. A deleted category claims nothing, so its extensions
/// fall through to `Other` like any unknown suffix.
pub fn classify(filename: &str, settings: &Settings) -> Category {
    let ext = match filename.rsplit_once('.') {
        Some((_, e)) if !e.is_empty() => e.to_ascii_lowercase(),
        _ => return Category::Other,
    };
    for cat in Category::ALL_VISIBLE {
        if settings.category_deleted(*cat) {
            continue;
        }
        if let Some(list) = settings.category_extensions.get(cat) {
            if list.iter().any(|e| e.eq_ignore_ascii_case(&ext)) {
                return *cat;
            }
        } else if cat.default_extensions().iter().any(|e| *e == ext) {
            return *cat;
        }
    }
    Category::Other
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deleted_category_falls_through_to_other() {
        let mut s = Settings::default();
        assert_eq!(classify("clip.mkv", &s), Category::Videos);
        s.deleted_categories = vec![Category::Videos];
        assert_eq!(classify("clip.mkv", &s), Category::Other);
        // Its neighbours keep classifying.
        assert_eq!(classify("song.mp3", &s), Category::Music);
    }

    #[test]
    fn deleting_other_is_not_possible() {
        let mut s = Settings {
            deleted_categories: vec![Category::Other],
            ..Settings::default()
        };
        s.normalize_categories();
        assert!(s.deleted_categories.is_empty());
    }
}

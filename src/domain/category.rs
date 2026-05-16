//! File-type categories shown in the sidebar tree.
//!
//! Categories are derived from the filename extension at render time; the
//! `Job` does not store a category. The default extension lists below
//! mirror the AB Download Manager categories. Users can extend them via
//! `Settings::category_extensions`.

use serde::{Deserialize, Serialize};

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
/// not customised.
pub fn classify(filename: &str, overrides: &indexmap::IndexMap<Category, Vec<String>>) -> Category {
    let ext = match filename.rsplit_once('.') {
        Some((_, e)) if !e.is_empty() => e.to_ascii_lowercase(),
        _ => return Category::Other,
    };
    for cat in Category::ALL_VISIBLE {
        if let Some(list) = overrides.get(cat) {
            if list.iter().any(|e| e.eq_ignore_ascii_case(&ext)) {
                return *cat;
            }
        } else if cat.default_extensions().iter().any(|e| *e == ext) {
            return *cat;
        }
    }
    Category::Other
}

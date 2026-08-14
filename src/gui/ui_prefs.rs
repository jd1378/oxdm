//! Persisted GUI view state (`$config/oxdm/ui-prefs.json`): the main
//! window's last size, its table columns, and which sidebar entry it was
//! looking at. Loaded on launch, saved as each changes.

use serde::{Deserialize, Serialize};

use crate::domain::{Category, QueueId};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct UiPrefs {
    #[serde(default)]
    pub window: Option<WindowPrefs>,
    /// Dropped when it cannot be read: the arrays are fixed-length, so a
    /// file from a build with a different column set fails to parse, and
    /// a stale table layout must not cost the user their window size.
    #[serde(default, deserialize_with = "columns_or_none")]
    pub columns: Option<ColumnsState>,
    /// Cached mirror of `Settings.custom_window_chrome`. The setting
    /// itself stays the source of truth; it is copied here because a
    /// window must decide on decorations when it is created, which is
    /// before the daemon connection that carries `Settings` exists.
    /// `None` = never seen, treated as the default (native chrome).
    #[serde(default)]
    pub custom_window_chrome: Option<bool>,
    /// The sidebar entry the main window was on when it last closed.
    /// `None` = never saved, or a file this build cannot read; the
    /// window then falls back to its default (the built-in queue).
    #[serde(default, deserialize_with = "sidebar_or_none")]
    pub sidebar: Option<SidebarPref>,
}

/// Serialisable twin of `windows::main::SidebarFilter`. Kept here rather
/// than deriving on the window's own enum so persistence does not depend
/// on a UI type — and so renaming a variant in the window is a
/// deliberate migration, not a silent change of the on-disk shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "view", rename_all = "snake_case")]
pub enum SidebarPref {
    All,
    Category { category: Category },
    Queue { id: QueueId },
}

/// A `sidebar` this build cannot parse (an unknown category, a
/// hand-edited file) must cost the user nothing else in the file.
fn sidebar_or_none<'de, D>(d: D) -> Result<Option<SidebarPref>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(raw).ok())
}

/// Number of table columns = `windows::main::SortColumn` variants.
pub const COLS: usize = 6;

/// Main-table column widths + visibility, indexed by
/// `windows::main::SortColumn as usize` (Name, Size, Status, Speed,
/// Eta, Date).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnsState {
    pub widths: [f32; COLS],
    pub hidden: [bool; COLS],
    /// Left-to-right display order as column indices. Separate from
    /// `widths`/`hidden`, which stay indexed by column so a reorder
    /// never has to rewrite them.
    #[serde(default = "identity_order")]
    pub order: [usize; COLS],
}

fn columns_or_none<'de, D>(d: D) -> Result<Option<ColumnsState>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(raw).ok())
}

fn identity_order() -> [usize; COLS] {
    std::array::from_fn(|i| i)
}

/// `SortColumn::Name` — the one column that can never be hidden.
const NAME_IDX: usize = 0;

/// Minimum table column width. The design's `ResizableHeader` says 60px,
/// but its headers are plain text; ours reserve room for the sort
/// chevron and the ellipsis, so 60 left the shortest labels truncated on
/// arrival.
pub const COL_MIN_W: f32 = 75.0;

/// Per-column minimum, indexed like `widths`.
const COL_MIN: [f32; COLS] = [COL_MIN_W; COLS];

impl Default for ColumnsState {
    fn default() -> Self {
        // Order = SortColumn (Name, Size, Status, Speed, Eta, Date),
        // at the design's default widths.
        Self {
            widths: [420.0, 90.0, 280.0, 100.0, 90.0, 130.0],
            hidden: [false; COLS],
            order: identity_order(),
        }
    }
}

impl ColumnsState {
    pub fn width(&self, idx: usize) -> f32 {
        self.widths[idx].max(COL_MIN[idx])
    }
    pub fn set_width(&mut self, idx: usize, w: f32) {
        self.widths[idx] = w.max(COL_MIN[idx]);
    }
    pub fn is_visible(&self, idx: usize) -> bool {
        !self.hidden[idx]
    }
    /// Name is always visible.
    pub fn toggle(&mut self, idx: usize) {
        if idx != NAME_IDX {
            self.hidden[idx] = !self.hidden[idx];
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct WindowPrefs {
    pub width: f32,
    pub height: f32,
}

fn prefs_path() -> Option<std::path::PathBuf> {
    Some(dirs::config_dir()?.join("oxdm").join("ui-prefs.json"))
}

impl ColumnsState {
    /// A hand-edited or truncated `order` must not drop or double a
    /// column — every index has to appear exactly once.
    fn order_is_sane(&self) -> bool {
        let mut seen = [false; COLS];
        for &i in &self.order {
            if i >= COLS || seen[i] {
                return false;
            }
            seen[i] = true;
        }
        true
    }
}

pub fn load() -> UiPrefs {
    let Some(path) = prefs_path() else {
        return UiPrefs::default();
    };
    let mut prefs: UiPrefs = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    if let Some(cols) = &mut prefs.columns
        && !cols.order_is_sane()
    {
        cols.order = identity_order();
    }
    prefs
}

/// Written beside the target and renamed over it, so a reader sees the
/// old file or the new one and never half of one. Every window kind is
/// its own process, so two can save at once — hence both the rename and
/// the pid in the temp name.
///
/// Deliberately not fsynced: this file holds window size, columns and
/// the sidebar view. Losing it to a power cut costs a re-tune, which is
/// not worth a disk flush on every sidebar click.
pub fn save(prefs: &UiPrefs) {
    let Some(path) = prefs_path() else { return };
    save_at(&path, prefs);
}

fn save_at(path: &std::path::Path, prefs: &UiPrefs) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let Ok(bytes) = serde_json::to_vec_pretty(prefs) else {
        return;
    };
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    if std::fs::write(&tmp, &bytes).is_err() || std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

pub fn save_window(w: WindowPrefs) {
    let mut prefs = load();
    prefs.window = Some(w);
    save(&prefs);
}

/// Refresh the cached chrome preference from the daemon's settings.
/// Costs nothing while the two agree, which is every case but the
/// snapshot right after the user flips the toggle.
pub fn sync_custom_window_chrome(v: bool) {
    if v == crate::gui::chrome::titlebar::use_custom() {
        return;
    }
    let mut prefs = load();
    prefs.custom_window_chrome = Some(v);
    save(&prefs);
}

pub fn save_sidebar(s: SidebarPref) {
    let mut prefs = load();
    if prefs.sidebar == Some(s) {
        return;
    }
    prefs.sidebar = Some(s);
    save(&prefs);
}

pub fn save_columns(c: &ColumnsState) {
    let mut prefs = load();
    prefs.columns = Some(c.clone());
    save(&prefs);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The file is replaced whole, and the scratch copy never survives
    /// the save that made it.
    #[test]
    fn a_save_replaces_the_file_and_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oxdm").join("ui-prefs.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{ truncated").unwrap();

        let prefs = UiPrefs {
            custom_window_chrome: Some(true),
            ..UiPrefs::default()
        };
        save_at(&path, &prefs);

        let back: UiPrefs = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(back.custom_window_chrome, Some(true));
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.file_name()))
            .filter(|n| n.to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "left {leftovers:?}");
    }
}

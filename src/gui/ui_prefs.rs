//! Persisted GUI view state (`$config/oxdm/ui-prefs.json`). Currently
//! the main window's last size; loaded on launch, saved on resize.

use serde::{Deserialize, Serialize};

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

pub fn save(prefs: &UiPrefs) {
    let Some(path) = prefs_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(prefs) {
        let _ = std::fs::write(path, bytes);
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

pub fn save_columns(c: &ColumnsState) {
    let mut prefs = load();
    prefs.columns = Some(c.clone());
    save(&prefs);
}

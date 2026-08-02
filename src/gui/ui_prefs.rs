//! Persisted GUI view state (`$config/oxdm/ui-prefs.json`). Currently
//! the main window's last size; loaded on launch, saved on resize.

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct UiPrefs {
    #[serde(default)]
    pub window: Option<WindowPrefs>,
    #[serde(default)]
    pub columns: Option<ColumnsState>,
}

/// Number of table columns = `windows::main::SortColumn` variants.
pub const COLS: usize = 7;

/// Main-table column widths + visibility, indexed by
/// `windows::main::SortColumn as usize` (Type, Name, Size, Status,
/// Speed, Eta, Date).
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

fn identity_order() -> [usize; COLS] {
    std::array::from_fn(|i| i)
}

/// `SortColumn::Name` — the one column that can never be hidden.
const NAME_IDX: usize = 1;

/// Minimum table column width. The design's `ResizableHeader` says 60px,
/// but its headers are plain text; ours reserve room for the sort
/// chevron and the ellipsis, so 60 left the shortest labels truncated on
/// arrival.
pub const COL_MIN_W: f32 = 75.0;

/// The Type column is not resizable and holds one fixed-width pill, so
/// its width is set by the header rather than the content: "TYPE" at
/// 11px is ~30px inside 8px of padding either side, and the 28px pill
/// needs 44. The sort chevron only appears while sorting *by* type, and
/// the header ellipsizes to make room for it then, so it does not have
/// to be reserved.
pub const TYPE_W: f32 = 58.0;

/// `SortColumn::Type` — fixed width, so it ignores any persisted value.
const TYPE_IDX: usize = 0;

/// Per-column minimum, indexed like `widths`.
const COL_MIN: [f32; COLS] = [
    TYPE_W, COL_MIN_W, COL_MIN_W, COL_MIN_W, COL_MIN_W, COL_MIN_W, COL_MIN_W,
];

impl Default for ColumnsState {
    fn default() -> Self {
        // Order = SortColumn (Type, Name, Size, Status, Speed, Eta,
        // Date). Design defaults: name 420, size 90, status 280,
        // speed 100, eta 90, date 130; Type is ours (the ext pill).
        Self {
            widths: [TYPE_W, 420.0, 90.0, 280.0, 100.0, 90.0, 130.0],
            hidden: [false; COLS],
            order: identity_order(),
        }
    }
}

impl ColumnsState {
    pub fn width(&self, idx: usize) -> f32 {
        if idx == TYPE_IDX {
            // Not resizable: a stored value would silently outrank the
            // constant and make changing it look like a no-op.
            return TYPE_W;
        }
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

pub fn save_columns(c: &ColumnsState) {
    let mut prefs = load();
    prefs.columns = Some(c.clone());
    save(&prefs);
}

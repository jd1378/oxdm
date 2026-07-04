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

/// Main-table column widths + visibility, indexed by
/// `windows::main::SortColumn as usize` (Name..Date). Same wire shape
/// as the egui-era struct so existing ui-prefs.json files load.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnsState {
    pub widths: [f32; 6],
    pub hidden: [bool; 6],
}

/// Minimum table column width (design `ResizableHeader` min 60px).
pub const COL_MIN_W: f32 = 60.0;

impl Default for ColumnsState {
    fn default() -> Self {
        // Order = SortColumn (Name, Size, Status, Speed, Eta, Date).
        // Design defaults: name 420, size 90, status 280, speed 100,
        // eta 90, date 130.
        Self {
            widths: [420.0, 90.0, 280.0, 100.0, 90.0, 130.0],
            hidden: [false; 6],
        }
    }
}

impl ColumnsState {
    pub fn width(&self, idx: usize) -> f32 {
        self.widths[idx].max(COL_MIN_W)
    }
    pub fn set_width(&mut self, idx: usize, w: f32) {
        self.widths[idx] = w.max(COL_MIN_W);
    }
    pub fn is_visible(&self, idx: usize) -> bool {
        !self.hidden[idx]
    }
    /// Name (idx 0) is always visible.
    pub fn toggle(&mut self, idx: usize) {
        if idx != 0 {
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

pub fn load() -> UiPrefs {
    let Some(path) = prefs_path() else {
        return UiPrefs::default();
    };
    std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
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

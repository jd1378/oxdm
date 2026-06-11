//! Persisted GUI view state (`$config/oxdm/ui-prefs.json`). Currently
//! the main window's last size; loaded on launch, saved on resize.

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct UiPrefs {
    #[serde(default)]
    pub window: Option<WindowPrefs>,
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

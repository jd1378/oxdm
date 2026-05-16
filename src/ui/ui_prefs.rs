//! Persistent GUI preferences. Stored as JSON at
//! `$config/oxdm/ui-prefs.json`. Survives across restarts.
//!
//! Holds user-tweakable view state: column widths/visibility, main
//! window size. Settings that affect daemon behavior live in
//! `domain::Settings` and travel over IPC instead.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ui::components::table::ColumnsState;

#[derive(Default, Serialize, Deserialize)]
pub struct UiPrefs {
    #[serde(default)]
    pub columns: Option<ColumnsState>,
    #[serde(default)]
    pub window: Option<WindowPrefs>,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct WindowPrefs {
    pub width: f32,
    pub height: f32,
}

pub fn path() -> Option<PathBuf> {
    let dir = dirs::data_dir().or_else(dirs::home_dir)?;
    Some(dir.join("oxdm").join("ui-prefs.json"))
}

pub fn load() -> UiPrefs {
    let Some(p) = path() else {
        return UiPrefs::default();
    };
    let bytes = match std::fs::read(&p) {
        Ok(b) => b,
        Err(e) => {
            // Missing file is the common case (first run) — only log
            // unexpected I/O errors.
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(path = %p.display(), error = %e, "ui-prefs: read failed");
            }
            return UiPrefs::default();
        }
    };
    // Parse leniently: accept whole-file deserialization first; fall
    // back to per-field extraction so a single corrupt section doesn't
    // wipe other valid fields.
    if let Ok(p) = serde_json::from_slice::<UiPrefs>(&bytes) {
        return p;
    }
    let value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(path = %p.display(), error = %e, "ui-prefs: corrupt JSON; recreating with defaults");
            backup_corrupt(&p, &bytes);
            let fresh = UiPrefs::default();
            save(&fresh);
            return fresh;
        }
    };
    let mut out = UiPrefs::default();
    if let Some(c) = value.get("columns") {
        match serde_json::from_value::<ColumnsState>(c.clone()) {
            Ok(c) => out.columns = Some(c),
            Err(e) => tracing::warn!(error = %e, "ui-prefs: ignoring corrupt columns"),
        }
    }
    if let Some(w) = value.get("window") {
        match serde_json::from_value::<WindowPrefs>(w.clone()) {
            Ok(w) => out.window = Some(w),
            Err(e) => tracing::warn!(error = %e, "ui-prefs: ignoring corrupt window"),
        }
    }
    out
}

fn backup_corrupt(path: &std::path::Path, bytes: &[u8]) {
    let bak = path.with_extension("json.bak");
    if let Err(e) = std::fs::write(&bak, bytes) {
        tracing::warn!(path = %bak.display(), error = %e, "ui-prefs: backup write failed");
    } else {
        tracing::info!(path = %bak.display(), "ui-prefs: corrupt file backed up");
    }
}

pub fn save(prefs: &UiPrefs) {
    let Some(p) = path() else { return };
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(prefs) {
        let _ = std::fs::write(&p, bytes);
    }
}

pub fn save_columns(c: &ColumnsState) {
    let mut prefs = load();
    prefs.columns = Some(c.clone());
    save(&prefs);
}

pub fn save_window(w: WindowPrefs) {
    let mut prefs = load();
    prefs.window = Some(w);
    save(&prefs);
}

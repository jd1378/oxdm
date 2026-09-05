//! Persisted GUI view state under `$config/oxdm/ui/`: each window's
//! last size, the main table's columns, and which sidebar entry the
//! main window was looking at. Loaded on launch, saved as each changes.
//!
//! One file per writer. Every window kind is its own process, and a
//! save is a read, a change and a write, so a file two processes both
//! saved to would carry one's stale copy over the other's change. The
//! main window owns `main.json`, the other windows own a file each, and
//! the chrome flag, which any window may refresh, is a single value
//! written whole. Nothing is shared, so nothing needs a lock.
//!
//! `ui-prefs.json`, which held all of this in one file, is read when a
//! window's own file does not exist yet, and never written.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::domain::{Category, QueueId};

/// What the main window remembers.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct MainPrefs {
    #[serde(default)]
    pub window: Option<WindowPrefs>,
    /// Dropped when it cannot be read: the arrays are fixed-length, so a
    /// file from a build with a different column set fails to parse, and
    /// a stale table layout must not cost the user their window size.
    #[serde(default, deserialize_with = "columns_or_none")]
    pub columns: Option<ColumnsState>,
    /// The sidebar entry the window was on when it last closed. `None`
    /// = never saved, or a file this build cannot read; the window then
    /// falls back to its default (the built-in queue).
    #[serde(default, deserialize_with = "sidebar_or_none")]
    pub sidebar: Option<SidebarPref>,
}

/// What a window that remembers only its size keeps.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct SizeOnly {
    #[serde(default)]
    window: Option<WindowPrefs>,
}

/// Cached mirror of `Settings.custom_window_chrome`. The setting itself
/// stays the source of truth; it is copied here because a window must
/// decide on decorations when it is created, which is before the daemon
/// connection that carries `Settings` exists. `None` = never seen,
/// treated as the default (native chrome).
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct ChromePrefs {
    #[serde(default)]
    custom_window_chrome: Option<bool>,
}

/// The one file every preference used to live in.
#[derive(Debug, Default, Clone, Deserialize)]
struct LegacyPrefs {
    #[serde(default)]
    window: Option<WindowPrefs>,
    #[serde(default, deserialize_with = "columns_or_none")]
    columns: Option<ColumnsState>,
    #[serde(default)]
    custom_window_chrome: Option<bool>,
    #[serde(default, deserialize_with = "sidebar_or_none")]
    sidebar: Option<SidebarPref>,
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

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowPrefs {
    pub width: f32,
    pub height: f32,
}

/// The windows whose size is remembered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowSlot {
    Main,
    Queues,
    Settings,
}

impl WindowSlot {
    fn file(self) -> &'static str {
        match self {
            WindowSlot::Main => "main.json",
            WindowSlot::Queues => "queues.json",
            WindowSlot::Settings => "settings.json",
        }
    }
}

const CHROME_FILE: &str = "chrome.json";
const LEGACY_FILE: &str = "ui-prefs.json";

/// The directory the per-window files live in.
fn prefs_dir() -> Option<std::path::PathBuf> {
    Some(dirs::config_dir()?.join("oxdm").join("ui"))
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

/// Everything the on-disk files are read and written through, so the
/// tests can point it at a scratch directory.
struct Files {
    dir: std::path::PathBuf,
}

impl Files {
    fn at_config() -> Option<Self> {
        Some(Self { dir: prefs_dir()? })
    }

    fn read<T: DeserializeOwned + Default>(&self, name: &str) -> Option<T> {
        let bytes = std::fs::read(self.dir.join(name)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// The legacy file sits one level up, beside the directory.
    fn legacy(&self) -> LegacyPrefs {
        self.dir
            .parent()
            .and_then(|d| std::fs::read(d.join(LEGACY_FILE)).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Written beside the target and renamed over it, so a reader sees
    /// the old file or the new one and never half of one.
    ///
    /// Deliberately not fsynced: losing a window size or a sidebar view
    /// to a power cut costs a re-tune, which is not worth a disk flush
    /// on every sidebar click.
    fn write<T: Serialize>(&self, name: &str, value: &T) {
        let _ = std::fs::create_dir_all(&self.dir);
        let Ok(bytes) = serde_json::to_vec_pretty(value) else {
            return;
        };
        let path = self.dir.join(name);
        let tmp = self.dir.join(format!("{name}.{}.tmp", std::process::id()));
        if std::fs::write(&tmp, &bytes).is_err() || std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }

    fn main(&self) -> MainPrefs {
        let mut prefs = self.read(WindowSlot::Main.file()).unwrap_or_else(|| {
            let old = self.legacy();
            MainPrefs {
                window: old.window,
                columns: old.columns,
                sidebar: old.sidebar,
            }
        });
        if let Some(cols) = &mut prefs.columns
            && !cols.order_is_sane()
        {
            cols.order = identity_order();
        }
        prefs
    }

    fn window(&self, slot: WindowSlot) -> Option<WindowPrefs> {
        match slot {
            WindowSlot::Main => self.main().window,
            _ => self.read::<SizeOnly>(slot.file())?.window,
        }
    }

    fn save_window(&self, slot: WindowSlot, w: WindowPrefs) {
        match slot {
            WindowSlot::Main => self.update_main(|p| {
                let changed = p.window != Some(w);
                p.window = Some(w);
                changed
            }),
            _ => self.write(slot.file(), &SizeOnly { window: Some(w) }),
        }
    }

    /// A change to the main window's file. Only that window's process
    /// writes it, and its updates run one after another on the update
    /// loop, so the read and the write cannot interleave with another.
    /// `f` says whether anything changed; an untouched file is not
    /// rewritten.
    fn update_main(&self, f: impl FnOnce(&mut MainPrefs) -> bool) {
        let mut prefs = self.main();
        if f(&mut prefs) {
            self.write(WindowSlot::Main.file(), &prefs);
        }
    }

    fn custom_window_chrome(&self) -> Option<bool> {
        match self.read::<ChromePrefs>(CHROME_FILE) {
            Some(c) => c.custom_window_chrome,
            None => self.legacy().custom_window_chrome,
        }
    }
}

pub fn load_main() -> MainPrefs {
    Files::at_config().map(|f| f.main()).unwrap_or_default()
}

pub fn load_window(slot: WindowSlot) -> Option<WindowPrefs> {
    Files::at_config()?.window(slot)
}

pub fn save_window(slot: WindowSlot, w: WindowPrefs) {
    if let Some(f) = Files::at_config() {
        f.save_window(slot, w);
    }
}

pub fn custom_window_chrome() -> Option<bool> {
    Files::at_config()?.custom_window_chrome()
}

/// Refresh the cached chrome preference from the daemon's settings.
/// Costs nothing while the two agree, which is every case but the
/// snapshot right after the user flips the toggle.
pub fn sync_custom_window_chrome(v: bool) {
    if v == crate::gui::chrome::titlebar::use_custom() {
        return;
    }
    if let Some(f) = Files::at_config() {
        f.write(
            CHROME_FILE,
            &ChromePrefs {
                custom_window_chrome: Some(v),
            },
        );
    }
}

pub fn save_sidebar(s: SidebarPref) {
    if let Some(f) = Files::at_config() {
        f.update_main(|p| {
            let changed = p.sidebar != Some(s);
            p.sidebar = Some(s);
            changed
        });
    }
}

pub fn save_columns(c: &ColumnsState) {
    if let Some(f) = Files::at_config() {
        f.update_main(|p| {
            p.columns = Some(c.clone());
            true
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(dir: &tempfile::TempDir) -> Files {
        Files {
            dir: dir.path().join("oxdm").join("ui"),
        }
    }

    /// The file is replaced whole, and the scratch copy never survives
    /// the save that made it.
    #[test]
    fn a_save_replaces_the_file_and_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let f = files(&dir);
        std::fs::create_dir_all(&f.dir).unwrap();
        std::fs::write(f.dir.join("queues.json"), b"{ truncated").unwrap();

        let size = WindowPrefs {
            width: 800.0,
            height: 600.0,
        };
        f.save_window(WindowSlot::Queues, size);

        assert_eq!(f.window(WindowSlot::Queues), Some(size));
        let leftovers: Vec<_> = std::fs::read_dir(&f.dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.file_name()))
            .filter(|n| n.to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "left {leftovers:?}");
    }

    /// Each window writes its own file, so one saving never touches
    /// what another saved.
    #[test]
    fn every_window_keeps_its_own_size() {
        let dir = tempfile::tempdir().unwrap();
        let f = files(&dir);
        let big = WindowPrefs {
            width: 1000.0,
            height: 700.0,
        };
        let small = WindowPrefs {
            width: 700.0,
            height: 500.0,
        };
        f.save_window(WindowSlot::Queues, big);
        f.save_window(WindowSlot::Settings, small);
        f.save_sidebar_for_test(SidebarPref::All);
        assert_eq!(f.window(WindowSlot::Main), None);
        assert_eq!(f.window(WindowSlot::Queues), Some(big));
        assert_eq!(f.window(WindowSlot::Settings), Some(small));
        assert_eq!(f.main().sidebar, Some(SidebarPref::All));
    }

    /// A file from before the split still restores the main window and
    /// the chrome flag, and the first save carries it over rather than
    /// starting from nothing.
    #[test]
    fn the_old_single_file_is_read_until_a_window_saves() {
        let dir = tempfile::tempdir().unwrap();
        let f = files(&dir);
        std::fs::create_dir_all(f.dir.parent().unwrap()).unwrap();
        std::fs::write(
            f.dir.parent().unwrap().join(LEGACY_FILE),
            r#"{"window":{"width":900.0,"height":600.0},"custom_window_chrome":true,
                "sidebar":{"view":"all"}}"#,
        )
        .unwrap();
        let old_size = WindowPrefs {
            width: 900.0,
            height: 600.0,
        };
        assert_eq!(f.window(WindowSlot::Main), Some(old_size));
        assert_eq!(f.window(WindowSlot::Queues), None);
        assert_eq!(f.custom_window_chrome(), Some(true));

        f.update_main(|p| {
            p.columns = Some(ColumnsState::default());
            true
        });
        let back = f.main();
        assert_eq!(back.window, Some(old_size), "seeded from the old file");
        assert_eq!(back.sidebar, Some(SidebarPref::All));
        assert!(back.columns.is_some());
    }

    impl Files {
        fn save_sidebar_for_test(&self, s: SidebarPref) {
            self.update_main(|p| {
                p.sidebar = Some(s);
                true
            });
        }
    }
}

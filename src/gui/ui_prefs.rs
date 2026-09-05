//! Persisted GUI view state (`$config/oxdm/ui-prefs.json`): each
//! window's last size, the main table's columns, and which sidebar entry
//! the main window was looking at. Loaded on launch, saved as each
//! changes, under a file lock so the windows (each its own process) do
//! not overwrite one another's saves.

use serde::{Deserialize, Serialize};

use crate::domain::{Category, QueueId};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct UiPrefs {
    /// The main window's last size. Named as it always was, so files
    /// written before the other windows had one still restore it.
    #[serde(default)]
    pub window: Option<WindowPrefs>,
    /// Last size of every other window that remembers one.
    #[serde(default)]
    pub queues_window: Option<WindowPrefs>,
    #[serde(default)]
    pub settings_window: Option<WindowPrefs>,
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

impl UiPrefs {
    pub fn window_for(&self, slot: WindowSlot) -> Option<WindowPrefs> {
        match slot {
            WindowSlot::Main => self.window,
            WindowSlot::Queues => self.queues_window,
            WindowSlot::Settings => self.settings_window,
        }
    }

    fn window_for_mut(&mut self, slot: WindowSlot) -> &mut Option<WindowPrefs> {
        match slot {
            WindowSlot::Main => &mut self.window,
            WindowSlot::Queues => &mut self.queues_window,
            WindowSlot::Settings => &mut self.settings_window,
        }
    }
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
    load_at(&path)
}

fn load_at(path: &std::path::Path) -> UiPrefs {
    let mut prefs: UiPrefs = std::fs::read(path)
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

/// Change one thing in the file, under a lock shared by every window.
///
/// Each window kind is its own process, and two can be saving at once:
/// the main window its sidebar, Settings its size. Each save is a read,
/// a change and a write, so without the lock the second write carried
/// the first one's stale copy and dropped its change. The lock is a
/// sibling file held for the duration; `f` returns whether anything
/// changed, and an untouched file is not rewritten.
pub fn update(f: impl FnOnce(&mut UiPrefs) -> bool) {
    let Some(path) = prefs_path() else { return };
    update_at(&path, f);
}

fn update_at(path: &std::path::Path, f: impl FnOnce(&mut UiPrefs) -> bool) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Held to the end of the function. A lock that cannot be taken is
    // not a reason to lose the change: the save goes ahead unlocked,
    // which is what it always did.
    let _lock = PrefsLock::take(path);
    let mut prefs = load_at(path);
    if f(&mut prefs) {
        save_at(path, &prefs);
    }
}

/// An exclusive advisory lock on `<prefs>.lock`, released on drop.
struct PrefsLock(std::fs::File);

impl PrefsLock {
    fn take(prefs_path: &std::path::Path) -> Option<Self> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(prefs_path.with_extension("lock"))
            .ok()?;
        file.lock().ok()?;
        Some(Self(file))
    }
}

impl Drop for PrefsLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// Written beside the target and renamed over it, so a reader sees the
/// old file or the new one and never half of one.
///
/// Deliberately not fsynced: this file holds window size, columns and
/// the sidebar view. Losing it to a power cut costs a re-tune, which is
/// not worth a disk flush on every sidebar click.
fn save_at(path: &std::path::Path, prefs: &UiPrefs) {
    let Ok(bytes) = serde_json::to_vec_pretty(prefs) else {
        return;
    };
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    if std::fs::write(&tmp, &bytes).is_err() || std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

pub fn save_window(slot: WindowSlot, w: WindowPrefs) {
    update(|prefs| {
        let cur = prefs.window_for_mut(slot);
        let changed = *cur != Some(w);
        *cur = Some(w);
        changed
    });
}

/// Refresh the cached chrome preference from the daemon's settings.
/// Costs nothing while the two agree, which is every case but the
/// snapshot right after the user flips the toggle.
pub fn sync_custom_window_chrome(v: bool) {
    if v == crate::gui::chrome::titlebar::use_custom() {
        return;
    }
    update(|prefs| {
        let changed = prefs.custom_window_chrome != Some(v);
        prefs.custom_window_chrome = Some(v);
        changed
    });
}

pub fn save_sidebar(s: SidebarPref) {
    update(|prefs| {
        let changed = prefs.sidebar != Some(s);
        prefs.sidebar = Some(s);
        changed
    });
}

pub fn save_columns(c: &ColumnsState) {
    update(|prefs| {
        prefs.columns = Some(c.clone());
        true
    });
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

        update_at(&path, |prefs| {
            prefs.custom_window_chrome = Some(true);
            true
        });

        let back: UiPrefs = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(back.custom_window_chrome, Some(true));
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.file_name()))
            .filter(|n| n.to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "left {leftovers:?}");
    }

    /// Two windows saving different things at once both land: the
    /// lock serialises the read-change-write, so neither carries the
    /// other's stale copy over its change.
    #[test]
    fn saves_from_two_windows_do_not_drop_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ui-prefs.json");
        let size = WindowPrefs {
            width: 800.0,
            height: 600.0,
        };
        let workers: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for _ in 0..25 {
                        update_at(&path, |prefs| {
                            match i % 2 {
                                0 => *prefs.window_for_mut(WindowSlot::Queues) = Some(size),
                                _ => prefs.sidebar = Some(SidebarPref::All),
                            }
                            true
                        });
                    }
                })
            })
            .collect();
        for w in workers {
            w.join().unwrap();
        }
        let back = load_at(&path);
        assert_eq!(back.window_for(WindowSlot::Queues), Some(size));
        assert_eq!(back.sidebar, Some(SidebarPref::All));
    }

    /// Each window's size lives in its own slot, and the main window's
    /// keeps the field it always had.
    #[test]
    fn every_window_keeps_its_own_size() {
        let mut prefs = UiPrefs::default();
        let big = WindowPrefs {
            width: 1000.0,
            height: 700.0,
        };
        let small = WindowPrefs {
            width: 700.0,
            height: 500.0,
        };
        *prefs.window_for_mut(WindowSlot::Queues) = Some(big);
        *prefs.window_for_mut(WindowSlot::Settings) = Some(small);
        assert_eq!(prefs.window_for(WindowSlot::Main), None);
        assert_eq!(prefs.window_for(WindowSlot::Queues), Some(big));
        assert_eq!(prefs.window_for(WindowSlot::Settings), Some(small));

        let old: UiPrefs =
            serde_json::from_str(r#"{"window":{"width":900.0,"height":600.0}}"#).unwrap();
        assert_eq!(
            old.window_for(WindowSlot::Main),
            Some(WindowPrefs {
                width: 900.0,
                height: 600.0
            })
        );
        assert_eq!(old.window_for(WindowSlot::Queues), None);
    }
}

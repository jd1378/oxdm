use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

use crate::domain::{Category, ProxyAdv, ProxyMode, QueueId};

/// oxdm-level settings. Wraps every `odl::config::Config` field plus
/// UI-only preferences. The data layer translates this into an
/// `odl::Config` whenever it changes.
///
/// One Settings struct = single source of truth that Settings UI binds
/// to. Defaults match `odl::config::Config::default`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    // ── odl pass-through ────────────────────────────────────────────
    /// Working directory for in-flight downloads — `metadata.pb` and
    /// every `.part` for every job live here, keyed by job id. Stable
    /// across save-folder changes so a user retargeting the final
    /// destination of an in-flight download does not orphan its
    /// partial state. Defaults to the platform data dir.
    #[serde(default = "default_work_dir")]
    pub work_dir: PathBuf,
    /// `None` = "Determine automatically": the UI/runtime applies a
    /// size-based heuristic at job creation. `Some(n)` = hard cap the
    /// user picked in Settings.
    pub max_connections: Option<u64>,
    pub max_concurrent_downloads: usize,
    pub max_retries: u32,
    #[serde(with = "humantime_serde")]
    pub wait_between_retries: Duration,
    pub n_fixed_retries: u32,
    pub user_agent: Option<String>,
    pub randomize_user_agent: bool,
    /// Global proxy, in parts — the same shape a job's own proxy uses,
    /// assembled into a URL only where it is handed to odl. Storing the
    /// assembled URL instead would mean parsing it back apart for every
    /// edit, and percent-decoding a username to show it in a field.
    /// `ProxyAdv::password` is UI-side scratch: `update_settings` routes
    /// it onto `enc_proxy_password` and never persists the plaintext.
    #[serde(default = "default_proxy")]
    pub proxy: ProxyAdv,
    /// The proxy password, encrypted under the app's master key.
    #[serde(default)]
    pub enc_proxy_password: Option<String>,
    pub use_server_time: bool,
    /// Let odl subdivide a long-running part mid-download when other
    /// connections have gone idle, instead of leaving the part layout
    /// as it was decided at the start.
    ///
    /// On by default, and worth leaving on: the tail of a download is
    /// otherwise one connection finishing alone while the rest sit
    /// there. Off is for servers that count reconnects, or anyone who
    /// wants the segment table to stay as they set it.
    #[serde(default = "yes_default")]
    pub dynamic_split: bool,
    pub accept_invalid_certs: bool,
    pub speed_limit: Option<u64>,
    #[serde(with = "humantime_serde")]
    pub connect_timeout: Option<Duration>,
    pub headers: IndexMap<String, String>,

    // ── oxdm-only ──────────────────────────────────────────────────
    /// IPC capture port for the browser-extension WebSocket bridge.
    pub ipc_port: u16,
    /// Auth token the extension presents in its first WebSocket frame.
    /// Empty string = generate on next boot. Users can regenerate from
    /// Settings; the value lives here so it survives restarts.
    #[serde(default)]
    pub ext_token: String,
    /// Suppress the Remove confirmation for incomplete downloads when
    /// the user has previously checked "Don't ask again." File data is
    /// still purged unconditionally — only the prompt is skipped.
    #[serde(default = "yes_default")]
    pub remove_confirm_incomplete: bool,
    /// Suppress the Remove confirmation for completed downloads. The
    /// "also delete file on disk" choice is **never** remembered, even
    /// when this is `false`; that decision is per-prompt by design.
    #[serde(default = "yes_default")]
    pub remove_confirm_completed: bool,
    /// Confirm the toolbar's Clean action, which removes every completed
    /// entry at once. Separate from `remove_confirm_completed`: that one
    /// answers for a selection the user made by hand, this one for a set
    /// they never saw listed.
    #[serde(default = "yes_default")]
    pub remove_confirm_clean: bool,
    /// Watch what each download has on disk — a finished one's saved
    /// file, an unfinished one's cached parts — and drop the entry from
    /// the list once that is no longer there.
    ///
    /// Off by default: the list is also a history, and a user who files
    /// their downloads away by hand has not asked to forget them. The
    /// check cannot tell a move from a delete or a rename — all it knows
    /// is that nothing is at the recorded path any more.
    #[serde(default)]
    pub forget_moved_files: bool,
    /// Warn when a kernel limit stops the filesystem watcher. On by
    /// default because the failure is otherwise invisible — the watch
    /// setting still reads "on" while nothing is being watched — and
    /// turned off by the dialog's own "Don't warn again", for a user
    /// who has decided to live with it.
    #[serde(default = "yes_default")]
    pub warn_watch_limit: bool,
    /// Pause running downloads while the connection is metered (cellular
    /// or a phone hotspot), and resume them when it is not.
    #[serde(default = "yes_default")]
    pub pause_on_metered: bool,
    /// Pause running downloads while the battery is low and discharging.
    #[serde(default)]
    pub pause_on_low_battery: bool,
    /// If true, app starts on system login (handled by platform code).
    pub start_at_login: bool,
    /// If true, launching the app starts hidden in the tray instead of
    /// showing the main window. Default: false (main window opens on
    /// every launch).
    #[serde(default)]
    pub start_to_tray: bool,
    /// If true, finishing a download surfaces the per-job window with a
    /// "Download complete" view. Owned by Settings → Notifications; the
    /// completion view itself carries no opt-out, so there is one place
    /// this is answered. Per-job `OnCompletion::show_dialog` still
    /// gates, so users can opt out individual jobs while keeping the
    /// global on.
    #[serde(default = "yes_default")]
    pub show_complete_dialog: bool,
    /// System notification when a download completes. Independent of
    /// `show_complete_dialog`: the dialog is a window that wants
    /// attention, the notification only reports. Off by default — the
    /// dialog already covers the event, and two surfaces for one thing
    /// is noise until the user asks for it.
    #[serde(default)]
    pub notify_complete: bool,
    /// If true, a failed download surfaces the per-job window on its
    /// error view. A download parked by a conflict is not a failure and
    /// keeps its own flow.
    #[serde(default = "yes_default")]
    pub show_failed_dialog: bool,
    /// System notification when a download fails. Off by default, like
    /// the other notifications.
    #[serde(default)]
    pub notify_failed: bool,
    /// If true, a download stopped on a conflict surfaces its window on
    /// the question. Its own pair of toggles rather than the failure
    /// ones: a conflict is not a failure — the bytes are fine and the
    /// download continues the moment it is answered — and it is the one
    /// stopped state where doing nothing means it never finishes.
    #[serde(default = "yes_default")]
    pub show_conflict_dialog: bool,
    /// System notification when a download stops on a conflict. On by
    /// default, unlike the others: nothing else is coming, and a
    /// download that quietly waits forever is worse than a
    /// notification nobody needed.
    #[serde(default = "yes_default")]
    pub notify_conflict: bool,
    /// Look for a new release without being asked: once at startup,
    /// then at most weekly and only while the machine is idle. On by
    /// default — a download manager that silently stays on an old
    /// version is the worse failure, and the check is one small JSON
    /// document.
    #[serde(default = "yes_default")]
    pub auto_check_updates: bool,
    /// How an automatic check announces what it found. Exclusive by
    /// construction — see [`Settings::update_surface`]: the dialog is
    /// the whole report, so a notification alongside it would be the
    /// same news twice. A manual check from About answers in About and
    /// raises neither.
    ///
    /// The notification is the default of the two. A new version is
    /// news, not a task: it can wait until the user is between things,
    /// and a window that takes focus over whatever they are doing is
    /// too much for something that will still be true tomorrow.
    /// Pressing the notification opens the window for anyone who wants
    /// it now.
    #[serde(default)]
    pub show_update_dialog: bool,
    #[serde(default = "yes_default")]
    pub notify_update: bool,
    /// Auto-update feed URL. Feed is a JSON document of shape
    /// `{ "version": "x.y.z", "url": "...", "notes": "...", "sha256": "..." }`.
    ///
    /// Empty means the built-in feed for this build, which is resolved
    /// when a check runs rather than stored — see
    /// `data::update_channel::built_in_feed_url`. It has to be decided
    /// then, because it depends on how the app is *running*: an
    /// AppImage updates itself with an AppImage, an installed build
    /// with a plain executable, and a user can move between the two
    /// without their settings knowing.
    #[serde(default)]
    pub update_feed_url: String,
    /// UI theme.
    pub theme: Theme,
    /// Honour reduce-motion. When `true`, animation sites bypass
    /// `animate_value_with_time` and paint the final value directly.
    /// Mirrors CSS `prefers-reduced-motion: reduce`. See
    /// `design/handoff/16_animations.md §4` for per-animation rules.
    #[serde(default)]
    pub reduce_motion: bool,
    /// Draw oxdm's own title bar, frame and resize grips instead of the
    /// desktop's. Off by default: native decorations match whatever
    /// window management the user already has (tiling, snapping,
    /// shortcuts, accessibility). A window picks this up when it opens,
    /// since decorations are fixed at window creation.
    #[serde(default)]
    pub custom_window_chrome: bool,
    /// Per-CSS-variable overrides applied on top of the active theme.
    /// Keys must be valid CSS custom-property names *without* the
    /// leading `--` (e.g. `accent`, `bg`, `text`). Values are any valid
    /// CSS color string. Empty by default — users opt in from Settings.
    #[serde(default)]
    pub theme_overrides: IndexMap<String, String>,
    /// Per-category extension overrides. A `Category` present here
    /// fully replaces its `Category::default_extensions` list (no merge).
    /// A `Category` absent from this map keeps the built-in defaults.
    /// Each extension string must be the bare suffix — lowercase ASCII,
    /// no leading dot (e.g. `"zip"`, not `".ZIP"` or `"Zip"`). The
    /// classifier compares case-insensitively, so casing is normalised
    /// at write time rather than enforced at read time.
    #[serde(default)]
    pub category_extensions: IndexMap<Category, Vec<String>>,
    /// Where each category saves. This is the *only* download-location
    /// setting: there is no separate global default, because every file
    /// has a category and `Other` is the catch-all every unclassified
    /// file falls into. Populated for all categories on first run and
    /// edited per category from there.
    ///
    /// A `Category` absent here (or mapped to an empty path) resolves to
    /// `default_category_folder`. Read through
    /// [`Settings::category_folder`] rather than directly, so every
    /// caller sees the same resolved path the user is shown. Applied on
    /// the non-interactive capture path (`add_from_capture`); the Add
    /// dialog prefills client-side so an explicit user choice always
    /// wins.
    #[serde(default)]
    pub category_folders: IndexMap<Category, PathBuf>,
    /// Per-category default queue. Same application rules as
    /// `category_folders`. A stale id (queue since deleted) is ignored
    /// at apply time — the job stays in the Main queue.
    #[serde(default)]
    pub category_queues: IndexMap<Category, QueueId>,
    /// True once the first-run welcome overlay has been shown and
    /// dismissed. The GUI sets it via `UpdateSettings` on either
    /// dismissal path.
    #[serde(default)]
    pub first_run_seen: bool,

    // ── browser-extension capture rules ─────────────────────────────
    // Single source of truth for which downloads the extension hands
    // to oxdm. Extension fetches via `get_capture_rules` on connect.
    // Authoring lives here so users do not have to maintain parallel
    // lists in both apps.
    /// Minimum reported size (bytes) before oxdm wants the capture.
    /// 0 disables the threshold.
    #[serde(default)]
    pub capture_min_size: u64,
    /// Hostnames whose downloads stay with the browser. A bare host
    /// matches itself and any subdomain.
    #[serde(default)]
    pub capture_skip_domains: Vec<String>,
    /// File extensions (bare, lowercase, no dot) excluded from capture.
    #[serde(default = "default_skip_extensions")]
    pub capture_skip_extensions: Vec<String>,
    /// MIME prefixes excluded from capture (e.g. `text/html`).
    #[serde(default = "default_skip_mime_prefixes")]
    pub capture_skip_mime_prefixes: Vec<String>,
    /// Optional allowlist — when non-empty, only URLs ending in one of
    /// these extensions are captured. Skip lists still subtract.
    #[serde(default)]
    pub capture_allow_extensions: Vec<String>,
    /// Optional allowlist — when non-empty, only downloads whose MIME
    /// starts with one of these prefixes are captured.
    #[serde(default)]
    pub capture_allow_mime_prefixes: Vec<String>,
}

fn default_skip_extensions() -> Vec<String> {
    ["html", "htm", "php", "asp", "aspx", "jsp"]
        .into_iter()
        .map(String::from)
        .collect()
}

fn default_skip_mime_prefixes() -> Vec<String> {
    ["text/html", "application/xhtml"]
        .into_iter()
        .map(String::from)
        .collect()
}

/// How a newly found version is announced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateSurface {
    /// Nothing is raised: automatic checks are off, or the user turned
    /// both surfaces off and will find out from About.
    Silent,
    /// Open About on the update, where the install lives.
    Dialog,
    /// A desktop notification that opens that same window when pressed.
    Notification,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    System,
    Light,
    Dark,
    Warm,
}

/// "Unlimited" concurrent downloads. Not a sentinel the scheduler knows
/// about — a ceiling no real queue reaches, so the limit simply never
/// binds while the field stays an ordinary number.
pub const UNLIMITED_CONCURRENT: usize = 9999;

/// The OS download folder, detected fresh. Only ever used to seed the
/// category folders on first run (and to repair a settings row that
/// somehow lost one) — nothing reads it as a live setting, so a user who
/// retargets their categories is never silently pulled back here.
pub fn detected_download_dir() -> PathBuf {
    dirs::download_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Where a category saves when the user has not retargeted it: a
/// same-named subfolder of `base`, except `Other` — the catch-all keeps
/// unsorted files in the download folder itself rather than pushing them
/// into a bin the user never asked for.
pub fn default_category_folder(base: &std::path::Path, cat: Category) -> PathBuf {
    match cat {
        Category::Other => base.to_path_buf(),
        _ => base.join(cat.label()),
    }
}

/// Every category materialised against `base`, in display order. New
/// installs store these outright so the folder a user sees is the folder
/// that is saved, with nothing derived behind their back.
pub fn default_category_folders(base: &std::path::Path) -> IndexMap<Category, PathBuf> {
    Category::ALL_ASSIGNABLE
        .iter()
        .map(|c| (*c, default_category_folder(base, *c)))
        .collect()
}

impl Settings {
    /// Resolved save folder for `cat`: the stored value when there is
    /// one, else the derived default. The single reader every call site
    /// goes through, so the Settings pane, the Add dialog and the
    /// capture path cannot disagree about where a file lands.
    pub fn category_folder(&self, cat: Category) -> PathBuf {
        self.category_folders
            .get(&cat)
            .filter(|p| !p.as_os_str().is_empty())
            .cloned()
            .unwrap_or_else(|| default_category_folder(&detected_download_dir(), cat))
    }

    /// What an automatic update check does with a version it found.
    ///
    /// The two toggles are mutually locked in Settings, so only one can
    /// be on; a settings row hand-edited to set both resolves to the
    /// dialog, which is the surface that can actually install it.
    /// Nothing is raised when automatic checks are off — the only
    /// checks left are the ones the user ran from About, and About is
    /// already in front of them.
    pub fn update_surface(&self) -> UpdateSurface {
        if !self.auto_check_updates {
            UpdateSurface::Silent
        } else if self.show_update_dialog {
            UpdateSurface::Dialog
        } else if self.notify_update {
            UpdateSurface::Notification
        } else {
            UpdateSurface::Silent
        }
    }

    /// Save folder for anything with no category yet — an empty Add
    /// dialog, a batch, odl's own config default. That is exactly what
    /// `Other` means, so it answers rather than a second global setting
    /// that could disagree with it.
    pub fn fallback_dir(&self) -> PathBuf {
        self.category_folder(Category::Other)
    }

    /// Every folder oxdm saves into. A category folder is created on
    /// first use, so this is how a typed path can be recognised as a
    /// folder before anything exists at it — see
    /// [`crate::domain::save_path::Resolver`].
    pub fn known_dirs(&self) -> Vec<PathBuf> {
        Category::ALL_ASSIGNABLE
            .iter()
            .map(|c| self.category_folder(*c))
            .collect()
    }
}

/// A global proxy has nothing to inherit from: unset means "System",
/// which is reqwest reading the standard proxy environment variables.
fn default_proxy() -> ProxyAdv {
    ProxyAdv {
        mode: ProxyMode::System,
        ..ProxyAdv::default()
    }
}

fn yes_default() -> bool {
    true
}

pub fn default_work_dir() -> PathBuf {
    dirs::data_local_dir()
        .or_else(dirs::data_dir)
        .or_else(dirs::config_dir)
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("oxdm")
        .join("work")
}

mod humantime_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    pub fn serialize<S, T>(value: &T, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        T: AsHumantime,
    {
        value.serialize_humantime(serializer)
    }

    pub fn deserialize<'de, D, T>(deserializer: D) -> Result<T, D::Error>
    where
        D: Deserializer<'de>,
        T: FromHumantime,
    {
        T::from_humantime(deserializer)
    }

    pub trait AsHumantime {
        fn serialize_humantime<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error>;
    }
    pub trait FromHumantime: Sized {
        fn from_humantime<'de, D: Deserializer<'de>>(d: D) -> Result<Self, D::Error>;
    }

    impl AsHumantime for Duration {
        fn serialize_humantime<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            humantime::format_duration(*self).to_string().serialize(s)
        }
    }
    impl FromHumantime for Duration {
        fn from_humantime<'de, D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            let s = String::deserialize(d)?;
            humantime::parse_duration(&s).map_err(serde::de::Error::custom)
        }
    }

    impl AsHumantime for Option<Duration> {
        fn serialize_humantime<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            match self {
                Some(d) => humantime::format_duration(*d).to_string().serialize(s),
                None => s.serialize_none(),
            }
        }
    }
    impl FromHumantime for Option<Duration> {
        fn from_humantime<'de, D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            let s = Option::<String>::deserialize(d)?;
            s.map(|s| humantime::parse_duration(&s).map_err(serde::de::Error::custom))
                .transpose()
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            work_dir: default_work_dir(),
            max_connections: None,
            max_concurrent_downloads: UNLIMITED_CONCURRENT,
            max_retries: 3,
            wait_between_retries: Duration::from_millis(700),
            n_fixed_retries: 3,
            user_agent: None,
            randomize_user_agent: false,
            proxy: default_proxy(),
            enc_proxy_password: None,
            use_server_time: false,
            dynamic_split: true,
            accept_invalid_certs: false,
            speed_limit: None,
            connect_timeout: Some(Duration::from_secs(5)),
            headers: IndexMap::new(),
            ipc_port: 27812,
            ext_token: String::new(),
            remove_confirm_incomplete: true,
            remove_confirm_completed: true,
            remove_confirm_clean: true,
            forget_moved_files: false,
            warn_watch_limit: true,
            pause_on_metered: true,
            pause_on_low_battery: false,
            start_at_login: false,
            start_to_tray: false,
            show_complete_dialog: true,
            notify_complete: false,
            show_failed_dialog: true,
            notify_failed: false,
            show_conflict_dialog: true,
            notify_conflict: true,
            auto_check_updates: true,
            show_update_dialog: false,
            notify_update: true,
            update_feed_url: String::new(),
            theme: Theme::System,
            reduce_motion: false,
            custom_window_chrome: false,
            theme_overrides: IndexMap::new(),
            category_extensions: IndexMap::new(),
            category_folders: default_category_folders(&detected_download_dir()),
            category_queues: IndexMap::new(),
            first_run_seen: false,
            capture_min_size: 0,
            capture_skip_domains: Vec::new(),
            capture_skip_extensions: default_skip_extensions(),
            capture_skip_mime_prefixes: default_skip_mime_prefixes(),
            capture_allow_extensions: Vec::new(),
            capture_allow_mime_prefixes: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// The catch-all keeps unsorted files in the download folder itself
    /// — no `/Other` bin the user never asked for. Named categories do
    /// get their subfolder.
    #[test]
    fn other_defaults_to_the_base_folder_itself() {
        let base = Path::new("/home/u/Downloads");
        assert_eq!(
            default_category_folder(base, Category::Other),
            PathBuf::from("/home/u/Downloads")
        );
        assert_eq!(
            default_category_folder(base, Category::Videos),
            PathBuf::from("/home/u/Downloads/Videos")
        );
    }

    #[test]
    fn one_update_surface_at_a_time() {
        // A new version is news, not a task: it reports rather than
        // taking focus, unless the user asks for the window.
        let s = Settings::default();
        assert_eq!(s.update_surface(), UpdateSurface::Notification);

        let dialog = Settings {
            show_update_dialog: true,
            notify_update: false,
            ..Settings::default()
        };
        assert_eq!(dialog.update_surface(), UpdateSurface::Dialog);

        // Both on is not reachable from Settings, but a hand-edited row
        // resolves to the surface that can install what it announces.
        let both = Settings {
            show_update_dialog: true,
            ..Settings::default()
        };
        assert_eq!(both.update_surface(), UpdateSurface::Dialog);

        let neither = Settings {
            notify_update: false,
            ..Settings::default()
        };
        assert_eq!(neither.update_surface(), UpdateSurface::Silent);
    }

    /// With automatic checks off the only checks left are the ones run
    /// from About, which reports in its own window.
    #[test]
    fn no_automatic_checks_means_no_announcement() {
        let s = Settings {
            auto_check_updates: false,
            ..Settings::default()
        };
        assert_eq!(s.update_surface(), UpdateSurface::Silent);
    }

    #[test]
    fn category_folder_prefers_an_override() {
        let mut s = Settings {
            category_folders: default_category_folders(Path::new("/base")),
            ..Settings::default()
        };
        s.category_folders
            .insert(Category::Videos, PathBuf::from("/mnt/media"));
        // An empty stored path is not a retarget — it resolves to the
        // detected default, the same as an absent key would.
        s.category_folders.insert(Category::Music, PathBuf::new());

        assert_eq!(
            s.category_folder(Category::Videos),
            PathBuf::from("/mnt/media")
        );
        assert_eq!(
            s.category_folder(Category::Music),
            default_category_folder(&detected_download_dir(), Category::Music)
        );
        assert_eq!(s.category_folder(Category::Other), PathBuf::from("/base"));
        assert_eq!(s.fallback_dir(), PathBuf::from("/base"));
    }
}

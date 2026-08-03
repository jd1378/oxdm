use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

use crate::domain::{Category, QueueId};

/// oxdm-level settings. Wraps every `odl::config::Config` field plus
/// UI-only preferences. The data layer translates this into an
/// `odl::Config` whenever it changes.
///
/// One Settings struct = single source of truth that Settings UI binds
/// to. Defaults match `odl::config::Config::default`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    // ── odl pass-through ────────────────────────────────────────────
    pub download_dir: PathBuf,
    /// Working directory for in-flight downloads — `metadata.pb` and
    /// every `.part` for every job live here, keyed by job id. Stable
    /// across `download_dir` changes so a user retargeting the final
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
    pub proxy: Option<String>,
    /// Proxy sign-in. The username rides in the `proxy` URL like any
    /// other authority component; the password never does — it travels
    /// once on `proxy_password` and only its ciphertext is kept, like a
    /// job's own proxy secret.
    #[serde(default)]
    pub enc_proxy_password: Option<String>,
    /// Transient: GUI → daemon only. Empty means "leave the stored
    /// secret alone", which is why clearing needs its own flag.
    #[serde(skip)]
    pub proxy_password: String,
    #[serde(skip)]
    pub clear_proxy_password: bool,
    pub use_server_time: bool,
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
    /// Behavior when a job in background mode hits a server / save
    /// conflict. `AutoPopup` re-opens the dialog. `NotifyAndPark` shows
    /// a system notification, moves the job to the end of the queue,
    /// marks it failed-with-conflict, and waits for explicit Resume.
    #[serde(default)]
    pub conflict_while_hidden: ConflictWhileHidden,
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
    /// "Download complete" view. Maps to IDM's "Don't show this dialog
    /// again" checkbox: unchecked = global suppression for future
    /// completions. Per-job `OnCompletion::show_dialog` still gates,
    /// so users can opt out individual jobs while keeping the global on.
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
    /// System notification when every job in a queue reaches a terminal
    /// phase. Queue completion has no dialog — the queue window and the
    /// tray already carry the state. Off by default.
    #[serde(default)]
    pub notify_queue_finished: bool,
    /// Update-available surfaces. Both are inert for now: the updater
    /// only checks on demand from the About dialog, so nothing raises
    /// these events and the settings rows stay disabled.
    #[serde(default)]
    pub show_update_dialog: bool,
    #[serde(default)]
    pub notify_update: bool,
    /// Auto-update feed URL. Empty disables update checks. Feed is a
    /// JSON document of shape `{ "version": "x.y.z", "url": "...", "notes": "..." }`.
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
    /// Per-category save-folder override. A capture classified into a
    /// `Category` present here lands in that folder instead of
    /// `download_dir`. Applied only on the non-interactive capture
    /// path (`add_from_capture`); the Add dialog prefills client-side
    /// so an explicit user choice always wins.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    System,
    Light,
    Dark,
    Warm,
}

/// Per `PLAN §9 / §5`: how oxdm reacts when a backgrounded job hits a
/// conflict and there is no visible dialog to host the prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConflictWhileHidden {
    /// Re-show the dialog. IDM-like, more intrusive.
    #[default]
    AutoPopup,
    /// Notify, move to end of queue, mark failed-with-conflict, no
    /// auto-retry. User must explicitly Resume.
    NotifyAndPark,
}

/// "Unlimited" concurrent downloads. Not a sentinel the scheduler knows
/// about — a ceiling no real queue reaches, so the limit simply never
/// binds while the field stays an ordinary number.
pub const UNLIMITED_CONCURRENT: usize = 9999;

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
        let dl = dirs::download_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            download_dir: dl,
            work_dir: default_work_dir(),
            max_connections: None,
            max_concurrent_downloads: UNLIMITED_CONCURRENT,
            max_retries: 3,
            wait_between_retries: Duration::from_millis(700),
            n_fixed_retries: 3,
            user_agent: None,
            randomize_user_agent: false,
            proxy: None,
            enc_proxy_password: None,
            proxy_password: String::new(),
            clear_proxy_password: false,
            use_server_time: false,
            accept_invalid_certs: false,
            speed_limit: None,
            connect_timeout: Some(Duration::from_secs(5)),
            headers: IndexMap::new(),
            ipc_port: 27812,
            ext_token: String::new(),
            conflict_while_hidden: ConflictWhileHidden::AutoPopup,
            remove_confirm_incomplete: true,
            remove_confirm_completed: true,
            remove_confirm_clean: true,
            pause_on_metered: true,
            pause_on_low_battery: false,
            start_at_login: false,
            start_to_tray: false,
            show_complete_dialog: true,
            notify_complete: false,
            show_failed_dialog: true,
            notify_failed: false,
            notify_queue_finished: false,
            show_update_dialog: false,
            notify_update: false,
            update_feed_url: String::new(),
            theme: Theme::System,
            reduce_motion: false,
            theme_overrides: IndexMap::new(),
            category_extensions: IndexMap::new(),
            category_folders: IndexMap::new(),
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

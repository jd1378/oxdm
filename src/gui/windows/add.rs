//! Add / Edit download window (`oxdm gui add [<id>] [--url <u>]`).
//! URL row + paste, detection card (empty / probing / detected /
//! error), location & category row, queue & segments row, Advanced
//! collapsible (Proxy / Headers / Auth / User agent / Cookies),
//! footer with Cancel / Add-to-queue / Download-now.

use std::path::PathBuf;
use std::sync::Arc;

use iced::widget::{column, container, row, text, text_editor};
use iced::{Alignment, Element, Length, Subscription, Task};

use crate::data::ProbeResult;
use crate::domain::{Category, JobId, QueueId};
use crate::gui::chrome::{self, WindowControl, titlebar};
use crate::gui::format::format_bytes;
use crate::gui::shot::Shot;
use crate::gui::theme::{self, Tokens};
use crate::gui::widget::{Btn, FileInput, TabBtn, TextInput, combo, field_label, hairline};
use crate::gui::{color, icons};
use crate::ipc_local::Client;
use crate::ipc_local::protocol::AddJobReq;

const DEBOUNCE_MS: u64 = 400;

/// Add-dialog window width (design `.dialog-add` = 580px).
const DIALOG_W: f32 = 580.0;
/// Big detection ext-tile edge (design `ext-big` = 44px).
const EXT_TILE: f32 = 44.0;
/// Segments option shown (and enforced) when the download can't be resumed.
const FORCED_SINGLE_SEGMENT: &str = "1 connection (forced)";
/// Window height while the probe-error block is shown (titlebar 28 +
/// body padding 2×16 + url row ≈60 + gap 14 + error block ≈300 +
/// footer ≈46; design §3.2 `.error-block` replaces the detected card).
/// Heights measured off the rendered dialog: the point where the last
/// row of content ends, plus the page's bottom padding and the footer
/// band. The window is sized to its contents rather than to a number
/// that once fit them.
///
/// Nothing probed yet: URL field and the placeholder card.
const IDLE_H: f32 = 204.0;
/// The detected-file card above the destination form.
const PROBED_H: f32 = 348.0;
/// With the Advanced pane open. Sized for its tallest tab — Proxy —
/// rather than for the one showing, so switching tabs does not resize
/// the window under the user's hands.
const ADVANCED_H: f32 = 531.0;
/// The "cannot be resumed" line and the gap above it: one 12px line of
/// bold text plus the body column's spacing.
const NOT_RESUMABLE_H: f32 = 27.0;
/// Dash length of the empty card's outline. Short: at this size the
/// outline is a hint that something goes here, not a fence around it.
const DASH_LEN: f32 = 3.0;
/// Above the probe result: the URL label, its field, and the gap.
const ABOVE_RESULT: f32 = 74.0;
/// The page's bottom padding plus the footer band.
const BELOW_CONTENT: f32 = 61.0;
/// Static "a few things to check" bullets for the probe-error panel
/// (design §3.2, `add-dialog.jsx` `.eb-checklist` copy).
const PROBE_CHECKLIST: &[&str] = &[
    "The link is still valid and hasn't expired.",
    "You're signed in if the file requires authentication. Add credentials under Advanced → Auth.",
    "A proxy or VPN isn't blocking the host. Review Advanced → Proxy.",
    "Try a different user agent if the server filters by browser.",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdvTab {
    Proxy,
    Headers,
    Auth,
    UserAgent,
    Cookies,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyKind {
    None,
    Http,
    Socks5,
}

impl std::fmt::Display for ProxyKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ProxyKind::None => "None",
            ProxyKind::Http => "HTTP",
            ProxyKind::Socks5 => "SOCKS5",
        })
    }
}

#[derive(Clone)]
pub enum Msg {
    Connected(Result<Boot, String>),
    Window(WindowControl),
    Daemon(crate::gui::ipc::DaemonSignal),
    UrlChanged(String),
    Paste,
    Pasted(Option<String>),
    DebounceFired(u64),
    Probed(u64, Result<ProbeResult, crate::domain::JobError>),
    SavePathChanged(String),
    BrowseSave,
    BrowsedSave(Option<PathBuf>),
    SetCategory(String),
    SetQueue(String),
    SetSegments(String),
    ToggleAdvanced,
    SetAdvTab(AdvTab),
    SetProxyKind(ProxyKind),
    ProxyHost(String),
    ProxyUser(String),
    ProxyPass(String),
    AuthUser(String),
    AuthPass(String),
    UserAgent(String),
    Cookies(text_editor::Action),
    HeaderName(usize, String),
    HeaderValue(usize, String),
    HeaderRemove(usize),
    HeaderAdd,
    RetryProbe,
    CopyText(String),
    Submit { start_now: bool },
    Submitted(Result<(), String>),
    Cancel,
    WinResized(f32, f32),
    ShotTick,
    Shot(iced::window::Screenshot),
    Themed(Box<Tokens>),
    Noop,
}

#[derive(Clone)]
pub struct Boot {
    client: Arc<Client>,
    settings: crate::domain::Settings,
    queues: Vec<(QueueId, String)>,
    main_queue: QueueId,
    edit: Option<Box<crate::domain::Job>>,
    prefill: Option<String>,
}

pub enum App {
    Connecting,
    Failed(String),
    Ready(Box<AddState>),
}

pub struct AddState {
    client: Arc<Client>,
    tokens: Tokens,
    settings: crate::domain::Settings,
    queues: Vec<(QueueId, String)>,
    edit_id: Option<JobId>,

    url: String,
    probe_gen: u64,
    probing: bool,
    /// Probe outcome; the error side keeps the structured `JobError`
    /// so the GUI wave can render a typed error panel (B2).
    probed: Option<Result<ProbeResult, crate::domain::JobError>>,

    save_path: String,
    category: Option<Category>,
    queue: QueueId,
    segments: u64,
    /// User touched the save-path field — category routing (F5) must
    /// not overwrite an explicit choice.
    save_dirty: bool,
    /// User picked a queue — same rule.
    queue_dirty: bool,
    /// The user picked the category by hand, so a later probe must not
    /// reclassify over their choice.
    category_dirty: bool,

    advanced_open: bool,
    /// The floor currently in force, tracked so it can come back down
    /// when the dialog gets shorter.
    min_h: f32,
    adv_tab: AdvTab,
    proxy_kind: ProxyKind,
    proxy_host: String,
    proxy_user: String,
    proxy_pass: String,
    auth_user: String,
    auth_pass: String,
    user_agent: String,
    cookies: text_editor::Content,
    headers: Vec<(String, String)>,

    error: Option<String>,
    shot: Option<Shot>,
}

impl AddState {
    fn detected(&self) -> Option<&ProbeResult> {
        match &self.probed {
            Some(Ok(p)) => Some(p),
            _ => None,
        }
    }

    fn build_req(&self) -> Option<AddJobReq> {
        let url: url::Url = self.url.trim().parse().ok()?;
        let p = PathBuf::from(self.save_path.trim());
        let (save_dir, filename) = if self.save_path.trim().is_empty() {
            (self.settings.fallback_dir(), None)
        } else if self.save_path.ends_with('/') || p.extension().is_none() && p.is_dir() {
            (p, None)
        } else {
            let dir = p
                .parent()
                .map(|d| d.to_path_buf())
                .unwrap_or_else(|| self.settings.fallback_dir());
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned());
            (dir, name)
        };
        let proxy = match self.proxy_kind {
            ProxyKind::None => None,
            kind if !self.proxy_host.trim().is_empty() => {
                let scheme = if kind == ProxyKind::Http {
                    "http"
                } else {
                    "socks5"
                };
                let user = if self.proxy_user.trim().is_empty() {
                    String::new()
                } else {
                    format!("{}@", self.proxy_user.trim())
                };
                Some(format!("{scheme}://{user}{}", self.proxy_host.trim()))
            }
            _ => None,
        };
        let mut headers = indexmap::IndexMap::new();
        for (k, v) in &self.headers {
            if !k.trim().is_empty() {
                headers.insert(k.trim().to_owned(), v.clone());
            }
        }
        // Non-resumable downloads are forced to a single connection (the
        // Segments combo is locked to "1 connection (forced)" in that state).
        let segments = if self.detected().is_some_and(|p| !p.is_resumable) {
            1
        } else {
            self.segments
        };
        let cookies_text = self.cookies.text();
        let opt = |s: &str| {
            let s = s.trim();
            (!s.is_empty()).then(|| s.to_owned())
        };
        Some(AddJobReq {
            url,
            save_dir,
            filename,
            referrer: None,
            headers,
            max_connections: Some(segments),
            proxy,
            auth_user: opt(&self.auth_user),
            auth_password: opt(&self.auth_pass),
            proxy_password: opt(&self.proxy_pass),
            cookies: opt(&cookies_text),
            category: self.category,
            size: self.detected().and_then(|p| p.size),
            checksums: self
                .detected()
                .map(|p| p.checksums.clone())
                .unwrap_or_default(),
        })
    }
}

/// The height this dialog's current contents want.
///
/// The Advanced pane is taller than anything above it; otherwise the
/// page is the plain form, the form under a detected-file card, or the
/// form under an error block, which is the tallest of the three.
fn wanted_height(st: &AddState) -> f32 {
    // A server that won't resume adds a line above the form, which the
    // fixed heights below don't account for: without this the Advanced
    // row falls under the footer and the page scrolls.
    let warning = match st.detected() {
        Some(p) if !p.is_resumable => NOT_RESUMABLE_H,
        _ => 0.0,
    };
    // The painted titlebar is part of the page when the user opts into
    // custom chrome, and space the heights below never counted: every
    // one of them was measured on an OS-decorated window.
    let content = if st.advanced_open {
        ADVANCED_H + warning
    } else {
        match &st.probed {
            Some(Ok(_)) => PROBED_H + warning,
            // The error block is as tall as the error is wordy, and the
            // same estimate the download window sizes itself with.
            Some(Err(e)) => {
                ABOVE_RESULT
                    + crate::gui::widget::error_panel::checklist_block_height(e, PROBE_CHECKLIST)
                    + BELOW_CONTENT
            }
            None => IDLE_H,
        }
    };
    content + chrome::overhead_h()
}

/// Resize to fit, and move the floor with it.
///
/// The minimum has to travel with the height in both directions: left
/// at the idle height the user could shrink a probed dialog until its
/// own contents were cut off, and left at the probed height a dialog
/// that goes back to idle keeps a band of empty surface it can never
/// lose.
fn fit_window(st: &mut AddState) -> Task<Msg> {
    let h = wanted_height(st);
    st.min_h = h;
    let size = iced::Size::new(DIALOG_W, h);
    iced::window::latest().and_then(move |id| {
        Task::batch([
            iced::window::set_min_size(id, Some(size)),
            iced::window::resize(id, size),
        ])
    })
}

fn parse_args() -> (Option<JobId>, Option<String>) {
    let mut args = std::env::args().skip(3);
    let mut edit_id = None;
    let mut prefill = None;
    while let Some(a) = args.next() {
        if a == "--url" {
            prefill = args.next();
        } else if let Ok(id) = a.parse::<JobId>() {
            edit_id = Some(id);
        }
    }
    (edit_id, prefill)
}

pub fn boot() -> (App, Task<Msg>) {
    let (edit_id, prefill) = parse_args();
    (
        App::Connecting,
        Task::perform(
            async move {
                let client = Client::connect_retry(std::time::Duration::from_secs(8))
                    .await
                    .map_err(|e| e.to_string())?;
                client
                    .hello(crate::ipc_local::protocol::GuiKind::Add)
                    .await?;
                let snap = client.snapshot().await?;
                let main_queue = snap
                    .queues
                    .iter()
                    .find(|q| q.builtin)
                    .or(snap.queues.first())
                    .map(|q| q.id)
                    .ok_or("no queues")?;
                let edit = match edit_id {
                    Some(id) => snap.jobs.iter().find(|j| j.id == id).cloned().map(Box::new),
                    None => None,
                };
                Ok(Boot {
                    client,
                    settings: snap.settings,
                    queues: snap.queues.iter().map(|q| (q.id, q.name.clone())).collect(),
                    main_queue,
                    edit,
                    prefill,
                })
            },
            Msg::Connected,
        ),
    )
}

fn debounce(generation: u64) -> Task<Msg> {
    Task::perform(
        async move {
            tokio::time::sleep(std::time::Duration::from_millis(DEBOUNCE_MS)).await;
            generation
        },
        Msg::DebounceFired,
    )
}

pub fn update(app: &mut App, msg: Msg) -> Task<Msg> {
    match msg {
        Msg::Connected(Ok(boot)) => {
            let mut st = AddState {
                tokens: Tokens::from_settings(&boot.settings),
                client: boot.client,
                queues: boot.queues,
                edit_id: boot.edit.as_ref().map(|j| j.id),
                url: String::new(),
                probe_gen: 0,
                probing: false,
                probed: None,
                save_path: boot.settings.fallback_dir().display().to_string(),
                category: None,
                queue: boot.main_queue,
                segments: 8,
                save_dirty: false,
                queue_dirty: false,
                category_dirty: false,
                advanced_open: false,
                min_h: IDLE_H + chrome::overhead_h(),
                adv_tab: AdvTab::Proxy,
                proxy_kind: ProxyKind::None,
                proxy_host: String::new(),
                proxy_user: String::new(),
                proxy_pass: String::new(),
                auth_user: String::new(),
                auth_pass: String::new(),
                user_agent: String::new(),
                cookies: text_editor::Content::new(),
                headers: Vec::new(),
                error: None,
                shot: Shot::from_env(),
                settings: boot.settings,
            };
            let mut task = Task::none();
            if let Some(job) = boot.edit {
                // Editing an existing job: its folder/queue are settled
                // choices — category routing must never move them (F5).
                st.save_dirty = true;
                st.queue_dirty = true;
                st.url = job.url.to_string();
                st.queue = job.queue_id;
                st.category = Some(job.category);
                if let Some(n) = job.max_connections {
                    st.segments = n;
                }
                let full = job
                    .save_dir
                    .join(job.filename.as_deref().unwrap_or(""))
                    .display()
                    .to_string();
                st.save_path = full;
                for (k, v) in &job.headers {
                    st.headers.push((k.clone(), v.clone()));
                }
                st.probe_gen += 1;
                st.probing = true;
                task = start_probe(&st);
            } else if let Some(u) = boot.prefill {
                st.url = u;
                st.probe_gen += 1;
                task = debounce(st.probe_gen);
            }
            *app = App::Ready(Box::new(st));
            task
        }
        Msg::Connected(Err(e)) => {
            *app = App::Failed(e);
            Task::none()
        }
        Msg::Window(ctl) => chrome::window_task(ctl),
        msg => {
            let App::Ready(st) = app else {
                return Task::none();
            };
            update_ready(st, msg)
        }
    }
}

fn start_probe(st: &AddState) -> Task<Msg> {
    let generation = st.probe_gen;
    let client = st.client.clone();
    let Ok(url) = st.url.trim().parse::<url::Url>() else {
        return Task::none();
    };
    Task::perform(
        async move {
            match tokio::time::timeout(std::time::Duration::from_millis(8000), client.probe(url))
                .await
            {
                Ok(Ok(inner)) => inner,
                Ok(Err(e)) => Err(crate::domain::JobError::Other(e)),
                Err(_) => Err(crate::domain::JobError::Other("probe timed out".to_owned())),
            }
        },
        move |res| Msg::Probed(generation, res),
    )
}

/// Per-category routing prefill (feature #10 / guardian F5): when the
/// category changes, seed the save folder from
/// `Settings::category_folder` and the queue from
/// `Settings::category_queues` — but never overwrite a field the user
/// already touched (`save_dirty` / `queue_dirty`). Client-side only;
/// the daemon applies the same routing solely on the non-interactive
/// capture path.
fn apply_category_prefill(st: &mut AddState) {
    let Some(cat) = st.category else {
        return;
    };
    if !st.save_dirty {
        // Resolved, not just explicit overrides: a category the user has
        // never retargeted still has a folder, and it is the one the
        // Settings pane shows them.
        let folder = st.settings.category_folder(cat);
        // Keep the detected filename; before detection the path is a
        // bare directory (a trailing separator marks it as such for
        // `build_req`).
        let name = st
            .detected()
            .map(|p| p.filename.clone())
            .unwrap_or_default();
        st.save_path = folder.join(name).display().to_string();
    }
    if !st.queue_dirty
        && let Some(qid) = st.settings.category_queues.get(&cat)
        && st.queues.iter().any(|(id, _)| id == qid)
    {
        st.queue = *qid;
    }
}

fn update_ready(st: &mut AddState, msg: Msg) -> Task<Msg> {
    match msg {
        Msg::UrlChanged(u) => {
            st.url = u;
            st.probed = None;
            st.error = None;
            st.probe_gen += 1;
            // A new URL is a new file: drop the classification so the
            // probe routes it by *its* extension. Without this, pasting
            // a .mp4 over a .zip kept the zip's category, and with it
            // the queue and folder that category prefilled.
            if !st.category_dirty {
                st.category = None;
            }
            let valid = st
                .url
                .trim()
                .parse::<url::Url>()
                .map(|u| matches!(u.scheme(), "http" | "https"))
                .unwrap_or(false);
            // The card or the error block just went away with the old
            // URL; the window follows the content back down rather than
            // leaving a band of empty surface above the buttons.
            let fit = fit_window(st);
            if valid {
                Task::batch([fit, debounce(st.probe_gen)])
            } else {
                st.probing = false;
                fit
            }
        }
        Msg::Paste => Task::perform(
            async { crate::gui::clipboard::clipboard_first_link() },
            Msg::Pasted,
        ),
        Msg::Pasted(Some(s)) => update_ready(st, Msg::UrlChanged(s.trim().to_owned())),
        Msg::Pasted(None) => Task::none(),
        Msg::DebounceFired(generation) => {
            if generation == st.probe_gen {
                st.probing = true;
                start_probe(st)
            } else {
                Task::none()
            }
        }
        Msg::Probed(generation, res) => {
            if generation != st.probe_gen {
                return Task::none();
            }
            st.probing = false;
            let mut classified = false;
            if let Ok(p) = &res {
                // derive save path: dir + detected filename
                let dir = PathBuf::from(st.save_path.trim());
                let dir = if dir.is_dir() {
                    dir
                } else {
                    dir.parent()
                        .map(|d| d.to_path_buf())
                        .unwrap_or_else(|| st.settings.fallback_dir())
                };
                st.save_path = dir.join(&p.filename).display().to_string();
                if st.category.is_none() {
                    st.category = Some(crate::domain::classify(
                        &p.filename,
                        &st.settings.category_extensions,
                    ));
                    classified = true;
                }
            }
            st.probed = Some(res);
            if classified {
                // Freshly classified → apply per-category routing (F5).
                apply_category_prefill(st);
            }
            // Grow the window for the detected card, or for the error
            // block (design §3.2: `.error-block` replaces the card).
            fit_window(st)
        }
        Msg::SavePathChanged(p) => {
            st.save_path = p;
            st.save_dirty = true;
            Task::none()
        }
        Msg::BrowseSave => {
            let start = PathBuf::from(st.save_path.trim());
            Task::perform(
                async move {
                    let dlg = rfd::AsyncFileDialog::new();
                    let dlg = match start.parent() {
                        Some(d) if d.exists() => dlg.set_directory(d),
                        _ => dlg,
                    };
                    dlg.pick_folder().await.map(|h| h.path().to_path_buf())
                },
                Msg::BrowsedSave,
            )
        }
        Msg::BrowsedSave(Some(dir)) => {
            let name = PathBuf::from(st.save_path.trim())
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            st.save_path = dir.join(name).display().to_string();
            st.save_dirty = true;
            Task::none()
        }
        Msg::BrowsedSave(None) => Task::none(),
        Msg::SetCategory(label) => {
            let prev = st.category;
            st.category = Category::ALL_ASSIGNABLE
                .iter()
                .copied()
                .find(|c| c.label() == label);
            st.category_dirty = true;
            if st.category != prev {
                apply_category_prefill(st);
            }
            Task::none()
        }
        Msg::SetQueue(name) => {
            if let Some((id, _)) = st.queues.iter().find(|(_, n)| *n == name) {
                st.queue = *id;
                st.queue_dirty = true;
            }
            Task::none()
        }
        Msg::SetSegments(s) => {
            if let Some(n) = s.split_whitespace().next().and_then(|n| n.parse().ok()) {
                st.segments = n;
            }
            Task::none()
        }
        Msg::ToggleAdvanced => {
            st.advanced_open = !st.advanced_open;
            fit_window(st)
        }
        Msg::SetAdvTab(tab) => {
            st.adv_tab = tab;
            Task::none()
        }
        Msg::SetProxyKind(k) => {
            st.proxy_kind = k;
            Task::none()
        }
        Msg::ProxyHost(v) => {
            st.proxy_host = v;
            Task::none()
        }
        Msg::ProxyUser(v) => {
            st.proxy_user = v;
            Task::none()
        }
        Msg::ProxyPass(v) => {
            st.proxy_pass = v;
            Task::none()
        }
        Msg::AuthUser(v) => {
            st.auth_user = v;
            Task::none()
        }
        Msg::AuthPass(v) => {
            st.auth_pass = v;
            Task::none()
        }
        Msg::UserAgent(v) => {
            st.user_agent = v;
            Task::none()
        }
        Msg::Cookies(action) => {
            st.cookies.perform(action);
            Task::none()
        }
        Msg::HeaderName(i, v) => {
            if let Some(h) = st.headers.get_mut(i) {
                h.0 = v;
            }
            Task::none()
        }
        Msg::HeaderValue(i, v) => {
            if let Some(h) = st.headers.get_mut(i) {
                h.1 = v;
            }
            Task::none()
        }
        Msg::HeaderRemove(i) => {
            if i < st.headers.len() {
                st.headers.remove(i);
            }
            Task::none()
        }
        Msg::HeaderAdd => {
            st.headers.push((String::new(), String::new()));
            Task::none()
        }
        Msg::RetryProbe => {
            st.probed = None;
            st.error = None;
            st.probe_gen += 1;
            st.probing = true;
            Task::batch([fit_window(st), start_probe(st)])
        }
        Msg::CopyText(s) => iced::clipboard::write(s),
        Msg::Submit { start_now } => {
            let Some(req) = st.build_req() else {
                st.error = Some("Enter a valid http(s) URL.".to_owned());
                return Task::none();
            };
            let client = st.client.clone();
            let queue = st.queue;
            let edit_id = st.edit_id;
            Task::perform(
                async move {
                    match edit_id {
                        Some(id) => {
                            let edit = crate::ipc_local::protocol::JobEdit {
                                url: req.url.clone(),
                                save_dir: req.save_dir.clone(),
                                filename: req.filename.clone(),
                                referrer: req.referrer.clone(),
                                headers: req.headers.clone(),
                                max_connections: req.max_connections,
                                proxy: req.proxy.clone(),
                                auth_user: req.auth_user.clone(),
                                auth_password: req.auth_password.clone(),
                                proxy_password: req.proxy_password.clone(),
                                cookies: req.cookies.clone(),
                            };
                            client.update_job_location(id, edit).await?;
                            if start_now {
                                client.start_job(id).await?;
                            }
                            Ok(())
                        }
                        None => {
                            let id = client.add_job(req).await?;
                            client.set_job_queue(id, queue).await?;
                            if start_now {
                                client.start_job(id).await?;
                                client.open_download_window(id).await?;
                            }
                            Ok(())
                        }
                    }
                },
                Msg::Submitted,
            )
        }
        Msg::Submitted(Ok(())) => iced::exit(),
        Msg::Submitted(Err(e)) => {
            st.error = Some(e);
            Task::none()
        }
        Msg::Cancel => iced::exit(),
        Msg::Daemon(crate::gui::ipc::DaemonSignal::Lost) => iced::exit(),
        Msg::Daemon(crate::gui::ipc::DaemonSignal::Event(ev)) => match ev {
            crate::ipc_local::protocol::Event::SettingsChanged => {
                crate::gui::theme::refresh_tokens(
                    st.client.clone(),
                    |t| Msg::Themed(Box::new(t)),
                    Msg::Noop,
                )
            }
            crate::ipc_local::protocol::Event::Close => iced::exit(),
            crate::ipc_local::protocol::Event::Focus => {
                iced::window::latest().and_then(iced::window::gain_focus)
            }
            _ => Task::none(),
        },
        Msg::Themed(t) => {
            st.tokens = *t;
            Task::none()
        }
        Msg::WinResized(w, h) => {
            chrome::enforce_min_size(iced::Size::new(w, h), iced::Size::new(DIALOG_W, st.min_h))
        }
        Msg::ShotTick => {
            if let Some(shot) = &mut st.shot
                && let Some(task) = shot.tick()
            {
                return task.map(Msg::Shot);
            }
            Task::none()
        }
        Msg::Shot(s) => match &st.shot {
            Some(shot) => shot.save_and_exit(s),
            None => Task::none(),
        },
        Msg::Connected(_) | Msg::Window(_) | Msg::Noop => Task::none(),
    }
}

// ---------------------------------------------------------------- view

pub fn view(app: &App) -> Element<'_, Msg> {
    chrome::framed(match app {
        App::Connecting => connecting_view("Connecting…"),
        App::Failed(e) => connecting_view(e),
        App::Ready(st) => ready_view(st),
    })
}

fn connecting_view(msg: &str) -> Element<'_, Msg> {
    let t = Tokens::dark();
    container(text(msg.to_owned()).color(t.fg_2))
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(move |_| container::Style {
            background: Some(t.bg_page.into()),
            ..Default::default()
        })
        .into()
}

fn ready_view(st: &AddState) -> Element<'_, Msg> {
    let t = &st.tokens;
    let title = if st.edit_id.is_some() {
        "Edit Download"
    } else {
        "Download File Info"
    };

    // Eval error replaces the detected-card entirely (design §3.2).
    let card: Element<'_, Msg> = match &st.probed {
        Some(Err(e)) => crate::gui::widget::error_panel::error_checklist_block(
            t,
            e,
            PROBE_CHECKLIST,
            Some(Msg::RetryProbe),
            Msg::CopyText(crate::gui::widget::error_panel::error_report(e)),
        ),
        _ => detect_card(st),
    };
    let mut body = column![url_row(st), card].spacing(theme::space::S3);

    if let Some(p) = st.detected()
        && !p.is_resumable
    {
        body = body.push(
            text("This download cannot be resumed")
                .font(theme::BODY_BOLD)
                .size(12.0)
                .color(t.status_warning),
        );
    }

    if st.detected().is_some() {
        body = body
            .push(
                row![
                    labeled(
                        t,
                        "save to",
                        FileInput::new(&st.save_path)
                            .mono()
                            .on_input(Msg::SavePathChanged)
                            .on_browse(Msg::BrowseSave)
                            .view(t)
                    ),
                    labeled(
                        t,
                        "category",
                        combo(
                            t,
                            Category::ALL_ASSIGNABLE
                                .iter()
                                .map(|c| c.label().to_owned())
                                .collect(),
                            st.category.map(|c| c.label().to_owned()),
                            Msg::SetCategory,
                            Length::Fill,
                        )
                    ),
                ]
                .spacing(theme::space::S3),
            )
            .push(
                row![
                    labeled(
                        t,
                        "queue",
                        combo(
                            t,
                            st.queues.iter().map(|(_, n)| n.clone()).collect(),
                            st.queues
                                .iter()
                                .find(|(id, _)| *id == st.queue)
                                .map(|(_, n)| n.clone()),
                            Msg::SetQueue,
                            Length::Fill,
                        )
                    ),
                    labeled(t, "segments", segments_combo(st)),
                ]
                .spacing(theme::space::S3),
            )
            .push(advanced_section(st));
    }

    if let Some(e) = &st.error {
        body = body.push(
            text(e.clone())
                .font(theme::BODY_BOLD)
                .size(12.0)
                .color(t.status_danger),
        );
    }

    let detected = st.detected().is_some();
    let queue_name = st
        .queues
        .iter()
        .find(|(id, _)| *id == st.queue)
        .map(|(_, n)| n.clone())
        .unwrap_or_default();
    let footer = footer(
        t,
        Btn::new("Cancel")
            .ghost()
            .icon("x")
            .on_press(Msg::Cancel)
            .view(t),
        row![
            Btn::new(if st.edit_id.is_some() {
                "Save".to_owned()
            } else {
                format!("Add to {queue_name}")
            })
            .secondary()
            .icon("clock")
            .enabled(detected)
            .on_press(Msg::Submit { start_now: false })
            .view(t),
            Btn::new("Download now")
                .primary()
                .icon("download")
                .enabled(detected)
                .on_press(Msg::Submit { start_now: true })
                .view(t),
        ]
        .spacing(theme::space::S2)
        .into(),
    );

    let t2 = *t;
    let page = column![
        titlebar::titlebar(t, title, false, Msg::Window),
        crate::gui::widget::vscroll(
            container(body)
                .padding(iced::Padding {
                    top: theme::space::S4,
                    bottom: theme::space::S4,
                    left: theme::space::S4,
                    right: theme::space::S4 - crate::gui::widget::SCROLL_GUTTER,
                })
                .width(Length::Fill)
        )
        .height(Length::Fill),
        hairline(t.border_subtle),
        footer,
    ];
    let content = container(page)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: Some(t2.bg_page.into()),
            text_color: Some(t2.fg_1),
            ..Default::default()
        });
    chrome::resize::resizable(t, content.into(), true, Msg::Window)
}

/// Segments select. When the detected download is non-resumable it is locked
/// to a single forced "1 connection (forced)" option (design `.dialog-add`
/// non-resumable variant); the value is forced to 1 in `build_req`. Message
/// wiring is unchanged — `SetSegments` still parses the leading integer.
fn segments_combo(st: &AddState) -> Element<'_, Msg> {
    let t = &st.tokens;
    let non_resumable = st.detected().is_some_and(|p| !p.is_resumable);
    let (options, selected) = if non_resumable {
        (
            vec![FORCED_SINGLE_SEGMENT.to_owned()],
            Some(FORCED_SINGLE_SEGMENT.to_owned()),
        )
    } else {
        (
            [1u64, 2, 4, 8, 16, 32]
                .iter()
                .map(|n| format!("{n} connections"))
                .collect(),
            Some(format!("{} connections", st.segments)),
        )
    };
    combo(t, options, selected, Msg::SetSegments, Length::Fill)
}

fn labeled<'a>(t: &Tokens, label: &str, body: Element<'a, Msg>) -> Element<'a, Msg> {
    column![field_label(t, label), body]
        .spacing(theme::space::S1)
        .width(Length::Fill)
        .into()
}

pub(crate) fn footer<'a, M: 'a>(
    t: &Tokens,
    left: Element<'a, M>,
    right: Element<'a, M>,
) -> Element<'a, M> {
    let t2 = *t;
    container(
        row![left, iced::widget::Space::new().width(Length::Fill), right]
            .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding([theme::space::S2, theme::space::S4])
    .style(move |_| container::Style {
        background: Some(t2.bg_sunken.into()),
        ..Default::default()
    })
    .into()
}

fn url_row(st: &AddState) -> Element<'_, Msg> {
    let t = &st.tokens;
    let filled = !st.url.trim().is_empty();
    let input = TextInput::new(&st.url)
        .hint("https://…")
        .mono()
        .border(if filled {
            t.border_brand
        } else {
            t.border_subtle
        })
        .on_input(Msg::UrlChanged)
        .on_submit(Msg::Noop)
        .view(t);
    column![
        field_label(t, "url"),
        row![
            input,
            Btn::new("Paste")
                .secondary()
                .icon("clipboard")
                .min_width(88.0)
                .on_press(Msg::Paste)
                .view(t),
        ]
        .spacing(theme::space::S2)
        .align_y(Alignment::Center),
    ]
    .spacing(theme::space::S1)
    .into()
}

fn detect_card(st: &AddState) -> Element<'_, Msg> {
    let t = &st.tokens;
    let t2 = *t;
    let detected = st.detected();
    let non_resumable = detected.is_some_and(|p| !p.is_resumable);

    // EXT_TILE (44px) icon tile.
    let (tile_bg, tile_fg) = if non_resumable {
        (color::ochre::O100, color::ochre::O500)
    } else {
        (color::clay::C100, color::clay::C700)
    };
    let tile: Element<'_, Msg> = match detected {
        Some(p) => {
            let ext = PathBuf::from(&p.filename)
                .extension()
                .map(|e| e.to_string_lossy().to_uppercase())
                .unwrap_or_else(|| "FILE".into());
            container(text(ext).font(theme::MONO_BOLD).size(12.0).color(tile_fg))
                .width(Length::Fixed(EXT_TILE))
                .height(Length::Fixed(EXT_TILE))
                .align_x(Alignment::Center)
                .align_y(Alignment::Center)
                .style(move |_| container::Style {
                    background: Some(tile_bg.into()),
                    border: iced::Border {
                        color: color::clay::C400,
                        width: 1.0,
                        radius: theme::radius::SM.into(),
                    },
                    ..Default::default()
                })
                .into()
        }
        None => container(icons::icon(
            if st.probing { "ellipsis" } else { "link" },
            22.0,
            t.fg_2,
        ))
        .width(Length::Fixed(EXT_TILE))
        .height(Length::Fixed(EXT_TILE))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(move |_| container::Style {
            background: Some(t2.bg_sunken.into()),
            border: iced::Border {
                color: color::clay::C400,
                width: 1.0,
                radius: theme::radius::SM.into(),
            },
            ..Default::default()
        })
        .into(),
    };

    // The probe-error state never reaches this card (the shared error
    // panel replaces it in `ready_view`), so only detected / probing /
    // empty remain.
    let text_col: Element<'_, Msg> = match (detected, st.probing) {
        (Some(p), _) => column![
            text(p.filename.clone())
                .font(theme::BODY_BOLD)
                .size(14.0)
                .color(t.fg_1),
            text(
                url::Url::parse(st.url.trim())
                    .ok()
                    .and_then(|u| u.host_str().map(str::to_owned))
                    .unwrap_or_default()
            )
            .font(theme::MONO)
            .size(11.0)
            .color(t.fg_3),
        ]
        .spacing(2.0)
        .into(),
        (None, true) => column![
            text("Detecting file information…")
                .font(theme::BODY_BOLD)
                .size(13.0)
                .color(t.fg_2),
            text("Probing the link for filename and size.")
                .font(theme::BODY)
                .size(12.0)
                .color(t.fg_3),
        ]
        .spacing(2.0)
        .into(),
        (None, false) => column![
            text("Paste a URL link")
                .font(theme::BODY_BOLD)
                .size(13.0)
                .color(t.fg_2),
            text("We'll detect filename, size, and resumability.")
                .font(theme::BODY)
                .size(12.0)
                .color(t.fg_3),
        ]
        .spacing(2.0)
        .into(),
    };

    // Only once something has said how big the file is. Before that
    // the column was a label over an em dash — a fact-shaped hole where
    // there is no fact yet, on the very card that explains it will find
    // one.
    let size_col: Element<'_, Msg> = match detected.and_then(|p| p.size) {
        Some(bytes) => column![
            text("SIZE").font(theme::BODY_BOLD).size(9.0).color(t.fg_3),
            text(format_bytes(bytes))
                .font(theme::MONO)
                .size(13.0)
                .color(t.fg_1),
        ]
        .spacing(2.0)
        .align_x(Alignment::End)
        .into(),
        None => iced::widget::Space::new().width(Length::Shrink).into(),
    };

    let (fill, border) = if non_resumable {
        (t.status_warning_bg, t.status_warning)
    } else if detected.is_some() {
        (t.bg_raised, t.border_subtle)
    } else {
        (t.bg_surface, t.border_default)
    };

    // Nothing asked for yet: the card is an invitation, not a record,
    // so it wears the dashed outline that says something goes here.
    // Drawn on a canvas behind the content — `iced::Border` strokes
    // solid. Once a probe is under way the card is reporting on a real
    // URL, and the outline goes solid with it.
    let waiting = detected.is_none() && !st.probing;
    let card = container(
        row![
            tile,
            text_col,
            iced::widget::Space::new().width(Length::Fill),
            size_col
        ]
        .spacing(theme::space::S3)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding(theme::space::S3)
    .style(move |_| container::Style {
        background: Some(fill.into()),
        border: iced::Border {
            color: if waiting {
                iced::Color::TRANSPARENT
            } else {
                border
            },
            width: 1.0,
            radius: theme::surface::RADIUS.into(),
        },
        ..Default::default()
    });
    if waiting {
        iced::widget::stack![
            card,
            crate::gui::widget::dashed_frame(border, theme::surface::RADIUS, DASH_LEN)
        ]
        .into()
    } else {
        card.into()
    }
}

fn advanced_section(st: &AddState) -> Element<'_, Msg> {
    let t = &st.tokens;
    let t2 = *t;
    let chev = if st.advanced_open {
        "chevron-down"
    } else {
        "chevron-right"
    };
    let header = iced::widget::mouse_area(
        row![
            icons::icon(chev, 12.0, t.fg_2),
            text("Advanced")
                .font(theme::BODY_BOLD)
                .size(12.0)
                .color(t.fg_2),
        ]
        .spacing(6.0)
        .align_y(Alignment::Center),
    )
    .on_press(Msg::ToggleAdvanced)
    .interaction(iced::mouse::Interaction::Pointer);

    if !st.advanced_open {
        return container(header).into();
    }

    let tabs = row![
        adv_tab(t, "Proxy", AdvTab::Proxy, st.adv_tab),
        adv_tab(t, "Headers", AdvTab::Headers, st.adv_tab),
        adv_tab(t, "Auth", AdvTab::Auth, st.adv_tab),
        adv_tab(t, "User agent", AdvTab::UserAgent, st.adv_tab),
        adv_tab(t, "Cookies", AdvTab::Cookies, st.adv_tab),
    ];

    let tab_body: Element<'_, Msg> = match st.adv_tab {
        AdvTab::Proxy => {
            let enabled = st.proxy_kind != ProxyKind::None;
            column![
                row![
                    labeled(
                        t,
                        "type",
                        combo(
                            t,
                            vec![ProxyKind::None, ProxyKind::Http, ProxyKind::Socks5],
                            Some(st.proxy_kind),
                            Msg::SetProxyKind,
                            Length::Fill,
                        )
                    ),
                    labeled(
                        t,
                        "host:port",
                        TextInput::new(&st.proxy_host)
                            .hint("127.0.0.1:1080")
                            .mono()
                            .enabled(enabled)
                            .on_input(Msg::ProxyHost)
                            .view(t)
                    ),
                ]
                .spacing(theme::space::S3),
                row![
                    labeled(
                        t,
                        "username",
                        TextInput::new(&st.proxy_user)
                            .hint("optional")
                            .enabled(enabled)
                            .on_input(Msg::ProxyUser)
                            .view(t)
                    ),
                    labeled(
                        t,
                        "password",
                        TextInput::new(&st.proxy_pass)
                            .hint("optional")
                            .secure(true)
                            .enabled(enabled)
                            .on_input(Msg::ProxyPass)
                            .view(t)
                    ),
                ]
                .spacing(theme::space::S3),
            ]
            .spacing(theme::space::S3)
            .into()
        }
        AdvTab::Headers => {
            let mut col = column![].spacing(theme::space::S3);
            for (i, (name, value)) in st.headers.iter().enumerate() {
                col = col.push(
                    row![
                        TextInput::new(name)
                            .hint("Name")
                            .on_input(move |v| Msg::HeaderName(i, v))
                            .view(t),
                        TextInput::new(value)
                            .hint("Value")
                            .on_input(move |v| Msg::HeaderValue(i, v))
                            .view(t),
                        Btn::new("")
                            .toolbar()
                            .icon_only("trash-2")
                            .on_press(Msg::HeaderRemove(i))
                            .view(t),
                    ]
                    .spacing(theme::space::S2)
                    .align_y(Alignment::Center),
                );
            }
            col = col.push(
                Btn::new("Add header")
                    .ghost()
                    .icon("plus")
                    .font_size(11.0)
                    .fill_width()
                    .on_press(Msg::HeaderAdd)
                    .view(t),
            );
            col.into()
        }
        AdvTab::Auth => row![
            labeled(
                t,
                "username",
                TextInput::new(&st.auth_user)
                    .on_input(Msg::AuthUser)
                    .view(t)
            ),
            labeled(
                t,
                "password",
                TextInput::new(&st.auth_pass)
                    .secure(true)
                    .on_input(Msg::AuthPass)
                    .view(t)
            ),
        ]
        .spacing(theme::space::S3)
        .into(),
        AdvTab::UserAgent => labeled(
            t,
            "user agent",
            TextInput::new(&st.user_agent)
                .hint("oxdm/1.0")
                .on_input(Msg::UserAgent)
                .view(t),
        ),
        AdvTab::Cookies => {
            let t3 = *t;
            text_editor::TextEditor::new(&st.cookies)
                .placeholder("session_id=…; csrf=…")
                .font(theme::MONO)
                .size(12.0)
                .height(Length::Fixed(96.0))
                .on_action(Msg::Cookies)
                .style(move |_th, _status| text_editor::Style {
                    background: t3.bg_raised.into(),
                    border: iced::Border {
                        color: t3.border_subtle,
                        width: 1.0,
                        radius: theme::control::RADIUS.into(),
                    },
                    placeholder: t3.fg_4,
                    value: t3.fg_1,
                    selection: t3.selection_bg(),
                })
                .into()
        }
    };

    container(
        column![
            header,
            container(column![tabs, hairline(t.border_subtle), tab_body].spacing(theme::space::S2))
                .padding(iced::Padding {
                    left: theme::space::S3,
                    right: theme::space::S3,
                    bottom: theme::space::S3,
                    top: theme::space::S2,
                })
                .width(Length::Fill)
                .style(move |_| container::Style {
                    background: Some(t2.bg_sunken.into()),
                    border: iced::Border {
                        radius: theme::surface::RADIUS.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
        ]
        .spacing(theme::space::S2),
    )
    .into()
}

fn adv_tab<'a>(t: &Tokens, label: &'a str, tab: AdvTab, current: AdvTab) -> Element<'a, Msg> {
    TabBtn::new(label)
        .height(28.0)
        .bottom_gap(6.0)
        .pad_x(10.0)
        .font_size(11.0)
        .active(tab == current)
        .on_press(Msg::SetAdvTab(tab))
        .view(t)
}

pub fn subscription(app: &App) -> Subscription<Msg> {
    let resize = iced::event::listen_with(|event, _status, _id| match event {
        iced::Event::Window(iced::window::Event::Resized(size)) => {
            Some(Msg::WinResized(size.width, size.height))
        }
        _ => None,
    });
    let events = crate::gui::ipc::lifecycle_events(crate::ipc_local::protocol::GuiKind::Add)
        .map(Msg::Daemon);
    match app {
        App::Ready(st) if st.shot.is_some() => {
            Subscription::batch([resize, events, Shot::frames().map(|_| Msg::ShotTick)])
        }
        _ => Subscription::batch([resize, events]),
    }
}

pub fn launch_add(_edit_id: Option<JobId>, _prefill: Option<String>) {
    // Args re-parsed in boot() (iced's boot closure takes no params).
    let mut app = iced::application(boot, update, view)
        .title(|app: &App| match app {
            App::Ready(st) if st.edit_id.is_some() => "oxdm — Edit Download".to_owned(),
            _ => "oxdm — Download File Info".to_owned(),
        })
        .theme(|app: &App| match app {
            App::Ready(st) => st.tokens.iced_theme(),
            _ => Tokens::dark().iced_theme(),
        })
        .subscription(subscription)
        .default_font(theme::BODY)
        .antialiasing(true)
        .window(chrome::window_settings(
            iced::Size::new(DIALOG_W, IDLE_H + chrome::overhead_h()),
            iced::Size::new(DIALOG_W, IDLE_H + chrome::overhead_h()),
        ));
    for f in theme::fonts::ALL {
        app = app.font(*f);
    }
    if let Err(e) = app.run() {
        eprintln!("gui error: {e}");
        std::process::exit(1);
    }
}

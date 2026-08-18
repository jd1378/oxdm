//! Per-job Properties window (`oxdm gui properties <id>`): General /
//! Checksums / Connection / Cookies / Headers tabs, hero
//! card, section cards with kv rows, footer with Open Containing
//! Folder / Close / Apply.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use iced::widget::{column, container, row, text, text_editor};
use iced::{Alignment, Element, Length, Subscription, Task};

use crate::domain::checksum::{Algo, CsSource, CsStatus};
use crate::domain::{Checksum, JobId, Phase};
use crate::gui::chrome::{self, WindowControl, titlebar};
use crate::gui::format::{format_bytes_2, format_int_grouped};
use crate::gui::ipc::DaemonSignal;
use crate::gui::shot::Shot;
use crate::gui::theme::{self, Tokens};
use crate::gui::widget::conn_form::{
    self, AUTH_SCHEME_VALUES, AuthForm, PROXY_MODE_VALUES, ProxyForm,
};
use crate::gui::widget::{
    Btn, BtnSize, TabBtn, TextInput, checkbox, combo, hairline, labeled_section, pill_progress,
    set_row, toggle, toggle_row,
};
use crate::gui::windows::add::footer;
// The same bounds and presets the download window's Speed tab uses:
// these two controls exist in both places and must agree.
use crate::gui::windows::download::{
    LIMIT_INPUT_W, MAX_CONN_DEFAULT, MAX_CONN_MAX, MAX_CONN_MIN, SPEED_PRESETS_KBS,
};
use crate::gui::{color, icons};
use crate::ipc_local::Client;
use crate::ipc_local::protocol::{Event, JobEntryView};

// --- Checksums tab (#5) -----------------------------------------------
/// Char-count meter under the hash input (design `.pac-meter` = 4px).
const PAC_METER_H: f32 = 4.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    General,
    Checksums,
    Connection,
    Cookies,
    Headers,
}

/// What one connected Properties window starts from.
#[derive(Clone)]
pub struct Session {
    client: Arc<Client>,
    entry: JobEntryView,
    settings: crate::domain::Settings,
    queues: Vec<(crate::domain::QueueId, String)>,
    /// Name keys the *other* downloads hold. One name identifies one
    /// download, so renaming onto a taken one is refused here as well
    /// as by the daemon — locally so the message names the field the
    /// user is looking at rather than arriving as a failed Apply.
    taken_names: Vec<String>,
}

/// Which copy button was pressed. Row indices, so a list that changes
/// under the confirmation simply drops it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyTarget {
    Url,
    Hash(usize),
    Expected(usize),
    Got(usize),
}

#[derive(Clone)]
pub enum Msg {
    Connected(Result<Box<Session>, String>),
    Entry(Box<JobEntryView>),
    Daemon(DaemonSignal),
    Window(WindowControl),
    SetTab(Tab),
    // General
    Url(String),
    SavePath(String),
    BrowseSave,
    BrowsedSave(Option<PathBuf>),
    CopyUrl,
    // Connection (#6)
    ProxyModeSel(usize),
    ProxyHost(String),
    ProxyPort(String),
    ProxyAuth(bool),
    ProxyUser(String),
    ProxyPass(String),
    RemoteDns(bool),
    AuthSchemeSel(usize),
    AuthUser(String),
    AuthPass(String),
    AuthToken(String),
    /// Explicit "delete the stored secret" for the current scheme.
    AuthSecretClear,
    /// Explicit "delete the stored proxy password".
    ProxyPassClear,
    // Cookies
    CookiesEnabled(bool),
    CookiesEdit(text_editor::Action),
    CookiesClear,
    // Headers — identification, then the free-form rows
    UserAgent(String),
    Referer(String),
    HeaderName(usize, String),
    HeaderValue(usize, String),
    HeaderRemove(usize),
    HeaderAdd,
    /// A copy button's confirmation has run its course.
    CopyExpired,
    MaxConn(String),
    UseLimiter(bool),
    LimitValue(String),
    LimitUnit(bool),
    SpeedPreset(u64),
    SetCategory(String),
    SetQueue(String),
    // Checksums (#5)
    CsAddOpen,
    CsAddCancel,
    CsAlgoPick(usize),
    CsAuto(bool),
    ChecksumHash(text_editor::Action),
    ChecksumSave,
    ChecksumRemove(usize),
    CsVerify(usize),
    /// Verify finished for the row identified by (algo, saved hash) —
    /// identity, not index, so a concurrent remove can't misfile it.
    CsVerifyFailed(String),
    CsCopy(CopyTarget, String),
    // Settings refresh (theme + will-send headers stay current)
    SettingsRefreshed(Box<crate::domain::Settings>),
    // Footer
    OpenFolder,
    CloseWin,
    Apply,
    Discard,
    Applied(Result<(), String>),
    WinResized(f32, f32),
    ShotTick,
    Shot(iced::window::Screenshot),
    Noop,
}

pub enum App {
    Connecting,
    Failed(String),
    Ready(Box<State>),
}

pub struct State {
    client: Arc<Client>,
    tokens: Tokens,
    id: JobId,
    entry: JobEntryView,
    settings: crate::domain::Settings,
    /// See `Session::taken_names`.
    taken_names: Vec<String>,
    tab: Tab,

    url: String,
    save_path: String,
    // Connection (#6). Secret inputs are scratch buffers: they start
    // empty, never mirror stored ciphertext, and an empty value on
    // Apply means "keep the stored secret" (guardian F1).
    /// Shared with the Add dialog's Advanced pane, controls and all —
    /// two windows editing one download's proxy cannot be allowed to
    /// mean different things by it.
    proxy: ProxyForm,
    auth: AuthForm,
    cookies_enabled: bool,
    cookies: text_editor::Content,
    /// Encrypted cookies exist on the job (shown as "(stored)" — the
    /// plaintext never round-trips back into the editor).
    has_stored_cookies: bool,
    /// Same rule as `proxy_pass_edited`, for the cookie editor.
    cookies_edited: bool,
    /// Identification, lifted out of the header rows below: both keys
    /// have a winner-takes-all rule of their own on the wire, so
    /// editing them among ordinary headers would hide which value the
    /// request actually carries. Empty means "inherit" — the UA falls
    /// back to Settings, the referrer is simply not sent.
    ua: String,
    referer: String,
    /// Free-form headers, never including `User-Agent` / `Referer`.
    headers: Vec<(String, String)>,
    /// Per-job transfer limits, staged like every other field here.
    /// Empty `max_conn` means "let oxdm choose"; the limit is kept as
    /// value + unit so the field reads the way the user typed it.
    max_conn: String,
    limit_on: bool,
    limit_value: String,
    limit_unit_mb: bool,
    /// Where the download files itself, and which queue runs it. Both
    /// are staged like every other field on this tab and go out on
    /// Apply.
    category: crate::domain::Category,
    queue: crate::domain::QueueId,
    queues: Vec<(crate::domain::QueueId, String)>,
    adv: crate::domain::Advanced,
    checksums: Vec<Checksum>,
    /// Which copy button is showing its confirmation, if any. Keyed so
    /// two buttons in the same window answer independently.
    copied: Option<CopyTarget>,
    // Checksums add-form (#5, design §3.4 AddChecksumForm)
    cs_adding: bool,
    cs_algo: Algo,
    cs_auto: bool,
    /// Hash entry is a multi-line editor (design `.pac-input` is a
    /// textarea): pasted hashes wrap instead of scrolling out of view.
    checksum_hash: text_editor::Content,
    /// Row currently hashing on a blocking thread, identified by
    /// (algo, saved hash).
    cs_verify_error: Option<String>,

    dirty: bool,
    /// How many settings differ from the saved job — the Discard
    /// button's label. Recomputed wherever `dirty` is set.
    dirty_count: usize,
    /// URL / save-path changed → `SetJobSource` on Apply.
    dirty_source: bool,
    /// Custom headers / cookies changed → `UpdateJobLocation` on Apply
    /// (the only IPC that persists `Job.headers` + `enc_cookies`).
    dirty_overlay: bool,
    error: Option<String>,
    shot: Option<Shot>,
}

impl State {
    fn locked(&self) -> bool {
        self.entry.counters.phase.is_running()
    }

    /// Caption for both the painted titlebar and the OS/taskbar title:
    /// what the download is, then which window this is. The URL stands
    /// in until evaluation resolves a filename.
    fn window_title(&self) -> String {
        let job = &self.entry.job;
        let name = job.filename.as_deref().unwrap_or(job.url.as_str());
        format!("{name} - Properties")
    }

    /// An explicit proxy mode missing the host or port it needs. The
    /// job would only fail at start; this blocks Apply while the user
    /// is still looking at the field.
    fn proxy_invalid(&self) -> bool {
        self.proxy.invalid()
    }
}

fn job_id_arg() -> Option<JobId> {
    std::env::args().nth(3)?.parse().ok()
}

pub fn boot() -> (App, Task<Msg>) {
    let Some(id) = job_id_arg() else {
        return (App::Failed("missing job id".into()), Task::none());
    };
    (
        App::Connecting,
        Task::perform(
            async move {
                let client = Client::connect_retry(Duration::from_secs(8))
                    .await
                    .map_err(|e| e.to_string())?;
                client
                    .hello(crate::ipc_local::protocol::GuiKind::Properties(id))
                    .await?;
                let entry = client.job_entry(id).await?.ok_or("job not found")?;
                let snap = client.snapshot().await?;
                let queues = snap
                    .queues
                    .iter()
                    .map(|q| (q.id, q.name.clone()))
                    .collect::<Vec<_>>();
                let taken_names = snap
                    .jobs
                    .iter()
                    .filter(|j| j.id != id)
                    .filter_map(|j| j.filename.as_deref())
                    .map(crate::domain::name_key)
                    .collect::<Vec<_>>();
                Ok(Box::new(Session {
                    client,
                    entry,
                    settings: snap.settings,
                    queues,
                    taken_names,
                }))
            },
            Msg::Connected,
        )
        // Started by the daemon, not by the user's own click, so the
        // window has to ask to be in front. See `focus_on_open`.
        .chain(chrome::focus_on_open()),
    )
}

/// The `Advanced` bundle this form would send. Shared by Apply and the
/// change count so the footer cannot disagree with what applying writes.
fn pending_advanced(st: &State) -> crate::domain::Advanced {
    // Advanced bundle. The daemon strips the secret fields into
    // the encrypted columns (guardian F1) and moves a non-empty
    // Basic username onto legacy `Job.auth_user` (F2). Empty
    // secret inputs mean "keep the stored secret".
    let mut adv = st.adv.clone();
    // Both halves come out of the shared form, including the
    // "emptied on purpose" flags that mean "delete the stored
    // secret" — the same rules the Add dialog fills in.
    let creds = crate::gui::widget::conn_form::creds(&st.proxy, &st.auth);
    adv.proxy = creds.proxy;
    adv.auth = creds.auth;
    adv.clear_cookie_jar = st.cookies_edited && st.cookies.text().trim().is_empty();
    adv.cookies_enabled = st.cookies_enabled;
    adv.cookie_jar = st.cookies.text();
    adv
}

const USER_AGENT_KEY: &str = "User-Agent";
const REFERER_KEY: &str = "Referer";

/// The header bag this form would store: the free-form rows plus the
/// User-Agent, which travels as an ordinary header (`start_job`
/// promotes it to odl's UA option at run time). The referrer does not
/// belong here — it has its own column; see `pending_referrer`.
fn pending_headers(st: &State) -> Vec<(String, String)> {
    compose_headers(&st.headers, &st.ua)
}

/// The composition itself, away from the form: free-form rows first,
/// then the User-Agent field if it names one. Nameless rows are still
/// being typed and never reach storage.
fn compose_headers(rows: &[(String, String)], ua: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = rows
        .iter()
        .filter(|(k, _)| !k.trim().is_empty())
        .map(|(k, v)| (k.trim().to_owned(), v.clone()))
        .collect();
    let ua = ua.trim();
    if !ua.is_empty() {
        out.push((USER_AGENT_KEY.to_owned(), ua.to_owned()));
    }
    out
}

/// What `hydrate` puts in the form: the User-Agent, the referrer, and
/// the rows that are neither.
fn split_identity(job: &crate::domain::Job) -> (String, String, Vec<(String, String)>) {
    let ua = job
        .headers
        .iter()
        .find(|(k, _)| crate::domain::header_name_eq(k, USER_AGENT_KEY))
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    let (_, referrer) = saved_identity(job);
    let rows = job
        .headers
        .iter()
        .filter(|(k, _)| {
            !crate::domain::header_name_eq(k, USER_AGENT_KEY)
                && !crate::domain::header_name_eq(k, REFERER_KEY)
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    (ua, referrer.unwrap_or_default(), rows)
}

/// The referrer as typed. `Err` is a non-empty field that is not a
/// URL — Apply refuses rather than dropping it silently, since a
/// referrer the server never sees looks the same as one it rejected.
fn pending_referrer(st: &State) -> Result<Option<url::Url>, ()> {
    let r = st.referer.trim();
    if r.is_empty() {
        return Ok(None);
    }
    r.parse().map(Some).map_err(|_| ())
}

/// The saved job as this form reads it: the header bag without the
/// referrer, and the referrer itself — from its column, or from a
/// legacy `Referer` header. Comparing against this (rather than the
/// raw bag) keeps a freshly-opened dialog from reporting the lift
/// itself as an unsaved change.
fn saved_identity(job: &crate::domain::Job) -> (Vec<(String, String)>, Option<String>) {
    let headers = job
        .headers
        .iter()
        .filter(|(k, _)| !crate::domain::header_name_eq(k, REFERER_KEY))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let referrer = job.referrer.as_ref().map(|u| u.to_string()).or_else(|| {
        job.headers
            .iter()
            .find(|(k, _)| crate::domain::header_name_eq(k, REFERER_KEY))
            .map(|(_, v)| v.clone())
    });
    (headers, referrer)
}

/// Header bags compare by content, not by order: hydration lifts
/// `User-Agent` out of the middle of the stored bag and `pending_headers`
/// puts it back at the end, which is the same request either way.
fn header_bag(rows: &[(String, String)]) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = rows
        .iter()
        .map(|(k, val)| (k.to_lowercase(), val.clone()))
        .collect();
    v.sort();
    v
}

/// Where the typed save path points, keeping the job's own file name
/// when the path names a folder.
fn pending_destination(st: &State) -> crate::domain::Destination {
    crate::gui::save_path::destination(
        &st.settings,
        &st.save_path,
        st.entry.job.filename.as_deref(),
    )
}

/// Does another download already answer to this name?
fn name_taken(st: &State, name: &str) -> bool {
    let key = crate::domain::name_key(name);
    !key.is_empty() && st.taken_names.contains(&key)
}

/// The name this form would save under, when there is one.
fn pending_name(st: &State) -> Option<String> {
    pending_destination(st)
        .filename
        .filter(|n| !n.trim().is_empty())
}

/// What the field leaves out: where the file lands, or the extension
/// the typed name drops or swaps.
fn save_note(st: &State) -> Option<crate::gui::save_path::Note> {
    // Refused rather than numbered: the name in the field is one the
    // user typed, and renaming it behind them would be a worse answer
    // than saying it is taken.
    if let Some(name) = pending_name(st).filter(|n| name_taken(st, n)) {
        return Some(crate::gui::save_path::Note {
            text: format!("Another download is already called {name}"),
            warning: true,
        });
    }
    crate::gui::save_path::note(
        &st.save_path,
        &pending_destination(st),
        st.entry.job.filename.as_deref(),
    )
}

/// The save-path field, with the destination spelled out underneath
/// whenever the text alone does not spell it. Deleting the file name to
/// retarget the folder is how people retarget the folder; the line is
/// where they see that the name comes back, rather than the folder
/// being written over as a file.
fn save_to_block(st: &State, editable: bool) -> Element<'_, Msg> {
    let t = &st.tokens;
    let mut col = column![
        text("Save as")
            .font(theme::BODY_MEDIUM)
            .size(12.0)
            .color(t.fg_1),
        row![
            TextInput::new(&st.save_path)
                .mono()
                .enabled(editable)
                .on_input(Msg::SavePath)
                .view(t),
            Btn::new("")
                .secondary()
                .icon_only("folder")
                .enabled(editable)
                .on_press(Msg::BrowseSave)
                .view(t),
        ]
        .spacing(6.0)
        .align_y(Alignment::Center),
    ]
    .spacing(6.0);
    if let Some(n) = save_note(st) {
        col = col.push(
            text(n.text)
                .font(theme::MONO)
                .size(11.0)
                .color(if n.warning { t.status_warning } else { t.fg_2 })
                .wrapping(iced::widget::text::Wrapping::None),
        );
    }
    col.into()
}

/// How many of the job's settings this form would change. The secret
/// fields count when freshly typed: an empty one means "keep", so it is
/// not a change, which is exactly what `pending_advanced` encodes.
fn count_changes(st: &State) -> usize {
    let job = &st.entry.job;
    let mut n = crate::gui::diff::count_changes(&job.advanced, &pending_advanced(st));
    if st.url.trim() != job.url.as_str() {
        n += 1;
    }
    // Compared as the destination it resolves to, not as text: a path
    // the user reshaped without moving the file (dropping the name off
    // its own folder, say) is not a change to count.
    let dest = pending_destination(st);
    if dest.dir != job.save_dir || dest.filename != job.filename {
        n += 1;
    }
    let (stored, stored_referrer) = saved_identity(job);
    if header_bag(&pending_headers(st)) != header_bag(&stored) {
        n += 1;
    }
    // An unparseable referrer still counts as a change — it is one the
    // user made, and Apply is where it gets refused.
    let typed_referrer = pending_referrer(st)
        .map(|r| r.map(|u| u.to_string()))
        .unwrap_or_else(|()| Some(st.referer.trim().to_owned()));
    if typed_referrer != stored_referrer {
        n += 1;
    }
    if st.checksums != job.checksums {
        n += 1;
    }
    if st.category != job.category {
        n += 1;
    }
    if st.queue != job.queue_id {
        n += 1;
    }
    if pending_max_conn(st) != job.max_connections {
        n += 1;
    }
    if pending_speed_limit(st) != job.speed_limit_override {
        n += 1;
    }
    n
}

fn hydrate(st: &mut State) {
    let job = &st.entry.job;
    st.url = job.url.to_string();
    st.save_path = job
        .save_dir
        .join(job.filename.as_deref().unwrap_or(""))
        .display()
        .to_string();
    // Both forms, including the display-side coercions for modes and
    // schemes this build cannot carry out (guardian F6) and for the
    // legacy Basic shape. Secrets stay out: the ciphertext never
    // round-trips into an input.
    st.proxy = ProxyForm::from_adv(&job.advanced.proxy);
    st.auth = AuthForm::from_adv(&job.advanced.auth, job.auth_user.as_deref());
    // Identification is lifted out of the bag and shown in its own
    // fields. A hand-written `Referer` header is folded into the
    // referrer field — it is the same thing said twice otherwise, and
    // the column is what the runner reads.
    let (ua, referer, rows) = split_identity(job);
    st.ua = ua;
    st.referer = referer;
    st.headers = rows;
    st.adv = job.advanced.clone();
    st.cookies_enabled = job.advanced.cookies_enabled;
    // `cookie_jar` is only non-empty on legacy blobs written before
    // cookies moved to the encrypted rail; fresh saves land on
    // `enc_cookies` and never round-trip plaintext back here.
    st.cookies = text_editor::Content::with_text(&job.advanced.cookie_jar);
    st.has_stored_cookies = job.enc_cookies.is_some();
    st.cookies_edited = false;
    st.checksums = job.checksums.clone();
    st.category = job.category;
    st.queue = job.queue_id;
    st.max_conn = job
        .max_connections
        .map(|n| n.to_string())
        .unwrap_or_default();
    // Shown in whichever unit divides evenly, the way the download
    // window and Settings both show a cap.
    st.limit_on = job.speed_limit_override.is_some();
    let bps = job.speed_limit_override.unwrap_or(0);
    st.limit_unit_mb = bps > 0 && bps.is_multiple_of(1024 * 1024);
    st.limit_value = match bps {
        0 => String::new(),
        b if st.limit_unit_mb => (b / 1024 / 1024).to_string(),
        b => (b / 1024).to_string(),
    };
}

/// The connection cap this form would send: `None` is "let oxdm
/// choose", and anything outside 1–16 is not a cap the daemon accepts.
fn pending_max_conn(st: &State) -> Option<u64> {
    st.max_conn
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|n| (MAX_CONN_MIN as u64..=MAX_CONN_MAX as u64).contains(n))
}

/// The per-job speed cap this form would send, in bytes per second.
/// `None` is unlimited — which is what an empty or unparseable value
/// means too, since a limit nobody can read is not a limit.
fn pending_speed_limit(st: &State) -> Option<u64> {
    if !st.limit_on {
        return None;
    }
    let unit = if st.limit_unit_mb { 1024 * 1024 } else { 1024 };
    st.limit_value
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|v| *v > 0)
        .map(|v| v * unit)
}

pub fn update(app: &mut App, msg: Msg) -> Task<Msg> {
    match msg {
        Msg::Connected(Ok(boxed)) => {
            let Session {
                client,
                entry,
                settings,
                queues,
                taken_names,
            } = *boxed;
            let mut st = State {
                tokens: Tokens::from_settings(&settings),
                id: entry.job.id,
                tab: Tab::General,
                url: String::new(),
                save_path: String::new(),
                ua: String::new(),
                referer: String::new(),
                // Placeholders: `hydrate` fills both in from the job
                // a few lines below.
                proxy: ProxyForm::default(),
                auth: AuthForm::default(),
                cookies_enabled: false,
                cookies: text_editor::Content::new(),
                has_stored_cookies: false,
                cookies_edited: false,
                headers: Vec::new(),
                max_conn: String::new(),
                limit_on: false,
                limit_value: String::new(),
                limit_unit_mb: false,
                category: entry.job.category,
                queue: entry.job.queue_id,
                queues,
                adv: Default::default(),
                checksums: Vec::new(),
                copied: None,
                cs_adding: false,
                cs_algo: Algo::Sha256,
                cs_auto: true,
                checksum_hash: text_editor::Content::new(),
                cs_verify_error: None,
                dirty: false,
                dirty_count: 0,
                dirty_source: false,
                dirty_overlay: false,
                error: None,
                shot: Shot::from_env(),
                client,
                entry,
                settings,
                taken_names,
            };
            hydrate(&mut st);
            *app = App::Ready(Box::new(st));
            Task::none()
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

fn update_ready(st: &mut State, msg: Msg) -> Task<Msg> {
    // Staged edits await Apply; the count drives the Discard label.
    let mark = |st: &mut State| {
        st.dirty = true;
        st.dirty_count = count_changes(st);
    };
    match msg {
        Msg::Entry(e) => {
            st.entry = *e;
            if !st.dirty {
                hydrate(st);
            }
            Task::none()
        }
        Msg::Daemon(DaemonSignal::Lost) => iced::exit(),
        Msg::Daemon(DaemonSignal::Event(ev)) => match ev {
            Event::Counters(list) => {
                if let Some(c) = list.into_iter().find(|c| c.id == st.id) {
                    st.entry.counters = c;
                }
                Task::none()
            }
            Event::VerifyFailed { id, message } if id == st.id => {
                st.cs_verify_error = Some(message);
                Task::none()
            }
            Event::JobsChanged => {
                let client = st.client.clone();
                let id = st.id;
                Task::perform(async move { client.job_entry(id).await }, |r| match r {
                    Ok(Some(e)) => Msg::Entry(Box::new(e)),
                    _ => Msg::Noop,
                })
            }
            Event::SettingsChanged => {
                let client = st.client.clone();
                Task::perform(async move { client.snapshot().await }, |r| match r {
                    Ok(snap) => Msg::SettingsRefreshed(Box::new(snap.settings)),
                    Err(_) => Msg::Noop,
                })
            }
            Event::Close => iced::exit(),
            Event::Focus => iced::window::latest().and_then(iced::window::gain_focus),
            _ => Task::none(),
        },
        Msg::SettingsRefreshed(s) => {
            st.tokens = Tokens::from_settings(&s);
            st.settings = *s;
            Task::none()
        }
        Msg::SetTab(tab) => {
            st.tab = tab;
            Task::none()
        }
        Msg::Url(v) => {
            st.url = v;
            st.dirty_source = true;
            mark(st);
            Task::none()
        }
        Msg::SavePath(v) => {
            st.save_path = v;
            st.dirty_source = true;
            mark(st);
            Task::none()
        }
        Msg::BrowseSave => {
            // Opens where the field points, not one level above it.
            let start = pending_destination(st).dir;
            Task::perform(
                async move {
                    let dlg = rfd::AsyncFileDialog::new();
                    let dlg = if start.is_dir() {
                        dlg.set_directory(start)
                    } else {
                        dlg
                    };
                    dlg.pick_folder().await.map(|h| h.path().to_path_buf())
                },
                Msg::BrowsedSave,
            )
        }
        Msg::BrowsedSave(Some(dir)) => {
            let name = pending_destination(st).filename.unwrap_or_default();
            st.save_path = dir.join(name).display().to_string();
            st.dirty_source = true;
            mark(st);
            Task::none()
        }
        Msg::BrowsedSave(None) => Task::none(),
        Msg::CopyUrl => confirm_copy(st, CopyTarget::Url, st.url.clone()),
        Msg::ProxyModeSel(i) => {
            if let Some(mode) = PROXY_MODE_VALUES.get(i) {
                st.proxy.mode = *mode;
                mark(st);
            }
            Task::none()
        }
        Msg::ProxyHost(v) => {
            st.proxy.host = v;
            mark(st);
            Task::none()
        }
        Msg::ProxyPort(v) => {
            st.proxy.port = v;
            mark(st);
            Task::none()
        }
        Msg::ProxyAuth(v) => {
            st.proxy.auth_enabled = v;
            mark(st);
            Task::none()
        }
        Msg::ProxyUser(v) => {
            st.proxy.username = v;
            mark(st);
            Task::none()
        }
        Msg::ProxyPass(v) => {
            st.proxy.password = v;
            st.proxy.password_edited = true;
            mark(st);
            Task::none()
        }
        Msg::RemoteDns(v) => {
            st.proxy.remote_dns = v;
            mark(st);
            Task::none()
        }
        Msg::AuthSchemeSel(i) => {
            if let Some(scheme) = AUTH_SCHEME_VALUES.get(i) {
                st.auth.scheme = *scheme;
                mark(st);
            }
            Task::none()
        }
        Msg::AuthUser(v) => {
            st.auth.username = v;
            mark(st);
            Task::none()
        }
        Msg::AuthPass(v) => {
            st.auth.password = v;
            st.auth.secret_edited = true;
            mark(st);
            Task::none()
        }
        Msg::AuthToken(v) => {
            st.auth.token = v;
            st.auth.secret_edited = true;
            mark(st);
            Task::none()
        }
        Msg::AuthSecretClear => {
            st.auth.password.clear();
            st.auth.token.clear();
            st.auth.secret_edited = true;
            mark(st);
            Task::none()
        }
        Msg::ProxyPassClear => {
            st.proxy.password.clear();
            st.proxy.password_edited = true;
            mark(st);
            Task::none()
        }
        Msg::CookiesEnabled(v) => {
            st.cookies_enabled = v;
            st.dirty_overlay = true;
            mark(st);
            Task::none()
        }
        Msg::CookiesEdit(a) => {
            let edit = a.is_edit();
            st.cookies.perform(a);
            if edit {
                st.cookies_edited = true;
                st.dirty_overlay = true;
                mark(st);
            }
            Task::none()
        }
        Msg::CookiesClear => {
            st.cookies = text_editor::Content::new();
            st.cookies_edited = true;
            st.dirty_overlay = true;
            mark(st);
            Task::none()
        }
        Msg::UserAgent(v) => {
            st.ua = v;
            st.dirty_overlay = true;
            mark(st);
            Task::none()
        }
        Msg::Referer(v) => {
            st.referer = v;
            st.dirty_overlay = true;
            mark(st);
            Task::none()
        }
        Msg::HeaderName(i, v) => {
            if let Some(h) = st.headers.get_mut(i) {
                h.0 = v;
                st.dirty_overlay = true;
                mark(st);
            }
            Task::none()
        }
        Msg::HeaderValue(i, v) => {
            if let Some(h) = st.headers.get_mut(i) {
                h.1 = v;
                st.dirty_overlay = true;
                mark(st);
            }
            Task::none()
        }
        Msg::HeaderRemove(i) => {
            if i < st.headers.len() {
                st.headers.remove(i);
                st.dirty_overlay = true;
                mark(st);
            }
            Task::none()
        }
        Msg::HeaderAdd => {
            st.headers.push((String::new(), String::new()));
            st.dirty_overlay = true;
            mark(st);
            Task::none()
        }
        // Guards mirror the disabled controls: a message can still
        // arrive from a click that raced the phase changing under it.
        Msg::MaxConn(_)
        | Msg::UseLimiter(_)
        | Msg::LimitValue(_)
        | Msg::LimitUnit(_)
        | Msg::SpeedPreset(_)
        | Msg::SetCategory(_)
            if st.locked() =>
        {
            Task::none()
        }
        Msg::MaxConn(v) => {
            st.max_conn = v.trim().to_owned();
            mark(st);
            Task::none()
        }
        Msg::UseLimiter(on) => {
            st.limit_on = on;
            // A limit switched on with no number is not a limit. The
            // default is the one the presets start from.
            if on && st.limit_value.trim().is_empty() {
                st.limit_value = LIMIT_SEED_KBS.to_string();
                st.limit_unit_mb = false;
            }
            mark(st);
            Task::none()
        }
        Msg::LimitValue(v) => {
            st.limit_value = v.trim().to_owned();
            mark(st);
            Task::none()
        }
        Msg::LimitUnit(mb) => {
            st.limit_unit_mb = mb;
            mark(st);
            Task::none()
        }
        Msg::SpeedPreset(kbs) => {
            // Pressing a preset *is* the request to limit, so it turns
            // the switch on the way the download window's does.
            st.limit_on = true;
            st.limit_unit_mb = kbs >= 1024 && kbs.is_multiple_of(1024);
            st.limit_value = if st.limit_unit_mb {
                (kbs / 1024).to_string()
            } else {
                kbs.to_string()
            };
            mark(st);
            Task::none()
        }
        Msg::SetCategory(label) => {
            if let Some(c) = crate::domain::Category::ALL_ASSIGNABLE
                .iter()
                .find(|c| c.label() == label)
            {
                st.category = *c;
                mark(st);
            }
            Task::none()
        }
        Msg::SetQueue(name) => {
            if let Some((id, _)) = st.queues.iter().find(|(_, n)| *n == name) {
                st.queue = *id;
                mark(st);
            }
            Task::none()
        }
        Msg::CsAddOpen => {
            st.cs_adding = true;
            st.checksum_hash = text_editor::Content::new();
            st.cs_auto = true;
            // Auto-pick the first algorithm not already in the list
            // (mock AddChecksumForm behavior).
            st.cs_algo = Algo::ALL
                .iter()
                .copied()
                .find(|a| !st.checksums.iter().any(|c| c.algo == *a))
                .unwrap_or(Algo::Sha256);
            Task::none()
        }
        Msg::CsAddCancel => {
            st.cs_adding = false;
            st.checksum_hash = text_editor::Content::new();
            Task::none()
        }
        Msg::CsAlgoPick(i) => {
            if let Some(a) = Algo::ALL.get(i) {
                st.cs_algo = *a;
                // A manual pick overrides auto-detection (mock: picker
                // click clears the autoDetect flag).
                st.cs_auto = false;
            }
            Task::none()
        }
        Msg::CsAuto(v) => {
            st.cs_auto = v;
            Task::none()
        }
        Msg::ChecksumHash(a) => {
            st.checksum_hash.perform(a);
            Task::none()
        }
        Msg::ChecksumSave => {
            let form = cs_form(st);
            if !form.valid {
                return Task::none();
            }
            st.checksums.push(Checksum {
                algo: form.algo,
                hash: form.canon,
                source: CsSource::User,
                status: CsStatus::Unverified,
                expected: None,
            });
            st.cs_adding = false;
            st.checksum_hash = text_editor::Content::new();
            persist_checksums(st)
        }
        Msg::ChecksumRemove(i) => {
            // Guards mirror the disabled button: no removal while the
            // download runs, and server-verified hashes are permanent
            // (design §3.5 lock rules).
            if let Some(cs) = st.checksums.get(i) {
                let protected = cs.source == CsSource::Server && cs.status == CsStatus::Verified;
                if !st.locked() && !protected {
                    st.checksums.remove(i);
                    return persist_checksums(st);
                }
            }
            Task::none()
        }
        Msg::CsVerify(i) => {
            if st.entry.verifying {
                return Task::none();
            }
            if st.checksums.get(i).is_none() {
                return Task::none();
            }
            // Verification hashes the finished file on disk — only
            // possible once the job has a final path.
            if st.entry.job.status.final_path.is_none() {
                return Task::none();
            }
            st.cs_verify_error = None;
            let client = st.client.clone();
            let id = st.id;
            // Save first: the daemon hashes against the rows it has,
            // and a hash the user just typed is not one of them yet.
            // Then hand the work over — it reads a file this window
            // cannot promise to outlive, and the verdict belongs on the
            // job rather than in one dialog's local copy.
            Task::batch([
                persist_checksums(st),
                Task::perform(
                    async move { client.verify_checksums(id).await },
                    |r| match r {
                        Ok(()) => Msg::Noop,
                        Err(e) => Msg::CsVerifyFailed(e),
                    },
                ),
            ])
        }
        Msg::CsVerifyFailed(e) => {
            st.cs_verify_error = Some(e);
            Task::none()
        }
        Msg::CsCopy(what, s) => confirm_copy(st, what, s),
        Msg::CopyExpired => {
            st.copied = None;
            Task::none()
        }
        Msg::OpenFolder => {
            // Reveal the file rather than open the folder around it,
            // and fall back to the folder only when there is nothing on
            // disk to point at yet.
            let file = st.entry.job.status.final_path.clone().unwrap_or_else(|| {
                st.entry
                    .job
                    .save_dir
                    .join(st.entry.job.filename.as_deref().unwrap_or(""))
            });
            if file.is_file() {
                crate::platform::reveal_in_folder(&file);
            } else {
                crate::platform::open_path(&st.entry.job.save_dir);
            }
            Task::none()
        }
        Msg::CloseWin => iced::exit(),
        Msg::Discard => {
            hydrate(st);
            st.dirty = false;
            st.dirty_source = false;
            st.dirty_overlay = false;
            st.dirty_count = 0;
            Task::none()
        }
        Msg::Apply => {
            if st.locked() || st.proxy_invalid() {
                return Task::none();
            }
            let client = st.client.clone();
            let id = st.id;
            let Ok(url) = st.url.trim().parse::<url::Url>() else {
                st.error = Some("Invalid URL".to_owned());
                return Task::none();
            };
            let Ok(referrer) = pending_referrer(st) else {
                st.error = Some(
                    "Referer must be a full address, like https://example.com/page".to_owned(),
                );
                return Task::none();
            };
            if let Some(name) = pending_name(st).filter(|n| name_taken(st, n)) {
                st.error = Some(format!(
                    "Another download is already called `{name}`. One name, one download."
                ));
                return Task::none();
            }
            let dest = pending_destination(st);
            let (save_dir, filename) = (dest.dir, dest.filename);

            let adv = pending_advanced(st);
            // Header/cookie edits need `UpdateJobLocation` — the only
            // IPC that persists `Job.headers` + `enc_cookies`. Pure
            // URL/save edits take the narrower `SetJobSource`, which
            // cannot disturb stored secrets or headers.
            let job = &st.entry.job;
            let edit = st.dirty_overlay.then(|| {
                // Nameless rows are still being typed; case-duplicates
                // fold onto the first spelling (`normalize_headers`), so
                // what is stored is what the wire would resolve to.
                let headers = crate::domain::normalize_headers(pending_headers(st));
                crate::ipc_local::protocol::JobEdit {
                    url: url.clone(),
                    save_dir: save_dir.clone(),
                    filename: filename.clone(),
                    referrer: referrer.clone(),
                    max_connections: job.max_connections,
                    // Credentials are not this Apply's business: the
                    // Connection tab writes them through
                    // `SetJobAdvanced` a few lines down, and sending
                    // them twice would be two chances to disagree.
                    creds: None,
                    headers,
                    cookies: st.cookies_enabled.then(|| st.cookies.text()),
                }
            });
            let source_dirty = st.dirty_source;
            let category = (st.category != st.entry.job.category).then_some(st.category);
            let queue = (st.queue != st.entry.job.queue_id).then_some(st.queue);
            // Sent only when they differ: both are separate daemon
            // calls that rewrite the job, and re-sending an unchanged
            // value would churn the row for nothing.
            let max_conn = pending_max_conn(st);
            let max_conn = (max_conn != st.entry.job.max_connections).then_some(max_conn);
            let speed = pending_speed_limit(st);
            let speed = (speed != st.entry.job.speed_limit_override).then_some(speed);
            // Optimistic: `dirty` re-arms on Applied(Err); the
            // sub-flags only clear once the daemon confirmed.
            st.dirty = false;
            Task::perform(
                async move {
                    if let Some(edit) = edit {
                        client.update_job_location(id, edit).await?;
                    } else if source_dirty {
                        client.set_job_source(id, url, save_dir, filename).await?;
                    }
                    if let Some(category) = category {
                        client.set_job_category(id, category).await?;
                    }
                    if let Some(queue) = queue {
                        client.set_job_queue(id, queue).await?;
                    }
                    if let Some(max_conn) = max_conn {
                        client.set_max_connections(id, max_conn).await?;
                    }
                    if let Some(speed) = speed {
                        client.set_persistent_speed_limit(id, speed).await?;
                    }
                    client.set_job_advanced(id, adv).await
                },
                Msg::Applied,
            )
        }
        Msg::Applied(Ok(())) => {
            st.dirty_source = false;
            st.dirty_overlay = false;
            // The daemon consumed the clears; a later Apply must not
            // re-send them off stale flags.
            st.proxy.password_edited = false;
            st.auth.secret_edited = false;
            st.cookies_edited = false;
            Task::none()
        }
        Msg::Applied(Err(e)) => {
            st.error = Some(e);
            st.dirty = true;
            st.dirty_count = count_changes(st);
            Task::none()
        }
        Msg::WinResized(w, h) => {
            chrome::enforce_min_size(iced::Size::new(w, h), iced::Size::new(600.0, 718.0))
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

/// Persist the staged checksum list (checksum edits apply immediately,
/// independent of the footer Apply).
fn persist_checksums(st: &State) -> Task<Msg> {
    let client = st.client.clone();
    let id = st.id;
    let cs = st.checksums.clone();
    Task::perform(
        async move { client.set_job_checksums(id, cs).await },
        |_| Msg::Noop,
    )
}

/// Canonical hex form of a pasted hash (design §3.4 Properties
/// AddChecksumForm): whitespace-separated tokens are scanned for a hex
/// run of a supported length — this strips `sha256sum`-style
/// "hash  filename" companions, "filename: hash" prefixes and
/// "SHA256:hex" tags. Falls back to whitespace-stripping. Lowercased.
fn canonical_hash(input: &str) -> String {
    for tok in input.split_whitespace() {
        let t = tok.rsplit(':').next().unwrap_or(tok).to_ascii_lowercase();
        if !t.is_empty() && t.bytes().all(|b| b.is_ascii_hexdigit()) && detect_algo(&t).is_some() {
            return t;
        }
    }
    input
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase()
}

/// Algorithm whose canonical hex length (`Algo::hex_len`) matches the
/// pasted hash exactly.
fn detect_algo(canon: &str) -> Option<Algo> {
    if canon.is_empty() || !canon.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Algo::ALL
        .iter()
        .copied()
        .find(|a| a.hex_len() == canon.len())
}

/// Live validation state of the add-checksum form.
struct CsForm {
    canon: String,
    /// Effective algorithm: auto-detected from hex length when the
    /// auto toggle is on, else the picker's choice.
    algo: Algo,
    /// Auto-detection is currently driving the algorithm.
    detected: bool,
    hex_ok: bool,
    duplicate: bool,
    valid: bool,
}

fn cs_form(st: &State) -> CsForm {
    let canon = canonical_hash(&st.checksum_hash.text());
    let hex_ok = canon.is_empty() || canon.bytes().all(|b| b.is_ascii_hexdigit());
    let det = detect_algo(&canon);
    let algo = if st.cs_auto {
        det.unwrap_or(st.cs_algo)
    } else {
        st.cs_algo
    };
    let duplicate = st.checksums.iter().any(|c| c.algo == algo);
    let valid = !canon.is_empty() && hex_ok && canon.len() == algo.hex_len() && !duplicate;
    CsForm {
        detected: st.cs_auto && det.is_some(),
        canon,
        algo,
        hex_ok,
        duplicate,
        valid,
    }
}

pub fn subscription(app: &App) -> Subscription<Msg> {
    let App::Ready(st) = app else {
        return Subscription::none();
    };
    let mut subs = vec![
        iced::event::listen_with(|event, _status, _id| match event {
            iced::Event::Window(iced::window::Event::Resized(size)) => {
                Some(Msg::WinResized(size.width, size.height))
            }
            _ => None,
        }),
        crate::gui::ipc::all_events(crate::ipc_local::protocol::GuiKind::Properties(st.id))
            .map(Msg::Daemon),
    ];
    if st.shot.is_some() {
        subs.push(Shot::frames().map(|_| Msg::ShotTick));
    }
    Subscription::batch(subs)
}

// ---------------------------------------------------------------- view

pub fn view(app: &App) -> Element<'_, Msg> {
    chrome::framed(match app {
        App::Connecting => splash("Connecting…".to_owned()),
        App::Failed(e) => splash(e.clone()),
        App::Ready(st) => ready_view(st),
    })
}

fn splash<'a>(msg: String) -> Element<'a, Msg> {
    let t = Tokens::dark();
    container(text(msg).color(t.fg_2))
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

fn tabbtn<'a>(t: &Tokens, label: &'a str, icon: &'a str, tab: Tab, cur: Tab) -> Element<'a, Msg> {
    tabbtn_counted(t, label, icon, tab, cur, None)
}

/// A tab that carries how much is behind it — the count belongs on the
/// tab rather than in a row on another page saying "3 saved" and
/// pointing here.
fn tabbtn_counted<'a>(
    t: &Tokens,
    label: &'a str,
    icon: &'a str,
    tab: Tab,
    cur: Tab,
    count: Option<usize>,
) -> Element<'a, Msg> {
    let mut b = TabBtn::new(label)
        .icon(icon)
        .icon_size(13.0)
        .height(35.0)
        .font_size(12.0)
        .active(tab == cur)
        .on_press(Msg::SetTab(tab));
    if let Some(n) = count.filter(|n| *n > 0) {
        b = b.count(n as u64);
    }
    b.view(t)
}

/// Put `text` on the clipboard and let the button that asked say so.
fn confirm_copy(st: &mut State, what: CopyTarget, text: String) -> Task<Msg> {
    st.copied = Some(what);
    Task::batch([
        iced::clipboard::write(text),
        Task::perform(crate::gui::widget::copy::expire(), |()| Msg::CopyExpired),
    ])
}

/// The two limits the download window's Speed tab drives live, in the
/// form this dialog uses for everything else: staged, applied together,
/// and about the *next* run rather than the one in flight — which is
/// why they lock while the job is running, and why the live pair stays
/// where it can take effect immediately.
fn transfer_section(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let editable = !st.locked();

    // Blank = auto (the daemon picks); a number is an explicit 1–16
    // override, the same reading the Speed tab gives it.
    let conn_auto = pending_max_conn(st).is_none();
    let conn_val = pending_max_conn(st)
        .map(|n| n as i64)
        .unwrap_or(MAX_CONN_DEFAULT);
    let mut conns = row![crate::gui::widget::segmented(
        t,
        &[("Auto", None), ("Custom", None)],
        if conn_auto { 0 } else { 1 },
        BtnSize::Md,
        move |i| Msg::MaxConn(if i == 0 {
            String::new()
        } else {
            MAX_CONN_DEFAULT.to_string()
        }),
    )]
    .spacing(theme::space::S2)
    .align_y(Alignment::Center);
    if !conn_auto {
        conns = conns.push(crate::gui::widget::number_stepper(
            t,
            conn_val,
            MAX_CONN_MIN,
            MAX_CONN_MAX,
            true,
            false,
            |n| Msg::MaxConn(n.to_string()),
        ));
    }

    let value_row = row![
        // Editable while the switch is off: the number is the limit you
        // *would* apply, and nothing is sent until Apply.
        TextInput::new(&st.limit_value)
            .width(Length::Fixed(LIMIT_INPUT_W))
            .enabled(editable)
            .on_input(Msg::LimitValue)
            .view(t),
        crate::gui::widget::segmented(
            t,
            &[("KB/s", None), ("MB/s", None)],
            if st.limit_unit_mb { 1 } else { 0 },
            BtnSize::Md,
            |i| Msg::LimitUnit(i == 1),
        ),
    ]
    .spacing(theme::space::S2)
    .align_y(Alignment::Center);

    let mut presets = row![].spacing(theme::space::S2).align_y(Alignment::Center);
    for (label, kbs) in SPEED_PRESETS_KBS {
        presets = presets.push(
            Btn::new(*label)
                .secondary()
                .size(BtnSize::Sm)
                .enabled(editable)
                .on_press(Msg::SpeedPreset(*kbs))
                .view(t),
        );
    }

    // The Speed tab's own shape — label and hint on the left, the
    // control at the right edge — so the same two settings read the
    // same way in both windows. Only the surface differs: this tab puts
    // its rows on a card.
    let rows: Vec<Element<'_, Msg>> = if editable {
        vec![
            set_row(
                t,
                "Max parallel connections",
                Some("Auto lets oxdm choose. Applies to the next run of this download."),
                conns.into(),
            ),
            row_sep(t),
            set_row(
                t,
                "Speed limit",
                Some("Caps this download alone, on top of the global limit."),
                toggle(t, st.limit_on, editable, Msg::UseLimiter),
            ),
            set_row(t, "Limit to", None, value_row.into()),
            set_row(t, "Quick set", None, presets.into()),
        ]
    } else {
        // Locked: the values still answer "what is this download set
        // to", which is the question Properties is open to answer.
        vec![
            kv_row(
                t,
                "Max parallel connections",
                match pending_max_conn(st) {
                    Some(n) => n.to_string(),
                    None => "Auto".to_owned(),
                },
                false,
            ),
            row_sep(t),
            kv_row(
                t,
                "Speed limit",
                match pending_speed_limit(st) {
                    Some(bps) => crate::gui::format::format_speed(bps as f64),
                    None => "Unlimited".to_owned(),
                },
                false,
            ),
        ]
    };
    let mut body = column![];
    for r in rows {
        body = body.push(r);
    }
    labeled_section(t, "transfer", body.into())
}

/// A failed row, laid out the way the download window's file-integrity
/// table lays it out: the algorithm, a mismatch chip, and the
/// expected/got pair with the computed digest struck through — plus the
/// source and per-row actions this tab carries.
#[allow(clippy::too_many_arguments)]
fn mismatch_row<'a>(
    t: &Tokens,
    cs: &'a Checksum,
    i: usize,
    got: &str,
    source_label: &'a str,
    actions: Element<'a, Msg>,
    copied: Option<CopyTarget>,
) -> Element<'a, Msg> {
    use crate::gui::widget::integrity;
    let t2 = *t;
    let (chip_bg, chip_fg, label, icon) = integrity::Verdict::Mismatch.chip(t);
    let values = column![
        integrity::hash_line(
            t,
            "expected",
            &cs.hash,
            false,
            copied == Some(CopyTarget::Expected(i)),
            Msg::CsCopy(CopyTarget::Expected(i), cs.hash.clone()),
        ),
        integrity::hash_line(
            t,
            "got",
            got,
            true,
            copied == Some(CopyTarget::Got(i)),
            Msg::CsCopy(CopyTarget::Got(i), got.to_owned()),
        ),
    ]
    .spacing(theme::space::S1);
    container(
        row![
            container(
                text(cs.algo.label().to_owned())
                    .font(theme::MONO_BOLD)
                    .size(integrity::ALGO_SIZE)
                    .color(t.fg_1)
            )
            .width(Length::Fixed(integrity::ALGO_W))
            .height(Length::Fixed(integrity::LINE_H))
            .align_y(Alignment::Center),
            container(integrity::chip(icon, label, chip_bg, chip_fg))
                .width(Length::Fixed(integrity::STATUS_W))
                .height(Length::Fixed(integrity::LINE_H))
                .align_y(Alignment::Center),
            container(values).width(Length::Fill),
            container(
                text(source_label)
                    .font(theme::MONO)
                    .size(10.0)
                    .color(t.fg_3)
            )
            .height(Length::Fixed(integrity::LINE_H))
            .align_y(Alignment::Center),
            container(actions)
                .height(Length::Fixed(integrity::LINE_H))
                .align_y(Alignment::Center),
        ]
        .spacing(theme::space::S2)
        .align_y(Alignment::Start),
    )
    .width(Length::Fill)
    .padding([integrity::PAD_Y, integrity::PAD_X])
    .style(move |_| container::Style {
        background: Some(t2.status_danger_bg.into()),
        border: iced::Border {
            color: integrity::DANGER_EDGE,
            width: 1.0,
            radius: theme::radius::XS.into(),
        },
        ..Default::default()
    })
    .into()
}

/// What the limit field starts at when the switch is turned on with no
/// number in it — the smallest preset, so an accidental toggle costs
/// throughput rather than surprising the user with a large cap.
const LIMIT_SEED_KBS: u64 = 64;

/// Width of the General tab's dropdowns — enough for the longest queue
/// name without the control swallowing the row.
const PICKER_W: f32 = 220.0;

/// A labelled row whose value is a control rather than text.
fn picker_row<'a>(t: &Tokens, label: &'a str, control: Element<'a, Msg>) -> Element<'a, Msg> {
    row![
        text(label)
            .font(theme::BODY_MEDIUM)
            .size(12.0)
            .color(t.fg_1),
        iced::widget::Space::new().width(Length::Fill),
        control,
    ]
    .align_y(Alignment::Center)
    .padding([6.0, theme::space::S3])
    .into()
}

fn kv_row<'a>(t: &Tokens, label: &'a str, value: String, mono: bool) -> Element<'a, Msg> {
    row![
        text(label)
            .font(theme::BODY_MEDIUM)
            .size(12.0)
            .color(t.fg_1),
        iced::widget::Space::new().width(Length::Fill),
        text(value)
            .font(if mono { theme::MONO } else { theme::BODY })
            .size(if mono { 11.0 } else { 13.0 })
            .color(t.fg_2),
    ]
    .align_y(Alignment::Center)
    .padding([10.0, theme::space::S3])
    .into()
}

fn row_sep<'a>(t: &Tokens) -> Element<'a, Msg> {
    hairline(t.border_subtle)
}

fn ready_view(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let tabs = container(
        row![
            tabbtn(t, "General", "info", Tab::General, st.tab),
            tabbtn_counted(
                t,
                "Checksums",
                "shield-check",
                Tab::Checksums,
                st.tab,
                Some(st.checksums.len()),
            ),
            tabbtn(t, "Connection", "globe", Tab::Connection, st.tab),
            tabbtn(t, "Cookies", "cookie", Tab::Cookies, st.tab),
            tabbtn(t, "Headers", "list", Tab::Headers, st.tab),
        ]
        .spacing(theme::space::S1),
    )
    .padding(iced::Padding {
        left: theme::space::S3,
        right: theme::space::S3,
        ..Default::default()
    });

    let tab_body: Element<'_, Msg> = match st.tab {
        Tab::General => general_tab(st),
        Tab::Checksums => checksums_tab(st),
        Tab::Connection => connection_tab(st),
        Tab::Cookies => cookies_tab(st),
        Tab::Headers => headers_tab(st),
    };
    // Lock banner tops every pane that has something locked in it.
    // General is included: its address, save location and category are
    // all read by the run that is in flight. Checksums is not — its
    // carve-out is explained inline, and nothing there is disabled.
    let show_lock_banner = st.locked() && st.tab != Tab::Checksums;
    let body: Element<'_, Msg> = if show_lock_banner {
        column![lock_banner(t), tab_body]
            .spacing(theme::space::S3)
            .into()
    } else {
        tab_body
    };

    let footer_el = footer(
        t,
        Btn::new(crate::platform::reveal_label())
            .toolbar()
            .icon("folder")
            .on_press(Msg::OpenFolder)
            .view(t),
        {
            let mut right = row![].spacing(theme::space::S2).align_y(Alignment::Center);
            if st.dirty {
                // No "● unsaved" dot: the Discard button beside it
                // appears for the same reason and already counts what
                // is staged. Settings and Queues say it once too.
                right = right.push(
                    Btn::new(format!(
                        "Discard {} change{}",
                        st.dirty_count.max(1),
                        if st.dirty_count == 1 { "" } else { "s" }
                    ))
                    .ghost()
                    .accent(true)
                    .icon("rotate-cw")
                    .on_press(Msg::Discard)
                    .view(t),
                );
            }
            right
                .push(Btn::new("Close").ghost().on_press(Msg::CloseWin).view(t))
                .push(
                    Btn::new("Apply")
                        .primary()
                        .icon("check")
                        .enabled(st.dirty && !st.locked() && !st.proxy_invalid())
                        .on_press(Msg::Apply)
                        .view(t),
                )
                .into()
        },
    );

    // Titlebar with a lock chip while the transfer runs (design
    // `.prop-titlebar-lock`). The chip is stacked over the (centered)
    // title's left gutter; a plain container passes pointer events
    // through to the drag region below.
    // Without a painted bar (macOS) there is nothing to stack the chip
    // over; the locked state still reads from the Apply/field states.
    let bar: Element<'_, Msg> = titlebar::titlebar(t, &st.window_title(), false, Msg::Window);
    let bar: Element<'_, Msg> = if st.locked() && titlebar::use_custom() {
        iced::widget::stack![
            bar,
            container(titlebar_lock_chip(t))
                .height(Length::Fixed(titlebar::HEIGHT))
                .align_y(Alignment::Center)
                .padding(iced::Padding {
                    left: theme::space::S3,
                    ..Default::default()
                }),
        ]
        .into()
    } else {
        bar
    };

    let t2 = *t;
    let page = column![
        bar,
        tabs,
        hairline(t.border_subtle),
        crate::gui::widget::vscroll(
            container(body)
                .padding(iced::Padding {
                    top: theme::space::S3,
                    bottom: theme::space::S3,
                    left: theme::space::S3,
                    right: theme::space::S3 - crate::gui::widget::SCROLL_GUTTER,
                })
                .width(Length::Fill)
        )
        .height(Length::Fill),
        hairline(t.border_subtle),
        footer_el,
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

/// Titlebar lock chip (design `.prop-titlebar-lock`: uppercase 9.5px,
/// 2×6 padding, 4px radius, low-alpha fill).
fn titlebar_lock_chip(t: &Tokens) -> Element<'_, Msg> {
    let t2 = *t;
    container(
        row![
            icons::icon("lock", 10.0, t.fg_3),
            text("DOWNLOADING")
                .font(theme::BODY_BOLD)
                .size(9.5)
                .color(t.fg_3),
        ]
        .spacing(3.0)
        .align_y(Alignment::Center),
    )
    .padding([2.0, 6.0])
    .style(move |_| container::Style {
        // rgba(0,0,0,.06) in the mock — derived from fg so it reads on
        // both themes.
        background: Some(color::with_alpha(t2.fg_1, 0.06).into()),
        border: iced::Border {
            radius: 4.0.into(),
            ..Default::default()
        },
        ..Default::default()
    })
    .into()
}

/// Read-only banner above editable panes while the download runs
/// (design `.prop-lock-banner`: sunken card, lock icon, bold lead).
fn lock_banner(t: &Tokens) -> Element<'_, Msg> {
    let t2 = *t;
    container(
        row![
            icons::icon("lock", 13.0, t.fg_3),
            column![
                text("Settings are read-only while this download is running.")
                    .font(theme::BODY_BOLD)
                    .size(11.5)
                    .color(t.fg_1),
                text(
                    "Pause it to edit its address, save location, category, or \
                     connection and transfer settings; changes take effect when you \
                     resume. The queue it runs in and its checksums can be changed at \
                     any time."
                )
                .font(theme::BODY)
                .size(11.5)
                .color(t.fg_2)
                .line_height(iced::widget::text::LineHeight::Relative(1.5)),
            ]
            .spacing(2.0),
        ]
        .spacing(theme::space::S2),
    )
    .width(Length::Fill)
    .padding([10.0, theme::space::S3])
    .style(move |_| container::Style {
        background: Some(t2.bg_sunken.into()),
        border: iced::Border {
            color: t2.border_subtle,
            width: 1.0,
            radius: 8.0.into(),
        },
        ..Default::default()
    })
    .into()
}

fn general_tab(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let t2 = *t;
    let job = &st.entry.job;
    let name = job.filename.clone().unwrap_or_default();
    let ext = crate::gui::format::ext_label(job.filename.as_deref());
    let total = st.entry.counters.total;
    let phase = st.entry.counters.phase;
    let (phase_color, phase_label) = match phase {
        Phase::Completed => (t.status_success, "COMPLETE"),
        Phase::Failed => (t.status_danger, "FAILED"),
        Phase::Conflict => (t.status_warning, "NEEDS YOUR ANSWER"),
        Phase::Paused => (t.fg_3, "PAUSED"),
        Phase::Queued => (t.status_info, "QUEUED"),
        Phase::Cancelled => (t.fg_3, "CANCELLED"),
        _ => (t.action_primary, "DOWNLOADING"),
    };

    let tile_bg = color::mix(t.bg_surface, t.action_primary, 0.20);
    let hero = container(
        row![
            container(crate::gui::widget::ellipsized_lines(
                ext,
                theme::MONO_BOLD,
                12.0,
                t.action_primary,
                2,
            ))
            .width(Length::Fixed(56.0))
            .height(Length::Fixed(56.0))
            .padding([0.0, crate::gui::windows::download::EXT_TILE_PAD])
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .style(move |_| container::Style {
                background: Some(tile_bg.into()),
                border: iced::Border {
                    radius: theme::radius::SM.into(),
                    ..Default::default()
                },
                ..Default::default()
            }),
            column![
                text(name.clone())
                    .font(theme::BODY_BOLD)
                    .size(14.0)
                    .color(t.fg_1),
                text(total.map(format_bytes_2).unwrap_or_else(|| "—".into()))
                    .font(theme::MONO)
                    .size(11.0)
                    .color(t.fg_3),
            ]
            .spacing(4.0),
            iced::widget::Space::new().width(Length::Fill),
            container(
                row![
                    crate::gui::widget::dot(6.0, phase_color),
                    text(phase_label)
                        .font(theme::BODY_BOLD)
                        .size(10.0)
                        .color(phase_color),
                ]
                .spacing(6.0)
                .align_y(Alignment::Center)
            )
            .padding([6.0, 9.0])
            .style(move |_| container::Style {
                background: Some(t2.bg_page.into()),
                border: iced::Border {
                    color: t2.border_subtle,
                    width: 1.0,
                    radius: theme::radius::SM.into(),
                },
                ..Default::default()
            }),
        ]
        .spacing(theme::space::S3)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding(theme::space::S3)
    .style(move |_| container::Style {
        background: Some(t2.bg_surface.into()),
        border: iced::Border {
            color: t2.border_subtle,
            width: 1.0,
            radius: theme::surface::RADIUS.into(),
        },
        ..Default::default()
    });

    let size_str = match total {
        Some(b) => format!("{}  ({} bytes)", format_bytes_2(b), format_int_grouped(b)),
        None => "—".to_owned(),
    };
    let editable = !st.locked();

    let file_section = labeled_section(
        t,
        "file",
        column![
            kv_row(t, "Name", name, true),
            row_sep(t),
            picker_row(
                t,
                "Category",
                // Locked while it runs: the folder this download is
                // writing into was decided when it started, and
                // re-filing it now would move the label without moving
                // the file — a setting that appears to work and does
                // not.
                if editable {
                    combo(
                        t,
                        crate::domain::Category::ALL_ASSIGNABLE
                            .iter()
                            .map(|c| c.label().to_owned())
                            .collect(),
                        Some(st.category.label().to_owned()),
                        Msg::SetCategory,
                        Length::Fixed(PICKER_W),
                    )
                } else {
                    crate::gui::widget::locked_combo(
                        t,
                        Some(st.category.label().to_owned()),
                        Length::Fixed(PICKER_W),
                    )
                },
            ),
            row_sep(t),
            picker_row(
                t,
                "Queue",
                combo(
                    t,
                    st.queues.iter().map(|(_, n)| n.clone()).collect(),
                    st.queues
                        .iter()
                        .find(|(id, _)| *id == st.queue)
                        .map(|(_, n)| n.clone()),
                    Msg::SetQueue,
                    Length::Fixed(PICKER_W),
                ),
            ),
            row_sep(t),
            kv_row(t, "Size", size_str, true),
            row_sep(t),
            container(save_to_block(st, editable)).padding([10.0, theme::space::S3]),
        ]
        .into(),
    );

    let source_section = labeled_section(
        t,
        "source",
        column![
            container(
                column![
                    text("URL")
                        .font(theme::BODY_MEDIUM)
                        .size(12.0)
                        .color(t.fg_1),
                    row![
                        TextInput::new(&st.url)
                            .mono()
                            .enabled(editable)
                            .on_input(Msg::Url)
                            .view(t),
                        crate::gui::widget::copy::copy_btn(
                            "",
                            st.copied == Some(CopyTarget::Url),
                            Msg::CopyUrl,
                        )
                        .secondary()
                        .view(t),
                    ]
                    .spacing(6.0)
                    .align_y(Alignment::Center),
                ]
                .spacing(6.0)
            )
            .padding([10.0, theme::space::S3]),
            row_sep(t),
            kv_row(
                t,
                "Server",
                job.url.host_str().unwrap_or("—").to_owned(),
                false
            ),
            row_sep(t),
            kv_row(
                t,
                "Created",
                job.created_at
                    .with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M:%S")
                    .to_string(),
                false
            ),
        ]
        .into(),
    );

    // Run history: how rough the transfer was. One number, because the
    // question is "did this go cleanly", not which of retry, reconnect
    // or resume fired.
    let n = job.interruptions;
    let history = labeled_section(
        t,
        "history",
        container(
            row![
                column![
                    text("Interruptions")
                        .font(theme::BODY_MEDIUM)
                        .size(12.0)
                        .color(t.fg_1),
                    text("Dropped connections and resumes during this download.")
                        .font(theme::BODY)
                        .size(11.0)
                        .color(t.fg_3),
                ]
                .spacing(2.0),
                iced::widget::Space::new().width(Length::Fill),
                text(if n == 0 {
                    "None".to_owned()
                } else {
                    n.to_string()
                })
                .font(theme::BODY)
                .size(12.0)
                .color(if n == 0 { t.fg_3 } else { t.fg_2 }),
            ]
            .align_y(Alignment::Center),
        )
        .padding([10.0, theme::space::S3])
        .into(),
    );

    column![hero, file_section, source_section, history]
        .spacing(theme::space::S3)
        .into()
}

fn checksums_tab(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let t2 = *t;
    let mut col = column![].spacing(theme::space::S3);

    if st.checksums.is_empty() {
        col = col.push(
            container(
                row![
                    container(icons::icon("shield-question", 18.0, color::ochre::O400))
                        .width(Length::Fixed(36.0))
                        .height(Length::Fixed(36.0))
                        .align_x(Alignment::Center)
                        .align_y(Alignment::Center)
                        .style(move |_| container::Style {
                            background: Some(t2.bg_page.into()),
                            border: iced::Border {
                                color: t2.border_subtle,
                                width: 1.0,
                                radius: 8.0.into(),
                            },
                            ..Default::default()
                        }),
                    column![
                        text("No checksums on file")
                            .font(theme::BODY_MEDIUM)
                            .size(14.0)
                            .color(t.fg_1),
                        text(
                            "Add a hash from the publisher's website to verify the file's \
                             integrity. MD5, SHA-1, SHA-256, SHA-384 and SHA-512 are supported."
                        )
                        .font(theme::BODY)
                        .size(12.0)
                        .color(t.fg_3),
                    ]
                    .spacing(2.0),
                ]
                .spacing(theme::space::S3)
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding([12.0, 14.0])
            .style(move |_| container::Style {
                background: Some(t2.bg_surface.into()),
                border: iced::Border {
                    color: t2.border_subtle,
                    width: 1.0,
                    radius: theme::surface::RADIUS.into(),
                },
                ..Default::default()
            }),
        );
    }

    // Sage carve-out hint while locked (design `.prop-cs-lockhint`):
    // the ONE thing that stays editable, so positive tone, not warning.
    if st.locked() {
        col = col.push(cs_lockhint(t));
    }

    if !st.checksums.is_empty() {
        let can_verify = st.entry.job.status.final_path.is_some();
        let mut list = column![];
        for (i, cs) in st.checksums.iter().enumerate() {
            // A check is running for the job, not for a row — but only
            // the rows still waiting on a verdict are waiting on it.
            // Painting "Verifying…" over rows that already have one
            // took their answer away and gave nothing back.
            let job_verifying = st.entry.verifying;
            let verifying = job_verifying && cs.status == CsStatus::Unverified;
            let (status_color, status_label) = if verifying {
                // Indeterminate: hashing reports no progress, and the
                // daemon owns the run (honesty decision #5).
                (t.fg_3, "Verifying…")
            } else {
                match cs.status {
                    CsStatus::Verified => (t.status_success, "verified"),
                    CsStatus::Mismatch => (t.status_danger, "mismatch"),
                    CsStatus::Unverified => (t.fg_3, "unverified"),
                }
            };
            let source_label = match cs.source {
                CsSource::Server => "server",
                CsSource::Computed => "local hash",
                CsSource::User => "you",
            };
            let hash_short = if cs.hash.len() > 24 {
                format!("{}…{}", &cs.hash[..10], &cs.hash[cs.hash.len() - 10..])
            } else {
                cs.hash.clone()
            };
            // Server-verified hashes can never be removed; everything
            // is removable only while not running (design §3.5).
            let protected = cs.source == CsSource::Server && cs.status == CsStatus::Verified;
            let removable = !st.locked() && !job_verifying && !protected;
            let mut actions = row![].spacing(theme::space::S1).align_y(Alignment::Center);
            // Only a row with no verdict is worth checking. A verdict
            // already answers the question, and nothing can change it
            // without changing the hash or the file — either of which
            // clears the verdict and brings the button back.
            if cs.status == CsStatus::Unverified {
                actions = actions.push(
                    Btn::new("Verify")
                        .toolbar()
                        .icon("shield-check")
                        .size(BtnSize::Sm)
                        .font_size(10.0)
                        .enabled(can_verify && !job_verifying)
                        .on_press(Msg::CsVerify(i))
                        .view(t),
                );
            }
            // A failed row prints its hash on the EXPECTED line, which
            // carries its own copy button; a second one beside it
            // copies the same string twice.
            if cs.status != CsStatus::Mismatch {
                actions = actions.push(
                    crate::gui::widget::copy::copy_btn(
                        "",
                        st.copied == Some(CopyTarget::Hash(i)),
                        Msg::CsCopy(CopyTarget::Hash(i), cs.hash.clone()),
                    )
                    .toolbar()
                    .size(BtnSize::Sm)
                    .view(t),
                );
            }
            actions = actions.push(
                Btn::new("")
                    .toolbar()
                    .icon_only("trash-2")
                    .size(BtnSize::Sm)
                    .enabled(removable)
                    .on_press(Msg::ChecksumRemove(i))
                    .view(t),
            );
            // A failed row *is* the integrity table: the algorithm and
            // the verdict live in its first two columns, so printing
            // them again above it said the same thing twice.
            let diff = (cs.status == CsStatus::Mismatch)
                .then_some(cs.expected.as_ref())
                .flatten();
            let body: Element<'_, Msg> = match diff {
                Some(got) => mismatch_row(t, cs, i, got, source_label, actions.into(), st.copied),
                None => row![
                    container(
                        text(cs.algo.label().to_owned())
                            .font(theme::MONO)
                            .size(11.0)
                            .color(t.fg_1)
                    )
                    .width(Length::Fixed(80.0)),
                    crate::gui::widget::status_dot(status_color, status_label, 11.0),
                    container(text(hash_short).font(theme::MONO).size(11.0).color(t.fg_2))
                        .width(Length::Fill),
                    text(source_label)
                        .font(theme::MONO)
                        .size(10.0)
                        .color(t.fg_3),
                    actions,
                ]
                .spacing(theme::space::S3)
                .align_y(Alignment::Center)
                .into(),
            };
            let row_col = column![body].spacing(theme::space::S2);
            list = list.push(container(row_col).padding([8.0, theme::space::S3]));
            if i + 1 < st.checksums.len() {
                list = list.push(row_sep(t));
            }
        }
        col = col.push(labeled_section(t, "checksums", list.into()));
    }

    if let Some(e) = &st.cs_verify_error {
        col = col.push(
            text(format!("Couldn't verify: {e}"))
                .font(theme::BODY)
                .size(11.0)
                .color(t.status_danger),
        );
    }

    if st.cs_adding {
        col = col.push(add_checksum_form(st));
    } else {
        // Add affordance + supported-algo chip list (design
        // `.prop-cs-addrow`). Deliberately live while locked — the
        // Checksums carve-out.
        let all_taken = Algo::ALL
            .iter()
            .all(|a| st.checksums.iter().any(|c| c.algo == *a));
        let mut addrow = row![
            Btn::new("Add checksum manually")
                .secondary()
                .icon("plus")
                .enabled(!all_taken)
                .on_press(Msg::CsAddOpen)
                .view(t),
            iced::widget::Space::new().width(Length::Fill),
        ]
        .spacing(theme::space::S1)
        .align_y(Alignment::Center);
        for algo in Algo::ALL {
            addrow = addrow.push(crate::gui::widget::chip(t, algo.label()));
        }
        col = col.push(addrow);
    }
    col.into()
}

/// Sage "still editable" hint (design `.prop-cs-lockhint`: dashed sage
/// border, low-alpha sage fill; iced borders can't dash → solid 1px).
fn cs_lockhint(t: &Tokens) -> Element<'_, Msg> {
    let t2 = *t;
    container(
        row![
            icons::icon("shield-check", 11.0, t.status_success),
            text(
                "Adding checksums is allowed even while the download is running; \
                 verification doesn't touch the transfer."
            )
            .font(theme::BODY_MEDIUM)
            .size(11.0)
            .color(t.status_success),
        ]
        .spacing(6.0)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding([6.0, 10.0])
    .style(move |_| container::Style {
        background: Some(color::with_alpha(t2.status_success, 0.06).into()),
        border: iced::Border {
            color: color::with_alpha(t2.status_success, 0.30),
            width: 1.0,
            radius: theme::radius::XS.into(),
        },
        ..Default::default()
    })
    .into()
}

/// `.pac-algos`: inner padding around the seg-radio chips, each chip
/// 4px-rounded (`radius::CTRL` is 5 and reads too soft here). The chip
/// padding runs a pixel over the design's 5×10 on each axis: the design
/// measures against a browser line box, and iced's is a pixel tighter.
const SEG_BOX_PAD: f32 = 3.0;
const SEG_RADIUS: f32 = 4.0;
const SEG_PAD_Y: f32 = 6.0;
const SEG_PAD_X: f32 = 11.0;

/// `.pac-input`: mono 11.5px in a 52px-min textarea.
const HASH_TEXT: f32 = 11.5;
const HASH_MIN_H: f32 = 52.0;

/// AddChecksumForm card border. The design asks for 1.5px clay, which a
/// 1x display cannot draw evenly — the stroke snapped to 2px while the
/// 1.5px padding laid the strips out on half pixels, so the border read
/// as thicker on some edges. Draw a whole 1px instead.
const PAC_BORDER_W: f32 = 1.0;
/// Corner radius of the header/footer strips: the outer 10px radius
/// minus the border they sit inside, so the tinted fills follow the
/// card's rounding (tiny-skia has no clip, memory `with_clip` no-op).
const PAC_INNER_R: f32 = theme::radius::SM - PAC_BORDER_W;

/// Inline add-checksum form (design §3.4 Properties `AddChecksumForm`:
/// clay-bordered card with a tinted uppercase header strip, algo
/// seg-radio in a contained chip box with "taken" lockout, hash input
/// with live char-count meter + validity message, auto-detect toggle,
/// and a sunken footer strip).
fn add_checksum_form(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let t2 = *t;
    let form = cs_form(st);

    // `.pac-head`: clay-50 strip, uppercase clay title, close button.
    let head = container(
        row![
            icons::icon("circle-plus", 13.0, t.action_primary),
            text("ADD CHECKSUM MANUALLY")
                .font(theme::BODY_BOLD)
                .size(10.0)
                .color(t.action_primary_press),
            iced::widget::Space::new().width(Length::Fill),
            Btn::new("")
                .toolbar()
                .icon_only("x")
                .size(BtnSize::Sm)
                .on_press(Msg::CsAddCancel)
                .view(t),
        ]
        .spacing(6.0)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding([4.0, 12.0])
    .style(move |_| container::Style {
        background: Some(t2.row_selected_bg.into()),
        border: iced::Border {
            radius: iced::border::Radius {
                top_left: PAC_INNER_R,
                top_right: PAC_INNER_R,
                bottom_left: 0.0,
                bottom_right: 0.0,
            },
            ..Default::default()
        },
        ..Default::default()
    });

    // `.pac-algos` seg-radio: chips live inside a contained box (page
    // bg, 2px inner padding). Selected chip: surface fill + clay ring +
    // clay text; taken algorithms are locked out at half opacity
    // unless currently selected.
    let mut chips = row![].spacing(2.0);
    for (i, algo) in Algo::ALL.iter().enumerate() {
        let taken = st.checksums.iter().any(|c| c.algo == *algo);
        let on = form.algo == *algo;
        let enabled = !taken || on;
        chips = chips.push(
            iced::widget::button(text(algo.label()).font(theme::MONO_SEMIBOLD).size(11.0))
                .padding([SEG_PAD_Y, SEG_PAD_X])
                .style(move |_, status| {
                    use iced::widget::button::Status;
                    let hovered = matches!(status, Status::Hovered | Status::Pressed);
                    let (bg, text_color, ring) = if on {
                        (
                            Some(t2.bg_surface),
                            t2.action_primary_press,
                            t2.border_brand,
                        )
                    } else if !enabled {
                        (
                            None,
                            color::with_alpha(t2.fg_2, 0.5),
                            iced::Color::TRANSPARENT,
                        )
                    } else if hovered {
                        (Some(t2.bg_sunken), t2.fg_1, iced::Color::TRANSPARENT)
                    } else {
                        (None, t2.fg_2, iced::Color::TRANSPARENT)
                    };
                    iced::widget::button::Style {
                        background: bg.map(Into::into),
                        text_color,
                        border: iced::Border {
                            color: ring,
                            width: 1.0,
                            radius: SEG_RADIUS.into(),
                        },
                        ..Default::default()
                    }
                })
                .on_press_maybe(enabled.then(|| Msg::CsAlgoPick(i))),
        );
    }
    let chip_box = container(chips)
        .padding(SEG_BOX_PAD)
        .style(move |_| container::Style {
            background: Some(t2.bg_page.into()),
            border: iced::Border {
                color: t2.border_default,
                width: 1.0,
                radius: theme::radius::XS.into(),
            },
            ..Default::default()
        });
    let mut algo_row = row![chip_box].spacing(10.0).align_y(Alignment::Center);
    if form.detected && !form.canon.is_empty() {
        algo_row = algo_row.push(
            text("auto-detected")
                .font(theme::BODY_MEDIUM)
                .size(10.0)
                .color(t.action_primary_press),
        );
    }

    let target = form.algo.hex_len();
    let count = form.canon.chars().count();
    let count_color = if !form.hex_ok || count > target {
        t.status_danger
    } else if count == target {
        t.status_success
    } else {
        t.fg_2
    };
    let fill = if !form.hex_ok || count > target {
        t.status_danger
    } else if count == target {
        t.status_success
    } else {
        t.fg_4
    };
    // Live validation copy (design §3.4 — precise and friendly).
    let (msg_color, msg_text) = if count == 0 {
        (
            t.fg_3,
            "Paste a hex hash. Whitespace and a leading filename are removed automatically."
                .to_owned(),
        )
    } else if !form.hex_ok {
        (t.status_danger, "Contains non-hex characters.".to_owned())
    } else if count < target {
        (t.fg_3, format!("{} more characters needed", target - count))
    } else if count > target {
        (
            t.status_danger,
            format!(
                "{} too many. This is too long for {}.",
                count - target,
                form.algo.label()
            ),
        )
    } else if form.duplicate {
        (
            t.status_danger,
            format!(
                "{} is already in the list. Remove it first to replace.",
                form.algo.label()
            ),
        )
    } else {
        (
            t.status_success,
            format!("Looks like a valid {} hash.", form.algo.label()),
        )
    };

    let meter = row![
        text(format!("{count}/{target}"))
            .font(theme::MONO)
            .size(11.0)
            .color(count_color),
        // `.pac-meter`: 4px pill track with a proportional fill.
        pill_progress(
            (count as f32 / target as f32).min(1.0),
            Length::Fill,
            PAC_METER_H,
            t.bg_sunken,
            fill,
        ),
    ]
    .spacing(theme::space::S2)
    .align_y(Alignment::Center);

    // `.pac-lbl`: sentence-case semibold label; the hash label carries
    // a lighter hint with the expected count in bold mono.
    let algo_field = column![
        text("Algorithm")
            .font(theme::BODY_BOLD)
            .size(11.0)
            .color(t.fg_2),
        algo_row,
    ]
    .spacing(6.0);
    // Single rich-text run so the mixed body/mono fragments share one
    // baseline (a `row` of `text`s can only box-align, not
    // baseline-align).
    let hash_lbl = iced::widget::rich_text::<(), Msg, _, _>([
        iced::widget::span("Hash")
            .font(theme::BODY_BOLD)
            .color(t.fg_2),
        iced::widget::span(" · expects ")
            .font(theme::BODY)
            .color(t.fg_3),
        iced::widget::span(target.to_string())
            .font(theme::MONO_BOLD)
            .color(t.fg_2),
        iced::widget::span(" hex characters")
            .font(theme::BODY)
            .color(t.fg_3),
    ])
    .size(11.0);
    let hash_field = column![
        hash_lbl,
        // `.pac-input` is a textarea, not an input: a 128-char SHA-512
        // wraps across lines instead of scrolling out of sight.
        text_editor::TextEditor::new(&st.checksum_hash)
            .placeholder(format!(
                "Paste the {} hash from the publisher's website…",
                form.algo.label()
            ))
            .font(theme::MONO)
            .size(HASH_TEXT)
            // A hash has nothing to break at either; without the
            // glyph fallback the "wraps across lines" above was not
            // true of any hash longer than the box.
            .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
            .height(Length::Fixed(HASH_MIN_H))
            .on_action(Msg::ChecksumHash)
            .style(move |_th, _| text_editor::Style {
                background: t2.bg_page.into(),
                border: iced::Border {
                    color: if form.canon.is_empty() || form.hex_ok {
                        t2.border_default
                    } else {
                        t2.status_danger
                    },
                    width: 1.0,
                    radius: theme::radius::XS.into(),
                },
                placeholder: t2.fg_4,
                value: t2.fg_1,
                selection: t2.selection_bg(),
            }),
        meter,
        text(msg_text)
            .font(theme::BODY_MEDIUM)
            .size(10.5)
            .color(msg_color),
    ]
    .spacing(6.0);

    let body = container(
        column![
            algo_field,
            hash_field,
            checkbox(
                t,
                "Auto-detect algorithm from hash length",
                st.cs_auto,
                true,
                Msg::CsAuto,
            ),
        ]
        .spacing(theme::space::S3 + 2.0),
    )
    .width(Length::Fill)
    .padding([theme::space::S3, 14.0]);

    // `.pac-foot`: sunken action strip below a hairline.
    let foot = container(
        row![
            Btn::new("Cancel")
                .ghost()
                .on_press(Msg::CsAddCancel)
                .view(t),
            iced::widget::Space::new().width(Length::Fill),
            Btn::new(format!("Save {}", form.algo.label()))
                .primary()
                .icon("check")
                .enabled(form.valid)
                .on_press(Msg::ChecksumSave)
                .view(t),
        ]
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding([theme::space::S2, 12.0])
    .style(move |_| container::Style {
        background: Some(t2.bg_sunken.into()),
        border: iced::Border {
            radius: iced::border::Radius {
                top_left: 0.0,
                top_right: 0.0,
                bottom_left: PAC_INNER_R,
                bottom_right: PAC_INNER_R,
            },
            ..Default::default()
        },
        ..Default::default()
    });

    container(column![
        head,
        hairline(color::with_alpha(t.action_primary, 0.20)),
        body,
        hairline(t.border_subtle),
        foot,
    ])
    .width(Length::Fill)
    .padding(PAC_BORDER_W)
    .style(move |_| container::Style {
        background: Some(t2.bg_surface.into()),
        border: iced::Border {
            color: t2.border_brand,
            width: PAC_BORDER_W,
            radius: theme::radius::SM.into(),
        },
        ..Default::default()
    })
    .into()
}

fn connection_tab(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let job = &st.entry.job;
    let ctx = |stored_secret| conn_form::FormCtx {
        editable: !st.locked(),
        stored_secret,
        // The legacy per-job proxy URL still wins under Inherit
        // (mapping.rs precedence) — the form says so rather than
        // letting the job quietly use a proxy the tab denies.
        legacy_url: job.proxy.as_deref(),
    };
    column![
        transfer_section(st),
        conn_form::proxy_section(
            t,
            &st.proxy,
            ctx(job.enc_proxy_password.is_some()),
            conn_form::ProxyMsgs {
                mode: Msg::ProxyModeSel,
                host: Msg::ProxyHost,
                port: Msg::ProxyPort,
                auth_enabled: Msg::ProxyAuth,
                username: Msg::ProxyUser,
                password: Msg::ProxyPass,
                password_clear: Msg::ProxyPassClear,
                remote_dns: Msg::RemoteDns,
            },
        ),
        conn_form::auth_section(
            t,
            &st.auth,
            ctx(job.enc_auth_password.is_some()),
            conn_form::AuthMsgs {
                scheme: Msg::AuthSchemeSel,
                username: Msg::AuthUser,
                password: Msg::AuthPass,
                token: Msg::AuthToken,
                secret_clear: Msg::AuthSecretClear,
            },
        ),
    ]
    .spacing(theme::space::S3)
    .into()
}

fn cookies_tab(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let t3 = *t;
    let editable = !st.locked();
    let parsed = st
        .cookies
        .text()
        .split(';')
        .filter(|s| s.contains('='))
        .count();
    // A stored jar never comes back as plaintext, so the editor is
    // empty for every capture that arrived with cookies. Counting what
    // is in the box and stopping there said "No cookies parsed yet."
    // over a download that had them all along, which reads as the
    // capture having dropped them.
    let blank = st.cookies.text().trim().is_empty();
    let caption = match (parsed, blank, st.has_stored_cookies, st.cookies_edited) {
        (0, true, true, true) => "Stored cookies will be removed when you apply.".to_owned(),
        (0, true, true, false) => "Cookies are stored for this download (encrypted).".to_owned(),
        (0, _, _, _) => "No cookies parsed yet.".to_owned(),
        _ => format!("{parsed} cookie(s) parsed."),
    };
    labeled_section(
        t,
        "cookies",
        column![
            toggle_row(
                t,
                "Send cookies",
                "Attach a Cookie header to every request for this download. Useful for \
                 paywalled mirrors or session-protected URLs.",
                st.cookies_enabled,
                editable,
                Msg::CookiesEnabled,
            ),
            row_sep(t),
            container(
                column![
                    text("Cookie store")
                        .font(theme::BODY_MEDIUM)
                        .size(12.0)
                        .color(t.fg_1),
                    text("Plain text or Netscape (cookies.txt) format. One cookie per line, or a single Cookie-header string.")
                        .font(theme::BODY)
                        .size(11.0)
                        .color(t.fg_3),
                    row![
                        iced::widget::Space::new().width(Length::Fill),
                        Btn::new("Clear")
                            .toolbar()
                            .icon("trash-2")
                            .size(BtnSize::Sm)
                            // A stored jar leaves the editor empty, so
                            // `parsed` alone would lock the only way to
                            // delete it.
                            .enabled(editable && (parsed > 0 || st.has_stored_cookies))
                            .on_press(Msg::CookiesClear)
                            .view(t),
                    ],
                    {
                        // Stored (encrypted) cookies never round-trip
                        // back as plaintext; the placeholder says so.
                        let mut ed = text_editor::TextEditor::new(&st.cookies)
                            .placeholder(if st.has_stored_cookies {
                                "Cookies stored (encrypted). Type to replace them."
                            } else {
                                "Paste cookies for this host.\nAccepts Netscape format (one \
                                 cookie per line)\nor a raw \"name=value; name2=value2\" string."
                            })
                            .font(theme::MONO)
                            .size(12.0)
                            .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
                            .height(Length::Fixed(110.0));
                        // Editing gated while the job runs (lock rules;
                        // guardian G2a-2) — no on_action, no edit path.
                        if editable {
                            ed = ed.on_action(Msg::CookiesEdit);
                        }
                        ed
                    }
                        .style(move |_th, _| text_editor::Style {
                            background: t3.bg_raised.into(),
                            border: iced::Border {
                                color: t3.border_subtle,
                                width: 1.0,
                                radius: theme::control::RADIUS.into(),
                            },
                            placeholder: t3.fg_4,
                            value: t3.fg_1,
                            selection: t3.selection_bg(),
                        }),
                    text(caption).font(theme::BODY).size(11.0).color(t.fg_3),
                ]
                .spacing(theme::space::S2)
            )
            .padding([10.0, theme::space::S3]),
        ]
        .into(),
    )
}

/// One identification field: name, why it exists, and a full-width
/// input whose placeholder is what an empty field falls back to.
fn ident_field<'a>(
    t: &Tokens,
    label: &'a str,
    hint: &'a str,
    value: &'a str,
    placeholder: String,
    editable: bool,
    on_input: fn(String) -> Msg,
) -> Element<'a, Msg> {
    container(
        column![
            text(label)
                .font(theme::BODY_MEDIUM)
                .size(12.0)
                .color(t.fg_1),
            text(hint)
                .font(theme::BODY)
                .size(11.0)
                .color(t.fg_3)
                .line_height(iced::widget::text::LineHeight::Relative(1.4)),
            TextInput::new(value)
                .hint(&placeholder)
                .enabled(editable)
                .on_input(on_input)
                .view(t),
        ]
        .spacing(theme::space::S1 + 2.0),
    )
    .padding([10.0, theme::space::S3])
    .into()
}

/// Read-only header table shared by the will-send and captured-response
/// sections (design `.prop-hdrs` / `.prop-hdr-row`). `custom` rows get
/// the clay accent (`.prop-hdr-row-custom`); `masked` rows dim their
/// value because it is a "(stored)" placeholder, never a real secret.
fn hdr_table<'a>(
    t: &Tokens,
    rows: impl IntoIterator<Item = (String, String, bool, bool)>,
) -> Element<'a, Msg> {
    let t2 = *t;
    let rows: Vec<_> = rows.into_iter().collect();
    let n = rows.len();
    let mut table = column![];
    for (i, (name, value, custom, masked)) in rows.into_iter().enumerate() {
        let name_color = if custom { t.action_primary } else { t.fg_2 };
        let value_color = if masked { t.fg_3 } else { t.fg_1 };
        let row_el = container(
            row![
                container(
                    text(name)
                        .font(theme::MONO_BOLD)
                        .size(11.0)
                        .color(name_color)
                )
                .width(Length::Fixed(140.0)),
                text(value).font(theme::MONO).size(11.0).color(value_color),
            ]
            .spacing(theme::space::S3)
            .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        .padding([6.0, theme::space::S3])
        .style(move |_| {
            if custom {
                // Faint clay wash; the clay key color carries the
                // "custom" signal (iced has no per-side border, so the
                // mock's 2px left rule is folded into these two cues).
                container::Style {
                    background: Some(color::with_alpha(t2.action_primary, 0.04).into()),
                    ..Default::default()
                }
            } else {
                container::Style::default()
            }
        });
        table = table.push(row_el);
        if i + 1 < n {
            table = table.push(row_sep(t));
        }
    }
    table.into()
}

/// Note line under a header table (design `.prop-note`).
fn hdr_note<'a>(t: &Tokens, body: String) -> Element<'a, Msg> {
    row![
        icons::icon("info", 12.0, t.fg_3),
        text(body)
            .font(theme::BODY_MEDIUM)
            .size(11.0)
            .color(t.fg_3)
            .line_height(iced::widget::text::LineHeight::Relative(1.4)),
    ]
    .spacing(6.0)
    .into()
}

fn headers_tab(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let editable = !st.locked();

    // --- Identification -----------------------------------------------
    // The two headers with a rule of their own: the UA that outranks
    // every header bag, and the referrer that lives on its own column.
    // Empty means inherit, so the UA field shows what it would inherit
    // as its placeholder rather than a blank that reads as "none".
    let inherited_ua = crate::domain::effective_user_agent(&st.settings)
        .unwrap_or_else(|| "randomized per request".to_owned());
    let ident = column![
        ident_field(
            t,
            "User-Agent",
            "Override the default UA for this download only.",
            &st.ua,
            inherited_ua,
            editable,
            Msg::UserAgent,
        ),
        row_sep(t),
        ident_field(
            t,
            "Referer",
            "The page the link came from. Filled in by the browser extension; \
             some hosts refuse a download without it.",
            &st.referer,
            "https://example.com/source-page".to_owned(),
            editable,
            Msg::Referer,
        ),
    ];

    // --- Read-only "Request headers (will send)" table (#7) -----------
    // Derived by the pure domain mirror of the run-time merge.
    let will_send = hdr_table(
        t,
        crate::domain::will_send_headers(&st.settings, &st.entry.job)
            .into_iter()
            .map(|h| (h.name, h.value, h.custom, h.masked)),
    );
    let will_send_section = column![
        labeled_section(t, "request headers (will send)", will_send),
        hdr_note(
            t,
            "Merged from your global settings and this download's overrides: \
             what oxdm sends on the next request. Stored cookies and credentials \
             are never displayed. The protocol adds its own on top: Accept-Encoding, \
             a Range per part, and a digest request on the first probe."
                .to_owned(),
        ),
    ]
    .spacing(theme::space::S2);

    // --- Read-only "Captured response" table (#7) ---------------------
    // Headers the server sent on the last evaluate probe. They describe
    // that one probe, so the note carries its timestamp. Three states:
    // never probed, probed with headers, and probed but nothing left to
    // show (every header was credential-bearing) — the last two must not
    // read alike, or a stripped probe looks like it never happened.
    let captured_section = match &st.entry.job.captured_response {
        Some(c) => {
            let when = chrono::DateTime::from_timestamp(c.probed_at, 0)
                .map(|d| {
                    d.with_timezone(&chrono::Local)
                        .format("%Y-%m-%d %H:%M:%S")
                        .to_string()
                })
                .unwrap_or_else(|| "an earlier request".to_owned());
            let body = if c.headers.is_empty() {
                container(
                    text(
                        "The server sent nothing displayable. Every header it returned \
                          was credential-bearing and is never stored.",
                    )
                    .font(theme::BODY)
                    .size(11.0)
                    .color(t.fg_3)
                    .line_height(iced::widget::text::LineHeight::Relative(1.4)),
                )
                .padding([10.0, theme::space::S3])
                .into()
            } else {
                hdr_table(
                    t,
                    c.headers
                        .iter()
                        .map(|h| (h.name.clone(), h.value.clone(), false, false)),
                )
            };
            column![
                labeled_section(t, "captured response", body),
                hdr_note(
                    t,
                    format!(
                        "Captured on {when}: what the server sent then, not necessarily \
                         what it would send now. Cookies and credential-bearing headers \
                         are never stored."
                    ),
                ),
            ]
        }
        None => column![labeled_section(
            t,
            "captured response",
            container(
                text(
                    "Nothing captured yet. Starting this download records the headers \
                     the server replies with."
                )
                .font(theme::BODY)
                .size(11.0)
                .color(t.fg_3)
                .line_height(iced::widget::text::LineHeight::Relative(1.4)),
            )
            .padding([10.0, theme::space::S3])
            .into(),
        )],
    }
    .spacing(theme::space::S2);

    let mut custom = column![
        container(
            column![
                text("Extra headers")
                    .font(theme::BODY_MEDIUM)
                    .size(12.0)
                    .color(t.fg_1),
                text(
                    "Sent alongside the defaults on every request. Useful for API keys, Origin \
                     overrides, or signed URLs."
                )
                .font(theme::BODY)
                .size(11.0)
                .color(t.fg_3),
            ]
            .spacing(2.0)
        )
        .padding([10.0, theme::space::S3]),
    ];
    for (i, (name, value)) in st.headers.iter().enumerate() {
        custom = custom.push(
            container(
                row![
                    TextInput::new(name)
                        .hint("Name")
                        .enabled(editable)
                        .on_input(move |v| Msg::HeaderName(i, v))
                        .view(t),
                    TextInput::new(value)
                        .hint("Value")
                        .enabled(editable)
                        .on_input(move |v| Msg::HeaderValue(i, v))
                        .view(t),
                    Btn::new("")
                        .toolbar()
                        .icon_only("trash-2")
                        .enabled(editable)
                        .on_press(Msg::HeaderRemove(i))
                        .view(t),
                ]
                .spacing(theme::space::S2)
                .align_y(Alignment::Center),
            )
            .padding([4.0, theme::space::S3]),
        );
    }
    custom = custom.push(
        container(
            Btn::new("Add header")
                .ghost()
                .icon("plus")
                .accent(true)
                .font_size(11.0)
                .enabled(editable)
                .on_press(Msg::HeaderAdd)
                .view(t),
        )
        .padding([6.0, theme::space::S3]),
    );

    // Identification sits with the editors, not with the read-only
    // tables: it is something to change, and the two tables above it
    // are the report of what changing it did.
    column![
        will_send_section,
        captured_section,
        labeled_section(t, "identification", ident.into()),
        labeled_section(t, "custom request headers", custom.into())
    ]
    .spacing(theme::space::S3)
    .into()
}

pub fn launch_properties(_id: JobId) {
    let mut app = iced::application(boot, update, view)
        .title(|app: &App| match app {
            App::Ready(st) => st.window_title(),
            _ => "oxdm - Properties".to_owned(),
        })
        .theme(|app: &App| match app {
            App::Ready(st) => st.tokens.iced_theme(),
            _ => Tokens::dark().iced_theme(),
        })
        .subscription(subscription)
        .default_font(theme::BODY)
        .antialiasing(true)
        .window(chrome::window_settings(
            iced::Size::new(600.0, 720.0),
            iced::Size::new(600.0, 718.0),
        ));
    for f in theme::fonts::ALL {
        app = app.font(*f);
    }
    if let Err(e) = app.run() {
        eprintln!("gui error: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job_with(headers: &[(&str, &str)], referrer: Option<&str>) -> crate::domain::Job {
        let mut job = crate::domain::Job {
            id: JobId::new(),
            url: "https://example.com/f.bin".parse().unwrap(),
            save_dir: std::path::PathBuf::from("/tmp"),
            filename: None,
            referrer: None,
            headers: Default::default(),
            max_connections: None,
            proxy: None,
            auth_user: None,
            enc_auth_password: None,
            enc_proxy_password: None,
            enc_cookies: None,
            speed_limit_override: None,
            queue_id: crate::domain::QueueId::new(),
            work_root: None,
            created_at: chrono::Utc::now(),
            started_at: None,
            active_ms: None,
            finished_at: None,
            retries: 0,
            interruptions: 0,
            verify_pending: false,
            status: Default::default(),
            advanced: crate::domain::Advanced::default(),
            checksums: Vec::new(),
            category: crate::domain::Category::Other,
            captured_response: None,
        };
        job.headers = headers
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        job.referrer = referrer.map(|r| r.parse().unwrap());
        job
    }

    /// Opening the dialog and applying it unchanged must store the
    /// same request: the User-Agent goes back into the bag it came
    /// from, and the referrer stays on its column.
    #[test]
    fn identification_survives_a_round_trip_through_the_form() {
        let job = job_with(
            &[("User-Agent", "browser/2"), ("X-Api-Key", "k")],
            Some("https://example.com/page"),
        );
        let (ua, referer, rows) = split_identity(&job);
        assert_eq!(ua, "browser/2");
        assert_eq!(referer, "https://example.com/page");
        assert_eq!(rows, vec![("X-Api-Key".to_owned(), "k".to_owned())]);

        let (stored, stored_referrer) = saved_identity(&job);
        assert_eq!(
            header_bag(&compose_headers(&rows, &ua)),
            header_bag(&stored),
            "a form nobody touched proposes the bag it was given"
        );
        assert_eq!(stored_referrer.as_deref(), Some(referer.as_str()));
    }

    /// A job from before the referrer had its own column carries it as
    /// a plain header. The form shows it in the referrer field, and
    /// the comparison baseline says so too — otherwise the dialog
    /// opens already claiming an unsaved change.
    #[test]
    fn a_legacy_referer_header_opens_clean() {
        let job = job_with(&[("Referer", "https://old.example/page")], None);
        let (_, referer, rows) = split_identity(&job);
        assert_eq!(referer, "https://old.example/page");
        assert!(rows.is_empty(), "not repeated among the free-form rows");

        let (stored, stored_referrer) = saved_identity(&job);
        assert!(stored.is_empty());
        assert_eq!(stored_referrer.as_deref(), Some("https://old.example/page"));
    }

    #[test]
    fn an_emptied_user_agent_stops_being_a_header() {
        let rows = vec![("X-Api-Key".to_owned(), "k".to_owned())];
        assert_eq!(compose_headers(&rows, "   "), rows);
    }
}

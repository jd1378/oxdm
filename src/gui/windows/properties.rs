//! Per-job Properties window (`oxdm gui properties <id>`): General /
//! Checksums / Connection / Cookies / Headers / Advanced tabs, hero
//! card, section cards with kv rows, footer with Open Containing
//! Folder / Close / Apply.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use iced::widget::{column, container, row, text, text_editor};
use iced::{Alignment, Element, Length, Subscription, Task};

use crate::domain::checksum::{Algo, CsSource, CsStatus};
use crate::domain::{AuthScheme, Checksum, JobId, Phase, ProxyMode};
use crate::gui::chrome::{self, WindowControl, titlebar};
use crate::gui::format::{format_bytes_2, format_int_grouped};
use crate::gui::ipc::DaemonSignal;
use crate::gui::shot::Shot;
use crate::gui::theme::{self, Tokens};
use crate::gui::widget::error_panel::hash_mismatch;
use crate::gui::widget::{
    Btn, BtnSize, TabBtn, TextInput, checkbox, eyebrow, hairline, pill_progress, status_dot, toggle,
};
use crate::gui::windows::add::footer;
use crate::gui::{color, icons};
use crate::ipc_local::Client;
use crate::ipc_local::protocol::{Event, JobEntryView};

// --- Connection tab (#6, honesty matrix) ------------------------------
/// Proxy-mode segmented options. Values and labels stay index-aligned.
/// odl can express: Inherit (no per-job override; a legacy `Job.proxy`
/// URL still applies under Inherit), System (clear the global proxy so
/// reqwest falls back to proxy environment variables) and explicit
/// HTTP / HTTPS / SOCKS5. "None (force direct)" is inexpressible in
/// odl and deliberately absent; a legacy persisted `ProxyMode::None`
/// is displayed as Inherit (guardian F6 — the runner logs the WARN).
const PROXY_MODE_VALUES: &[ProxyMode] = &[
    ProxyMode::Inherit,
    ProxyMode::System,
    ProxyMode::Http,
    ProxyMode::Https,
    ProxyMode::Socks5,
];
const PROXY_MODE_LABELS: &[(&str, Option<&str>)] = &[
    ("Inherit", None),
    ("System", None),
    ("HTTP", None),
    ("HTTPS", None),
    ("SOCKS5", None),
];
/// Site-auth schemes. Digest is deliberately absent (no odl/reqwest
/// implementation); a legacy persisted `Digest` is displayed as None
/// (guardian F6).
const AUTH_SCHEME_VALUES: &[AuthScheme] =
    &[AuthScheme::None, AuthScheme::Basic, AuthScheme::Bearer];
const AUTH_SCHEME_LABELS: &[(&str, Option<&str>)] =
    &[("None", None), ("HTTP Basic", None), ("Bearer token", None)];
/// Width of the proxy port field (design `.prop-proxy-port` ≈ 90px).
const PORT_INPUT_W: f32 = 90.0;

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
    Advanced,
}

#[derive(Clone)]
pub enum Msg {
    Connected(Result<Box<(Arc<Client>, JobEntryView, crate::domain::Settings)>, String>),
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
    // Headers
    HeaderName(usize, String),
    HeaderValue(usize, String),
    HeaderRemove(usize),
    HeaderAdd,
    // Advanced
    AdvAutoVerify(bool),
    // Checksums (#5)
    CsAddOpen,
    CsAddCancel,
    CsAlgoPick(usize),
    CsAuto(bool),
    ChecksumHash(String),
    ChecksumSave,
    ChecksumRemove(usize),
    CsVerify(usize),
    /// Verify finished for the row identified by (algo, saved hash) —
    /// identity, not index, so a concurrent remove can't misfile it.
    CsVerified(Algo, String, Result<String, String>),
    CsCopy(String),
    // Settings refresh (theme + will-send headers stay current)
    SettingsRefreshed(Box<crate::domain::Settings>),
    // Footer
    OpenFolder,
    CloseWin,
    Apply,
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
    tab: Tab,

    url: String,
    save_path: String,
    // Connection (#6). Secret inputs are scratch buffers: they start
    // empty, never mirror stored ciphertext, and an empty value on
    // Apply means "keep the stored secret" (guardian F1).
    proxy_mode: ProxyMode,
    proxy_host: String,
    proxy_port: String,
    proxy_auth: bool,
    proxy_user: String,
    proxy_pass: String,
    /// The user edited the proxy password field this session. Empty +
    /// edited means "delete the stored secret"; empty + untouched keeps
    /// it (the ciphertext never round-trips into the form).
    proxy_pass_edited: bool,
    remote_dns: bool,
    auth_scheme: AuthScheme,
    auth_user: String,
    auth_pass: String,
    auth_token: String,
    /// Same rule as `proxy_pass_edited`, for whichever secret field the
    /// active scheme uses.
    auth_secret_edited: bool,
    cookies_enabled: bool,
    cookies: text_editor::Content,
    /// Encrypted cookies exist on the job (shown as "(stored)" — the
    /// plaintext never round-trips back into the editor).
    has_stored_cookies: bool,
    /// Same rule as `proxy_pass_edited`, for the cookie editor.
    cookies_edited: bool,
    headers: Vec<(String, String)>,
    adv: crate::domain::Advanced,
    checksums: Vec<Checksum>,
    // Checksums add-form (#5, design §3.4 AddChecksumForm)
    cs_adding: bool,
    cs_algo: Algo,
    cs_auto: bool,
    checksum_hash: String,
    /// Row currently hashing on a blocking thread, identified by
    /// (algo, saved hash).
    cs_verifying: Option<(Algo, String)>,
    cs_verify_error: Option<String>,

    dirty: bool,
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

    /// Mode that synthesizes its own `scheme://host:port` and therefore
    /// needs both fields (`Inherit` / `System` carry no address).
    fn proxy_explicit(&self) -> bool {
        matches!(
            self.proxy_mode,
            ProxyMode::Http | ProxyMode::Https | ProxyMode::Socks5
        )
    }

    /// Explicit mode with no host. `synth_proxy_url` rejects this, but
    /// only at job start — catch it while the user is still looking at
    /// the field.
    fn host_invalid(&self) -> bool {
        self.proxy_explicit() && self.proxy_host.trim().is_empty()
    }

    /// Explicit mode without a usable port (inline validation, design
    /// §3.5: 1–65535). Empty counts: the data layer has no default to
    /// fall back on. Blocks Apply.
    fn port_invalid(&self) -> bool {
        self.proxy_explicit() && !self.proxy_port.trim().parse::<u16>().is_ok_and(|p| p >= 1)
    }

    fn proxy_invalid(&self) -> bool {
        self.host_invalid() || self.port_invalid()
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
                Ok(Box::new((client, entry, snap.settings)))
            },
            Msg::Connected,
        ),
    )
}

fn hydrate(st: &mut State) {
    let job = &st.entry.job;
    st.url = job.url.to_string();
    st.save_path = job
        .save_dir
        .join(job.filename.as_deref().unwrap_or(""))
        .display()
        .to_string();
    let p = &job.advanced.proxy;
    // Display-side legacy coercions (guardian F6); the runner logs the
    // WARN when it applies the same coercion for real.
    st.proxy_mode = match p.mode {
        ProxyMode::None => ProxyMode::Inherit,
        m => m,
    };
    st.proxy_host = p.host.clone();
    st.proxy_port = p.port.clone();
    st.proxy_auth = p.auth_enabled;
    st.proxy_user = p.username.clone();
    st.proxy_pass.clear();
    st.proxy_pass_edited = false;
    st.remote_dns = p.remote_dns;
    st.auth_scheme = match job.advanced.auth.scheme {
        AuthScheme::Digest => AuthScheme::None,
        // Legacy Basic jobs (Add-dialog path) carry credentials on
        // `auth_user`/`enc_auth_password` with the advanced scheme
        // still at its None default — the runner sends Basic for
        // them, so the tab must say Basic (guardian F4 coherence).
        AuthScheme::None if job.auth_user.is_some() => AuthScheme::Basic,
        s => s,
    };
    // Basic username lives on the legacy `Job.auth_user` field — the
    // single source of truth the runner reads (guardian F2).
    st.auth_user = job.auth_user.clone().unwrap_or_default();
    st.auth_pass.clear();
    st.auth_token.clear();
    st.auth_secret_edited = false;
    st.headers = job
        .headers
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    st.adv = job.advanced.clone();
    st.cookies_enabled = job.advanced.cookies_enabled;
    // `cookie_jar` is only non-empty on legacy blobs written before
    // cookies moved to the encrypted rail; fresh saves land on
    // `enc_cookies` and never round-trip plaintext back here.
    st.cookies = text_editor::Content::with_text(&job.advanced.cookie_jar);
    st.has_stored_cookies = job.enc_cookies.is_some();
    st.cookies_edited = false;
    st.checksums = job.checksums.clone();
}

pub fn update(app: &mut App, msg: Msg) -> Task<Msg> {
    match msg {
        Msg::Connected(Ok(boxed)) => {
            let (client, entry, settings) = *boxed;
            let mut st = State {
                tokens: Tokens::from_settings(&settings),
                id: entry.job.id,
                tab: Tab::General,
                url: String::new(),
                save_path: String::new(),
                proxy_mode: ProxyMode::Inherit,
                proxy_host: String::new(),
                proxy_port: String::new(),
                proxy_auth: false,
                proxy_user: String::new(),
                proxy_pass: String::new(),
                proxy_pass_edited: false,
                remote_dns: true,
                auth_scheme: AuthScheme::None,
                auth_user: String::new(),
                auth_pass: String::new(),
                auth_token: String::new(),
                auth_secret_edited: false,
                cookies_enabled: false,
                cookies: text_editor::Content::new(),
                has_stored_cookies: false,
                cookies_edited: false,
                headers: Vec::new(),
                adv: Default::default(),
                checksums: Vec::new(),
                cs_adding: false,
                cs_algo: Algo::Sha256,
                cs_auto: true,
                checksum_hash: String::new(),
                cs_verifying: None,
                cs_verify_error: None,
                dirty: false,
                dirty_source: false,
                dirty_overlay: false,
                error: None,
                shot: Shot::from_env(),
                client,
                entry,
                settings,
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
    let mark = |st: &mut State| st.dirty = true;
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
            st.dirty_source = true;
            mark(st);
            Task::none()
        }
        Msg::BrowsedSave(None) => Task::none(),
        Msg::CopyUrl => iced::clipboard::write(st.url.clone()),
        Msg::ProxyModeSel(i) => {
            if let Some(mode) = PROXY_MODE_VALUES.get(i) {
                st.proxy_mode = *mode;
                mark(st);
            }
            Task::none()
        }
        Msg::ProxyHost(v) => {
            st.proxy_host = v;
            mark(st);
            Task::none()
        }
        Msg::ProxyPort(v) => {
            st.proxy_port = v;
            mark(st);
            Task::none()
        }
        Msg::ProxyAuth(v) => {
            st.proxy_auth = v;
            mark(st);
            Task::none()
        }
        Msg::ProxyUser(v) => {
            st.proxy_user = v;
            mark(st);
            Task::none()
        }
        Msg::ProxyPass(v) => {
            st.proxy_pass = v;
            st.proxy_pass_edited = true;
            mark(st);
            Task::none()
        }
        Msg::RemoteDns(v) => {
            st.remote_dns = v;
            mark(st);
            Task::none()
        }
        Msg::AuthSchemeSel(i) => {
            if let Some(scheme) = AUTH_SCHEME_VALUES.get(i) {
                st.auth_scheme = *scheme;
                mark(st);
            }
            Task::none()
        }
        Msg::AuthUser(v) => {
            st.auth_user = v;
            mark(st);
            Task::none()
        }
        Msg::AuthPass(v) => {
            st.auth_pass = v;
            st.auth_secret_edited = true;
            mark(st);
            Task::none()
        }
        Msg::AuthToken(v) => {
            st.auth_token = v;
            st.auth_secret_edited = true;
            mark(st);
            Task::none()
        }
        Msg::AuthSecretClear => {
            st.auth_pass.clear();
            st.auth_token.clear();
            st.auth_secret_edited = true;
            mark(st);
            Task::none()
        }
        Msg::ProxyPassClear => {
            st.proxy_pass.clear();
            st.proxy_pass_edited = true;
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
        Msg::AdvAutoVerify(v) => {
            st.adv.auto_verify = v;
            mark(st);
            Task::none()
        }
        Msg::CsAddOpen => {
            st.cs_adding = true;
            st.checksum_hash.clear();
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
            st.checksum_hash.clear();
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
        Msg::ChecksumHash(v) => {
            st.checksum_hash = v;
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
            st.checksum_hash.clear();
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
            if st.cs_verifying.is_some() {
                return Task::none();
            }
            let Some(cs) = st.checksums.get(i) else {
                return Task::none();
            };
            // Verification hashes the finished file on disk — only
            // possible once the job has a final path.
            let Some(path) = st.entry.job.status.final_path.clone() else {
                return Task::none();
            };
            let algo = cs.algo;
            let saved = cs.hash.clone();
            st.cs_verifying = Some((algo, saved.clone()));
            st.cs_verify_error = None;
            // Streaming hasher on a blocking thread — never on the
            // iced executor (precedent: download.rs CsCompute).
            Task::perform(
                async move {
                    tokio::task::spawn_blocking(move || {
                        crate::domain::checksum::compute_file(&path, algo)
                            .map_err(|e| e.to_string())
                    })
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|r| r)
                },
                move |r| Msg::CsVerified(algo, saved.clone(), r),
            )
        }
        Msg::CsVerified(algo, saved, res) => {
            st.cs_verifying = None;
            match res {
                Ok(digest) => {
                    let Some(cs) = st
                        .checksums
                        .iter_mut()
                        .find(|c| c.algo == algo && c.hash == saved)
                    else {
                        return Task::none();
                    };
                    if digest.eq_ignore_ascii_case(saved.trim()) {
                        cs.status = CsStatus::Verified;
                        cs.expected = None;
                    } else {
                        // `expected` carries the digest computed from
                        // the file on disk — the "Got" side of the
                        // Expected/Got diff (the saved hash stays the
                        // "Expected" side).
                        cs.status = CsStatus::Mismatch;
                        cs.expected = Some(digest);
                    }
                    persist_checksums(st)
                }
                Err(e) => {
                    st.cs_verify_error = Some(e);
                    Task::none()
                }
            }
        }
        Msg::CsCopy(s) => iced::clipboard::write(s),
        Msg::OpenFolder => {
            crate::platform::open_path(&st.entry.job.save_dir);
            Task::none()
        }
        Msg::CloseWin => iced::exit(),
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
            let p = PathBuf::from(st.save_path.trim());
            let (save_dir, filename) = (
                p.parent()
                    .map(|d| d.to_path_buf())
                    .unwrap_or_else(|| st.entry.job.save_dir.clone()),
                p.file_name().map(|n| n.to_string_lossy().into_owned()),
            );

            // Advanced bundle. The daemon strips the secret fields into
            // the encrypted columns (guardian F1) and moves a non-empty
            // Basic username onto legacy `Job.auth_user` (F2). Empty
            // secret inputs mean "keep the stored secret".
            let mut adv = st.adv.clone();
            adv.proxy.mode = st.proxy_mode;
            adv.proxy.host = st.proxy_host.trim().to_owned();
            adv.proxy.port = st.proxy_port.trim().to_owned();
            adv.proxy.auth_enabled = st.proxy_auth;
            adv.proxy.username = st.proxy_user.trim().to_owned();
            adv.proxy.password = st.proxy_pass.clone();
            // Emptying a field that held a stored secret is the only way
            // to delete it; leaving it untouched still means "keep".
            adv.proxy.clear_password = st.proxy_pass_edited && st.proxy_pass.is_empty();
            adv.proxy.remote_dns = st.remote_dns;
            // Emptying a secret field that held a stored value is the
            // only way to delete it; untouched still means "keep".
            adv.auth.clear_secret = st.auth_secret_edited
                && match st.auth_scheme {
                    AuthScheme::Bearer => st.auth_token.is_empty(),
                    _ => st.auth_pass.is_empty(),
                };
            adv.clear_cookie_jar = st.cookies_edited && st.cookies.text().trim().is_empty();
            adv.auth.scheme = st.auth_scheme;
            adv.auth.username = st.auth_user.trim().to_owned();
            adv.auth.password = st.auth_pass.clone();
            adv.auth.token = st.auth_token.clone();
            adv.cookies_enabled = st.cookies_enabled;
            adv.cookie_jar = st.cookies.text();

            // Header/cookie edits need `UpdateJobLocation` — the only
            // IPC that persists `Job.headers` + `enc_cookies`. It
            // re-encrypts secrets from its payload, so it carries any
            // freshly-typed ones too. Pure URL/save edits take the
            // narrower `SetJobSource`, which cannot disturb stored
            // secrets or headers.
            let opt = |s: &str| {
                let s = s.trim();
                (!s.is_empty()).then(|| s.to_owned())
            };
            let job = &st.entry.job;
            let edit = st.dirty_overlay.then(|| {
                let mut headers = indexmap::IndexMap::new();
                for (k, v) in &st.headers {
                    if !k.trim().is_empty() {
                        headers.insert(k.trim().to_owned(), v.clone());
                    }
                }
                crate::ipc_local::protocol::JobEdit {
                    url: url.clone(),
                    save_dir: save_dir.clone(),
                    filename: filename.clone(),
                    // No UI for these any more (dead-fields inventory);
                    // pass the job's current values through unchanged.
                    referrer: job.referrer.clone(),
                    max_connections: job.max_connections,
                    // Legacy per-job proxy URL: preserved as-is — the
                    // Connection tab edits `advanced.proxy` instead.
                    proxy: job.proxy.clone(),
                    auth_user: job.auth_user.clone(),
                    auth_password: opt(match st.auth_scheme {
                        AuthScheme::Basic => &st.auth_pass,
                        AuthScheme::Bearer => &st.auth_token,
                        _ => "",
                    }),
                    proxy_password: opt(&st.proxy_pass),
                    headers,
                    cookies: st.cookies_enabled.then(|| st.cookies.text()),
                }
            });
            let source_dirty = st.dirty_source;
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
            st.proxy_pass_edited = false;
            st.auth_secret_edited = false;
            st.cookies_edited = false;
            Task::none()
        }
        Msg::Applied(Err(e)) => {
            st.error = Some(e);
            st.dirty = true;
            Task::none()
        }
        Msg::WinResized(w, h) => {
            chrome::enforce_min_size(iced::Size::new(w, h), iced::Size::new(650.0, 718.0))
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
    let canon = canonical_hash(&st.checksum_hash);
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
    TabBtn::new(label)
        .icon(icon)
        .icon_size(13.0)
        .height(35.0)
        .font_size(12.0)
        .active(tab == cur)
        .on_press(Msg::SetTab(tab))
        .view(t)
}

fn section<'a>(t: &Tokens, label: &str, body: Element<'a, Msg>) -> Element<'a, Msg> {
    let t2 = *t;
    column![
        container(eyebrow(t, label)).padding(iced::Padding {
            left: 2.0,
            ..Default::default()
        }),
        container(body)
            .width(Length::Fill)
            .style(move |_| container::Style {
                background: Some(t2.bg_surface.into()),
                border: iced::Border {
                    color: t2.border_subtle,
                    width: 1.0,
                    radius: theme::surface::RADIUS.into(),
                },
                ..Default::default()
            }),
    ]
    .spacing(theme::space::S1 + 2.0)
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
    let name = st
        .entry
        .job
        .filename
        .clone()
        .unwrap_or_else(|| "download".to_owned());

    let tabs = container(
        row![
            tabbtn(t, "General", "info", Tab::General, st.tab),
            tabbtn(t, "Checksums", "shield-check", Tab::Checksums, st.tab),
            tabbtn(t, "Connection", "globe", Tab::Connection, st.tab),
            tabbtn(t, "Cookies", "cookie", Tab::Cookies, st.tab),
            tabbtn(t, "Headers", "list", Tab::Headers, st.tab),
            tabbtn(t, "Advanced", "sliders-horizontal", Tab::Advanced, st.tab),
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
        Tab::Advanced => advanced_tab(st),
    };
    // Lock banner tops every editable pane while the download runs;
    // skipped on General (read-only display) and Checksums (its
    // carve-out is explained inline) — design §3.5 lock rules.
    let show_lock_banner = st.locked() && !matches!(st.tab, Tab::General | Tab::Checksums);
    let body: Element<'_, Msg> = if show_lock_banner {
        column![lock_banner(t), tab_body]
            .spacing(theme::space::S3)
            .into()
    } else {
        tab_body
    };

    let footer_el = footer(
        t,
        Btn::new("Open Containing Folder")
            .toolbar()
            .icon("folder")
            .on_press(Msg::OpenFolder)
            .view(t),
        {
            let mut right = row![].spacing(theme::space::S2).align_y(Alignment::Center);
            if st.dirty {
                // clay "● unsaved" dirty-dot — staged edits await Apply.
                right = right.push(status_dot(t.action_primary, "unsaved", 11.0));
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
    let bar: Element<'_, Msg> =
        titlebar::titlebar(t, &format!("Properties — {name}"), false, Msg::Window);
    let bar: Element<'_, Msg> = if st.locked() {
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
        hairline(t.border_subtle),
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
                    "Pause it to edit connection, cookie, or transfer settings — your \
                     changes will take effect when you resume. Checksums can still be \
                     added at any time."
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
    let ext = PathBuf::from(&name)
        .extension()
        .map(|e| e.to_string_lossy().to_uppercase())
        .unwrap_or_else(|| "FILE".into());
    let total = st.entry.counters.total;
    let phase = st.entry.counters.phase;
    let (phase_color, phase_label) = match phase {
        Phase::Completed => (t.status_success, "COMPLETE"),
        Phase::Failed => (t.status_danger, "FAILED"),
        Phase::Paused => (t.fg_3, "PAUSED"),
        Phase::Queued => (t.status_info, "QUEUED"),
        Phase::Cancelled => (t.fg_3, "CANCELLED"),
        _ => (t.action_primary, "DOWNLOADING"),
    };

    let tile_bg = color::mix(t.bg_surface, t.action_primary, 0.20);
    let hero = container(
        row![
            container(
                text(ext)
                    .font(theme::MONO_BOLD)
                    .size(12.0)
                    .color(t.action_primary)
            )
            .width(Length::Fixed(56.0))
            .height(Length::Fixed(56.0))
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

    let file_section = section(
        t,
        "file",
        column![
            kv_row(t, "Name", name, true),
            row_sep(t),
            kv_row(t, "Category", job.category.label().to_owned(), false),
            row_sep(t),
            kv_row(t, "Size", size_str, true),
            row_sep(t),
            container(
                column![
                    text("Save to")
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
                .spacing(6.0)
            )
            .padding([10.0, theme::space::S3]),
        ]
        .into(),
    );

    let source_section = section(
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
                        Btn::new("")
                            .secondary()
                            .icon_only("copy")
                            .on_press(Msg::CopyUrl)
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

    let cs_summary = if st.checksums.is_empty() {
        "None — open the Checksums tab to add one.".to_owned()
    } else {
        format!("{} saved", st.checksums.len())
    };
    let integrity = section(
        t,
        "integrity",
        container(
            row![
                column![
                    text("Checksums")
                        .font(theme::BODY_MEDIUM)
                        .size(12.0)
                        .color(t.fg_1),
                    text("Hashes saved for this file.")
                        .font(theme::BODY)
                        .size(11.0)
                        .color(t.fg_3),
                ]
                .spacing(2.0),
                iced::widget::Space::new().width(Length::Fill),
                text(cs_summary).font(theme::BODY).size(12.0).color(t.fg_3),
            ]
            .align_y(Alignment::Center),
        )
        .padding([10.0, theme::space::S3])
        .into(),
    );

    column![hero, file_section, source_section, integrity]
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
            let verifying = st
                .cs_verifying
                .as_ref()
                .is_some_and(|(a, h)| *a == cs.algo && *h == cs.hash);
            let (status_color, status_label) = if verifying {
                // Indeterminate row state — compute_file is one-shot,
                // no live progress exists (honesty decision #5).
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
            let removable = !st.locked() && !verifying && !protected;
            let mut actions = row![].spacing(theme::space::S1).align_y(Alignment::Center);
            if cs.status != CsStatus::Verified {
                actions = actions.push(
                    Btn::new("Verify")
                        .toolbar()
                        .icon("shield-check")
                        .size(BtnSize::Sm)
                        .font_size(10.0)
                        .enabled(can_verify && !verifying && st.cs_verifying.is_none())
                        .on_press(Msg::CsVerify(i))
                        .view(t),
                );
            }
            actions = actions
                .push(
                    Btn::new("")
                        .toolbar()
                        .icon_only("copy")
                        .size(BtnSize::Sm)
                        .on_press(Msg::CsCopy(cs.hash.clone()))
                        .view(t),
                )
                .push(
                    Btn::new("")
                        .toolbar()
                        .icon_only("trash-2")
                        .size(BtnSize::Sm)
                        .enabled(removable)
                        .on_press(Msg::ChecksumRemove(i))
                        .view(t),
                );
            let mut row_col = column![
                row![
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
                .align_y(Alignment::Center),
            ]
            .spacing(theme::space::S2);
            // Stacked Expected/Got diff on mismatch (design §3.4):
            // Expected = the saved (publisher) hash, Got = the digest
            // computed from the file on disk (stored in `expected`).
            if cs.status == CsStatus::Mismatch
                && let Some(got) = &cs.expected
            {
                row_col = row_col.push(hash_mismatch(t, cs.algo.label(), &cs.hash, got));
            }
            list = list.push(container(row_col).padding([8.0, theme::space::S3]));
            if i + 1 < st.checksums.len() {
                list = list.push(row_sep(t));
            }
        }
        col = col.push(section(t, "checksums", list.into()));
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
                "Adding checksums is allowed even while the download is running — \
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

/// AddChecksumForm card border (design `.prop-add-cs`: 1.5px clay).
const PAC_BORDER_W: f32 = 1.5;
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
            iced::widget::button(text(algo.label()).font(theme::MONO).size(11.0))
                .padding([5.0, 10.0])
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
                            radius: theme::radius::CTRL.into(),
                        },
                        ..Default::default()
                    }
                })
                .on_press_maybe(enabled.then(|| Msg::CsAlgoPick(i))),
        );
    }
    let chip_box = container(chips)
        .padding(3.0)
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
                "{} too many — this is too long for {}.",
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
        TextInput::new(&st.checksum_hash)
            .hint(format!(
                "Paste the {} hash from the publisher's website…",
                form.algo.label()
            ))
            .mono()
            .on_input(Msg::ChecksumHash)
            .view(t),
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

fn toggle_row<'a>(
    t: &Tokens,
    title: &'a str,
    desc: &'a str,
    on: bool,
    enabled: bool,
    msg: fn(bool) -> Msg,
) -> Element<'a, Msg> {
    container(
        row![
            column![
                text(title)
                    .font(theme::BODY_MEDIUM)
                    .size(12.0)
                    .color(t.fg_1),
                text(desc).font(theme::BODY).size(11.0).color(t.fg_3),
            ]
            .spacing(2.0)
            .width(Length::Fill),
            toggle(t, on, enabled, msg),
        ]
        .spacing(theme::space::S2)
        .align_y(Alignment::Center),
    )
    .padding([10.0, theme::space::S3])
    .into()
}

/// Footer for a secret input whose stored value never round-trips into
/// the form. Without it "delete the stored secret" would be an
/// invisible gesture — type into an already-empty field, then erase it
/// — so the state and the way out are both spelled out.
fn stored_secret_row<'a>(
    t: &Tokens,
    editable: bool,
    pending_clear: bool,
    clear: Msg,
) -> Element<'a, Msg> {
    if pending_clear {
        return row![
            icons::icon("triangle-alert", 11.0, t.status_danger),
            text("Stored secret will be removed on Apply.")
                .font(theme::BODY)
                .size(11.0)
                .color(t.status_danger),
        ]
        .spacing(4.0)
        .align_y(Alignment::Center)
        .into();
    }
    row![
        icons::icon("lock", 11.0, t.status_success),
        text("Stored (encrypted). Leave blank to keep it.")
            .font(theme::BODY)
            .size(11.0)
            .color(t.fg_3),
        iced::widget::Space::new().width(Length::Fill),
        Btn::new("Remove")
            .toolbar()
            .icon("trash-2")
            .size(BtnSize::Sm)
            .enabled(editable)
            .on_press(clear)
            .view(t),
    ]
    .spacing(4.0)
    .align_y(Alignment::Center)
    .into()
}

fn connection_tab(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let editable = !st.locked();
    let explicit = matches!(
        st.proxy_mode,
        ProxyMode::Http | ProxyMode::Https | ProxyMode::Socks5
    );

    // --- Proxy section (feature #6 honesty matrix) --------------------
    let mode_idx = PROXY_MODE_VALUES
        .iter()
        .position(|m| *m == st.proxy_mode)
        .unwrap_or(0);
    let mode_row = column![
        text("Use proxy")
            .font(theme::BODY_MEDIUM)
            .size(12.0)
            .color(t.fg_1),
        text(
            "Route this download's traffic through a proxy server. Overrides the \
             global setting in Settings → Network."
        )
        .font(theme::BODY)
        .size(11.0)
        .color(t.fg_3),
        if editable {
            crate::gui::widget::segmented(
                t,
                PROXY_MODE_LABELS,
                mode_idx,
                BtnSize::Sm,
                Msg::ProxyModeSel,
            )
        } else {
            // Locked: render the selection read-only.
            text(PROXY_MODE_LABELS[mode_idx].0)
                .font(theme::BODY_MEDIUM)
                .size(12.0)
                .color(t.fg_2)
                .into()
        },
    ]
    .spacing(6.0);

    // Honest per-mode explanation (guardian System-mode gate: odl CAN
    // express "clear the global proxy" via `builder.proxy(None)`).
    let mode_hint: Option<String> = match st.proxy_mode {
        ProxyMode::Inherit => {
            let mut hint = "Inherit (global / environment) — uses the proxy from \
                            Settings → Network, or your proxy environment variables."
                .to_owned();
            if let Some(legacy) = &st.entry.job.proxy {
                // Legacy explicit Job.proxy URL wins under Inherit
                // (mapping.rs precedence) — surface it, don't hide it.
                hint.push_str(&format!("\nThis job carries a proxy URL: {legacy}"));
            }
            Some(hint)
        }
        ProxyMode::System => Some(
            "System (environment variables) — ignores the global oxdm proxy for this \
             job; the standard proxy environment variables still apply."
                .to_owned(),
        ),
        _ => None,
    };

    let mut proxy_body = column![container(mode_row).padding([10.0, theme::space::S3])];
    if let Some(hint) = mode_hint {
        proxy_body = proxy_body.push(
            container(
                text(hint)
                    .font(theme::BODY)
                    .size(11.0)
                    .color(t.fg_3)
                    .line_height(iced::widget::text::LineHeight::Relative(1.5)),
            )
            .padding(iced::Padding {
                left: theme::space::S3,
                right: theme::space::S3,
                bottom: 10.0,
                ..Default::default()
            }),
        );
    }
    if explicit {
        let socks5 = st.proxy_mode == ProxyMode::Socks5;
        let mut server = column![
            text("Server")
                .font(theme::BODY_MEDIUM)
                .size(12.0)
                .color(t.fg_1),
            row![
                TextInput::new(&st.proxy_host)
                    .hint("proxy.example.com")
                    .mono()
                    .enabled(editable)
                    .on_input(Msg::ProxyHost)
                    .view(t),
                text(":").font(theme::MONO).size(12.0).color(t.fg_3),
                TextInput::new(&st.proxy_port)
                    .hint(if socks5 { "1080" } else { "8080" })
                    .mono()
                    .width(Length::Fixed(PORT_INPUT_W))
                    .enabled(editable)
                    .on_input(Msg::ProxyPort)
                    .view(t),
            ]
            .spacing(6.0)
            .align_y(Alignment::Center),
        ]
        .spacing(6.0);
        // Inline validation (design §3.5: 1–65535). Both fields are
        // required — the data layer has no fallback for either, so an
        // explicit mode with a blank one only fails at job start.
        let problem = if st.host_invalid() {
            Some("Host is required for an explicit proxy.")
        } else if st.proxy_port.trim().is_empty() {
            Some("Port is required for an explicit proxy.")
        } else if st.port_invalid() {
            Some("Port must be between 1 and 65535.")
        } else {
            None
        };
        if let Some(problem) = problem {
            server = server.push(
                row![
                    icons::icon("triangle-alert", 10.0, t.status_danger),
                    text(problem)
                        .font(theme::BODY_MEDIUM)
                        .size(10.5)
                        .color(t.status_danger),
                ]
                .spacing(4.0)
                .align_y(Alignment::Center),
            );
        }
        proxy_body = proxy_body
            .push(row_sep(t))
            .push(container(server).padding([10.0, theme::space::S3]))
            .push(row_sep(t))
            .push(toggle_row(
                t,
                "Proxy authentication",
                "Username and password sent to the proxy itself (not the destination).",
                st.proxy_auth,
                editable,
                Msg::ProxyAuth,
            ));
        if st.proxy_auth {
            let mut creds = column![
                row![
                    TextInput::new(&st.proxy_user)
                        .hint("username")
                        .enabled(editable)
                        .on_input(Msg::ProxyUser)
                        .view(t),
                    // Stored secret never round-trips into the form;
                    // empty input on Apply keeps it (guardian F1).
                    TextInput::new(&st.proxy_pass)
                        .hint(if st.entry.job.enc_proxy_password.is_some() {
                            "(unchanged)"
                        } else {
                            "password"
                        })
                        .secure(true)
                        .enabled(editable)
                        .on_input(Msg::ProxyPass)
                        .view(t),
                ]
                .spacing(theme::space::S2)
            ]
            .spacing(6.0);
            if st.entry.job.enc_proxy_password.is_some() {
                creds = creds.push(stored_secret_row(
                    t,
                    editable,
                    st.proxy_pass_edited && st.proxy_pass.is_empty(),
                    Msg::ProxyPassClear,
                ));
            }
            proxy_body = proxy_body.push(container(creds).padding([10.0, theme::space::S3]));
        }
        if socks5 {
            proxy_body = proxy_body.push(row_sep(t)).push(toggle_row(
                t,
                "Resolve DNS through proxy",
                "Send hostname lookups through the SOCKS5 server. Hides DNS queries \
                 from your local resolver.",
                st.remote_dns,
                editable,
                Msg::RemoteDns,
            ));
        }
    }
    let proxy = section(t, "proxy", proxy_body.into());

    // --- Site authentication (None / Basic / Bearer — no Digest) ------
    let scheme_idx = AUTH_SCHEME_VALUES
        .iter()
        .position(|s| *s == st.auth_scheme)
        .unwrap_or(0);
    let stored_secret = st.entry.job.enc_auth_password.is_some();
    let mut auth_body = column![
        container(
            column![
                text("Scheme")
                    .font(theme::BODY_MEDIUM)
                    .size(12.0)
                    .color(t.fg_1),
                text("Sent to the destination server, not the proxy.")
                    .font(theme::BODY)
                    .size(11.0)
                    .color(t.fg_3),
                if editable {
                    crate::gui::widget::segmented(
                        t,
                        AUTH_SCHEME_LABELS,
                        scheme_idx,
                        BtnSize::Sm,
                        Msg::AuthSchemeSel,
                    )
                } else {
                    text(AUTH_SCHEME_LABELS[scheme_idx].0)
                        .font(theme::BODY_MEDIUM)
                        .size(12.0)
                        .color(t.fg_2)
                        .into()
                },
            ]
            .spacing(6.0)
        )
        .padding([10.0, theme::space::S3]),
    ];
    match st.auth_scheme {
        AuthScheme::Basic => {
            let mut creds = column![
                row![
                    TextInput::new(&st.auth_user)
                        .hint("username")
                        .enabled(editable)
                        .on_input(Msg::AuthUser)
                        .view(t),
                    TextInput::new(&st.auth_pass)
                        .hint(if stored_secret {
                            "(unchanged)"
                        } else {
                            "password"
                        })
                        .secure(true)
                        .enabled(editable)
                        .on_input(Msg::AuthPass)
                        .view(t),
                ]
                .spacing(theme::space::S2)
            ]
            .spacing(6.0);
            if stored_secret {
                creds = creds.push(stored_secret_row(
                    t,
                    editable,
                    st.auth_secret_edited && st.auth_pass.is_empty(),
                    Msg::AuthSecretClear,
                ));
            }
            auth_body = auth_body
                .push(row_sep(t))
                .push(container(creds).padding([10.0, theme::space::S3]));
        }
        AuthScheme::Bearer => {
            let mut field = column![
                text("Token")
                    .font(theme::BODY_MEDIUM)
                    .size(12.0)
                    .color(t.fg_1),
                TextInput::new(&st.auth_token)
                    .hint(if stored_secret {
                        "(unchanged)"
                    } else {
                        "eyJhbGciOi…"
                    })
                    .mono()
                    .secure(true)
                    .enabled(editable)
                    .on_input(Msg::AuthToken)
                    .view(t),
            ]
            .spacing(6.0);
            if stored_secret {
                field = field.push(stored_secret_row(
                    t,
                    editable,
                    st.auth_secret_edited && st.auth_token.is_empty(),
                    Msg::AuthSecretClear,
                ));
            }
            auth_body = auth_body
                .push(row_sep(t))
                .push(container(field).padding([10.0, theme::space::S3]));
        }
        _ => {}
    }

    column![proxy, section(t, "site authentication", auth_body.into())]
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
    let caption = if parsed == 0 {
        "No cookies parsed yet.".to_owned()
    } else {
        format!("{parsed} cookie(s) parsed.")
    };
    section(
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

    // --- Read-only "Request headers (will send)" table (#7) -----------
    // Derived by the pure domain mirror of the run-time merge.
    let will_send = hdr_table(
        t,
        crate::domain::will_send_headers(&st.settings, &st.entry.job)
            .into_iter()
            .map(|h| (h.name, h.value, h.custom, h.masked)),
    );
    let will_send_section = column![
        section(t, "request headers (will send)", will_send),
        hdr_note(
            t,
            "Merged from your global settings and this download's overrides — \
             what oxdm sends on the next request. Stored cookies and credentials \
             are never displayed."
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
                        "The server sent nothing displayable — every header it returned \
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
                section(t, "captured response", body),
                hdr_note(
                    t,
                    format!(
                        "Captured on {when} — what the server sent then, not necessarily \
                         what it would send now. Cookies and credential-bearing headers \
                         are never stored."
                    ),
                ),
            ]
        }
        None => column![section(
            t,
            "captured response",
            container(
                text(
                    "Nothing captured yet — starting this download records the headers \
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

    column![
        will_send_section,
        captured_section,
        section(t, "custom request headers", custom.into())
    ]
    .spacing(theme::space::S3)
    .into()
}

/// Advanced pane. Dead-fields inventory (guardian amendment): every
/// editable-but-dead `Advanced` field was REMOVED from the UI —
/// `user_agent`, `referer`, `segments` (duplicates
/// `Job.max_connections`), `speed_kbps`, `timeout`, `retries`,
/// `run_command`, `open_when_done` — none is wired through
/// `data/mapping.rs::job_overlay_options` to a real odl option, so
/// showing an editor would fake behavior. Only `auto_verify` remains:
/// the runner gates `add_checksums` on it (guardian F3).
fn advanced_tab(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let editable = !st.locked();

    let transfer = section(
        t,
        "transfer",
        toggle_row(
            t,
            "Auto-verify checksums",
            "Compute & compare every saved hash when the download completes.",
            st.adv.auto_verify,
            editable,
            Msg::AdvAutoVerify,
        ),
    );

    column![transfer].spacing(theme::space::S3).into()
}

pub fn launch_properties(_id: JobId) {
    let mut app = iced::application(boot, update, view)
        .title(|app: &App| match app {
            App::Ready(st) => format!(
                "oxdm — Properties {}",
                st.entry.job.filename.as_deref().unwrap_or("")
            ),
            _ => "oxdm — Properties".to_owned(),
        })
        .theme(|app: &App| match app {
            App::Ready(st) => st.tokens.iced_theme(),
            _ => Tokens::dark().iced_theme(),
        })
        .subscription(subscription)
        .default_font(theme::BODY)
        .antialiasing(true)
        .window(chrome::window_settings(
            iced::Size::new(650.0, 720.0),
            iced::Size::new(650.0, 718.0),
        ));
    for f in theme::fonts::ALL {
        app = app.font(*f);
    }
    if let Err(e) = app.run() {
        eprintln!("gui error: {e}");
        std::process::exit(1);
    }
}

//! Per-job Properties window (`oxdm gui properties <id>`): General /
//! Checksums / Connection / Cookies / Headers / Advanced tabs, hero
//! card, section cards with kv rows, footer with Open Containing
//! Folder / Close / Apply.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use iced::widget::{column, container, row, scrollable, text, text_editor};
use iced::{Alignment, Element, Length, Subscription, Task};

use crate::domain::{Checksum, JobId, Phase};
use crate::gui::chrome::{self, WindowControl, titlebar};
use crate::gui::format::{format_bytes_2, format_int_grouped};
use crate::gui::ipc::DaemonSignal;
use crate::gui::shot::Shot;
use crate::gui::theme::{self, Tokens};
use crate::gui::widget::{
    Btn, BtnSize, TabBtn, TextInput, combo, eyebrow, hairline, number_stepper, toggle,
};
use crate::gui::windows::add::footer;
use crate::gui::{color, icons};
use crate::ipc_local::Client;
use crate::ipc_local::protocol::{Event, JobEntryView};

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
    // Connection
    ProxyEnabled(bool),
    ProxyUrl(String),
    AuthScheme(String),
    AuthUser(String),
    AuthPass(String),
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
    AdvUserAgent(String),
    AdvReferer(String),
    AdvSegments(i64),
    AdvTimeout(i64),
    AdvRetries(i64),
    AdvAutoVerify(bool),
    AdvOpenDone(bool),
    // Checksums
    AddChecksum(&'static str),
    ChecksumHash(String),
    ChecksumSave,
    ChecksumRemove(usize),
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
    tab: Tab,

    url: String,
    save_path: String,
    proxy_enabled: bool,
    proxy_url: String,
    auth_scheme: String,
    auth_user: String,
    auth_pass: String,
    cookies_enabled: bool,
    cookies: text_editor::Content,
    headers: Vec<(String, String)>,
    adv: crate::domain::Advanced,
    checksums: Vec<Checksum>,
    adding_checksum: Option<&'static str>,
    checksum_hash: String,

    dirty: bool,
    error: Option<String>,
    shot: Option<Shot>,
}

impl State {
    fn locked(&self) -> bool {
        self.entry.counters.phase.is_running()
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
    st.proxy_enabled = job.proxy.is_some();
    st.proxy_url = job.proxy.clone().unwrap_or_default();
    st.auth_scheme = if job.auth_user.is_some() {
        "Basic".to_owned()
    } else {
        "None".to_owned()
    };
    st.auth_user = job.auth_user.clone().unwrap_or_default();
    st.headers = job
        .headers
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    st.adv = job.advanced.clone();
    st.cookies_enabled = job.advanced.cookies_enabled;
    st.cookies = text_editor::Content::with_text(&job.advanced.cookie_jar);
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
                proxy_enabled: false,
                proxy_url: String::new(),
                auth_scheme: "None".to_owned(),
                auth_user: String::new(),
                auth_pass: String::new(),
                cookies_enabled: false,
                cookies: text_editor::Content::new(),
                headers: Vec::new(),
                adv: Default::default(),
                checksums: Vec::new(),
                adding_checksum: None,
                checksum_hash: String::new(),
                dirty: false,
                error: None,
                shot: Shot::from_env(),
                client,
                entry,
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
            Event::Close => iced::exit(),
            Event::Focus => iced::window::latest().and_then(iced::window::gain_focus),
            _ => Task::none(),
        },
        Msg::SetTab(tab) => {
            st.tab = tab;
            Task::none()
        }
        Msg::Url(v) => {
            st.url = v;
            mark(st);
            Task::none()
        }
        Msg::SavePath(v) => {
            st.save_path = v;
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
            mark(st);
            Task::none()
        }
        Msg::BrowsedSave(None) => Task::none(),
        Msg::CopyUrl => iced::clipboard::write(st.url.clone()),
        Msg::ProxyEnabled(v) => {
            st.proxy_enabled = v;
            mark(st);
            Task::none()
        }
        Msg::ProxyUrl(v) => {
            st.proxy_url = v;
            mark(st);
            Task::none()
        }
        Msg::AuthScheme(v) => {
            st.auth_scheme = v;
            mark(st);
            Task::none()
        }
        Msg::AuthUser(v) => {
            st.auth_user = v;
            mark(st);
            Task::none()
        }
        Msg::AuthPass(v) => {
            st.auth_pass = v;
            mark(st);
            Task::none()
        }
        Msg::CookiesEnabled(v) => {
            st.cookies_enabled = v;
            mark(st);
            Task::none()
        }
        Msg::CookiesEdit(a) => {
            let edit = a.is_edit();
            st.cookies.perform(a);
            if edit {
                mark(st);
            }
            Task::none()
        }
        Msg::CookiesClear => {
            st.cookies = text_editor::Content::new();
            mark(st);
            Task::none()
        }
        Msg::HeaderName(i, v) => {
            if let Some(h) = st.headers.get_mut(i) {
                h.0 = v;
                mark(st);
            }
            Task::none()
        }
        Msg::HeaderValue(i, v) => {
            if let Some(h) = st.headers.get_mut(i) {
                h.1 = v;
                mark(st);
            }
            Task::none()
        }
        Msg::HeaderRemove(i) => {
            if i < st.headers.len() {
                st.headers.remove(i);
                mark(st);
            }
            Task::none()
        }
        Msg::HeaderAdd => {
            st.headers.push((String::new(), String::new()));
            mark(st);
            Task::none()
        }
        Msg::AdvUserAgent(v) => {
            st.adv.user_agent = v;
            mark(st);
            Task::none()
        }
        Msg::AdvReferer(v) => {
            st.adv.referer = v;
            mark(st);
            Task::none()
        }
        Msg::AdvSegments(v) => {
            st.adv.segments = v;
            mark(st);
            Task::none()
        }
        Msg::AdvTimeout(v) => {
            st.adv.timeout = v;
            mark(st);
            Task::none()
        }
        Msg::AdvRetries(v) => {
            st.adv.retries = v;
            mark(st);
            Task::none()
        }
        Msg::AdvAutoVerify(v) => {
            st.adv.auto_verify = v;
            mark(st);
            Task::none()
        }
        Msg::AdvOpenDone(v) => {
            st.adv.open_when_done = v;
            mark(st);
            Task::none()
        }
        Msg::AddChecksum(algo) => {
            st.adding_checksum = Some(algo);
            st.checksum_hash.clear();
            Task::none()
        }
        Msg::ChecksumHash(v) => {
            st.checksum_hash = v;
            Task::none()
        }
        Msg::ChecksumSave => {
            use crate::domain::checksum::{Algo, CsSource, CsStatus};
            let Some(algo_name) = st.adding_checksum.take() else {
                return Task::none();
            };
            let algo = match algo_name {
                "MD5" => Algo::Md5,
                "SHA-1" => Algo::Sha1,
                "SHA-384" => Algo::Sha384,
                "SHA-512" => Algo::Sha512,
                _ => Algo::Sha256,
            };
            let hash = st.checksum_hash.trim().to_lowercase();
            if hash.is_empty() {
                return Task::none();
            }
            st.checksums.push(Checksum {
                algo,
                hash: hash.clone(),
                source: CsSource::User,
                status: CsStatus::Unverified,
                expected: None,
            });
            let client = st.client.clone();
            let id = st.id;
            let cs = st.checksums.clone();
            Task::perform(
                async move { client.set_job_checksums(id, cs).await },
                |_| Msg::Noop,
            )
        }
        Msg::ChecksumRemove(i) => {
            if i < st.checksums.len() {
                st.checksums.remove(i);
                let client = st.client.clone();
                let id = st.id;
                let cs = st.checksums.clone();
                return Task::perform(
                    async move { client.set_job_checksums(id, cs).await },
                    |_| Msg::Noop,
                );
            }
            Task::none()
        }
        Msg::OpenFolder => {
            crate::platform::open_path(&st.entry.job.save_dir);
            Task::none()
        }
        Msg::CloseWin => iced::exit(),
        Msg::Apply => {
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
            let mut headers = indexmap::IndexMap::new();
            for (k, v) in &st.headers {
                if !k.trim().is_empty() {
                    headers.insert(k.trim().to_owned(), v.clone());
                }
            }
            let opt = |s: &str| {
                let s = s.trim();
                (!s.is_empty()).then(|| s.to_owned())
            };
            let edit = crate::ipc_local::protocol::JobEdit {
                url: url.clone(),
                save_dir,
                filename,
                referrer: st.adv.referer.trim().parse().ok(),
                headers,
                max_connections: (st.adv.segments > 0).then_some(st.adv.segments as u64),
                proxy: st.proxy_enabled.then(|| st.proxy_url.trim().to_owned()),
                auth_user: (st.auth_scheme != "None")
                    .then(|| opt(&st.auth_user))
                    .flatten(),
                auth_password: (st.auth_scheme != "None")
                    .then(|| opt(&st.auth_pass))
                    .flatten(),
                proxy_password: None,
                cookies: st.cookies_enabled.then(|| st.cookies.text()),
            };
            let adv = st.adv.clone();
            st.dirty = false;
            Task::perform(
                async move {
                    client.update_job_location(id, edit).await?;
                    client.set_job_advanced(id, adv).await
                },
                Msg::Applied,
            )
        }
        Msg::Applied(Ok(())) => Task::none(),
        Msg::Applied(Err(e)) => {
            st.error = Some(e);
            st.dirty = true;
            Task::none()
        }
        Msg::WinResized(w, h) => {
            chrome::enforce_min_size(iced::Size::new(w, h), iced::Size::new(650.0, 480.0))
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
        crate::gui::ipc::all_events().map(Msg::Daemon),
    ];
    if st.shot.is_some() {
        subs.push(Shot::frames().map(|_| Msg::ShotTick));
    }
    Subscription::batch(subs)
}

// ---------------------------------------------------------------- view

pub fn view(app: &App) -> Element<'_, Msg> {
    match app {
        App::Connecting => splash("Connecting…".to_owned()),
        App::Failed(e) => splash(e.clone()),
        App::Ready(st) => ready_view(st),
    }
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

    let body: Element<'_, Msg> = match st.tab {
        Tab::General => general_tab(st),
        Tab::Checksums => checksums_tab(st),
        Tab::Connection => connection_tab(st),
        Tab::Cookies => cookies_tab(st),
        Tab::Headers => headers_tab(st),
        Tab::Advanced => advanced_tab(st),
    };

    let footer_el = footer(
        t,
        Btn::new("Open Containing Folder")
            .toolbar()
            .icon("folder")
            .on_press(Msg::OpenFolder)
            .view(t),
        row![
            Btn::new("Close").ghost().on_press(Msg::CloseWin).view(t),
            Btn::new("Apply")
                .primary()
                .icon("check")
                .enabled(st.dirty && !st.locked())
                .on_press(Msg::Apply)
                .view(t),
        ]
        .spacing(theme::space::S2)
        .into(),
    );

    let t2 = *t;
    let page = column![
        titlebar::titlebar(t, &format!("Properties — {name}"), false, Msg::Window),
        hairline(t.border_subtle),
        tabs,
        hairline(t.border_subtle),
        scrollable(
            container(body)
                .padding(theme::space::S3)
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
    } else {
        let mut list = column![];
        for (i, cs) in st.checksums.iter().enumerate() {
            use crate::domain::checksum::CsStatus;
            let (status_color, status_label) = match cs.status {
                CsStatus::Verified => (t.status_success, "verified"),
                CsStatus::Mismatch => (t.status_danger, "mismatch"),
                CsStatus::Unverified => (t.fg_3, "unverified"),
            };
            let hash_short = if cs.hash.len() > 24 {
                format!("{}…{}", &cs.hash[..10], &cs.hash[cs.hash.len() - 10..])
            } else {
                cs.hash.clone()
            };
            list = list.push(
                container(
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
                        Btn::new("")
                            .toolbar()
                            .icon_only("trash-2")
                            .size(BtnSize::Sm)
                            .on_press(Msg::ChecksumRemove(i))
                            .view(t),
                    ]
                    .spacing(theme::space::S3)
                    .align_y(Alignment::Center),
                )
                .padding([8.0, theme::space::S3]),
            );
            if i + 1 < st.checksums.len() {
                list = list.push(row_sep(t));
            }
        }
        col = col.push(section(t, "checksums", list.into()));
    }

    if let Some(algo) = st.adding_checksum {
        col = col.push(crate::gui::widget::card(
            t,
            theme::space::S3,
            column![
                text(format!("Add {algo} checksum"))
                    .font(theme::BODY_BOLD)
                    .size(13.0)
                    .color(t.fg_1),
                row![
                    TextInput::new(&st.checksum_hash)
                        .hint("paste hash…")
                        .mono()
                        .on_input(Msg::ChecksumHash)
                        .view(t),
                    Btn::new("Save")
                        .primary()
                        .size(BtnSize::Sm)
                        .on_press(Msg::ChecksumSave)
                        .view(t),
                ]
                .spacing(theme::space::S2)
                .align_y(Alignment::Center),
            ]
            .spacing(theme::space::S2)
            .into(),
        ));
    }

    let mut chips = row![
        Btn::new("Add checksum manually")
            .secondary()
            .icon("plus")
            .on_press(Msg::AddChecksum("SHA-256"))
            .view(t),
        iced::widget::Space::new().width(Length::Fill),
    ]
    .spacing(theme::space::S1)
    .align_y(Alignment::Center);
    for algo in ["MD5", "SHA-1", "SHA-256", "SHA-384", "SHA-512"] {
        chips = chips.push(
            Btn::new(algo)
                .toolbar()
                .size(BtnSize::Sm)
                .font_size(10.0)
                .on_press(Msg::AddChecksum(match algo {
                    "MD5" => "MD5",
                    "SHA-1" => "SHA-1",
                    "SHA-384" => "SHA-384",
                    "SHA-512" => "SHA-512",
                    _ => "SHA-256",
                }))
                .view(t),
        );
    }
    col = col.push(chips);
    col.into()
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

fn connection_tab(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let editable = !st.locked();
    let proxy = section(
        t,
        "proxy",
        column![
            toggle_row(
                t,
                "Use proxy",
                "Route this download's traffic through a proxy server. Overrides the global \
                 setting in Preferences → Network.",
                st.proxy_enabled,
                editable,
                Msg::ProxyEnabled,
            ),
            row_sep(t),
            container(
                TextInput::new(&st.proxy_url)
                    .hint("http://127.0.0.1:8080")
                    .mono()
                    .enabled(editable && st.proxy_enabled)
                    .on_input(Msg::ProxyUrl)
                    .view(t)
            )
            .padding([10.0, theme::space::S3]),
        ]
        .into(),
    );

    let auth_body = column![
        container(
            row![
                column![
                    text("Scheme")
                        .font(theme::BODY_MEDIUM)
                        .size(12.0)
                        .color(t.fg_1),
                    text("Sent to the destination server, not the proxy.")
                        .font(theme::BODY)
                        .size(11.0)
                        .color(t.fg_3),
                ]
                .spacing(2.0)
                .width(Length::Fill),
                combo(
                    t,
                    vec!["None".to_owned(), "Basic".to_owned()],
                    Some(st.auth_scheme.clone()),
                    Msg::AuthScheme,
                    Length::Fixed(135.0),
                ),
            ]
            .spacing(theme::space::S2)
            .align_y(Alignment::Center)
        )
        .padding([10.0, theme::space::S3]),
    ];
    let auth_body = if st.auth_scheme != "None" {
        auth_body.push(row_sep(t)).push(
            container(
                row![
                    TextInput::new(&st.auth_user)
                        .hint("username")
                        .enabled(editable)
                        .on_input(Msg::AuthUser)
                        .view(t),
                    TextInput::new(&st.auth_pass)
                        .hint(if st.entry.job.enc_auth_password.is_some() {
                            "(stored)"
                        } else {
                            "password"
                        })
                        .secure(true)
                        .enabled(editable)
                        .on_input(Msg::AuthPass)
                        .view(t),
                ]
                .spacing(theme::space::S2),
            )
            .padding([10.0, theme::space::S3]),
        )
    } else {
        auth_body
    };

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
                            .enabled(editable && parsed > 0)
                            .on_press(Msg::CookiesClear)
                            .view(t),
                    ],
                    text_editor::TextEditor::new(&st.cookies)
                        .placeholder(
                            "Paste cookies for this host.\nAccepts Netscape format (one cookie \
                             per line)\nor a raw \"name=value; name2=value2\" string."
                        )
                        .font(theme::MONO)
                        .size(12.0)
                        .height(Length::Fixed(110.0))
                        .on_action(Msg::CookiesEdit)
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

fn headers_tab(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let editable = !st.locked();

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

    column![section(t, "custom request headers", custom.into())]
        .spacing(theme::space::S3)
        .into()
}

fn advanced_tab(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let editable = !st.locked();
    let stepper_row = |title: &'static str,
                       desc: &'static str,
                       value: i64,
                       min: i64,
                       max: i64,
                       msg: fn(i64) -> Msg,
                       suffix: Option<&'static str>| {
        let mut r = row![
            column![
                text(title)
                    .font(theme::BODY_MEDIUM)
                    .size(12.0)
                    .color(t.fg_1),
                text(desc).font(theme::BODY).size(11.0).color(t.fg_3),
            ]
            .spacing(2.0)
            .width(Length::Fill),
            number_stepper(t, value, min, max, editable, msg),
        ]
        .spacing(theme::space::S2)
        .align_y(Alignment::Center);
        if let Some(sfx) = suffix {
            r = r.push(text(sfx).font(theme::BODY).size(12.0).color(t.fg_3));
        }
        container(r).padding([10.0, theme::space::S3])
    };

    let identification = section(
        t,
        "identification",
        column![
            container(
                column![
                    text("User-Agent")
                        .font(theme::BODY_MEDIUM)
                        .size(12.0)
                        .color(t.fg_1),
                    text("Override the default UA for this download only.")
                        .font(theme::BODY)
                        .size(11.0)
                        .color(t.fg_3),
                    TextInput::new(&st.adv.user_agent)
                        .mono()
                        .enabled(editable)
                        .on_input(Msg::AdvUserAgent)
                        .view(t),
                ]
                .spacing(6.0)
            )
            .padding([10.0, theme::space::S3]),
            row_sep(t),
            container(
                column![
                    text("Referer")
                        .font(theme::BODY_MEDIUM)
                        .size(12.0)
                        .color(t.fg_1),
                    TextInput::new(&st.adv.referer)
                        .hint("https://example.com/source-page")
                        .mono()
                        .enabled(editable)
                        .on_input(Msg::AdvReferer)
                        .view(t),
                ]
                .spacing(6.0)
            )
            .padding([10.0, theme::space::S3]),
        ]
        .into(),
    );

    let transfer = section(
        t,
        "transfer",
        column![
            stepper_row(
                "Max segments",
                "Parallel connections. Lower this for fragile servers.",
                st.adv.segments,
                1,
                16,
                Msg::AdvSegments,
                None
            ),
            row_sep(t),
            stepper_row(
                "Connection timeout",
                "How long to wait for the server before giving up on a connection attempt.",
                st.adv.timeout,
                1,
                300,
                Msg::AdvTimeout,
                Some("seconds")
            ),
            row_sep(t),
            stepper_row(
                "Auto-retry on failure",
                "Retries are exponential — 1s, 2s, 4s, 8s, capped at 60s.",
                st.adv.retries,
                0,
                20,
                Msg::AdvRetries,
                None
            ),
            row_sep(t),
            toggle_row(
                t,
                "Auto-verify checksums",
                "Compute & compare every saved hash when the download completes.",
                st.adv.auto_verify,
                editable,
                Msg::AdvAutoVerify,
            ),
        ]
        .into(),
    );

    let after = section(
        t,
        "after completion",
        toggle_row(
            t,
            "Open file when done",
            "",
            st.adv.open_when_done,
            editable,
            Msg::AdvOpenDone,
        ),
    );

    column![identification, transfer, after]
        .spacing(theme::space::S3)
        .into()
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
            iced::Size::new(650.0, 480.0),
        ));
    for f in theme::fonts::ALL {
        app = app.font(*f);
    }
    if let Err(e) = app.run() {
        eprintln!("gui error: {e}");
        std::process::exit(1);
    }
}

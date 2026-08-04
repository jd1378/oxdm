//! Batch-capture triage window (`oxdm gui batch <staged-json-path>`):
//! row per captured link with probe status, queue selector, select
//! all, Start-now toggle, "Send N to oxdm" footer.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use iced::widget::{column, container, row, text};
use iced::{Alignment, Element, Length, Subscription, Task};

use crate::data::ProbeResult;
use crate::domain::{CaptureRequest, QueueId};
use crate::gui::chrome::{self, WindowControl, titlebar};
use crate::gui::format::format_bytes;
use crate::gui::shot::Shot;
use crate::gui::theme::{self, Tokens};
use crate::gui::widget::{Btn, checkbox, combo, hairline, sibling};
use crate::ipc_local::Client;
use crate::ipc_local::protocol::AddJobReq;

#[derive(Clone)]
pub struct Row {
    req: CaptureRequest,
    selected: bool,
    probe: Option<Result<(String, Option<u64>, bool), String>>,
}

#[derive(Clone)]
pub enum Msg {
    Connected(Result<Box<(Arc<Client>, Vec<(QueueId, String)>, crate::domain::Settings)>, String>),
    Window(WindowControl),
    Daemon(crate::gui::ipc::DaemonSignal),
    Probed(usize, Result<Box<ProbeResult>, String>),
    SelectAll(bool),
    Select(usize, bool),
    SetQueue(String),
    StartNow(bool),
    Send,
    Sent(Result<(), String>),
    Cancel,
    WinResized(f32, f32),
    ShotTick,
    Shot(iced::window::Screenshot),
    Themed(Box<Tokens>),
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
    rows: Vec<Row>,
    queues: Vec<(QueueId, String)>,
    queue: Option<QueueId>,
    start_now: bool,
    save_dir: PathBuf,
    shot: Option<Shot>,
}

fn staged_path() -> Option<PathBuf> {
    std::env::args().nth(3).map(PathBuf::from)
}

pub fn boot() -> (App, Task<Msg>) {
    let Some(path) = staged_path() else {
        return (App::Failed("missing staged path".into()), Task::none());
    };
    (
        App::Connecting,
        Task::perform(
            async move {
                let client = Client::connect_retry(Duration::from_secs(8))
                    .await
                    .map_err(|e| e.to_string())?;
                client
                    .hello(crate::ipc_local::protocol::GuiKind::Batch)
                    .await?;
                let snap = client.snapshot().await?;
                let items =
                    crate::ipc::batch::load_and_consume(&path).map_err(|e| e.to_string())?;
                Ok(Box::new((
                    client,
                    snap.queues
                        .iter()
                        .map(|q| (q.id, q.name.clone()))
                        .collect::<Vec<_>>(),
                    snap.settings,
                    items,
                )))
            },
            |r: Result<
                Box<(
                    Arc<Client>,
                    Vec<(QueueId, String)>,
                    crate::domain::Settings,
                    Vec<CaptureRequest>,
                )>,
                String,
            >| {
                match r {
                    Ok(b) => {
                        let (client, queues, settings, items) = *b;
                        Msg::Connected(Ok(Box::new((
                            client,
                            queues,
                            settings_with_items(settings, items),
                        ))))
                    }
                    Err(e) => Msg::Connected(Err(e)),
                }
            },
        ),
    )
}

// Smuggle the items through Settings? No — keep a thread_local handoff.
// Simpler: stash items in a global once cell set during boot.
static ITEMS: std::sync::OnceLock<Vec<CaptureRequest>> = std::sync::OnceLock::new();

fn settings_with_items(
    settings: crate::domain::Settings,
    items: Vec<CaptureRequest>,
) -> crate::domain::Settings {
    let _ = ITEMS.set(items);
    settings
}

pub fn update(app: &mut App, msg: Msg) -> Task<Msg> {
    match msg {
        Msg::Connected(Ok(boxed)) => {
            let (client, queues, settings) = *boxed;
            let items = ITEMS.get().cloned().unwrap_or_default();
            let rows: Vec<Row> = items
                .into_iter()
                .map(|req| Row {
                    req,
                    selected: true,
                    probe: None,
                })
                .collect();
            let mut probes = Vec::new();
            for (i, r) in rows.iter().enumerate() {
                let client = client.clone();
                let url = r.req.url.clone();
                probes.push(Task::perform(
                    async move {
                        match client.probe(url).await {
                            // Batch rows render a flat message; flatten
                            // the structured `JobError` here.
                            Ok(inner) => inner.map(Box::new).map_err(|e| e.to_string()),
                            Err(e) => Err(e),
                        }
                    },
                    move |res| Msg::Probed(i, res),
                ));
            }
            let main_queue = queues.first().map(|(id, _)| *id);
            *app = App::Ready(Box::new(State {
                tokens: Tokens::from_settings(&settings),
                rows,
                queue: main_queue,
                queues,
                start_now: false,
                save_dir: settings.download_dir.clone(),
                shot: Shot::from_env(),
                client,
            }));
            Task::batch(probes)
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
    match msg {
        Msg::Probed(i, res) => {
            if let Some(r) = st.rows.get_mut(i) {
                r.probe = Some(res.map(|p| (p.filename, p.size, p.is_resumable)));
            }
            Task::none()
        }
        Msg::SelectAll(v) => {
            for r in &mut st.rows {
                r.selected = v;
            }
            Task::none()
        }
        Msg::Select(i, v) => {
            if let Some(r) = st.rows.get_mut(i) {
                r.selected = v;
            }
            Task::none()
        }
        Msg::SetQueue(name) => {
            if let Some((id, _)) = st.queues.iter().find(|(_, n)| *n == name) {
                st.queue = Some(*id);
            }
            Task::none()
        }
        Msg::StartNow(v) => {
            st.start_now = v;
            Task::none()
        }
        Msg::Send => {
            let client = st.client.clone();
            let queue = st.queue;
            let start_now = st.start_now;
            let save_dir = st.save_dir.clone();
            let reqs: Vec<(CaptureRequest, Option<String>)> = st
                .rows
                .iter()
                .filter(|r| r.selected)
                .map(|r| {
                    (
                        r.req.clone(),
                        r.probe
                            .as_ref()
                            .and_then(|p| p.as_ref().ok())
                            .map(|(name, _, _)| name.clone()),
                    )
                })
                .collect();
            Task::perform(
                async move {
                    for (req, probed_name) in reqs {
                        let add = AddJobReq {
                            url: req.url.clone(),
                            save_dir: save_dir.clone(),
                            filename: req.filename.clone().or(probed_name),
                            referrer: req.referrer.clone(),
                            headers: req.headers.clone(),
                            max_connections: None,
                            proxy: None,
                            auth_user: None,
                            auth_password: None,
                            proxy_password: None,
                            cookies: req.cookies.clone(),
                            category: None,
                        };
                        let id = client.add_job(add).await?;
                        if let Some(q) = queue {
                            client.set_job_queue(id, q).await?;
                        }
                        if start_now {
                            // Bulk: the triage list can start dozens at
                            // once, so failures belong in the list, not
                            // in a window each.
                            client.start_job_bulk(id).await?;
                        }
                    }
                    Ok(())
                },
                Msg::Sent,
            )
        }
        Msg::Sent(_) => iced::exit(),
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
            chrome::enforce_min_size(iced::Size::new(w, h), iced::Size::new(520.0, 360.0))
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
    let resize = iced::event::listen_with(|event, _status, _id| match event {
        iced::Event::Window(iced::window::Event::Resized(size)) => {
            Some(Msg::WinResized(size.width, size.height))
        }
        _ => None,
    });
    let events = crate::gui::ipc::lifecycle_events(crate::ipc_local::protocol::GuiKind::Batch)
        .map(Msg::Daemon);
    match app {
        App::Ready(st) if st.shot.is_some() => {
            Subscription::batch([resize, events, Shot::frames().map(|_| Msg::ShotTick)])
        }
        _ => Subscription::batch([resize, events]),
    }
}

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

fn ready_view(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let t2 = *t;
    let n_total = st.rows.len();
    let n_sel = st.rows.iter().filter(|r| r.selected).count();
    let all = n_sel == n_total && n_total > 0;

    let header = row![
        text(format!(
            "Send {n_sel} of {n_total} link{} to oxdm",
            if n_total == 1 { "" } else { "s" }
        ))
        .font(theme::BODY_BOLD)
        .size(14.0)
        .color(t.fg_1),
        iced::widget::Space::new().width(Length::Fill),
        text("Queue").font(theme::BODY).size(12.0).color(t.fg_2),
        combo(
            t,
            st.queues.iter().map(|(_, n)| n.clone()).collect(),
            st.queue.and_then(|q| st
                .queues
                .iter()
                .find(|(id, _)| *id == q)
                .map(|(_, n)| n.clone())),
            Msg::SetQueue,
            Length::Fixed(160.0),
        ),
    ]
    .spacing(theme::space::S2)
    .align_y(Alignment::Center);

    let mut list = column![].spacing(2.0);
    list = list.push(checkbox(t, "Select all", all, true, Msg::SelectAll));
    list = list.push(hairline(t.border_subtle));
    for (i, r) in st.rows.iter().enumerate() {
        let detail: Element<'_, Msg> = match &r.probe {
            None => text("…").font(theme::MONO).size(11.0).color(t.fg_3).into(),
            Some(Ok((name, size, resum))) => text(format!(
                "{name}  ·  {}  ·  {}",
                size.map(format_bytes).unwrap_or_else(|| "—".into()),
                if *resum { "resumable" } else { "no resume" },
            ))
            .font(theme::BODY)
            .size(11.0)
            .color(t.fg_3)
            .into(),
            Some(Err(e)) => text(format!("probe failed: {e}"))
                .font(theme::BODY)
                .size(11.0)
                .color(t.status_danger)
                .into(),
        };
        list = list.push(
            container(
                row![
                    checkbox(t, "", r.selected, true, move |v| Msg::Select(i, v)),
                    column![
                        text(r.req.url.to_string())
                            .font(theme::MONO)
                            .size(12.0)
                            .color(t.fg_1),
                        detail,
                    ]
                    .spacing(2.0),
                ]
                .spacing(theme::space::S2)
                .align_y(Alignment::Center),
            )
            .padding([6.0, 0.0]),
        );
        if i + 1 < st.rows.len() {
            list = list.push(hairline(t.border_subtle));
        }
    }

    let footer_el = crate::gui::windows::add::footer(
        t,
        row![
            Btn::new("Cancel").ghost().on_press(Msg::Cancel).view(t),
            checkbox(t, "Start now", st.start_now, true, Msg::StartNow),
        ]
        .spacing(theme::space::S3)
        .align_y(Alignment::Center)
        .into(),
        Btn::new(format!("Send {n_sel} to oxdm"))
            .primary()
            .icon("download")
            .enabled(n_sel > 0)
            .on_press(Msg::Send)
            .view(t),
    );

    let page = column![
        titlebar::titlebar(t, "Send to oxdm", false, Msg::Window),
        hairline(t.border_subtle),
        container(
            column![
                sibling(header.into()),
                crate::gui::widget::vscroll(list).height(Length::Fill)
            ]
            .spacing(theme::space::S2)
        )
        .padding(iced::Padding {
            top: theme::space::S4,
            bottom: theme::space::S4,
            left: theme::space::S4,
            right: theme::space::S4 - crate::gui::widget::SCROLL_GUTTER,
        })
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

pub fn launch_batch(_path: PathBuf) {
    let mut app = iced::application(boot, update, view)
        .title(|_: &App| "oxdm — Send to oxdm".to_owned())
        .theme(|app: &App| match app {
            App::Ready(st) => st.tokens.iced_theme(),
            _ => Tokens::dark().iced_theme(),
        })
        .subscription(subscription)
        .default_font(theme::BODY)
        .antialiasing(true)
        .window(chrome::window_settings(
            iced::Size::new(760.0, 520.0),
            iced::Size::new(520.0, 360.0),
        ));
    for f in theme::fonts::ALL {
        app = app.font(*f);
    }
    if let Err(e) = app.run() {
        eprintln!("gui error: {e}");
        std::process::exit(1);
    }
}

//! Per-job download window (`oxdm gui download <id>`): header card
//! with 56px tile + % readout, striped progress, Info / Speed /
//! On Completion tabs, transfer-rate chart, segments table, footer —
//! and the "Download complete" view once the job finishes.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use iced::widget::{column, container, row, scrollable, text};
use iced::{Alignment, Element, Length, Subscription, Task};

use crate::domain::{JobId, OnCompletion, Phase, ShutdownAction};
use crate::gui::chrome::{self, WindowControl, titlebar};
use crate::gui::color;
use crate::gui::format::{format_bytes, format_bytes_2, format_eta, format_speed};
use crate::gui::ipc::DaemonSignal;
use crate::gui::shot::Shot;
use crate::gui::theme::{self, Tokens};
use crate::gui::widget::{
    Btn, RateChart, TabBtn, TextInput, checkbox, collapsible_card, combo, hairline, pill_progress,
    rate_chart, striped_progress,
};
use crate::gui::windows::add::footer;
use crate::ipc_local::Client;
use crate::ipc_local::protocol::{Event, JobEntryView};

const CHART_SAMPLES: usize = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Info,
    Speed,
    OnCompletion,
}

#[derive(Clone)]
pub enum Msg {
    Connected(Result<Box<(Arc<Client>, JobEntryView, crate::domain::Settings)>, String>),
    Entry(Box<JobEntryView>),
    Daemon(DaemonSignal),
    Window(WindowControl),
    SetTab(Tab),
    ToggleRate,
    ToggleSegments,
    ResetChart,
    SampleTick,
    AnimTick,
    // Speed tab form
    UseLimiter(bool),
    LimitKbs(String),
    RememberLimit(bool),
    MaxConn(String),
    ApplySpeed,
    // Completion tab form
    NotifyDone(bool),
    ExitDone(bool),
    PowerEnabled(bool),
    PowerAction(String),
    ForceTerminate(bool),
    ApplyCompletion,
    // Footer / complete view
    PauseResume,
    Cancel,
    Open,
    OpenFolder,
    CloseWin,
    MinimizeTray,
    DontShowAgain(bool),
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
    show_complete_dialog: bool,

    tab: Tab,
    rate_open: bool,
    segments_open: bool,
    samples: Vec<f32>,
    peak: f32,
    anim_t: f32,

    use_limiter: bool,
    limit_kbs: String,
    remember_limit: bool,
    max_conn: String,

    on_completion: OnCompletion,

    shot: Option<Shot>,
}

impl State {
    fn phase(&self) -> Phase {
        self.entry.counters.phase
    }
    fn frac(&self) -> f32 {
        match self.entry.counters.total {
            Some(t) if t > 0 => (self.entry.counters.downloaded as f64 / t as f64) as f32,
            _ => 0.0,
        }
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
                    .hello(crate::ipc_local::protocol::GuiKind::Download(id))
                    .await?;
                let entry = client.job_entry(id).await?.ok_or("job not found")?;
                let snap = client.snapshot().await?;
                Ok(Box::new((client, entry, snap.settings)))
            },
            Msg::Connected,
        ),
    )
}

fn refetch(client: Arc<Client>, id: JobId) -> Task<Msg> {
    Task::perform(async move { client.job_entry(id).await }, |r| match r {
        Ok(Some(e)) => Msg::Entry(Box::new(e)),
        _ => Msg::Noop,
    })
}

pub fn update(app: &mut App, msg: Msg) -> Task<Msg> {
    match msg {
        Msg::Connected(Ok(boxed)) => {
            let (client, entry, settings) = *boxed;
            let on_completion = entry.on_completion.clone();
            let limit = entry.job.speed_limit_override;
            *app = App::Ready(Box::new(State {
                tokens: Tokens::from_settings(&settings),
                id: entry.job.id,
                show_complete_dialog: settings.show_complete_dialog,
                tab: Tab::Info,
                rate_open: false,
                segments_open: false,
                samples: Vec::new(),
                peak: 0.0,
                anim_t: 0.0,
                use_limiter: limit.is_some() || entry.session_speed_override > 0,
                limit_kbs: limit
                    .or((entry.session_speed_override > 0).then_some(entry.session_speed_override))
                    .map(|b| (b / 1024).to_string())
                    .unwrap_or_else(|| "100".to_owned()),
                remember_limit: limit.is_some(),
                max_conn: entry
                    .job
                    .max_connections
                    .map(|n| n.to_string())
                    .unwrap_or_default(),
                on_completion,
                shot: Shot::from_env(),
                client,
                entry,
            }));
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
    match msg {
        Msg::Entry(e) => {
            st.entry = *e;
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
            Event::JobsChanged | Event::SettingsChanged => refetch(st.client.clone(), st.id),
            Event::Close => iced::exit(),
            Event::Focus => iced::window::latest().and_then(iced::window::gain_focus),
            _ => Task::none(),
        },
        Msg::SetTab(tab) => {
            st.tab = tab;
            Task::none()
        }
        Msg::ToggleRate => {
            st.rate_open = !st.rate_open;
            Task::none()
        }
        Msg::ToggleSegments => {
            st.segments_open = !st.segments_open;
            Task::none()
        }
        Msg::ResetChart => {
            st.samples.clear();
            st.peak = 0.0;
            Task::none()
        }
        Msg::SampleTick => {
            let s = st.entry.counters.speed_bps as f32;
            st.samples.push(s);
            st.peak = st.peak.max(s);
            if st.samples.len() > CHART_SAMPLES {
                let n = st.samples.len() - CHART_SAMPLES;
                st.samples.drain(..n);
            }
            Task::none()
        }
        Msg::AnimTick => {
            st.anim_t += 0.033;
            Task::none()
        }
        Msg::UseLimiter(v) => {
            st.use_limiter = v;
            Task::none()
        }
        Msg::LimitKbs(v) => {
            st.limit_kbs = v;
            Task::none()
        }
        Msg::RememberLimit(v) => {
            st.remember_limit = v;
            Task::none()
        }
        Msg::MaxConn(v) => {
            st.max_conn = v;
            Task::none()
        }
        Msg::ApplySpeed => {
            let client = st.client.clone();
            let id = st.id;
            let bps = st
                .use_limiter
                .then(|| st.limit_kbs.trim().parse::<u64>().ok().map(|k| k * 1024))
                .flatten();
            let persist = st.remember_limit;
            let conns = st
                .max_conn
                .trim()
                .parse::<u64>()
                .ok()
                .filter(|n| (1..=16).contains(n));
            Task::perform(
                async move {
                    if persist {
                        client.set_persistent_speed_limit(id, bps).await?;
                    } else {
                        client.set_session_speed_limit(id, bps).await?;
                    }
                    client.set_max_connections(id, conns).await
                },
                |_| Msg::Noop,
            )
        }
        Msg::NotifyDone(v) => {
            st.on_completion.show_dialog = v;
            Task::none()
        }
        Msg::ExitDone(v) => {
            st.on_completion.exit_app = v;
            Task::none()
        }
        Msg::PowerEnabled(v) => {
            st.on_completion.shutdown = v.then_some(ShutdownAction::ShutDown);
            Task::none()
        }
        Msg::PowerAction(s) => {
            st.on_completion.shutdown = Some(match s.as_str() {
                "Restart" => ShutdownAction::Restart,
                "Sleep" => ShutdownAction::Sleep,
                _ => ShutdownAction::ShutDown,
            });
            Task::none()
        }
        Msg::ForceTerminate(v) => {
            st.on_completion.force_terminate = v;
            Task::none()
        }
        Msg::ApplyCompletion => {
            let client = st.client.clone();
            let id = st.id;
            let prefs = st.on_completion.clone();
            Task::perform(
                async move { client.set_on_completion(id, prefs).await },
                |_| Msg::Noop,
            )
        }
        Msg::PauseResume => {
            let client = st.client.clone();
            let id = st.id;
            let running = st.phase().is_running();
            Task::perform(
                async move {
                    if running {
                        client.pause(id).await
                    } else {
                        client.resume(id).await
                    }
                },
                |_| Msg::Noop,
            )
        }
        Msg::Cancel => {
            let client = st.client.clone();
            let id = st.id;
            Task::perform(async move { client.cancel_to_queued(id).await }, |_| {
                Msg::CloseWin
            })
        }
        Msg::Open => {
            let path = final_path(&st.entry);
            crate::platform::open_path(&path);
            Task::none()
        }
        Msg::OpenFolder => {
            crate::platform::open_path(&st.entry.job.save_dir);
            Task::none()
        }
        Msg::CloseWin => iced::exit(),
        Msg::MinimizeTray => iced::window::latest().and_then(|id| iced::window::minimize(id, true)),
        Msg::DontShowAgain(dont) => {
            st.show_complete_dialog = !dont;
            let client = st.client.clone();
            let show = !dont;
            Task::perform(
                async move {
                    let snap = client.snapshot().await?;
                    let mut s = snap.settings;
                    s.show_complete_dialog = show;
                    client.update_settings(s).await
                },
                |_| Msg::Noop,
            )
        }
        Msg::WinResized(w, h) => {
            chrome::enforce_min_size(iced::Size::new(w, h), iced::Size::new(530.0, 410.0))
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

fn final_path(entry: &JobEntryView) -> PathBuf {
    entry
        .job
        .save_dir
        .join(entry.job.filename.as_deref().unwrap_or(""))
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
    if st.phase().is_running() {
        subs.push(iced::time::every(Duration::from_millis(33)).map(|_| Msg::AnimTick));
        if st.rate_open {
            subs.push(iced::time::every(Duration::from_millis(500)).map(|_| Msg::SampleTick));
        }
    }
    Subscription::batch(subs)
}

// ---------------------------------------------------------------- view

pub fn view(app: &App) -> Element<'_, Msg> {
    match app {
        App::Connecting => splash("Connecting…".to_owned()),
        App::Failed(e) => splash(e.clone()),
        App::Ready(st) => {
            if st.phase() == Phase::Completed {
                complete_view(st)
            } else {
                running_view(st)
            }
        }
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

fn page<'a>(t: &Tokens, content: Element<'a, Msg>) -> Element<'a, Msg> {
    let t2 = *t;
    let c = container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: Some(t2.bg_page.into()),
            text_color: Some(t2.fg_1),
            ..Default::default()
        });
    chrome::resize::resizable(t, c.into(), true, Msg::Window)
}

fn header_card(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let name = st
        .entry
        .job
        .filename
        .clone()
        .unwrap_or_else(|| st.entry.job.url.to_string());
    let ext = PathBuf::from(&name)
        .extension()
        .map(|e| e.to_string_lossy().to_uppercase())
        .unwrap_or_else(|| "FILE".into());
    let host = st.entry.job.url.host_str().unwrap_or("").to_owned();
    let resum = match st.entry.counters.is_resumable {
        1 => "resumable",
        -1 => "no resume",
        _ => "checking",
    };
    let cat_color = match st.entry.job.category {
        crate::domain::Category::Compressed => t.cat_compressed,
        crate::domain::Category::Programs => t.cat_programs,
        crate::domain::Category::Videos => t.cat_videos,
        crate::domain::Category::Music => t.cat_music,
        crate::domain::Category::Pictures => t.cat_pictures,
        crate::domain::Category::Documents => t.cat_documents,
        crate::domain::Category::Other => t.fg_3,
    };
    let tile_bg = color::mix(t.bg_surface, cat_color, 0.20);

    let dotsep = || text("·").size(11.0).color(t.fg_4);

    let tile = container(text(ext).font(theme::MONO_BOLD).size(12.0).color(cat_color))
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
        });

    let pct = format!("{}%", (st.frac() * 100.0).round() as u32);

    container(
        row![
            tile,
            column![
                text(name).font(theme::BODY_BOLD).size(14.0).color(t.fg_1),
                row![
                    text(host).font(theme::MONO).size(11.0).color(t.fg_3),
                    dotsep(),
                    text(st.entry.job.category.label())
                        .font(theme::BODY)
                        .size(11.0)
                        .color(t.fg_3),
                    dotsep(),
                    text(resum).font(theme::BODY).size(11.0).color(t.fg_3),
                ]
                .spacing(6.0)
                .align_y(Alignment::Center),
            ]
            .spacing(4.0),
            iced::widget::Space::new().width(Length::Fill),
            text(pct).font(theme::DISPLAY).size(28.0).color(t.fg_1),
        ]
        .spacing(theme::space::S3)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .into()
}

fn running_view(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let phase = st.phase();
    let striped = phase.is_running();
    let (track, fill, gradient) = match phase {
        Phase::Failed => (t.status_danger_bg, t.status_danger, None),
        p if p.is_running() => (
            t.progress_track,
            t.progress_fill,
            Some((color::clay::C400, color::clay::C300)),
        ),
        _ => (t.progress_track, t.fg_4, None),
    };

    let name = st
        .entry
        .job
        .filename
        .clone()
        .unwrap_or_else(|| "download".to_owned());

    let tabs = row![
        TabBtn::new("Info")
            .icon("info")
            .icon_size(13.0)
            .height(28.0)
            .bottom_gap(8.0)
            .font_size(12.0)
            .active(st.tab == Tab::Info)
            .on_press(Msg::SetTab(Tab::Info))
            .view(t),
        TabBtn::new("Speed")
            .icon("gauge")
            .icon_size(13.0)
            .height(28.0)
            .bottom_gap(8.0)
            .font_size(12.0)
            .active(st.tab == Tab::Speed)
            .on_press(Msg::SetTab(Tab::Speed))
            .view(t),
        TabBtn::new("On Completion")
            .icon("circle-check-big")
            .icon_size(13.0)
            .height(28.0)
            .bottom_gap(8.0)
            .font_size(12.0)
            .active(st.tab == Tab::OnCompletion)
            .on_press(Msg::SetTab(Tab::OnCompletion))
            .view(t),
    ]
    .spacing(theme::space::S2);

    let tab_body: Element<'_, Msg> = match st.tab {
        Tab::Info => info_tab(st),
        Tab::Speed => speed_tab(st),
        Tab::OnCompletion => completion_tab(st),
    };

    let footer_el = footer(
        t,
        Btn::new("Minimize to tray")
            .toolbar()
            .icon("minimize-2")
            .on_press(Msg::MinimizeTray)
            .view(t),
        row![
            Btn::new(if phase.is_running() {
                "Pause"
            } else {
                "Resume"
            })
            .primary()
            .icon(if phase.is_running() { "pause" } else { "play" })
            .on_press(Msg::PauseResume)
            .view(t),
            Btn::new("Cancel")
                .ghost()
                .icon("x")
                .on_press(Msg::Cancel)
                .view(t),
        ]
        .spacing(theme::space::S2)
        .into(),
    );

    page(
        t,
        column![
            titlebar::titlebar(t, &name, false, Msg::Window),
            hairline(t.border_subtle),
            container(
                column![
                    header_card(st),
                    striped_progress(
                        st.frac(),
                        Length::Fill,
                        10.0,
                        track,
                        fill,
                        gradient,
                        striped,
                        st.anim_t,
                    ),
                    // Tabs + hairline as one unspaced group so the
                    // active underline sits on the hairline.
                    column![tabs, hairline(t.border_subtle)],
                    scrollable(tab_body).height(Length::Fill),
                ]
                .spacing(theme::space::S3)
            )
            .padding(theme::space::S4)
            .height(Length::Fill),
            hairline(t.border_subtle),
            footer_el,
        ]
        .into(),
    )
}

fn stat<'a>(t: &Tokens, label: &'a str, value: String, accent: bool) -> Element<'a, Msg> {
    column![
        text(label.to_uppercase())
            .font(theme::BODY_BOLD)
            .size(9.0)
            .color(t.fg_3),
        text(value).font(theme::MONO).size(14.0).color(if accent {
            t.action_primary
        } else {
            t.fg_1
        }),
    ]
    .spacing(2.0)
    .width(Length::Fill)
    .into()
}

fn info_tab(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let t2 = *t;
    let c = &st.entry.counters;
    let speed = c.speed_bps;
    let eta = match (c.total, speed > 1.0) {
        (Some(total), true) if total > c.downloaded => {
            format_eta(((total - c.downloaded) as f64 / speed) as u64)
        }
        _ => "—".to_owned(),
    };

    let strip = container(
        row![
            stat(t, "speed", format_speed(speed), speed > 1.0),
            stat(t, "time left", eta, false),
            stat(t, "downloaded", format_bytes(c.downloaded), false),
            stat(
                t,
                "total",
                c.total.map(format_bytes).unwrap_or_else(|| "—".into()),
                false
            ),
        ]
        .spacing(theme::space::S2),
    )
    .width(Length::Fill)
    .padding(theme::space::S3)
    .style(move |_| container::Style {
        background: Some(t2.bg_sunken.into()),
        border: iced::Border {
            radius: theme::surface::RADIUS.into(),
            ..Default::default()
        },
        ..Default::default()
    });

    let avg = if st.samples.is_empty() {
        0.0
    } else {
        st.samples.iter().sum::<f32>() / st.samples.len() as f32
    };
    let max = st.peak.max(1.0);

    let rate_body = {
        let chart = RateChart {
            samples: st.samples.clone(),
            max,
            avg,
            accent: t.action_primary,
            grid: color::with_alpha(t.fg_4, 170.0 / 255.0),
        };
        let legend_item = |label: &'static str, value: String, color: iced::Color| {
            row![
                crate::gui::widget::dot(8.0, color),
                text(label).font(theme::BODY).size(11.0).color(t2.fg_3),
                text(value).font(theme::BODY_BOLD).size(11.0).color(t2.fg_1),
            ]
            .spacing(4.0)
            .align_y(Alignment::Center)
        };
        column![
            container(rate_chart(chart, 124.0))
                .width(Length::Fill)
                .padding(iced::Padding {
                    left: 12.0,
                    right: 12.0,
                    top: 22.0,
                    bottom: 10.0,
                })
                .style(move |_| container::Style {
                    background: Some(t2.bg_sunken.into()),
                    border: iced::Border {
                        radius: theme::radius::XS.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
            row![
                legend_item("Current", format_speed(speed), t2.action_primary),
                legend_item(
                    "Avg",
                    format_speed(avg as f64),
                    color::with_alpha(t2.fg_3, 0.9)
                ),
                legend_item("Peak", format_speed(st.peak as f64), t2.action_primary),
                iced::widget::Space::new().width(Length::Fill),
                Btn::new("")
                    .toolbar()
                    .icon_only("rotate-cw")
                    .on_press(Msg::ResetChart)
                    .view(t),
            ]
            .spacing(theme::space::S3)
            .align_y(Alignment::Center),
        ]
        .spacing(theme::space::S2)
    };

    let n_parts = c.parts.len();
    let segments_right = text(format!("{} parallel connections", c.parts.len().max(1)))
        .font(theme::BODY)
        .size(11.0)
        .color(t.fg_3);

    let segments_body: Element<'_, Msg> = if n_parts == 0 {
        text("No segment data yet.")
            .font(theme::BODY)
            .size(12.0)
            .color(t.fg_3)
            .into()
    } else {
        let mut rows = column![].spacing(2.0);
        for (i, p) in c.parts.iter().enumerate() {
            let frac = if p.size > 0 {
                p.downloaded as f32 / p.size as f32
            } else {
                0.0
            };
            let done = p.finished;
            let dot_color = if done {
                t.status_success
            } else if p.speed_bps > 1.0 {
                t.action_primary
            } else {
                t.fg_4
            };
            rows = rows.push(
                row![
                    container(
                        text(format!("{}", i + 1))
                            .font(theme::MONO)
                            .size(11.0)
                            .color(t.fg_3)
                    )
                    .width(Length::Fixed(28.0)),
                    container(crate::gui::widget::dot(8.0, dot_color)).width(Length::Fixed(80.0)),
                    container(
                        text(format_bytes(p.downloaded))
                            .font(theme::MONO)
                            .size(11.0)
                            .color(t.fg_2)
                    )
                    .width(Length::Fixed(100.0)),
                    container(
                        text(format_bytes(p.size))
                            .font(theme::MONO)
                            .size(11.0)
                            .color(t.fg_2)
                    )
                    .width(Length::Fixed(90.0)),
                    pill_progress(frac, Length::Fill, 6.0, t.progress_track, t.progress_fill),
                    container(
                        text(format!("{}%", (frac * 100.0).round() as u32))
                            .font(theme::MONO)
                            .size(11.0)
                            .color(t.fg_2)
                    )
                    .width(Length::Fixed(48.0))
                    .align_x(Alignment::End),
                ]
                .spacing(theme::space::S1)
                .align_y(Alignment::Center)
                .height(Length::Fixed(28.0)),
            );
        }
        scrollable(rows).height(Length::Shrink).into()
    };

    column![
        strip,
        collapsible_card(
            t,
            "Transfer rate",
            None,
            st.rate_open,
            Msg::ToggleRate,
            || { rate_body.into() }
        ),
        collapsible_card(
            t,
            "Segments",
            Some(segments_right.into()),
            st.segments_open,
            Msg::ToggleSegments,
            || segments_body,
        ),
    ]
    .spacing(theme::space::S3)
    .into()
}

fn speed_tab(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let body = column![
        checkbox(
            t,
            "Use speed limiter",
            st.use_limiter,
            true,
            Msg::UseLimiter
        ),
        row![
            text("Maximum speed (KB/s)")
                .font(theme::BODY)
                .size(13.0)
                .color(if st.use_limiter { t.fg_1 } else { t.fg_3 }),
            TextInput::new(&st.limit_kbs)
                .width(Length::Fixed(120.0))
                .enabled(st.use_limiter)
                .on_input(Msg::LimitKbs)
                .view(t),
        ]
        .spacing(theme::space::S3)
        .align_y(Alignment::Center),
        checkbox(
            t,
            "Remember for this file",
            st.remember_limit,
            st.use_limiter,
            Msg::RememberLimit
        ),
        hairline(t.border_subtle),
        row![
            text("Max parallel connections")
                .font(theme::BODY)
                .size(13.0)
                .color(t.fg_1),
            TextInput::new(&st.max_conn)
                .width(Length::Fixed(60.0))
                .on_input(Msg::MaxConn)
                .view(t),
            text("(1–16, blank = auto)")
                .font(theme::BODY)
                .size(12.0)
                .color(t.fg_3),
        ]
        .spacing(theme::space::S3)
        .align_y(Alignment::Center),
        Btn::new("Apply")
            .primary()
            .size(crate::gui::widget::BtnSize::Sm)
            .on_press(Msg::ApplySpeed)
            .view(t),
    ]
    .spacing(theme::space::S3);
    crate::gui::widget::card(t, theme::space::S3, body.into())
}

fn completion_tab(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let oc = &st.on_completion;
    let power_on = oc.shutdown.is_some();
    let power_label = match oc.shutdown {
        Some(ShutdownAction::Restart) => "Restart",
        Some(ShutdownAction::Sleep) => "Sleep",
        _ => "Shut down",
    };
    let body = column![
        checkbox(
            t,
            "Show notification when done",
            oc.show_dialog,
            true,
            Msg::NotifyDone
        ),
        checkbox(t, "Exit oxdm when done", oc.exit_app, true, Msg::ExitDone),
        row![
            checkbox(t, "Power action", power_on, true, Msg::PowerEnabled),
            combo(
                t,
                vec![
                    "Shut down".to_owned(),
                    "Restart".to_owned(),
                    "Sleep".to_owned()
                ],
                power_on.then(|| power_label.to_owned()),
                Msg::PowerAction,
                Length::Fixed(140.0),
            ),
        ]
        .spacing(theme::space::S3)
        .align_y(Alignment::Center),
        checkbox(
            t,
            "Force terminate",
            oc.force_terminate,
            true,
            Msg::ForceTerminate
        ),
        Btn::new("Apply")
            .primary()
            .size(crate::gui::widget::BtnSize::Sm)
            .on_press(Msg::ApplyCompletion)
            .view(t),
    ]
    .spacing(theme::space::S3);
    crate::gui::widget::card(t, theme::space::S3, body.into())
}

fn complete_view(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let name = st
        .entry
        .job
        .filename
        .clone()
        .unwrap_or_else(|| "download".to_owned());
    let total = st
        .entry
        .counters
        .total
        .unwrap_or(st.entry.counters.downloaded);
    let path = final_path(&st.entry).display().to_string();
    let address = st.entry.job.url.to_string();

    let ext = PathBuf::from(&name)
        .extension()
        .map(|e| e.to_string_lossy().to_uppercase())
        .unwrap_or_else(|| "FILE".into());
    let tile_bg = color::mix(t.bg_surface, color::clay::C400, 0.20);
    let t2 = *t;
    let tile = container(
        text(ext)
            .font(theme::MONO_BOLD)
            .size(12.0)
            .color(color::clay::C400),
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
    });

    let header = container(
        row![
            tile,
            column![
                text("Download complete")
                    .font(theme::DISPLAY)
                    .size(20.0)
                    .color(t.fg_1),
                text(format!(
                    "Downloaded {} ({} bytes)",
                    format_bytes_2(total),
                    crate::gui::format::format_int_grouped(total)
                ))
                .font(theme::BODY)
                .size(12.0)
                .color(t.fg_2),
            ]
            .spacing(4.0),
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

    let label = |s: &'static str| text(s).font(theme::BODY).size(11.0).color(t2.fg_3);
    // Read-only "input": mono text in an input-styled box (egui used a
    // non-interactive TextEdit).
    let ro_field = |v: String| {
        container(text(v).font(theme::MONO).size(11.0).color(t2.fg_1))
            .width(Length::Fill)
            .height(Length::Fixed(theme::control::H_MD))
            .align_y(Alignment::Center)
            .padding([0.0, theme::control::INPUT_PAD_X])
            .style(move |_| container::Style {
                background: Some(t2.bg_raised.into()),
                border: iced::Border {
                    color: t2.border_subtle,
                    width: 1.0,
                    radius: theme::control::RADIUS.into(),
                },
                ..Default::default()
            })
    };

    page(
        t,
        column![
            titlebar::titlebar(t, &name, false, Msg::Window),
            hairline(t.border_subtle),
            container(
                column![
                    header,
                    label("Address"),
                    ro_field(address),
                    label("The file saved as"),
                    ro_field(path),
                    row![
                        Btn::new("Open")
                            .primary()
                            .icon("play")
                            .on_press(Msg::Open)
                            .view(t),
                        Btn::new("Open Containing Folder")
                            .toolbar()
                            .icon("folder")
                            .on_press(Msg::OpenFolder)
                            .view(t),
                        iced::widget::Space::new().width(Length::Fill),
                        Btn::new("Close")
                            .toolbar()
                            .icon("x")
                            .on_press(Msg::CloseWin)
                            .view(t),
                    ]
                    .spacing(theme::space::S2)
                    .align_y(Alignment::Center),
                    checkbox(
                        t,
                        "Don't show this dialog again",
                        !st.show_complete_dialog,
                        true,
                        Msg::DontShowAgain
                    ),
                ]
                .spacing(theme::space::S3)
            )
            .padding(theme::space::S4)
            .height(Length::Fill),
        ]
        .into(),
    )
}

pub fn launch_download(_id: JobId) {
    let mut app = iced::application(boot, update, view)
        .title(|app: &App| match app {
            App::Ready(st) => st
                .entry
                .job
                .filename
                .clone()
                .map(|n| format!("oxdm — download {n}"))
                .unwrap_or_else(|| "oxdm — download".to_owned()),
            _ => "oxdm — download".to_owned(),
        })
        .theme(|app: &App| match app {
            App::Ready(st) => st.tokens.iced_theme(),
            _ => Tokens::dark().iced_theme(),
        })
        .subscription(subscription)
        .default_font(theme::BODY)
        .antialiasing(true)
        .window(chrome::window_settings(
            iced::Size::new(540.0, 460.0),
            iced::Size::new(530.0, 410.0),
        ));
    for f in theme::fonts::ALL {
        app = app.font(*f);
    }
    if let Err(e) = app.run() {
        eprintln!("gui error: {e}");
        std::process::exit(1);
    }
}

//! Per-job download window (`oxdm gui download <id>`): header card
//! with 56px tile + % readout, striped progress, Info / Speed /
//! On Completion tabs, transfer-rate chart, segments table, footer —
//! and the "Download complete" view once the job finishes.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use iced::widget::{canvas, column, container, row, stack, text};
use iced::{Alignment, Element, Length, Point, Rectangle, Subscription, Task};

use crate::domain::checksum::{Algo, CsStatus};
use crate::domain::{JobError, JobId, OnCompletion, Phase, ShutdownAction};
use crate::gui::chrome::{self, WindowControl, titlebar};
use crate::gui::color;
use crate::gui::format::{format_bytes, format_bytes_2, format_eta, format_speed};
use crate::gui::icons;
use crate::gui::ipc::DaemonSignal;
use crate::gui::shot::Shot;
use crate::gui::theme::{self, Tokens};
use crate::gui::widget::error_panel::{
    HASH_TRUNCATE_CHARS, error_block, hash_mismatch, mid_truncate,
};
use crate::gui::widget::{
    Btn, BtnSize, RateChart, TabBtn, TextInput, card, checkbox, collapsible_card, combo, hairline,
    number_stepper, pill_progress, rate_chart, segmented, sibling, status_dot, striped_progress,
    toggle,
};
use crate::gui::windows::add::footer;
use crate::ipc_local::Client;
use crate::ipc_local::protocol::{Event, JobEntryView};

const CHART_SAMPLES: usize = 120;

// --- Speed tab (design §3.3 "Speed" pane) ----------------------------
/// Max parallel connections stepper bounds — mirrors the runner's
/// per-job part cap (`ApplySpeed` filters to `1..=16`).
const MAX_CONN_MIN: i64 = 1;
const MAX_CONN_MAX: i64 = 16;
/// Stepper seed when the job has no explicit `max_connections` (auto).
const MAX_CONN_DEFAULT: i64 = 8;
const BYTES_PER_KB: u64 = 1024;
const KB_PER_MB: u64 = 1024;
/// Width of the speed-limit value input (KB/s ‖ MB/s numeric field).
const LIMIT_INPUT_W: f32 = 80.0;
/// Dashed quick-preset pills (design `.qp`), values in KB/s.
const SPEED_PRESETS_KBS: &[(&str, u64)] = &[
    ("64 KB/s", 64),
    ("256 KB/s", 256),
    ("1 MB/s", 1024),
    ("10 MB/s", 10240),
];

// --- Completed view (design §3.3 completed / §3.4 ChecksumBox) -------
/// Middle-truncation budget (chars) for the saved-path / source-URL
/// rows so long values stay on one line. (Hash truncation lives in
/// `widget::error_panel::HASH_TRUNCATE_CHARS`.)
const PATH_TRUNCATE_CHARS: usize = 52;

// --- Completion burst (design §3.3 `.complete-burst`, anim `cb-pop`) -
/// 88px burst stage — two pulsing rings around a gradient check circle.
const BURST_STAGE: f32 = 88.0;
/// Inner gradient circle diameter (design 64px clay circle).
const BURST_CIRCLE: f32 = 64.0;
/// Check / shield-alert glyph size, centered over the burst circle.
const BURST_ICON: f32 = 30.0;
/// Outer-ring max radius as a fraction of the stage half-extent — the
/// rings breathe between the circle edge and this on each pulse.
const BURST_RING_MAX: f32 = 0.96;
/// Burst/pulse oscillation rate (rad/s feel applied to `anim_t`).
const PULSE_RATE: f32 = 3.2;

// --- Reconnect banner (design §3.3 `.reconnect-banner`, ochre) -------
/// Banner background alpha floor/ceiling for the gentle ochre pulse.
const RECONNECT_PULSE_MIN: f32 = 0.55;
const RECONNECT_PULSE_MAX: f32 = 1.0;

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
    LimitUnit(bool),  // false = KB/s, true = MB/s
    SpeedPreset(u64), // quick-set value, in KB/s
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
    // Completed view — copy / reveal / checksum verify
    Copy(String),
    Reveal(PathBuf),
    CsPaste(String),
    // Local checksum compute (hash `final_path` off the UI executor).
    CsCompute,
    CsComputed(Result<String, String>),
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
    limit_unit_mb: bool,
    remember_limit: bool,
    max_conn: String,

    on_completion: OnCompletion,

    /// Hash the user pasted into the completed-view ChecksumBox to
    /// compare against the job's saved checksum (verify, not compute).
    cs_paste: String,
    /// Local "Compute from file" state — drives the button label and the
    /// match/mismatch render once a digest comes back.
    cs_compute: CsCompute,

    /// Gates every animation (reconnect pulse, completion burst). Read
    /// once at boot from `Settings.reduce_motion` (W6).
    reduce_motion: bool,

    shot: Option<Shot>,
}

/// Local-compute lifecycle for the completed-view ChecksumBox.
#[derive(Default)]
enum CsCompute {
    #[default]
    Idle,
    Running,
    /// Digest hex on success, or a short error string.
    Done(Result<String, String>),
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
                limit_unit_mb: false,
                remember_limit: limit.is_some(),
                max_conn: entry
                    .job
                    .max_connections
                    .map(|n| n.to_string())
                    .unwrap_or_default(),
                on_completion,
                cs_paste: String::new(),
                cs_compute: CsCompute::Idle,
                reduce_motion: settings.reduce_motion,
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
        Msg::LimitUnit(mb) => {
            st.limit_unit_mb = mb;
            Task::none()
        }
        Msg::SpeedPreset(kbs) => {
            st.use_limiter = true;
            // Render whole-MB presets in MB/s, sub-MB in KB/s.
            if kbs >= KB_PER_MB && kbs % KB_PER_MB == 0 {
                st.limit_unit_mb = true;
                st.limit_kbs = (kbs / KB_PER_MB).to_string();
            } else {
                st.limit_unit_mb = false;
                st.limit_kbs = kbs.to_string();
            }
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
            // Value field is in the selected unit (KB/s or MB/s); convert
            // to bytes/sec for the daemon. KB/s stays `* 1024` (unchanged).
            let unit = if st.limit_unit_mb {
                BYTES_PER_KB * KB_PER_MB
            } else {
                BYTES_PER_KB
            };
            let bps = st
                .use_limiter
                .then(|| st.limit_kbs.trim().parse::<u64>().ok().map(|v| v * unit))
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
            chrome::enforce_min_size(iced::Size::new(w, h), iced::Size::new(530.0, 418.0))
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
        Msg::Copy(s) => iced::clipboard::write(s),
        Msg::Reveal(path) => {
            crate::platform::reveal_in_folder(&path);
            Task::none()
        }
        Msg::CsPaste(v) => {
            st.cs_paste = v;
            Task::none()
        }
        Msg::CsCompute => {
            // Hash with the saved checksum's algorithm so the digest is
            // directly comparable. Run the streaming hasher on a blocking
            // thread (N3: never on the iced UI executor).
            let Some(cs) = st.entry.job.checksums.first() else {
                return Task::none();
            };
            let Some(path) = st.entry.job.status.final_path.clone() else {
                return Task::none();
            };
            if matches!(st.cs_compute, CsCompute::Running) {
                return Task::none();
            }
            let algo = cs.algo;
            st.cs_compute = CsCompute::Running;
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
                Msg::CsComputed,
            )
        }
        Msg::CsComputed(res) => {
            st.cs_compute = CsCompute::Done(res);
            Task::none()
        }
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
        crate::gui::ipc::all_events(crate::ipc_local::protocol::GuiKind::Download(st.id))
            .map(Msg::Daemon),
    ];
    if st.shot.is_some() {
        subs.push(Shot::frames().map(|_| Msg::ShotTick));
    }
    if st.phase().is_running() {
        // The 30fps tick only drives motion (barber-pole stripes,
        // reconnect pulse); skip it under reduce_motion (W6). Rate
        // sampling is data, not motion, so it stays.
        if !st.reduce_motion {
            subs.push(iced::time::every(Duration::from_millis(33)).map(|_| Msg::AnimTick));
        }
        if st.rate_open {
            subs.push(iced::time::every(Duration::from_millis(500)).map(|_| Msg::SampleTick));
        }
    } else if st.phase() == Phase::Completed && !st.reduce_motion {
        // Drive the completion-burst pulse; the running branch's tick is
        // gone once terminal, so the burst needs its own (W6-gated) tick.
        subs.push(iced::time::every(Duration::from_millis(33)).map(|_| Msg::AnimTick));
    }
    Subscription::batch(subs)
}

// ---------------------------------------------------------------- view

pub fn view(app: &App) -> Element<'_, Msg> {
    chrome::framed(match app {
        App::Connecting => splash("Connecting…".to_owned()),
        App::Failed(e) => splash(e.clone()),
        App::Ready(st) => {
            if st.phase() == Phase::Completed {
                complete_view(st)
            } else {
                running_view(st)
            }
        }
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
    // Barber-pole stripes are motion → off under reduce_motion (W6).
    let striped = phase.is_running() && !st.reduce_motion;
    let (track, fill, gradient) = match phase {
        Phase::Failed => (t.status_danger_bg, t.status_danger, None),
        // Reconnecting reads ochre (design `is-reconnecting`), pairing
        // with the banner above; still striped (it's a running phase).
        Phase::Reconnecting => (
            t.progress_track,
            t.status_warning,
            Some((color::ochre::O400, color::ochre::O300)),
        ),
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

    // A severe error replaces the tabs + pane entirely (design §3.3
    // "Severe error"): friendly title → detail → what-to-check → quiet
    // code footer, driven only by the real `JobStatus.error` field.
    let error = st.entry.job.status.error.clone();

    let lower: Element<'_, Msg> = if let Some(err) = &error {
        crate::gui::widget::vscroll(error_block(&st.tokens, err, Msg::Copy(err.to_string())))
            .height(Length::Fill)
            .into()
    } else {
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

        column![
            // Tabs + hairline as one unspaced group so the active
            // underline sits on the hairline.
            sibling(column![tabs, hairline(t.border_subtle)].into()),
            crate::gui::widget::vscroll(tab_body).height(Length::Fill),
        ]
        .spacing(theme::space::S3)
        .into()
    };

    // Footer morphs on error kind (design §3.3 "context-dependent on
    // error kind"); the normal transfer footer is pause/resume + cancel.
    let footer_right: Element<'_, Msg> = match &error {
        Some(err) => error_footer(t, err),
        None => row![
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
                .danger()
                .icon("x")
                .on_press(Msg::Cancel)
                .view(t),
        ]
        .spacing(theme::space::S2)
        .into(),
    };
    let footer_el = footer(
        t,
        Btn::new("Minimize to tray")
            .toolbar()
            .icon("minimize-2")
            .on_press(Msg::MinimizeTray)
            .view(t),
        footer_right,
    );

    // Reconnect banner sits above the progress bar (design §3.3),
    // ochre, only while the whole transfer is mid-retry.
    let mut hero = column![sibling(header_card(st))].spacing(theme::space::S3);
    if phase == Phase::Reconnecting {
        hero = hero.push(reconnect_banner(st));
    }
    hero = hero
        .push(sibling(striped_progress(
            st.frac(),
            Length::Fill,
            10.0,
            track,
            fill,
            gradient,
            striped,
            st.anim_t,
        )))
        .push(lower);

    page(
        t,
        column![
            titlebar::titlebar(t, &name, false, Msg::Window),
            hairline(t.border_subtle),
            container(hero)
                .padding(iced::Padding {
                    top: theme::space::S4,
                    bottom: theme::space::S4,
                    left: theme::space::S4,
                    right: theme::space::S4 - crate::gui::widget::SCROLL_GUTTER,
                })
                .height(Length::Fill),
            hairline(t.border_subtle),
            footer_el,
        ]
        .into(),
    )
}

/// Error-state footer button group (right side). Maps the `JobError`
/// kind to the actions that make sense, wiring ONLY to messages that
/// already exist (Resume = retry a failed job, OpenFolder, CloseWin =
/// keep). Cancel stays the danger action in every variant. Buttons
/// whose backend action doesn't exist in this window (e.g. delete the
/// tampered file on disk) are intentionally omitted, not invented.
fn error_footer<'a>(t: &Tokens, err: &JobError) -> Element<'a, Msg> {
    let cancel = Btn::new("Cancel")
        .danger()
        .icon("x")
        .on_press(Msg::Cancel)
        .view(t);
    let retry = || {
        Btn::new("Retry")
            .primary()
            .icon("rotate-cw")
            .on_press(Msg::PauseResume)
            .view(t)
    };
    let group = match err {
        // Write / disk problems: offer the folder so the user can free
        // space or fix permissions, then cancel.
        JobError::Io(_) | JobError::SaveConflict(_) => row![
            Btn::new("Open folder")
                .toolbar()
                .icon("folder")
                .on_press(Msg::OpenFolder)
                .view(t),
            cancel,
        ],
        // Integrity failure: "Keep" closes (file stays on disk); the
        // on-disk "Delete" action has no message in this window → omit.
        JobError::ChecksumMismatch { .. } => row![
            Btn::new("Keep")
                .toolbar()
                .icon("check")
                .on_press(Msg::CloseWin)
                .view(t),
            cancel,
        ],
        // Transient network/DNS + everything else: retry then cancel.
        _ => row![retry(), cancel],
    };
    group.spacing(theme::space::S2).into()
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
            label_color: t.fg_3,
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
        crate::gui::widget::vscroll(rows)
            .height(Length::Shrink)
            .into()
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
    let limited = st.use_limiter;

    // Chip-toggle Unlimited / Limit-to (design `.chip-toggle`).
    let limiter_chips = segmented(
        t,
        &[("Unlimited", None), ("Limit to", None)],
        if limited { 1 } else { 0 },
        BtnSize::Sm,
        |i| Msg::UseLimiter(i == 1),
    );

    // value field + KB/s ‖ MB/s unit-toggle.
    let unit_toggle = segmented(
        t,
        &[("KB/s", None), ("MB/s", None)],
        if st.limit_unit_mb { 1 } else { 0 },
        BtnSize::Sm,
        |i| Msg::LimitUnit(i == 1),
    );
    let value_row = row![
        TextInput::new(&st.limit_kbs)
            .width(Length::Fixed(LIMIT_INPUT_W))
            .enabled(limited)
            .on_input(Msg::LimitKbs)
            .view(t),
        unit_toggle,
    ]
    .spacing(theme::space::S2)
    .align_y(Alignment::Center);

    // Dashed quick-preset pills (design `.qp`). iced/tiny-skia can't
    // dash a border, so these read as small outlined pills.
    let mut presets = row![text("Quick set").font(theme::BODY).size(12.0).color(t.fg_3)]
        .spacing(theme::space::S2)
        .align_y(Alignment::Center);
    for (label, kbs) in SPEED_PRESETS_KBS {
        presets = presets.push(
            Btn::new(*label)
                .secondary()
                .size(BtnSize::Sm)
                .on_press(Msg::SpeedPreset(*kbs))
                .view(t),
        );
    }

    let limit_row = row![
        text("Speed limit")
            .font(theme::BODY)
            .size(13.0)
            .color(t.fg_1)
            .width(Length::Fill),
        limiter_chips,
    ]
    .spacing(theme::space::S3)
    .align_y(Alignment::Center);

    let mut body = column![limit_row].spacing(theme::space::S3);
    if limited {
        body = body.push(value_row).push(presets).push(toggle_row(
            t,
            "Remember for this file",
            st.remember_limit,
            true,
            Msg::RememberLimit,
        ));
    }

    // Blank `max_conn` = auto (daemon picks); a non-empty value is an
    // explicit 1–16 override. The Auto/Custom chip-toggle makes the
    // auto state visible and re-selectable; ApplySpeed wiring (blank ⇒
    // `conns = None` ⇒ auto) is unchanged.
    let conn_auto = st.max_conn.trim().is_empty();
    let conn_val = st
        .max_conn
        .trim()
        .parse::<i64>()
        .unwrap_or(MAX_CONN_DEFAULT)
        .clamp(MAX_CONN_MIN, MAX_CONN_MAX);
    let conn_mode = segmented(
        t,
        &[("Auto", None), ("Custom", None)],
        if conn_auto { 0 } else { 1 },
        BtnSize::Sm,
        |i| {
            if i == 0 {
                Msg::MaxConn(String::new())
            } else {
                Msg::MaxConn(MAX_CONN_DEFAULT.to_string())
            }
        },
    );
    let mut conn_controls = row![conn_mode]
        .spacing(theme::space::S2)
        .align_y(Alignment::Center);
    if !conn_auto {
        conn_controls = conn_controls.push(number_stepper(
            t,
            conn_val,
            MAX_CONN_MIN,
            MAX_CONN_MAX,
            true,
            |n| Msg::MaxConn(n.to_string()),
        ));
    }
    let conn_row = row![
        column![
            text("Max parallel connections")
                .font(theme::BODY)
                .size(13.0)
                .color(t.fg_1),
            text("Auto lets oxdm choose; applying reconnects active segments.")
                .font(theme::BODY)
                .size(11.0)
                .color(t.fg_3),
        ]
        .spacing(2.0)
        .width(Length::Fill),
        conn_controls,
    ]
    .spacing(theme::space::S3)
    .align_y(Alignment::Center);

    let body = body.push(hairline(t.border_subtle)).push(conn_row).push(
        Btn::new("Apply")
            .primary()
            .size(BtnSize::Sm)
            .on_press(Msg::ApplySpeed)
            .view(t),
    );
    card(t, theme::space::S3, body.into())
}

/// Settings-style toggle row: label (+optional hint) left, switch right.
fn toggle_row<'a>(
    t: &Tokens,
    label: &'a str,
    on: bool,
    enabled: bool,
    msg: impl Fn(bool) -> Msg + 'a,
) -> Element<'a, Msg> {
    row![
        text(label)
            .font(theme::BODY)
            .size(13.0)
            .color(if enabled { t.fg_1 } else { t.fg_3 })
            .width(Length::Fill),
        toggle(t, on, enabled, msg),
    ]
    .spacing(theme::space::S3)
    .align_y(Alignment::Center)
    .into()
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

    let power_row = row![
        text("Power action")
            .font(theme::BODY)
            .size(13.0)
            .color(t.fg_1)
            .width(Length::Fill),
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
        toggle(t, power_on, true, Msg::PowerEnabled),
    ]
    .spacing(theme::space::S3)
    .align_y(Alignment::Center);

    let mut body = column![
        toggle_row(
            t,
            "Show notification when done",
            oc.show_dialog,
            true,
            Msg::NotifyDone
        ),
        toggle_row(t, "Exit oxdm when done", oc.exit_app, true, Msg::ExitDone),
        power_row,
        toggle_row(
            t,
            "Force terminate other transfers",
            oc.force_terminate,
            true,
            Msg::ForceTerminate
        ),
    ]
    .spacing(theme::space::S3);

    if let Some(warn) = completion_warn(st) {
        body = body.push(warn);
    }
    body = body.push(
        Btn::new("Apply")
            .primary()
            .size(BtnSize::Sm)
            .on_press(Msg::ApplyCompletion)
            .view(t),
    );
    card(t, theme::space::S3, body.into())
}

/// Destructive-action warning panel (design `.pane-warn`, rust). Lists
/// exactly what will happen and promises the shutdown-grace cancel
/// prompt (`SHUTDOWN_GRACE_SECS` = 60 s; F4 reconciliation of the
/// mock's 30 s vs 60 s contradiction).
/// Built from the real `OnCompletion` / `ShutdownAction` values.
fn completion_warn(st: &State) -> Option<Element<'_, Msg>> {
    let t = &st.tokens;
    let t2 = *t;
    let oc = &st.on_completion;

    let mut items: Vec<&'static str> = Vec::new();
    match oc.shutdown {
        Some(ShutdownAction::ShutDown) => items.push("Your computer will shut down."),
        Some(ShutdownAction::Restart) => items.push("Your computer will restart."),
        Some(ShutdownAction::Sleep) => items.push("Your computer will go to sleep."),
        None => {}
    }
    if oc.exit_app {
        items.push("oxdm will quit.");
    }
    if oc.force_terminate {
        items.push("Other running transfers will be terminated without finishing.");
    }
    if items.is_empty() {
        return None;
    }

    let mut list = column![
        row![
            icons::icon("triangle-alert", 17.0, t.status_danger),
            text("This will run destructive actions when the download finishes")
                .font(theme::BODY_BOLD)
                .size(12.0)
                .color(t.status_danger),
        ]
        .spacing(theme::space::S2)
        .align_y(Alignment::Center)
    ]
    .spacing(theme::space::S2);
    for it in items {
        list = list.push(
            row![
                text("•")
                    .font(theme::BODY)
                    .size(12.0)
                    .color(t.status_danger),
                text(it).font(theme::BODY).size(12.0).color(t.fg_2),
            ]
            .spacing(theme::space::S2),
        );
    }
    list = list.push(
        text(format!(
            "You'll get a {}-second prompt to cancel before any of this happens.",
            crate::domain::SHUTDOWN_GRACE_SECS
        ))
        .font(theme::BODY)
        .size(11.0)
        .color(t.fg_3),
    );

    Some(
        container(list)
            .width(Length::Fill)
            .padding(theme::space::S3)
            .style(move |_| container::Style {
                background: Some(t2.status_danger_bg.into()),
                border: iced::Border {
                    color: t2.status_danger,
                    width: 1.0,
                    radius: theme::surface::RADIUS.into(),
                },
                ..Default::default()
            })
            .into(),
    )
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
    // Tampered = saved or computed checksum mismatch → escalate the
    // whole completed view (rust accent, "don't open" warning) per
    // design §3.3 tampered variant.
    let tampered = is_tampered(st);
    let accent = if tampered {
        color::rust::R300
    } else {
        color::clay::C400
    };
    let tile_bg = color::mix(t.bg_surface, accent, 0.20);
    let t2 = *t;
    let tile = container(text(ext).font(theme::MONO_BOLD).size(12.0).color(accent))
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

    let (title_text, title_color) = if tampered {
        ("Integrity check failed", t.status_danger)
    } else {
        ("Download complete", t.fg_1)
    };
    let header = container(
        row![
            tile,
            column![
                text(title_text)
                    .font(theme::DISPLAY)
                    .size(20.0)
                    .color(title_color),
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

    // "From" (source URL) row — copy only (design `FromUrlRow`).
    let from_row = column![
        label("Address"),
        row![
            ro_field(mid_truncate(&address, PATH_TRUNCATE_CHARS)),
            Btn::new("")
                .secondary()
                .icon_only("copy")
                .size(BtnSize::Md)
                .on_press(Msg::Copy(address.clone()))
                .view(t),
        ]
        .spacing(6.0)
        .align_y(Alignment::Center),
    ]
    .spacing(6.0);

    // "Saved to" row — copy + reveal-in-folder (design `SavedToRow`).
    let saved_row = column![
        label("The file saved as"),
        row![
            ro_field(mid_truncate(&path, PATH_TRUNCATE_CHARS)),
            Btn::new("")
                .secondary()
                .icon_only("copy")
                .size(BtnSize::Md)
                .on_press(Msg::Copy(path.clone()))
                .view(t),
            Btn::new("")
                .secondary()
                .icon_only("folder-open")
                .size(BtnSize::Md)
                .on_press(Msg::Reveal(final_path(&st.entry)))
                .view(t),
        ]
        .spacing(6.0)
        .align_y(Alignment::Center),
    ]
    .spacing(6.0);

    // Burst stage, centered above the header (`cb-pop`).
    let burst = container(completion_burst(st, tampered))
        .width(Length::Fill)
        .align_x(Alignment::Center);

    let mut body = column![burst, header].spacing(theme::space::S3);
    // Tampered files get a heavy "don't open" warning right under the
    // header (design `.tamper-banner`).
    if tampered {
        body = body.push(banner(
            t,
            t.status_danger,
            t.status_danger_bg,
            "shield-alert",
            "Don't open this file — it may be corrupted, compromised, or intercepted.".to_owned(),
        ));
    }
    if let Some(stats) = completion_stats(st) {
        body = body.push(stats);
    }
    body = body.push(from_row).push(saved_row);
    if let Some(cs_box) = checksum_box(st) {
        body = body.push(cs_box);
    }
    body = body
        .push(
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
        )
        .push(checkbox(
            t,
            "Don't show this dialog again",
            !st.show_complete_dialog,
            true,
            Msg::DontShowAgain,
        ));

    page(
        t,
        column![
            titlebar::titlebar(t, &name, false, Msg::Window),
            hairline(t.border_subtle),
            container(crate::gui::widget::vscroll(body).height(Length::Fill))
                .padding(iced::Padding {
                    top: theme::space::S4,
                    bottom: theme::space::S4,
                    left: theme::space::S4,
                    right: theme::space::S4 - crate::gui::widget::SCROLL_GUTTER,
                })
                .height(Length::Fill),
        ]
        .into(),
    )
}

/// Completed-view ChecksumBox (design §3.4): shows the job's saved
/// checksum + status, a paste field to verify against the publisher's
/// hash, AND a local "Compute from file" action that hashes the saved
/// file (off the UI executor) and compares. Algorithm is auto-detected
/// from hex length for paste; compute uses the saved checksum's algo.
fn checksum_box(st: &State) -> Option<Element<'_, Msg>> {
    let t = &st.tokens;
    let cs = st.entry.job.checksums.first()?;

    let (status_color, status_label) = match cs.status {
        CsStatus::Verified => (t.status_success, "verified"),
        CsStatus::Mismatch => (t.status_danger, "mismatch"),
        CsStatus::Unverified => (t.fg_3, "unverified"),
    };

    let saved_hash = cs.hash.to_lowercase();
    let intro = row![
        icons::icon("shield-check", 17.0, t.action_primary),
        text("File integrity")
            .font(theme::BODY_BOLD)
            .size(13.0)
            .color(t.fg_1),
        iced::widget::Space::new().width(Length::Fill),
        status_dot(status_color, status_label, 11.0),
    ]
    .spacing(theme::space::S2)
    .align_y(Alignment::Center);

    let saved_row = row![
        container(
            text(cs.algo.label())
                .font(theme::MONO)
                .size(11.0)
                .color(t.fg_2)
        )
        .width(Length::Fixed(72.0)),
        text(mid_truncate(&saved_hash, HASH_TRUNCATE_CHARS))
            .font(theme::MONO)
            .size(11.0)
            .color(t.fg_2)
            .width(Length::Fill),
        Btn::new("")
            .toolbar()
            .icon_only("copy")
            .size(BtnSize::Sm)
            .on_press(Msg::Copy(cs.hash.clone()))
            .view(t),
    ]
    .spacing(theme::space::S2)
    .align_y(Alignment::Center);

    let paste_field = TextInput::new(&st.cs_paste)
        .hint("Paste the publisher's hash to compare…")
        .mono()
        .on_input(Msg::CsPaste)
        .view(t);

    // Normalize: drop whitespace + a leading "filename:" prefix, lower.
    let normalized: String = st
        .cs_paste
        .rsplit(':')
        .next()
        .unwrap_or(&st.cs_paste)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_lowercase();
    let detected = Algo::ALL.iter().find(|a| a.hex_len() == normalized.len());

    let result: Element<'_, Msg> = if normalized.is_empty() {
        text("The algorithm is detected automatically from the hash length.")
            .font(theme::BODY)
            .size(11.0)
            .color(t.fg_3)
            .into()
    } else if let Some(algo) = detected {
        if normalized == saved_hash {
            banner(
                t,
                t.status_success,
                t.status_success_bg,
                "circle-check",
                format!("Matches the saved {} hash.", algo.label()),
            )
        } else {
            // Saved algo (cs.algo), NOT the pasted-length auto-detected
            // `algo` — they differ when the pasted hash is wrong-length.
            hash_mismatch(t, cs.algo.label(), &saved_hash, &normalized)
        }
    } else {
        text(format!(
            "Doesn't look like a known hash ({} hex chars).",
            normalized.len()
        ))
        .font(theme::BODY)
        .size(11.0)
        .color(t.status_warning)
        .into()
    };

    // Local "Compute from file" — only when the file exists on disk.
    // Hashes with the saved checksum's algorithm so the digest compares
    // directly; the heavy work runs off the UI executor (see update).
    let compute_section: Option<Element<'_, Msg>> =
        st.entry.job.status.final_path.as_ref().map(|_| {
            let action: Element<'_, Msg> = match &st.cs_compute {
                CsCompute::Running => Btn::new("Computing…")
                    .secondary()
                    .size(BtnSize::Sm)
                    .icon("refresh-cw")
                    .enabled(false)
                    .view(t),
                _ => Btn::new("Compute from file")
                    .secondary()
                    .size(BtnSize::Sm)
                    .icon("shield-check")
                    .on_press(Msg::CsCompute)
                    .view(t),
            };
            let row_el = row![
                action,
                text("Hash the saved file and compare to the saved checksum.")
                    .font(theme::BODY)
                    .size(11.0)
                    .color(t.fg_3),
            ]
            .spacing(theme::space::S2)
            .align_y(Alignment::Center);

            match &st.cs_compute {
                CsCompute::Done(Ok(digest)) => {
                    let got = digest.to_lowercase();
                    let result: Element<'_, Msg> = if got == saved_hash {
                        banner(
                            t,
                            t.status_success,
                            t.status_success_bg,
                            "circle-check",
                            format!("File hash matches the saved {} hash.", cs.algo.label()),
                        )
                    } else {
                        hash_mismatch(t, cs.algo.label(), &saved_hash, &got)
                    };
                    column![row_el, result].spacing(theme::space::S2).into()
                }
                CsCompute::Done(Err(e)) => column![
                    row_el,
                    text(format!("Couldn't read the file: {e}"))
                        .font(theme::BODY)
                        .size(11.0)
                        .color(t.status_warning),
                ]
                .spacing(theme::space::S2)
                .into(),
                _ => row_el.into(),
            }
        });

    let mut content = column![intro, saved_row, paste_field, result].spacing(theme::space::S2);
    if let Some(section) = compute_section {
        content = content.push(hairline(t.border_subtle)).push(section);
    }
    Some(card(t, theme::space::S3, content.into()))
}

/// Small tinted callout: icon + message on a `*-bg` surface.
fn banner<'a>(
    _t: &Tokens,
    fg: iced::Color,
    bg: iced::Color,
    icon_name: &'a str,
    msg: String,
) -> Element<'a, Msg> {
    container(
        row![
            icons::icon(icon_name, 17.0, fg),
            text(msg).font(theme::BODY_MEDIUM).size(12.0).color(fg),
        ]
        .spacing(theme::space::S2)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding(theme::space::S3)
    .style(move |_| container::Style {
        background: Some(bg.into()),
        border: iced::Border {
            color: fg,
            width: 1.0,
            radius: theme::radius::XS.into(),
        },
        ..Default::default()
    })
    .into()
}

/// Ochre "Reconnecting…" banner shown above the progress bar while the
/// whole transfer is mid-retry (`Phase::Reconnecting`). Appends the
/// live attempt count from `job.retries` when known, and gently pulses
/// its tint unless `reduce_motion` (W6).
fn reconnect_banner(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let fg = t.status_warning;
    // Map a sine of the running anim clock to the configured alpha band.
    let pulse = if st.reduce_motion {
        RECONNECT_PULSE_MAX
    } else {
        let s = (st.anim_t * PULSE_RATE).sin() * 0.5 + 0.5;
        RECONNECT_PULSE_MIN + (RECONNECT_PULSE_MAX - RECONNECT_PULSE_MIN) * s
    };
    let bg = color::with_alpha(t.status_warning_bg, pulse);

    let retries = st.entry.job.retries;
    let label = if retries > 0 {
        format!("Reconnecting… · attempt {retries}")
    } else {
        "Reconnecting…".to_owned()
    };

    container(
        row![
            icons::icon("rotate-cw", 17.0, fg),
            text(label).font(theme::BODY_MEDIUM).size(12.0).color(fg),
        ]
        .spacing(theme::space::S2)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding(theme::space::S3)
    .style(move |_| container::Style {
        background: Some(bg.into()),
        border: iced::Border {
            color: fg,
            width: 1.0,
            radius: theme::radius::XS.into(),
        },
        ..Default::default()
    })
    .into()
}

/// Whether this completed download is *tampered*: a saved checksum
/// reports `Mismatch`, or a locally-computed digest disagrees with the
/// saved hash. Drives the rust burst + "don't open" warning.
fn is_tampered(st: &State) -> bool {
    let saved_mismatch = st
        .entry
        .job
        .checksums
        .iter()
        .any(|c| c.status == CsStatus::Mismatch);
    let computed_mismatch = match (&st.cs_compute, st.entry.job.checksums.first()) {
        (CsCompute::Done(Ok(digest)), Some(cs)) => digest.to_lowercase() != cs.hash.to_lowercase(),
        _ => false,
    };
    saved_mismatch || computed_mismatch
}

/// Completion burst (design `.complete-burst`, anim `cb-pop`): an 88px
/// stage with two pulsing rings around a gradient circle + a centered
/// glyph. Clay/check when healthy; rust/`shield-alert` when tampered.
/// Pulse is frozen (rings at rest) when `reduce_motion`.
fn completion_burst(st: &State, tampered: bool) -> Element<'_, Msg> {
    let phase_t = if st.reduce_motion { 0.0 } else { st.anim_t };
    let (ring, circle) = if tampered {
        (color::rust::R300, color::rust::R400)
    } else {
        (color::clay::C400, color::clay::C300)
    };
    let rings = canvas(Burst {
        t: phase_t,
        ring,
        circle,
    })
    .width(Length::Fixed(BURST_STAGE))
    .height(Length::Fixed(BURST_STAGE));

    let glyph = container(icons::icon(
        if tampered { "shield-alert" } else { "check" },
        BURST_ICON,
        iced::Color::WHITE,
    ))
    .width(Length::Fixed(BURST_STAGE))
    .height(Length::Fixed(BURST_STAGE))
    .align_x(Alignment::Center)
    .align_y(Alignment::Center);

    stack![rings, glyph]
        .width(Length::Fixed(BURST_STAGE))
        .height(Length::Fixed(BURST_STAGE))
        .into()
}

/// Canvas program for the burst: gradient circle + two breathing rings.
struct Burst {
    t: f32,
    ring: iced::Color,
    circle: iced::Color,
}

impl<M> canvas::Program<M> for Burst {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let center = Point::new(bounds.width / 2.0, bounds.height / 2.0);
        let half = bounds.width.min(bounds.height) / 2.0;
        let circle_r = BURST_CIRCLE / 2.0;

        // Two rings, phase-offset, breathing from the circle edge out to
        // `BURST_RING_MAX`; alpha fades as they expand (pulse-out feel).
        for i in 0..2 {
            let phase = (self.t * PULSE_RATE + i as f32 * std::f32::consts::PI).sin() * 0.5 + 0.5;
            let r = circle_r + (half * BURST_RING_MAX - circle_r) * phase;
            let alpha = (1.0 - phase) * 0.5;
            let path = canvas::Path::circle(center, r);
            frame.stroke(
                &path,
                canvas::Stroke::default()
                    .with_width(2.0)
                    .with_color(color::with_alpha(self.ring, alpha)),
            );
        }

        // Solid gradient-ish circle (vertical mix from `circle`→`ring`).
        let body = canvas::Path::circle(center, circle_r);
        frame.fill(&body, color::mix(self.circle, self.ring, 0.5));

        vec![frame.into_geometry()]
    }
}

/// Completion stat grid (design `.complete-stats`): Avg speed · Time
/// taken · Finished at, computed from `started_at`/`finished_at`. W3:
/// any cell whose source timestamp is `None` is HIDDEN (no `created_at`
/// fallback); never divides by zero. A "retried N times" sub-line shows
/// only when `job.retries > 0`. Returns `None` when nothing is showable.
fn completion_stats(st: &State) -> Option<Element<'_, Msg>> {
    let t = &st.tokens;
    let t2 = *t;
    let job = &st.entry.job;
    let downloaded = st.entry.counters.downloaded;

    let mut cells: Vec<Element<'_, Msg>> = Vec::new();

    // Avg speed + Time taken both need a [started, finished] interval.
    if let (Some(started), Some(finished)) = (job.started_at, job.finished_at) {
        let secs = (finished - started).num_seconds().max(0) as u64;
        if secs > 0 {
            let avg = downloaded as f64 / secs as f64;
            cells.push(stat(t, "avg speed", format_speed(avg), true));
        }
        cells.push(stat(t, "time taken", format_eta(secs), false));
    }
    if let Some(finished) = job.finished_at {
        let local = finished
            .with_timezone(&chrono::Local)
            .format("%H:%M")
            .to_string();
        cells.push(stat(t, "finished at", local, false));
    }

    if cells.is_empty() {
        return None;
    }

    let mut grid = row![].spacing(theme::space::S2);
    for c in cells {
        grid = grid.push(c);
    }

    let mut col = column![
        container(grid)
            .width(Length::Fill)
            .padding(theme::space::S3)
            .style(move |_| container::Style {
                background: Some(t2.bg_sunken.into()),
                border: iced::Border {
                    radius: theme::surface::RADIUS.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
    ]
    .spacing(theme::space::S1);

    if job.retries > 0 {
        col = col.push(
            text(format!("Retried {} times", job.retries))
                .font(theme::BODY)
                .size(11.0)
                .color(t.fg_3),
        );
    }
    Some(col.into())
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
            iced::Size::new(530.0, 418.0),
        ));
    for f in theme::fonts::ALL {
        app = app.font(*f);
    }
    if let Err(e) = app.run() {
        eprintln!("gui error: {e}");
        std::process::exit(1);
    }
}

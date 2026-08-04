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
    number_stepper, pill_progress, rate_chart, segmented, set_row, set_row_panel, set_rows,
    sibling, status_dot, striped_progress, toggle,
};
use crate::gui::windows::add::footer;
use crate::ipc_local::Client;
use crate::ipc_local::protocol::{Event, JobEntryView};

const CHART_SAMPLES: usize = 120;

// --- Window geometry -------------------------------------------------
/// The window opens at its floor height: the tab bodies scroll, so extra
/// launch height only ever showed empty surface.
const WIN_W: f32 = 540.0;
const WIN_MIN_W: f32 = 530.0;
/// Floor height, minus the bottom gap that moved inside the scroll port
/// and so no longer has to be reserved by the frame.
const WIN_MIN_H: f32 = 418.0 - theme::space::S4;

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
/// Idle time after the last keystroke before the typed limit is pushed
/// to the daemon. Longer than the Add window's URL-probe debounce: a
/// half-typed limit is a *live* throttle on a running transfer, so it
/// is worth waiting until the user has clearly stopped typing.
const LIMIT_DEBOUNCE_MS: u64 = 700;
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
    /// The typed limit stopped changing `LIMIT_DEBOUNCE_MS` ago —
    /// carries the edit generation it was scheduled for.
    LimitSettled(u64),
    LimitUnit(bool),  // false = KB/s, true = MB/s
    SpeedPreset(u64), // quick-set value, in KB/s
    RememberLimit(bool),
    MaxConn(String),
    ApplyConns,
    // Completion tab form
    NotifyDone(bool),
    ExitDone(bool),
    PowerEnabled(bool),
    PowerAction(String),
    Disconnect(bool),
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
    /// Bumped on every edit of `limit_kbs`; a settle timer only applies
    /// its value if it is still the newest edit when it fires.
    limit_edit: u64,
    max_conn: String,
    /// Connection count the daemon is already running with. The Apply
    /// button only lights up while `max_conn` differs from it, so the
    /// button reads as "apply *this*" rather than as the whole tab's
    /// commit (the speed limit beside it applies live).
    applied_conn: Option<u64>,

    on_completion: OnCompletion,
    /// Which power action the picker shows, independent of whether the
    /// switch has armed it. Kept out of `on_completion.shutdown` so
    /// choosing an action can never be what turns it on.
    power_choice: ShutdownAction,
    power_force: bool,

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

/// The connection count `max_conn` currently asks for: blank (or a
/// value outside the runner's cap) means auto, i.e. `None`.
fn conn_selection(max_conn: &str) -> Option<u64> {
    max_conn
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|n| (MAX_CONN_MIN as u64..=MAX_CONN_MAX as u64).contains(n))
}

/// Push the speed limit to the daemon. The limit has no Apply of its
/// own — it takes effect the moment it changes, because unlike the
/// connection count it needs no segment reconnect to land.
fn apply_limit(st: &State) -> Task<Msg> {
    let unit = if st.limit_unit_mb {
        BYTES_PER_KB * KB_PER_MB
    } else {
        BYTES_PER_KB
    };
    let bps = if st.use_limiter {
        // A blank or half-typed field is mid-edit, not a request for
        // "unlimited": leave the running limit alone until it parses.
        match st.limit_kbs.trim().parse::<u64>() {
            Ok(v) if v > 0 => Some(v * unit),
            _ => return Task::none(),
        }
    } else {
        None
    };
    let client = st.client.clone();
    let id = st.id;
    let persist = st.remember_limit;
    Task::perform(
        async move {
            if persist {
                client.set_persistent_speed_limit(id, bps).await
            } else {
                // "Remember" off means no stored override, so clear any
                // earlier one instead of leaving it to outlive the
                // session limit we set next.
                client.set_persistent_speed_limit(id, None).await?;
                client.set_session_speed_limit(id, bps).await
            }
        },
        |_| Msg::Noop,
    )
}

/// Push the completion prefs to the daemon. Like the speed limit these
/// apply on change: they are only read when the job finishes, so there
/// is nothing to reconnect and nothing an Apply step would protect.
fn apply_completion(st: &State) -> Task<Msg> {
    let client = st.client.clone();
    let id = st.id;
    let prefs = st.on_completion.clone();
    Task::perform(
        async move { client.set_on_completion(id, prefs).await },
        |_| Msg::Noop,
    )
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
                limit_edit: 0,
                max_conn: entry
                    .job
                    .max_connections
                    .map(|n| n.to_string())
                    .unwrap_or_default(),
                applied_conn: entry.job.max_connections,
                power_choice: on_completion.shutdown.unwrap_or(POWER_DEFAULT),
                power_force: on_completion.force_shutdown,
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
            Event::JobsChanged => refetch(st.client.clone(), st.id),
            // Settings carry the palette, so a theme change from another
            // window has to land here too.
            Event::SettingsChanged => Task::batch([
                refetch(st.client.clone(), st.id),
                crate::gui::theme::refresh_tokens(
                    st.client.clone(),
                    |t| Msg::Themed(Box::new(t)),
                    Msg::Noop,
                ),
            ]),
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
            apply_limit(st)
        }
        Msg::LimitKbs(v) => {
            st.limit_kbs = v;
            st.limit_edit += 1;
            let edit = st.limit_edit;
            Task::perform(
                async move {
                    tokio::time::sleep(Duration::from_millis(LIMIT_DEBOUNCE_MS)).await;
                    edit
                },
                Msg::LimitSettled,
            )
        }
        Msg::LimitSettled(edit) => {
            if edit == st.limit_edit {
                apply_limit(st)
            } else {
                Task::none()
            }
        }
        Msg::LimitUnit(mb) => {
            st.limit_unit_mb = mb;
            apply_limit(st)
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
            // A preset *is* the finished edit — retire any settle timer
            // still pending from typing, so it cannot re-fire behind it.
            st.limit_edit += 1;
            apply_limit(st)
        }
        Msg::RememberLimit(v) => {
            st.remember_limit = v;
            apply_limit(st)
        }
        Msg::MaxConn(v) => {
            st.max_conn = v;
            Task::none()
        }
        Msg::ApplyConns => {
            let client = st.client.clone();
            let id = st.id;
            let conns = conn_selection(&st.max_conn);
            st.applied_conn = conns;
            Task::perform(
                async move { client.set_max_connections(id, conns).await },
                |_| Msg::Noop,
            )
        }
        Msg::NotifyDone(v) => {
            st.on_completion.show_dialog = v;
            apply_completion(st)
        }
        Msg::ExitDone(v) => {
            st.on_completion.exit_app = v;
            apply_completion(st)
        }
        Msg::PowerEnabled(v) => {
            // The switch is the only thing that arms the action; it
            // commits whatever the picker is showing.
            st.on_completion.shutdown = v.then_some(st.power_choice);
            st.on_completion.force_shutdown = v && st.power_force;
            apply_completion(st)
        }
        Msg::PowerAction(s) => {
            let (action, force) = parse_power_label(&s);
            st.power_choice = action;
            st.power_force = force;
            // Re-point an already-armed action, but never arm a disarmed
            // one: picking from a list is not consent to power off.
            if st.on_completion.shutdown.is_some() {
                st.on_completion.shutdown = Some(action);
                st.on_completion.force_shutdown = force;
                return apply_completion(st);
            }
            Task::none()
        }
        Msg::Disconnect(v) => {
            st.on_completion.disconnect = v;
            apply_completion(st)
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
        Msg::Themed(t) => {
            st.tokens = *t;
            Task::none()
        }
        Msg::WinResized(w, h) => {
            chrome::enforce_min_size(iced::Size::new(w, h), iced::Size::new(WIN_MIN_W, WIN_MIN_H))
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
    // Nothing is said until evaluation answers: "checking" is the
    // absence of an answer, and a subtitle that changes under the user
    // costs more than the fact is worth.
    let resum = match st.entry.counters.is_resumable {
        1 => Some("resumable"),
        -1 => Some("no resume"),
        _ => None,
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
                {
                    let mut meta = row![
                        text(host).font(theme::MONO).size(11.0).color(t.fg_3),
                        dotsep(),
                        text(st.entry.job.category.label())
                            .font(theme::BODY)
                            .size(11.0)
                            .color(t.fg_3),
                    ]
                    .spacing(6.0)
                    .align_y(Alignment::Center);
                    if let Some(resum) = resum {
                        meta = meta
                            .push(dotsep())
                            .push(text(resum).font(theme::BODY).size(11.0).color(t.fg_3));
                    }
                    meta
                },
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
        // Both gaps around the tab body ride *inside* the scroll port,
        // as leading/trailing spacers. As outer spacing/padding they
        // would be dead bands the content slides behind; scrolled with
        // the content they read as breathing room at each end.
        // Both ends use the same `S3` the tab bodies put between their
        // own cards, so scrolled-home the last card sits off the footer
        // hairline by exactly one inter-card gap.
        let tab_body: Element<'_, Msg> = column![
            iced::widget::Space::new().height(theme::space::S3),
            tab_body,
            iced::widget::Space::new().height(theme::space::S3),
        ]
        .into();

        column![
            // Tabs + hairline as one unspaced group so the active
            // underline sits on the hairline.
            sibling(column![tabs, hairline(t.border_subtle)].into()),
            crate::gui::widget::vscroll(tab_body).height(Length::Fill),
        ]
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
                // No bottom pad: the tab body already scrolls, so its
                // last row should meet the footer hairline instead of
                // leaving a dead band above it.
                .padding(iced::Padding {
                    top: theme::space::S4,
                    bottom: 0.0,
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

/// One-line "this transfer goes through a proxy" note, mirroring
/// `apply_job_proxy`'s precedence: an explicit mode wins, `Inherit`
/// falls back to the legacy per-job proxy URL, and `System` only means
/// "whatever the environment says" — no host to name, and claiming one
/// would be a guess. Credentials are never rendered.
fn proxy_note(st: &State) -> Option<Element<'_, Msg>> {
    use crate::domain::ProxyMode;
    let t = &st.tokens;
    let p = &st.entry.job.advanced.proxy;
    let text_line = match p.mode {
        ProxyMode::Http | ProxyMode::Https | ProxyMode::Socks5 if !p.host.trim().is_empty() => {
            let scheme = match p.mode {
                ProxyMode::Http => "HTTP",
                ProxyMode::Https => "HTTPS",
                _ => "SOCKS5",
            };
            let port = p.port.trim();
            let host = p.host.trim();
            if port.is_empty() {
                format!("Downloading through {scheme} proxy {host}")
            } else {
                format!("Downloading through {scheme} proxy {host}:{port}")
            }
        }
        ProxyMode::System => "Downloading through the system proxy settings".to_owned(),
        // `Inherit` (and a legacy `None`) still honour `Job.proxy`.
        _ => {
            let host = st
                .entry
                .job
                .proxy
                .as_deref()
                .and_then(|u| url::Url::parse(u).ok())
                .and_then(|u| u.host_str().map(|h| (h.to_owned(), u.port())))?;
            match host {
                (h, Some(port)) => format!("Downloading through proxy {h}:{port}"),
                (h, None) => format!("Downloading through proxy {h}"),
            }
        }
    };

    let t2 = *t;
    Some(
        container(
            row![
                icons::icon("globe", 14.0, t.fg_3),
                text(text_line).font(theme::BODY).size(11.5).color(t.fg_2),
            ]
            .spacing(theme::space::S2)
            .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        .padding([6.0, theme::space::S3])
        .style(move |_| container::Style {
            background: Some(t2.bg_sunken.into()),
            border: iced::Border {
                radius: theme::radius::XS.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into(),
    )
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

    let mut body = column![strip].spacing(theme::space::S3);
    if let Some(note) = proxy_note(st) {
        body = body.push(note);
    }
    body.push(collapsible_card(
        t,
        "Transfer rate",
        None,
        st.rate_open,
        Msg::ToggleRate,
        || rate_body.into(),
    ))
    .push(collapsible_card(
        t,
        "Segments",
        Some(segments_right.into()),
        st.segments_open,
        Msg::ToggleSegments,
        || segments_body,
    ))
    .into()
}

fn speed_tab(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let limited = st.use_limiter;

    // value field + KB/s ‖ MB/s unit-toggle. `Md` so the toggle is the
    // input's height — a shorter button beside a field reads as floating
    // rather than as part of the same control.
    let unit_toggle = segmented(
        t,
        &[("KB/s", None), ("MB/s", None)],
        if st.limit_unit_mb { 1 } else { 0 },
        BtnSize::Md,
        |i| Msg::LimitUnit(i == 1),
    );
    let value_row = row![
        // Editable even while the limiter is off — the value is the
        // limit you *would* apply, and `apply_limit` sends nothing until
        // the switch is on.
        TextInput::new(&st.limit_kbs)
            .width(Length::Fixed(LIMIT_INPUT_W))
            .on_input(Msg::LimitKbs)
            .view(t),
        unit_toggle,
    ]
    .spacing(theme::space::S2)
    .align_y(Alignment::Center);

    // Dashed quick-preset pills (design `.qp`). iced/tiny-skia can't
    // dash a border, so these read as small outlined pills. They stay
    // live while the limiter is off — pressing one *is* the request to
    // limit, and `SpeedPreset` flips the switch on.
    let mut presets = row![].spacing(theme::space::S2).align_y(Alignment::Center);
    for (label, kbs) in SPEED_PRESETS_KBS {
        presets = presets.push(
            Btn::new(*label)
                .secondary()
                .size(BtnSize::Sm)
                .on_press(Msg::SpeedPreset(*kbs))
                .view(t),
        );
    }

    // Blank `max_conn` = auto (daemon picks); a non-empty value is an
    // explicit 1–16 override. The Auto/Custom chip-toggle makes the
    // auto state visible and re-selectable. `Md` matches the stepper's
    // height so the three controls sit on one baseline.
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
        BtnSize::Md,
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
    // Apply rides in the same row as the control it commits, and stays
    // dead until that control actually differs from what the daemon is
    // running — so it can never read as the tab's global save.
    conn_controls = conn_controls.push(
        Btn::new("Apply")
            .primary()
            .size(BtnSize::Md)
            .enabled(conn_selection(&st.max_conn) != st.applied_conn)
            .on_press(Msg::ApplyConns)
            .view(t),
    );

    set_rows(
        t,
        vec![
            set_row(
                t,
                "Max parallel connections",
                Some("Auto lets oxdm choose; applying reconnects active segments."),
                conn_controls.into(),
            ),
            set_row(
                t,
                "Speed limit",
                Some("Cap this job's throughput. Takes effect as you change it."),
                toggle(t, limited, true, Msg::UseLimiter),
            ),
            set_row(t, "Limit to", None, value_row.into()),
            set_row(t, "Quick set", None, presets.into()),
            set_row(
                t,
                "Remember for this file",
                Some("Keep the limit after this session ends."),
                toggle(t, st.remember_limit, limited, Msg::RememberLimit),
            ),
        ],
    )
}

/// Windows' `shutdown /f` closes open applications without letting them
/// save. That is a property of *how* the machine goes down, so it rides
/// along with the chosen action instead of being a separate toggle.
/// `run_shutdown` ignores the flag on Linux/macOS, so the forced
/// variants are only offered where they mean something. Sleep has no
/// forced form (`shutdown /h` rejects `/f`).
const FORCE_SUFFIX: &str = " (force)";
const POWER_FORCEABLE: bool = cfg!(target_os = "windows");
/// What the picker offers before the user has chosen — the least
/// destructive of the three, so an accidental arm costs the least.
const POWER_DEFAULT: ShutdownAction = ShutdownAction::Sleep;

fn power_options() -> Vec<String> {
    let mut out = Vec::with_capacity(5);
    for base in ["Shut down", "Restart"] {
        out.push(base.to_owned());
        if POWER_FORCEABLE {
            out.push(format!("{base}{FORCE_SUFFIX}"));
        }
    }
    out.push("Sleep".to_owned());
    out
}

fn power_label(action: ShutdownAction, force: bool) -> String {
    let base = match action {
        ShutdownAction::ShutDown => "Shut down",
        ShutdownAction::Restart => "Restart",
        ShutdownAction::Sleep => "Sleep",
    };
    if force && POWER_FORCEABLE && action != ShutdownAction::Sleep {
        format!("{base}{FORCE_SUFFIX}")
    } else {
        base.to_owned()
    }
}

fn parse_power_label(label: &str) -> (ShutdownAction, bool) {
    let force = label.ends_with(FORCE_SUFFIX);
    let action = match label.trim_end_matches(FORCE_SUFFIX) {
        "Restart" => ShutdownAction::Restart,
        "Sleep" => ShutdownAction::Sleep,
        _ => ShutdownAction::ShutDown,
    };
    (action, force)
}

fn completion_tab(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let oc = &st.on_completion;
    let power_on = oc.shutdown.is_some();

    // Picker + switch share the row: the switch arms the action, the
    // picker says which one, and neither reads without the other.
    let power_controls = row![
        combo(
            t,
            power_options(),
            Some(power_label(st.power_choice, st.power_force)),
            Msg::PowerAction,
            Length::Fixed(if POWER_FORCEABLE { 176.0 } else { 140.0 }),
        ),
        toggle(t, power_on, true, Msg::PowerEnabled),
    ]
    .spacing(theme::space::S3)
    .align_y(Alignment::Center);

    let mut rows = vec![
        set_row(
            t,
            "Show notification when done",
            Some("Open the completion dialog instead of acting unattended."),
            toggle(t, oc.show_dialog, true, Msg::NotifyDone),
        ),
        set_row(
            t,
            "Exit oxdm when done",
            None,
            toggle(t, oc.exit_app, true, Msg::ExitDone),
        ),
        set_row(
            t,
            "Power action",
            Some("Runs after a 60-second cancellable countdown."),
            power_controls.into(),
        ),
    ];
    // The warning sits directly under the power row, inside the group:
    // it is the consequence of what was just armed, so it reads as part
    // of that control rather than as a banner over the whole pane.
    if let Some(warn) = completion_warn(st) {
        rows.push(set_row_panel(warn));
    }
    rows.push(set_row(
        t,
        "Disconnect from network when done",
        Some("Superseded by a power action when one is set."),
        toggle(t, oc.disconnect, !power_on, Msg::Disconnect),
    ));

    set_rows(t, rows)
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
    if oc.force_shutdown && oc.shutdown.is_some() {
        items.push("Open apps will be closed without saving.");
    }
    // Mirrors the daemon's precedence: a power action supersedes the
    // disconnect, so don't promise something that won't run.
    if oc.disconnect && oc.shutdown.is_none() {
        items.push(
            "Your network connection will be turned off — other running transfers will fail.",
        );
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
            iced::Size::new(WIN_W, WIN_MIN_H),
            iced::Size::new(WIN_MIN_W, WIN_MIN_H),
        ));
    for f in theme::fonts::ALL {
        app = app.font(*f);
    }
    if let Err(e) = app.run() {
        eprintln!("gui error: {e}");
        std::process::exit(1);
    }
}

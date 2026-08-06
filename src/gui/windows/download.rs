//! Per-job download window (`oxdm gui download <id>`): header card
//! with 56px tile + % readout, striped progress, Info / Speed /
//! On Completion tabs, transfer-rate chart, segments table, footer —
//! and the "Download complete" view once the job finishes.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use iced::widget::{column, container, row, stack, text};
use iced::{Alignment, Element, Length, Subscription, Task};

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
use crate::gui::widget::striped::striped_progress_hatched;
use crate::gui::widget::{
    Btn, BtnSize, RateChart, TabBtn, TextInput, card, collapsible_card, combo, hairline,
    number_stepper, pill_progress, rate_chart, segmented, set_row, set_row_panel, set_rows,
    sibling, status_dot, surface, toggle, vdivider,
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
/// Launch height for the completion view: hero burst and its title, the
/// file card, the saved-to and address rows, and the actions. Fixed
/// content, so one measured number covers it.
const WIN_COMPLETE_H: f32 = 336.0;
/// What each optional block above the footer adds. The completion view
/// is a fixed page except for these, and a single "tampered" constant
/// was wrong for every job that did not have all of them — a mismatch
/// with no saved hash left a screenful of empty surface. All four
/// measured off the rendered page.
const TAMPER_BANNER_H: f32 = 130.0;
const DIGEST_PANEL_H: f32 = 86.0;
const INTEGRITY_BOX_H: f32 = 180.0;
/// The stats strip, which a job with no finish time does not draw.
const STATS_H: f32 = 74.0;
/// Everything the error view puts around the error card: title bar,
/// hero, progress bar, the gaps between them and the footer. The card
/// itself is measured from its own copy — see `error_block_height`.
const WIN_ERROR_CHROME_H: f32 = 206.0;

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

/// The "don't open this" warning (design `.tamper-banner`): an
/// alert-octagon, a one-line verdict, and the paragraph explaining what
/// a hash mismatch can mean. A 3px rust rule runs down the left edge —
/// iced borders are uniform, so it is a filled strip inside the frame
/// rather than a border-left.
fn tamper_banner<'a>(t: &Tokens) -> Element<'a, Msg> {
    let t2 = *t;
    let bold = |s: &'static str| {
        iced::widget::span(s)
            .font(theme::BODY_BOLD)
            .color(color::rust::R300)
    };
    let plain = |s: &'static str| iced::widget::span(s);
    let body = column![
        text("This file doesn't match its expected checksum.")
            .font(theme::BODY_BOLD)
            .size(TAMPER_TITLE_SIZE)
            .color(t.fg_1),
        iced::widget::rich_text::<(), Msg, _, _>([
            plain(
                "The download finished, but the file's hash differs from what the publisher \
                   signed. This can mean the file is "
            ),
            bold("corrupted in transit"),
            plain(", the source has been "),
            bold("compromised"),
            plain(", or the connection was "),
            bold("intercepted"),
            plain(
                ". Don't open or run this file until you've re-downloaded it from a trusted \
                  source."
            ),
        ])
        .font(theme::BODY)
        .size(TAMPER_TEXT_SIZE)
        .line_height(iced::widget::text::LineHeight::Relative(1.55))
        .color(t.fg_2),
    ]
    .spacing(3.0);

    // The rule takes the frame's corner radius on its own left side,
    // otherwise its square corners poke out past the rounded frame.
    let rule = container(iced::widget::Space::new())
        .width(Length::Fixed(TAMPER_RULE_W))
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: Some(color::rust::R300.into()),
            border: iced::Border {
                radius: iced::border::Radius {
                    top_left: theme::surface::RADIUS,
                    bottom_left: theme::surface::RADIUS,
                    top_right: 0.0,
                    bottom_right: 0.0,
                },
                ..Default::default()
            },
            ..Default::default()
        });

    container(
        row![
            rule,
            container(
                row![
                    icons::icon("octagon-alert", TAMPER_ICON, color::rust::R300),
                    body,
                ]
                .spacing(theme::space::S3)
                .align_y(Alignment::Start),
            )
            .padding([theme::space::S3, TAMPER_PAD_X]),
        ]
        .align_y(Alignment::Start),
    )
    .width(Length::Fill)
    .style(move |_| container::Style {
        background: Some(t2.status_danger_bg.into()),
        border: iced::Border {
            color: color::rust::R100,
            width: 1.0,
            radius: theme::surface::RADIUS.into(),
        },
        ..Default::default()
    })
    .into()
}

/// Completed-view file card (design `.complete-file`): a 40px ext tile
/// beside the name and size.
const FILE_TILE: f32 = 40.0;
const FILE_TILE_RADIUS: f32 = 7.0;
const FILE_EXT_SIZE: f32 = 10.0;
const FILE_NAME_SIZE: f32 = 13.5;
const FILE_META_SIZE: f32 = 11.0;
/// `.tamper-banner` — 12/14 padding behind a 3px left rule, a 16px
/// mark, and two sizes of copy.
const TAMPER_RULE_W: f32 = 3.0;
const TAMPER_PAD_X: f32 = 14.0;
const TAMPER_ICON: f32 = 16.0;
const TAMPER_TITLE_SIZE: f32 = 12.5;
const TAMPER_TEXT_SIZE: f32 = 11.5;

/// `.cf-state` — the outcome chip on the file card, and the 3px dot
/// separating it from the size.
const FILE_STATE_SIZE: f32 = 11.5;
const FILE_STATE_ICON: f32 = 12.0;
const FILE_DOT: f32 = 3.0;

/// Completion stat cells (design `.complete-stats`): an eyebrow label
/// over a mono value, centered, with the interruption note under the
/// last one.
const STAT_CELL_PAD_Y: f32 = 8.0;
const STAT_CELL_PAD_X: f32 = 10.0;
const STAT_LABEL_SIZE: f32 = 9.5;
const STAT_VALUE_SIZE: f32 = 13.0;
const STAT_SUB_ICON: f32 = 10.0;
/// The cell's own padding twice, plus the label, value and sub-line
/// boxes. Pinned so `vdivider` has a height to draw against.
const STAT_CELL_H: f32 = STAT_CELL_PAD_Y * 2.0 + 13.0 + 18.0 + 15.0;

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
    /// Failure recovery (design §3.3): discard the partial file and
    /// fetch from byte 0. The only way forward when the server refuses
    /// to resume or the remote file changed.
    RestartFromZero,
    /// Failure recovery for a write fault: pick a new destination, then
    /// retry. The bytes already downloaded carry over.
    Open,
    OpenFolder,
    /// Delete the saved file, via the confirmation.
    DeleteAsk,
    DeleteCancel,
    DeleteConfirm,
    CloseWin,
    MinimizeTray,
    // Completed view — copy / reveal / checksum verify
    Copy(String),
    Reveal(PathBuf),
    CsPaste(String),
    // Local checksum compute (hash `final_path` off the UI executor).
    CsCompute,
    CsComputed(Result<String, String>),
    WinResized(f32, f32),
    WinFocused(bool),
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

    tab: Tab,
    /// The "delete the saved file?" confirmation is up.
    confirm_delete: bool,
    /// Live window width, so a height correction can leave it alone.
    win_w: f32,
    /// Height this window last imposed on itself. Kept so the
    /// correction fires once per state change rather than on every
    /// event — otherwise a user resizing a completed window would be
    /// snapped back by the next counter tick.
    imposed_h: Option<f32>,
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
    /// Caption for both the painted titlebar and the OS/taskbar title:
    /// what the download is, then how it is doing. The URL stands in
    /// until evaluation resolves a filename.
    fn window_title(&self) -> String {
        let job = &self.entry.job;
        let name = job.filename.as_deref().unwrap_or(job.url.as_str());
        format!("{name} — {}", self.phase().label())
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
                tab: Tab::Info,
                confirm_delete: false,
                win_w: WIN_W,
                imposed_h: LAUNCH_H.get().copied(),
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
            let App::Ready(st) = app else {
                return Task::none();
            };
            fit_window(st)
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

/// Height this window should open at, when the state it opens in is a
/// single page rather than the scrolling transfer view. `None` = keep
/// the launch size. Only the *launch* size: `WIN_MIN_H` is untouched,
/// so the user can still shrink the window afterwards.
fn launch_height(st: &State) -> Option<f32> {
    let h = job_height(&st.entry.job)?;
    // A digest the *window* computed can turn a healthy page into a
    // tampered one, and no job snapshot can know about it. That adds
    // the banner; the box was already counted, and the expected-vs-got
    // panel needs the runner's error, which this case does not have.
    Some(if is_tampered(st) && !job_tampered(&st.entry.job) {
        h + TAMPER_BANNER_H
    } else {
        h
    })
}

/// Does the job itself report a failed integrity check?
fn job_tampered(job: &crate::domain::Job) -> bool {
    matches!(job.status.error, Some(JobError::ChecksumMismatch { .. }))
        || job.checksums.iter().any(|c| c.status == CsStatus::Mismatch)
}

/// The height a window showing `job` wants, from the job alone — so the
/// launcher can size the window before it exists rather than resizing it
/// afterwards. `None` = the transfer view, which asks for nothing.
fn job_height(job: &crate::domain::Job) -> Option<f32> {
    let tampered = job_tampered(job);
    if job.status.phase == Phase::Completed || tampered {
        let mut h = WIN_COMPLETE_H;
        if tampered {
            h += TAMPER_BANNER_H;
        }
        // The expected-vs-got panel only has something to say when the
        // run itself reported the mismatch.
        if matches!(job.status.error, Some(JobError::ChecksumMismatch { .. })) {
            h += DIGEST_PANEL_H;
        }
        if !job.checksums.is_empty() {
            h += INTEGRITY_BOX_H;
        }
        // No finish time, no stats strip — `completion_stats` renders
        // nothing rather than a row of dashes.
        if job.finished_at.is_none() {
            h -= STATS_H;
        }
        return Some(h);
    }
    let err = job.status.error.as_ref()?;
    let wanted = WIN_ERROR_CHROME_H + crate::gui::widget::error_panel::error_block_height(err);
    // Never below the floor: a two-line error must not open a window
    // smaller than the user can resize it to.
    Some(wanted.max(WIN_MIN_H))
}

/// Resize to the height the current state wants, if that is not the
/// height we last asked for. Width is left alone — the user owns it.
///
/// The completion and error views are single pages with a known height;
/// a download that finishes while its window is open would otherwise
/// keep whatever height the transfer view had, which is either a lot of
/// empty surface or a page the user has to scroll. The transfer view
/// itself asks for nothing, so a window the user has resized stays
/// resized for as long as it is one.
fn fit_window(st: &mut State) -> Task<Msg> {
    let Some(h) = launch_height(st) else {
        // Back to a free-form view: forget what we imposed, so the next
        // time one of the fixed pages comes up it is applied afresh,
        // and give the transfer view its floor back.
        if st.imposed_h.take().is_some() {
            let min = iced::Size::new(WIN_MIN_W, WIN_MIN_H);
            return iced::window::latest()
                .and_then(move |id| iced::window::set_min_size(id, Some(min)));
        }
        return Task::none();
    };
    if st.imposed_h == Some(h) {
        return Task::none();
    }
    st.imposed_h = Some(h);
    resize_to(st.win_w, h)
}

fn resize_to<M: Send + 'static>(w: f32, h: f32) -> Task<M> {
    let size = iced::Size::new(w, h);
    // The minimum has to come down with the height: the completion page
    // is shorter than the transfer view's floor, and a resize clamped by
    // a stale minimum leaves a band of empty surface the window can
    // never lose.
    let min = iced::Size::new(WIN_MIN_W, WIN_MIN_H.min(h));
    iced::window::latest().and_then(move |id| {
        Task::batch([
            iced::window::set_min_size(id, Some(min)),
            iced::window::resize(id, size),
        ])
    })
}

fn update_ready(st: &mut State, msg: Msg) -> Task<Msg> {
    let task = update_state(st, msg);
    // Any message can be the one that completes the download or lands
    // the error, so the check rides along with all of them rather than
    // being duplicated across the handful that mutate the entry.
    Task::batch([task, fit_window(st)])
}

fn update_state(st: &mut State, msg: Msg) -> Task<Msg> {
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
            // The failure path fires `JobFailed` and no `JobsChanged`,
            // so without this the window an already-focused user is
            // looking at would keep showing the transfer view.
            Event::JobFailed { id, .. } if id == st.id => refetch(st.client.clone(), st.id),
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
        Msg::RestartFromZero => {
            let client = st.client.clone();
            let id = st.id;
            Task::perform(async move { client.restart_job(id).await }, |_| Msg::Noop)
        }
        Msg::Open => {
            let path = final_path(&st.entry);
            crate::platform::open_path(&path);
            Task::none()
        }
        Msg::DeleteAsk => {
            st.confirm_delete = true;
            Task::none()
        }
        Msg::DeleteCancel => {
            st.confirm_delete = false;
            Task::none()
        }
        Msg::DeleteConfirm => {
            st.confirm_delete = false;
            let client = st.client.clone();
            let id = st.id;
            // Nothing left on this page to act on once the file is
            // gone, so the window goes with it. The job keeps its row
            // in the list.
            Task::perform(async move { client.delete_final_file(id).await }, |_| {
                Msg::CloseWin
            })
        }
        Msg::OpenFolder => {
            crate::platform::open_path(&st.entry.job.save_dir);
            Task::none()
        }
        Msg::CloseWin => iced::exit(),
        Msg::MinimizeTray => iced::window::latest().and_then(|id| iced::window::minimize(id, true)),
        Msg::Themed(t) => {
            st.tokens = *t;
            Task::none()
        }
        Msg::WinFocused(focused) => {
            let client = st.client.clone();
            Task::perform(async move { client.window_focused(focused).await }, |_| {
                Msg::Noop
            })
        }
        Msg::WinResized(w, h) => {
            st.win_w = w;
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
            // The daemon reopens this window when the job fails; it
            // only needs to when the user isn't already looking at it.
            iced::Event::Window(iced::window::Event::Focused) => Some(Msg::WinFocused(true)),
            iced::Event::Window(iced::window::Event::Unfocused) => Some(Msg::WinFocused(false)),
            // Escape backs out of the delete confirmation. Enter is
            // deliberately NOT wired to confirm: a stray keypress must
            // not be what deletes a file.
            iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                ..
            }) => Some(Msg::DeleteCancel),
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
    }
    Subscription::batch(subs)
}

// ---------------------------------------------------------------- view

pub fn view(app: &App) -> Element<'_, Msg> {
    chrome::framed(match app {
        App::Connecting => splash("Connecting…".to_owned()),
        App::Failed(e) => splash(e.clone()),
        App::Ready(st) => {
            let page = if shows_complete(st) {
                complete_view(st)
            } else {
                running_view(st)
            };
            // Always the same shape, overlay or not. Swapping the root
            // between `page` and `stack![page, …]` rebuilds the subtree
            // under a different parent, and the scrollable loses its
            // state with it — the page would jump back to the top the
            // moment the confirmation opened.
            delete_overlay(st, page)
        }
    })
}

/// The completed view also owns the failed-integrity case: the bytes
/// did arrive and the file is on disk, so the user needs the completed
/// page's answers — which hash was expected, what landed, where the
/// file is — not a transfer error card that hides all of it.
fn shows_complete(st: &State) -> bool {
    st.phase() == Phase::Completed || checksum_failure(st).is_some()
}

/// The expected/actual pair from a verification failure, if that is why
/// this job failed.
fn checksum_failure(st: &State) -> Option<(&str, &str)> {
    match st.entry.job.status.error.as_ref()? {
        JobError::ChecksumMismatch { expected, actual } => {
            Some((expected.as_str(), actual.as_str()))
        }
        _ => None,
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
    // Design §3.3 gives the bar three interrupted looks:
    //   is-reconnecting → ochre, still trying (below, a running phase)
    //   is-errored      → rust on a rust-tinted track, frozen
    //   is-will-restart → dimmed rust struck through: the bytes under
    //                     the bar are going to be thrown away
    let error = st.entry.job.status.error.clone();
    let restart_required = matches!(
        error,
        Some(crate::domain::JobError::FileChanged(_) | crate::domain::JobError::NotResumable(_))
    );
    let hatch = restart_required.then_some(color::rust::R300);
    let (track, fill, gradient) = match phase {
        Phase::Failed if restart_required => (
            color::with_alpha(t.status_danger_bg, 0.55),
            color::with_alpha(color::rust::R200, 0.55),
            None,
        ),
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

    // A severe error replaces the tabs + pane entirely (design §3.3
    // "Severe error"): friendly title → detail → what-to-check → quiet
    // code footer, driven only by the real `JobStatus.error` field.
    let lower: Element<'_, Msg> = if let Some(err) = &error {
        let report = crate::gui::widget::error_panel::error_report(err);
        let block = crate::gui::widget::error_panel::error_recovery_block(
            &st.tokens,
            err,
            Msg::Copy(report.clone()),
        )
        .unwrap_or_else(|| error_block(&st.tokens, err, Msg::Copy(report)));
        crate::gui::widget::vscroll(block)
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
    if let Some(b) = nonresume_banner(st) {
        hero = hero.push(b);
    }
    hero = hero
        .push(sibling(striped_progress_hatched(
            st.frac(),
            Length::Fill,
            10.0,
            track,
            fill,
            gradient,
            striped,
            st.anim_t,
            hatch,
        )))
        .push(lower);

    page(
        t,
        column![
            titlebar::titlebar(t, &st.window_title(), false, Msg::Window),
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
    let restart = |label: &'a str, primary: bool| {
        let btn = Btn::new(label).icon("rotate-ccw");
        let btn = if primary {
            btn.primary()
        } else {
            btn.toolbar()
        };
        btn.on_press(Msg::RestartFromZero).view(t)
    };
    let group = match err {
        // The server will not continue from the bytes on disk: retry in
        // case it was transient, or discard them and start over.
        JobError::NotResumable(_) => row![restart("Restart from 0", false), retry(), cancel],
        // Continuing would splice two different files, so retrying is
        // not on offer — only starting over, or giving up.
        JobError::FileChanged(_) => row![restart("Restart from 0", true), cancel],
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
            false,
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
            "Show completion dialog when done",
            None,
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
        items
            .push("Your network connection will be turned off. Other running transfers will fail.");
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
    // Design `.ext-big` outlines the tile in clay-200 — a step up from
    // its own fill, so the tile keeps an edge on a surface that is
    // nearly the same value. Mixed from the accent rather than hardcoded
    // so the danger variant's rust tile gets a rust edge.
    let tile_border = color::mix(t.bg_surface, accent, 0.45);
    let t2 = *t;
    // Design `.complete-file .ext-big` overrides the 44px detected-card
    // tile with a 40px one: this card names a file that is already on
    // disk, so the extension is a label, not the headline.
    let tile = container(
        text(ext)
            .font(theme::MONO_BOLD)
            .size(FILE_EXT_SIZE)
            .color(accent),
    )
    .width(Length::Fixed(FILE_TILE))
    .height(Length::Fixed(FILE_TILE))
    .align_x(Alignment::Center)
    .align_y(Alignment::Center)
    .style(move |_| container::Style {
        background: Some(tile_bg.into()),
        border: iced::Border {
            color: tile_border,
            width: 1.0,
            radius: FILE_TILE_RADIUS.into(),
        },
        ..Default::default()
    });

    // The verdict belongs to the burst, the file card to the file
    // (design §3.3 `.complete-file` = ext tile + name + size): putting
    // the outcome in the card's title slot left the filename with
    // nowhere to go, and the user reads this page to find their file.
    //
    // The outcome rides on the card too (design `.cf-state`): one
    // object, one state, rather than a hero above it repeating what the
    // card is about. Failure recolors the whole card.
    let state_row = row![
        row![
            icons::icon(
                if tampered { "shield-alert" } else { "check" },
                FILE_STATE_ICON,
                accent,
            ),
            text(if tampered {
                "Integrity check failed"
            } else {
                "Download complete"
            })
            .font(theme::BODY_BOLD)
            .size(FILE_STATE_SIZE)
            .color(accent),
        ]
        .spacing(theme::space::S1)
        .align_y(Alignment::Center),
        crate::gui::widget::dot(FILE_DOT, t.fg_4),
        text(format_bytes_2(total))
            .font(theme::MONO)
            .size(FILE_META_SIZE)
            .color(t.fg_3),
    ]
    .spacing(7.0)
    .align_y(Alignment::Center);

    let card = set_row_panel(
        row![
            tile,
            column![
                text(name.clone())
                    .font(theme::BODY_BOLD)
                    .size(FILE_NAME_SIZE)
                    .color(t.fg_1),
                state_row,
            ]
            .spacing(4.0),
        ]
        .spacing(theme::space::S3)
        .align_y(Alignment::Center)
        .into(),
    );
    // `.is-bad` swaps the card's edge for rust and tints its fill: the
    // whole object reads as the problem, not one line inside it.
    let header = if tampered {
        surface(
            color::mix(t.bg_surface, accent, 0.08),
            color::rust::R300,
            0.0,
            card,
        )
    } else {
        set_rows(t, vec![card])
    };

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
        label("URL"),
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

    let mut body = column![header].spacing(theme::space::S3);
    // Tampered files get a heavy "don't open" warning right under the
    // header (design `.tamper-banner`).
    if tampered {
        body = body.push(tamper_banner(t));
    }
    if let Some(panel) = failed_digest_panel(st) {
        body = body.push(panel);
    }
    if let Some(stats) = completion_stats(st) {
        body = body.push(stats);
    }
    body = body.push(from_row).push(saved_row);
    // Healthy: the integrity box is optional tooling, so it sits above
    // the actions. Failed: the box is a tall verification workbench and
    // the page already said what went wrong — leaving it above would
    // push the close/reveal actions off the bottom of the window.
    let mut cs_box = checksum_box(st);
    if !tampered && let Some(cs_box) = cs_box.take() {
        body = body.push(cs_box);
    }
    if let Some(cs_box) = cs_box {
        body = body.push(cs_box);
    }

    // Actions live in the footer, the same band the transfer view uses,
    // so the window's controls are always in the same place. Tampered
    // offers no way to open the file: the banner above says not to, and
    // an "open anyway" next to it would be the app arguing with itself.
    let footer_el = if tampered {
        footer(
            t,
            Btn::new("Delete tampered file")
                .danger()
                .icon("trash-2")
                .on_press(Msg::DeleteAsk)
                .view(t),
            Btn::new("Keep anyway")
                .ghost()
                .on_press(Msg::CloseWin)
                .view(t),
        )
    } else {
        footer(
            t,
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
            ]
            .spacing(theme::space::S2)
            .align_y(Alignment::Center)
            .into(),
            Btn::new("Close")
                .toolbar()
                .icon("x")
                .on_press(Msg::CloseWin)
                .view(t),
        )
    };

    page(
        t,
        column![
            titlebar::titlebar(t, &st.window_title(), false, Msg::Window),
            // Design `.complete-body` opens with 16px. It pads inside
            // the scroll region rather than around it, so the first row
            // scrolls with the rest instead of sitting in a fixed band.
            container(
                crate::gui::widget::vscroll(
                    container(body).padding(iced::Padding::default().top(theme::space::S4)),
                )
                .height(Length::Fill),
            )
            .padding(iced::Padding {
                top: 0.0,
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

/// "Delete the saved file?" confirmation (pattern: queues
/// `delete_overlay`). Names the file and says what survives, since the
/// download keeps its row in the list either way.
///
/// Always wraps, and adds the scrim + card only while confirming, so
/// the page underneath keeps its widget state — scroll position first
/// among them.
///
/// Escape backs out. Nothing confirms on Enter — see the subscription.
fn delete_overlay<'a>(st: &'a State, base: Element<'a, Msg>) -> Element<'a, Msg> {
    if !st.confirm_delete {
        return stack![base].into();
    }
    let t = &st.tokens;
    let t2 = *t;
    let name = st
        .entry
        .job
        .filename
        .clone()
        .unwrap_or_else(|| "This file".to_owned());
    let card = container(
        column![
            row![
                icons::icon("trash-2", 20.0, t.status_danger),
                text("Delete this file?")
                    .font(theme::BODY_BOLD)
                    .size(14.0)
                    .color(t.fg_1),
            ]
            .spacing(theme::space::S2)
            .align_y(Alignment::Center),
            text(format!(
                "{name} is permanently deleted from your disk. This download keeps its \
                 place in the list, so you can fetch it again from the same address."
            ))
            .font(theme::BODY)
            .size(12.0)
            .color(t.fg_2)
            .line_height(iced::widget::text::LineHeight::Relative(1.4)),
            row![
                iced::widget::Space::new().width(Length::Fill),
                Btn::new("Cancel")
                    .ghost()
                    .on_press(Msg::DeleteCancel)
                    .view(t),
                Btn::new("Delete file")
                    .danger_filled()
                    .icon("trash-2")
                    .on_press(Msg::DeleteConfirm)
                    .view(t),
            ]
            .spacing(theme::space::S2)
            .align_y(Alignment::Center),
        ]
        .spacing(theme::space::S3),
    )
    .width(Length::Fixed(400.0))
    .padding(theme::space::S4)
    .style(move |_| container::Style {
        background: Some(t2.bg_surface.into()),
        border: iced::Border {
            color: t2.border_default,
            width: 1.0,
            radius: theme::surface::RADIUS.into(),
        },
        shadow: iced::Shadow {
            color: color::with_alpha(iced::Color::BLACK, 80.0 / 255.0),
            offset: iced::Vector::new(0.0, 4.0),
            blur_radius: 16.0,
        },
        ..Default::default()
    });

    let scrim = iced::widget::opaque(
        iced::widget::mouse_area(
            container(iced::widget::Space::new())
                .width(Length::Fill)
                .height(Length::Fill)
                .style(|_| container::Style {
                    background: Some(color::with_alpha(iced::Color::BLACK, 120.0 / 255.0).into()),
                    ..Default::default()
                }),
        )
        .on_press(Msg::DeleteCancel),
    );

    stack![
        base,
        scrim,
        container(iced::widget::opaque(card))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center),
    ]
    .into()
}

/// Expected-vs-got panel for a verification failure, naming the digest
/// that did not match. The algorithm comes from the saved checksum that
/// carries the expected hash; failing that, from the hash's own length,
/// so the panel still names something real when the job holds no saved
/// checksum row.
fn failed_digest_panel(st: &State) -> Option<Element<'_, Msg>> {
    let (expected, actual) = checksum_failure(st)?;
    let algo = st
        .entry
        .job
        .checksums
        .iter()
        .find(|c| c.hash.eq_ignore_ascii_case(expected))
        .map(|c| c.algo)
        .or_else(|| {
            Algo::ALL
                .iter()
                .copied()
                .find(|a| a.hex_len() == expected.len())
        });
    Some(hash_mismatch(
        &st.tokens,
        algo.map(|a| a.label()).unwrap_or("checksum"),
        &expected.to_lowercase(),
        &actual.to_lowercase(),
    ))
}

/// Completed-view ChecksumBox (design §3.4): shows the job's saved
/// checksum + status, a paste field to verify against the publisher's
/// hash, AND a local "Compute from file" action that hashes the saved
/// file (off the UI executor) and compares. Algorithm is auto-detected
/// from hex length for paste; compute uses the saved checksum's algo.
fn checksum_box(st: &State) -> Option<Element<'_, Msg>> {
    let t = &st.tokens;
    let cs = st.entry.job.checksums.first()?;

    // A run that failed verification is a mismatch even when the stored
    // row has not been restamped yet — the box must not read
    // "unverified" under a page titled "Integrity check failed".
    let failed_here = checksum_failure(st).is_some_and(|(e, _)| cs.hash.eq_ignore_ascii_case(e));
    let (status_color, status_label) = match cs.status {
        _ if failed_here => (t.status_danger, "mismatch"),
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

/// Ochre "no resume" banner (design `.nonresume-banner`), shown once
/// evaluation reports the server will not continue a transfer. Resuming
/// is the assumed default, so nothing is said in the normal case and
/// nothing is said while the answer is still unknown — this banner is
/// the only place the fact appears.
fn nonresume_banner(st: &State) -> Option<Element<'_, Msg>> {
    (st.entry.counters.is_resumable == -1).then(|| {
        banner(
            &st.tokens,
            st.tokens.status_warning,
            st.tokens.status_warning_bg,
            "plug-zap",
            "Single connection · no resume — pausing or losing the connection restarts \
             this download from the beginning."
                .to_owned(),
        )
    })
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
    if checksum_failure(st).is_some() {
        return true;
    }
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

/// Completion stat grid (design `.complete-stats`): Avg speed · Time
/// taken · Finished at, computed from `started_at`/`finished_at`. W3:
/// any cell whose source timestamp is `None` is HIDDEN (no `created_at`
/// fallback); never divides by zero. A "retried N times" sub-line shows
/// only when `job.retries > 0`. Returns `None` when nothing is showable.
fn completion_stats(st: &State) -> Option<Element<'_, Msg>> {
    let t = &st.tokens;
    let job = &st.entry.job;
    let downloaded = st.entry.counters.downloaded;

    // Every cell renders, dash where the fact is missing (design shows
    // "—" for both timing cells): three columns that come and go would
    // move the interruption line around under the user.
    let (avg, taken) = match (job.started_at, job.finished_at) {
        (Some(started), Some(finished)) => {
            let secs = (finished - started).num_seconds().max(0) as u64;
            let avg = (secs > 0).then(|| format_speed(downloaded as f64 / secs as f64));
            (avg, Some(format_eta(secs)))
        }
        _ => (None, None),
    };
    let finished = job.finished_at.map(|f| {
        f.with_timezone(&chrono::Local)
            .format("%-I:%M %p")
            .to_string()
    });
    if avg.is_none() && taken.is_none() && finished.is_none() {
        return None;
    }

    let n = job.interruptions;
    let (icon, tint, note) = if n == 0 {
        ("check", t.action_primary, "No interruptions".to_owned())
    } else {
        (
            "rotate-ccw",
            t.fg_3,
            format!("{n} interruption{}", if n == 1 { "" } else { "s" }),
        )
    };
    let sub = row![
        icons::icon(icon, STAT_SUB_ICON, tint),
        text(note).font(theme::BODY_MEDIUM).size(10.0).color(t.fg_3),
    ]
    .spacing(theme::space::S1)
    .align_y(Alignment::Center);

    // Same material as the About window's build facts: settings-row
    // surface, cells split by a vertical hairline (design
    // `.complete-stats`). The row is pinned because `vdivider` needs a
    // concrete height.
    let grid: Element<'_, Msg> = row![
        stat_cell(t, "average speed", avg, None),
        vdivider(t.border_subtle, STAT_CELL_H),
        stat_cell(t, "time taken", taken, None),
        vdivider(t.border_subtle, STAT_CELL_H),
        stat_cell(t, "finished at", finished, Some(sub.into())),
    ]
    .height(Length::Fixed(STAT_CELL_H))
    .into();

    Some(set_rows(t, vec![grid]))
}

/// One `.cs-cell`: eyebrow label over a mono value, centered, with an
/// optional sub-line under it. `None` prints the design's em dash — the
/// fact is unknown, not zero.
fn stat_cell<'a>(
    t: &Tokens,
    label: &'a str,
    value: Option<String>,
    sub: Option<Element<'a, Msg>>,
) -> Element<'a, Msg> {
    let mut col = column![
        text(label.to_uppercase())
            .font(theme::BODY_BOLD)
            .size(STAT_LABEL_SIZE)
            .color(t.fg_3),
        text(value.unwrap_or_else(|| "—".to_owned()))
            .font(theme::MONO_BOLD)
            .size(STAT_VALUE_SIZE)
            .color(t.fg_1),
    ]
    .spacing(3.0)
    .align_x(Alignment::Center);
    if let Some(sub) = sub {
        col = col.push(sub);
    }
    // Top-aligned, not centered: the last cell carries a sub-line the
    // others don't, and centering each cell in the row would drop the
    // first two labels below the third.
    container(col)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Start)
        .padding([STAT_CELL_PAD_Y, STAT_CELL_PAD_X])
        .into()
}

/// The height this window should be *created* at. Asking the daemon
/// before the window exists is the only deterministic way to get it:
/// resizing a window that has just been mapped is a request the
/// compositor is free to drop, with no event back to say it did — which
/// is exactly how the completion view kept arriving at the transfer
/// view's height. A window born the right size never has to be
/// corrected.
///
/// Blocking is fine here: nothing is on screen yet, and iced has not
/// started. A daemon that does not answer inside the timeout leaves the
/// default, and `boot`'s own connect reports the real failure.
fn launch_size(id: JobId) -> f32 {
    /// Long enough for a local socket round-trip, short enough that a
    /// wedged daemon does not hold up the window.
    const PREFLIGHT: Duration = Duration::from_millis(600);

    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return WIN_MIN_H;
    };
    rt.block_on(async {
        let query = async {
            let client = Client::connect().await.ok()?;
            client.job_entry(id).await.ok().flatten()
        };
        tokio::time::timeout(PREFLIGHT, query)
            .await
            .ok()
            .flatten()
            .and_then(|entry| job_height(&entry.job))
            .unwrap_or(WIN_MIN_H)
    })
}

/// The height the window was created at, so `boot` can seed
/// `imposed_h` and skip a resize that would ask for the size the window
/// already has.
static LAUNCH_H: std::sync::OnceLock<f32> = std::sync::OnceLock::new();

pub fn launch_download(id: JobId) {
    let height = launch_size(id);
    let _ = LAUNCH_H.set(height);
    let mut app = iced::application(boot, update, view)
        .title(|app: &App| match app {
            // Taskbar/switcher entry: identity first, then the phase,
            // which is the one thing the window body shows but the
            // title bar doesn't. The URL stands in until evaluation
            // resolves a filename.
            App::Ready(st) => {
                let job = &st.entry.job;
                let name = job.filename.as_deref().unwrap_or(job.url.as_str());
                format!("{name} — {}", st.phase().label())
            }
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
            iced::Size::new(WIN_W, height),
            // The completion page is shorter than the transfer view's
            // floor; keeping that floor would open it with a band of
            // empty surface it can never lose.
            iced::Size::new(WIN_MIN_W, WIN_MIN_H.min(height)),
        ));
    for f in theme::fonts::ALL {
        app = app.font(*f);
    }
    if let Err(e) = app.run() {
        eprintln!("gui error: {e}");
        std::process::exit(1);
    }
}

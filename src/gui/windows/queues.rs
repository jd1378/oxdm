//! Queues & scheduling window (`oxdm gui queues`): queue list on the
//! left, editor (name + color, concurrency presets, schedule, on-
//! finish hooks) on the right, Cancel / Save footer, delete-confirm
//! overlay.

use std::sync::Arc;
use std::time::Duration;

use iced::widget::{button, column, container, mouse_area, row, scrollable, text};
use iced::{Alignment, Element, Length, Subscription, Task};

use crate::domain::{
    CMD_INTERVAL_RANGE, CondCombine, CondCommand, CondKind, CondSet, IDLE_MINUTES_RANGE, Queue,
    QueueHook, QueueId, QueueSchedule, ShutdownAction, WeekDayMask,
};
use crate::gui::chrome::{self, WindowControl, titlebar};
use crate::gui::color;
use crate::gui::icons;
use crate::gui::ipc::DaemonSignal;
use crate::gui::shot::Shot;
use crate::gui::theme::{self, Tokens};
use crate::gui::widget::{Btn, TextInput, hairline, number_stepper, section_card, toggle};
use crate::ipc_local::Client;
use crate::ipc_local::protocol::Event;

/// Left queue-list column width (design Queues grid `[220px list] [1fr]`).
const LIST_W: f32 = 220.0;
/// Day-grid toggle square (design `.day-grid .d` ~28px square).
const DAY_SQUARE: f32 = 28.0;
/// Concurrency preset pill text + x-padding — matches a secondary
/// `Md` button so the pills sit flush with the custom stepper.
const CONC_PILL_FONT: f32 = 13.0;
const CONC_PILL_PAD_X: f32 = 14.0;
/// Custom-concurrency stepper bounds. Min 1 keeps at least one active
/// download; no design max, so cap at a sane parallelism ceiling.
const CONC_MIN: i64 = 1;
const CONC_MAX: i64 = 16;
/// Value the custom stepper shows before an explicit count is set
/// (non-preset, so it reads as a distinct "custom" choice).
const CONC_DEFAULT: i64 = 4;
/// Queue color button (design `.q-color-btn`: 24px square, 6px radius,
/// 2px border; hover border brightens to fg-3).
const COLOR_BTN: f32 = 24.0;
const COLOR_BTN_RADIUS: f32 = 6.0;
const COLOR_BTN_BORDER: f32 = 2.0;
/// Color-pop geometry (design `.q-color-pop`: 22px swatches with 2px
/// selection ring, 6px gap, 8px padding, 2px border).
const POP_SWATCH: f32 = 22.0;
const POP_GAP: f32 = 6.0;
const POP_PAD: f32 = 8.0;
const POP_BORDER: f32 = 2.0;
/// Queue-name input renders in the display serif at 16px (design
/// `queue-dialog.jsx` name input: `--font-display`, 16px, weight 500 —
/// the bundled Fraunces SemiBold stands in for 500).
const NAME_FONT_SIZE: f32 = 16.0;
/// Default window size; also the overlay clamp fallback before the
/// first resize event arrives.
const WIN_DEFAULT_W: f32 = 820.0;
const WIN_DEFAULT_H: f32 = 620.0;

/// Preset queue swatches (design `queue-dialog.jsx` `QUEUE_COLORS`).
/// Persisted `Queue.color` *data* values, not theme styling tokens:
/// clay/rust/ochre/moss/slate coincide with `color.rs` ramp stops
/// (clay-400 / clay-500 / ochre-300 / moss-400 / slate-300); olive,
/// forest, wine and sand exist only in the mock's preset list.
const QUEUE_PRESETS: [[u8; 3]; 9] = [
    [0xC9, 0x70, 0x3F], // clay (clay-400)
    [0xB2, 0x5A, 0x2A], // rust (clay-500)
    [0xDD, 0xAA, 0x38], // ochre (ochre-300)
    [0xA3, 0x91, 0x42], // olive
    [0x7A, 0x8B, 0x4A], // moss (moss-400)
    [0x4D, 0x6B, 0x4A], // forest
    [0x5E, 0x68, 0x77], // slate (slate-300)
    [0x8E, 0x4B, 0x5A], // wine
    [0xBE, 0xA4, 0x7A], // sand
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedKind {
    Manual,
    Recurring,
    OneOff,
    Condition,
}

/// Condition-builder geometry (design `.cond-builder` family,
/// styles.css:2111-2192). Cards: radius 8, head padding 10/12 gap 11;
/// 30px icon tile radius 7; params rows indent to the text column
/// (12 + 30 + 11 = 53) and use 58px number inputs. Combine bar padding
/// 8/10 radius 7 with a mini segmented (buttons pad 3/12 radius 4).
/// Connector rows inset 27 with a 999-radius AND/OR pill (pad 2/9).
const COND_GAP: f32 = 10.0;
const COND_LIST_GAP: f32 = 6.0;
const COND_CARD_RADIUS: f32 = 8.0;
const COND_HEAD_PAD_Y: f32 = 10.0;
const COND_HEAD_PAD_X: f32 = 12.0;
const COND_HEAD_GAP: f32 = 11.0;
const COND_ICO: f32 = 30.0;
const COND_ICO_RADIUS: f32 = 7.0;
const COND_PARAM_INDENT: f32 = COND_HEAD_PAD_X + COND_ICO + COND_HEAD_GAP;
const COND_NUM_W: f32 = 58.0;
const COMBINE_PAD_Y: f32 = 8.0;
const COMBINE_PAD_X: f32 = 10.0;
const COMBINE_RADIUS: f32 = 7.0;
const COMBINE_BTN_PAD_Y: f32 = 3.0;
const COMBINE_BTN_PAD_X: f32 = 12.0;
const CONNECTOR_INSET: f32 = 27.0;
const CONJ_PAD_Y: f32 = 2.0;
const CONJ_PAD_X: f32 = 9.0;
/// Param defaults shown when a card is first enabled (design
/// `state.minutes ?? 10`, `state.interval ?? 60`).
const DEFAULT_IDLE_MINUTES: u16 = 10;
const DEFAULT_CMD_INTERVAL: u32 = 60;

/// One-off inputs (design `once` row: 160px date, 100px time) and the
/// calendar popup: 32px day cells, 7 columns.
const ONCE_DATE_W: f32 = 160.0;
const ONCE_TIME_W: f32 = 100.0;
const CAL_CELL: f32 = 32.0;
const CAL_GAP: f32 = 2.0;
const CAL_PAD: f32 = 12.0;

fn parse_once_date(s: &str) -> Option<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()
}

fn parse_once_time(s: &str) -> Option<chrono::NaiveTime> {
    chrono::NaiveTime::parse_from_str(s.trim(), "%H:%M").ok()
}

/// Builder cards in display order (design `CONDS`). Copy is verbatim
/// from the mock, except "Wi-Fi / Ethernet" phrasing which the mock
/// already carries; the mock gates `unmetered` per platform
/// (`platformSupportsMetered`) — here `CondKind::SUPPORTED` gates
/// every card by what this build can honestly probe.
const COND_CARDS: [(CondKind, &str, &str, &str); 4] = [
    (
        CondKind::Unmetered,
        "wifi",
        "On an unmetered connection",
        "Only run when the active network is not billed by usage (Wi-Fi / Ethernet, not cellular or a metered hotspot).",
    ),
    (
        CondKind::Idle,
        "mouse-pointer-click",
        "System has been idle",
        "Wait until there\u{2019}s been no keyboard or mouse activity for a while.",
    ),
    (
        CondKind::AcPower,
        "plug-zap",
        "On AC power",
        "Only run while plugged in \u{2014} never on battery.",
    ),
    (
        CondKind::Command,
        "terminal",
        "Custom command returns true",
        "Poll a shell command; the queue runs while it exits 0.",
    ),
];

/// Dark-remapped clay tint trio (bg / fg / border) used by the active
/// card icon tile and the AND/OR pill — same remap as `conc_pill`.
/// clay-600 stays saturated in both themes (tokens.css keeps the
/// middle of the ramp untouched), so the tinted glyph uses `C600`
/// everywhere except dark, where the remapped `DARK_C700` text tint
/// keeps contrast on the dark clay-50.
fn clay_tint(t: &Tokens) -> (iced::Color, iced::Color, iced::Color) {
    match t.theme {
        theme::ResolvedTheme::Dark => (
            color::clay::DARK_C50,
            color::clay::DARK_C700,
            color::clay::DARK_C200,
        ),
        _ => (color::clay::C50, color::clay::C600, color::clay::C200),
    }
}

/// On-finish choices. The mock's "Disconnect" pill is omitted: no
/// disconnect hook exists in the domain (it round-tripped to a silent
/// no-op — dishonest UI per the features-pass rubric).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinishKind {
    Nothing,
    Notify,
    Sleep,
    Shutdown,
    RunCommand,
}

#[derive(Clone)]
pub enum Msg {
    Connected(
        Result<
            Box<(
                Arc<Client>,
                Vec<Queue>,
                crate::domain::Settings,
                Vec<CondKind>,
            )>,
            String,
        >,
    ),
    Queues(Vec<Queue>),
    Daemon(DaemonSignal),
    Window(WindowControl),
    Select(QueueId),
    AddQueue,
    Name(String),
    ColorToggle,
    ColorClose,
    ColorPick([u8; 3]),
    ColorHex(String),
    Concurrency(Option<usize>),
    Sched(SchedKind),
    SchedStart(String),
    SchedDay(u8, bool),
    OnceDate(String),
    OnceTime(String),
    CalToggle,
    CalClose,
    /// Shift the calendar's displayed month by ±1.
    CalMonth(i32),
    CalPick(chrono::NaiveDate),
    CondToggle(CondKind),
    CondCombine(CondCombine),
    CondIdleMin(String),
    CondCmdText(String),
    CondCmdInterval(String),
    Finish(FinishKind),
    FinishCommand(String),
    DeleteAsk,
    DeleteConfirm,
    DeleteCancel,
    KeyPressed(iced::keyboard::Key),
    Save,
    Saved(Result<(), String>),
    Cancel,
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
    queues: Vec<Queue>,
    selected: Option<QueueId>,

    name: String,
    max_concurrent: Option<usize>,
    sched: SchedKind,
    sched_start: String,
    sched_days: WeekDayMask,
    cond_combine: CondCombine,
    cond_unmetered: bool,
    cond_ac: bool,
    cond_idle: bool,
    /// Text buffers for the idle-minutes / command / interval params;
    /// parsed + clamped to the domain ranges on Save.
    cond_idle_min: String,
    cond_cmd: bool,
    cond_cmd_text: String,
    cond_cmd_interval: String,
    /// Conditions this host can evaluate (from the daemon snapshot);
    /// cards outside this set are hidden and don't participate.
    cond_avail: Vec<CondKind>,
    /// One-off date/time buffers, validated on change (`YYYY-MM-DD`,
    /// `HH:MM`).
    once_date: String,
    once_time: String,
    cal_open: bool,
    /// Month the calendar popup is showing: (year, month 1-12).
    cal_ym: (i32, u32),
    finish: FinishKind,
    finish_cmd: String,

    /// Staged swatch (rides `upsert_queue` on Save like other edits).
    color: Option<[u8; 3]>,
    /// Free-form hex mirror of `color` for the popup's custom input.
    color_hex: String,
    color_open: bool,
    win_size: (f32, f32),

    confirm_delete: bool,
    shot: Option<Shot>,
}

/// `#RRGGBB` for the hex mirror.
fn hex_string([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02X}{g:02X}{b:02X}")
}

/// Parse `#RRGGBB` / `RRGGBB`; anything else is not (yet) a color.
fn parse_hex_color(s: &str) -> Option<[u8; 3]> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

impl State {
    fn selected_queue(&self) -> Option<&Queue> {
        self.queues.iter().find(|q| Some(q.id) == self.selected)
    }

    fn hydrate(&mut self) {
        let Some(q) = self.selected_queue().cloned() else {
            return;
        };
        self.name = q.name;
        self.color = q.color;
        self.color_hex = q.color.map(hex_string).unwrap_or_default();
        self.color_open = false;
        self.max_concurrent = q.max_concurrent;
        self.sched = match q.schedule {
            QueueSchedule::Manual => SchedKind::Manual,
            QueueSchedule::Daily { .. } => SchedKind::Recurring,
            QueueSchedule::Once { .. } => SchedKind::OneOff,
            QueueSchedule::Condition { .. } => SchedKind::Condition,
        };
        self.sched_start = match q.schedule {
            QueueSchedule::Daily { start, .. } => start.format("%H:%M").to_string(),
            _ => String::new(),
        };
        let once_start = match q.schedule {
            QueueSchedule::Once { start, .. } => start.naive_local(),
            _ => chrono::Local::now()
                .date_naive()
                .and_hms_opt(9, 0, 0)
                .unwrap(),
        };
        self.once_date = once_start.format("%Y-%m-%d").to_string();
        self.once_time = once_start.format("%H:%M").to_string();
        self.cal_open = false;
        self.cal_ym = (
            chrono::Datelike::year(&once_start),
            chrono::Datelike::month(&once_start),
        );
        self.sched_days = match q.schedule {
            QueueSchedule::Daily { days, .. } => days,
            _ => WeekDayMask(0x7F),
        };
        let conds = match q.schedule {
            QueueSchedule::Condition(set) => set,
            _ => CondSet::default(),
        };
        self.cond_combine = conds.combine;
        self.cond_unmetered = conds.unmetered;
        self.cond_ac = conds.ac_power;
        self.cond_idle = conds.idle_minutes.is_some();
        self.cond_idle_min = conds
            .idle_minutes
            .unwrap_or(DEFAULT_IDLE_MINUTES)
            .to_string();
        self.cond_cmd = conds.command.is_some();
        self.cond_cmd_text = conds
            .command
            .as_ref()
            .map(|c| c.cmd.clone())
            .unwrap_or_default();
        self.cond_cmd_interval = conds
            .command
            .as_ref()
            .map(|c| c.interval_secs)
            .unwrap_or(DEFAULT_CMD_INTERVAL)
            .to_string();
        self.finish = q
            .on_finish
            .first()
            .map(|h| match h {
                QueueHook::Shutdown(_) => FinishKind::Shutdown,
                QueueHook::Sleep | QueueHook::Hibernate => FinishKind::Sleep,
                QueueHook::ExitOxdm => FinishKind::Nothing,
                QueueHook::RunCommand { .. } => FinishKind::RunCommand,
                QueueHook::Notify { .. } => FinishKind::Notify,
            })
            .unwrap_or(FinishKind::Nothing);
        self.finish_cmd = q
            .on_finish
            .iter()
            .find_map(|h| match h {
                QueueHook::RunCommand { cmd, .. } => Some(cmd.clone()),
                _ => None,
            })
            .unwrap_or_default();
    }

    fn build_queue(&self) -> Option<Queue> {
        let mut q = self.selected_queue()?.clone();
        q.name = self.name.trim().to_owned();
        q.color = self.color;
        q.max_concurrent = self.max_concurrent;
        q.schedule = match self.sched {
            SchedKind::Manual => QueueSchedule::Manual,
            SchedKind::Condition => QueueSchedule::Condition(CondSet {
                combine: self.cond_combine,
                unmetered: self.cond_unmetered,
                ac_power: self.cond_ac,
                idle_minutes: self.cond_idle.then(|| {
                    self.cond_idle_min
                        .trim()
                        .parse::<u16>()
                        .unwrap_or(DEFAULT_IDLE_MINUTES)
                        .clamp(*IDLE_MINUTES_RANGE.start(), *IDLE_MINUTES_RANGE.end())
                }),
                command: self.cond_cmd.then(|| CondCommand {
                    cmd: self.cond_cmd_text.trim().to_owned(),
                    interval_secs: self
                        .cond_cmd_interval
                        .trim()
                        .parse::<u32>()
                        .unwrap_or(DEFAULT_CMD_INTERVAL)
                        .clamp(*CMD_INTERVAL_RANGE.start(), *CMD_INTERVAL_RANGE.end()),
                }),
            }),
            SchedKind::Recurring => QueueSchedule::Daily {
                start: chrono::NaiveTime::parse_from_str(self.sched_start.trim(), "%H:%M")
                    .unwrap_or_else(|_| chrono::NaiveTime::from_hms_opt(9, 0, 0).unwrap()),
                stop: None,
                days: self.sched_days,
            },
            SchedKind::OneOff => QueueSchedule::Once {
                start: parse_once_date(&self.once_date)
                    .zip(parse_once_time(&self.once_time))
                    .map(|(d, t)| d.and_time(t))
                    .and_then(|n| n.and_local_timezone(chrono::Local).single())
                    .unwrap_or_else(chrono::Local::now),
                stop: None,
            },
        };
        q.on_finish = match self.finish {
            FinishKind::Nothing => vec![],
            FinishKind::Notify => vec![QueueHook::Notify {
                title: "Queue finished".to_owned(),
                body: q.name.clone(),
            }],
            FinishKind::Sleep => vec![QueueHook::Sleep],
            FinishKind::Shutdown => vec![QueueHook::Shutdown(ShutdownAction::ShutDown)],
            FinishKind::RunCommand => vec![QueueHook::RunCommand {
                cmd: self.finish_cmd.trim().to_owned(),
                args: vec![],
            }],
        };
        Some(q)
    }
}

pub fn boot() -> (App, Task<Msg>) {
    (
        App::Connecting,
        Task::perform(
            async {
                let client = Client::connect_retry(Duration::from_secs(8))
                    .await
                    .map_err(|e| e.to_string())?;
                client
                    .hello(crate::ipc_local::protocol::GuiKind::Queues)
                    .await?;
                let snap = client.snapshot().await?;
                Ok(Box::new((
                    client,
                    snap.queues,
                    snap.settings,
                    snap.cond_available,
                )))
            },
            Msg::Connected,
        ),
    )
}

pub fn update(app: &mut App, msg: Msg) -> Task<Msg> {
    match msg {
        Msg::Connected(Ok(boxed)) => {
            let (client, queues, settings, cond_avail) = *boxed;
            let mut st = State {
                tokens: Tokens::from_settings(&settings),
                selected: queues.first().map(|q| q.id),
                queues,
                name: String::new(),
                max_concurrent: None,
                sched: SchedKind::Manual,
                sched_start: String::new(),
                sched_days: WeekDayMask(0x7F),
                cond_combine: CondCombine::default(),
                cond_unmetered: false,
                cond_ac: false,
                cond_idle: false,
                cond_idle_min: DEFAULT_IDLE_MINUTES.to_string(),
                cond_cmd: false,
                cond_cmd_text: String::new(),
                cond_cmd_interval: DEFAULT_CMD_INTERVAL.to_string(),
                cond_avail,
                once_date: String::new(),
                once_time: String::new(),
                cal_open: false,
                cal_ym: (2026, 1),
                finish: FinishKind::Nothing,
                finish_cmd: String::new(),
                color: None,
                color_hex: String::new(),
                color_open: false,
                win_size: (0.0, 0.0),
                confirm_delete: false,
                shot: Shot::from_env(),
                client,
            };
            st.hydrate();
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
    match msg {
        Msg::Queues(qs) => {
            st.queues = qs;
            if st.selected_queue().is_none() {
                st.selected = st.queues.first().map(|q| q.id);
                st.hydrate();
            }
            Task::none()
        }
        Msg::Daemon(DaemonSignal::Lost) => iced::exit(),
        Msg::Daemon(DaemonSignal::Event(ev)) => match ev {
            Event::QueuesChanged => {
                let client = st.client.clone();
                Task::perform(async move { client.snapshot().await }, |r| match r {
                    Ok(s) => Msg::Queues(s.queues),
                    Err(_) => Msg::Noop,
                })
            }
            Event::Close => iced::exit(),
            Event::Focus => iced::window::latest().and_then(iced::window::gain_focus),
            _ => Task::none(),
        },
        Msg::Select(id) => {
            st.selected = Some(id);
            st.confirm_delete = false;
            st.hydrate();
            Task::none()
        }
        Msg::AddQueue => {
            let client = st.client.clone();
            let n = st.queues.len();
            Task::perform(
                async move {
                    let mut q = Queue::new_main();
                    q.builtin = false;
                    q.name = format!("Queue {}", n + 1);
                    q.color = Some(crate::domain::random_vivid_color());
                    client.upsert_queue(q).await
                },
                |_| Msg::Noop,
            )
        }
        Msg::Name(v) => {
            st.name = v;
            Task::none()
        }
        Msg::ColorToggle => {
            st.color_open = !st.color_open;
            Task::none()
        }
        Msg::ColorClose => {
            st.color_open = false;
            Task::none()
        }
        Msg::ColorPick(c) => {
            st.color = Some(c);
            st.color_hex = hex_string(c);
            st.color_open = false;
            Task::none()
        }
        Msg::ColorHex(v) => {
            if let Some(c) = parse_hex_color(&v) {
                st.color = Some(c);
            }
            st.color_hex = v;
            Task::none()
        }
        Msg::Concurrency(v) => {
            st.max_concurrent = v;
            Task::none()
        }
        Msg::Sched(k) => {
            st.sched = k;
            Task::none()
        }
        Msg::SchedStart(v) => {
            st.sched_start = v;
            Task::none()
        }
        Msg::SchedDay(bit, on) => {
            if on {
                st.sched_days.0 |= 1 << bit;
            } else {
                st.sched_days.0 &= !(1 << bit);
            }
            Task::none()
        }
        Msg::OnceDate(v) => {
            st.once_date = v;
            Task::none()
        }
        Msg::OnceTime(v) => {
            st.once_time = v;
            Task::none()
        }
        Msg::CalToggle => {
            st.cal_open = !st.cal_open;
            if st.cal_open
                && let Some(d) = parse_once_date(&st.once_date)
            {
                st.cal_ym = (chrono::Datelike::year(&d), chrono::Datelike::month(&d));
            }
            Task::none()
        }
        Msg::CalClose => {
            st.cal_open = false;
            Task::none()
        }
        Msg::CalMonth(delta) => {
            let (y, m) = st.cal_ym;
            let total = y * 12 + (m as i32 - 1) + delta;
            st.cal_ym = (total.div_euclid(12), (total.rem_euclid(12) + 1) as u32);
            Task::none()
        }
        Msg::CalPick(d) => {
            st.once_date = d.format("%Y-%m-%d").to_string();
            st.cal_open = false;
            Task::none()
        }
        Msg::CondToggle(kind) => {
            match kind {
                CondKind::Unmetered => st.cond_unmetered = !st.cond_unmetered,
                CondKind::AcPower => st.cond_ac = !st.cond_ac,
                CondKind::Idle => st.cond_idle = !st.cond_idle,
                CondKind::Command => st.cond_cmd = !st.cond_cmd,
            }
            Task::none()
        }
        Msg::CondCombine(c) => {
            st.cond_combine = c;
            Task::none()
        }
        Msg::CondIdleMin(v) => {
            st.cond_idle_min = v;
            Task::none()
        }
        Msg::CondCmdText(v) => {
            st.cond_cmd_text = v;
            Task::none()
        }
        Msg::CondCmdInterval(v) => {
            st.cond_cmd_interval = v;
            Task::none()
        }
        Msg::Finish(k) => {
            st.finish = k;
            Task::none()
        }
        Msg::FinishCommand(v) => {
            st.finish_cmd = v;
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
            let Some(id) = st.selected else {
                return Task::none();
            };
            let client = st.client.clone();
            Task::perform(async move { client.delete_queue(id).await }, |_| Msg::Noop)
        }
        // Confirm-dialog keys (design `confirm-dialog.jsx`): Enter
        // confirms, Escape cancels. `listen_with` ignores capture
        // status, so both are gated on the confirm overlay being open.
        // Escape also dismisses the color popup when it is open.
        Msg::KeyPressed(key) => {
            use iced::keyboard::key::Named;
            if st.confirm_delete {
                match key.as_ref() {
                    iced::keyboard::Key::Named(Named::Enter) => {
                        return update_ready(st, Msg::DeleteConfirm);
                    }
                    iced::keyboard::Key::Named(Named::Escape) => {
                        return update_ready(st, Msg::DeleteCancel);
                    }
                    _ => {}
                }
            } else if st.color_open
                && matches!(key.as_ref(), iced::keyboard::Key::Named(Named::Escape))
            {
                return update_ready(st, Msg::ColorClose);
            } else if st.cal_open
                && matches!(key.as_ref(), iced::keyboard::Key::Named(Named::Escape))
            {
                return update_ready(st, Msg::CalClose);
            }
            Task::none()
        }
        Msg::Save => {
            let Some(q) = st.build_queue() else {
                return Task::none();
            };
            let client = st.client.clone();
            Task::perform(async move { client.upsert_queue(q).await }, Msg::Saved)
        }
        Msg::Saved(_) => iced::exit(),
        Msg::Cancel => iced::exit(),
        Msg::WinResized(w, h) => {
            st.win_size = (w, h);
            chrome::enforce_min_size(iced::Size::new(w, h), iced::Size::new(640.0, 518.0))
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
            iced::Event::Keyboard(iced::keyboard::Event::KeyPressed { key, .. }) => {
                Some(Msg::KeyPressed(key))
            }
            _ => None,
        }),
        crate::gui::ipc::all_events(crate::ipc_local::protocol::GuiKind::Queues).map(Msg::Daemon),
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

fn seg_btn<'a>(
    t: &Tokens,
    label: &'a str,
    icon: Option<&'a str>,
    selected: bool,
    msg: Msg,
) -> Element<'a, Msg> {
    let mut b = Btn::new(label).secondary().selected(selected).on_press(msg);
    if let Some(icon) = icon {
        b = b.icon(icon);
    }
    b.view(t)
}

/// Concurrency preset pill. Idle/hover mirror a `secondary` button so
/// it reads identically to the other preset rows, but the *selected*
/// state uses the design's `.radio-pill .on` clay tint (clay-50 bg /
/// clay-700 text / clay-200 border) instead of the generic
/// secondary-selected (sunken + brand border). See T1-QUEUES.
fn conc_pill<'a>(t: &Tokens, label: &'a str, selected: bool, msg: Msg) -> Element<'a, Msg> {
    let t2 = *t;
    // tokens.css remaps clay-50/200/700 to dark warm tints under the
    // dark theme so the active pill doesn't punch a bright hole.
    let (on_bg, on_fg, on_border) = match t.theme {
        theme::ResolvedTheme::Dark => (
            color::clay::DARK_C50,
            color::clay::DARK_C700,
            color::clay::DARK_C200,
        ),
        _ => (color::clay::C50, color::clay::C700, color::clay::C200),
    };
    let content = container(text(label).font(theme::BODY_BOLD).size(CONC_PILL_FONT))
        .center_x(Length::Fill)
        .center_y(Length::Fill);
    button(content)
        .height(Length::Fixed(theme::control::H_MD))
        .padding([0.0, CONC_PILL_PAD_X])
        .on_press(msg)
        .style(move |_th, status| {
            use iced::widget::button::Status::*;
            if selected {
                return iced::widget::button::Style {
                    background: Some(on_bg.into()),
                    text_color: on_fg,
                    border: iced::Border {
                        color: on_border,
                        width: 1.0,
                        radius: theme::control::RADIUS.into(),
                    },
                    shadow: iced::Shadow::default(),
                    snap: true,
                };
            }
            let bg = match status {
                Hovered => color::mix(t2.bg_raised, t2.bg_sunken, 0.5),
                Pressed => t2.bg_sunken,
                _ => t2.bg_raised,
            };
            iced::widget::button::Style {
                background: Some(bg.into()),
                text_color: t2.fg_1,
                border: iced::Border {
                    color: t2.border_default,
                    width: 1.0,
                    radius: theme::control::RADIUS.into(),
                },
                shadow: iced::Shadow::default(),
                snap: true,
            }
        })
        .into()
}

/// One square of the recurring-schedule day grid: a ~28px toggle
/// button bearing the day initial. On → theme clay-400 accent fill /
/// inverse text (design `.day-grid .d.on`); off → sunken square.
/// Preserves the per-day `Msg::SchedDay(bit, _)` toggle. See T2-QUEUES.
fn day_square<'a>(t: &Tokens, label: &str, on: bool, msg: Msg) -> Element<'a, Msg> {
    let t2 = *t;
    let initial = label.chars().next().unwrap_or(' ').to_string();
    let content = container(text(initial).font(theme::BODY_BOLD).size(CONC_PILL_FONT))
        .center_x(Length::Fill)
        .center_y(Length::Fill);
    button(content)
        .width(Length::Fixed(DAY_SQUARE))
        .height(Length::Fixed(DAY_SQUARE))
        .padding(0.0)
        .on_press(msg)
        .style(move |_th, status| {
            use iced::widget::button::Status::*;
            let (bg, fg, border) = if on {
                (t2.action_primary, t2.action_primary_fg, t2.action_primary)
            } else {
                let bg = match status {
                    Hovered | Pressed => t2.bg_sunken_hover,
                    _ => t2.bg_sunken,
                };
                (bg, t2.fg_2, t2.border_default)
            };
            iced::widget::button::Style {
                background: Some(bg.into()),
                text_color: fg,
                border: iced::Border {
                    color: border,
                    width: 1.0,
                    radius: theme::radius::XS.into(),
                },
                shadow: iced::Shadow::default(),
                snap: true,
            }
        })
        .into()
}

impl State {
    fn cond_on(&self, kind: CondKind) -> bool {
        match kind {
            CondKind::Unmetered => self.cond_unmetered,
            CondKind::Idle => self.cond_idle,
            CondKind::AcPower => self.cond_ac,
            CondKind::Command => self.cond_cmd,
        }
    }
}

/// The design's `.cond-builder`: help line, All/Any combine bar (when
/// ≥2 conditions are on), the card list with AND/OR connectors, and
/// the rust empty-state warning.
fn cond_builder<'a>(t: &Tokens, st: &'a State) -> Element<'a, Msg> {
    let mut col = column![
        text("The queue starts automatically while its conditions hold, and pauses when they no longer do.")
            .font(theme::BODY)
            .size(11.0)
            .color(t.fg_3),
    ]
    .spacing(COND_GAP);

    // Only cards this host can evaluate right now (daemon capability
    // snapshot). A hidden condition does not participate in the
    // scheduler either, so showing it — even enabled by a foreign
    // config — would be dishonest.
    let visible: Vec<&(CondKind, &str, &str, &str)> = COND_CARDS
        .iter()
        .filter(|(k, ..)| st.cond_avail.contains(k))
        .collect();
    let enabled_count = visible.iter().filter(|(k, ..)| st.cond_on(*k)).count();

    if enabled_count >= 2 {
        col = col.push(combine_bar(t, st.cond_combine, enabled_count));
    }

    let mut list = column![].spacing(COND_LIST_GAP);
    let mut enabled_seen = 0usize;
    for (kind, icon_name, label, desc) in visible {
        let on = st.cond_on(*kind);
        if on && enabled_seen > 0 {
            list = list.push(cond_connector(t, st.cond_combine));
        }
        if on {
            enabled_seen += 1;
        }
        list = list.push(cond_card(t, st, *kind, icon_name, label, desc, on));
    }
    col = col.push(list);

    if enabled_count == 0 {
        col = col.push(
            row![
                icons::icon("toggle-left", 13.0, t.status_danger),
                text("Enable at least one condition, or the queue will never start on its own.")
                    .font(theme::BODY)
                    .size(11.0)
                    .color(t.status_danger),
            ]
            .spacing(7.0)
            .align_y(Alignment::Center),
        );
    }
    col.into()
}

/// `.cond-combine`: "Start when [All|Any] of the N enabled conditions
/// are/is met" in a sunken bar.
fn combine_bar<'a>(t: &Tokens, combine: CondCombine, enabled: usize) -> Element<'a, Msg> {
    let t2 = *t;
    let seg_btn = |label: &'a str, value: CondCombine| {
        let on = combine == value;
        button(text(label).font(theme::BODY_BOLD).size(11.5))
            .padding([COMBINE_BTN_PAD_Y, COMBINE_BTN_PAD_X])
            .on_press(Msg::CondCombine(value))
            .style(move |_th, _status| iced::widget::button::Style {
                background: Some(if on {
                    t2.action_primary.into()
                } else {
                    iced::Color::TRANSPARENT.into()
                }),
                text_color: if on { t2.action_primary_fg } else { t2.fg_2 },
                border: iced::Border {
                    radius: 4.0.into(),
                    ..Default::default()
                },
                shadow: iced::Shadow::default(),
                snap: true,
            })
    };
    let seg = container(
        row![
            seg_btn("All", CondCombine::All),
            seg_btn("Any", CondCombine::Any)
        ]
        .spacing(2.0),
    )
    .padding(2.0)
    .style(move |_| container::Style {
        background: Some(t2.bg_page.into()),
        border: iced::Border {
            color: t2.border_default,
            width: 1.0,
            radius: 6.0.into(),
        },
        ..Default::default()
    });
    let tail = format!(
        "of the {enabled} enabled conditions {}",
        if combine == CondCombine::All {
            "are met"
        } else {
            "is met"
        }
    );
    container(
        row![
            text("Start when")
                .font(theme::BODY)
                .size(11.5)
                .color(t.fg_2),
            seg,
            text(tail).font(theme::BODY).size(11.5).color(t.fg_2),
        ]
        .spacing(theme::space::S2)
        .align_y(Alignment::Center),
    )
    .padding([COMBINE_PAD_Y, COMBINE_PAD_X])
    .width(Length::Fill)
    .style(move |_| container::Style {
        background: Some(t2.bg_sunken.into()),
        border: iced::Border {
            color: t2.border_subtle,
            width: 1.0,
            radius: COMBINE_RADIUS.into(),
        },
        ..Default::default()
    })
    .into()
}

/// `.cond-connector`: hairline · AND/OR pill · hairline between
/// enabled cards; clicking the pill flips the combine mode.
fn cond_connector<'a>(t: &Tokens, combine: CondCombine) -> Element<'a, Msg> {
    let t2 = *t;
    let (tint_bg, tint_fg, tint_border) = clay_tint(t);
    let flipped = match combine {
        CondCombine::All => CondCombine::Any,
        CondCombine::Any => CondCombine::All,
    };
    let conj = button(
        text(if combine == CondCombine::All {
            "AND"
        } else {
            "OR"
        })
        .font(theme::MONO_BOLD)
        .size(10.0),
    )
    .padding([CONJ_PAD_Y, CONJ_PAD_X])
    .on_press(Msg::CondCombine(flipped))
    .style(move |_th, status| {
        let hovered = matches!(status, iced::widget::button::Status::Hovered);
        iced::widget::button::Style {
            background: Some(
                if hovered {
                    color::mix(tint_bg, tint_border, 0.3)
                } else {
                    tint_bg
                }
                .into(),
            ),
            text_color: tint_fg,
            border: iced::Border {
                color: tint_border,
                width: 1.0,
                radius: 999.0.into(),
            },
            shadow: iced::Shadow::default(),
            snap: true,
        }
    });
    container(
        row![hairline(t2.border_subtle), conj, hairline(t2.border_subtle)]
            .spacing(theme::space::S2)
            .align_y(Alignment::Center),
    )
    .padding(iced::Padding {
        top: 1.0,
        bottom: 1.0,
        left: CONNECTOR_INSET,
        right: 0.0,
    })
    .into()
}

/// One `.cond-card`: clickable head (icon tile + label/desc + switch)
/// and, when on, the per-condition parameter row.
fn cond_card<'a>(
    t: &Tokens,
    st: &'a State,
    kind: CondKind,
    icon_name: &'a str,
    label: &'a str,
    desc: &'a str,
    on: bool,
) -> Element<'a, Msg> {
    let t2 = *t;
    let (tint_bg, tint_fg, tint_border) = clay_tint(t);

    let ico = container(icons::icon(
        icon_name,
        15.0,
        if on { tint_fg } else { t.fg_3 },
    ))
    .width(Length::Fixed(COND_ICO))
    .height(Length::Fixed(COND_ICO))
    .align_x(Alignment::Center)
    .align_y(Alignment::Center)
    .style(move |_| container::Style {
        background: Some(if on { tint_bg } else { t2.bg_sunken }.into()),
        border: iced::Border {
            color: if on { tint_border } else { t2.border_subtle },
            width: 1.0,
            radius: COND_ICO_RADIUS.into(),
        },
        ..Default::default()
    });

    let text_col = column![
        text(label).font(theme::BODY_BOLD).size(12.0).color(t.fg_1),
        text(desc)
            .font(theme::BODY)
            .size(11.0)
            .color(t.fg_3)
            .line_height(1.35),
    ]
    .spacing(2.0)
    .width(Length::Fill);

    // The switch handles its own click; the rest of the head is one
    // big toggle target (design: the whole `.cc-head` toggles).
    let head = row![
        mouse_area(
            row![ico, text_col]
                .spacing(COND_HEAD_GAP)
                .align_y(Alignment::Center)
                .width(Length::Fill),
        )
        .on_press(Msg::CondToggle(kind))
        .interaction(iced::mouse::Interaction::Pointer),
        toggle(t, on, true, move |_| Msg::CondToggle(kind)),
    ]
    .spacing(COND_HEAD_GAP)
    .align_y(Alignment::Center)
    .padding([COND_HEAD_PAD_Y, COND_HEAD_PAD_X]);

    let mut card = column![head];

    let params_pad = iced::Padding {
        top: 0.0,
        right: COND_HEAD_PAD_X,
        bottom: 12.0,
        left: COND_PARAM_INDENT,
    };
    let param_label = |s: &'a str| text(s).font(theme::BODY).size(11.0).color(t2.fg_3);
    if on && kind == CondKind::Idle {
        card = card.push(
            container(
                row![
                    param_label("for"),
                    TextInput::new(&st.cond_idle_min)
                        .mono()
                        .width(Length::Fixed(COND_NUM_W))
                        .on_input(Msg::CondIdleMin)
                        .view(t),
                    param_label("minutes"),
                ]
                .spacing(theme::space::S2)
                .align_y(Alignment::Center),
            )
            .padding(params_pad),
        );
    }
    if on && kind == CondKind::Command {
        card = card.push(
            container(
                column![
                    TextInput::new(&st.cond_cmd_text)
                        .hint("/usr/local/bin/ready-to-download.sh")
                        .mono()
                        .width(Length::Fill)
                        .on_input(Msg::CondCmdText)
                        .view(t),
                    row![
                        param_label("re-check every"),
                        TextInput::new(&st.cond_cmd_interval)
                            .mono()
                            .width(Length::Fixed(COND_NUM_W))
                            .on_input(Msg::CondCmdInterval)
                            .view(t),
                        param_label("seconds \u{b7} runs while exit code is 0"),
                    ]
                    .spacing(theme::space::S2)
                    .align_y(Alignment::Center),
                ]
                .spacing(theme::space::S2),
            )
            .padding(params_pad),
        );
    }

    container(card)
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: Some(if on { t2.bg_surface } else { t2.bg_page }.into()),
            border: iced::Border {
                color: if on { tint_border } else { t2.border_subtle },
                width: 1.0,
                radius: COND_CARD_RADIUS.into(),
            },
            ..Default::default()
        })
        .into()
}

fn ready_view(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let t2 = *t;

    // Left: queue list.
    let mut list = column![]
        .spacing(theme::space::S2)
        .padding(theme::space::S3);
    for q in &st.queues {
        let active = Some(q.id) == st.selected;
        let count = q.max_concurrent.unwrap_or(0);
        list = list.push(
            mouse_area(
                container(
                    row![
                        crate::gui::widget::dot(8.0, t.queue_color(q)),
                        text(q.name.clone())
                            .font(theme::BODY_MEDIUM)
                            .size(13.0)
                            .color(t.fg_1),
                        iced::widget::Space::new().width(Length::Fill),
                        text(format!("{count}\u{00d7}"))
                            .font(theme::MONO)
                            .size(11.0)
                            .color(t.fg_3),
                    ]
                    .spacing(theme::space::S2)
                    .align_y(Alignment::Center),
                )
                .width(Length::Fill)
                .height(Length::Fixed(44.0))
                .align_y(Alignment::Center)
                .padding([0.0, theme::space::S3])
                .style(move |_| container::Style {
                    background: Some(t2.bg_surface.into()),
                    border: iced::Border {
                        color: if active {
                            t2.border_brand
                        } else {
                            t2.border_subtle
                        },
                        width: 1.0,
                        radius: theme::radius::SM.into(),
                    },
                    ..Default::default()
                }),
            )
            .on_press(Msg::Select(q.id))
            .interaction(iced::mouse::Interaction::Pointer),
        );
    }
    list = list.push(
        Btn::new("Add queue")
            .ghost()
            .icon("plus")
            .on_press(Msg::AddQueue)
            .view(t),
    );
    let sidebar = container(scrollable(list).height(Length::Fill))
        .width(Length::Fixed(LIST_W))
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: Some(t2.bg_sidebar.into()),
            ..Default::default()
        });

    // Right: editor.
    let is_main = st.selected_queue().is_some_and(|q| q.builtin);
    // Effective swatch: staged pick wins, else the queue's stored /
    // name-derived color (same fallback as the list dots).
    let eff_color = st
        .color
        .map(|[r, g, b]| iced::Color::from_rgb8(r, g, b))
        .or_else(|| st.selected_queue().map(|q| t.queue_color(q)))
        .unwrap_or(t.action_primary);
    let color_btn = button(iced::widget::Space::new())
        .width(Length::Fixed(COLOR_BTN))
        .height(Length::Fixed(COLOR_BTN))
        .padding(0.0)
        .on_press(Msg::ColorToggle)
        .style(move |_th, status| iced::widget::button::Style {
            background: Some(eff_color.into()),
            text_color: t2.fg_1,
            border: iced::Border {
                color: if matches!(status, iced::widget::button::Status::Hovered) {
                    t2.fg_3
                } else {
                    t2.border_default
                },
                width: COLOR_BTN_BORDER,
                radius: COLOR_BTN_RADIUS.into(),
            },
            shadow: iced::Shadow::default(),
            snap: true,
        });
    let head = row![
        color_btn,
        TextInput::new(&st.name)
            .font(theme::DISPLAY, NAME_FONT_SIZE)
            .on_input(Msg::Name)
            .view(t),
        Btn::new("Delete")
            .danger_filled()
            .icon("trash-2")
            .enabled(!is_main)
            .on_press(Msg::DeleteAsk)
            .view(t),
    ]
    .spacing(theme::space::S2)
    .align_y(Alignment::Center);

    let conc = st.max_concurrent;
    let concurrency = section_card(
        t,
        "layers",
        "Concurrency",
        column![
            text("How many downloads from this queue can run in parallel.")
                .font(theme::BODY)
                .size(12.0)
                .color(t.fg_3),
            row![
                // "Auto" = inherit the global concurrency (value `None`);
                // kept explicitly selectable so a queue can be set back
                // to inherit after a concrete count was chosen.
                conc_pill(t, "Auto", conc.is_none(), Msg::Concurrency(None)),
                conc_pill(t, "1x", conc == Some(1), Msg::Concurrency(Some(1))),
                conc_pill(t, "2x", conc == Some(2), Msg::Concurrency(Some(2))),
                conc_pill(t, "3x", conc == Some(3), Msg::Concurrency(Some(3))),
                conc_pill(t, "5x", conc == Some(5), Msg::Concurrency(Some(5))),
                conc_pill(t, "8x", conc == Some(8), Msg::Concurrency(Some(8))),
                // "Custom" pill → inline stepper, reusing the existing
                // `Concurrency(Some(_))` message for an arbitrary count.
                // Disabled (neutral) while Auto is active so it never
                // implies a concrete value for an inheriting queue.
                number_stepper(
                    t,
                    conc.map(|c| c as i64).unwrap_or(CONC_DEFAULT),
                    CONC_MIN,
                    CONC_MAX,
                    conc.is_some(),
                    |n| Msg::Concurrency(Some(n as usize)),
                ),
            ]
            .spacing(4.0)
            .align_y(Alignment::Center),
        ]
        .spacing(theme::space::S3)
        .into(),
    );

    let mut sched_pills = row![
        seg_btn(
            t,
            "Manual",
            Some("calendar"),
            st.sched == SchedKind::Manual,
            Msg::Sched(SchedKind::Manual)
        ),
        seg_btn(
            t,
            "Recurring",
            Some("refresh-cw"),
            st.sched == SchedKind::Recurring,
            Msg::Sched(SchedKind::Recurring)
        ),
        seg_btn(
            t,
            "One-off",
            Some("zap"),
            st.sched == SchedKind::OneOff,
            Msg::Sched(SchedKind::OneOff)
        ),
    ]
    .spacing(4.0);
    // Offer Condition only where this host can evaluate at least one
    // condition; still show the pill when the saved schedule already
    // is one (honest display beats silently rewriting to Manual).
    if !st.cond_avail.is_empty() || st.sched == SchedKind::Condition {
        sched_pills = sched_pills.push(seg_btn(
            t,
            "Condition",
            Some("wifi"),
            st.sched == SchedKind::Condition,
            Msg::Sched(SchedKind::Condition),
        ));
    }
    let mut sched_col = column![sched_pills].spacing(theme::space::S3);
    match st.sched {
        SchedKind::Recurring => {
            let mut days = row![].spacing(theme::space::S1);
            for (bit, label) in [
                (0u8, "Mon"),
                (1, "Tue"),
                (2, "Wed"),
                (3, "Thu"),
                (4, "Fri"),
                (5, "Sat"),
                (6, "Sun"),
            ] {
                let on = st.sched_days.0 & (1 << bit) != 0;
                days = days.push(day_square(t, label, on, Msg::SchedDay(bit, !on)));
            }
            sched_col = sched_col
                .push(
                    row![
                        text("Start time")
                            .font(theme::BODY)
                            .size(13.0)
                            .color(t.fg_2),
                        TextInput::new(&st.sched_start)
                            .hint("09:00")
                            .mono()
                            .width(Length::Fixed(90.0))
                            .on_input(Msg::SchedStart)
                            .view(t),
                    ]
                    .spacing(theme::space::S2)
                    .align_y(Alignment::Center),
                )
                .push(days);
        }
        SchedKind::OneOff => {
            let date_ok = parse_once_date(&st.once_date).is_some();
            let time_ok = parse_once_time(&st.once_time).is_some();
            sched_col = sched_col.push(
                row![
                    text("Start at").font(theme::BODY).size(13.0).color(t.fg_2),
                    TextInput::new(&st.once_date)
                        .hint("2026-06-11")
                        .mono()
                        .width(Length::Fixed(ONCE_DATE_W))
                        .on_input(Msg::OnceDate)
                        .view(t),
                    Btn::new("")
                        .icon_only("calendar")
                        .on_press(Msg::CalToggle)
                        .view(t),
                    TextInput::new(&st.once_time)
                        .hint("09:00")
                        .mono()
                        .width(Length::Fixed(ONCE_TIME_W))
                        .on_input(Msg::OnceTime)
                        .view(t),
                ]
                .spacing(theme::space::S2)
                .align_y(Alignment::Center),
            );
            if !date_ok || !time_ok {
                let what = match (date_ok, time_ok) {
                    (false, false) => "date (YYYY-MM-DD) and time (HH:MM)",
                    (false, true) => "date — use YYYY-MM-DD",
                    _ => "time — use HH:MM",
                };
                sched_col = sched_col.push(
                    row![
                        icons::icon("triangle-alert", 12.0, t.status_danger),
                        text(format!("Invalid {what}."))
                            .font(theme::BODY)
                            .size(11.0)
                            .color(t.status_danger),
                    ]
                    .spacing(theme::space::S1)
                    .align_y(Alignment::Center),
                );
            }
        }
        SchedKind::Condition => {
            sched_col = sched_col.push(cond_builder(t, st));
        }
        SchedKind::Manual => {}
    }
    let schedule = section_card(t, "calendar", "Schedule", sched_col.into());

    let mut finish_col = column![
        row![
            seg_btn(
                t,
                "Nothing",
                None,
                st.finish == FinishKind::Nothing,
                Msg::Finish(FinishKind::Nothing)
            ),
            seg_btn(
                t,
                "Notify",
                Some("bell"),
                st.finish == FinishKind::Notify,
                Msg::Finish(FinishKind::Notify)
            ),
            seg_btn(
                t,
                "Sleep",
                Some("moon"),
                st.finish == FinishKind::Sleep,
                Msg::Finish(FinishKind::Sleep)
            ),
            seg_btn(
                t,
                "Shutdown",
                Some("power"),
                st.finish == FinishKind::Shutdown,
                Msg::Finish(FinishKind::Shutdown)
            ),
        ]
        .spacing(4.0),
        row![seg_btn(
            t,
            "Run command",
            Some("terminal"),
            st.finish == FinishKind::RunCommand,
            Msg::Finish(FinishKind::RunCommand)
        ),]
        .spacing(4.0),
    ]
    .spacing(theme::space::S2);
    if st.finish == FinishKind::RunCommand {
        finish_col = finish_col.push(
            TextInput::new(&st.finish_cmd)
                .hint("notify-send 'queue done'")
                .mono()
                .on_input(Msg::FinishCommand)
                .view(t),
        );
    }
    if let Some(warn) = finish_warn(t, st.finish) {
        finish_col = finish_col.push(warn);
    }
    let on_finish = section_card(t, "clock", "When the queue finishes", finish_col.into());

    let editor = crate::gui::widget::vscroll(
        container(column![head, concurrency, schedule, on_finish].spacing(theme::space::S3))
            .padding(iced::Padding {
                top: theme::space::S4,
                bottom: theme::space::S4,
                left: theme::space::S4,
                right: theme::space::S4 - crate::gui::widget::SCROLL_GUTTER,
            })
            .width(Length::Fill),
    )
    .height(Length::Fill);

    let footer_el = crate::gui::windows::add::footer(
        t,
        Btn::new("Cancel").ghost().on_press(Msg::Cancel).view(t),
        Btn::new("Save")
            .primary()
            .icon("check")
            .on_press(Msg::Save)
            .view(t),
    );

    let body: Element<'_, Msg> = column![
        row![sidebar, editor].height(Length::Fill),
        hairline(t.border_subtle),
        footer_el,
    ]
    .into();

    let overlaid: Element<'_, Msg> = if st.confirm_delete {
        delete_overlay(st, body)
    } else if st.color_open {
        color_pop_overlay(st, body)
    } else if st.cal_open {
        calendar_overlay(st, body)
    } else {
        body
    };

    let content = container(column![
        titlebar::titlebar(t, "oxdm — Queues & scheduling", false, Msg::Window),
        hairline(t.border_subtle),
        overlaid,
    ])
    .width(Length::Fill)
    .height(Length::Fill)
    .style(move |_| container::Style {
        background: Some(t2.bg_page.into()),
        text_color: Some(t2.fg_1),
        ..Default::default()
    });
    chrome::resize::resizable(t, content.into(), true, Msg::Window)
}

/// Rust warning panel under the on-finish pills for destructive power
/// actions (design §3.6 shutdown warning; tokens follow the
/// download-window completion warning: `status_danger_bg` panel with a
/// 1px `status_danger` border). Copy is queue-scoped (guardian G2b-2)
/// and quotes the real grace from `SHUTDOWN_GRACE_SECS` — true since
/// the Wave B grace + countdown-banner event landed. Sleep is included
/// because the grace covers every destructive power action.
fn finish_warn<'a>(t: &Tokens, finish: FinishKind) -> Option<Element<'a, Msg>> {
    let verb = match finish {
        FinishKind::Shutdown => "shut down",
        FinishKind::Sleep => "go to sleep",
        _ => return None,
    };
    let t2 = *t;
    Some(
        container(
            row![
                crate::gui::icons::icon("triangle-alert", 13.0, t.status_danger),
                text(format!(
                    "System will {verb} {} seconds after this queue finishes. \
                     A countdown banner with Cancel will appear.",
                    crate::domain::SHUTDOWN_GRACE_SECS
                ))
                .font(theme::BODY)
                .size(12.0)
                .color(t.status_danger),
            ]
            .spacing(theme::space::S2)
            .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        .padding(theme::space::S2)
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

/// Anchored color popup (design `.q-color-pop`): preset swatch row +
/// a hex input for custom colors (stands in for the mock's native
/// color input — no such control exists in iced). Anchored at the
/// opening click position and clamped inside the window, following
/// the main-window context-menu stack pattern.
fn color_pop_overlay<'a>(st: &'a State, base: Element<'a, Msg>) -> Element<'a, Msg> {
    let t = &st.tokens;
    let t2 = *t;

    let mut swatches = row![].spacing(POP_GAP);
    for c in QUEUE_PRESETS {
        let col = iced::Color::from_rgb8(c[0], c[1], c[2]);
        let on = st.color == Some(c);
        swatches = swatches.push(
            button(iced::widget::Space::new())
                .width(Length::Fixed(POP_SWATCH))
                .height(Length::Fixed(POP_SWATCH))
                .padding(0.0)
                .on_press(Msg::ColorPick(c))
                .style(move |_th, status| iced::widget::button::Style {
                    background: Some(col.into()),
                    text_color: t2.fg_1,
                    border: iced::Border {
                        // `.q-swatch.on` ring is fg-1; hover previews it.
                        color: if on || matches!(status, iced::widget::button::Status::Hovered) {
                            t2.fg_1
                        } else {
                            iced::Color::TRANSPARENT
                        },
                        width: POP_BORDER,
                        radius: theme::control::RADIUS.into(),
                    },
                    shadow: iced::Shadow::default(),
                    snap: true,
                }),
        );
    }

    let pop = container(
        column![
            swatches,
            TextInput::new(&st.color_hex)
                .hint("#C9703F")
                .mono()
                .on_input(Msg::ColorHex)
                .view(t),
        ]
        .spacing(POP_GAP),
    )
    .padding(POP_PAD)
    .style(move |_| container::Style {
        background: Some(t2.bg_surface.into()),
        border: iced::Border {
            color: t2.border_default,
            width: POP_BORDER,
            radius: theme::radius::SM.into(),
        },
        shadow: iced::Shadow {
            color: color::with_alpha(iced::Color::BLACK, 80.0 / 255.0),
            offset: iced::Vector::new(0.0, 4.0),
            blur_radius: 16.0,
        },
        ..Default::default()
    });

    let scrim = mouse_area(
        container(iced::widget::Space::new())
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .on_press(Msg::ColorClose)
    .on_right_press(Msg::ColorClose);

    // Content-derived popup extent for the window-edge clamp.
    let mw = 9.0 * POP_SWATCH + 8.0 * POP_GAP + 2.0 * (POP_PAD + POP_BORDER);
    let mh = POP_SWATCH + POP_GAP + theme::control::H_MD + 2.0 * (POP_PAD + POP_BORDER);
    // Anchor just below the swatch button. Its position is fixed by
    // layout (editor column starts after the list at LIST_W, padded by
    // S4; the swatch is the first element of the head row), so the
    // anchor is derived from those constants rather than the cursor —
    // pointer-position capture races synthetic/fast clicks.
    let cx = LIST_W + theme::space::S4;
    let cy = theme::space::S4 + COLOR_BTN + POP_GAP;
    let (ww, wh) = if st.win_size.0 > 0.0 {
        (st.win_size.0, st.win_size.1 - titlebar::HEIGHT - 1.0)
    } else {
        (WIN_DEFAULT_W, WIN_DEFAULT_H - titlebar::HEIGHT - 1.0)
    };
    let left = cx.min(ww - mw).max(0.0);
    let top = cy.min(wh - mh).max(0.0);
    iced::widget::stack![
        base,
        scrim,
        container(iced::widget::opaque(pop)).padding(iced::Padding {
            left,
            top,
            ..Default::default()
        }),
    ]
    .into()
}

/// Centered month-calendar popup for the One-off date input. Centered
/// (not anchored) because the schedule card's position shifts with the
/// editor's scroll offset.
fn calendar_overlay<'a>(st: &'a State, base: Element<'a, Msg>) -> Element<'a, Msg> {
    use chrono::Datelike;
    let t = &st.tokens;
    let t2 = *t;
    let (year, month) = st.cal_ym;
    let selected = parse_once_date(&st.once_date);
    let today = chrono::Local::now().date_naive();

    let nav = |icon_name: &'static str, delta: i32| {
        button(icons::icon(icon_name, 14.0, t2.fg_2))
            .padding(4.0)
            .on_press(Msg::CalMonth(delta))
            .style(move |_th, status| iced::widget::button::Style {
                background: Some(
                    if matches!(status, iced::widget::button::Status::Hovered) {
                        t2.bg_sunken_hover
                    } else {
                        iced::Color::TRANSPARENT
                    }
                    .into(),
                ),
                text_color: t2.fg_2,
                border: iced::Border {
                    radius: theme::radius::XS.into(),
                    ..Default::default()
                },
                shadow: iced::Shadow::default(),
                snap: true,
            })
    };
    let month_name = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ][(month - 1) as usize];
    let head = row![
        nav("chevron-left", -1),
        text(format!("{month_name} {year}"))
            .font(theme::BODY_BOLD)
            .size(13.0)
            .color(t.fg_1)
            .width(Length::Fill)
            .center(),
        nav("chevron-right", 1),
    ]
    .align_y(Alignment::Center);

    let mut weekdays = row![].spacing(CAL_GAP);
    for d in ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"] {
        weekdays = weekdays.push(
            container(text(d).font(theme::BODY).size(11.0).color(t.fg_3))
                .width(Length::Fixed(CAL_CELL))
                .align_x(Alignment::Center),
        );
    }

    let first = chrono::NaiveDate::from_ymd_opt(year, month, 1)
        .unwrap_or_else(|| today.with_day(1).unwrap_or(today));
    let lead = first.weekday().num_days_from_monday() as usize;
    let days_in_month = (1..=31u32)
        .rev()
        .find_map(|d| chrono::NaiveDate::from_ymd_opt(year, month, d).map(|_| d))
        .unwrap_or(28);

    let mut grid = column![].spacing(CAL_GAP);
    let mut cells: Vec<Element<'_, Msg>> = Vec::new();
    for _ in 0..lead {
        cells.push(
            iced::widget::Space::new()
                .width(Length::Fixed(CAL_CELL))
                .height(Length::Fixed(CAL_CELL))
                .into(),
        );
    }
    for day in 1..=days_in_month {
        let date = first.with_day(day).unwrap_or(first);
        let is_sel = selected == Some(date);
        let is_today = date == today;
        cells.push(
            button(
                container(text(day.to_string()).font(theme::BODY).size(12.0))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(Alignment::Center)
                    .align_y(Alignment::Center),
            )
            .width(Length::Fixed(CAL_CELL))
            .height(Length::Fixed(CAL_CELL))
            .padding(0.0)
            .on_press(Msg::CalPick(date))
            .style(move |_th, status| {
                let hovered = matches!(status, iced::widget::button::Status::Hovered);
                iced::widget::button::Style {
                    background: Some(
                        if is_sel {
                            t2.action_primary
                        } else if hovered {
                            t2.bg_sunken_hover
                        } else {
                            iced::Color::TRANSPARENT
                        }
                        .into(),
                    ),
                    text_color: if is_sel {
                        t2.action_primary_fg
                    } else {
                        t2.fg_1
                    },
                    border: iced::Border {
                        color: if is_today && !is_sel {
                            t2.border_brand
                        } else {
                            iced::Color::TRANSPARENT
                        },
                        width: 1.0,
                        radius: theme::radius::XS.into(),
                    },
                    shadow: iced::Shadow::default(),
                    snap: true,
                }
            })
            .into(),
        );
    }
    let mut cells = cells.into_iter();
    loop {
        let week: Vec<Element<'_, Msg>> = cells.by_ref().take(7).collect();
        if week.is_empty() {
            break;
        }
        let mut r = row![].spacing(CAL_GAP);
        for c in week {
            r = r.push(c);
        }
        grid = grid.push(r);
    }

    let card = container(
        column![head, weekdays, grid]
            .spacing(theme::space::S2)
            .width(Length::Shrink),
    )
    .padding(CAL_PAD)
    .style(move |_| container::Style {
        background: Some(t2.bg_surface.into()),
        border: iced::Border {
            color: t2.border_default,
            width: 1.0,
            radius: theme::radius::SM.into(),
        },
        shadow: iced::Shadow {
            color: color::with_alpha(iced::Color::BLACK, 80.0 / 255.0),
            offset: iced::Vector::new(0.0, 4.0),
            blur_radius: 16.0,
        },
        ..Default::default()
    });

    let scrim = mouse_area(
        container(iced::widget::Space::new())
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .on_press(Msg::CalClose)
    .on_right_press(Msg::CalClose);

    iced::widget::stack![
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

fn delete_overlay<'a>(st: &'a State, base: Element<'a, Msg>) -> Element<'a, Msg> {
    let t = &st.tokens;
    let t2 = *t;
    let name = st
        .selected_queue()
        .map(|q| q.name.clone())
        .unwrap_or_default();
    let n_jobs = st.selected_queue().map(|q| q.job_ids.len()).unwrap_or(0);
    let card = container(
        column![
            text(format!("Delete queue \"{name}\"?"))
                .font(theme::BODY_BOLD)
                .size(14.0)
                .color(t.fg_1),
            text(format!(
                "{n_jobs} job(s) will become queueless. Files on disk are not touched."
            ))
            .font(theme::BODY)
            .size(12.0)
            .color(t.fg_2),
            row![
                iced::widget::Space::new().width(Length::Fill),
                Btn::new("Cancel")
                    .ghost()
                    .on_press(Msg::DeleteCancel)
                    .view(t),
                Btn::new("Delete")
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
    .width(Length::Fixed(380.0))
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
        mouse_area(
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

    iced::widget::stack![
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

pub fn launch_queues() {
    let mut app = iced::application(boot, update, view)
        .title(|_: &App| "oxdm — Queues & scheduling".to_owned())
        .theme(|app: &App| match app {
            App::Ready(st) => st.tokens.iced_theme(),
            _ => Tokens::dark().iced_theme(),
        })
        .subscription(subscription)
        .default_font(theme::BODY)
        .antialiasing(true)
        .window(chrome::window_settings(
            iced::Size::new(WIN_DEFAULT_W, WIN_DEFAULT_H),
            iced::Size::new(640.0, 518.0),
        ));
    for f in theme::fonts::ALL {
        app = app.font(*f);
    }
    if let Err(e) = app.run() {
        eprintln!("gui error: {e}");
        std::process::exit(1);
    }
}

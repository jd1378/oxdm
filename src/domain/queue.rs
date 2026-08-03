//! Queue domain types.
//!
//! A `Queue` is an ordered, schedulable group of `Job`s. Every job
//! belongs to exactly one queue; the built-in **Main** queue is created
//! at boot and cannot be deleted. Schedules + hooks let users automate
//! "start at 02:00, run until 06:00, shutdown when finished."

use chrono::{DateTime, Local, NaiveTime};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::{JobId, ShutdownAction};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct QueueId(pub Uuid);

impl QueueId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for QueueId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for QueueId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Queue {
    pub id: QueueId,
    pub name: String,
    /// `true` for the built-in Main queue. UI hides delete affordance.
    #[serde(default)]
    pub builtin: bool,
    /// Ordered list of jobs assigned to this queue.
    pub job_ids: Vec<JobId>,
    pub schedule: QueueSchedule,
    pub on_start: Vec<QueueHook>,
    pub on_finish: Vec<QueueHook>,
    /// `None` = inherit `Settings::max_concurrent_downloads`.
    pub max_concurrent: Option<usize>,
    /// If `true`, a job error halts the queue's auto-start sequence.
    pub stop_on_error: bool,
    /// User-chosen sRGB swatch. `None` falls back to a name-derived hue.
    #[serde(default)]
    pub color: Option<[u8; 3]>,
}

impl Queue {
    pub const MAIN_NAME: &'static str = "Main";

    /// Downloads a queue runs at once unless it is told otherwise. Its
    /// own number, not a share of `Settings::max_concurrent_downloads` —
    /// that one caps every queue together.
    pub const DEFAULT_CONCURRENT: usize = 3;

    pub fn new_main() -> Self {
        Self {
            id: QueueId::new(),
            name: Self::MAIN_NAME.into(),
            builtin: true,
            job_ids: Vec::new(),
            schedule: QueueSchedule::Manual,
            on_start: Vec::new(),
            on_finish: Vec::new(),
            max_concurrent: Some(Self::DEFAULT_CONCURRENT),
            stop_on_error: false,
            color: None,
        }
    }
}

/// Pick a vivid, saturated sRGB triple suitable for queue accents.
pub fn random_vivid_color() -> [u8; 3] {
    use rand::Rng;
    let mut rng = rand::rng();
    let hue: f32 = rng.random_range(0.0..360.0);
    let sat: f32 = rng.random_range(0.65..0.85);
    let val: f32 = rng.random_range(0.85..1.0);
    hsv_to_srgb(hue, sat, val)
}

fn hsv_to_srgb(h: f32, s: f32, v: f32) -> [u8; 3] {
    let c = v * s;
    let hh = (h.rem_euclid(360.0)) / 60.0;
    let x = c * (1.0 - (hh.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match hh as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [
        ((r + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((g + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((b + m) * 255.0).round().clamp(0.0, 255.0) as u8,
    ]
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[derive(Default)]
pub enum QueueSchedule {
    #[default]
    Manual,
    Daily {
        start: NaiveTime,
        stop: Option<NaiveTime>,
        days: WeekDayMask,
    },
    Once {
        start: DateTime<Local>,
        stop: Option<DateTime<Local>>,
    },
    /// Run while the enabled conditions hold; the scheduler
    /// re-evaluates every tick, so the queue pauses when they lapse.
    Condition(CondSet),
}

/// The condition builder's state: which of the four conditions are
/// enabled (plus their parameters) and how verdicts combine. Mirrors
/// the design's `schedule: { mode: 'condition', combine, conditions }`
/// shape. No condition enabled ⇒ the queue never starts on its own
/// (the builder's empty-state copy says exactly that).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CondSet {
    #[serde(default)]
    pub combine: CondCombine,
    /// Active connection is not marked metered (NetworkManager).
    #[serde(default)]
    pub unmetered: bool,
    /// On mains power, or no discharging battery is present.
    #[serde(default)]
    pub ac_power: bool,
    /// `Some(minutes)` = enabled: no input activity for that long
    /// (as reported by the session manager's idle hint).
    #[serde(default)]
    pub idle_minutes: Option<u16>,
    /// `Some` = enabled: poll a shell command, run while it exits 0.
    #[serde(default)]
    pub command: Option<CondCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CondCommand {
    pub cmd: String,
    /// Re-check period in seconds; the scheduler's tick floors the
    /// effective rate.
    pub interval_secs: u32,
}

/// Bounds from the design's number inputs (queue-dialog.jsx).
pub const IDLE_MINUTES_RANGE: std::ops::RangeInclusive<u16> = 1..=480;
pub const CMD_INTERVAL_RANGE: std::ops::RangeInclusive<u32> = 5..=3600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CondCombine {
    #[default]
    All,
    Any,
}

/// Identity of one builder condition, for capability gating and
/// verdict plumbing. Serialized in the IPC snapshot (the daemon tells
/// GUIs which conditions this host can actually evaluate).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CondKind {
    Unmetered,
    Idle,
    AcPower,
    Command,
}

impl CondKind {
    /// Conditions this build can honestly evaluate — the UI hides the
    /// rest (per-platform gating, matching the mock's
    /// `platformSupportsMetered` comment). Probes are Linux-only today
    /// (NetworkManager D-Bus, sysfs power_supply, logind idle hint);
    /// the command poll works anywhere with a POSIX shell. A saved
    /// condition from another platform still deserializes — its
    /// unsupported probes fail open.
    pub const SUPPORTED: &'static [CondKind] = {
        #[cfg(target_os = "linux")]
        {
            &[
                CondKind::Unmetered,
                CondKind::Idle,
                CondKind::AcPower,
                CondKind::Command,
            ]
        }
        #[cfg(all(unix, not(target_os = "linux")))]
        {
            &[CondKind::Command]
        }
        #[cfg(not(unix))]
        {
            &[]
        }
    };
}

impl CondSet {
    /// Enabled conditions, in the builder's display order.
    pub fn enabled(&self) -> Vec<CondKind> {
        let mut v = Vec::new();
        if self.unmetered {
            v.push(CondKind::Unmetered);
        }
        if self.idle_minutes.is_some() {
            v.push(CondKind::Idle);
        }
        if self.ac_power {
            v.push(CondKind::AcPower);
        }
        if self.command.is_some() {
            v.push(CondKind::Command);
        }
        v
    }

    /// Combine per-condition verdicts, considering only conditions in
    /// `available` — a condition this host cannot evaluate (hidden in
    /// the UI) does not participate at all. Nothing enabled *and
    /// available* ⇒ `false` (a queue with no live condition never
    /// starts on its own).
    pub fn holds(&self, available: &[CondKind], verdict: impl Fn(CondKind) -> bool) -> bool {
        let enabled: Vec<CondKind> = self
            .enabled()
            .into_iter()
            .filter(|k| available.contains(k))
            .collect();
        if enabled.is_empty() {
            return false;
        }
        match self.combine {
            CondCombine::All => enabled.into_iter().all(verdict),
            CondCombine::Any => enabled.into_iter().any(verdict),
        }
    }
}

/// Bitmask of weekdays. Bit 0 = Monday … bit 6 = Sunday.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct WeekDayMask(pub u8);

impl WeekDayMask {
    pub const ALL: Self = Self(0b0111_1111);
    pub const WEEKDAYS: Self = Self(0b0001_1111);
    pub const WEEKEND: Self = Self(0b0110_0000);

    pub fn contains(self, weekday: chrono::Weekday) -> bool {
        let bit = weekday.num_days_from_monday();
        (self.0 >> bit) & 1 == 1
    }

    pub fn set(&mut self, weekday: chrono::Weekday, on: bool) {
        let bit = weekday.num_days_from_monday();
        if on {
            self.0 |= 1 << bit;
        } else {
            self.0 &= !(1 << bit);
        }
    }
}

/// Action fired before the first job runs (`on_start`) or after the last
/// running job finishes (`on_finish`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QueueHook {
    Shutdown(ShutdownAction),
    Sleep,
    Hibernate,
    ExitOxdm,
    RunCommand {
        cmd: String,
        #[serde(default)]
        args: Vec<String>,
    },
    Notify {
        title: String,
        body: String,
    },
}

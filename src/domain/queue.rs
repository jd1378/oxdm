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

    /// Stored title of the default finish hook. Shared with the queues
    /// editor, which rebuilds the hook when the user switches its kind:
    /// a second spelling here would read as an edit the moment the
    /// window opened.
    pub const FINISH_NOTIFY_TITLE: &'static str = "Queue finished";

    /// Both strings are placeholders — when the hook fires,
    /// `data::hooks` replaces them with [`finish_title`] and
    /// [`finish_summary`], which know the queue and how the run went.
    pub fn finish_notify() -> QueueHook {
        QueueHook::Notify {
            title: Self::FINISH_NOTIFY_TITLE.into(),
            body: String::new(),
        }
    }

    pub fn new_main() -> Self {
        Self {
            id: QueueId::new(),
            name: Self::MAIN_NAME.into(),
            builtin: true,
            job_ids: Vec::new(),
            schedule: QueueSchedule::Manual,
            on_start: Vec::new(),
            // Telling the user their queue is done is the useful
            // default; every other finish action (sleep, shutdown, run
            // a command) is a deliberate choice.
            on_finish: vec![Self::finish_notify()],
            max_concurrent: Some(Self::DEFAULT_CONCURRENT),
            stop_on_error: false,
            color: None,
        }
    }
}

/// Pick a vivid, saturated sRGB triple suitable for queue accents.
pub fn random_vivid_color() -> [u8; 3] {
    use rand::RngExt;
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
    /// `Some(minutes)` = enabled: no input activity for that long, as
    /// the platform reports it (logind's idle hint, Quartz's
    /// seconds-since-last-event, `GetLastInputInfo`).
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
    /// `platformSupportsMetered` comment). What each platform can
    /// answer, and why the gaps are gaps:
    ///
    /// - **Idle** — everywhere: logind's idle hint, Quartz's
    ///   seconds-since-last-event, `GetLastInputInfo`. Still runtime-
    ///   gated on top of this: a Linux session with no logind is a
    ///   build that supports idle running on a host that cannot report
    ///   it, and `conditions::available_conditions` drops it there.
    /// - **AC power** — Linux (sysfs) and Windows
    ///   (`GetSystemPowerStatus`). macOS can answer "on mains"
    ///   (`IOPSGetTimeRemainingEstimate`) but not "has a battery", and
    ///   without the second question the condition is trivially true on
    ///   every desktop, so it stays hidden.
    /// - **Unmetered** — Linux (NetworkManager's `Metered` property) and
    ///   Windows (WinRT `NetworkInformation.GetConnectionCost`). The
    ///   macOS answer lives behind `NWPathMonitor.isExpensive`, which
    ///   is only reachable through an Objective-C block on a dispatch
    ///   queue — two more dependencies for one boolean.
    /// - **Command** — anywhere there is a shell, which now includes
    ///   Windows via `cmd /C`.
    ///
    /// A saved condition from another platform still deserializes; its
    /// unsupported kinds simply do not participate.
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
        #[cfg(target_os = "windows")]
        {
            &[
                CondKind::Unmetered,
                CondKind::Idle,
                CondKind::AcPower,
                CondKind::Command,
            ]
        }
        #[cfg(target_os = "macos")]
        {
            &[CondKind::Idle, CondKind::Command]
        }
        #[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
        {
            &[CondKind::Command]
        }
        #[cfg(not(any(unix, target_os = "windows")))]
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

/// Title of a queue-finished notification. The queue name leads because
/// notification surfaces that collapse or truncate keep the title and
/// drop the body — a generic "Queue finished" there says nothing about
/// which one.
pub fn finish_title(queue: &str) -> String {
    format!("{queue} finished")
}

/// What a queue run produced, as one sentence. Pairs with
/// [`finish_title`], which already carries the queue name, so this does
/// not repeat it.
///
/// Failures are named because a queue that "finished" with downloads
/// still broken has not done what the user asked, and a bare total
/// would hide that.
///
/// Downloads waiting on a question are named separately, and last,
/// because they are the part with something to do about them: nothing
/// went wrong, and each will run as soon as it is answered. This
/// notification is the only word a queue run gets — its downloads
/// raise no windows and no notifications of their own — so anything
/// left undone has to be in this sentence.
pub fn finish_summary(completed: u32, failed: u32, needs_answer: u32) -> String {
    let files = |n: u32| if n == 1 { " file" } else { " files" };
    let mut parts: Vec<String> = Vec::new();
    // Only the first clause carries the noun — "3 files downloaded, 2
    // files failed" says "files" twice about the same pile.
    let noun = |n: u32, first: bool| if first { files(n) } else { "" };
    if completed > 0 {
        parts.push(format!(
            "{completed}{} downloaded",
            noun(completed, parts.is_empty())
        ));
    }
    if failed > 0 {
        parts.push(format!("{failed}{} failed", noun(failed, parts.is_empty())));
    }
    if needs_answer > 0 {
        let verb = if needs_answer == 1 { "needs" } else { "need" };
        parts.push(format!(
            "{needs_answer}{} {verb} your answer",
            noun(needs_answer, parts.is_empty())
        ));
    }
    if parts.is_empty() {
        return "Nothing was downloaded.".to_owned();
    }
    format!("{}.", parts.join(", "))
}

#[cfg(test)]
mod finish_summary_tests {
    use super::{finish_summary, finish_title};

    #[test]
    fn title_names_the_queue() {
        assert_eq!(finish_title("Main"), "Main finished");
    }

    #[test]
    fn reports_both_counts_and_singularises() {
        assert_eq!(finish_summary(4, 0, 0), "4 files downloaded.");
        assert_eq!(finish_summary(1, 0, 0), "1 file downloaded.");
        assert_eq!(finish_summary(3, 2, 0), "3 files downloaded, 2 failed.");
        assert_eq!(finish_summary(0, 1, 0), "1 file failed.");
        // A queue stopped before anything finished says so rather than
        // claiming success.
        assert_eq!(finish_summary(0, 0, 0), "Nothing was downloaded.");
    }

    /// A queued download stopped on a question raises nothing of its
    /// own, so this sentence is where the user hears about it — and
    /// calling it a failure would be both wrong and unactionable.
    #[test]
    fn what_is_waiting_on_the_user_is_not_called_a_failure() {
        assert_eq!(finish_summary(0, 0, 1), "1 file needs your answer.");
        assert_eq!(finish_summary(0, 0, 2), "2 files need your answer.");
        assert_eq!(
            finish_summary(5, 1, 2),
            "5 files downloaded, 1 failed, 2 need your answer."
        );
        assert_eq!(
            finish_summary(3, 0, 1),
            "3 files downloaded, 1 needs your answer."
        );
    }
}

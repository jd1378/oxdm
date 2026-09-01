//! Per-queue start / stop scheduler.
//!
//! Single tokio task that wakes every 30 s and asks each queue whether
//! it should be running right now. Transitions are edge-triggered: a
//! queue that should be running but has no active jobs gets `start_queue`
//! called once; one that should be stopped gets `stop_queue`. Hooks fire
//! from `data::hooks` off `QueueStarted` / `QueueFinished` events.
//!
//! Condition schedules: shared probes (metered / AC) run once per tick
//! via `conditions::probe` and idleness comes from the daemon-wide
//! [`crate::data::idle`] watch; command conditions are polled per-queue
//! at their configured interval (floored to the tick) with the last
//! verdict cached in [`CmdPoll`].
//!
//! Every condition is symmetric: the tick that sees the user come back
//! sees `idle` fall to zero, the verdict flips, and the edge calls
//! `stop_queue` — an idle-only queue does not keep running over the
//! session it was waiting to stay out of. The same holds for a link
//! that turns metered or a laptop unplugged mid-run.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{Datelike, Local, NaiveTime};

use crate::data::conditions::{self, CondSnapshot};
use crate::data::state::AppState;
use crate::domain::{CondKind, Queue, QueueId, QueueSchedule, WeekDayMask};

const TICK: Duration = Duration::from_secs(30);

/// How long a one-shot schedule with no end time stays due after its
/// start. Long enough that a machine asleep at the appointed hour still
/// runs the queue when it wakes; short enough that a launch days later
/// does not resurrect it.
const ONESHOT_WINDOW_HOURS: i64 = 12;

/// How long a recurring occurrence stays due after the moment it names.
/// Wide enough for a tick that lands late and for a daemon restarted
/// just after the hour; narrow enough that the queue is starting at the
/// time the user picked and not merely some time afterwards.
const DAILY_GRACE_MINUTES: i64 = 15;

/// Cached verdict of one queue's condition command.
struct CmdPoll {
    /// The command the verdict belongs to; a config edit invalidates it.
    cmd: String,
    checked: Instant,
    ok: bool,
}

pub fn spawn(state: Arc<AppState>, idle: crate::data::idle::IdleWatch) {
    tokio::spawn(async move {
        // last_active[q] = previous tick's "should be running" verdict.
        // Edge transitions drive start/stop; same-state ticks are no-ops.
        let mut last_active: HashMap<QueueId, bool> = HashMap::new();
        let mut cmd_polls: HashMap<QueueId, CmdPoll> = HashMap::new();
        loop {
            // A scheduled window opening during shutdown would start
            // the very downloads the exit just paused.
            if state.is_exiting() {
                return;
            }
            let queues = state.queues_snapshot().await;
            // Runtime capability set (e.g. AcPower only with a battery
            // present and a probe that answers); unavailable conditions
            // are neither probed nor evaluated — they simply don't
            // participate. Same source the queue builder is shown, so a
            // condition on offer is one the scheduler can decide.
            let available = conditions::available_conditions(state.cond_support());
            // Probe only the conditions some queue actually uses;
            // anything else is left unread, which reads as "does not
            // hold" — and cannot bind, since it is not enabled.
            let needed: HashSet<CondKind> = queues
                .iter()
                .filter_map(|q| match &q.schedule {
                    QueueSchedule::Condition(set) => Some(set.enabled()),
                    _ => None,
                })
                .flatten()
                .filter(|k| available.contains(k))
                .collect();
            // Sampling costs a bus round trip, so it only runs while
            // some queue is actually waiting on it. Withdrawing the
            // need parks the poller and drops the last reading, so the
            // first tick after a queue re-enables the condition sees no
            // answer and holds — one tick, and it fails closed.
            idle.want(
                crate::data::idle::Want::Queues,
                needed.contains(&CondKind::Idle),
            );
            let conds = conditions::probe(&needed, idle.current()).await;
            if available.contains(&CondKind::Command) {
                poll_due_commands(&queues, &mut cmd_polls).await;
            }
            let now = Local::now();
            for q in &queues {
                let cmd_ok = cmd_polls.get(&q.id).map(|p| p.ok);
                let active = should_run(q, now, &available, &conds, cmd_ok);
                let prev = last_active.get(&q.id).copied().unwrap_or(false);
                if active && !prev {
                    if let Err(e) = state.start_queue(q.id).await {
                        tracing::warn!(queue = %q.name, error = %e, "scheduler start_queue");
                    }
                } else if !may_continue(q, now, &available, &conds, cmd_ok) {
                    // Two ways to be running: the tick started it (the
                    // edge below remembers that), or an arriving
                    // download did. The second kind is invisible to
                    // `last_active`, so it is asked about directly —
                    // and only for queues whose trigger can start
                    // them, so a queue the *user* started by hand is
                    // never stopped from here.
                    let by_trigger = matches!(
                        &q.schedule,
                        QueueSchedule::Condition(set) if set.on_job_added
                    ) && state.is_queue_active(q.id).await;
                    if (prev || by_trigger)
                        && let Err(e) = state.stop_queue(q.id).await
                    {
                        tracing::warn!(queue = %q.name, error = %e, "scheduler stop_queue");
                    }
                }
                last_active.insert(q.id, active);
            }
            // Drop entries for deleted queues so the maps do not grow.
            let live: HashSet<QueueId> = queues.iter().map(|q| q.id).collect();
            last_active.retain(|k, _| live.contains(k));
            cmd_polls.retain(|k, _| live.contains(k));
            tokio::time::sleep(TICK).await;
        }
    });
}

/// Re-run each queue's condition command when its interval has elapsed
/// (or the command text changed). Runs concurrently across queues;
/// each command is already timeout-bounded in `check_command`.
async fn poll_due_commands(queues: &[Queue], polls: &mut HashMap<QueueId, CmdPoll>) {
    let mut due: Vec<(QueueId, String)> = Vec::new();
    for q in queues {
        let QueueSchedule::Condition(set) = &q.schedule else {
            continue;
        };
        let Some(cc) = &set.command else { continue };
        let fresh = polls.get(&q.id).is_some_and(|p| {
            p.cmd == cc.cmd
                && p.checked.elapsed() < Duration::from_secs(u64::from(cc.interval_secs)).max(TICK)
        });
        if !fresh {
            due.push((q.id, cc.cmd.clone()));
        }
    }
    let checks = due.into_iter().map(|(id, cmd)| async move {
        let ok = conditions::check_command(&cmd).await;
        (id, cmd, ok)
    });
    for (id, cmd, ok) in futures::future::join_all(checks).await {
        polls.insert(
            id,
            CmdPoll {
                cmd,
                checked: Instant::now(),
                ok,
            },
        );
    }
}

/// Is a clock-scheduled queue due right now?
///
/// Asked by the state layer when a download lands in a queue that is
/// not running: a job added at 02:15 to a queue scheduled 02:00–04:00
/// belongs to the run that is already meant to be happening, and
/// waiting for the next tick — or, if the run had already started and
/// finished its work, for tomorrow — is not what the schedule says.
///
/// A recurring schedule with no end time is due only around the moment
/// it names, so a job added at nine in the evening to a queue set for
/// 02:00 waits for 02:00, which is the whole point of setting it.
pub fn within_window(q: &Queue, now: chrono::DateTime<Local>) -> bool {
    match &q.schedule {
        QueueSchedule::Daily { .. } | QueueSchedule::Once { .. } => {
            verdict(q, now, &[], &CondSnapshot::default(), None, false)
        }
        _ => false,
    }
}

/// `cmd_ok`: cached verdict of this queue's condition command, if it
/// has one (`None` before the first poll completes ⇒ not met yet, like
/// every other unread condition).
fn should_run(
    q: &Queue,
    now: chrono::DateTime<Local>,
    available: &[CondKind],
    conds: &CondSnapshot,
    cmd_ok: Option<bool>,
) -> bool {
    verdict(q, now, available, conds, cmd_ok, false)
}

/// May a queue that is *already running* keep going?
///
/// The same conditions, read as a running queue sees them: a moment
/// that has already passed — "a download was added", "the clock
/// reached 02:00" — counts as satisfied, because it fired when the
/// queue started and cannot be re-observed. Without this, a queue
/// combining
/// `JobAdded` with `All` was never running as far as the tick was
/// concerned, so nothing ever stopped it — it kept downloading after
/// the link went metered or the machine went back on battery, which is
/// exactly what the other half of the combination was for.
fn may_continue(
    q: &Queue,
    now: chrono::DateTime<Local>,
    available: &[CondKind],
    conds: &CondSnapshot,
    cmd_ok: Option<bool>,
) -> bool {
    verdict(q, now, available, conds, cmd_ok, true)
}

fn verdict(
    q: &Queue,
    now: chrono::DateTime<Local>,
    available: &[CondKind],
    conds: &CondSnapshot,
    cmd_ok: Option<bool>,
    running: bool,
) -> bool {
    match &q.schedule {
        QueueSchedule::Manual => false,
        // Without a stop time the run lasts until the queue has
        // nothing left, so a queue already running is never stopped
        // from here.
        QueueSchedule::Daily { stop: None, .. } if running => true,
        QueueSchedule::Daily { start, stop, days } => daily_due(*start, *stop, *days, now),
        QueueSchedule::Once { start, stop } => {
            if now < *start {
                return false;
            }
            match stop {
                Some(stop) => now < *stop,
                // No end time: start the queue and let it run until it
                // has nothing left. `None` used to answer "not now"
                // both before and after `start`, which made the whole
                // one-shot option dead — the queue never ran, and
                // nothing said why.
                //
                // *Starting* is bounded by a window, because the
                // scheduler cannot remember across restarts that it
                // already fired: without it, every launch weeks later
                // would start a queue that was scheduled for one
                // afternoon. The window is wide enough to cover a
                // machine that was asleep when the time came. It is
                // not a deadline for the run it began — that ends when
                // the queue drains.
                None => running || now < *start + chrono::Duration::hours(ONESHOT_WINDOW_HOURS),
            }
        }
        QueueSchedule::Condition(set) => set.holds(available, |kind| match kind {
            // Never true when deciding whether to *start*: nothing was
            // added a moment ago, or this would not be a tick. The
            // queue starts from the event itself — see
            // `AppState::queue_took_a_job` — and with `All` this is
            // what stops the tick from starting a queue whose trigger
            // has not fired. `may_continue` passes true instead, so
            // the other conditions can still stop a running queue.
            CondKind::JobAdded => running,
            CondKind::Unmetered => conds.unmetered(),
            CondKind::AcPower => conds.on_ac(),
            CondKind::Idle => conds.idle_at_least(set.idle_minutes.unwrap_or(u16::MAX)),
            CondKind::Command => cmd_ok.unwrap_or(false),
        }),
    }
}

/// Is a recurring occurrence due right now?
///
/// An occurrence belongs to the day it starts on and lasts until its
/// stop time. With no stop time it is a moment rather than everything
/// after it: 02:00 means the queue begins when the clock reaches
/// 02:00, and reading it as "at or after 02:00" made a queue set for
/// the small hours start downloading the moment it was configured, at
/// nine in the evening. The short grace covers a tick that lands late;
/// a machine that slept through it runs the next occurrence instead.
///
/// Yesterday is scanned as well, so a window (or a grace) that reaches
/// past midnight is still the previous day's run: 23:00–02:00 on a
/// Sunday-only schedule is running at one on Monday morning, and a
/// Monday-only one is not.
fn daily_due(
    start: NaiveTime,
    stop: Option<NaiveTime>,
    days: WeekDayMask,
    now: chrono::DateTime<Local>,
) -> bool {
    let len = match stop {
        Some(stop) => {
            let span = stop.signed_duration_since(start);
            // Negative wraps past midnight; zero is a stop time equal
            // to the start, which reads as the whole day rather than
            // as no time at all.
            if span > chrono::Duration::zero() {
                span
            } else {
                span + chrono::Duration::days(1)
            }
        }
        None => chrono::Duration::minutes(DAILY_GRACE_MINUTES),
    };
    let n = now.naive_local();
    let today = now.date_naive();
    [today.pred_opt(), Some(today)]
        .into_iter()
        .flatten()
        .any(|d| {
            days.contains(d.weekday()) && {
                let occ = d.and_time(start);
                n >= occ && n < occ + len
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    use crate::domain::{CondCombine, CondCommand, CondSet};

    fn queue_with(schedule: QueueSchedule) -> Queue {
        let mut q = Queue::new_main();
        q.schedule = schedule;
        q
    }

    const ALL: &[CondKind] = &[
        CondKind::JobAdded,
        CondKind::Unmetered,
        CondKind::Idle,
        CondKind::AcPower,
        CondKind::Command,
    ];

    fn snap(unmetered: bool, on_ac: bool, idle_secs: u64) -> CondSnapshot {
        CondSnapshot::fixed(
            Some(unmetered),
            Some(on_ac),
            Some(Duration::from_secs(idle_secs)),
        )
    }

    /// The one-shot option was dead: `stop: None` answered "not now"
    /// at every tick, before and after the appointed time.
    #[test]
    fn a_one_off_schedule_with_no_end_time_fires() {
        let now = Local::now();
        let due = queue_with(QueueSchedule::Once {
            start: now - chrono::Duration::minutes(1),
            stop: None,
        });
        assert!(should_run(&due, now, ALL, &snap(true, true, 0), None));

        let not_yet = queue_with(QueueSchedule::Once {
            start: now + chrono::Duration::minutes(1),
            stop: None,
        });
        assert!(!should_run(&not_yet, now, ALL, &snap(true, true, 0), None));

        // Not resurrected days later by a fresh daemon.
        let stale = queue_with(QueueSchedule::Once {
            start: now - chrono::Duration::hours(ONESHOT_WINDOW_HOURS + 1),
            stop: None,
        });
        assert!(!should_run(&stale, now, ALL, &snap(true, true, 0), None));
    }

    #[test]
    fn a_one_off_schedule_with_an_end_time_is_its_window() {
        let now = Local::now();
        let inside = queue_with(QueueSchedule::Once {
            start: now - chrono::Duration::minutes(5),
            stop: Some(now + chrono::Duration::minutes(5)),
        });
        assert!(should_run(&inside, now, ALL, &snap(true, true, 0), None));

        let past = queue_with(QueueSchedule::Once {
            start: now - chrono::Duration::hours(2),
            stop: Some(now - chrono::Duration::hours(1)),
        });
        assert!(!should_run(&past, now, ALL, &snap(true, true, 0), None));
    }

    #[test]
    fn a_daily_window_runs_on_its_days_and_hours() {
        // A fixed date: "today at 02:00" is the hour the clocks skip
        // over in spring, and the test would stop existing twice a year.
        let at = |h, m| Local.with_ymd_and_hms(2026, 6, 8, h, m, 0).unwrap();
        let base = at(10, 0);
        let every_day = crate::domain::WeekDayMask::ALL;
        let q = queue_with(QueueSchedule::Daily {
            start: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            stop: Some(NaiveTime::from_hms_opt(11, 0, 0).unwrap()),
            days: every_day,
        });
        assert!(should_run(&q, base, ALL, &snap(true, true, 0), None));

        let before = at(8, 0);
        assert!(!should_run(&q, before, ALL, &snap(true, true, 0), None));

        // A window that wraps past midnight is inside at 23:30.
        let overnight = queue_with(QueueSchedule::Daily {
            start: NaiveTime::from_hms_opt(23, 0, 0).unwrap(),
            stop: Some(NaiveTime::from_hms_opt(2, 0, 0).unwrap()),
            days: every_day,
        });
        let late = at(23, 30);
        assert!(should_run(
            &overnight,
            late,
            ALL,
            &snap(true, true, 0),
            None
        ));
        assert!(!should_run(
            &overnight,
            base,
            ALL,
            &snap(true, true, 0),
            None
        ));
    }

    /// The reported bug: a queue set to run at 02:00 started
    /// downloading the moment it was saved at nine in the evening,
    /// because "no end time" was read as "every moment after 02:00".
    #[test]
    fn a_recurring_start_time_is_a_moment_not_the_rest_of_the_day() {
        let q = queue_with(QueueSchedule::Daily {
            start: NaiveTime::from_hms_opt(2, 0, 0).unwrap(),
            stop: None,
            days: crate::domain::WeekDayMask::ALL,
        });
        let s = snap(true, true, 0);
        // A fixed date, not "today": 02:00 is the hour the clocks skip
        // over, and the test would stop existing twice a year.
        let at = |h, m| Local.with_ymd_and_hms(2026, 6, 8, h, m, 0).unwrap();

        assert!(should_run(&q, at(2, 0), ALL, &s, None));
        assert!(should_run(&q, at(2, 14), ALL, &s, None)); // late tick
        assert!(!should_run(&q, at(1, 59), ALL, &s, None));
        assert!(!should_run(&q, at(2, 16), ALL, &s, None));
        assert!(!should_run(&q, at(21, 0), ALL, &s, None));

        // Nor does the state layer start it for an arriving download.
        assert!(!within_window(&q, at(21, 0)));
        assert!(within_window(&q, at(2, 5)));
    }

    /// Having started, the run lasts until the queue has nothing left:
    /// the schedule named a start, not a deadline, so the tick after
    /// the grace closes must not pull the downloads out from under it.
    #[test]
    fn a_recurring_run_with_no_end_time_is_never_stopped_by_the_clock() {
        let q = queue_with(QueueSchedule::Daily {
            start: NaiveTime::from_hms_opt(2, 0, 0).unwrap(),
            stop: None,
            days: crate::domain::WeekDayMask::ALL,
        });
        let s = snap(true, true, 0);
        assert!(may_continue(&q, Local::now(), ALL, &s, None));
    }

    /// A day the schedule does not name never comes due, and grace
    /// that runs past midnight still belongs to the day it started on.
    #[test]
    fn recurring_grace_belongs_to_the_day_the_occurrence_started() {
        let mask = |d: chrono::Weekday| {
            let mut m = crate::domain::WeekDayMask(0);
            m.set(d, true);
            m
        };
        // Anchor on a known weekday rather than "today".
        let base = Local
            .with_ymd_and_hms(2026, 6, 8, 23, 55, 0) // a Monday
            .unwrap();
        let monday_night = queue_with(QueueSchedule::Daily {
            start: NaiveTime::from_hms_opt(23, 55, 0).unwrap(),
            stop: None,
            days: mask(chrono::Weekday::Mon),
        });
        let s = snap(true, true, 0);
        assert!(should_run(&monday_night, base, ALL, &s, None));
        // 00:05 on Tuesday is still Monday's occurrence.
        assert!(should_run(
            &monday_night,
            base + chrono::Duration::minutes(10),
            ALL,
            &s,
            None
        ));
        // Tuesday's own 23:55 is not on the schedule.
        assert!(!should_run(
            &monday_night,
            base + chrono::Duration::days(1),
            ALL,
            &s,
            None
        ));
    }

    /// A stop time ends the run: the queue is started at 02:00 and
    /// stopped at 04:00 whether or not it has finished, which is the
    /// whole reason for offering one.
    #[test]
    fn a_recurring_stop_time_ends_the_run() {
        let q = queue_with(QueueSchedule::Daily {
            start: NaiveTime::from_hms_opt(2, 0, 0).unwrap(),
            stop: Some(NaiveTime::from_hms_opt(4, 0, 0).unwrap()),
            days: crate::domain::WeekDayMask::ALL,
        });
        let s = snap(true, true, 0);
        let at = |h, m| Local.with_ymd_and_hms(2026, 6, 8, h, m, 0).unwrap();

        assert!(!should_run(&q, at(1, 59), ALL, &s, None));
        assert!(should_run(&q, at(2, 0), ALL, &s, None));
        // Unlike a schedule with no end time, the whole window is due:
        // a daemon started at 03:00 joins the run it names.
        assert!(should_run(&q, at(3, 0), ALL, &s, None));
        assert!(!may_continue(&q, at(4, 0), ALL, &s, None));
        assert!(!should_run(&q, at(21, 0), ALL, &s, None));
    }

    /// A window that reaches past midnight belongs to the day it
    /// started on, not to the day it ends in.
    #[test]
    fn an_overnight_window_belongs_to_the_day_it_started_on() {
        let mut mon = crate::domain::WeekDayMask(0);
        mon.set(chrono::Weekday::Mon, true);
        let q = queue_with(QueueSchedule::Daily {
            start: NaiveTime::from_hms_opt(23, 0, 0).unwrap(),
            stop: Some(NaiveTime::from_hms_opt(2, 0, 0).unwrap()),
            days: mon,
        });
        let s = snap(true, true, 0);
        // 2026-06-08 is a Monday.
        let at = |d, h, m| Local.with_ymd_and_hms(2026, 6, d, h, m, 0).unwrap();

        assert!(should_run(&q, at(8, 23, 30), ALL, &s, None));
        assert!(should_run(&q, at(9, 1, 30), ALL, &s, None)); // Tuesday, 01:30
        assert!(!should_run(&q, at(9, 2, 0), ALL, &s, None)); // window closed
        assert!(!should_run(&q, at(9, 23, 30), ALL, &s, None)); // Tuesday night
    }

    /// The one-off's window bounds *starting*, not the run it began: a
    /// download still going twelve hours later is not pulled out from
    /// under the user by a schedule that named no end.
    #[test]
    fn a_one_off_with_no_end_time_is_never_stopped_by_the_clock() {
        let now = Local::now();
        let q = queue_with(QueueSchedule::Once {
            start: now - chrono::Duration::hours(ONESHOT_WINDOW_HOURS + 8),
            stop: None,
        });
        let s = snap(true, true, 0);
        assert!(!should_run(&q, now, ALL, &s, None));
        assert!(may_continue(&q, now, ALL, &s, None));
    }

    #[test]
    fn a_one_off_stop_time_ends_the_run() {
        let now = Local::now();
        let q = queue_with(QueueSchedule::Once {
            start: now - chrono::Duration::hours(2),
            stop: Some(now - chrono::Duration::minutes(1)),
        });
        let s = snap(true, true, 0);
        assert!(!may_continue(&q, now, ALL, &s, None));
    }

    #[test]
    fn single_condition_follows_its_verdict() {
        let now = Local::now();
        let q = queue_with(QueueSchedule::Condition(CondSet {
            unmetered: true,
            ..CondSet::default()
        }));
        assert!(should_run(&q, now, ALL, &snap(true, false, 0), None));
        assert!(!should_run(&q, now, ALL, &snap(false, true, 0), None));
    }

    #[test]
    fn combine_all_vs_any() {
        let now = Local::now();
        let mixed = snap(true, false, 0); // unmetered yes, AC no
        let all = queue_with(QueueSchedule::Condition(CondSet {
            unmetered: true,
            ac_power: true,
            combine: CondCombine::All,
            ..CondSet::default()
        }));
        let any = queue_with(QueueSchedule::Condition(CondSet {
            unmetered: true,
            ac_power: true,
            combine: CondCombine::Any,
            ..CondSet::default()
        }));
        assert!(!should_run(&all, now, ALL, &mixed, None));
        assert!(should_run(&any, now, ALL, &mixed, None));
    }

    #[test]
    fn idle_threshold_compares_minutes() {
        let now = Local::now();
        let q = queue_with(QueueSchedule::Condition(CondSet {
            idle_minutes: Some(10),
            ..CondSet::default()
        }));
        assert!(should_run(&q, now, ALL, &snap(true, true, 600), None));
        assert!(!should_run(&q, now, ALL, &snap(true, true, 599), None));
    }

    /// The verdict has to fall as well as rise: the tick after the user
    /// touches the keyboard reads zero idle time, and the scheduler's
    /// edge from true to false is what pauses the queue's downloads.
    #[test]
    fn a_queue_waiting_for_idle_stops_when_the_user_comes_back() {
        let now = Local::now();
        let q = queue_with(QueueSchedule::Condition(CondSet {
            idle_minutes: Some(10),
            ..CondSet::default()
        }));
        assert!(should_run(&q, now, ALL, &snap(true, true, 900), None));
        assert!(!should_run(&q, now, ALL, &snap(true, true, 0), None));
    }

    /// The trigger is an instant, and a tick is never that instant.
    /// With `All` it holds the whole set back until the event fires;
    /// with `Any` it simply contributes nothing, leaving the other
    /// conditions to decide.
    #[test]
    fn a_job_added_trigger_never_starts_a_queue_on_a_tick() {
        let now = Local::now();
        let idle_now = snap(true, true, 9999);

        let alone = queue_with(QueueSchedule::Condition(CondSet {
            on_job_added: true,
            ..CondSet::default()
        }));
        assert!(!should_run(&alone, now, ALL, &idle_now, None));

        // `All`: the queue waits for the event even though the other
        // half of the pair is true right now.
        let gated = queue_with(QueueSchedule::Condition(CondSet {
            on_job_added: true,
            idle_minutes: Some(10),
            combine: CondCombine::All,
            ..CondSet::default()
        }));
        assert!(!should_run(&gated, now, ALL, &idle_now, None));

        // `Any`: idle still starts it, as it would without the trigger.
        let either = queue_with(QueueSchedule::Condition(CondSet {
            on_job_added: true,
            idle_minutes: Some(10),
            combine: CondCombine::Any,
            ..CondSet::default()
        }));
        assert!(should_run(&either, now, ALL, &idle_now, None));
        assert!(!should_run(&either, now, ALL, &snap(true, true, 0), None));
    }

    #[test]
    fn command_uses_cached_verdict_and_defaults_to_not_met() {
        let now = Local::now();
        let q = queue_with(QueueSchedule::Condition(CondSet {
            command: Some(CondCommand {
                cmd: "true".into(),
                interval_secs: 60,
            }),
            ..CondSet::default()
        }));
        let s = snap(true, true, 0);
        assert!(!should_run(&q, now, ALL, &s, None)); // first poll pending
        assert!(should_run(&q, now, ALL, &s, Some(true)));
        assert!(!should_run(&q, now, ALL, &s, Some(false)));
    }

    #[test]
    fn no_enabled_condition_never_runs() {
        let q = queue_with(QueueSchedule::Condition(CondSet::default()));
        assert!(!should_run(
            &q,
            Local::now(),
            ALL,
            &snap(true, true, 9999),
            None
        ));
        assert!(!should_run(
            &queue_with(QueueSchedule::Manual),
            Local::now(),
            ALL,
            &snap(true, true, 9999),
            None
        ));
    }

    /// A tick where the probes came back empty starts nothing. Not one
    /// of these conditions means "run unless told otherwise": each
    /// names a moment that is cheap for the user, and no reading is not
    /// that moment. A host that can *never* read one does not offer it
    /// (see below), so this only covers the transient case.
    #[test]
    fn an_unread_condition_does_not_run() {
        let blank = CondSnapshot::default();
        for set in [
            CondSet {
                unmetered: true,
                ..CondSet::default()
            },
            CondSet {
                ac_power: true,
                ..CondSet::default()
            },
            CondSet {
                idle_minutes: Some(480),
                ..CondSet::default()
            },
            CondSet {
                unmetered: true,
                ac_power: true,
                idle_minutes: Some(480),
                combine: CondCombine::Any,
                ..CondSet::default()
            },
        ] {
            let q = queue_with(QueueSchedule::Condition(set));
            assert!(!should_run(&q, Local::now(), ALL, &blank, None));
        }
    }

    /// With no way to read a condition it is not on the menu, so a
    /// queue configured for it on another machine simply never runs
    /// here — rather than running on a guess.
    #[test]
    fn a_host_that_cannot_read_a_condition_does_not_offer_it() {
        let available = conditions::available_conditions(conditions::CondSupport::default());
        assert!(!available.contains(&CondKind::Idle));
        assert!(!available.contains(&CondKind::Unmetered));
        assert!(!available.contains(&CondKind::AcPower));

        let idle_only = queue_with(QueueSchedule::Condition(CondSet {
            idle_minutes: Some(10),
            ..CondSet::default()
        }));
        assert!(!should_run(
            &idle_only,
            Local::now(),
            &available,
            &snap(true, true, 9999),
            None
        ));
    }

    #[test]
    fn unavailable_conditions_do_not_participate() {
        let now = Local::now();
        // Foreign config: AC-power enabled, but this host has no
        // battery — AC alone must not start the queue.
        let ac_only = queue_with(QueueSchedule::Condition(CondSet {
            ac_power: true,
            ..CondSet::default()
        }));
        let no_battery: &[CondKind] = &[CondKind::Unmetered, CondKind::Idle, CondKind::Command];
        assert!(!should_run(
            &ac_only,
            now,
            no_battery,
            &snap(true, true, 0),
            None
        ));

        // Mixed with All: the unavailable AC condition drops out and
        // the remaining unmetered verdict decides.
        let both = queue_with(QueueSchedule::Condition(CondSet {
            unmetered: true,
            ac_power: true,
            combine: CondCombine::All,
            ..CondSet::default()
        }));
        assert!(should_run(
            &both,
            now,
            no_battery,
            &snap(true, false, 0),
            None
        ));
        assert!(!should_run(
            &both,
            now,
            no_battery,
            &snap(false, true, 0),
            None
        ));
    }
}

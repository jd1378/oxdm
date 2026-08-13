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

use chrono::{Datelike, Local, NaiveTime, Timelike};

use crate::data::conditions::{self, CondSnapshot};
use crate::data::state::AppState;
use crate::domain::{CondKind, Queue, QueueId, QueueSchedule};

const TICK: Duration = Duration::from_secs(30);

/// How long a one-shot schedule with no end time stays due after its
/// start. Long enough that a machine asleep at the appointed hour still
/// runs the queue when it wakes; short enough that a launch days later
/// does not resurrect it.
const ONESHOT_WINDOW_HOURS: i64 = 12;

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

/// Is a clock-scheduled queue inside its window right now?
///
/// Asked by the state layer when a download lands in a queue that is
/// not running: a job added at 02:15 to a queue scheduled 02:00–04:00
/// belongs to the run that is already meant to be happening, and
/// waiting for the next tick — or, if the run had already started and
/// finished its work, for tomorrow — is not what the schedule says.
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
/// The same conditions, except that "a download was added" counts as
/// satisfied: it fired when the queue started, and a moment that has
/// passed cannot be re-observed. Without this, a queue combining
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
    job_added_holds: bool,
) -> bool {
    match &q.schedule {
        QueueSchedule::Manual => false,
        QueueSchedule::Daily { start, stop, days } => {
            if !days.contains(now.weekday()) {
                return false;
            }
            let n = current_naive_time(now);
            match stop {
                Some(stop) if stop > start => n >= *start && n < *stop,
                Some(stop) => n >= *start || n < *stop, // wraps midnight
                None => n >= *start,
            }
        }
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
                // Bounded by a window rather than open-ended, because
                // the scheduler cannot remember across restarts that it
                // already fired: without it, every launch weeks later
                // would start a queue that was scheduled for one
                // afternoon. The window is wide enough to cover a
                // machine that was asleep when the time came.
                None => now < *start + chrono::Duration::hours(ONESHOT_WINDOW_HOURS),
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
            CondKind::JobAdded => job_added_holds,
            CondKind::Unmetered => conds.unmetered(),
            CondKind::AcPower => conds.on_ac(),
            CondKind::Idle => conds.idle_at_least(set.idle_minutes.unwrap_or(u16::MAX)),
            CondKind::Command => cmd_ok.unwrap_or(false),
        }),
    }
}

fn current_naive_time(now: chrono::DateTime<Local>) -> NaiveTime {
    NaiveTime::from_hms_opt(now.hour(), now.minute(), now.second()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let base = Local::now()
            .with_hour(10)
            .unwrap()
            .with_minute(0)
            .unwrap()
            .with_second(0)
            .unwrap();
        let every_day = crate::domain::WeekDayMask::ALL;
        let q = queue_with(QueueSchedule::Daily {
            start: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            stop: Some(NaiveTime::from_hms_opt(11, 0, 0).unwrap()),
            days: every_day,
        });
        assert!(should_run(&q, base, ALL, &snap(true, true, 0), None));

        let before = base.with_hour(8).unwrap();
        assert!(!should_run(&q, before, ALL, &snap(true, true, 0), None));

        // A window that wraps past midnight is inside at 23:30.
        let overnight = queue_with(QueueSchedule::Daily {
            start: NaiveTime::from_hms_opt(23, 0, 0).unwrap(),
            stop: Some(NaiveTime::from_hms_opt(2, 0, 0).unwrap()),
            days: every_day,
        });
        let late = base.with_hour(23).unwrap().with_minute(30).unwrap();
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

//! Per-queue start / stop scheduler.
//!
//! Single tokio task that wakes every 30 s and asks each queue whether
//! it should be running right now. Transitions are edge-triggered: a
//! queue that should be running but has no active jobs gets `start_queue`
//! called once; one that should be stopped gets `stop_queue`. Hooks fire
//! from `data::hooks` off `QueueStarted` / `QueueFinished` events.
//!
//! Condition schedules: shared probes (metered / AC / idle) run once
//! per tick via `conditions::probe`; command conditions are polled
//! per-queue at their configured interval (floored to the tick) with
//! the last verdict cached in [`CmdPoll`].

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{Datelike, Local, NaiveTime, Timelike};

use crate::data::conditions::{self, CondSnapshot};
use crate::data::state::AppState;
use crate::domain::{CondKind, Queue, QueueId, QueueSchedule};

const TICK: Duration = Duration::from_secs(30);

/// Cached verdict of one queue's condition command.
struct CmdPoll {
    /// The command the verdict belongs to; a config edit invalidates it.
    cmd: String,
    checked: Instant,
    ok: bool,
}

pub fn spawn(state: Arc<AppState>) {
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
            // present); unavailable conditions are neither probed nor
            // evaluated — they simply don't participate.
            let available = conditions::available_conditions();
            // Probe only the conditions some queue actually uses; the
            // snapshot fails open for everything else.
            let needed: HashSet<CondKind> = queues
                .iter()
                .filter_map(|q| match &q.schedule {
                    QueueSchedule::Condition(set) => Some(set.enabled()),
                    _ => None,
                })
                .flatten()
                .filter(|k| available.contains(k))
                .collect();
            let conds = conditions::probe(&needed).await;
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
                } else if !active
                    && prev
                    && let Err(e) = state.stop_queue(q.id).await
                {
                    tracing::warn!(queue = %q.name, error = %e, "scheduler stop_queue");
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

/// `cmd_ok`: cached verdict of this queue's condition command, if it
/// has one (`None` before the first poll completes ⇒ not met yet —
/// commands are explicit user configuration, so unlike the passive
/// probes they do not fail open).
fn should_run(
    q: &Queue,
    now: chrono::DateTime<Local>,
    available: &[CondKind],
    conds: &CondSnapshot,
    cmd_ok: Option<bool>,
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
                None => false, // one-shot start with no end window
            }
        }
        QueueSchedule::Condition(set) => set.holds(available, |kind| match kind {
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

    #[test]
    fn unprobed_passive_conditions_fail_open() {
        let q = queue_with(QueueSchedule::Condition(CondSet {
            unmetered: true,
            ac_power: true,
            idle_minutes: Some(480),
            combine: CondCombine::All,
            ..CondSet::default()
        }));
        assert!(should_run(
            &q,
            Local::now(),
            ALL,
            &CondSnapshot::default(),
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

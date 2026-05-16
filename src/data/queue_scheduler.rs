//! Per-queue start / stop scheduler.
//!
//! Single tokio task that wakes every 30 s and asks each queue whether
//! it should be running right now. Transitions are edge-triggered: a
//! queue that should be running but has no active jobs gets `start_queue`
//! called once; one that should be stopped gets `stop_queue`. Hooks fire
//! from `data::hooks` off `QueueStarted` / `QueueFinished` events.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{Datelike, Local, NaiveTime, Timelike};

use crate::data::state::AppState;
use crate::domain::{Queue, QueueId, QueueSchedule};

const TICK: Duration = Duration::from_secs(30);

pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        // last_active[q] = previous tick's "should be running" verdict.
        // Edge transitions drive start/stop; same-state ticks are no-ops.
        let mut last_active: HashMap<QueueId, bool> = HashMap::new();
        loop {
            let queues = state.queues_snapshot().await;
            let now = Local::now();
            for q in &queues {
                let active = should_run(q, now);
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
            // Drop entries for deleted queues so the map does not grow.
            let live: std::collections::HashSet<QueueId> = queues.iter().map(|q| q.id).collect();
            last_active.retain(|k, _| live.contains(k));
            tokio::time::sleep(TICK).await;
        }
    });
}

fn should_run(q: &Queue, now: chrono::DateTime<Local>) -> bool {
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
    }
}

fn current_naive_time(now: chrono::DateTime<Local>) -> NaiveTime {
    NaiveTime::from_hms_opt(now.hour(), now.minute(), now.second()).unwrap_or_default()
}

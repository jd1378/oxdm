//! Pause / resume downloads as the machine's circumstances change.
//!
//! Two rules, both opt-out from Settings → General:
//!
//! - metered connection (cellular, phone hotspot) — downloading on
//!   someone's data plan is the expensive mistake this prevents;
//! - low battery while discharging — a long download is the last thing
//!   a nearly-flat laptop should be doing.
//!
//! The guard only ever touches jobs it paused itself: a job the user
//! paused by hand stays paused when the rule clears, and a job started
//! while a rule holds is caught on the next tick. Nothing is persisted —
//! after a restart the rules simply re-evaluate.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use crate::data::{AppState, conditions};
use crate::domain::JobId;

/// Same cadence as the queue scheduler: fast enough that a few seconds
/// of cellular data is the worst case, slow enough to stay invisible.
const TICK: Duration = Duration::from_secs(30);

/// Below this, and discharging, counts as low.
const LOW_BATTERY_PERCENT: u8 = 20;

pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        // Jobs this guard paused, and therefore may resume.
        let mut held: HashSet<JobId> = HashSet::new();
        loop {
            tokio::time::sleep(TICK).await;
            let settings = state.settings().await;
            let reason =
                blocking_reason(settings.pause_on_metered, settings.pause_on_low_battery).await;

            match reason {
                Some(reason) => {
                    for id in state.running_job_ids().await {
                        if state.pause(id).await.is_ok() {
                            tracing::info!(job = %id, reason, "paused by environment guard");
                            held.insert(id);
                        }
                    }
                }
                None => {
                    for id in std::mem::take(&mut held) {
                        // A job removed meanwhile simply fails to resume.
                        if state.resume(id).await.is_ok() {
                            tracing::info!(job = %id, "resumed by environment guard");
                        }
                    }
                }
            }
        }
    });
}

/// Why downloads should be held right now, if they should. Probes are
/// fail-open: a machine that cannot report its network or battery is
/// treated as fine to download on, since the alternative is a download
/// manager that never downloads.
async fn blocking_reason(on_metered: bool, on_low_battery: bool) -> Option<&'static str> {
    if on_metered && conditions::unmetered().await == Some(false) {
        return Some("metered connection");
    }
    if on_low_battery
        && conditions::on_ac() == Some(false)
        && conditions::battery_percent().is_some_and(|p| p < LOW_BATTERY_PERCENT)
    {
        return Some("low battery");
    }
    None
}

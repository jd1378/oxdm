//! Automatic update checks.
//!
//! Two triggers, one rule each:
//!
//! - **Startup.** One check, ignoring the weekly interval. A build the
//!   user has just launched is the moment they can act on the news, and
//!   a machine that is only ever on for an hour a day would otherwise
//!   never reach an idle window.
//! - **Idle.** After that, at most once a week, and only once the
//!   machine has been quiet for [`IDLE_MINUTES`] — a check is network
//!   and CPU the user did not ask for, so it waits for a moment they
//!   are not using either. The idle reading is the daemon-wide one from
//!   [`crate::data::idle`], the same sample the queue scheduler runs on.
//!
//! The idle reading is only asked for while one of those two things is
//! pending — see [`IdleWatch::want`]. A daemon whose last check was
//! yesterday, or whose user turned automatic checks off, does not make
//! the session manager answer questions nobody will act on.
//!
//! Whatever is found is announced when the user is *back*, never into
//! an empty room: a dialog raised over a locked screen is one the user
//! meets as a surprise window from an unknown time, and a notification
//! fired at 04:00 has expired from the shade by morning. So a find
//! during idle is held, and released on the edge back to active.

use std::sync::Arc;
use std::time::Duration;

use crate::data::idle::{IdleWatch, Want};
use crate::data::state::AppState;
use crate::data::{DomainEvent, UpdateInfo};

/// How long the machine must be quiet before a check is allowed.
/// Deliberately short: this is "the user stepped away", not the hours a
/// queue might wait for. It is not a setting because there is no
/// decision here for a user to make — the check is invisible either way.
const IDLE_MINUTES: u16 = 5;

/// Gap between automatic checks. A release the user learns about a few
/// days late costs nothing; asking GitHub on every idle moment would be
/// traffic with no answer to give.
const CHECK_EVERY: chrono::Duration = chrono::Duration::days(7);

/// Longest this task sleeps while it has nothing to watch for. Bounds
/// how late it notices automatic checks being switched back on, and how
/// far a suspend or a clock change can push it off schedule. Nothing
/// probes on this timer — it wakes, compares two timestamps, and
/// usually sleeps again.
const IDLE_RECHECK: Duration = Duration::from_secs(15 * 60);

pub fn spawn(state: Arc<AppState>, idle: IdleWatch) {
    tokio::spawn(async move {
        run(state, idle).await;
    });
}

async fn run(state: Arc<AppState>, mut idle: IdleWatch) {
    // Held here rather than published straight away: see the module
    // note on announcing into an empty room.
    let mut held: Option<UpdateInfo> = None;
    let mut was_idle = idle.idle_at_least(IDLE_MINUTES);

    // The startup check ignores the interval, but not the settings: a
    // user who turned automatic checks off has said so.
    if state.settings().await.auto_check_updates
        && let Some(info) = check(&state).await
    {
        held = Some(info);
    }
    if !was_idle {
        release(&state, &mut held).await;
    }

    loop {
        if state.is_exiting() {
            idle.want(Want::Updates, false);
            return;
        }
        let auto = state.settings().await.auto_check_updates;
        let last = state.last_update_check().await;
        let now = chrono::Utc::now();
        // Two reasons to watch, and only these two: a check that is due
        // needs to see the machine go quiet, and a version found while
        // it was quiet needs to see the user come back. Anything else
        // and the poller should be parked — asking whether someone is
        // at their desk is not free, and nothing here would act on the
        // answer.
        let watching = auto && (held.is_some() || due(last, now));
        idle.want(Want::Updates, watching);
        if !watching {
            // Whatever this task is waiting for is a clock, not the
            // user: either the week running out, or the setting being
            // switched back on. Sleeping until then costs nothing and
            // wakes no probe.
            was_idle = false;
            tokio::time::sleep(nap(auto.then_some(last).flatten(), now)).await;
            continue;
        }
        if !idle.next_sample().await {
            return; // watch gone: the daemon is shutting down
        }
        let now_idle = idle.idle_at_least(IDLE_MINUTES);
        let edge = (was_idle, now_idle);
        was_idle = now_idle;
        match edge {
            // Gone quiet: the moment to spend someone's bandwidth.
            // Re-checked rather than trusted from the top of the loop,
            // because a sample can arrive minutes later.
            (false, true) => {
                if state.settings().await.auto_check_updates
                    && due(state.last_update_check().await, chrono::Utc::now())
                    && let Some(info) = check(&state).await
                {
                    held = Some(info);
                }
            }
            // Back at the keyboard: say what we found while they were
            // away.
            (true, false) => release(&state, &mut held).await,
            _ => {}
        }
    }
}

/// How long to sleep while nothing is being watched for: until the next
/// check falls due, or [`IDLE_RECHECK`] — whichever is sooner.
///
/// The cap is what notices a user turning automatic checks back on, and
/// what keeps a suspended machine or a stepped clock from parking this
/// task for a week. `None` (no stamp, or checks off) means there is no
/// deadline to aim at, so it is the cap.
fn nap(
    last: Option<chrono::DateTime<chrono::Utc>>,
    now: chrono::DateTime<chrono::Utc>,
) -> Duration {
    let until_due = match last {
        // No deadline to aim at, so the cap decides.
        None => IDLE_RECHECK,
        // A negative remainder is a check already overdue, not a
        // missing one — the caller would be watching rather than
        // napping, but conflating the two with "no stamp" would be a
        // trap for the next reader.
        Some(t) => (t + CHECK_EVERY - now).to_std().unwrap_or(Duration::ZERO),
    };
    until_due.min(IDLE_RECHECK)
}

/// Run one check, keeping only a version worth telling the user about.
/// Errors are logged and nothing else: an automatic check that cannot
/// reach the network is not news, and the About window is where a user
/// who wants to know goes.
async fn check(state: &Arc<AppState>) -> Option<UpdateInfo> {
    match state.check_for_update().await {
        Ok(found) => found,
        Err(e) => {
            tracing::debug!(error = %e, "automatic update check failed");
            None
        }
    }
}

/// Publish a held find, once. The surface (dialog, notification, or
/// nothing) is decided by whoever listens — this only says that a
/// version is there and the user is present to hear it.
async fn release(state: &Arc<AppState>, held: &mut Option<UpdateInfo>) {
    let Some(info) = held.take() else {
        return;
    };
    // A version already fetched and waiting to install has been
    // announced by the flow that fetched it; saying it again would ask
    // the user to start a download they have already finished.
    if state.pending_update().await.is_some() {
        return;
    }
    state.publish(DomainEvent::UpdateAvailable { info });
}

/// Is an automatic check due? `None` (never checked, or the stamp was
/// lost) is due, and so is a stamp in the future — a clock that moved
/// backwards should not park the checker for a week.
fn due(last: Option<chrono::DateTime<chrono::Utc>>, now: chrono::DateTime<chrono::Utc>) -> bool {
    match last {
        None => true,
        Some(t) if t > now => true,
        Some(t) => now - t >= CHECK_EVERY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_check_is_due_weekly_and_on_a_fresh_install() {
        let now = chrono::Utc::now();
        assert!(due(None, now), "never checked");
        assert!(due(Some(now - chrono::Duration::days(8)), now));
        assert!(!due(Some(now - chrono::Duration::days(6)), now));
        assert!(!due(Some(now - chrono::Duration::minutes(1)), now));
    }

    /// While nothing is pending this task is waiting on a clock, and
    /// the wait is the shorter of "until the week is up" and the cap
    /// that notices the setting coming back on.
    #[test]
    fn a_parked_watcher_sleeps_until_it_has_something_to_do() {
        let now = chrono::Utc::now();
        // Six days in: a day of waiting left, capped to the recheck.
        assert_eq!(
            nap(Some(now - chrono::Duration::days(6)), now),
            IDLE_RECHECK
        );
        // Minutes from due: sleep exactly that long, not the full cap.
        let nearly = now - CHECK_EVERY + chrono::Duration::minutes(2);
        let left = nap(Some(nearly), now);
        assert!(left < IDLE_RECHECK && left > Duration::from_secs(60));
        // Checks off, or never checked: nothing to aim at, so the cap.
        assert_eq!(nap(None, now), IDLE_RECHECK);
        // Already due (the caller would be watching, not napping) must
        // still terminate rather than underflow.
        assert_eq!(
            nap(Some(now - chrono::Duration::days(30)), now),
            Duration::ZERO
        );
    }

    /// A stamp from the future means the clock moved, not that the
    /// check happened. Waiting a week on it would leave the checker
    /// parked until the stored time is genuinely in the past.
    #[test]
    fn a_clock_that_went_backwards_does_not_park_the_checker() {
        let now = chrono::Utc::now();
        assert!(due(Some(now + chrono::Duration::days(30)), now));
    }
}

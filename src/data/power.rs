//! Delayed destructive power actions.
//!
//! Queue hooks (`hooks.rs`) and per-job completion actions
//! (`daemon/completion_actions.rs`) both promise "the system shuts
//! down N seconds after the downloads finish, with a cancellable
//! countdown". [`PowerGuard`] is the single mechanism behind that
//! promise: arming spawns a task that waits out
//! [`SHUTDOWN_GRACE_SECS`], emits `ShutdownPending` / `ShutdownCancelled`
//! domain events for the GUI banner, and only ever holds **one**
//! pending action — arming while one is pending is a no-op, so two
//! queues finishing back-to-back cannot stack timers.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::broadcast;

use crate::data::events::DomainEvent;
use crate::domain::{PowerAction, SHUTDOWN_GRACE_SECS};

/// Env override for the grace period, in whole seconds. Exists for
/// tests and the visual sweep — never documented as a user setting.
pub const GRACE_ENV: &str = "OXDM_SHUTDOWN_GRACE_SECS";

/// Kill switch for destructive power actions. Set to `1` / `true` and
/// [`PowerGuard::arm`] refuses to arm at all — nothing is scheduled, no
/// countdown is shown, and no `systemctl poweroff` can ever run.
///
/// Exists because an automated harness (the visual-test sandbox) runs a
/// **real** daemon: XDG dirs are sandboxed, but a power action is not —
/// a queue with a shutdown hook powers off the developer's machine for
/// real. Never documented as a user setting.
pub const DISABLE_ENV: &str = "OXDM_NO_POWER_ACTIONS";

fn power_actions_disabled() -> bool {
    matches!(
        std::env::var(DISABLE_ENV).as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE")
    )
}

fn grace_period() -> Duration {
    let secs = std::env::var(GRACE_ENV)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(SHUTDOWN_GRACE_SECS);
    Duration::from_secs(secs)
}

struct Pending {
    /// Monotonic arm counter. The grace task only fires if the slot
    /// still holds *its own* arming — guards against the (abort-raced)
    /// interleaving arm A → cancel → arm B where A's task wakes late
    /// and would otherwise claim B's pending slot.
    seq: u64,
    action: PowerAction,
    deadline_ms: i64,
    abort: tokio::task::AbortHandle,
    /// "Confirm now" signal: firing it wakes the grace task early so
    /// the action executes immediately (same claim path as expiry).
    fire: Option<tokio::sync::oneshot::Sender<()>>,
}

/// One-slot guard for pending destructive power actions. Owned by
/// `AppState`; both power-action call sites go through it.
pub struct PowerGuard {
    /// `(next_seq, current)` under one lock so seq allocation and slot
    /// occupancy stay consistent.
    pending: Mutex<(u64, Option<Pending>)>,
    events: broadcast::Sender<DomainEvent>,
    /// [`DISABLE_ENV`] was set at construction: every arm is refused.
    /// Read once, so a test harness cannot lose the switch to a later
    /// env mutation.
    disabled: bool,
}

impl PowerGuard {
    pub fn new(events: broadcast::Sender<DomainEvent>) -> Self {
        Self::with_disabled(events, power_actions_disabled())
    }

    fn with_disabled(events: broadcast::Sender<DomainEvent>, disabled: bool) -> Self {
        if disabled {
            tracing::warn!(
                env = DISABLE_ENV,
                "destructive power actions are disabled for this process"
            );
        }
        Self {
            pending: Mutex::new((0, None)),
            events,
            disabled,
        }
    }

    /// Arm `action` to execute after the grace period. Returns `false`
    /// (and does nothing) when another action is already pending — the
    /// first arming wins and its countdown keeps running.
    ///
    /// `execute` runs on the spawned grace task after it has atomically
    /// removed its own pending entry, so a concurrent [`cancel`] that
    /// wins the race means no action, and a cancel that loses it is a
    /// clean no-op ("cancel after expiry").
    ///
    /// [`cancel`]: Self::cancel
    pub fn arm<F>(self: &Arc<Self>, action: PowerAction, execute: F) -> bool
    where
        F: FnOnce() -> Result<(), String> + Send + 'static,
    {
        if self.disabled {
            tracing::warn!(
                ?action,
                env = DISABLE_ENV,
                "power actions disabled for this process; refusing to arm"
            );
            return false;
        }
        let delay = grace_period();
        let deadline_ms = chrono::Utc::now().timestamp_millis() + delay.as_millis() as i64;
        let mut g = self.pending.lock().expect("power guard mutex poisoned");
        if g.1.is_some() {
            tracing::info!(?action, "power action already pending; new arm ignored");
            return false;
        }
        g.0 += 1;
        let seq = g.0;
        let guard = Arc::clone(self);
        let (fire_tx, fire_rx) = tokio::sync::oneshot::channel::<()>();
        // Spawn while still holding the lock: with a zero grace (tests,
        // debug override) the task could otherwise win the race and
        // find an empty slot below.
        let handle = tokio::spawn(async move {
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = fire_rx => {}
            }
            let claimed = {
                let mut g = guard.pending.lock().expect("power guard mutex poisoned");
                match &g.1 {
                    Some(p) if p.seq == seq => {
                        g.1 = None;
                        true
                    }
                    _ => false,
                }
            };
            if !claimed {
                return;
            }
            tracing::warn!(?action, "shutdown grace elapsed; executing power action");
            if let Err(e) = execute() {
                tracing::warn!(?action, error = %e, "power action failed");
            }
        });
        g.1 = Some(Pending {
            seq,
            action,
            deadline_ms,
            abort: handle.abort_handle(),
            fire: Some(fire_tx),
        });
        drop(g);
        let _ = self.events.send(DomainEvent::ShutdownPending {
            action,
            deadline_ms,
        });
        tracing::info!(?action, grace_secs = delay.as_secs(), "power action armed");
        true
    }

    /// Cancel the pending action, if any. Idempotent: cancelling with
    /// nothing pending (including "the timer just fired") is a no-op.
    pub fn cancel(&self) {
        let taken = {
            let mut g = self.pending.lock().expect("power guard mutex poisoned");
            g.1.take()
        };
        let Some(p) = taken else {
            return;
        };
        p.abort.abort();
        let _ = self.events.send(DomainEvent::ShutdownCancelled);
        tracing::info!(action = ?p.action, "pending power action cancelled");
    }

    /// Execute the pending action *now*, skipping the rest of the
    /// grace period ("confirm instantly" in the countdown window).
    /// Returns `false` when nothing is pending (or the timer already
    /// fired) — idempotent like [`cancel`](Self::cancel). The grace
    /// task's claim path runs unchanged, so a racing cancel still
    /// wins or loses atomically.
    pub fn confirm(&self) -> bool {
        let fire = {
            let mut g = self.pending.lock().expect("power guard mutex poisoned");
            g.1.as_mut().and_then(|p| p.fire.take())
        };
        match fire {
            Some(tx) => {
                tracing::info!("pending power action confirmed; executing immediately");
                // Err = grace task already gone (fired/aborted) — no-op.
                tx.send(()).is_ok()
            }
            None => false,
        }
    }

    /// Currently pending action, for snapshots to late-connecting GUIs.
    pub fn pending(&self) -> Option<(PowerAction, i64)> {
        self.pending
            .lock()
            .expect("power guard mutex poisoned")
            .1
            .as_ref()
            .map(|p| (p.action, p.deadline_ms))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn guard() -> (Arc<PowerGuard>, broadcast::Receiver<DomainEvent>) {
        let (tx, rx) = broadcast::channel(16);
        (Arc::new(PowerGuard::with_disabled(tx, false)), rx)
    }

    fn disabled_guard() -> (Arc<PowerGuard>, broadcast::Receiver<DomainEvent>) {
        let (tx, rx) = broadcast::channel(16);
        (Arc::new(PowerGuard::with_disabled(tx, true)), rx)
    }

    fn counter_action(hits: &Arc<AtomicU32>) -> impl FnOnce() -> Result<(), String> + Send + use<> {
        let hits = hits.clone();
        move || {
            hits.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    /// Past the grace period plus slack — enough for the timer to fire
    /// under paused time regardless of the env override.
    async fn advance_past_grace() {
        tokio::time::sleep(grace_period() + Duration::from_secs(5)).await;
    }

    #[tokio::test(start_paused = true)]
    async fn disabled_guard_never_arms_or_fires() {
        // The visual-test harness runs a real daemon; without this
        // switch a queue with a shutdown hook powers off the developer's
        // machine (it did, once). Arming must be refused outright — no
        // pending action, no countdown banner, no execute.
        let (g, mut rx) = disabled_guard();
        let hits = Arc::new(AtomicU32::new(0));
        assert!(!g.arm(PowerAction::ShutDown, counter_action(&hits)));
        assert!(g.pending().is_none());
        advance_past_grace().await;
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        assert!(rx.try_recv().is_err(), "no ShutdownPending event");
        // "Confirm now" cannot resurrect it either.
        g.confirm();
        advance_past_grace().await;
        assert_eq!(hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn arm_then_cancel_never_fires() {
        let (g, mut rx) = guard();
        let hits = Arc::new(AtomicU32::new(0));
        assert!(g.arm(PowerAction::ShutDown, counter_action(&hits)));
        assert!(matches!(
            g.pending(),
            Some((PowerAction::ShutDown, ms)) if ms > 0
        ));
        assert!(matches!(
            rx.try_recv(),
            Ok(DomainEvent::ShutdownPending {
                action: PowerAction::ShutDown,
                ..
            })
        ));
        g.cancel();
        assert!(matches!(rx.try_recv(), Ok(DomainEvent::ShutdownCancelled)));
        advance_past_grace().await;
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        assert!(g.pending().is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn double_arm_keeps_single_pending() {
        let (g, _rx) = guard();
        let first = Arc::new(AtomicU32::new(0));
        let second = Arc::new(AtomicU32::new(0));
        assert!(g.arm(PowerAction::Restart, counter_action(&first)));
        assert!(!g.arm(PowerAction::ShutDown, counter_action(&second)));
        // The first arming stays authoritative.
        assert!(matches!(g.pending(), Some((PowerAction::Restart, _))));
        advance_past_grace().await;
        assert_eq!(first.load(Ordering::SeqCst), 1);
        assert_eq!(second.load(Ordering::SeqCst), 0);
        assert!(g.pending().is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn cancel_after_expiry_is_a_noop() {
        let (g, mut rx) = guard();
        let hits = Arc::new(AtomicU32::new(0));
        assert!(g.arm(PowerAction::Sleep, counter_action(&hits)));
        let _ = rx.try_recv(); // drain ShutdownPending
        advance_past_grace().await;
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert!(g.pending().is_none());
        // Late cancel: no panic, no spurious ShutdownCancelled.
        g.cancel();
        assert!(rx.try_recv().is_err());
        // Slot is free again — a new action can be armed.
        assert!(g.arm(PowerAction::Hibernate, counter_action(&hits)));
        assert!(matches!(g.pending(), Some((PowerAction::Hibernate, _))));
    }

    #[tokio::test(start_paused = true)]
    async fn confirm_executes_immediately_without_waiting_grace() {
        let (g, mut rx) = guard();
        let hits = Arc::new(AtomicU32::new(0));
        assert!(g.arm(PowerAction::Sleep, counter_action(&hits)));
        let _ = rx.try_recv(); // ShutdownPending
        assert!(g.confirm());
        // Yield to the grace task without advancing the paused clock —
        // the action must fire from the confirm signal, not the timer.
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert!(g.pending().is_none());
        // Second confirm: nothing pending → no-op.
        assert!(!g.confirm());
    }

    #[tokio::test(start_paused = true)]
    async fn confirm_with_nothing_pending_is_noop() {
        let (g, _rx) = guard();
        assert!(!g.confirm());
    }

    #[tokio::test(start_paused = true)]
    async fn cancel_after_confirm_signal_does_not_double_fire() {
        let (g, _rx) = guard();
        let hits = Arc::new(AtomicU32::new(0));
        assert!(g.arm(PowerAction::ShutDown, counter_action(&hits)));
        assert!(g.confirm());
        g.cancel();
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        advance_past_grace().await;
        assert!(hits.load(Ordering::SeqCst) <= 1);
        assert!(g.pending().is_none());
    }
}

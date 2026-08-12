//! Shared session-idle watch.
//!
//! One task polls the session manager and publishes how long the
//! machine has been idle; everything that cares reads the same sample.
//! Before this, the queue scheduler probed idle inside its own tick and
//! nothing else could see the answer — a second consumer (the update
//! checker) would have meant a second D-Bus round trip per tick, and
//! two callers disagreeing about whether the user is at the keyboard.
//!
//! Polling is on demand. Nobody is at the keyboard is only worth
//! knowing while something is waiting for it — a queue with the idle
//! condition enabled, or an update check that is due — so consumers
//! declare interest with [`IdleWatch::want`] and the poller parks when
//! that set is empty. A parked watch publishes `None` rather than
//! leaving its last reading behind: a sample from an hour ago is not an
//! answer about now, and every reader treats no answer as "not idle".
//!
//! `None` means the probe failed or the platform has no answer, and
//! every reader treats that as "not idle". Failing open would be worse
//! than useless here: idleness gates work that costs bandwidth, CPU and
//! battery, so guessing "nobody is at the machine" on a host that
//! cannot tell us would start exactly the work the user asked to keep
//! out of their way. A host that has never answered reports
//! [`IdleWatch::supported`] as false, and the condition is then hidden
//! from the queue builder rather than offered as something that quietly
//! never holds.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::Duration;

use tokio::sync::{Notify, watch};

/// How often the session is sampled. Matches the scheduler tick, which
/// is what consumes it most often; idle is a state that changes on the
/// scale of minutes, so nothing is gained by asking more.
///
/// The sample itself is two property reads on a connection held open
/// for the life of the daemon — microseconds. Reconnecting each time
/// cost ~50× that, all of it in the bus handshake, which is why
/// [`Prober`] exists.
const POLL: Duration = Duration::from_secs(30);

/// Who currently needs idle readings. One bit each, so a consumer can
/// declare and withdraw interest without knowing about the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Want {
    /// A queue whose schedule has the idle condition enabled.
    Queues,
    /// An update check that is due, or a found version waiting for the
    /// user to come back before it is announced.
    Updates,
}

impl Want {
    const fn bit(self) -> u8 {
        match self {
            Want::Queues => 1,
            Want::Updates => 2,
        }
    }
}

/// The demand set, plus the doorbell that unparks the poller.
#[derive(Default)]
struct Demand {
    mask: AtomicU8,
    woken: Notify,
}

/// A handle on the latest sample. Cheap to clone — every consumer gets
/// its own cursor into the same channel.
#[derive(Clone)]
pub struct IdleWatch {
    rx: watch::Receiver<Option<Duration>>,
    /// Set the first time a probe answers, and never cleared: a session
    /// that reported once can report again, so a momentary D-Bus
    /// failure must not take the condition out of the UI underneath a
    /// user who is configuring it.
    supported: Arc<AtomicBool>,
    demand: Arc<Demand>,
    /// Holds the sending half alive for a [`IdleWatch::fixed`] handle,
    /// which has no poller behind it. `None` for the real thing, where
    /// the poll task owns the sender.
    _pinned: Option<Arc<watch::Sender<Option<Duration>>>>,
}

impl IdleWatch {
    /// The most recent sample without waiting for a new one.
    pub fn current(&self) -> Option<Duration> {
        *self.rx.borrow()
    }

    /// Has the session been idle for at least `minutes`?
    ///
    /// No reading means no: everything behind this question costs the
    /// user something, and a host that cannot say whether they are
    /// there is not permission to assume they are not.
    pub fn idle_at_least(&self, minutes: u16) -> bool {
        matches!(self.current(), Some(d) if d >= Duration::from_secs(u64::from(minutes) * 60))
    }

    /// Can this host report idleness at all? False until the first
    /// probe answers, and true from then on.
    pub fn supported(&self) -> bool {
        self.supported.load(Ordering::Relaxed)
    }

    /// Declare (or withdraw) a need for fresh readings.
    ///
    /// Idempotent, and cheap enough to call every tick with whatever
    /// the caller currently needs — which is the intended use, since
    /// "does any queue want idle" changes whenever a queue is edited.
    /// The poller wakes on the first declaration and parks when the
    /// last one is withdrawn.
    pub fn want(&self, who: Want, wanted: bool) {
        let bit = who.bit();
        let before = match wanted {
            true => self.demand.mask.fetch_or(bit, Ordering::AcqRel),
            false => self.demand.mask.fetch_and(!bit, Ordering::AcqRel),
        };
        if wanted && before == 0 {
            self.demand.woken.notify_one();
        }
    }

    /// Wait for the next sample. Returns `false` once the watcher is
    /// gone (daemon shutting down), so callers can end their loop.
    pub async fn next_sample(&mut self) -> bool {
        self.rx.changed().await.is_ok()
    }

    /// A watch pinned to one reading, for tests and for consumers built
    /// before the poller exists.
    pub fn fixed(idle: Option<Duration>) -> Self {
        let (tx, rx) = watch::channel(idle);
        Self {
            rx,
            supported: Arc::new(AtomicBool::new(idle.is_some())),
            demand: Arc::new(Demand::default()),
            _pinned: Some(Arc::new(tx)),
        }
    }
}

/// Take the first sample, then start the (demand-gated) poller.
///
/// The first probe is awaited rather than left to the background task,
/// and runs whether or not anything wants readings yet, because
/// [`IdleWatch::supported`] decides whether the queue builder offers
/// the idle condition at all. A window opening in the gap would be told
/// "this machine cannot do idle" by a watch that simply had not asked
/// yet. One bus round trip at startup buys an answer that is right from
/// the first read — and it is the only probe a daemon with no idle
/// queues and no due update check ever makes.
pub async fn spawn() -> IdleWatch {
    let mut prober = Prober::default();
    let first = prober.sample().await;
    let (tx, rx) = watch::channel(first);
    let supported = Arc::new(AtomicBool::new(first.is_some()));
    let seen = supported.clone();
    let demand = Arc::new(Demand::default());
    let wanted = demand.clone();
    tokio::spawn(async move {
        loop {
            if wanted.mask.load(Ordering::Acquire) == 0 {
                // Park. The reading goes with it: consumers must not
                // decide anything on a sample from before the pause,
                // and `None` is exactly "we are not watching".
                if tx.send(None).is_err() {
                    return;
                }
                tracing::debug!("idle polling parked: nothing is waiting on it");
                wanted.woken.notified().await;
                tracing::debug!("idle polling resumed");
                // Straight to a fresh sample — whoever just asked is
                // waiting on an answer, not on the poll interval.
                continue;
            }
            let sample = prober.sample().await;
            if sample.is_some() {
                seen.store(true, Ordering::Relaxed);
            }
            // `send` fails only when every receiver is gone; the daemon
            // holds one for its lifetime, so this is the shutdown path.
            if tx.send(sample).is_err() {
                return;
            }
            tokio::time::sleep(POLL).await;
        }
    });
    IdleWatch {
        rx,
        supported,
        demand,
        _pinned: None,
    }
}

/// Reads session idle time, holding the bus connection between polls.
///
/// The connection is the expensive part by two orders of magnitude —
/// socket, SASL handshake and `Hello` per poll, against two cached
/// property reads once it is up. Kept in an `Option` so a bus that goes
/// away (logind restart, session teardown) is reconnected on the next
/// poll instead of wedging the watch for the life of the daemon.
#[cfg(target_os = "linux")]
#[derive(Default)]
struct Prober {
    proxy: Option<zbus::Proxy<'static>>,
}

#[cfg(target_os = "linux")]
impl Prober {
    /// Session idle time via logind's caller-session object
    /// (`/org/freedesktop/login1/session/auto`): `IdleHint` false ⇒
    /// ZERO, true ⇒ now − `IdleSinceHint` (µs, CLOCK_REALTIME).
    /// Sessions whose desktop never sets the hint read as never idle,
    /// which is what "as reported by the session manager" means.
    ///
    /// `None` is reserved for probe *failure*: a host with no logind,
    /// no session object, or a broken bus. That is what marks idleness
    /// unsupported, so the distinction from an honest `Some(ZERO)`
    /// matters — one hides the condition, the other says the user is
    /// here.
    async fn sample(&mut self) -> Option<Duration> {
        match self.read().await {
            Ok(d) => Some(d),
            Err(e) => {
                // Drop it: a failure here is usually the connection,
                // and the next poll should build a fresh one.
                self.proxy = None;
                tracing::debug!(error = %e, "logind idle probe failed; treating as not idle");
                None
            }
        }
    }

    async fn read(&mut self) -> zbus::Result<Duration> {
        let proxy = match &self.proxy {
            Some(p) => p,
            None => {
                let conn = zbus::Connection::system().await?;
                self.proxy.insert(
                    zbus::Proxy::new(
                        &conn,
                        "org.freedesktop.login1",
                        "/org/freedesktop/login1/session/auto",
                        "org.freedesktop.login1.Session",
                    )
                    .await?,
                )
            }
        };
        let hint: bool = proxy.get_property("IdleHint").await?;
        if !hint {
            return Ok(Duration::ZERO);
        }
        let since_us: u64 = proxy.get_property("IdleSinceHint").await?;
        let now_us = u64::try_from(chrono::Utc::now().timestamp_micros()).unwrap_or(0);
        Ok(Duration::from_micros(now_us.saturating_sub(since_us)))
    }
}

/// Windows: `GetLastInputInfo` reports the tick of the last keyboard or
/// mouse input for the calling session, so idle time is the gap from
/// then to now. Both clocks are the same 64-bit tick count, so the
/// 49-day `u32` wrap of the returned value is repaired by comparing
/// against the low half of `GetTickCount64`.
///
/// Session-scoped by definition, which is what the condition means. The
/// call cannot fail short of a wrong struct size, so `None` here really
/// does mean "this host cannot answer".
#[cfg(target_os = "windows")]
#[derive(Default)]
struct Prober;

#[cfg(target_os = "windows")]
impl Prober {
    async fn sample(&mut self) -> Option<Duration> {
        use windows_sys::Win32::System::SystemInformation::GetTickCount64;
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};

        let mut info = LASTINPUTINFO {
            cbSize: u32::try_from(std::mem::size_of::<LASTINPUTINFO>()).ok()?,
            dwTime: 0,
        };
        // SAFETY: `info` is a correctly sized, initialised LASTINPUTINFO
        // and the call only writes `dwTime`.
        if unsafe { GetLastInputInfo(&mut info) } == 0 {
            tracing::debug!("GetLastInputInfo failed; treating as not idle");
            return None;
        }
        let now = unsafe { GetTickCount64() };
        // `dwTime` is the low 32 bits of the same tick count. Rebuild
        // the full value from the current high half, stepping back one
        // wrap when the low half has rolled over since the input.
        let low = u64::from(info.dwTime);
        let last = match (now & 0xFFFF_FFFF) >= low {
            true => (now & !0xFFFF_FFFF) | low,
            false => ((now & !0xFFFF_FFFF).checked_sub(0x1_0000_0000)?) | low,
        };
        Some(Duration::from_millis(now.saturating_sub(last)))
    }
}

/// macOS: Quartz reports seconds since the last input event of any kind
/// across the whole session (`kCGAnyInputEventType` = 0xFFFFFFFF,
/// `kCGEventSourceStateCombinedSessionState` = 0), which is exactly the
/// question. A negative result would mean the framework refused, and is
/// treated as no answer.
#[cfg(target_os = "macos")]
#[derive(Default)]
struct Prober;

#[cfg(target_os = "macos")]
impl Prober {
    async fn sample(&mut self) -> Option<Duration> {
        /// `kCGEventSourceStateCombinedSessionState` — every input
        /// source in the session, not one HID device.
        /// `CGEventSourceStateID` is a signed 32-bit enum.
        const COMBINED_SESSION_STATE: i32 = 0;
        /// `kCGAnyInputEventType`, an unsigned `CGEventType`.
        const ANY_INPUT_EVENT_TYPE: u32 = 0xFFFF_FFFF;

        #[link(name = "CoreGraphics", kind = "framework")]
        unsafe extern "C" {
            fn CGEventSourceSecondsSinceLastEventType(state: i32, event_type: u32) -> f64;
        }

        // SAFETY: both arguments are the documented constants and the
        // call has no out-parameters.
        let secs = unsafe {
            CGEventSourceSecondsSinceLastEventType(COMBINED_SESSION_STATE, ANY_INPUT_EVENT_TYPE)
        };
        if !secs.is_finite() || secs < 0.0 {
            tracing::debug!(secs, "Quartz idle query gave no answer");
            return None;
        }
        Some(Duration::from_secs_f64(secs))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
#[derive(Default)]
struct Prober;

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
impl Prober {
    async fn sample(&mut self) -> Option<Duration> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "We cannot tell" is not "nobody is there": the work behind this
    /// question is work the user asked to be kept out of their way.
    #[test]
    fn a_missing_reading_is_not_idle() {
        let unknown = IdleWatch::fixed(None);
        assert!(!unknown.idle_at_least(480));
        assert!(!unknown.idle_at_least(0));
        assert!(!unknown.supported());
    }

    #[test]
    fn a_reading_is_compared_in_minutes() {
        let w = IdleWatch::fixed(Some(Duration::from_secs(600)));
        assert!(w.supported());
        assert!(w.idle_at_least(10));
        assert!(!w.idle_at_least(11));
        assert!(!IdleWatch::fixed(Some(Duration::ZERO)).idle_at_least(5));
    }
}

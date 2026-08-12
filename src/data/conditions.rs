//! Environmental-condition probes for `QueueSchedule::Condition`.
//!
//! The scheduler (`queue_scheduler.rs`) probes once per tick — and only
//! the conditions some queue actually uses — then evaluates every queue
//! against the resulting [`CondSnapshot`]. Every condition fails
//! *closed*: no reading means the condition does not hold, so a queue
//! waiting for one stays put. All three describe a moment that is
//! cheap for the user to spend — an unmetered link, mains power, an
//! empty chair — and a probe that cannot answer is not evidence that
//! the moment has arrived. Guessing "yes" spends their data allowance
//! or their battery, which is the exact thing the condition exists to
//! avoid.
//!
//! A condition this host can never answer is not offered at all
//! ([`available_conditions`]), so failing closed cannot turn into a
//! queue that mysteriously never runs: either the option is absent, or
//! it works.
//!
//! Command polling is per-queue state, so it lives in the scheduler;
//! only the one-shot runner is here.

use std::collections::HashSet;
use std::time::Duration;

use crate::domain::CondKind;

/// Conditions this host can evaluate *right now*: the compile-time
/// [`CondKind::SUPPORTED`] set, minus the ones this machine cannot
/// answer. An unavailable condition is hidden in the queue builder and
/// excluded from evaluation entirely, so it can never read as a
/// condition that quietly holds — or quietly does not.
///
/// - `Unmetered` needs something that reports link cost — on Linux,
///   NetworkManager. A host running systemd-networkd has no answer.
/// - `AcPower` needs both a battery (on a desktop the condition would
///   be trivially true, which is worth nothing) and a probe that
///   answers.
/// - `Idle` needs a session manager that reports it.
///
/// `Command` and `JobAdded` are always available: one answers by
/// definition because the user wrote it, and the other is oxdm's own
/// event.
pub fn available_conditions(support: CondSupport) -> Vec<CondKind> {
    CondKind::SUPPORTED
        .iter()
        .copied()
        .filter(|k| match k {
            // Not a probe of anything: oxdm raises it itself.
            CondKind::JobAdded => true,
            CondKind::Unmetered => support.unmetered,
            CondKind::AcPower => support.ac_power,
            CondKind::Idle => support.idle,
            CondKind::Command => true,
        })
        .collect()
}

/// Which conditions this machine can actually answer.
///
/// Probed once at daemon start rather than per tick: the answer is a
/// property of the host (is there a battery, is NetworkManager on this
/// system bus), not of the moment. Since every condition now fails
/// closed, this is what keeps "cannot answer" from reading as "never
/// runs" — an unanswerable condition is never offered in the first
/// place.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CondSupport {
    pub unmetered: bool,
    pub ac_power: bool,
    pub idle: bool,
}

/// Ask each probe once whether it can answer at all. `idle` comes from
/// the shared watch, which has already taken its first sample.
pub async fn detect_support(idle: bool) -> CondSupport {
    let support = CondSupport {
        unmetered: network_unmetered().await.is_some(),
        // Both halves matter: a laptop whose probe fails and a desktop
        // whose probe works are equally unable to make this condition
        // mean anything.
        ac_power: has_battery() && on_ac_power().is_some(),
        idle,
    };
    tracing::debug!(
        unmetered = support.unmetered,
        ac_power = support.ac_power,
        idle = support.idle,
        "queue conditions this host can evaluate"
    );
    support
}

#[cfg(target_os = "linux")]
fn has_battery() -> bool {
    has_battery_in(std::path::Path::new("/sys/class/power_supply"))
}

#[cfg(target_os = "linux")]
fn has_battery_in(dir: &std::path::Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|e| {
        std::fs::read_to_string(e.path().join("type"))
            .map(|t| t.trim() == "Battery")
            .unwrap_or(false)
    })
}

/// `BATTERY_FLAG_NO_BATTERY` (128) is the one flag that answers this
/// directly; 255 means the status is unknown, which is not a battery.
#[cfg(target_os = "windows")]
fn has_battery() -> bool {
    const NO_BATTERY: u8 = 128;
    const UNKNOWN: u8 = 255;
    match power_status() {
        Some(s) => s.BatteryFlag != UNKNOWN && s.BatteryFlag & NO_BATTERY == 0,
        None => false,
    }
}

/// Not detected on macOS: the AC probe above answers "on mains" without
/// ever saying whether a battery exists, and the dictionary walk that
/// would say so is a dependency for one boolean. The condition stays
/// hidden here rather than offered on desktops where it means nothing.
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn has_battery() -> bool {
    false
}

/// One tick's shared probe results. `None` = probe failed or was not
/// requested, and every reader treats it as "condition does not hold".
#[derive(Debug, Clone, Copy, Default)]
pub struct CondSnapshot {
    unmetered: Option<bool>,
    on_ac: Option<bool>,
    /// Time since last input activity; `Some(ZERO)` = actively in use.
    idle: Option<Duration>,
}

impl CondSnapshot {
    #[cfg(test)]
    pub(crate) fn fixed(
        unmetered: Option<bool>,
        on_ac: Option<bool>,
        idle: Option<Duration>,
    ) -> Self {
        Self {
            unmetered,
            on_ac,
            idle,
        }
    }

    /// No reading means the link is not known to be unmetered, which
    /// is not the same as free to use — see the module note.
    pub fn unmetered(&self) -> bool {
        self.unmetered == Some(true)
    }

    /// No reading means not known to be on mains, so the queue waits
    /// rather than spending a battery that might be discharging.
    pub fn on_ac(&self) -> bool {
        self.on_ac == Some(true)
    }

    pub fn idle_at_least(&self, minutes: u16) -> bool {
        matches!(self.idle, Some(d) if d >= Duration::from_secs(u64::from(minutes) * 60))
    }
}

/// Probe exactly the conditions in `needed` (Command is per-queue and
/// not probed here).
///
/// Idleness is not probed: it is sampled once for the whole daemon by
/// [`crate::data::idle`] and passed in, so the scheduler and the update
/// checker cannot disagree about whether the user is at the keyboard.
pub async fn probe(needed: &HashSet<CondKind>, idle: Option<Duration>) -> CondSnapshot {
    CondSnapshot {
        unmetered: if needed.contains(&CondKind::Unmetered) {
            network_unmetered().await
        } else {
            None
        },
        on_ac: if needed.contains(&CondKind::AcPower) {
            on_ac_power()
        } else {
            None
        },
        idle: if needed.contains(&CondKind::Idle) {
            idle
        } else {
            None
        },
    }
}

/// The platform's shell and its "run this string" flag: `sh -c`
/// everywhere but Windows, where there is no `sh` and `cmd /C` is what
/// a user writing a condition command would expect to be running.
#[cfg(not(target_os = "windows"))]
const SHELL: (&str, &str) = ("sh", "-c");
#[cfg(target_os = "windows")]
const SHELL: (&str, &str) = ("cmd", "/C");

/// Run one condition-command check through [`SHELL`], true while it
/// exits 0 (the builder card's contract). Bounded so a hung script
/// cannot stall the scheduler; timeout or spawn failure count as false
/// — unlike the passive probes this one is explicit user configuration,
/// so a broken command should read as "condition not met", not "met".
pub async fn check_command(cmd: &str) -> bool {
    const CMD_TIMEOUT: Duration = Duration::from_secs(20);
    if cmd.trim().is_empty() {
        return false; // an empty command exits 0; that is "off", not "always on"
    }
    let run = async {
        tokio::process::Command::new(SHELL.0)
            .arg(SHELL.1)
            .arg(cmd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
    };
    match tokio::time::timeout(CMD_TIMEOUT, run).await {
        Ok(Ok(status)) => status.success(),
        Ok(Err(e)) => {
            tracing::debug!(error = %e, "condition command failed to spawn");
            false
        }
        Err(_) => {
            tracing::warn!(cmd, "condition command timed out; treating as not met");
            false
        }
    }
}

/// NetworkManager `Metered` property (NM_METERED_*): 0 unknown, 1 yes,
/// 2 no, 3 guess-yes, 4 guess-no. Only explicit yes / guess-yes count
/// as metered — NM's own guidance treats unknown as unmetered. A
/// missing NM (systemd-networkd hosts) answers `None`, which takes the
/// condition off the menu rather than guessing either way.
/// The connection NetworkManager is asked over, held between probes:
/// the handshake costs more than the read it precedes. Rebuilt on any
/// failure, so an NM restart costs one probe rather than every future
/// one.
///
/// Property caching stays off here for the same reason as the idle
/// probe (see [`crate::data::idle`]): zbus refreshes a cached property
/// from `PropertiesChanged`, and a daemon that does not emit it leaves
/// the cache frozen at the value it had when oxdm started. NM is
/// better-behaved than logind about this, but a metered link that reads
/// as unmetered forever is a bill, not a glitch, and one bus round trip
/// per probe is not worth that risk.
#[cfg(target_os = "linux")]
static NM: tokio::sync::Mutex<Option<zbus::Proxy<'static>>> = tokio::sync::Mutex::const_new(None);

#[cfg(target_os = "linux")]
async fn network_unmetered() -> Option<bool> {
    async fn metered(slot: &mut Option<zbus::Proxy<'static>>) -> zbus::Result<u32> {
        let proxy = match slot {
            Some(p) => p,
            None => {
                let conn = zbus::Connection::system().await?;
                slot.insert(
                    zbus::proxy::Builder::new(&conn)
                        .destination("org.freedesktop.NetworkManager")?
                        .path("/org/freedesktop/NetworkManager")?
                        .interface("org.freedesktop.NetworkManager")?
                        .cache_properties(zbus::proxy::CacheProperties::No)
                        .build()
                        .await?,
                )
            }
        };
        proxy.get_property("Metered").await
    }
    let mut slot = NM.lock().await;
    match metered(&mut slot).await {
        Ok(v) => Some(!matches!(v, 1 | 3)),
        Err(e) => {
            *slot = None;
            tracing::debug!(error = %e, "NetworkManager metered probe failed");
            None
        }
    }
}

/// Windows: WinRT's connection cost for the profile actually carrying
/// internet traffic. `NetworkCostType` is Unknown / Unrestricted /
/// Fixed / Variable — the last two are the ones that cost the user
/// money — and roaming or a blown data cap count as metered whatever
/// the plan says, which is how Windows' own "metered connection"
/// switch behaves.
///
/// `None` when there is no internet profile at all (offline, or a
/// machine whose only link is one Windows does not classify): with
/// nothing connected there is nothing to be metered about, and
/// answering "unmetered" would let a queue start on a dead link.
#[cfg(target_os = "windows")]
async fn network_unmetered() -> Option<bool> {
    use windows::Networking::Connectivity::{NetworkCostType, NetworkInformation};

    fn cost() -> windows::core::Result<bool> {
        // WinRT needs an initialised apartment. The MTA reference is
        // process-wide, never released, and safe to take repeatedly —
        // the alternative, per-thread CoInitializeEx, would have to be
        // undone on a thread pool that outlives this call.
        // SAFETY: no arguments, no out-parameter we keep.
        unsafe { windows::Win32::System::Com::CoIncrementMTAUsage() }?;
        let profile = NetworkInformation::GetInternetConnectionProfile()?;
        let cost = profile.GetConnectionCost()?;
        let plan_is_metered = matches!(
            cost.NetworkCostType()?,
            NetworkCostType::Fixed | NetworkCostType::Variable
        );
        Ok(plan_is_metered || cost.Roaming()? || cost.OverDataLimit()?)
    }
    // Blocking COM/WinRT calls off the async worker: each is a local
    // RPC to the network service, not a computation.
    match tokio::task::spawn_blocking(cost).await {
        Ok(Ok(metered)) => Some(!metered),
        Ok(Err(e)) => {
            tracing::debug!(error = %e, "connection cost probe failed");
            None
        }
        Err(e) => {
            tracing::debug!(error = %e, "connection cost probe panicked");
            None
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
async fn network_unmetered() -> Option<bool> {
    None
}

/// Mains detection via `/sys/class/power_supply`: any online `Mains`
/// supply wins; otherwise a `Discharging` battery means battery power;
/// otherwise (desktops report no supplies at all) assume AC.
#[cfg(target_os = "linux")]
fn on_ac_power() -> Option<bool> {
    let entries = match std::fs::read_dir("/sys/class/power_supply") {
        Ok(e) => e,
        Err(_) => return None,
    };
    let mut discharging = false;
    for entry in entries.flatten() {
        let read = |file: &str| {
            std::fs::read_to_string(entry.path().join(file))
                .map(|s| s.trim().to_owned())
                .unwrap_or_default()
        };
        match read("type").as_str() {
            "Mains" if read("online") == "1" => return Some(true),
            "Battery" if read("status") == "Discharging" => discharging = true,
            _ => {}
        }
    }
    Some(!discharging)
}

/// Windows answers mains, battery presence and charge in one call.
/// `ACLineStatus`: 0 offline, 1 online, 255 unknown — and unknown is
/// `None`, the same "no answer" the Linux probe returns.
#[cfg(target_os = "windows")]
fn power_status() -> Option<windows_sys::Win32::System::Power::SYSTEM_POWER_STATUS> {
    let mut status = unsafe { std::mem::zeroed() };
    // SAFETY: the call fills a caller-owned SYSTEM_POWER_STATUS.
    if unsafe { windows_sys::Win32::System::Power::GetSystemPowerStatus(&mut status) } == 0 {
        tracing::debug!("GetSystemPowerStatus failed");
        return None;
    }
    Some(status)
}

#[cfg(target_os = "windows")]
fn on_ac_power() -> Option<bool> {
    match power_status()?.ACLineStatus {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

/// macOS: `IOPSGetTimeRemainingEstimate` answers the AC question
/// without touching a CoreFoundation dictionary — "unlimited" is the
/// documented value for a machine running off mains, whether or not it
/// has a battery at all.
#[cfg(target_os = "macos")]
fn on_ac_power() -> Option<bool> {
    /// `kIOPSTimeRemainingUnknown`: a battery still settling after a
    /// state change, so neither answer is safe yet.
    const UNKNOWN: f64 = -1.0;
    /// `kIOPSTimeRemainingUnlimited`: drawing from mains.
    const UNLIMITED: f64 = -2.0;

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOPSGetTimeRemainingEstimate() -> f64;
    }
    // SAFETY: no arguments, no out-parameters, returns a plain double.
    let estimate = unsafe { IOPSGetTimeRemainingEstimate() };
    if estimate == UNLIMITED {
        return Some(true);
    }
    if estimate == UNKNOWN {
        return None;
    }
    // Any finite estimate is time left on a battery being drained.
    Some(false)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn on_ac_power() -> Option<bool> {
    None
}

/// Battery charge percentage, or `None` where there is no battery (or
/// the kernel does not report one). Reads the first battery it finds:
/// multi-battery laptops report a per-pack figure and the guard only
/// needs "is the machine about to die".
#[cfg(target_os = "linux")]
pub fn battery_percent() -> Option<u8> {
    let entries = std::fs::read_dir("/sys/class/power_supply").ok()?;
    for entry in entries.flatten() {
        let read = |file: &str| {
            std::fs::read_to_string(entry.path().join(file))
                .map(|s| s.trim().to_owned())
                .unwrap_or_default()
        };
        if read("type") == "Battery"
            && let Ok(pct) = read("capacity").parse::<u8>()
        {
            return Some(pct);
        }
    }
    None
}

/// `BatteryLifePercent` is 0–100, or 255 for "unknown" — which is what
/// a desktop with no battery reports, and is not a charge level.
#[cfg(target_os = "windows")]
pub fn battery_percent() -> Option<u8> {
    match power_status()?.BatteryLifePercent {
        pct @ 0..=100 => Some(pct),
        _ => None,
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn battery_percent() -> Option<u8> {
    None
}

/// `on_ac_power`, for callers outside the scheduler.
pub fn on_ac() -> Option<bool> {
    on_ac_power()
}

/// `network_unmetered`, for callers outside the scheduler.
pub async fn unmetered() -> Option<bool> {
    network_unmetered().await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every condition describes a moment that is cheap for the user to
    /// spend. No reading is not that moment.
    #[test]
    fn unprobed_conditions_never_hold() {
        let snap = CondSnapshot::default();
        assert!(!snap.unmetered());
        assert!(!snap.on_ac());
        assert!(!snap.idle_at_least(480));
    }

    /// A condition this host cannot answer must not be offered: hidden
    /// beats offered-but-never-true, which is what failing closed would
    /// otherwise look like to the user.
    #[test]
    fn only_answerable_conditions_are_offered() {
        let none = available_conditions(CondSupport::default());
        assert!(!none.contains(&CondKind::Unmetered));
        assert!(!none.contains(&CondKind::AcPower));
        assert!(!none.contains(&CondKind::Idle));
        // Never dropped, whatever the host can probe. A combination
        // that lost it would fall back to starting on the conditions
        // it was meant to gate — `holds` ignores what is unavailable.
        assert!(none.contains(&CondKind::JobAdded));

        let all = available_conditions(CondSupport {
            unmetered: true,
            ac_power: true,
            idle: true,
        });
        for kind in CondKind::SUPPORTED {
            assert!(all.contains(kind), "{kind:?} is supported by this build");
        }
    }

    #[test]
    fn probed_verdicts_map_to_their_own_fields() {
        let snap = CondSnapshot::fixed(Some(false), Some(true), Some(Duration::from_secs(600)));
        assert!(!snap.unmetered());
        assert!(snap.on_ac());
        assert!(snap.idle_at_least(10));
        assert!(!snap.idle_at_least(11));
    }

    #[tokio::test]
    async fn empty_command_is_never_met() {
        assert!(!check_command("   ").await);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn command_verdict_follows_exit_code() {
        assert!(check_command("true").await);
        assert!(!check_command("false").await);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn battery_detection_gates_ac_power() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!has_battery_in(dir.path())); // no supplies at all
        std::fs::create_dir(dir.path().join("AC")).unwrap();
        std::fs::write(dir.path().join("AC/type"), "Mains\n").unwrap();
        assert!(!has_battery_in(dir.path())); // mains only ≠ battery
        std::fs::create_dir(dir.path().join("BAT0")).unwrap();
        std::fs::write(dir.path().join("BAT0/type"), "Battery\n").unwrap();
        assert!(has_battery_in(dir.path()));
        assert!(!has_battery_in(&dir.path().join("missing")));
    }
}

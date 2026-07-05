//! Environmental-condition probes for `QueueSchedule::Condition`.
//!
//! The scheduler (`queue_scheduler.rs`) probes once per tick — and only
//! the conditions some queue actually uses — then evaluates every queue
//! against the resulting [`CondSnapshot`]. Probes fail open (condition
//! treated as holding) so a missing NetworkManager, an exotic sysfs
//! layout, or a session manager that never reports idle degrades to
//! "runs anyway" rather than a queue that silently never starts.
//! Command polling is per-queue state, so it lives in the scheduler;
//! only the one-shot runner is here.

use std::collections::HashSet;
use std::time::Duration;

use crate::domain::CondKind;

/// Conditions this host can evaluate *right now*: the compile-time
/// [`CondKind::SUPPORTED`] set, minus `AcPower` when no battery is
/// present — on a desktop the condition would be trivially true, so it
/// is hidden in the UI and excluded from evaluation entirely.
pub fn available_conditions() -> Vec<CondKind> {
    CondKind::SUPPORTED
        .iter()
        .copied()
        .filter(|k| *k != CondKind::AcPower || has_battery())
        .collect()
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

#[cfg(not(target_os = "linux"))]
fn has_battery() -> bool {
    false
}

/// One tick's shared probe results. `None` = probe failed or was not
/// requested — both fail open at evaluation time.
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

    pub fn unmetered(&self) -> bool {
        self.unmetered.unwrap_or(true)
    }

    pub fn on_ac(&self) -> bool {
        self.on_ac.unwrap_or(true)
    }

    pub fn idle_at_least(&self, minutes: u16) -> bool {
        match self.idle {
            Some(d) => d >= Duration::from_secs(u64::from(minutes) * 60),
            None => true, // probe unavailable → fail open
        }
    }
}

/// Probe exactly the conditions in `needed` (Command is per-queue and
/// not probed here).
pub async fn probe(needed: &HashSet<CondKind>) -> CondSnapshot {
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
            session_idle().await
        } else {
            None
        },
    }
}

/// Run one condition-command check: `sh -c <cmd>`, true while it exits
/// 0 (the builder card's contract). Bounded so a hung script cannot
/// stall the scheduler; timeout or spawn failure count as false —
/// unlike the passive probes this one is explicit user configuration,
/// so a broken command should read as "condition not met", not "met".
pub async fn check_command(cmd: &str) -> bool {
    const CMD_TIMEOUT: Duration = Duration::from_secs(20);
    if cmd.trim().is_empty() {
        return false; // `sh -c ""` exits 0; an empty command is "off", not "always on"
    }
    let run = async {
        tokio::process::Command::new("sh")
            .arg("-c")
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
/// as metered — NM's own guidance treats unknown as unmetered — and a
/// missing NM (systemd-networkd hosts) fails open.
#[cfg(target_os = "linux")]
async fn network_unmetered() -> Option<bool> {
    async fn metered() -> zbus::Result<u32> {
        let conn = zbus::Connection::system().await?;
        let proxy = zbus::Proxy::new(
            &conn,
            "org.freedesktop.NetworkManager",
            "/org/freedesktop/NetworkManager",
            "org.freedesktop.NetworkManager",
        )
        .await?;
        proxy.get_property("Metered").await
    }
    match metered().await {
        Ok(v) => Some(!matches!(v, 1 | 3)),
        Err(e) => {
            tracing::debug!(error = %e, "NetworkManager metered probe failed; assuming unmetered");
            None
        }
    }
}

#[cfg(not(target_os = "linux"))]
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

#[cfg(not(target_os = "linux"))]
fn on_ac_power() -> Option<bool> {
    None
}

/// Session idle time via logind's caller-session object
/// (`/org/freedesktop/login1/session/auto`): `IdleHint` false ⇒ ZERO,
/// true ⇒ now − `IdleSinceHint` (µs, CLOCK_REALTIME). Sessions whose
/// desktop never sets the hint read as never idle — the builder card
/// says "as reported by the session manager", and fail-open only
/// covers probe *errors*, not an honest "not idle".
#[cfg(target_os = "linux")]
async fn session_idle() -> Option<Duration> {
    async fn idle() -> zbus::Result<Duration> {
        let conn = zbus::Connection::system().await?;
        let proxy = zbus::Proxy::new(
            &conn,
            "org.freedesktop.login1",
            "/org/freedesktop/login1/session/auto",
            "org.freedesktop.login1.Session",
        )
        .await?;
        let hint: bool = proxy.get_property("IdleHint").await?;
        if !hint {
            return Ok(Duration::ZERO);
        }
        let since_us: u64 = proxy.get_property("IdleSinceHint").await?;
        let now_us = u64::try_from(chrono::Utc::now().timestamp_micros()).unwrap_or(0);
        Ok(Duration::from_micros(now_us.saturating_sub(since_us)))
    }
    match idle().await {
        Ok(d) => Some(d),
        Err(e) => {
            tracing::debug!(error = %e, "logind idle probe failed; failing open");
            None
        }
    }
}

#[cfg(not(target_os = "linux"))]
async fn session_idle() -> Option<Duration> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unprobed_conditions_fail_open() {
        let snap = CondSnapshot::default();
        assert!(snap.unmetered());
        assert!(snap.on_ac());
        assert!(snap.idle_at_least(480));
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

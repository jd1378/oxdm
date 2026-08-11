//! Raising an inotify limit, with the user's consent and their
//! system's own authentication.
//!
//! oxdm never elevates itself. The change is handed to `pkexec`, which
//! puts the request in front of the desktop's policy agent — the same
//! prompt the user sees for any other administrative change — and
//! oxdm learns only whether it was allowed.
//!
//! The change is written to `/etc/sysctl.d/` as well as applied, so it
//! survives a reboot. A runtime-only bump would mean the same warning
//! and the same password prompt after every restart, which teaches
//! people to click through prompts.

use crate::domain::WatchLimitKind;

/// The drop-in oxdm owns. Named so it sorts late (overriding
/// distribution defaults) and so it is obvious who wrote it and safe
/// to delete.
const DROP_IN: &str = "/etc/sysctl.d/90-oxdm-inotify.conf";

/// Can this system be offered the one-click fix? Linux with a working
/// `pkexec`. A sandbox without one, or any other OS, gets the command
/// to run and no button — better than a button that cannot work.
pub fn can_raise() -> bool {
    cfg!(target_os = "linux") && which_pkexec().is_some()
}

fn which_pkexec() -> Option<std::path::PathBuf> {
    ["/usr/bin/pkexec", "/bin/pkexec", "/usr/local/bin/pkexec"]
        .iter()
        .map(std::path::PathBuf::from)
        .find(|p| p.exists())
}

/// The shell oxdm asks `pkexec` to run, and the same text the dialog
/// shows. Nothing here comes from the user: `kind` is one of two
/// compile-time keys and `value` is a `u64` this app computed, so the
/// interpolation cannot carry anything but digits.
pub fn command_text(kind: WatchLimitKind, value: u64) -> String {
    format!(
        "printf '%s\\n' '{} = {}' > {DROP_IN} && sysctl -p {DROP_IN}",
        kind.sysctl_key(),
        value
    )
}

/// Ask for the limit to be raised. Blocking — the caller runs it off
/// the UI thread — and finished only when the user has answered the
/// authentication prompt.
pub fn raise(kind: WatchLimitKind, value: u64) -> Result<(), String> {
    let Some(pkexec) = which_pkexec() else {
        return Err("This system has no pkexec to ask for permission with.".to_owned());
    };
    let out = std::process::Command::new(pkexec)
        .arg("sh")
        .arg("-c")
        .arg(command_text(kind, value))
        .output()
        .map_err(|e| format!("Could not ask for permission: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    // 126 / 127 are pkexec's own: dismissed or not authorised. Anything
    // else came from sysctl, and its stderr says more than we could.
    let msg = match out.status.code() {
        Some(126) => "The request was dismissed.".to_owned(),
        Some(127) => "Not authorised to change system settings.".to_owned(),
        _ => {
            let err = String::from_utf8_lossy(&out.stderr);
            let line = err.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
            if line.is_empty() {
                "The change could not be applied.".to_owned()
            } else {
                line.to_owned()
            }
        }
    };
    Err(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The command is shown to the user and run as root. It must be
    /// exactly what it appears to be — one key, one number, one file.
    #[test]
    fn the_command_writes_the_drop_in_and_applies_it() {
        let cmd = command_text(WatchLimitKind::Instances, 1024);
        assert_eq!(
            cmd,
            "printf '%s\\n' 'fs.inotify.max_user_instances = 1024' \
             > /etc/sysctl.d/90-oxdm-inotify.conf && \
             sysctl -p /etc/sysctl.d/90-oxdm-inotify.conf"
        );
    }

    #[test]
    fn each_limit_writes_its_own_key() {
        assert!(
            command_text(WatchLimitKind::Watches, 524_288)
                .contains("fs.inotify.max_user_watches = 524288")
        );
    }
}

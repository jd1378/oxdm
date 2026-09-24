//! Reaching the running oxdm, starting it first when nobody has.

use std::sync::Arc;
use std::time::Duration;

use super::failure::Failure;
use crate::ipc_local::Client;

/// A cold start opens the database and binds the socket; generous,
/// because giving up early leaves a daemon starting with nobody to
/// talk to it.
const START_DEADLINE: Duration = Duration::from_secs(20);

/// Connect to the daemon, starting it in the tray when none is running.
pub async fn connect() -> Result<Arc<Client>, Failure> {
    if let Ok(client) = Client::connect().await {
        return Ok(client);
    }
    start_daemon()?;
    Client::connect_retry(START_DEADLINE)
        .await
        .map_err(|e| Failure::daemon(format!("oxdm did not come up: {e}")))
}

/// A client call's error as a failure. The client flattens two things
/// into its `String`: the daemon's answer, and its own transport errors
/// (`CodecError`'s wording). Only the latter mean oxdm is gone.
pub fn lost(e: String) -> Failure {
    let transport = e == "connection closed" || e.starts_with("io: ") || e.starts_with("json: ");
    if !transport {
        return Failure::new(super::failure::Kind::Other, e);
    }
    // An oxdm upgraded while it ran still runs the old version, and hangs
    // up on requests it has never heard of.
    Failure::daemon(format!(
        "oxdm stopped answering ({e}). If it was updated while running, restart it: \
         `oxdm --quit`, then start it again"
    ))
}

/// Launch `oxdm --tray` detached from this process, so the downloads it
/// takes on outlive the command (and the terminal) that asked for them.
///
/// A second daemon racing this one is harmless: the single-instance lock
/// sends the loser away.
fn start_daemon() -> Result<(), Failure> {
    refuse_without_a_desktop()?;
    let exe = crate::platform::current_exe()
        .map_err(|e| Failure::daemon(format!("cannot find the oxdm executable: {e}")))?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--tray")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    crate::platform::attach_close_high_fds(&mut cmd);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own session: a Ctrl-C or a closed terminal aimed at this
        // command must not reach the daemon.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    cmd.spawn()
        .map(drop)
        .map_err(|e| Failure::daemon(format!("could not start oxdm: {e}")))
}

/// On Linux the daemon inherits this process's environment for good,
/// and every window it opens later comes from it. Started from a shell
/// with no display (ssh, a stripped-down sandbox) it would run downloads
/// the user can never see: better to say so and let them start oxdm
/// from their desktop.
fn refuse_without_a_desktop() -> Result<(), Failure> {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let set = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty());
        if !set("DISPLAY") && !set("WAYLAND_DISPLAY") {
            return Err(Failure::daemon(
                "oxdm is not running, and this shell has no display to start it on; \
                 start oxdm from your desktop (or run `oxdm --tray` there), then retry",
            ));
        }
    }
    Ok(())
}

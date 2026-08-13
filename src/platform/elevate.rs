//! Running one command as an administrator.
//!
//! Used for exactly one thing: installing an update into a directory
//! the user does not own. A copy of oxdm put in `/usr/local/bin` or
//! `C:\Program Files` cannot replace its own files, and the choice is
//! between asking for rights and telling the user to do it by hand.
//!
//! Each platform's own prompt does the asking. oxdm never sees a
//! password, never stores one, and never runs a shell it built out of
//! user input: the program and its arguments are passed as a list, so
//! a path with a space or a quote in it cannot become another command.
//!
//! Declining is a normal answer. Every function here reports "no" the
//! same way whether the mechanism is missing, the prompt was
//! dismissed, or authentication failed, because the caller does the
//! same thing in all three cases: put the old version back and say so.

use std::path::Path;
use std::process::Command;

/// Could this machine even ask? Answered before anything is staged, so
/// oxdm can say "this needs administrator rights and I cannot ask for
/// them here" while there is still a window to say it in.
pub fn available() -> bool {
    #[cfg(target_os = "linux")]
    {
        which("pkexec").is_some()
    }
    #[cfg(target_os = "macos")]
    {
        which("osascript").is_some()
    }
    #[cfg(windows)]
    {
        true
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        false
    }
}

/// Run `program` with `args` as an administrator and wait for it.
///
/// `Ok(())` means the command ran and exited zero. Anything else is an
/// error the caller should treat as "not installed", including the
/// user closing the prompt.
pub fn run(program: &Path, args: &[&str]) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        // pkexec puts the request in front of whatever polkit agent the
        // desktop runs, which is the same dialog the rest of the system
        // uses. `--disable-internal-agent` keeps it from falling back to
        // a text prompt on a terminal nobody is watching.
        let mut cmd = Command::new("pkexec");
        cmd.arg("--disable-internal-agent").arg(program).args(args);
        status(cmd, "pkexec")
    }
    #[cfg(target_os = "macos")]
    {
        // osascript takes one string, so this is the one place a
        // command line has to be built. Every part is quoted for the
        // shell, and then the whole thing for AppleScript.
        let mut line = shell_quote(&program.display().to_string());
        for a in args {
            line.push(' ');
            line.push_str(&shell_quote(a));
        }
        let script = format!(
            "do shell script {} with administrator privileges",
            applescript_quote(&line)
        );
        let mut cmd = Command::new("osascript");
        cmd.arg("-e").arg(script);
        status(cmd, "osascript")
    }
    #[cfg(windows)]
    {
        windows_runas(program, args)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = (program, args);
        Err("this system has no way to ask for administrator rights".into())
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn status(mut cmd: Command, tool: &str) -> Result<(), String> {
    match cmd.status() {
        Ok(s) if s.success() => Ok(()),
        // 126 is pkexec's "not authorised", 127 its "dismissed"; both
        // are the user saying no, which is not a fault to report as
        // one.
        Ok(s) => Err(match s.code() {
            Some(126) | Some(127) => "administrator access was declined".to_string(),
            Some(c) => format!("the elevated command failed (exit {c})"),
            None => "the elevated command was interrupted".to_string(),
        }),
        Err(e) => Err(format!("{tool}: {e}")),
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn which(program: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(program))
            .find(|p| p.is_file())
    })
}

/// Single-quote for `/bin/sh`.
#[cfg(target_os = "macos")]
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Double-quote for AppleScript, where a backslash and a quote are the
/// only characters that need escaping.
#[cfg(target_os = "macos")]
fn applescript_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', r"\\").replace('"', "\\\""))
}

/// `ShellExecuteW` with the `runas` verb, which is what raises the UAC
/// prompt. There is no `Command` equivalent: elevation on Windows is a
/// property of how a process is started, not a program that starts it.
#[cfg(windows)]
fn windows_runas(program: &Path, args: &[&str]) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, INFINITE, WaitForSingleObject,
    };
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };

    fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }
    // Arguments are one string here, so anything with a space is
    // quoted the way the C runtime parses it back.
    let joined = args
        .iter()
        .map(|a| {
            if a.contains(' ') || a.contains('"') {
                format!("\"{}\"", a.replace('"', "\\\""))
            } else {
                (*a).to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let verb = wide("runas");
    let file = wide(&program.display().to_string());
    let params = wide(&joined);

    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = params.as_ptr();
    info.nShow = 0; // SW_HIDE: the child has no window of its own.

    // SAFETY: every pointer above outlives the call, and `hProcess` is
    // only read when the call reports success.
    let started = unsafe { ShellExecuteExW(&mut info) } != 0;
    if !started {
        // The usual reason is the user dismissing the UAC prompt.
        return Err("administrator access was declined".into());
    }
    if info.hProcess.is_null() {
        return Err("the elevated command did not start".into());
    }
    // SAFETY: `hProcess` came from a successful ShellExecuteExW and is
    // closed on every path out.
    let code = unsafe {
        let waited = WaitForSingleObject(info.hProcess, INFINITE);
        let mut code: u32 = 1;
        if waited == WAIT_OBJECT_0 {
            GetExitCodeProcess(info.hProcess, &mut code);
        }
        CloseHandle(info.hProcess);
        code
    };
    if code == 0 {
        Ok(())
    } else {
        Err(format!("the elevated command failed (exit {code})"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whatever the answer is on this machine, asking must not panic
    /// and must not block.
    #[test]
    fn asking_whether_we_can_elevate_is_cheap_and_safe() {
        let _ = available();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_path_with_a_quote_cannot_become_another_command() {
        assert_eq!(shell_quote("/opt/o'brien/oxdm"), r"'/opt/o'\''brien/oxdm'");
        assert_eq!(applescript_quote(r#"say "hi""#), r#""say \"hi\"""#);
    }
}

//! The scripting command line: `oxdm add`, `oxdm wait`, `oxdm list`, …
//!
//! Another client of the daemon, like the windows: every command goes
//! over the same authenticated local socket, so what a script or an AI
//! agent downloads is in the user's list, visible and resumable there.
//! `--format json` is a documented contract (see `args::MACHINE_INTERFACE`
//! and the agent skill under `plugins/oxdm`).

mod add;
mod args;
mod control;
mod daemon;
mod failure;
mod input;
mod output;
mod query;
mod select;
mod view;
mod wait;

use std::ffi::OsString;

use clap::Parser;

use args::{Cli, Command};
use failure::Failure;
use output::{Format, Out};

/// The first words `main` hands to this module rather than to the
/// daemon's own dispatch.
const COMMANDS: &[&str] = &[
    "add",
    "list",
    "ls",
    "status",
    "wait",
    "pause",
    "resume",
    "restart",
    "remove",
    "rm",
    "probe",
    "queues",
    "restore-agent",
    "help",
];

/// Is this invocation for the command line (as opposed to starting the
/// daemon, a window, or one of the internal entry points)?
pub fn handles(first_arg: &str) -> bool {
    COMMANDS.contains(&first_arg) || first_arg == "--format" || first_arg.starts_with("--format=")
}

/// Run one command; returns the process exit code.
pub fn main(argv: Vec<OsString>) -> i32 {
    init_logging();
    let cli = match Cli::try_parse_from(&argv) {
        Ok(cli) => cli,
        Err(e) => return grammar_error(&e, &argv),
    };
    let out = Out { format: cli.format };
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            out.error(&Failure::new(failure::Kind::Other, format!("runtime: {e}")));
            return failure::Kind::Other.exit_code().into();
        }
    };
    match rt.block_on(dispatch(cli.command, out)) {
        Ok(()) => 0,
        Err(f) => {
            out.error(&f);
            f.kind.exit_code().into()
        }
    }
}

async fn dispatch(command: Command, out: Out) -> Result<(), Failure> {
    // Input that can be checked without the daemon is, so a typo never
    // starts one.
    if let Command::Add(a) = command {
        let plan = add::plan(a)?;
        let client = daemon::connect().await?;
        return add::add(&client, plan, out).await;
    }
    if let Command::Probe(a) = &command {
        input::parse_url(&a.url)?;
    }
    let client = daemon::connect().await?;
    match command {
        Command::Add(_) => unreachable!("handled above"),
        Command::List(a) => query::list(&client, a, out).await,
        Command::Status(a) => query::status(&client, a, out).await,
        Command::Wait(a) => {
            let snap = control::snapshot(&client).await?;
            let ids = select::resolve_all(&snap.jobs, &a.jobs)?;
            let already = snap
                .jobs
                .iter()
                .filter(|j| {
                    ids.contains(&j.id) && j.status.phase == crate::domain::Phase::Completed
                })
                .map(|j| j.id)
                .collect();
            wait::follow(&client, ids, already, a.timeout, out).await
        }
        Command::Pause(a) => control::pause(&client, a, out).await,
        Command::Resume(a) => control::resume(&client, a, out).await,
        Command::Restart(a) => control::restart(&client, a, out).await,
        Command::Remove(a) => control::remove(&client, a, out).await,
        Command::Probe(a) => query::probe(&client, a, out).await,
        Command::Queues => query::queues(&client, out).await,
        Command::RestoreAgent => query::restore_agent(&client, out).await,
    }
}

/// Quiet unless asked: stdout is the result, stderr the one error, and a
/// stray log line on either breaks a parser. `RUST_LOG` turns it on.
fn init_logging() {
    if let Ok(filter) = tracing_subscriber::EnvFilter::try_from_default_env() {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .init();
    }
}

/// Help and version go out as clap renders them. A real mistake is a
/// usage error, in JSON when JSON was asked for. The format flag may be
/// part of what failed to parse, so it is looked for by hand.
fn grammar_error(e: &clap::Error, argv: &[OsString]) -> i32 {
    use clap::error::ErrorKind;
    if matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) {
        let _ = e.print();
        return 0;
    }
    let json = argv
        .windows(2)
        .any(|w| w[0] == "--format" && w[1] == "json")
        || argv.iter().any(|a| a == "--format=json");
    if !json {
        let _ = e.print();
        return failure::Kind::Usage.exit_code().into();
    }
    // clap's text without the usage block and help hint that follow it.
    let rendered = e.render().to_string();
    let message = rendered
        .split("\nUsage:")
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_start_matches("error: ")
        .to_owned();
    let f = Failure::usage(message);
    Out {
        format: Format::Json,
    }
    .error(&f);
    f.kind.exit_code().into()
}

#[cfg(test)]
pub(crate) mod fixtures {
    use crate::domain::{Category, Job, JobId, QueueId};

    /// A download with every field at its plainest.
    pub fn job() -> Job {
        Job {
            id: JobId::new(),
            url: url::Url::parse("https://example.com/a.zip").unwrap(),
            save_dir: std::path::PathBuf::from("/nonexistent-oxdm-cli/dl"),
            filename: Some("a.zip".into()),
            referrer: None,
            headers: indexmap::IndexMap::new(),
            max_connections: None,
            proxy: None,
            auth_user: None,
            enc_auth_password: None,
            enc_proxy_password: None,
            enc_cookies: None,
            speed_limit_override: None,
            queue_id: QueueId::new(),
            work_root: None,
            created_at: chrono::Utc::now(),
            started_at: None,
            active_ms: None,
            finished_at: None,
            retries: 0,
            interruptions: 0,
            verify_pending: false,
            status: Default::default(),
            advanced: Default::default(),
            checksums: Vec::new(),
            category: Category::Compressed,
            captured_response: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The daemon's own entry points must keep reaching `main`'s dispatch.
    #[test]
    fn only_command_words_are_taken() {
        for word in ["add", "wait", "list", "help", "--format", "--format=json"] {
            assert!(handles(word), "{word}");
        }
        for word in [
            "gui",
            "--tray",
            "--quit",
            "--help",
            "--version",
            "--install-update",
        ] {
            assert!(!handles(word), "{word}");
        }
    }

    #[test]
    fn a_bad_command_line_is_a_usage_error_even_in_json() {
        let argv: Vec<OsString> = ["oxdm", "--format", "json", "add", "--bogus"]
            .iter()
            .map(OsString::from)
            .collect();
        let e = Cli::try_parse_from(&argv).unwrap_err();
        assert_eq!(grammar_error(&e, &argv), 2);
    }
}

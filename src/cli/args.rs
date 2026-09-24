//! The command line's grammar.

use std::path::PathBuf;
use std::time::Duration;

use clap::{Args, Parser, Subcommand, ValueEnum};

use super::output::Format;

/// Shown under `oxdm help`. A documented contract: scripts and agents
/// may rely on these exit codes and JSON shapes, so keep it in step with
/// `failure::Kind` and `view::Line`.
const MACHINE_INTERFACE: &str = "\
DAEMON:
  Every command talks to the running oxdm, starting it in the tray when
  none is running. Downloads belong to it: they show in its window, and
  they carry on after the command that asked for them exits.

  `add` files new downloads in the \"Agent\" category, saved in its
  folder (e.g. ~/Downloads/Agent), and in the \"Agent\" queue. Both are
  created by the first download and never again: if the user deletes
  the category, downloads are filed by type; if they delete the queue,
  downloads go to Main. -o and --queue override the folder and queue.
  Re-running the same `add` does not start a second copy: it reports the
  download already in the list and resumes it if it had stopped (--new
  always adds). `oxdm restore-agent` brings back a deleted Agent category
  and queue; it is the user's call, so scripts should not run it.

EXIT CODES:
  0    success
  1    other / internal error
  2    usage or invalid input (bad flag, URL, download id or queue name)
  3    network (DNS, connection, HTTP status); see `retryable`
  4    conflict: needs a decision (file changed on server, checksum
       mismatch); `oxdm restart` downloads it again from the start
  5    I/O (disk full, not enough space, destination not writable)
  6    oxdm is not running and could not be started, or stopped answering
  7    stopped: paused, cancelled or removed before it finished
  124  --timeout ran out (the download continues)
  130  interrupted (the download continues)

JSON OUTPUT (--format json):
  stdout carries one JSON object per line, each with a \"type\":
    added        {id, url, queue, save_dir, filename, existing, state}
    rejected     {id, url, error}         (this item was not done)
    paused / resumed / restarted / removed {id, ...}
    phase        {id, phase}
    filename     {id, filename}
    progress     {id, downloaded, total, speed_bps}   (at most 1/s)
    retry_scheduled {id, part, attempt, max_attempts, delay_ms, server_requested}
    completed    {id, path, already_complete}          (terminal)
    failed       {id, phase, error}                    (terminal)
    stopped      {id, phase}                           (terminal)
    downloads    {count, downloads: [...]}             (list, status)
    queues       {queues: [...]}
    agent        {category, save_dir, queue}       (restore-agent)
    probe        {url, filename, size, resumable, etag, last_modified,
                  requires_auth, checksums}
  error objects: {kind, message, retryable}
  `state` of a download: queued, active, paused, completed, failed,
  conflict, cancelled.
  On failure stderr carries one object:
    {\"type\":\"error\", \"kind\", \"message\", \"exit_code\"}";

#[derive(Debug, Parser)]
#[command(
    name = "oxdm",
    version,
    about = "Drive the oxdm download manager from a terminal or a script",
    after_long_help = MACHINE_INTERFACE
)]
pub struct Cli {
    /// Output format; `json` is the stable machine interface
    #[arg(long, value_enum, default_value_t = Format::Text, global = true)]
    pub format: Format,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Add downloads and start them
    Add(AddArgs),
    /// List downloads
    #[command(visible_alias = "ls")]
    List(ListArgs),
    /// Show the named downloads
    Status(JobsArgs),
    /// Wait for downloads to finish, reporting progress
    Wait(WaitArgs),
    /// Pause downloads
    Pause(JobsArgs),
    /// Resume stopped or failed downloads
    Resume(ResumeArgs),
    /// Discard what was downloaded and start again
    Restart(RestartArgs),
    /// Remove downloads from the list
    #[command(visible_alias = "rm")]
    Remove(RemoveArgs),
    /// Ask the server about a URL without downloading it
    Probe(ProbeArgs),
    /// List queues
    Queues,
    /// Recreate the Agent category and queue after they were deleted
    RestoreAgent,
}

#[derive(Debug, Args)]
pub struct WaitOpts {
    /// Wait for the downloads to finish, reporting progress
    #[arg(long)]
    pub wait: bool,
    /// Stop waiting after this long (e.g. 90s, 10m); the downloads continue
    #[arg(long, value_name = "DURATION", requires = "wait", value_parser = parse_duration)]
    pub timeout: Option<Duration>,
}

#[derive(Debug, Args)]
pub struct AddArgs {
    /// http(s) URLs to download
    #[arg(value_name = "URL", required_unless_present = "input")]
    pub urls: Vec<String>,
    /// Also read URLs from FILE, one per line ('-' for stdin); blank lines
    /// and lines starting with '#' or '//' are skipped
    #[arg(short, long, value_name = "FILE")]
    pub input: Option<String>,
    /// Save to PATH: a folder (existing, or ending in '/'), or a file path
    /// when adding one URL. Default: the Agent category's folder
    #[arg(short, long, value_name = "PATH")]
    pub output: Option<PathBuf>,
    /// Queue to add to; it must exist. Default: the Agent category's queue
    /// ("Agent", created on first use)
    #[arg(long, value_name = "NAME")]
    pub queue: Option<String>,
    /// Request header "Name: Value" (repeatable). "@FILE" reads one per
    /// line, "@-" from stdin. Cookie and Basic/Bearer Authorization are
    /// stored encrypted
    #[arg(short = 'H', long = "header", value_name = "HEADER")]
    pub headers: Vec<String>,
    /// Referer to send
    #[arg(long, value_name = "URL")]
    pub referrer: Option<String>,
    /// Parallel connections for these downloads
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..=64))]
    pub connections: Option<u64>,
    /// Expected checksum ALGO[:ENCODING]:DIGEST, checked when the file is
    /// saved: md5, sha1, sha256, sha384 or sha512; ENCODING hex (default)
    /// or base64. Repeatable; one URL only
    #[arg(long, value_name = "ALGO:DIGEST")]
    pub checksum: Vec<String>,
    /// Add without starting
    #[arg(long, conflicts_with = "wait")]
    pub no_start: bool,
    /// Add a new download even if this URL is already in the list
    #[arg(long)]
    pub new: bool,
    /// Open the download window for each download
    #[arg(long)]
    pub show: bool,
    #[command(flatten)]
    pub wait: WaitOpts,
}

/// Coarse states `list --state` filters on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum StateArg {
    Queued,
    Active,
    Paused,
    Completed,
    Failed,
    Conflict,
    Cancelled,
}

#[derive(Debug, Args)]
pub struct ListArgs {
    /// Only downloads whose URL or file name contains TEXT, or whose id
    /// starts with it
    #[arg(value_name = "TEXT")]
    pub filter: Option<String>,
    /// Only downloads in this queue
    #[arg(long, value_name = "NAME")]
    pub queue: Option<String>,
    /// Only downloads in this state (repeatable)
    #[arg(long, value_enum, value_name = "STATE")]
    pub state: Vec<StateArg>,
    /// Only downloads in this category (agent, videos, programs, ...)
    #[arg(long, value_name = "NAME")]
    pub category: Option<String>,
}

#[derive(Debug, Args)]
pub struct JobsArgs {
    /// Download ids, or unique prefixes of them (at least 4 characters)
    #[arg(value_name = "ID", required = true)]
    pub jobs: Vec<String>,
}

#[derive(Debug, Args)]
pub struct WaitArgs {
    /// Download ids, or unique prefixes of them
    #[arg(value_name = "ID", required = true)]
    pub jobs: Vec<String>,
    /// Stop waiting after this long (e.g. 90s, 10m); the downloads continue
    #[arg(long, value_name = "DURATION", value_parser = parse_duration)]
    pub timeout: Option<Duration>,
}

#[derive(Debug, Args)]
pub struct ResumeArgs {
    /// Download ids, or unique prefixes of them
    #[arg(value_name = "ID", required_unless_present = "failed")]
    pub jobs: Vec<String>,
    /// Resume every failed download (in --queue, when given)
    #[arg(long)]
    pub failed: bool,
    /// With --failed: only this queue
    #[arg(long, value_name = "NAME", requires = "failed")]
    pub queue: Option<String>,
    #[command(flatten)]
    pub wait: WaitOpts,
}

#[derive(Debug, Args)]
pub struct RestartArgs {
    /// Download ids, or unique prefixes of them
    #[arg(value_name = "ID", required = true)]
    pub jobs: Vec<String>,
    /// Move the saved file to the trash first. Without it, a download
    /// whose file is still there saves the new copy under a numbered name
    #[arg(long)]
    pub delete_file: bool,
    #[command(flatten)]
    pub wait: WaitOpts,
}

#[derive(Debug, Args)]
pub struct RemoveArgs {
    /// Download ids, or unique prefixes of them
    #[arg(value_name = "ID", required = true)]
    pub jobs: Vec<String>,
    /// Also move the saved file to the trash
    #[arg(long)]
    pub delete_file: bool,
}

#[derive(Debug, Args)]
pub struct ProbeArgs {
    /// http(s) URL to ask about
    #[arg(value_name = "URL")]
    pub url: String,
}

fn parse_duration(s: &str) -> Result<Duration, String> {
    humantime::parse_duration(s).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grammar_is_consistent() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    #[test]
    fn format_is_accepted_before_or_after_the_command() {
        let a = Cli::try_parse_from(["oxdm", "--format", "json", "queues"]).unwrap();
        let b = Cli::try_parse_from(["oxdm", "queues", "--format", "json"]).unwrap();
        assert_eq!(a.format, Format::Json);
        assert_eq!(b.format, Format::Json);
    }

    #[test]
    fn a_timeout_only_makes_sense_while_waiting() {
        assert!(Cli::try_parse_from(["oxdm", "add", "https://e.x/f", "--timeout", "5s"]).is_err());
        assert!(
            Cli::try_parse_from(["oxdm", "add", "https://e.x/f", "--no-start", "--wait"]).is_err()
        );
        let ok = Cli::try_parse_from(["oxdm", "add", "https://e.x/f", "--wait", "--timeout", "2m"])
            .unwrap();
        let Command::Add(add) = ok.command else {
            panic!("not add")
        };
        assert_eq!(add.wait.timeout, Some(Duration::from_secs(120)));
    }

    #[test]
    fn resume_takes_ids_or_failed() {
        assert!(Cli::try_parse_from(["oxdm", "resume"]).is_err());
        assert!(Cli::try_parse_from(["oxdm", "resume", "--failed"]).is_ok());
        assert!(Cli::try_parse_from(["oxdm", "resume", "abcd1234"]).is_ok());
        assert!(Cli::try_parse_from(["oxdm", "resume", "abcd", "--queue", "Agent"]).is_err());
    }
}

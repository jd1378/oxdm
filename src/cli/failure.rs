//! Why a command did not do what it was asked, as a category a script
//! can branch on. Each kind owns one exit code; the pairing is part of
//! the documented contract (`oxdm help`), so neither may change.

use crate::domain::JobError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Other,
    /// The command itself is wrong: a bad flag, URL, id or queue name.
    Usage,
    /// The link or the server: DNS, connection, HTTP status.
    Network,
    /// A decision only a person can make: the file changed on the
    /// server, the checksum did not match, the name is taken.
    Conflict,
    /// The disk: out of space, not writable.
    Io,
    /// oxdm is not running and could not be started, or stopped
    /// answering.
    Daemon,
    /// Paused, cancelled or removed by someone else before it finished.
    Stopped,
    /// `--timeout` ran out; the download goes on without us.
    Timeout,
    /// Ctrl-C on this command; the download goes on without us.
    Interrupted,
}

impl Kind {
    pub fn exit_code(self) -> u8 {
        match self {
            Kind::Other => 1,
            Kind::Usage => 2,
            Kind::Network => 3,
            Kind::Conflict => 4,
            Kind::Io => 5,
            Kind::Daemon => 6,
            Kind::Stopped => 7,
            Kind::Timeout => 124,
            Kind::Interrupted => 130,
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            Kind::Other => "other",
            Kind::Usage => "usage",
            Kind::Network => "network",
            Kind::Conflict => "conflict",
            Kind::Io => "io",
            Kind::Daemon => "daemon",
            Kind::Stopped => "stopped",
            Kind::Timeout => "timeout",
            Kind::Interrupted => "interrupted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub kind: Kind,
    pub message: String,
}

impl Failure {
    pub fn new(kind: Kind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn usage(message: impl Into<String>) -> Self {
        Self::new(Kind::Usage, message)
    }

    pub fn daemon(message: impl Into<String>) -> Self {
        Self::new(Kind::Daemon, message)
    }

    pub fn from_job_error(e: &JobError) -> Self {
        Self::new(classify(e).0, describe(e))
    }
}

/// The kind of a download's error, and whether trying again as it is
/// can be expected to help.
pub fn classify(e: &JobError) -> (Kind, bool) {
    match e {
        JobError::Network(_) | JobError::Dns { .. } => (Kind::Network, true),
        // Refusals that are the server's final word (403, 404, 410) are
        // not worth repeating; overload, rate limits and outages are.
        JobError::HttpStatus { code, .. } => {
            (Kind::Network, *code == 408 || *code == 429 || *code >= 500)
        }
        JobError::ServerConflict(_)
        | JobError::NotResumable(_)
        | JobError::FileChanged(_)
        | JobError::SaveConflict(_)
        | JobError::DuplicateActive { .. }
        | JobError::NameTaken { .. }
        | JobError::ChecksumMismatch { .. }
        | JobError::ConflictPending(_) => (Kind::Conflict, false),
        JobError::Io(_)
        | JobError::DiskFull(_)
        | JobError::InsufficientSpace { .. }
        | JobError::PermissionDenied(_) => (Kind::Io, false),
        JobError::Cancelled => (Kind::Stopped, false),
        // Not a failure: the download is queued and starts by itself.
        JobError::Deferred => (Kind::Other, true),
        JobError::Other(_) => (Kind::Other, false),
    }
}

/// The error in words, without the "needs your answer" wrapper a parked
/// conflict carries: what the reader has to decide about is the cause.
pub fn describe(e: &JobError) -> String {
    match e {
        JobError::ConflictPending(cause) => cause.to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The codes are a published contract: scripts branch on them.
    #[test]
    fn every_kind_keeps_its_documented_exit_code() {
        let table = [
            (Kind::Other, 1),
            (Kind::Usage, 2),
            (Kind::Network, 3),
            (Kind::Conflict, 4),
            (Kind::Io, 5),
            (Kind::Daemon, 6),
            (Kind::Stopped, 7),
            (Kind::Timeout, 124),
            (Kind::Interrupted, 130),
        ];
        for (kind, code) in table {
            assert_eq!(kind.exit_code(), code, "{kind:?}");
        }
    }

    #[test]
    fn a_server_that_refuses_for_good_is_not_worth_retrying() {
        let status = |code| JobError::HttpStatus {
            code,
            reason: None,
            url: None,
        };
        assert_eq!(classify(&status(404)), (Kind::Network, false));
        assert_eq!(classify(&status(403)), (Kind::Network, false));
        assert_eq!(classify(&status(429)), (Kind::Network, true));
        assert_eq!(classify(&status(503)), (Kind::Network, true));
        assert_eq!(
            classify(&JobError::Network("reset".into())),
            (Kind::Network, true)
        );
    }

    #[test]
    fn a_parked_conflict_is_a_conflict_described_by_its_cause() {
        let e = JobError::ConflictPending(Box::new(JobError::FileChanged("size".into())));
        assert_eq!(classify(&e).0, Kind::Conflict);
        assert_eq!(describe(&e), "the file on the server changed: size");
    }

    #[test]
    fn disk_trouble_is_io() {
        let e = JobError::InsufficientSpace {
            path: "/dl".into(),
            needed: 10,
            available: 1,
        };
        assert_eq!(classify(&e).0, Kind::Io);
        assert_eq!(classify(&JobError::DiskFull("x".into())).0, Kind::Io);
    }
}

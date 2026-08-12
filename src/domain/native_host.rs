//! What installing the browser bridge did, in terms the UI can show.
//!
//! The install itself is platform work and lives in `data`; these are
//! the results it reports back — one row per browser it found, plus
//! whatever the user still has to do by hand.

use serde::{Deserialize, Serialize};

/// The name the extension passes to `runtime.connectNative()`. The
/// manifest is stored under this name, and the browser will not look
/// for any other.
pub const HOST_NAME: &str = "io.github.jd1378.oxdm.host";

/// oxdm's own extension, as published. A user pairing a build of their
/// own passes their id instead; these are what the app installs when
/// nobody says otherwise.
pub const CHROMIUM_EXTENSION_ID: &str = "bfefefnlghppdcgjjimkllklpifkcokj";
pub const FIREFOX_EXTENSION_ID: &str = "oxdm@jd1378.github.io";

/// Which manifest shape a browser wants. Chromium lists origins,
/// Firefox lists extension ids; the two are not interchangeable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Family {
    Chromium,
    Firefox,
}

/// How a browser is installed, which decides whether the manifest can
/// name the host binary directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Packaging {
    /// Ordinary install: it can execute any path the manifest names.
    Native,
    /// Flatpak. Runs sandboxed, so it needs a wrapper inside its own
    /// data dir and a filesystem grant before it can reach ours.
    Flatpak,
    /// Snap. The manifest path is right, but confinement may refuse to
    /// execute a host outside the snap — best effort.
    Snap,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HostOutcome {
    /// The manifest was written (or rewritten over a stale one).
    Written,
    /// Already said exactly this. Reruns are meant to be boring.
    Unchanged,
    Failed(String),
}

/// One browser oxdm found, and what happened to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostEntry {
    /// What to call it in the UI — "Firefox", "Chromium (Flatpak)".
    pub browser: String,
    pub family: Family,
    pub packaging: Packaging,
    /// Where the manifest went.
    pub manifest: String,
    pub outcome: HostOutcome,
}

impl HostEntry {
    pub fn ok(&self) -> bool {
        !matches!(self.outcome, HostOutcome::Failed(_))
    }
}

/// The whole run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostReport {
    pub entries: Vec<HostEntry>,
    /// Commands the user must run themselves: a Flatpak browser cannot
    /// be granted access to a path from inside oxdm, and doing it for
    /// them would mean shelling out to `flatpak override` with their
    /// rights — a bigger promise than "we wrote a file".
    pub flatpak_grants: Vec<String>,
    /// Nothing was found to install into. Distinguished from "all
    /// good" so the UI can say which.
    pub no_browsers: bool,
}

impl HostReport {
    pub fn failures(&self) -> usize {
        self.entries.iter().filter(|e| !e.ok()).count()
    }

    pub fn installed(&self) -> usize {
        self.entries.iter().filter(|e| e.ok()).count()
    }

    /// One line for a notification or a CLI summary.
    pub fn summary(&self) -> String {
        if self.no_browsers {
            return "No supported browser found.".to_owned();
        }
        let failed = self.failures();
        let ok = self.installed();
        let mut s = format!(
            "{ok} browser{} set up for oxdm",
            if ok == 1 { "" } else { "s" }
        );
        if failed > 0 {
            s.push_str(&format!(", {failed} failed"));
        }
        s
    }
}

//! Self-update transport.
//!
//! Two implementations live here:
//!
//! - [`NoopUpdateChannel`] — default. `check` returns `Ok(None)`, so the
//!   "Check for updates" button reports "you're up to date" without any
//!   network activity. Used when the user has not configured a feed.
//! - [`HttpFeedUpdateChannel`] — fetches a small JSON document, semver-
//!   compares against the running version, and downloads the artifact
//!   to a temp file. **Apply** is intentionally non-destructive: it
//!   prepares the new binary at a sibling path and lets the user
//!   restart manually. Replacing a running executable across all three
//!   platforms is fragile, so v0 stops at "downloaded; restart to
//!   apply" rather than risk corrupting the install.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[async_trait]
pub trait UpdateChannel: Send + Sync + 'static {
    /// Lightweight feed check (no download). Used for the "Check for
    /// updates" button.
    async fn check(&self) -> Result<Option<UpdateInfo>, String>;
    /// Where the feed lives — the updater process refetches it itself
    /// so the heavy lifting (download, hash, swap) all stays in one
    /// process boundary.
    fn feed_url(&self) -> Option<url::Url>;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateInfo {
    pub version: String,
    pub url: url::Url,
    pub notes: Option<String>,
    /// Lowercase hex SHA-256 of the artifact at `url`. Verification is
    /// **mandatory** — the updater process refuses to install when the
    /// hash is missing or does not match.
    pub sha256: String,
}

/// Status messages the installer prints on stdout, one JSON per line.
/// The download runs through the regular `DownloadManager`, which
/// checks the digest; oxdm then re-runs itself from a copy
/// (`--install-update`) to swap the files and relaunch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum UpdaterEvent {
    Started,
    Verified,
    Ready,
    Installing,
    Done,
    Error { message: String },
}

/// Default no-op channel. `check` returns `Ok(None)`.
pub struct NoopUpdateChannel;

#[async_trait]
impl UpdateChannel for NoopUpdateChannel {
    async fn check(&self) -> Result<Option<UpdateInfo>, String> {
        Ok(None)
    }
    fn feed_url(&self) -> Option<url::Url> {
        None
    }
}

/// HTTP-feed implementation. Constructed with the feed URL from
/// [`built_in_feed_url`]. Returns `Ok(None)` when the feed
/// reports a version less-than-or-equal to ours.
pub struct HttpFeedUpdateChannel {
    feed_url: url::Url,
    current_version: String,
    client: reqwest::Client,
}

impl HttpFeedUpdateChannel {
    pub fn new(feed_url: url::Url, current_version: String) -> Self {
        let client = reqwest::Client::builder()
            // The same identity as every other request oxdm makes.
            // `current_version` is for comparing against the feed, not
            // for naming ourselves.
            .user_agent(crate::domain::default_user_agent())
            .build()
            .expect("reqwest client");
        Self {
            feed_url,
            current_version,
            client,
        }
    }
}

#[async_trait]
impl UpdateChannel for HttpFeedUpdateChannel {
    async fn check(&self) -> Result<Option<UpdateInfo>, String> {
        let resp = self
            .client
            .get(self.feed_url.clone())
            .send()
            .await
            .map_err(|e| format!("feed fetch: {e}"))?
            .error_for_status()
            .map_err(|e| format!("feed status: {e}"))?;
        let info: UpdateInfo = resp.json().await.map_err(|e| format!("feed parse: {e}"))?;

        let current = semver::Version::parse(&self.current_version)
            .map_err(|e| format!("current version semver: {e}"))?;
        let upstream = semver::Version::parse(&info.version)
            .map_err(|e| format!("feed version semver: {e}"))?;
        if upstream > current {
            Ok(Some(info))
        } else {
            Ok(None)
        }
    }

    fn feed_url(&self) -> Option<url::Url> {
        Some(self.feed_url.clone())
    }
}

/// Is this process running from an AppImage?
///
/// The AppImage runtime exports `APPIMAGE` with the path of the bundle
/// itself. It matters twice over: the artifact to fetch is a whole
/// AppImage rather than a bare executable, and the file to replace is
/// the bundle, not `current_exe()` — which points inside a read-only
/// mount that disappears when the app exits.
pub fn running_as_appimage() -> Option<std::path::PathBuf> {
    std::env::var_os("APPIMAGE")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
}

/// The feed for this build, as it is currently running.
///
/// Resolved per check rather than stored in settings: the same
/// installed files can be launched as an AppImage or not, and each
/// wants a different artifact. `releases/latest` keeps the URL stable
/// across releases and resolves to the newest one not tagged as a
/// pre-release.
pub fn built_in_feed_url() -> String {
    feed_url_for(env!("OXDM_TARGET"), running_as_appimage().is_some())
}

/// The feed naming the artifact this flavour of install replaces
/// itself with.
fn feed_url_for(target: &str, appimage: bool) -> String {
    let flavour = if appimage { "-appimage" } else { "" };
    format!("https://github.com/jd1378/oxdm/releases/latest/download/update-{target}{flavour}.json")
}

/// The channel this build updates through.
///
/// There is one, and it is not configurable. A feed decides which
/// binary oxdm replaces itself with, so pointing it elsewhere is
/// pointing the app at a different program to become — a setting worth
/// having only if someone would genuinely use it, and nothing in the
/// UI ever offered it.
pub fn built_in() -> Arc<dyn UpdateChannel> {
    channel_for(&built_in_feed_url())
}

/// The one place a feed URL becomes a channel, so the https rule holds
/// wherever the URL came from.
fn channel_for(url: &str) -> Arc<dyn UpdateChannel> {
    match url::Url::parse(url) {
        // https only. The feed names the next program the user runs
        // and carries the digest that program is checked against, so
        // anyone able to rewrite it in flight chooses both — which is
        // exactly why the document itself has to be authenticated.
        // Belt and braces over a URL this build assembles itself.
        Ok(u) if u.scheme() == "https" => Arc::new(HttpFeedUpdateChannel::new(
            u,
            env!("CARGO_PKG_VERSION").to_string(),
        )),
        Ok(u) => {
            tracing::warn!(url = %u, "update feed ignored: only https feeds are used");
            Arc::new(NoopUpdateChannel)
        }
        Err(_) => Arc::new(NoopUpdateChannel),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The URL is assembled from a constant and this build's target,
    /// so it is https by construction — but the rule is checked at the
    /// point of use, not assumed from where the string came from.
    #[test]
    fn only_an_https_feed_becomes_a_channel() {
        assert!(
            channel_for("https://example.com/feed.json")
                .feed_url()
                .is_some()
        );
        assert!(
            channel_for("http://example.com/feed.json")
                .feed_url()
                .is_none()
        );
        assert!(channel_for("file:///tmp/feed.json").feed_url().is_none());
        assert!(channel_for("not a url").feed_url().is_none());
    }

    /// An installed build and a bundle are updated with different
    /// artifacts, so they read different feeds — and the same files
    /// can be run either way, which is why this is decided per check
    /// rather than stored.
    #[test]
    fn each_flavour_reads_its_own_feed() {
        assert!(
            feed_url_for("x86_64-unknown-linux-gnu", false)
                .ends_with("/update-x86_64-unknown-linux-gnu.json")
        );
        assert!(
            feed_url_for("x86_64-unknown-linux-gnu", true)
                .ends_with("/update-x86_64-unknown-linux-gnu-appimage.json")
        );
        // Always the `latest` release, so the URL survives releases.
        assert!(feed_url_for("aarch64-apple-darwin", false).contains("/releases/latest/download/"));
    }

    /// There is one feed and this build knows it.
    #[test]
    fn the_channel_reads_the_built_in_feed() {
        let url = built_in().feed_url().expect("built-in feed");
        assert_eq!(url.scheme(), "https");
        assert!(url.as_str().contains("/releases/latest/download/update-"));
    }
}

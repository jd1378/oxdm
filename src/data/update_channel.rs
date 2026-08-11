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

/// Status messages emitted by `oxdm-updater` on stdout, one JSON per
/// line. Artifact-mode helper — the GUI does the actual download via
/// the regular `DownloadManager`, then hands the assembled file to
/// the helper for verify + swap + relaunch.
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
/// `Settings::update_feed_url`. Returns `Ok(None)` when the feed
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

/// Build the channel that matches the current settings. Called by
/// `AppState::update_channel`.
pub fn from_settings(s: &crate::domain::Settings) -> Arc<dyn UpdateChannel> {
    let url = s.update_feed_url.trim();
    if url.is_empty() {
        return Arc::new(NoopUpdateChannel);
    }
    match url::Url::parse(url) {
        Ok(u) => Arc::new(HttpFeedUpdateChannel::new(
            u,
            env!("CARGO_PKG_VERSION").to_string(),
        )),
        Err(_) => Arc::new(NoopUpdateChannel),
    }
}

//! What a probe asks a link with.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::{AuthScheme, CaptureRequest, Creds, ProxyMode};

/// A link and everything its download would send it. A probe that asks
/// with less can be told about another file, or refused: a session, a
/// password or a user agent each change what a server answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeTarget {
    pub url: url::Url,
    #[serde(default)]
    pub referrer: Option<url::Url>,
    /// As a job keeps them: `User-Agent` among them, `Cookie` not.
    #[serde(default)]
    pub headers: IndexMap<String, String>,
    #[serde(default)]
    pub cookies: Option<String>,
    /// Plaintext, as a form holds them.
    #[serde(default)]
    pub creds: Creds,
}

impl ProbeTarget {
    pub fn bare(url: url::Url) -> Self {
        Self {
            url,
            referrer: None,
            headers: IndexMap::new(),
            cookies: None,
            creds: Creds::default(),
        }
    }

    /// What a job made from `capture` sends.
    pub fn from_capture(capture: &CaptureRequest) -> Self {
        let (headers, cookies) = capture.job_headers();
        Self {
            referrer: capture.referrer.clone(),
            headers,
            cookies,
            ..Self::bare(capture.url.clone())
        }
    }

    /// Whether it asks with nothing but the link.
    pub fn is_bare(&self) -> bool {
        self.referrer.is_none()
            && self.headers.is_empty()
            && self.cookies.as_deref().is_none_or(str::is_empty)
            && self.creds.proxy.mode == ProxyMode::Inherit
            && self.creds.auth.scheme == AuthScheme::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only a target that adds nothing to the link may go out as a
    /// plain `Probe`, which a daemon answers for the link alone.
    #[test]
    fn a_target_is_bare_only_when_it_adds_nothing() {
        let url: url::Url = "https://example.com/f.zip".parse().unwrap();
        assert!(ProbeTarget::bare(url.clone()).is_bare());

        let mut capture = CaptureRequest::from_url(url.clone());
        assert!(ProbeTarget::from_capture(&capture).is_bare());
        capture.cookies = Some("sid=1".into());
        assert!(!ProbeTarget::from_capture(&capture).is_bare());

        let mut signed = ProbeTarget::bare(url.clone());
        signed.creds.auth.scheme = AuthScheme::Bearer;
        assert!(!signed.is_bare());
        let mut proxied = ProbeTarget::bare(url);
        proxied.creds.proxy.mode = ProxyMode::None;
        assert!(!proxied.is_bare());
    }
}

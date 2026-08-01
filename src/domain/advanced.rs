//! Per-job advanced settings — the "Advanced", "Connection", "Cookies"
//! and "Headers" tabs of the Properties dialog. Pulled into `domain`
//! (out of the dialog file) so the IPC protocol can ferry the whole
//! bundle without pulling in any UI types.
//!
//! The on-the-wire shape is a single `Advanced` blob; the daemon
//! stores it as JSON in the `advanced_json` column on `jobs`. Defaults
//! mirror the dialog's old hard-coded defaults.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyMode {
    Inherit,
    None,
    System,
    Http,
    Https,
    Socks5,
}

impl Default for ProxyMode {
    fn default() -> Self {
        Self::Inherit
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyAdv {
    pub mode: ProxyMode,
    pub host: String,
    pub port: String,
    pub auth_enabled: bool,
    pub username: String,
    /// Plaintext password as edited in the dialog — UI-side scratch
    /// only. `set_job_advanced` strips it from the blob and routes it
    /// onto the encrypted `Job::enc_proxy_password` column.
    pub password: String,
    /// Set by the UI when the user emptied a password field that held a
    /// stored secret. Without it an empty `password` is ambiguous —
    /// "keep what's stored" (the common case, since the ciphertext
    /// never round-trips into the form) versus "delete it".
    /// `set_job_advanced` consumes the flag and never persists it.
    #[serde(default)]
    pub clear_password: bool,
    pub remote_dns: bool,
    /// Unused: odl exposes no `no_proxy`/bypass API. Kept for serde
    /// compat with persisted blobs; never surfaced in the UI.
    pub bypass: String,
}

impl Default for ProxyAdv {
    fn default() -> Self {
        Self {
            mode: ProxyMode::Inherit,
            host: String::new(),
            port: String::new(),
            auth_enabled: false,
            username: String::new(),
            password: String::new(),
            clear_password: false,
            remote_dns: true,
            bypass: String::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthScheme {
    None,
    Basic,
    Bearer,
    Digest,
}

impl Default for AuthScheme {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AuthAdv {
    pub scheme: AuthScheme,
    pub username: String,
    /// See `ProxyAdv::password` — same caveat applies.
    pub password: String,
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CustomHeader {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Advanced {
    pub user_agent: String,
    pub referer: String,
    pub cookies_enabled: bool,
    pub cookie_jar: String,
    pub segments: i64,
    /// 0 = unlimited.
    pub speed_kbps: i64,
    #[serde(default)]
    pub speed_unit_mb: bool,
    pub timeout: i64,
    pub retries: i64,
    pub auto_verify: bool,
    pub open_when_done: bool,
    pub run_command: String,
    #[serde(default)]
    pub headers: Vec<CustomHeader>,
    #[serde(default)]
    pub proxy: ProxyAdv,
    #[serde(default)]
    pub auth: AuthAdv,
}

impl Default for Advanced {
    fn default() -> Self {
        Self {
            user_agent: "oxdm/2.4.1 (Macintosh; arm64; like wget)".into(),
            referer: String::new(),
            cookies_enabled: true,
            cookie_jar: String::new(),
            segments: 8,
            speed_kbps: 0,
            speed_unit_mb: false,
            timeout: 30,
            retries: 5,
            auto_verify: true,
            open_when_done: false,
            run_command: String::new(),
            headers: Vec::new(),
            proxy: ProxyAdv::default(),
            auth: AuthAdv::default(),
        }
    }
}

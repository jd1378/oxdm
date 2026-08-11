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
    /// See `ProxyAdv::clear_password`. Covers whichever secret the
    /// current scheme uses — both land on `Job::enc_auth_password`.
    #[serde(default)]
    pub clear_secret: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CustomHeader {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Advanced {
    // No `user_agent` / `referer` here: identification is not a
    // per-job blob field. The UA rides the job's `User-Agent` header
    // (which `start_job` promotes to odl's UA option) and the referrer
    // rides `Job::referrer`. Old blobs still carry both keys; serde
    // drops them on load.
    pub cookies_enabled: bool,
    pub cookie_jar: String,
    /// See `ProxyAdv::clear_password`. An emptied cookie editor is
    /// otherwise indistinguishable from "the stored jar never came
    /// back down to the form", which is the normal case.
    #[serde(default)]
    pub clear_cookie_jar: bool,
    pub segments: i64,
    /// 0 = unlimited.
    pub speed_kbps: i64,
    #[serde(default)]
    pub speed_unit_mb: bool,
    pub timeout: i64,
    pub retries: i64,
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
            cookies_enabled: true,
            cookie_jar: String::new(),
            clear_cookie_jar: false,
            segments: 8,
            speed_kbps: 0,
            speed_unit_mb: false,
            timeout: 30,
            retries: 5,
            open_when_done: false,
            run_command: String::new(),
            headers: Vec::new(),
            proxy: ProxyAdv::default(),
            auth: AuthAdv::default(),
        }
    }
}

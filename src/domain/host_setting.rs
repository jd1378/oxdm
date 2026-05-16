//! Per-host overrides applied at evaluate time.
//!
//! Lookup is exact, case-insensitive on the URL host. Resolution order:
//! global `Settings` → per-host overrides → per-job overrides (per-job
//! still wins).
//!
//! Passwords are **never** stored in this struct or in SQLite. The
//! presence of a password is tracked via `has_password`; the secret
//! itself lives in the OS keyring under service `"oxdm"`, account
//! `"host:<host>"`. See PLAN §10.9.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostSetting {
    /// Exact host match, case-insensitive (e.g. `example.com`,
    /// `cdn.example.com`). No glob, no port stripping yet.
    pub host: String,
    /// `None` = unlimited.
    pub speed_limit: Option<u64>,
    /// `None` = inherit `Settings::max_connections`.
    pub thread_count: Option<u64>,
    pub username: Option<String>,
    /// Sentinel — `true` means a password exists in the OS keyring.
    /// The secret value never travels through this struct.
    #[serde(default)]
    pub has_password: bool,
    pub default_user_agent: Option<String>,
}

impl HostSetting {
    pub fn new(host: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            speed_limit: None,
            thread_count: None,
            username: None,
            has_password: false,
            default_user_agent: None,
        }
    }

    /// Normalised host key — lowercased so lookups are case-insensitive.
    pub fn host_key(host: &str) -> String {
        host.to_ascii_lowercase()
    }
}

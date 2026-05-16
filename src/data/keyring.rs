//! OS keyring wrapper.
//!
//! Service `"oxdm"`. Accounts:
//!   - `host:<lowercased-host>` — Settings → Hosts password.
//!   - `master-key` — base64-encoded 32-byte AES-256-GCM key. Single
//!     entry that protects every per-job secret stored in the DB.
//!
//! Failures (no Secret Service running, locked keychain, …) are
//! surfaced as errors; callers decide whether to prompt the user or
//! fall back to a degraded mode.

use crate::domain::HostSetting;

const SERVICE: &str = "oxdm";
const MASTER_KEY_ACCOUNT: &str = "master-key";

fn entry_for(account: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, account).map_err(|e| e.to_string())
}

fn host_account(host: &str) -> String {
    format!("host:{}", HostSetting::host_key(host))
}

pub fn set_password(host: &str, password: &str) -> Result<(), String> {
    entry_for(&host_account(host))?
        .set_password(password)
        .map_err(|e| e.to_string())
}

pub fn get_password(host: &str) -> Result<Option<String>, String> {
    get_secret(&host_account(host))
}

pub fn delete_password(host: &str) -> Result<(), String> {
    delete_secret(&host_account(host))
}

pub fn get_master_key() -> Result<Option<String>, String> {
    get_secret(MASTER_KEY_ACCOUNT)
}

pub fn set_master_key(b64: &str) -> Result<(), String> {
    entry_for(MASTER_KEY_ACCOUNT)?
        .set_password(b64)
        .map_err(|e| e.to_string())
}

#[allow(dead_code)]
pub fn delete_master_key() -> Result<(), String> {
    delete_secret(MASTER_KEY_ACCOUNT)
}

fn get_secret(account: &str) -> Result<Option<String>, String> {
    match entry_for(account)?.get_password() {
        Ok(p) => Ok(Some(p)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

fn delete_secret(account: &str) -> Result<(), String> {
    match entry_for(account)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

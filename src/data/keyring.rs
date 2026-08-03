//! OS keyring wrapper.
//!
//! Service `"oxdm"`, one account: `master-key` — a base64-encoded
//! 32-byte AES-256-GCM key that protects every per-job secret stored in
//! the DB.
//!
//! Failures (no Secret Service running, locked keychain, …) are
//! surfaced as errors; callers decide whether to prompt the user or
//! fall back to a degraded mode.

const SERVICE: &str = "oxdm";
const MASTER_KEY_ACCOUNT: &str = "master-key";

fn entry_for(account: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, account).map_err(|e| e.to_string())
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

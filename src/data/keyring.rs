//! OS keyring wrapper.
//!
//! Service `"oxdm"`, one account: `master-key` — a base64-encoded
//! 32-byte AES-256-GCM key that protects every per-job secret stored in
//! the DB.
//!
//! Failures (no Secret Service running, locked keychain, …) are
//! surfaced as errors; callers decide whether to prompt the user or
//! fall back to a degraded mode.
//!
//! Every call runs on a thread of its own, and that is not incidental.
//! The Linux store talks Secret Service over zbus, and zbus builds its
//! blocking wrapper on whatever async runtime the build selected —
//! tokio, here, because oxdm asks for that feature elsewhere and cargo
//! unifies it. Blocking on a runtime from a thread already inside one
//! is an immediate "Cannot start a runtime from within a runtime"
//! panic, which is what the daemon did at boot. A plain OS thread
//! carries no runtime context, so the store gets to make its own
//! arrangements. These calls happen at boot and when the user
//! regenerates a key, so a thread each costs nothing worth counting.

const SERVICE: &str = "oxdm";
const MASTER_KEY_ACCOUNT: &str = "master-key";

fn entry_for(account: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, account).map_err(|e| e.to_string())
}

/// Run one keyring operation clear of any async runtime. See the module
/// note for why this is not optional.
fn off_runtime<T, F>(what: &'static str, f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    std::thread::Builder::new()
        .name(format!("keyring-{what}"))
        .spawn(f)
        .map_err(|e| e.to_string())?
        .join()
        .map_err(|_| format!("the keyring {what} thread panicked"))?
}

pub fn get_master_key() -> Result<Option<String>, String> {
    get_secret(MASTER_KEY_ACCOUNT)
}

pub fn set_master_key(b64: &str) -> Result<(), String> {
    let b64 = b64.to_owned();
    off_runtime("set", move || {
        entry_for(MASTER_KEY_ACCOUNT)?
            .set_password(&b64)
            .map_err(|e| e.to_string())
    })
}

#[allow(dead_code)]
pub fn delete_master_key() -> Result<(), String> {
    delete_secret(MASTER_KEY_ACCOUNT)
}

fn get_secret(account: &str) -> Result<Option<String>, String> {
    let account = account.to_owned();
    off_runtime("get", move || match entry_for(&account)?.get_password() {
        Ok(p) => Ok(Some(p)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.to_string()),
    })
}

fn delete_secret(account: &str) -> Result<(), String> {
    let account = account.to_owned();
    off_runtime("delete", move || {
        match entry_for(&account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    })
}

#[cfg(test)]
mod tests {
    /// The boot path, in the shape that panicked: a keyring call made
    /// from inside a multi-threaded runtime.
    ///
    /// Ignored by default because it needs a real, unlocked Secret
    /// Service — CI has none. Run it by hand on a desktop session:
    /// `cargo test --lib keyring -- --ignored`. It writes and removes
    /// its own account and never touches the master key.
    #[test]
    #[ignore = "needs a desktop keyring"]
    fn a_keyring_call_survives_being_made_from_inside_a_runtime() {
        const ACCOUNT: &str = "self-test";
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            super::off_runtime("set", move || {
                super::entry_for(ACCOUNT)?
                    .set_password("probe")
                    .map_err(|e| e.to_string())
            })
            .expect("set from inside a runtime");
            assert_eq!(
                super::get_secret(ACCOUNT).unwrap().as_deref(),
                Some("probe")
            );
            super::delete_secret(ACCOUNT).unwrap();
            assert_eq!(super::get_secret(ACCOUNT).unwrap(), None);
        });
    }
}

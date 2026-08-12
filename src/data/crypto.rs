//! Field-level encryption for per-job secrets (HTTP Basic password,
//! proxy password, cookies).
//!
//! Threat model: protect secrets at rest in the on-disk SQLite DB. The
//! master key lives in the user's OS keyring (single entry —
//! `oxdm/master-key`); the DB stores only ciphertext. An attacker with
//! the DB alone learns nothing; an attacker with both the DB and the
//! keyring is at parity with plaintext storage (out of scope).
//!
//! Wire format for each encrypted field, base64-encoded as a `TEXT`
//! column:
//!
//! ```text
//! [ version(1) | nonce(12) | ciphertext+gcm_tag(N+16) ]
//! ```
//!
//! - `version = 0x01` — single key version today. Future rotations
//!   bump this and the loader picks the right key per row.
//! - `nonce` — 96 random bits, per-encryption. AES-GCM nonce reuse is
//!   catastrophic, so a fresh `OsRng` draw on every `encrypt` call.
//! - AAD binds the ciphertext to its `(job_id, field_name)` slot so
//!   an attacker with DB write access cannot swap a stored
//!   `auth_password` into a `cookies` column and have the runner
//!   happily decrypt it as the wrong field.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use rand::TryRng;

use crate::domain::JobId;

const VERSION: u8 = 0x01;
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;

/// In-memory holder for the AES-256 key. Constructed once at daemon
/// boot via [`MasterKey::bootstrap`]; the byte buffer is wrapped in
/// `aes_gcm`'s zero-on-drop machinery via the cipher itself.
#[derive(Clone)]
pub struct MasterKey(Aes256Gcm);

impl std::fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MasterKey(***)")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("keyring unavailable: {0}")]
    Keyring(String),
    #[error("master key missing from keyring")]
    Missing,
    #[error("malformed key material: {0}")]
    KeyMaterial(String),
    #[error("malformed ciphertext: {0}")]
    Format(String),
    #[error("decryption failed")]
    Decrypt,
}

/// Field tag included in the AAD so a ciphertext cannot be lifted
/// from one column and replayed under another.
#[derive(Debug, Clone, Copy)]
pub enum Field {
    AuthPassword,
    ProxyPassword,
    Cookies,
}

impl Field {
    fn tag(self) -> &'static str {
        match self {
            Field::AuthPassword => "auth_password",
            Field::ProxyPassword => "proxy_password",
            Field::Cookies => "cookies",
        }
    }
}

/// Outcome of the bootstrap probe — drives daemon startup mode.
#[derive(Debug)]
pub enum BootOutcome {
    /// Key was loaded (either pre-existing or freshly generated).
    Ready(MasterKey),
    /// Keyring has no key AND the DB already holds ciphertext. Caller
    /// must surface a dialog asking the user to acknowledge a wipe
    /// before the daemon can proceed.
    Locked,
}

impl MasterKey {
    /// Inspect the keyring and decide which mode to boot in.
    ///
    /// - Key present in keyring → `Ready`.
    /// - No key, DB has no encrypted rows → generate, store, `Ready`.
    /// - No key, DB has encrypted rows → `Locked` (UI must intervene).
    pub fn bootstrap(db_has_ciphertext: bool) -> Result<BootOutcome, CryptoError> {
        match crate::data::keyring::get_master_key().map_err(CryptoError::Keyring)? {
            Some(b64) => Self::from_base64(&b64).map(BootOutcome::Ready),
            None if db_has_ciphertext => Ok(BootOutcome::Locked),
            None => {
                let key = Self::generate()?;
                Ok(BootOutcome::Ready(key))
            }
        }
    }

    /// Generate a fresh 256-bit key, store it in the OS keyring, and
    /// return the in-memory cipher.
    pub fn generate() -> Result<Self, CryptoError> {
        let mut bytes = [0u8; KEY_LEN];
        rand::rngs::SysRng
            .try_fill_bytes(&mut bytes)
            .map_err(|e| CryptoError::KeyMaterial(e.to_string()))?;
        let b64 = STANDARD.encode(bytes);
        crate::data::keyring::set_master_key(&b64).map_err(CryptoError::Keyring)?;
        Self::from_bytes(&bytes)
    }

    fn from_base64(b64: &str) -> Result<Self, CryptoError> {
        let bytes = STANDARD
            .decode(b64.trim())
            .map_err(|e| CryptoError::KeyMaterial(e.to_string()))?;
        if bytes.len() != KEY_LEN {
            return Err(CryptoError::KeyMaterial(format!(
                "expected {KEY_LEN} bytes, got {}",
                bytes.len()
            )));
        }
        Self::from_bytes(&bytes)
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        let cipher = Aes256Gcm::new_from_slice(bytes)
            .map_err(|e| CryptoError::KeyMaterial(e.to_string()))?;
        Ok(Self(cipher))
    }

    /// Encrypt `plaintext` for storage in the named field of the named
    /// job. Returns the base64 blob to drop into the DB column.
    pub fn encrypt(&self, id: JobId, field: Field, plaintext: &str) -> Result<String, CryptoError> {
        let mut nonce_bytes = [0u8; NONCE_LEN];
        rand::rngs::SysRng
            .try_fill_bytes(&mut nonce_bytes)
            .map_err(|e| CryptoError::KeyMaterial(e.to_string()))?;
        let nonce = &Nonce::from(nonce_bytes);
        let aad = aad(id, field);
        let ct = self
            .0
            .encrypt(
                nonce,
                Payload {
                    msg: plaintext.as_bytes(),
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| CryptoError::Decrypt)?;
        let mut out = Vec::with_capacity(1 + NONCE_LEN + ct.len());
        out.push(VERSION);
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ct);
        Ok(STANDARD.encode(out))
    }

    /// Decrypt a DB blob produced by [`encrypt`] for the same
    /// `(job_id, field)` pair. Returns `Ok(None)` when the blob is
    /// empty so callers can treat NULL and `""` uniformly.
    pub fn decrypt(
        &self,
        id: JobId,
        field: Field,
        blob: &str,
    ) -> Result<Option<String>, CryptoError> {
        if blob.is_empty() {
            return Ok(None);
        }
        let raw = STANDARD
            .decode(blob)
            .map_err(|e| CryptoError::Format(e.to_string()))?;
        if raw.len() < 1 + NONCE_LEN + 16 {
            return Err(CryptoError::Format("blob too short".into()));
        }
        if raw[0] != VERSION {
            return Err(CryptoError::Format(format!(
                "unknown ciphertext version: {}",
                raw[0]
            )));
        }
        // Length is guaranteed by the check above, so the slice is
        // exactly a nonce; `from_slice` said the same thing and is
        // deprecated in aes-gcm 0.11.
        let nonce = &Nonce::try_from(&raw[1..1 + NONCE_LEN])
            .map_err(|_| CryptoError::Format("bad nonce".into()))?;
        let ct = &raw[1 + NONCE_LEN..];
        let aad = aad(id, field);
        let pt = self
            .0
            .decrypt(
                nonce,
                Payload {
                    msg: ct,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| CryptoError::Decrypt)?;
        let s = String::from_utf8(pt).map_err(|e| CryptoError::Format(e.to_string()))?;
        Ok(Some(s))
    }
}

fn aad(id: JobId, field: Field) -> String {
    format!("{id}|{}", field.tag())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> MasterKey {
        MasterKey::from_bytes(&[0x42u8; KEY_LEN]).unwrap()
    }

    /// A blob written by an older build must still open.
    ///
    /// Produced by aes-gcm 0.10 under the same key, id and field, and
    /// pinned here because the wire format is user data: every stored
    /// password and cookie jar in every existing install is one of
    /// these. An upgrade that changes how the nonce, tag or AAD are
    /// laid out would lock people out of their own secrets, and the
    /// round-trip test alone would not notice — it re-encrypts before
    /// it decrypts.
    #[test]
    fn a_blob_from_the_previous_library_still_decrypts() {
        const FIXTURE: &str = "AU43j7h4ke6aL4cUhm0priRtxgOy6daw8yJEGQeilSY71UduxiURvA==";
        let key = test_key();
        let id = JobId(uuid::Uuid::from_u128(
            0x0123_4567_89ab_cdef_0123_4567_89ab_cdef,
        ));
        let out = key.decrypt(id, Field::Cookies, FIXTURE).unwrap();
        assert_eq!(out.as_deref(), Some("session=abc"));
    }

    #[test]
    fn round_trip() {
        let key = test_key();
        let id = JobId::new();
        let blob = key.encrypt(id, Field::Cookies, "session=abc").unwrap();
        let out = key.decrypt(id, Field::Cookies, &blob).unwrap();
        assert_eq!(out.as_deref(), Some("session=abc"));
    }

    #[test]
    fn empty_blob_is_none() {
        let key = test_key();
        let id = JobId::new();
        assert!(key.decrypt(id, Field::Cookies, "").unwrap().is_none());
    }

    #[test]
    fn aad_swap_rejected() {
        let key = test_key();
        let id = JobId::new();
        let blob = key.encrypt(id, Field::AuthPassword, "secret").unwrap();
        assert!(key.decrypt(id, Field::Cookies, &blob).is_err());
    }

    #[test]
    fn id_swap_rejected() {
        let key = test_key();
        let blob = key
            .encrypt(JobId::new(), Field::AuthPassword, "secret")
            .unwrap();
        assert!(
            key.decrypt(JobId::new(), Field::AuthPassword, &blob)
                .is_err()
        );
    }
}

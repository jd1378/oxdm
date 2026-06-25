//! Per-job integrity checksums. The Properties dialog's Checksums tab
//! mutates these; the runner verifies them at completion when
//! `Advanced::auto_verify` is on.
//!
//! Lives in `domain` (and not under `ui::components::properties`) so
//! the IPC protocol + `Job` can reference it without pulling in any
//! UI types.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Algo {
    Md5,
    Sha1,
    Sha256,
    Sha384,
    Sha512,
}

impl Default for Algo {
    fn default() -> Self {
        Self::Sha256
    }
}

impl Algo {
    pub const ALL: &'static [Algo] = &[
        Algo::Md5,
        Algo::Sha1,
        Algo::Sha256,
        Algo::Sha384,
        Algo::Sha512,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Algo::Md5 => "MD5",
            Algo::Sha1 => "SHA-1",
            Algo::Sha256 => "SHA-256",
            Algo::Sha384 => "SHA-384",
            Algo::Sha512 => "SHA-512",
        }
    }

    /// Canonical hex character length.
    pub fn hex_len(self) -> usize {
        match self {
            Algo::Md5 => 32,
            Algo::Sha1 => 40,
            Algo::Sha256 => 64,
            Algo::Sha384 => 96,
            Algo::Sha512 => 128,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CsSource {
    Server,
    Computed,
    User,
}

impl Default for CsSource {
    fn default() -> Self {
        Self::User
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CsStatus {
    Verified,
    Mismatch,
    Unverified,
}

impl Default for CsStatus {
    fn default() -> Self {
        Self::Unverified
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Checksum {
    pub algo: Algo,
    pub hash: String,
    pub source: CsSource,
    pub status: CsStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
}

/// Compute the hex digest of a file with the given algorithm by streaming it
/// in chunks (constant memory regardless of file size). Blocking I/O — call
/// off any async UI executor.
///
/// Lives in `domain` so the GUI can verify a completed download's integrity
/// without importing the download engine's hasher (keeps layering intact).
pub fn compute_file(path: &std::path::Path, algo: Algo) -> std::io::Result<String> {
    use std::io::Read;

    /// 1 MiB read buffer — balances syscall count against memory.
    const CHUNK: usize = 1 << 20;

    fn stream<D: sha2::digest::Digest>(mut f: std::fs::File) -> std::io::Result<Vec<u8>> {
        let mut hasher = D::new();
        let mut buf = vec![0u8; CHUNK];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(hasher.finalize().to_vec())
    }

    let f = std::fs::File::open(path)?;
    let bytes = match algo {
        Algo::Md5 => stream::<md5::Md5>(f)?,
        Algo::Sha1 => stream::<sha1::Sha1>(f)?,
        Algo::Sha256 => stream::<sha2::Sha256>(f)?,
        Algo::Sha384 => stream::<sha2::Sha384>(f)?,
        Algo::Sha512 => stream::<sha2::Sha512>(f)?,
    };
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

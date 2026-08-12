//! Per-job integrity checksums. The Properties dialog's Checksums tab
//! mutates these; the runner hands every well-formed `Server`/`User`
//! row to odl and they are compared at completion.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The published digests of "abc", pinned per algorithm.
    ///
    /// A checksum library is only useful if its answer never changes:
    /// these values decide whether a finished download is reported as
    /// intact, and the self-update helper refuses to install an
    /// artifact whose digest does not match. An upgrade that altered
    /// any of them — or a `stream::<D>` wired to the wrong type — would
    /// otherwise surface as users being told their files are corrupt.
    #[test]
    fn every_algorithm_matches_its_published_vector() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("abc.txt");
        std::fs::write(&path, b"abc").unwrap();

        for (algo, expected) in [
            (Algo::Md5, "900150983cd24fb0d6963f7d28e17f72"),
            (Algo::Sha1, "a9993e364706816aba3e25717850c26c9cd0d89d"),
            (
                Algo::Sha256,
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
            (
                Algo::Sha384,
                "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed\
                 8086072ba1e7cc2358baeca134c825a7",
            ),
            (
                Algo::Sha512,
                "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
                 2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
            ),
        ] {
            assert_eq!(
                compute_file(&path, algo).unwrap(),
                expected.replace(char::is_whitespace, ""),
                "{algo:?}",
            );
        }
    }
}

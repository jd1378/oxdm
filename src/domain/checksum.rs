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

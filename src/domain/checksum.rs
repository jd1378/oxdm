//! Per-job integrity checksums. The Properties dialog's Checksums tab
//! mutates these; the runner hands every well-formed `Server`/`User`
//! row to odl and they are compared at completion.
//!
//! Lives in `domain` (and not under `ui::components::properties`) so
//! the IPC protocol + `Job` can reference it without pulling in any
//! UI types.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Algo {
    Md5,
    Sha1,
    #[default]
    Sha256,
    Sha384,
    Sha512,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CsSource {
    Server,
    Computed,
    #[default]
    User,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CsStatus {
    Verified,
    Mismatch,
    #[default]
    Unverified,
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

impl Checksum {
    /// Whether this row names the same digest: one algorithm, one value,
    /// whatever case it was typed in.
    pub fn same_digest(&self, algo: Algo, hash: &str) -> bool {
        self.algo == algo && self.hash.trim().eq_ignore_ascii_case(hash.trim())
    }
}

/// What a hash check found for one row, named by the digest it judged
/// rather than by where the row sat in the list. The list can change
/// while a file is hashed (the Properties window allows it), and a
/// verdict recorded by position would land on whatever row moved there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub algo: Algo,
    pub hash: String,
    pub status: CsStatus,
    /// The digest the file actually has, where it disagrees.
    pub computed: Option<String>,
}

/// Name each `(row index, status, computed)` result by the row of `rows`
/// it was produced from.
pub fn verdicts(
    rows: &[Checksum],
    results: impl IntoIterator<Item = (usize, CsStatus, Option<String>)>,
) -> Vec<Verdict> {
    results
        .into_iter()
        .filter_map(|(i, status, computed)| {
            rows.get(i).map(|r| Verdict {
                algo: r.algo,
                hash: r.hash.clone(),
                status,
                computed,
            })
        })
        .collect()
}

/// Record `verdicts` on the rows they are about. A row deleted since the
/// check was asked for gets nothing; one added since keeps its own state.
pub fn apply_verdicts(rows: &mut [Checksum], verdicts: &[Verdict]) {
    for v in verdicts {
        for row in rows.iter_mut().filter(|r| r.same_digest(v.algo, &v.hash)) {
            row.status = v.status;
            row.expected = v.computed.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(algo: Algo, hash: &str) -> Checksum {
        Checksum {
            algo,
            hash: hash.to_owned(),
            source: CsSource::User,
            status: CsStatus::Unverified,
            expected: None,
        }
    }

    /// A row added while the file was hashed shifts every position after
    /// it; the verdict still lands on the digest it was about.
    #[test]
    fn a_verdict_follows_its_digest_not_its_position() {
        let checked = vec![row(Algo::Md5, "aa"), row(Algo::Sha1, "bb")];
        let found = verdicts(
            &checked,
            [
                (0, CsStatus::Verified, None),
                (1, CsStatus::Mismatch, Some("cc".into())),
            ],
        );

        let mut now = vec![
            row(Algo::Sha256, "new"),
            checked[0].clone(),
            row(Algo::Sha1, "BB"),
        ];
        apply_verdicts(&mut now, &found);
        assert_eq!(
            now[0].status,
            CsStatus::Unverified,
            "added during the check"
        );
        assert_eq!(now[1].status, CsStatus::Verified);
        assert_eq!(
            now[2].status,
            CsStatus::Mismatch,
            "matched whatever the case"
        );
        assert_eq!(now[2].expected.as_deref(), Some("cc"));
    }

    #[test]
    fn a_verdict_for_a_deleted_row_is_dropped() {
        let checked = vec![row(Algo::Md5, "aa")];
        let found = verdicts(
            &checked,
            [(0, CsStatus::Verified, None), (5, CsStatus::Verified, None)],
        );
        assert_eq!(found.len(), 1, "an index past the list names nothing");
        let mut now = vec![row(Algo::Sha1, "bb")];
        apply_verdicts(&mut now, &found);
        assert_eq!(now[0].status, CsStatus::Unverified);
    }
}

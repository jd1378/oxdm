//! What the server offers. One entry per download behaviour worth
//! testing against; the handler reads the entry and misbehaves exactly
//! as it says.

pub const SIZE: u64 = 1024 * 1024 * 1024;

/// How the endpoint answers `Range`.
#[derive(Clone, Copy, PartialEq)]
pub enum Ranges {
    /// Advertises `Accept-Ranges` and honours the request. Resumable.
    Honour,
    /// No `Accept-Ranges`, `Range` ignored. Not resumable — a client
    /// that loses the connection has to start over.
    Absent,
    /// Advertises `Accept-Ranges`, then answers every `Range` with a
    /// plain 200 from byte zero. A resume gets the whole file again,
    /// and a client that trusts the advertisement without checking the
    /// status writes those bytes at the wrong offset.
    Fake,
}

/// Which checksums the response headers carry, and whether they are
/// the file's own.
#[derive(Clone, Copy, PartialEq)]
pub enum Checksums {
    None,
    /// Both digests are this file's.
    Valid,
    /// Both digests belong to a different file, so verification fails
    /// on every algorithm.
    Invalid,
    /// MD5 is this file's, SHA-1 is not — one row passes, one fails.
    Mixed,
}

pub struct Endpoint {
    pub path: &'static str,
    pub blurb: &'static str,
    /// Byte the body is filled with: `0x00` for "zeros", `0xFF` for
    /// "ones". Visible in a hex dump, so a mis-assembled file is easy
    /// to spot.
    pub fill: u8,
    pub ranges: Ranges,
    /// `false` omits `Content-Length` entirely and closes the
    /// connection at the end of the body, so the client cannot know
    /// the size — no percentage, no ETA, no split.
    pub length_known: bool,
    pub checksums: Checksums,
}

pub const ENDPOINTS: &[Endpoint] = &[
    Endpoint {
        path: "/zeros-1g.bin",
        blurb: "plain 1 GiB of zeros, resumable. The baseline.",
        fill: 0x00,
        ranges: Ranges::Honour,
        length_known: true,
        checksums: Checksums::None,
    },
    Endpoint {
        path: "/ones-1g-checksums-ok.bin",
        blurb: "resumable, two server-offered checksums, both correct.",
        fill: 0xFF,
        ranges: Ranges::Honour,
        length_known: true,
        checksums: Checksums::Valid,
    },
    Endpoint {
        path: "/ones-1g-checksums-bad.bin",
        blurb: "resumable, two server-offered checksums, both wrong \
                (they are the zeros file's digests) — integrity fails.",
        fill: 0xFF,
        ranges: Ranges::Honour,
        length_known: true,
        checksums: Checksums::Invalid,
    },
    Endpoint {
        path: "/ones-1g-checksums-mixed.bin",
        blurb: "resumable, two server-offered checksums, MD5 correct \
                and SHA-1 wrong — one row passes, one fails.",
        fill: 0xFF,
        ranges: Ranges::Honour,
        length_known: true,
        checksums: Checksums::Mixed,
    },
    Endpoint {
        path: "/zeros-1g-norange.bin",
        blurb: "no Accept-Ranges, Range ignored — not resumable, \
                single connection.",
        fill: 0x00,
        ranges: Ranges::Absent,
        length_known: true,
        checksums: Checksums::None,
    },
    Endpoint {
        path: "/zeros-1g-unknown-length.bin",
        blurb: "no Accept-Ranges and no Content-Length — the client \
                learns the size only when the connection closes.",
        fill: 0x00,
        ranges: Ranges::Absent,
        length_known: false,
        checksums: Checksums::None,
    },
    Endpoint {
        path: "/model-weights-1g.safetensors",
        blurb: "ordinary resumable download under a long extension — \
                the name the extension tile has to fit in its square.",
        fill: 0x00,
        ranges: Ranges::Honour,
        length_known: true,
        checksums: Checksums::None,
    },
    Endpoint {
        path: "/ubuntu-24.04.2-desktop-amd64-with-language-packs-restricted-extra-codecs-and-firmware-2026-08-20.iso",
        blurb: "ordinary resumable download under a 100-character file \
                name — nothing in a window that shows it may widen, \
                wrap, or push the progress out of sight.",
        fill: 0x00,
        ranges: Ranges::Honour,
        length_known: true,
        checksums: Checksums::None,
    },
    Endpoint {
        path: "/zeros-1g-fake-range.bin",
        blurb: "claims Accept-Ranges, then answers every Range with a \
                200 from byte zero — a resume silently restarts.",
        fill: 0x00,
        ranges: Ranges::Fake,
        length_known: true,
        checksums: Checksums::None,
    },
];

pub fn find(path: &str) -> Option<&'static Endpoint> {
    ENDPOINTS.iter().find(|e| e.path == path)
}

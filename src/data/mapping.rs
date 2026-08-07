//! Translation between oxdm `domain` types and `odl` types.
//!
//! Centralizing this means the rest of the codebase never imports `odl::*`
//! directly — and a future `odl` API change touches only this module.

use std::time::Duration;

use odl::config::{Config as OdlConfig, ConfigBuilder, DownloadOptions, DownloadOptionsBuilder};
use odl::error::{ConflictError, NetworkError, OdlError};
use odl::hash::{HashDigest, HashEncoding};
use odl::progress::Phase as OdlPhase;

use crate::domain::{
    Algo, AuthScheme, CapturedResponse, Checksum, CsSource, Job, JobError, Phase, ProxyAdv,
    ProxyMode, ResponseHeader, Settings,
};

pub fn settings_to_odl_config(
    s: &Settings,
    proxy_password: Option<&str>,
) -> Result<OdlConfig, String> {
    let download = settings_to_download_options(s, proxy_password)?;
    ConfigBuilder::default()
        // odl needs a config-level default; `Other` is ours. Per-job
        // instructions override it anyway.
        .download_dir(s.fallback_dir())
        .max_concurrent_downloads(s.max_concurrent_downloads)
        .download(download)
        .build()
        .map_err(|e| e.to_string())
}

pub fn settings_to_download_options(
    s: &Settings,
    proxy_password: Option<&str>,
) -> Result<DownloadOptions, String> {
    let mut b = DownloadOptionsBuilder::default();
    // odl still checks the assembled file's *size*, but hashing is ours
    // now: doing it after the download lets oxdm decide when to spend
    // the I/O, keep the result, and show it. odl verifying inline would
    // fail the job before oxdm ever saw the digest.
    b.verify_checksums(false);
    // `None` means "Determine automatically"; the per-job overlay set
    // by add_window (size-based suggest_segments) provides the real
    // value. Fall back to 8 here for jobs created without a per-job
    // override (e.g. captures).
    b.max_connections(s.max_connections.unwrap_or(8))
        .max_retries(s.max_retries)
        .wait_between_retries(s.wait_between_retries)
        .n_fixed_retries(s.n_fixed_retries)
        .user_agent(s.user_agent.clone())
        .randomize_user_agent(s.randomize_user_agent)
        // Assembled here and nowhere else: the settings hold the parts,
        // and the password comes straight from the secret store.
        .proxy(global_proxy_url(s, proxy_password)?)
        .use_server_time(s.use_server_time)
        .accept_invalid_certs(s.accept_invalid_certs)
        .speed_limit(s.speed_limit)
        .connect_timeout(s.connect_timeout);
    if !s.headers.is_empty() {
        b.headers(Some(s.headers.clone()));
    }
    b.build().map_err(|e| e.to_string())
}

/// Build a per-job overlay on top of the global download options. Job
/// fields that are `Some` win over the base; everything else inherits.
///
/// `proxy_password` and `auth_secret` are the secrets decrypted from
/// the job's encrypted columns at job-start time — merged into the
/// proxy URL / an `Authorization: Bearer` header just before handing
/// the options to odl, so neither is ever persisted in plaintext.
///
/// Site-auth precedence (guardian F2): HTTP **Basic** rides the legacy
/// `Job.auth_user` + decrypted password through `odl::Credentials` (see
/// `runner::build_credentials`) — those fields stay authoritative.
/// `advanced.auth` carries the scheme selection only; **Bearer** means
/// the decrypted secret is a token and travels as a header here.
pub fn job_overlay_options(
    base: &DownloadOptions,
    job: &Job,
    proxy_password: Option<&str>,
    cookies: Option<&str>,
    auth_secret: Option<&str>,
) -> Result<DownloadOptions, String> {
    let mut b = base.clone().into_builder();
    if let Some(n) = job.max_connections {
        b.max_connections(n);
    }
    apply_job_proxy(&mut b, job, proxy_password)?;
    let bearer = bearer_header(job, auth_secret);
    let cookies_present = cookies.is_some_and(|s| !s.is_empty());
    if !job.headers.is_empty() || cookies_present || bearer.is_some() {
        // Merge per-job headers on top of global; the decrypted
        // cookie jar (never stored in `Job.headers`) is injected here
        // so it lives only in the per-run overlay.
        // `upsert_header`, not `insert`: field names are
        // case-insensitive, so a job's `x-api-key` has to replace a
        // global `X-API-Key` here rather than ride alongside it.
        let mut merged = base.headers().cloned().unwrap_or_default();
        for (k, v) in job.headers.iter() {
            crate::domain::upsert_header(&mut merged, k, v.clone());
        }
        if let Some(c) = cookies.filter(|s| !s.is_empty()) {
            crate::domain::upsert_header(&mut merged, "Cookie", c.to_string());
        }
        if let Some(v) = bearer {
            crate::domain::upsert_header(&mut merged, "Authorization", v);
        }
        b.headers(Some(merged));
    }
    b.build().map_err(|e| e.to_string())
}

/// Translate the job's proxy configuration onto the options builder.
///
/// Mode semantics (honesty matrix, feature #6):
/// - `Inherit`: no per-job override — except the legacy explicit
///   `Job.proxy` string, which wins under Inherit for backward compat.
/// - `System`: clear any configured proxy for this job
///   (`builder.proxy(None)` overrides a global `Some`); reqwest then
///   falls back to its standard environment-variable pickup.
/// - `Http` / `Https` / `Socks5`: synthesize
///   `scheme://[user[:pw]@]host:port` (socks5h when remote-DNS is on).
/// - `None` (legacy persisted value): coerced to `Inherit` with a WARN
///   — odl cannot disable reqwest's env-proxy pickup, so "force
///   direct" is inexpressible and must never be silently faked (F6).
fn apply_job_proxy(
    b: &mut DownloadOptionsBuilder,
    job: &Job,
    proxy_password: Option<&str>,
) -> Result<(), String> {
    let adv = &job.advanced.proxy;
    let mode = match adv.mode {
        ProxyMode::None => {
            tracing::warn!(
                job = %job.id,
                "legacy ProxyMode::None cannot be honoured (odl/reqwest env-proxy \
                 pickup cannot be disabled); treating as Inherit"
            );
            ProxyMode::Inherit
        }
        m => m,
    };
    match mode {
        ProxyMode::None => unreachable!("coerced above"),
        ProxyMode::Inherit => {
            if let Some(p) = job.proxy.clone() {
                let merged = merge_proxy_password(&p, proxy_password)?;
                b.proxy(Some(merged));
            }
        }
        ProxyMode::System => {
            b.proxy(None);
        }
        ProxyMode::Http | ProxyMode::Https | ProxyMode::Socks5 => {
            b.proxy(Some(synth_proxy_url(mode, adv, proxy_password)?));
        }
    }
    Ok(())
}

/// `scheme://[user[:pw]@]host:port` from the advanced proxy bundle.
/// The `url` crate percent-encodes credentials for us.
/// The global proxy as a URL, or `None` for System / an unconfigured
/// one — reqwest then falls back to the proxy environment variables.
fn global_proxy_url(s: &Settings, password: Option<&str>) -> Result<Option<String>, String> {
    if !matches!(
        s.proxy.mode,
        ProxyMode::Http | ProxyMode::Https | ProxyMode::Socks5
    ) || s.proxy.host.trim().is_empty()
    {
        return Ok(None);
    }
    synth_proxy_url(s.proxy.mode, &s.proxy, password).map(Some)
}

fn synth_proxy_url(
    mode: ProxyMode,
    adv: &ProxyAdv,
    password: Option<&str>,
) -> Result<String, String> {
    let host = adv.host.trim();
    if host.is_empty() {
        return Err("proxy host is empty".into());
    }
    let port: u16 = adv
        .port
        .trim()
        .parse()
        .ok()
        .filter(|p| *p >= 1)
        .ok_or_else(|| format!("invalid proxy port `{}` (expected 1–65535)", adv.port))?;
    let scheme = match mode {
        ProxyMode::Http => "http",
        ProxyMode::Https => "https",
        ProxyMode::Socks5 if adv.remote_dns => "socks5h",
        ProxyMode::Socks5 => "socks5",
        _ => unreachable!("callers only pass explicit proxy modes"),
    };
    let mut u = url::Url::parse(&format!("{scheme}://{host}:{port}"))
        .map_err(|e| format!("invalid proxy host/port: {e}"))?;
    if adv.auth_enabled {
        if !adv.username.is_empty() {
            u.set_username(&adv.username)
                .map_err(|_| "cannot attach username to proxy URL".to_string())?;
        }
        if let Some(pw) = password.filter(|s| !s.is_empty()) {
            u.set_password(Some(pw))
                .map_err(|_| "cannot attach password to proxy URL".to_string())?;
        }
    }
    Ok(u.into())
}

/// `Authorization` header value for Bearer-scheme jobs. Basic returns
/// `None` here — it rides the legacy `odl::Credentials` path (F2).
/// Legacy persisted `Digest` is coerced to no auth with a WARN (F6):
/// neither odl nor reqwest implements Digest, and silently degrading
/// it to something else would be dishonest.
fn bearer_header(job: &Job, auth_secret: Option<&str>) -> Option<String> {
    match job.advanced.auth.scheme {
        AuthScheme::Bearer => auth_secret
            .filter(|s| !s.is_empty())
            .map(|t| format!("Bearer {t}")),
        AuthScheme::Digest => {
            tracing::warn!(
                job = %job.id,
                "legacy AuthScheme::Digest is unsupported (no odl/reqwest Digest \
                 implementation); treating as no auth"
            );
            None
        }
        AuthScheme::None | AuthScheme::Basic => None,
    }
}

/// Expected-integrity digests for a job, ready for
/// `Instruction::add_checksums`.
///
/// Empty unless the job's persisted `advanced.auto_verify` toggle is on
/// (guardian F3 — never verify behind the user's back). Only `Server` /
/// `User`-sourced entries qualify: a `Computed` record describes a
/// previous run's bytes and would permanently fail a legitimate
/// re-download of changed content. Entries with a wrong-length or
/// non-hex digest are skipped; hex is lowercased to match odl's
/// lowercase verification output.
pub fn job_expected_digests(job: &Job) -> Vec<HashDigest> {
    if !job.advanced.auto_verify {
        return Vec::new();
    }
    checksum_digests(job)
}

/// Every digest on the job that is well-formed enough to check against,
/// regardless of the `auto_verify` preference — that one answers
/// "check without being asked", not "may be checked".
pub fn checksum_digests(job: &Job) -> Vec<HashDigest> {
    job.checksums
        .iter()
        .filter(|c| matches!(c.source, CsSource::Server | CsSource::User))
        .filter_map(checksum_to_digest)
        .collect()
}

/// oxdm's algorithm enum in odl's terms.
pub fn odl_algorithm(a: Algo) -> odl::hash::HashAlgorithm {
    match a {
        Algo::Md5 => odl::hash::HashAlgorithm::MD5,
        Algo::Sha1 => odl::hash::HashAlgorithm::SHA1,
        Algo::Sha256 => odl::hash::HashAlgorithm::SHA256,
        Algo::Sha384 => odl::hash::HashAlgorithm::SHA384,
        Algo::Sha512 => odl::hash::HashAlgorithm::SHA512,
    }
}

fn checksum_to_digest(c: &Checksum) -> Option<HashDigest> {
    let h = c.hash.trim();
    if h.len() != c.algo.hex_len() || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let hex = h.to_ascii_lowercase();
    Some(match c.algo {
        Algo::Md5 => HashDigest::MD5(hex, HashEncoding::Hex),
        Algo::Sha1 => HashDigest::SHA1(hex, HashEncoding::Hex),
        Algo::Sha256 => HashDigest::SHA256(hex, HashEncoding::Hex),
        Algo::Sha384 => HashDigest::SHA384(hex, HashEncoding::Hex),
        Algo::Sha512 => HashDigest::SHA512(hex, HashEncoding::Hex),
    })
}

/// Response headers the server sent on the `evaluate` probe, ready for
/// `Job::captured_response`.
///
/// `None` when odl made no probe (`quick_evaluate`) — distinct from a
/// probe whose headers were all filtered away, which is `Some` with an
/// empty list.
///
/// The probe/no-probe signal is `response_headers_probed_at`, NOT the
/// header list: odl persists the timestamp independently, so an
/// instruction rebuilt from `metadata.pb` whose every header was
/// dropped comes back with `response_headers() == None` but the
/// timestamp intact. Branching on the list would report that probe as
/// "never happened".
///
/// Filtering (credential-bearing names dropped, oversized values and an
/// oversized total capped) is odl's: `stored_response_headers` applies
/// exactly what it writes to `metadata.pb`, so a job displays the same
/// headers before and after a restart. `Download::response_headers`
/// would hand back the *raw* map — never use it for anything we store
/// or show.
pub fn captured_response(instr: &odl::Download) -> Option<CapturedResponse> {
    let probed_at = instr.response_headers_probed_at()?;
    Some(CapturedResponse {
        headers: instr
            .stored_response_headers()
            .into_iter()
            .map(|h| ResponseHeader {
                name: h.name,
                value: h.value,
            })
            .collect(),
        probed_at,
    })
}

pub fn phase_from_odl(p: OdlPhase) -> Phase {
    match p {
        OdlPhase::Evaluating => Phase::Evaluating,
        OdlPhase::ResolvingConflicts => Phase::ResolvingConflicts,
        OdlPhase::Downloading => Phase::Downloading,
        OdlPhase::Assembling => Phase::Assembling,
        OdlPhase::Flushing => Phase::Flushing,
        OdlPhase::Verifying => Phase::Verifying,
        // odl 2.0 added `PostProcessing` for work an external tool does
        // on the bytes before they are usable (muxing, mostly). oxdm
        // forces the HTTP engine, which never emits it, and the phase
        // is open-ended anyway — anything new reads as "still working".
        _ => Phase::Assembling,
    }
}

pub fn job_error_from_odl(e: &OdlError) -> JobError {
    match e {
        OdlError::Network(n) => match n {
            NetworkError::Connect => JobError::Network("connection failed".into()),
            NetworkError::Dns { host, message } => JobError::Dns {
                host: host.clone(),
                message: message.clone(),
            },
            NetworkError::Timeout => JobError::Network("timeout".into()),
            NetworkError::ResponseBody => JobError::Network("response body error".into()),
            NetworkError::Status {
                status_code,
                reason,
                url,
            } => JobError::HttpStatus {
                code: *status_code,
                reason: reason.clone(),
                url: url.as_ref().map(|u| u.to_string()),
            },
            NetworkError::Other { message } => JobError::Network(message.clone()),
        },
        OdlError::Conflict(c) => match c {
            ConflictError::Save { conflict } => JobError::SaveConflict(conflict.to_string()),
            // Two of odl's server conflicts have their own recovery in
            // the UI (restart from zero vs. retry), so they keep their
            // identity instead of collapsing into one string.
            ConflictError::Server { conflict } => match conflict {
                odl::conflict::ServerConflict::FileChanged => {
                    JobError::FileChanged(conflict.to_string())
                }
                odl::conflict::ServerConflict::NotResumable => {
                    JobError::NotResumable(conflict.to_string())
                }
                _ => JobError::ServerConflict(conflict.to_string()),
            },
            ConflictError::ChecksumMismatch { expected, actual } => JobError::ChecksumMismatch {
                expected: expected.clone(),
                actual: actual.clone(),
            },
        },
        OdlError::Cancelled => JobError::Cancelled,
        OdlError::StdIoError { e, extra_info } => {
            let msg = match extra_info {
                Some(extra) => format!("{e} ({extra})"),
                None => e.to_string(),
            };
            // A full disk and a rejected folder are the two write
            // faults the user can actually fix, so they are named.
            // `ErrorKind` already folds the per-OS codes (ENOSPC,
            // ERROR_DISK_FULL, …) into these two.
            match e.kind() {
                std::io::ErrorKind::StorageFull => JobError::DiskFull(msg),
                std::io::ErrorKind::PermissionDenied => JobError::PermissionDenied(msg),
                _ => JobError::Io(msg),
            }
        }
        other => JobError::Other(other.to_string()),
    }
}

#[allow(dead_code)]
pub fn duration_zero() -> Duration {
    Duration::from_secs(0)
}

fn merge_proxy_password(proxy_url: &str, password: Option<&str>) -> Result<String, String> {
    let Some(pw) = password else {
        return Ok(proxy_url.to_string());
    };
    if pw.is_empty() {
        return Ok(proxy_url.to_string());
    }
    let mut u = url::Url::parse(proxy_url).map_err(|e| format!("invalid proxy URL: {e}"))?;
    u.set_password(Some(pw))
        .map_err(|_| "cannot attach password to proxy URL".to_string())?;
    Ok(u.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{CsStatus, JobId, JobStatus};

    fn sample_job() -> Job {
        Job {
            id: JobId::new(),
            url: url::Url::parse("https://example.com/file.zip").unwrap(),
            save_dir: std::path::PathBuf::from("/tmp/oxdm-test"),
            filename: Some("file.zip".into()),
            referrer: None,
            headers: indexmap::IndexMap::new(),
            max_connections: None,
            proxy: None,
            auth_user: None,
            enc_auth_password: None,
            enc_proxy_password: None,
            enc_cookies: None,
            speed_limit_override: None,
            queue_id: crate::domain::QueueId::new(),
            created_at: chrono::Utc::now(),
            started_at: None,
            finished_at: None,
            retries: 0,
            interruptions: 0,
            status: JobStatus::default(),
            advanced: crate::domain::Advanced::default(),
            checksums: Vec::new(),
            category: crate::domain::Category::Other,
            captured_response: None,
        }
    }

    fn checksum(algo: Algo, hash: &str, source: CsSource) -> Checksum {
        Checksum {
            algo,
            hash: hash.into(),
            source,
            status: CsStatus::Unverified,
            expected: None,
        }
    }

    const SHA256_UPPER: &str = "B94D27B9934D3E08A52E52D7DA7DABFAC484EFE37A5380EE9088F7ACE2EFCDE9";

    #[test]
    fn digests_gated_on_auto_verify() {
        let mut job = sample_job();
        job.checksums = vec![checksum(Algo::Sha256, SHA256_UPPER, CsSource::User)];
        job.advanced.auto_verify = false;
        assert!(job_expected_digests(&job).is_empty());
        job.advanced.auto_verify = true;
        assert_eq!(job_expected_digests(&job).len(), 1);
    }

    #[test]
    fn digests_exclude_computed_source() {
        let mut job = sample_job();
        job.advanced.auto_verify = true;
        job.checksums = vec![
            checksum(Algo::Sha256, SHA256_UPPER, CsSource::Computed),
            checksum(
                Algo::Md5,
                "5eb63bbbe01eeed093cb22bb8f5acdc3",
                CsSource::Server,
            ),
        ];
        let digests = job_expected_digests(&job);
        assert_eq!(digests.len(), 1);
        assert!(matches!(&digests[0], HashDigest::MD5(h, HashEncoding::Hex)
            if h == "5eb63bbbe01eeed093cb22bb8f5acdc3"));
    }

    #[test]
    fn digests_skip_invalid_hex_and_lowercase_valid() {
        let mut job = sample_job();
        job.advanced.auto_verify = true;
        job.checksums = vec![
            // Wrong length for SHA-256.
            checksum(Algo::Sha256, "abcd", CsSource::User),
            // Non-hex characters.
            checksum(Algo::Md5, &"z".repeat(32), CsSource::User),
            // Valid uppercase — must be lowercased.
            checksum(Algo::Sha256, SHA256_UPPER, CsSource::User),
        ];
        let digests = job_expected_digests(&job);
        assert_eq!(digests.len(), 1);
        assert!(
            matches!(&digests[0], HashDigest::SHA256(h, HashEncoding::Hex)
            if h == &SHA256_UPPER.to_ascii_lowercase())
        );
    }

    #[test]
    fn global_proxy_assembles_from_parts_with_the_stored_secret() {
        let mut s = Settings::default();
        // System (the default) is "no explicit proxy" — environment wins.
        assert_eq!(global_proxy_url(&s, None).unwrap(), None);

        s.proxy = ProxyAdv {
            mode: ProxyMode::Http,
            host: "proxy.lan".into(),
            port: "3128".into(),
            auth_enabled: true,
            username: "user".into(),
            ..ProxyAdv::default()
        };
        // The password comes from the secret store, never from the parts.
        assert_eq!(
            global_proxy_url(&s, Some("s3cret")).unwrap().unwrap(),
            "http://user:s3cret@proxy.lan:3128/"
        );
        assert_eq!(
            global_proxy_url(&s, None).unwrap().unwrap(),
            "http://user@proxy.lan:3128/"
        );

        // A mode chosen but no host yet is not a proxy.
        s.proxy.host = String::new();
        assert_eq!(global_proxy_url(&s, Some("s3cret")).unwrap(), None);
    }

    #[test]
    fn synth_proxy_url_shapes() {
        let mut adv = ProxyAdv {
            mode: ProxyMode::Socks5,
            host: "proxy.lan".into(),
            port: "1080".into(),
            ..ProxyAdv::default()
        };
        // remote_dns defaults to true → socks5h.
        assert_eq!(
            synth_proxy_url(ProxyMode::Socks5, &adv, None).unwrap(),
            "socks5h://proxy.lan:1080"
        );
        adv.remote_dns = false;
        assert_eq!(
            synth_proxy_url(ProxyMode::Socks5, &adv, None).unwrap(),
            "socks5://proxy.lan:1080"
        );
        adv.auth_enabled = true;
        adv.username = "us er".into();
        // `Url` normalizes special schemes with a trailing "/" path —
        // harmless for reqwest's proxy parser.
        assert_eq!(
            synth_proxy_url(ProxyMode::Http, &adv, Some("p@ss")).unwrap(),
            "http://us%20er:p%40ss@proxy.lan:1080/"
        );
        // Port validation: 0 / junk / out-of-range all rejected.
        for bad in ["0", "junk", "65536", ""] {
            adv.port = bad.into();
            assert!(synth_proxy_url(ProxyMode::Http, &adv, None).is_err());
        }
        adv.port = "8080".into();
        adv.host = "  ".into();
        assert!(synth_proxy_url(ProxyMode::Http, &adv, None).is_err());
    }

    #[test]
    fn overlay_system_mode_clears_global_proxy() {
        // Gate check for the GUI "System" proxy mode: a base with a
        // global proxy must come out with proxy == None (reqwest env
        // pickup) when the job selects System.
        let base = DownloadOptionsBuilder::default()
            .proxy(Some("http://global:3128".to_owned()))
            .build()
            .unwrap();
        let mut job = sample_job();
        job.advanced.proxy.mode = ProxyMode::System;
        let opts = job_overlay_options(&base, &job, None, None, None).unwrap();
        assert_eq!(opts.proxy(), None);
        // Inherit keeps the global.
        job.advanced.proxy.mode = ProxyMode::Inherit;
        let opts = job_overlay_options(&base, &job, None, None, None).unwrap();
        assert_eq!(opts.proxy(), Some("http://global:3128"));
        // Legacy per-job proxy string wins under Inherit.
        job.proxy = Some("http://legacy:9999".to_owned());
        let opts = job_overlay_options(&base, &job, None, None, None).unwrap();
        assert_eq!(opts.proxy(), Some("http://legacy:9999"));
    }

    #[test]
    fn overlay_bearer_sets_authorization_header() {
        let base = DownloadOptionsBuilder::default().build().unwrap();
        let mut job = sample_job();
        job.advanced.auth.scheme = AuthScheme::Bearer;
        let opts = job_overlay_options(&base, &job, None, None, Some("tok123")).unwrap();
        assert_eq!(
            opts.headers().and_then(|h| h.get("Authorization")),
            Some(&"Bearer tok123".to_string())
        );
        // No secret → no header at all.
        let opts = job_overlay_options(&base, &job, None, None, None).unwrap();
        assert!(opts.headers().is_none());
        // Basic never rides the header path (legacy Credentials own it).
        job.advanced.auth.scheme = AuthScheme::Basic;
        let opts = job_overlay_options(&base, &job, None, None, Some("pw")).unwrap();
        assert!(opts.headers().is_none());
    }

    #[test]
    fn overlay_job_header_replaces_a_differently_cased_global() {
        // Field names are case-insensitive: the job's spelling must
        // override the global entry, not append a second one that
        // reqwest would fold away unpredictably.
        let mut global = indexmap::IndexMap::new();
        global.insert("X-API-Key".to_owned(), "global".to_owned());
        let base = DownloadOptionsBuilder::default()
            .headers(Some(global))
            .build()
            .unwrap();
        let mut job = sample_job();
        job.headers
            .insert("x-api-key".to_owned(), "per-job".to_owned());

        let opts = job_overlay_options(&base, &job, None, None, None).unwrap();
        let headers = opts.headers().expect("headers");
        assert_eq!(headers.len(), 1, "one header, not two spellings of one");
        assert_eq!(headers["X-API-Key"], "per-job");
    }

    #[test]
    fn overlay_stored_cookie_replaces_a_differently_cased_custom_one() {
        let mut global = indexmap::IndexMap::new();
        global.insert("cookie".to_owned(), "stale=1".to_owned());
        let base = DownloadOptionsBuilder::default()
            .headers(Some(global))
            .build()
            .unwrap();
        let job = sample_job();

        let opts = job_overlay_options(&base, &job, None, Some("fresh=2"), None).unwrap();
        let headers = opts.headers().expect("headers");
        assert_eq!(headers.len(), 1);
        assert_eq!(headers["cookie"], "fresh=2");
    }

    /// Rebuild an instruction the way odl does on resume, so
    /// `captured_response` runs against a real `Download`.
    fn instruction_with_headers(headers: &[(&str, &str)], probed_at: Option<i64>) -> odl::Download {
        let metadata = odl::download_metadata::DownloadMetadata {
            url: "https://example.com/file.zip".into(),
            filename: "file.zip".into(),
            save_dir: "/tmp/oxdm-test".into(),
            max_connections: 1,
            response_headers: headers
                .iter()
                .map(|(n, v)| odl::download_metadata::ResponseHeader {
                    name: (*n).to_owned(),
                    value: (*v).to_owned(),
                })
                .collect(),
            response_headers_probed_at: probed_at,
            ..Default::default()
        };
        odl::Download::from_metadata(std::path::PathBuf::from("/tmp/oxdm-test"), metadata)
            .expect("valid metadata")
    }

    #[test]
    fn captured_response_maps_stored_headers_in_order() {
        // Repeated names survive (a response may carry several `Vary`
        // lines), and server order is preserved.
        let instr = instruction_with_headers(
            &[
                ("content-type", "application/zip"),
                ("vary", "accept-encoding"),
                ("vary", "origin"),
            ],
            Some(1_700_000_000),
        );
        let captured = captured_response(&instr).expect("probe headers present");
        assert_eq!(
            captured
                .headers
                .iter()
                .map(|h| (h.name.as_str(), h.value.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("content-type", "application/zip"),
                ("vary", "accept-encoding"),
                ("vary", "origin"),
            ]
        );
        assert_eq!(captured.probed_at, 1_700_000_000);
    }

    #[test]
    fn captured_response_goes_through_odls_secret_filter() {
        // Guards the contract we depend on: we call
        // `stored_response_headers`, never the raw map, so nothing
        // credential-bearing can reach the store or the UI.
        let instr = instruction_with_headers(
            &[
                ("content-type", "application/zip"),
                ("set-cookie", "sid=deadbeef; HttpOnly"),
                ("www-authenticate", "Basic realm=\"x\""),
                ("x-amz-security-token", "AQoDY..."),
            ],
            Some(1),
        );
        let captured = captured_response(&instr).expect("probe headers present");
        assert_eq!(
            captured
                .headers
                .iter()
                .map(|h| h.name.as_str())
                .collect::<Vec<_>>(),
            vec!["content-type"]
        );
    }

    #[test]
    fn captured_response_is_none_without_a_probe() {
        // No probe (`quick_evaluate`, or a pre-1.2 metadata file) must
        // stay distinguishable from a probe that yielded no displayable
        // headers.
        let instr = instruction_with_headers(&[], None);
        assert!(captured_response(&instr).is_none());
    }

    #[test]
    fn captured_response_keeps_a_probe_whose_headers_were_all_filtered() {
        // odl persists the timestamp independently of the list, so a
        // metadata file whose every header was dropped rebuilds with
        // `response_headers() == None` but the timestamp intact. That
        // probe DID happen — branching on the list would erase it.
        let instr = instruction_with_headers(&[], Some(1_700_000_000));
        assert!(instr.response_headers().is_none());
        let captured = captured_response(&instr).expect("the probe still counts");
        assert!(captured.headers.is_empty());
        assert_eq!(captured.probed_at, 1_700_000_000);
    }
}

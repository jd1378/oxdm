//! Translation between oxdm `domain` types and `odl` types.
//!
//! Centralizing this means the rest of the codebase never imports `odl::*`
//! directly — and a future `odl` API change touches only this module.

use std::time::Duration;

use odl::config::{Config as OdlConfig, ConfigBuilder, DownloadOptions, DownloadOptionsBuilder};
use odl::error::{ConflictError, NetworkError, OdlError};
use odl::progress::Phase as OdlPhase;

use crate::domain::{Job, JobError, Phase, Settings};

pub fn settings_to_odl_config(s: &Settings) -> Result<OdlConfig, String> {
    let download = settings_to_download_options(s)?;
    ConfigBuilder::default()
        .download_dir(s.download_dir.clone())
        .max_concurrent_downloads(s.max_concurrent_downloads)
        .download(download)
        .build()
        .map_err(|e| e.to_string())
}

pub fn settings_to_download_options(s: &Settings) -> Result<DownloadOptions, String> {
    let mut b = DownloadOptionsBuilder::default();
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
        .proxy(s.proxy.clone())
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
/// `proxy_password` is the secret loaded from the keyring at job-start
/// time — it gets merged into `job.proxy`'s URL just before handing the
/// options to odl, so the password never has to be persisted on disk.
pub fn job_overlay_options(
    base: &DownloadOptions,
    job: &Job,
    proxy_password: Option<&str>,
    cookies: Option<&str>,
) -> Result<DownloadOptions, String> {
    let mut b = base.clone().into_builder();
    if let Some(n) = job.max_connections {
        b.max_connections(n);
    }
    if let Some(p) = job.proxy.clone() {
        let merged = merge_proxy_password(&p, proxy_password)?;
        b.proxy(Some(merged));
    }
    let cookies_present = cookies.is_some_and(|s| !s.is_empty());
    if !job.headers.is_empty() || cookies_present {
        // Merge per-job headers on top of global; the decrypted
        // cookie jar (never stored in `Job.headers`) is injected here
        // so it lives only in the per-run overlay.
        let mut merged = base.headers().cloned().unwrap_or_default();
        for (k, v) in job.headers.iter() {
            merged.insert(k.clone(), v.clone());
        }
        if let Some(c) = cookies.filter(|s| !s.is_empty()) {
            merged.insert("Cookie".into(), c.to_string());
        }
        b.headers(Some(merged));
    }
    b.build().map_err(|e| e.to_string())
}

pub fn phase_from_odl(p: OdlPhase) -> Phase {
    match p {
        OdlPhase::Evaluating => Phase::Evaluating,
        OdlPhase::ResolvingConflicts => Phase::ResolvingConflicts,
        OdlPhase::Downloading => Phase::Downloading,
        OdlPhase::Assembling => Phase::Assembling,
        OdlPhase::Flushing => Phase::Flushing,
        OdlPhase::Verifying => Phase::Verifying,
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
            } => {
                let mut msg = format!("status {status_code}");
                if let Some(r) = reason {
                    msg.push_str(&format!(" {r}"));
                }
                if let Some(u) = url {
                    msg.push_str(&format!(" (url: {u})"));
                }
                JobError::Network(msg)
            }
            NetworkError::Other { message } => JobError::Network(message.clone()),
        },
        OdlError::Conflict(c) => match c {
            ConflictError::Save { conflict } => JobError::SaveConflict(conflict.to_string()),
            ConflictError::Server { conflict } => JobError::ServerConflict(conflict.to_string()),
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
            JobError::Io(msg)
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

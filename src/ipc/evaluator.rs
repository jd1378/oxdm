//! HEAD-probe one URL on behalf of an extension's mass-select dialog.
//!
//! Reuses the same headers (cookies, UA, referrer) the extension would
//! send for the real download, so anti-leech servers see consistent
//! traffic between probe + capture.

use indexmap::IndexMap;
use std::time::Duration;

use crate::domain::capture::CaptureResponse;

pub async fn evaluate(
    id: String,
    url: url::Url,
    referrer: Option<url::Url>,
    cookies: Option<String>,
    user_agent: Option<String>,
    headers: IndexMap<String, String>,
) -> CaptureResponse {
    // The probe runs in the daemon process with the user's full
    // network identity — refuse to point it at non-public targets so
    // a hostile extension can't turn the bridge into an internal-net
    // scanner.
    if let Err(reason) = crate::ipc::guard_public_http_url(&url) {
        return err(id, url, reason);
    }
    let mut hdr = reqwest::header::HeaderMap::new();
    let ua = user_agent.as_deref().unwrap_or("oxdm/0");
    if let Ok(v) = reqwest::header::HeaderValue::from_str(ua) {
        hdr.insert(reqwest::header::USER_AGENT, v);
    }
    if let Some(r) = referrer.as_ref()
        && let Ok(v) = reqwest::header::HeaderValue::from_str(r.as_str())
    {
        hdr.insert(reqwest::header::REFERER, v);
    }
    if let Some(c) = cookies.as_deref()
        && let Ok(v) = reqwest::header::HeaderValue::from_str(c)
    {
        hdr.insert(reqwest::header::COOKIE, v);
    }
    for (k, v) in headers {
        if let (Ok(name), Ok(val)) = (
            reqwest::header::HeaderName::from_bytes(k.as_bytes()),
            reqwest::header::HeaderValue::from_str(&v),
        ) {
            hdr.insert(name, val);
        }
    }

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .default_headers(hdr)
        .build()
    {
        Ok(c) => c,
        Err(e) => return err(id, url, e.to_string()),
    };

    // Try HEAD first; fall back to ranged GET for hosts that 405 HEAD.
    let url_str = url.to_string();
    let head = client.head(url.clone()).send().await;
    let resp = match head {
        Ok(r) if r.status().is_success() || r.status().is_redirection() => r,
        _ => match client
            .get(url.clone())
            .header(reqwest::header::RANGE, "bytes=0-0")
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return err(id, url, e.to_string()),
        },
    };

    let headers = resp.headers().clone();
    let size = headers
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());
    let size = match (size, headers.get(reqwest::header::CONTENT_RANGE)) {
        (None, Some(cr)) => cr
            .to_str()
            .ok()
            .and_then(|s| s.split('/').nth(1))
            .and_then(|s| s.trim().parse::<u64>().ok()),
        (s, _) => s,
    };
    let mime_type = headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(';').next().unwrap_or(s).trim().to_owned());
    let etag = headers
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_owned());
    let supports_resume = headers
        .get(reqwest::header::ACCEPT_RANGES)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.eq_ignore_ascii_case("bytes"));
    let filename = headers
        .get(reqwest::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .and_then(filename_from_disposition)
        .or_else(|| filename_from_url(&url));

    CaptureResponse::Evaluated {
        id,
        url: url_str,
        filename,
        size,
        mime_type,
        etag,
        supports_resume,
        error: None,
    }
}

fn err(id: String, url: url::Url, message: String) -> CaptureResponse {
    CaptureResponse::Evaluated {
        id,
        url: url.to_string(),
        filename: None,
        size: None,
        mime_type: None,
        etag: None,
        supports_resume: None,
        error: Some(message),
    }
}

fn filename_from_disposition(value: &str) -> Option<String> {
    // Naive parse: prefer RFC 5987 `filename*=UTF-8''...`, else `filename="..."`.
    for part in value.split(';') {
        let p = part.trim();
        if let Some(rest) = p.strip_prefix("filename*=") {
            let rest = rest.trim_matches('"');
            if let Some(idx) = rest.find("''") {
                let enc = &rest[idx + 2..];
                if let Ok(decoded) = urlencoding_decode(enc) {
                    return Some(decoded);
                }
            }
        } else if let Some(rest) = p.strip_prefix("filename=") {
            return Some(rest.trim_matches('"').to_owned());
        }
    }
    None
}

fn urlencoding_decode(s: &str) -> Result<String, std::str::Utf8Error> {
    let mut out = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push(((h << 4) | l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b);
        i += 1;
    }
    std::str::from_utf8(&out).map(|s| s.to_owned())
}

fn filename_from_url(url: &url::Url) -> Option<String> {
    let last = url.path_segments()?.next_back()?;
    if last.is_empty() {
        return None;
    }
    Some(last.to_owned())
}

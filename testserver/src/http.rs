//! Just enough HTTP/1.1 to answer a download client, hand-rolled on
//! purpose: half the point of this server is to answer *wrongly* —
//! omit `Content-Length`, ignore a `Range` it advertised — and a real
//! server library exists to stop you doing that.
//!
//! Every response closes the connection. Keep-alive buys nothing here
//! and a download client opens its own connections anyway.

use std::io;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const MAX_HEAD: usize = 16 * 1024;

pub struct Request {
    pub method: String,
    target: String,
    headers: Vec<(String, String)>,
}

impl Request {
    /// The request target verbatim, query string and all.
    pub fn target(&self) -> &str {
        &self.target
    }

    /// Path with the query string stripped.
    pub fn path(&self) -> &str {
        match self.target.split_once('?') {
            Some((p, _)) => p,
            None => &self.target,
        }
    }

    pub fn query(&self, key: &str) -> Option<&str> {
        let q = self.target.split_once('?')?.1;
        q.split('&').find_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            (k == key).then_some(v)
        })
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// Reads one request head. `Ok(None)` means the peer hung up or sent
/// something unparseable — either way the caller just drops it.
pub async fn read_request(stream: &mut TcpStream) -> io::Result<Option<Request>> {
    let mut buf = Vec::with_capacity(1024);
    loop {
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > MAX_HEAD {
            return Ok(None);
        }
        let mut chunk = [0u8; 1024];
        match stream.read(&mut chunk).await? {
            0 => return Ok(None),
            n => buf.extend_from_slice(&chunk[..n]),
        }
    }

    let text = String::from_utf8_lossy(&buf).into_owned();
    let mut lines = text.split("\r\n");
    let mut start = lines.next().unwrap_or_default().split(' ');
    let (Some(method), Some(target)) = (start.next(), start.next()) else {
        return Ok(None);
    };

    let headers = lines
        .take_while(|l| !l.is_empty())
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();

    Ok(Some(Request {
        method: method.to_ascii_uppercase(),
        target: target.to_string(),
        headers,
    }))
}

/// A byte range the client asked for, resolved against a known size.
pub struct RangeSpec {
    pub start: u64,
    /// Inclusive, as in `Content-Range`.
    pub end: u64,
}

impl RangeSpec {
    pub fn len(&self) -> u64 {
        self.end - self.start + 1
    }
}

/// `Ok(None)`: no usable range header. `Err(())`: syntactically fine
/// but unsatisfiable against `size`, which is a 416.
pub fn parse_range(value: &str, size: u64) -> Result<Option<RangeSpec>, ()> {
    let Some(spec) = value.trim().strip_prefix("bytes=") else {
        return Ok(None);
    };
    // Multi-range requests are legal and nothing here needs them; a
    // server may answer with the whole file instead.
    let Some((from, to)) = spec.split_once('-') else {
        return Ok(None);
    };
    if spec.contains(',') {
        return Ok(None);
    }

    let (start, end) = match (from.trim(), to.trim()) {
        // "bytes=-N": the last N bytes.
        ("", n) => {
            let n: u64 = n.parse().map_err(|_| ())?;
            if n == 0 || size == 0 {
                return Err(());
            }
            (size.saturating_sub(n), size - 1)
        }
        (a, "") => (a.parse().map_err(|_| ())?, size.saturating_sub(1)),
        (a, b) => (
            a.parse().map_err(|_| ())?,
            b.parse::<u64>()
                .map_err(|_| ())?
                .min(size.saturating_sub(1)),
        ),
    };

    if size == 0 || start >= size || start > end {
        return Err(());
    }
    Ok(Some(RangeSpec { start, end }))
}

/// Writes a status line, the given headers, `Connection: close`, and
/// the blank line. Body (if any) is streamed by the caller.
pub async fn write_head(
    stream: &mut TcpStream,
    status: &str,
    headers: &[(String, String)],
) -> io::Result<()> {
    let mut head = format!("HTTP/1.1 {status}\r\n");
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("Connection: close\r\n\r\n");
    stream.write_all(head.as_bytes()).await
}

pub async fn write_text(stream: &mut TcpStream, status: &str, ctype: &str, body: &str) {
    let headers = [
        ("Content-Type".into(), ctype.into()),
        ("Content-Length".into(), body.len().to_string()),
    ];
    let _ = write_head(stream, status, &headers).await;
    let _ = stream.write_all(body.as_bytes()).await;
}

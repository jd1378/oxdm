//! Local HTTP server that serves large synthetic files, each endpoint
//! wrong in one specific way, so oxdm's download paths can be exercised
//! without depending on the internet.
//!
//!     cargo run -p oxdm-testserver -- --port 8088 --rate 5MiB
//!
//! Then open `http://127.0.0.1:8088/` for the index.

mod catalog;
mod http;

use std::io;
use std::net::SocketAddr;
use std::sync::OnceLock;
use std::time::Duration;

use catalog::{Checksums, Endpoint, Ranges, SIZE};
use md5::Md5;
use sha1::{Digest, Sha1};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};

const CHUNK: usize = 256 * 1024;

/// The digests of the two file bodies this server can produce. Both are
/// computed at startup: the endpoints that lie about their checksums do
/// it by handing out the *other* body's digest, which looks like a real
/// hash and is reliably wrong.
struct Digests {
    ones_md5: String,
    ones_sha1: String,
    zeros_md5: String,
    zeros_sha1: String,
}

static DIGESTS: OnceLock<Digests> = OnceLock::new();

fn digests() -> &'static Digests {
    DIGESTS.get().expect("computed before serving")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hash_fill(fill: u8) -> (String, String) {
    let block = vec![fill; CHUNK];
    let (mut md5, mut sha1) = (Md5::new(), Sha1::new());
    let mut left = SIZE as usize;
    while left > 0 {
        let n = left.min(CHUNK);
        md5.update(&block[..n]);
        sha1.update(&block[..n]);
        left -= n;
    }
    (hex(&md5.finalize()), hex(&sha1.finalize()))
}

struct Config {
    addr: SocketAddr,
    /// Bytes per second per connection; 0 is unthrottled.
    rate: u64,
}

fn parse_rate(spec: &str) -> Result<u64, String> {
    let spec = spec.trim();
    let digits = spec.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    let unit = spec[digits.len()..].to_ascii_lowercase();
    let n: u64 = digits
        .parse()
        .map_err(|_| format!("not a number: {spec:?}"))?;
    let mult = match unit.trim_end_matches("ib").trim_end_matches('b') {
        "" => 1,
        "k" => 1024,
        "m" => 1024 * 1024,
        "g" => 1024 * 1024 * 1024,
        u => return Err(format!("unknown unit {u:?} in {spec:?}")),
    };
    Ok(n * mult)
}

fn parse_args() -> Result<Config, String> {
    let mut port = 8088u16;
    let mut host = "127.0.0.1".to_string();
    let mut rate = 0u64;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--port" => port = value()?.parse().map_err(|_| "bad --port".to_string())?,
            "--host" => host = value()?,
            "--rate" => rate = parse_rate(&value()?)?,
            "--help" | "-h" => {
                println!(
                    "oxdm-testserver [--host H] [--port N] [--rate BYTES_PER_SEC]\n\
                     \n\
                     --rate accepts 500k / 5MiB / 1G; 0 (default) is unthrottled.\n\
                     Any endpoint takes ?rate=… to override it per request."
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }

    let addr = format!("{host}:{port}")
        .parse()
        .map_err(|_| format!("bad address {host}:{port}"))?;
    Ok(Config { addr, rate })
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let cfg = match parse_args() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("oxdm-testserver: {e}");
            std::process::exit(2);
        }
    };

    println!("hashing {} MiB of each body…", SIZE / 1024 / 1024);
    let (ones, zeros) = tokio::join!(
        tokio::task::spawn_blocking(|| hash_fill(0xFF)),
        tokio::task::spawn_blocking(|| hash_fill(0x00)),
    );
    let (ones_md5, ones_sha1) = ones.expect("hash task");
    let (zeros_md5, zeros_sha1) = zeros.expect("hash task");
    let _ = DIGESTS.set(Digests {
        ones_md5,
        ones_sha1,
        zeros_md5,
        zeros_sha1,
    });

    let listener = TcpListener::bind(cfg.addr).await?;
    let base = format!("http://{}", listener.local_addr()?);
    println!("serving {base}/");
    if cfg.rate > 0 {
        println!("throttled to {} bytes/s per connection", cfg.rate);
    }
    for e in catalog::ENDPOINTS {
        println!("  {base}{}", e.path);
    }

    loop {
        let (stream, _) = listener.accept().await?;
        tokio::spawn(async move {
            let _ = serve(stream, cfg.rate).await;
        });
    }
}

async fn serve(mut stream: TcpStream, default_rate: u64) -> io::Result<()> {
    let Some(req) = http::read_request(&mut stream).await? else {
        return Ok(());
    };

    // One line per request: the point of this server is to be able to
    // say what a client actually asked for — how many probes a paste
    // turned into, whether a resume sent the Range it claimed.
    println!(
        "{} {}{}",
        req.method,
        req.target(),
        req.header("range")
            .map(|r| format!(" [{r}]"))
            .unwrap_or_default()
    );

    if !matches!(req.method.as_str(), "GET" | "HEAD") {
        http::write_text(&mut stream, "405 Method Not Allowed", "text/plain", "").await;
        return Ok(());
    }

    let rate = req
        .query("rate")
        .and_then(|r| parse_rate(r).ok())
        .unwrap_or(default_rate);

    match req.path() {
        "/" => {
            let body = index_html();
            let ctype = "text/html; charset=utf-8";
            if req.method == "HEAD" {
                http::write_text(&mut stream, "200 OK", ctype, "").await;
            } else {
                http::write_text(&mut stream, "200 OK", ctype, &body).await;
            }
        }
        path => match catalog::find(path) {
            Some(e) => send_file(&mut stream, &req, e, rate).await?,
            None => {
                http::write_text(
                    &mut stream,
                    "404 Not Found",
                    "text/plain",
                    "no such endpoint\n",
                )
                .await
            }
        },
    }
    Ok(())
}

fn checksum_headers(e: &Endpoint) -> Vec<(String, String)> {
    let d = digests();
    // `X-Checksum-*` on purpose: odl's header sniffing stops at the
    // first strong digest it finds, so MD5 + SHA-1 is the pair that
    // reaches the client as two separate rows.
    let (md5, sha1) = match e.checksums {
        Checksums::None => return Vec::new(),
        Checksums::Valid => (&d.ones_md5, &d.ones_sha1),
        Checksums::Invalid => (&d.zeros_md5, &d.zeros_sha1),
        Checksums::Mixed => (&d.ones_md5, &d.zeros_sha1),
    };
    vec![
        ("X-Checksum-Md5".into(), md5.clone()),
        ("X-Checksum-Sha1".into(), sha1.clone()),
    ]
}

async fn send_file(
    stream: &mut TcpStream,
    req: &http::Request,
    e: &Endpoint,
    rate: u64,
) -> io::Result<()> {
    let mut headers = vec![("Content-Type".into(), "application/octet-stream".into())];
    headers.extend(checksum_headers(e));
    if e.ranges != Ranges::Absent {
        headers.push(("Accept-Ranges".into(), "bytes".into()));
    }

    // Only the honest endpoint reads the header at all. The faking one
    // advertises ranges and then serves from zero, which is the whole
    // behaviour under test.
    let range = match (e.ranges, req.header("range")) {
        (Ranges::Honour, Some(v)) => match http::parse_range(v, SIZE) {
            Ok(r) => r,
            Err(()) => {
                let h = [("Content-Range".into(), format!("bytes */{SIZE}"))];
                return http::write_head(stream, "416 Range Not Satisfiable", &h).await;
            }
        },
        _ => None,
    };

    let (status, body_len) = match &range {
        Some(r) => {
            headers.push((
                "Content-Range".into(),
                format!("bytes {}-{}/{}", r.start, r.end, SIZE),
            ));
            ("206 Partial Content", r.len())
        }
        None => ("200 OK", SIZE),
    };

    if e.length_known {
        headers.push(("Content-Length".into(), body_len.to_string()));
    }

    http::write_head(stream, status, &headers).await?;
    if req.method == "HEAD" {
        return Ok(());
    }
    send_body(stream, e.fill, body_len, rate).await
}

async fn send_body(stream: &mut TcpStream, fill: u8, len: u64, rate: u64) -> io::Result<()> {
    let block = vec![fill; CHUNK];
    let mut left = len;
    while left > 0 {
        let n = left.min(CHUNK as u64) as usize;
        // A dropped connection is the normal end of a paused or
        // cancelled download, not an error worth reporting.
        if stream.write_all(&block[..n]).await.is_err() {
            return Ok(());
        }
        left -= n as u64;
        if rate > 0 {
            tokio::time::sleep(Duration::from_secs_f64(n as f64 / rate as f64)).await;
        }
    }
    stream.flush().await
}

fn index_html() -> String {
    let d = digests();
    let rows: String = catalog::ENDPOINTS
        .iter()
        .map(|e| {
            let checks = match e.checksums {
                Checksums::None => "—".to_string(),
                _ => {
                    let h = checksum_headers(e);
                    let mark = |ok: bool| if ok { "correct" } else { "wrong" };
                    let (m_ok, s_ok) = match e.checksums {
                        Checksums::Valid => (true, true),
                        Checksums::Invalid => (false, false),
                        _ => (true, false),
                    };
                    format!(
                        "<code>MD5</code> {} <small>{}</small><br>\
                         <code>SHA-1</code> {} <small>{}</small>",
                        mark(m_ok),
                        h[0].1,
                        mark(s_ok),
                        h[1].1,
                    )
                }
            };
            format!(
                "<tr><td><a href=\"{path}\">{path}</a></td>\
                 <td>{blurb}</td><td>{checks}</td></tr>",
                path = e.path,
                blurb = e.blurb,
            )
        })
        .collect();

    format!(
        "<!doctype html><meta charset=utf-8><title>oxdm test server</title>
<style>
 body{{font:14px/1.5 system-ui,sans-serif;margin:2rem;max-width:70rem}}
 table{{border-collapse:collapse;width:100%}}
 td,th{{border-bottom:1px solid #ccc;padding:.5rem;vertical-align:top;text-align:left}}
 small{{font-family:ui-monospace,monospace;color:#666;word-break:break-all}}
</style>
<h1>oxdm test server</h1>
<p>Every file is {size} MiB. <code>zeros</code> bodies are <code>0x00</code>,
 <code>ones</code> are <code>0xFF</code>.
 Append <code>?rate=1MiB</code> to any URL to throttle that response.</p>
<table>
 <tr><th>endpoint</th><th>behaviour</th><th>offered checksums</th></tr>
 {rows}
</table>
<p>Reference digests — zeros: <small>md5 {zm} · sha1 {zs}</small><br>
 ones: <small>md5 {om} · sha1 {os}</small></p>",
        size = SIZE / 1024 / 1024,
        zm = d.zeros_md5,
        zs = d.zeros_sha1,
        om = d.ones_md5,
        os = d.ones_sha1,
    )
}

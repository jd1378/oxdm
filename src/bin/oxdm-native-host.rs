//! Browser native-messaging host.
//!
//! Browsers (Chrome, Firefox, …) launch this binary via a per-OS
//! manifest and exchange length-prefixed JSON over stdin/stdout:
//!
//! ```text
//!   stdin :  [u32 LE length] [N bytes UTF-8 JSON]
//!   stdout:  [u32 LE length] [N bytes UTF-8 JSON]
//! ```
//!
//! This host is a thin shim: every received `CaptureRequest` (or any
//! tagged IPC frame) is forwarded to the local oxdm WebSocket bridge.
//! That keeps the wire protocol — and the auth model — identical
//! between transports.
//!
//! ## Discovery
//!
//! Two sources, in priority order, supply `port` + `token`:
//!
//! 1. Command-line flags:
//!
//!    ```text
//!      --port      <u16>          IPC port the main app's WS bridge listens on
//!      --token     <string>       extension auth token (visible in argv —
//!                                 dev-only; prefer --token-fd or DB discovery)
//!      --token-fd  <N>            read the token from inherited file
//!                                 descriptor N. Token never appears in
//!                                 `ps` / `/proc/<pid>/cmdline`. Pattern:
//!                                 `oxdm-native-host --token-fd 3 3< file`
//!      --db-path   <PATH>         override the oxdm.db location used for
//!                                 auto-discovery (testing / portable installs)
//!    ```
//!
//!    The browser native-messaging launcher always invokes the binary
//!    with the extension origin as `argv[1]`; that token is ignored
//!    here (it has no `--` prefix). Any extra argv parsed by the loop
//!    below applies in addition.
//!
//! 2. Auto-discovery: read `ipc_port` + `ext_token` from `oxdm.db`'s
//!    `settings` table. The DB path defaults to
//!    `dirs::data_dir()/oxdm/oxdm.db` — `~/.local/share/oxdm/oxdm.db`
//!    (Linux), `~/Library/Application Support/oxdm/oxdm.db` (macOS),
//!    `%APPDATA%\oxdm\oxdm.db` (Windows) — overridable via `--db-path`.
//!    The query opens the file read-only so a running daemon's
//!    writers are never blocked.
//!
//! If neither source yields both fields, the host exits with code `1`
//! and the error is surfaced to the browser via stderr.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use futures::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let cfg = match resolve_config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("oxdm-native-host: {e}");
            std::process::exit(1);
        }
    };

    let url = format!("ws://127.0.0.1:{}", cfg.port);
    let (mut ws, _) = match tokio_tungstenite::connect_async(&url).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("oxdm-native-host: cannot reach {url}: {e}");
            std::process::exit(1);
        }
    };

    let auth = serde_json::json!({ "token": cfg.token }).to_string();
    if let Err(e) = ws.send(Message::text(auth)).await {
        eprintln!("oxdm-native-host: auth send failed: {e}");
        std::process::exit(1);
    }

    // stdin is a sync blocking pipe; read it on a dedicated OS thread
    // and forward decoded frames to the async side via a channel.
    // Running stdin->ws concurrently with ws->stdout is required: the
    // browser keeps stdin open for the lifetime of a persistent port
    // and expects responses to stream back as the bridge produces them.
    let (tx, mut rx) = mpsc::channel::<String>(16);
    std::thread::spawn(move || {
        let mut stdin = io::stdin();
        loop {
            let mut len_buf = [0u8; 4];
            if stdin.read_exact(&mut len_buf).is_err() {
                break;
            }
            let len = u32::from_le_bytes(len_buf) as usize;
            // Chrome caps inbound messages at 4 MB; reject anything
            // larger to avoid an OOM-by-bad-frame on a hostile origin.
            if len == 0 || len > 4 * 1024 * 1024 {
                break;
            }
            let mut buf = vec![0u8; len];
            if stdin.read_exact(&mut buf).is_err() {
                break;
            }
            let text = match String::from_utf8(buf) {
                Ok(s) => s,
                Err(_) => continue,
            };
            // Sanity-parse so we drop garbage on the floor instead of
            // forwarding it to the bridge.
            if serde_json::from_str::<Value>(&text).is_err() {
                continue;
            }
            if tx.blocking_send(text).is_err() {
                break;
            }
        }
        // Dropping tx signals stdin EOF to the async loop.
    });

    // Which side ended the session. The browser owns the lifetime: it
    // closes the port, stdin reaches EOF, and that is the one ordinary
    // way to finish. The socket going first is a failure every time,
    // and it used to be a silent one — status 0, empty stderr — so a
    // rejected token and a healthy shutdown were the same event from
    // the outside, and the extension could only report "native host
    // disconnected".
    let mut browser_hung_up = false;
    let mut why: Option<String> = None;
    loop {
        tokio::select! {
            maybe_frame = rx.recv() => match maybe_frame {
                Some(text) => {
                    if let Err(e) = ws.send(Message::text(text)).await {
                        why = Some(format!("sending to oxdm failed: {e}"));
                        break;
                    }
                }
                None => {
                    let _ = ws.close(None).await;
                    browser_hung_up = true;
                    break;
                }
            },
            maybe_msg = ws.next() => match maybe_msg {
                Some(Ok(Message::Text(t))) => write_framed(&t),
                Some(Ok(Message::Close(frame))) => {
                    why = Some(close_reason(frame));
                    break;
                }
                None => {
                    why = Some("oxdm closed the connection".to_owned());
                    break;
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => {
                    why = Some(format!("connection to oxdm failed: {e}"));
                    break;
                }
            },
        }
    }

    if !browser_hung_up {
        eprintln!(
            "oxdm-native-host: {}",
            why.unwrap_or_else(|| "oxdm closed the connection".to_owned())
        );
        std::process::exit(1);
    }

    // Drain any remaining ws frames the bridge already queued before
    // the socket closes, so a last reply isn't lost when stdin EOFs
    // immediately after the final request.
    while let Some(Ok(Message::Text(t))) = ws.next().await {
        write_framed(&t);
    }
}

/// What a close frame says, for stderr.
///
/// The daemon names the cause ("auth rejected") precisely so this line
/// can carry it. A close with no reason still gets a sentence, because
/// the alternative is the silence this exists to end.
fn close_reason(frame: Option<tokio_tungstenite::tungstenite::protocol::CloseFrame>) -> String {
    match frame {
        Some(f) if !f.reason.is_empty() => format!("oxdm closed the connection: {}", f.reason),
        _ => "oxdm closed the connection".to_owned(),
    }
}

fn write_framed(payload: &str) {
    let bytes = payload.as_bytes();
    let len = bytes.len() as u32;
    let mut stdout = io::stdout().lock();
    let _ = stdout.write_all(&len.to_le_bytes());
    let _ = stdout.write_all(bytes);
    let _ = stdout.flush();
}

struct HostConfig {
    port: u16,
    token: String,
}

fn resolve_config() -> Result<HostConfig, String> {
    let mut port: Option<u16> = None;
    let mut token: Option<String> = None;
    let mut db_override: Option<PathBuf> = None;

    let mut argv = std::env::args().skip(1);
    while let Some(a) = argv.next() {
        match a.as_str() {
            "--port" => {
                port = argv.next().and_then(|v| v.parse().ok());
            }
            "--token" => {
                token = argv.next();
            }
            "--token-fd" => {
                let n: Option<i32> = argv.next().and_then(|v| v.parse().ok());
                token = match n.and_then(|fd| read_token_from_fd(fd).ok()) {
                    Some(s) => Some(s),
                    None => return Err("--token-fd: invalid fd or read failed".into()),
                };
            }
            "--db-path" => {
                db_override = argv.next().map(PathBuf::from);
            }
            // Browsers pass extension origin + parent-window-id as bare
            // positional args. Ignore anything we don't recognize so
            // the host stays compatible with launcher conventions.
            _ => {}
        }
    }

    if let (Some(port), Some(token)) = (port, token.clone()) {
        return Ok(HostConfig { port, token });
    }

    let db = db_override.unwrap_or_else(default_db_path);
    let (db_port, db_token) = read_db(&db).map_err(|e| {
        format!(
            "no --port/--token flags and could not read {}: {e}",
            db.display()
        )
    })?;
    Ok(HostConfig {
        port: port.unwrap_or(db_port),
        token: token.unwrap_or(db_token),
    })
}

/// Read a token from an inherited file descriptor. Trims trailing
/// newline. Limit to a sane upper bound so a hostile parent can't
/// blow up the host with a multi-GB "secret".
#[cfg(unix)]
fn read_token_from_fd(fd: i32) -> std::io::Result<String> {
    use std::os::fd::FromRawFd;
    if fd < 0 {
        return Err(std::io::Error::other("negative fd"));
    }
    // SAFETY: we take ownership of the inherited fd; the parent's
    // contract is that it's a read end pre-positioned at the secret.
    let mut f = unsafe { std::fs::File::from_raw_fd(fd) };
    let mut buf = Vec::with_capacity(64);
    use std::io::Read;
    (&mut f).take(4096).read_to_end(&mut buf)?;
    let s = String::from_utf8(buf).map_err(std::io::Error::other)?;
    Ok(s.trim_end_matches(['\r', '\n']).to_owned())
}

#[cfg(not(unix))]
fn read_token_from_fd(_fd: i32) -> std::io::Result<String> {
    Err(std::io::Error::other(
        "--token-fd is unsupported on this platform; use --token or DB discovery",
    ))
}

fn default_db_path() -> PathBuf {
    let dir = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    dir.join("oxdm").join("oxdm.db")
}

fn read_db(path: &Path) -> Result<(u16, String), String> {
    use rusqlite::{Connection, OpenFlags};
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| e.to_string())?;
    let port: String = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'ipc_port'",
            [],
            |r| r.get(0),
        )
        .map_err(|e| format!("settings.ipc_port: {e}"))?;
    let token: String = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'ext_token'",
            [],
            |r| r.get(0),
        )
        .map_err(|e| format!("settings.ext_token: {e}"))?;
    // Settings values are stored as JSON literals — strip quotes from
    // strings, parse port as integer.
    let token = token.trim_matches('"').to_owned();
    let port: u16 = port
        .trim_matches('"')
        .parse()
        .map_err(|e| format!("settings.ipc_port not numeric: {e}"))?;
    if token.is_empty() {
        return Err("settings.ext_token is empty; open oxdm Settings to generate one".into());
    }
    Ok((port, token))
}

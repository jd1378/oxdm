//! Loopback WebSocket bridge for browser extensions.
//!
//! Bound to `127.0.0.1:<port>` only — never any external interface.
//! The handshake's `Origin` must be a browser-extension one (or absent,
//! for non-browser callers), and the first message must be
//! `{"token":"…"}` matching a non-empty `AppState::ext_token`.
//!
//! Protocol: each post-auth frame is JSON. Two shapes accepted:
//!   - **Tagged** — has a `kind` field (`capture` / `list_queues` /
//!     `evaluate_url` / `batch_capture`). Carries an optional `id` for
//!     reply correlation.
//!   - **Bare v1** — no `kind`, decoded as a [`CaptureRequest`] directly.
//!     Replies are emitted without an `id` to match the original wire
//!     shape that extensions in the wild already expect.

use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::Value;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

use crate::data::AppState;
use crate::domain::CaptureRequest;
use crate::domain::capture::{CaptureResponse, CaptureRules, IpcRequest, QueueSummary};
use crate::ipc::IpcError;

pub async fn run(state: Arc<AppState>, port: u16) -> Result<(), IpcError> {
    let addr = format!("127.0.0.1:{port}");
    let listener = TcpListener::bind(&addr).await?;
    tracing::info!(%addr, "ipc websocket listening");

    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, "accept failed");
                continue;
            }
        };
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(state, stream).await {
                tracing::debug!(?peer, error = %e, "ws session ended");
            }
        });
    }
}

#[derive(Deserialize)]
struct AuthFrame {
    token: String,
}

/// Which `Origin`s may open the bridge.
///
/// A browser sends `Origin` on every WebSocket handshake it makes on a
/// page's behalf, so this is what separates "the extension we pair
/// with" from "any site the user happens to be visiting", which can
/// otherwise reach 127.0.0.1 freely. Extension schemes are allowed;
/// web schemes are refused. A missing header means the caller is not a
/// browser page at all (the CLI, a test) and is left to the token.
fn origin_allowed(origin: Option<&str>) -> bool {
    match origin {
        None => true,
        Some(o) => {
            let o = o.trim();
            o.starts_with("chrome-extension://")
                || o.starts_with("moz-extension://")
                || o.starts_with("safari-web-extension://")
                || o.starts_with("extension://")
        }
    }
}

async fn handle(state: Arc<AppState>, stream: tokio::net::TcpStream) -> Result<(), IpcError> {
    use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};

    let mut ws = tokio_tungstenite::accept_hdr_async(
        stream,
        |req: &Request, res: Response| -> Result<Response, ErrorResponse> {
            let origin = req
                .headers()
                .get("origin")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            if origin_allowed(origin.as_deref()) {
                return Ok(res);
            }
            tracing::warn!(origin = ?origin, "ws handshake refused: origin is not an extension");
            let mut deny = ErrorResponse::new(Some("origin not allowed".into()));
            *deny.status_mut() = tokio_tungstenite::tungstenite::http::StatusCode::FORBIDDEN;
            Err(deny)
        },
    )
    .await
    .map_err(|e| IpcError::Other(e.to_string()))?;

    // Auth. Bounded: an unauthenticated connection that says nothing
    // holds a task and a socket for as long as it likes otherwise.
    let first = match tokio::time::timeout(std::time::Duration::from_secs(10), ws.next()).await {
        Ok(Some(Ok(Message::Text(t)))) => t,
        Ok(_) => return Err(IpcError::Other("missing auth frame".into())),
        Err(_) => {
            let _ = ws.close(None).await;
            return Err(IpcError::Other("auth frame timed out".into()));
        }
    };
    let auth: AuthFrame =
        serde_json::from_str(&first).map_err(|e| IpcError::Other(format!("bad auth json: {e}")))?;
    let expected = state.ext_token().await;
    // An empty stored token used to mean "accept anything", which is
    // exactly backwards: no token means nothing has been paired yet, so
    // there is nobody to let in.
    if expected.is_empty() || !crate::ipc_local::auth::token_matches(&expected, &auth.token) {
        let _ = ws.close(None).await;
        return Err(IpcError::Other("auth rejected".into()));
    }

    // Capture frames.
    while let Some(msg) = ws.next().await {
        let msg = msg.map_err(|e| IpcError::Other(e.to_string()))?;
        let text = match msg {
            Message::Text(t) => t,
            Message::Close(_) => break,
            _ => continue,
        };
        let resp = dispatch(&state, &text).await;
        let payload = serde_json::to_string(&resp).unwrap();
        if ws.send(Message::Text(payload)).await.is_err() {
            break;
        }
    }
    Ok(())
}

async fn dispatch(state: &Arc<AppState>, text: &str) -> CaptureResponse {
    // Peek at `kind` to decide between tagged + bare shapes.
    let value: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            return CaptureResponse::Rejected {
                reason: e.to_string(),
                id: None,
            };
        }
    };
    let has_kind = value.get("kind").and_then(|v| v.as_str()).is_some();

    if !has_kind {
        // v1 bare path. Decode as CaptureRequest; reply without id.
        let req: CaptureRequest = match serde_json::from_value(value) {
            Ok(r) => r,
            Err(e) => {
                return CaptureResponse::Rejected {
                    reason: e.to_string(),
                    id: None,
                };
            }
        };
        return match crate::ipc::accept_capture(state, req).await {
            Ok(job_id) => CaptureResponse::Accepted {
                job_id: job_id.to_string(),
                id: None,
            },
            Err(reason) => CaptureResponse::Rejected { reason, id: None },
        };
    }

    let req: IpcRequest = match serde_json::from_value(value) {
        Ok(r) => r,
        Err(e) => {
            return CaptureResponse::Rejected {
                reason: e.to_string(),
                id: None,
            };
        }
    };

    match req {
        IpcRequest::Capture { id, req } => match crate::ipc::accept_capture(state, req).await {
            Ok(job_id) => CaptureResponse::Accepted {
                job_id: job_id.to_string(),
                id,
            },
            Err(reason) => CaptureResponse::Rejected { reason, id },
        },
        IpcRequest::GetCaptureRules { id } => {
            let s = state.settings().await;
            CaptureResponse::CaptureRules {
                id,
                rules: CaptureRules {
                    min_size: s.capture_min_size,
                    skip_domains: s.capture_skip_domains,
                    skip_extensions: s.capture_skip_extensions,
                    skip_mime_prefixes: s.capture_skip_mime_prefixes,
                    allow_extensions: s.capture_allow_extensions,
                    allow_mime_prefixes: s.capture_allow_mime_prefixes,
                },
            }
        }
        IpcRequest::ListQueues { id } => {
            let queues = state
                .queues_snapshot()
                .await
                .into_iter()
                .map(|q| QueueSummary {
                    id: q.id.to_string(),
                    name: q.name,
                })
                .collect();
            CaptureResponse::Queues { id, queues }
        }
        IpcRequest::EvaluateUrl {
            id,
            url,
            referrer,
            cookies,
            user_agent,
            headers,
        } => crate::ipc::evaluator::evaluate(id, url, referrer, cookies, user_agent, headers).await,
        IpcRequest::BatchCapture {
            id,
            interactive,
            queue,
            queue_name,
            auto_start_queue,
            mut items,
        } => {
            let default_qid = crate::ipc::resolve_queue(state, queue, queue_name.as_deref()).await;
            for item in items.iter_mut() {
                if item.queue.is_none()
                    && item.queue_name.is_none()
                    && let Some(qid) = default_qid
                {
                    item.queue = Some(qid.0);
                }
                if !item.auto_start_queue {
                    item.auto_start_queue = auto_start_queue;
                }
            }
            // Triage dialog unless the caller explicitly opted out
            // *and* every item has a resolvable queue. The queue
            // requirement is the power-user signal an extension-side
            // attack can't forge — the extension itself never sets
            // `queue` / `queue_name` on the wire, so a hostile page
            // can't bypass triage by toggling `interactive: false`.
            let every_item_routed = !items.is_empty()
                && items
                    .iter()
                    .all(|i| i.queue.is_some() || i.queue_name.is_some());
            let take_fast_path = !interactive && every_item_routed;
            if take_fast_path {
                let mut accepted = Vec::new();
                let mut rejected = Vec::new();
                for item in items {
                    match crate::ipc::accept_capture(state, item).await {
                        Ok(jid) => accepted.push(jid.to_string()),
                        Err(reason) => rejected.push(reason),
                    }
                }
                return CaptureResponse::BatchResult {
                    id,
                    accepted,
                    rejected,
                };
            }
            match crate::ipc::batch::stage_for_dialog(&items) {
                Ok(path) => {
                    crate::daemon::tray::spawn_batch_gui(&path);
                    CaptureResponse::BatchResult {
                        id,
                        accepted: Vec::new(),
                        rejected: Vec::new(),
                    }
                }
                Err(e) => CaptureResponse::Rejected {
                    reason: format!("batch stage failed: {e}"),
                    id: Some(id),
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_extension_pages_may_open_the_bridge() {
        assert!(origin_allowed(Some("chrome-extension://abcdefg")));
        assert!(origin_allowed(Some("moz-extension://abcdefg")));
        // Not a browser page at all — the token is the gate there.
        assert!(origin_allowed(None));
    }

    #[test]
    fn a_web_page_may_not() {
        assert!(!origin_allowed(Some("https://example.com")));
        assert!(!origin_allowed(Some("http://localhost:3000")));
        assert!(!origin_allowed(Some("null")));
        assert!(!origin_allowed(Some("")));
    }
}

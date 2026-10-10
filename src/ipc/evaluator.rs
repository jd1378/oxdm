//! Probe one URL on behalf of an extension's mass-select dialog.
//!
//! Asks the way the capture's download will: through odl's own probe,
//! with the cookies, UA and referrer the capture would carry, the
//! global headers and the configured proxy. A probe of its own would
//! describe a file the download may never get. A HEAD, for one, is
//! routed differently from the GET that parts send by some hosts:
//! github.com sends a signed-in HEAD for a release asset to a host
//! that answers 401.

use std::time::Duration;

use crate::data::AppState;
use crate::domain::ProbeTarget;
use crate::domain::capture::{CaptureRequest, CaptureResponse};

/// The dialog shows a row per link and waits on each; a server that
/// keeps it waiting longer has as good as said nothing.
const EVALUATE_TIMEOUT: Duration = Duration::from_secs(10);

pub async fn evaluate(state: &AppState, id: String, capture: CaptureRequest) -> CaptureResponse {
    let url = capture.url.to_string();
    if let Err(reason) = crate::ipc::guard_public_http_url(&capture.url) {
        return err(id, url, reason);
    }
    let probe = match tokio::time::timeout(
        EVALUATE_TIMEOUT,
        state.probe_shared(ProbeTarget::from_capture(&capture)),
    )
    .await
    {
        Ok(Ok(probe)) => probe,
        Ok(Err(e)) => return err(id, url, e.to_string()),
        Err(_) => return err(id, url, "timed out".to_owned()),
    };
    CaptureResponse::Evaluated {
        id,
        url,
        filename: Some(probe.filename).filter(|n| !n.is_empty()),
        size: probe.size,
        mime_type: probe.mime_type,
        etag: probe.etag,
        supports_resume: Some(probe.is_resumable),
        error: None,
    }
}

fn err(id: String, url: String, message: String) -> CaptureResponse {
    CaptureResponse::Evaluated {
        id,
        url,
        filename: None,
        size: None,
        mime_type: None,
        etag: None,
        supports_resume: None,
        error: Some(message),
    }
}

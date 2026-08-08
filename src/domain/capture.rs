use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// Wire-level payload received from a browser extension over the IPC
/// bridge. Defines the *only* contract extensions must implement.
///
/// All fields except `url` are optional. The download manager will fill
/// gaps from server response headers during evaluation (filename, size).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureRequest {
    pub url: url::Url,
    /// Suggested filename from `Content-Disposition` or page link text.
    #[serde(default)]
    pub filename: Option<String>,
    /// Page the link came from. Used as `Referer` if not overridden in
    /// `headers`.
    #[serde(default)]
    pub referrer: Option<url::Url>,
    /// Cookie header value, *as a single string* — the extension is the
    /// only component that can read the browser's cookie jar.
    #[serde(default)]
    pub cookies: Option<String>,
    /// User-Agent the browser used. Honored verbatim so anti-leech
    /// servers see the same UA on the resumed download.
    #[serde(default)]
    pub user_agent: Option<String>,
    /// Arbitrary extra headers. Merged on top of cookies/referrer/UA.
    #[serde(default)]
    pub headers: IndexMap<String, String>,
    /// Reported size in bytes if the extension already saw it.
    #[serde(default)]
    pub size: Option<u64>,
    /// MIME type if known. Display-only, never used for routing.
    #[serde(default)]
    pub mime_type: Option<String>,
    /// If `true`, oxdm should pop the Add-Download dialog. If `false`,
    /// queue immediately with defaults. Mirrors IDM's "ask each time"
    /// vs "auto-start" preference.
    #[serde(default)]
    pub interactive: bool,
    /// Power-user override — target queue by id. Drops to `Main` when
    /// the id is unknown.
    #[serde(default)]
    pub queue: Option<uuid::Uuid>,
    /// Power-user override — target queue by case-insensitive name.
    /// Ignored when `queue` is set.
    #[serde(default)]
    pub queue_name: Option<String>,
    /// If `true`, oxdm also tells the receiving queue to start its
    /// scheduler after adding the job. Lets scripts say "drop this in
    /// Mirrors and go" in a single round-trip. When the captured job
    /// goes interactive, this is treated as the dialog's Start now
    /// preselect.
    #[serde(default)]
    pub auto_start_queue: bool,
}

fn default_true() -> bool {
    true
}

impl CaptureRequest {
    /// A capture that is nothing but a link — what a pasted or
    /// dropped list gives us, with every other field left for the
    /// probe to fill in.
    pub fn from_url(url: url::Url) -> Self {
        Self {
            url,
            filename: None,
            referrer: None,
            cookies: None,
            user_agent: None,
            headers: IndexMap::new(),
            size: None,
            mime_type: None,
            interactive: true,
            queue: None,
            queue_name: None,
            auto_start_queue: false,
        }
    }
}

/// Response sent back to the extension after a capture is accepted.
///
/// `id` echoes the request's correlation id when one was sent. The v1
/// bare-CaptureRequest path emits responses without an `id`, matching
/// the original wire shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum CaptureResponse {
    Accepted {
        job_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    Rejected {
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    Queues {
        id: String,
        queues: Vec<QueueSummary>,
    },
    Evaluated {
        id: String,
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        etag: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        supports_resume: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    BatchResult {
        id: String,
        accepted: Vec<String>,
        rejected: Vec<String>,
    },
    CaptureRules {
        id: String,
        rules: CaptureRules,
    },
}

/// Capture-filter rules the browser extension applies before forwarding
/// a download. Authored in oxdm; the extension fetches via
/// `get_capture_rules` on connect and caches the result.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CaptureRules {
    #[serde(default)]
    pub min_size: u64,
    #[serde(default)]
    pub skip_domains: Vec<String>,
    #[serde(default)]
    pub skip_extensions: Vec<String>,
    #[serde(default)]
    pub skip_mime_prefixes: Vec<String>,
    #[serde(default)]
    pub allow_extensions: Vec<String>,
    #[serde(default)]
    pub allow_mime_prefixes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueSummary {
    pub id: String,
    pub name: String,
}

/// Tagged inbound request. Discriminated by the `kind` field. The
/// WebSocket bridge keeps back-compat with v1 by treating any frame
/// *without* `kind` as a bare `CaptureRequest`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IpcRequest {
    Capture {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(flatten)]
        req: CaptureRequest,
    },
    ListQueues {
        id: String,
    },
    EvaluateUrl {
        id: String,
        url: url::Url,
        #[serde(default)]
        referrer: Option<url::Url>,
        #[serde(default)]
        cookies: Option<String>,
        #[serde(default)]
        user_agent: Option<String>,
        #[serde(default)]
        headers: indexmap::IndexMap<String, String>,
    },
    GetCaptureRules {
        id: String,
    },
    BatchCapture {
        id: String,
        /// Default is "open the triage dialog" — bulk-add without a
        /// prompt is only granted when the caller passes
        /// `interactive: false` *and* names a queue (id or name) at
        /// the top level or on every item. The extension itself never
        /// sets this; the wire surface is reserved for power-user
        /// scripts that hold the auth token and know exactly where
        /// the items belong.
        #[serde(default = "default_true")]
        interactive: bool,
        /// Default queue id for items that don't carry their own.
        /// Falls back to Main when the id is unknown.
        #[serde(default)]
        queue: Option<uuid::Uuid>,
        /// Default queue name (case-insensitive) for items that don't
        /// carry their own. Ignored when `queue` is set.
        #[serde(default)]
        queue_name: Option<String>,
        /// If `true`, the resolved queue is also told to start its
        /// scheduler once all items are added.
        #[serde(default)]
        auto_start_queue: bool,
        items: Vec<CaptureRequest>,
    },
}

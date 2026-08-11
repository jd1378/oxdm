# oxdm Browser Extension API

oxdm provides a stable host-side contract so any third-party browser extension can hand it download links. **No extension is shipped from this repo** — implementations live elsewhere and depend only on what is documented here.

## Transports

Extensions can talk to oxdm over either of two transports. They are equivalent at the message level; pick whichever fits the browser.

### 1. WebSocket (recommended for development)

- Endpoint: `ws://127.0.0.1:<port>` (loopback only — never bound to an external interface).
- Default port: **27812**, configurable in `Settings → Browser integration → IPC port`.
- The first frame from the extension **must** be the auth frame:

  ```json
  { "token": "<base64url-256-bit>" }
  ```

  The token is generated on first launch, displayed in `Settings → Browser integration → Extension token`, and rotated by the **Regenerate** button. A mismatched token closes the socket immediately.

- After auth, the extension sends one or more `CaptureRequest` JSON messages. oxdm replies with one `CaptureResponse` per request.

### 2. Native messaging (recommended for store-published extensions)

Browsers launch a registered host binary and exchange length-prefixed JSON over stdin/stdout (4-byte little-endian length, then UTF-8 JSON). oxdm ships **`oxdm-native-host`** as a thin shim that opens a fresh WebSocket session to the running oxdm app and forwards every framed message.

The host auto-discovers `port` + `token` by reading the `settings`
table of `oxdm.db` (read-only, no lock contention with a running
daemon). The extension does **not** see the token in this mode. CLI
overrides exist for dev / portable installs:

```text
  oxdm-native-host [--port <u16>] [--token <STR> | --token-fd <N>] [--db-path <PATH>]
```

`--token` is visible in `/proc/<pid>/cmdline`; prefer `--token-fd <N>`
which reads the secret from an inherited file descriptor instead, or
omit both flags entirely and let the host pick up the token from
`oxdm.db`. The bundled `tools/install-native-host.sh` supports a
`--token-file <PATH>` option that drops a wrapper script for the
`--token-fd` pattern automatically.

If `oxdm` is not running when the host launches, `oxdm-native-host`
exits with code `1` and the error is surfaced to the browser via
stderr.

#### Sample manifest (Chromium-family, Linux)

`~/.config/google-chrome/NativeMessagingHosts/io.github.jd1378.oxdm.host.json`:

```json
{
  "name": "io.github.jd1378.oxdm.host",
  "description": "oxdm download capture host",
  "path": "/usr/local/bin/oxdm-native-host",
  "type": "stdio",
  "allowed_origins": [
    "chrome-extension://<extension-id>/"
  ]
}
```

Use the bundled installers (no wrapper script needed):

```sh
# Linux / macOS
oxdm/tools/install-native-host.sh \
    --chromium-id <32-char-extension-id> \
    --firefox-id  oxdm@jd1378.github.io

# Windows (PowerShell)
.\oxdm\tools\install-native-host.ps1 `
    -ChromiumId <32-char-extension-id> `
    -FirefoxId  oxdm@jd1378.github.io
```

Both write manifests to the per-user manifest directory for every
detected Chromium-family and Firefox-family browser.

#### Firefox manifest

`~/.mozilla/native-messaging-hosts/io.github.jd1378.oxdm.host.json`:

```json
{
  "name": "io.github.jd1378.oxdm.host",
  "description": "oxdm download capture host",
  "path": "/usr/local/bin/oxdm-native-host",
  "type": "stdio",
  "allowed_extensions": [
    "oxdm@jd1378.github.io"
  ]
}
```

## Wire shape (v1.1)

Two frame styles are accepted after auth:

- **Bare** — a top-level `CaptureRequest` with no `kind` field. v1 compat.
- **Tagged** — has a `kind` discriminant. Carries an optional `id` field
  the host echoes back for reply correlation:

  ```json
  { "kind": "capture", "id": "r17", "url": "https://…" }
  ```

  Tagged kinds: `capture`, `list_queues`, `evaluate_url`, `batch_capture`, `get_capture_rules`.

## Message: `CaptureRequest`

```json
{
  "url":         "https://example.com/file.zip",
  "filename":    "file.zip",
  "referrer":    "https://example.com/page",
  "cookies":     "session=abc; csrf=xyz",
  "user_agent":  "Mozilla/5.0 …",
  "headers":     { "Authorization": "Bearer …" },
  "size":        1048576,
  "mime_type":   "application/zip",
  "interactive": true
}
```

| field         | type                  | required | meaning                                                                          |
|---------------|-----------------------|----------|----------------------------------------------------------------------------------|
| `url`         | string (http/https)   | yes      | Target URL.                                                                      |
| `filename`    | string                | no       | Suggested filename. oxdm overrides with the server-provided name when present, and numbers it (`file (1).zip`) if another download in the list already has that name — names identify downloads, so they are unique across the whole list regardless of folder. |
| `referrer`    | string (URL)          | no       | Stored on the job and sent as `Referer` on every request. Shown and editable in Properties → Headers → Identification. A `Referer` in `headers` overrides it. Send it whenever the page it came from matters — hosts that check it reject the download otherwise. |
| `cookies`     | string                | no       | Cookie header value (the extension is the only component that can read jars).    |
| `user_agent`  | string                | no       | Stored as the job's `User-Agent` header and honoured verbatim, so anti-leech hosts see the browser's UA. Outranks both the global setting and oxdm's own default (`oxdm/<version>`), which is what a job without one sends. |
| `headers`     | `{ string: string }`  | no       | Extra headers. Merged on top of cookies / referrer / UA — a key here wins over the dedicated field of the same name. |
| `size`        | integer (bytes)       | no       | Reported size if the extension already saw it.                                   |
| `mime_type`   | string                | no       | Display-only.                                                                    |
| `interactive` | bool (default false)  | no       | If `true`, oxdm opens the Add-Download dialog. If `false`, it queues immediately. |
| `queue`            | UUID                | no | Target queue by id. Unknown id falls back to the Main queue. |
| `queue_name`       | string              | no | Target queue by case-insensitive name. Ignored when `queue` is set. |
| `auto_start_queue` | bool (default false)| no | After adding the job, also call `start_queue` on the resolved queue. Lets a script say "drop this in Mirrors and go" in a single round-trip. |

## Message: `CaptureResponse`

```json
{ "result": "accepted", "job_id": "01HX…" }
```
or
```json
{ "result": "rejected", "reason": "<human-readable>" }
```

A queued job's progress is **not** streamed back to the extension. oxdm owns the UX from this point on.

## Message: `ListQueues`

```json
{ "kind": "list_queues", "id": "r1" }
```

Reply:

```json
{ "result": "queues", "id": "r1",
  "queues": [ { "id": "01HX…", "name": "Main" } ] }
```

## Message: `EvaluateUrl`

Lets the extension's mass-select dialog probe a URL with the same
cookies/UA/referrer that the eventual capture would use. The host
issues a `HEAD` (falls back to a 1-byte ranged `GET` for hosts that
reject `HEAD`) and returns whatever metadata it could extract.

```json
{ "kind": "evaluate_url", "id": "r2",
  "url": "https://example.com/file.zip",
  "referrer": "https://example.com/page",
  "cookies": "session=abc",
  "user_agent": "Mozilla/5.0 …" }
```

Reply:

```json
{ "result": "evaluated", "id": "r2",
  "url": "https://example.com/file.zip",
  "filename": "file.zip",
  "size": 1048576,
  "mime_type": "application/zip",
  "etag": "\"abc\"",
  "supports_resume": true }
```

On failure, `error` is set and the metadata fields are absent.

## Message: `BatchCapture`

Submit N captures in one shot. Each item has the same shape as
`CaptureRequest` (so per-item `queue`, `queue_name`, `auto_start_queue`
all work). Top-level fields supply defaults for items that don't carry
their own.

```json
{ "kind": "batch_capture", "id": "r3",
  "interactive": false,
  "queue_name": "Mirrors",
  "auto_start_queue": true,
  "items": [ { "url": "https://…/a.zip" }, { "url": "https://…/b.zip" } ] }
```

| field              | type    | meaning                                                                                  |
|--------------------|---------|------------------------------------------------------------------------------------------|
| `interactive`      | bool    | Default `true`. The fast-path (no dialog) only fires when `interactive: false` *and* every item has a resolvable queue. Defence-in-depth: an attacker page driving the extension can't suppress triage because the extension never sets `queue` on the wire. |
| `queue`            | UUID    | Default target queue for items that don't carry their own.                              |
| `queue_name`       | string  | Default target queue by name. Ignored when `queue` is set.                              |
| `auto_start_queue` | bool    | Start the resolved queue's scheduler after adding. Applies to each item's queue.        |
| `items`            | array   | List of `CaptureRequest`s.                                                              |

Reply (only on `interactive: false`):

```json
{ "result": "batch_result", "id": "r3",
  "accepted": ["01HX…", "01HY…"],
  "rejected": [] }
```

On `interactive: true` the dialog handles the rest of the flow itself
and the reply carries empty `accepted` / `rejected` arrays.

## Message: `GetCaptureRules`

Authoring of "which downloads should the extension forward to oxdm" lives in oxdm. The extension fetches the rules on connect and caches them; oxdm is the single source of truth.

```json
{ "kind": "get_capture_rules", "id": "r4" }
```

Reply:

```json
{ "result": "capture_rules", "id": "r4",
  "rules": {
    "min_size": 0,
    "skip_domains": ["internal.example.com"],
    "skip_extensions": ["html", "htm", "php", "asp", "aspx", "jsp"],
    "skip_mime_prefixes": ["text/html", "application/xhtml"],
    "allow_extensions": [],
    "allow_mime_prefixes": []
  } }
```

| field                 | type        | meaning                                                                              |
|-----------------------|-------------|--------------------------------------------------------------------------------------|
| `min_size`            | u64 (bytes) | Minimum reported size before capture fires. `0` disables the threshold.              |
| `skip_domains`        | string[]    | Hostnames excluded from capture. Bare host matches the host itself and any subdomain. |
| `skip_extensions`     | string[]    | Lowercase, no leading dot.                                                            |
| `skip_mime_prefixes`  | string[]    | Matched by `startsWith` against the reported MIME.                                    |
| `allow_extensions`    | string[]    | Optional positive list. When non-empty, only matching extensions are captured.       |
| `allow_mime_prefixes` | string[]    | Optional positive list. When non-empty, only matching MIMEs are captured.            |

Skip lists subtract from the allow list. Authoring lives in `Settings → Browser integration` (or directly in `oxdm.db`'s `settings` blob); the extension is not expected to expose UI for these.

## Reference flow (extension author's perspective)

1. User clicks an "Send to oxdm" button on a page (or a context menu).
2. Extension reads the link's URL, the page's referrer, and the appropriate cookie jar entries.
3. Extension serializes a `CaptureRequest` and sends it via either transport.
4. Extension shows a brief toast based on `CaptureResponse`.
5. oxdm shows the download in its Queue / dialog.

## Versioning

Breaking changes to fields documented here will be gated behind a new top-level field (`"protocol": 2`) and a deprecation period. Adding optional fields is non-breaking.

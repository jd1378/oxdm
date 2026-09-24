# oxdm machine-interface reference

The authoritative copy is `oxdm help` (the `EXIT CODES` and `JSON OUTPUT`
sections). This file mirrors it for offline use. If they disagree, trust
`oxdm help` for the installed version.

## Invocation

```
oxdm [--format text|json] <COMMAND> [ARGS]
```

`--format` is accepted before or after the command. Every command talks to
the running oxdm over its private local socket. If none is running, the
command starts one in the tray (`oxdm --tray`) and waits up to 20 s for it.
On Linux it refuses to do that from a shell with neither `DISPLAY` nor
`WAYLAND_DISPLAY` (exit 6), since the user could never see that instance.

Input is validated before oxdm is contacted: a bad URL or flag exits 2
without starting anything.

## Commands

| command | purpose |
|---------|---------|
| `add URL... [-i FILE]` | add downloads and start them |
| `list [TEXT] [--queue NAME] [--state STATE]... [--category NAME]` | downloads, filtered |
| `status ID...` | the named downloads |
| `wait ID... [--timeout DUR]` | follow until each ends |
| `pause ID...` | pause |
| `resume ID... \| --failed [--queue NAME]` | continue from where it stopped |
| `restart ID... [--delete-file]` | discard downloaded data, start again |
| `remove ID... [--delete-file]` (alias `rm`) | take off the list |
| `probe URL` | ask the server without downloading |
| `queues` | list queues |
| `restore-agent` | recreate a deleted Agent category and queue (only when the user asks) |

`ID`: a full download id, or a unique prefix of at least 4 characters.
`wait` follows a queued download until it has run and ended, so do not
wait on one added with `--no-start` unless something will start it.
`add`, `resume` and `restart` accept `--wait [--timeout DUR]` to follow
what they started, exactly like `wait`. `DUR` is like `90s`, `10m`, `1h`.

### `add` flags

| flag | purpose |
|------|---------|
| `-i, --input FILE` | also read URLs from FILE, one per line (`-` = stdin); blank lines and lines starting with `#` or `//` skipped |
| `-o, --output PATH` | a folder (existing, or ending in `/`), or with one URL a file path. Default: the Agent category's folder |
| `--queue NAME` | an existing queue (case-insensitive). Default: the Agent category's queue |
| `-H, --header "Name: Value"` | repeatable. `@FILE` reads one header per line, `@-` from stdin. `Cookie` and `Authorization: Basic/Bearer` are stored encrypted |
| `--referrer URL` | Referer to send |
| `--connections N` | parallel connections (1 to 64) |
| `--checksum ALGO:HEX` | expected digest, checked at the end; repeatable. `md5`, `sha1`, `sha256`, `sha384`, `sha512` |
| `--no-start` | add only |
| `--new` | add even if the URL is already in the list |
| `--show` | open the progress window for each download |

Only `http` and `https` URLs are accepted. Repeats within one command are
dropped.

**Where new downloads go.** The first `add` creates an "Agent" category
(with its own folder, by default `Agent` inside the user's download
folder) and an "Agent" queue. Every new download is filed under that
category (`"category": "agent"`), saved in its folder and put in the
queue the category names in Settings (the Agent queue, unless the user
changed it). Neither is ever created a second time:

- category deleted by the user: downloads are filed by file type again,
  saved in that type's folder
- queue deleted by the user: downloads go to Main

Settings' "Reset Categories" does not bring a deleted Agent category
back. `oxdm restore-agent` recreates whichever of the two is missing and
points the category at the queue again; running it when nothing is
missing changes nothing. It answers with one line:

```json
{"type":"agent","category":"agent","save_dir":"/home/u/Downloads/Agent","queue":"Agent"}
```

`-o` and `--queue` override the folder and the queue; the category stays
Agent.

**Reuse.** Unless `--new` is given, a URL already in the list (same URL,
and when `-o` was given, the same folder and file name) is not added
again. It is reported with `"existing": true` and:

- finished, file present: `state: "complete"`, nothing downloaded
- finished, file gone: downloaded again
- stopped, failed, queued: resumed
- running: left alone
- in conflict (checksum mismatch, file changed): refused with exit 4

### `restart --delete-file`, `remove --delete-file`

Move the saved file to the desktop trash first (recoverable). Without it,
`restart` of a download whose file is still there saves the new copy
under a numbered name, and `remove` leaves the file on disk. `remove`
pauses a running download first; its partial data is always discarded.

## Exit codes

| code | kind | meaning |
|------|------|---------|
| 0 | | success |
| 1 | `other` | other / internal |
| 2 | `usage` | bad flag, URL, download id, or queue name |
| 3 | `network` | DNS, connection, HTTP status. See `retryable` |
| 4 | `conflict` | needs a decision: checksum mismatch, file changed on the server, name taken |
| 5 | `io` | disk full, not enough space, destination not writable |
| 6 | `daemon` | oxdm is not running and could not be started, or stopped answering |
| 7 | `stopped` | paused, cancelled or removed before it finished |
| 124 | `timeout` | `--timeout` ran out; the download continues |
| 130 | `interrupted` | Ctrl-C; the download continues |

With several downloads, the first one (in the order you named them) that
did not finish decides the code. A refused item (`rejected`) outranks
what happened while waiting.

## stdout: one JSON object per line

Every line has `"type"`. Lines about a download carry its `"id"`.

| `type` | fields | notes |
|--------|--------|-------|
| `added` | `id`, `url`, `queue`, `save_dir`, `filename`, `existing`, `state` | one per URL. `filename` is `null` until the server names it |
| `rejected` | `id`, `url`, `error` | this item was not done; others may have been. `id` is `null` when nothing was added |
| `paused` | `id`, `state` | `state` is the download's state after the command |
| `resumed` / `restarted` | `id`, `state` | |
| `removed` | `id`, `warning` | `warning`: the file could not be trashed, and why |
| `phase` | `id`, `phase` | informational; treat unknown values as such |
| `filename` | `id`, `filename` | the name it is being saved as |
| `progress` | `id`, `downloaded`, `total`, `speed_bps` | at most once a second; `total` is `null` when unknown; only while bytes are arriving |
| `retry_scheduled` | `id`, `part`, `attempt`, `max_attempts`, `delay_ms`, `server_requested` | a pause before a retry, not a hang. `part` is `null` for a whole-download step |
| `completed` | `id`, `path`, `already_complete` | terminal. `already_complete: true`: finished before this command |
| `failed` | `id`, `phase`, `error` | terminal. `phase` is `failed` or `conflict` |
| `stopped` | `id`, `phase` | terminal. `phase`: `paused`, `cancelled`, `removed` |
| `downloads` | `count`, `downloads` | `list` and `status` |
| `queues` | `queues` | |
| `agent` | `category`, `save_dir`, `queue` | `restore-agent` |
| `probe` | `url`, `filename`, `size`, `resumable`, `etag`, `last_modified`, `requires_auth`, `checksums` | |

When following several downloads, each gets exactly one terminal line.

`state` in `added` / `resumed` / `restarted`:

| value | meaning |
|-------|---------|
| `started` | its run started |
| `queued` | every download slot is busy; it starts by itself when one frees |
| `not_started` | `--no-start` |
| `running` | already downloading; left alone |
| `complete` | already finished and the file is there |

### `downloads` entries

```json
{
  "id": "4e23b477-93da-43c2-882f-a223ef941513",
  "url": "https://example.com/zeros-1g.bin",
  "filename": "zeros-1g.bin",
  "save_dir": "/home/u/Downloads/Agent",
  "path": "/home/u/Downloads/Agent/zeros-1g.bin",
  "file_exists": true,
  "queue": "Agent",
  "category": "agent",
  "state": "completed",
  "phase": "completed",
  "downloaded": 1073741824,
  "total": 1073741824,
  "percent": 100.0,
  "speed_bps": 0,
  "resumable": true,
  "error": null,
  "created_at": "2026-09-24T10:12:03Z",
  "finished_at": "2026-09-24T10:14:40Z"
}
```

- `state`: `queued`, `active`, `paused`, `completed`, `failed`,
  `conflict`, `cancelled`. Branch on this.
- `phase`: the exact step: `queued`, `evaluating`, `resolving_conflicts`,
  `downloading`, `reconnecting`, `assembling`, `flushing`, `verifying`,
  `paused`, `conflict`, `completed`, `failed`, `cancelled`.
- `path`: the saved file, or where it will be written once the name is
  known (`null` until then).
- `resumable`: `null` until the server has been asked.
- `error`: only for `failed` and `conflict`.
- Headers, cookies and credentials are never printed.

### `probe`

```json
{"type":"probe","url":"https://example.com/a.bin","filename":"a.bin","size":1073741824,
 "resumable":true,"etag":null,"last_modified":null,"requires_auth":false,
 "checksums":[{"algorithm":"md5","digest":"cd573cfaace07e7949bc0c46028904ff"}]}
```

The probe is made by oxdm with its own settings (proxy, user agent) and
without any headers you would pass to `add`, so a link behind a login
reports `requires_auth: true`.

## Error objects

In `failed` and `rejected` lines:

```json
{"kind": "network", "message": "HTTP 404 Not Found", "retryable": false}
```

`retryable: true` means trying again unchanged (`oxdm resume`) can be
expected to help: connection errors, timeouts, HTTP 408, 429 and 5xx.

On stderr, once, when the exit code is not 0:

```json
{"type": "error", "kind": "timeout", "message": "...", "exit_code": 124}
```

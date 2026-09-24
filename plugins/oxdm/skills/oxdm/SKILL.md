---
name: oxdm
description: Download files through the user's oxdm download manager, so every download shows in their oxdm window and can be resumed there or from the command line after an error. Uses the `oxdm` CLI with machine-readable NDJSON output and typed exit codes. Use when the user has oxdm installed and asks to download a file or URL, fetch a list of links, check on or resume a download, or retry one that failed. Trigger on "download", "fetch this URL", "grab this file", "download these links", "resume the download", "retry the download", "what is downloading", "use oxdm".
---

# oxdm: agent download interface

oxdm is a desktop download manager. Its `oxdm` command hands downloads to
the running oxdm (starting it in the tray if needed), so the user sees
everything you download in their own window, and a download that fails
can be resumed from where it stopped instead of starting over.

Check the CLI exists with `oxdm help` (it lists `add`, `wait`, `list`).
The full contract is under `oxdm help`; `reference.md` mirrors it.

## Golden rule

**Always pass `--format json`.** You get one JSON object per line on
stdout and, on failure, one JSON error object on stderr. Text mode is for
people.

## The loop

```bash
# 1. Add and follow. Stay under your shell tool's time limit.
oxdm --format json add "https://example.com/file.zip" -o ./downloads/ --wait --timeout 5m

# 2. Branch on the exit code:
#    0    done; the `completed` line has the real `path`
#    124  still downloading; keep following it (it did not stop)
oxdm --format json wait <ID> --timeout 5m
#    3    network; if the error says "retryable": true, resume from where it stopped
oxdm --format json resume <ID> --wait --timeout 5m
```

Re-running the exact same `add` command is also safe: a URL already in
the list is reported (`"existing": true`), resumed if it had stopped, and
left alone if it already finished. It is never downloaded twice unless
you pass `--new`.

## Core commands

```bash
oxdm --format json add URL... [-o PATH] [--wait [--timeout DUR]]  # add + start
oxdm --format json add -i urls.txt -o ./downloads/                  # a list, one URL per line
oxdm --format json wait ID...  [--timeout DUR]                      # follow to the end
oxdm --format json list [TEXT] [--state failed] [--category agent]  # what is there
oxdm --format json status ID...                                     # one or more, in detail
oxdm --format json resume ID... | --failed [--wait]                 # continue after an error
oxdm --format json pause ID...
oxdm --format json restart ID... [--delete-file] [--wait]           # discard and start over
oxdm --format json remove ID... [--delete-file]                     # take it off the list
oxdm --format json probe URL                                        # size, name, resumable
oxdm --format json queues
```

`ID` is the `id` from any output line, or a unique prefix of it (at least
4 characters; text output shows the first 8).

## Exit codes

| code | meaning | what to do |
|------|---------|------------|
| 0 | success | read `path` from the `completed` line |
| 2 | usage: bad flag, URL, id or queue | fix the command |
| 3 | network | if `retryable`, `oxdm resume ID`; else the link is wrong or gone (404, 403) |
| 4 | conflict: checksum mismatch, file changed on the server | ask the user; `restart` discards what was downloaded |
| 5 | I/O: disk full, not writable | tell the user; free space or pick another `-o` |
| 6 | oxdm not running and could not be started | ask the user to start oxdm, then retry |
| 7 | stopped: paused, cancelled or removed by someone else | the user (or oxdm's metered/battery guard) stopped it; ask before resuming |
| 124 | `--timeout` ran out | the download continues; `oxdm wait ID` again |
| 130 | interrupted | the download continues; `oxdm wait ID` again |
| 1 | other | read the message |

## Load-bearing gotchas

- **Downloads outlive your command.** They belong to oxdm. A timeout,
  Ctrl-C or a killed shell ends the waiting, never the download. Do not
  re-add to "restart" a wait; use `oxdm wait ID`.
- **Always bound waits** with `--timeout` shorter than your tool's own
  time limit, then loop on exit 124. A large file can take hours.
- **Read the path, do not guess it.** Without `-o`, a download saves to
  the user's Agent folder (e.g. `~/Downloads/Agent`), or to a folder by
  file type if they deleted the Agent category. A name already used by
  another download gets a numbered one (`file_1.zip`). The `completed`
  line's `path` is where the file is.
- **`-o` ending in `/`, or naming an existing folder, is a folder**;
  otherwise, with one URL, it is the file path. Relative paths are
  resolved against your current directory.
- **Agent category and queue.** New downloads are filed under an
  "Agent" category and an "Agent" queue, both created by your first
  download, so the user sees at a glance everything you fetched,
  finished or not. If they delete either, it stays deleted; that is
  their choice, not an error. Never run `oxdm restore-agent` (which
  recreates them) unless the user asks you to. `--queue NAME` picks
  another existing queue; `oxdm list --category agent` lists what you
  downloaded.
- **Exit 7 means a person decided.** Do not resume in a loop what the user
  paused. Exit 4 is also theirs to decide: `restart` throws away the
  downloaded data, and `restart --delete-file` moves the old file to the
  trash.
- **Secrets never on the command line.** Pipe headers in: `-H @-` reads
  `Name: Value` lines from stdin. `Cookie` and `Authorization` (Basic,
  Bearer) are stored encrypted by oxdm.
- **Quiet by design.** Downloads you start do not pop up windows or
  notifications; they appear in the user's list. `--show` opens a
  progress window when the user asks to watch one.
- **No display, no start.** On Linux, from a shell without `DISPLAY` or
  `WAYLAND_DISPLAY`, oxdm will not be started for you (exit 6), because
  the user could never see it. Ask them to start oxdm.

## More detail

- `reference.md`: every command and flag, every JSON line and field,
  the error object.
- `examples.md`: copy-paste recipes: follow a download, batch, resume
  after a failure, secrets, checksums, a retry loop.

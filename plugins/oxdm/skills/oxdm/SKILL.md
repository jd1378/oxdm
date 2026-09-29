---
name: oxdm
description: Download files through the user's oxdm download manager, so each download shows in their oxdm window and can be resumed after an error. Uses the `oxdm` CLI (NDJSON output, typed exit codes). Use when the user has oxdm and asks to download or fetch a file, URL or list of links, check what is downloading, resume or retry a download, or says "use oxdm".
---

# oxdm: agent download interface

`oxdm` hands downloads to the user's running oxdm (starting it in the
tray if needed), so they see everything you download in their window,
and a failed download resumes from where it stopped. Full contract:
`oxdm help` and `oxdm <command> --help`.

**Always pass `--format json`** (omitted from the command list below).
stdout then carries one JSON object per line and, on failure, stderr
carries one JSON error object.

## The loop

```bash
oxdm --format json add "https://example.com/file.zip" -o ./downloads/ --wait --timeout 5m
```

Then branch on the exit code:

| code | meaning | what to do |
|------|---------|------------|
| 0 | done | read `path` from the `completed` line |
| 124 | `--timeout` ran out; the download continues | `oxdm --format json wait ID --timeout 5m` |
| 130 | interrupted; the download continues | `wait ID` again |
| 3 | network | error says `"retryable": true`: `resume ID --wait --timeout 5m`; otherwise the link is wrong or gone (404, 403) |
| 2 | usage: bad flag, URL, id or queue | fix the command |
| 4 | conflict: checksum mismatch, file changed on the server | ask the user; `restart` discards the downloaded data, `--delete-file` also trashes the file |
| 5 | I/O: disk full, not writable | tell the user; free space or pick another `-o` |
| 6 | oxdm not running and could not be started | ask the user to start oxdm, then retry. On Linux without `DISPLAY` or `WAYLAND_DISPLAY` it is never started for you |
| 7 | paused, cancelled or removed by the user (or oxdm's metered/battery guard) | a person decided: ask before resuming, never resume in a loop |
| 1 | other | read the message |

## Commands

```bash
oxdm add URL... [-o PATH] [--wait [--timeout DUR]]
oxdm add -i urls.txt -o ./downloads/            # one URL per line
oxdm wait ID... [--timeout DUR]
oxdm list [TEXT] [--state failed] [--category agent]
oxdm status ID...
oxdm resume ID... | --failed [--wait]
oxdm pause ID...
oxdm restart ID... [--delete-file] [--wait]     # discard and start over
oxdm remove ID... [--delete-file]
oxdm probe URL                                  # size, name, resumable
oxdm queues
```

`ID` is the `id` from any output line, or a unique prefix of at least 4
characters.

## Rules

- **Downloads outlive your command.** A timeout, Ctrl-C or killed shell
  ends the waiting, never the download. Bound every wait with a
  `--timeout` shorter than your tool's time limit and loop on exit 124;
  never re-add to restart a wait.
- **Re-running `add` is safe.** A URL already in the list is reported
  (`"existing": true`), resumed if it had stopped, left alone if it
  finished. Only `--new` downloads it twice.
- **Read `path`, do not build it.** Without `-o`, files go to the
  user's Agent folder (e.g. `~/Downloads/Agent`), and a name already
  taken gets a number (`file_1.zip`). `-o` ending in `/` or naming an
  existing folder is a folder; otherwise, with one URL, the file path.
- **The Agent category and queue are the user's.** Your downloads are
  filed under both (`list --category agent`). If the user deletes
  either, it stays deleted: never run `oxdm restore-agent` unless asked.
- **Secrets never on the command line.** `-H @-` reads `Name: Value`
  lines from stdin; `Cookie` and `Authorization` are stored encrypted.
- **Checksums:** `--checksum ALGO:DIGEST`, with a single URL. Re-running
  `add` with one attaches it to the existing download; a finished file
  is hashed right away (exit 4 on a mismatch).
- **Quiet by design.** No windows or notifications; `--show` opens a
  progress window when the user wants to watch.
- **Remove only what you downloaded**, and only when the user asks.

## If `oxdm` is not found

Try the installers' defaults: `~/.local/bin/oxdm` (Linux, macOS) or
`%LOCALAPPDATA%\Programs\oxdm\oxdm.exe` (Windows). If one runs, use
that full path from then on and tell the user once that its folder is
not on their `PATH` (Linux, macOS: add `export PATH="$HOME/.local/bin:$PATH"`
to the shell profile; Windows: a new terminal picks it up). If none
runs, ask the user where oxdm is or point them to
https://github.com/jd1378/oxdm#install. Do not install it yourself.

## More detail

- `reference.md`: every flag, JSON line and field, reuse rules, the
  error object.
- `examples.md`: batch, follow later, authenticated download,
  checksum, a bash retry loop.

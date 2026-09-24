<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/oxdm_about_light.png">
  <img src="assets/oxdm_about_dark.png" alt="oxdm" width="112">
</picture>

<h1>oxdm</h1>

<p>Download manager for Linux, macOS and Windows.</p>

<p>
  <a href="https://github.com/jd1378/oxdm/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/jd1378/oxdm?style=flat-square&color=e07a5f"></a>
  <a href="https://github.com/jd1378/oxdm/actions/workflows/ci.yml"><img alt="CI" src="https://img.shields.io/github/actions/workflow/status/jd1378/oxdm/ci.yml?branch=main&style=flat-square&label=CI"></a>
  <a href="LICENSE"><img alt="License: AGPL-3.0" src="https://img.shields.io/badge/license-AGPL--3.0-blue?style=flat-square"></a>
  <img alt="Platforms" src="https://img.shields.io/badge/platforms-Linux%20%7C%20macOS%20%7C%20Windows-555?style=flat-square">
  <a href="https://github.com/sponsors/jd1378"><img alt="Sponsor" src="https://img.shields.io/badge/sponsor-%E2%99%A5-e07a5f?style=flat-square"></a>
</p>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/oxdm_screenshot_dark.png">
  <img src="assets/oxdm_screenshot_light.png" alt="The oxdm main window" width="900">
</picture>

</div>

## Features

- **Lightweight UI.** Drawn in software: no GPU, no toolkit. Every window is its own process and opens in about 90 ms.
- **Scheduled queues.** Run on a timetable, or only under conditions you pick: unmetered connection, AC power, an idle machine. When a run ends it can notify you, run a command, or shut the machine down.
- **Segmented downloads.** A file is split across connections.
- **Browser capture.** Downloads arrive from the extension with their cookies, headers and referrer, over WebSocket or native messaging.
- **Resilient.** Interrupted parts resume, and failures retry on a fixed-then-exponential backoff that you can configure.
- **Per-job settings.** Proxy, credentials, headers, cookies and checksum, with speed limits set globally or per job.
- **Easy updates.** It tells you a release is out; you download and install it from About.
- **Scriptable.** `oxdm add`, `oxdm wait`, `oxdm resume` and friends drive the running app from a terminal, a script or an AI agent, with JSON output and typed exit codes.

## Install

### Linux / macOS

```bash
curl -fsSL https://raw.githubusercontent.com/jd1378/oxdm/main/tools/install.sh | sh
```

Downloads the release archive for your platform, checks it against the
published SHA-256, installs `oxdm` and `oxdm-native-host` into
`~/.local/bin`, and adds a launcher entry.

Custom directory:

```bash
curl -fsSL https://raw.githubusercontent.com/jd1378/oxdm/main/tools/install.sh | sh -s -- --dir /usr/local/bin
```

### Windows

```powershell
irm https://raw.githubusercontent.com/jd1378/oxdm/main/tools/install.ps1 | iex
```

Installs to `%LOCALAPPDATA%\Programs\oxdm`, adds it to your user PATH, drops a Start-menu shortcut.

### Build from source

```bash
git clone https://github.com/jd1378/oxdm
cd oxdm
cargo build --release --bins
```

No system dev libraries are needed beyond a C toolchain. Everything
that talks to the desktop (tray, notifications, keyring, network and
power state) goes over D-Bus in pure Rust, so there is nothing to
install for it.

Outputs:

- `target/release/oxdm`: main app
- `target/release/oxdm-native-host`: browser native-messaging bridge

### Packaging oxdm

If you are building oxdm for a distro repository, a Flatpak, or any
other channel where a package manager owns the installed files, build
it with:

```bash
OXDM_NO_SELF_UPDATE=1 cargo build --release --bins
```

That build never checks for a new version, never downloads one, and
offers no way to install one: About shows the version and nothing else,
and the update rows disappear from Settings → General and Settings →
Notifications. The daemon refuses update requests over its own IPC
socket too, so nothing left over from a previous install can start a
self-install behind the package manager's back.

Settings → Advanced drops its "launcher entry" button for the same
reason: the `.desktop` file and its icon are yours to ship, and a copy
written to `~/.local/share/applications` would shadow the packaged one.

Leave it unset for anything users install themselves — the tarball, the
install scripts, a local build.

For development there is also `cargo run -p oxdm-testserver`, a local
server whose endpoints each misbehave in one specific way (no ranges,
unknown length, wrong checksums, ranges advertised but ignored). Its
index page at `http://127.0.0.1:8088/` lists them. It is a separate
workspace member, so release builds do not include it.

## Uninstall

Linux / macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/jd1378/oxdm/main/tools/uninstall.sh | sh
# also wipe settings + queue DB
curl -fsSL https://raw.githubusercontent.com/jd1378/oxdm/main/tools/uninstall.sh | sh -s -- --purge
```

Windows:

```powershell
irm https://raw.githubusercontent.com/jd1378/oxdm/main/tools/uninstall.ps1 | iex
# also wipe settings + queue DB
$env:OXDM_PURGE = "1"; irm https://raw.githubusercontent.com/jd1378/oxdm/main/tools/uninstall.ps1 | iex
```

## Browser extension

The companion extension is published for both browsers:

- [Chrome Web Store](https://chromewebstore.google.com/detail/oxdm-download-manager-int/bfefefnlghppdcgjjimkllklpifkcokj)
- [Firefox Add-ons](https://addons.mozilla.org/addon/oxdm-download-manager-bridge/)

*Tools → Browser extension* links to the same pages.

The host side is a stable contract (see [`docs/EXTENSION_API.md`](docs/EXTENSION_API.md)), so any extension can use it. Both transports are supported:

- **WebSocket** at `ws://127.0.0.1:<port>`, simplest for development.
- **Native messaging** via the `oxdm-native-host` shim plus a per-OS manifest.
  oxdm registers the manifest itself: on first run, again whenever it
  finds one missing or stale, and on demand from *Settings → Browser*.
  `oxdm --install-native-host [--chromium-id ID]` does the same from a
  terminal.

The pairing code the extension asks for lives in *Settings → Browser*, with Copy and Regenerate buttons. It bundles the port and the auth token in one string.

## Command line and AI agents

The `oxdm` binary also drives the running app. Whatever a script or an AI
agent downloads this way lands in your list, where you can watch it,
pause it, or resume it after an error. If oxdm is not running, the
command starts it in the tray.

The first such download creates an **Agent** category (saving into
`Agent` inside your download folder) and an **Agent** queue, so what
your agents fetched is one click away in the sidebar, finished or not.
Change the category's folder or queue in *Settings → Categories*. Delete
either and it stays deleted, even through *Reset Categories*: downloads
are then filed by type, or go to Main. `oxdm restore-agent` brings them
back.

```bash
oxdm add https://example.com/big.iso --wait      # add, start, follow to the end
oxdm list --state failed                         # what went wrong
oxdm resume --failed --wait                      # pick them up where they stopped
oxdm help                                        # every command, exit codes, JSON contract
```

Add `--format json` for machine-readable output: one JSON object per line
on stdout, and on failure one error object on stderr with a stable exit
code (`3` network, `4` needs a decision, `5` disk, `7` stopped by you,
`124` timed out; the download itself carries on). Re-running the same
`oxdm add` does not download twice: it resumes the existing download or
reports it as already complete.

### Agent skill

This repo ships an [Agent Skill](plugins/oxdm/skills/oxdm/) (the open
`SKILL.md` standard) that teaches AI agents to download through oxdm and
to handle its exit codes.

Linux / macOS:

```bash
# Interactive: asks which agent (claude/codex/...) and global vs. project
curl -fsSL https://raw.githubusercontent.com/jd1378/oxdm/main/tools/install-skill.sh | sh

# Non-interactive
curl -fsSL https://raw.githubusercontent.com/jd1378/oxdm/main/tools/install-skill.sh | sh -s -- codex --project
```

`tools/install-skill.sh --help` covers other agents, `--dir`, and
flattening into an `AGENTS.md`.

Windows (PowerShell):

```powershell
# Claude Code, all projects. Other places: $HOME\.codex\skills\oxdm for
# Codex, or .claude\skills\oxdm (.codex\skills\oxdm) for this project only.
$d = "$HOME\.claude\skills\oxdm"; New-Item -ItemType Directory -Force $d | Out-Null
"SKILL.md", "reference.md", "examples.md" | % { irm "https://raw.githubusercontent.com/jd1378/oxdm/main/plugins/oxdm/skills/oxdm/$_" -OutFile "$d\$_" }
```

Claude Code users can install it as a plugin instead, which keeps it
updated:

```text
/plugin marketplace add jd1378/oxdm
/plugin install oxdm@oxdm
```

## Configuration

Settings + queue persist in a SQLite DB:

| OS      | path                                            |
|---------|-------------------------------------------------|
| Linux   | `~/.local/share/oxdm/db/oxdm.db`                |
| macOS   | `~/Library/Application Support/oxdm/db/oxdm.db` |
| Windows | `%APPDATA%\oxdm\db\oxdm.db`                     |

It has a directory to itself so a sandboxed browser can be granted
read access to the database without also being given the working and
update directories beside it. A database from before that change is
moved into `db/` the next time the daemon starts.

## License

Copyright (C) 2026 jd1378

GNU Affero General Public License v3.0. See [LICENSE](LICENSE).

This program is free software: you can redistribute it and/or modify it
under the terms of version 3 of the GNU Affero General Public License as
published by the Free Software Foundation. Later versions of that
license do not apply. It is distributed WITHOUT ANY WARRANTY; without
even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR
PURPOSE. See the GNU Affero General Public License for more details.

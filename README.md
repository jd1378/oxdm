# oxdm

Cross-platform download manager built on the [`odl`](https://crates.io/crates/odl) crate, with an [iced](https://iced.rs) desktop UI (software-rendered, no GPU required) and a pluggable browser-extension bridge.

## Install

### Linux / macOS

```bash
curl -fsSL https://raw.githubusercontent.com/jd1378/oxdm/main/tools/install.sh | sh
```

Downloads the release archive for your platform, checks it against the
published SHA-256, installs `oxdm`, `oxdm-native-host` and
`oxdm-updater` into `~/.local/bin`, and adds a launcher entry.

Custom directory:

```bash
curl -fsSL https://raw.githubusercontent.com/jd1378/oxdm/main/tools/install.sh | sh -s -- --dir /usr/local/bin
```

The UI is rendered in software and links no toolkit: on Linux the only
runtime libraries beyond libc are D-Bus and systemd's, both of which a
desktop session already has. The tray and desktop notifications use
D-Bus; without it oxdm still runs, minus those two.

### Windows

```powershell
irm https://raw.githubusercontent.com/jd1378/oxdm/main/tools/install.ps1 | iex
```

Installs to `%LOCALAPPDATA%\Programs\oxdm`, adds it to your user PATH, drops a Start-menu shortcut.

### AppImage (Linux)

Desktop Linux releases also carry `oxdm-<tag>-<arch>.AppImage` for
people who prefer a bundle. Download it from the
[releases page](https://github.com/jd1378/oxdm/releases), `chmod +x`
it, and run it. There is nothing to install and the installer script
does not use it.

Either way oxdm updates itself in place: it notices at run time whether
it was launched from a bundle and fetches the matching artifact, so an
AppImage stays an AppImage and an installed build stays installed
binaries.

### Build from source

```bash
git clone https://github.com/jd1378/oxdm
cd oxdm
cargo build --release --bins
```

No system dev libraries are needed beyond a C toolchain and D-Bus
headers (`dbus-devel` on Fedora, `libdbus-1-dev` on Debian/Ubuntu).

Outputs:

- `target/release/oxdm`: main app
- `target/release/oxdm-native-host`: browser native-messaging bridge
- `target/release/oxdm-updater`: verifies and installs a self-update

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

oxdm exposes a stable host-side contract (see [`docs/EXTENSION_API.md`](docs/EXTENSION_API.md)) but does not ship an extension itself. Both transports are supported:

- **WebSocket** at `ws://127.0.0.1:<port>`, simplest for development.
- **Native messaging** via the `oxdm-native-host` shim plus a per-OS manifest.

The auth token lives in *Settings → Browser integration*, with a Regenerate button.

## Configuration

Settings + queue persist in a SQLite DB:

| OS      | path                                                   |
|---------|--------------------------------------------------------|
| Linux   | `~/.config/oxdm/oxdm.db`                               |
| macOS   | `~/Library/Application Support/oxdm/oxdm.db`           |
| Windows | `%APPDATA%\oxdm\oxdm.db`                               |

Every `odl::config::Config` field is editable from Settings, plus oxdm-only knobs (theme, IPC port, conflict-while-hidden behavior, remove-confirm prompts).

## Architecture

Four-layer clean architecture (`domain` → `data` → `ipc_local` → `gui`):
`domain` is pure, `odl` types never leak past `data`, and the GUI
windows are separate processes that talk to the daemon over a local
socket. Pause/cancel and the update channel sit behind traits, so
swapping either does not touch the UI.

## License

AGPL-3.0

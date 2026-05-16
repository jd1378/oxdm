# oxdm

Cross-platform download manager built on the [`odl`](https://crates.io/crates/odl) crate, with a Dioxus desktop UI and a pluggable browser-extension bridge.

## Install

### Linux / macOS

```bash
curl -fsSL https://raw.githubusercontent.com/jd1378/oxdm/main/tools/install.sh | sh
```

Custom directory:

```bash
curl -fsSL https://raw.githubusercontent.com/jd1378/oxdm/main/tools/install.sh | sh -s -- --dir /usr/local/bin
```

Linux runtime libraries the script will warn about if missing:

| distro                  | command                                                                       |
|-------------------------|-------------------------------------------------------------------------------|
| Debian / Ubuntu         | `sudo apt install libwebkit2gtk-4.1-0 libsoup-3.0-0 libgtk-3-0 libxdo3`       |
| Fedora / RHEL           | `sudo dnf install webkit2gtk4.1 libsoup3 gtk3 libxdo`                         |
| Arch                    | `sudo pacman -S webkit2gtk-4.1 libsoup3 gtk3 xdotool`                         |

### Windows

```powershell
irm https://raw.githubusercontent.com/jd1378/oxdm/main/tools/install.ps1 | iex
```

Installs to `%LOCALAPPDATA%\Programs\oxdm`, adds it to your user PATH, drops a Start-menu shortcut. Requires the [Edge WebView2 runtime](https://developer.microsoft.com/microsoft-edge/webview2/) (already present on Windows 11 and most Windows 10 installs).

### Build from source

```bash
git clone https://github.com/jd1378/oxdm
cd oxdm
cargo build --release --bins
```

System dev libs needed on Linux: `webkit2gtk4.1-devel`, `libsoup3-devel`, `gtk3-devel`, `libxdo-devel` (Fedora) — equivalent `-dev` packages on Debian/Ubuntu/Arch.

Outputs:

- `target/release/oxdm` — main app
- `target/release/oxdm-native-host` — browser native-messaging bridge

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

oxdm exposes a stable host-side contract — see [`docs/EXTENSION_API.md`](docs/EXTENSION_API.md) — but does not ship an extension itself. Both transports are supported:

- **WebSocket** at `ws://127.0.0.1:<port>` — simplest for development.
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

See [`PLAN.md`](PLAN.md) for the full design. TL;DR — four-layer clean architecture (`domain` → `data` → `ipc` / `app`), `odl` types never leak past the `data` layer, pause/cancel/update channel are all behind traits so future swaps don't touch the UI.

## License

AGPL-3.0

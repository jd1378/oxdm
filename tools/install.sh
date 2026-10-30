#!/bin/sh
# oxdm installer — Linux / macOS.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/jd1378/oxdm/main/tools/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/jd1378/oxdm/main/tools/install.sh | sh -s -- --dir /usr/local/bin
#
# Env:
#   OXDM_INSTALL_DIR     directory for the `oxdm` command (default: $HOME/.local/bin)
#   OXDM_APP_DIR         where oxdm.app goes on macOS (default: $HOME/Applications)
#   OXDM_VERSION         tag to install (default: latest)
#   OXDM_NO_DESKTOP      set to skip writing the .desktop entry (Linux)
#   OXDM_NO_MODIFY_PATH  set to leave shell startup files alone
#   NO_COLOR             disable color output

set -eu

REPO="jd1378/oxdm"
INSTALL_DIR="${OXDM_INSTALL_DIR:-$HOME/.local/bin}"
APP_DIR="${OXDM_APP_DIR:-$HOME/Applications}"
VERSION="${OXDM_VERSION:-latest}"

while [ $# -gt 0 ]; do
  case "$1" in
    --dir) INSTALL_DIR="$2"; shift 2 ;;
    --app-dir) APP_DIR="$2"; shift 2 ;;
    --version) VERSION="$2"; shift 2 ;;
    -h|--help)
      cat <<EOF
oxdm installer
  --dir <path>      directory for the oxdm command (default: \$HOME/.local/bin)
  --app-dir <path>  macOS: where oxdm.app goes (default: \$HOME/Applications)
  --version <tag>   release tag (default: latest)
EOF
      exit 0
      ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
  C_BOLD=$(printf '\033[1m')
  C_DIM=$(printf '\033[2m')
  C_CYAN=$(printf '\033[36m')
  C_GREEN=$(printf '\033[32m')
  C_YEL=$(printf '\033[33m')
  C_RED=$(printf '\033[31m')
  C_RESET=$(printf '\033[0m')
else
  C_BOLD= C_DIM= C_CYAN= C_GREEN= C_YEL= C_RED= C_RESET=
fi
step() { printf '%s==>%s %s%s%s\n' "$C_CYAN$C_BOLD" "$C_RESET" "$C_BOLD" "$*" "$C_RESET"; }
info() { printf '    %s%s%s\n' "$C_DIM" "$*" "$C_RESET"; }
ok()   { printf '%s✓%s %s\n' "$C_GREEN" "$C_RESET" "$*"; }
warn() { printf '%s!%s %s\n' "$C_YEL" "$C_RESET" "$*"; }
err()  { printf '%serror:%s %s\n' "$C_RED$C_BOLD" "$C_RESET" "$*" >&2; exit 1; }

if command -v curl >/dev/null 2>&1; then HAVE=curl
elif command -v wget >/dev/null 2>&1; then HAVE=wget
else err "need curl or wget"; fi

fetch() {
  case "$HAVE" in
    curl) curl -fsSL "$1" ;;
    wget) wget -qO- "$1" ;;
  esac
}
download() {
  case "$HAVE" in
    curl)
      if [ -t 2 ]; then curl -fL --progress-bar -o "$2" "$1"
      else curl -fsSL -o "$2" "$1"; fi ;;
    wget)
      if [ -t 2 ]; then wget --show-progress -q -O "$2" "$1"
      else wget -q -O "$2" "$1"; fi ;;
  esac
}

step "Detecting platform"
OS="$(uname -s)"
ARCH="$(uname -m)"
case "$OS" in
  Linux)
    case "$ARCH" in
      x86_64|amd64) TARGET="x86_64-unknown-linux-gnu" ;;
      aarch64|arm64) TARGET="aarch64-unknown-linux-gnu" ;;
      *) err "unsupported linux arch: $ARCH" ;;
    esac
    ASSET_EXT="tar.gz"
    ;;
  Darwin)
    case "$ARCH" in
      x86_64) TARGET="x86_64-apple-darwin" ;;
      arm64|aarch64) TARGET="aarch64-apple-darwin" ;;
      *) err "unsupported macos arch: $ARCH" ;;
    esac
    ASSET_EXT="tar.gz"
    ;;
  *) err "unsupported OS: $OS (use install.ps1 on windows)" ;;
esac
info "$OS / $ARCH → $TARGET"

# No runtime dependency check: there is nothing to check for. The UI is
# software-rendered and links no toolkit, and everything that talks to
# the desktop — tray, notifications, keyring, network and power state —
# speaks D-Bus through zbus in pure Rust, so `libdbus` is never loaded.
# This used to warn when `libdbus-1.so.3` was missing, which named a
# package the user does not need and a failure that does not follow.

step "Resolving release"
if [ "$VERSION" = latest ]; then
  REL_JSON="$(fetch "https://api.github.com/repos/$REPO/releases/latest")" \
    || err "failed to query latest release"
  TAG="$(printf '%s\n' "$REL_JSON" \
    | grep '"tag_name"' \
    | head -n1 \
    | sed -E 's/.*"tag_name"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/')"
  [ -n "$TAG" ] || err "could not resolve latest tag"
  info "latest = $TAG"
else
  TAG="$VERSION"
  info "pinned = $TAG"
fi

ASSET="oxdm-${TAG}-${TARGET}.${ASSET_EXT}"
URL="https://github.com/$REPO/releases/download/${TAG}/${ASSET}"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

step "Downloading $ASSET"
info "$URL"
download "$URL" "$TMP/$ASSET" || err "download failed: $URL"

# Every asset is published with its digest beside it. Checking it costs
# one small request and is the difference between "downloaded from
# GitHub" and "downloaded what the release actually built".
step "Verifying"
if fetch "${URL}.sha256" > "$TMP/$ASSET.sha256" 2>/dev/null && [ -s "$TMP/$ASSET.sha256" ]; then
  WANT="$(cut -d' ' -f1 < "$TMP/$ASSET.sha256")"
  if command -v sha256sum >/dev/null 2>&1; then
    GOT="$(sha256sum "$TMP/$ASSET" | cut -d' ' -f1)"
  elif command -v shasum >/dev/null 2>&1; then
    GOT="$(shasum -a 256 "$TMP/$ASSET" | cut -d' ' -f1)"
  else
    GOT=""
    warn "no sha256 tool found — skipping verification"
  fi
  if [ -n "$GOT" ]; then
    [ "$WANT" = "$GOT" ] || err "checksum mismatch: expected $WANT, got $GOT"
    ok "sha256 matches"
  fi
else
  warn "no published checksum for $ASSET — skipping verification"
fi

step "Extracting"
tar -xzf "$TMP/$ASSET" -C "$TMP"

OXDM_BIN="$(find "$TMP" -type f -name oxdm | head -n1)"
HOST_BIN="$(find "$TMP" -type f -name oxdm-native-host | head -n1)"
[ -n "$OXDM_BIN" ] || err "binary 'oxdm' not found in archive"

# A string as one single-quoted shell word.
sh_quote() { printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"; }

if [ "$OS" = "Darwin" ]; then
  # Finder, Launchpad and Spotlight list application bundles, never a
  # bare program, so the programs live inside oxdm.app and the `oxdm`
  # command execs into it. A symlink would not do: oxdm looks for its
  # browser host and installs updates beside the path it was started
  # from, which would then be the link's directory.
  APP="$APP_DIR/oxdm.app"
  step "Installing $APP"
  STAGE="$APP_DIR/.oxdm.app.new"
  rm -rf "$STAGE"
  mkdir -p "$STAGE/Contents/MacOS" "$STAGE/Contents/Resources"
  install -m 0755 "$OXDM_BIN" "$STAGE/Contents/MacOS/oxdm"
  if [ -n "$HOST_BIN" ]; then
    install -m 0755 "$HOST_BIN" "$STAGE/Contents/MacOS/oxdm-native-host"
  else
    warn "'oxdm-native-host' is not in this archive — browser integration will be unavailable."
  fi
  ICON_SRC="$(find "$TMP" -type f -name 'oxdm.png' | head -n1)"
  if [ -n "$ICON_SRC" ]; then
    sips -s format icns "$ICON_SRC" --out "$STAGE/Contents/Resources/oxdm.icns" >/dev/null 2>&1 \
      || warn "could not convert the icon; oxdm.app will show a generic one"
  fi
  # No version keys: oxdm updates its programs in place, and a version
  # written here would go stale at the first update.
  cat > "$STAGE/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key><string>oxdm</string>
    <key>CFBundleIdentifier</key><string>io.github.jd1378.oxdm</string>
    <key>CFBundleName</key><string>oxdm</string>
    <key>CFBundleDisplayName</key><string>oxdm</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleIconFile</key><string>oxdm</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
  rm -rf "$APP"
  mv "$STAGE" "$APP"
  # Spotlight finds it on its own, eventually; registering makes it
  # show up in Launchpad and the Applications list now.
  LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
  if [ -x "$LSREGISTER" ]; then "$LSREGISTER" -f "$APP" >/dev/null 2>&1 || true; fi
  ok "installed: $APP"

  step "Installing the oxdm command to $INSTALL_DIR"
  mkdir -p "$INSTALL_DIR"
  printf '#!/bin/sh\nexec %s "$@"\n' "$(sh_quote "$APP/Contents/MacOS/oxdm")" > "$INSTALL_DIR/.oxdm.new"
  chmod 0755 "$INSTALL_DIR/.oxdm.new"
  mv -f "$INSTALL_DIR/.oxdm.new" "$INSTALL_DIR/oxdm"
  ok "installed: $INSTALL_DIR/oxdm"
  # Where an earlier installer put it. The browsers are pointed at the
  # copy inside oxdm.app the next time oxdm starts.
  if [ -f "$INSTALL_DIR/oxdm-native-host" ]; then
    rm -f "$INSTALL_DIR/oxdm-native-host"
    ok "removed the previous $INSTALL_DIR/oxdm-native-host"
  fi
else
  step "Installing to $INSTALL_DIR"
  mkdir -p "$INSTALL_DIR"
  install -m 0755 "$OXDM_BIN" "$INSTALL_DIR/oxdm"
  ok "installed: $INSTALL_DIR/oxdm"
  # Not fatal when an archive lacks it: oxdm runs without the browser
  # bridge, minus that integration.
  if [ -n "$HOST_BIN" ]; then
    install -m 0755 "$HOST_BIN" "$INSTALL_DIR/oxdm-native-host"
    ok "installed: $INSTALL_DIR/oxdm-native-host"
  else
    warn "'oxdm-native-host' is not in this archive — browser integration will be unavailable."
  fi
fi

# Linux .desktop entry so the app appears in launchers.
if [ "$OS" = "Linux" ] && [ -z "${OXDM_NO_DESKTOP:-}" ]; then
  APPS="$HOME/.local/share/applications"
  mkdir -p "$APPS"

  # oxdm's own icon when the archive carries one; the generic
  # download arrow from the theme when it does not.
  ICON="folder-download"
  ICON_SRC="$(find "$TMP" -type f -name 'oxdm.png' | head -n1)"
  if [ -n "$ICON_SRC" ]; then
    ICON_DIR="$HOME/.local/share/icons/hicolor/512x512/apps"
    mkdir -p "$ICON_DIR"
    install -m 0644 "$ICON_SRC" "$ICON_DIR/oxdm.png"
    ICON="oxdm"
    # A stale icon-theme.cache in the theme root hides every icon added
    # after it was built: readers compare its mtime against the theme
    # directory, not against the size/context directory the file landed
    # in, so writing the PNG alone leaves the cache looking current.
    # Only GTK keeps that cache, and only GTK desktops ship the tool to
    # rebuild it, so its absence is not worth reporting.
    command -v gtk-update-icon-cache >/dev/null 2>&1 \
      && gtk-update-icon-cache -q -t -f "$HOME/.local/share/icons/hicolor" >/dev/null 2>&1 || true
    ok "icon: $ICON_DIR/oxdm.png"
  fi

  cat > "$APPS/oxdm.desktop" <<EOF
[Desktop Entry]
Name=oxdm
Comment=Cross-platform download manager
Exec=$INSTALL_DIR/oxdm
Icon=$ICON
Terminal=false
Type=Application
Categories=Network;FileTransfer;
StartupWMClass=oxdm
EOF
  command -v update-desktop-database >/dev/null 2>&1 \
    && update-desktop-database "$APPS" >/dev/null 2>&1 || true
  ok "desktop entry: $APPS/oxdm.desktop"
fi

# macOS puts no per-user directory on PATH, and not every Linux
# session does either, so without this `oxdm` is "command not found".
# Appended once to the startup file of the user's shell.
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    case "$(basename "${SHELL:-sh}")" in
      zsh)  RC="${ZDOTDIR:-$HOME}/.zshrc" ;;
      # Terminal.app starts bash as a login shell, which skips .bashrc.
      bash) if [ "$OS" = "Darwin" ]; then RC="$HOME/.bash_profile"; else RC="$HOME/.bashrc"; fi ;;
      fish) RC="${XDG_CONFIG_HOME:-$HOME/.config}/fish/conf.d/oxdm.fish" ;;
      *)    RC="$HOME/.profile" ;;
    esac
    case "$RC" in
      *.fish) PATH_LINE="fish_add_path $(sh_quote "$INSTALL_DIR")" ;;
      *)      PATH_LINE="export PATH=$(sh_quote "$INSTALL_DIR"):\"\$PATH\"" ;;
    esac
    if [ -n "${OXDM_NO_MODIFY_PATH:-}" ]; then
      warn "$INSTALL_DIR is not in PATH. Add to your shell profile: $PATH_LINE"
    else
      if [ -f "$RC" ] && grep -qxF "$PATH_LINE" "$RC"; then
        info "$RC already adds $INSTALL_DIR to PATH"
      else
        mkdir -p "$(dirname "$RC")"
        printf '\n# Added by the oxdm installer\n%s\n' "$PATH_LINE" >> "$RC"
        ok "added $INSTALL_DIR to PATH in $RC"
      fi
      warn "open a new terminal to use the oxdm command, or run: $PATH_LINE"
    fi
    ;;
esac

VER_OUT="$("$INSTALL_DIR/oxdm" --version 2>/dev/null || true)"
[ -n "$VER_OUT" ] && info "$VER_OUT"
ok "done. Run: oxdm"

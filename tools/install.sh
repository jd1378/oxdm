#!/bin/sh
# oxdm installer — Linux / macOS.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/jd1378/oxdm/main/tools/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/jd1378/oxdm/main/tools/install.sh | sh -s -- --dir /usr/local/bin
#
# Env:
#   OXDM_INSTALL_DIR  install directory (default: $HOME/.local/bin)
#   OXDM_VERSION      tag to install (default: latest)
#   OXDM_NO_DESKTOP   set to skip writing the .desktop entry (Linux)
#   NO_COLOR          disable color output

set -eu

REPO="jd1378/oxdm"
INSTALL_DIR="${OXDM_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${OXDM_VERSION:-latest}"

while [ $# -gt 0 ]; do
  case "$1" in
    --dir) INSTALL_DIR="$2"; shift 2 ;;
    --version) VERSION="$2"; shift 2 ;;
    -h|--help)
      cat <<EOF
oxdm installer
  --dir <path>     install directory (default: \$HOME/.local/bin)
  --version <tag>  release tag (default: latest)
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

# Linux runtime deps. The UI is software-rendered and links no toolkit,
# so the only thing worth checking for is D-Bus, which the tray and
# desktop notifications use. Its absence is a warning, not an error:
# oxdm runs without them.
if [ "$OS" = "Linux" ]; then
  if ! ldconfig -p 2>/dev/null | grep -q "libdbus-1.so.3"; then
    warn "libdbus-1 not found — the tray icon and desktop notifications will be unavailable."
    if   [ -f /etc/debian_version ]; then
      info "Install with: sudo apt install libdbus-1-3"
    elif [ -f /etc/fedora-release ] || [ -f /etc/redhat-release ]; then
      info "Install with: sudo dnf install dbus-libs"
    elif [ -f /etc/arch-release ]; then
      info "Install with: sudo pacman -S dbus"
    fi
  fi
fi

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

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) warn "$INSTALL_DIR not in PATH. Add: export PATH=\"$INSTALL_DIR:\$PATH\"" ;;
esac

VER_OUT="$("$INSTALL_DIR/oxdm" --version 2>/dev/null || true)"
[ -n "$VER_OUT" ] && info "$VER_OUT"
ok "done. Run: oxdm"

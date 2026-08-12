#!/bin/sh
# oxdm uninstaller — Linux / macOS.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/jd1378/oxdm/main/tools/uninstall.sh | sh
#   # also wipe config / queue DB:
#   curl -fsSL https://raw.githubusercontent.com/jd1378/oxdm/main/tools/uninstall.sh | sh -s -- --purge
#
# Env:
#   OXDM_INSTALL_DIR  override install directory (default: $HOME/.local/bin)

set -eu

INSTALL_DIR="${OXDM_INSTALL_DIR:-$HOME/.local/bin}"
PURGE=0

while [ $# -gt 0 ]; do
  case "$1" in
    --dir) INSTALL_DIR="$2"; shift 2 ;;
    --purge) PURGE=1; shift ;;
    -h|--help)
      cat <<EOF
oxdm uninstaller
  --dir <path>  install directory (default: \$HOME/.local/bin)
  --purge       also delete user config + queue DB
EOF
      exit 0
      ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

step() { printf '\033[36m==>\033[0m %s\n' "$*"; }
ok()   { printf '\033[32m✓\033[0m %s\n' "$*"; }
warn() { printf '\033[33m!\033[0m %s\n' "$*"; }

OS="$(uname -s)"

step "Removing binaries"
for bin in oxdm oxdm-native-host; do
  if [ -f "$INSTALL_DIR/$bin" ]; then
    rm -f "$INSTALL_DIR/$bin"
    ok "removed $INSTALL_DIR/$bin"
  fi
done

if [ "$OS" = "Linux" ]; then
  for f in "$HOME/.local/share/applications/oxdm.desktop" \
           "$HOME/.config/autostart/oxdm.desktop" \
           "$HOME/.local/share/icons/hicolor/512x512/apps/oxdm.png"; do
    if [ -f "$f" ]; then rm -f "$f"; ok "removed $f"; fi
  done
fi

if [ "$PURGE" = 1 ]; then
  step "Purging user data"
  for d in "$HOME/.config/oxdm" "$HOME/Library/Application Support/oxdm"; do
    if [ -d "$d" ]; then rm -rf "$d"; ok "removed $d"; fi
  done
else
  warn "user data preserved (run with --purge to also delete settings + queue)"
fi

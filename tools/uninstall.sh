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

# Removing the program does not stop it: the daemon would keep its
# tray icon and downloads going, and under --purge write its database
# back after it was deleted.
if [ -x "$INSTALL_DIR/oxdm" ] && pgrep -x oxdm >/dev/null 2>&1; then
  step "Stopping oxdm"
  "$INSTALL_DIR/oxdm" --quit >/dev/null 2>&1 || true
  i=0
  while pgrep -x oxdm >/dev/null 2>&1 && [ "$i" -lt 40 ]; do
    sleep 0.5; i=$((i + 1))
  done
  if pgrep -x oxdm >/dev/null 2>&1; then
    warn "oxdm is still running; quit it from its tray icon"
  else
    ok "oxdm stopped"
  fi
fi

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

if [ "$OS" = "Darwin" ]; then
  # Login autostart, written when "start with system" is on.
  f="$HOME/Library/LaunchAgents/com.oxdm.app.plist"
  if [ -f "$f" ]; then
    launchctl unload "$f" >/dev/null 2>&1 || true
    rm -f "$f"; ok "removed $f"
  fi
fi

if [ "$PURGE" = 1 ]; then
  step "Purging user data"
  # The database, the queue and the update staging live under the
  # *data* directory — `~/.local/share/oxdm` on Linux, which this list
  # used to miss entirely, so `--purge` removed nothing there and
  # reinstalling picked the old database straight back up. The config
  # directory holds the window's own preferences, and the cache
  # directory whatever has been staged.
  for d in \
    "${XDG_DATA_HOME:-$HOME/.local/share}/oxdm" \
    "${XDG_CONFIG_HOME:-$HOME/.config}/oxdm" \
    "${XDG_CACHE_HOME:-$HOME/.cache}/oxdm" \
    "${XDG_STATE_HOME:-$HOME/.local/state}/oxdm" \
    "$HOME/Library/Application Support/oxdm" \
    "$HOME/Library/Caches/oxdm"; do
    if [ -d "$d" ]; then rm -rf "$d"; ok "removed $d"; fi
  done
  # The browser registrations point at a binary that is now gone.
  for d in \
    "${XDG_CONFIG_HOME:-$HOME/.config}"/*/NativeMessagingHosts \
    "$HOME"/.mozilla/native-messaging-hosts \
    "$HOME"/.zen/native-messaging-hosts \
    "$HOME"/.librewolf/native-messaging-hosts \
    "$HOME"/.var/app/*/config/*/NativeMessagingHosts \
    "$HOME"/.var/app/*/.mozilla/native-messaging-hosts \
    "$HOME"/.var/app/*/.librewolf/native-messaging-hosts \
    "$HOME"/Library/Application\ Support/*/NativeMessagingHosts; do
    f="$d/io.github.jd1378.oxdm.host.json"
    if [ -f "$f" ]; then rm -f "$f"; ok "removed $f"; fi
  done
  for w in "$HOME"/.var/app/*/data/oxdm-native-host; do
    if [ -f "$w" ]; then rm -f "$w"; ok "removed $w"; fi
  done
else
  warn "user data preserved (run with --purge to also delete settings + queue)"
fi

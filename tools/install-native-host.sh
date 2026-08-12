#!/usr/bin/env sh
# Register oxdm as a native-messaging host with the browsers on this
# machine, so an extension can hand downloads to the app.
#
# Usage:
#   install-native-host.sh [--chromium-id <ID>[,<ID>...]]
#                          [--firefox-id <ID>[,<ID>...]]
#                          [--oxdm <PATH>] [--dry-run]
#
#   --chromium-id / --firefox-id
#       Pair a build of the extension other than the published one.
#       Given for one family, the shipped id for the other still
#       applies — pairing a development build in Chrome should not
#       unpair the store build in Firefox.
#   --oxdm <PATH>
#       The oxdm binary to ask. Defaults to one beside this script,
#       then to whatever is on $PATH.
#   --dry-run
#       Print what would be written, write nothing.
#
# The work itself lives in the app (`oxdm --install-native-host`), and
# this script only finds the binary and forwards the flags. It used to
# carry its own copy of every browser's manifest path, which is one
# copy too many: a script and an app that disagree about where a
# manifest goes produce a browser that quietly stops capturing, and
# nothing that says so.
#
# oxdm also does this by itself — on first run, and again on every
# start if a manifest has gone missing or stale. Reach for this script
# when you want a specific extension id, a dry run, or a machine set up
# without opening the app.
#
# No root needed. Per-user, per-browser, no system-wide changes.

set -eu

err() { printf 'error: %s\n' "$*" >&2; exit 1; }

OXDM=""
ARGS=""

while [ $# -gt 0 ]; do
    case "$1" in
        --oxdm)
            shift; [ $# -gt 0 ] || err "--oxdm needs a value"
            OXDM="$1" ;;
        --chromium-id|--firefox-id)
            flag="$1"; shift; [ $# -gt 0 ] || err "$flag needs a value"
            ARGS="$ARGS $flag $1" ;;
        --dry-run)
            ARGS="$ARGS --dry-run" ;;
        -h|--help)
            sed -n '2,32p' "$0"; exit 0 ;;
        *) err "unknown flag: $1" ;;
    esac
    shift
done

if [ -z "$OXDM" ]; then
    script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
    if [ -x "$script_dir/oxdm" ]; then
        OXDM="$script_dir/oxdm"
    elif command -v oxdm >/dev/null 2>&1; then
        OXDM=$(command -v oxdm)
    else
        err "could not find the oxdm binary; pass --oxdm <PATH>"
    fi
fi
[ -x "$OXDM" ] || err "$OXDM is not executable"

# Word splitting is what carries the collected flags through; each one
# is a bare token or an extension id, neither of which contains spaces.
# shellcheck disable=SC2086
exec "$OXDM" --install-native-host $ARGS

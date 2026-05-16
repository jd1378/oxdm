#!/usr/bin/env sh
# Install the oxdm native-messaging manifest for one or more browsers.
#
# Usage:
#   install-native-host.sh --chromium-id <ID> [--firefox-id <ID>] \
#       [--host-binary <PATH>] [--dry-run]
#
#   --chromium-id <ID>   Chromium-family extension ID (32-char). Required
#                        for Chrome / Chromium / Edge / Brave / Vivaldi /
#                        Opera installs. Repeat or comma-separate to
#                        allow multiple extensions to talk to the host.
#   --firefox-id  <ID>   Firefox extension id (e.g. "oxdm@jd1378.github.io").
#                        Required for Firefox / Zen / LibreWolf installs.
#   --host-binary <PATH> Override the absolute path written into the
#                        manifest's `path` field. Defaults to the
#                        `oxdm-native-host` binary co-located with this
#                        script (`<script-dir>/../bin/oxdm-native-host`)
#                        when present, otherwise whichever
#                        `oxdm-native-host` is on $PATH.
#   --token-file <PATH>  Use --token-fd to pass the auth token to the
#                        host so it never appears in argv / `ps`.
#                        The installer writes a tiny wrapper shell
#                        script that exec's the host with fd 3
#                        redirected from <PATH>; the manifest's
#                        `path` then points at the wrapper. Skip this
#                        flag to let the host self-discover token +
#                        port from oxdm.db (the default).
#   --db-path <PATH>     Absolute path to oxdm.db used by the host for
#                        auto-discovery of port + ext_token. Defaults
#                        to `~/.config/oxdm/oxdm.db` (Linux) or the
#                        equivalent per-platform user config dir.
#                        Only consulted for Flatpak targets, where the
#                        sandboxed `$HOME` would otherwise misroute the
#                        host's own `dirs::config_dir()` autodiscovery.
#   --patch-desktop      For each Flatpak browser whose manifest gets
#                        written, look for an editable user .desktop
#                        file under ~/.local/share/applications and
#                        splice the required --filesystem= args into
#                        its Exec= line. Prompts before writing. Skip
#                        the flag to just print the args at the end.
#   --yes / -y           Accept all interactive prompts (currently:
#                        --patch-desktop). Useful for scripted runs.
#   --dry-run            Print what would be written instead of writing.
#
# The script writes one `io.github.jd1378.oxdm.host.json` per supported browser into
# the per-user manifest dir. Existing files are overwritten in place
# (always back up first if your extension is shared with other hosts).
#
# Covers native, Flatpak, and Snap installs of every supported browser.
# Uninstalled browsers are skipped automatically (parent config dir
# missing = no write).
#
# Sandbox notes:
#   - For Flatpak browsers, this script writes a small wrapper into
#     `~/.var/app/<id>/data/oxdm-native-host` and points the manifest
#     at it. The wrapper exec's the real host with --db-path set to
#     the host-side `oxdm.db`, since the sandbox's `$HOME` rewrites
#     `dirs::config_dir()` to `~/.var/app/<id>/config` where no DB
#     exists. The host binary and oxdm.db still need to be visible
#     inside the sandbox; after install, the script prints the exact
#     `flatpak override` / `flatpak run` args needed (binary + DB,
#     read-only). Never use `--filesystem=host:ro` here — narrow grants
#     are enough and far safer.
#   - Snap browsers run under stricter confinement. Native messaging
#     hooks exist (the manifest paths below are correct) but executing
#     a host binary outside the snap usually requires snap-side
#     interface support that not every snap exposes. Treat the snap
#     paths as best-effort.
#
# No root needed. Per-user, per-browser, no system-wide changes.

set -eu

err() { printf 'error: %s\n' "$*" >&2; exit 1; }

CHROMIUM_IDS=""
FIREFOX_IDS=""
HOST_BINARY=""
TOKEN_FILE=""
DB_PATH=""
PATCH_DESKTOP=""
ASSUME_YES=""
DRY_RUN=""

while [ $# -gt 0 ]; do
    case "$1" in
        --chromium-id)
            shift; [ $# -gt 0 ] || err "--chromium-id needs a value"
            CHROMIUM_IDS="${CHROMIUM_IDS}${CHROMIUM_IDS:+,}$1" ;;
        --firefox-id)
            shift; [ $# -gt 0 ] || err "--firefox-id needs a value"
            FIREFOX_IDS="${FIREFOX_IDS}${FIREFOX_IDS:+,}$1" ;;
        --host-binary)
            shift; [ $# -gt 0 ] || err "--host-binary needs a value"
            HOST_BINARY="$1" ;;
        --token-file)
            shift; [ $# -gt 0 ] || err "--token-file needs a value"
            TOKEN_FILE="$1" ;;
        --db-path)
            shift; [ $# -gt 0 ] || err "--db-path needs a value"
            DB_PATH="$1" ;;
        --patch-desktop) PATCH_DESKTOP=1 ;;
        -y|--yes) ASSUME_YES=1 ;;
        --dry-run) DRY_RUN=1 ;;
        -h|--help) sed -n '2,45p' "$0"; exit 0 ;;
        *) err "unknown flag: $1" ;;
    esac
    shift
done

[ -n "$CHROMIUM_IDS$FIREFOX_IDS" ] \
    || err "supply at least one of --chromium-id / --firefox-id"

# Resolve DB path used by Flatpak wrappers (--db-path on the host).
if [ -z "$DB_PATH" ]; then
    case "$(uname -s)" in
        Darwin) DB_PATH="$HOME/Library/Application Support/oxdm/oxdm.db" ;;
        Linux|*) DB_PATH="$HOME/.config/oxdm/oxdm.db" ;;
    esac
fi
case "$DB_PATH" in
    /*) ;;
    *) err "--db-path must be an absolute path; got: $DB_PATH" ;;
esac

# Resolve host binary path.
if [ -z "$HOST_BINARY" ]; then
    script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
    if [ -x "$script_dir/oxdm-native-host" ]; then
        HOST_BINARY="$script_dir/oxdm-native-host"
    elif command -v oxdm-native-host >/dev/null 2>&1; then
        HOST_BINARY=$(command -v oxdm-native-host)
    else
        err "could not locate oxdm-native-host; pass --host-binary <PATH>"
    fi
fi
case "$HOST_BINARY" in
    /*) ;; # absolute, good
    *) err "--host-binary must be an absolute path; got: $HOST_BINARY" ;;
esac
[ -x "$HOST_BINARY" ] || err "$HOST_BINARY is not executable"

# If --token-file was supplied, plant a wrapper next to the binary
# that exec's the host with the secret on fd 3. The manifest's `path`
# becomes the wrapper's path. The host never sees the token in argv.
# The wrapper carries 0700 perms; the token file must be 0600 (the
# script enforces this).
MANIFEST_TARGET_PATH="$HOST_BINARY"
WRAPPER_PATH=""
if [ -n "$TOKEN_FILE" ]; then
    case "$TOKEN_FILE" in
        /*) ;;
        *) err "--token-file must be an absolute path; got: $TOKEN_FILE" ;;
    esac
    [ -r "$TOKEN_FILE" ] || err "token file not readable: $TOKEN_FILE"
    # Tighten the secret's perms before we trust it.
    chmod 0600 "$TOKEN_FILE" 2>/dev/null || true
    WRAPPER_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/oxdm"
    WRAPPER_PATH="$WRAPPER_DIR/oxdm-native-host-fd.sh"
    if [ -z "$DRY_RUN" ]; then
        mkdir -p "$WRAPPER_DIR"
        umask 077
        cat >"$WRAPPER_PATH" <<EOF
#!/bin/sh
# Generated by oxdm/tools/install-native-host.sh. Reads the oxdm
# extension token from a fixed file and pipes it to the native host
# on fd 3 so the secret never lands in argv.
exec "$HOST_BINARY" --token-fd 3 "\$@" 3< "$TOKEN_FILE"
EOF
        chmod 0700 "$WRAPPER_PATH"
        printf 'wrote %s\n' "$WRAPPER_PATH"
    else
        printf 'would write wrapper %s pointing fd 3 at %s\n' "$WRAPPER_PATH" "$TOKEN_FILE"
    fi
    MANIFEST_TARGET_PATH="$WRAPPER_PATH"
fi

# Per-browser manifest dirs. The host name `io.github.jd1378.oxdm.host` must match
# what the extension passes to runtime.connectNative().
HOST_NAME="io.github.jd1378.oxdm.host"

case "$(uname -s)" in
    Darwin)
        CHROMIUM_DIRS="
$HOME/Library/Application Support/Google/Chrome/NativeMessagingHosts
$HOME/Library/Application Support/Chromium/NativeMessagingHosts
$HOME/Library/Application Support/Microsoft Edge/NativeMessagingHosts
$HOME/Library/Application Support/BraveSoftware/Brave-Browser/NativeMessagingHosts
$HOME/Library/Application Support/Vivaldi/NativeMessagingHosts
$HOME/Library/Application Support/com.operasoftware.Opera/NativeMessagingHosts
"
        FIREFOX_DIRS="
$HOME/Library/Application Support/Mozilla/NativeMessagingHosts
$HOME/Library/Application Support/zen/NativeMessagingHosts
$HOME/Library/Application Support/LibreWolf/NativeMessagingHosts
"
        ;;
    Linux|*)
        # Native (non-sandboxed) installs.
        CHROMIUM_DIRS="
$HOME/.config/google-chrome/NativeMessagingHosts
$HOME/.config/chromium/NativeMessagingHosts
$HOME/.config/ungoogled-chromium/NativeMessagingHosts
$HOME/.config/microsoft-edge/NativeMessagingHosts
$HOME/.config/BraveSoftware/Brave-Browser/NativeMessagingHosts
$HOME/.config/vivaldi/NativeMessagingHosts
$HOME/.config/opera/NativeMessagingHosts
"
        FIREFOX_DIRS="
$HOME/.mozilla/native-messaging-hosts
$HOME/.zen/native-messaging-hosts
$HOME/.librewolf/native-messaging-hosts
"
        # Flatpak installs. Each browser app stores its config under
        # `~/.var/app/<flatpak-id>/`; native-messaging dirs sit at the
        # same relative paths a non-sandboxed install would use.
        CHROMIUM_DIRS="$CHROMIUM_DIRS
$HOME/.var/app/com.google.Chrome/config/google-chrome/NativeMessagingHosts
$HOME/.var/app/org.chromium.Chromium/config/chromium/NativeMessagingHosts
$HOME/.var/app/io.github.ungoogled_software.ungoogled_chromium/config/chromium/NativeMessagingHosts
$HOME/.var/app/com.microsoft.Edge/config/microsoft-edge/NativeMessagingHosts
$HOME/.var/app/com.brave.Browser/config/BraveSoftware/Brave-Browser/NativeMessagingHosts
$HOME/.var/app/com.vivaldi.Vivaldi/config/vivaldi/NativeMessagingHosts
$HOME/.var/app/com.opera.Opera/config/opera/NativeMessagingHosts
"
        FIREFOX_DIRS="$FIREFOX_DIRS
$HOME/.var/app/org.mozilla.firefox/.mozilla/native-messaging-hosts
$HOME/.var/app/app.zen_browser.zen/.zen/native-messaging-hosts
$HOME/.var/app/io.gitlab.librewolf-community/.librewolf/native-messaging-hosts
"
        # Snap installs. Snap confinement may still block exec of an
        # out-of-snap host binary; see the docstring for caveats.
        CHROMIUM_DIRS="$CHROMIUM_DIRS
$HOME/snap/chromium/current/.config/chromium/NativeMessagingHosts
"
        FIREFOX_DIRS="$FIREFOX_DIRS
$HOME/snap/firefox/common/.mozilla/native-messaging-hosts
"
        ;;
esac

# Build allowed_origins / allowed_extensions JSON arrays.
csv_to_json_array() {
    # csv → ["a","b",...]
    set -- $(printf '%s' "$1" | tr ',' ' ')
    printf '['
    sep=""
    for x in "$@"; do
        printf '%s"%s"' "$sep" "$x"
        sep=","
    done
    printf ']'
}

chromium_origins=""
if [ -n "$CHROMIUM_IDS" ]; then
    set -- $(printf '%s' "$CHROMIUM_IDS" | tr ',' ' ')
    sep=""
    for id in "$@"; do
        chromium_origins="${chromium_origins}${sep}\"chrome-extension://${id}/\""
        sep=","
    done
    chromium_origins="[$chromium_origins]"
fi
firefox_exts=""
[ -n "$FIREFOX_IDS" ] && firefox_exts=$(csv_to_json_array "$FIREFOX_IDS")

# Write $2 to file $1 only when content differs. On first divergence
# preserves the prior file as `$1.oxdm.bak` so a misbehaving install
# can be reverted by hand. Idempotent reruns are no-ops (no mtime
# churn, no spurious backups). Returns 0 on success; non-zero only on
# real IO failure.
write_if_changed() {
    target="$1"
    content="$2"
    mode="${3:-0600}"
    if [ -f "$target" ] \
        && [ "$(printf '%s\n' "$content")" = "$(cat "$target")" ]; then
        printf 'unchanged %s\n' "$target"
        return 0
    fi
    if [ -f "$target" ] && [ ! -f "$target.oxdm.bak" ]; then
        cp -p "$target" "$target.oxdm.bak" \
            && printf 'backed up %s -> %s.oxdm.bak\n' "$target" "$target"
    fi
    mkdir -p "$(dirname "$target")"
    umask 077
    printf '%s\n' "$content" >"$target.oxdm.tmp" \
        && chmod "$mode" "$target.oxdm.tmp" \
        && mv "$target.oxdm.tmp" "$target"
    printf 'wrote %s\n' "$target"
}

# Extract the Flatpak app id from a manifest dir (~/.var/app/<id>/...)
# or echo nothing if the dir does not live inside a Flatpak data tree.
flatpak_id_for_dir() {
    case "$1" in
        "$HOME"/.var/app/*)
            tmp=${1#"$HOME"/.var/app/}
            printf '%s' "${tmp%%/*}"
            ;;
    esac
}

# Plant a per-Flatpak wrapper inside the sandbox's own data dir. The
# wrapper exec's the host binary (bind-mounted from the host) with the
# real on-host `--db-path`, because dirs::config_dir() inside the
# sandbox resolves to ~/.var/app/<id>/config, not ~/.config. Returns
# the wrapper's path on stdout.
plant_flatpak_wrapper() {
    fp_id="$1"
    wrapper_dir="$HOME/.var/app/$fp_id/data"
    wrapper="$wrapper_dir/oxdm-native-host"
    inner_body="$HOST_BINARY --db-path \"$DB_PATH\""
    if [ -n "$TOKEN_FILE" ]; then
        # Honor --token-file by routing the secret through fd 3 from
        # inside the sandbox. The token file must be reachable inside
        # the sandbox — caller is responsible for the filesystem grant.
        inner_body="$inner_body --token-fd 3"
        redir=" 3< \"$TOKEN_FILE\""
    else
        redir=""
    fi
    wrapper_body=$(cat <<EOF
#!/bin/sh
# Generated by oxdm/tools/install-native-host.sh.
# Flatpak Chromium / Chrome / Edge / … spawns this from inside its
# sandbox. The host binary is bind-mounted at its real path via
# --filesystem=...:ro; we pass --db-path so the host stops looking for
# oxdm.db under the sandbox's own \$HOME.
exec $inner_body "\$@"$redir
EOF
)
    if [ -n "$DRY_RUN" ]; then
        printf 'would write %s (Flatpak wrapper for %s)\n' "$wrapper" "$fp_id" >&2
    else
        write_if_changed "$wrapper" "$wrapper_body" 0755 >&2
    fi
    printf '%s' "$wrapper"
}

write_manifest() {
    target_dir="$1"
    body="$2"
    [ -d "$(dirname "$target_dir")" ] || return 0  # browser not present
    target_path="$target_dir/${HOST_NAME}.json"

    # Flatpak path? Plant a wrapper, rewrite manifest "path".
    fp_id=$(flatpak_id_for_dir "$target_dir")
    if [ -n "$fp_id" ]; then
        wrapper=$(plant_flatpak_wrapper "$fp_id")
        # Replace the "path" line. The original body uses
        # MANIFEST_TARGET_PATH (host binary or --token-file wrapper);
        # for Flatpak we always want the per-sandbox wrapper instead.
        body=$(printf '%s' "$body" | sed \
            -e "s|\"path\": \"[^\"]*\"|\"path\": \"$wrapper\"|")
    fi

    if [ -n "$DRY_RUN" ]; then
        printf 'would write %s:\n%s\n\n' "$target_path" "$body"
        return 0
    fi
    # Manifest contains a token-bearing path only; chmod 600 so other
    # local users can't read it. (No secret in there today, but keep
    # the habit if the schema grows.)
    write_if_changed "$target_path" "$body" 0600
}

chromium_body=$(cat <<JSON
{
  "name": "$HOST_NAME",
  "description": "oxdm download capture host",
  "path": "$MANIFEST_TARGET_PATH",
  "type": "stdio",
  "allowed_origins": $chromium_origins
}
JSON
)

firefox_body=$(cat <<JSON
{
  "name": "$HOST_NAME",
  "description": "oxdm download capture host",
  "path": "$MANIFEST_TARGET_PATH",
  "type": "stdio",
  "allowed_extensions": $firefox_exts
}
JSON
)

# `while read` runs in a subshell under POSIX sh, which would lose any
# FLATPAK_IDS_TOUCHED updates we make from inside. Route the touched
# ids through a temp file so the summary below can see them.
FLATPAK_TOUCHED_FILE=$(mktemp)
trap 'rm -f "$FLATPAK_TOUCHED_FILE"' EXIT

record_flatpak_touch() {
    fp=$(flatpak_id_for_dir "$1")
    [ -n "$fp" ] || return 0
    grep -qxF "$fp" "$FLATPAK_TOUCHED_FILE" 2>/dev/null \
        || printf '%s\n' "$fp" >>"$FLATPAK_TOUCHED_FILE"
}

if [ -n "$CHROMIUM_IDS" ]; then
    echo "$CHROMIUM_DIRS" | while IFS= read -r dir; do
        [ -z "$dir" ] && continue
        write_manifest "$dir" "$chromium_body"
        # Only record dirs whose parent actually exists (browser
        # installed); write_manifest is a no-op otherwise.
        [ -d "$(dirname "$dir")" ] && record_flatpak_touch "$dir"
    done
fi
if [ -n "$FIREFOX_IDS" ]; then
    echo "$FIREFOX_DIRS" | while IFS= read -r dir; do
        [ -z "$dir" ] && continue
        write_manifest "$dir" "$firefox_body"
        [ -d "$(dirname "$dir")" ] && record_flatpak_touch "$dir"
    done
fi

#--- Post-install Flatpak summary --------------------------------------

# Build the --filesystem= arg list every Flatpak browser will need at
# launch time so the bind-mounted host binary + DB are visible.
# Mount the DB's *directory*, not the file: sqlite runs in WAL mode
# and reads sidecar `<db>-wal` / `<db>-shm` files alongside the DB
# even for read-only opens.
flatpak_filesystem_args() {
    db_dir=$(dirname -- "$DB_PATH")
    printf -- '--filesystem=%s:ro --filesystem=%s:ro' "$HOST_BINARY" "$db_dir"
    if [ -n "$TOKEN_FILE" ]; then
        printf -- ' --filesystem=%s:ro' "$TOKEN_FILE"
    fi
}

# Splice the filesystem args into a .desktop file's Exec= line.
# Heuristics: only touch the first Exec= line, and only if the args
# aren't already present. Writes to <orig>.desktop in
# ~/.local/share/applications so the system-wide file stays untouched.
patch_desktop_file() {
    fp_id="$1"
    src=""
    # Prefer user-installed desktop file; fall back to copying the
    # system one (Flatpak installs system-wide by default).
    for cand in \
        "$HOME/.local/share/applications/$fp_id.desktop" \
        "/var/lib/flatpak/exports/share/applications/$fp_id.desktop" \
        "/var/lib/flatpak/app/$fp_id/current/active/export/share/applications/$fp_id.desktop"
    do
        [ -r "$cand" ] && src="$cand" && break
    done
    if [ -z "$src" ]; then
        printf '  (no .desktop file found for %s)\n' "$fp_id"
        return 0
    fi
    dest="$HOME/.local/share/applications/$fp_id.desktop"

    fs_args=$(flatpak_filesystem_args)

    if [ -z "$ASSUME_YES" ]; then
        printf '  patch %s -> %s ? [y/N] ' "$src" "$dest" >&2
        read ans
        case "$ans" in y|Y|yes|YES) ;; *) printf '  skipped\n'; return 0 ;; esac
    fi
    if [ -n "$DRY_RUN" ]; then
        printf '  would write %s with %s spliced into Exec=\n' "$dest" "$fs_args"
        return 0
    fi
    # Insert each `--filesystem=…:ro` token after `flatpak run` on every
    # Exec= line (desktop files commonly carry extra Exec= entries
    # inside [Desktop Action …] sections — incognito launcher etc. —
    # and all of them need the same grants). Per-token idempotency:
    # the index() guard checks each arg independently, so reruns are
    # safe even when the original Exec already lists some of the
    # filesystem args in a different order than this script would
    # have written them.
    new_body=$(awk -v fsargs="$fs_args" '
        BEGIN {
            n = split(fsargs, toks, " ")
        }
        /^Exec=/ {
            for (i = 1; i <= n; i++) {
                if (toks[i] != "" && index($0, toks[i]) == 0) {
                    sub(/flatpak[[:space:]]+run/, "& " toks[i])
                }
            }
        }
        { print }
    ' "$src")
    write_if_changed "$dest" "$new_body" 0644 | sed 's/^/  /'
}

if [ -s "$FLATPAK_TOUCHED_FILE" ]; then
    printf '\nFlatpak browsers touched:\n'
    while IFS= read -r fp; do
        printf '  - %s\n' "$fp"
    done <"$FLATPAK_TOUCHED_FILE"

    fs_args=$(flatpak_filesystem_args)
    printf '\nEach Flatpak browser above needs read access to the host\n'
    printf 'binary and oxdm.db. Pick ONE of the following per browser:\n\n'
    printf '  (A) Persistent override (recommended):\n'
    while IFS= read -r fp; do
        printf '        flatpak override --user %s %s\n' "$fs_args" "$fp"
    done <"$FLATPAK_TOUCHED_FILE"
    printf '\n  (B) Per-launch args (no persistent state):\n'
    printf '        flatpak run %s <flatpak-id>\n' "$fs_args"

    if [ -n "$PATCH_DESKTOP" ]; then
        printf '\nPatching .desktop files (--patch-desktop):\n'
        while IFS= read -r fp; do
            patch_desktop_file "$fp"
        done <"$FLATPAK_TOUCHED_FILE"
    else
        printf '\nRerun with --patch-desktop to splice the same --filesystem args\n'
        printf 'into each user .desktop Exec= line under ~/.local/share/applications.\n'
    fi
fi

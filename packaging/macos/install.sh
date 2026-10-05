#!/bin/sh
# Installs (or updates) the Submarine service on macOS. Run with sudo from the
# repository root after `scripts/macos/build.sh`.
#
#   sudo packaging/macos/install.sh [--all-users | --only-me]
#
# On the first installation it asks whether every user of this Mac may use
# Submarine, or only the user who ran sudo (administrators always can); the
# options answer without asking. Updates keep the previous choice.
set -eu

LABEL=it.mensys.submarine.daemon
PLIST=/Library/LaunchDaemons/$LABEL.plist
BIN=/Library/PrivilegedHelperTools/submarine-daemon
ACCESS="/Library/Application Support/Submarine/access.json"

if [ "$(id -u)" -ne 0 ]; then
    echo "run with sudo" >&2
    exit 1
fi

# 1. who may use the service, from the options
mode=""
case "${1:-}" in
    --all-users) mode=all ;;
    --only-me) mode=only ;;
    "") ;;
    *)
        echo "usage: sudo $0 [--all-users | --only-me]" >&2
        exit 2
        ;;
esac
# "only me" is the user who ran sudo, unknown when root ran the script directly
if [ "$mode" = only ] && [ -z "${SUDO_USER:-}" ]; then
    echo "--only-me needs sudo from the account of the user" >&2
    exit 1
fi

# 2. stop of the service being updated, if any, and installation of the daemon
# and of its launchd job
launchctl bootout system "$PLIST" 2>/dev/null || true
install -d -m 755 /Library/PrivilegedHelperTools
install -m 755 target/release/submarine-daemon "$BIN"
install -m 644 packaging/macos/$LABEL.plist "$PLIST"

# 3. the question, on the first installation only and with a terminal to ask on;
# otherwise every user is allowed, as without the access file
if [ -z "$mode" ] && [ ! -f "$ACCESS" ]; then
    mode=all
    if [ -t 0 ] && [ -n "${SUDO_USER:-}" ]; then
        printf 'Install Submarine for all the users of this Mac, or only for %s? [A/m] ' "$SUDO_USER"
        read -r answer || answer=""
        case "$answer" in
            [mM]*) mode=only ;;
        esac
    fi
fi
case "$mode" in
    all) "$BIN" access all ;;
    only) "$BIN" access only "$SUDO_USER" ;;
esac

# 4. start of the service
launchctl bootstrap system "$PLIST"
echo "service installed; log: /var/log/submarine-daemon.log"

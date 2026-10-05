#!/bin/sh
# Installs (or updates) the Submarine service on macOS. Run with sudo from the
# repository root after `scripts/macos/build.sh`.
#
#   sudo packaging/macos/install.sh
set -eu

LABEL=it.mensys.submarine.daemon
PLIST=/Library/LaunchDaemons/$LABEL.plist
BIN=/Library/PrivilegedHelperTools/submarine-daemon

if [ "$(id -u)" -ne 0 ]; then
    echo "run with sudo" >&2
    exit 1
fi

# 1. stop of the service being updated, if any
launchctl bootout system "$PLIST" 2>/dev/null || true
# 2. installation of the daemon and of its launchd job
install -d -m 755 /Library/PrivilegedHelperTools
install -m 755 target/release/submarine-daemon "$BIN"
install -m 644 packaging/macos/$LABEL.plist "$PLIST"
# 3. start of the service
launchctl bootstrap system "$PLIST"
echo "service installed; log: /var/log/submarine-daemon.log"

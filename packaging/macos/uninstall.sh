#!/bin/sh
# Removes the Submarine service, its kill switch rules and DNS changes. The data
# directory, with the stored tunnels, is left in place.
#
#   sudo packaging/macos/uninstall.sh
set -eu

LABEL=it.mensys.submarine.daemon
PLIST=/Library/LaunchDaemons/$LABEL.plist
BIN=/Library/PrivilegedHelperTools/submarine-daemon

# 1. stop of the service
launchctl bootout system "$PLIST" 2>/dev/null || true
# 2. removal of the kill switch rules
# NB: stopping the service leaves an enabled kill switch in place on purpose
[ -x "$BIN" ] && "$BIN" reset-firewall || true
# 3. removal of the launchd job and of the daemon
rm -f "$PLIST" "$BIN"
echo "service removed"

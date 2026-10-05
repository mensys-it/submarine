#!/bin/sh
# Builds Submarine on a Mac: the service, the development CLI and the desktop app
# (.app and .dmg). Requirements: Xcode command line tools, Rust, Node.js.
#
#   scripts/macos/build.sh
#
# NB: plain sh has no pipefail, and no pipeline here needs it.
set -eu

# 1. service and development CLI, from the repository root
cd "$(dirname "$0")/../.."
cargo build --release -p submarine-daemon -p submarine-cli
# 2. desktop app, bundled as .app and .dmg
(cd apps/desktop && npm ci && npx tauri build --bundles app,dmg)
# 3. where the results are, and how to install the service
echo "service: target/release/submarine-daemon"
echo "app:     target/release/bundle/"
echo "install the service with: sudo packaging/macos/install.sh"

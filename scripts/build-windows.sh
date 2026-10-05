#!/usr/bin/env bash
# Builds the Windows release from Linux/WSL, in Docker. Requirements: Docker,
# Node.js, curl and unzip. Output in dist/windows:
#   Submarine_<version>_x64-setup.exe   installer (app and service)
#   submarine-daemon.exe, submarine.exe (CLI), wintun.dll
#
#   scripts/build-windows.sh [--copy-to /mnt/c/Users/<you>/Downloads/submarine]
#
# NB: the split tunnel driver cannot be built here, since it needs Visual Studio
# and the WDK. Download it from the GitHub Actions "windows-driver" workflow into
# dist/driver/, or point SUBMARINE_DRIVER_SYS to it. Without it the installer is
# built anyway, but per-app split tunneling is not available.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT="$ROOT/dist/windows"
TARGET=x86_64-pc-windows-msvc
WINTUN_URL=https://www.wintun.net/builds/wintun-0.14.1.zip
WINTUN_SHA256=07c256185d6ee3652e09fa55c0b673e2624b565e02c4b9091c79ca7d2f24ef51
COPY_TO=""
[ "${1:-}" = "--copy-to" ] && COPY_TO=${2:?"--copy-to needs a directory"}

mkdir -p "$OUT"
BIN_DIR="$ROOT/apps/desktop/src-tauri/binaries"
mkdir -p "$BIN_DIR"

# 1. wintun.dll, downloaded once and checked against its hash
echo "== wintun"
if [ ! -f "$BIN_DIR/wintun.dll" ]; then
    tmp=$(mktemp -d)
    curl -fsSL -o "$tmp/wintun.zip" "$WINTUN_URL"
    echo "$WINTUN_SHA256  $tmp/wintun.zip" | sha256sum -c --quiet
    unzip -q -j "$tmp/wintun.zip" wintun/bin/amd64/wintun.dll -d "$BIN_DIR"
    rm -rf "$tmp"
fi

# 2. service, a Tauri sidecar, and the Rust development CLI
echo "== service and CLI"
"$ROOT/scripts/windows/cargo.sh" build --release -p submarine-daemon -p submarine-cli
REL="$ROOT/target/windows-build/$TARGET/release"
# sidecars carry the target triple in their name
cp "$REL/submarine-daemon.exe" "$BIN_DIR/submarine-daemon-$TARGET.exe"

# 3. `submarine` CLI, a single executable built by Bun
echo "== CLI"
(cd "$ROOT/apps/cli" && { [ -d node_modules ] || npm ci --silent; } && npx bun run build:windows >/dev/null)
cp "$ROOT/apps/cli/dist/submarine.exe" "$BIN_DIR/submarine.exe"

# 4. split tunnel driver, among the installer resources only when present
echo "== split tunnel driver"
DRIVER=${SUBMARINE_DRIVER_SYS:-$ROOT/dist/driver/submarine-split-tunnel.sys}
TAURI_CONFIG='{"build":{"beforeBuildCommand":""},"bundle":{"resources":{"binaries/wintun.dll":"wintun.dll","binaries/submarine.exe":"submarine.exe"}}}'
if [ -f "$DRIVER" ]; then
    cp "$DRIVER" "$BIN_DIR/submarine-split-tunnel.sys"
    TAURI_CONFIG='{"build":{"beforeBuildCommand":""},"bundle":{"resources":{"binaries/wintun.dll":"wintun.dll","binaries/submarine.exe":"submarine.exe","binaries/submarine-split-tunnel.sys":"submarine-split-tunnel.sys"}}}'
    echo "including $DRIVER"
else
    rm -f "$BIN_DIR/submarine-split-tunnel.sys"
    echo "WARNING: driver not found ($DRIVER): per-app split tunneling will not be available"
fi

# 5. desktop app and NSIS installer, cross-compiled in the build image
echo "== desktop app"
(cd "$ROOT/apps/desktop" && npm ci --silent && npm run build --silent)
CACHE=${SUBMARINE_WINDOWS_CACHE:-$HOME/.cache/submarine-windows}
mkdir -p "$CACHE/home"
docker run --rm --user "$(id -u):$(id -g)" \
    -v "$ROOT:/src" -w /src/apps/desktop \
    -e HOME=/tmp/home -e CARGO_HOME=/tmp/home/cargo \
    -v "$CACHE/home:/tmp/home" \
    -e CARGO_TARGET_DIR=/src/target/windows-build \
    -e XWIN_ACCEPT_LICENSE=1 \
    submarine-windows-build \
    npx tauri build --runner cargo-xwin --target "$TARGET" --bundles nsis \
        --config "$TAURI_CONFIG"

# 6. release files collected in dist/windows
cp "$REL/submarine-daemon.exe" "$BIN_DIR/submarine.exe" "$BIN_DIR/wintun.dll" "$OUT/"
# the Rust development tool is NOT shipped
rm -f "$OUT/submarine-cli.exe"
cp "$REL"/bundle/nsis/*-setup.exe "$OUT/"
echo "== done: $OUT"
ls -la "$OUT"

# 7. optional copy, e.g. to a Windows folder from WSL
if [ -n "$COPY_TO" ]; then
    mkdir -p "$COPY_TO"
    cp "$OUT"/* "$COPY_TO/"
    echo "copied to $COPY_TO"
fi

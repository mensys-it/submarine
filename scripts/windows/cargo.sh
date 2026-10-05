#!/usr/bin/env bash
# Runs a cargo-xwin command for the Windows target (x86_64-pc-windows-msvc) in the
# Docker build image, which is built on first use. Requirements: Docker.
#
#   scripts/windows/cargo.sh <cargo command> [cargo options...]
#   scripts/windows/cargo.sh clippy -p submarine-daemon -- -D warnings
#   scripts/windows/cargo.sh build --release -p submarine-daemon
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
IMAGE=submarine-windows-build
CACHE=${SUBMARINE_WINDOWS_CACHE:-$HOME/.cache/submarine-windows}

# 1. persistent home: cargo registry, the MSVC CRT and Windows SDK downloaded by
# cargo-xwin, Tauri's NSIS tools
mkdir -p "$CACHE/home"

# 2. the build image, if missing
docker image inspect "$IMAGE" >/dev/null 2>&1 || docker build -q -t "$IMAGE" "$ROOT/scripts/windows" >/dev/null

# 3. the command, run as the calling user so that the build output is not owned
# by root
cmd=$1
shift
exec docker run --rm \
    --user "$(id -u):$(id -g)" \
    -v "$ROOT:/src" -w /src \
    -e HOME=/tmp/home -e CARGO_HOME=/tmp/home/cargo \
    -v "$CACHE/home:/tmp/home" \
    -e CARGO_TARGET_DIR=/src/target/windows-build \
    -e XWIN_ACCEPT_LICENSE=1 \
    "$IMAGE" cargo xwin "$cmd" --target x86_64-pc-windows-msvc "$@"

#!/usr/bin/env bash
# Type-checks the macOS build from Linux (cargo check or clippy), in the Docker
# image of the Windows cross build. Requirements: Docker.
#
#   scripts/macos/check.sh [check | clippy] [cargo options...]
#   scripts/macos/check.sh clippy -p submarine-daemon -- -D warnings
#
# NB: there is no Apple SDK here, so nothing is linked: real builds and tests
# MUST run on a Mac. The few C headers the build scripts need come from
# scripts/macos/stub-include.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
IMAGE=submarine-windows-build
CACHE=${SUBMARINE_WINDOWS_CACHE:-$HOME/.cache/submarine-windows}

# 1. persistent home shared with the Windows build, and the image if missing
mkdir -p "$CACHE/home"
docker image inspect "$IMAGE" >/dev/null 2>&1 || docker build -q -t "$IMAGE" "$ROOT/scripts/windows" >/dev/null

# 2. the cargo command, `check` by default, run as the calling user for the
# aarch64-apple-darwin target, with clang and the stub headers for C code
cmd=${1:-check}
shift || true
exec docker run --rm --user "$(id -u):$(id -g)" \
    -v "$ROOT:/src" -w /src \
    -e HOME=/tmp/home -e CARGO_HOME=/tmp/home/cargo -v "$CACHE/home:/tmp/home" \
    -e CARGO_TARGET_DIR=/src/target/macos-check \
    -e CC_aarch64_apple_darwin=clang \
    -e CFLAGS_aarch64_apple_darwin="--target=arm64-apple-macos11 -ffreestanding -nostdlibinc -I/src/scripts/macos/stub-include" \
    "$IMAGE" cargo "$cmd" --target aarch64-apple-darwin "$@"

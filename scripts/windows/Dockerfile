# Image that cross-compiles Submarine for Windows (x86_64-pc-windows-msvc) from
# Linux, used by scripts/windows/cargo.sh and scripts/build-windows.sh. It also
# type-checks the macOS code (scripts/macos/check.sh).
#
# NB: cargo-xwin downloads the MSVC CRT and the Windows SDK on first use: the
# scripts keep them in a persistent home mounted on every run.

# Base Image
FROM rust:1

# System Packages
# clang and lld compile and link for MSVC, nsis bundles the installer, Node.js
# builds the desktop frontend
RUN apt-get update -qq \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends \
        clang lld llvm nsis nodejs npm curl unzip \
        `# tauri-cli looks for it even when bundling for Windows` \
        libayatana-appindicator3-dev pkg-config >/dev/null \
    && rm -rf /var/lib/apt/lists/*

# Rust Targets And Tools
# aarch64-apple-darwin is ONLY for `cargo check` of the macOS code: no SDK, no linking
RUN rustup target add x86_64-pc-windows-msvc aarch64-apple-darwin \
    && rustup component add clippy \
    && cargo install --locked cargo-xwin@0.23.1 \
    && rm -rf /usr/local/cargo/registry

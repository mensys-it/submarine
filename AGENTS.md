# Agent guidelines

Rules for AI coding agents (and humans) working on Submarine, a cross-platform
WireGuard VPN client: Rust service and libraries in `crates/`, CLI and desktop app
in `apps/`, a vendored Windows driver in `third_party/`.

## Commits

- Message format: `(type): message`, e.g. `(feature): ...`, `(fix): ...`,
  `(maintenance): ...`, `(test): ...`.
- The message is all lowercase, except proper nouns (WireGuard, Windows, macOS,
  Linux, Tauri, IPC, DNS, ...).
- One commit per logical change. Unrelated fixes go in separate commits.
- Come on bro, everyone knows you help us, don't be so selfish: NEVER add AI
  attribution. No `Co-Authored-By` trailers for an AI, no "Generated with ..."
  lines, in commits or pull requests.
- The default branch is `develop`.
- Commit or push ONLY when explicitly asked. NEVER force push without explicit
  approval.

## Review before every commit

Every file included in a commit MUST be reviewed first. Its comments are rewritten
in the project style described below. This applies to new files and to files that
are only moved or touched.

- Only comments change during a review pass. Code, identifiers and string literals
  stay byte-identical, unless fixing them is the purpose of the commit.
- Upstream code in `third_party/` is NOT re-commented. Only the files we own there
  (README, `.inf`, `resource.rc`) are reviewed, so the fork stays comparable with
  upstream.

## Keep the README up to date

After every change to the code, the configuration, the scripts or the packaging,
check whether `README.md` still describes the project correctly, and update it in
the same change if it does not. Typical triggers: new or removed features, CLI
commands and options, configuration keys, supported platforms, build, install or
test steps, requirements and repository layout.

- `README.it.md` is the Italian translation of `README.md`: every update MUST be
  applied to both files, so they stay aligned section by section.
- Changes that do not affect what the README says (internal refactors, bug fixes
  with no visible effect) need no update, but the check is still done.

## Comment style

All comments are in English.

- **File and module docs** (`//!` in Rust, a header block in scripts): what the
  module does and how it fits in the project.
- **Doc comments** (`///`, JSDoc): every public item, plus non-trivial private
  functions, types, constants and fields. Use capitalized full sentences, summary
  first. Describe non-obvious inputs and outputs. Add `# Errors` / `# Safety`
  sections where they help, never boilerplate.
- **Step comments** (`//`) inside function bodies:
  - lowercase start;
  - one above each logical step of a non-trivial function;
  - a short noun phrase or sentence, on its own line above the code.

  For example:
  ```rust
  // resolution of the endpoint hostnames, without holding the state lock
  // undo whatever a previous instance left behind if it crashed
  ```
- **Explain the why** when it is not obvious: constraints, side effects, rejected
  alternatives. This may take 2-4 lines.
- **Caveats** start with `NB:`. Uppercase stresses key words (NOT, ONLY, NEVER,
  MUST).
- **`// SAFETY:` comments** above every `unsafe` block are mandatory.
- **Tests**: one short `//` line above each test, saying the scenario checked.
- **Config files** (`.gitignore`, `Cargo.toml`, Dockerfile): `# Section Title`
  headers in Title Case, a blank line between sections, and an inline comment for
  every non-obvious entry.
- **Shell scripts**:
  - a header (what the script does, requirements, usage);
  - `set -euo pipefail`;
  - numbered step comments (`# 1. ...`).
- **Things to avoid**:
  - end-of-line comments, except short ones in test data or tables;
  - banner art;
  - emojis;
  - invented TODOs;
  - license headers in source files.
- Comment lines stay within 100 columns.

## What goes in the repository

- Only what a user or contributor needs to build, run, test or contribute. Build
  output, local tooling state, design mockups, internal notes, and personal or
  machine-specific scripts stay out (see `.gitignore`).
- No real hostnames, IP addresses, device names, keys or certificates. Use
  documentation values (`vpn.example.com`, `192.0.2.0/24`, `203.0.113.0/24`,
  `2001:db8::/32`) and generated test keys.
- License: GPL-3.0-or-later (`LICENSE`). `third_party/win-split-tunnel` is dual
  licensed GPL-3.0 / MPL-2.0 like its upstream.

## Verification before committing Rust code

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
# cross-platform checks (Docker), for code under cfg(windows) / cfg(target_os = "macos")
scripts/windows/cargo.sh clippy -p submarine-daemon -p submarine-net --all-targets -- -D warnings
scripts/macos/check.sh clippy -p submarine-daemon -p submarine-net -- -D warnings
```

Every commit must build and pass its tests on its own.

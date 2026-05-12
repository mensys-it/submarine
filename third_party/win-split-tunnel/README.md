# Submarine split tunnel driver

Fork of the [Mullvad split tunnel driver](https://github.com/mullvad/win-split-tunnel)
(upstream commit `0a0eb97f67d1dbcb3d08bda66d3b24f465d95475`), dual licensed
GPL-3.0 / MPL-2.0 like upstream (see `LICENSE-GPL.md`, `LICENSE-MPL.txt`).
Upstream documentation: `UPSTREAM-README.md`.

Changes from upstream:

- Renamed device (`\\.\SUBMARINESPLITTUNNEL`), files and display strings, and
  new GUIDs for every WFP object, so it can coexist with Mullvad VPN.

No kernel logic was changed. Submarine's "only the chosen apps use the VPN"
mode reuses the exclusion logic with the tunnel and internet addresses
swapped by the user-mode agent (`crates/submarine-net/src/windows/split.rs`):
the chosen apps are then redirected *to* the tunnel and blocked outside it.

Build: `scripts/windows/build-driver.ps1` (Visual Studio 2022 + WDK), also run by
the GitHub Actions workflow `.github/workflows/windows-driver.yml`.

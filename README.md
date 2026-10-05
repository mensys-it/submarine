<p align="center">🇬🇧 <strong>English</strong> | 🇮🇹 <a href="README.it.md">Italiano</a></p>

<p align="center">
  <img src=".github/assets/icon.png" alt="Submarine" width="160">
</p>

<h1 align="center">Submarine</h1>

<p align="center">
  <strong>A cross-platform WireGuard VPN client for Linux, Windows and macOS.</strong>
</p>

<p align="center">
  <a href="#quick-start">Quick Start</a> &middot;
  <a href="#cli">CLI</a> &middot;
  <a href="#windows">Windows</a> &middot;
  <a href="#macos">macOS</a> &middot;
  <a href="#linux">Linux</a> &middot;
  <a href="#development">Development</a> &middot;
  <a href="#project-layout">Layout</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Rust-2024-000000?logo=rust&logoColor=white" alt="Rust 2024">
  <img src="https://img.shields.io/badge/WireGuard-boringtun-88171A?logo=wireguard&logoColor=white" alt="WireGuard (boringtun)">
  <img src="https://img.shields.io/badge/Tauri-2-24C8DB?logo=tauri&logoColor=white" alt="Tauri 2">
  <img src="https://img.shields.io/badge/React-19-61DAFB?logo=react&logoColor=black" alt="React 19">
  <img src="https://img.shields.io/badge/TypeScript-7-3178C6?logo=typescript&logoColor=white" alt="TypeScript">
  <img src="https://img.shields.io/badge/Bun-Ink-000000?logo=bun&logoColor=white" alt="Bun + Ink">
  <img src="https://img.shields.io/badge/Vite-646CFF?logo=vite&logoColor=white" alt="Vite">
  <br>
  <img src="https://img.shields.io/badge/Linux-FCC624?logo=linux&logoColor=black" alt="Linux">
  <img src="https://img.shields.io/badge/Windows-0078D4?logo=windows&logoColor=white" alt="Windows">
  <img src="https://img.shields.io/badge/macOS-000000?logo=apple&logoColor=white" alt="macOS">
  <img src="https://img.shields.io/badge/license-GPL--3.0--or--later-blue" alt="GPL-3.0-or-later">
</p>

---

Submarine is a WireGuard VPN client with a userspace engine in Rust
([boringtun](https://github.com/cloudflare/boringtun)), a privileged system service that
manages tunnels and the network, a Tauri + React desktop app and an interactive terminal CLI.

```sh
submarine connect office      # tunnel name, even partial
submarine status --json
submarine killswitch on
```

## Why Submarine?

- **One engine, three systems.** The same Rust service on Linux, Windows and macOS, with
  native routing, DNS and firewall integration on each one.
- **Kill switch.** Traffic outside the tunnel is blocked with nftables, WFP or `pf`.
- **Per-app split tunnel.** Only the chosen apps go through the VPN, or everything except
  them (Linux and Windows).
- **Network changes followed.** Moving from Wi-Fi to cable moves the encrypted traffic to the
  new interface without dropping the tunnel.
- **Automatic reconnection.** A tunnel that fails, or whose server leaves the handshakes
  unanswered for 30 seconds, is connected again with growing delays (1 s up to 1 min),
  resolving the server name again. With the kill switch, traffic stays blocked meanwhile.
- **Desktop app and CLI.** A Tauri app with tray icon and an Ink prompt with completion,
  history and scriptable commands.
- **Trusted Wi-Fi networks.** Joining a Wi-Fi network that is not trusted connects the chosen
  tunnel, joining a trusted one (home, office) can disconnect it. The rules act only when the
  network changes, so a manual choice afterwards is respected.
- **Pause.** Disconnects for 5 minutes, 15 minutes or an hour, kill switch included, then
  connects again by itself; from the window, the tray menu or `submarine pause 15`.
- **Live traffic.** Download and upload rates, a chart of the last minute and the session
  time in the app; rates, a sparkline and the session time in the CLI status line.
- **Keys encrypted at rest.** Stored tunnels are encrypted with a key protected by the OS
  keystore: DPAPI on Windows, the System keychain on macOS, `systemd-creds` (TPM2 when present)
  on Linux. Without a keystore the key is protected by the file permissions only.
- **Standard configuration.** Plain wg-quick `.conf` files, imported as they are.

## Quick Start

### 1. Build

```sh
cargo build                                   # service, libraries and dev tool
cd apps/desktop && npm install && npm run tauri build   # desktop app
cd apps/cli && npm install && npx bun run build         # single-file CLI
```

Installers and per-platform builds are described in [Windows](#windows) and [macOS](#macos).

### 2. Start the service

```sh
sudo groupadd -f submarine && sudo usermod -aG submarine "$USER"   # then log in again
sudo ./target/debug/submarine-daemon
```

Every user who can open the service socket may use it. To allow only some users (root and
administrators always can), run as root `submarine-daemon access only <user>...`; `access all`
allows everybody again, `access` shows the current choice.

### 3. Import a tunnel and connect

```sh
submarine import ./office.conf office   # or /import inside the interactive prompt
submarine connect office
```

## CLI

`submarine` without arguments opens an interactive prompt: `/` commands (`/connect`,
`/status`, `/pause`, `/killswitch`, `/log`, `/split`, `/apps`, ...), a completion menu (Tab,
arrows, Esc), history, the connection followed step by step and the state always visible. The
animated ocean in the header follows the state of the service. With `NO_COLOR` or outside a
terminal there are no animations; in a narrow or short terminal a single wave line remains.

With a command it runs and exits, for scripts:

```sh
submarine connect office           # tunnel name, even partial
submarine status --json
submarine killswitch on            # changes only that field of the settings
submarine pause 15                 # 15 minutes without VPN, then it reconnects (or `resume`)
submarine wifi trust               # trusts the Wi-Fi network in use; `wifi auto office` for the others
submarine apps add "C:\Program Files\Mozilla Firefox\firefox.exe"
```

Exit codes: `0` ok, `1` error, `2` wrong usage, `3` service unreachable.

| Task | Command (in `apps/cli`) |
| --- | --- |
| Run from source | `npm install && npx bun run start` |
| Demo against a fake service, without touching the network | `npx bun run demo` |
| Tests | `npx bun test` |
| Single executable | `npx bun run build` (`build:windows`, `build:macos`) |

The Windows installer installs the CLI and adds it to the `PATH`.

## Windows

Built from Linux/WSL in Docker (MSVC cross-compile with `cargo-xwin`):

```sh
scripts/build-windows.sh                                   # output in dist/windows/
scripts/build-windows.sh --copy-to /mnt/c/Users/<you>/Downloads/submarine
scripts/windows/cargo.sh clippy -p submarine-daemon -- -D warnings   # checks only
```

It produces `Submarine_<version>_x64-setup.exe`, which installs the app,
`submarine-daemon.exe` and `wintun.dll` in `Program Files\Submarine` and registers the
**Submarine** service (automatic start, LocalSystem account, restart on crash). A page of the
installer asks whether every user may use Submarine or only the current one; on an update it
shows the current choice, which is kept unless changed. Uninstalling removes the service and the kill switch rules.

Without the installer, from an **administrator** terminal, with `wintun.dll` next to the
executable:

```powershell
.\submarine-daemon.exe                  # in the foreground, logs on the console
.\submarine-daemon.exe service install  # or as a service
.\submarine-cli.exe import Office .\office.conf
.\submarine-cli.exe connect <id>
.\submarine-daemon.exe reset-firewall   # removes the kill switch rules
.\submarine-daemon.exe access only <user>   # only this user (and administrators) may use it
```

<details>
<summary><strong>How it works on Windows</strong></summary>

- Routes with IP Helper (`0.0.0.0/1` + `128.0.0.0/1` for full traffic).
- Tunnel socket bound to the physical interface (`IP_UNICAST_IF`), plus a host route to the
  endpoint through the current gateway.
- DNS on the tunnel interface with the lowest metric; with an "all traffic" tunnel, DNS
  queries outside the tunnel are blocked even without the kill switch.
- Kill switch with persistent WFP filters.
- The named pipe is accessible only to interactively logged-on users, who cannot create
  instances of it; clients refuse a pipe not owned by SYSTEM or Administrators. Its name
  changes at every start and is published in `HKLM\SOFTWARE\Submarine`, so a pipe created
  in advance by another user cannot keep the service from starting. The
  `ProgramData\Submarine` folder is accessible only to SYSTEM and Administrators; one
  created by another user before the service is moved aside and replaced.
- Stored tunnels encrypted with a key protected by DPAPI, bound to the computer.
- Service log in `ProgramData\Submarine\daemon.log`, moved to `daemon.log.old` past 10 MB.
- Per-app tunnel: kernel driver in `third_party/win-split-tunnel` (fork of the Mullvad
  driver), built and test-signed by GitHub Actions on a hosted Windows runner. The service
  loads it from `submarine-split-tunnel.sys` next to `submarine-daemon.exe`. If
  `dist/driver/submarine-split-tunnel.sys` exists, `build-windows.sh` includes it in the
  installer. In "only the chosen apps" mode the chosen apps do not reach the local network,
  and with the kill switch they are blocked outside the tunnel even if the driver fails.
  Their DNS queries still go through the system resolver (the DNS Client service), outside
  the tunnel, so the names they look up are visible to the local network's DNS server.

</details>

**Current limits:** the installer does not include the per-app tunnel driver, which is not
signed by Microsoft yet (that requires an EV certificate and Microsoft attestation signing).
To use the per-app tunnel, build the driver yourself and run it with Windows in test mode, as
described in [`third_party/win-split-tunnel/BUILDING.md`](third_party/win-split-tunnel/BUILDING.md).
Network changes are followed only for IPv4 endpoints; the installer is not signed
(SmartScreen will flag it).

## macOS

From Linux only the type-check is possible (without the Apple SDK nothing links):

```sh
scripts/macos/check.sh clippy -p submarine-daemon -- -D warnings
```

On a Mac (Xcode command line tools, Rust, Node.js):

```sh
scripts/macos/build.sh              # service, CLI and app (.app/.dmg)
sudo packaging/macos/install.sh     # launchd service it.mensys.submarine.daemon; asks who
                                    # may use it (or --all-users / --only-me)
sudo packaging/macos/uninstall.sh   # removal, kill switch rules included
```

<details>
<summary><strong>How it works on macOS</strong></summary>

- `utun` interface, routes with `route` (`/1` plus a host route to the endpoint through the
  physical gateway, updated when the network changes).
- DNS with `networksetup` on every network service, with a backup restored even after a crash
  or a reboot.
- Kill switch with a `pf` anchor (`com.apple/submarine`) that allows the encrypted traffic by
  endpoint address and port.
- The service socket is accessible to the `staff` group, that is to local users;
  `submarine-daemon access only <user>` restricts it to some of them.
- Stored tunnels encrypted with a key kept in the System keychain.
- Log in `/var/log/submarine-daemon.log`, moved to `submarine-daemon.log.old` past 10 MB.

</details>

**Current limits:** no per-app tunnel (it needs a Network Extension); recent macOS versions may
hide the Wi-Fi network name from the service, and then the trusted networks rules do not act;
app and service are neither signed nor notarized; the service is installed manually.

## Linux

Service requirements:

- `nft` (package `nftables`) for kill switch and split tunnel;
- a kernel with `CONFIG_CGROUP_NET_CLASSID` for the per-app split tunnel (cgroup `net_cls`);
- `resolvectl` if the system uses systemd-resolved (otherwise `/etc/resolv.conf` is managed);
- `iw` or NetworkManager's `nmcli` for the trusted Wi-Fi networks;
- `systemd-creds` (systemd 250 or later) to protect the key of the stored tunnels, with the
  TPM2 when present; without it the key is protected by the file permissions only.

**Known limits of the split tunnel:** apps are recognized by the path of their executable with
a scan of `/proc` every 500 ms, so the very first packets of a newly started app may take the
default route. Flatpak/Snap apps and script launchers are not supported. In "only the chosen
apps" mode the included apps use the system DNS.

## Development

```sh
cargo test                  # unit tests (the desktop app is excluded)
cargo clippy --all-targets -- -D warnings
./tests/docker/e2e.sh       # end-to-end tests in privileged containers
```

UI in the browser with a simulated service (`?mock=empty`, `?mock=offline` for the other
states):

```sh
cd apps/desktop && npm install && npm run dev   # http://localhost:5173
```

Real desktop app. On Linux it needs the Tauri system libraries:

```sh
sudo apt install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev
# in one terminal: the service (root), with the socket accessible to the submarine group
sudo ./target/debug/submarine-daemon
# in another one: the app
cd apps/desktop && npm run tauri dev
```

Contribution rules (commits, comment style, checks before committing) are in
[`AGENTS.md`](AGENTS.md).

## Project Layout

| Path | Contents |
| --- | --- |
| `crates/submarine-config` | Parser/serializer of `.conf` files (wg-quick format) |
| `crates/submarine-tunnel` | Userspace WireGuard tunnel: TUN + UDP + boringtun |
| `crates/submarine-net` | OS integration: routes, DNS, kill switch, per-app split tunnel |
| `crates/submarine-ipc` | JSON protocol between UI and service, client and transport |
| `crates/submarine-daemon` | Privileged service that manages tunnels and the network |
| `crates/submarine-cli` | Development tool for end-to-end tests (`up`, `import`, ...) |
| `apps/cli` | `submarine` CLI (TypeScript + Ink): interactive prompt and script commands |
| `apps/desktop` | Tauri 2 + React desktop app |
| `third_party/win-split-tunnel` | Windows driver for the per-app tunnel (Mullvad fork, GPL/MPL) |
| `packaging/` | systemd unit, launchd plist, macOS install scripts |
| `scripts/` | Windows build and macOS type-check from Linux (Docker) |

## License

[GPL-3.0-or-later](LICENSE). `third_party/win-split-tunnel` is dual licensed GPL-3.0 /
MPL-2.0 like its upstream.

---

<p align="center">Made with ❤️ by <a href="https://www.mensys.it/en/">Mensys</a></p>

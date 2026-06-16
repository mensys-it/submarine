//! OS integration for a running tunnel: routes, DNS, kill switch firewall and
//! per-app split tunneling.
//!
//! Every platform module (Linux, Windows, macOS, plus a fallback for the others)
//! exposes the same types (`RouteManager`, `DnsManager`, `Firewall`, `SplitTunnel`
//! and the `FWMARK` / `ROUTING_TABLE` constants), so the daemon drives them
//! without any platform-specific code of its own.

use std::io;
use std::net::{IpAddr, SocketAddr};

use ipnet::IpNet;

// platform backends: exactly one of them is compiled and re-exported
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::{DnsManager, FWMARK, Firewall, ROUTING_TABLE, RouteManager, SplitTunnel};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{
    DnsManager, FWMARK, Firewall, ROUTING_TABLE, RouteManager, SplitTunnel, remove_split_driver,
};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{DnsManager, FWMARK, Firewall, ROUTING_TABLE, RouteManager, SplitTunnel};

// platform-neutral parsing and pf ruleset of the macOS backend, also compiled in tests
// so that they are unit tested on every platform
#[cfg(any(target_os = "macos", test))]
mod macos_text;
#[cfg(any(target_os = "macos", test))]
mod pf;
// NB: outside Windows only its tests run, hence the dead_code allowance
#[cfg(any(windows, test))]
#[cfg_attr(not(windows), allow(dead_code))]
mod split_driver;

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod unsupported;
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub use unsupported::{DnsManager, FWMARK, Firewall, ROUTING_TABLE, RouteManager, SplitTunnel};

/// Errors raised while configuring the OS networking.
#[derive(Debug, thiserror::Error)]
pub enum NetError {
    /// A netlink request (Linux routes and policy rules) failed.
    #[error("netlink: {0}")]
    Netlink(String),
    /// An external tool (`nft`, `resolvectl`, `route`, `pfctl`, ...) exited with an
    /// error; `stderr` holds its trimmed error output.
    #[error("{command} failed: {stderr}")]
    Command { command: String, stderr: String },
    /// A file or process I/O error.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// A system API call (Windows) returned the error `code`.
    #[error("{call} failed with error {code:#x}")]
    System { call: &'static str, code: u32 },
    /// The split tunnel driver (Windows) failed or is not usable.
    #[error("split tunnel driver: {0}")]
    Driver(String),
    /// The requested feature is not implemented on the current platform.
    #[error("not supported on this platform yet")]
    Unsupported,
}

/// Result type of the crate, with [`NetError`] as the error.
pub type Result<T> = std::result::Result<T, NetError>;

/// Mark carried by packets of split-tunnel apps.
pub const SPLIT_MARK: u32 = 0x5376;

/// Per-app split tunneling mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SplitMode {
    /// No split tunneling: every app follows the tunnel routes.
    #[default]
    Off,
    /// Only the chosen apps use the tunnel.
    Include,
    /// Everything uses the tunnel except the chosen apps.
    Exclude,
}

/// How the tunnel's routes are laid out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RouteOptions {
    /// Split tunneling mode the routes must account for.
    pub split: SplitMode,
    /// AllowedIPs narrower than /0 win over local networks that overlap them
    /// (e.g. the tunnel reaches a remote 192.168.0.0/24 while the PC is on a
    /// LAN with the same addresses). Otherwise the system picks as usual.
    pub prefer_tunnel: bool,
}

/// Desired firewall state, recomputed by the daemon on every change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirewallPolicy {
    /// Kill switch: drop traffic that would bypass the tunnel.
    pub block: bool,
    /// Keep the local network reachable while blocking.
    pub allow_lan: bool,
    /// Interface of the running tunnel, if any.
    pub tunnel_if: Option<String>,
    /// Index of that interface (used on Windows).
    pub tunnel_index: Option<u32>,
    /// Resolved peer endpoints (used on macOS, where pf cannot tell the
    /// service's own packets apart).
    pub endpoints: Vec<SocketAddr>,
    /// Allow plain DNS while endpoint hostnames are resolved under the kill switch.
    pub allow_dns: bool,
    /// Windows: drop DNS queries that do not go through the tunnel, even
    /// without the kill switch (Windows may otherwise ask every interface).
    pub block_dns_leaks: bool,
    /// Split tunneling; only effective while a tunnel is up.
    pub split: SplitMode,
    /// DNS servers of the tunnel, reachable only through it.
    pub tunnel_dns: Vec<IpAddr>,
    /// Executables chosen for split tunneling (used on Windows).
    pub split_apps: Vec<std::path::PathBuf>,
}

/// What the OS needs to know about a tunnel to route traffic through it.
#[derive(Debug, Clone)]
pub struct TunnelNetConfig {
    /// Name of the tunnel interface.
    pub if_name: String,
    /// OS index of the tunnel interface.
    pub if_index: u32,
    /// Addresses of the tunnel interface.
    pub addresses: Vec<IpNet>,
    /// Union of every peer's AllowedIPs.
    pub allowed_ips: Vec<IpNet>,
    /// Resolved peer endpoints, which must keep using the physical network.
    pub endpoints: Vec<SocketAddr>,
    /// DNS servers to use while the tunnel is up (none: DNS is left alone).
    pub dns_servers: Vec<IpAddr>,
    /// DNS search domains to use while the tunnel is up.
    pub dns_search: Vec<String>,
}

/// Local networks that must also be routed through the tunnel so that it wins
/// over them: every `local` prefix that lies inside an AllowedIP narrower
/// than /0 and is not one already. A local prefix wider than the AllowedIP
/// needs nothing, the AllowedIP is more specific. Host routes (the machine's
/// own addresses, broadcast), link-local, loopback and multicast are never
/// overridden: they keep using the local network.
///
/// `allowed`: the tunnel's AllowedIPs.
/// `local`: destination prefixes of the routes on every interface but the tunnel.
#[cfg(any(windows, test))]
// NB: outside Windows only its tests run, hence the dead_code allowance
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn tunnel_overrides(allowed: &[IpNet], local: &[IpNet]) -> Vec<IpNet> {
    // normalization of the AllowedIPs, dropping /0 that never needs an override
    let allowed: Vec<IpNet> = allowed
        .iter()
        .map(IpNet::trunc)
        .filter(|n| n.prefix_len() > 0)
        .collect();
    let mut out: Vec<IpNet> = Vec::new();
    for net in local.iter().map(IpNet::trunc) {
        // skip of default and host routes, special ranges, the AllowedIPs themselves and
        // duplicates
        let special = match net.addr() {
            IpAddr::V4(a) => a.is_loopback() || a.is_link_local() || a.is_multicast(),
            IpAddr::V6(a) => a.is_loopback() || a.is_unicast_link_local() || a.is_multicast(),
        };
        if net.prefix_len() == 0
            || net.prefix_len() == net.max_prefix_len()
            || special
            || allowed.contains(&net)
            || out.contains(&net)
        {
            continue;
        }
        // only local networks that lie inside an AllowedIP need the override
        if allowed.iter().any(|a| a.contains(&net)) {
            out.push(net);
        }
    }
    out
}

/// Runs an external tool and returns its stdout.
///
/// # Errors
/// [`NetError::Command`] with the trimmed stderr when the tool exits with an error,
/// [`NetError::Io`] when it cannot be started.
#[cfg(target_os = "linux")]
pub(crate) async fn run(program: &str, args: &[&str]) -> Result<String> {
    let output = tokio::process::Command::new(program)
        .args(args)
        .output()
        .await?;
    if !output.status.success() {
        return Err(NetError::Command {
            command: format!("{program} {}", args.join(" ")),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses a list of prefixes.
    fn nets(list: &[&str]) -> Vec<IpNet> {
        list.iter().map(|s| s.parse().unwrap()).collect()
    }

    // only local networks inside an AllowedIP narrower than /0 are overridden
    #[test]
    fn overrides_cover_local_networks_inside_allowed_ips() {
        let allowed = nets(&["0.0.0.0/0", "192.168.0.0/16", "10.8.0.0/24"]);
        let local = nets(&[
            "0.0.0.0/0",
            "192.168.0.0/24",  // inside 192.168.0.0/16: override
            "192.168.0.10/32", // own address: kept
            "192.168.0.255/32",
            "10.0.0.0/8",    // wider than 10.8.0.0/24, which already wins
            "172.17.0.0/16", // only inside /0: kept
            "169.254.0.0/16",
            "224.0.0.0/4",
            "192.168.0.0/24", // duplicate
        ]);
        assert_eq!(
            tunnel_overrides(&allowed, &local),
            nets(&["192.168.0.0/24"])
        );
    }

    // a local network equal to an AllowedIP needs no override; link-local is ignored
    #[test]
    fn overrides_skip_allowed_ips_themselves() {
        let allowed = nets(&["192.168.0.0/24", "fd00::/8"]);
        let local = nets(&["192.168.0.0/24", "fd00:1::/64", "fe80::/64"]);
        assert_eq!(tunnel_overrides(&allowed, &local), nets(&["fd00:1::/64"]));
    }
}

//! OS integration for a running tunnel: routes, DNS, kill switch firewall and
//! per-app split tunneling.
//!
//! Every platform module (Linux, plus a fallback for the others)
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

#[cfg(not(target_os = "linux"))]
mod unsupported;
#[cfg(not(target_os = "linux"))]
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

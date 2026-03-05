//! WireGuard configuration model, compatible with the `wg-quick` `.conf` format.
//!
//! The crate parses `.conf` files into a [`TunnelConfig`] ([`parse`], [`parse_edited`]) and
//! serializes them back ([`TunnelConfig::to_conf_string`]); the daemon and the UI share these
//! types over IPC, so they are also `serde` serializable.
//!
//! Hook scripts (`PreUp`, `PostUp`, `PreDown`, `PostDown`) and other wg-quick-specific
//! directives are NEVER executed: they are dropped and reported as [`Warning`]s, since the
//! daemon runs with elevated privileges.

mod key;
mod parse;
mod write;

use std::fmt;
use std::net::{IpAddr, SocketAddr};

use ipnet::IpNet;
use serde::{Deserialize, Serialize};

pub use key::{KEY_LEN, PublicKey, SecretKey};
pub use parse::{Parsed, parse, parse_edited};

/// Placeholder that stands for a secret key in a configuration shown to the user.
pub const HIDDEN: &str = "(hidden)";

/// A complete tunnel configuration: the local interface plus its peers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelConfig {
    /// Settings of the `[Interface]` section.
    pub interface: Interface,
    /// One entry per `[Peer]` section, in file order (never empty after a parse).
    pub peers: Vec<Peer>,
}

/// The `[Interface]` section: local key, addresses and network settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Interface {
    /// Private key of the local endpoint.
    pub private_key: SecretKey,
    /// Addresses assigned to the tunnel adapter (`Address`).
    pub addresses: Vec<IpNet>,
    /// IP entries of the `DNS` directive, used as DNS servers.
    pub dns_servers: Vec<IpAddr>,
    /// Non-IP entries of the `DNS` directive, used as search domains.
    pub dns_search: Vec<String>,
    /// MTU of the tunnel adapter, if overridden.
    pub mtu: Option<u16>,
    /// Local UDP port, if fixed; otherwise a random one is used.
    pub listen_port: Option<u16>,
}

/// A `[Peer]` section: a remote WireGuard endpoint and the traffic routed to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Peer {
    /// Public key identifying the peer.
    pub public_key: PublicKey,
    /// Optional symmetric key mixed into the handshake.
    pub preshared_key: Option<SecretKey>,
    /// Networks routed through this peer (`AllowedIPs`).
    pub allowed_ips: Vec<IpNet>,
    /// Remote address of the peer, if known.
    pub endpoint: Option<Endpoint>,
    /// Keepalive interval in seconds; `None` when disabled (`0` or `off`).
    pub persistent_keepalive: Option<u16>,
}

impl Peer {
    /// True if this peer routes all IPv4 or IPv6 traffic (`0.0.0.0/0` or `::/0`).
    pub fn is_default_route(&self) -> bool {
        self.allowed_ips.iter().any(|net| net.prefix_len() == 0)
    }
}

/// The `Endpoint` of a peer, either already resolved or still a hostname.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Endpoint {
    Addr(SocketAddr),
    /// A hostname that must be resolved before connecting.
    Host {
        host: String,
        port: u16,
    },
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Endpoint::Addr(addr) => write!(f, "{addr}"),
            Endpoint::Host { host, port } => write!(f, "{host}:{port}"),
        }
    }
}

/// A non-fatal issue found while parsing (ignored or unsupported directive).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Warning {
    /// 1-based line number the warning refers to.
    pub line: usize,
    /// Human-readable description, shown to the user.
    pub message: String,
}

/// Fatal errors that make a configuration unusable.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// A key is not valid base64 or does not decode to exactly 32 bytes.
    #[error("invalid key: expected 32 bytes encoded as base64")]
    InvalidKey,
    /// The line is not a section header nor a `Key = Value` directive.
    #[error("line {line}: {message}")]
    Syntax { line: usize, message: String },
    /// A known directive has a value that cannot be parsed.
    #[error("line {line}: invalid value for {key}: {value:?}")]
    InvalidValue {
        line: usize,
        key: String,
        value: String,
    },
    /// The configuration has no `[Interface]` section.
    #[error("missing [Interface] section")]
    MissingInterface,
    /// The `[Interface]` section has no `PrivateKey`.
    #[error("[Interface] is missing PrivateKey")]
    MissingPrivateKey,
    /// The `[Peer]` section starting at `line` has no `PublicKey`.
    #[error("peer at line {line} is missing PublicKey")]
    MissingPublicKey { line: usize },
    /// The configuration has no `[Peer]` section.
    #[error("configuration has no [Peer] sections")]
    NoPeers,
}

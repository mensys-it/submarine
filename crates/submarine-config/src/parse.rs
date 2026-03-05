//! Parser for wg-quick style `.conf` files.
//!
//! Parsing is line based: section headers switch the current section and `Key = Value`
//! directives fill per-section builders, which are validated and turned into a
//! [`TunnelConfig`] at the end. Unknown or unsupported directives produce [`Warning`]s
//! instead of errors, so configurations exported by other clients still load.

use std::net::{IpAddr, SocketAddr};

use ipnet::IpNet;

use crate::{
    ConfigError, Endpoint, HIDDEN, Interface, Peer, PublicKey, SecretKey, TunnelConfig, Warning,
};

/// Result of a successful parse: the configuration plus any non-fatal issues.
#[derive(Debug, Clone)]
pub struct Parsed {
    /// The parsed configuration.
    pub config: TunnelConfig,
    /// Ignored or unsupported directives, in file order.
    pub warnings: Vec<Warning>,
}

/// Fields of the `[Interface]` section collected while parsing; the private key is
/// optional here and checked only at the end.
#[derive(Default)]
struct InterfaceBuilder {
    private_key: Option<SecretKey>,
    addresses: Vec<IpNet>,
    dns_servers: Vec<IpAddr>,
    dns_search: Vec<String>,
    mtu: Option<u16>,
    listen_port: Option<u16>,
}

/// Fields of a `[Peer]` section collected while parsing.
struct PeerBuilder {
    /// Line of the `[Peer]` header, used in error messages.
    line: usize,
    public_key: Option<PublicKey>,
    preshared_key: Option<SecretKey>,
    allowed_ips: Vec<IpNet>,
    endpoint: Option<Endpoint>,
    persistent_keepalive: Option<u16>,
    /// Line of a `PresharedKey = (hidden)`, resolved against the previous
    /// configuration once the peer's public key is known.
    hidden_preshared_key: Option<usize>,
}

/// Section the parser is currently in.
enum Section {
    /// Before the first section header.
    None,
    Interface,
    Peer,
    /// An unrecognized section, whose directives are skipped.
    Unknown,
}

/// Parses a wg-quick style configuration.
///
/// # Errors
/// Any [`ConfigError`] other than [`ConfigError::InvalidKey`]: syntax errors, invalid values
/// and missing mandatory sections or keys.
pub fn parse(input: &str) -> Result<Parsed, ConfigError> {
    parse_with(input, None)
}

/// Parses an edited version of `previous`, as produced by
/// [`TunnelConfig::to_redacted_conf_string`]: keys left as [`HIDDEN`] keep
/// their previous value (preshared keys are matched by the peer's public key).
///
/// # Errors
/// Same as [`parse`]; a hidden preshared key on a peer that had none in `previous` (or whose
/// public key was changed) is reported as [`ConfigError::InvalidValue`].
pub fn parse_edited(input: &str, previous: &TunnelConfig) -> Result<Parsed, ConfigError> {
    parse_with(input, Some(previous))
}

/// Shared implementation of [`parse`] and [`parse_edited`]; `previous` is used only to
/// resolve [`HIDDEN`] keys.
fn parse_with(input: &str, previous: Option<&TunnelConfig>) -> Result<Parsed, ConfigError> {
    let mut warnings = Vec::new();
    let mut interface: Option<InterfaceBuilder> = None;
    let mut peers: Vec<PeerBuilder> = Vec::new();
    let mut section = Section::None;

    for (idx, raw) in input.lines().enumerate() {
        // line numbers are 1-based, as shown by editors; empty and comment-only lines are skipped
        let line = idx + 1;
        let content = strip_comment(raw).trim();
        if content.is_empty() {
            continue;
        }

        // section header: names are case-insensitive, only one [Interface] is allowed
        if let Some(name) = content.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            section = match name.trim().to_ascii_lowercase().as_str() {
                "interface" => {
                    if interface.is_some() {
                        return Err(syntax(line, "duplicate [Interface] section"));
                    }
                    interface = Some(InterfaceBuilder::default());
                    Section::Interface
                }
                "peer" => {
                    peers.push(PeerBuilder::new(line));
                    Section::Peer
                }
                other => {
                    warnings.push(warn(line, format!("ignoring unknown section [{other}]")));
                    Section::Unknown
                }
            };
            continue;
        }

        // `Key = Value` directive, split on the first `=`
        let Some((key, value)) = content.split_once('=') else {
            return Err(syntax(line, "expected `Key = Value`"));
        };
        let key = key.trim();
        let value = value.trim();
        // base64 padding contains `=`, so `PrivateKey <key>=` (missing separator) would
        // otherwise split there: keys containing whitespace are rejected to catch it
        if key.is_empty() || key.contains(char::is_whitespace) {
            return Err(syntax(line, "expected `Key = Value`"));
        }

        // dispatch of the directive to the builder of the current section
        match section {
            Section::None => return Err(syntax(line, "directive outside of a section")),
            Section::Unknown => {}
            Section::Interface => {
                let iface = interface.as_mut().expect("interface section is open");
                parse_interface_key(iface, line, key, value, previous, &mut warnings)?;
            }
            Section::Peer => {
                let peer = peers.last_mut().expect("peer section is open");
                parse_peer_key(peer, line, key, value, &mut warnings)?;
            }
        }
    }

    // validation of the mandatory parts and build of the peers
    let iface = interface.ok_or(ConfigError::MissingInterface)?;
    if peers.is_empty() {
        return Err(ConfigError::NoPeers);
    }
    let peers = peers
        .into_iter()
        .map(|peer| peer.build(previous))
        .collect::<Result<_, _>>()?;

    Ok(Parsed {
        config: TunnelConfig {
            interface: Interface {
                private_key: iface.private_key.ok_or(ConfigError::MissingPrivateKey)?,
                addresses: iface.addresses,
                dns_servers: iface.dns_servers,
                dns_search: iface.dns_search,
                mtu: iface.mtu,
                listen_port: iface.listen_port,
            },
            peers,
        },
        warnings,
    })
}

/// Applies one `[Interface]` directive to the builder. Keys are case-insensitive;
/// list-valued keys may be repeated and accumulate.
fn parse_interface_key(
    iface: &mut InterfaceBuilder,
    line: usize,
    key: &str,
    value: &str,
    previous: Option<&TunnelConfig>,
    warnings: &mut Vec<Warning>,
) -> Result<(), ConfigError> {
    match key.to_ascii_lowercase().as_str() {
        // a hidden private key is accepted ONLY when editing, and keeps the previous value
        "privatekey" => {
            iface.private_key = Some(match previous {
                Some(previous) if value == HIDDEN => previous.interface.private_key.clone(),
                _ => parse_key(line, key, value)?,
            })
        }
        "address" => {
            for item in list(value) {
                iface.addresses.push(parse_ip_net(line, key, item)?);
            }
        }
        // entries that are not IP addresses are treated as search domains, like wg-quick
        "dns" => {
            for item in list(value) {
                match item.parse::<IpAddr>() {
                    Ok(ip) => iface.dns_servers.push(ip),
                    Err(_) => iface.dns_search.push(item.to_owned()),
                }
            }
        }
        "mtu" => iface.mtu = Some(parse_num(line, key, value)?),
        "listenport" => iface.listen_port = Some(parse_num(line, key, value)?),
        // hook scripts are NEVER run: the daemon has elevated privileges
        "preup" | "postup" | "predown" | "postdown" => warnings.push(warn(
            line,
            format!("{key} scripts are not supported and will not be executed"),
        )),
        // routing and persistence are handled by the daemon itself
        "table" | "fwmark" | "saveconfig" => warnings.push(warn(
            line,
            format!("{key} is managed by Submarine and was ignored"),
        )),
        _ => warnings.push(warn(
            line,
            format!("ignoring unknown [Interface] key {key}"),
        )),
    }
    Ok(())
}

/// Applies one `[Peer]` directive to the builder. Keys are case-insensitive;
/// `AllowedIPs` may be repeated and accumulates.
fn parse_peer_key(
    peer: &mut PeerBuilder,
    line: usize,
    key: &str,
    value: &str,
    warnings: &mut Vec<Warning>,
) -> Result<(), ConfigError> {
    match key.to_ascii_lowercase().as_str() {
        "publickey" => peer.public_key = Some(parse_key(line, key, value)?),
        // a hidden preshared key is only recorded here: it is resolved in `build`, once the
        // public key (which may come later in the section) is known
        "presharedkey" if value == HIDDEN => peer.hidden_preshared_key = Some(line),
        "presharedkey" => peer.preshared_key = Some(parse_key(line, key, value)?),
        "allowedips" => {
            for item in list(value) {
                peer.allowed_ips.push(parse_ip_net(line, key, item)?);
            }
        }
        "endpoint" => peer.endpoint = Some(parse_endpoint(line, key, value)?),
        // both `off` and `0` disable the keepalive
        "persistentkeepalive" => {
            peer.persistent_keepalive = if value.eq_ignore_ascii_case("off") {
                None
            } else {
                match parse_num(line, key, value)? {
                    0 => None,
                    n => Some(n),
                }
            }
        }
        _ => warnings.push(warn(line, format!("ignoring unknown [Peer] key {key}"))),
    }
    Ok(())
}

impl PeerBuilder {
    /// Creates an empty builder for the `[Peer]` section starting at `line`.
    fn new(line: usize) -> Self {
        Self {
            line,
            public_key: None,
            preshared_key: None,
            allowed_ips: Vec::new(),
            endpoint: None,
            persistent_keepalive: None,
            hidden_preshared_key: None,
        }
    }

    /// Validates the collected fields and builds the [`Peer`].
    ///
    /// # Errors
    /// [`ConfigError::MissingPublicKey`] if there is no public key;
    /// [`ConfigError::InvalidValue`] if a hidden preshared key cannot be resolved against
    /// `previous`.
    fn build(self, previous: Option<&TunnelConfig>) -> Result<Peer, ConfigError> {
        let public_key = self
            .public_key
            .ok_or(ConfigError::MissingPublicKey { line: self.line })?;
        // a hidden preshared key takes the value of the previous peer with the same public key
        let preshared_key = match self.hidden_preshared_key {
            None => self.preshared_key,
            Some(line) => Some(
                previous
                    .into_iter()
                    .flat_map(|config| &config.peers)
                    .find(|peer| peer.public_key == public_key)
                    .and_then(|peer| peer.preshared_key.clone())
                    .ok_or_else(|| invalid(line, "PresharedKey", HIDDEN))?,
            ),
        };
        Ok(Peer {
            public_key,
            preshared_key,
            allowed_ips: self.allowed_ips,
            endpoint: self.endpoint,
            persistent_keepalive: self.persistent_keepalive,
        })
    }
}

/// Removes a trailing comment: both `#` and `;` start a comment.
fn strip_comment(line: &str) -> &str {
    match line.find(['#', ';']) {
        Some(pos) => &line[..pos],
        None => line,
    }
}

/// Splits a comma-separated value, trimming items and skipping empty ones.
fn list(value: &str) -> impl Iterator<Item = &str> {
    value.split(',').map(str::trim).filter(|s| !s.is_empty())
}

/// Parses a base64 key, reporting failures as an invalid value of `key` at `line`.
fn parse_key<K: std::str::FromStr>(line: usize, key: &str, value: &str) -> Result<K, ConfigError> {
    value.parse().map_err(|_| invalid(line, key, value))
}

/// Parses a numeric value, reporting failures as an invalid value of `key` at `line`.
fn parse_num<N: std::str::FromStr>(line: usize, key: &str, value: &str) -> Result<N, ConfigError> {
    value.parse().map_err(|_| invalid(line, key, value))
}

/// Accepts both CIDR notation and bare addresses (treated as host routes).
fn parse_ip_net(line: usize, key: &str, value: &str) -> Result<IpNet, ConfigError> {
    value
        .parse::<IpNet>()
        .or_else(|_| value.parse::<IpAddr>().map(IpNet::from))
        .map_err(|_| invalid(line, key, value))
}

/// Parses an `Endpoint`: a socket address (IPv6 in brackets) or `hostname:port`.
/// Hostnames are restricted to ASCII letters, digits, `-` and `.`.
fn parse_endpoint(line: usize, key: &str, value: &str) -> Result<Endpoint, ConfigError> {
    // literal address first, so IPv6 colons are not mistaken for the port separator
    if let Ok(addr) = value.parse::<SocketAddr>() {
        return Ok(Endpoint::Addr(addr));
    }
    // otherwise hostname and port, split on the last `:`
    let (host, port) = value
        .rsplit_once(':')
        .ok_or_else(|| invalid(line, key, value))?;
    let port = port.parse().map_err(|_| invalid(line, key, value))?;
    let valid_host = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.');
    if !valid_host {
        return Err(invalid(line, key, value));
    }
    Ok(Endpoint::Host {
        host: host.to_owned(),
        port,
    })
}

/// Builds a [`ConfigError::Syntax`] for `line`.
fn syntax(line: usize, message: &str) -> ConfigError {
    ConfigError::Syntax {
        line,
        message: message.to_owned(),
    }
}

/// Builds a [`ConfigError::InvalidValue`] for `key` at `line`.
fn invalid(line: usize, key: &str, value: &str) -> ConfigError {
    ConfigError::InvalidValue {
        line,
        key: key.to_owned(),
        value: value.to_owned(),
    }
}

/// Builds a [`Warning`] for `line`.
fn warn(line: usize, message: String) -> Warning {
    Warning { line, message }
}

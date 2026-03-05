//! Serialization of a [`TunnelConfig`] back to the wg-quick `.conf` format.

use std::fmt::{self, Display, Write};

use crate::{HIDDEN, SecretKey, TunnelConfig};

/// Joins items into a comma-separated list, as used by multi-valued directives.
fn join<T: Display>(items: &[T]) -> String {
    items
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

impl TunnelConfig {
    /// Serializes back to the wg-quick `.conf` format.
    pub fn to_conf_string(&self) -> String {
        self.conf_string(false)
    }

    /// Like [`Self::to_conf_string`], with private and preshared keys written
    /// as [`HIDDEN`]: safe to show, and read back by [`crate::parse_edited`].
    pub fn to_redacted_conf_string(&self) -> String {
        self.conf_string(true)
    }

    /// Renders the configuration into a new `String`, redacting secrets if `redact` is true.
    fn conf_string(&self, redact: bool) -> String {
        let mut out = String::new();
        self.write_conf(&mut out, redact)
            .expect("writing to a String cannot fail");
        out
    }

    /// Writes the `[Interface]` section followed by one `[Peer]` section per peer.
    /// Optional directives are emitted only when set.
    fn write_conf(&self, out: &mut String, redact: bool) -> fmt::Result {
        // secret keys are replaced by the placeholder when redacting
        let secret = |key: &SecretKey| {
            if redact {
                HIDDEN.to_owned()
            } else {
                key.to_base64()
            }
        };
        // interface section
        let iface = &self.interface;
        writeln!(out, "[Interface]")?;
        writeln!(out, "PrivateKey = {}", secret(&iface.private_key))?;
        if !iface.addresses.is_empty() {
            writeln!(out, "Address = {}", join(&iface.addresses))?;
        }
        // DNS servers and search domains share the same directive, servers first
        let dns: Vec<String> = iface
            .dns_servers
            .iter()
            .map(ToString::to_string)
            .chain(iface.dns_search.iter().cloned())
            .collect();
        if !dns.is_empty() {
            writeln!(out, "DNS = {}", dns.join(", "))?;
        }
        if let Some(mtu) = iface.mtu {
            writeln!(out, "MTU = {mtu}")?;
        }
        if let Some(port) = iface.listen_port {
            writeln!(out, "ListenPort = {port}")?;
        }

        // one section per peer, separated by a blank line
        for peer in &self.peers {
            writeln!(out)?;
            writeln!(out, "[Peer]")?;
            writeln!(out, "PublicKey = {}", peer.public_key)?;
            if let Some(psk) = &peer.preshared_key {
                writeln!(out, "PresharedKey = {}", secret(psk))?;
            }
            if !peer.allowed_ips.is_empty() {
                writeln!(out, "AllowedIPs = {}", join(&peer.allowed_ips))?;
            }
            if let Some(endpoint) = &peer.endpoint {
                writeln!(out, "Endpoint = {endpoint}")?;
            }
            if let Some(keepalive) = peer.persistent_keepalive {
                writeln!(out, "PersistentKeepalive = {keepalive}")?;
            }
        }
        Ok(())
    }
}

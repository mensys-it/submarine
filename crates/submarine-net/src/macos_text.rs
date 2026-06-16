//! Parsing of macOS tool output. Platform-neutral so it is unit tested on
//! every platform.

use std::net::IpAddr;

/// Where the default route of one address family points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Gateway {
    /// A router address.
    Address(IpAddr),
    /// An interface without a router address (e.g. a point-to-point link).
    Interface(String),
}

/// Parses `route -n get default`, ignoring routes through utun interfaces
/// (the tunnel itself or another VPN).
pub(crate) fn parse_route_get(output: &str) -> Option<Gateway> {
    let field = |name: &str| {
        output
            .lines()
            .find_map(|l| l.trim().strip_prefix(name).map(|v| v.trim().to_owned()))
    };
    // no interface line: no default route at all
    let interface = field("interface:")?;
    if interface.starts_with("utun") {
        return None;
    }
    // IPv6 link-local gateways carry a scope (fe80::1%en0), which is dropped
    match field("gateway:").and_then(|g| g.split('%').next()?.parse().ok()) {
        Some(ip) => Some(Gateway::Address(ip)),
        None => Some(Gateway::Interface(interface)),
    }
}

/// Enabled services from `networksetup -listallnetworkservices`.
pub(crate) fn parse_services(output: &str) -> Vec<String> {
    // the first line is the legend "An asterisk (*) denotes that a network service is
    // disabled.", and disabled services start with that asterisk
    output
        .lines()
        .skip(1)
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('*'))
        .map(str::to_owned)
        .collect()
}

/// Output of `-getdnsservers` / `-getsearchdomains`: one value per line, or a
/// sentence when nothing is set manually.
pub(crate) fn parse_list(output: &str) -> Vec<String> {
    if output.contains("There aren't any") {
        return Vec::new();
    }
    output
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Token printed by `pfctl -E`, e.g. `Token : 7342871298475`.
pub(crate) fn parse_pf_token(output: &str) -> Option<String> {
    output.lines().find_map(|l| {
        let value = l
            .trim()
            .strip_prefix("Token")?
            .trim_start()
            .strip_prefix(':')?
            .trim();
        (!value.is_empty()).then(|| value.to_owned())
    })
}

/// DNS settings of one network service, as saved in the backup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ServiceDns {
    /// Name of the network service (e.g. `Wi-Fi`).
    pub service: String,
    /// Manually set DNS servers; empty means automatic (DHCP).
    pub servers: Vec<String>,
    /// Manually set search domains; empty means automatic (DHCP).
    pub search: Vec<String>,
}

/// One service per line: `name<TAB>servers<TAB>search`, lists comma-separated.
pub(crate) fn format_backup(entries: &[ServiceDns]) -> String {
    entries
        .iter()
        .map(|e| {
            format!(
                "{}\t{}\t{}\n",
                e.service,
                e.servers.join(","),
                e.search.join(",")
            )
        })
        .collect()
}

/// Parses the backup written by [`format_backup`], skipping malformed lines.
pub(crate) fn parse_backup(text: &str) -> Vec<ServiceDns> {
    let list = |s: &str| {
        s.split(',')
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
            .collect()
    };
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let service = parts.next().filter(|s| !s.is_empty())?.to_owned();
            Some(ServiceDns {
                service,
                servers: list(parts.next()?),
                search: list(parts.next()?),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // gateway address (with the IPv6 scope dropped), interface only, utun and no route
    #[test]
    fn default_gateway_address_or_interface() {
        let out = "   route to: default\ndestination: default\n       mask: default\n    gateway: 192.168.1.1\n  interface: en0\n      flags: <UP,GATEWAY,DONE,STATIC,PRCLONING>\n";
        assert_eq!(
            parse_route_get(out),
            Some(Gateway::Address("192.168.1.1".parse().unwrap()))
        );
        let v6 = "   route to: default\n    gateway: fe80::1%en0\n  interface: en0\n";
        assert_eq!(
            parse_route_get(v6),
            Some(Gateway::Address("fe80::1".parse().unwrap()))
        );
        let p2p = "   route to: default\n  interface: ppp0\n";
        assert_eq!(
            parse_route_get(p2p),
            Some(Gateway::Interface("ppp0".into()))
        );
        assert_eq!(
            parse_route_get("   route to: default\n    gateway: 10.0.0.1\n  interface: utun3\n"),
            None
        );
        assert_eq!(
            parse_route_get("route: writing to routing socket: not in table\n"),
            None
        );
    }

    // the legend line and the disabled services are skipped
    #[test]
    fn services_skip_header_and_disabled() {
        let out = "An asterisk (*) denotes that a network service is disabled.\nWi-Fi\n*Bluetooth PAN\nThunderbolt Bridge\n";
        assert_eq!(parse_services(out), vec!["Wi-Fi", "Thunderbolt Bridge"]);
    }

    // the "nothing set" sentence is an empty list
    #[test]
    fn dns_lists() {
        assert!(parse_list("There aren't any DNS Servers set on Wi-Fi.\n").is_empty());
        assert_eq!(parse_list("1.1.1.1\n8.8.8.8\n"), vec!["1.1.1.1", "8.8.8.8"]);
    }

    // the token is found among the other `pfctl -E` messages
    #[test]
    fn pf_token() {
        let out = "No ALTQ support in kernel\nALTQ related functions disabled\npf enabled\nToken : 7342871298475\n";
        assert_eq!(parse_pf_token(out).as_deref(), Some("7342871298475"));
        assert_eq!(parse_pf_token("pf enabled\n"), None);
    }

    // services with empty and non-empty lists survive a write and read
    #[test]
    fn backup_round_trip() {
        let entries = vec![
            ServiceDns {
                service: "Wi-Fi".into(),
                servers: vec![],
                search: vec![],
            },
            ServiceDns {
                service: "USB 10/100 LAN".into(),
                servers: vec!["1.1.1.1".into(), "9.9.9.9".into()],
                search: vec!["corp.example".into()],
            },
        ];
        assert_eq!(parse_backup(&format_backup(&entries)), entries);
    }
}

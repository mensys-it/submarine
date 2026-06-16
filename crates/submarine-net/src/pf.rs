//! pf ruleset for the macOS kill switch. Platform-neutral so it can be unit
//! tested anywhere; loaded into the `com.apple/submarine` anchor, which the
//! stock /etc/pf.conf evaluates through `anchor "com.apple/*"`.
//!
//! pf cannot match the process that sent a packet, so the tunnel's own
//! encrypted traffic is allowed by endpoint address and port instead.

use std::fmt::Write;
use std::net::SocketAddr;

use crate::FirewallPolicy;

/// IPv4 local networks: private ranges, link-local, multicast and broadcast.
const LAN_V4: &str =
    "{ 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, 169.254.0.0/16, 224.0.0.0/4, 255.255.255.255 }";
/// IPv6 local networks: link-local, unique local and multicast.
const LAN_V6: &str = "{ fe80::/10, fc00::/7, ff00::/8 }";

/// Renders the anchor ruleset for the policy: `quick` pass rules for the allowed
/// traffic, then a final block of everything else.
/// Returns `None` when nothing must be blocked.
pub(crate) fn render(p: &FirewallPolicy) -> Option<String> {
    if !p.block {
        return None;
    }
    let mut out = String::new();
    let w = &mut out;
    // loopback, the tunnel and the encrypted traffic towards the endpoints
    let _ = writeln!(w, "pass quick on lo0 all");
    if let Some(tun) = &p.tunnel_if {
        let _ = writeln!(w, "pass quick on {tun} all");
    }
    for endpoint in &p.endpoints {
        let _ = writeln!(w, "{}", endpoint_rule(endpoint));
    }
    // DHCPv4, DHCPv6 and IPv6 router/neighbor discovery keep the physical link working
    let _ = writeln!(
        w,
        "pass out quick inet proto udp from any port 68 to any port 67"
    );
    let _ = writeln!(
        w,
        "pass in quick inet proto udp from any port 67 to any port 68"
    );
    let _ = writeln!(
        w,
        "pass out quick inet6 proto udp from any port 546 to any port 547"
    );
    let _ = writeln!(
        w,
        "pass in quick inet6 proto udp from any port 547 to any port 546"
    );
    let _ = writeln!(
        w,
        "pass quick inet6 proto icmp6 all icmp6-type {{ routersol, routeradv, neighbrsol, neighbradv }}"
    );
    // optional exceptions: local networks both ways, plain DNS
    if p.allow_lan {
        let _ = writeln!(w, "pass quick inet from any to {LAN_V4}");
        let _ = writeln!(w, "pass quick inet from {LAN_V4} to any");
        let _ = writeln!(w, "pass quick inet6 from any to {LAN_V6}");
        let _ = writeln!(w, "pass quick inet6 from {LAN_V6} to any");
    }
    if p.allow_dns {
        let _ = writeln!(
            w,
            "pass out quick proto {{ udp, tcp }} from any to any port 53"
        );
    }
    // everything else is dropped
    let _ = writeln!(w, "block drop quick all");
    Some(out)
}

/// Pass rule for the encrypted UDP traffic towards one peer endpoint.
fn endpoint_rule(endpoint: &SocketAddr) -> String {
    let family = if endpoint.is_ipv4() { "inet" } else { "inet6" };
    format!(
        "pass out quick {family} proto udp from any to {} port {}",
        endpoint.ip(),
        endpoint.port()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SplitMode;

    /// Kill switch on with a tunnel up, two endpoints and no exceptions.
    fn policy() -> FirewallPolicy {
        FirewallPolicy {
            block: true,
            allow_lan: false,
            tunnel_if: Some("utun4".into()),
            tunnel_index: None,
            endpoints: vec![
                "203.0.113.10:51820".parse().unwrap(),
                "[2001:db8::1]:443".parse().unwrap(),
            ],
            allow_dns: false,
            block_dns_leaks: false,
            split: SplitMode::Off,
            tunnel_dns: vec![],
            split_apps: vec![],
        }
    }

    // no ruleset without the kill switch
    #[test]
    fn nothing_without_kill_switch() {
        assert_eq!(
            render(&FirewallPolicy {
                block: false,
                ..policy()
            }),
            None
        );
    }

    // tunnel and endpoints are allowed, everything else is blocked last
    #[test]
    fn allows_tunnel_and_endpoints_then_blocks() {
        let r = render(&policy()).unwrap();
        assert!(r.contains("pass quick on utun4 all"));
        assert!(r.contains("pass out quick inet proto udp from any to 203.0.113.10 port 51820"));
        assert!(r.contains("pass out quick inet6 proto udp from any to 2001:db8::1 port 443"));
        assert!(r.trim_end().ends_with("block drop quick all"));
        assert!(!r.contains("port 53"));
        assert!(!r.contains("192.168.0.0/16"));
    }

    // allow_lan and allow_dns add their exceptions; no tunnel rule without a tunnel
    #[test]
    fn optional_lan_and_dns() {
        let r = render(&FirewallPolicy {
            allow_lan: true,
            allow_dns: true,
            tunnel_if: None,
            ..policy()
        })
        .unwrap();
        assert!(r.contains("pass quick inet from any to { 10.0.0.0/8"));
        assert!(r.contains("pass quick inet6 from { fe80::/10"));
        assert!(r.contains("pass out quick proto { udp, tcp } from any to any port 53"));
        assert!(!r.contains("utun"));
    }
}

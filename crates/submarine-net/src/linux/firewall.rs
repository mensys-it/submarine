//! Kill switch and split-tunnel packet marking, as a single nftables table
//! (`inet submarine`) replaced atomically on every change.
//!
//! A packet must be accepted by every base chain on a hook, so our drop rules
//! hold regardless of other firewalls (ufw, firewalld, Docker).
//!
//! The table is intentionally left in place if the daemon dies: the system
//! stays blocked (fail-closed) until the daemon restarts and decides again.

use std::fmt::Write;
use std::net::IpAddr;

use tokio::io::AsyncWriteExt;

use super::split::CLASSID;
use crate::{FWMARK, FirewallPolicy, NetError, Result, SPLIT_MARK, SplitMode};

/// Family and name of our nftables table.
const TABLE: &str = "inet submarine";

/// IPv4 local networks: private ranges, link-local and multicast.
const LAN_V4: &[&str] = &[
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "169.254.0.0/16",
    "224.0.0.0/4",
];
/// IPv6 local networks: link-local, unique local and multicast.
const LAN_V6: &[&str] = &["fe80::/10", "fc00::/7", "ff00::/8"];

/// nftables backend of the kill switch and of the split tunnel marking.
pub struct Firewall;

impl Firewall {
    /// Creates the firewall; nothing is loaded until [`Firewall::apply`].
    pub fn new() -> Self {
        Self
    }

    /// Replaces our table with the rules for `policy`, or removes it when no rules
    /// are needed.
    pub async fn apply(&mut self, policy: &FirewallPolicy) -> Result<()> {
        match render(policy) {
            Some(ruleset) => {
                // creating the table first makes the delete valid on a clean system;
                // the whole script is one transaction, so there is no unprotected gap
                let script = format!("table {TABLE}\ndelete table {TABLE}\n{ruleset}");
                nft(&script).await?;
                tracing::info!(block = policy.block, split = ?policy.split, "firewall applied");
                Ok(())
            }
            None => self.reset().await,
        }
    }

    /// Removes our table, including one left by a previous run.
    pub async fn reset(&mut self) -> Result<()> {
        nft(&format!("table {TABLE}\ndelete table {TABLE}\n")).await
    }
}

impl Default for Firewall {
    fn default() -> Self {
        Self::new()
    }
}

/// Runs an nftables script through `nft -f -`.
async fn nft(script: &str) -> Result<()> {
    // the script is fed on stdin, stderr is kept for the error message
    let mut child = tokio::process::Command::new("nft")
        .args(["-f", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(script.as_bytes())
        .await?;
    let output = child.wait_with_output().await?;
    if !output.status.success() {
        return Err(NetError::Command {
            command: "nft -f -".into(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    Ok(())
}

/// Renders the whole `inet submarine` table for the policy.
/// Returns `None` when no rules are needed at all.
pub(crate) fn render(p: &FirewallPolicy) -> Option<String> {
    // split tunneling only matters while a tunnel is up
    let split = p.tunnel_if.is_some() && p.split != SplitMode::Off;
    if !p.block && !split {
        return None;
    }

    let mut out = String::new();
    let w = &mut out;
    let _ = writeln!(w, "table {TABLE} {{");

    // in include mode only the chosen apps use the VPN, so only their traffic
    // is subject to the kill switch
    let global_block = p.block && p.split != SplitMode::Include;
    let app_block = p.block && p.split == SplitMode::Include;

    if global_block {
        render_block(w, p);
    }
    if app_block {
        // postrouting, NOT output: the chosen apps are rerouted by their mark,
        // and only here does the output interface reflect the new route
        let _ = writeln!(w, "  chain app_block {{");
        let _ = writeln!(
            w,
            "    type filter hook postrouting priority filter; policy accept;"
        );
        if let Some(tun) = &p.tunnel_if {
            let _ = writeln!(w, "    meta cgroup {CLASSID:#x} oifname \"{tun}\" accept");
        }
        let _ = writeln!(w, "    meta cgroup {CLASSID:#x} oif lo accept");
        if p.allow_lan {
            lan_rules(w, "daddr", &format!("    meta cgroup {CLASSID:#x} "));
        }
        let _ = writeln!(w, "    meta cgroup {CLASSID:#x} drop");
        let _ = writeln!(w, "  }}");
    }
    if split {
        render_split(w, p);
    }

    let _ = writeln!(w, "}}");
    Some(out)
}

/// Global kill switch: output, input and forward chains with a drop policy that
/// accept only loopback, the tunnel, encrypted traffic and the optional exceptions.
fn render_block(w: &mut String, p: &FirewallPolicy) {
    let tun = p.tunnel_if.as_deref();

    // outgoing traffic of the host
    let _ = writeln!(w, "  chain output {{");
    let _ = writeln!(
        w,
        "    type filter hook output priority filter; policy drop;"
    );
    let _ = writeln!(w, "    oif lo accept");
    if let Some(tun) = tun {
        let _ = writeln!(w, "    oifname \"{tun}\" accept");
    }
    // encrypted tunnel traffic, from the socket carrying FWMARK
    let _ = writeln!(w, "    meta mark {FWMARK:#x} accept");
    // excluded apps bypass the tunnel, and with it the kill switch
    if p.split == SplitMode::Exclude && tun.is_some() {
        let _ = writeln!(w, "    meta mark {SPLIT_MARK:#x} accept");
    }
    dhcp_and_nd(w, "out");
    if p.allow_lan {
        lan_rules(w, "daddr", "    ");
    }
    if p.allow_dns {
        let _ = writeln!(w, "    udp dport 53 accept");
        let _ = writeln!(w, "    tcp dport 53 accept");
    }
    let _ = writeln!(w, "  }}");

    // incoming traffic: replies of allowed connections are accepted by conntrack
    let _ = writeln!(w, "  chain input {{");
    let _ = writeln!(
        w,
        "    type filter hook input priority filter; policy drop;"
    );
    let _ = writeln!(w, "    iif lo accept");
    if let Some(tun) = tun {
        let _ = writeln!(w, "    iifname \"{tun}\" accept");
    }
    let _ = writeln!(w, "    ct state established,related accept");
    dhcp_and_nd(w, "in");
    if p.allow_lan {
        lan_rules(w, "saddr", "    ");
    }
    let _ = writeln!(w, "  }}");

    // containers and VMs forward through the host and must not leak either
    let _ = writeln!(w, "  chain forward {{");
    let _ = writeln!(
        w,
        "    type filter hook forward priority filter; policy drop;"
    );
    if let Some(tun) = tun {
        let _ = writeln!(w, "    oifname \"{tun}\" accept");
        let _ = writeln!(w, "    iifname \"{tun}\" accept");
    }
    let _ = writeln!(w, "    ct state established,related accept");
    if p.allow_lan {
        lan_rules(w, "daddr", "    ");
    }
    let _ = writeln!(w, "  }}");
}

/// Rules that keep the physical link working while blocking: DHCPv4, DHCPv6 and
/// IPv6 neighbor/router discovery. `direction` is `"out"` or `"in"`.
fn dhcp_and_nd(w: &mut String, direction: &str) {
    if direction == "out" {
        let _ = writeln!(w, "    udp sport 68 udp dport 67 accept");
        let _ = writeln!(w, "    udp sport 546 udp dport 547 accept");
        let _ = writeln!(
            w,
            "    icmpv6 type {{ nd-router-solicit, nd-neighbor-solicit, nd-neighbor-advert }} accept"
        );
    } else {
        let _ = writeln!(w, "    udp sport 67 udp dport 68 accept");
        let _ = writeln!(w, "    udp sport 547 udp dport 546 accept");
        let _ = writeln!(
            w,
            "    icmpv6 type {{ nd-router-advert, nd-neighbor-solicit, nd-neighbor-advert }} accept"
        );
    }
}

/// Accept rules for the local networks.
///
/// `field`: the address to match, `"daddr"` or `"saddr"`.
/// `prefix`: text written before each rule (indentation and extra matches).
fn lan_rules(w: &mut String, field: &str, prefix: &str) {
    let _ = writeln!(w, "{prefix}ip {field} {{ {} }} accept", LAN_V4.join(", "));
    let _ = writeln!(w, "{prefix}ip6 {field} {{ {} }} accept", LAN_V6.join(", "));
}

/// Marks packets of processes in the split-tunnel cgroup. The `route` chain
/// type makes the kernel re-run the routing decision after the mark changes.
fn render_split(w: &mut String, p: &FirewallPolicy) {
    let tun = p.tunnel_if.as_deref().expect("split requires a tunnel");
    let _ = writeln!(w, "  chain split_mark {{");
    let _ = writeln!(
        w,
        "    type route hook output priority mangle; policy accept;"
    );
    if p.split == SplitMode::Exclude {
        // excluded apps still resolve names through the tunnel's DNS servers,
        // which are only reachable inside the tunnel: those packets stay unmarked
        let (v4, v6): (Vec<IpAddr>, Vec<IpAddr>) = p.tunnel_dns.iter().partition(|ip| ip.is_ipv4());
        for (family, addrs) in [("ip", v4), ("ip6", v6)] {
            if !addrs.is_empty() {
                let list = addrs
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                let _ = writeln!(
                    w,
                    "    meta cgroup {CLASSID:#x} {family} daddr {{ {list} }} return"
                );
            }
        }
    }
    // marking of the apps' packets, saved on the connection for the replies
    let _ = writeln!(
        w,
        "    meta cgroup {CLASSID:#x} meta mark set {SPLIT_MARK:#x}"
    );
    let _ = writeln!(w, "    meta mark {SPLIT_MARK:#x} ct mark set meta mark");
    let _ = writeln!(w, "  }}");

    // replies carry the connection's mark, so reverse-path checks use the same table
    let _ = writeln!(w, "  chain split_restore {{");
    let _ = writeln!(
        w,
        "    type filter hook prerouting priority mangle; policy accept;"
    );
    let _ = writeln!(w, "    ct mark {SPLIT_MARK:#x} meta mark set ct mark");
    let _ = writeln!(w, "  }}");

    // the source address was chosen before the mark rerouted the packet, so
    // it belongs to the other interface and must be rewritten
    let oif_match = match p.split {
        SplitMode::Exclude => format!("oifname != \"{tun}\""),
        _ => format!("oifname \"{tun}\""),
    };
    let _ = writeln!(w, "  chain split_nat {{");
    let _ = writeln!(
        w,
        "    type nat hook postrouting priority srcnat; policy accept;"
    );
    let _ = writeln!(w, "    meta mark {SPLIT_MARK:#x} {oif_match} masquerade");
    let _ = writeln!(w, "  }}");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Kill switch on with a tunnel up, no exceptions and no split tunneling.
    fn policy() -> FirewallPolicy {
        FirewallPolicy {
            block: true,
            allow_lan: false,
            tunnel_if: Some("submarine0".into()),
            tunnel_index: Some(7),
            endpoints: vec![],
            allow_dns: false,
            block_dns_leaks: false,
            split: SplitMode::Off,
            tunnel_dns: vec!["10.99.0.1".parse().unwrap()],
            split_apps: vec![],
        }
    }

    // no table at all without kill switch and without an effective split
    #[test]
    fn nothing_to_do_without_block_or_split() {
        let p = FirewallPolicy {
            block: false,
            ..policy()
        };
        assert_eq!(render(&p), None);
        // split settings are inert without a tunnel
        let p = FirewallPolicy {
            block: false,
            tunnel_if: None,
            split: SplitMode::Exclude,
            ..policy()
        };
        assert_eq!(render(&p), None);
    }

    // the kill switch lets through only the tunnel and the encrypted traffic
    #[test]
    fn kill_switch_allows_only_tunnel_and_encrypted_traffic() {
        let r = render(&policy()).unwrap();
        assert!(r.contains("hook output priority filter; policy drop;"));
        assert!(r.contains("oifname \"submarine0\" accept"));
        assert!(r.contains("meta mark 0x5375 accept"));
        assert!(r.contains("hook forward priority filter; policy drop;"));
        assert!(!r.contains("192.168.0.0/16"));
        assert!(!r.contains("dport 53"));
        assert!(!r.contains("split_mark"));
    }

    // without a tunnel there is no interface exception; DNS can be allowed
    #[test]
    fn blocked_without_tunnel_has_no_interface_exception() {
        let p = FirewallPolicy {
            tunnel_if: None,
            allow_dns: true,
            ..policy()
        };
        let r = render(&p).unwrap();
        assert!(!r.contains("submarine0"));
        assert!(r.contains("udp dport 53 accept"));
    }

    // allow_lan accepts the local networks as destination and as source
    #[test]
    fn allow_lan_adds_private_ranges_both_ways() {
        let r = render(&FirewallPolicy {
            allow_lan: true,
            ..policy()
        })
        .unwrap();
        assert!(r.contains("ip daddr { 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16"));
        assert!(r.contains("ip saddr { 10.0.0.0/8"));
        assert!(r.contains("ip6 daddr { fe80::/10, fc00::/7, ff00::/8 } accept"));
    }

    // exclude mode marks the apps (except tunnel DNS), accepts and masquerades them
    #[test]
    fn exclude_marks_apps_and_lets_them_bypass_the_kill_switch() {
        let r = render(&FirewallPolicy {
            split: SplitMode::Exclude,
            ..policy()
        })
        .unwrap();
        assert!(r.contains("meta cgroup 0x53750001 ip daddr { 10.99.0.1 } return"));
        assert!(r.contains("meta cgroup 0x53750001 meta mark set 0x5376"));
        assert!(r.contains("meta mark 0x5376 accept"));
        assert!(r.contains("meta mark 0x5376 oifname != \"submarine0\" masquerade"));
    }

    // include mode drops only the chosen apps' traffic outside the tunnel
    #[test]
    fn include_blocks_only_chosen_apps() {
        let r = render(&FirewallPolicy {
            split: SplitMode::Include,
            ..policy()
        })
        .unwrap();
        assert!(!r.contains("policy drop"));
        assert!(r.contains("hook postrouting priority filter"));
        assert!(r.contains("meta cgroup 0x53750001 oifname \"submarine0\" accept"));
        assert!(r.contains("meta cgroup 0x53750001 drop"));
        assert!(r.contains("meta mark 0x5376 oifname \"submarine0\" masquerade"));
        assert!(!r.contains("daddr { 10.99.0.1 } return"));
    }

    // include mode without a tunnel drops the chosen apps, except towards the LAN
    #[test]
    fn include_block_without_tunnel_drops_chosen_apps() {
        let p = FirewallPolicy {
            tunnel_if: None,
            split: SplitMode::Include,
            allow_lan: true,
            ..policy()
        };
        let r = render(&p).unwrap();
        assert!(r.contains("meta cgroup 0x53750001 ip daddr { 10.0.0.0/8"));
        assert!(r.contains("meta cgroup 0x53750001 drop"));
        assert!(!r.contains("split_mark"));
    }
}

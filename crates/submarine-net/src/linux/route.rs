//! Routing in the style of wg-quick:
//!
//! - AllowedIPs narrower than /0 are routed through the tunnel in the main table.
//! - A /0 AllowedIP (full tunnel) goes into a dedicated table, selected by
//!   `ip rule not fwmark FWMARK table ROUTING_TABLE`. The tunnel's own UDP
//!   socket carries FWMARK, so encrypted packets keep using the main table.
//! - `ip rule table main suppress_prefixlength 0` lets specific routes in the
//!   main table (LAN, link-local) win over the tunnel default route.
//!
//! Split tunneling adds rules for packets carrying SPLIT_MARK:
//! - exclude: marked packets use the main table, skipping the tunnel default
//!   route (routes narrower than /0 still apply to them);
//! - include: every tunnel route lives in ROUTING_TABLE and only marked
//!   packets look it up, after the main table's specific routes.
//!
//! When the tunnel is preferred, its routes narrower than /0 also live in
//! ROUTING_TABLE, and a first rule looks them up (`suppress_prefixlength 0`,
//! so the default route is left to the rules above) before the main table can
//! offer an overlapping LAN route. It applies to the packets that may use the
//! tunnel: all but the tunnel's own in full/exclude, the marked ones in include.

use futures::TryStreamExt;
use ipnet::IpNet;
use rtnetlink::packet_route::route::RouteMessage;
use rtnetlink::packet_route::rule::{RuleAction, RuleAttribute, RuleFlags, RuleMessage};
use rtnetlink::{Handle, IpVersion, RouteMessageBuilder};

use crate::{NetError, Result, RouteOptions, SPLIT_MARK, SplitMode, TunnelNetConfig};

/// Mark of the tunnel's own UDP socket (0x5375 is "Su" in ASCII).
pub const FWMARK: u32 = 0x5375;
/// Dedicated routing table of the tunnel routes.
pub const ROUTING_TABLE: u32 = 0x5375;

/// The kernel's main routing table (`RT_TABLE_MAIN`).
const MAIN_TABLE: u32 = 254;
/// Metric of the tunnel routes. Not 0, the kernel's own LAN routes use it: a
/// route with the same prefix and metric would replace them. Higher, so for
/// the same prefix the local network wins unless the tunnel is preferred.
const ROUTE_METRIC: u32 = 0x5375;

// priorities of our policy rules, looked up in increasing order: they also identify
// our rules on reset

/// Preferred tunnel: its specific routes win over the main table.
const PRIORITY_PREFER: u32 = 30_980;
/// Split tunneling: marked packets look up the main table.
const PRIORITY_SPLIT_MAIN: u32 = 30_990;
/// Include mode: marked packets look up the tunnel table.
const PRIORITY_SPLIT_TUNNEL: u32 = 30_991;
/// Specific routes of the main table win over the tunnel default route.
const PRIORITY_SUPPRESS: u32 = 31_000;
/// Full tunnel: everything but the tunnel's own packets looks up the tunnel table.
const PRIORITY_TUNNEL: u32 = 31_001;
/// Every priority above, to recognize our rules.
const OUR_PRIORITIES: [u32; 5] = [
    PRIORITY_PREFER,
    PRIORITY_SPLIT_MAIN,
    PRIORITY_SPLIT_TUNNEL,
    PRIORITY_SUPPRESS,
    PRIORITY_TUNNEL,
];

/// One policy rule (`ip rule`), independent of the address family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RuleSpec {
    // rule priority, one of OUR_PRIORITIES
    priority: u32,
    // routing table looked up by the matching packets
    table: u32,
    // mark the packets must carry to match, if any
    fwmark: Option<u32>,
    // if true, the rule matches the packets that do NOT carry `fwmark`
    invert: bool,
    // if true, a default (/0) route found in `table` is ignored (`suppress_prefixlength 0`)
    suppress_default: bool,
}

/// Policy rules for one address family. `full` is true when the tunnel has a
/// /0 route for that family, `specific` when it has narrower ones.
fn rules_for(opts: RouteOptions, full: bool, specific: bool) -> Vec<RuleSpec> {
    let mut rules = Vec::new();
    // the preferred tunnel is looked up first: by every packet but the tunnel's own,
    // or only by the marked ones in include mode
    if opts.prefer_tunnel && specific {
        let include = opts.split == SplitMode::Include;
        rules.push(RuleSpec {
            priority: PRIORITY_PREFER,
            table: ROUTING_TABLE,
            fwmark: Some(if include { SPLIT_MARK } else { FWMARK }),
            invert: !include,
            suppress_default: true,
        });
    }
    rules.extend(base_rules(opts.split, full));
    rules
}

/// Policy rules for full tunnel and split tunneling, without the preferred tunnel
/// rule (see the module docs for the layout).
fn base_rules(split: SplitMode, full: bool) -> Vec<RuleSpec> {
    let suppress = |priority, fwmark| RuleSpec {
        priority,
        table: MAIN_TABLE,
        fwmark,
        invert: false,
        suppress_default: true,
    };
    match split {
        // marked packets: specific routes of the main table first, then the tunnel
        SplitMode::Include => vec![
            suppress(PRIORITY_SPLIT_MAIN, Some(SPLIT_MARK)),
            RuleSpec {
                priority: PRIORITY_SPLIT_TUNNEL,
                table: ROUTING_TABLE,
                fwmark: Some(SPLIT_MARK),
                invert: false,
                suppress_default: false,
            },
        ],
        // without a /0 route the main table is enough
        _ if !full => vec![],
        // full tunnel, with the excluded apps sent to the main table first
        mode => {
            let mut rules = Vec::new();
            if mode == SplitMode::Exclude {
                rules.push(RuleSpec {
                    priority: PRIORITY_SPLIT_MAIN,
                    table: MAIN_TABLE,
                    fwmark: Some(SPLIT_MARK),
                    invert: false,
                    suppress_default: false,
                });
            }
            rules.push(suppress(PRIORITY_SUPPRESS, None));
            rules.push(RuleSpec {
                priority: PRIORITY_TUNNEL,
                table: ROUTING_TABLE,
                fwmark: Some(FWMARK),
                invert: true,
                suppress_default: false,
            });
            rules
        }
    }
}

/// Converts a netlink error into a [`NetError::Netlink`].
fn netlink(err: impl std::fmt::Display) -> NetError {
    NetError::Netlink(err.to_string())
}

/// Routes and policy rules of the tunnel, managed through netlink.
pub struct RouteManager {
    // handle of the netlink connection
    handle: Handle,
}

impl RouteManager {
    /// Opens the netlink connection.
    ///
    /// NB: it must be called inside a Tokio runtime, the connection task is spawned on it.
    pub fn new() -> Result<Self> {
        let (connection, handle, _) = rtnetlink::new_connection()?;
        tokio::spawn(connection);
        Ok(Self { handle })
    }

    /// Adds the routes of the AllowedIPs and the policy rules they need.
    pub async fn apply(&mut self, cfg: &TunnelNetConfig, opts: RouteOptions) -> Result<()> {
        // clearing of the leftovers of a previous run that did not shut down cleanly
        self.reset().await?;

        // one route per AllowedIP: /0 always into the tunnel table, the narrower ones
        // into the main table unless include mode or the preferred tunnel need them there
        let split = opts.split;
        let (mut full_v4, mut full_v6) = (false, false);
        let (mut specific_v4, mut specific_v6) = (false, false);
        for net in &cfg.allowed_ips {
            let net = net.trunc();
            let full = net.prefix_len() == 0;
            match net {
                IpNet::V4(_) if full => full_v4 = true,
                IpNet::V6(_) if full => full_v6 = true,
                IpNet::V4(_) => specific_v4 = true,
                IpNet::V6(_) => specific_v6 = true,
            }
            let table = if full || split == SplitMode::Include || opts.prefer_tunnel {
                ROUTING_TABLE
            } else {
                MAIN_TABLE
            };
            self.add_route(&net, cfg.if_index, table).await?;
        }

        // policy rules, computed per address family
        let v4 = rules_for(opts, full_v4, specific_v4);
        let v6 = rules_for(opts, full_v6, specific_v6);
        if !v4.is_empty() || !v6.is_empty() {
            // rp_filter must take the fwmark into account for replies
            tokio::fs::write("/proc/sys/net/ipv4/conf/all/src_valid_mark", "1").await?;
        }
        for spec in &v4 {
            self.add_rule(IpVersion::V4, spec).await?;
        }
        for spec in &v6 {
            self.add_rule(IpVersion::V6, spec).await?;
        }
        tracing::info!(
            interface = %cfg.if_name,
            full_v4,
            full_v6,
            ?split,
            prefer_tunnel = opts.prefer_tunnel,
            "routes applied"
        );
        Ok(())
    }

    /// Follows physical network changes; only needed on Windows.
    pub async fn refresh(&mut self, _cfg: &TunnelNetConfig) -> Result<Option<u32>> {
        Ok(None)
    }

    /// Removes the policy rules. Routes bound to the tunnel interface are
    /// removed by the kernel together with the interface.
    pub async fn reset(&mut self) -> Result<()> {
        for version in [IpVersion::V4, IpVersion::V6] {
            let rules: Vec<RuleMessage> = self
                .handle
                .rule()
                .get(version)
                .execute()
                .try_collect()
                .await
                .map_err(netlink)?;
            for rule in rules.into_iter().filter(is_ours) {
                self.handle
                    .rule()
                    .del(rule)
                    .execute()
                    .await
                    .map_err(netlink)?;
            }
        }
        Ok(())
    }

    /// Adds (or replaces) a route of `net` through the interface `if_index` in `table`.
    async fn add_route(&self, net: &IpNet, if_index: u32, table: u32) -> Result<()> {
        let route: RouteMessage = RouteMessageBuilder::<std::net::IpAddr>::new()
            .destination_prefix(net.addr(), net.prefix_len())
            .map_err(netlink)?
            .output_interface(if_index)
            .table_id(table)
            .priority(ROUTE_METRIC)
            .build();
        self.handle
            .route()
            .add(route)
            .replace()
            .execute()
            .await
            .map_err(netlink)
    }

    /// Adds the policy rule `spec` for one address family.
    async fn add_rule(&self, version: IpVersion, spec: &RuleSpec) -> Result<()> {
        let mut request = self
            .handle
            .rule()
            .add()
            .priority(spec.priority)
            .table_id(spec.table)
            .action(RuleAction::ToTable);
        if let Some(mark) = spec.fwmark {
            request = request.fw_mark(mark);
        }
        // inversion and suppression have no builder method: set on the raw message
        let message = request.message_mut();
        if spec.invert {
            message.header.flags |= RuleFlags::Invert;
        }
        if spec.suppress_default {
            message.attributes.push(RuleAttribute::SuppressPrefixLen(0));
        }
        match version {
            IpVersion::V4 => request.v4().execute().await,
            IpVersion::V6 => request.v6().execute().await,
        }
        .map_err(netlink)
    }
}

/// True when the rule has one of our priorities.
fn is_ours(rule: &RuleMessage) -> bool {
    rule.attributes
        .iter()
        .any(|attr| matches!(attr, RuleAttribute::Priority(p) if OUR_PRIORITIES.contains(p)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Options without the preferred tunnel.
    fn opts(split: SplitMode) -> RouteOptions {
        RouteOptions {
            split,
            prefer_tunnel: false,
        }
    }

    /// Options with the preferred tunnel.
    fn preferred(split: SplitMode) -> RouteOptions {
        RouteOptions {
            split,
            prefer_tunnel: true,
        }
    }

    // only specific routes and no include mode: the main table is enough
    #[test]
    fn no_rules_without_full_tunnel_or_split() {
        assert!(rules_for(opts(SplitMode::Off), false, true).is_empty());
        assert!(rules_for(opts(SplitMode::Exclude), false, true).is_empty());
    }

    // full tunnel: suppress rule on the main table, then the tunnel table for unmarked
    #[test]
    fn full_tunnel_rules() {
        let rules = rules_for(opts(SplitMode::Off), true, false);
        assert_eq!(rules.len(), 2);
        assert!(rules[0].suppress_default && rules[0].fwmark.is_none());
        assert!(rules[1].invert && rules[1].fwmark == Some(FWMARK));
    }

    // exclude mode: marked packets reach the main table before the tunnel rules
    #[test]
    fn exclude_sends_marked_packets_to_main_first() {
        let rules = rules_for(opts(SplitMode::Exclude), true, false);
        assert_eq!(rules[0].fwmark, Some(SPLIT_MARK));
        assert_eq!(rules[0].table, MAIN_TABLE);
        assert!(rules[0].priority < rules[1].priority);
    }

    // include mode: only marked packets match, with or without a /0 route
    #[test]
    fn include_routes_only_marked_packets() {
        let rules = rules_for(opts(SplitMode::Include), true, false);
        assert!(
            rules
                .iter()
                .all(|r| r.fwmark == Some(SPLIT_MARK) && !r.invert)
        );
        assert_eq!(rules.last().unwrap().table, ROUTING_TABLE);
        assert_eq!(rules_for(opts(SplitMode::Include), false, false), rules);
    }

    // the preferred tunnel rule comes first and leaves the other rules unchanged
    #[test]
    fn preferred_tunnel_is_looked_up_first() {
        for split in [SplitMode::Off, SplitMode::Exclude] {
            for full in [false, true] {
                let rules = rules_for(preferred(split), full, true);
                let first = rules[0];
                assert_eq!(first.priority, PRIORITY_PREFER);
                assert_eq!(first.table, ROUTING_TABLE);
                assert!(first.suppress_default && first.invert);
                assert_eq!(first.fwmark, Some(FWMARK));
                assert_eq!(rules[1..], rules_for(opts(split), full, true)[..]);
            }
        }
    }

    // in include mode the preferred tunnel rule only matches marked packets
    #[test]
    fn preferred_tunnel_in_include_mode_is_only_for_marked_packets() {
        let rules = rules_for(preferred(SplitMode::Include), false, true);
        assert_eq!(rules[0].fwmark, Some(SPLIT_MARK));
        assert!(!rules[0].invert && rules[0].suppress_default);
        assert!(rules.windows(2).all(|w| w[0].priority < w[1].priority));
    }

    // without specific routes there is nothing to prefer
    #[test]
    fn preferring_needs_specific_routes() {
        assert!(rules_for(preferred(SplitMode::Off), false, false).is_empty());
        assert_eq!(
            rules_for(preferred(SplitMode::Off), true, false),
            rules_for(opts(SplitMode::Off), true, false)
        );
    }
}

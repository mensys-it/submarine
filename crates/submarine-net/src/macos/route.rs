//! Routes in the style of wg-quick on macOS: a /0 AllowedIP becomes two /1
//! routes on the utun interface, and each endpoint gets a host route through
//! the current physical gateway so encrypted traffic stays outside the tunnel.

use std::net::IpAddr;

use ipnet::{IpNet, Ipv4Net, Ipv6Net};

use super::exec;
use crate::macos_text::{Gateway, parse_route_get};
use crate::{Result, RouteOptions, SplitMode, TunnelNetConfig};

/// Linux-only concept, kept so callers compile on every platform.
pub const FWMARK: u32 = 0;
/// Linux-only concept, kept so callers compile on every platform.
pub const ROUTING_TABLE: u32 = 0;

/// Routes of the tunnel, managed through the `route` tool.
pub struct RouteManager {
    // `route add` arguments of the tunnel routes, replayed as `route delete` on reset
    tunnel_routes: Vec<Vec<String>>,
    // `route add` arguments of the endpoint host routes, same as above
    endpoint_routes: Vec<Vec<String>>,
    // physical default gateways the endpoint routes currently go through
    gateway_v4: Option<Gateway>,
    gateway_v6: Option<Gateway>,
}

impl RouteManager {
    /// Creates a manager with no routes; never fails.
    pub fn new() -> Result<Self> {
        Ok(Self {
            tunnel_routes: Vec::new(),
            endpoint_routes: Vec::new(),
            gateway_v4: None,
            gateway_v6: None,
        })
    }

    /// Adds the endpoint host routes and the routes of the AllowedIPs through the
    /// tunnel, replacing the previous ones.
    pub async fn apply(&mut self, cfg: &TunnelNetConfig, opts: RouteOptions) -> Result<()> {
        self.reset().await?;
        // per-app split tunneling is not available (see `SplitTunnel`)
        if opts.split == SplitMode::Include {
            tracing::warn!("per-app split tunneling is not available on macOS; routing everything");
        }
        // a duplicate of a LAN route cannot be added with `route`; preferring
        // the tunnel needs another approach
        if opts.prefer_tunnel {
            tracing::warn!(
                "preferring the tunnel over the local network is not available on macOS yet"
            );
        }
        // endpoint routes first, so encrypted traffic never enters the tunnel
        self.gateway_v4 = default_gateway(false).await;
        self.gateway_v6 = default_gateway(true).await;
        self.add_endpoint_routes(cfg).await?;

        // a /0 becomes two /1 routes, which win over the default route without
        // replacing it
        for net in cfg
            .allowed_ips
            .iter()
            .flat_map(|n| expand_default(n.trunc()))
        {
            let family = if net.addr().is_ipv4() {
                "-inet"
            } else {
                "-inet6"
            };
            let args = vec![
                family.into(),
                net.to_string(),
                "-interface".into(),
                cfg.if_name.clone(),
            ];
            add(&args).await?;
            self.tunnel_routes.push(args);
        }
        tracing::info!(interface = %cfg.if_name, routes = self.tunnel_routes.len(), "routes applied");
        Ok(())
    }

    /// Follows physical network changes: if the default gateway moved, the
    /// endpoint routes are recreated through the new one.
    pub async fn refresh(&mut self, cfg: &TunnelNetConfig) -> Result<Option<u32>> {
        let (v4, v6) = (default_gateway(false).await, default_gateway(true).await);
        // nothing to do while offline (routes are kept for when the network comes
        // back) or when the gateways did not change
        let offline = v4.is_none() && v6.is_none();
        if offline || (v4 == self.gateway_v4 && v6 == self.gateway_v6) {
            return Ok(None);
        }
        tracing::info!(?v4, ?v6, "physical network changed");
        self.delete_endpoint_routes().await;
        self.gateway_v4 = v4;
        self.gateway_v6 = v6;
        self.add_endpoint_routes(cfg).await?;
        Ok(None)
    }

    /// Deletes every route added by this manager.
    pub async fn reset(&mut self) -> Result<()> {
        // utun routes vanish with the interface; delete them anyway in case
        // the tunnel stays up (e.g. when only the split mode changes)
        for args in self.tunnel_routes.drain(..) {
            delete(&args).await;
        }
        self.delete_endpoint_routes().await;
        Ok(())
    }

    /// Adds a host route for each endpoint through the physical default gateway of
    /// its family; endpoints of a family without gateway are skipped.
    async fn add_endpoint_routes(&mut self, cfg: &TunnelNetConfig) -> Result<()> {
        for endpoint in &cfg.endpoints {
            let (family, gateway, len) = match endpoint.ip() {
                IpAddr::V4(_) => ("-inet", &self.gateway_v4, 32),
                IpAddr::V6(_) => ("-inet6", &self.gateway_v6, 128),
            };
            let Some(gateway) = gateway else { continue };
            // the gateway is an address, or an interface for point-to-point links
            let mut args = vec![family.to_owned(), format!("{}/{len}", endpoint.ip())];
            match gateway {
                Gateway::Address(ip) => args.push(ip.to_string()),
                Gateway::Interface(name) => args.extend(["-interface".into(), name.clone()]),
            }
            add(&args).await?;
            self.endpoint_routes.push(args);
        }
        Ok(())
    }

    /// Deletes the endpoint host routes, ignoring the ones already gone.
    async fn delete_endpoint_routes(&mut self) {
        for args in self.endpoint_routes.drain(..) {
            delete(&args).await;
        }
    }
}

/// Runs `route add` with the given destination arguments.
async fn add(args: &[String]) -> Result<()> {
    let mut full = vec!["-q", "-n", "add"];
    full.extend(args.iter().map(String::as_str));
    exec("route", &full, None).await.map(|_| ())
}

/// Runs `route delete` for the route added with `args`, ignoring errors.
async fn delete(args: &[String]) {
    // deleting only needs the family and the destination; the route may already be gone
    let mut full = vec!["-q", "-n", "delete"];
    full.extend(args.iter().take(2).map(String::as_str));
    let _ = exec("route", &full, None).await;
}

/// Current physical default gateway of one family, `None` when there is none or it
/// is a utun interface.
async fn default_gateway(ipv6: bool) -> Option<Gateway> {
    let family = if ipv6 { "-inet6" } else { "-inet" };
    let (out, _) = exec("route", &["-n", "get", family, "default"], None)
        .await
        .ok()?;
    parse_route_get(&out)
}

/// Splits a /0 network into its two /1 halves; any other network is returned as is.
fn expand_default(net: IpNet) -> Vec<IpNet> {
    match net {
        IpNet::V4(n) if n.prefix_len() == 0 => vec![
            Ipv4Net::new([0, 0, 0, 0].into(), 1).unwrap().into(),
            Ipv4Net::new([128, 0, 0, 0].into(), 1).unwrap().into(),
        ],
        IpNet::V6(n) if n.prefix_len() == 0 => vec![
            Ipv6Net::new(std::net::Ipv6Addr::UNSPECIFIED, 1)
                .unwrap()
                .into(),
            Ipv6Net::new([0x8000, 0, 0, 0, 0, 0, 0, 0].into(), 1)
                .unwrap()
                .into(),
        ],
        other => vec![other],
    }
}

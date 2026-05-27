//! Routes on the tunnel interface. A /0 AllowedIP becomes two /1 routes,
//! which beat the physical default route without replacing it. The tunnel's
//! UDP socket is bound to the physical interface (`IP_UNICAST_IF`, set in
//! submarine-tunnel), so encrypted traffic never loops into the tunnel.
//!
//! When the tunnel is preferred, each local network inside an AllowedIP is
//! also routed through the tunnel: same prefix, lower metric, so the tunnel
//! wins. The set is kept in sync with the local networks on every refresh.

use std::mem::{MaybeUninit, offset_of};

use ipnet::{IpNet, Ipv4Net, Ipv6Net};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    CreateIpForwardEntry2, DeleteIpForwardEntry2, FreeMibTable, GetBestRoute2, GetIpForwardTable2,
    GetIpInterfaceEntry, InitializeIpForwardEntry, InitializeIpInterfaceEntry, MIB_IPFORWARD_ROW2,
    MIB_IPFORWARD_TABLE2, MIB_IPINTERFACE_ROW, SetIpInterfaceEntry,
};
use windows_sys::Win32::Networking::WinSock::{
    ADDRESS_FAMILY, AF_INET, AF_INET6, AF_UNSPEC, MIB_IPPROTO_NETMGMT,
};

use super::{check, ip_from_sockaddr, luid_from_index, sockaddr_inet};
use crate::{Result, RouteOptions, SplitMode, TunnelNetConfig};

/// Firewall mark of the tunnel's packets: a Linux-only concept, kept so
/// callers compile on every platform.
pub const FWMARK: u32 = 0;
/// Policy routing table of the tunnel: a Linux-only concept, kept so callers
/// compile on every platform.
pub const ROUTING_TABLE: u32 = 0;

/// Returned by CreateIpForwardEntry2 when the route already exists.
const ERROR_OBJECT_ALREADY_EXISTS: u32 = 5010;
/// Returned by DeleteIpForwardEntry2 when the route no longer exists.
const ERROR_NOT_FOUND: u32 = 1168;
/// Lowest interface metric: the tunnel's DNS servers and routes win ties.
const TUNNEL_METRIC: u32 = 0;
/// Include mode: the tunnel must lose against the physical network for
/// every app except the chosen ones, which the driver binds to the tunnel.
const INCLUDE_METRIC: u32 = 5000;

/// Routes of the tunnel, plus host routes that keep the endpoints on the
/// physical network. Every route it creates is remembered so it can be deleted.
pub struct RouteManager {
    /// Routes for the AllowedIPs, created by `apply`.
    created: Vec<MIB_IPFORWARD_ROW2>,
    /// Tunnel routes covering local networks, when the tunnel is preferred.
    overrides: Vec<(IpNet, MIB_IPFORWARD_ROW2)>,
    /// Whether local networks overlapping the AllowedIPs go through the tunnel.
    prefer_tunnel: bool,
    /// Host routes to the endpoints through the physical network.
    endpoint_routes: Vec<MIB_IPFORWARD_ROW2>,
    /// Interface those routes (and the tunnel socket) currently use.
    physical_index: Option<u32>,
}

impl RouteManager {
    pub fn new() -> Result<Self> {
        Ok(Self {
            created: Vec::new(),
            overrides: Vec::new(),
            prefer_tunnel: false,
            endpoint_routes: Vec::new(),
            physical_index: None,
        })
    }

    /// Follows changes of the physical network (e.g. Wi-Fi to Ethernet): when
    /// the IPv4 default route moves to another interface, the endpoint routes
    /// are recreated through it. The tunnel overrides of local networks are
    /// also brought in sync.
    ///
    /// Returns the new interface, which the tunnel socket must be pinned to,
    /// or `None` when nothing changed.
    pub async fn refresh(&mut self, cfg: &TunnelNetConfig) -> Result<Option<u32>> {
        // local networks may have appeared or disappeared
        if self.prefer_tunnel {
            self.sync_overrides(cfg)?;
        }
        // lookup of the current physical default route
        let Some(default) = physical_default_route(cfg.if_index)? else {
            // offline: keep the current state until a network appears
            return Ok(None);
        };
        if self.physical_index == Some(default.InterfaceIndex) {
            return Ok(None);
        }
        // the network changed: endpoint routes are moved to the new interface
        tracing::info!(
            interface = default.InterfaceIndex,
            "physical network changed"
        );
        self.delete_endpoint_routes()?;
        for endpoint in cfg.endpoints.iter().filter(|e| e.is_ipv4()) {
            self.add_endpoint_route(endpoint.ip(), &default)?;
        }
        self.physical_index = Some(default.InterfaceIndex);
        Ok(Some(default.InterfaceIndex))
    }

    /// Replaces the routes with those for `cfg`: interface metric, endpoint
    /// host routes, AllowedIPs routes and, when the tunnel is preferred, the
    /// overrides of local networks.
    pub async fn apply(&mut self, cfg: &TunnelNetConfig, opts: RouteOptions) -> Result<()> {
        // removal of the previous routes
        self.reset().await?;
        // interface metric, depending on the split tunnel mode
        let include = opts.split == SplitMode::Include;
        set_interface_metric(
            cfg.if_index,
            if include {
                INCLUDE_METRIC
            } else {
                TUNNEL_METRIC
            },
        )?;
        // before the tunnel routes exist: pin each endpoint to its current
        // physical route, as a fallback for the socket binding in submarine-tunnel
        for endpoint in &cfg.endpoints {
            self.pin_endpoint(endpoint.ip(), cfg.if_index)?;
        }

        // routes for the AllowedIPs, /0 split in two halves except in include mode
        for net in cfg
            .allowed_ips
            .iter()
            // include mode keeps a plain default route: with the high metric it
            // is only used by sockets bound to the tunnel address
            .flat_map(|n| {
                if include {
                    vec![n.trunc()]
                } else {
                    expand_default(n.trunc())
                }
            })
        {
            let metric = if include { INCLUDE_METRIC } else { 0 };
            if let Some(row) = add_tunnel_route(cfg.if_index, net, metric)? {
                self.created.push(row);
            }
        }
        // overrides of local networks; include mode needs none: the driver
        // binds the chosen apps to the tunnel, where they only see its routes
        self.prefer_tunnel = opts.prefer_tunnel && !include;
        if self.prefer_tunnel {
            self.sync_overrides(cfg)?;
        }
        tracing::info!(
            interface = %cfg.if_name,
            routes = self.created.len(),
            overrides = self.overrides.len(),
            "routes applied"
        );
        Ok(())
    }

    /// Routes through the tunnel the local networks that overlap its AllowedIPs,
    /// removing the overrides of networks that are gone.
    fn sync_overrides(&mut self, cfg: &TunnelNetConfig) -> Result<()> {
        let wanted = crate::tunnel_overrides(&cfg.allowed_ips, &local_prefixes(cfg.if_index)?);
        // removal of the overrides no longer wanted
        let mut kept = Vec::new();
        for (net, row) in self.overrides.drain(..) {
            if wanted.contains(&net) {
                kept.push((net, row));
            } else {
                tracing::info!(%net, "local network gone, tunnel route removed");
                delete_route(&row)?;
            }
        }
        self.overrides = kept;
        // creation of the missing ones
        for net in wanted {
            if self.overrides.iter().any(|(n, _)| *n == net) {
                continue;
            }
            if let Some(row) = add_tunnel_route(cfg.if_index, net, 0)? {
                tracing::info!(%net, "local network routed through the tunnel");
                self.overrides.push((net, row));
            }
        }
        Ok(())
    }

    /// Adds a host route for the endpoint `ip` through its current best route,
    /// unless that route already goes through the tunnel. The first interface
    /// found becomes the tracked physical interface.
    fn pin_endpoint(&mut self, ip: std::net::IpAddr, tunnel_index: u32) -> Result<()> {
        // lookup of the best route to the endpoint
        let dest = sockaddr_inet(ip);
        let mut best = MaybeUninit::<MIB_IPFORWARD_ROW2>::zeroed();
        let mut source = Default::default();
        // SAFETY: valid destination and out pointers; zero LUID/index means any interface.
        let code = unsafe {
            GetBestRoute2(
                std::ptr::null(),
                0,
                std::ptr::null(),
                &dest,
                0,
                best.as_mut_ptr(),
                &mut source,
            )
        };
        check("GetBestRoute2", code)?;
        // SAFETY: filled by GetBestRoute2.
        let best = unsafe { fixed_row(best.as_mut_ptr()) };
        // already through the tunnel: nothing to pin
        if best.InterfaceIndex == tunnel_index {
            return Ok(());
        }
        self.physical_index.get_or_insert(best.InterfaceIndex);
        self.add_endpoint_route(ip, &best)
    }

    /// Adds a host route for `ip` with the interface and next hop of `via`.
    fn add_endpoint_route(&mut self, ip: std::net::IpAddr, via: &MIB_IPFORWARD_ROW2) -> Result<()> {
        let created = create_route(|row| {
            // SAFETY: `create_route` passes a valid, writable row.
            unsafe {
                (*row).InterfaceLuid = via.InterfaceLuid;
                (*row).DestinationPrefix.Prefix = sockaddr_inet(ip);
                (*row).DestinationPrefix.PrefixLength = if ip.is_ipv4() { 32 } else { 128 };
                (*row).NextHop = via.NextHop;
                (*row).Metric = 0;
            }
        })?;
        self.endpoint_routes.extend(created);
        Ok(())
    }

    /// Deletes the host routes to the endpoints.
    fn delete_endpoint_routes(&mut self) -> Result<()> {
        for row in self.endpoint_routes.drain(..) {
            delete_route(&row)?;
        }
        Ok(())
    }

    /// Deletes the routes we created. Windows also drops them when the
    /// tunnel adapter disappears.
    pub async fn reset(&mut self) -> Result<()> {
        for row in self.created.drain(..) {
            delete_route(&row)?;
        }
        for (_, row) in self.overrides.drain(..) {
            delete_route(&row)?;
        }
        self.prefer_tunnel = false;
        self.delete_endpoint_routes()?;
        self.physical_index = None;
        Ok(())
    }
}

/// Creates an on-link route for `net` through the tunnel with the given
/// metric. Returns the row to delete it with, `None` if it already exists.
fn add_tunnel_route(if_index: u32, net: IpNet, metric: u32) -> Result<Option<MIB_IPFORWARD_ROW2>> {
    let luid = luid_from_index(if_index)?;
    create_route(|row| {
        // SAFETY: `create_route` passes a valid, writable row.
        unsafe {
            (*row).InterfaceLuid = luid;
            (*row).DestinationPrefix.Prefix = sockaddr_inet(net.addr());
            (*row).DestinationPrefix.PrefixLength = net.prefix_len();
            // unspecified next hop of the matching family: on-link through the tunnel
            (*row).NextHop = sockaddr_inet(match net {
                IpNet::V4(_) => std::net::Ipv4Addr::UNSPECIFIED.into(),
                IpNet::V6(_) => std::net::Ipv6Addr::UNSPECIFIED.into(),
            });
            (*row).Metric = metric;
        }
    })
}

/// Creates a route: `fill` sets its fields over InitializeIpForwardEntry's
/// defaults. Returns the row to delete it with, `None` if it already existed.
fn create_route(fill: impl FnOnce(*mut MIB_IPFORWARD_ROW2)) -> Result<Option<MIB_IPFORWARD_ROW2>> {
    // filled through a raw pointer: InitializeIpForwardEntry stores 0xFF in
    // the BOOLEANs, which must reach CreateIpForwardEntry2 unchanged but are
    // NOT valid `bool`s (see `fixed_row`)
    let mut row = MaybeUninit::<MIB_IPFORWARD_ROW2>::zeroed();
    let ptr = row.as_mut_ptr();
    // SAFETY: `ptr` points to a writable row.
    unsafe { InitializeIpForwardEntry(ptr) };
    // fields set by the caller, then marked as a static route
    fill(ptr);
    // SAFETY: `ptr` points to a writable row.
    unsafe { (*ptr).Protocol = MIB_IPPROTO_NETMGMT };
    // SAFETY: the row is fully initialized.
    let code = unsafe { CreateIpForwardEntry2(ptr) };
    // an existing route is not ours, so it is not returned for deletion
    if code == ERROR_OBJECT_ALREADY_EXISTS {
        return Ok(None);
    }
    check("CreateIpForwardEntry2", code)?;
    // SAFETY: initialized above.
    Ok(Some(unsafe { fixed_row(ptr) }))
}

/// Reads a row Windows wrote. windows-sys types its BOOLEANs as `bool`,
/// but Windows stores any byte there (InitializeIpForwardEntry writes 0xFF),
/// and a `bool` other than 0 or 1 is undefined behaviour: the compiler uses
/// those values to encode enum variants, so a created route wrapped in
/// `Result<Option<_>>` came back as an `Err` made of garbage, and formatting
/// it crashed the service. They are normalized here, before Rust reads them.
///
/// # Safety
/// `row` must point to a writable, initialized MIB_IPFORWARD_ROW2 (BOOLEANs
/// aside).
unsafe fn fixed_row(row: *mut MIB_IPFORWARD_ROW2) -> MIB_IPFORWARD_ROW2 {
    // every BOOLEAN is forced to 0 or 1 through a byte pointer
    let base = row.cast::<u8>();
    for offset in [
        offset_of!(MIB_IPFORWARD_ROW2, Loopback),
        offset_of!(MIB_IPFORWARD_ROW2, AutoconfigureAddress),
        offset_of!(MIB_IPFORWARD_ROW2, Publish),
        offset_of!(MIB_IPFORWARD_ROW2, Immortal),
    ] {
        // SAFETY: a byte inside the row, accessed as a plain u8.
        unsafe {
            let byte = base.add(offset);
            *byte = u8::from(*byte != 0);
        }
    }
    // SAFETY: every field now holds a valid value.
    unsafe { row.read() }
}

/// Returns a copy of the system's routes for `family`, normalized by
/// `fixed_row`.
fn forward_table(family: ADDRESS_FAMILY) -> Result<Vec<MIB_IPFORWARD_ROW2>> {
    let mut table: *mut MIB_IPFORWARD_TABLE2 = std::ptr::null_mut();
    // SAFETY: valid out pointer; the table is freed below.
    check("GetIpForwardTable2", unsafe {
        GetIpForwardTable2(family, &mut table)
    })?;
    // SAFETY: the API returned a table with `NumEntries` rows, ours to
    // modify until it is freed; no reference to them is created before
    // `fixed_row` has normalized them.
    let rows = unsafe {
        let first = (&raw mut (*table).Table).cast::<MIB_IPFORWARD_ROW2>();
        (0..(*table).NumEntries as usize)
            .map(|i| fixed_row(first.add(i)))
            .collect()
    };
    // SAFETY: allocated by GetIpForwardTable2.
    unsafe { FreeMibTable(table.cast()) };
    Ok(rows)
}

/// Destination prefixes of the routes on every interface but the tunnel.
fn local_prefixes(tunnel_index: u32) -> Result<Vec<IpNet>> {
    Ok(forward_table(AF_UNSPEC)?
        .iter()
        .filter(|r| r.InterfaceIndex != tunnel_index)
        .filter_map(|r| {
            let ip = ip_from_sockaddr(&r.DestinationPrefix.Prefix)?;
            IpNet::new(ip, r.DestinationPrefix.PrefixLength).ok()
        })
        .collect())
}

/// Deletes a route created by us; a route that no longer exists is not an
/// error.
fn delete_route(row: &MIB_IPFORWARD_ROW2) -> Result<()> {
    // SAFETY: `row` was accepted by CreateIpForwardEntry2.
    let code = unsafe { DeleteIpForwardEntry2(row) };
    if code == ERROR_NOT_FOUND {
        // already gone, e.g. with its interface
        return Ok(());
    }
    check("DeleteIpForwardEntry2", code)
}

/// Returns the IPv4 default route with the lowest effective metric (route
/// metric plus interface metric) among connected interfaces, ignoring the
/// tunnel; `None` when offline.
pub(super) fn physical_default_route(tunnel_index: u32) -> Result<Option<MIB_IPFORWARD_ROW2>> {
    Ok(forward_table(AF_INET)?
        .into_iter()
        .filter(|r| r.DestinationPrefix.PrefixLength == 0 && r.InterfaceIndex != tunnel_index)
        .filter_map(|r| interface_metric(&r).map(|m| (r.Metric + m, r)))
        .min_by_key(|(metric, _)| *metric)
        .map(|(_, row)| row))
}

/// Metric of the route's interface, or `None` if it is not connected.
fn interface_metric(route: &MIB_IPFORWARD_ROW2) -> Option<u32> {
    let mut row = MIB_IPINTERFACE_ROW::default();
    // SAFETY: `row` is a valid, writable MIB_IPINTERFACE_ROW.
    unsafe { InitializeIpInterfaceEntry(&mut row) };
    row.Family = AF_INET;
    row.InterfaceLuid = route.InterfaceLuid;
    // SAFETY: `row` identifies the interface by family and LUID.
    let ok = unsafe { GetIpInterfaceEntry(&mut row) } == 0;
    (ok && row.Connected).then_some(row.Metric)
}

/// Splits a /0 network into two /1 halves, which beat the physical default
/// route without replacing it; other networks are returned as they are.
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

/// Sets a fixed metric on the interface for both IPv4 and IPv6, skipping the
/// families that are not enabled on it.
fn set_interface_metric(if_index: u32, metric: u32) -> Result<()> {
    for family in [AF_INET, AF_INET6] {
        let mut row = MIB_IPINTERFACE_ROW::default();
        // SAFETY: `row` is a valid, writable MIB_IPINTERFACE_ROW.
        unsafe { InitializeIpInterfaceEntry(&mut row) };
        row.Family = family;
        row.InterfaceLuid = luid_from_index(if_index)?;
        // SAFETY: `row` identifies the interface by family and LUID.
        if unsafe { GetIpInterfaceEntry(&mut row) } != 0 {
            // family not enabled on the adapter
            continue;
        }
        row.UseAutomaticMetric = false;
        row.Metric = metric;
        // NB: must be zero when setting an IPv4 interface
        row.SitePrefixLength = 0;
        // SAFETY: `row` was filled by GetIpInterfaceEntry.
        check("SetIpInterfaceEntry", unsafe {
            SetIpInterfaceEntry(&mut row)
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // /0 routes of both families are split in two halves, others are kept
    #[test]
    fn default_routes_become_two_halves() {
        let halves = expand_default("0.0.0.0/0".parse().unwrap());
        assert_eq!(
            halves,
            vec![
                "0.0.0.0/1".parse().unwrap(),
                "128.0.0.0/1".parse::<IpNet>().unwrap()
            ]
        );
        let v6 = expand_default("::/0".parse().unwrap());
        assert_eq!(
            v6,
            vec![
                "::/1".parse().unwrap(),
                "8000::/1".parse::<IpNet>().unwrap()
            ]
        );
        assert_eq!(expand_default("10.0.0.0/8".parse().unwrap()).len(), 1);
    }
}

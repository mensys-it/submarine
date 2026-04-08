//! Linux backend: policy routing through netlink, DNS through systemd-resolved or
//! `/etc/resolv.conf`, kill switch and split tunnel marking through nftables, and
//! per-app tracking through a `net_cls` cgroup.

mod dns;
mod firewall;
mod route;
mod split;

pub use dns::DnsManager;
pub use firewall::Firewall;
pub use route::{FWMARK, ROUTING_TABLE, RouteManager};
pub use split::SplitTunnel;

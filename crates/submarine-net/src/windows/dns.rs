//! DNS servers on the tunnel interface. With the lowest interface metric,
//! Windows prefers them over the physical adapters' servers.

use windows_sys::Win32::NetworkManagement::IpHelper::{
    ConvertInterfaceLuidToGuid, DNS_INTERFACE_SETTINGS, DNS_INTERFACE_SETTINGS_VERSION1,
    DNS_SETTING_IPV6, DNS_SETTING_NAMESERVER, DNS_SETTING_SEARCHLIST, SetInterfaceDnsSettings,
};
use windows_sys::core::GUID;

use super::{check, luid_from_index, wide};
use crate::{Result, TunnelNetConfig};

/// Sets and clears the DNS servers and search list of the tunnel interface.
pub struct DnsManager {
    /// GUID of the interface whose settings we changed, if any.
    active: Option<GUID>,
}

impl DnsManager {
    pub fn new() -> Self {
        Self { active: None }
    }

    /// Replaces any previous settings with the servers and search domains of
    /// `cfg`. IPv4 and IPv6 servers are set separately, as the API requires.
    pub async fn apply(&mut self, cfg: &TunnelNetConfig) -> Result<()> {
        // removal of the previous settings, then nothing else to do without servers
        self.reset().await?;
        if cfg.dns_servers.is_empty() {
            return Ok(());
        }
        // the DNS settings API identifies the interface by GUID
        let luid = luid_from_index(cfg.if_index)?;
        let mut guid = GUID::from_u128(0);
        // SAFETY: valid in and out pointers.
        check("ConvertInterfaceLuidToGuid", unsafe {
            ConvertInterfaceLuidToGuid(&luid, &mut guid)
        })?;

        // one comma-separated list per family, with the same search list
        let (v4, v6): (Vec<&std::net::IpAddr>, Vec<&std::net::IpAddr>) =
            cfg.dns_servers.iter().partition(|ip| ip.is_ipv4());
        let search = cfg.dns_search.join(",");
        for (servers, ipv6) in [(v4, false), (v6, true)] {
            if servers.is_empty() {
                continue;
            }
            let list = servers
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",");
            set_dns(guid, &list, &search, ipv6)?;
        }
        self.active = Some(guid);
        tracing::info!(servers = ?cfg.dns_servers, "dns applied");
        Ok(())
    }

    /// Clears the settings applied by [`apply`](Self::apply). Errors are
    /// ignored, so it never fails.
    pub async fn reset(&mut self) -> Result<()> {
        if let Some(guid) = self.active.take() {
            // the adapter may already be gone together with its settings
            let _ = set_dns(guid, "", "", false);
            let _ = set_dns(guid, "", "", true);
        }
        Ok(())
    }
}

impl Default for DnsManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Sets the name servers and search list of one address family on an
/// interface; empty strings clear them.
fn set_dns(interface: GUID, servers: &str, search: &str, ipv6: bool) -> Result<()> {
    let mut servers = wide(servers);
    let mut search = wide(search);
    let mut flags = (DNS_SETTING_NAMESERVER | DNS_SETTING_SEARCHLIST) as u64;
    if ipv6 {
        flags |= DNS_SETTING_IPV6 as u64;
    }
    let settings = DNS_INTERFACE_SETTINGS {
        Version: DNS_INTERFACE_SETTINGS_VERSION1,
        Flags: flags,
        NameServer: servers.as_mut_ptr(),
        SearchList: search.as_mut_ptr(),
        ..Default::default()
    };
    // SAFETY: the strings outlive the call.
    check("SetInterfaceDnsSettings", unsafe {
        SetInterfaceDnsSettings(interface, &settings)
    })
}

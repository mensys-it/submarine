//! Windows backend: routes and DNS through IP Helper, kill switch through the
//! Windows Filtering Platform, per-app split tunneling through our kernel
//! driver (third_party/win-split-tunnel).
//!
//! This module also holds the small helpers shared by the submodules: error
//! mapping of Win32 return codes, LUID lookup and `SOCKADDR_INET` conversions.

mod dns;
mod firewall;
mod route;
mod split;

use std::net::IpAddr;

use windows_sys::Win32::NetworkManagement::IpHelper::ConvertInterfaceIndexToLuid;
use windows_sys::Win32::NetworkManagement::Ndis::NET_LUID_LH;
use windows_sys::Win32::Networking::WinSock::{
    AF_INET, AF_INET6, IN_ADDR, IN_ADDR_0, IN6_ADDR, IN6_ADDR_0, SOCKADDR_IN, SOCKADDR_IN6,
    SOCKADDR_INET,
};

pub use dns::DnsManager;
pub use firewall::Firewall;
pub use route::{FWMARK, ROUTING_TABLE, RouteManager};
pub use split::{SplitTunnel, remove_driver as remove_split_driver};

use crate::{NetError, Result};

/// Success code returned by the IP Helper functions.
const NO_ERROR: u32 = 0;

/// Turns a Win32 return code into a `Result`, naming the failed `call`.
fn check(call: &'static str, code: u32) -> Result<()> {
    if code == NO_ERROR {
        Ok(())
    } else {
        Err(NetError::System { call, code })
    }
}

/// Converts an interface index into its LUID, the identifier most IP Helper
/// functions expect.
fn luid_from_index(index: u32) -> Result<NET_LUID_LH> {
    let mut luid = NET_LUID_LH::default();
    // SAFETY: `luid` is a valid out pointer.
    check("ConvertInterfaceIndexToLuid", unsafe {
        ConvertInterfaceIndexToLuid(index, &mut luid)
    })?;
    Ok(luid)
}

/// Builds a `SOCKADDR_INET` (port 0) holding `ip`.
fn sockaddr_inet(ip: IpAddr) -> SOCKADDR_INET {
    match ip {
        IpAddr::V4(v4) => SOCKADDR_INET {
            Ipv4: SOCKADDR_IN {
                sin_family: AF_INET,
                sin_addr: IN_ADDR {
                    // network byte order, as in the C struct
                    S_un: IN_ADDR_0 {
                        S_addr: u32::from_ne_bytes(v4.octets()),
                    },
                },
                ..Default::default()
            },
        },
        IpAddr::V6(v6) => SOCKADDR_INET {
            Ipv6: SOCKADDR_IN6 {
                sin6_family: AF_INET6,
                sin6_addr: IN6_ADDR {
                    u: IN6_ADDR_0 { Byte: v6.octets() },
                },
                ..Default::default()
            },
        },
    }
}

/// Reads the address of a `SOCKADDR_INET`; `None` for families other than
/// IPv4 and IPv6.
fn ip_from_sockaddr(addr: &SOCKADDR_INET) -> Option<IpAddr> {
    // SAFETY: the family tag selects the valid union member.
    unsafe {
        match addr.si_family {
            AF_INET => Some(IpAddr::from(addr.Ipv4.sin_addr.S_un.S_addr.to_ne_bytes())),
            AF_INET6 => Some(IpAddr::from(addr.Ipv6.sin6_addr.u.Byte)),
            _ => None,
        }
    }
}

/// Converts a string into NUL-terminated UTF-16 for Windows APIs.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

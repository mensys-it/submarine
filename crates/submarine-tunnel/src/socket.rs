//! UDP socket shared by all the peers of a tunnel.

use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6};

use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;

/// UDP socket used to talk to peers. Prefers a dual-stack IPv6 socket and
/// falls back to IPv4-only on hosts without IPv6.
pub(crate) struct PeerSocket {
    socket: UdpSocket,
    /// True for an IPv6 socket that also carries IPv4 (as IPv4-mapped addresses).
    dual_stack: bool,
}

impl PeerSocket {
    /// Binds to `port` on all addresses (`0` for a random port), setting `fwmark` on Linux.
    pub fn bind(port: u16, fwmark: Option<u32>) -> io::Result<Self> {
        match Self::bind_domain(Domain::IPV6, port, fwmark) {
            Ok(socket) => Ok(Self {
                socket,
                dual_stack: true,
            }),
            Err(err) => {
                tracing::debug!("dual-stack UDP socket unavailable ({err}), using IPv4 only");
                let socket = Self::bind_domain(Domain::IPV4, port, fwmark)?;
                Ok(Self {
                    socket,
                    dual_stack: false,
                })
            }
        }
    }

    /// Creates a non-blocking UDP socket of the given family; IPv6 sockets are dual-stack.
    fn bind_domain(domain: Domain, port: u16, fwmark: Option<u32>) -> io::Result<UdpSocket> {
        let socket = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP))?;
        let addr: SocketAddr = if domain == Domain::IPV6 {
            socket.set_only_v6(false)?;
            (Ipv6Addr::UNSPECIFIED, port).into()
        } else {
            (Ipv4Addr::UNSPECIFIED, port).into()
        };
        // the mark lets routing rules send the encrypted traffic outside the tunnel
        #[cfg(target_os = "linux")]
        if let Some(mark) = fwmark {
            socket.set_mark(mark)?;
        }
        #[cfg(not(target_os = "linux"))]
        let _ = fwmark;
        socket.set_nonblocking(true)?;
        socket.bind(&addr.into())?;
        UdpSocket::from_std(socket.into())
    }

    /// Windows: sends through the interface that currently routes to `dest`,
    /// so the tunnel's own routes cannot capture the encrypted traffic.
    /// Elsewhere this is handled by routing rules and is a no-op.
    #[cfg(windows)]
    pub fn pin_to_current_route(&self, dest: SocketAddr) -> io::Result<()> {
        use windows_sys::Win32::NetworkManagement::IpHelper::GetBestInterfaceEx;
        use windows_sys::Win32::Networking::WinSock::SOCKADDR;

        let addr = socket2::SockAddr::from(dest);
        let mut index = 0u32;
        // SAFETY: `addr` is a valid socket address, `index` a valid out pointer.
        let rc = unsafe { GetBestInterfaceEx(addr.as_ptr() as *const SOCKADDR, &mut index) };
        if rc != 0 {
            return Err(io::Error::from_raw_os_error(rc as i32));
        }
        self.pin_to_interface(index, dest.is_ipv6())?;
        tracing::info!(%dest, interface = index, "tunnel socket pinned to interface");
        Ok(())
    }

    /// Windows: sends through the given interface. `v6` tells whether IPv6
    /// peers must be pinned too; for IPv4 peers a failure there (e.g. the
    /// interface has no IPv6) is harmless.
    #[cfg(windows)]
    pub fn pin_to_interface(&self, index: u32, v6: bool) -> io::Result<()> {
        use std::os::windows::io::AsRawSocket;

        use windows_sys::Win32::Networking::WinSock::{
            IP_UNICAST_IF, IPPROTO_IP, IPPROTO_IPV6, IPV6_UNICAST_IF, SOCKET, setsockopt,
        };

        let raw = self.socket.as_raw_socket() as SOCKET;
        let set = |level, name, value: u32, option: &str| {
            // SAFETY: `value` is a 4-byte option value that outlives the call.
            let rc = unsafe { setsockopt(raw, level, name, (&value as *const u32).cast(), 4) };
            if rc == 0 {
                Ok(())
            } else {
                let err = io::Error::last_os_error();
                Err(io::Error::new(err.kind(), format!("{option}: {err}")))
            }
        };
        // NB: IPv4 takes the index in network byte order, IPv6 in host order
        set(IPPROTO_IP, IP_UNICAST_IF, index.to_be(), "IP_UNICAST_IF")?;
        // the IPv6 option is set only on dual-stack sockets and is fatal only for IPv6 peers
        if self.dual_stack
            && let Err(err) = set(IPPROTO_IPV6, IPV6_UNICAST_IF, index, "IPV6_UNICAST_IF")
        {
            if v6 {
                return Err(err);
            }
            tracing::debug!(interface = index, "IPv6 traffic not pinned: {err}");
        }
        Ok(())
    }

    /// No-op: on other platforms the encrypted traffic is kept out of the tunnel by
    /// routing rules.
    #[cfg(not(windows))]
    pub fn pin_to_current_route(&self, _dest: SocketAddr) -> io::Result<()> {
        Ok(())
    }

    /// No-op outside Windows.
    #[cfg(not(windows))]
    pub fn pin_to_interface(&self, _index: u32, _v6: bool) -> io::Result<()> {
        Ok(())
    }

    /// Local port the socket is bound to.
    pub fn local_port(&self) -> io::Result<u16> {
        Ok(self.socket.local_addr()?.port())
    }

    /// Sends a datagram; on a dual-stack socket IPv4 destinations are converted to
    /// IPv4-mapped IPv6 addresses.
    pub async fn send_to(&self, buf: &[u8], addr: SocketAddr) -> io::Result<usize> {
        let target = match addr {
            SocketAddr::V4(v4) if self.dual_stack => {
                SocketAddr::V6(SocketAddrV6::new(v4.ip().to_ipv6_mapped(), v4.port(), 0, 0))
            }
            other => other,
        };
        self.socket.send_to(buf, target).await
    }

    /// Receives a datagram; IPv4-mapped source addresses are returned as IPv4.
    pub async fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        let (len, addr) = self.socket.recv_from(buf).await?;
        let addr = SocketAddr::new(addr.ip().to_canonical(), addr.port());
        Ok((len, addr))
    }
}

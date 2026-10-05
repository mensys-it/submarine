//! Userspace WireGuard tunnel: a TUN device bridged to peers through boringtun.
//!
//! A [`Tunnel`] owns the TUN device and a single UDP socket shared by all peers, and runs
//! three tasks: outbound packets (TUN to UDP), inbound datagrams (UDP to TUN) and the
//! WireGuard timers (handshakes, keepalives).
//!
//! This crate only moves packets. Routes, DNS and firewall rules are the
//! responsibility of `submarine-net`.

mod peer;
mod socket;

use std::io;
use std::net::{IpAddr, SocketAddr};
use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use boringtun::noise::handshake::parse_handshake_anon;
use boringtun::noise::rate_limiter::RateLimiter;
use boringtun::noise::{Packet, Tunn, TunnResult};
use boringtun::x25519;
use ipnet::IpNet;
use submarine_config::{Endpoint, PublicKey, TunnelConfig};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tun_rs::{AsyncDevice, DeviceBuilder};

use peer::{Peer, Router};
use socket::PeerSocket;

/// MTU of the TUN device when the configuration does not set one (same as wg-quick).
pub const DEFAULT_MTU: u16 = 1420;
/// Interval at which the WireGuard timers of every peer are updated.
const TIMER_TICK: Duration = Duration::from_millis(250);
/// Size of the packet buffers: the largest possible IP packet / UDP datagram.
const BUF_SIZE: usize = 65536;
/// Message type of a cookie reply, the first byte of the datagram.
const COOKIE_REPLY: u8 = 3;
/// Handshake messages per second, across all peers, above which a valid cookie
/// (proof that the sender receives at its source address) is required.
/// NB: every handshake message is counted twice, by [`udp_loop`] and again by boringtun
/// inside `decapsulate`, so the limiter is created with twice this value.
const HANDSHAKE_RATE_LIMIT: u64 = 100;

/// Errors that prevent a tunnel from starting.
#[derive(Debug, thiserror::Error)]
pub enum TunnelError {
    /// The TUN device cannot be created or configured.
    #[error("failed to create TUN device: {0}")]
    Device(#[source] io::Error),
    /// The UDP socket towards the peers cannot be bound.
    #[error("failed to bind UDP socket: {0}")]
    Socket(#[source] io::Error),
    /// A peer hostname cannot be resolved.
    #[error("failed to resolve endpoint {endpoint}: {source}")]
    Resolve {
        endpoint: String,
        #[source]
        source: io::Error,
    },
}

/// Platform-specific settings for [`Tunnel::start`], not part of the `.conf` file.
#[derive(Debug, Clone, Default)]
pub struct TunnelOptions {
    /// Interface name. Ignored on macOS, where the kernel assigns `utunN`.
    pub name: Option<String>,
    /// Linux only: fwmark set on the UDP socket so that encrypted traffic
    /// bypasses the tunnel routing table.
    pub fwmark: Option<u32>,
    /// Endpoints already resolved with [`resolve_endpoints`], one per peer.
    /// Resolved again when absent.
    pub endpoints: Option<Vec<Option<SocketAddr>>>,
}

/// Resolves every peer's endpoint (`None` for peers without one).
/// Hostnames resolving to several addresses prefer IPv4.
///
/// # Errors
/// [`TunnelError::Resolve`] for the first endpoint that cannot be resolved.
pub async fn resolve_endpoints(
    config: &TunnelConfig,
) -> Result<Vec<Option<SocketAddr>>, TunnelError> {
    let mut endpoints = Vec::with_capacity(config.peers.len());
    for peer in &config.peers {
        endpoints.push(match &peer.endpoint {
            Some(endpoint) => Some(resolve(endpoint).await?),
            None => None,
        });
    }
    Ok(endpoints)
}

/// Runtime statistics of a peer, as reported by boringtun.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerStats {
    /// Public key identifying the peer.
    pub public_key: PublicKey,
    /// Current endpoint, updated when the peer roams.
    pub endpoint: Option<SocketAddr>,
    /// Time of the last completed handshake, if any.
    pub last_handshake: Option<SystemTime>,
    /// How long the peer has not answered our handshake initiations: `None`
    /// when no initiation is waiting for an answer. Only grows while there is
    /// traffic to send (or a persistent keepalive), as WireGuard starts
    /// handshakes on demand.
    pub handshake_pending: Option<Duration>,
    /// Bytes sent to the peer.
    pub tx_bytes: u64,
    /// Bytes received from the peer.
    pub rx_bytes: u64,
}

/// State shared by the packet processing tasks of a tunnel.
struct Shared {
    device: AsyncDevice,
    socket: PeerSocket,
    /// Peers in configuration order; the position is the peer index used everywhere.
    peers: Vec<Peer>,
    /// Maps destination/source addresses to peer indices (AllowedIPs).
    router: Router,
    /// Local static key pair, needed to identify the sender of handshake initiations.
    private_key: x25519::StaticSecret,
    public_key: x25519::PublicKey,
    /// MAC check and rate limit of handshake messages, shared with every peer's
    /// boringtun session so that they all hand out the same cookies.
    rate_limiter: Arc<RateLimiter>,
    /// Set to the failure reason when a packet loop stops with an I/O error.
    failure: watch::Sender<Option<String>>,
}

/// A running tunnel. Dropping it stops all packet processing and removes the
/// TUN device.
pub struct Tunnel {
    shared: Arc<Shared>,
    /// Name of the TUN device, as assigned by the OS.
    name: String,
    /// OS index of the TUN device.
    if_index: u32,
    /// Resolved endpoints of the peers that have one.
    endpoints: Vec<SocketAddr>,
    /// Packet processing tasks, aborted on drop.
    tasks: Vec<JoinHandle<()>>,
}

impl Tunnel {
    /// Creates the TUN device and the UDP socket, then starts packet processing.
    /// The returned tunnel is already running; the first handshake is started by
    /// outbound traffic or by the timers.
    ///
    /// # Errors
    /// [`TunnelError`] if an endpoint cannot be resolved or the device or socket cannot
    /// be created.
    pub async fn start(config: &TunnelConfig, options: TunnelOptions) -> Result<Self, TunnelError> {
        // endpoints resolved by the caller are reused only if there is one per peer
        let endpoints = match options.endpoints.clone() {
            Some(endpoints) if endpoints.len() == config.peers.len() => endpoints,
            _ => resolve_endpoints(config).await?,
        };

        // creation of the TUN device and of the UDP socket
        let device = build_device(config, &options).map_err(TunnelError::Device)?;
        let name = device.name().map_err(TunnelError::Device)?;
        let if_index = device.if_index().map_err(TunnelError::Device)?;
        let socket = PeerSocket::bind(config.interface.listen_port.unwrap_or(0), options.fwmark)
            .map_err(TunnelError::Socket)?;
        // the socket is pinned to the route of the first endpoint before the tunnel routes
        // exist (Windows only); a failure is not fatal
        if let Some(dest) = endpoints.iter().flatten().next()
            && let Err(err) = socket.pin_to_current_route(*dest)
        {
            tracing::warn!(%dest, "cannot pin the tunnel socket to the physical interface: {err}");
        }

        // one boringtun state machine per peer, indexed by position
        let private_key = x25519::StaticSecret::from(*config.interface.private_key.as_bytes());
        let public_key = x25519::PublicKey::from(&private_key);
        let rate_limiter = Arc::new(RateLimiter::new(&public_key, 2 * HANDSHAKE_RATE_LIMIT));
        let peers = config
            .peers
            .iter()
            .zip(&endpoints)
            .enumerate()
            .map(|(idx, (peer, endpoint))| {
                Peer::new(idx as u32, &private_key, &rate_limiter, peer, *endpoint)
            })
            .collect();

        let shared = Arc::new(Shared {
            device,
            socket,
            peers,
            router: Router::new(&config.peers),
            private_key,
            public_key,
            rate_limiter,
            failure: watch::Sender::new(None),
        });

        // spawn of the packet loops; tun and udp loops report I/O errors through `failure`
        let tasks = vec![
            tokio::spawn(run_until_failure(shared.clone(), "tun", tun_loop)),
            tokio::spawn(run_until_failure(shared.clone(), "udp", udp_loop)),
            tokio::spawn(timer_loop(shared.clone())),
        ];

        tracing::info!(%name, port = ?shared.socket.local_port().ok(), "tunnel started");
        Ok(Self {
            shared,
            name,
            if_index,
            endpoints: endpoints.into_iter().flatten().collect(),
            tasks,
        })
    }

    /// Name of the TUN device (e.g. `utun5` on macOS).
    pub fn interface_name(&self) -> &str {
        &self.name
    }

    /// OS index of the TUN device.
    pub fn interface_index(&self) -> u32 {
        self.if_index
    }

    /// Resolved peer endpoints; these must be routed outside the tunnel.
    pub fn endpoints(&self) -> &[SocketAddr] {
        &self.endpoints
    }

    /// Windows: moves the encrypted traffic to another physical interface
    /// after a network change. No-op elsewhere.
    pub fn repin(&self, if_index: u32) -> io::Result<()> {
        let v6 = self.endpoints.iter().any(SocketAddr::is_ipv6);
        self.shared.socket.pin_to_interface(if_index, v6)
    }

    /// Statistics of every peer, in configuration order.
    pub fn stats(&self) -> Vec<PeerStats> {
        self.shared.peers.iter().map(Peer::stats).collect()
    }

    /// Resolves to `Some(reason)` once the tunnel has stopped due to an I/O error.
    pub fn failure(&self) -> watch::Receiver<Option<String>> {
        self.shared.failure.subscribe()
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        // the tasks hold references to the device: aborting them lets it be removed
        // once the last reference is dropped
        for task in &self.tasks {
            task.abort();
        }
        tracing::info!(name = %self.name, "tunnel stopped");
    }
}

/// Resolves a single endpoint; literal addresses are returned as they are.
async fn resolve(endpoint: &Endpoint) -> Result<SocketAddr, TunnelError> {
    match endpoint {
        Endpoint::Addr(addr) => Ok(*addr),
        Endpoint::Host { host, port } => {
            let err = |source| TunnelError::Resolve {
                endpoint: endpoint.to_string(),
                source,
            };
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), *port))
                .await
                .map_err(err)?
                .collect();
            // IPv4 is preferred: it works on every host, IPv6 connectivity is not guaranteed
            addrs
                .iter()
                .find(|a| a.is_ipv4())
                .or(addrs.first())
                .copied()
                .ok_or_else(|| err(io::Error::new(io::ErrorKind::NotFound, "no addresses")))
        }
    }
}

/// Creates the TUN device with the configured MTU, name and addresses.
fn build_device(config: &TunnelConfig, options: &TunnelOptions) -> io::Result<AsyncDevice> {
    let mtu = config.interface.mtu.unwrap_or(DEFAULT_MTU);
    let mut builder = DeviceBuilder::new().mtu(mtu);
    if let Some(name) = &options.name {
        builder = builder.name(name.clone());
    }

    // the builder accepts a single IPv4 address but any number of IPv6 ones: the other
    // IPv4 addresses are added once the device exists
    let (v4, v6): (Vec<&IpNet>, Vec<&IpNet>) = config
        .interface
        .addresses
        .iter()
        .partition(|net| net.addr().is_ipv4());
    if let Some(net) = v4.first() {
        let IpAddr::V4(addr) = net.addr() else {
            unreachable!()
        };
        builder = builder.ipv4(addr, net.prefix_len(), None);
    }
    for net in &v6 {
        let IpAddr::V6(addr) = net.addr() else {
            unreachable!()
        };
        builder = builder.ipv6(addr, net.prefix_len());
    }

    let device = builder.build_async()?;
    for net in v4.iter().skip(1) {
        let IpAddr::V4(addr) = net.addr() else {
            unreachable!()
        };
        device.add_address_v4(addr, net.prefix_len())?;
    }
    Ok(device)
}

/// Runs a packet loop and, if it fails, logs the error and publishes it as the tunnel
/// failure reason (prefixed with `name`).
async fn run_until_failure<F, Fut>(shared: Arc<Shared>, name: &'static str, task: F)
where
    F: FnOnce(Arc<Shared>) -> Fut,
    Fut: Future<Output = io::Result<()>>,
{
    if let Err(err) = task(shared.clone()).await {
        tracing::error!("{name} loop failed: {err}");
        shared.failure.send_replace(Some(format!("{name}: {err}")));
    }
}

/// Position of `sub` inside the buffer starting at `base`; boringtun returns
/// slices of the buffer it was given.
fn range_in(base: usize, sub: &[u8]) -> Range<usize> {
    let start = sub.as_ptr() as usize - base;
    start..start + sub.len()
}

/// What to do with the output of a boringtun call, as ranges of the output buffer so the
/// buffer borrow (and the peer lock) can be released before the `await`.
enum Action {
    /// Nothing to send (also used for errors, which are only traced).
    None,
    /// Encrypted datagram to send to the peer.
    Network(Range<usize>),
    /// Decrypted packet to write to the TUN device, with its source address.
    Tunnel(Range<usize>, IpAddr),
}

/// Converts a [`TunnResult`] into an [`Action`]; `base` is the start of the output buffer.
fn classify(base: usize, result: TunnResult<'_>) -> Action {
    match result {
        TunnResult::Done => Action::None,
        TunnResult::Err(err) => {
            tracing::trace!("wireguard: {err:?}");
            Action::None
        }
        TunnResult::WriteToNetwork(pkt) => Action::Network(range_in(base, pkt)),
        TunnResult::WriteToTunnelV4(pkt, src) => Action::Tunnel(range_in(base, pkt), src.into()),
        TunnResult::WriteToTunnelV6(pkt, src) => Action::Tunnel(range_in(base, pkt), src.into()),
    }
}

/// Outbound: packets from the OS are encrypted and sent to the matching peer.
async fn tun_loop(shared: Arc<Shared>) -> io::Result<()> {
    let mut src = vec![0u8; BUF_SIZE];
    // the extra 32 bytes hold the WireGuard data header (16) and authentication tag (16)
    let mut dst = vec![0u8; BUF_SIZE + 32];
    loop {
        // the peer is chosen by destination address (cryptokey routing); packets without a
        // matching peer, or for peers with no known endpoint, are dropped
        let len = shared.device.recv(&mut src).await?;
        let packet = &src[..len];
        let Some(peer_idx) = Tunn::dst_address(packet).and_then(|ip| shared.router.lookup(ip))
        else {
            continue;
        };
        let peer = &shared.peers[peer_idx];
        let Some(endpoint) = peer.endpoint() else {
            continue;
        };
        // encryption under the peer lock, which is released before sending
        let action = {
            let mut tunn = peer.tunn.lock().unwrap();
            classify(dst.as_ptr() as usize, tunn.encapsulate(packet, &mut dst))
        };
        if let Action::Network(range) = action {
            send_best_effort(&shared, peer, &dst[range], endpoint).await;
        }
    }
}

/// Inbound: datagrams from peers are decrypted and written to the TUN device.
async fn udp_loop(shared: Arc<Shared>) -> io::Result<()> {
    let mut src = vec![0u8; BUF_SIZE];
    let mut dst = vec![0u8; BUF_SIZE];
    loop {
        // datagrams not belonging to any known peer are dropped
        let (len, from) = shared.socket.recv_from(&mut src).await?;
        let datagram = &src[..len];
        // MAC check and rate limit, before any public key operation or peer state
        match gate(&shared.rate_limiter, from.ip(), datagram, &mut dst) {
            Gate::Pass => {}
            Gate::Cookie(range) => {
                if let Err(err) = shared.socket.send_to(&dst[range], from).await {
                    tracing::debug!(%from, "udp send failed: {err}");
                }
                continue;
            }
            Gate::Drop => continue,
        }
        let Some(peer_idx) = find_peer(&shared, datagram) else {
            continue;
        };
        let peer = &shared.peers[peer_idx];

        // decapsulation may produce several outputs (handshake replies, queued packets):
        // boringtun is called again with an empty input until it has nothing left
        let mut input: &[u8] = datagram;
        loop {
            let action = {
                let mut tunn = peer.tunn.lock().unwrap();
                classify(
                    dst.as_ptr() as usize,
                    tunn.decapsulate(Some(from.ip()), input, &mut dst),
                )
            };
            match action {
                Action::None => break,
                // any authenticated datagram updates the peer endpoint (roaming)
                Action::Network(range) => {
                    if roams(&dst[range.clone()]) {
                        peer.set_endpoint(from);
                    }
                    send_best_effort(&shared, peer, &dst[range], from).await;
                    // flush of the packets queued while the handshake was in progress
                    input = &[];
                }
                Action::Tunnel(range, src_ip) => {
                    peer.set_endpoint(from);
                    // packets whose source is not in this peer's AllowedIPs are dropped
                    if shared.router.lookup(src_ip) == Some(peer_idx) {
                        shared.device.send(&dst[range]).await?;
                    }
                    break;
                }
            }
        }
    }
}

/// Whether the reply boringtun produced for an incoming datagram proves that the
/// datagram was authenticated, so its source can become the peer endpoint.
/// NB: a cookie reply does NOT. It only needs a valid MAC1, which anyone knowing our
/// public key can compute, so roaming on it would let a spoofed source hijack the
/// endpoint and redirect the whole tunnel.
fn roams(reply: &[u8]) -> bool {
    reply.first() != Some(&COOKIE_REPLY)
}

/// Outcome of [`gate`] for an incoming datagram.
#[derive(Debug, PartialEq, Eq)]
enum Gate {
    /// Not a handshake message, or one with valid MACs: on to the peer.
    Pass,
    /// Handshake under load without a valid cookie: the cookie reply to send back,
    /// as a range of the output buffer. No peer state is touched.
    Cookie(Range<usize>),
    /// Malformed datagram or handshake message with an invalid MAC.
    Drop,
}

/// Checks the MACs of a handshake message before it reaches a peer. Without this a
/// flood of forged initiations would cost a key exchange each (see [`find_peer`]); here
/// it costs a hash, and under load the sender must prove with a cookie that it receives
/// at its source address.
fn gate(limiter: &RateLimiter, from: IpAddr, datagram: &[u8], dst: &mut [u8]) -> Gate {
    let base = dst.as_ptr() as usize;
    match limiter.verify_packet(Some(from), datagram, dst) {
        Ok(_) => Gate::Pass,
        Err(TunnResult::WriteToNetwork(cookie)) => Gate::Cookie(range_in(base, cookie)),
        Err(_) => Gate::Drop,
    }
}

/// Drives the WireGuard timers of every peer: handshake (re)initiation, keepalives and
/// session expiry. Peers without a known endpoint are skipped.
async fn timer_loop(shared: Arc<Shared>) {
    let mut dst = vec![0u8; BUF_SIZE];
    let mut interval = tokio::time::interval(TIMER_TICK);
    loop {
        interval.tick().await;
        // the shared limiter is not reset by boringtun; it resets at most once a second
        shared.rate_limiter.reset_count();
        for peer in &shared.peers {
            let Some(endpoint) = peer.endpoint() else {
                continue;
            };
            let action = {
                let mut tunn = peer.tunn.lock().unwrap();
                classify(dst.as_ptr() as usize, tunn.update_timers(&mut dst))
            };
            if let Action::Network(range) = action {
                send_best_effort(&shared, peer, &dst[range], endpoint).await;
            }
        }
    }
}

/// Sends a datagram to `peer`. UDP send errors (e.g. network temporarily
/// unreachable) must not kill the tunnel. Handshake initiations are recorded
/// even when the send fails: the peer is not answering either way.
async fn send_best_effort(shared: &Shared, peer: &Peer, buf: &[u8], to: SocketAddr) {
    peer.note_sent(buf);
    if let Err(err) = shared.socket.send_to(buf, to).await {
        tracing::debug!(%to, "udp send failed: {err}");
    }
}

/// Finds the peer a datagram belongs to, from its receiver index or, for
/// handshake initiations, from the initiator's static key.
fn find_peer(shared: &Shared, datagram: &[u8]) -> Option<usize> {
    const HANDSHAKE_RESPONSE: u8 = 2;
    const DATA: u8 = 4;

    // the message type is the first byte; responses, cookie replies and data messages
    // carry the receiver index chosen by us, at a type-dependent offset
    let receiver_offset = match *datagram.first()? {
        HANDSHAKE_RESPONSE => 8,
        COOKIE_REPLY | DATA => 4,
        // handshake initiations (and anything else) have no receiver index: the
        // initiator's static key is decrypted and matched against the peers
        _ => {
            let Ok(Packet::HandshakeInit(init)) = Tunn::parse_incoming_packet(datagram) else {
                return None;
            };
            let half = parse_handshake_anon(&shared.private_key, &shared.public_key, &init).ok()?;
            return shared
                .peers
                .iter()
                .position(|p| p.public_key.as_bytes() == &half.peer_static_public);
        }
    };
    let bytes = datagram.get(receiver_offset..receiver_offset + 4)?;
    let receiver = u32::from_le_bytes(bytes.try_into().ok()?);
    // `Tunn::new` shifts the peer index left by 8 bits to form session indices
    let idx = (receiver >> 8) as usize;
    (idx < shared.peers.len()).then_some(idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Our static public key and a peer session that sends handshakes to it.
    fn keys() -> (x25519::PublicKey, Tunn) {
        let ours = x25519::StaticSecret::from([1u8; 32]);
        let public = x25519::PublicKey::from(&ours);
        let peer = Tunn::new(
            x25519::StaticSecret::from([2u8; 32]),
            public,
            None,
            None,
            0,
            None,
        );
        (public, peer)
    }

    /// A fresh handshake initiation from `peer`.
    fn initiation(peer: &mut Tunn) -> Vec<u8> {
        let mut buf = vec![0u8; BUF_SIZE];
        match peer.format_handshake_initiation(&mut buf, true) {
            TunnResult::WriteToNetwork(pkt) => pkt.to_vec(),
            _ => panic!("no handshake initiation"),
        }
    }

    // a genuine initiation passes, one with a forged MAC is dropped before the peer lookup
    #[test]
    fn gate_drops_forged_initiations() {
        let (public, mut peer) = keys();
        let limiter = RateLimiter::new(&public, 2 * HANDSHAKE_RATE_LIMIT);
        let from: IpAddr = "203.0.113.7".parse().unwrap();
        let mut dst = vec![0u8; BUF_SIZE];

        let mut packet = initiation(&mut peer);
        assert_eq!(gate(&limiter, from, &packet, &mut dst), Gate::Pass);
        let last = packet.len() - 20;
        packet[last] ^= 0xff;
        assert_eq!(gate(&limiter, from, &packet, &mut dst), Gate::Drop);
    }

    // past the rate limit an initiation without a cookie gets a cookie reply
    #[test]
    fn gate_answers_with_a_cookie_under_load() {
        let (public, mut peer) = keys();
        let limiter = RateLimiter::new(&public, 2 * HANDSHAKE_RATE_LIMIT);
        let from: IpAddr = "203.0.113.7".parse().unwrap();
        let mut dst = vec![0u8; BUF_SIZE];
        let packet = initiation(&mut peer);

        for _ in 0..2 * HANDSHAKE_RATE_LIMIT {
            assert_eq!(gate(&limiter, from, &packet, &mut dst), Gate::Pass);
        }
        let Gate::Cookie(range) = gate(&limiter, from, &packet, &mut dst) else {
            panic!("no cookie under load");
        };
        assert_eq!(dst[range.start], COOKIE_REPLY);
        assert!(!roams(&dst[range]));
    }

    // a handshake response, produced only for an authenticated initiation, roams
    #[test]
    fn handshake_response_roams() {
        let ours = x25519::StaticSecret::from([1u8; 32]);
        let (_, mut peer) = keys();
        let mut local = Tunn::new(
            ours,
            x25519::PublicKey::from(&x25519::StaticSecret::from([2u8; 32])),
            None,
            None,
            0,
            None,
        );
        let packet = initiation(&mut peer);
        let mut dst = vec![0u8; BUF_SIZE];
        let from: IpAddr = "203.0.113.7".parse().unwrap();
        let TunnResult::WriteToNetwork(reply) = local.decapsulate(Some(from), &packet, &mut dst)
        else {
            panic!("no handshake response");
        };
        assert!(roams(reply));
    }
}

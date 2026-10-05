//! Per-peer WireGuard state and cryptokey routing.

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Instant, SystemTime};

use boringtun::noise::Tunn;
use boringtun::noise::rate_limiter::RateLimiter;
use boringtun::x25519;
use ip_network::IpNetwork;
use ip_network_table::IpNetworkTable;
use ipnet::IpNet;
use submarine_config::PublicKey;

use crate::PeerStats;

/// First byte of a WireGuard handshake initiation message.
const HANDSHAKE_INIT: u8 = 1;

/// Runtime state of a peer: its boringtun session and current endpoint.
pub(crate) struct Peer {
    /// Public key identifying the peer.
    pub public_key: PublicKey,
    /// boringtun state machine (handshake, sessions, timers).
    pub tunn: Mutex<Tunn>,
    /// Updated to the source of the latest authenticated packet (roaming).
    pub endpoint: RwLock<Option<SocketAddr>>,
    /// Time of the first handshake initiation sent since the latest completed
    /// handshake: how long the peer has left us waiting for an answer.
    handshake_sent: Mutex<Option<Instant>>,
}

impl Peer {
    /// Creates the boringtun session for `peer`. `index` is the peer position and is
    /// embedded in the session indices, so incoming datagrams can be mapped back to it.
    /// `rate_limiter` is the tunnel-wide one, shared by all peers.
    pub fn new(
        index: u32,
        private_key: &x25519::StaticSecret,
        rate_limiter: &Arc<RateLimiter>,
        peer: &submarine_config::Peer,
        endpoint: Option<SocketAddr>,
    ) -> Self {
        let tunn = Tunn::new(
            private_key.clone(),
            x25519::PublicKey::from(*peer.public_key.as_bytes()),
            peer.preshared_key.as_ref().map(|k| *k.as_bytes()),
            peer.persistent_keepalive,
            index,
            Some(rate_limiter.clone()),
        );
        Self {
            public_key: peer.public_key,
            tunn: Mutex::new(tunn),
            endpoint: RwLock::new(endpoint),
            handshake_sent: Mutex::new(None),
        }
    }

    /// Current endpoint, `None` until configured or learned from an incoming packet.
    pub fn endpoint(&self) -> Option<SocketAddr> {
        *self.endpoint.read().unwrap()
    }

    /// Records `addr` as the peer endpoint (roaming).
    pub fn set_endpoint(&self, addr: SocketAddr) {
        // the write lock is taken only on change, not on every packet
        if self.endpoint() != Some(addr) {
            *self.endpoint.write().unwrap() = Some(addr);
        }
    }

    /// Records a datagram about to be sent to the peer: a handshake initiation
    /// starts the wait for an answer, unless one is already pending.
    pub fn note_sent(&self, datagram: &[u8]) {
        if datagram.first() == Some(&HANDSHAKE_INIT) {
            self.handshake_sent
                .lock()
                .unwrap()
                .get_or_insert_with(Instant::now);
        }
    }

    /// Snapshot of the peer statistics; the handshake age is converted to a wall-clock time.
    pub fn stats(&self) -> PeerStats {
        let (since_handshake, tx_bytes, rx_bytes, _, _) = self.tunn.lock().unwrap().stats();
        let now = Instant::now();
        // the wait ends with a handshake completed after the first initiation
        let handshake_at = since_handshake.and_then(|d| now.checked_sub(d));
        let mut sent = self.handshake_sent.lock().unwrap();
        if let (Some(sent_at), Some(done_at)) = (*sent, handshake_at)
            && done_at >= sent_at
        {
            *sent = None;
        }
        PeerStats {
            public_key: self.public_key,
            endpoint: self.endpoint(),
            last_handshake: since_handshake.and_then(|d| SystemTime::now().checked_sub(d)),
            handshake_pending: sent.map(|sent_at| now.duration_since(sent_at)),
            tx_bytes: tx_bytes as u64,
            rx_bytes: rx_bytes as u64,
        }
    }
}

/// Cryptokey routing: maps AllowedIPs to peer indices.
pub(crate) struct Router {
    /// AllowedIPs of all peers, looked up by longest prefix match.
    table: IpNetworkTable<usize>,
}

impl Router {
    /// Builds the table from the peers' AllowedIPs. If several peers claim the same network
    /// the last one wins.
    pub fn new(peers: &[submarine_config::Peer]) -> Self {
        let mut table = IpNetworkTable::new();
        for (idx, peer) in peers.iter().enumerate() {
            for net in &peer.allowed_ips {
                table.insert(to_network(net), idx);
            }
        }
        Self { table }
    }

    /// Index of the peer whose AllowedIPs best match `addr`, if any.
    pub fn lookup(&self, addr: IpAddr) -> Option<usize> {
        self.table.longest_match(addr).map(|(_, idx)| *idx)
    }
}

/// Converts to the table's network type; host bits are cleared first because
/// `IpNetwork` rejects them.
fn to_network(net: &IpNet) -> IpNetwork {
    let net = net.trunc();
    IpNetwork::new(net.addr(), net.prefix_len()).expect("truncated network is valid")
}

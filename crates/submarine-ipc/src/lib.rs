//! Protocol between the unprivileged UI and `submarine-daemon`.
//!
//! Messages are newline-delimited JSON over a local socket (Unix domain socket
//! or Windows named pipe). Every request carries an `id` echoed by its
//! response; events are pushed to every connected client at any time.
//!
//! The daemon side uses [`bind`], [`read_message`] and [`write_message`] directly;
//! the UI uses the higher level [`Client`].

mod client;
mod transport;
#[cfg(windows)]
mod winpipe;

use std::fmt;

use serde::{Deserialize, Serialize};
use submarine_config::TunnelConfig;

pub use client::{Client, ClientError};
pub use transport::{
    Connection, Listener, MAX_MESSAGE_LEN, bind, connect, read_message, socket_path, write_message,
};

/// A command sent by a client; serialized as `{"method": ..., "params": ...}`.
/// Its `Debug` output leaves out the configuration text, see the impl below.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum Request {
    /// Current connection status, answered with [`Response::Status`].
    GetStatus,
    /// Stored tunnels, answered with [`Response::Tunnels`].
    ListTunnels,
    /// Stores a new tunnel; `config` is the text of a wg-quick `.conf` file.
    /// Answered with [`Response::Imported`].
    ImportTunnel {
        name: String,
        config: String,
    },
    /// The tunnel's configuration with its keys hidden, for editing.
    GetTunnelConfig {
        id: String,
    },
    /// Renames a tunnel and replaces its configuration. `config` is the text
    /// of a `.conf` file, where keys left as `submarine_config::HIDDEN` keep
    /// their stored value. A tunnel in use reconnects with the new one.
    UpdateTunnel {
        id: String,
        name: String,
        config: String,
    },
    /// Deletes a stored tunnel.
    DeleteTunnel {
        id: String,
    },
    /// Connects the given tunnel.
    Connect {
        id: String,
    },
    Disconnect,
    /// Disconnects for `seconds` with the kill switch suspended, then connects the
    /// same tunnel again. `Connect` resumes earlier, `Disconnect` cancels the resume.
    Pause {
        seconds: u64,
    },
    /// Current settings, answered with [`Response::Settings`].
    GetSettings,
    /// Replaces all the settings.
    SetSettings {
        settings: Settings,
    },
    /// The service's most recent log lines.
    GetLogs,
    /// Empties the in-memory log buffer. The log file is left as it is.
    ClearLogs,
}

/// Written by hand so that the `.conf` text of `ImportTunnel` and `UpdateTunnel`, which
/// holds the private keys, never ends up in logs (the daemon logs every request at debug
/// level, and log lines are pushed to every client). The other requests carry no
/// secrets and are shown as their JSON.
impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ImportTunnel { name, config: _ } => f
                .debug_struct("ImportTunnel")
                .field("name", name)
                .finish_non_exhaustive(),
            Self::UpdateTunnel {
                id,
                name,
                config: _,
            } => f
                .debug_struct("UpdateTunnel")
                .field("id", id)
                .field("name", name)
                .finish_non_exhaustive(),
            other => match serde_json::to_string(other) {
                Ok(json) => f.write_str(&json),
                Err(_) => f.write_str("Request"),
            },
        }
    }
}

/// Successful result of a [`Request`]; serialized as `{"type": ..., "data": ...}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Response {
    /// Success with no payload.
    Ok,
    Status(Status),
    Tunnels(Vec<TunnelInfo>),
    /// The stored tunnel and the parser warnings. Also the answer to `UpdateTunnel`.
    Imported {
        tunnel: TunnelInfo,
        warnings: Vec<String>,
    },
    /// Answer to `GetTunnelConfig`: tunnel name and redacted `.conf` text.
    TunnelConfig {
        name: String,
        config: String,
    },
    Settings(Settings),
    Logs(Vec<LogLine>),
}

/// A notification pushed by the daemon to every client, without a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Event {
    StatusChanged(Status),
    TunnelsChanged(Vec<TunnelInfo>),
    SettingsChanged(Settings),
    /// A line just written to the service log (see `Request::GetLogs`).
    /// Clients that fall behind may miss some.
    LogLine(LogLine),
}

/// Envelope of a request on the wire: the request is flattened next to its `id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientMessage {
    /// Chosen by the client, echoed by the matching response.
    pub id: u64,
    #[serde(flatten)]
    pub request: Request,
}

/// Envelope of everything the daemon sends: either a response or an event.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Result of the request with the same `id`; the error is a message for the user.
    Response {
        id: u64,
        result: Result<Response, String>,
    },
    Event {
        event: Event,
    },
}

/// Lifecycle state of the tunnel connection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Disconnecting,
    /// The connection the user asked for failed; see `Status::error`.
    Failed,
    /// The tunnel stopped working (it failed or the server stopped answering) and
    /// is being connected again; `Status::retry_at` is set while waiting for the
    /// next attempt. The tunnel of such an attempt stays in this state, with its
    /// interface up, until the first handshake. The kill switch keeps blocking
    /// meanwhile.
    Reconnecting,
    /// Disconnected on purpose for a while, kill switch included, until
    /// `Status::paused_until`; then the tunnel is connected again.
    Paused,
}

/// Connection status, sent on request and on every change.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Status {
    pub state: ConnectionState,
    /// Tunnel the state refers to, if any.
    pub tunnel_id: Option<String>,
    /// Name of the TUN device while connected.
    pub interface: Option<String>,
    /// Per-peer statistics while connected.
    pub peers: Vec<PeerStatus>,
    /// Reason of the last failure (see `ConnectionState::Failed` and
    /// `ConnectionState::Reconnecting`).
    pub error: Option<String>,
    /// Unix timestamp (seconds) of when the tunnel came up, while connected.
    pub connected_since: Option<u64>,
    /// Unix timestamp (seconds) of the next reconnection attempt, while
    /// reconnecting and waiting for it.
    pub retry_at: Option<u64>,
    /// Unix timestamp (seconds) of when a pause ends, while paused.
    pub paused_until: Option<u64>,
    /// The kill switch is currently blocking traffic outside the tunnel.
    pub blocked: bool,
    /// Set when the kill switch or split tunneling could not be applied.
    pub protection_error: Option<String>,
    /// Name (SSID) of the Wi-Fi network the computer is on, if any and if the
    /// OS lets the service read it.
    pub wifi: Option<String>,
}

/// Kill switch mode: when traffic outside the tunnel is blocked.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KillSwitch {
    #[default]
    Off,
    /// Block traffic outside the tunnel while connected, and keep blocking
    /// while a tunnel that stopped working is reconnected, until the user
    /// disconnects. A pause suspends it.
    OnConnect,
    /// Block traffic outside the tunnel at all times, even when disconnected.
    Always,
}

/// Per-application split tunneling mode, applied to [`Settings::split_apps`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitTunnelMode {
    #[default]
    Off,
    /// Only the listed apps use the tunnel.
    Include,
    /// Every app uses the tunnel except the listed ones.
    Exclude,
}

/// An application selected for split tunneling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppRule {
    /// Display name.
    pub name: String,
    /// Absolute path of the executable.
    pub path: String,
}

/// User settings stored by the daemon. Missing fields take their default value.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub kill_switch: KillSwitch,
    /// Local network traffic is allowed even when the kill switch blocks.
    pub allow_lan: bool,
    pub split_mode: SplitTunnelMode,
    /// Applications the split tunneling mode applies to.
    pub split_apps: Vec<AppRule>,
    /// Addresses both in the tunnel's AllowedIPs (narrower than /0) and on a
    /// local network go through the tunnel. Off: the system decides, usually
    /// in favour of the local network.
    pub prefer_tunnel: bool,
    /// Tunnel to connect when the service starts (i.e. at boot), unless a
    /// connection left up before is being restored.
    pub auto_connect: Option<String>,
    /// Tunnel connected automatically on joining a Wi-Fi network that is not
    /// in `trusted_networks`, if nothing is connected yet.
    pub untrusted_tunnel: Option<String>,
    /// Names (SSIDs) of the trusted Wi-Fi networks.
    pub trusted_networks: Vec<String>,
    /// Joining a trusted Wi-Fi network disconnects the VPN.
    pub disconnect_on_trusted: bool,
}

/// A line of the service log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogLine {
    /// Unix time in milliseconds.
    pub time: u64,
    /// "error", "warn", "info", "debug" or "trace".
    pub level: String,
    /// Module that emitted the line.
    pub target: String,
    pub message: String,
}

/// Statistics of a connected peer, in a UI-friendly form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerStatus {
    /// Base64 public key.
    pub public_key: String,
    /// Current endpoint, if known.
    pub endpoint: Option<String>,
    /// Unix timestamp (seconds) of the latest handshake.
    pub last_handshake: Option<u64>,
    /// Bytes received from and sent to the peer.
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

/// Summary of a stored tunnel. Never contains key material.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelInfo {
    /// Identifier assigned by the daemon.
    pub id: String,
    /// Name chosen by the user.
    pub name: String,
    /// Interface addresses, in CIDR notation.
    pub addresses: Vec<String>,
    /// Endpoints of the peers that have one.
    pub endpoints: Vec<String>,
    /// DNS servers followed by search domains.
    pub dns: Vec<String>,
    /// True if at least one peer routes all traffic (`0.0.0.0/0` or `::/0`).
    pub full_tunnel: bool,
    /// Number of peers.
    pub peers: usize,
}

impl TunnelInfo {
    /// Builds the summary of a stored configuration.
    pub fn new(id: String, name: String, config: &TunnelConfig) -> Self {
        let iface = &config.interface;
        TunnelInfo {
            id,
            name,
            addresses: iface.addresses.iter().map(ToString::to_string).collect(),
            endpoints: config
                .peers
                .iter()
                .filter_map(|p| p.endpoint.as_ref().map(ToString::to_string))
                .collect(),
            dns: iface
                .dns_servers
                .iter()
                .map(ToString::to_string)
                .chain(iface.dns_search.iter().cloned())
                .collect(),
            full_tunnel: config.peers.iter().any(|p| p.is_default_route()),
            peers: config.peers.len(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // the configuration text, with its keys, is left out of the debug output
    #[test]
    fn request_debug_hides_the_config() {
        let config = "[Interface]\nPrivateKey = c2VjcmV0\n".to_owned();
        let import = Request::ImportTunnel {
            name: "office".into(),
            config: config.clone(),
        };
        let update = Request::UpdateTunnel {
            id: "0123".into(),
            name: "office".into(),
            config,
        };
        for request in [import, update] {
            let text = format!("{request:?}");
            assert!(text.contains("office"));
            assert!(!text.contains("PrivateKey") && !text.contains("c2VjcmV0"));
        }
    }

    // requests without secrets are shown in full
    #[test]
    fn request_debug_shows_other_requests() {
        let text = format!("{:?}", Request::Connect { id: "0123".into() });
        assert!(text.contains("connect") && text.contains("0123"));
    }
}

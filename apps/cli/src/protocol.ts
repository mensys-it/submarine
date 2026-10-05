// Types of the IPC protocol with submarine-daemon, mirroring crates/submarine-ipc
// (like apps/desktop/src/api.ts). Field names stay in snake_case, as serialized by
// the service. NB: any change to the Rust types MUST be applied here too.

/** State of the VPN connection. */
export type ConnectionState = "disconnected" | "connecting" | "connected" | "disconnecting" | "failed" | "reconnecting" | "paused";

/** Live statistics of a WireGuard peer. */
export interface PeerStatus {
  public_key: string;
  /** Current endpoint, if known. */
  endpoint: string | null;
  /** Unix time in seconds of the latest handshake, null before the first one. */
  last_handshake: number | null;
  rx_bytes: number;
  tx_bytes: number;
}

/** Connection status, as published by the service. */
export interface Status {
  state: ConnectionState;
  /** Tunnel the state refers to, if any. */
  tunnel_id: string | null;
  /** Name of the TUN device while connected. */
  interface: string | null;
  peers: PeerStatus[];
  /** Reason of the last failure, or of the reconnection in progress. */
  error: string | null;
  /** Unix time in seconds of when the tunnel came up, while connected. */
  connected_since?: number | null;
  /** Unix time in seconds of the next reconnection attempt, while waiting for it. */
  retry_at?: number | null;
  /** Unix time in seconds of when the pause ends, while paused. */
  paused_until?: number | null;
  /** The kill switch is blocking the traffic outside the tunnel. */
  blocked: boolean;
  /** Set when the kill switch or split tunneling could not be applied. */
  protection_error: string | null;
  /** Per-app split tunneling cannot work here (no driver on Windows): its settings are ignored. */
  split_unavailable?: boolean;
  /** Wi-Fi network the computer is on, if any and if the service can read it. */
  wifi?: string | null;
}

/** Kill switch mode: never, while connected (and after a drop), or always. */
export type KillSwitch = "off" | "on_connect" | "always";
/** Split tunneling: off, only the listed apps in the tunnel, or all but them. */
export type SplitTunnelMode = "off" | "include" | "exclude";

/** An app of the split tunneling list. */
export interface AppRule {
  /** Display name. */
  name: string;
  /** Absolute path of the executable. */
  path: string;
}

/** Settings of the service, saved on disk. */
export interface Settings {
  kill_switch: KillSwitch;
  /** The local network stays reachable while the kill switch blocks. */
  allow_lan: boolean;
  split_mode: SplitTunnelMode;
  split_apps: AppRule[];
  prefer_tunnel?: boolean;
  /** Tunnel connected when the service starts. */
  auto_connect?: string | null;
  /** Tunnel connected on joining a Wi-Fi network that is not trusted. */
  untrusted_tunnel?: string | null;
  /** Names (SSIDs) of the trusted Wi-Fi networks. */
  trusted_networks?: string[];
  /** Joining a trusted Wi-Fi network disconnects the VPN. */
  disconnect_on_trusted?: boolean;
}

/** An imported tunnel, without its keys: they never leave the service. */
export interface TunnelInfo {
  id: string;
  name: string;
  /** Interface addresses, in CIDR notation. */
  addresses: string[];
  /** Endpoints of the peers that have one. */
  endpoints: string[];
  /** DNS servers followed by search domains. */
  dns: string[];
  /** The tunnel routes all the traffic, not only its own networks. */
  full_tunnel: boolean;
  /** Number of peers. */
  peers: number;
}

/** Request to the service; it travels with an `id` added by the client. */
export type Request =
  | { method: "get_status" }
  | { method: "list_tunnels" }
  | { method: "import_tunnel"; params: { name: string; config: string } }
  | { method: "delete_tunnel"; params: { id: string } }
  | { method: "connect"; params: { id: string } }
  | { method: "disconnect" }
  | { method: "pause"; params: { seconds: number } }
  | { method: "get_settings" }
  | { method: "set_settings"; params: { settings: Settings } }
  | { method: "get_logs" }
  | { method: "clear_logs" };

/** Successful response to a request. */
export type Response =
  | { type: "ok" }
  | { type: "status"; data: Status }
  | { type: "tunnels"; data: TunnelInfo[] }
  | { type: "imported"; data: { tunnel: TunnelInfo; warnings: string[] } }
  | { type: "settings"; data: Settings }
  | { type: "logs"; data: LogLine[] };

/** A line of the service log. */
export interface LogLine {
  /** Unix time in milliseconds. */
  time: number;
  /** "error", "warn", "info", "debug" or "trace". */
  level: string;
  /** Module that emitted the line. */
  target: string;
  /** The message first, then the other fields as `key=value`. */
  message: string;
}

/** Event pushed by the service to every client. */
export type DaemonEvent =
  | { type: "status_changed"; data: Status }
  | { type: "tunnels_changed"; data: TunnelInfo[] }
  | { type: "settings_changed"; data: Settings }
  | { type: "log_line"; data: LogLine };

/** Message from the service: the response to request `id`, or an event. */
export type ServerMessage =
  | { kind: "response"; id: number; result: { Ok: Response } | { Err: string } }
  | { kind: "event"; event: DaemonEvent };

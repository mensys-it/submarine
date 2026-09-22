// Typed access to the backend. The types mirror crates/submarine-ipc (daemon requests,
// responses and events) and src-tauri (app preferences, platform commands). Outside Tauri
// (plain browser, used for UI development) a small in-memory mock stands in for both.

import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { open } from "@tauri-apps/plugin-dialog";

/** Life cycle of the tunnel connection, as reported by the daemon. */
export type ConnectionState =
  | "disconnected"
  | "connecting"
  | "connected"
  | "disconnecting"
  | "failed"
  | "reconnecting";

/** Live state of one WireGuard peer of the active tunnel. */
export interface PeerStatus {
  /** Base64 public key of the peer. */
  public_key: string;
  endpoint: string | null;
  /** Unix time in seconds of the latest handshake; null if none happened yet. */
  last_handshake: number | null;
  rx_bytes: number;
  tx_bytes: number;
}

/** Connection state of the daemon, sent on request and on every change. */
export interface Status {
  state: ConnectionState;
  /** Tunnel connected or being connected; null when idle. */
  tunnel_id: string | null;
  /** Name of the network interface of the tunnel, once it exists. */
  interface: string | null;
  peers: PeerStatus[];
  /** Why the last connection attempt failed, or why the tunnel is being reconnected. */
  error: string | null;
  /** Unix time in seconds of when the tunnel came up, while connected. */
  connected_since: number | null;
  /** Unix time in seconds of the next reconnection attempt, while waiting for it. */
  retry_at: number | null;
  /** The kill switch is blocking traffic outside the tunnel right now. */
  blocked: boolean;
  /** Why the kill switch or split tunneling could not be applied, if they could not. */
  protection_error: string | null;
}

/** When the kill switch blocks traffic outside the tunnel. */
export type KillSwitch = "off" | "on_connect" | "always";
/** Whether split tunneling routes ONLY the listed apps through the tunnel, or all BUT them. */
export type SplitTunnelMode = "off" | "include" | "exclude";

/** An application selected for split tunneling. */
export interface AppRule {
  /** Display name. */
  name: string;
  /** Executable the rule matches. */
  path: string;
}

/** Protection and routing settings, stored and enforced by the daemon. */
export interface Settings {
  kill_switch: KillSwitch;
  /** The local network stays reachable while the kill switch blocks the rest. */
  allow_lan: boolean;
  split_mode: SplitTunnelMode;
  split_apps: AppRule[];
  /** Addresses both in the tunnel and on the local network go through the tunnel. */
  prefer_tunnel: boolean;
  /** Tunnel id to connect when the service starts. */
  auto_connect: string | null;
}

/** One line of the daemon log. */
export interface LogLine {
  /** Unix time in milliseconds. */
  time: number;
  level: "error" | "warn" | "info" | "debug" | "trace";
  /** Module that emitted the line, e.g. `submarine_daemon`. */
  target: string;
  message: string;
}

/** Preferences of the desktop app itself (src-tauri/src/prefs.rs). */
export interface Prefs {
  /** Desktop notifications on connection changes. */
  notifications: boolean;
  /** Chosen language; "system" follows the OS. */
  language: "system" | "it" | "en";
  launch_at_login: boolean;
  /** The close button keeps the app in the tray; off, it quits. */
  close_to_tray: boolean;
  /** Quitting the app also disconnects the VPN. */
  disconnect_on_quit: boolean;
  /** Language actually in use. */
  resolved_language: "it" | "en";
}

/** Summary of an imported tunnel, without its keys. */
export interface TunnelInfo {
  /** Opaque id assigned by the daemon. */
  id: string;
  name: string;
  /** Interface addresses, in CIDR notation. */
  addresses: string[];
  /** Peer endpoints, as `host:port`. */
  endpoints: string[];
  dns: string[];
  /** All traffic goes through the tunnel (a default route among the allowed IPs). */
  full_tunnel: boolean;
  /** Number of peers. */
  peers: number;
}

/** Requests to the daemon, serialized with the same tags as the Rust enum. */
export type Request =
  | { method: "get_status" }
  | { method: "list_tunnels" }
  | { method: "import_tunnel"; params: { name: string; config: string } }
  | { method: "get_tunnel_config"; params: { id: string } }
  | { method: "update_tunnel"; params: { id: string; name: string; config: string } }
  | { method: "delete_tunnel"; params: { id: string } }
  | { method: "connect"; params: { id: string } }
  | { method: "disconnect" }
  | { method: "get_settings" }
  | { method: "set_settings"; params: { settings: Settings } }
  | { method: "get_logs" }
  | { method: "clear_logs" };

/** Answers of the daemon; each request expects one specific variant. */
export type Response =
  | { type: "ok" }
  | { type: "status"; data: Status }
  | { type: "tunnels"; data: TunnelInfo[] }
  /** Also the answer to update_tunnel. */
  | { type: "imported"; data: { tunnel: TunnelInfo; warnings: string[] } }
  /** Keys appear as HIDDEN; left that way, they keep their stored value. */
  | { type: "tunnel_config"; data: { name: string; config: string } }
  | { type: "settings"; data: Settings }
  | { type: "logs"; data: LogLine[] };

/** Notifications the daemon pushes to every connected client. */
export type DaemonEvent =
  | { type: "status_changed"; data: Status }
  | { type: "tunnels_changed"; data: TunnelInfo[] }
  | { type: "settings_changed"; data: Settings }
  | { type: "log_line"; data: LogLine };

/** The daemon, reached through the Tauri backend, which owns the IPC connection. */
export interface Daemon {
  /** Sends one request; rejects with the daemon's error message. */
  request(request: Request): Promise<Response>;
  /** Applications installed for the current user, for split tunneling. */
  listApps(): Promise<AppRule[]>;
  /** Reads the preferences of the desktop app. */
  getPrefs(): Promise<Prefs>;
  /** Saves the preferences of the desktop app and returns them as applied. */
  setPrefs(prefs: Prefs): Promise<Prefs>;
  /** Whether the backend is connected to the daemon right now. */
  isConnected(): Promise<boolean>;
  /** Subscribes to daemon events. */
  onEvent(handler: (event: DaemonEvent) => void): Promise<UnlistenFn>;
  /** Subscribes to the backend connecting to or losing the daemon. */
  onConnection(handler: (connected: boolean) => void): Promise<UnlistenFn>;
}

/** A WireGuard configuration read from disk. */
export interface ConfigFile {
  /** File name, used as the default tunnel name. */
  name: string;
  /** Contents of the file. */
  text: string;
}

/** Files dragged over the window (the webview's own HTML5 drop never fires in Tauri). */
export type FileDrag = { type: "over" } | { type: "leave" } | { type: "drop"; paths: string[] };

/** Desktop services outside the daemon: files, drag and drop, clipboard. */
export interface Platform {
  /** Asks for a .conf file; null if the user cancels. */
  pickConfigFile(): Promise<ConfigFile | null>;
  /** Rejects with "too_large", "not_text" or an OS error. */
  readConfigFile(path: string): Promise<ConfigFile>;
  /** Subscribes to files dragged over the window. */
  onFileDrag(handler: (drag: FileDrag) => void): Promise<UnlistenFn>;
  /** Writes text to the system clipboard. */
  copyText(text: string): Promise<void>;
  /** Version of the desktop app. */
  appVersion(): Promise<string>;
}

/** Stands for a secret key in a configuration being edited (submarine_config::HIDDEN). */
export const HIDDEN = "(hidden)";

/** Last component of a path, with either separator; the whole path if there is none. */
export const fileName = (path: string) => path.split(/[\\/]/).pop() || path;

/** Daemon backed by the Tauri commands and events of src-tauri. */
const tauriDaemon: Daemon = {
  request: (request) => invoke<Response>("daemon_request", { request }),
  listApps: () => invoke<AppRule[]>("list_apps"),
  getPrefs: () => invoke<Prefs>("get_prefs"),
  setPrefs: (prefs) => invoke<Prefs>("set_prefs", { prefs }),
  isConnected: () => invoke<boolean>("daemon_connected"),
  onEvent: (handler) => listen<DaemonEvent>("daemon-event", (e) => handler(e.payload)),
  onConnection: (handler) => listen<boolean>("daemon-connection", (e) => handler(e.payload)),
};

/** Reads a configuration file through the backend, which enforces size and encoding. */
const readConfigFile = async (path: string): Promise<ConfigFile> => ({
  name: fileName(path),
  text: await invoke<string>("read_config_file", { path }),
});

/** Platform services backed by Tauri and its plugins. */
const tauriPlatform: Platform = {
  async pickConfigFile() {
    const path = await open({
      multiple: false,
      directory: false,
      filters: [{ name: "WireGuard", extensions: ["conf"] }],
    });
    return path ? readConfigFile(path) : null;
  },
  readConfigFile,
  onFileDrag: (handler) =>
    getCurrentWebview().onDragDropEvent(({ payload }) => {
      if (payload.type === "drop") handler({ type: "drop", paths: payload.paths });
      else if (payload.type === "leave") handler({ type: "leave" });
      else handler({ type: "over" });
    }),
  copyText: writeText,
  appVersion: getVersion,
};

// NB: the mock is imported dynamically, so it stays out of the Tauri bundle path and does NOT
// run its side effects (sample logs, URL parsing) inside the real app
const inTauri = "__TAURI_INTERNALS__" in window;
const mock = inTauri ? null : await import("./mock");

/** The daemon in use: the real one in Tauri, the mock in a browser. */
export const daemon: Daemon = mock ? mock.mockDaemon : tauriDaemon;
/** The platform services in use: the real ones in Tauri, the mock in a browser. */
export const platform: Platform = mock ? mock.mockPlatform : tauriPlatform;

/**
 * Sends a request and narrows the answer to the expected response type.
 *
 * Rejects with the daemon's error, or if the daemon answers with a different type.
 */
export async function request<T extends Response["type"]>(
  req: Request,
  expected: T,
): Promise<Extract<Response, { type: T }>> {
  const response = await daemon.request(req);
  if (response.type !== expected) {
    throw new Error(`Unexpected response from the service: ${response.type}`);
  }
  return response as Extract<Response, { type: T }>;
}

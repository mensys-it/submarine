// In-memory daemon and platform for developing the UI in a browser (`npm run dev`),
// loaded by api.ts outside Tauri. `?mock=empty` starts without tunnels, `?mock=offline`
// without a daemon. The behavior is a rough imitation, with sample data and fake traffic.

import type { ConfigFile, Daemon, DaemonEvent, LogLine, Platform, Prefs, Request, Response, Settings, Status, TunnelInfo } from "./api";
import { browserLang } from "./lang";
import packageJson from "../package.json";

// not imported from ./api, which loads this module while it initializes
const HIDDEN = "(hidden)";

/** Scenario picked with the `mock` query parameter. */
const mode = new URLSearchParams(location.search).get("mock");

/** Sample tunnels, or none in the "empty" scenario. */
let tunnels: TunnelInfo[] =
  mode === "empty"
    ? []
    : [
        {
          id: "a1",
          name: "Ufficio Milano",
          addresses: ["10.8.0.2/32"],
          endpoints: ["vpn.example.com:51820"],
          dns: ["10.8.0.1"],
          full_tunnel: true,
          peers: 1,
        },
        {
          id: "b2",
          name: "Laboratorio",
          addresses: ["10.20.0.7/24"],
          endpoints: ["203.0.113.40:51820"],
          dns: [],
          full_tunnel: false,
          peers: 1,
        },
      ];

/** Status of the daemon with no tunnel connected. */
const idle: Status = {
  state: "disconnected",
  tunnel_id: null,
  interface: null,
  peers: [],
  error: null,
  connected_since: null,
  retry_at: null,
  paused_until: null,
  blocked: false,
  protection_error: null,
};
// state of the fake daemon
let status: Status = idle;
let settings: Settings = { kill_switch: "off", allow_lan: false, split_mode: "off", split_apps: [], prefer_tunnel: false, auto_connect: null };
let prefs: Prefs = {
  notifications: true,
  language: "system",
  launch_at_login: false,
  close_to_tray: true,
  disconnect_on_quit: false,
  resolved_language: browserLang(),
};
/** Log buffer, served by get_logs. */
const logs: LogLine[] = [];
/** Appends a log line and pushes it to the subscribers, like the daemon does. */
function log(level: LogLine["level"], target: string, message: string) {
  const line = { time: Date.now(), level, target, message };
  logs.push(line);
  emit({ type: "log_line", data: line });
}
/** With the "always" kill switch traffic stays blocked even without a tunnel. */
const blockedWhenIdle = () => settings.kill_switch === "always";
/** Subscribers to daemon events. */
const handlers = new Set<(event: DaemonEvent) => void>();
/** Interval updating the traffic counters while connected. */
let ticker: number | undefined;
/** End of the pause in progress, if any. */
let resume: number | undefined;

/** Pushes an event to every subscriber. */
function emit(event: DaemonEvent) {
  handlers.forEach((h) => h(event));
}

// sample lines, so the Log view is not empty
log("info", "submarine_daemon", "submarine daemon ready socket=/run/submarine/daemon.sock");
log("warn", "submarine_net::linux::dns", "dns restore failed: interface gone");

/** Changes the status and notifies the subscribers. */
function setStatus(next: Status) {
  status = next;
  emit({ type: "status_changed", data: next });
}

/** Resolves after the given milliseconds, to imitate the latency of the daemon. */
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/**
 * Answers one request like the daemon would.
 *
 * NB: errors are thrown as plain strings, the way Tauri rejects failed commands.
 */
async function handle(req: Request): Promise<Response> {
  await sleep(120);
  switch (req.method) {
    case "get_status":
      return { type: "status", data: status };
    case "list_tunnels":
      return { type: "tunnels", data: tunnels };
    case "import_tunnel": {
      // the configuration is only checked for its section, the tunnel summary is made up
      if (!req.params.config.includes("[Interface]")) throw "missing [Interface] section";
      const tunnel: TunnelInfo = {
        id: crypto.randomUUID(),
        name: req.params.name.trim(),
        addresses: ["10.0.0.2/32"],
        endpoints: ["example.net:51820"],
        dns: [],
        full_tunnel: true,
        peers: 1,
      };
      tunnels = [...tunnels, tunnel];
      emit({ type: "tunnels_changed", data: tunnels });
      return { type: "imported", data: { tunnel, warnings: ["line 7: PostUp scripts are not supported and will not be executed"] } };
    }
    case "get_tunnel_config": {
      const tunnel = tunnels.find((t) => t.id === req.params.id);
      if (!tunnel) throw "unknown tunnel";
      const config = [
        "[Interface]",
        `PrivateKey = ${HIDDEN}`,
        `Address = ${tunnel.addresses.join(", ")}`,
        ...(tunnel.dns.length ? [`DNS = ${tunnel.dns.join(", ")}`] : []),
        "",
        "[Peer]",
        "PublicKey = xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=",
        `AllowedIPs = ${tunnel.full_tunnel ? "0.0.0.0/0, ::/0" : "10.20.0.0/24"}`,
        `Endpoint = ${tunnel.endpoints[0]}`,
        "PersistentKeepalive = 25",
        "",
      ].join("\n");
      return { type: "tunnel_config", data: { name: tunnel.name, config } };
    }
    case "update_tunnel": {
      const { id, name, config } = req.params;
      if (!config.includes("[Interface]")) throw "missing [Interface] section";
      // just enough parsing of `Key = a, b` lines to update the summary
      const value = (key: string) =>
        config
          .match(new RegExp(`^\\s*${key}\\s*=(.*)$`, "im"))?.[1]
          ?.split(",")
          .map((x) => x.trim())
          .filter(Boolean) ?? [];
      tunnels = tunnels.map((t) =>
        t.id === id
          ? {
              ...t,
              name: name.trim(),
              addresses: value("Address"),
              dns: value("DNS"),
              endpoints: value("Endpoint"),
              full_tunnel: value("AllowedIPs").some((x) => x.endsWith("/0")),
            }
          : t,
      );
      emit({ type: "tunnels_changed", data: tunnels });
      return { type: "imported", data: { tunnel: tunnels.find((t) => t.id === id)!, warnings: [] } };
    }
    case "delete_tunnel":
      tunnels = tunnels.filter((t) => t.id !== req.params.id);
      emit({ type: "tunnels_changed", data: tunnels });
      return { type: "ok" };
    case "connect": {
      const id = req.params.id;
      // a slow handshake, then traffic counters that grow every second
      clearTimeout(resume);
      setStatus({ ...idle, state: "connecting", tunnel_id: id, blocked: settings.kill_switch !== "off" });
      await sleep(1400);
      let rx = 0;
      let tx = 0;
      const now = () => Math.floor(Date.now() / 1000);
      const peer = () => ({
        public_key: "xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=",
        endpoint: "203.0.113.10:51820",
        last_handshake: now() - 4,
        rx_bytes: rx,
        tx_bytes: tx,
      });
      setStatus({
        ...idle,
        state: "connected",
        tunnel_id: id,
        interface: "submarine0",
        peers: [peer()],
        connected_since: now(),
      });
      log("info", "submarine_tunnel", `tunnel started name=submarine0 tunnel=${id}`);
      clearInterval(ticker);
      ticker = window.setInterval(() => {
        rx += 48_000 + Math.random() * 120_000;
        tx += 9_000 + Math.random() * 30_000;
        setStatus({ ...status, peers: [peer()] });
      }, 1000);
      return { type: "ok" };
    }
    case "pause": {
      // the fake tunnel stops and comes back by itself at the end of the pause
      const id = status.tunnel_id;
      if (!id || !["connected", "connecting", "reconnecting", "paused"].includes(status.state)) {
        throw "there is no connection to pause";
      }
      clearInterval(ticker);
      clearTimeout(resume);
      const until = Math.floor(Date.now() / 1000) + req.params.seconds;
      setStatus({ ...idle, state: "paused", tunnel_id: id, paused_until: until });
      resume = window.setTimeout(() => void handle({ method: "connect", params: { id } }), req.params.seconds * 1000);
      return { type: "ok" };
    }
    case "disconnect":
      clearInterval(ticker);
      clearTimeout(resume);
      setStatus({ ...status, state: "disconnecting" });
      await sleep(500);
      setStatus({ ...idle, blocked: blockedWhenIdle() });
      return { type: "ok" };
    case "get_settings":
      return { type: "settings", data: settings };
    case "get_logs":
      return { type: "logs", data: logs.slice(-1000) };
    case "clear_logs":
      logs.length = 0;
      return { type: "ok" };
    case "set_settings":
      settings = req.params.settings;
      emit({ type: "settings_changed", data: settings });
      if (status.state !== "connected") setStatus({ ...status, blocked: blockedWhenIdle() });
      return { type: "settings", data: settings };
  }
}

/** Fake daemon; in the "offline" scenario every request fails. */
export const mockDaemon: Daemon = {
  async request(req) {
    if (mode === "offline") throw "The Submarine service is not running";
    return handle(req);
  },
  isConnected: async () => mode !== "offline",
  getPrefs: async () => prefs,
  setPrefs: async (next) => {
    prefs = { ...next, resolved_language: next.language === "system" ? browserLang() : next.language };
    return prefs;
  },
  listApps: async () => [
    { name: "Firefox", path: "/usr/lib/firefox/firefox" },
    { name: "Thunderbird", path: "/usr/lib/thunderbird/thunderbird" },
    { name: "Visual Studio Code", path: "/usr/share/code/code" },
    { name: "Spotify", path: "/usr/share/spotify/spotify" },
    { name: "Terminale", path: "/usr/bin/gnome-terminal" },
  ],
  async onEvent(handler) {
    handlers.add(handler);
    return () => handlers.delete(handler);
  },
  onConnection: async () => () => {},
};

/**
 * Platform services for the browser. The browser has no paths: "Choose file" falls back to
 * a file input, and files dropped on the window are not supported.
 */
export const mockPlatform: Platform = {
  pickConfigFile: () =>
    new Promise<ConfigFile | null>((resolve) => {
      const input = document.createElement("input");
      input.type = "file";
      input.accept = ".conf,text/plain";
      input.onchange = async () => {
        const file = input.files?.[0];
        resolve(file ? { name: file.name, text: await file.text() } : null);
      };
      input.oncancel = () => resolve(null);
      input.click();
    }),
  readConfigFile: async () => {
    throw "not available in the browser";
  },
  onFileDrag: async () => () => {},
  copyText: (text) => navigator.clipboard.writeText(text),
  appVersion: async () => packageJson.version,
};

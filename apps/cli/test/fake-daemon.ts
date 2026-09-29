// In-memory submarine-daemon speaking the real wire protocol over a Unix socket.
// Connecting goes through the same events as the real service: `connecting`,
// the "tunnel started" log line, `connected`, then the first handshake and
// statistics every `statsMs`. Used by the tests and by the demo; tests drive it
// through its public fields (tunnels, settings, `failNext`, `silentServer`).

import { mkdtempSync } from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";

import type { DaemonEvent, LogLine, Request, Response, Settings, Status, TunnelInfo } from "../src/protocol.ts";

/** Status of a service with no tunnel. */
export const idle: Status = {
  state: "disconnected",
  tunnel_id: null,
  interface: null,
  peers: [],
  error: null,
  blocked: false,
  protection_error: null,
};

/** Resolves after `ms` milliseconds. */
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Timings of the fake daemon. */
export interface FakeOptions {
  /** Pause between the steps of a connection. */
  stepMs?: number;
  /** Interval of the statistics while connected. */
  statsMs?: number;
}

/** The fake service; `start` listens on a socket in a fresh temporary directory. */
export class FakeDaemon {
  /** Socket path, to pass to `DaemonClient.connect` or `SUBMARINE_SOCKET`. */
  path = path.join(mkdtempSync(path.join(os.tmpdir(), "submarine-cli-")), "daemon.sock");
  /** Every request received, without its id. */
  requests: Request[] = [];
  tunnels: TunnelInfo[] = [
    { id: "a1b2c3", name: "Ufficio Milano", addresses: ["10.8.0.2/32"], endpoints: ["vpn.example.it:51820"], dns: ["10.8.0.1"], full_tunnel: true, peers: 1 },
    { id: "d4e5f6", name: "Laboratorio", addresses: ["10.20.0.7/24"], endpoints: ["203.0.113.40:51820"], dns: [], full_tunnel: false, peers: 1 },
  ];
  status: Status = { ...idle };
  settings: Settings = { kill_switch: "off", allow_lan: false, split_mode: "off", split_apps: [] };
  logs: LogLine[] = [{ time: Date.now(), level: "info", target: "submarine_daemon", message: "submarine daemon ready socket=submarine" }];
  /** The next connection fails with this error, like an unknown host. */
  failNext: string | null = null;
  /** The server never answers the handshake. */
  silentServer = false;
  private stepMs: number;
  private statsMs: number;
  /** Bumped by every connect, disconnect and stop: a stale connection attempt gives up. */
  private generation = 0;
  /** Timer of the statistics while connected. */
  private stats: ReturnType<typeof setInterval> | undefined;
  /** End of the pause in progress, which connects the tunnel again. */
  private resume: ReturnType<typeof setTimeout> | undefined;
  private server = net.createServer((socket) => this.serve(socket));
  /** Connected clients, which receive the events. */
  private sockets = new Set<net.Socket>();

  constructor({ stepMs = 20, statsMs = 1000 }: FakeOptions = {}) {
    this.stepMs = stepMs;
    this.statsMs = statsMs;
  }

  /** Starts listening; returns the daemon itself, for chaining. */
  async start(): Promise<this> {
    await new Promise<void>((resolve) => this.server.listen(this.path, resolve));
    return this;
  }

  /** Stops the connection in progress and the statistics, and closes every client. */
  async stop() {
    clearInterval(this.stats);
    clearTimeout(this.resume);
    this.generation++;
    for (const s of this.sockets) s.destroy();
    await new Promise<void>((resolve) => this.server.close(() => resolve()));
  }

  /** Sends an event to every client. */
  emit(event: DaemonEvent) {
    const line = JSON.stringify({ kind: "event", event }) + "\n";
    for (const s of this.sockets) s.write(line);
  }

  /** Adds a line to the log, keeping the latest 200, and sends it as an event. */
  log(level: string, message: string) {
    const line = { time: Date.now(), level, target: "submarine_daemon", message };
    this.logs = [...this.logs, line].slice(-200);
    this.emit({ type: "log_line", data: line });
  }

  /** Changes the status and sends it as an event. */
  private setStatus(status: Status) {
    this.status = status;
    this.emit({ type: "status_changed", data: status });
  }

  /** Serves a client: one JSON request per line, answered with its id. */
  private serve(socket: net.Socket) {
    this.sockets.add(socket);
    socket.on("close", () => this.sockets.delete(socket));
    let buffer = "";
    socket.setEncoding("utf8");
    socket.on("data", (chunk: string) => {
      buffer += chunk;
      let i: number;
      while ((i = buffer.indexOf("\n")) >= 0) {
        const { id, ...req } = JSON.parse(buffer.slice(0, i)) as { id: number } & Request;
        buffer = buffer.slice(i + 1);
        this.requests.push(req);
        this.handle(req).then(
          (ok) => socket.write(JSON.stringify({ kind: "response", id, result: { Ok: ok } }) + "\n"),
          (err) => socket.write(JSON.stringify({ kind: "response", id, result: { Err: String((err as Error).message) } }) + "\n"),
        );
      }
    });
  }

  /**
   * Connection with the steps of the real service, `stepMs` apart; it answers once
   * connected, before the handshake.
   */
  private async connect(id: string) {
    const tunnel = this.tunnels.find((t) => t.id === id);
    if (!tunnel) throw new Error("unknown tunnel");
    // a new attempt supersedes the previous one
    clearInterval(this.stats);
    const generation = ++this.generation;
    const current = () => generation === this.generation;
    this.setStatus({ ...idle, state: "connecting", tunnel_id: id });
    await sleep(this.stepMs);
    if (!current()) return;
    // a failure planned by the test, e.g. an endpoint that does not resolve
    if (this.failNext) {
      const error = this.failNext;
      this.failNext = null;
      this.log("error", `connect failed: ${error}`);
      this.setStatus({ ...idle, state: "failed", tunnel_id: id, error });
      throw new Error(error);
    }
    // tunnel started, then routes and firewall
    this.log("info", "tunnel started name=submarine0 port=51820");
    await sleep(this.stepMs);
    if (!current()) return;
    this.log("info", "routes applied interface=submarine0");
    this.log("info", `firewall applied allow_lan=${this.settings.allow_lan}`);
    // a hostname endpoint is shown resolved to a documentation address
    const endpoint = tunnel.endpoints[0]?.replace(/^[^:]+/, (h) => (/^\d/.test(h) ? h : "203.0.113.10")) ?? null;
    const peer = { public_key: "k", endpoint, last_handshake: null as number | null, rx_bytes: 0, tx_bytes: 0 };
    this.setStatus({
      ...idle,
      state: "connected",
      tunnel_id: id,
      interface: "submarine0",
      peers: [peer],
      connected_since: Math.floor(Date.now() / 1000),
    });
    // first handshake after one more step, then statistics every `statsMs`
    setTimeout(() => {
      if (!current() || this.silentServer) return;
      peer.last_handshake = Math.floor(Date.now() / 1000);
      peer.rx_bytes = 1_234_000;
      peer.tx_bytes = 56_000;
      this.setStatus({ ...this.status, peers: [{ ...peer }] });
      this.stats = setInterval(() => {
        peer.rx_bytes += Math.round(20_000 + Math.random() * 180_000);
        peer.tx_bytes += Math.round(4_000 + Math.random() * 50_000);
        if (Math.random() < 0.1) peer.last_handshake = Math.floor(Date.now() / 1000);
        this.setStatus({ ...this.status, peers: [{ ...peer }] });
      }, this.statsMs);
    }, this.stepMs);
  }

  /** Answers a request like the real service; rejections become `Err` responses. */
  private async handle(req: Request): Promise<Response> {
    switch (req.method) {
      case "get_status":
        return { type: "status", data: this.status };
      case "list_tunnels":
        return { type: "tunnels", data: this.tunnels };
      case "get_settings":
        return { type: "settings", data: this.settings };
      case "set_settings":
        this.settings = req.params.settings;
        this.log("info", `settings saved kill_switch=${this.settings.kill_switch}`);
        this.emit({ type: "settings_changed", data: this.settings });
        return { type: "settings", data: this.settings };
      case "connect":
        clearTimeout(this.resume);
        await this.connect(req.params.id);
        return { type: "ok" };
      case "pause": {
        const id = this.status.tunnel_id;
        if (!id || !["connecting", "connected", "reconnecting", "paused"].includes(this.status.state)) {
          throw new Error("there is no connection to pause");
        }
        clearInterval(this.stats);
        clearTimeout(this.resume);
        this.generation++;
        const until = Math.floor(Date.now() / 1000) + req.params.seconds;
        this.setStatus({ ...idle, state: "paused", tunnel_id: id, paused_until: until });
        this.resume = setTimeout(() => void this.connect(id), req.params.seconds * 1000);
        return { type: "ok" };
      }
      case "disconnect":
        clearInterval(this.stats);
        clearTimeout(this.resume);
        this.generation++;
        if (this.status.state !== "disconnected") this.log("info", "tunnel stopped name=submarine0");
        this.setStatus({ ...idle });
        return { type: "ok" };
      case "get_logs":
        return { type: "logs", data: this.logs };
      case "clear_logs":
        this.logs = [];
        return { type: "ok" };
      case "import_tunnel": {
        if (!req.params.config.includes("[Interface]")) throw new Error("missing [Interface] section");
        // a copy of the second tunnel with the new name, and a fixed warning
        const tunnel = { ...this.tunnels[1], id: "new123", name: req.params.name };
        this.tunnels = [...this.tunnels, tunnel];
        return { type: "imported", data: { tunnel, warnings: ["line 5: PostUp scripts are not supported and will not be executed"] } };
      }
      case "delete_tunnel":
        if (this.status.tunnel_id === req.params.id) throw new Error("disconnect before deleting this tunnel");
        this.tunnels = this.tunnels.filter((t) => t.id !== req.params.id);
        return { type: "ok" };
    }
  }
}

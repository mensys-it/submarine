// Client side of the IPC with submarine-daemon: a Unix socket on Linux and macOS,
// a named pipe on Windows, carrying newline-delimited JSON. Used by the commands
// and by the interactive prompt.

import { EventEmitter } from "node:events";
import net from "node:net";

import type { DaemonEvent, Request, Response, ServerMessage } from "./protocol.ts";

/**
 * Path of the service socket, same defaults as crates/submarine-ipc; `SUBMARINE_SOCKET`
 * overrides them. On Windows it may be a bare pipe name or a full `\\.\pipe\...` path.
 */
export function socketPath(env = process.env, platform = process.platform): string {
  const custom = env.SUBMARINE_SOCKET;
  if (platform === "win32") {
    const name = custom ?? "submarine";
    return name.startsWith("\\\\") ? name : `\\\\.\\pipe\\${name}`;
  }
  if (custom) return custom;
  return platform === "darwin" ? "/var/run/submarine/daemon.sock" : "/run/submarine/daemon.sock";
}

/** The service is unreachable, went away, or rejected a request; the message is for the user. */
export class ServiceError extends Error {}

/** Callbacks of a request waiting for its response. */
interface Pending {
  resolve(response: Response): void;
  reject(error: Error): void;
}

/**
 * Connection to submarine-daemon: newline-delimited JSON, responses matched
 * by id, events emitted as "event". Emits "close" when the service goes away.
 */
export class DaemonClient extends EventEmitter<{ event: [DaemonEvent]; close: [] }> {
  /** Received text not yet terminated by a newline. */
  private buffer = "";
  /** Id of the next request. */
  private nextId = 1;
  /** Requests waiting for a response, by id. */
  private pending = new Map<number, Pending>();

  private constructor(private socket: net.Socket) {
    super();
    socket.setEncoding("utf8");
    socket.on("data", (chunk: string) => this.onData(chunk));
    // the requests still waiting fail together when the service goes away
    socket.on("close", () => {
      for (const p of this.pending.values()) p.reject(new ServiceError("Il servizio Submarine si è disconnesso"));
      this.pending.clear();
      this.emit("close");
    });
    // errors are followed by "close", handled above; an unhandled "error" event would throw
    socket.on("error", () => {});
  }

  /**
   * Connects to the service.
   *
   * @throws {ServiceError} When the socket cannot be reached, with a hint for the user.
   */
  static connect(path = socketPath()): Promise<DaemonClient> {
    return new Promise((resolve, reject) => {
      const socket = net.connect(path);
      socket.once("connect", () => resolve(new DaemonClient(socket)));
      socket.once("error", (err: NodeJS.ErrnoException) => {
        const hint =
          err.code === "EACCES"
            ? "permesso negato: su Linux aggiungi il tuo utente al gruppo submarine"
            : "il servizio non è in esecuzione";
        reject(new ServiceError(`Impossibile raggiungere il servizio Submarine (${hint})`));
      });
    });
  }

  /**
   * Sends a request and waits for its response, which must be of type `expected`.
   *
   * @throws {ServiceError} When the service rejects the request, answers with another type
   * or disconnects meanwhile.
   */
  async request<T extends Response["type"]>(req: Request, expected: T): Promise<Extract<Response, { type: T }>> {
    const id = this.nextId++;
    const response = await new Promise<Response>((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.socket.write(JSON.stringify({ id, ...req }) + "\n");
    });
    if (response.type !== expected) {
      throw new ServiceError(`Risposta inattesa dal servizio: ${response.type}`);
    }
    return response as Extract<Response, { type: T }>;
  }

  /** Closes the connection; the requests still waiting fail. */
  close(): void {
    this.socket.end();
  }

  /** Splits the received text into lines, one JSON message each. */
  private onData(chunk: string) {
    this.buffer += chunk;
    let newline: number;
    while ((newline = this.buffer.indexOf("\n")) >= 0) {
      const line = this.buffer.slice(0, newline);
      this.buffer = this.buffer.slice(newline + 1);
      if (line.trim()) this.onMessage(JSON.parse(line) as ServerMessage);
    }
  }

  /** Emits an event, or settles the request a response belongs to. */
  private onMessage(message: ServerMessage) {
    if (message.kind === "event") {
      this.emit("event", message.event);
      return;
    }
    // a response nobody is waiting for is dropped
    const pending = this.pending.get(message.id);
    if (!pending) return;
    this.pending.delete(message.id);
    if ("Ok" in message.result) pending.resolve(message.result.Ok);
    else pending.reject(new ServiceError(message.result.Err));
  }
}

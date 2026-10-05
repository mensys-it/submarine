// Client side of the IPC with submarine-daemon: a Unix socket on Linux and macOS,
// a named pipe on Windows, carrying newline-delimited JSON. Used by the commands
// and by the interactive prompt.
//
// On Windows the pipe is reached through `submarine-daemon pipe-proxy`: Node and Bun
// open a pipe asking for GENERIC_WRITE, which includes the right to create pipe
// instances that the pipe's ACL denies to users, and cannot check who owns it.

import { type ChildProcess, spawn } from "node:child_process";
import { EventEmitter } from "node:events";
import { existsSync } from "node:fs";
import net from "node:net";
import path from "node:path";

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

/**
 * Command line of the pipe proxy: `submarine-daemon.exe` next to this executable, as
 * the installer puts them, otherwise the one on the PATH.
 */
export function proxyCommand(execPath = process.execPath): string {
  const local = path.join(path.dirname(execPath), "submarine-daemon.exe");
  return existsSync(local) ? local : "submarine-daemon";
}

/** The service is unreachable, went away, or rejected a request; the message is for the user. */
export class ServiceError extends Error {}

/** What the client needs from a connection: a socket, or the standard streams of the proxy. */
interface Channel {
  write(text: string): void;
  end(): void;
  /** Registers the receiver of the incoming text and of the end of the connection. */
  listen(onData: (chunk: string) => void, onClose: () => void): void;
}

/** A Unix socket or named pipe opened directly. */
function socketChannel(socket: net.Socket): Channel {
  socket.setEncoding("utf8");
  // errors are followed by "close"; an unhandled "error" event would throw
  socket.on("error", () => {});
  return {
    write: (text) => socket.write(text),
    end: () => socket.end(),
    listen(onData, onClose) {
      socket.on("data", onData);
      socket.on("close", onClose);
    },
  };
}

/** The standard streams of a running `pipe-proxy`. */
function proxyChannel(child: ChildProcess): Channel {
  const { stdin, stdout } = child;
  if (!stdin || !stdout) throw new Error("pipe proxy without standard streams");
  stdout.setEncoding("utf8");
  // a proxy that already exited makes writes fail: the "close" below reports it
  stdin.on("error", () => {});
  return {
    write: (text) => stdin.write(text),
    end: () => stdin.end(),
    listen(onData, onClose) {
      stdout.on("data", onData);
      child.on("close", onClose);
    },
  };
}

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

  private constructor(private channel: Channel) {
    super();
    // the requests still waiting fail together when the service goes away
    channel.listen(
      (chunk) => this.onData(chunk),
      () => {
        for (const p of this.pending.values()) p.reject(new ServiceError("Il servizio Submarine si è disconnesso"));
        this.pending.clear();
        this.emit("close");
      },
    );
  }

  /**
   * Connects to the service at `path`, or by default to the local one: on Windows
   * through the pipe proxy, elsewhere directly.
   *
   * @throws {ServiceError} When the service cannot be reached, with a hint for the user.
   */
  static connect(path?: string): Promise<DaemonClient> {
    if (path === undefined && process.platform === "win32") return DaemonClient.viaProxy();
    return DaemonClient.direct(path ?? socketPath());
  }

  /** Opens the socket at `path`. */
  private static direct(path: string): Promise<DaemonClient> {
    return new Promise((resolve, reject) => {
      const socket = net.connect(path);
      socket.once("connect", () => resolve(new DaemonClient(socketChannel(socket))));
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
   * Starts `command pipe-proxy` and waits for its first output, the empty line sent
   * once it is connected; its error output becomes the hint when it exits before.
   */
  static viaProxy(command = proxyCommand()): Promise<DaemonClient> {
    return new Promise((resolve, reject) => {
      const child = spawn(command, ["pipe-proxy"], { stdio: ["pipe", "pipe", "pipe"], windowsHide: true });
      let errors = "";
      child.stderr?.setEncoding("utf8");
      child.stderr?.on("data", (chunk: string) => (errors += chunk));
      const fail = (hint: string) =>
        reject(new ServiceError(`Impossibile raggiungere il servizio Submarine (${hint})`));
      child.once("error", () => fail(`${command} non trovato`));
      child.once("exit", () => fail(errors.trim() || "il servizio non è in esecuzione"));
      child.stdout?.setEncoding("utf8");
      child.stdout?.once("data", (chunk: string) => {
        child.removeAllListeners("exit");
        const client = new DaemonClient(proxyChannel(child));
        // the first chunk may already hold the first events after the empty line
        client.onData(chunk);
        resolve(client);
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
      this.channel.write(JSON.stringify({ id, ...req }) + "\n");
    });
    if (response.type !== expected) {
      throw new ServiceError(`Risposta inattesa dal servizio: ${response.type}`);
    }
    return response as Extract<Response, { type: T }>;
  }

  /** Closes the connection; the requests still waiting fail. */
  close(): void {
    this.channel.end();
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

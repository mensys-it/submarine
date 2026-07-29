// A connection followed step by step. The service reports no steps, so they
// are read from what it does publish: the "tunnel started" log line (the
// endpoints are resolved and the interface is up), the `connected` state
// (routes, DNS and firewall applied) and the first handshake in the
// statistics. A failure comes as the `failed` state or the request's error.
// Pure functions: App keeps the current run and feeds it the service events.

import { describeConnected, routingLabel } from "../commands.ts";
import { head, type Line, marks, sub } from "../lines.ts";
import type { DaemonEvent, Status, TunnelInfo } from "../protocol.ts";
import { SPIN } from "./theme.ts";

/** State of a step, as drawn: waiting, with a spinner, ticked or crossed. */
export type StepState = "pending" | "active" | "done" | "fail";
/** Steps of a connection: tunnel start, routes and firewall, first handshake. */
export type StepKey = "start" | "routes" | "handshake";
/** How a run ended; `no_handshake` means connected, but the server has not answered. */
export type Result = "ok" | "no_handshake" | "error" | "cancelled";

/** A connection attempt followed by the prompt. */
export interface ConnectRun {
  /** Identifies the attempt, so late answers to an older one are ignored. */
  id: number;
  tunnel: TunnelInfo;
  /** Endpoint host, without the port. */
  host: string | null;
  /** The host is an IP address, so there is no name to resolve. */
  isIp: boolean;
  steps: Record<StepKey, StepState>;
  /** Unix seconds of the start. */
  since: number;
  /** Latest status of this tunnel. */
  status: Status | null;
  /** Set once the run is over. */
  result?: Result;
  /** Error message of a failed run. */
  error?: string;
}

/** The steps in order. */
export const STEPS: StepKey[] = ["start", "routes", "handshake"];
/** After this long without a handshake the run ends with a warning. */
export const HANDSHAKE_WAIT_MS = 15_000;

/** Host of an endpoint (`host:port` or `[ipv6]:port`) and whether it is an IP address. */
function splitHost(endpoint: string | undefined): { host: string | null; isIp: boolean } {
  if (!endpoint) return { host: null, isIp: false };
  const host = endpoint.startsWith("[") ? endpoint.slice(1, endpoint.indexOf("]")) : endpoint.replace(/:\d+$/, "");
  return { host, isIp: /^\d+\.\d+\.\d+\.\d+$/.test(host) || host.includes(":") };
}

/** A new run at its first step; `now` is in Unix seconds. */
export function startRun(id: number, tunnel: TunnelInfo, now = Date.now() / 1000): ConnectRun {
  return {
    id,
    tunnel,
    ...splitHost(tunnel.endpoints[0]),
    steps: { start: "active", routes: "pending", handshake: "pending" },
    since: now,
    status: null,
  };
}

/** Ends the run; on an error the step in progress is marked as failed. */
function finish(run: ConnectRun, result: Result, error?: string): ConnectRun {
  if (result === "error") {
    const failed = STEPS.find((k) => run.steps[k] === "active");
    const steps = failed ? { ...run.steps, [failed]: "fail" as const } : run.steps;
    return { ...run, steps, result, error };
  }
  return { ...run, result };
}

/** Marks `key` and the steps before it done, and the next step active. */
function complete(run: ConnectRun, key: StepKey): ConnectRun {
  const steps = { ...run.steps };
  const i = STEPS.indexOf(key);
  STEPS.forEach((k, j) => {
    if (j <= i) steps[k] = "done";
    else if (j === i + 1 && steps[k] === "pending") steps[k] = "active";
  });
  return { ...run, steps };
}

/**
 * A handshake happened during this run. The second of tolerance covers the rounding
 * of `since` and the clocks of the CLI and the service.
 */
function handshakeSeen(run: ConnectRun, st: Status): boolean {
  const last = st.peers.find((p) => p.last_handshake)?.last_handshake;
  return last != null && last >= Math.floor(run.since) - 1;
}

/** Applies an event of the service; returns the same run when nothing changes. */
export function onEvent(run: ConnectRun, event: DaemonEvent): ConnectRun {
  if (run.result) return run;
  // the log line marks the end of the first step
  if (event.type === "log_line") {
    if (event.data.message.startsWith("tunnel started") && run.steps.start === "active") return complete(run, "start");
    return run;
  }
  if (event.type !== "status_changed") return run;
  // only the status of this tunnel counts
  const st = event.data;
  if (st.tunnel_id !== run.tunnel.id) return run;
  let next: ConnectRun = { ...run, status: st };
  if (st.state === "failed") return finish(next, "error", st.error ?? undefined);
  if (st.state === "connected") {
    next = complete(next, "routes");
    if (handshakeSeen(next, st)) next = finish(complete(next, "handshake"), "ok");
  }
  return next;
}

/** Ends the run with an error, unless it is already over. */
export const failRun = (run: ConnectRun, message: string) => (run.result ? run : finish(run, "error", message));
/** Ends the run as cancelled, unless it is already over. */
export const cancelRun = (run: ConnectRun) => (run.result ? run : finish(run, "cancelled"));
/** The tunnel is up but the server has not answered yet. */
export const timeoutRun = (run: ConnectRun) => (run.result ? run : finish(run, "no_handshake"));

/** Text of a step in progress or done; the endpoint IP comes from the latest status. */
function stepText(run: ConnectRun, key: StepKey, state: StepState): string {
  const ip = run.status?.peers[0]?.endpoint?.replace(/:\d+$/, "").replace(/^\[|\]$/g, "");
  switch (key) {
    case "start":
      if (!run.host) return state === "done" ? "Tunnel avviato" : "Avvio il tunnel…";
      if (run.isIp) return state === "done" ? `Tunnel verso ${run.host} avviato` : `Avvio il tunnel verso ${run.host}…`;
      if (state === "done") return ip && ip !== run.host ? `Risolto ${run.host} → ${ip}` : `Risolto ${run.host}`;
      return `Risolvo ${run.host}…`;
    case "routes":
      return state === "done" ? "Rotte e firewall applicati" : "Applico rotte e firewall…";
    case "handshake":
      return state === "done" ? "Handshake completato" : "Handshake con il server…";
  }
}

/** Sweep under the active step: 16 dashes with a bright stretch moving right. */
function sweep(t: number): Line {
  const pos = (t % 22) - 4;
  let a = "";
  let b = "";
  let c = "";
  for (let i = 0; i < 16; i++) {
    if (i < pos) a += "━";
    else if (i < pos + 4) b += "━";
    else c += "━";
  }
  return [["  ", "fg"], [a, "faint"], [b, "acc"], [c, "faint"]];
}

/** The run as lines: live with spinner (`t` = frame), or final once it has a result. */
export function runLines(run: ConnectRun, t: number, animate: boolean): Line[] {
  // one line per step reached so far
  const out: Line[] = [];
  for (const key of STEPS) {
    const state = run.steps[key];
    if (state === "done") out.push([[marks.ok, "acc"], [stepText(run, key, state), "fg"]]);
    else if (state === "fail") {
      out.push([[marks.fail, "red"], [stepText(run, key, "active").replace("…", ""), "fg"], [" non riuscito", "red"]]);
    } else if (state === "active" && !run.result) {
      const line: Line = [[`${SPIN[t % SPIN.length]} `, "yel"], [stepText(run, key, state), "fg"]];
      out.push(animate ? [...line, ...sweep(t)] : line);
    } else if (state === "active" && run.result === "no_handshake") {
      out.push([[marks.warn, "yel"], ["Nessun handshake dal server per ora", "fg"]]);
    }
  }
  // the outcome below the steps, once the run is over
  const name = run.tunnel.name;
  switch (run.result) {
    case "ok":
      out.push(...describeConnected(run.tunnel, run.status!));
      break;
    case "no_handshake":
      out.push(head([marks.on, "acc"], `Connesso a ${name}`));
      out.push(sub("il server non ha ancora risposto: controlla che sia raggiungibile e che le chiavi siano giuste", "yel"));
      out.push(sub(`instradamento: ${routingLabel(run.tunnel)}`));
      break;
    case "error": {
      const message = run.error ?? "errore sconosciuto";
      // a name resolution failure gets a specific explanation
      if (/failed to resolve endpoint/i.test(message)) {
        out.push([[marks.fail, "red"], ["Non trovo il server", "redB"]]);
        out.push(sub(`${run.host} non risponde: controlla internet o il DNS del server`, "fg"));
      } else {
        out.push([[marks.fail, "red"], ["Connessione non riuscita", "redB"]]);
      }
      out.push(sub(message, "faint"));
      out.push([[marks.detail, "faint"], ["riprova con ", "dim"], [`/connect ${name}`, "acc"]]);
      break;
    }
    case "cancelled":
      out.push([[marks.wait, "dim"], ["Annullato", "dim"]]);
      break;
  }
  return out;
}

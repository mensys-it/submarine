// Commands of the CLI, shared by the interactive prompt (`/connect`) and script
// mode (`submarine connect`). Each command talks to submarine-daemon through a
// `DaemonClient` and returns an `Outcome`: lines to print, raw data for `--json`,
// or a choice to offer when an argument is missing. The texts shown to the user
// are in Italian.

import { readdir, readFile, stat } from "node:fs/promises";
import path from "node:path";

import { formatAgo, formatBytes, formatClock, formatDuration } from "./format.ts";
import type { DaemonClient } from "./ipc.ts";
import { head, type Line, marks, sub } from "./lines.ts";
import type { KillSwitch, Settings, SplitTunnelMode, Status, TunnelInfo } from "./protocol.ts";

/** A question offered in the interactive prompt when an argument is missing. */
export interface Choice {
  /** Question shown above the options. */
  title: string;
  /** Options in display order; `value` is what `choose` receives. */
  options: { label: string; hint?: string; value: string }[];
  /** Runs the command with the chosen value; the outcome may chain another choice. */
  choose(value: string): Promise<Outcome>;
}

/** Result of a command, rendered by the prompt or by script mode. */
export interface Outcome {
  /** Lines to print. */
  lines: Line[];
  /** Raw service data printed by `--json`. */
  json?: unknown;
  /** Question to ask before going on (interactive mode only). */
  choice?: Choice;
  /** Interactive mode: the prompt follows the connection step by step. */
  connect?: TunnelInfo;
  /** The prompt clears the screen. */
  clear?: boolean;
  /** The prompt exits. */
  exit?: boolean;
}

/** Context of a command run. */
export interface Session {
  /** Connection to the service. */
  client: DaemonClient;
  /** True in the interactive prompt, false in script mode. */
  interactive: boolean;
  /** Script mode: confirmation for destructive commands. */
  yes: boolean;
}

/** A command of the CLI, available as `/name` in the prompt and `submarine name` in scripts. */
export interface Command {
  /** Name typed by the user, lowercase. */
  name: string;
  /** Alternative names, lowercase. */
  aliases?: string[];
  /** Synopsis for the help and the usage errors, without the leading `/` or `submarine`. */
  usage: string;
  /** One-line description for the help and the suggestions. */
  description: string;
  /** Spinner text while the command runs. */
  busy?: string;
  /** Only in the interactive prompt. */
  interactiveOnly?: boolean;
  /** Works without the service. */
  offline?: boolean;
  /** Candidates for everything after the command name. */
  complete?(session: Session, args: string[]): Promise<string[]>;
  /**
   * Runs the command.
   *
   * @throws {UsageError} On a wrong or missing argument.
   * @throws {ServiceError} When the service is unreachable or rejects the request.
   */
  run(session: Session, args: string[]): Promise<Outcome>;
}

/** A mistake of the user: `detail` goes on the line below the message. */
export class UsageError extends Error {
  constructor(
    message: string,
    readonly detail?: string,
  ) {
    super(message);
  }
}

/** Outcome made only of the given lines. */
const lines = (...l: Line[]): Outcome => ({ lines: l });
/** "✓ Title" success line, followed by the given details. */
const done = (title: string, ...rest: Line[]): Line[] => [head([marks.ok, "acc"], title), ...rest];

/** Short name of each kill switch mode, shown in the status and the prompt. */
export const killSwitchLabels: Record<KillSwitch, string> = {
  off: "spento",
  on_connect: "attivo",
  always: "sempre attivo",
};
/** What each kill switch mode does, in plain words. */
const killSwitchHints: Record<KillSwitch, string> = {
  off: "se la VPN cade, il traffico esce fuori dal tunnel",
  on_connect: "se la VPN cade, internet resta bloccato finché non ti disconnetti",
  always: "internet passa solo dal tunnel, anche a VPN spenta",
};
/** What each split tunneling mode does, in plain words. */
const splitLabels: Record<SplitTunnelMode, string> = {
  off: "tutte le app usano il tunnel",
  include: "solo le app scelte usano il tunnel",
  exclude: "tutte tranne le app scelte",
};

/** Which traffic goes through the tunnel: everything or only its networks. */
export const routingLabel = (t: TunnelInfo) => (t.full_tunnel ? "tutto il traffico" : "solo reti del tunnel");

/** Tunnels known to the service. */
async function tunnels(s: Session) {
  return (await s.client.request({ method: "list_tunnels" }, "tunnels")).data;
}
/** Current connection status. */
async function status(s: Session) {
  return (await s.client.request({ method: "get_status" }, "status")).data;
}
/** Current settings. */
async function settings(s: Session) {
  return (await s.client.request({ method: "get_settings" }, "settings")).data;
}
/**
 * Applies `patch` on top of the current settings and returns the saved ones.
 *
 * NB: the read and the write are two requests, so a change made by another client in
 * between is overwritten.
 */
async function saveSettings(s: Session, patch: Partial<Settings>) {
  const current = await settings(s);
  return (await s.client.request({ method: "set_settings", params: { settings: { ...current, ...patch } } }, "settings"))
    .data;
}

/**
 * Finds the tunnel the user means: exact name, then unique name or id prefix;
 * case-insensitive.
 *
 * @throws {UsageError} When nothing matches, or more than one tunnel does.
 */
export function findTunnel(list: TunnelInfo[], query: string): TunnelInfo {
  const q = query.toLowerCase();
  // an exact name wins even when it is also the prefix of other names
  const exact = list.filter((t) => t.name.toLowerCase() === q);
  if (exact.length === 1) return exact[0];
  const prefix = list.filter((t) => t.name.toLowerCase().startsWith(q) || t.id.startsWith(q));
  if (prefix.length === 1) return prefix[0];
  // no match or an ambiguous one: the detail lists the candidates
  const names = list.map((t) => t.name).join(", ");
  if (prefix.length === 0) throw new UsageError(`Nessun tunnel “${query}”`, names ? `disponibili: ${names}` : undefined);
  throw new UsageError(`“${query}” corrisponde a più tunnel`, prefix.map((t) => t.name).join(", "));
}

/** Option of a tunnel choice: name, first endpoint and routing; the value is the id. */
function tunnelOption(t: TunnelInfo, connectedId: string | null) {
  const hint = [t.endpoints[0], routingLabel(t)].filter(Boolean).join(", ");
  return { label: t.id === connectedId ? `${t.name} (connesso)` : t.name, hint, value: t.id };
}

/**
 * Offers `choice` in the interactive prompt; in script mode a missing argument is an error.
 *
 * @throws {UsageError} In script mode, with the command synopsis.
 */
function needChoice(s: Session, usage: string, choice: Choice): Outcome {
  if (!s.interactive) throw new UsageError(`Manca un argomento. Uso: submarine ${usage}`);
  return { lines: [], choice };
}

/** Arguments joined back into one value, for names that contain spaces. */
function joinArgs(args: string[]): string {
  return args.join(" ").trim();
}

/** Bytes received and sent, summed over all the peers. */
export const sumBytes = (st: Status) => ({
  rx: st.peers.reduce((n, p) => n + p.rx_bytes, 0),
  tx: st.peers.reduce((n, p) => n + p.tx_bytes, 0),
});

/**
 * Lines describing the connection status, the kill switch block and any protection error.
 *
 * `list` gives the tunnel name; `now` (Unix seconds) is the reference for the handshake age.
 */
/** Seconds left before the reconnection attempt at `retryAt` (Unix seconds), never negative. */
export const retryIn = (retryAt: number, now = Date.now() / 1000) => Math.max(0, Math.ceil(retryAt - now));

export function describeStatus(st: Status, list: TunnelInfo[], now = Date.now() / 1000): Line[] {
  const name = list.find((t) => t.id === st.tunnel_id)?.name ?? "tunnel";
  const out: Line[] = [];
  switch (st.state) {
    case "connected": {
      out.push(head([marks.on, "acc"], `Connesso a ${name}`));
      if (st.connected_since) out.push(sub(`da ${formatDuration(now - st.connected_since)}`));
      const handshake = st.peers[0]?.last_handshake;
      out.push(sub(handshake ? `handshake ${formatAgo(handshake, now)}` : "nessun handshake per ora"));
      const { rx, tx } = sumBytes(st);
      out.push(sub(`ricevuti ${formatBytes(rx)}, inviati ${formatBytes(tx)}`));
      break;
    }
    case "connecting":
      out.push(head([marks.wait, "yel"], `Connessione a ${name} in corso`));
      break;
    case "disconnecting":
      out.push(head([marks.wait, "yel"], "Disconnessione in corso"));
      break;
    case "failed":
      out.push(head([marks.fail, "red"], "Connessione interrotta"));
      if (st.error) out.push(sub(st.error, "faint"));
      out.push(sub("/connect per riprovare"));
      break;
    case "reconnecting":
      out.push(head([marks.wait, "yel"], `Riconnessione a ${name}`));
      if (st.error) out.push(sub(st.error, "faint"));
      if (st.retry_at) out.push(sub(`nuovo tentativo tra ${retryIn(st.retry_at, now)} s`));
      out.push(sub("/disconnect per smettere di riprovare"));
      break;
    default:
      out.push(head([marks.off, "dim"], "Non connesso"));
  }
  // protection state, whatever the connection state
  if (st.blocked) out.push(sub("il kill switch sta bloccando il traffico fuori dal tunnel", "yel"));
  if (st.protection_error) out.push(head([marks.fail, "red"], "Protezione non attiva"), sub(st.protection_error, "faint"));
  return out;
}

/** Lines describing the kill switch, the local network and split tunneling. */
function describeSettings(st: Settings): Line[] {
  const out: Line[] = [
    head([marks.on, st.kill_switch === "off" ? "dim" : "acc"], `Kill switch ${killSwitchLabels[st.kill_switch]}`),
    sub(killSwitchHints[st.kill_switch]),
    sub(`rete locale ${st.allow_lan ? "consentita" : "bloccata"} quando il kill switch blocca`),
    head([marks.on, st.split_mode === "off" ? "dim" : "acc"], "Tunnel per app: ", [splitLabels[st.split_mode], "fg"]),
  ];
  for (const app of st.split_apps) out.push(sub(`${app.name}  ${app.path}`));
  return out;
}

/** Lines after a successful connection. */
export function describeConnected(t: TunnelInfo, st: Status): Line[] {
  const endpoint = st.peers[0]?.endpoint ?? t.endpoints[0];
  return [
    head([marks.on, "acc"], `Connesso a ${t.name}`),
    ...(endpoint ? [sub(`server ${endpoint}`)] : []),
    sub(`instradamento: ${routingLabel(t)}`),
  ];
}

/** Kill switch modes by the word typed by the user. */
const killSwitchValues: Record<string, KillSwitch> = { off: "off", on: "on_connect", always: "always" };
/** Split tunneling modes, as typed by the user. */
const splitValues: SplitTunnelMode[] = ["off", "include", "exclude"];

/** Every command, in the order of the help. */
export const commands: Command[] = [
  {
    name: "connect",
    usage: "connect <tunnel>",
    description: "connettiti a un tunnel",
    busy: "Connessione in corso",
    async complete(s, args) {
      return (await tunnels(s)).map((t) => t.name).filter((n) => n.toLowerCase().startsWith(joinArgs(args).toLowerCase()));
    },
    async run(s, args) {
      const [list, current] = await Promise.all([tunnels(s), status(s)]);
      // connection to the chosen tunnel; the interactive prompt runs it step by step itself
      const connect = async (t: TunnelInfo): Promise<Outcome> => {
        if (current.state === "connected" && current.tunnel_id === t.id) {
          return { lines: [head([marks.on, "acc"], `Già connesso a ${t.name}`)], json: current };
        }
        if (s.interactive) return { lines: [], connect: t };
        await s.client.request({ method: "connect", params: { id: t.id } }, "ok");
        const st = await status(s);
        return { lines: st.state === "connected" ? describeConnected(t, st) : describeStatus(st, list), json: st };
      };
      // the tunnel named in the arguments, the only one, or a choice
      if (list.length === 0) throw new UsageError("Nessun tunnel da connettere", "importane uno con /import <file.conf>");
      if (args.length > 0) return connect(findTunnel(list, joinArgs(args)));
      if (list.length === 1) return connect(list[0]);
      return needChoice(s, this.usage, {
        title: "Quale tunnel vuoi connettere?",
        options: list.map((t) => tunnelOption(t, current.tunnel_id)),
        choose: (id) => connect(list.find((t) => t.id === id)!),
      });
    },
  },
  {
    name: "disconnect",
    usage: "disconnect",
    description: "chiudi la connessione",
    busy: "Disconnessione in corso",
    async run(s) {
      const [before, list] = await Promise.all([status(s), tunnels(s)]);
      if (before.state === "disconnected") return { lines: [[[marks.off, "dim"], ["Non sei connesso", "fg"]]], json: before };
      await s.client.request({ method: "disconnect" }, "ok");
      // the traffic totals come from the status before the disconnect, which resets them
      const st = await status(s);
      const name = list.find((t) => t.id === before.tunnel_id)?.name ?? "tunnel";
      const out: Line[] = [head([marks.off, "dim"], `Disconnesso da ${name}`)];
      if (before.state === "connected") {
        const { rx, tx } = sumBytes(before);
        out.push(sub(`in questa sessione: ↓ ${formatBytes(rx)}  ↑ ${formatBytes(tx)}`));
      }
      // with the `always` kill switch the traffic stays blocked after the disconnect
      if (st.blocked) out.push(sub("il kill switch è sempre attivo: internet resta bloccato finché non ti connetti", "yel"));
      return { lines: out, json: st };
    },
  },
  {
    name: "status",
    usage: "status",
    description: "stato di connessione e protezione",
    async run(s) {
      const [st, list, cfg] = await Promise.all([status(s), tunnels(s), settings(s)]);
      return { lines: [...describeStatus(st, list), ...describeSettings(cfg)], json: { status: st, settings: cfg } };
    },
  },
  {
    name: "tunnels",
    aliases: ["list", "ls"],
    usage: "tunnels",
    description: "elenca i tunnel importati",
    async run(s) {
      const [list, st] = await Promise.all([tunnels(s), status(s)]);
      if (list.length === 0) {
        return { lines: [head([marks.off, "dim"], "Nessun tunnel"), sub("importane uno con /import <file.conf>")], json: [] };
      }
      // one line per tunnel, then its endpoint, addresses and routing in columns
      const out: Line[] = [];
      for (const t of list) {
        const live = t.id === st.tunnel_id && st.state === "connected";
        out.push(head([live ? marks.on : marks.off, live ? "acc" : "dim"], t.name, [live ? "  connesso" : "", "acc"]));
        out.push([
          [marks.detail, "faint"],
          [(t.endpoints[0] ?? "senza server").padEnd(22) + " ", "fg"],
          [t.addresses.join(", ").padEnd(16) + " ", "blue"],
          [routingLabel(t), "dim"],
        ]);
      }
      return { lines: out, json: list };
    },
  },
  {
    name: "killswitch",
    aliases: ["ks"],
    usage: "killswitch <on|off|always>",
    description: "attiva o spegni il kill switch (on | off | always)",
    async complete(_s, args) {
      return Object.keys(killSwitchValues).filter((v) => v.startsWith(args[0] ?? ""));
    },
    async run(s, args) {
      const apply = async (mode: KillSwitch): Promise<Outcome> => {
        const cfg = await saveSettings(s, { kill_switch: mode });
        const line = head([marks.on, mode === "off" ? "dim" : "acc"], `Kill switch ${killSwitchLabels[mode]}`);
        return { lines: [line, sub(killSwitchHints[mode])], json: cfg };
      };
      // the mode in the arguments, otherwise a choice with the current one marked
      if (args[0]) {
        const mode = killSwitchValues[args[0].toLowerCase()];
        if (!mode) throw new UsageError("Valori possibili: on, off, always");
        return apply(mode);
      }
      const current = (await settings(s)).kill_switch;
      return needChoice(s, this.usage, {
        title: "Kill switch",
        options: (["off", "on_connect", "always"] as KillSwitch[]).map((m) => ({
          label: killSwitchLabels[m],
          hint: m === current ? "attuale" : killSwitchHints[m],
          value: m,
        })),
        choose: (m) => apply(m as KillSwitch),
      });
    },
  },
  {
    name: "log",
    aliases: ["logs"],
    usage: "log",
    description: "ultime righe del servizio",
    async run(s) {
      const { data } = await s.client.request({ method: "get_logs" }, "logs");
      if (data.length === 0) return { lines: [[[marks.off, "dim"], ["Nessuna riga nel registro", "fg"]]], json: [] };
      // only the last lines, colored by level
      const out = data.slice(-8).map((r): Line => {
        const level = r.level === "error" ? "red" : r.level === "warn" ? "yel" : "blue";
        return [[`${formatClock(r.time)}  `, "dim"], [r.level.padEnd(7), level], [r.message, r.level === "error" ? "red" : "fg"]];
      });
      return { lines: out, json: data };
    },
  },
  {
    name: "import",
    usage: "import <file.conf> [nome]",
    description: "importa un file di configurazione WireGuard",
    async complete(_s, args) {
      return completePath(args[0] ?? "");
    },
    async run(s, args) {
      const [file, ...nameParts] = args;
      if (!file) throw new UsageError("Indica il file", "/import <file.conf> [nome]");
      // the CLI reads the file and sends its content, so the service never needs access
      // to the user's files
      let config: string;
      try {
        config = await readFile(file, "utf8");
      } catch {
        throw new UsageError(`Non riesco a leggere ${file}`);
      }
      // the name defaults to the file name without `.conf`
      const name = joinArgs(nameParts) || path.basename(file).replace(/\.conf$/i, "");
      const { data } = await s.client.request({ method: "import_tunnel", params: { name, config } }, "imported");
      // the warnings list the configuration keys the service ignored
      const out = done(`Tunnel “${data.tunnel.name}” importato`);
      for (const w of data.warnings) out.push(sub(`ignorato: ${w}`, "yel"));
      out.push(sub(`connettiti con /connect ${data.tunnel.name}`));
      return { lines: out, json: data };
    },
  },
  {
    name: "delete",
    aliases: ["rm"],
    usage: "delete <tunnel>",
    description: "elimina un tunnel",
    async complete(s, args) {
      return (await tunnels(s)).map((t) => t.name).filter((n) => n.toLowerCase().startsWith(joinArgs(args).toLowerCase()));
    },
    async run(s, args) {
      const list = await tunnels(s);
      const remove = async (t: TunnelInfo): Promise<Outcome> => {
        await s.client.request({ method: "delete_tunnel", params: { id: t.id } }, "ok");
        return lines(...done(`Tunnel “${t.name}” eliminato`));
      };
      // interactive confirmation; script mode needs `--yes` instead
      const confirm = (t: TunnelInfo): Outcome => {
        if (!s.interactive) throw new UsageError(`Per eliminare “${t.name}” aggiungi --yes`);
        return {
          lines: [],
          choice: {
            title: `Eliminare “${t.name}”? Il file di configurazione verrà rimosso.`,
            options: [
              { label: "Elimina", value: "yes" },
              { label: "Annulla", value: "no" },
            ],
            choose: async (v) => (v === "yes" ? remove(t) : lines([[marks.wait, "dim"], ["Eliminazione annullata", "dim"]])),
          },
        };
      };
      // a tunnel to choose, then the confirmation
      if (args.length === 0) {
        const st = await status(s);
        return needChoice(s, this.usage, {
          title: "Quale tunnel vuoi eliminare?",
          options: list.map((t) => tunnelOption(t, st.tunnel_id)),
          choose: async (id) => confirm(list.find((t) => t.id === id)!),
        });
      }
      const t = findTunnel(list, joinArgs(args));
      return s.yes ? remove(t) : confirm(t);
    },
  },
  {
    name: "lan",
    usage: "lan <on|off>",
    description: "rete locale quando il kill switch blocca",
    async complete(_s, args) {
      return ["on", "off"].filter((v) => v.startsWith(args[0] ?? ""));
    },
    async run(s, args) {
      const value = args[0]?.toLowerCase();
      if (value && value !== "on" && value !== "off") throw new UsageError("Valori possibili: on, off");
      // without an argument the setting is toggled
      const allow = value === "on" ? true : value === "off" ? false : !(await settings(s)).allow_lan;
      const cfg = await saveSettings(s, { allow_lan: allow });
      return { lines: done(`Rete locale ${allow ? "consentita" : "bloccata"} quando il kill switch blocca`), json: cfg };
    },
  },
  {
    name: "split",
    usage: "split <off|include|exclude>",
    description: "scegli quali app usano il tunnel",
    async complete(_s, args) {
      return splitValues.filter((v) => v.startsWith(args[0] ?? ""));
    },
    async run(s, args) {
      const apply = async (mode: SplitTunnelMode): Promise<Outcome> => {
        const cfg = await saveSettings(s, { split_mode: mode });
        const out = done(`Tunnel per app: ${splitLabels[mode]}`);
        // a mode with an empty app list does nothing useful: hint to add some apps
        if (mode !== "off" && cfg.split_apps.length === 0) {
          out.push(sub("nessuna app scelta: aggiungine con /apps add <percorso>", "yel"));
        }
        return { lines: out, json: cfg };
      };
      // the mode in the arguments, otherwise a choice with the current one marked
      if (args[0]) {
        const mode = args[0].toLowerCase() as SplitTunnelMode;
        if (!splitValues.includes(mode)) throw new UsageError("Valori possibili: off, include, exclude");
        return apply(mode);
      }
      const current = (await settings(s)).split_mode;
      return needChoice(s, this.usage, {
        title: "Tunnel per app",
        options: splitValues.map((m) => ({ label: splitLabels[m], hint: m === current ? "attuale" : m, value: m })),
        choose: (m) => apply(m as SplitTunnelMode),
      });
    },
  },
  {
    name: "apps",
    usage: "apps [add <percorso> | remove <nome>]",
    description: "app scelte per il tunnel per app",
    async complete(s, args) {
      // the action first, then a path to add or an app name to remove
      if (args.length <= 1) return ["add", "remove"].filter((v) => v.startsWith(args[0] ?? ""));
      if (args[0] === "add") return (await completePath(args.slice(1).join(" "))).map((p) => `add ${p}`);
      const q = args.slice(1).join(" ").toLowerCase();
      return (await settings(s)).split_apps
        .map((a) => a.name)
        .filter((n) => n.toLowerCase().startsWith(q))
        .map((n) => `remove ${n}`);
    },
    async run(s, args) {
      const [action, ...rest] = args;
      const cfg = await settings(s);
      // without an action: the list of the apps
      if (!action) {
        if (cfg.split_apps.length === 0) {
          return { lines: [head([marks.off, "dim"], "Nessuna app scelta"), sub("aggiungine con /apps add <percorso>")], json: [] };
        }
        return { lines: cfg.split_apps.flatMap((a) => [head([marks.on, "dim"], a.name), sub(a.path)]), json: cfg.split_apps };
      }
      const target = joinArgs(rest);
      // addition of an existing file, by absolute path, compared case-insensitively
      if (action === "add") {
        if (!target) throw new UsageError("Indica l'eseguibile", "/apps add <percorso>");
        const full = path.resolve(target);
        if (!(await stat(full).catch(() => null))?.isFile()) throw new UsageError(`${full} non esiste`);
        if (cfg.split_apps.some((a) => a.path.toLowerCase() === full.toLowerCase())) {
          return lines([[marks.on, "dim"], [`${full} è già nell'elenco`, "fg"]]);
        }
        // the app name is the file name without `.exe`
        const name = path.basename(full).replace(/\.exe$/i, "");
        const saved = await saveSettings(s, { split_apps: [...cfg.split_apps, { name, path: full }] });
        const out = done(`${name} aggiunta`);
        if (saved.split_mode === "off") out.push(sub("il tunnel per app è spento: attivalo con /split", "yel"));
        return { lines: out, json: saved };
      }
      // removal by name or by path
      if (action === "remove") {
        const q = target.toLowerCase();
        const app = cfg.split_apps.find((a) => a.name.toLowerCase() === q || a.path.toLowerCase() === q);
        if (!app) throw new UsageError(`“${target}” non è nell'elenco`, "/apps per vederlo");
        const saved = await saveSettings(s, { split_apps: cfg.split_apps.filter((a) => a !== app) });
        return { lines: done(`${app.name} rimossa`), json: saved };
      }
      throw new UsageError("Uso: /apps, /apps add <percorso>, /apps remove <nome>");
    },
  },
  {
    name: "clear",
    usage: "clear",
    description: "pulisci lo schermo",
    interactiveOnly: true,
    offline: true,
    async run() {
      return { lines: [], clear: true };
    },
  },
  {
    name: "help",
    aliases: ["?"],
    usage: "help",
    description: "tutti i comandi",
    offline: true,
    async run(s) {
      // `/name` in the prompt, the full synopsis in script mode, in aligned columns
      const shown = commands.filter((c) => s.interactive || !c.interactiveOnly);
      const label = (c: Command) => (s.interactive ? `/${c.name}` : c.usage);
      const width = Math.max(14, ...shown.map((c) => label(c).length + 2));
      const out: Line[] = [[["Comandi", "bold"]]];
      for (const c of shown) out.push([[`  ${label(c).padEnd(width)}`, "acc"], [c.description, "fg"]]);
      if (s.interactive) out.push([[" ", "fg"]], [["Tab completa · ↑↓ scorre · Esc chiude il menu", "dim"]]);
      return { lines: out };
    },
  },
  {
    name: "exit",
    aliases: ["quit", "q"],
    usage: "exit",
    description: "esci (la VPN resta attiva)",
    interactiveOnly: true,
    offline: true,
    async run() {
      return { lines: [[["Ciao. ", "bold"], ["La VPN resta gestita dal servizio.", "dim"]]], exit: true };
    },
  },
];

/** The command with this name or alias, case-insensitive. */
export function findCommand(name: string): Command | undefined {
  const n = name.toLowerCase();
  return commands.find((c) => c.name === n || c.aliases?.includes(n));
}

/**
 * Best guess for a mistyped name, for the "did you mean" hint: the first command that
 * shares its first three letters ("/conn" → connect). Names shorter than 2 letters get none.
 */
export function guessCommand(name: string): Command | undefined {
  const n = name.toLowerCase().slice(0, 3);
  return n.length < 2 ? undefined : commands.find((c) => c.name.startsWith(n));
}

/** Splits a command line, honouring double quotes: `import "a b.conf" Ufficio`. */
export function parseLine(input: string): string[] {
  const out: string[] = [];
  const re = /"([^"]*)"|(\S+)/g;
  for (const m of input.matchAll(re)) out.push(m[1] ?? m[2]);
  return out;
}

/**
 * File name candidates for `partial`, at most 20, hidden files excluded. Directories end
 * with the path separator; names with spaces get an opening quote, and files also the
 * closing one, so a directory can still be completed further.
 */
async function completePath(partial: string): Promise<string[]> {
  // split of the partial path into the directory to list and the name prefix
  const unquoted = partial.replace(/^"/, "");
  const endsWithSep = /[\\/]$/.test(unquoted);
  const hasDir = /[\\/]/.test(unquoted);
  const dir = endsWithSep ? unquoted : hasDir ? path.dirname(unquoted) : ".";
  const base = endsWithSep ? "" : path.basename(unquoted);
  // an unreadable directory simply has no candidates
  try {
    const entries = await readdir(dir, { withFileTypes: true });
    return entries
      .filter((e) => e.name.toLowerCase().startsWith(base.toLowerCase()) && !e.name.startsWith("."))
      .slice(0, 20)
      .map((e) => {
        const full = (hasDir ? path.join(dir, e.name) : e.name) + (e.isDirectory() ? path.sep : "");
        return full.includes(" ") ? `"${full}` + (e.isDirectory() ? "" : '"') : full;
      });
  } catch {
    return [];
  }
}

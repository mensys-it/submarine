#!/usr/bin/env bun
// Entry point of the `submarine` CLI: without arguments it opens the interactive
// prompt (an Ink app, see ui/App.tsx); with a command it runs it once and exits,
// for scripts. `--json` prints the raw service data instead of the text lines.

import { render } from "ink";

import { findCommand, UsageError } from "./commands.ts";
import { DaemonClient, ServiceError } from "./ipc.ts";
import { head, type Line, marks, sub } from "./lines.ts";
import { App } from "./ui/App.tsx";
import { ansi } from "./ui/theme.ts";

/** Version printed by `--version` and shown in the prompt header. */
const VERSION = "0.1.0";

/** Help of script mode, in Italian like the rest of the CLI. */
const HELP = `Submarine VPN, riga di comando

Uso:
  submarine                         modalità interattiva
  submarine <comando> [argomenti]   esegue un comando ed esce

Comandi:
  status                            stato della connessione e della protezione
  tunnels                           tunnel importati
  connect <tunnel>                  connetti (per nome, anche parziale)
  disconnect                        disconnetti
  import <file.conf> [nome]         importa una configurazione WireGuard
  delete <tunnel> --yes             elimina un tunnel
  killswitch <off|on|always>        kill switch
  lan <on|off>                      rete locale con il kill switch attivo
  split <off|include|exclude>       tunnel per app
  apps [add <percorso> | remove <nome>]
  log                               ultime righe del servizio

Opzioni:
  --json        output JSON, per gli script
  --yes, -y     conferma le operazioni distruttive
  --version     versione
`;

/** Prints lines to stdout, with ANSI colors only if `color` is set. */
function print(lines: Line[], color: boolean) {
  for (const line of lines) console.log(line.map(([text, style]) => ansi(style, text, color)).join(""));
}

/** Lines describing an error, with the usage detail below when there is one. */
function errorLines(err: unknown): Line[] {
  const message = err instanceof Error ? err.message : String(err);
  const out = [head([marks.fail, "red"], message)];
  if (err instanceof UsageError && err.detail) out.push(sub(err.detail));
  return out;
}

/**
 * Runs one command in script mode and returns the exit code: 0 on success, 2 on a usage
 * error, 3 when the service is unreachable or rejects the request, 1 otherwise.
 */
async function runOnce(argv: string[]): Promise<number> {
  // the flags can go anywhere; what remains is the command and its arguments
  const json = argv.includes("--json");
  const yes = argv.includes("--yes") || argv.includes("-y");
  const [name, ...args] = argv.filter((a) => !["--json", "--yes", "-y"].includes(a));
  const color = process.stdout.isTTY && !process.env.NO_COLOR;

  // commands of the interactive prompt only are unknown here
  const command = findCommand(name);
  if (!command || command.interactiveOnly) {
    console.error(`Comando sconosciuto: ${name}\n\n${HELP}`);
    return 2;
  }
  if (command.name === "help") {
    console.log(HELP);
    return 0;
  }

  // execution against the service; with `--json` errors are JSON too
  let client: DaemonClient | undefined;
  try {
    client = await DaemonClient.connect();
    const outcome = await command.run({ client, interactive: false, yes }, args);
    if (json) console.log(JSON.stringify(outcome.json ?? null, null, 2));
    else print(outcome.lines, color);
    return 0;
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    if (json) console.log(JSON.stringify({ error: message }));
    else print(errorLines(err), color);
    return err instanceof UsageError ? 2 : err instanceof ServiceError ? 3 : 1;
  } finally {
    client?.close();
  }
}

// dispatch: version, help, interactive prompt or a single command
const argv = process.argv.slice(2);
if (argv.includes("--version")) {
  console.log(VERSION);
} else if (argv.includes("--help") || argv.includes("-h")) {
  console.log(HELP);
} else if (argv.length === 0) {
  // without a terminal (e.g. piped input) the prompt cannot work: print the help instead
  if (!process.stdin.isTTY) {
    console.log(HELP);
  } else {
    const { waitUntilExit } = render(<App version={VERSION} />, { exitOnCtrlC: true });
    await waitUntilExit();
  }
} else {
  process.exitCode = await runOnce(argv);
}

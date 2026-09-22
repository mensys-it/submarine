// Tests of the interactive prompt, rendered with ink-testing-library against the
// fake daemon and driven by writing keys to its stdin, and of the header scene.

import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { render } from "ink-testing-library";

import { DaemonClient } from "../src/ipc.ts";
import { App } from "../src/ui/App.tsx";
import { drawScene, drawWave, restingDepth, stepDepth } from "../src/ui/scene.ts";
import { FakeDaemon } from "./fake-daemon.ts";

let daemon: FakeDaemon;
// a fresh fake daemon for every test
beforeEach(async () => {
  daemon = await new FakeDaemon().start();
});
afterEach(async () => {
  await daemon.stop();
});

/** Resolves after `ms` milliseconds, to let the app render. */
const tick = (ms = 60) => new Promise((r) => setTimeout(r, ms));
/**
 * Waits until the frame contains `text`.
 *
 * @throws {Error} After `ms` milliseconds, with the last frame.
 */
async function waitFor(frame: () => string | undefined, text: string, ms = 2000) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    if (frame()?.includes(text)) return;
    await tick(20);
  }
  throw new Error(`"${text}" not found in:\n${frame()}`);
}

/** Types `text` one key at a time, like a person. */
async function type(stdin: { write(s: string): void }, text: string) {
  for (const ch of text) {
    stdin.write(ch);
    await tick(5);
  }
}

/** Down arrow key. */
const DOWN = "\x1B[B";
/** Escape key. */
const ESC = "\x1B";

/** Renders the app connected to the fake daemon. */
function start(props: Partial<Parameters<typeof App>[0]> = {}) {
  const connect = () => DaemonClient.connect(daemon.path);
  return render(<App connect={connect} version="0.1.0" {...props} />);
}

// a full session: menu, connection steps, live status, settings, disconnect, mistakes
test("interactive session: menu, connection step by step, live status", async () => {
  const { lastFrame, stdin, frames, unmount } = start();
  await waitFor(lastFrame, "Non connesso");
  expect(lastFrame()).toContain("Submarine VPN");
  expect(lastFrame()).toContain("in superficie");
  expect(lastFrame()).toContain("kill switch spento");

  await type(stdin, "/con");
  await waitFor(lastFrame, "connettiti a Laboratorio");
  expect(lastFrame()).toContain("❯ /connect ");
  stdin.write(DOWN);
  await tick(20);
  // second suggestion: /connect Laboratorio
  stdin.write(DOWN);
  await tick(20);
  stdin.write("\r");
  await waitFor(lastFrame, "Handshake completato");
  const frame = lastFrame()!;
  expect(frame).toContain("› /connect Laboratorio");
  expect(frame).toContain("✓ Tunnel verso 203.0.113.40 avviato");
  expect(frame).toContain("✓ Rotte e firewall applicati");
  expect(frame).toContain("● Connesso a Laboratorio");
  expect(frame).toContain("⎿ server 203.0.113.40:51820");
  expect(frame).toContain("⎿ instradamento: solo reti del tunnel");
  await waitFor(lastFrame, "↓ 1,2 MB/s");
  expect(lastFrame()).toContain("in navigazione nel tunnel");
  // the status line shows the session time next to the rates
  expect(lastFrame()).toMatch(/↑ [\d,]+ KB\/s .* \d+ s/);
  // the steps went through the service's events, in order
  const steps = frames.filter((f) => f.includes("Handshake con il server…"));
  expect(steps.length).toBeGreaterThan(0);

  await type(stdin, "/ks on");
  stdin.write("\r");
  await waitFor(lastFrame, "kill switch attivo");
  expect(daemon.settings.kill_switch).toBe("on_connect");

  await type(stdin, "/disconnect");
  stdin.write("\r");
  await waitFor(lastFrame, "Disconnesso da Laboratorio");
  expect(lastFrame()).toContain("in questa sessione: ↓ 1,2 MB");

  await type(stdin, "/dcaf");
  stdin.write("\r");
  await waitFor(lastFrame, "Comando sconosciuto /dcaf");
  expect(lastFrame()).toContain("scrivi /help per l’elenco dei comandi");

  await type(stdin, "/conn");
  // Esc closes the menu: Enter runs what was typed
  stdin.write(ESC);
  await tick(20);
  stdin.write("\r");
  await waitFor(lastFrame, "forse intendevi /connect?");

  await type(stdin, "status");
  stdin.write("\r");
  await waitFor(lastFrame, "I comandi iniziano con /. Prova /status");

  if (process.env.SHOW_FRAMES) console.log(frames.join("\n────────\n"));
  unmount();
});

// `/connect` without a name asks which tunnel, and connects to the chosen one
test("connect without a name offers a choice", async () => {
  const { lastFrame, stdin, unmount } = start();
  await waitFor(lastFrame, "Non connesso");
  await type(stdin, "/connect");
  stdin.write("\r");
  await waitFor(lastFrame, "Quale tunnel vuoi connettere?");
  stdin.write(DOWN);
  await tick();
  stdin.write("\r");
  await waitFor(lastFrame, "● Connesso a Laboratorio");
  unmount();
});

// a name resolution failure marks the step as failed and suggests a retry
test("a failed connection says why and how to retry", async () => {
  daemon.failNext = "failed to resolve endpoint vpn.example.it:51820: Host sconosciuto. (os error 11001)";
  const { lastFrame, stdin, unmount } = start();
  await waitFor(lastFrame, "Non connesso");
  await type(stdin, "/connect uff");
  stdin.write("\r");
  await waitFor(lastFrame, "Non trovo il server");
  const frame = lastFrame()!;
  expect(frame).toContain("✗ Risolvo vpn.example.it non riuscito");
  expect(frame).toContain("vpn.example.it non risponde: controlla internet o il DNS del server");
  expect(frame).toContain("riprova con /connect Ufficio Milano");
  expect(frame).toContain("bloccato in superficie");
  unmount();
});

// a disconnect while waiting for the handshake cancels the run
test("disconnecting during a connection cancels it", async () => {
  daemon.silentServer = true;
  const { lastFrame, stdin, unmount } = start();
  await waitFor(lastFrame, "Non connesso");
  await type(stdin, "/connect lab");
  stdin.write("\r");
  await waitFor(lastFrame, "Handshake con il server…");
  await type(stdin, "/disconnect");
  stdin.write("\r");
  await waitFor(lastFrame, "Disconnesso da Laboratorio");
  expect(lastFrame()).toContain("◌ Annullato");
  unmount();
});

// the help, the service log and the clearing of the transcript
test("help, log and clear", async () => {
  const { lastFrame, stdin, unmount } = start();
  await waitFor(lastFrame, "Non connesso");
  await type(stdin, "/help");
  stdin.write("\r");
  await waitFor(lastFrame, "Tab completa · ↑↓ scorre · Esc chiude il menu");
  expect(lastFrame()).toContain("/killswitch");

  await type(stdin, "/log");
  stdin.write("\r");
  await waitFor(lastFrame, "submarine daemon ready socket=submarine");

  await type(stdin, "/clear");
  stdin.write("\r");
  await tick(100);
  expect(lastFrame()).not.toContain("submarine daemon ready");
  expect(lastFrame()).toContain("Submarine VPN");
  unmount();
});

// Tab completes a command name, the up arrow recalls the previous command
test("history and Tab completion", async () => {
  const { lastFrame, stdin, unmount } = start();
  await waitFor(lastFrame, "Non connesso");
  await type(stdin, "/tun");
  stdin.write("\t");
  await tick();
  expect(lastFrame()).toContain("› /tunnels");
  stdin.write("\r");
  await waitFor(lastFrame, "vpn.example.it:51820");
  // up arrow: previous command
  stdin.write("\x1B[A");
  await tick();
  expect(lastFrame()).toMatch(/│ › \/tunnels/);
  unmount();
});

// a pasted line with its Enter runs as typed, ignoring the menu
test("a line sent in one chunk with its Enter runs as typed", async () => {
  const { lastFrame, stdin, unmount } = start();
  await waitFor(lastFrame, "Non connesso");
  await type(stdin, "/");
  await waitFor(lastFrame, "❯ /connect");
  // the menu suggests /connect: not taken
  stdin.write("disconnect\r");
  await waitFor(lastFrame, "Non sei connesso");
  expect(lastFrame()).toMatch(/│ › +Scrivi un comando/);
  await type(stdin, "/st");
  expect(lastFrame()).toMatch(/│ › \/st +│/);
  unmount();
});

// an unreachable service is shown, and retried
test("service down: shows it and reconnects", async () => {
  await daemon.stop();
  const { lastFrame, unmount } = start({ retryMs: 50 });
  await waitFor(lastFrame, "non raggiungibile");
  daemon = await new FakeDaemon().start();
  unmount();
});

// keys faster than the renders, and a held backspace, lose no edit
test("fast typing keeps every character", async () => {
  const { lastFrame, stdin, unmount } = start();
  await waitFor(lastFrame, "Non connesso");
  // no pause between keys
  for (const ch of "/tunnels") stdin.write(ch);
  await tick();
  expect(lastFrame()).toContain("› /tunnels");
  stdin.write("xyz");
  await tick();
  // held backspace: one chunk, three keys
  stdin.write("\x7f\x7f\x7f");
  await tick();
  expect(lastFrame()).toMatch(/› \/tunnels +│/);
  stdin.write("\r");
  await waitFor(lastFrame, "Laboratorio");
  unmount();
});

// the default `connect` connects once, without a render loop
test("default connection does not loop (stable effect dependencies)", async () => {
  const previous = process.env.SUBMARINE_SOCKET;
  process.env.SUBMARINE_SOCKET = daemon.path;
  const errors: string[] = [];
  const original = console.error;
  console.error = (...args: unknown[]) => errors.push(args.join(" "));
  try {
    // no `connect` prop
    const { lastFrame, unmount } = render(<App version="0.1.0" />);
    await waitFor(lastFrame, "Non connesso");
    await tick(300);
    const connects = daemon.requests.filter((r) => r.method === "get_status").length;
    expect(connects).toBe(1);
    expect(errors.join("\n")).not.toContain("Maximum update depth");
    unmount();
  } finally {
    console.error = original;
    if (previous === undefined) delete process.env.SUBMARINE_SOCKET;
    else process.env.SUBMARINE_SOCKET = previous;
  }
});

describe("scene", () => {
  /** Text of each row, without colors. */
  const text = (rows: { text: string }[][]) => rows.map((r) => r.map((s) => s.text).join(""));

  // the grid has the requested size and merges cells with the same colors
  test("a grid of the requested size, colour runs merged", () => {
    const depth = restingDepth("off", 8);
    const grid = drawScene({ t: 5, width: 118, height: 8, depth, mood: "off", accent: "#3DD6C4" });
    expect(grid).toHaveLength(8);
    for (const row of text(grid)) expect([...row]).toHaveLength(118);
    // water rows are one background, with a few "~" in another colour
    expect(grid[7].length).toBeLessThan(20);
    // portholes on the water line
    expect(text(grid)[6]).toContain("●");
  });

  // from off to on the water and the submarine move one row per step
  test("the submarine dives when connected, a row every step", () => {
    let d = restingDepth("off", 8);
    expect(d).toEqual({ level: 6, subRow: 5 });
    for (let i = 0; i < 10; i++) d = stepDepth(d, "on", 8);
    expect(d).toEqual(restingDepth("on", 8));
    expect(d).toEqual({ level: 1, subRow: 4 });
  });

  // a single line of crests, as wide as requested
  test("wave line for narrow terminals", () => {
    const wave = drawWave(3, 40);
    expect(wave.map((r) => r.text).join("")).toMatch(/^[▁▂▃▄▅▆]{40}$/);
  });
});

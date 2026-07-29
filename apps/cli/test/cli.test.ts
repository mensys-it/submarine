// Tests of the IPC client, the commands in script mode and the formatting, against
// the in-memory fake daemon (see fake-daemon.ts) on a temporary Unix socket.

import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";

import { findCommand, findTunnel, parseLine, type Session, UsageError } from "../src/commands.ts";
import { formatAgo, formatBytes } from "../src/format.ts";
import { plain } from "../src/lines.ts";
import { DaemonClient, ServiceError, socketPath } from "../src/ipc.ts";
import { FakeDaemon } from "./fake-daemon.ts";

let daemon: FakeDaemon;
let client: DaemonClient;
/** Script mode session, without `--yes`. */
const script = (): Session => ({ client, interactive: false, yes: false });
/** Runs command `name`, in script mode unless another session is given. */
const run = (name: string, args: string[] = [], session = script()) => findCommand(name)!.run(session, args);

// a fresh fake daemon and connection for every test
beforeEach(async () => {
  daemon = await new FakeDaemon().start();
  client = await DaemonClient.connect(daemon.path);
});
afterEach(async () => {
  client.close();
  await daemon.stop();
});

describe("ipc", () => {
  // platform defaults and the `SUBMARINE_SOCKET` override, also as a bare pipe name
  test("default socket paths", () => {
    expect(socketPath({}, "linux")).toBe("/run/submarine/daemon.sock");
    expect(socketPath({}, "darwin")).toBe("/var/run/submarine/daemon.sock");
    expect(socketPath({}, "win32")).toBe("\\\\.\\pipe\\submarine");
    expect(socketPath({ SUBMARINE_SOCKET: "test" }, "win32")).toBe("\\\\.\\pipe\\test");
    expect(socketPath({ SUBMARINE_SOCKET: "/tmp/x.sock" }, "linux")).toBe("/tmp/x.sock");
  });

  // a response, an error from the service and an event on the same connection
  test("requests, errors and events", async () => {
    const { data } = await client.request({ method: "list_tunnels" }, "tunnels");
    expect(data).toHaveLength(2);
    await expect(client.request({ method: "connect", params: { id: "nope" } }, "ok")).rejects.toThrow("unknown tunnel");
    const event = new Promise((resolve) => client.once("event", resolve));
    await client.request({ method: "disconnect" }, "ok");
    expect(await event).toMatchObject({ type: "status_changed" });
  });

  // a missing socket is a ServiceError
  test("unreachable service", async () => {
    await expect(DaemonClient.connect(path.join(os.tmpdir(), "missing.sock"))).rejects.toBeInstanceOf(ServiceError);
  });
});

describe("commands", () => {
  // a quoted argument with spaces stays one argument
  test("parseLine honours quotes", () => {
    expect(parseLine('import "My Files/a b.conf" Ufficio')).toEqual(["import", "My Files/a b.conf", "Ufficio"]);
  });

  // exact name, name prefix, id prefix, and no match
  test("findTunnel by name, prefix and id", () => {
    const list = daemon.tunnels;
    expect(findTunnel(list, "laboratorio").id).toBe("d4e5f6");
    expect(findTunnel(list, "uff").id).toBe("a1b2c3");
    expect(findTunnel(list, "d4e").id).toBe("d4e5f6");
    expect(() => findTunnel(list, "zzz")).toThrow(UsageError);
  });

  // connection by name prefix, traffic in the status after the handshake, no second connect
  test("connect by partial name, then status", async () => {
    const outcome = await run("connect", ["ufficio"]);
    expect(daemon.requests).toContainEqual({ method: "connect", params: { id: "a1b2c3" } });
    expect(plain(outcome.lines[0])).toBe("● Connesso a Ufficio Milano");
    expect(outcome.lines.map(plain)).toContain("  ⎿ server 203.0.113.10:51820");
    // wait for the first handshake
    await Bun.sleep(60);
    const status = await run("status");
    expect(status.lines.map(plain).join("\n")).toContain("ricevuti 1,2 MB");
    const again = await run("connect", ["ufficio"]);
    expect(plain(again.lines[0])).toBe("● Già connesso a Ufficio Milano");
  });

  // the traffic of the session on disconnect, then a disconnect with nothing to close
  test("disconnect reports the session, or that there was nothing to close", async () => {
    await run("connect", ["lab"]);
    await Bun.sleep(60);
    const out = await run("disconnect");
    expect(out.lines.map(plain)).toEqual(["○ Disconnesso da Laboratorio", "  ⎿ in questa sessione: ↓ 1,2 MB  ↑ 56,0 KB"]);
    const none = await run("disconnect");
    expect(plain(none.lines[0])).toBe("○ Non sei connesso");
  });

  // in interactive mode the command only hands the tunnel to the prompt
  test("interactive connect is left to the prompt, which follows the steps", async () => {
    const outcome = await run("connect", ["lab"], { client, interactive: true, yes: false });
    expect(outcome.connect?.id).toBe("d4e5f6");
    expect(daemon.requests.some((r) => r.method === "connect")).toBe(false);
  });

  // the latest log line, formatted and colored by level
  test("log shows the service's latest lines", async () => {
    daemon.log("error", "connect failed: boom");
    const out = await run("log");
    const text = out.lines.map(plain);
    expect(text.at(-1)).toMatch(/^\d\d:\d\d:\d\d  error  connect failed: boom$/);
    expect(out.lines.at(-1)![1][1]).toBe("red");
  });

  // an unknown tunnel name lists the available ones in the detail
  test("unknown tunnel lists the available ones", async () => {
    const err = await run("connect", ["zzz"]).catch((e) => e);
    expect(err).toBeInstanceOf(UsageError);
    expect(err.message).toBe("Nessun tunnel “zzz”");
    expect(err.detail).toBe("disponibili: Ufficio Milano, Laboratorio");
  });

  // a missing tunnel name is an error in scripts and a choice in the prompt
  test("connect without a name asks to choose, only when interactive", async () => {
    await expect(run("connect")).rejects.toThrow(UsageError);
    const outcome = await run("connect", [], { client, interactive: true, yes: false });
    expect(outcome.choice?.options.map((o) => o.label)).toEqual(["Ufficio Milano", "Laboratorio"]);
    const chosen = await outcome.choice!.choose("d4e5f6");
    expect(chosen.connect?.name).toBe("Laboratorio");
  });

  // a setting command keeps the other settings, and rejects unknown values
  test("settings are changed one field at a time", async () => {
    daemon.settings = { ...daemon.settings, split_mode: "exclude", split_apps: [{ name: "firefox", path: "/usr/bin/firefox" }] };
    await run("killswitch", ["on"]);
    expect(daemon.settings).toEqual({ kill_switch: "on_connect", allow_lan: false, split_mode: "exclude", split_apps: [{ name: "firefox", path: "/usr/bin/firefox" }] });
    await run("lan", ["on"]);
    expect(daemon.settings.allow_lan).toBe(true);
    await expect(run("killswitch", ["maybe"])).rejects.toThrow("on, off, always");
  });

  // an app is added with the split tunneling hint, removed, and a missing file rejected
  test("apps add and remove", async () => {
    const exe = path.join(os.tmpdir(), "fake-app.exe");
    writeFileSync(exe, "");
    const added = await run("apps", ["add", exe]);
    expect(plain(added.lines[0])).toBe("✓ fake-app aggiunta");
    expect(plain(added.lines[1])).toContain("/split");
    expect(daemon.settings.split_apps).toEqual([{ name: "fake-app", path: exe }]);
    await run("apps", ["remove", "fake-app"]);
    expect(daemon.settings.split_apps).toEqual([]);
    await expect(run("apps", ["add", "/does/not/exist"])).rejects.toThrow("non esiste");
  });

  // deletion in script mode only with `--yes`
  test("delete needs --yes in scripts", async () => {
    await expect(run("delete", ["lab"])).rejects.toThrow("--yes");
    await run("delete", ["lab"], { client, interactive: false, yes: true });
    expect(daemon.tunnels.map((t) => t.name)).toEqual(["Ufficio Milano"]);
  });

  // the warnings of an import are shown as ignored lines
  test("import reports ignored lines", async () => {
    const file = path.join(os.tmpdir(), "casa.conf");
    writeFileSync(file, "[Interface]\nPostUp = x\n");
    const outcome = await run("import", [file]);
    expect(plain(outcome.lines[0])).toBe("✓ Tunnel “casa” importato");
    expect(plain(outcome.lines[1])).toStartWith("  ⎿ ignorato: line 5");
    expect(outcome.lines[1][1][1]).toBe("yel");
  });
});

// byte counts in the Italian locale and relative times
test("formatting", () => {
  expect(formatBytes(1_234_000)).toBe("1,2 MB");
  expect(formatAgo(100, 112)).toBe("12 s fa");
});

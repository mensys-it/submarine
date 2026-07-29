// Playground: the interactive CLI against the in-memory fake daemon, without
// touching the network or needing the service, with slower steps to watch them.
//
// Usage: `npx bun run demo`
import { spawn } from "node:child_process";

import { FakeDaemon } from "./fake-daemon.ts";

// the fake daemon on a temporary socket, then the CLI pointed at it; the daemon
// stops when the CLI exits
const daemon = await new FakeDaemon({ stepMs: 900, statsMs: 1000 }).start();
const child = spawn(process.execPath, ["run", new URL("../src/index.tsx", import.meta.url).pathname], {
  stdio: "inherit",
  env: { ...process.env, SUBMARINE_SOCKET: daemon.path },
});
child.on("exit", async (code) => {
  await daemon.stop();
  process.exit(code ?? 0);
});

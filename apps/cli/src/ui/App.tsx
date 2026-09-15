// Interactive prompt of the CLI, an Ink app: the transcript of commands and their
// output, the animated header, the input with its suggestion menu, and the status
// line. It keeps a connection to submarine-daemon (retrying while it is down),
// mirrors its live state from the events, and follows connections step by step.

import { Box, Static, Text, useApp, useInput, useStdout, useWindowSize } from "ink";
import { useCallback, useEffect, useRef, useState } from "react";

import { type Choice, type Command, commands, findCommand, guessCommand, type Outcome, parseLine, type Session, UsageError } from "../commands.ts";
import { DaemonClient } from "../ipc.ts";
import { head, type Line, marks, sub } from "../lines.ts";
import type { DaemonEvent, Settings, Status, TunnelInfo } from "../protocol.ts";
import { ChoiceList } from "./ChoiceList.tsx";
import { cancelRun, type ConnectRun, failRun, HANDSHAKE_WAIT_MS, onEvent, runLines, startRun, timeoutRun } from "./connectRun.ts";
import { FrameProvider, useFrame } from "./Frame.tsx";
import { Header, type SceneState } from "./Header.tsx";
import { LinesView } from "./LineView.tsx";
import { Prompt } from "./Prompt.tsx";
import { SCENE_ROWS } from "./scene.ts";
import { type Service, StatusLine } from "./StatusLine.tsx";
import { MENU_ROWS, type Suggestion, Suggestions } from "./Suggestions.tsx";
import { animationsEnabled, c, palette, SPIN } from "./theme.ts";

/** Block of the transcript: a command typed by the user, or the lines it printed. */
type NewEntry = { kind: "cmd"; text: string } | { kind: "lines"; lines: Line[] };
/** Transcript block with the key used by `<Static>`. */
type Entry = { id: number } & NewEntry;

interface Props {
  /** Opens the service connection; tests pass a fake daemon. */
  connect?: () => Promise<DaemonClient>;
  /** Version shown in the header. */
  version: string;
  /** Delay before retrying the service connection. */
  retryMs?: number;
  /** Defaults to on in a colour terminal. */
  animate?: boolean;
}

/** Delay before the spinner shows up, so quick commands do not flicker. */
const BUSY_DELAY_MS = 150;
/** Number of samples in the traffic sparkline of the status line. */
const SPARK_LENGTH = 14;
/**
 * Rows kept for everything but the ocean: header chrome (5), prompt (3),
 * menu (7), a running step (4), status line and one spare. Ink redraws the
 * whole screen when its live area is as tall as the terminal.
 */
const RESERVED_ROWS = 5 + 3 + MENU_ROWS + 4 + 2;

/**
 * State mirrored in a ref that is updated synchronously: keystrokes and
 * service events can arrive faster than React re-renders, and handlers must
 * always see the latest value.
 */
function useSyncState<T>(initial: T) {
  const [state, setState] = useState(initial);
  const ref = useRef(initial);
  const set = useCallback((value: T | ((previous: T) => T)) => {
    ref.current = typeof value === "function" ? (value as (previous: T) => T)(ref.current) : value;
    setState(ref.current);
  }, []);
  return [state, set, ref] as const;
}

/**
 * Default `connect`; module-level, so its identity is stable: the connection effect
 * depends on it.
 */
const connectToService = () => DaemonClient.connect();

/** Lines describing an error, with the usage detail below when there is one. */
const errorLines = (err: unknown): Line[] => {
  const message = err instanceof Error ? err.message : String(err);
  const out: Line[] = [head([marks.fail, "red"], message)];
  if (err instanceof UsageError && err.detail) out.push(sub(err.detail));
  return out;
};

/** The command synopsis has arguments, so completing its name adds a space. */
const takesArgs = (cmd: Command) => /[<[]/.test(cmd.usage);

/** Ready-made full commands offered in the menu along with the command name. */
const extras: Record<string, (tunnels: TunnelInfo[]) => Suggestion[]> = {
  connect: (tunnels) =>
    tunnels.map((t) => ({ label: `/connect ${t.name}`, description: `connettiti a ${t.name}`, value: `/connect ${t.name}`, runnable: true })),
  killswitch: () =>
    [
      ["on", "blocca internet se la VPN cade"],
      ["off", "spegni il kill switch"],
      ["always", "blocca internet anche a VPN spenta"],
    ].map(([v, d]) => ({ label: `/killswitch ${v}`, description: d, value: `/killswitch ${v}`, runnable: true })),
};

/**
 * Menu for a command name being typed: "/" lists the commands, more letters also their
 * common arguments. Aliases match from the second letter on; what is already typed in
 * full is left out.
 */
function commandSuggestions(typed: string, tunnels: TunnelInfo[]): Suggestion[] {
  const q = typed.toLowerCase();
  const out: Suggestion[] = [];
  for (const cmd of commands) {
    const label = `/${cmd.name}`;
    const byAlias = cmd.aliases?.some((a) => `/${a}`.startsWith(q) && q.length > 1);
    if ((label.startsWith(q) || byAlias) && label !== q) {
      out.push({ label, description: cmd.description, value: label + (takesArgs(cmd) ? " " : ""), runnable: cmd.name !== "import" });
    }
    // the bare "/" lists only the commands, to keep the menu short
    if (q === "/") continue;
    for (const s of extras[cmd.name]?.(tunnels) ?? []) {
      if (s.label.toLowerCase().startsWith(q) && s.label.toLowerCase() !== q) out.push(s);
    }
  }
  return out;
}

/** Mood and caption of the header scene, from the service and the connection state. */
function sceneState(service: Service, status: Status | null): SceneState {
  if (service === "down") return { mood: "error", label: "servizio non raggiungibile", labelColor: palette.red };
  switch (status?.state) {
    case "connected":
      return { mood: "on", label: "in navigazione nel tunnel", labelColor: palette.accent };
    case "connecting":
      return { mood: "connecting", label: "immersione…", labelColor: palette.yellow };
    case "disconnecting":
      return { mood: "off", label: "riemersione…", labelColor: palette.yellow };
    case "failed":
      return { mood: "error", label: "bloccato in superficie", labelColor: palette.red };
    case "reconnecting":
      return { mood: "connecting", label: "di nuovo in immersione…", labelColor: palette.yellow };
    default:
      return { mood: "off", label: "in superficie", labelColor: palette.dim };
  }
}

/** Root component of the interactive prompt. */
export function App({ connect = connectToService, version, retryMs = 2000, animate = animationsEnabled }: Props) {
  const { exit } = useApp();
  const { write } = useStdout();
  const { columns, rows } = useWindowSize();

  // transcript: printed once by <Static>; /clear starts a new one and clears the
  // screen and the scrollback
  const nextId = useRef(1);
  const [entries, setEntries] = useState<Entry[]>([]);
  const [transcript, setTranscript] = useState(0);
  const push = useCallback((entry: NewEntry) => {
    setEntries((list) => [...list, { ...entry, id: nextId.current++ }]);
  }, []);
  useEffect(() => {
    if (transcript > 0) write("\x1b[2J\x1b[3J\x1b[H");
  }, [transcript, write]);

  // service connection and live state
  const [client, setClient] = useState<DaemonClient | null>(null);
  const [service, setService] = useState<Service>("connecting");
  const [status, setStatus, statusRef] = useSyncState<Status | null>(null);
  const [tunnels, setTunnels, tunnelsRef] = useSyncState<TunnelInfo[]>([]);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [spark, setSpark] = useState<number[]>([]);
  const lastRx = useRef<{ tunnel: string; rx: number } | null>(null);
  const [now, setNow] = useState(() => Date.now() / 1000);

  // connection in progress, followed step by step
  const [run, setRun, runRef] = useSyncState<ConnectRun | null>(null);
  const nextRun = useRef(1);
  // applies `change` to run `id` only if it is still the current one; a finished run
  // moves to the transcript
  const updateRun = useCallback(
    (id: number, change: (run: ConnectRun) => ConnectRun) => {
      const current = runRef.current;
      if (!current || current.id !== id) return;
      const next = change(current);
      if (next === current) return;
      if (next.result) {
        push({ kind: "lines", lines: runLines(next, 0, false) });
        setRun(null);
      } else setRun(next);
    },
    [push, setRun, runRef],
  );

  // live state from the service events; the traffic sparkline gets the bytes received
  // since the previous status, and restarts with another tunnel or when disconnected
  const onServiceEvent = useCallback(
    (e: DaemonEvent) => {
      if (e.type === "status_changed") {
        const st = e.data;
        setStatus(st);
        if (st.state === "connected" && st.tunnel_id) {
          const rx = st.peers.reduce((n, p) => n + p.rx_bytes, 0);
          const last = lastRx.current;
          if (last?.tunnel === st.tunnel_id) setSpark((s) => [...s, Math.max(0, rx - last.rx)].slice(-SPARK_LENGTH));
          else setSpark([]);
          lastRx.current = { tunnel: st.tunnel_id, rx };
        } else {
          lastRx.current = null;
          setSpark([]);
        }
      } else if (e.type === "tunnels_changed") setTunnels(e.data);
      else if (e.type === "settings_changed") setSettings(e.data);
      // the connection in progress advances with the events too
      const current = runRef.current;
      if (current) updateRun(current.id, (r) => onEvent(r, e));
    },
    [setStatus, setTunnels, updateRun, runRef],
  );

  // connection to the service, retried every `retryMs` while it is down, and the clock
  // for the relative times
  useEffect(() => {
    let cancelled = false;
    let current: DaemonClient | null = null;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const attempt = async () => {
      try {
        const c = await connect();
        if (cancelled) return c.close();
        // subscription first, so no event between the snapshot and the listener is lost
        current = c;
        c.on("event", onServiceEvent);
        c.on("close", () => {
          if (cancelled) return;
          setClient(null);
          setService("down");
          timer = setTimeout(attempt, retryMs);
        });
        // initial snapshot of the state
        const [st, list, cfg] = await Promise.all([
          c.request({ method: "get_status" }, "status"),
          c.request({ method: "list_tunnels" }, "tunnels"),
          c.request({ method: "get_settings" }, "settings"),
        ]);
        setStatus(st.data);
        setTunnels(list.data);
        setSettings(cfg.data);
        setClient(c);
        setService("up");
      } catch {
        if (cancelled) return;
        setService("down");
        timer = setTimeout(attempt, retryMs);
      }
    };
    void attempt();
    const tick = setInterval(() => setNow(Date.now() / 1000), 1000);
    return () => {
      cancelled = true;
      clearTimeout(timer);
      clearInterval(tick);
      current?.close();
    };
  }, [connect, retryMs, onServiceEvent, setStatus, setTunnels]);

  // the tunnel is up: give the server some time to answer the handshake
  const waitingHandshake = run && run.steps.routes === "done" && !run.result ? run.id : null;
  useEffect(() => {
    if (waitingHandshake === null) return;
    const timer = setTimeout(() => updateRun(waitingHandshake, timeoutRun), HANDSHAKE_WAIT_MS);
    return () => clearTimeout(timer);
  }, [waitingHandshake, updateRun]);

  // start of a connection followed step by step; switching tunnel first notes the
  // disconnection from the previous one
  const startConnect = useCallback(
    (c: DaemonClient, tunnel: TunnelInfo) => {
      const before = statusRef.current;
      if (before?.state === "connected" && before.tunnel_id !== tunnel.id) {
        const name = tunnelsRef.current.find((t) => t.id === before.tunnel_id)?.name ?? "tunnel";
        push({ kind: "lines", lines: [[[marks.off, "dim"], [`Disconnesso da ${name}`, "fg"]]] });
      }
      const id = nextRun.current++;
      setRun(startRun(id, tunnel));
      c.request({ method: "connect", params: { id: tunnel.id } }, "ok")
        .then(async () => {
          // the service also answers Ok when the attempt was cancelled elsewhere
          const { data } = await c.request({ method: "get_status" }, "status");
          if (data.tunnel_id !== tunnel.id || data.state === "disconnected") updateRun(id, cancelRun);
          else updateRun(id, (r) => onEvent(r, { type: "status_changed", data }));
        })
        .catch((err: unknown) => updateRun(id, (r) => failRun(r, err instanceof Error ? err.message : String(err))));
    },
    [push, setRun, updateRun, statusRef, tunnelsRef],
  );

  // prompt state
  const [input, setInput, inputRef] = useSyncState("");
  /** Bumped when the input is replaced, so the cursor moves to the end. */
  const [revision, setRevision] = useState(0);
  /** Input before the latest keystroke, restored when a Ctrl shortcut inserts its letter. */
  const stableInput = useRef("");
  const [, setHistory, historyRef] = useSyncState<string[]>([]);
  const [, setHistoryIndex, historyIndexRef] = useSyncState<number | null>(null);
  const [asyncSuggestions, setAsyncSuggestions] = useState<Suggestion[]>([]);
  const [selected, setSelected, selectedRef] = useSyncState(0);
  /** The menu was closed with Esc, until the input changes. */
  const [menuOff, setMenuOff] = useSyncState(false);
  /** The user moved through the menu with the arrows. */
  const [, setNavigated, navigatedRef] = useSyncState(false);
  const [busy, setBusy] = useState<string | null>(null);
  /** A command is running: input is ignored meanwhile. */
  const [running, setRunning, runningRef] = useSyncState(false);
  const [choice, setChoice, choiceRef] = useSyncState<Choice | null>(null);
  const [choiceIndex, setChoiceIndex, choiceIndexRef] = useSyncState(0);

  // commands need the service, except the offline ones
  const session: Session | null = client ? { client, interactive: true, yes: false } : null;

  // replacement of the whole input (completion, history, reset); `menu` false keeps the
  // menu closed, e.g. while browsing the history
  const replaceInput = useCallback(
    (value: string, menu = true) => {
      setInput(value);
      stableInput.current = value;
      setRevision((r) => r + 1);
      setSelected(0);
      setNavigated(false);
      setMenuOff(!menu);
    },
    [setInput, setSelected, setNavigated, setMenuOff],
  );

  // typing reopens the menu and leaves the history
  const onChange = (value: string) => {
    setInput(value);
    setSelected(0);
    setNavigated(false);
    setMenuOff(false);
    setHistoryIndex(null);
  };

  // arguments are completed by the commands (tunnel names, files...); a stale answer
  // for an older input is dropped
  const text = input.trimStart();
  const space = text.indexOf(" ");
  useEffect(() => {
    let stale = false;
    setAsyncSuggestions([]);
    if (!text.startsWith("/") || space < 0 || !session) return;
    const cmd = findCommand(text.slice(1, space));
    if (!cmd?.complete) return;
    const args = parseLine(text.slice(space + 1));
    // a trailing space starts a new, empty argument
    if (text.endsWith(" ")) args.push("");
    const typed = text.slice(space + 1);
    cmd
      .complete(session, args)
      .then((items) => {
        if (stale) return;
        setAsyncSuggestions(
          items
            .filter((item) => item !== typed)
            .map((item) => ({
              label: `/${cmd.name} ${item}`,
              value: `/${cmd.name} ${item}`,
              // directories and the `apps` actions still need more input
              runnable: !/[\\/]$/.test(item) && item !== "add" && item !== "remove",
            })),
        );
      })
      .catch(() => {});
    return () => {
      stale = true;
    };
    // `session` changes identity every render; the client is what matters
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [text, client]);

  // the menu: command names while the first word is typed, then the argument candidates
  const suggestions: Suggestion[] =
    !text.startsWith("/") || menuOff || choice ? [] : space < 0 ? commandSuggestions(text, tunnels) : asyncSuggestions;
  const suggestionsRef = useRef(suggestions);
  suggestionsRef.current = suggestions;

  // effects of a command outcome on the transcript and the prompt
  const handleOutcome = useCallback(
    (outcome: Outcome) => {
      if (outcome.lines.length > 0) push({ kind: "lines", lines: outcome.lines });
      if (outcome.choice) {
        setChoice(outcome.choice);
        setChoiceIndex(0);
      }
      if (outcome.connect && client) startConnect(client, outcome.connect);
      if (outcome.clear) {
        setEntries([]);
        setTranscript((n) => n + 1);
      }
      // let the goodbye line reach the terminal first
      if (outcome.exit) setTimeout(() => exit(), 30);
    },
    [client, exit, push, setChoice, setChoiceIndex, startConnect],
  );

  // a command run with the input disabled; the spinner shows up only if it takes long
  const runTask = useCallback(
    async (label: string | undefined, task: () => Promise<Outcome>) => {
      setRunning(true);
      const timer = setTimeout(() => setBusy(label ?? "Attendere"), BUSY_DELAY_MS);
      try {
        handleOutcome(await task());
      } catch (err) {
        push({ kind: "lines", lines: errorLines(err) });
      } finally {
        clearTimeout(timer);
        setBusy(null);
        setRunning(false);
      }
    },
    [handleOutcome, push, setRunning],
  );

  // execution of a submitted line
  const execute = useCallback(
    (line: string) => {
      const typed = line.trim();
      if (!typed) return;
      // the line goes to the transcript and the history, without duplicates
      push({ kind: "cmd", text: typed });
      setHistory((h) => [...h.filter((x) => x !== typed), typed]);
      setHistoryIndex(null);
      replaceInput("");
      // plain text, an unknown command (with a guess) or no service: only a hint
      if (!typed.startsWith("/")) {
        push({ kind: "lines", lines: [[[marks.ask, "yel"], ["I comandi iniziano con /. Prova ", "fg"], ["/status", "acc"]]] });
        return;
      }
      const [name = "", ...args] = parseLine(typed.slice(1));
      const cmd = findCommand(name);
      if (!cmd) {
        const guess = guessCommand(name);
        push({
          kind: "lines",
          lines: [
            [[marks.fail, "red"], ["Comando sconosciuto ", "fg"], [`/${name}`, "bold"]],
            guess
              ? [[marks.detail, "faint"], ["forse intendevi ", "dim"], [`/${guess.name}`, "acc"], ["?", "dim"]]
              : sub("scrivi /help per l’elenco dei comandi"),
          ],
        });
        return;
      }
      if (!session && !cmd.offline) {
        push({ kind: "lines", lines: [head([marks.fail, "red"], "Il servizio Submarine non è raggiungibile"), sub("riprovo a collegarmi da solo")] });
        return;
      }
      // a new connection or a disconnection replaces the attempt in progress
      const current = runRef.current;
      if (current && (cmd.name === "connect" || cmd.name === "disconnect")) updateRun(current.id, cancelRun);
      void runTask(cmd.busy, () => cmd.run(session ?? ({ interactive: true, yes: false } as Session), args));
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [client, push, runTask, replaceInput, updateRun],
  );

  // Enter: the selected suggestion wins when the user navigated the menu, or when a
  // single word is being completed into a command name; `direct` skips the menu
  const onSubmit = (value: string, direct = false) => {
    if (runningRef.current) return;
    if (direct) return execute(value);
    const items = suggestionsRef.current;
    const suggestion = items[Math.min(selectedRef.current, items.length - 1)];
    const singleWord = !value.trim().includes(" ");
    if (suggestion && (navigatedRef.current || (singleWord && !suggestion.label.includes(" ")))) {
      if (suggestion.runnable) execute(suggestion.label);
      else replaceInput(suggestion.value);
      return;
    }
    execute(value);
  };

  // keys outside the text input: choices, menu, history, Ctrl shortcuts
  useInput((ch, key) => {
    if (runningRef.current) return;

    // a choice takes every key: arrows, Enter or a digit, Esc to cancel
    const choice = choiceRef.current;
    if (choice) {
      const count = choice.options.length;
      if (key.escape) {
        setChoice(null);
        push({ kind: "lines", lines: [[[marks.wait, "dim"], ["Annullato", "dim"]]] });
      } else if (key.upArrow) setChoiceIndex((i) => (i - 1 + count) % count);
      else if (key.downArrow) setChoiceIndex((i) => (i + 1) % count);
      else if (key.return || /^[1-9]$/.test(ch)) {
        const option = choice.options[key.return ? choiceIndexRef.current : Number(ch) - 1];
        if (!option) return;
        setChoice(null);
        push({ kind: "cmd", text: option.label });
        void runTask(undefined, () => choice.choose(option.value));
      }
      return;
    }

    // typing, cursor and Enter belong to the text input; these keys do not
    const items = suggestionsRef.current;
    if (key.ctrl && /^[a-z]$/.test(ch)) {
      // the text input would insert the letter: undo it once it has; Ctrl+U clears
      // the line
      const before = stableInput.current;
      queueMicrotask(() => replaceInput(ch === "u" ? "" : before));
      return;
    }
    // the input after this keystroke becomes the one to restore
    queueMicrotask(() => {
      stableInput.current = inputRef.current;
    });
    // Tab completes the selected suggestion
    if (key.tab) {
      const suggestion = items[selectedRef.current];
      if (suggestion) replaceInput(suggestion.value);
      return;
    }
    // arrows move through the menu when it is open, through the history otherwise
    if (key.upArrow || key.downArrow) {
      if (items.length > 0) {
        const n = items.length;
        setSelected((i) => (key.upArrow ? (i - 1 + n) % n : (i + 1) % n));
        setNavigated(true);
        return;
      }
      const history = historyRef.current;
      const index = historyIndexRef.current;
      if (history.length === 0) return;
      const last = history.length - 1;
      const next = key.upArrow
        ? index === null ? last : Math.max(0, index - 1)
        : index === null ? null : index >= last ? null : index + 1;
      setHistoryIndex(next);
      replaceInput(next === null ? "" : history[next], false);
      return;
    }
    // Esc closes the menu, or clears the line when there is none
    if (key.escape) {
      if (items.length > 0) setMenuOff(true);
      else replaceInput("", false);
    }
  });

  // the scene shrinks on short terminals, see `RESERVED_ROWS`
  const sceneRows = Math.min(SCENE_ROWS, (rows || 24) - RESERVED_ROWS);
  const menuOpen = suggestions.length > 0;

  return (
    <FrameProvider animate={animate}>
      <Box flexDirection="column">
        <Static key={transcript} items={entries}>
          {(entry) => (
            // a blank line after each block, as in the design
            <Box key={entry.id} marginBottom={1}>
              {entry.kind === "cmd" ? (
                <Text color={c(palette.dim)} dimColor={!c(palette.dim)}>
                  › {entry.text}
                </Text>
              ) : (
                <LinesView lines={entry.lines} />
              )}
            </Box>
          )}
        </Static>

        {run && <LiveRun run={run} />}
        {busy && !run && <Busy label={busy} />}

        <Header version={version} width={columns || 80} sceneRows={sceneRows} scene={sceneState(service, status)} />
        {choice ? (
          <ChoiceList choice={choice} selected={choiceIndex} />
        ) : (
          <Prompt value={input} revision={revision} menuOpen={menuOpen} disabled={running} onChange={onChange} onSubmit={onSubmit} />
        )}
        {menuOpen && <Suggestions items={suggestions} selected={Math.min(selected, suggestions.length - 1)} />}
        <StatusLine service={service} status={status} tunnels={tunnels} settings={settings} spark={spark} now={now} />
      </Box>
    </FrameProvider>
  );
}

/** The connection in progress, animated, above the header. */
function LiveRun({ run }: { run: ConnectRun }) {
  const { t, animate } = useFrame();
  return (
    <Box marginBottom={1}>
      <LinesView lines={runLines(run, t, animate)} />
    </Box>
  );
}

/** Spinner with the text of the running command. */
function Busy({ label }: { label: string }) {
  const { t } = useFrame();
  return (
    <Box marginBottom={1}>
      <Text color={c(palette.yellow)}>{SPIN[t % SPIN.length]} </Text>
      <Text color={c(palette.fg)}>{label}…</Text>
    </Box>
  );
}

// Log view: the service's recent log lines, live, with a filter for warnings and errors and
// buttons to clear or copy them. Only reachable while the service is up.
import { useEffect, useRef, useState } from "react";

import { daemon, type LogLine, platform, request } from "../api";
import { Icon } from "../components/Icon";
import { LogList } from "../components/LogList";
import { Switch } from "../components/Switch";
import { useApp, useT } from "../store";

/** How long the copy button says "copied". */
const COPIED_MS = 1600;
/** At most as many lines as the daemon keeps. */
const MAX_LINES = 1000;
/** How far back a line logged during the first request may also appear. */
const OVERLAP = 200;

/** Whether a line is a warning or an error, for the problems filter. */
const isProblem = (l: LogLine) => l.level === "warn" || l.level === "error";
/** Whether two lines are the same entry, as lines have no id. */
const sameLine = (a: LogLine, b: LogLine) => a.time === b.time && a.target === b.target && a.message === b.message;
/** The newest MAX_LINES lines. */
const capped = (lines: LogLine[]) => (lines.length > MAX_LINES ? lines.slice(-MAX_LINES) : lines);

/** The service's recent log lines. */
export function LogsView() {
  const t = useT();
  const showToast = useApp((s) => s.showToast);
  const [lines, setLines] = useState<LogLine[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [onlyProblems, setOnlyProblems] = useState(false);
  const [copied, setCopied] = useState(false);
  const copiedTimer = useRef<ReturnType<typeof setTimeout>>(undefined);

  // the buffer once, then each new line as the daemon logs it
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    let loaded = false;
    // lines that arrive before the buffer is loaded, merged in once it is
    const early: LogLine[] = [];

    void (async () => {
      // subscription first, so no line falls between the buffer and the events
      const stop = await daemon.onEvent((event) => {
        if (event.type !== "log_line") return;
        if (loaded) setLines((current) => capped([...current, event.data]));
        else early.push(event.data);
      });
      if (cancelled) return stop();
      unlisten = stop;
      try {
        const { data } = await request({ method: "get_logs" }, "logs");
        if (cancelled) return;
        // lines logged while the request was in flight can be in both
        const tail = data.slice(-OVERLAP);
        const fresh = early.filter((line) => !tail.some((known) => sameLine(known, line)));
        loaded = true;
        setLines(capped([...data, ...fresh]));
        setError(null);
      } catch (err) {
        if (!cancelled) setError(String(err instanceof Error ? err.message : err));
      }
    })();

    return () => {
      cancelled = true;
      unlisten?.();
      clearTimeout(copiedTimer.current);
    };
  }, []);

  // newest first, as the list shows them
  const shown = (onlyProblems ? lines.filter(isProblem) : lines).slice().reverse();

  /** Clears the service's in-memory buffer only: its log file is not touched. */
  async function clear() {
    try {
      await request({ method: "clear_logs" }, "ok");
      setLines([]);
      showToast(t.logs.cleared);
    } catch (err) {
      showToast(t.logs.clearFailed(String(err instanceof Error ? err.message : err)), "error");
    }
  }

  /** Copies every line, as plain text with ISO times, to the clipboard. */
  async function copy() {
    const text = lines
      .map((l) => `${new Date(l.time).toISOString()} ${l.level.toUpperCase()} ${l.target}: ${l.message}`)
      .join("\n");
    try {
      await platform.copyText(text);
      setCopied(true);
      clearTimeout(copiedTimer.current);
      copiedTimer.current = setTimeout(() => setCopied(false), COPIED_MS);
    } catch {
      showToast(t.logs.copyFailed, "error");
    }
  }

  return (
    <div className="view page page-wide stagger">
      <header className="page-header">
        <h1>{t.logs.title}</h1>
        <p className="lead">{t.logs.lead}</p>
      </header>

      <div className="logs-toolbar">
        <Switch size="sm" className="switch-pill" pressed={onlyProblems} onChange={setOnlyProblems}>
          {t.logs.onlyProblems}
        </Switch>
        <span className="logs-count">{t.logs.lines(shown.length)}</span>
        <button type="button" className="btn btn-secondary" onClick={clear} disabled={lines.length === 0}>
          <Icon name="trash" size={16} strokeWidth={2} />
          {t.logs.clear}
        </button>
        <button type="button" className="btn btn-secondary" onClick={copy} disabled={lines.length === 0}>
          <Icon name="copy" size={16} strokeWidth={2} />
          <span aria-live="polite">{copied ? t.logs.copied : t.logs.copy}</span>
        </button>
      </div>

      {error && (
        <div className="banner" data-kind="error" role="alert">
          <Icon name="warning" />
          <p>{error}</p>
        </div>
      )}

      {shown.length === 0 ? <p className="hint">{t.logs.empty}</p> : <LogList lines={shown} locale={t.locale} />}
    </div>
  );
}

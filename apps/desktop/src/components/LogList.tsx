// List of service log lines, used by the Log view.
import { useMemo } from "react";

import type { LogLine } from "../api";
import { formatTime } from "../format";

/** Props of LogList. */
interface Props {
  /** Newest first. */
  lines: LogLine[];
  /** Locale for the times. */
  locale: string;
}

/**
 * Service log lines. Keys follow the content, not the position, so only lines that just
 * arrived play the entry animation.
 */
export function LogList({ lines, locale }: Props) {
  const keyed = useMemo(() => {
    // identical lines get a counter, so their keys stay unique
    const seen = new Map<string, number>();
    return lines.map((line) => {
      const base = `${line.time}|${line.target}|${line.message}`;
      const n = (seen.get(base) ?? 0) + 1;
      seen.set(base, n);
      return { line, key: `${base}|${n}` };
    });
  }, [lines]);

  return (
    <ol className="log-list">
      {keyed.map(({ line, key }) => (
        <li key={key} className="log-row" data-level={line.level}>
          <time className="log-time" dateTime={new Date(line.time).toISOString()}>
            {formatTime(line.time, locale)}
          </time>
          <span className="log-level">{line.level}</span>
          <span className="log-message">{line.message}</span>
        </li>
      ))}
    </ol>
  );
}

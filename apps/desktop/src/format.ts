// Locale-aware formatting of the values shown in the UI: traffic counters and log times.

/** Units for formatBytes, in steps of 1000 (SI, as most file managers show them). */
const units = ["B", "KB", "MB", "GB", "TB"];

/**
 * Formats a byte count with the largest unit that keeps the value at least 1, e.g.
 * `1.5 MB`. One decimal digit below 100, none above, so the width stays stable.
 */
export function formatBytes(bytes: number, locale: string): string {
  let value = bytes;
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit++;
  }
  const digits = unit === 0 || value >= 100 ? 0 : 1;
  return `${value.toLocaleString(locale, { maximumFractionDigits: digits, minimumFractionDigits: digits })} ${units[unit]}`;
}

/** Formats a Unix time in milliseconds as a local time of day with seconds. */
export function formatTime(ms: number, locale: string): string {
  return new Date(ms).toLocaleTimeString(locale, { hour: "2-digit", minute: "2-digit", second: "2-digit" });
}

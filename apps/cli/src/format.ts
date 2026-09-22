// Formatting of byte counts and times for the CLI output, in the Italian locale
// like the rest of the texts.

/** Byte units, in powers of 1000. */
const units = ["B", "KB", "MB", "GB", "TB"];

/**
 * Byte count in the largest unit below 1000 (decimal, not binary): "512 B", "1,5 MB",
 * "120 MB". One decimal digit under 100, none above.
 */
export function formatBytes(bytes: number): string {
  let value = bytes;
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit++;
  }
  const digits = unit === 0 || value >= 100 ? 0 : 1;
  return `${value.toLocaleString("it-IT", { maximumFractionDigits: digits, minimumFractionDigits: digits })} ${units[unit]}`;
}

/**
 * Age of a Unix time in seconds, relative to `now`: "adesso", "12 s fa", "3 min fa",
 * "2 h fa". Times in the future count as now.
 */
export function formatAgo(unixSeconds: number, now = Date.now() / 1000): string {
  const seconds = Math.max(0, Math.round(now - unixSeconds));
  if (seconds < 3) return "adesso";
  if (seconds < 60) return `${seconds} s fa`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)} min fa`;
  return `${Math.floor(seconds / 3600)} h fa`;
}

/** Clock time of a Unix time in milliseconds, local time: "11:35:14". */
export function formatClock(unixMs: number): string {
  const d = new Date(unixMs);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

/** Duration in seconds with its two largest units: "45 s", "12 min 05 s", "3 h 07 min". */
export function formatDuration(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  const pad = (n: number) => String(n).padStart(2, "0");
  if (s < 60) return `${s} s`;
  if (s < 3600) return `${Math.floor(s / 60)} min ${pad(s % 60)} s`;
  return `${Math.floor(s / 3600)} h ${pad(Math.floor(s / 60) % 60)} min`;
}

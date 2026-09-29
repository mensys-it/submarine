// Status line at the bottom of the interactive prompt: service and connection
// state with live traffic on the left, kill switch on the right.

import { Box, Text } from "ink";

import { killSwitchLabels, retryIn } from "../commands.ts";
import { formatAgo, formatBytes, formatDuration } from "../format.ts";
import type { Seg } from "../lines.ts";
import type { Settings, Status, TunnelInfo } from "../protocol.ts";
import { useFrame } from "./Frame.tsx";
import { SPIN, textProps } from "./theme.ts";

/** State of the connection to the service, not to the VPN. */
export type Service = "connecting" | "up" | "down";

/** Bytes received and sent in the latest second, as of Unix time `at` (seconds). */
export interface Rate {
  rx: number;
  tx: number;
  at: number;
}

interface Props {
  service: Service;
  /** Connection status; null until the first answer of the service. */
  status: Status | null;
  /** Known tunnels, for the name of the connected one. */
  tunnels: TunnelInfo[];
  /** Settings; null until the first answer of the service. */
  settings: Settings | null;
  /** Bytes received between consecutive updates from the service. */
  spark: number[];
  /** Latest throughput; null until two updates of the same tunnel arrived. */
  rate: Rate | null;
  /** Unix time in seconds, for the session time, the handshake age and the rate age. */
  now: number;
}

/** Bar heights of the sparkline, lowest first. */
const BARS = "▁▂▃▄▅▆▇█";

/** Bars scaled to the largest value; all flat when every value is 0. */
export function sparkline(values: number[]): string {
  const max = Math.max(...values);
  if (!(max > 0)) return values.map(() => BARS[0]).join("");
  return values.map((v) => BARS[Math.min(7, Math.floor((v / max) * 7.99))]).join("");
}

/** One line under the prompt: connection on the left, protection on the right. */
export function StatusLine({ service, status, tunnels, settings, spark, rate, now }: Props) {
  const { t } = useFrame();
  const spin = SPIN[t % SPIN.length] + " ";
  const name = tunnels.find((x) => x.id === status?.tunnel_id)?.name ?? "tunnel";

  // left: the service first, then the connection state
  let left: Seg[];
  if (service === "down") left = [["✗ ", "red"], ["Servizio Submarine non raggiungibile", "fg"], ["  riprovo…", "dim"]];
  else if (service === "connecting" || !status) left = [[spin, "yel"], ["Collegamento al servizio…", "dim"]];
  else {
    switch (status.state) {
      case "connected": {
        // the service sends nothing while the counters stand still: an old rate is zero
        const fresh = rate && now - rate.at < 2 ? rate : null;
        const handshake = status.peers[0]?.last_handshake;
        left = [
          // a slow pulse, every 8 frames
          [(t >> 3) % 2 ? "◉ " : "● ", "acc"],
          [name, "bold"],
          [`  ↓ ${formatBytes(fresh?.rx ?? 0)}/s`, "fg"],
          [`  ↑ ${formatBytes(fresh?.tx ?? 0)}/s`, "fg"],
        ];
        if (spark.length) left.push([`  ${sparkline(spark)}`, "acc"]);
        if (status.connected_since) left.push([`  ${formatDuration(now - status.connected_since)}`, "fg"]);
        left.push([handshake ? `  handshake ${formatAgo(handshake, now)}` : "  nessun handshake", "dim"]);
        break;
      }
      case "connecting":
        left = [[spin, "yel"], [`Connessione a ${name}…`, "fg"]];
        break;
      case "disconnecting":
        left = [[spin, "yel"], ["Disconnessione…", "fg"]];
        break;
      case "failed":
        left = [["✗ ", "red"], ["Connessione interrotta", "fg"], ["  /connect per riprovare", "dim"]];
        break;
      case "paused":
        left = [["‖ ", "yel"], [`${name} in pausa`, "fg"]];
        if (status.paused_until) left.push([`  riprende tra ${formatDuration(status.paused_until - now)}`, "dim"]);
        left.push(["  kill switch sospeso", "yel"]);
        break;
      case "reconnecting":
        left = [[spin, "yel"], [`Riconnessione a ${name}…`, "fg"]];
        if (status.retry_at) left.push([`  nuovo tentativo tra ${retryIn(status.retry_at, now)} s`, "dim"]);
        break;
      default:
        left = [["○ ", "dim"], ["Non connesso", "dim"]];
    }
  }

  // right: the kill switch, or the block it is enforcing
  const ks = settings?.kill_switch ?? "off";
  const right: Seg[] = status?.blocked
    ? [["internet bloccato dal kill switch", "yel"]]
    : [[`kill switch ${killSwitchLabels[ks]}`, ks === "off" ? "dim" : "acc"]];
  right.push(["  ·  ", "faint"], ["? /help", "dim"]);

  return (
    <Box justifyContent="space-between" paddingX={1} gap={2}>
      <Segs segs={left} />
      <Segs segs={right} />
    </Box>
  );
}

/** Segments on a single line, truncated at the terminal width. */
function Segs({ segs }: { segs: Seg[] }) {
  return (
    <Text wrap="truncate">
      {segs.map(([text, style], i) => (
        <Text key={i} {...textProps(style)}>
          {text}
        </Text>
      ))}
    </Text>
  );
}

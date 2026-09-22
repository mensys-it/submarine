// Live throughput chart of the tunnel view: download and upload rates over the last
// minute, drawn as inline SVG from the samples the store derives from status events.

import type { TrafficSample } from "../store";

/** Width of the time window shown, in milliseconds. */
export const TRAFFIC_WINDOW_MS = 60_000;

/** Size of the SVG coordinate system; the chart is stretched to the card width. */
const W = 600;
const H = 120;

/** Props of TrafficChart. */
interface Props {
  samples: TrafficSample[];
  /** Current time in milliseconds: the right edge of the window. */
  now: number;
  /** Accessible description of the chart, with the current rates. */
  label: string;
}

/** Polyline points of one direction, scaled to `max`; x follows the sample time. */
function points(samples: TrafficSample[], pick: (s: TrafficSample) => number, now: number, max: number) {
  const start = now - TRAFFIC_WINDOW_MS;
  return samples.map((s) => {
    const x = Math.max(0, ((s.time - start) / TRAFFIC_WINDOW_MS) * W);
    const y = H - (pick(s) / max) * (H - 4);
    return `${x.toFixed(1)},${y.toFixed(1)}`;
  });
}

/**
 * Download as a filled area, upload as a line, both scaled to the highest rate in the
 * window so that slow links still show their shape.
 */
export function TrafficChart({ samples, now, label }: Props) {
  const visible = samples.filter((s) => s.time >= now - TRAFFIC_WINDOW_MS);
  const max = Math.max(1, ...visible.map((s) => Math.max(s.rx, s.tx)));
  const rx = points(visible, (s) => s.rx, now, max);
  const tx = points(visible, (s) => s.tx, now, max);
  // the area closes on the baseline under the first and the last sample
  const first = rx[0]?.split(",")[0];
  const last = rx[rx.length - 1]?.split(",")[0];

  return (
    <svg className="traffic-chart" viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none" role="img" aria-label={label}>
      {[0.25, 0.5, 0.75].map((f) => (
        <line key={f} className="traffic-grid" x1="0" x2={W} y1={H * f} y2={H * f} />
      ))}
      {rx.length > 1 && (
        <>
          <polygon className="traffic-rx-area" points={`${first},${H} ${rx.join(" ")} ${last},${H}`} />
          <polyline className="traffic-rx" points={rx.join(" ")} />
          <polyline className="traffic-tx" points={tx.join(" ")} />
        </>
      )}
    </svg>
  );
}

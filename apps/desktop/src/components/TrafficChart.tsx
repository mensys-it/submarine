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

/** A point in the SVG coordinate system. */
type Point = [x: number, y: number];

/** Points of one direction, scaled to `max`; x follows the sample time. */
function points(samples: TrafficSample[], pick: (s: TrafficSample) => number, now: number, max: number): Point[] {
  const start = now - TRAFFIC_WINDOW_MS;
  return samples.map((s) => [
    Math.max(0, ((s.time - start) / TRAFFIC_WINDOW_MS) * W),
    H - (pick(s) / max) * (H - 4),
  ]);
}

/**
 * Smooth SVG path through `p`, made of cubic Bézier segments with monotone tangents
 * (Steffen's method).
 *
 * The tangents are limited by the slopes on both sides of each point, so the curve
 * NEVER overshoots the samples: it does not dip below the baseline after a burst, nor
 * rise above the highest rate, which a plain Catmull-Rom spline would do.
 */
function smoothPath(p: Point[]): string {
  const n = p.length;
  // width and slope of each segment; samples clamped to the left edge give zero widths
  const h = p.slice(1).map((q, i) => q[0] - p[i][0]);
  const s = p.slice(1).map((q, i) => (h[i] > 0 ? (q[1] - p[i][1]) / h[i] : 0));
  // tangent at each point: zero on a local peak or valley, the secant at the ends
  const m = p.map((_, i) => {
    if (i === 0) return s[0];
    if (i === n - 1) return s[n - 2];
    const [s0, s1, h0, h1] = [s[i - 1], s[i], h[i - 1], h[i]];
    const mid = h0 + h1 > 0 ? (s0 * h1 + s1 * h0) / (h0 + h1) : 0;
    return (Math.sign(s0) + Math.sign(s1)) * Math.min(Math.abs(s0), Math.abs(s1), 0.5 * Math.abs(mid));
  });
  // one cubic per segment, control points at a third of its width
  const f = (v: number) => v.toFixed(1);
  let d = `M${f(p[0][0])},${f(p[0][1])}`;
  for (let i = 0; i < n - 1; i++) {
    const [x0, y0] = p[i];
    const [x1, y1] = p[i + 1];
    const dx = h[i] / 3;
    d += `C${f(x0 + dx)},${f(y0 + m[i] * dx)} ${f(x1 - dx)},${f(y1 - m[i + 1] * dx)} ${f(x1)},${f(y1)}`;
  }
  return d;
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
  const rxPath = rx.length > 1 ? smoothPath(rx) : "";

  return (
    <svg className="traffic-chart" viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none" role="img" aria-label={label}>
      {[0.25, 0.5, 0.75].map((f) => (
        <line key={f} className="traffic-grid" x1="0" x2={W} y1={H * f} y2={H * f} />
      ))}
      {rx.length > 1 && (
        <>
          {/* the area closes on the baseline under the last and the first sample */}
          <path
            className="traffic-rx-area"
            d={`${rxPath}L${rx[rx.length - 1][0].toFixed(1)},${H}L${rx[0][0].toFixed(1)},${H}Z`}
          />
          <path className="traffic-rx" d={rxPath} />
          <path className="traffic-tx" d={smoothPath(tx)} />
        </>
      )}
    </svg>
  );
}

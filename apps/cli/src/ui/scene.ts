// The ocean in the header, drawn as a grid of characters for frame `t`
// (one frame every ~110 ms). Pure functions: the component only keeps the
// water level and the submarine's depth, which move one row every 3 frames.

/** What the scene shows: surfaced, diving, cruising underwater, or in trouble. */
export type Mood = "off" | "connecting" | "on" | "error";

/** Text sharing the same colors, the unit drawn by the header. */
export interface Run {
  text: string;
  fg?: string;
  bg?: string;
}

/** One character of the grid. */
interface Cell {
  ch: string;
  fg?: string;
  bg?: string;
}

/** Vertical position of the water and of the submarine. */
export interface Depth {
  /** Row of the wave crests; rows below it are water. */
  level: number;
  /** Top row of the submarine (3 rows tall). */
  subRow: number;
}

/** Rows of the scene on a tall enough terminal. */
export const SCENE_ROWS = 8;
/** Below this the submarine does not fit: a wave line instead. */
export const MIN_SCENE_ROWS = 5;
/** Below this the scene is too cramped: a wave line instead. */
export const MIN_SCENE_WIDTH = 60;

/** The submarine, 3 rows; `P` is the propeller, `●` the portholes. */
const SUB = ["      ▗▟█▖      ", "P▟███●██●██●██▙▖", "  ▝▀▀▀▀▀▀▀▀▀▀▀▘ "];
/** Frames of the spinning propeller. */
const PROP = ["-", "\\", "|", "/"];
/** Wave crest heights, lowest first. */
const CREST = "▁▂▃▄▅▆";
/** Water background by depth, lighter at the surface. */
const WATER = ["#145A82", "#104A6E", "#0D3C5A", "#0A2F48", "#08263B", "#071F31", "#061A29"];
/** Stars of the 118-column prototype: [x, row]. */
const STARS = [[4, 0], [19, 1], [33, 0], [47, 2], [61, 0], [74, 1], [88, 0], [101, 2], [12, 3], [95, 3], [112, 1], [55, 4]];

/**
 * Depth a mood aims at: the water rises from off to connecting to on. The submarine
 * floats at the surface (`level`), except when connected, where it cruises near the
 * bottom.
 */
function targets(mood: Mood, height: number, level: number): Depth {
  const target = mood === "on" ? 1 : mood === "connecting" ? Math.max(1, height - 4) : height - 2;
  return { level: target, subRow: mood === "on" ? height - 4 : level - 1 };
}

/** Where the scene starts for a mood, without the dive. */
export function restingDepth(mood: Mood, height: number): Depth {
  const { level } = targets(mood, height, 0);
  return { level, subRow: targets(mood, height, level).subRow };
}

/** One step of the dive (or of the way back up) towards the mood's depth. */
export function stepDepth(d: Depth, mood: Mood, height: number): Depth {
  let level = d.level;
  const target = targets(mood, height, level).level;
  if (level !== target) level += target > level ? 1 : -1;
  const sub = targets(mood, height, level).subRow;
  const subRow = d.subRow === sub ? sub : d.subRow + (sub > d.subRow ? 1 : -1);
  return { level, subRow };
}

/** Crest at column `x`: two sine waves moving at different speeds. */
function crest(x: number, t: number): string {
  const v = Math.sin(x * 0.32 + t * 0.35) + 0.6 * Math.sin(x * 0.11 - t * 0.22);
  return CREST[Math.max(0, Math.min(5, Math.round(((v + 1.6) / 3.2) * 5)))];
}

/** Adjacent cells with the same colours become one run. */
function runs(row: Cell[]): Run[] {
  const out: Run[] = [];
  for (const cell of row) {
    const last = out[out.length - 1];
    if (last && last.fg === cell.fg && last.bg === cell.bg) last.text += cell.ch;
    else out.push({ text: cell.ch, fg: cell.fg, bg: cell.bg });
  }
  return out;
}

/** Inputs of `drawScene`. */
export interface SceneOptions {
  /** Animation frame. */
  t: number;
  width: number;
  height: number;
  depth: Depth;
  mood: Mood;
  /** Color of the signals and of the sonar. */
  accent: string;
}

/** The scene for frame `t` as rows of runs, `height` rows of `width` columns. */
export function drawScene({ t, width: W, height: H, depth, mood, accent }: SceneOptions): Run[][] {
  const { level, subRow } = depth;
  const g: Cell[][] = Array.from({ length: H }, () => Array.from({ length: W }, () => ({ ch: " " })));
  const put = (y: number, x: number, ch: string, fg?: string) => {
    if (y < 0 || y >= H || x < 0 || x >= W) return;
    g[y][x] = { ch, fg, bg: g[y][x].bg };
  };

  // twinkling stars above the water, scaled from the prototype width, and the moon
  STARS.forEach(([sx, sy], i) => {
    if (sy >= level) return;
    const on = ((t >> 2) + i * 3) % 8 < 5;
    put(sy, Math.round((sx * W) / 118), on ? "·" : " ", "#5B6B82");
  });
  if (level > 1) put(0, W - 6, "◖", "#E6EEF7");

  // crests, then the water, darker with depth and with a sparse moving texture
  for (let x = 0; x < W; x++) g[level][x] = { ch: crest(x, t), fg: WATER[0] };
  for (let y = level + 1; y < H; y++) {
    const bg = WATER[Math.min(y - level - 1, WATER.length - 1)];
    for (let x = 0; x < W; x++) {
      g[y][x] = (x * 7 + y * 13 + (t >> 1)) % 37 === 0 ? { ch: "~", fg: "#2F86B5", bg } : { ch: " ", bg };
    }
  }

  // the submarine, swaying while cruising; propeller and portholes follow the mood
  const sway = mood === "on" ? Math.round(12 * Math.sin(t * 0.03)) : 0;
  const subX = Math.min(W - SUB[0].length - 1, Math.round((78 * W) / 118) + sway);
  const prop = mood === "on" ? PROP[t % 4] : mood === "connecting" ? PROP[(t >> 2) % 4] : "|";
  const port = mood === "on" ? "#BFFBF2" : mood === "connecting" ? "#F5D58A" : "#7A5A1A";
  SUB.forEach((shape, r) => {
    [...shape].forEach((ch, i) => {
      if (ch === " ") return;
      const glyph = ch === "P" ? prop : ch;
      put(subRow + r, subX + i, glyph, ch === "●" ? port : r === 0 || i === 0 ? "#E2A132" : "#F4B740");
    });
  });

  // signal above the conning tower
  const blink = (t >> 2) % 2 === 0;
  if (mood === "error" && blink) put(subRow - 1, subX + 8, "!", "#FF7A6B");
  if (mood === "connecting" && blink) put(subRow - 1, subX + 8, "•", "#F5B54A");
  if (mood === "on") put(subRow - 1, subX + 8, "•", accent);

  // bubbles rising behind the submarine while cruising
  if (mood === "on") {
    for (let k = 0; k < 6; k++) {
      const age = (t + k * 9) % 30;
      const y = subRow + 1 - Math.floor(age / 5);
      const x = subX - 1 - Math.floor(age / 8) + (k % 2);
      if (y > level) put(y, x, age < 10 ? "·" : age < 20 ? "°" : "o", "#BFF4FF");
    }
  }
  if (mood === "connecting") {
    // sonar pings on both sides
    for (const off of [0, 7]) {
      const r = (((t >> 1) + off) % 14) + 1;
      put(subRow + 1, subX - 1 - r, "(", accent);
      put(subRow + 1, subX + SUB[0].length + r, ")", accent);
    }
  }
  return g.map(runs);
}

/** Narrow terminals: just the crests. */
export function drawWave(t: number, width: number): Run[] {
  return runs(Array.from({ length: width }, (_, x) => ({ ch: crest(x, t), fg: "#2F86B5" })));
}

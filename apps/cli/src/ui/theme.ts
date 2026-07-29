// Colors of the CLI design, and their use in the interactive prompt (Ink props)
// and in script mode (ANSI escapes). Terminals have no alpha, so the translucent
// borders of the design are blended over its page background (#0B111B).

import type { Style } from "../lines.ts";

/** Colors of the design, by role. */
export const palette = {
  fg: "#C9D4E0",
  dim: "#6E7F95",
  faint: "#3A4B61",
  bold: "#F1F5FA",
  accent: "#3DD6C4",
  yellow: "#F4B740",
  red: "#FF7A6B",
  blue: "#8FB4FF",
  placeholder: "#55657A",
  /** Accent at 40%. */
  boxBorder: "#1F605F",
  /** Accent at 60%, input with the menu open. */
  inputActive: "#298780",
  inputBorder: "#33465F",
};

/** Colors are off with `NO_COLOR` (https://no-color.org) or output that is not a terminal. */
export const colorEnabled = !process.env.NO_COLOR && Boolean(process.stdout.isTTY);
/** Animations need a terminal and colour: the scene is drawn with backgrounds. */
export const animationsEnabled = colorEnabled;

/** A colour, or nothing when colours are off. */
export const c = (hex: string | null | undefined) => (colorEnabled && hex ? hex : undefined);

/** Color and weight of each style of the output lines. */
const styles: Record<Style, { color: string; bold?: boolean }> = {
  fg: { color: palette.fg },
  dim: { color: palette.dim },
  faint: { color: palette.faint },
  bold: { color: palette.bold, bold: true },
  acc: { color: palette.accent },
  yel: { color: palette.yellow },
  red: { color: palette.red },
  redB: { color: palette.red, bold: true },
  blue: { color: palette.blue },
};

/** Props for `<Text>` for a style of the design; without colors the dim styles use `dimColor`. */
export function textProps(style: Style) {
  const s = styles[style];
  return { color: c(s.color), bold: s.bold, dimColor: !colorEnabled && (style === "dim" || style === "faint") };
}

/** Same, as 24-bit ANSI escapes for script mode; plain text when `color` is false. */
export function ansi(style: Style, text: string, color: boolean): string {
  if (!color) return text;
  const s = styles[style];
  const [r, g, b] = [1, 3, 5].map((i) => parseInt(s.color.slice(i, i + 2), 16));
  return `\x1b[${s.bold ? "1;" : ""}38;2;${r};${g};${b}m${text}\x1b[0m`;
}

/** Frames of the spinner. */
export const SPIN = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

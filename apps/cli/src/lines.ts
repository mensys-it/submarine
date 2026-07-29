// Output lines as styled segments, shared by the interactive prompt and
// script mode. The styles are the roles of the design (see ui/theme.ts), mapped
// to Ink colors in the prompt and to ANSI codes in script mode.

/** Role of a text segment; ui/theme.ts gives each one its color. */
export type Style = "fg" | "dim" | "faint" | "bold" | "acc" | "yel" | "red" | "redB" | "blue";

/** A piece of text with its style. */
export type Seg = readonly [text: string, style: Style];
/** One output line, as a sequence of segments. */
export type Line = readonly Seg[];

/** Marks at the start of a line, with their trailing space. */
export const marks = {
  on: "● ",
  off: "○ ",
  wait: "◌ ",
  ok: "✓ ",
  fail: "✗ ",
  warn: "! ",
  ask: "? ",
  detail: "  ⎿ ",
} as const;

/** "● Title" with the mark in its own colour, then optional extra segments. */
export function head(mark: Seg, title: string, ...rest: Seg[]): Line {
  return [mark, [title, "bold"], ...rest];
}

/** Indented detail under the previous line. */
export function sub(text: string, style: Style = "dim"): Line {
  return [[marks.detail, "faint"], [text, style]];
}

/** Text of a line without styles, e.g. for tests and width computations. */
export function plain(line: Line): string {
  return line.map((s) => s[0]).join("");
}

/** Empty line; a single space so it still takes up a row when rendered. */
export const blank: Line = [[" ", "fg"]];

// Suggestion menu under the prompt: commands and argument completions, with a
// window of at most MENU_ROWS rows that follows the selection.

import { Box, Text } from "ink";

import { c, palette } from "./theme.ts";

/** An entry of the menu. */
export interface Suggestion {
  /** Full command, e.g. "/connect Laboratorio". */
  label: string;
  /** Shown next to the label. */
  description?: string;
  /** Input after accepting the suggestion with Tab. */
  value: string;
  /** Enter runs it right away. */
  runnable: boolean;
}

/** Maximum number of rows of the menu. */
export const MENU_ROWS = 7;

/** The menu, with `selected` highlighted and the labels in a column. */
export function Suggestions({ items, selected }: { items: Suggestion[]; selected: number }) {
  // first visible row, so the selection stays in the window
  const start = Math.max(0, Math.min(selected - MENU_ROWS + 1, items.length - MENU_ROWS));
  const visible = items.slice(start, start + MENU_ROWS);
  const width = Math.max(28, ...visible.map((s) => s.label.length + 2));
  return (
    <Box flexDirection="column" paddingX={2}>
      {visible.map((s, i) => {
        const active = start + i === selected;
        return (
          <Text key={s.value} wrap="truncate">
            <Text color={c(active ? palette.accent : palette.fg)} bold={active}>
              {(active ? "❯ " : "  ") + s.label.padEnd(width)}
            </Text>
            <Text color={c(active ? palette.fg : palette.dim)}>{s.description ?? ""}</Text>
          </Text>
        );
      })}
    </Box>
  );
}

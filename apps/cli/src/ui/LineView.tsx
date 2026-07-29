// Rendering of the styled output lines (see lines.ts) with Ink.

import { Box, Text } from "ink";

import type { Line } from "../lines.ts";
import { textProps } from "./theme.ts";

/** One line, wrapped at the terminal width. */
export function LineView({ line }: { line: Line }) {
  return (
    <Text wrap="wrap">
      {line.map(([text, style], i) => (
        <Text key={i} {...textProps(style)}>
          {text}
        </Text>
      ))}
    </Text>
  );
}

/** Lines stacked in a column. */
export function LinesView({ lines }: { lines: readonly Line[] }) {
  return (
    <Box flexDirection="column">
      {lines.map((line, i) => (
        <LineView key={i} line={line} />
      ))}
    </Box>
  );
}

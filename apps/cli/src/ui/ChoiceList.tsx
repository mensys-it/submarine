// The question asked when a command needs an argument the user did not give
// (e.g. which tunnel to connect), shown in place of the prompt. The keys are
// handled by App.

import { Box, Text } from "ink";

import type { Choice } from "../commands.ts";
import { c, palette } from "./theme.ts";

/** Numbered options of `choice`, with `selected` highlighted. */
export function ChoiceList({ choice, selected }: { choice: Choice; selected: number }) {
  return (
    <Box flexDirection="column" borderStyle="round" borderColor={c(palette.inputActive)} paddingX={1}>
      <Text color={c(palette.bold)} bold>
        {choice.title}
      </Text>
      <Box flexDirection="column" marginTop={1}>
        {choice.options.map((o, i) => {
          const active = i === selected;
          return (
            <Text key={o.value} wrap="truncate">
              <Text color={c(active ? palette.accent : palette.fg)} bold={active}>
                {active ? "❯ " : "  "}
                {i + 1}. {o.label}
              </Text>
              {o.hint && <Text color={c(palette.dim)}>  {o.hint}</Text>}
            </Text>
          );
        })}
      </Box>
      <Box marginTop={1}>
        <Text color={c(palette.dim)}>↑↓ per scegliere · Invio conferma · Esc annulla</Text>
      </Box>
    </Box>
  );
}

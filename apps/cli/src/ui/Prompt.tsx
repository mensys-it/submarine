// Input line of the interactive prompt, built on ink-text-input. It fixes the
// edits that arrive faster than the renders and runs pasted lines directly.

import { Box, Text } from "ink";
import TextInput from "ink-text-input";
import { useLayoutEffect, useRef, useState } from "react";

import { c, palette } from "./theme.ts";

interface Props {
  /** Current input, owned by App. */
  value: string;
  /** Changes when the value is replaced (completion, history): the cursor goes to the end. */
  revision: number;
  /** The suggestion menu is open: the border is highlighted. */
  menuOpen: boolean;
  /** A command is running: the input ignores keys. */
  disabled: boolean;
  onChange(value: string): void;
  /** `direct`: run exactly this line, ignoring the menu (pasted text). */
  onSubmit(value: string, direct?: boolean): void;
}

/** The edit that turns `a` into `b`: at `at`, `removed` characters replaced by `inserted`. */
function diff(a: string, b: string) {
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start++;
  let end = 0;
  while (end < a.length - start && end < b.length - start && a[a.length - 1 - end] === b[b.length - 1 - end]) end++;
  return { at: start, removed: a.length - start - end, inserted: b.slice(start, b.length - end) };
}

/** The bordered input line, with a placeholder when empty. */
export function Prompt({ value, revision, menuOpen, disabled, onChange, onSubmit }: Props) {
  // ink-text-input computes each edit from the value of the last render. Keys
  // that arrive together (a burst, a held backspace) would all start from
  // that same value: replay each edit on the latest one instead, then
  // remount the input so its cursor is right again
  const rendered = useRef(value);
  const latest = useRef(value);
  const shift = useRef(0);
  const [resync, setResync] = useState(0);
  useLayoutEffect(() => {
    rendered.current = value;
    latest.current = value;
    shift.current = 0;
  }, [value, revision]);

  // replay of an edit on the latest value; `shift` tracks how far the edits not yet
  // rendered moved the text
  const change = (next: string) => {
    const stale = latest.current !== rendered.current;
    const { at, removed, inserted } = diff(rendered.current, next);
    const pos = Math.max(0, Math.min(latest.current.length, at + shift.current));
    const applied = latest.current.slice(0, pos) + inserted + latest.current.slice(pos + removed);
    shift.current += inserted.length - removed;
    // pasted text (or keys sent in one go) may end lines with "\r": run the
    // first line as typed
    const newline = applied.search(/[\r\n]/);
    if (newline >= 0) {
      latest.current = rendered.current;
      shift.current = 0;
      onSubmit(applied.slice(0, newline), true);
      setResync((n) => n + 1);
      return;
    }
    // a plain edit; the input is remounted only if it rendered a stale value
    latest.current = applied;
    onChange(applied);
    if (stale) setResync((n) => n + 1);
  };

  return (
    <Box borderStyle="round" borderColor={c(menuOpen ? palette.inputActive : palette.inputBorder)} paddingX={1}>
      <Text color={c(palette.accent)} bold>
        ›{" "}
      </Text>
      <Text color={c(palette.bold)}>
        <TextInput
          key={`${revision}:${resync}`}
          value={value}
          focus={!disabled}
          onChange={change}
          onSubmit={() => onSubmit(latest.current)}
        />
      </Text>
      {value === "" && <Text color={c(palette.placeholder)}>{disabled ? "attendi…" : "Scrivi un comando, oppure / per l’elenco"}</Text>}
    </Box>
  );
}

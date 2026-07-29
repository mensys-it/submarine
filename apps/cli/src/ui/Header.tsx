// Header of the interactive prompt: title, version, the state caption, the ocean
// scene with the submarine (see scene.ts) and the main hints.

import { Box, Text } from "ink";
import { useRef } from "react";

import { useFrame } from "./Frame.tsx";
import { type Depth, drawScene, drawWave, MIN_SCENE_ROWS, MIN_SCENE_WIDTH, type Mood, restingDepth, type Run, stepDepth } from "./scene.ts";
import { c, colorEnabled, palette } from "./theme.ts";

/** What the header shows of the connection state. */
export interface SceneState {
  /** Drives the depth and the look of the scene. */
  mood: Mood;
  /** Caption on the right of the title. */
  label: string;
  /** Hex color of the caption. */
  labelColor: string;
}

interface Props {
  version: string;
  /** Terminal columns. */
  width: number;
  /** Rows for the ocean; below MIN_SCENE_ROWS a single wave line. */
  sceneRows: number;
  scene: SceneState;
}

/** Title, ocean and hints, in a rounded box. */
export function Header({ version, width, sceneRows, scene }: Props) {
  const inner = Math.max(10, width - 4);
  return (
    <Box borderStyle="round" borderColor={c(palette.boxBorder)} paddingX={1} flexDirection="column">
      <Box justifyContent="space-between">
        <Text wrap="truncate">
          <Text color={c(palette.accent)}>≋ </Text>
          <Text color={c(palette.bold)} bold>
            Submarine VPN
          </Text>
          <Text color={c(palette.dim)}> v{version}</Text>
        </Text>
        <Text color={c(scene.labelColor)} wrap="truncate">
          {scene.label}
        </Text>
      </Box>
      <Ocean width={inner} rows={sceneRows} mood={scene.mood} />
      <Text wrap="truncate">
        <Text color={c(palette.accent)}>/connect</Text>
        <Text color={c(palette.fg)}> per connetterti, </Text>
        <Text color={c(palette.accent)}>/status</Text>
        <Text color={c(palette.fg)}> per lo stato, </Text>
        <Text color={c(palette.accent)}>/help</Text>
        <Text color={c(palette.fg)}> per tutti i comandi</Text>
      </Text>
      <Text color={c(palette.dim)} dimColor={!colorEnabled} wrap="truncate">
        Uscire non chiude la VPN: resta gestita dal servizio.
      </Text>
    </Box>
  );
}

/** The scene; a single wave line when it does not fit, or without colors. */
function Ocean({ width, rows, mood }: { width: number; rows: number; mood: Mood }) {
  const { t, animate } = useFrame();
  const full = colorEnabled && rows >= MIN_SCENE_ROWS && width >= MIN_SCENE_WIDTH;
  const depth = useDepth(mood, rows, t, animate);
  if (!full) return <RunsRow runs={drawWave(t, width)} />;
  const grid = drawScene({ t, width, height: rows, depth, mood, accent: palette.accent });
  return (
    <Box flexDirection="column">
      {grid.map((runs, i) => (
        <RunsRow key={i} runs={runs} />
      ))}
    </Box>
  );
}

/** One row of the scene, as runs of text sharing the same colors. */
function RunsRow({ runs }: { runs: Run[] }) {
  return (
    <Text wrap="truncate">
      {runs.map((r, i) => (
        <Text key={i} color={c(r.fg)} backgroundColor={c(r.bg)}>
          {r.text}
        </Text>
      ))}
    </Text>
  );
}

/**
 * The submarine dives and surfaces one row every 3 frames. Without
 * animations it is drawn straight at the mood's depth.
 */
function useDepth(mood: Mood, rows: number, t: number, animate: boolean): Depth {
  const state = useRef<{ depth: Depth; t: number; rows: number } | null>(null);
  const s = state.current;
  // a new terminal height restarts from the resting depth
  if (!animate || !s || s.rows !== rows) {
    state.current = { depth: restingDepth(mood, rows), t, rows };
  } else {
    // catch-up of the frames since the previous render, which may skip some
    for (let f = s.t + 1; f <= t; f++) if (f % 3 === 0) s.depth = stepDepth(s.depth, mood, rows);
    s.t = t;
  }
  return state.current!.depth;
}

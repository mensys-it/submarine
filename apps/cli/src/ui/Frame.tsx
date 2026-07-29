// Animation clock of the interactive prompt, shared through a React context.

import { createContext, type ReactNode, useContext, useEffect, useState } from "react";

/** Duration of an animation frame. */
export const FRAME_MS = 110;

/** Current frame and whether animations are on. */
const FrameContext = createContext({ t: 0, animate: false });

/**
 * One timer for the scene, the spinners and the status line. Only the
 * components that read the frame re-render on each tick. Without animations
 * the frame stays 0.
 */
export function FrameProvider({ animate, children }: { animate: boolean; children: ReactNode }) {
  const [t, setT] = useState(0);
  useEffect(() => {
    if (!animate) return;
    const timer = setInterval(() => setT((n) => n + 1), FRAME_MS);
    return () => clearInterval(timer);
  }, [animate]);
  return <FrameContext.Provider value={{ t, animate }}>{children}</FrameContext.Provider>;
}

/** Current frame `t` (0 without animations) and the `animate` flag. */
export const useFrame = () => useContext(FrameContext);

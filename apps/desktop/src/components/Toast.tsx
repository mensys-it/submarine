// Toast at the bottom of the window, showing the store's latest notice for a few seconds.
import { useEffect } from "react";

import { useApp } from "../store";

/** How long a toast stays on screen. */
const VISIBLE_MS = 3200;

/** In-app feedback for the last action; one at a time, newest wins. */
export function Toast() {
  const toast = useApp((s) => s.toast);
  const dismiss = useApp((s) => s.dismissToast);

  // dismissal after a while; a newer toast restarts the timer
  useEffect(() => {
    if (!toast) return;
    const timer = setTimeout(() => dismiss(toast.id), VISIBLE_MS);
    return () => clearTimeout(timer);
  }, [toast, dismiss]);

  // the live region stays mounted so screen readers announce each message
  return (
    <div className="toast-region" role="status" aria-live="polite">
      {toast && (
        <div key={toast.id} className="toast" data-kind={toast.kind}>
          <span className="toast-dot" aria-hidden />
          <span>{toast.text}</span>
        </div>
      )}
    </div>
  );
}

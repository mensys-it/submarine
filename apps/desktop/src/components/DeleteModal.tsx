// Confirmation dialog for deleting a tunnel, opened from the tunnel view. Deleting
// removes the stored configuration, so the destructive button is red and the dialog
// always asks first.
import { useEffect, useId, useRef } from "react";

import type { TunnelInfo } from "../api";
import { useT } from "../store";
import { Icon } from "./Icon";

/** Props of DeleteModal. */
interface Props {
  /** Tunnel to delete; null keeps the dialog closed. */
  tunnel: TunnelInfo | null;
  onConfirm(): void;
  onCancel(): void;
}

/** Asks for confirmation before deleting `tunnel`; Esc, the backdrop and Cancel dismiss it. */
export function DeleteModal({ tunnel, onConfirm, onCancel }: Props) {
  const t = useT();
  const dialog = useRef<HTMLDialogElement>(null);
  const ids = { title: useId(), body: useId() };

  // native modal in step with the prop
  useEffect(() => {
    const el = dialog.current;
    if (!el) return;
    if (tunnel && !el.open) el.showModal();
    else if (!tunnel && el.open) el.close();
  }, [tunnel]);

  return (
    <dialog
      ref={dialog}
      className="modal modal-sm"
      role="alertdialog"
      aria-labelledby={ids.title}
      aria-describedby={ids.body}
      onClose={onCancel}
      // a click on the backdrop lands on the dialog element itself, and closes it
      onClick={(e) => e.target === e.currentTarget && onCancel()}
    >
      <div className="modal-body">
        <header className="modal-header">
          <span className="card-badge card-badge-lg danger-badge" aria-hidden>
            <Icon name="trash" strokeWidth={2} />
          </span>
          <div>
            <h2 id={ids.title}>{t.tunnel.deleteTitle}</h2>
            <p id={ids.body} className="lead">
              {tunnel ? t.tunnel.deleteConfirm(tunnel.name) : ""}
            </p>
          </div>
        </header>
        <div className="modal-actions">
          {/* the safe choice gets the focus, so Enter does not delete by mistake */}
          <button type="button" className="btn btn-secondary btn-lg" onClick={onCancel} autoFocus>
            {t.common.cancel}
          </button>
          <button type="button" className="btn btn-danger-solid btn-lg" onClick={onConfirm}>
            <Icon name="trash" size={18} strokeWidth={2} />
            {t.tunnel.deleteYes}
          </button>
        </div>
      </div>
    </dialog>
  );
}

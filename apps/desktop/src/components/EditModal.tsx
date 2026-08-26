// Dialog for renaming a tunnel and editing its WireGuard configuration, opened from the
// tunnel view. The store holds which tunnel is being edited.
import { useEffect, useId, useRef, useState, type FormEvent } from "react";

import { useApp, useT } from "../store";
import { Icon } from "./Icon";

/** Message of an error of any type, as thrown by Tauri or by the mock. */
const message = (err: unknown) => (err instanceof Error ? err.message : String(err));

/**
 * Renames a tunnel and edits its configuration. The daemon sends the keys hidden: left as
 * they are, they keep their stored value.
 */
export function EditModal() {
  const editId = useApp((s) => s.editId);
  const closeEdit = useApp((s) => s.closeEdit);
  const tunnelConfig = useApp((s) => s.tunnelConfig);
  const updateTunnel = useApp((s) => s.updateTunnel);
  const status = useApp((s) => s.status);
  const t = useT();
  const dialog = useRef<HTMLDialogElement>(null);
  const [name, setName] = useState("");
  const [config, setConfig] = useState("");
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const ids = { title: useId(), name: useId(), config: useId(), hint: useId(), error: useId() };

  // native modal in step with the store, with an empty form at every opening
  useEffect(() => {
    const el = dialog.current;
    if (!el) return;
    if (editId && !el.open) {
      setName("");
      setConfig("");
      setLoaded(false);
      setError(null);
      el.showModal();
    } else if (!editId && el.open) {
      el.close();
    }
  }, [editId]);

  // configuration of the tunnel being edited, ignored if the dialog moved on meanwhile
  useEffect(() => {
    if (!editId) return;
    let cancelled = false;
    tunnelConfig(editId).then(
      (data) => {
        if (cancelled) return;
        setName(data.name);
        setConfig(data.config);
        setLoaded(true);
      },
      (err) => !cancelled && setError(t.edit.loadError(message(err))),
    );
    return () => {
      cancelled = true;
    };
    // the texts only shape the error message: no need to reload when they change
  }, [editId, tunnelConfig]);

  /** Saves the changes; the store closes the dialog on success. */
  async function onSubmit(e: FormEvent) {
    e.preventDefault();
    if (!editId) return;
    setSaving(true);
    setError(null);
    try {
      await updateTunnel(editId, name.trim(), config);
    } catch (err) {
      setError(t.edit.invalid(message(err)));
    } finally {
      setSaving(false);
    }
  }

  // editing the active tunnel takes effect only at the next connection
  const inUse = status.tunnel_id === editId && (status.state === "connected" || status.state === "connecting");
  const canSubmit = loaded && !saving && config.trim() !== "" && name.trim() !== "";

  return (
    <dialog
      ref={dialog}
      className="modal"
      aria-labelledby={ids.title}
      onClose={closeEdit}
      // a click on the backdrop lands on the dialog element itself, and closes it
      onClick={(e) => e.target === e.currentTarget && closeEdit()}
    >
      <form className="modal-body" onSubmit={onSubmit}>
        <header className="modal-header">
          <div>
            <h2 id={ids.title}>{t.edit.title}</h2>
            <p className="lead">{t.edit.lead}</p>
          </div>
          <button type="button" className="icon-button btn" onClick={closeEdit} aria-label={t.common.close}>
            <Icon name="close" size={18} strokeWidth={2} />
          </button>
        </header>

        <div className="field">
          <label htmlFor={ids.name}>{t.import.name}</label>
          <input
            id={ids.name}
            className="input"
            value={name}
            onChange={(e) => setName(e.target.value)}
            maxLength={64}
            placeholder={t.import.namePlaceholder}
            disabled={!loaded}
            required
          />
        </div>

        <div className="field">
          <label htmlFor={ids.config}>{t.edit.config}</label>
          <textarea
            id={ids.config}
            className="input input-code input-code-tall"
            value={loaded ? config : ""}
            onChange={(e) => setConfig(e.target.value)}
            placeholder={loaded ? t.import.configPlaceholder : t.edit.loading}
            spellCheck={false}
            disabled={!loaded}
            aria-describedby={`${ids.hint}${error ? ` ${ids.error}` : ""}`}
          />
          <p id={ids.hint} className="hint">
            {t.edit.hiddenKeys}
          </p>
        </div>

        {inUse && (
          <div className="banner" data-kind="blocked" role="status">
            <Icon name="warning" size={18} strokeWidth={2} />
            <p>{t.edit.reconnect}</p>
          </div>
        )}

        {error && (
          <p id={ids.error} className="form-error" role="alert">
            {error}
          </p>
        )}

        <div className="modal-actions">
          <button type="button" className="btn btn-secondary btn-lg" onClick={closeEdit}>
            {t.common.cancel}
          </button>
          <button type="submit" className="btn btn-primary btn-lg" disabled={!canSubmit}>
            {saving ? t.edit.saving : t.edit.submit}
          </button>
        </div>
      </form>
    </dialog>
  );
}

// Dialog for importing a tunnel from a WireGuard .conf file, opened from the sidebar or the
// empty view, or by dragging a file over the window. The daemon validates the configuration.
import { useEffect, useId, useRef, useState, type FormEvent } from "react";

import { type ConfigFile, platform } from "../api";
import { useApp, useT } from "../store";
import { Icon } from "./Icon";

/** Message of an error of any type, as thrown by Tauri or by the mock. */
const message = (err: unknown) => (err instanceof Error ? err.message : String(err));

/**
 * Imports a .conf by dropping it on the window, choosing it or pasting it. Dragging a file
 * over the window opens the dialog by itself.
 */
export function ImportModal() {
  const open = useApp((s) => s.importOpen);
  const closeImport = useApp((s) => s.closeImport);
  const importTunnel = useApp((s) => s.importTunnel);
  const t = useT();
  const dialog = useRef<HTMLDialogElement>(null);
  const [name, setName] = useState("");
  const [config, setConfig] = useState("");
  const [fileName, setFileName] = useState<string | null>(null);
  const [dragging, setDragging] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const ids = { title: useId(), name: useId(), config: useId(), error: useId() };

  // native modal in step with the store, with an empty form at every opening
  useEffect(() => {
    const el = dialog.current;
    if (!el) return;
    if (open && !el.open) {
      setName("");
      setConfig("");
      setFileName(null);
      setError(null);
      el.showModal();
    } else if (!open && el.open) {
      el.close();
    }
  }, [open]);

  // a file read from disk: its text, and its name as the default tunnel name
  const loadFile = (file: ConfigFile) => {
    setConfig(file.text);
    setFileName(file.name);
    setError(null);
    setName((current) => current || file.name.replace(/\.conf$/i, ""));
  };

  // the backend rejects with a code for the known cases, otherwise with the OS error
  const readError = (err: unknown) => {
    const code = message(err);
    setError(code === "too_large" ? t.import.tooLarge : code === "not_text" ? t.import.notText : t.import.readError(code));
  };

  // kept in a ref so the drag listener, registered once, sees the current texts
  const readErrorRef = useRef(readError);
  readErrorRef.current = readError;

  // file drags over the whole window, listened to for the lifetime of the component
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void platform
      .onFileDrag((drag) => {
        // drags are ignored while the service is down, since importing would fail anyway
        const { daemonUp, importOpen, openImport } = useApp.getState();
        if (!daemonUp) return;
        if (drag.type === "over") {
          if (!importOpen) openImport();
          setDragging(true);
        } else if (drag.type === "leave") {
          setDragging(false);
        } else {
          setDragging(false);
          const path = drag.paths[0];
          if (path) platform.readConfigFile(path).then(loadFile, (err) => readErrorRef.current(err));
        }
      })
      // NB: the subscription may resolve after an unmount, then it is undone right away
      .then((fn) => (cancelled ? fn() : (unlisten = fn)));
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  /** Loads the file chosen in the system dialog. */
  async function choose() {
    try {
      const file = await platform.pickConfigFile();
      if (file) loadFile(file);
    } catch (err) {
      readError(err);
    }
  }

  /** Imports the tunnel; the store closes the dialog on success. */
  async function onSubmit(e: FormEvent) {
    e.preventDefault();
    setSaving(true);
    setError(null);
    try {
      await importTunnel(name.trim(), config);
    } catch (err) {
      setError(t.import.invalid(message(err)));
    } finally {
      setSaving(false);
    }
  }

  const dropTitle = dragging ? t.import.release : fileName ? t.import.loaded(fileName) : t.import.drop;
  const canSubmit = !saving && config.trim() !== "" && name.trim() !== "";

  return (
    <dialog
      ref={dialog}
      className="modal"
      aria-labelledby={ids.title}
      onClose={closeImport}
      // a click on the backdrop lands on the dialog element itself
      onClick={(e) => e.target === e.currentTarget && closeImport()}
    >
      <form className="modal-body" onSubmit={onSubmit}>
        <header className="modal-header">
          <div>
            <h2 id={ids.title}>{t.import.title}</h2>
            <p className="lead">{t.import.lead}</p>
          </div>
          <button type="button" className="icon-button btn" onClick={closeImport} aria-label={t.common.close}>
            <Icon name="close" size={18} strokeWidth={2} />
          </button>
        </header>

        <div className="dropzone" data-dragging={dragging || undefined}>
          <svg className="dropzone-border" aria-hidden>
            <rect x="0.8" y="0.8" width="100%" height="100%" rx="19" />
          </svg>
          <span className="card-badge card-badge-lg">
            <Icon name="upload" size={22} strokeWidth={2} />
          </span>
          <span className="dropzone-text">
            <span className="row-title ellipsis">{dropTitle}</span>
            <span className="row-help">{t.import.fileKind}</span>
          </span>
          <button type="button" className="btn btn-primary btn-sm" onClick={choose}>
            {t.import.chooseFile}
          </button>
        </div>

        <div className="field">
          <label htmlFor={ids.config}>{t.import.paste}</label>
          <textarea
            id={ids.config}
            className="input input-code"
            value={config}
            onChange={(e) => {
              setConfig(e.target.value);
              setFileName(null);
            }}
            placeholder={t.import.configPlaceholder}
            spellCheck={false}
            aria-describedby={error ? ids.error : undefined}
          />
        </div>

        <div className="field">
          <label htmlFor={ids.name}>{t.import.name}</label>
          <input
            id={ids.name}
            className="input"
            value={name}
            onChange={(e) => setName(e.target.value)}
            maxLength={64}
            placeholder={t.import.namePlaceholder}
            required
          />
        </div>

        {error && (
          <p id={ids.error} className="form-error" role="alert">
            {error}
          </p>
        )}

        <div className="modal-actions">
          <button type="button" className="btn btn-secondary btn-lg" onClick={closeImport}>
            {t.common.cancel}
          </button>
          <button type="submit" className="btn btn-primary btn-lg" disabled={!canSubmit}>
            {saving ? t.import.importing : t.import.submit}
          </button>
        </div>
      </form>
    </dialog>
  );
}

// Dialog for choosing the applications of split tunneling, opened from the Protection view:
// a searchable list of the installed apps, plus a field for any executable by path.
import { useEffect, useId, useMemo, useRef, useState, type FormEvent } from "react";

import { daemon, type AppRule } from "../api";
import { useT } from "../store";
import { Icon } from "./Icon";

/** Props of AppPickerDialog. */
interface Props {
  /** Whether the dialog is shown; the parent owns this state. */
  open: boolean;
  /** Apps already in the rule list, shown as added. */
  chosen: AppRule[];
  /** Called with the apps to add; the dialog stays open for more. */
  onAdd(apps: AppRule[]): void;
  /** Called when the dialog closes, by button, Escape or a click on the backdrop. */
  onClose(): void;
}

/** Picker of the applications to route in or out of the tunnel. */
export function AppPickerDialog({ open, chosen, onAdd, onClose }: Props) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [apps, setApps] = useState<AppRule[] | null>(null);
  const [query, setQuery] = useState("");
  const [manualPath, setManualPath] = useState("");
  const [manualError, setManualError] = useState<string | null>(null);
  const ids = { title: useId(), search: useId(), path: useId() };
  const t = useT();

  // native modal in step with `open`, with a fresh form and app list at every opening
  useEffect(() => {
    const el = dialog.current;
    if (!el) return;
    if (open && !el.open) {
      setQuery("");
      setManualPath("");
      setManualError(null);
      el.showModal();
      daemon.listApps().then(setApps, () => setApps([]));
    } else if (!open && el.open) {
      el.close();
    }
  }, [open]);

  // apps already chosen, and the ones matching the search
  const chosenPaths = useMemo(() => new Set(chosen.map((a) => a.path)), [chosen]);
  const visible = (apps ?? []).filter((a) => a.name.toLowerCase().includes(query.trim().toLowerCase()));

  /** Adds the executable typed by hand, if it looks like an absolute path. */
  function addManual(e: FormEvent) {
    e.preventDefault();
    const path = manualPath.trim();
    // absolute Unix path, or a Windows one with a drive letter
    if (!path.startsWith("/") && !/^[a-zA-Z]:\\/.test(path)) {
      setManualError(t.picker.manualError);
      return;
    }
    const name = path.split(/[\\/]/).pop() || path;
    onAdd([{ name, path }]);
    setManualPath("");
    setManualError(null);
  }

  return (
    <dialog
      ref={dialog}
      className="modal"
      onClose={onClose}
      // a click on the backdrop lands on the dialog element itself, and closes it
      onClick={(e) => e.target === e.currentTarget && onClose()}
      aria-labelledby={ids.title}
    >
      <div className="modal-body">
        <header className="modal-header">
          <h2 id={ids.title}>{t.picker.title}</h2>
          <button type="button" className="icon-button btn" onClick={onClose} aria-label={t.common.close}>
            <Icon name="close" size={18} strokeWidth={2} />
          </button>
        </header>

        <div className="field">
          <label htmlFor={ids.search}>{t.picker.search}</label>
          <input
            id={ids.search}
            className="input"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={t.picker.searchPlaceholder}
            autoFocus
          />
        </div>

        <ul className="picker-list" aria-live="polite">
          {apps === null && <li className="hint">{t.picker.loading}</li>}
          {apps !== null && visible.length === 0 && <li className="hint">{t.picker.noneFound}</li>}
          {visible.map((app) => {
            const added = chosenPaths.has(app.path);
            return (
              <li key={app.path} className="app-row">
                <span className="app-text">
                  <span className="app-name">{app.name}</span>
                  <span className="app-path">{app.path}</span>
                </span>
                <button type="button" className="btn btn-secondary btn-sm" disabled={added} onClick={() => onAdd([app])}>
                  {added ? t.picker.added : t.picker.add}
                </button>
              </li>
            );
          })}
        </ul>

        <form className="field" onSubmit={addManual}>
          <label htmlFor={ids.path}>{t.picker.manual}</label>
          <div className="inline-field">
            <input
              id={ids.path}
              className="input input-mono"
              value={manualPath}
              onChange={(e) => setManualPath(e.target.value)}
              placeholder="/opt/app/bin/app"
              spellCheck={false}
            />
            <button type="submit" className="btn btn-secondary" disabled={!manualPath.trim()}>
              {t.picker.add}
            </button>
          </div>
          {manualError && (
            <p className="form-error" role="alert">
              {manualError}
            </p>
          )}
        </form>

        <div className="modal-actions">
          <button type="button" className="btn btn-primary btn-lg" onClick={onClose}>
            {t.picker.done}
          </button>
        </div>
      </div>
    </dialog>
  );
}

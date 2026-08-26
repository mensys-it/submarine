// The single UI store (zustand). Daemon state (tunnels, status, settings) mirrors what the
// backend sends through events and is never changed locally, except for the optimistic
// settings save. Everything else here is UI state: current view, dialogs, toast, language.

import { create } from "zustand";

import { daemon, platform, type Prefs, request, type Settings, type Status, type TunnelInfo } from "./api";
import { type Texts, texts } from "./i18n";
import { browserLang, type Lang } from "./lang";

/** Status shown while the daemon is unknown or unreachable. */
const idle: Status = {
  state: "disconnected",
  tunnel_id: null,
  interface: null,
  peers: [],
  error: null,
  blocked: false,
  protection_error: null,
};

/** Settings shown until the daemon sends its own. */
export const defaultSettings: Settings = {
  kill_switch: "off",
  allow_lan: false,
  split_mode: "off",
  split_apps: [],
  prefer_tunnel: false,
  auto_connect: null,
};

/** Page shown in the main area; "tunnel" is the selected tunnel. */
export type View = "tunnel" | "protection" | "settings" | "logs" | "about";

/** A short notice shown at the bottom of the window. */
export interface Toast {
  /** Increasing id, so a timer dismisses only the toast it was started for. */
  id: number;
  text: string;
  kind: "ok" | "error";
}

/** State and actions of the store. */
interface AppState {
  /** Whether the daemon is reachable; null until the first check completes. */
  daemonUp: boolean | null;
  tunnels: TunnelInfo[];
  status: Status;
  settings: Settings;
  /** Preferences of the desktop app; null until loaded. */
  prefs: Prefs | null;
  /** Language of the UI texts. */
  lang: Lang;
  view: View;
  selectedId: string | null;
  importOpen: boolean;
  /** Tunnel whose edit dialog is open. */
  editId: string | null;
  /** Version of the desktop app; null until known. */
  version: string | null;
  sidebarCollapsed: boolean;
  /** Non-fatal notes from the last import, shown once. */
  importWarnings: string[];
  /** Whether those notes come from an import or an edit. */
  warningsFrom: "import" | "edit";
  toast: Toast | null;

  /** Subscribes to the backend and loads the initial state; called once by App. */
  start(): Promise<void>;
  /** Selects a tunnel and shows its view. */
  select(id: string): void;
  show(view: View): void;
  openImport(): void;
  closeImport(): void;
  openEdit(id: string): void;
  closeEdit(): void;
  toggleSidebar(): void;
  /** Shows a toast, replacing the current one. */
  showToast(text: string, kind?: Toast["kind"]): void;
  dismissToast(id: number): void;
  dismissWarnings(): void;
  /** Saves the app preferences; failures end up in a toast. */
  savePrefs(prefs: Prefs): Promise<void>;
  /** Saves the daemon settings optimistically; failures roll back and end up in a toast. */
  saveSettings(settings: Settings): Promise<void>;
  connect(id: string): Promise<void>;
  disconnect(): Promise<void>;
  /** Imports a tunnel; rejects so the dialog can show the error. */
  importTunnel(name: string, config: string): Promise<void>;
  /** Returns the name and the configuration with its keys hidden. */
  tunnelConfig(id: string): Promise<{ name: string; config: string }>;
  /** Replaces a tunnel's name and configuration; rejects so the dialog can show the error. */
  updateTunnel(id: string, name: string, config: string): Promise<void>;
  deleteTunnel(id: string): Promise<void>;
}

/** Message of an error of any type, as thrown by Tauri or by the mock. */
const message = (err: unknown) => (err instanceof Error ? err.message : String(err));

/** Last toast id handed out. */
let toastId = 0;

// NB: the sidebar state is a per-device convenience, so browser storage is enough; it may be
// unavailable (private mode, blocked storage), hence the fallbacks
const SIDEBAR_KEY = "submarine.sidebarCollapsed";
const storedCollapsed = () => {
  try {
    return localStorage.getItem(SIDEBAR_KEY) === "1";
  } catch {
    return false;
  }
};

/** The app store. Daemon events and connection changes update it once `start` ran. */
export const useApp = create<AppState>((set, get) => {
  // helpers shared by the actions below
  const t = () => texts(get().lang);
  const tunnelName = (id: string | null) => get().tunnels.find((x) => x.id === id)?.name ?? "Submarine";

  // keeps the selection on a tunnel that still exists, or moves it to the first one
  const setTunnels = (tunnels: TunnelInfo[]) => {
    const { selectedId } = get();
    const stillThere = tunnels.some((x) => x.id === selectedId);
    set({ tunnels, selectedId: stillThere ? selectedId : (tunnels[0]?.id ?? null) });
  };

  // immediate feedback on connecting and dropping. The backend skips the matching system
  // notifications while the window is in front (src-tauri/src/notify.rs)
  const setStatus = (next: Status) => {
    const before = get().status;
    set({ status: next });
    const name = tunnelName(next.tunnel_id);
    if (before.state === "connecting" && next.state === "connected") get().showToast(t().toast.connected(name));
    else if (before.state === "connecting" && next.state === "failed") get().showToast(t().toast.failed(name), "error");
    else if (before.state === "connected" && next.state === "failed") get().showToast(t().toast.dropped(name), "error");
  };

  // full reload of the daemon state, on start and on every reconnection
  const refresh = async () => {
    try {
      const [tunnels, status, settings] = await Promise.all([
        request({ method: "list_tunnels" }, "tunnels"),
        request({ method: "get_status" }, "status"),
        request({ method: "get_settings" }, "settings"),
      ]);
      setTunnels(tunnels.data);
      set({ status: status.data, settings: settings.data, daemonUp: true });
      // the active tunnel is the most useful one to show
      if (status.data.tunnel_id) set({ selectedId: status.data.tunnel_id });
    } catch {
      set({ daemonUp: false, status: idle });
    }
  };

  // action whose errors are only worth a toast
  const run = async (action: () => Promise<unknown>) => {
    try {
      await action();
    } catch (err) {
      get().showToast(message(err), "error");
    }
  };

  return {
    daemonUp: null,
    tunnels: [],
    status: idle,
    settings: defaultSettings,
    prefs: null,
    lang: browserLang(),
    view: "tunnel",
    selectedId: null,
    importOpen: false,
    editId: null,
    version: null,
    sidebarCollapsed: storedCollapsed(),
    importWarnings: [],
    warningsFrom: "import",
    toast: null,

    async start() {
      // preferences and version are not needed to start, so they load in the background
      daemon.getPrefs().then(
        (prefs) => set({ prefs, lang: prefs.resolved_language }),
        () => {},
      );
      platform.appVersion().then(
        (version) => set({ version }),
        () => {},
      );
      // subscriptions before the first load, so no change is lost in between
      await daemon.onEvent((event) => {
        // log lines are handled by the Log view while it is open
        if (event.type === "status_changed") setStatus(event.data);
        else if (event.type === "settings_changed") set({ settings: event.data });
        else if (event.type === "tunnels_changed") setTunnels(event.data);
      });
      await daemon.onConnection((connected) => {
        if (connected) void refresh();
        else set({ daemonUp: false, status: idle });
      });
      // first load, if the backend is already connected
      if (await daemon.isConnected()) await refresh();
      else set({ daemonUp: false });
    },

    select: (id) => set({ selectedId: id, view: "tunnel", importWarnings: [] }),

    show: (view) => set({ view }),

    openImport: () => set({ importOpen: true }),

    closeImport: () => set({ importOpen: false }),

    openEdit: (id) => set({ editId: id }),

    closeEdit: () => set({ editId: null }),

    toggleSidebar: () => {
      const sidebarCollapsed = !get().sidebarCollapsed;
      set({ sidebarCollapsed });
      try {
        localStorage.setItem(SIDEBAR_KEY, sidebarCollapsed ? "1" : "0");
      } catch {
        // not remembered: it still works for this session
      }
    },

    showToast: (text, kind = "ok") => set({ toast: { id: ++toastId, text, kind } }),

    dismissToast: (id) => {
      if (get().toast?.id === id) set({ toast: null });
    },

    dismissWarnings: () => set({ importWarnings: [] }),

    async savePrefs(prefs) {
      try {
        const saved = await daemon.setPrefs(prefs);
        set({ prefs: saved, lang: saved.resolved_language });
      } catch (err) {
        get().showToast(t().toast.notSaved(message(err)), "error");
      }
    },

    // applied optimistically; the daemon's answer (or event) is authoritative
    async saveSettings(settings) {
      const previous = get().settings;
      set({ settings });
      try {
        const saved = await request({ method: "set_settings", params: { settings } }, "settings");
        set({ settings: saved.data });
      } catch (err) {
        set({ settings: previous });
        get().showToast(t().toast.notSaved(message(err)), "error");
      }
    },

    connect: (id) => run(() => request({ method: "connect", params: { id } }, "ok")),

    disconnect: () => run(() => request({ method: "disconnect" }, "ok")),

    // errors propagate so the import dialog can show them next to the input
    async importTunnel(name, config) {
      const { data } = await request({ method: "import_tunnel", params: { name, config } }, "imported");
      set({ selectedId: data.tunnel.id, view: "tunnel", importWarnings: data.warnings, warningsFrom: "import", importOpen: false });
      get().showToast(t().toast.imported(data.tunnel.name));
    },

    async tunnelConfig(id) {
      const { data } = await request({ method: "get_tunnel_config", params: { id } }, "tunnel_config");
      return data;
    },

    // like importTunnel, errors propagate to the dialog
    async updateTunnel(id, name, config) {
      const { data } = await request({ method: "update_tunnel", params: { id, name, config } }, "imported");
      set({ selectedId: id, view: "tunnel", importWarnings: data.warnings, warningsFrom: "edit", editId: null });
      get().showToast(t().toast.updated(data.tunnel.name));
    },

    deleteTunnel: (id) => run(() => request({ method: "delete_tunnel", params: { id } }, "ok")),
  };
});

/** The UI texts in the current language; re-renders the caller when it changes. */
export function useT(): Texts {
  return texts(useApp((s) => s.lang));
}

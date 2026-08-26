// Root component of the desktop app: the sidebar, the view picked in the store and the
// dialogs and toast that can appear over any view.
import { useEffect } from "react";

import { EditModal } from "./components/EditModal";
import { ImportModal } from "./components/ImportModal";
import { Sidebar } from "./components/Sidebar";
import { Toast } from "./components/Toast";
import { useApp, useT } from "./store";
import { AboutView } from "./views/AboutView";
import { EmptyView } from "./views/EmptyView";
import { LogsView } from "./views/LogsView";
import { ProtectionView } from "./views/ProtectionView";
import { SettingsView } from "./views/SettingsView";
import { TunnelView } from "./views/TunnelView";

/**
 * Lays out the window and chooses the main view. The Log and Protection views need the
 * daemon, so without it the app falls back to the selected tunnel or the empty view.
 */
export function App() {
  const { start, tunnels, selectedId, daemonUp, view } = useApp();
  const t = useT();

  // document language kept in step with the UI texts, for screen readers and hyphenation
  useEffect(() => {
    document.documentElement.lang = t.locale.slice(0, 2);
  }, [t]);

  // daemon connection and event subscriptions, once on mount
  useEffect(() => {
    void start();
  }, [start]);

  const selected = tunnels.find((x) => x.id === selectedId);

  // the key replays the entry animation when switching views or tunnels
  let content;
  if (view === "settings") content = <SettingsView key="settings" />;
  else if (view === "about") content = <AboutView key="about" />;
  else if (view === "logs" && daemonUp) content = <LogsView key="logs" />;
  else if (view === "protection" && daemonUp) content = <ProtectionView key="protection" />;
  else if (selected) content = <TunnelView key={selected.id} tunnel={selected} />;
  else content = <EmptyView />;

  return (
    <div className="app">
      <Sidebar />
      <main className="main">{content}</main>
      <ImportModal />
      <EditModal />
      <Toast />
    </div>
  );
}

// Sidebar of the window: brand, tunnel list, import button, links to the other views and the
// state of the service. It can collapse to icons, remembered by the store.
import type { ConnectionState } from "../api";
import { useApp, useT, type View } from "../store";
import { Icon, type IconName } from "./Icon";

/** Dot colour and pulse for a connection state. */
export function StatusDot({ state }: { state: ConnectionState }) {
  return <span className="status-dot" data-state={state} aria-hidden />;
}

/** Initials for the tunnel avatar: "Ufficio Milano" to "UM", "office-laptop" to "OL". */
const initials = (name: string) =>
  name
    .split(/[\s_-]+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((word) => [...word][0]!.toUpperCase())
    .join("");

/**
 * Navigation of the app. Collapsed, it keeps only icons: labels stay in the accessibility
 * tree and show as tooltips.
 */
export function Sidebar() {
  const { tunnels, selectedId, status, daemonUp, select, view, show, settings, openImport, sidebarCollapsed, toggleSidebar, version } =
    useApp();
  const t = useT();
  // tooltips only when collapsed, since the labels are visible otherwise
  const tip = (label: string) => (sidebarCollapsed ? label : undefined);
  const service = daemonUp === null ? t.service.checking : daemonUp ? t.service.up : t.service.down;
  const toggleLabel = sidebarCollapsed ? t.nav.expand : t.nav.collapse;

  // link to a section view; most of them need the service and are disabled without it
  const section = (target: View, icon: IconName, label: string, detail: string, needsService = true) => (
    <button
      type="button"
      className="nav-item"
      aria-current={view === target ? "page" : undefined}
      onClick={() => show(target)}
      disabled={needsService && !daemonUp}
      title={tip(label)}
    >
      <Icon name={icon} />
      <span className="nav-text">
        <span className="nav-title">{label}</span>
        <span className="nav-detail">{detail}</span>
      </span>
    </button>
  );

  return (
    <aside className="sidebar" data-collapsed={sidebarCollapsed || undefined}>
      <div className="brand">
        {/* the product icon (src-tauri/icons/source.png); already a rounded tile */}
        <img className="brand-mark" src="/icon.png" alt="" width={44} height={44} />
        <span className="brand-text">
          <span className="brand-name">Submarine</span>
          <span className="brand-tagline">{t.brand.tagline}</span>
        </span>
        <button
          type="button"
          className="sidebar-toggle"
          onClick={toggleSidebar}
          aria-expanded={!sidebarCollapsed}
          aria-label={toggleLabel}
          title={toggleLabel}
        >
          <Icon name={sidebarCollapsed ? "panelOpen" : "panelClose"} />
        </button>
      </div>

      <div className="tunnel-group">
        <h2 className="eyebrow" id="tunnels-title">
          {t.nav.tunnels}
        </h2>
        <ul className="tunnel-list" aria-labelledby="tunnels-title">
          {tunnels.map((tunnel) => {
            // a tunnel is disconnected unless it is the one the status is about
            const state: ConnectionState = status.tunnel_id === tunnel.id ? status.state : "disconnected";
            return (
              <li key={tunnel.id}>
                <button
                  type="button"
                  className="nav-item"
                  aria-current={view === "tunnel" && tunnel.id === selectedId ? "page" : undefined}
                  onClick={() => select(tunnel.id)}
                  title={tip(`${tunnel.name} · ${t.state[state]}`)}
                >
                  <StatusDot state={state} />
                  <span className="tunnel-avatar" aria-hidden>
                    {initials(tunnel.name)}
                  </span>
                  <span className="nav-text">
                    <span className="nav-title ellipsis">{tunnel.name}</span>
                    <span className="nav-detail">{t.state[state]}</span>
                  </span>
                </button>
              </li>
            );
          })}
        </ul>
        <button
          type="button"
          className="import-button btn"
          onClick={openImport}
          disabled={!daemonUp}
          title={tip(t.nav.import)}
        >
          <Icon name="plus" size={18} strokeWidth={2} />
          <span className="nav-text">{t.nav.import}</span>
        </button>
      </div>

      <nav className="section-nav" aria-label={t.nav.sections}>
        {section("protection", "shield", t.nav.protection, t.summary.killSwitch[settings.kill_switch])}
        {section("settings", "sliders", t.nav.settings, t.nav.settingsDetail, false)}
        {section("logs", "lines", t.nav.logs, t.nav.logsDetail)}
        {section("about", "info", t.nav.about, t.nav.aboutDetail(version), false)}
      </nav>

      <p className="service-pill" data-up={daemonUp ?? "unknown"} role="status" title={tip(service)}>
        <span className="service-dot" aria-hidden />
        <span className="nav-text">{service}</span>
      </p>
    </aside>
  );
}

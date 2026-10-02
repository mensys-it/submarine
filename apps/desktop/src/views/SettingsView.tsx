// Settings view: preferences of the desktop app (language, notifications, start and quit
// behavior), stored by src-tauri, plus the network and Wi-Fi settings of the service.
import { type FormEvent, useState } from "react";

import type { Prefs } from "../api";
import { Icon } from "../components/Icon";
import { Switch, SwitchText } from "../components/Switch";
import { useApp, useT } from "../store";

/** Language choices, in the order of the buttons. */
const languages: Prefs["language"][] = ["system", "it", "en"];
// the service cannot prefer the tunnel over the LAN on macOS yet
const mac = navigator.userAgent.includes("Mac");

/** Longest SSID, in bytes, as the service accepts it (IEEE 802.11). */
const MAX_SSID_BYTES = 32;

/** Preferences of the desktop app, plus the network settings of the service. */
export function SettingsView() {
  const { prefs, savePrefs, settings, saveSettings, daemonUp } = useApp();
  const t = useT();
  const s = t.settings;
  // NB: until the preferences are loaded there is nothing to patch, so changes are ignored
  const save = (patch: Partial<Prefs>) => prefs && void savePrefs({ ...prefs, ...patch });

  return (
    <div className="view page stagger">
      <header className="page-header">
        <h1>{s.title}</h1>
        <p className="lead">{s.lead}</p>
      </header>

      {!prefs && <p className="hint">{s.unavailable}</p>}

      <section className="page-section" aria-labelledby="language-title">
        <h2 id="language-title">{s.language}</h2>
        <div className="segmented" role="group" aria-labelledby="language-title">
          {languages.map((lang) => (
            <button
              key={lang}
              type="button"
              aria-pressed={prefs?.language === lang}
              disabled={!prefs}
              onClick={() => save({ language: lang })}
            >
              {s.languages[lang]}
            </button>
          ))}
        </div>
      </section>

      <section className="page-section" aria-labelledby="general-title">
        <h2 id="general-title">{s.general}</h2>
        <div className="group-card">
          <Switch
            disabled={!prefs}
            pressed={prefs?.notifications ?? false}
            onChange={(notifications) => save({ notifications })}
          >
            <SwitchText title={s.notifications} help={s.notificationsHelp} />
          </Switch>
          <hr />
          <Switch
            disabled={!prefs}
            pressed={prefs?.launch_at_login ?? false}
            onChange={(launch_at_login) => save({ launch_at_login })}
          >
            <SwitchText title={s.launch} help={s.launchHelp} />
          </Switch>
        </div>
      </section>

      <section className="page-section" aria-labelledby="closing-title">
        <h2 id="closing-title">{s.closing}</h2>
        <div className="group-card">
          <Switch
            disabled={!prefs}
            pressed={prefs?.close_to_tray ?? true}
            onChange={(close_to_tray) => save({ close_to_tray })}
          >
            <SwitchText title={s.closeToTray} help={s.closeToTrayHelp} />
          </Switch>
          <hr />
          <Switch
            disabled={!prefs}
            pressed={prefs?.disconnect_on_quit ?? false}
            onChange={(disconnect_on_quit) => save({ disconnect_on_quit })}
          >
            <SwitchText title={s.disconnectOnQuit} help={s.disconnectOnQuitHelp} />
          </Switch>
        </div>
      </section>

      <section className="page-section" aria-labelledby="network-title">
        <h2 id="network-title">{s.network}</h2>
        {mac && <p className="hint">{s.preferTunnelMac}</p>}
        <div className="group-card">
          <Switch
            disabled={!daemonUp || mac}
            pressed={settings.prefer_tunnel}
            onChange={(prefer_tunnel) => void saveSettings({ ...settings, prefer_tunnel })}
          >
            <SwitchText title={s.preferTunnel} help={s.preferTunnelHelp} />
          </Switch>
        </div>
        <p className="hint">{s.preferTunnelFullHelp}</p>
      </section>

      <WifiSection />
    </div>
  );
}

/**
 * Trusted Wi-Fi networks: the tunnel to connect on any other network, whether a trusted
 * one disconnects, and the list itself, with a shortcut for the network in use.
 */
function WifiSection() {
  const { settings, saveSettings, daemonUp, tunnels, status } = useApp();
  const t = useT();
  const w = t.wifi;
  const [draft, setDraft] = useState("");
  const [error, setError] = useState<string | null>(null);
  const trusted = settings.trusted_networks;
  const current = status.wifi;
  const currentTrusted = current != null && trusted.includes(current);
  const setTrusted = (list: string[]) => void saveSettings({ ...settings, trusted_networks: list });

  /** Adds the typed network, unless empty, too long or already listed. */
  function onAdd(e: FormEvent) {
    e.preventDefault();
    const ssid = draft.trim();
    if (!ssid) return;
    if (new TextEncoder().encode(ssid).length > MAX_SSID_BYTES) {
      setError(w.tooLong);
      return;
    }
    if (!trusted.includes(ssid)) setTrusted([...trusted, ssid]);
    setDraft("");
    setError(null);
  }

  return (
    <section className="page-section" aria-labelledby="wifi-title">
      <h2 id="wifi-title">{w.title}</h2>

      {/* the network in use, with the shortcut to trust it or not */}
      <div className="wifi-current card">
        <Icon name="wifi" strokeWidth={2} className={currentTrusted ? "tone-rx" : "tone-time"} />
        <p>
          {current == null ? w.none : `${w.current(current)} ${currentTrusted ? w.trusted : w.untrusted}`}
        </p>
        {current != null && (
          <button
            type="button"
            className="btn btn-secondary btn-sm"
            disabled={!daemonUp}
            onClick={() => setTrusted(currentTrusted ? trusted.filter((x) => x !== current) : [...trusted, current])}
          >
            {currentTrusted ? w.untrust : w.trust}
          </button>
        )}
      </div>

      <div className="group-card">
        <Switch
          disabled={!daemonUp || tunnels.length === 0}
          pressed={settings.untrusted_tunnel != null}
          onChange={(on) => void saveSettings({ ...settings, untrusted_tunnel: on ? (tunnels[0]?.id ?? null) : null })}
        >
          <SwitchText title={w.connect} help={w.connectHelp} />
        </Switch>
        {settings.untrusted_tunnel != null && (
          <label className="field switch-nested wifi-tunnel view-in">
            <span>{w.tunnel}</span>
            <select
              className="input"
              value={settings.untrusted_tunnel}
              disabled={!daemonUp}
              onChange={(e) => void saveSettings({ ...settings, untrusted_tunnel: e.target.value })}
            >
              {tunnels.map((tunnel) => (
                <option key={tunnel.id} value={tunnel.id}>
                  {tunnel.name}
                </option>
              ))}
            </select>
          </label>
        )}
        <hr />
        <Switch
          disabled={!daemonUp}
          pressed={settings.disconnect_on_trusted}
          onChange={(disconnect_on_trusted) => void saveSettings({ ...settings, disconnect_on_trusted })}
        >
          <SwitchText title={w.disconnect} help={w.disconnectHelp} />
        </Switch>
      </div>

      <div className="apps-box">
        <h3 className="wifi-list-title">{w.list}</h3>
        {trusted.length === 0 ? (
          <p className="apps-empty">{w.empty}</p>
        ) : (
          <ul className="app-list">
            {trusted.map((ssid) => (
              <li key={ssid} className="app-row">
                <span className="app-text">
                  <span className="app-name">{ssid}</span>
                </span>
                <button
                  type="button"
                  className="btn btn-secondary btn-sm"
                  disabled={!daemonUp}
                  onClick={() => setTrusted(trusted.filter((x) => x !== ssid))}
                  aria-label={w.removeNetwork(ssid)}
                >
                  {w.remove}
                </button>
              </li>
            ))}
          </ul>
        )}
        <form className="inline-field" onSubmit={onAdd}>
          <input
            className="input"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            placeholder={w.placeholder}
            aria-label={w.placeholder}
            disabled={!daemonUp}
          />
          <button type="submit" className="btn btn-secondary" disabled={!daemonUp || !draft.trim()}>
            <Icon name="plus" size={16} strokeWidth={2} />
            {w.add}
          </button>
        </form>
        {error && (
          <p className="form-error" role="alert">
            {error}
          </p>
        )}
      </div>
      <p className="hint">{w.rulesHint}</p>
      {mac && <p className="hint">{w.macHint}</p>}
    </section>
  );
}

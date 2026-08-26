// Settings view: preferences of the desktop app (language, notifications, start and quit
// behavior), stored by src-tauri, plus the network settings of the service.
import type { Prefs } from "../api";
import { Switch, SwitchText } from "../components/Switch";
import { useApp, useT } from "../store";

/** Language choices, in the order of the buttons. */
const languages: Prefs["language"][] = ["system", "it", "en"];
// the service cannot prefer the tunnel over the LAN on macOS yet
const mac = navigator.userAgent.includes("Mac");

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
    </div>
  );
}

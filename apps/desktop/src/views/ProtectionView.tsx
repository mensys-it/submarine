// Protection view: kill switch, local network access and per-app split tunneling. Every
// change is saved to the daemon at once, through the store.
import { useState } from "react";

import type { AppRule, KillSwitch, Settings, SplitTunnelMode } from "../api";
import { AppPickerDialog } from "../components/AppPickerDialog";
import { Icon } from "../components/Icon";
import { ProtectionBanner } from "../components/ProtectionBanner";
import { RadioCard } from "../components/RadioCard";
import { Switch, SwitchText } from "../components/Switch";
import { useApp, useT } from "../store";

// per-app split tunneling needs a Network Extension on macOS, not available yet; `platform`
// names the platform where it is missing, null where it works
const platform = navigator.userAgent.includes("Mac") ? "macOS" : null;
const splitSupported = platform === null;
// on Windows the included apps lose the local network, so that mode gets its own help text
const windows = navigator.userAgent.includes("Windows");
const splitModes: SplitTunnelMode[] = ["off", "include", "exclude"];

/** Settings of the kill switch and of split tunneling. */
export function ProtectionView() {
  const { settings, saveSettings } = useApp();
  const [picking, setPicking] = useState(false);
  const t = useT();
  const p = t.protection;

  // saving of a partial change on top of the current settings
  const save = (patch: Partial<Settings>) => void saveSettings({ ...settings, ...patch });
  const killSwitchOn = settings.kill_switch !== "off";
  const setKillSwitch = (kill_switch: KillSwitch) => save({ kill_switch });

  // apps added from the picker, skipping the ones already in the list
  const addApps = (apps: AppRule[]) => {
    const known = new Set(settings.split_apps.map((a) => a.path));
    save({ split_apps: [...settings.split_apps, ...apps.filter((a) => !known.has(a.path))] });
  };
  const removeApp = (path: string) => save({ split_apps: settings.split_apps.filter((a) => a.path !== path) });

  return (
    <div className="view page stagger">
      <header className="page-header">
        <h1>{p.title}</h1>
        <p className="lead">{p.lead}</p>
      </header>

      <ProtectionBanner />

      <section className="page-section" aria-labelledby="ks-title">
        <h2 id="ks-title">{p.killSwitch}</h2>
        <div className="group-card">
          {/* the main switch turns the kill switch on in its basic mode, on connect */}
          <Switch pressed={killSwitchOn} onChange={(on) => setKillSwitch(on ? "on_connect" : "off")}>
            <span className="row-with-badge">
              <span className="card-badge card-badge-lg" data-on={killSwitchOn || undefined}>
                <Icon name="shieldCheck" size={22} />
              </span>
              <span className="row-text">
                <SwitchText title={p.blockOnDrop} help={p.blockOnDropHelp} />
              </span>
            </span>
          </Switch>
          <hr />
          <Switch
            className="switch-nested"
            disabled={!killSwitchOn}
            pressed={settings.kill_switch === "always"}
            onChange={(on) => setKillSwitch(on ? "always" : "on_connect")}
          >
            <SwitchText title={p.always} help={p.alwaysHelp} />
          </Switch>
          <Switch
            className="switch-nested"
            disabled={!killSwitchOn}
            pressed={settings.allow_lan}
            onChange={(allow_lan) => save({ allow_lan })}
          >
            <SwitchText title={p.lan} help={p.lanHelp} />
          </Switch>
        </div>
      </section>

      <section className="page-section" aria-labelledby="split-title">
        <h2 id="split-title">{p.perApp}</h2>
        {!splitSupported && <p className="hint">{p.platformLater(platform ?? "")}</p>}
        <div className="radio-grid" role="radiogroup" aria-labelledby="split-title">
          {splitModes.map((mode) => {
            const option = p.modes[mode];
            const help = mode === "include" && windows ? p.modes.include.helpWindows : option.help;
            return (
              <RadioCard
                key={mode}
                name="split-mode"
                checked={settings.split_mode === mode}
                disabled={!splitSupported}
                title={option.label}
                description={help}
                onSelect={() => save({ split_mode: mode })}
              />
            );
          })}
        </div>

        {splitSupported && settings.split_mode !== "off" && (
          <div className="apps-box view-in">
            {settings.split_apps.length === 0 ? (
              <p className="apps-empty">{p.noApps}</p>
            ) : (
              <ul className="app-list">
                {settings.split_apps.map((app) => (
                  <li key={app.path} className="app-row">
                    <span className="app-text">
                      <span className="app-name">{app.name}</span>
                      <span className="app-path">{app.path}</span>
                    </span>
                    <button
                      type="button"
                      className="link-button"
                      onClick={() => removeApp(app.path)}
                      aria-label={p.removeApp(app.name)}
                    >
                      {p.remove}
                    </button>
                  </li>
                ))}
              </ul>
            )}
            <div className="apps-footer">
              <p className="hint">{p.appsHint}</p>
              <button type="button" className="btn btn-secondary" onClick={() => setPicking(true)}>
                <Icon name="plus" size={16} strokeWidth={2} />
                {p.addApp}
              </button>
            </div>
          </div>
        )}
      </section>

      <AppPickerDialog open={picking} chosen={settings.split_apps} onAdd={addApps} onClose={() => setPicking(false)} />
    </div>
  );
}

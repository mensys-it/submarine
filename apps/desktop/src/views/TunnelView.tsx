// Tunnel view, the main page of the app: the ocean scene with the connection state and the
// connect button, live counters, the tunnel's details and its edit and delete actions.
import { useEffect, useState } from "react";

import type { ConnectionState, Settings, TunnelInfo } from "../api";
import { DeleteModal } from "../components/DeleteModal";
import { Icon } from "../components/Icon";
import { InfoCard } from "../components/InfoCard";
import { type HeroState, OceanHero } from "../components/OceanHero";
import { ProtectionBanner } from "../components/ProtectionBanner";
import { StatusDot } from "../components/Sidebar";
import { StatCard } from "../components/StatCard";
import { TrafficChart } from "../components/TrafficChart";
import { Switch, SwitchText } from "../components/Switch";
import { formatBytes, formatClock, formatDuration, formatRate } from "../format";
import { formatAgo, type Texts } from "../i18n";
import { useApp, useT } from "../store";

/** Scene shown for each connection state. */
const heroStates: Record<ConnectionState, HeroState> = {
  disconnected: "off",
  disconnecting: "off",
  connecting: "connecting",
  reconnecting: "connecting",
  paused: "off",
  connected: "on",
  failed: "error",
};

/** Bare host of an endpoint: "vpn.example.com:51820" or "[2001:db8::1]:51820" to the host. */
const hostOf = (endpoint: string) => endpoint.replace(/:\d+$/, "").replace(/^\[(.*)\]$/, "$1");

/**
 * One-line summary of the protection settings, e.g. "Kill switch on, only 2 apps". The apps are
 * left out when split tunneling is `unavailable` on this computer, since the daemon ignores them.
 */
function protectionSummary(t: Texts, settings: Settings, unavailable: boolean): string {
  const killSwitch = t.summary.killSwitch[settings.kill_switch];
  const count = settings.split_apps.length;
  if (unavailable || settings.split_mode === "off" || count === 0) return killSwitch;
  return `${killSwitch}, ${settings.split_mode === "include" ? t.summary.only : t.summary.except} ${t.summary.apps(count)}`;
}

/** Re-renders every second so relative times stay current. */
function useNow() {
  const [now, setNow] = useState(() => Date.now() / 1000);
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now() / 1000), 1000);
    return () => clearInterval(id);
  }, []);
  return now;
}

/** Page of one tunnel; it shows the live state only if the daemon's status is about it. */
export function TunnelView({ tunnel }: { tunnel: TunnelInfo }) {
  const {
    status,
    settings,
    connect,
    disconnect,
    pause,
    deleteTunnel,
    importWarnings,
    warningsFrom,
    dismissWarnings,
    show,
    saveSettings,
    openEdit,
  } = useApp();
  const t = useT();
  const [confirmDelete, setConfirmDelete] = useState(false);
  // a pending delete confirmation does not carry over to another tunnel
  useEffect(() => setConfirmDelete(false), [tunnel.id]);

  // state of this tunnel, and the endpoint actually in use while connected
  const isThis = status.tunnel_id === tunnel.id;
  const state: ConnectionState = isThis ? status.state : "disconnected";
  const hero = heroStates[state];
  const deletable = state === "disconnected" || state === "failed";
  const otherActive = !isThis && status.state === "connected";
  const peer = isThis ? status.peers[0] : undefined;
  const endpoint = peer?.endpoint ?? tunnel.endpoints[0] ?? null;
  const host = endpoint ? hostOf(endpoint) : t.common.none;
  const now = useNow();
  const retryIn = status.retry_at == null ? null : Math.max(0, Math.ceil(status.retry_at - now));

  // second line of the hero, depending on the scene
  const pausedUntil = state === "paused" ? status.paused_until : null;
  const subline = {
    off:
      pausedUntil != null
        ? t.hero.paused(formatClock(pausedUntil * 1000, t.locale), formatDuration(pausedUntil - now))
        : state === "disconnecting"
          ? t.hero.disconnecting
          : t.hero.off,
    connecting: state === "reconnecting" ? t.hero.reconnecting(retryIn) : t.hero.connecting(host),
    on: tunnel.full_tunnel ? t.hero.onFull : t.hero.onSplit,
    error: t.hero.error,
  }[hero];

  return (
    <div className="view tunnel-view">
      <OceanHero state={hero}>
        <p className="hero-status" data-state={state} role="status" aria-live="polite">
          <StatusDot state={state} />
          {t.state[state]}
        </p>
        <h1 className="hero-title">{tunnel.name}</h1>
        <p className="hero-subline">{subline}</p>
        {state === "failed" && status.error && <ConnectError t={t} error={status.error} host={host} />}
        {state === "reconnecting" && status.error && <p className="hero-note">{status.error}</p>}
        <div className="hero-actions">
          <MainButton
            t={t}
            state={state}
            onConnect={() => connect(tunnel.id)}
            onDisconnect={disconnect}
          />
          {state === "connected" && <PauseControl t={t} onPause={pause} />}
          {state === "paused" && (
            <button type="button" className="hero-pause btn" onClick={disconnect}>
              {t.hero.disconnect}
            </button>
          )}
        </div>
        {otherActive && <p className="hero-note">{t.hero.otherActive}</p>}
      </OceanHero>

      <ProtectionBanner />

      {importWarnings.length > 0 && (
        <div className="notice" role="status">
          <div>
            <p>{warningsFrom === "edit" ? t.tunnel.edited : t.tunnel.imported}</p>
            <ul>
              {importWarnings.map((w) => (
                <li key={w}>{w}</li>
              ))}
            </ul>
          </div>
          <button type="button" className="btn btn-secondary btn-sm" onClick={dismissWarnings}>
            {t.common.close}
          </button>
        </div>
      )}

      {state === "connected" && <LiveStats t={t} />}

      <div className="card-grid stagger">
        <InfoCard icon="route" label={t.tunnel.routing}>
          {tunnel.full_tunnel ? t.tunnel.allTraffic : t.tunnel.tunnelNetworks}
        </InfoCard>
        <InfoCard icon="server" label={t.tunnel.server} mono>
          {endpoint ?? t.common.none}
        </InfoCard>
        <InfoCard icon="pin" label={t.tunnel.address} mono>
          {tunnel.addresses.join(", ") || t.common.none}
        </InfoCard>
        <InfoCard icon="globe" label={t.tunnel.dns} mono>
          {tunnel.dns.join(", ") || t.tunnel.systemDns}
        </InfoCard>
        <button type="button" className="info-card card card-button btn" onClick={() => show("protection")}>
          <span className="card-badge" data-on={settings.kill_switch !== "off" || undefined}>
            <Icon name="shield" />
          </span>
          <span className="info-body">
            <span className="info-label">{t.tunnel.protection}</span>
            <span className="info-value">{protectionSummary(t, settings, status.split_unavailable)}</span>
          </span>
          <Icon name="chevron" size={18} strokeWidth={2} className="card-chevron" />
        </button>
        <Switch
          className="card switch-card"
          pressed={settings.auto_connect === tunnel.id}
          onChange={(on) => void saveSettings({ ...settings, auto_connect: on ? tunnel.id : null })}
        >
          <SwitchText title={t.tunnel.autoConnect} help={t.tunnel.autoConnectHelp} />
        </Switch>
      </div>

      <footer className="view-footer">
        <button type="button" className="btn btn-secondary btn-sm" onClick={() => openEdit(tunnel.id)}>
          <Icon name="edit" size={16} strokeWidth={2} />
          {t.tunnel.edit}
        </button>
        {/* a tunnel in use cannot be deleted: the tooltip says why */}
        <button
          type="button"
          className="btn btn-danger btn-sm footer-end"
          onClick={() => setConfirmDelete(true)}
          disabled={!deletable}
          title={deletable ? undefined : t.tunnel.deleteInUse}
        >
          <Icon name="trash" size={16} strokeWidth={2} />
          {t.tunnel.delete}
        </button>
      </footer>

      <DeleteModal
        tunnel={confirmDelete ? tunnel : null}
        onCancel={() => setConfirmDelete(false)}
        onConfirm={() => {
          setConfirmDelete(false);
          void deleteTunnel(tunnel.id);
        }}
      />
    </div>
  );
}

/** Durations offered for a pause, in minutes. */
const PAUSE_MINUTES = [5, 15, 60];

/**
 * The pause button next to the main one: a click shows the durations, a second click on
 * the button (or a choice) hides them again.
 */
function PauseControl({ t, onPause }: { t: Texts; onPause(seconds: number): void }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button
        type="button"
        className="hero-pause btn"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
        title={t.hero.pauseFor}
      >
        <Icon name="pause" strokeWidth={2.2} />
        {t.hero.pause}
      </button>
      {open && (
        <div className="pause-options view-in" role="group" aria-label={t.hero.pauseFor}>
          {PAUSE_MINUTES.map((minutes) => (
            <button
              key={minutes}
              type="button"
              className="pause-option btn"
              onClick={() => {
                setOpen(false);
                onPause(minutes * 60);
              }}
            >
              {t.hero.pauseOption(minutes)}
            </button>
          ))}
        </div>
      )}
    </>
  );
}

/** Props of MainButton. */
interface MainButtonProps {
  t: Texts;
  state: ConnectionState;
  onConnect(): void;
  onDisconnect(): void;
}

/**
 * The big connect button. While connecting it cancels, so a stuck handshake can always be
 * stopped; while reconnecting it disconnects, which also stops the retries.
 */
function MainButton({ t, state, onConnect, onDisconnect }: MainButtonProps) {
  const connecting = state === "connecting" || state === "reconnecting";
  const label = {
    disconnected: t.hero.connect,
    connecting: t.hero.cancel,
    connected: t.hero.disconnect,
    disconnecting: t.hero.disconnectingButton,
    failed: t.hero.retry,
    reconnecting: t.hero.disconnect,
    paused: t.hero.resume,
  }[state];
  const stop = state === "connected" || connecting;
  return (
    <button
      type="button"
      className="hero-button btn"
      data-state={state}
      onClick={stop ? onDisconnect : onConnect}
      disabled={state === "disconnecting"}
    >
      <Icon name={connecting ? "spinner" : "power"} strokeWidth={2.2} className={connecting ? "spin" : undefined} />
      {label}
      {connecting && <span className="shimmer" aria-hidden />}
    </button>
  );
}

/**
 * Explanation of a failed connection, with the raw error. An endpoint that does not resolve
 * gets its own text, naming the host to check.
 */
function ConnectError({ t, error, host }: { t: Texts; error: string; host: string }) {
  // NB: matched on the daemon's error text, so it MUST follow any change of that message
  const dns = error.includes("failed to resolve endpoint");
  return (
    <div className="hero-error view-in" role="alert">
      <Icon name="warning" strokeWidth={2} />
      <div>
        <p className="hero-error-title">{dns ? t.hero.dnsTitle : t.hero.errorTitle}</p>
        <p className="hero-error-body">{dns ? t.hero.dnsBody(host) : t.hero.errorBody}</p>
        <code>{error}</code>
      </div>
    </div>
  );
}

/**
 * Live rates, totals and session time from the daemon's status events (one per second
 * while the counters change), with the throughput chart of the last minute.
 */
function LiveStats({ t }: { t: Texts }) {
  const peers = useApp((s) => s.status.peers);
  const since = useApp((s) => s.status.connected_since);
  const traffic = useApp((s) => s.traffic);
  const now = useNow();

  // totals of the session and the latest rates; a sample older than 2 s means no traffic
  const rx = peers.reduce((sum, p) => sum + p.rx_bytes, 0);
  const tx = peers.reduce((sum, p) => sum + p.tx_bytes, 0);
  const latest = traffic[traffic.length - 1];
  const fresh = latest && latest.time >= now * 1000 - 2000 ? latest : null;
  const rxRate = formatRate(fresh?.rx ?? 0, t.locale);
  const txRate = formatRate(fresh?.tx ?? 0, t.locale);
  const handshakes = peers.map((p) => p.last_handshake).filter((h): h is number => h != null);
  const last = handshakes.length > 0 ? Math.max(...handshakes) : null;

  return (
    <>
      <div className="stat-grid stagger">
        <StatCard
          icon="down"
          tone="rx"
          label={t.stats.received}
          value={rxRate}
          detail={t.stats.total(formatBytes(rx, t.locale))}
        />
        <StatCard
          icon="up"
          tone="tx"
          label={t.stats.sent}
          value={txRate}
          detail={t.stats.total(formatBytes(tx, t.locale))}
        />
        <StatCard
          icon="clock"
          tone="time"
          label={t.stats.session}
          value={since == null ? "–" : formatDuration(now - since)}
          detail={last == null ? t.stats.noHandshake : t.stats.handshake(formatAgo(t, last, now))}
        />
      </div>
      <div className="traffic-card card">
        <div className="traffic-head">
          <span className="stat-label">{t.stats.chart}</span>
          <span className="traffic-legend" aria-hidden>
            <span className="tone-rx">↓ {rxRate}</span>
            <span className="tone-tx">↑ {txRate}</span>
          </span>
        </div>
        <TrafficChart samples={traffic} now={now * 1000} label={t.stats.chartLabel(rxRate, txRate)} />
      </div>
    </>
  );
}

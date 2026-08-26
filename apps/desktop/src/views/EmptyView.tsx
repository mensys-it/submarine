// Main view when there is no tunnel to show, with the way forward: start the service or
// import a first tunnel.
import { Icon } from "../components/Icon";
import { OceanHero } from "../components/OceanHero";
import { useApp, useT } from "../store";

/** No tunnel to show: the service is down, or nothing was imported yet. */
export function EmptyView() {
  const { daemonUp, openImport } = useApp();
  const t = useT();
  // nothing until the first check of the service, so the wrong message never flashes
  if (daemonUp === null) return null;

  return (
    <div className="view tunnel-view">
      <OceanHero state={daemonUp ? "off" : "error"}>
        <h1 className="hero-title">{daemonUp ? t.empty.noTunnelsTitle : t.empty.serviceTitle}</h1>
        <p className="hero-subline">{daemonUp ? t.empty.noTunnelsText : t.empty.serviceText}</p>
        {daemonUp && (
          <div className="hero-actions">
            <button type="button" className="hero-button btn" data-state="disconnected" onClick={openImport}>
              <Icon name="plus" strokeWidth={2.2} />
              {t.nav.import}
            </button>
          </div>
        )}
      </OceanHero>
    </div>
  );
}

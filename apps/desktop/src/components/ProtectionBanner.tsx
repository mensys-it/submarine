// Banner about the kill switch and split tunneling, shown at the top of the tunnel and
// Protection views when they affect the traffic.
import { useApp, useT } from "../store";
import { Icon } from "./Icon";

/**
 * Explains why traffic is blocked, or that protection could not be applied. Renders nothing
 * while connecting, when the block is expected and short-lived.
 */
export function ProtectionBanner() {
  const { status, settings, disconnect, show, view } = useApp();
  const t = useT();

  // a failure to apply the protection outranks everything else
  if (status.protection_error) {
    return (
      <div className="banner" data-kind="error" role="alert">
        <Icon name="warning" />
        <p>{t.banner.protectionOff(status.protection_error)}</p>
      </div>
    );
  }
  if (!status.blocked || status.state === "connecting") return null;

  // what is blocked, and whether the block follows a dropped connection or the settings
  const onlyApps = settings.split_mode === "include" && settings.split_apps.length > 0;
  const what = onlyApps ? t.banner.blockedApps : t.banner.blocked;
  // a tunnel that dropped: failed for good, or being reconnected
  const dropped = status.state === "failed" || status.state === "reconnecting";

  return (
    <div className="banner" data-kind="blocked" role="status">
      <Icon name="shield" />
      <p>
        {status.state === "reconnecting"
          ? t.banner.reconnecting(what)
          : dropped
            ? t.banner.dropped(what)
            : t.banner.always(what)}
      </p>
      {/* after a drop the way out is disconnecting, otherwise changing the settings */}
      {dropped ? (
        <button type="button" className="btn btn-secondary" onClick={disconnect}>
          {t.banner.unblock}
        </button>
      ) : (
        view !== "protection" && (
          <button type="button" className="btn btn-secondary" onClick={() => show("protection")}>
            {t.banner.edit}
          </button>
        )
      )}
    </div>
  );
}

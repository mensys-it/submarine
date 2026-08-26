// Card with one fact about a tunnel (address, endpoint, DNS, ...), used by the tunnel view.
import type { ReactNode } from "react";

import { Icon, type IconName } from "./Icon";

/** Props of InfoCard. */
interface Props {
  icon: IconName;
  /** Caption above the value. */
  label: string;
  /** Monospace value, for addresses and host names. */
  mono?: boolean;
  /** The value. */
  children: ReactNode;
}

/** One fact about a tunnel. */
export function InfoCard({ icon, label, mono, children }: Props) {
  return (
    <div className="info-card card">
      <span className="card-badge">
        <Icon name={icon} />
      </span>
      <div className="info-body">
        <span className="info-label">{label}</span>
        <span className={mono ? "info-value mono" : "info-value"}>{children}</span>
      </div>
    </div>
  );
}

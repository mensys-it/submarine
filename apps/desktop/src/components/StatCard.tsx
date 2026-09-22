// Card with one live counter (download, upload, session), used by the tunnel view.
import { Icon, type IconName } from "./Icon";

/** Props of StatCard. */
interface Props {
  icon: IconName;
  /** Accent of the icon: "rx", "tx" or "time". */
  tone: "rx" | "tx" | "time";
  /** Caption next to the icon. */
  label: string;
  /** Formatted value. */
  value: string;
  /** Secondary line under the value, e.g. the total behind a rate. */
  detail?: string;
}

/** A live counter shown while connected. */
export function StatCard({ icon, tone, label, value, detail }: Props) {
  return (
    <div className="stat-card card">
      <span className="stat-label">
        <Icon name={icon} size={16} strokeWidth={2} className={`tone-${tone}`} />
        {label}
      </span>
      <span className="stat-value num">{value}</span>
      {detail && <span className="stat-detail num">{detail}</span>}
    </div>
  );
}

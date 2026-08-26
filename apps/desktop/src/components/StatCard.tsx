// Card with one live counter (received, sent, last handshake), used by the tunnel view.
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
}

/** A live counter shown while connected. */
export function StatCard({ icon, tone, label, value }: Props) {
  return (
    <div className="stat-card card">
      <span className="stat-label">
        <Icon name={icon} size={16} strokeWidth={2} className={`tone-${tone}`} />
        {label}
      </span>
      <span className="stat-value num">{value}</span>
    </div>
  );
}

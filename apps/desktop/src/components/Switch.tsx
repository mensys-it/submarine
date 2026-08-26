// Toggle switch and its usual label content, used by the settings and protection views.
import type { ReactNode } from "react";

/** Props of Switch. */
interface Props {
  /** Whether the switch is on. */
  pressed: boolean;
  /** Called with the new value on every click. */
  onChange(pressed: boolean): void;
  disabled?: boolean;
  /** "sm" is the compact pill used in toolbars. */
  size?: "md" | "sm";
  className?: string;
  /** Label content; the whole button toggles, the track sits at the end. */
  children: ReactNode;
}

/** A toggle: a button with aria-pressed whose label is its own content. */
export function Switch({ pressed, onChange, disabled, size = "md", className, children }: Props) {
  return (
    <button
      type="button"
      className={`switch switch-${size}${className ? ` ${className}` : ""}`}
      aria-pressed={pressed}
      disabled={disabled}
      onClick={() => onChange(!pressed)}
    >
      {size === "sm" && <SwitchTrack />}
      <span className="switch-label">{children}</span>
      {size === "md" && <SwitchTrack />}
    </button>
  );
}

/** Track and knob of the switch, decorative. */
function SwitchTrack() {
  return (
    <span className="switch-track" aria-hidden>
      <span className="switch-knob" />
    </span>
  );
}

/** Title and description, the usual content of a settings switch. */
export function SwitchText({ title, help }: { title: string; help?: string }) {
  return (
    <>
      <span className="row-title">{title}</span>
      {help && <span className="row-help">{help}</span>}
    </>
  );
}

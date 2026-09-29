// Line icons from the design, drawn on a 24×24 grid with currentColor, so they take the
// color of the surrounding text. Used by every component and view.

/** SVG content of each icon, without the outer svg element. */

const paths = {
  plus: <path d="M12 5v14M5 12h14" />,
  panelClose: (
    <>
      <rect x="3" y="4" width="18" height="16" rx="3" />
      <path d="M9 4v16M16 10l-2 2 2 2" strokeLinejoin="round" />
    </>
  ),
  panelOpen: (
    <>
      <rect x="3" y="4" width="18" height="16" rx="3" />
      <path d="M9 4v16M14 10l2 2-2 2" strokeLinejoin="round" />
    </>
  ),
  shield: <path d="M12 3l7 3v5c0 4.5-3 8.5-7 10-4-1.5-7-5.5-7-10V6l7-3z" strokeLinejoin="round" />,
  shieldCheck: (
    <>
      <path d="M12 3l7 3v5c0 4.5-3 8.5-7 10-4-1.5-7-5.5-7-10V6l7-3z" strokeLinejoin="round" />
      <path d="M9 12l2 2 4-4" />
    </>
  ),
  sliders: (
    <>
      <path d="M4 7h10M18 7h2M4 17h4M12 17h8" />
      <circle cx="16" cy="7" r="2" />
      <circle cx="10" cy="17" r="2" />
    </>
  ),
  lines: <path d="M5 6h14M5 12h14M5 18h9" />,
  power: (
    <>
      <path d="M12 3v8" />
      <path d="M6.3 6.8a8 8 0 1 0 11.4 0" />
    </>
  ),
  spinner: <path d="M21 12a9 9 0 1 1-9-9" />,
  pause: <path d="M9 5v14M15 5v14" />,
  down: <path d="M12 4v16M6 14l6 6 6-6" />,
  up: <path d="M12 20V4M6 10l6-6 6 6" />,
  clock: (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 7v5l3 2" />
    </>
  ),
  route: (
    <>
      <circle cx="6" cy="18" r="2" />
      <circle cx="18" cy="6" r="2" />
      <path d="M8 18h7a3.5 3.5 0 0 0 0-7H9a3.5 3.5 0 0 1 0-7h7" />
    </>
  ),
  server: (
    <>
      <rect x="4" y="4" width="16" height="7" rx="2" />
      <rect x="4" y="13" width="16" height="7" rx="2" />
      <path d="M8 7.5h.01M8 16.5h.01" />
    </>
  ),
  pin: (
    <>
      <path d="M12 21s-7-6.2-7-11a7 7 0 0 1 14 0c0 4.8-7 11-7 11z" />
      <circle cx="12" cy="10" r="2.5" />
    </>
  ),
  globe: (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M3 12h18M12 3c2.5 2.5 3.8 5.5 3.8 9s-1.3 6.5-3.8 9c-2.5-2.5-3.8-5.5-3.8-9S9.5 5.5 12 3z" />
    </>
  ),
  chevron: <path d="M9 6l6 6-6 6" />,
  warning: (
    <>
      <path d="M12 9v4M12 17h.01" />
      <path d="M10.3 3.9L2.4 18a2 2 0 0 0 1.7 3h15.8a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z" />
    </>
  ),
  close: <path d="M6 6l12 12M18 6L6 18" />,
  upload: (
    <>
      <path d="M12 16V4M7 9l5-5 5 5" strokeLinejoin="round" />
      <path d="M5 20h14" />
    </>
  ),
  trash: (
    <>
      <path d="M4 7h16M10 11v6M14 11v6" />
      <path d="M6 7l1 12a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-12M9 7V4h6v3" strokeLinejoin="round" />
    </>
  ),
  edit: (
    <>
      <path d="M4 20h4L19 9a2.8 2.8 0 0 0-4-4L4 16v4z" strokeLinejoin="round" />
      <path d="M13.5 6.5l4 4" />
    </>
  ),
  info: (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 11v5M12 8h.01" />
    </>
  ),
  heart: (
    <path
      d="M12 20.5s-8-4.9-8-11A4.5 4.5 0 0 1 12 6.6a4.5 4.5 0 0 1 8 2.9c0 6.1-8 11-8 11z"
      fill="currentColor"
    />
  ),
  copy: (
    <>
      <rect x="9" y="9" width="11" height="11" rx="2" />
      <path d="M5 15V5a2 2 0 0 1 2-2h8" />
    </>
  ),
};

/** Name of an available icon. */
export type IconName = keyof typeof paths;

/** Props of Icon. */
interface Props {
  name: IconName;
  /** Width and height in pixels; 20 by default. */
  size?: number;
  /** Line width in grid units; 1.8 by default. */
  strokeWidth?: number;
  className?: string;
}

/** An icon, hidden from screen readers: the control around it carries the label. */
export function Icon({ name, size = 20, strokeWidth = 1.8, className }: Props) {
  return (
    <svg
      className={className}
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={strokeWidth}
      strokeLinecap="round"
      aria-hidden
    >
      {paths[name]}
    </svg>
  );
}

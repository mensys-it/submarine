// Animated ocean scene at the top of the tunnel view, showing the connection state with a
// submarine. The animations themselves live in the CSS, keyed on `data-state`.
import type { ReactNode } from "react";

/** What the scene shows; set from the tunnel's connection state. */
export type HeroState = "off" | "connecting" | "on" | "error";

/** Stars of the night sky, as [left %, top %, twinkle delay in s]. */
const stars = [
  [8, 12, 0],
  [22, 30, 0.8],
  [41, 9, 1.6],
  [57, 22, 0.4],
  [68, 8, 2.1],
  [79, 28, 1.1],
  [90, 14, 2.6],
  [95, 40, 0.2],
];

// wave crests repeat every 400 units, so sliding by half the 1600-unit path loops seamlessly
const crest = "T400 30 T600 30 T800 30 T1000 30 T1200 30 T1400 30 T1600 30 V140 H0 Z";

/**
 * One layer of waves: the path `d` draws the first crest, the shared tail repeats it and
 * closes the shape down to the bottom.
 */
function Wave({ className, d, fill }: { className: string; d: string; fill: string }) {
  return (
    <div className={`wave ${className}`}>
      <svg viewBox="0 0 1600 140" preserveAspectRatio="none" width="100%" height="100%">
        <path d={`${d} ${crest}`} fill={fill} />
      </svg>
    </div>
  );
}

/**
 * The ocean scene behind a tunnel's name, given as `children`: the submarine floats on the
 * surface when off, dives with sonar pings while connecting, cruises underwater when
 * connected and lists on the surface after an error.
 */
export function OceanHero({ state, children }: { state: HeroState; children: ReactNode }) {
  const on = state === "on";
  return (
    <section className="hero" data-state={state}>
      <div className="hero-scene" aria-hidden>
        {stars.map(([left, top, delay]) => (
          <span key={left} className="star" style={{ left: `${left}%`, top: `${top}%`, animationDelay: `${delay}s` }} />
        ))}
        <div className="moon" />

        <div className="water water-back">
          <Wave className="w1" d="M0 30 Q100 6 200 30" fill="var(--sea-wave-1)" />
          <Wave className="w2" d="M0 30 Q100 50 200 30" fill="var(--sea-wave-2)" />
          <div className="water-body" />
          {on && (
            <>
              <div className="ray" style={{ left: "46%" }} />
              <div className="ray" style={{ left: "63%", animationDelay: "2s" }} />
              <div className="ray" style={{ left: "80%", animationDelay: "3.6s" }} />
              <span className="mote" style={{ left: "52%", top: 200 }} />
              <span className="mote" style={{ left: "71%", top: 240, animationDelay: "2.4s" }} />
              <span className="mote" style={{ left: "88%", top: 180, animationDelay: "4.1s" }} />
              <span className="mote" style={{ left: "60%", top: 280, animationDelay: "5.2s" }} />
            </>
          )}
        </div>

        <div className="sub-depth">
          <div className="sub-cruise">
            <div className="sub-bob">
              {state === "connecting" && (
                <>
                  <span className="sonar" />
                  <span className="sonar" style={{ animationDelay: ".7s" }} />
                  <span className="sonar" style={{ animationDelay: "1.4s" }} />
                </>
              )}
              {on && (
                <>
                  <span className="bubble" style={{ left: 6, top: 60, width: 10, height: 10 }} />
                  <span className="bubble" style={{ left: 14, top: 70, width: 7, height: 7, animationDelay: ".6s" }} />
                  <span className="bubble" style={{ left: 2, top: 66, width: 13, height: 13, animationDelay: "1.3s" }} />
                  <span className="bubble" style={{ left: 20, top: 58, width: 6, height: 6, animationDelay: "2s" }} />
                  <span className="bubble" style={{ left: 10, top: 74, width: 9, height: 9, animationDelay: "2.7s" }} />
                  <span className="bubble" style={{ left: 120, top: 22, width: 6, height: 6, animationDelay: "1.7s" }} />
                </>
              )}
              <Submarine />
            </div>
          </div>
        </div>

        {/* drawn over the submarine, tinting whatever is underwater */}
        <div className="water water-front">
          <Wave className="w3" d="M0 30 Q100 12 200 30" fill="var(--sea-wave-3)" />
          <div className="water-tint" />
          <svg className="waterline" viewBox="0 0 1000 12" preserveAspectRatio="none">
            <path d="M0 8 Q 125 2 250 8 T 500 8 T 750 8 T 1000 8" />
          </svg>
        </div>
      </div>

      <div className="hero-copy">{children}</div>
    </section>
  );
}

/** The submarine drawing; its propeller, beacon, portholes and beam are animated by CSS. */
function Submarine() {
  return (
    <svg className="submarine" width="220" height="120" viewBox="0 0 220 120">
      <defs>
        <linearGradient id="sub-beam" x1="0" x2="1" y1="0" y2="0">
          <stop offset="0" stopColor="#FFF3C4" stopOpacity=".8" />
          <stop offset="1" stopColor="#FFF3C4" stopOpacity="0" />
        </linearGradient>
      </defs>
      <ellipse className="propeller" cx="14" cy="70" rx="4" ry="15" fill="#C98B22" />
      <rect x="14" y="67" width="22" height="6" rx="3" fill="#B87D1E" />
      <path d="M44 70 L24 50 L30 70 L24 90 Z" fill="#D9951F" />
      <path
        d="M32 70 C32 48 62 40 112 40 L170 40 C200 40 214 56 214 70 C214 86 200 98 170 98 L62 98 C42 98 32 88 32 70 Z"
        fill="var(--sub-hull)"
      />
      <path
        d="M50 56 C66 46 92 44 112 44 L168 44 C186 44 198 50 205 58"
        stroke="#FFE1A0"
        strokeWidth="3"
        fill="none"
        strokeLinecap="round"
        opacity=".8"
      />
      <path
        d="M40 82 C52 92 70 94 90 94 L172 94 C192 94 204 86 210 78"
        stroke="#D08F22"
        strokeWidth="5"
        fill="none"
        strokeLinecap="round"
        opacity=".7"
      />
      <rect x="100" y="18" width="48" height="26" rx="9" fill="var(--sub-hull)" />
      <rect x="106" y="24" width="14" height="8" rx="3" fill="#0B2238" />
      <path d="M134 18 V5 H148" stroke="#E2A132" strokeWidth="4" fill="none" strokeLinecap="round" strokeLinejoin="round" />
      <circle className="beacon" cx="110" cy="13" r="4.5" />
      {[84, 118, 152].map((cx) => (
        <g key={cx}>
          <circle cx={cx} cy="70" r="10" fill="#E2A132" />
          <circle className="porthole" cx={cx} cy="70" r="7" />
          <circle cx={cx - 3} cy="67" r="2" fill="#FFFFFF" opacity=".55" />
        </g>
      ))}
      <path className="beam" d="M214 66 L320 40 L320 100 L214 76 Z" fill="url(#sub-beam)" />
    </svg>
  );
}

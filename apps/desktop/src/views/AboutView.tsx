// About view, opened from the sidebar: the app's name, icon, version and credits.
import { Icon } from "../components/Icon";
import { OceanHero } from "../components/OceanHero";
import { useApp, useT } from "../store";

/** The app's name, icon and version. Room for the update check to come. */
export function AboutView() {
  const version = useApp((s) => s.version);
  const t = useT();

  return (
    <div className="view tunnel-view about-view">
      <OceanHero state="on">
        <img className="about-mark" src="/icon.png" alt="" width={88} height={88} />
        <h1 className="hero-title">Submarine</h1>
        <p className="hero-subline">{t.about.description}</p>
        <p className="about-version num">{version ? t.about.version(version) : t.about.versionUnknown}</p>
      </OceanHero>

      <p className="about-credit">
        Brought to you with
        <Icon name="heart" size={18} strokeWidth={0} className="about-heart" />
        <span className="visually-hidden">love</span>
        by <strong>Mensys</strong> from
        <ItalianFlag />
      </p>
    </div>
  );
}

/**
 * Drawn rather than the 🇮🇹 emoji: Windows has no flag emoji and would show the letters
 * "IT".
 */
function ItalianFlag() {
  return (
    <svg className="about-flag" viewBox="0 0 3 2" width={24} height={16} role="img" aria-label="Italy">
      <rect width="1" height="2" fill="#009246" />
      <rect x="1" width="1" height="2" fill="#f1f2f1" />
      <rect x="2" width="1" height="2" fill="#ce2b37" />
    </svg>
  );
}

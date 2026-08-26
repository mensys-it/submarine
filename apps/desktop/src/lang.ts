// Languages of the UI, and the default one outside Tauri. Shared by the texts, the store and
// the mock, without the text catalogue.

/** A language the UI is translated into. */
export type Lang = "it" | "en";

/** Language used when the app runs outside Tauri (browser preview). */
export function browserLang(): Lang {
  return navigator.language.toLowerCase().startsWith("it") ? "it" : "en";
}

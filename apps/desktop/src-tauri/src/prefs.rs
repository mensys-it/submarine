//! Preferences of the desktop app itself (not of the VPN service), stored as
//! `prefs.json` in the user's config directory.
//!
//! Launch at login is NOT stored there: the OS keeps it, through the autostart
//! plugin.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tauri_plugin_autostart::ManagerExt;

use crate::i18n::{Lang, LanguagePref};

/// Preferences stored in the file; missing keys take their default value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// System notifications on connection changes.
    pub notifications: bool,
    /// Language of the app.
    pub language: LanguagePref,
    /// The window's close button keeps the app in the tray; off, it quits.
    pub close_to_tray: bool,
    /// Quitting the app also disconnects the VPN, which the daemon would
    /// otherwise keep up.
    pub disconnect_on_quit: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            notifications: true,
            language: LanguagePref::System,
            close_to_tray: true,
            disconnect_on_quit: false,
        }
    }
}

/// What the UI edits: stored preferences plus the OS launch setting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrefsView {
    #[serde(flatten)]
    /// Stored preferences, flattened in the same JSON object.
    pub prefs: Prefs,
    /// Whether the app starts at login, as the OS reports it.
    pub launch_at_login: bool,
    /// Language actually in use, "it" or "en"; ignored when the UI sends it.
    #[serde(default)]
    pub resolved_language: String,
}

/// Current preferences, shared as Tauri state.
#[derive(Default)]
pub struct PrefsState(Mutex<Prefs>);

impl PrefsState {
    /// Loads the preferences from the file; a missing or invalid file gives the
    /// defaults.
    pub fn load(app: &AppHandle) -> Self {
        let prefs = path(app)
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default();
        Self(Mutex::new(prefs))
    }

    /// Returns a copy of the current preferences.
    pub fn get(&self) -> Prefs {
        self.0.lock().unwrap().clone()
    }

    /// Returns the language in use.
    pub fn lang(&self) -> Lang {
        self.get().language.resolve()
    }
}

/// Path of the preferences file, if the config directory is known.
fn path(app: &AppHandle) -> Option<PathBuf> {
    Some(app.path().app_config_dir().ok()?.join("prefs.json"))
}

/// Current preferences as the UI sees them.
fn view(app: &AppHandle) -> PrefsView {
    let prefs = app.state::<PrefsState>().get();
    let lang = prefs.language.resolve();
    PrefsView {
        launch_at_login: app.autolaunch().is_enabled().unwrap_or(false),
        resolved_language: match lang {
            Lang::It => "it",
            Lang::En => "en",
        }
        .into(),
        prefs,
    }
}

/// Tauri command returning the preferences.
#[tauri::command]
pub fn get_prefs(app: AppHandle) -> PrefsView {
    view(&app)
}

/// Tauri command saving the preferences and applying them; returns them as
/// now in effect.
///
/// # Errors
///
/// Fails if the launch at login cannot be changed or the file cannot be written.
#[tauri::command]
pub fn set_prefs(app: AppHandle, prefs: PrefsView) -> Result<PrefsView, String> {
    // launch at login, changed in the OS only if it differs
    let autolaunch = app.autolaunch();
    if prefs.launch_at_login != autolaunch.is_enabled().unwrap_or(false) {
        let result = if prefs.launch_at_login {
            autolaunch.enable()
        } else {
            autolaunch.disable()
        };
        result.map_err(|e| e.to_string())?;
    }
    // file first, so the state changes only if it was saved
    if let Some(path) = path(&app) {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let data = serde_json::to_vec_pretty(&prefs.prefs).map_err(|e| e.to_string())?;
        std::fs::write(path, data).map_err(|e| e.to_string())?;
    }
    *app.state::<PrefsState>().0.lock().unwrap() = prefs.prefs;
    // the tray texts may have changed language
    app.state::<crate::live::Live>().refresh_all(&app);
    Ok(view(&app))
}

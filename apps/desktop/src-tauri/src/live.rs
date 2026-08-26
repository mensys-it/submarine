//! Copy of the daemon state in the UI process, kept up to date from its events.
//!
//! It drives the tray menu and the notifications, which live outside the
//! webview and so cannot use the state of the frontend.

use std::sync::Mutex;

use submarine_ipc::{Event, Settings, Status, TunnelInfo};
use tauri::{AppHandle, Manager};

use crate::prefs::PrefsState;
use crate::{i18n, notify, tray};

/// Daemon state as last seen by the app.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// Whether the app is connected to the daemon.
    pub service_up: bool,
    /// Connection status.
    pub status: Status,
    /// Imported tunnels.
    pub tunnels: Vec<TunnelInfo>,
    /// Daemon settings.
    pub settings: Settings,
}

/// Live daemon state, shared as Tauri state.
#[derive(Default)]
pub struct Live {
    /// Current state.
    snapshot: Mutex<Snapshot>,
    /// Key of the tray menu last built, to rebuild it only when its items change.
    menu: Mutex<Option<tray::MenuKey>>,
}

impl Live {
    /// Returns a copy of the current state.
    pub fn get(&self) -> Snapshot {
        self.snapshot.lock().unwrap().clone()
    }

    /// Applies a change and updates everything that depends on it.
    pub fn update(&self, app: &AppHandle, change: impl FnOnce(&mut Snapshot)) {
        // change applied under the lock, keeping both versions for the notifications
        let (before, after) = {
            let mut snapshot = self.snapshot.lock().unwrap();
            let before = snapshot.clone();
            change(&mut snapshot);
            (before, snapshot.clone())
        };
        // the menu is rebuilt only when its items change, otherwise only refreshed
        let key = tray::MenuKey::of(&after);
        let rebuild = {
            let mut last = self.menu.lock().unwrap();
            let changed = last.as_ref() != Some(&key);
            *last = Some(key);
            changed
        };
        // tray and notifications, in the language of the preferences
        let prefs = app.state::<PrefsState>().get();
        let texts = i18n::texts(prefs.language.resolve());
        if let Err(err) = tray::refresh(app, texts, &after, rebuild) {
            tracing::warn!("tray update failed: {err}");
        }
        if prefs.notifications {
            notify::on_change(app, texts, &before, &after);
        }
    }

    /// Redraws the tray, e.g. after a language change.
    pub fn refresh_all(&self, app: &AppHandle) {
        *self.menu.lock().unwrap() = None;
        self.update(app, |_| {});
    }

    /// Applies a daemon event to the state.
    pub fn apply_event(&self, app: &AppHandle, event: &Event) {
        match event {
            Event::StatusChanged(status) => self.update(app, |s| s.status = status.clone()),
            Event::TunnelsChanged(tunnels) => self.update(app, |s| s.tunnels = tunnels.clone()),
            Event::SettingsChanged(settings) => self.update(app, |s| s.settings = settings.clone()),
            // only the window shows log lines: nothing to redraw here
            Event::LogLine(_) => {}
        }
    }
}

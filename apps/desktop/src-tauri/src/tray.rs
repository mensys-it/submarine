//! Tray icon: state-dependent icon and tooltip, and a menu to connect any
//! tunnel, disconnect, open the window or quit.
//!
//! The daemon sends a status event every second (traffic counters), so the
//! menu is rebuilt only when something it shows changes: rebuilding an open
//! menu would make it flicker or close.

use submarine_ipc::{ConnectionState, Request};
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, IsMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

use crate::i18n::Texts;
use crate::live::Snapshot;
use crate::prefs::PrefsState;
use crate::{Daemon, i18n, show_main_window};

/// Identifier of the tray icon.
const TRAY_ID: &str = "main";
/// Icon when not connected.
const ICON_IDLE: &[u8] = include_bytes!("../icons/tray/idle.png");
/// Icon when connected.
const ICON_CONNECTED: &[u8] = include_bytes!("../icons/tray/connected.png");
/// Icon when the kill switch blocks the traffic, whatever the state.
const ICON_BLOCKED: &[u8] = include_bytes!("../icons/tray/blocked.png");

/// What the menu shows; the menu is rebuilt only when this changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuKey {
    service_up: bool,
    state: ConnectionState,
    tunnel_id: Option<String>,
    blocked: bool,
    /// `(id, name)` of each tunnel.
    tunnels: Vec<(String, String)>,
}

impl MenuKey {
    /// Returns the key of the menu for the state `s`.
    pub fn of(s: &Snapshot) -> Self {
        Self {
            service_up: s.service_up,
            state: s.status.state,
            tunnel_id: s.status.tunnel_id.clone(),
            blocked: s.status.blocked,
            tunnels: s
                .tunnels
                .iter()
                .map(|t| (t.id.clone(), t.name.clone()))
                .collect(),
        }
    }
}

/// Creates the tray icon, showing the service as down until the first snapshot.
///
/// # Errors
///
/// Fails if the icon or the menu cannot be built.
pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let snapshot = Snapshot::default();
    let t = i18n::texts(app.state::<PrefsState>().lang());
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon(&snapshot)?)
        .tooltip(tooltip(t, &snapshot))
        .menu(&menu(app, t, &snapshot)?)
        // left click opens the window, right click the menu (where supported)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .on_menu_event(|app, event| on_menu(app, event.id().as_ref()))
        .build(app)?;
    Ok(())
}

/// Updates icon and tooltip, and the menu when `rebuild_menu` is set.
///
/// # Errors
///
/// Fails if the tray cannot be updated.
pub fn refresh(app: &AppHandle, t: &Texts, s: &Snapshot, rebuild_menu: bool) -> tauri::Result<()> {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return Ok(());
    };
    tray.set_icon(Some(icon(s)?))?;
    tray.set_tooltip(Some(tooltip(t, s)))?;
    if rebuild_menu {
        tray.set_menu(Some(menu(app, t, s)?))?;
    }
    Ok(())
}

/// Icon for the state: blocked first, since it matters even when not connected.
fn icon(s: &Snapshot) -> tauri::Result<Image<'static>> {
    let bytes = if s.status.blocked {
        ICON_BLOCKED
    } else if s.service_up && s.status.state == ConnectionState::Connected {
        ICON_CONNECTED
    } else {
        ICON_IDLE
    };
    Image::from_bytes(bytes)
}

/// Name of the current tunnel, if any.
fn tunnel_name(s: &Snapshot) -> Option<&str> {
    let id = s.status.tunnel_id.as_deref()?;
    s.tunnels
        .iter()
        .find(|t| t.id == id)
        .map(|t| t.name.as_str())
}

/// One line describing the situation, for the tooltip and the menu header.
pub fn summary(t: &Texts, s: &Snapshot) -> String {
    if !s.service_up {
        return t.service_down.into();
    }
    let with_name = |state: &str| match tunnel_name(s) {
        Some(name) => format!("{state} {} {name}", t.to),
        None => state.to_owned(),
    };
    let line = match s.status.state {
        ConnectionState::Connected => with_name(t.connected),
        ConnectionState::Connecting => with_name(t.connecting),
        ConnectionState::Disconnecting => t.disconnecting.into(),
        ConnectionState::Failed => with_name(t.failed),
        ConnectionState::Reconnecting => with_name(t.reconnecting),
        ConnectionState::Disconnected => t.disconnected.into(),
    };
    if s.status.blocked {
        format!("{line}, {}", t.internet_blocked)
    } else {
        line
    }
}

/// Tooltip of the icon.
fn tooltip(t: &Texts, s: &Snapshot) -> String {
    format!("Submarine: {}", summary(t, s))
}

/// Builds the menu: the summary, the tunnels to connect, then disconnect, open
/// and quit.
///
/// Item ids are what `on_menu` dispatches on; a tunnel item is
/// `connect:<tunnel id>`.
fn menu(app: &AppHandle, t: &Texts, s: &Snapshot) -> tauri::Result<Menu<Wry>> {
    // disabled header with the summary
    let mut items: Vec<Box<dyn IsMenuItem<Wry>>> = Vec::new();
    items.push(Box::new(MenuItem::with_id(
        app,
        "status",
        summary(t, s),
        false,
        None::<&str>,
    )?));
    items.push(Box::new(PredefinedMenuItem::separator(app)?));

    // one check item per tunnel, checked for the current one
    if s.tunnels.is_empty() {
        items.push(Box::new(MenuItem::with_id(
            app,
            "none",
            t.no_tunnels,
            false,
            None::<&str>,
        )?));
    }
    for t in &s.tunnels {
        let live = s.status.tunnel_id.as_deref() == Some(t.id.as_str())
            && matches!(
                s.status.state,
                ConnectionState::Connected
                    | ConnectionState::Connecting
                    | ConnectionState::Reconnecting
            );
        let id = format!("connect:{}", t.id);
        items.push(Box::new(CheckMenuItem::with_id(
            app,
            id,
            &t.name,
            s.service_up,
            live,
            None::<&str>,
        )?));
    }

    // disconnect, also to lift the block that the kill switch keeps after a drop
    items.push(Box::new(PredefinedMenuItem::separator(app)?));
    let can_disconnect = s.service_up
        && (s.status.state != ConnectionState::Disconnected
            || s.status.blocked && s.status.tunnel_id.is_some());
    items.push(Box::new(MenuItem::with_id(
        app,
        "disconnect",
        t.disconnect,
        can_disconnect,
        None::<&str>,
    )?));
    // window and quit
    items.push(Box::new(PredefinedMenuItem::separator(app)?));
    items.push(Box::new(MenuItem::with_id(
        app,
        "show",
        t.open,
        true,
        None::<&str>,
    )?));
    items.push(Box::new(MenuItem::with_id(
        app,
        "quit",
        t.quit,
        true,
        None::<&str>,
    )?));

    let refs: Vec<&dyn IsMenuItem<Wry>> = items.iter().map(|i| i.as_ref()).collect();
    Menu::with_items(app, &refs)
}

/// Handles a click on the menu item `id`.
///
/// Connect and disconnect go to the daemon in the background; the menu follows
/// from the events that come back.
fn on_menu(app: &AppHandle, id: &str) {
    // request for the item, or a local action
    let request = match id {
        "show" => {
            show_main_window(app);
            return;
        }
        // goes through the exit handler in lib.rs, which may disconnect first
        "quit" => return app.exit(0),
        "disconnect" => Request::Disconnect,
        _ => match id.strip_prefix("connect:") {
            Some(tunnel) => {
                let snapshot = app.state::<crate::live::Live>().get();
                let already = snapshot.status.tunnel_id.as_deref() == Some(tunnel)
                    && snapshot.status.state == ConnectionState::Connected;
                if already {
                    // clicking toggled the check mark: put the menu back as it was
                    app.state::<crate::live::Live>().refresh_all(app);
                    return;
                }
                Request::Connect {
                    id: tunnel.to_owned(),
                }
            }
            None => return,
        },
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(err) = app.state::<Daemon>().request(request).await {
            tracing::warn!("tray action failed: {err}");
            // restore of the check marks that the click toggled
            app.state::<crate::live::Live>().refresh_all(&app);
        }
    });
}

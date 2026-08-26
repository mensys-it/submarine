//! Backend of the Tauri desktop app: forwards the UI requests to
//! `submarine-daemon` over IPC and relays its events to the webview, the tray
//! and the notifications.
//!
//! The app runs unprivileged: every network change happens in the daemon. It
//! also owns the app lifecycle: single instance, start at login (hidden in the
//! tray), close to tray, and the optional disconnect before quitting.

mod apps;
mod config_file;
mod i18n;
mod live;
mod notify;
mod prefs;
mod tray;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use submarine_ipc::{Client, ConnectionState, Request, Response};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WindowEvent};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
use tokio::sync::RwLock;

/// Delay between attempts to reach the daemon, also after losing it.
const RECONNECT_DELAY: Duration = Duration::from_secs(2);
/// How long quitting waits for the daemon to disconnect.
const QUIT_DISCONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Set when quitting starts, so the work before exiting runs only once.
static QUITTING: AtomicBool = AtomicBool::new(false);
/// Set once the work before exiting is done, so the final exit goes through.
static READY_TO_EXIT: AtomicBool = AtomicBool::new(false);
/// Command line flag of the launch at login: the app starts in the tray only.
const HIDDEN_FLAG: &str = "--hidden";

/// Frontend event carrying each daemon event as is.
const EVENT_DAEMON: &str = "daemon-event";
/// Frontend event carrying whether the daemon is connected.
const EVENT_CONNECTION: &str = "daemon-connection";

/// Connection to the daemon, shared as Tauri state.
#[derive(Default)]
pub(crate) struct Daemon {
    /// IPC client, `None` while the daemon is not reachable.
    client: RwLock<Option<Arc<Client>>>,
}

impl Daemon {
    /// Sends `request` to the daemon and waits for its response.
    ///
    /// # Errors
    ///
    /// Fails with a message for the UI if the daemon is not connected or the
    /// exchange fails.
    pub(crate) async fn request(&self, request: Request) -> Result<Response, String> {
        let client = self.client.read().await.clone();
        let client = client.ok_or("The Submarine service is not running")?;
        client.request(request).await.map_err(|e| e.to_string())
    }
}

/// Tauri command forwarding a UI request to the daemon.
#[tauri::command]
async fn daemon_request(daemon: State<'_, Daemon>, request: Request) -> Result<Response, String> {
    daemon.request(request).await
}

/// Tauri command telling the UI whether the daemon is connected.
#[tauri::command]
async fn daemon_connected(daemon: State<'_, Daemon>) -> Result<bool, String> {
    Ok(daemon.client.read().await.is_some())
}

/// Keeps a connection to the daemon open, reconnecting when it restarts.
///
/// On every connection it loads a fresh snapshot of the daemon state, then
/// forwards each event to the live state and to the frontend until the daemon
/// goes away.
async fn maintain_connection(app: AppHandle) {
    let path = submarine_ipc::socket_path();
    loop {
        match Client::connect(&path).await {
            Ok(client) => {
                // subscription before the snapshot, so no event is lost in between
                let client = Arc::new(client);
                let mut events = client.subscribe();
                *app.state::<Daemon>().client.write().await = Some(client.clone());
                if let Err(err) = load_snapshot(&app, &client).await {
                    tracing::warn!("cannot read the daemon state: {err}");
                }
                let _ = app.emit(EVENT_CONNECTION, true);

                // relay of the events until the daemon closes the connection
                // NB: a lagged receiver only skips events; each event carries a full
                // status, list or settings, so later ones bring the state back in line
                loop {
                    match events.recv().await {
                        Ok(event) => {
                            app.state::<live::Live>().apply_event(&app, &event);
                            let _ = app.emit(EVENT_DAEMON, event);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }

                // connection lost: the UI and the tray show the service as down
                *app.state::<Daemon>().client.write().await = None;
                app.state::<live::Live>()
                    .update(&app, |s| s.service_up = false);
                let _ = app.emit(EVENT_CONNECTION, false);
                tracing::warn!("lost connection to the daemon");
            }
            Err(err) => tracing::debug!("daemon not reachable: {err}"),
        }
        tokio::time::sleep(RECONNECT_DELAY).await;
    }
}

/// Replaces the live state with the status, tunnels and settings read from the
/// daemon.
///
/// An unexpected response leaves the state untouched.
///
/// # Errors
///
/// Fails if a request to the daemon fails.
async fn load_snapshot(app: &AppHandle, client: &Client) -> Result<(), submarine_ipc::ClientError> {
    let Response::Status(status) = client.request(Request::GetStatus).await? else {
        return Ok(());
    };
    let Response::Tunnels(tunnels) = client.request(Request::ListTunnels).await? else {
        return Ok(());
    };
    let Response::Settings(settings) = client.request(Request::GetSettings).await? else {
        return Ok(());
    };
    app.state::<live::Live>().update(app, |s| {
        *s = live::Snapshot {
            service_up: true,
            status,
            tunnels,
            settings,
        };
    });
    Ok(())
}

/// Shows and focuses the main window; returns false if that was not possible.
pub(crate) fn show_main_window(app: &AppHandle) -> bool {
    let Some(window) = app.get_webview_window("main") else {
        return false;
    };
    let shown = window.show().is_ok();
    let _ = window.unminimize();
    shown && window.set_focus().is_ok()
}

/// Every way of quitting (tray, close button, Cmd+Q, logout) ends up here:
/// with `disconnect_on_quit` the VPN goes down before the app exits.
fn before_exit(app: &AppHandle) {
    // only the first exit request does the work
    if QUITTING.swap(true, Ordering::SeqCst) {
        return;
    }
    // disconnect if wanted, bounded in time, then the real exit
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let wanted = app.state::<prefs::PrefsState>().get().disconnect_on_quit;
        let state = app.state::<live::Live>().get().status.state;
        if wanted && state != ConnectionState::Disconnected {
            let daemon = app.state::<Daemon>();
            let request = daemon.request(Request::Disconnect);
            match tokio::time::timeout(QUIT_DISCONNECT_TIMEOUT, request).await {
                Ok(Ok(_)) => tracing::info!("disconnected before quitting"),
                Ok(Err(err)) => tracing::warn!("cannot disconnect before quitting: {err}"),
                Err(_) => tracing::warn!("the service did not disconnect in time"),
            }
        }
        READY_TO_EXIT.store(true, Ordering::SeqCst);
        app.exit(0);
    });
}

/// Called in the running instance when the app is launched again.
fn on_second_instance(app: &AppHandle) {
    if !show_main_window(app) {
        app.dialog()
            .message("Submarine is already running.")
            .title("Submarine")
            .kind(MessageDialogKind::Info)
            .show(|_| {});
    }
}

/// Builds and runs the desktop app, until it exits.
///
/// # Panics
///
/// Panics if the Tauri app cannot be built.
pub fn run() {
    tauri::Builder::default()
        // single instance first of all plugins: a second launch only wakes this one
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            on_second_instance(app)
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init())
        // start at login with --hidden, so the app stays in the tray
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![HIDDEN_FLAG]),
        ))
        .manage(Daemon::default())
        .manage(live::Live::default())
        .invoke_handler(tauri::generate_handler![
            daemon_request,
            daemon_connected,
            apps::list_apps,
            config_file::read_config_file,
            prefs::get_prefs,
            prefs::set_prefs
        ])
        .setup(|app| {
            let handle = app.handle();
            handle.manage(prefs::PrefsState::load(handle));
            tray::create(handle)?;
            // the window starts hidden (tauri.conf.json), to avoid a flash at login
            if !std::env::args().any(|a| a == HIDDEN_FLAG) {
                show_main_window(handle);
            }
            tauri::async_runtime::spawn(maintain_connection(app.handle().clone()));
            Ok(())
        })
        .on_window_event(|window, event| {
            // close button: hide to the tray, or quit, as the preferences say
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let app = window.app_handle();
                if app.state::<prefs::PrefsState>().get().close_to_tray {
                    let _ = window.hide();
                } else {
                    app.exit(0);
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building Submarine")
        .run(|app, event| {
            // every exit is held back until `before_exit` has done its work
            if let RunEvent::ExitRequested { api, .. } = event {
                if !READY_TO_EXIT.load(Ordering::SeqCst) {
                    api.prevent_exit();
                    before_exit(app);
                }
            }
        });
}

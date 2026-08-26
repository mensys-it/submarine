//! System notifications, on meaningful state changes only: not at startup,
//! not when the user disconnects on purpose, not for traffic counters.
//!
//! Connecting and dropping are NOT notified while the window is in front,
//! because the window shows its own toast for those.

use submarine_ipc::ConnectionState;
use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

use crate::i18n::Texts;
use crate::live::Snapshot;

/// Shows the notifications due to the change from `before` to `after`, with the
/// texts `t`.
pub fn on_change(app: &AppHandle, t: &Texts, before: &Snapshot, after: &Snapshot) {
    // initial load or service reconnection: nothing to report
    if !before.service_up || !after.service_up {
        return;
    }
    // name of the current tunnel, or the app name if unknown
    let name = after
        .status
        .tunnel_id
        .as_deref()
        .and_then(|id| after.tunnels.iter().find(|t| t.id == id))
        .map(|t| t.name.as_str())
        .unwrap_or("Submarine");
    let (b, a) = (&before.status, &after.status);
    let in_front = window_in_front(app);

    // connected, dropped, or traffic blocked by the kill switch: at most one of them
    if b.state != ConnectionState::Connected && a.state == ConnectionState::Connected {
        if !in_front {
            show(
                app,
                t.notify_connected,
                &format!("{} {} {name}", t.connected, t.to),
            );
        }
    } else if b.state == ConnectionState::Connected && a.state == ConnectionState::Failed {
        if !in_front {
            let body = if a.blocked {
                t.notify_dropped_blocked
            } else {
                t.notify_dropped_open
            };
            show(app, t.notify_dropped, &format!("{name}: {body}"));
        }
    } else if !b.blocked && a.blocked && a.state != ConnectionState::Connecting {
        show(app, t.notify_blocked, t.notify_blocked_body);
    }
    // failure in applying the protection (firewall), reported whenever it appears
    if b.protection_error.is_none()
        && let Some(error) = &a.protection_error
    {
        show(app, t.notify_protection, error);
    }
}

/// Whether the main window is visible, not minimized and focused.
fn window_in_front(app: &AppHandle) -> bool {
    app.get_webview_window("main").is_some_and(|w| {
        w.is_visible().unwrap_or(false)
            && !w.is_minimized().unwrap_or(false)
            && w.is_focused().unwrap_or(false)
    })
}

/// Shows a system notification; a failure is only logged.
fn show(app: &AppHandle, title: &str, body: &str) {
    if let Err(err) = app.notification().builder().title(title).body(body).show() {
        tracing::warn!("notification failed: {err}");
    }
}

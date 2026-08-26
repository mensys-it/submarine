//! Entry point of the desktop app: everything lives in the library crate, so
//! the same code also builds for the mobile targets of Tauri.

// no extra console window on Windows in release builds
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// Runs the desktop app.
fn main() {
    submarine_desktop_lib::run()
}

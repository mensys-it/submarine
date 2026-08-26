//! Build script of the desktop app: Tauri generates the context of the app
//! (configuration, icons, permissions) and, on Windows, the resources of the
//! executable.

/// Runs the Tauri build steps.
fn main() {
    tauri_build::build()
}

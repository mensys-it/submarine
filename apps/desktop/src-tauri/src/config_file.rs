//! Reads a WireGuard `.conf` the user picked or dropped on the window: the
//! dialog and the webview's drag and drop only hand the UI a path.

use std::io::Read;

/// Far larger than any real configuration.
const MAX_SIZE: u64 = 64 * 1024;

#[tauri::command]
pub fn read_config_file(path: String) -> Result<String, String> {
    let file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err("not a file".into());
    }
    if metadata.len() > MAX_SIZE {
        return Err("too_large".into());
    }
    let mut data = Vec::new();
    file.take(MAX_SIZE + 1)
        .read_to_end(&mut data)
        .map_err(|e| e.to_string())?;
    if data.len() as u64 > MAX_SIZE {
        return Err("too_large".into());
    }
    String::from_utf8(data).map_err(|_| "not_text".into())
}

//! Installed applications the user can pick for split tunneling.
//!
//! Split tunneling matches processes by executable path, so only entries
//! whose command resolves to a real binary are listed: shell-script
//! launchers and sandboxed apps (Flatpak, Snap) run a different executable
//! and would silently not match.

use serde::Serialize;

/// Application offered in the split tunneling list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstalledApp {
    /// Name shown to the user.
    pub name: String,
    /// Absolute path of the executable, the one split tunneling matches.
    pub path: String,
}

/// Tauri command listing the installed applications, sorted by name.
///
/// The scan runs on a blocking thread, since it reads many files or registry
/// keys; a failure of that thread yields an empty list.
#[tauri::command]
pub async fn list_apps() -> Vec<InstalledApp> {
    tauri::async_runtime::spawn_blocking(platform::list)
        .await
        .unwrap_or_default()
}

/// Linux: applications from the freedesktop `.desktop` entries.
#[cfg(target_os = "linux")]
mod platform {
    use std::collections::BTreeMap;
    use std::io::Read;
    use std::path::{Path, PathBuf};

    use super::InstalledApp;

    /// Directories holding `.desktop` entries, system wide and of the user.
    fn application_dirs() -> Vec<PathBuf> {
        let mut dirs = vec![
            PathBuf::from("/usr/share/applications"),
            PathBuf::from("/usr/local/share/applications"),
        ];
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(home).join(".local/share/applications"));
        }
        dirs
    }

    /// Visible applications whose `Exec` resolves to a real binary.
    pub fn list() -> Vec<InstalledApp> {
        // keyed by path: several entries often launch the same binary
        let mut apps = BTreeMap::new();
        for dir in application_dirs() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            // only readable `.desktop` files, the first entry of each binary wins
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_none_or(|e| e != "desktop") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                if let Some((name, exec)) = parse_desktop_entry(&text)
                    && let Some(binary) = resolve_binary(&exec)
                {
                    let key = binary.to_string_lossy().into_owned();
                    apps.entry(key.clone())
                        .or_insert(InstalledApp { name, path: key });
                }
            }
        }
        let mut apps: Vec<InstalledApp> = apps.into_values().collect();
        apps.sort_by_key(|a| a.name.to_lowercase());
        apps
    }

    /// Returns `(Name, Exec)` of a desktop entry, if it is a visible application.
    ///
    /// Only the `[Desktop Entry]` group is read: actions have their own `Name`
    /// and `Exec`. Localized keys (`Name[it]`) are ignored.
    pub(super) fn parse_desktop_entry(text: &str) -> Option<(String, String)> {
        let mut in_entry = false;
        let (mut name, mut exec, mut app, mut hidden) = (None, None, false, false);
        for line in text.lines().map(str::trim) {
            if line.starts_with('[') {
                in_entry = line == "[Desktop Entry]";
                continue;
            }
            if !in_entry {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key.trim() {
                "Name" => name = Some(value.trim().to_owned()),
                "Exec" => exec = Some(value.trim().to_owned()),
                "Type" => app = value.trim() == "Application",
                "NoDisplay" | "Hidden" => hidden |= value.trim() == "true",
                _ => {}
            }
        }
        // NB: `Type` is mandatory, so an entry without it is not an application
        (app && !hidden).then_some(())?;
        Some((name?, exec?))
    }

    /// Resolves the program of an `Exec` line to the real binary it runs.
    ///
    /// The program is the first word, after an `env VAR=value` prefix if any,
    /// looked up in PATH when it is not a path, with symlinks resolved. Returns
    /// `None` for sandbox launchers (Flatpak, Snap, bubblewrap) and scripts,
    /// whose processes run a different executable.
    pub(super) fn resolve_binary(exec: &str) -> Option<PathBuf> {
        // program word, skipping an `env` prefix and its assignments
        let mut words = exec.split_whitespace().map(|w| w.trim_matches('"'));
        let mut program = words.next()?;
        if Path::new(program).file_name().is_some_and(|n| n == "env") {
            program = words.find(|w| !w.contains('='))?;
        }
        // lookup in PATH, only for a bare name
        let path = if program.contains('/') {
            PathBuf::from(program)
        } else {
            std::env::var_os("PATH")
                .map(|p| {
                    std::env::split_paths(&p)
                        .map(|d| d.join(program))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
                .into_iter()
                .find(|p| p.is_file())?
        };
        // real file behind the symlinks, rejected if it is a launcher or a script
        let path = path.canonicalize().ok()?;
        let name = path.file_name()?.to_str()?;
        if matches!(name, "flatpak" | "snap" | "bwrap") || is_script(&path) {
            return None;
        }
        Some(path)
    }

    /// Whether the file starts with a shebang; an unreadable file counts as one,
    /// so it is left out.
    fn is_script(path: &Path) -> bool {
        let mut magic = [0u8; 2];
        std::fs::File::open(path)
            .and_then(|mut f| f.read_exact(&mut magic))
            .map(|()| &magic == b"#!")
            .unwrap_or(true)
    }
}

/// Windows: applications from the registry, of the machine and of the user.
#[cfg(windows)]
mod platform {
    use std::collections::BTreeMap;
    use std::path::Path;

    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    use super::InstalledApp;

    /// Key where applications register their executable for the Run dialog.
    const APP_PATHS: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths";
    /// Keys of the installed programs list, native and 32-bit.
    const UNINSTALL: &[&str] = &[
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
    ];

    /// Apps registered under "App Paths" and in the uninstall list, whose
    /// executable exists.
    pub fn list() -> Vec<InstalledApp> {
        // keyed by lowercase path, since Windows paths are case insensitive
        let mut apps = BTreeMap::new();
        for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
            let root = RegKey::predef(hive);
            for (name, path) in app_paths(&root).into_iter().chain(uninstall_entries(&root)) {
                apps.entry(path.to_lowercase())
                    .or_insert(InstalledApp { name, path });
            }
        }
        let mut apps: Vec<InstalledApp> = apps.into_values().collect();
        apps.sort_by_key(|a| a.name.to_lowercase());
        apps
    }

    /// `(name, path)` of the "App Paths" entries, named after the executable.
    fn app_paths(root: &RegKey) -> Vec<(String, String)> {
        let Ok(key) = root.open_subkey(APP_PATHS) else {
            return Vec::new();
        };
        key.enum_keys()
            .flatten()
            .filter_map(|sub| {
                let path: String = key.open_subkey(&sub).ok()?.get_value("").ok()?;
                let path = clean_path(&path)?;
                let name = Path::new(&path).file_stem()?.to_string_lossy().into_owned();
                Some((name, path))
            })
            .collect()
    }

    /// `(name, path)` of the installed programs whose `DisplayIcon` is their
    /// executable.
    ///
    /// The icon is the only hint of the main executable there; entries whose
    /// icon is the uninstaller are skipped.
    fn uninstall_entries(root: &RegKey) -> Vec<(String, String)> {
        UNINSTALL
            .iter()
            .filter_map(|p| root.open_subkey(p).ok())
            .flat_map(|key| {
                key.enum_keys()
                    .flatten()
                    .filter_map(|sub| {
                        let entry = key.open_subkey(&sub).ok()?;
                        let name: String = entry.get_value("DisplayName").ok()?;
                        let icon: String = entry.get_value("DisplayIcon").ok()?;
                        let path = clean_path(&icon)?;
                        let file = Path::new(&path)
                            .file_name()?
                            .to_string_lossy()
                            .to_lowercase();
                        (!file.starts_with("unins") && !file.contains("uninstall"))
                            .then_some((name, path))
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Turns `"C:\x\app.exe",0` into `C:\x\app.exe`, if it is an existing `.exe`.
    fn clean_path(raw: &str) -> Option<String> {
        let path = raw.split(',').next()?.trim().trim_matches('"').to_owned();
        let ok = path.to_lowercase().ends_with(".exe") && Path::new(&path).is_file();
        ok.then_some(path)
    }
}

/// Other platforms (macOS): no applications to offer.
#[cfg(not(any(target_os = "linux", windows)))]
mod platform {
    use super::InstalledApp;

    /// Returns an empty list.
    // TODO(macos): list applications once split tunneling is available there.
    pub fn list() -> Vec<InstalledApp> {
        Vec::new()
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::platform::*;

    // only the main entry of visible applications is read
    #[test]
    fn parses_visible_applications() {
        let entry = "[Desktop Entry]\nType=Application\nName=Files\nExec=nautilus --new-window %U\n\n[Desktop Action new]\nName=Other\nExec=other\n";
        assert_eq!(
            parse_desktop_entry(entry),
            Some(("Files".into(), "nautilus --new-window %U".into()))
        );
        assert_eq!(
            parse_desktop_entry(
                "[Desktop Entry]\nType=Application\nName=X\nExec=x\nNoDisplay=true\n"
            ),
            None
        );
        assert_eq!(
            parse_desktop_entry("[Desktop Entry]\nType=Link\nName=X\nExec=x\n"),
            None
        );
    }

    // paths, PATH lookups and `env` prefixes resolve; unknown programs do not
    #[test]
    fn resolves_binaries_and_skips_scripts() {
        assert_eq!(
            resolve_binary("/bin/sh -c true").map(|p| p.is_absolute()),
            Some(true)
        );
        assert!(resolve_binary("env FOO=1 sh").is_some());
        assert_eq!(resolve_binary("does-not-exist-anywhere"), None);
    }
}

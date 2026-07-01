//! Persistent storage: imported tunnels (one JSON file each), settings and the
//! connection the user asked for. Files contain private keys and are readable
//! only by the daemon's user.
//!
//! Layout of the data directory:
//! - `tunnels/<id>.json`: one `StoredTunnel` per tunnel, `<id>` being a random
//!   UUID in 32-digit hex form;
//! - `settings.json`: the user settings;
//! - `state.json`: the tunnel the user left connected.
//!
//! Every write is atomic (temporary file + rename).

use std::io;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use submarine_config::TunnelConfig;
use submarine_ipc::{Settings, TunnelInfo};

/// Maximum length of a tunnel name, in characters.
const MAX_NAME_LEN: usize = 64;

/// Content of a tunnel file.
#[derive(Serialize, Deserialize)]
pub struct StoredTunnel {
    /// Name chosen by the user, already trimmed.
    pub name: String,
    /// Parsed WireGuard configuration, private key included.
    pub config: TunnelConfig,
}

/// Survives restarts, so a crash or reboot reconnects the tunnel the user
/// left connected (and keeps the kill switch engaged meanwhile).
#[derive(Default, Serialize, Deserialize)]
struct DaemonState {
    /// Id of the tunnel to reconnect, `None` if the user disconnected.
    connected_tunnel: Option<String>,
}

/// Access to the files in the data directory.
pub struct TunnelStore {
    /// Data directory (settings and state).
    root: PathBuf,
    /// `tunnels` subdirectory.
    dir: PathBuf,
}

impl TunnelStore {
    /// Opens the store in `data_dir`, creating it and the `tunnels` subdirectory
    /// if missing, and restricting both to the daemon's user.
    ///
    /// # Errors
    /// Fails if the directories cannot be created or their permissions set.
    pub fn open(data_dir: &Path) -> io::Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        restrict(data_dir, 0o700)?;
        let dir = data_dir.join("tunnels");
        std::fs::create_dir_all(&dir)?;
        restrict(&dir, 0o700)?;
        Ok(Self {
            root: data_dir.to_owned(),
            dir,
        })
    }

    /// Returns the saved settings, or the defaults if missing or unreadable.
    pub fn settings(&self) -> Settings {
        read_json(&self.root.join("settings.json")).unwrap_or_default()
    }

    /// Saves the settings.
    pub fn save_settings(&self, settings: &Settings) -> io::Result<()> {
        write_json(&self.root.join("settings.json"), settings)
    }

    /// Returns the tunnel the user left connected, if any (`None` also when the
    /// state file is missing or unreadable).
    pub fn connected_tunnel(&self) -> Option<String> {
        read_json::<DaemonState>(&self.root.join("state.json"))
            .ok()?
            .connected_tunnel
    }

    /// Saves the tunnel the user asked to connect, `None` after a disconnect.
    pub fn save_connected_tunnel(&self, id: Option<&str>) -> io::Result<()> {
        let state = DaemonState {
            connected_tunnel: id.map(str::to_owned),
        };
        write_json(&self.root.join("state.json"), &state)
    }

    /// Lists the stored tunnels sorted by name (case-insensitive). Unreadable
    /// files are logged and skipped, so one corrupt file does not hide the others.
    ///
    /// # Errors
    /// Fails only if the directory itself cannot be read.
    pub fn list(&self) -> io::Result<Vec<TunnelInfo>> {
        let mut tunnels = Vec::new();
        // only `*.json` files count: e.g. leftover `.tmp` files are ignored
        for entry in std::fs::read_dir(&self.dir)? {
            let path = entry?.path();
            let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            match self.get(id) {
                Ok(stored) => tunnels.push(TunnelInfo::new(id.into(), stored.name, &stored.config)),
                Err(err) => {
                    tracing::warn!(path = %path.display(), "skipping unreadable tunnel: {err}")
                }
            }
        }
        tunnels.sort_by_key(|t| t.name.to_lowercase());
        Ok(tunnels)
    }

    /// Reads a tunnel.
    ///
    /// # Errors
    /// `NotFound` for an invalid or unknown id, `InvalidData` for a corrupt file.
    pub fn get(&self, id: &str) -> io::Result<StoredTunnel> {
        read_json(&self.path(id)?)
    }

    /// Stores a new tunnel and returns its id. The name is trimmed.
    ///
    /// # Errors
    /// `InvalidInput` if the name is empty or longer than `MAX_NAME_LEN`.
    pub fn insert(&self, name: &str, config: TunnelConfig) -> io::Result<String> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        write_json(&self.path(&id)?, &stored(name, config)?)?;
        Ok(id)
    }

    /// Replaces the name and configuration of an existing tunnel.
    ///
    /// # Errors
    /// `NotFound` if the tunnel does not exist, `InvalidInput` for a bad name.
    pub fn update(&self, id: &str, name: &str, config: TunnelConfig) -> io::Result<()> {
        let path = self.path(id)?;
        if !path.exists() {
            return Err(io::Error::new(io::ErrorKind::NotFound, "unknown tunnel"));
        }
        write_json(&path, &stored(name, config)?)
    }

    /// Deletes a tunnel file.
    pub fn delete(&self, id: &str) -> io::Result<()> {
        std::fs::remove_file(self.path(id)?)
    }

    /// Returns the file path of tunnel `id`.
    ///
    /// Ids come from clients: only the format we generate (32 hex digits) is
    /// accepted, so they can NEVER escape the storage directory. Any other id is
    /// reported as `NotFound`.
    fn path(&self, id: &str) -> io::Result<PathBuf> {
        if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(io::Error::new(io::ErrorKind::NotFound, "unknown tunnel"));
        }
        Ok(self.dir.join(format!("{id}.json")))
    }
}

/// Validates and trims the name, building the content of a tunnel file.
fn stored(name: &str, config: TunnelConfig) -> io::Result<StoredTunnel> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("tunnel name must be 1 to {MAX_NAME_LEN} characters"),
        ));
    }
    Ok(StoredTunnel {
        name: name.to_owned(),
        config,
    })
}

/// Reads and deserializes a JSON file; parse errors become `InvalidData`.
fn read_json<T: DeserializeOwned>(path: &Path) -> io::Result<T> {
    let data = std::fs::read(path)?;
    serde_json::from_slice(&data).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

/// Atomic write of a file readable only by the owner. The temporary file is
/// created with restricted permissions, so secrets are never world-readable.
fn write_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    use std::io::Write;
    // removal of a temporary file left by an interrupted write, required by
    // `create_new`
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    // creation with mode 0600 on Unix (on Windows the directory ACL applies)
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    // write, flush to disk and rename over the destination: readers see
    // either the old or the new content, never a partial file
    let mut file = options.open(&tmp)?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.sync_all()?;
    std::fs::rename(&tmp, path)
}

/// Unix: sets the permission bits of `path` to `mode`.
#[cfg(unix)]
fn restrict(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

/// Windows: only SYSTEM and Administrators may access the directory; files
/// created inside inherit the ACL. `ProgramData` would otherwise let every
/// user read the private keys.
#[cfg(windows)]
fn restrict(path: &Path, _mode: u32) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1, SE_FILE_OBJECT,
        SetNamedSecurityInfoW,
    };
    use windows_sys::Win32::Security::{
        ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
    };

    // files inherit the ACL from their directory
    if !path.is_dir() {
        return Ok(());
    }
    // protected DACL (parent ACEs not inherited) granting full access, inherited
    // by files and subdirectories, only to SYSTEM (SY) and Administrators (BA)
    let sddl: Vec<u16> = "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)"
        .encode_utf16()
        .chain([0])
        .collect();
    let wide_path: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: valid NUL-terminated SDDL and out pointer.
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut sd,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // extraction of the DACL from the descriptor and its application to the path
    let (mut present, mut defaulted) = (0, 0);
    let mut dacl: *mut ACL = std::ptr::null_mut();
    // SAFETY: `sd` was allocated above; out pointers are valid.
    let result =
        if unsafe { GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            // SAFETY: valid path and DACL; owner, group and SACL are left unchanged.
            let code = unsafe {
                SetNamedSecurityInfoW(
                    wide_path.as_ptr(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    dacl,
                    std::ptr::null(),
                )
            };
            if code == 0 {
                Ok(())
            } else {
                Err(io::Error::from_raw_os_error(code as i32))
            }
        };
    // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW and
    // freed exactly once, after its last use.
    unsafe { LocalFree(sd) };
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal valid configuration with a full tunnel (`0.0.0.0/0`).
    const CONF: &str = "[Interface]\nPrivateKey = yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=\nAddress = 10.0.0.2/32\n[Peer]\nPublicKey = xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=\nAllowedIPs = 0.0.0.0/0\nEndpoint = 1.2.3.4:51820\n";

    /// Opens a store in a new temporary directory, returned for cleanup.
    fn temp_store() -> (TunnelStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!("submarine-test-{}", uuid::Uuid::new_v4()));
        (TunnelStore::open(&dir).unwrap(), dir)
    }

    // full lifecycle of a tunnel: insert, list, get, update (also invalid) and delete
    #[test]
    fn insert_list_get_delete() {
        let (store, dir) = temp_store();
        let config = submarine_config::parse(CONF).unwrap().config;
        let id = store.insert("  Office  ", config.clone()).unwrap();

        let list = store.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Office");
        assert!(list[0].full_tunnel);
        assert_eq!(store.get(&id).unwrap().config, config);

        store.update(&id, " Home ", config.clone()).unwrap();
        assert_eq!(store.get(&id).unwrap().name, "Home");
        assert!(store.update(&id, "", config.clone()).is_err());
        assert!(store.update(&"0".repeat(32), "x", config).is_err());

        store.delete(&id).unwrap();
        assert!(store.list().unwrap().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    // ids trying to escape the directory and blank names are rejected
    #[test]
    fn rejects_bad_ids_and_names() {
        let (store, dir) = temp_store();
        assert!(store.get("../../etc/passwd").is_err());
        assert!(store.delete("..").is_err());
        let config = submarine_config::parse(CONF).unwrap().config;
        assert!(store.insert("   ", config).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    // defaults when nothing is saved, then settings and state read back as saved
    #[test]
    fn settings_and_state_round_trip() {
        let (store, dir) = temp_store();
        assert_eq!(store.settings(), Settings::default());
        assert_eq!(store.connected_tunnel(), None);
        let settings = Settings {
            allow_lan: true,
            ..Settings::default()
        };
        store.save_settings(&settings).unwrap();
        store.save_connected_tunnel(Some("abc")).unwrap();
        assert_eq!(store.settings(), settings);
        assert_eq!(store.connected_tunnel().as_deref(), Some("abc"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    // tunnel files are created with mode 0600
    #[cfg(unix)]
    #[test]
    fn files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let (store, dir) = temp_store();
        let id = store
            .insert("x", submarine_config::parse(CONF).unwrap().config)
            .unwrap();
        let mode = std::fs::metadata(store.path(&id).unwrap())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

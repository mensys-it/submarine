//! Persistent storage: imported tunnels (one encrypted file each), settings and
//! the connection the user asked for. Every file is readable only by the
//! daemon's user, and the tunnel files, which hold the private keys, are also
//! encrypted (see `vault`).
//!
//! Layout of the data directory:
//! - `tunnels/<id>.tunnel`: one encrypted `StoredTunnel` per tunnel, `<id>`
//!   being a random UUID in 32-digit hex form;
//! - `vault.key`: the key of those files, protected by the OS keystore;
//! - `settings.json`: the user settings;
//! - `state.json`: the tunnel the user left connected.
//!
//! Tunnels of older versions, stored as plain `tunnels/<id>.json`, are
//! encrypted when the store opens. Every write is atomic (temporary file +
//! rename).

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use submarine_config::TunnelConfig;
use submarine_ipc::{Settings, TunnelInfo};
use zeroize::Zeroizing;

use crate::vault::Vault;

/// Maximum length of a tunnel name, in characters.
const MAX_NAME_LEN: usize = 64;
/// Most tunnels that can be stored. With the limits of `submarine_config` on a single
/// configuration, the summaries of all of them fit in one IPC message.
const MAX_TUNNELS: usize = 32;

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
    /// Key of the tunnel files, or why it could not be loaded: then the tunnels
    /// cannot be read nor written, but settings and state still work.
    vault: Result<Vault, String>,
}

impl TunnelStore {
    /// Opens the store in `data_dir`, creating it and the `tunnels` subdirectory
    /// if missing, and restricting both to the daemon's user. Plain tunnel files
    /// of older versions are encrypted on the way.
    ///
    /// A vault key that cannot be unlocked is logged, not returned: the daemon
    /// still starts, and reports the error on every tunnel operation.
    ///
    /// # Errors
    /// Fails if the directories cannot be created or their permissions set.
    pub fn open(data_dir: &Path) -> io::Result<Self> {
        let dir = data_dir.join("tunnels");
        for path in [data_dir, &dir] {
            if let Some(aside) = crate::datadir::prepare(path)? {
                tracing::warn!(
                    "{} was not created by the service: moved to {}",
                    path.display(),
                    aside.display()
                );
            }
        }
        let vault = Vault::open(data_dir).map_err(|err| {
            tracing::error!("cannot unlock the stored tunnels: {err}");
            err.to_string()
        });
        if let Ok(vault) = &vault {
            tracing::info!(
                keystore = vault.keystore().name(),
                "stored tunnels unlocked"
            );
        }
        let store = Self {
            root: data_dir.to_owned(),
            dir,
            vault,
        };
        store.encrypt_plain_files();
        Ok(store)
    }

    /// Encrypts the plain `<id>.json` tunnel files of older versions, then removes
    /// them. A file that fails is logged and left as it is, still readable.
    fn encrypt_plain_files(&self) {
        if self.vault.is_err() {
            return;
        }
        let Ok(ids) = self.ids("json") else {
            return;
        };
        for id in ids {
            let result = read_json::<StoredTunnel>(&self.plain_path(&id))
                .and_then(|stored| self.write(&id, &stored));
            match result {
                Ok(()) => tracing::info!(%id, "tunnel file encrypted"),
                Err(err) => tracing::warn!(%id, "cannot encrypt the tunnel file: {err}"),
            }
        }
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
        // encrypted files, plus plain ones not encrypted yet
        let mut ids = self.ids("tunnel")?;
        ids.extend(self.ids("json")?);
        let mut tunnels = Vec::new();
        for id in ids {
            match self.get(&id) {
                Ok(stored) => tunnels.push(TunnelInfo::new(id, stored.name, &stored.config)),
                Err(err) => tracing::warn!(%id, "skipping unreadable tunnel: {err}"),
            }
        }
        tunnels.sort_by_key(|t| t.name.to_lowercase());
        Ok(tunnels)
    }

    /// Reads a tunnel, from its encrypted file or else from a plain one of an
    /// older version.
    ///
    /// # Errors
    /// `NotFound` for an invalid or unknown id, `InvalidData` for a corrupt file,
    /// and an error naming the vault when the tunnels are locked.
    pub fn get(&self, id: &str) -> io::Result<StoredTunnel> {
        let data = match std::fs::read(self.path(id)?) {
            Ok(data) => data,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return read_json(&self.plain_path(id)).map_err(unknown_if_missing);
            }
            Err(err) => return Err(err),
        };
        let plain = self.vault()?.unseal(&data, id.as_bytes())?;
        serde_json::from_slice(&plain)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
    }

    /// Stores a new tunnel and returns its id. The name is trimmed.
    ///
    /// # Errors
    /// `InvalidInput` if the name is empty or longer than `MAX_NAME_LEN`, or if
    /// `MAX_TUNNELS` tunnels are already stored.
    pub fn insert(&self, name: &str, config: TunnelConfig) -> io::Result<String> {
        // encrypted files, plus plain ones not encrypted yet
        if self.ids("tunnel")?.len() + self.ids("json")?.len() >= MAX_TUNNELS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("at most {MAX_TUNNELS} tunnels can be stored"),
            ));
        }
        let id = uuid::Uuid::new_v4().simple().to_string();
        self.write(&id, &stored(name, config)?)?;
        Ok(id)
    }

    /// Replaces the name and configuration of an existing tunnel.
    ///
    /// # Errors
    /// `NotFound` if the tunnel does not exist, `InvalidInput` for a bad name.
    pub fn update(&self, id: &str, name: &str, config: TunnelConfig) -> io::Result<()> {
        if !self.path(id)?.exists() && !self.plain_path(id).exists() {
            return Err(io::Error::new(io::ErrorKind::NotFound, "unknown tunnel"));
        }
        self.write(id, &stored(name, config)?)
    }

    /// Deletes a tunnel, whether its file is encrypted or still plain.
    pub fn delete(&self, id: &str) -> io::Result<()> {
        let encrypted = std::fs::remove_file(self.path(id)?);
        let plain = std::fs::remove_file(self.plain_path(id));
        // an error only if neither file could be removed
        encrypted.or(plain).map_err(unknown_if_missing)
    }

    /// Encrypts and writes tunnel `id`, then removes its plain file of an older
    /// version, if any.
    fn write(&self, id: &str, stored: &StoredTunnel) -> io::Result<()> {
        let plain = Zeroizing::new(serde_json::to_vec(stored)?);
        let sealed = self.vault()?.seal(&plain, id.as_bytes())?;
        write_private(&self.path(id)?, &sealed)?;
        match std::fs::remove_file(self.plain_path(id)) {
            Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        }
    }

    /// The vault, or an error saying why the tunnels are locked.
    fn vault(&self) -> io::Result<&Vault> {
        self.vault
            .as_ref()
            .map_err(|err| io::Error::other(format!("the stored tunnels are locked: {err}")))
    }

    /// Ids of the tunnel files with extension `ext`; e.g. leftover `.tmp` files
    /// and names that are not ids are ignored.
    fn ids(&self, ext: &str) -> io::Result<BTreeSet<String>> {
        let mut ids = BTreeSet::new();
        for entry in std::fs::read_dir(&self.dir)? {
            let path = entry?.path();
            if path.extension().is_none_or(|e| e != ext) {
                continue;
            }
            if let Some(id) = path.file_stem().and_then(|s| s.to_str())
                && valid_id(id)
            {
                ids.insert(id.to_owned());
            }
        }
        Ok(ids)
    }

    /// Returns the path of the encrypted file of tunnel `id`.
    ///
    /// Ids come from clients: only the format we generate (32 hex digits) is
    /// accepted, so they can NEVER escape the storage directory. Any other id is
    /// reported as `NotFound`.
    fn path(&self, id: &str) -> io::Result<PathBuf> {
        if !valid_id(id) {
            return Err(io::Error::new(io::ErrorKind::NotFound, "unknown tunnel"));
        }
        Ok(self.dir.join(format!("{id}.tunnel")))
    }

    /// Path of the plain file of an older version, for an id already checked by
    /// `path` or `ids`.
    fn plain_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }
}

/// Whether `id` has the format of the ids we generate: 32 hex digits.
fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
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

/// Atomic write of a JSON file readable only by the owner.
fn write_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    write_private(path, &serde_json::to_vec_pretty(value)?)
}

/// Atomic write of a file readable only by the owner. The temporary file is
/// created with restricted permissions, so secrets are never world-readable.
pub(crate) fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
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
    file.write_all(data)?;
    file.sync_all()?;
    std::fs::rename(&tmp, path)
}

/// Turns the "no such file" of a missing tunnel file into the error shown for an
/// unknown tunnel, which tells the user what is wrong.
fn unknown_if_missing(err: io::Error) -> io::Error {
    if err.kind() == io::ErrorKind::NotFound {
        io::Error::new(io::ErrorKind::NotFound, "unknown tunnel")
    } else {
        err
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // a missing tunnel is reported as unknown, not as a missing file
    #[test]
    fn unknown_tunnel_message() {
        let (store, dir) = temp_store();
        let id = "f".repeat(32);
        assert_eq!(store.get(&id).err().unwrap().to_string(), "unknown tunnel");
        assert_eq!(store.delete(&id).unwrap_err().to_string(), "unknown tunnel");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Minimal valid configuration with a full tunnel (`0.0.0.0/0`).
    const CONF: &str = "[Interface]\nPrivateKey = yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=\nAddress = 10.0.0.2/32\n[Peer]\nPublicKey = xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=\nAllowedIPs = 0.0.0.0/0\nEndpoint = 203.0.113.10:51820\n";

    // the summaries of the most tunnels, each as large as the parser allows, fit in
    // one IPC message
    #[test]
    fn largest_tunnel_list_fits_in_a_message() {
        let name = "\u{1F30A}".repeat(MAX_NAME_LEN);
        let long = format!("{}.example", "a".repeat(245));
        let addresses = vec!["fd00:1111:2222:3333:4444:5555:6666:7777/128"; 16].join(", ");
        let dns = vec![long.as_str(); 16].join(", ");
        let mut text = format!(
            "[Interface]\nPrivateKey = yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=\nAddress = {addresses}\nDNS = {dns}\n"
        );
        for _ in 0..64 {
            text.push_str(&format!(
                "[Peer]\nPublicKey = xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=\nAllowedIPs = 0.0.0.0/0\nEndpoint = {long}:65535\n"
            ));
        }
        let config = submarine_config::parse(&text).unwrap().config;
        let info = TunnelInfo::new("0".repeat(32), name, &config);
        let message = submarine_ipc::ServerMessage::Response {
            id: u64::MAX,
            result: Ok(submarine_ipc::Response::Tunnels(vec![info; MAX_TUNNELS])),
        };
        let len = serde_json::to_vec(&message).unwrap().len() as u64;
        assert!(len < submarine_ipc::MAX_MESSAGE_LEN, "{len} bytes");
    }

    // no tunnel is stored beyond the limit
    #[test]
    fn at_most_max_tunnels() {
        let (store, dir) = temp_store();
        let config = || submarine_config::parse(CONF).unwrap().config;
        for i in 0..MAX_TUNNELS {
            store.insert(&format!("t{i}"), config()).unwrap();
        }
        let err = store.insert("one more", config()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        std::fs::remove_dir_all(dir).unwrap();
    }

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

    // the private key never reaches the disk in clear text
    #[test]
    fn tunnel_files_are_encrypted() {
        let (store, dir) = temp_store();
        let id = store
            .insert("x", submarine_config::parse(CONF).unwrap().config)
            .unwrap();
        let data = std::fs::read(store.path(&id).unwrap()).unwrap();
        let text = String::from_utf8_lossy(&data);
        assert!(!text.contains("yAnz5TF"));
        assert!(!text.contains("PrivateKey") && !text.contains("private_key"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    // a plain tunnel file of an older version is encrypted when the store opens
    #[test]
    fn plain_files_are_encrypted_on_open() {
        let (store, dir) = temp_store();
        let id = "0123456789abcdef0123456789abcdef";
        let stored = StoredTunnel {
            name: "Old".into(),
            config: submarine_config::parse(CONF).unwrap().config,
        };
        write_json(&store.plain_path(id), &stored).unwrap();
        drop(store);

        let store = TunnelStore::open(&dir).unwrap();
        assert!(!store.plain_path(id).exists());
        assert!(store.path(id).unwrap().exists());
        assert_eq!(store.get(id).unwrap().name, "Old");
        assert_eq!(store.list().unwrap().len(), 1);
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

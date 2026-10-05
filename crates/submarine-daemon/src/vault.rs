//! Encryption at rest of the stored tunnels, whose files hold the private keys.
//!
//! A random 256-bit master key encrypts every tunnel file with ChaCha20-Poly1305,
//! with the tunnel id as associated data, so a file cannot be passed off as another
//! one. The master key lives in `vault.key`, protected by the keystore of the OS:
//! - Windows: DPAPI, bound to this computer;
//! - Linux: `systemd-creds`, bound to the TPM2 when there is one, otherwise to the
//!   credential secret of the host;
//! - macOS: the System keychain.
//!
//! Without a usable keystore the master key is stored as it is, protected only
//! by the file permissions like the tunnel files used to be, and a warning is
//! logged. Either way, the tunnel files alone never reveal the private keys.

use std::io;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use zeroize::Zeroizing;

/// Name of the file holding the protected master key, in the data directory.
const KEY_FILE: &str = "vault.key";
/// First bytes of an encrypted tunnel file, with the format version.
const MAGIC: &[u8] = b"SUBMARINE-VAULT-1\n";
/// Length of the ChaCha20-Poly1305 nonce, stored after the magic.
const NONCE_LEN: usize = 12;
/// Length of the master key.
const KEY_LEN: usize = 32;

/// How the master key is protected on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keystore {
    /// Windows DPAPI, machine scope.
    Dpapi,
    /// `systemd-creds` (TPM2 or host secret).
    SystemdCreds,
    /// macOS System keychain; the key file only marks it.
    Keychain,
    /// No keystore: the key file holds the key itself.
    Plain,
}

impl Keystore {
    /// Name written on the first line of the key file.
    pub fn name(self) -> &'static str {
        match self {
            Self::Dpapi => "dpapi",
            Self::SystemdCreds => "systemd-creds",
            Self::Keychain => "keychain",
            Self::Plain => "plain",
        }
    }

    /// Inverse of [`Keystore::name`].
    fn parse(name: &str) -> Option<Self> {
        [Self::Dpapi, Self::SystemdCreds, Self::Keychain, Self::Plain]
            .into_iter()
            .find(|k| k.name() == name)
    }
}

/// The master key of the stored tunnels, ready to encrypt and decrypt them.
pub struct Vault {
    cipher: ChaCha20Poly1305,
    keystore: Keystore,
}

impl Vault {
    /// Loads the master key from `dir`, or creates and protects a new one on
    /// first use.
    ///
    /// # Errors
    /// When the key file exists but cannot be read or unlocked. It is NOT replaced
    /// then: the tunnels encrypted with it would be lost for good.
    pub fn open(dir: &Path) -> io::Result<Self> {
        let path = dir.join(KEY_FILE);
        match std::fs::read(&path) {
            Ok(data) => {
                let (keystore, key) = unseal_key(&data)?;
                Ok(Self::with_key(&key, keystore))
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                let key = Zeroizing::new(<[u8; KEY_LEN]>::from(ChaCha20Poly1305::generate_key(
                    &mut OsRng,
                )));
                let (keystore, sealed) = seal_key(&key);
                crate::store::write_private(&path, &sealed)?;
                tracing::info!(keystore = keystore.name(), "vault key created");
                Ok(Self::with_key(&key, keystore))
            }
            Err(err) => Err(err),
        }
    }

    /// Builds the cipher for `key`.
    fn with_key(key: &[u8; KEY_LEN], keystore: Keystore) -> Self {
        Self {
            cipher: ChaCha20Poly1305::new(Key::from_slice(key)),
            keystore,
        }
    }

    /// The keystore protecting the master key.
    pub fn keystore(&self) -> Keystore {
        self.keystore
    }

    /// Encrypts `plain` with a fresh nonce. `aad` (the tunnel id) is
    /// authenticated but not stored: decrypting needs the same one.
    ///
    /// # Errors
    /// Only if the cipher fails, which does not happen with valid sizes.
    pub fn seal(&self, plain: &[u8], aad: &[u8]) -> io::Result<Vec<u8>> {
        let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
        let sealed = self
            .cipher
            .encrypt(&nonce, Payload { msg: plain, aad })
            .map_err(|_| io::Error::other("encryption failed"))?;
        let mut out = Vec::with_capacity(MAGIC.len() + NONCE_LEN + sealed.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&sealed);
        Ok(out)
    }

    /// Decrypts what [`Vault::seal`] produced with the same `aad`.
    ///
    /// # Errors
    /// `InvalidData` for a file that is not encrypted, damaged, or encrypted for
    /// another tunnel or with another master key.
    pub fn unseal(&self, data: &[u8], aad: &[u8]) -> io::Result<Zeroizing<Vec<u8>>> {
        let invalid =
            |message: &str| io::Error::new(io::ErrorKind::InvalidData, message.to_owned());
        let rest = data
            .strip_prefix(MAGIC)
            .ok_or_else(|| invalid("not an encrypted tunnel file"))?;
        if rest.len() < NONCE_LEN {
            return Err(invalid("truncated tunnel file"));
        }
        let (nonce, sealed) = rest.split_at(NONCE_LEN);
        self.cipher
            .decrypt(Nonce::from_slice(nonce), Payload { msg: sealed, aad })
            .map(Zeroizing::new)
            .map_err(|_| {
                invalid("tunnel file damaged, or encrypted for another tunnel or computer")
            })
    }
}

/// Protects the master key with the keystore of the OS, falling back to
/// [`Keystore::Plain`]. The keystore is trusted only after the key reads back
/// unchanged from it.
fn seal_key(key: &[u8; KEY_LEN]) -> (Keystore, Vec<u8>) {
    let protected = platform::seal(key).and_then(|(keystore, blob)| {
        let back = platform::unseal(keystore, &blob)?;
        if back.as_slice() == key.as_slice() {
            Ok((keystore, blob))
        } else {
            Err(io::Error::other("the key read back differs"))
        }
    });
    match protected {
        Ok((keystore, blob)) => (keystore, encode(keystore, &blob)),
        Err(err) => {
            tracing::warn!(
                "no keystore available ({err}): the vault key is protected only by the file permissions"
            );
            (Keystore::Plain, encode(Keystore::Plain, key))
        }
    }
}

/// Content of the key file: the keystore name, then the base64 payload.
fn encode(keystore: Keystore, payload: &[u8]) -> Vec<u8> {
    format!("{}\n{}\n", keystore.name(), BASE64.encode(payload)).into_bytes()
}

/// Reads the key file back into the keystore and the master key.
fn unseal_key(data: &[u8]) -> io::Result<(Keystore, Zeroizing<[u8; KEY_LEN]>)> {
    let invalid = |message: String| io::Error::new(io::ErrorKind::InvalidData, message);
    let text =
        std::str::from_utf8(data).map_err(|_| invalid("vault key file is not text".into()))?;
    let (name, payload) = text.split_once('\n').unwrap_or((text, ""));
    let keystore = Keystore::parse(name.trim())
        .ok_or_else(|| invalid(format!("unknown vault keystore {name:?}")))?;
    let blob = Zeroizing::new(
        BASE64
            .decode(payload.trim())
            .map_err(|_| invalid("vault key file damaged".into()))?,
    );
    let key = match keystore {
        Keystore::Plain => blob,
        other => platform::unseal(other, &blob).map_err(|err| {
            io::Error::new(
                err.kind(),
                format!("cannot unlock the vault key with {}: {err}", other.name()),
            )
        })?,
    };
    let key: [u8; KEY_LEN] = key
        .as_slice()
        .try_into()
        .map_err(|_| invalid("vault key has the wrong length".into()))?;
    Ok((keystore, Zeroizing::new(key)))
}

/// Runs `program` with `input` on its stdin and returns its stdout. Secrets go
/// through stdin, so they never show in the process list.
#[cfg(unix)]
fn run_with_input(program: &str, args: &[&str], input: &[u8]) -> io::Result<Zeroizing<Vec<u8>>> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    // NB: the inputs are a few hundred bytes at most, well within the pipe buffer,
    // so writing everything before reading cannot deadlock
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input)?;
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(Zeroizing::new(output.stdout))
}

#[cfg(target_os = "linux")]
mod platform {
    use std::io;

    use zeroize::Zeroizing;

    use super::{Keystore, run_with_input};

    /// Name the credential is bound to: a blob made for another name is refused.
    const CREDENTIAL: &str = "--name=submarine-vault";

    /// Encrypts the key with `systemd-creds` (TPM2 and/or host secret).
    pub fn seal(key: &[u8]) -> io::Result<(Keystore, Vec<u8>)> {
        let blob = run_with_input("systemd-creds", &["encrypt", CREDENTIAL, "-", "-"], key)?;
        Ok((Keystore::SystemdCreds, blob.to_vec()))
    }

    /// Decrypts a blob made by [`seal`].
    pub fn unseal(keystore: Keystore, blob: &[u8]) -> io::Result<Zeroizing<Vec<u8>>> {
        match keystore {
            Keystore::SystemdCreds => {
                run_with_input("systemd-creds", &["decrypt", CREDENTIAL, "-", "-"], blob)
            }
            other => Err(super::unsupported(other)),
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::io;

    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_LOCAL_MACHINE, CRYPTPROTECT_UI_FORBIDDEN,
        CryptProtectData, CryptUnprotectData,
    };
    use zeroize::Zeroizing;

    use super::Keystore;

    /// Encrypts the key with DPAPI in machine scope.
    ///
    /// NB: machine scope, not the account of the service: the daemon may also run
    /// in the foreground as an administrator, and must still read the key later as
    /// a service. The data directory is restricted to SYSTEM and Administrators.
    pub fn seal(key: &[u8]) -> io::Result<(Keystore, Vec<u8>)> {
        let blob = dpapi(key, false)?;
        Ok((Keystore::Dpapi, blob.to_vec()))
    }

    /// Decrypts a blob made by [`seal`].
    pub fn unseal(keystore: Keystore, blob: &[u8]) -> io::Result<Zeroizing<Vec<u8>>> {
        match keystore {
            Keystore::Dpapi => dpapi(blob, true),
            other => Err(super::unsupported(other)),
        }
    }

    /// Runs `CryptProtectData`, or `CryptUnprotectData` when `decrypt` is set.
    fn dpapi(data: &[u8], decrypt: bool) -> io::Result<Zeroizing<Vec<u8>>> {
        let input = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB::default();
        let flags = CRYPTPROTECT_UI_FORBIDDEN | CRYPTPROTECT_LOCAL_MACHINE;
        // SAFETY: valid input blob (only read by the call) and out blob; the
        // optional arguments are null.
        let ok = unsafe {
            if decrypt {
                CryptUnprotectData(
                    &input,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    flags,
                    &mut output,
                )
            } else {
                CryptProtectData(
                    &input,
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    flags,
                    &mut output,
                )
            }
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: on success the API allocated `cbData` bytes at `pbData`.
        let bytes = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) };
        let result = Zeroizing::new(bytes.to_vec());
        // SAFETY: the buffer is wiped (it may hold the key) and then freed once,
        // with LocalFree as the API requires.
        unsafe {
            std::ptr::write_bytes(output.pbData, 0, output.cbData as usize);
            LocalFree(output.pbData as _);
        }
        Ok(result)
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::io;

    use zeroize::Zeroizing;

    use super::{Keystore, run_with_input};

    /// Keychain item holding the key.
    const SERVICE: &str = "it.mensys.submarine.vault";
    /// Keychain of the system, available to root without a login session.
    const KEYCHAIN: &str = "/Library/Keychains/System.keychain";

    /// Stores the key, hex encoded, in the System keychain. The key file then
    /// only names the keystore.
    ///
    /// NB: the command goes through the stdin of `security -i`, so the key never
    /// shows in the process list as an argument would.
    pub fn seal(key: &[u8]) -> io::Result<(Keystore, Vec<u8>)> {
        let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
        let command = Zeroizing::new(format!(
            "add-generic-password -U -s {SERVICE} -a vault -w {hex} {KEYCHAIN}\n"
        ));
        run_with_input("security", &["-i"], command.as_bytes())?;
        Ok((Keystore::Keychain, Vec::new()))
    }

    /// Reads the key back from the System keychain.
    pub fn unseal(keystore: Keystore, _blob: &[u8]) -> io::Result<Zeroizing<Vec<u8>>> {
        if keystore != Keystore::Keychain {
            return Err(super::unsupported(keystore));
        }
        let out = run_with_input(
            "security",
            &[
                "find-generic-password",
                "-s",
                SERVICE,
                "-a",
                "vault",
                "-w",
                KEYCHAIN,
            ],
            &[],
        )?;
        let hex = std::str::from_utf8(&out)
            .map_err(|_| io::Error::other("keychain returned no text"))?
            .trim();
        (0..hex.len())
            .step_by(2)
            .map(|i| {
                hex.get(i..i + 2)
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
            })
            .collect::<Option<Vec<u8>>>()
            .map(Zeroizing::new)
            .ok_or_else(|| io::Error::other("keychain returned a malformed key"))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod platform {
    use std::io;

    use zeroize::Zeroizing;

    use super::Keystore;

    /// Other platforms have no keystore support.
    pub fn seal(_key: &[u8]) -> io::Result<(Keystore, Vec<u8>)> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }

    /// Other platforms have no keystore support.
    pub fn unseal(keystore: Keystore, _blob: &[u8]) -> io::Result<Zeroizing<Vec<u8>>> {
        Err(super::unsupported(keystore))
    }
}

/// Error for a key file made with a keystore this platform does not have.
fn unsupported(keystore: Keystore) -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        format!("keystore {} is not available here", keystore.name()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A vault with a fixed key, without touching any keystore.
    fn vault() -> Vault {
        Vault::with_key(&[7; KEY_LEN], Keystore::Plain)
    }

    // what is sealed comes back with the same id, and is not readable as it is
    #[test]
    fn seal_round_trip() {
        let v = vault();
        let sealed = v.seal(b"PrivateKey = secret", b"id1").unwrap();
        assert!(sealed.starts_with(MAGIC));
        assert!(!sealed.windows(6).any(|w| w == b"secret"));
        assert_eq!(
            v.unseal(&sealed, b"id1").unwrap().as_slice(),
            b"PrivateKey = secret"
        );
    }

    // another id, another key, a flipped byte or a plaintext file are all refused
    #[test]
    fn unseal_refuses_anything_else() {
        let v = vault();
        let sealed = v.seal(b"data", b"id1").unwrap();
        assert!(v.unseal(&sealed, b"id2").is_err());
        let other = Vault::with_key(&[8; KEY_LEN], Keystore::Plain);
        assert!(other.unseal(&sealed, b"id1").is_err());
        let mut tampered = sealed.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(v.unseal(&tampered, b"id1").is_err());
        assert!(v.unseal(b"{\"name\":\"x\"}", b"id1").is_err());
    }

    // two seals of the same data differ, thanks to the random nonce
    #[test]
    fn nonces_are_fresh() {
        let v = vault();
        assert_ne!(
            v.seal(b"data", b"id").unwrap(),
            v.seal(b"data", b"id").unwrap()
        );
    }

    // the key file is created once, then read back with the same key
    #[test]
    fn key_file_is_reused() {
        let dir = std::env::temp_dir().join(format!("submarine-vault-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = Vault::open(&dir).unwrap();
        let sealed = first.seal(b"data", b"id").unwrap();
        let second = Vault::open(&dir).unwrap();
        assert_eq!(second.keystore(), first.keystore());
        assert_eq!(second.unseal(&sealed, b"id").unwrap().as_slice(), b"data");
        std::fs::remove_dir_all(dir).unwrap();
    }

    // a damaged key file is an error, never silently replaced
    #[test]
    fn damaged_key_file_is_an_error() {
        assert!(unseal_key(b"plain\nnot base64!\n").is_err());
        assert!(unseal_key(b"unknown\nAAAA\n").is_err());
        assert!(unseal_key(format!("plain\n{}\n", BASE64.encode([1u8; 5])).as_bytes()).is_err());
    }
}

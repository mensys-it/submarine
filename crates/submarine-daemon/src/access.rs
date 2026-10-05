//! Who may use the service through the IPC socket.
//!
//! The socket itself is open to every user of the group that may talk to the daemon
//! (Linux), to every local user (macOS) or to every interactive user (Windows), and
//! any of them could route the traffic of the whole computer through a server of
//! their choice or turn the kill switch off. `access.json` in the data directory,
//! readable and writable only by root or SYSTEM, narrows that down: the installer
//! writes it with `submarine-daemon access only <user>` when Submarine is installed
//! for the current user only, or `access all` when it is installed for everybody.
//!
//! Root, SYSTEM and administrators running elevated are always allowed. Without the
//! file every user that can open the socket is allowed, as in older versions.

use std::collections::BTreeSet;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use submarine_ipc::Connection;

/// Name of the access file inside the data directory.
const FILE: &str = "access.json";

/// Content of the access file.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Access {
    /// Users allowed besides the privileged ones, as uid (Unix) or SID (Windows);
    /// `None` allows every user that can open the socket.
    pub allowed_users: Option<BTreeSet<String>>,
}

/// The user on the other side of a connection.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Peer {
    /// uid (Unix) or SID (Windows), in the form stored in [`Access`].
    pub id: String,
    /// Root, SYSTEM or an elevated administrator: always allowed.
    pub privileged: bool,
}

impl Access {
    /// Reads the access file of `data_dir`; a missing file allows everybody.
    ///
    /// # Errors
    /// Fails if the file cannot be read or is not valid.
    pub(crate) fn load(data_dir: &Path) -> io::Result<Self> {
        match std::fs::read(data_dir.join(FILE)) {
            Ok(data) => serde_json::from_slice(&data).map_err(io::Error::other),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(err),
        }
    }

    /// Writes the access file of `data_dir`, creating the directory if needed.
    ///
    /// # Errors
    /// Fails if the directory cannot be prepared or the file written, e.g. when not
    /// running as root or administrator.
    pub(crate) fn save(&self, data_dir: &Path) -> io::Result<()> {
        crate::datadir::prepare(data_dir)?;
        let json = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        crate::store::write_private(&data_dir.join(FILE), &json)
    }

    /// Whether `peer` may use the service.
    pub(crate) fn allows(&self, peer: &Peer) -> bool {
        peer.privileged
            || self
                .allowed_users
                .as_ref()
                .is_none_or(|users| users.contains(&peer.id))
    }
}

/// Checks that the user on the other side of `conn` may use the service.
/// NB: an access file that cannot be read lets ONLY the privileged users in, as it
/// can only be broken by somebody with root or administrator rights.
///
/// # Errors
/// The reason of the refusal, for the service log.
pub(crate) fn check(conn: &Connection, data_dir: &Path) -> Result<(), String> {
    let peer = peer(conn).map_err(|err| format!("cannot identify the client: {err}"))?;
    let access = match Access::load(data_dir) {
        Ok(access) => access,
        Err(err) => {
            tracing::error!("cannot read {FILE}: {err}");
            Access {
                allowed_users: Some(BTreeSet::new()),
            }
        }
    };
    if access.allows(&peer) {
        Ok(())
    } else {
        Err(format!("user {} is not allowed", peer.id))
    }
}

/// Identifies the user on the other side of `conn` from the credentials the kernel
/// recorded when it connected.
#[cfg(unix)]
fn peer(conn: &Connection) -> io::Result<Peer> {
    use interprocess::local_socket::traits::StreamCommon;

    let uid = conn
        .peer_creds()?
        .euid()
        .ok_or_else(|| io::Error::other("no user id"))?;
    Ok(Peer {
        id: uid.to_string(),
        privileged: uid == 0,
    })
}

/// Identifies the user on the other side of `conn` from the token of the client
/// process.
#[cfg(windows)]
fn peer(conn: &Connection) -> io::Result<Peer> {
    use interprocess::local_socket::traits::StreamCommon;

    let pid = conn
        .peer_creds()?
        .pid()
        .ok_or_else(|| io::Error::other("no process id"))?;
    windows::process_user(pid)
}

/// Resolves a user name, or a uid (Unix) / SID (Windows) given as is, to the form
/// stored in [`Access`].
///
/// # Errors
/// Fails if the user does not exist.
#[cfg(unix)]
pub(crate) fn resolve_user(name: &str) -> io::Result<String> {
    if !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(name.to_owned());
    }
    let c_name = std::ffi::CString::new(name)?;
    // SAFETY: getpwnam receives a valid C string; the result is only read immediately,
    // by a single-threaded command.
    let uid = unsafe {
        let entry = libc::getpwnam(c_name.as_ptr());
        (!entry.is_null()).then(|| (*entry).pw_uid)
    };
    uid.map(|uid| uid.to_string())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("no such user: {name}")))
}

/// Resolves a user name, or a uid (Unix) / SID (Windows) given as is, to the form
/// stored in [`Access`].
///
/// # Errors
/// Fails if the user does not exist.
#[cfg(windows)]
pub(crate) fn resolve_user(name: &str) -> io::Result<String> {
    windows::account_sid(name)
}

/// `submarine-daemon access [all | only <user>...]`: shows or sets who may use the
/// service.
///
/// # Errors
/// Fails on wrong arguments, unknown users, or if the access file cannot be read or
/// written.
pub(crate) fn command(args: &[String], data_dir: &Path) -> io::Result<()> {
    let usage = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: submarine-daemon access [all | only <user>...]",
        )
    };
    let access = match args.first().map(String::as_str) {
        None => {
            match Access::load(data_dir)?.allowed_users {
                None => println!("every user may use the service"),
                Some(users) => {
                    println!("only administrators and these users may use the service:");
                    for user in users {
                        println!("  {user}");
                    }
                }
            }
            return Ok(());
        }
        Some("all") if args.len() == 1 => Access::default(),
        Some("only") if args.len() > 1 => Access {
            allowed_users: Some(
                args[1..]
                    .iter()
                    .map(|name| resolve_user(name))
                    .collect::<io::Result<_>>()?,
            ),
        },
        _ => return Err(usage()),
    };
    access.save(data_dir)?;
    println!("access updated; it applies to new connections");
    Ok(())
}

#[cfg(windows)]
mod windows {
    use std::io;
    use std::ptr;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSidToSidW,
    };
    use windows_sys::Win32::Security::{
        GetTokenInformation, IsWellKnownSid, LookupAccountNameW, PSID, SID_AND_ATTRIBUTES,
        SID_NAME_USE, TOKEN_GROUPS, TOKEN_INFORMATION_CLASS, TOKEN_QUERY, TOKEN_USER, TokenGroups,
        TokenUser, WinBuiltinAdministratorsSid, WinLocalSystemSid,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    use super::Peer;

    /// Attribute of a token group that is enabled. A non-elevated administrator has
    /// the Administrators group only for deny, without it.
    const SE_GROUP_ENABLED: u32 = 0x4;

    /// Handle closed on drop.
    struct Handle(HANDLE);

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: a valid handle, closed only here.
            unsafe { CloseHandle(self.0) };
        }
    }

    /// User of the process `pid`, and whether it is SYSTEM or an elevated
    /// administrator.
    pub(super) fn process_user(pid: u32) -> io::Result<Peer> {
        // SAFETY: plain call; the handle is closed by `Handle`.
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if process.is_null() {
            return Err(io::Error::last_os_error());
        }
        let process = Handle(process);
        let mut token: HANDLE = ptr::null_mut();
        // SAFETY: valid process handle and out pointer.
        if unsafe { OpenProcessToken(process.0, TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let token = Handle(token);

        // the user of the token, as a SID string
        let user = token_information(&token, TokenUser)?;
        // SAFETY: the buffer holds a TOKEN_USER, written by GetTokenInformation.
        let user_sid = unsafe { (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        let id = sid_string(user_sid)?;
        // SAFETY: `user_sid` points into `user`, still alive.
        let system = unsafe { IsWellKnownSid(user_sid, WinLocalSystemSid) } != 0;

        // an enabled Administrators group, i.e. an elevated administrator
        let groups = token_information(&token, TokenGroups)?;
        let groups = groups.as_ptr().cast::<TOKEN_GROUPS>();
        // SAFETY: the buffer holds a TOKEN_GROUPS with `GroupCount` entries, written by
        // GetTokenInformation; the pointer to them is taken without going through the
        // one-element array type.
        let entries: &[SID_AND_ATTRIBUTES] = unsafe {
            std::slice::from_raw_parts(
                (&raw const (*groups).Groups).cast(),
                (*groups).GroupCount as usize,
            )
        };
        let admin = entries.iter().any(|group| {
            // SAFETY: every entry holds a valid SID inside the buffer.
            group.Attributes & SE_GROUP_ENABLED != 0
                && unsafe { IsWellKnownSid(group.Sid, WinBuiltinAdministratorsSid) } != 0
        });
        Ok(Peer {
            id,
            privileged: system || admin,
        })
    }

    /// Information of class `class` about `token`, in a buffer aligned for the
    /// structures it holds.
    fn token_information(token: &Handle, class: TOKEN_INFORMATION_CLASS) -> io::Result<Vec<u64>> {
        // size query first, then the actual read
        let mut size = 0u32;
        // SAFETY: valid token; a null buffer of size 0 only returns the needed size.
        unsafe { GetTokenInformation(token.0, class, ptr::null_mut(), 0, &mut size) };
        let mut buf = vec![0u64; (size as usize).div_ceil(8)];
        // SAFETY: valid token and a buffer of at least `size` bytes.
        let ok = unsafe {
            GetTokenInformation(token.0, class, buf.as_mut_ptr().cast(), size, &mut size)
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(buf)
    }

    /// String form (`S-1-5-...`) of a SID.
    fn sid_string(sid: PSID) -> io::Result<String> {
        let mut text: *mut u16 = ptr::null_mut();
        // SAFETY: valid SID and out pointer; the string is freed below.
        if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: NUL-terminated string allocated by ConvertSidToStringSidW.
        let len = unsafe { (0..).take_while(|&i| *text.add(i) != 0).count() };
        // SAFETY: `len` characters were just counted.
        let result = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, len) });
        // SAFETY: allocated by ConvertSidToStringSidW and freed exactly once.
        unsafe { LocalFree(text.cast()) };
        Ok(result)
    }

    /// SID of the account `name`, or `name` itself when it is already a valid SID.
    pub(super) fn account_sid(name: &str) -> io::Result<String> {
        let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
        // a SID given as is, checked by converting it
        if name.starts_with("S-1-") {
            let mut sid: PSID = ptr::null_mut();
            // SAFETY: valid NUL-terminated string and out pointer.
            if unsafe { ConvertStringSidToSidW(wide.as_ptr(), &mut sid) } == 0 {
                return Err(io::Error::last_os_error());
            }
            let result = sid_string(sid);
            // SAFETY: allocated by ConvertStringSidToSidW and freed exactly once.
            unsafe { LocalFree(sid) };
            return result;
        }
        // lookup of the account: size query first, then the actual lookup
        let (mut sid_size, mut domain_size) = (0u32, 0u32);
        let mut kind: SID_NAME_USE = 0;
        // SAFETY: valid name; null buffers of size 0 only return the needed sizes.
        unsafe {
            LookupAccountNameW(
                ptr::null(),
                wide.as_ptr(),
                ptr::null_mut(),
                &mut sid_size,
                ptr::null_mut(),
                &mut domain_size,
                &mut kind,
            )
        };
        let mut sid = vec![0u64; (sid_size as usize).div_ceil(8)];
        let mut domain = vec![0u16; domain_size as usize];
        // SAFETY: valid name and buffers of the sizes just returned.
        let ok = unsafe {
            LookupAccountNameW(
                ptr::null(),
                wide.as_ptr(),
                sid.as_mut_ptr().cast(),
                &mut sid_size,
                domain.as_mut_ptr(),
                &mut domain_size,
                &mut kind,
            )
        };
        if ok == 0 {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("no such user: {name} ({})", io::Error::last_os_error()),
            ));
        }
        sid_string(sid.as_mut_ptr().cast())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A peer that is not privileged.
    fn user(id: &str) -> Peer {
        Peer {
            id: id.into(),
            privileged: false,
        }
    }

    // without an access file every user is allowed
    #[test]
    fn everybody_by_default() {
        assert!(Access::default().allows(&user("1000")));
    }

    // with a list, only its users and the privileged ones are allowed
    #[test]
    fn only_listed_users() {
        let access = Access {
            allowed_users: Some(["1000".to_owned()].into()),
        };
        assert!(access.allows(&user("1000")));
        assert!(!access.allows(&user("1001")));
        assert!(access.allows(&Peer {
            id: "0".into(),
            privileged: true,
        }));
    }

    // the file round trips, and a missing file means everybody
    #[test]
    fn load_and_save() {
        let dir = std::env::temp_dir().join(format!("submarine-access-{}", uuid::Uuid::new_v4()));
        assert_eq!(Access::load(&dir).unwrap(), Access::default());
        let access = Access {
            allowed_users: Some(["1000".to_owned(), "1001".to_owned()].into()),
        };
        access.save(&dir).unwrap();
        assert_eq!(Access::load(&dir).unwrap(), access);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // the uid of a real client is checked against the list
    #[cfg(unix)]
    #[tokio::test]
    async fn checks_the_uid_of_the_client() {
        use interprocess::local_socket::traits::tokio::Listener as _;

        let dir = std::env::temp_dir().join(format!("submarine-access-{}", uuid::Uuid::new_v4()));
        let socket = dir.join("daemon.sock").to_string_lossy().into_owned();
        let listener = submarine_ipc::bind(&socket).unwrap();
        // SAFETY: plain call without arguments.
        let uid = unsafe { libc::geteuid() }.to_string();
        for (users, allowed) in [
            (vec![uid.clone()], true),
            (vec!["4242".to_owned()], uid == "0"),
        ] {
            Access {
                allowed_users: Some(users.into_iter().collect()),
            }
            .save(&dir)
            .unwrap();
            let (conn, _client) =
                tokio::join!(listener.accept(), submarine_ipc::Client::connect(&socket));
            assert_eq!(check(&conn.unwrap(), &dir).is_ok(), allowed);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // user names resolve to uids, and uids are kept as they are
    #[cfg(unix)]
    #[test]
    fn resolves_unix_users() {
        assert_eq!(resolve_user("root").unwrap(), "0");
        assert_eq!(resolve_user("1000").unwrap(), "1000");
        assert!(resolve_user("no-such-user-submarine").is_err());
    }
}

//! Local socket transport: endpoint naming, listener creation and newline-delimited
//! JSON framing.

use std::io;

#[cfg(not(windows))]
use interprocess::local_socket::GenericFilePath;
#[cfg(windows)]
use interprocess::local_socket::GenericNamespaced;
use interprocess::local_socket::tokio::prelude::*;
use interprocess::local_socket::{ListenerOptions, Name};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// A connected local socket (either side).
pub type Connection = interprocess::local_socket::tokio::Stream;
/// The daemon's listening local socket.
pub type Listener = interprocess::local_socket::tokio::Listener;

/// Upper bound for one message, so a misbehaving peer cannot exhaust memory.
pub const MAX_MESSAGE_LEN: u64 = 1024 * 1024;

/// Registry key, under `HKEY_LOCAL_MACHINE`, where the Windows service publishes the
/// name of its pipe: every user can read it, ONLY administrators can write it.
#[cfg(windows)]
const PIPE_KEY: &str = r"SOFTWARE\Submarine";
/// Registry value holding the pipe name.
#[cfg(windows)]
const PIPE_VALUE: &str = "Pipe";

/// Socket path (Unix) or pipe name (Windows). `SUBMARINE_SOCKET` overrides it.
/// On Windows it is the name published by the service (see [`publish_pipe_name`]),
/// or `submarine` when none is.
pub fn socket_path() -> String {
    if let Ok(path) = std::env::var("SUBMARINE_SOCKET") {
        return path;
    }
    if cfg!(windows) {
        #[cfg(windows)]
        if let Some(name) = published_pipe_name() {
            return name;
        }
        "submarine".into()
    } else if cfg!(target_os = "macos") {
        "/var/run/submarine/daemon.sock".into()
    } else {
        "/run/submarine/daemon.sock".into()
    }
}

/// Publishes `name` as the pipe of the service, for [`socket_path`].
/// NB: the service picks a new random name at every start. A pipe created in advance
/// under a fixed name by another user would otherwise keep the service from starting.
///
/// # Errors
/// Fails if the registry cannot be written, e.g. when not running as administrator.
#[cfg(windows)]
pub fn publish_pipe_name(name: &str) -> io::Result<()> {
    use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, REG_SZ, RegSetKeyValueW};

    let (key, value) = (wide(PIPE_KEY), wide(PIPE_VALUE));
    let data = wide(name);
    // SAFETY: valid NUL-terminated strings; the size includes the terminator.
    let code = unsafe {
        RegSetKeyValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            REG_SZ,
            data.as_ptr().cast(),
            (data.len() * 2) as u32,
        )
    };
    match code {
        0 => Ok(()),
        code => Err(io::Error::from_raw_os_error(code as i32)),
    }
}

/// Removes the published pipe name, when the service is uninstalled.
#[cfg(windows)]
pub fn unpublish_pipe_name() {
    use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RegDeleteKeyValueW};

    let (key, value) = (wide(PIPE_KEY), wide(PIPE_VALUE));
    // SAFETY: valid NUL-terminated strings.
    unsafe { RegDeleteKeyValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), value.as_ptr()) };
}

/// The pipe name published by the service, if any. Only letters, digits and `-` are
/// accepted, so the value can never point to a remote pipe.
#[cfg(windows)]
fn published_pipe_name() -> Option<String> {
    use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};

    let (key, value) = (wide(PIPE_KEY), wide(PIPE_VALUE));
    let mut buf = [0u16; 128];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY: valid NUL-terminated strings, and a buffer of `size` bytes.
    let code = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if code != 0 {
        return None;
    }
    // the size includes the terminator
    let len = (size as usize / 2).saturating_sub(1);
    let name = String::from_utf16(&buf[..len]).ok()?;
    let valid = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    valid.then_some(name)
}

/// NUL-terminated UTF-16 form of `text`.
#[cfg(windows)]
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

/// Converts the path into a socket name: a namespaced pipe name on Windows, a file
/// system path elsewhere.
fn to_name(path: &str) -> io::Result<Name<'_>> {
    #[cfg(windows)]
    return path.to_ns_name::<GenericNamespaced>();
    #[cfg(not(windows))]
    return path.to_fs_name::<GenericFilePath>();
}

/// Creates the daemon's listening socket, replacing a stale one.
/// On Unix the parent directory is created if needed; on Windows the pipe gets an ACL
/// that lets interactive users connect, but not create instances of their own.
pub fn bind(path: &str) -> io::Result<Listener> {
    // the runtime directory may not exist yet (e.g. after a reboot on a tmpfs)
    #[cfg(unix)]
    if let Some(dir) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(dir)?;
    }
    let options = ListenerOptions::new()
        .name(to_name(path)?)
        .try_overwrite(true);
    #[cfg(windows)]
    let options = {
        use interprocess::os::windows::local_socket::ListenerOptionsExt;
        use interprocess::os::windows::security_descriptor::SecurityDescriptor;
        // SYSTEM and Administrators: full access; interactive (logged-on) users: only
        // the rights clients request, i.e. they can talk to the service; nobody else
        // NB: interactive users MUST NOT get GENERIC_WRITE. On a pipe it includes
        // FILE_CREATE_PIPE_INSTANCE, which would let any of them add rogue server
        // instances and intercept the other users' clients.
        let sddl = format!(
            "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;{:#x};;;IU)",
            crate::winpipe::CLIENT_ACCESS
        );
        let sddl = widestring::U16CString::from_str(sddl).map_err(io::Error::other)?;
        options.security_descriptor(SecurityDescriptor::deserialize(&sddl)?)
    };
    options.create_tokio()
}

/// Connects to the daemon's socket.
/// On Windows the pipe is opened by [`crate::winpipe`], which requests only the rights
/// granted by the pipe ACL and refuses a pipe not created by the service.
pub async fn connect(path: &str) -> io::Result<Connection> {
    #[cfg(windows)]
    {
        use interprocess::os::windows::named_pipe::local_socket::tokio::Stream as PipeStream;

        let name = path.to_owned();
        let handle = tokio::task::spawn_blocking(move || crate::winpipe::open(&name))
            .await
            .map_err(io::Error::other)??;
        let stream = PipeStream::try_from(handle)?;
        Ok(Connection::from(stream))
    }
    #[cfg(not(windows))]
    Connection::connect(to_name(path)?).await
}

/// Reads one message; `Ok(None)` means the peer closed the connection.
/// `buf` is a reusable line buffer, cleared on every call.
///
/// # Errors
/// [`io::ErrorKind::InvalidData`] if the message exceeds [`MAX_MESSAGE_LEN`], is cut by the
/// end of the stream or is not valid JSON for `T`.
pub async fn read_message<T, R>(reader: &mut R, buf: &mut String) -> io::Result<Option<T>>
where
    T: DeserializeOwned,
    R: AsyncBufRead + Unpin,
{
    buf.clear();
    // the read is capped, so a line without a newline cannot grow without bound
    let len = reader.take(MAX_MESSAGE_LEN).read_line(buf).await?;
    if len == 0 {
        return Ok(None);
    }
    // a complete message always ends with the newline delimiter
    if !buf.ends_with('\n') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "message too long or truncated",
        ));
    }
    serde_json::from_str(buf)
        .map(Some)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

/// Writes one message as a single JSON line and flushes it.
pub async fn write_message<T, W>(writer: &mut W, message: &T) -> io::Result<()>
where
    T: Serialize,
    W: AsyncWrite + Unpin,
{
    let mut line = serde_json::to_vec(message)?;
    line.push(b'\n');
    writer.write_all(&line).await?;
    writer.flush().await
}

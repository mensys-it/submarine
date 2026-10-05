//! Client side of the Windows named pipe: the pipe is opened with the least access
//! rights, and its owner is checked before anything is sent to it.
//!
//! The daemon's pipe ACL (see [`crate::bind`]) does NOT grant interactive users
//! `FILE_CREATE_PIPE_INSTANCE`, so they cannot add rogue server instances to the
//! daemon's pipe and intercept other users' requests (e.g. the private keys of an
//! imported tunnel). That right is part of `GENERIC_WRITE`, which interprocess's own
//! connect requests, so the pipe is opened here instead.
//!
//! A pipe created by another user while the service is not running has that user as
//! owner, while the daemon's pipe is owned by SYSTEM or Administrators, which a
//! standard user cannot assign: the owner check rejects the former.

use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    ERROR_PIPE_BUSY, ERROR_SEM_TIMEOUT, INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_KERNEL_OBJECT};
use windows_sys::Win32::Security::{
    IsWellKnownSid, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    WinBuiltinAdministratorsSid, WinLocalSystemSid,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, FILE_GENERIC_READ, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FILE_WRITE_DATA, OPEN_EXISTING, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
};
use windows_sys::Win32::System::Pipes::WaitNamedPipeW;

/// Access rights requested by clients and granted to interactive users by the pipe
/// ACL: read (which includes `READ_CONTROL`, needed by the owner check) and write of
/// data, but NOT append, which on a pipe means creation of new instances.
pub(crate) const CLIENT_ACCESS: u32 = FILE_GENERIC_READ | FILE_WRITE_DATA;

/// How long a client waits for a free pipe instance before giving up.
const BUSY_TIMEOUT: Duration = Duration::from_secs(10);

/// Opens the pipe `name` (`\\.\pipe\` is prepended, as interprocess does for
/// namespaced names) and checks that it belongs to the Submarine service.
/// Blocking: meant to run on a blocking thread.
///
/// # Errors
/// [`io::ErrorKind::PermissionDenied`] if the pipe is owned by an untrusted user,
/// [`io::ErrorKind::TimedOut`] if no instance became free within [`BUSY_TIMEOUT`],
/// otherwise the OS error of the open (e.g. not found when the service is down).
pub(crate) fn open(name: &str) -> io::Result<OwnedHandle> {
    let path: Vec<u16> = format!(r"\\.\pipe\{name}")
        .encode_utf16()
        .chain([0])
        .collect();
    let deadline = Instant::now() + BUSY_TIMEOUT;
    let handle = loop {
        // identification level only: a server can tell who the client is, but it
        // cannot impersonate it
        // SAFETY: valid NUL-terminated path; no security attributes nor template file.
        let raw = unsafe {
            CreateFileW(
                path.as_ptr(),
                CLIENT_ACCESS,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                ptr::null_mut(),
            )
        };
        if raw != INVALID_HANDLE_VALUE {
            // SAFETY: valid handle just opened, owned by nobody else.
            break unsafe { OwnedHandle::from_raw_handle(raw) };
        }
        let err = io::Error::last_os_error();
        if err.raw_os_error() != Some(ERROR_PIPE_BUSY as i32) {
            return Err(err);
        }
        // every instance is serving another client: wait for one to become free
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "every pipe instance is busy",
            ));
        }
        let millis = u32::try_from(left.as_millis()).unwrap_or(u32::MAX).max(1);
        // SAFETY: valid NUL-terminated path.
        if unsafe { WaitNamedPipeW(path.as_ptr(), millis) } == 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() != Some(ERROR_SEM_TIMEOUT as i32) {
                return Err(err);
            }
        }
    };
    // refusal of a pipe not created by the service, before anything is sent to it
    check_owner(&handle)?;
    Ok(handle)
}

/// Fails unless the pipe is owned by SYSTEM or Administrators: the owner of a pipe is
/// its creator, and only those accounts can run the daemon.
fn check_owner(handle: &OwnedHandle) -> io::Result<()> {
    let mut owner: PSID = ptr::null_mut();
    let mut sd: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: valid handle opened with READ_CONTROL; the out pointers are valid and
    // `owner` points into `sd`, freed below.
    let code = unsafe {
        GetSecurityInfo(
            handle.as_raw_handle(),
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut sd,
        )
    };
    if code != 0 {
        return Err(io::Error::from_raw_os_error(code as i32));
    }
    // SAFETY: `owner` is a valid SID inside `sd`, which is still allocated.
    let trusted = unsafe {
        IsWellKnownSid(owner, WinLocalSystemSid) != 0
            || IsWellKnownSid(owner, WinBuiltinAdministratorsSid) != 0
    };
    // SAFETY: allocated by GetSecurityInfo and freed exactly once; `owner` is not
    // used afterwards.
    unsafe { LocalFree(sd) };
    if trusted {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the pipe does not belong to the Submarine service",
        ))
    }
}

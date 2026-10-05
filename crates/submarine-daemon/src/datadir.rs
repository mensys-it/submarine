//! Preparation of the data directory and its subdirectories, which hold the private
//! keys: created if missing and restricted to the daemon's user.
//!
//! On Windows the directory lives in `ProgramData`, where by default any user can
//! create folders and owns them. A user who creates the data directory before the
//! service keeps the owner's implicit right to change its ACL, so restricting the ACL
//! afterwards would not keep the keys from them, and links planted inside could
//! redirect the files the service writes as SYSTEM. Such a directory is moved aside,
//! and a new one is created with the restricted ACL from the start.

use std::io;
use std::path::{Path, PathBuf};

/// Makes `dir` a directory accessible only to the daemon's user, creating it and its
/// parents if missing.
///
/// Returns where an existing `dir` that could not be trusted was moved (Windows
/// only), for the caller to log.
///
/// # Errors
/// Fails if the directory cannot be created, moved aside or restricted.
#[cfg(unix)]
pub(crate) fn prepare(dir: &Path) -> io::Result<Option<PathBuf>> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    Ok(None)
}

/// Makes `dir` a directory accessible only to the daemon's user, creating it and its
/// parents if missing.
///
/// Returns where an existing `dir` that could not be trusted was moved (Windows
/// only), for the caller to log.
///
/// # Errors
/// Fails if the directory cannot be created, moved aside or restricted.
#[cfg(windows)]
pub(crate) fn prepare(dir: &Path) -> io::Result<Option<PathBuf>> {
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut moved = None;
    // a few rounds, in case another user recreates the directory in the meantime
    for _ in 0..3 {
        match windows::is_trusted(dir) {
            // the ACL is set again, in case an older version left it looser
            Ok(true) => {
                windows::restrict(dir)?;
                return Ok(moved);
            }
            Ok(false) => {
                let aside = aside_path(dir);
                windows::move_entry(dir, &aside)?;
                moved = Some(aside);
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
        match windows::create_restricted(dir) {
            Ok(()) => return Ok(moved),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::other(format!(
        "{} keeps being recreated by another user",
        dir.display()
    )))
}

/// Name for an untrusted `dir` moved aside: `<dir>.untrusted-<unix seconds>`.
#[cfg(windows)]
fn aside_path(dir: &Path) -> PathBuf {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut name = dir.file_name().unwrap_or_default().to_owned();
    name.push(format!(".untrusted-{secs}"));
    dir.with_file_name(name)
}

#[cfg(windows)]
mod windows {
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::path::Path;
    use std::ptr;

    use windows_sys::Win32::Foundation::{
        ERROR_NOT_ALL_ASSIGNED, GetLastError, HANDLE, INVALID_HANDLE_VALUE, LUID, LocalFree,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1,
        SE_FILE_OBJECT, SetNamedSecurityInfoW,
    };
    use windows_sys::Win32::Security::{
        ACL, AdjustTokenPrivileges, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl,
        IsWellKnownSid, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, OWNER_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SE_PRIVILEGE_ENABLED,
        SE_RESTORE_NAME, SECURITY_ATTRIBUTES, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES,
        TOKEN_QUERY, WinBuiltinAdministratorsSid, WinLocalSystemSid,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, CreateDirectoryW, CreateFileW, DELETE,
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_RENAME_INFO, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, FileRenameInfo, GetFileInformationByHandle, MoveFileExW,
        OPEN_EXISTING, READ_CONTROL, SYNCHRONIZE, SetFileInformationByHandle,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// Protected DACL (parent ACEs not inherited) granting full access, inherited by
    /// files and subdirectories, only to SYSTEM (SY) and Administrators (BA).
    const SDDL: &str = "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)";

    /// NUL-terminated UTF-16 form of a path or string.
    fn wide(text: &std::ffi::OsStr) -> Vec<u16> {
        text.encode_wide().chain([0]).collect()
    }

    /// Security descriptor built from [`SDDL`], freed on drop.
    struct Descriptor(PSECURITY_DESCRIPTOR);

    impl Descriptor {
        fn new() -> io::Result<Self> {
            let sddl = wide(SDDL.as_ref());
            let mut sd: PSECURITY_DESCRIPTOR = ptr::null_mut();
            // SAFETY: valid NUL-terminated SDDL and out pointer.
            let ok = unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    SDDL_REVISION_1,
                    &mut sd,
                    ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self(sd))
        }
    }

    impl Drop for Descriptor {
        fn drop(&mut self) {
            // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW
            // and freed only here.
            unsafe { LocalFree(self.0) };
        }
    }

    /// Whether `dir` is a real directory (not a link nor a file) owned by SYSTEM or
    /// Administrators, which a standard user cannot assign as owner.
    ///
    /// A directory that cannot be opened is not trusted either.
    ///
    /// # Errors
    /// [`io::ErrorKind::NotFound`] if nothing exists at `dir`.
    pub(super) fn is_trusted(dir: &Path) -> io::Result<bool> {
        // the entry itself is opened, without following a link
        let path = wide(dir.as_os_str());
        // SAFETY: valid NUL-terminated path; no security attributes nor template file.
        let raw = unsafe {
            CreateFileW(
                path.as_ptr(),
                READ_CONTROL | FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                ptr::null_mut(),
            )
        };
        if raw == INVALID_HANDLE_VALUE {
            let err = io::Error::last_os_error();
            // NB: the service's own directory always grants it full access: one that
            // it cannot open was created by somebody else, who may deny the service
            // access on purpose to keep it from starting
            return match err.kind() {
                io::ErrorKind::PermissionDenied => Ok(false),
                _ => Err(err),
            };
        }
        // SAFETY: valid handle just opened, owned by nobody else.
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };

        // a link or a file in place of the directory
        // SAFETY: plain C struct, for which all zeroes is a valid value.
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: valid handle and out pointer.
        if unsafe { GetFileInformationByHandle(handle.as_raw_handle(), &mut info) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let attributes = info.dwFileAttributes;
        if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || attributes & FILE_ATTRIBUTE_DIRECTORY == 0
        {
            return Ok(false);
        }

        // owner of the directory
        let mut owner: PSID = ptr::null_mut();
        let mut sd: PSECURITY_DESCRIPTOR = ptr::null_mut();
        // SAFETY: valid handle opened with READ_CONTROL; the out pointers are valid and
        // `owner` points into `sd`, freed below.
        let code = unsafe {
            GetSecurityInfo(
                handle.as_raw_handle(),
                SE_FILE_OBJECT,
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
        Ok(trusted)
    }

    /// Creates `dir` with the restricted ACL already in place, so that nobody else
    /// can open it, even for an instant.
    ///
    /// # Errors
    /// [`io::ErrorKind::AlreadyExists`] if something already exists at `dir`.
    pub(super) fn create_restricted(dir: &Path) -> io::Result<()> {
        let descriptor = Descriptor::new()?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: 0,
        };
        let path = wide(dir.as_os_str());
        // SAFETY: valid NUL-terminated path and security attributes, which outlive
        // the call.
        if unsafe { CreateDirectoryW(path.as_ptr(), &attributes) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Renames the entry at `from` to `to`. A link is renamed itself, not followed.
    /// NB: another user may deny this service any access to a directory they created,
    /// to keep the service from starting. Such an entry is renamed with the restore
    /// privilege, which grants the right to delete, and so to rename, whatever the ACL.
    pub(super) fn move_entry(from: &Path, to: &Path) -> io::Result<()> {
        let (wide_from, wide_to) = (wide(from.as_os_str()), wide(to.as_os_str()));
        // SAFETY: valid NUL-terminated paths; no flags, so nothing is replaced.
        if unsafe { MoveFileExW(wide_from.as_ptr(), wide_to.as_ptr(), 0) } != 0 {
            return Ok(());
        }
        let err = io::Error::last_os_error();
        if err.kind() != io::ErrorKind::PermissionDenied {
            return Err(err);
        }

        // the entry itself opened for deletion, with the privilege enabled only meanwhile
        let _privilege = RestorePrivilege::enable()?;
        // SAFETY: valid NUL-terminated path; no security attributes nor template file.
        let raw = unsafe {
            CreateFileW(
                wide_from.as_ptr(),
                DELETE | SYNCHRONIZE,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                ptr::null_mut(),
            )
        };
        if raw == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: valid handle just opened, owned by nobody else.
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        rename_handle(&handle, to)
    }

    /// Renames the open entry `handle` to the full path `to`, without replacing an
    /// existing entry.
    fn rename_handle(handle: &OwnedHandle, to: &Path) -> io::Result<()> {
        // the new name as a verbatim path, which the kernel takes as it is
        let mut name: Vec<u16> = r"\\?\".encode_utf16().collect();
        name.extend(to.as_os_str().encode_wide());
        let name_bytes = name.len() * 2;
        // FILE_RENAME_INFO followed by the name, in a buffer aligned for it
        let size = size_of::<FILE_RENAME_INFO>() + name_bytes;
        let mut buf = vec![0u64; size.div_ceil(8)];
        let info = buf.as_mut_ptr().cast::<FILE_RENAME_INFO>();
        // SAFETY: the buffer is zeroed (no replace, no root directory) and large enough
        // for the header and the name; the name is written through a raw pointer, as
        // the field is declared with a single element.
        unsafe {
            (&raw mut (*info).FileNameLength).write(name_bytes as u32);
            ptr::copy_nonoverlapping(
                name.as_ptr(),
                (&raw mut (*info).FileName).cast::<u16>(),
                name.len(),
            );
        }
        // SAFETY: valid handle opened with DELETE, and a buffer of `size` bytes.
        let ok = unsafe {
            SetFileInformationByHandle(
                handle.as_raw_handle(),
                FileRenameInfo,
                info.cast(),
                size as u32,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// The restore privilege of this process, enabled until drop.
    struct RestorePrivilege {
        token: OwnedHandle,
        luid: LUID,
    }

    impl RestorePrivilege {
        /// Enables the privilege, which SYSTEM and administrators hold but keep
        /// disabled.
        fn enable() -> io::Result<Self> {
            let mut token: HANDLE = ptr::null_mut();
            // SAFETY: pseudo handle of this process and a valid out pointer.
            let ok = unsafe {
                OpenProcessToken(
                    GetCurrentProcess(),
                    TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
                    &mut token,
                )
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: valid handle just opened, owned by nobody else.
            let token = unsafe { OwnedHandle::from_raw_handle(token) };
            let mut luid = LUID::default();
            // SAFETY: valid privilege name and out pointer.
            if unsafe { LookupPrivilegeValueW(ptr::null(), SE_RESTORE_NAME, &mut luid) } == 0 {
                return Err(io::Error::last_os_error());
            }
            let privilege = Self { token, luid };
            privilege.set(SE_PRIVILEGE_ENABLED)?;
            Ok(privilege)
        }

        /// Sets the attributes of the privilege in the token.
        fn set(&self, attributes: u32) -> io::Result<()> {
            let state = TOKEN_PRIVILEGES {
                PrivilegeCount: 1,
                Privileges: [LUID_AND_ATTRIBUTES {
                    Luid: self.luid,
                    Attributes: attributes,
                }],
            };
            // SAFETY: valid token opened with TOKEN_ADJUST_PRIVILEGES and new state; the
            // previous state is not needed.
            let ok = unsafe {
                AdjustTokenPrivileges(
                    self.token.as_raw_handle(),
                    0,
                    &state,
                    0,
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            };
            // the call succeeds even when the token does not hold the privilege
            // SAFETY: plain call.
            if ok == 0 || unsafe { GetLastError() } == ERROR_NOT_ALL_ASSIGNED {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
    }

    impl Drop for RestorePrivilege {
        fn drop(&mut self) {
            let _ = self.set(0);
        }
    }

    /// Sets the restricted ACL on `dir`; owner, group and SACL are left unchanged.
    pub(super) fn restrict(dir: &Path) -> io::Result<()> {
        let descriptor = Descriptor::new()?;
        // extraction of the DACL from the descriptor and its application to the path
        let (mut present, mut defaulted) = (0, 0);
        let mut dacl: *mut ACL = ptr::null_mut();
        // SAFETY: `descriptor` is valid; out pointers are valid.
        if unsafe {
            GetSecurityDescriptorDacl(descriptor.0, &mut present, &mut dacl, &mut defaulted)
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let path = wide(dir.as_os_str());
        // SAFETY: valid path and DACL, which lives in `descriptor` until the end.
        let code = unsafe {
            SetNamedSecurityInfoW(
                path.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                dacl,
                ptr::null(),
            )
        };
        if code != 0 {
            return Err(io::Error::from_raw_os_error(code as i32));
        }
        Ok(())
    }
}

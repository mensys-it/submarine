//! Per-app split tunneling: processes of the chosen executables are moved into
//! a `net_cls` cgroup whose class id the firewall matches with `meta cgroup`.
//!
//! `net_cls` is a cgroup v1 controller; on unified (v2-only) systems we mount
//! a private v1 hierarchy for it. Children inherit the cgroup, so anything an
//! app starts follows the same route.
//!
//! Processes are found by polling `/proc`: a newly started app may send its
//! first packets through the default route before it is picked up.

use std::collections::HashSet;
use std::ffi::CString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::task::JoinHandle;

use crate::{Result, SplitMode, TunnelNetConfig};

/// `net_cls` class id of our cgroup, matched by the firewall with `meta cgroup`.
pub const CLASSID: u32 = 0x5375_0001;
/// Where the private `net_cls` hierarchy is mounted when the system has none.
const MOUNT_POINT: &str = "/run/submarine/net_cls";
/// Name of our cgroup inside the `net_cls` hierarchy.
const GROUP: &str = "submarine";
/// How often `/proc` is scanned for new processes of the chosen apps.
const SCAN_INTERVAL: Duration = Duration::from_millis(500);

/// Tracks the processes of the chosen executables and keeps them in our cgroup.
pub struct SplitTunnel {
    // canonical paths of the chosen executables, shared with the watcher task
    apps: Arc<RwLock<HashSet<PathBuf>>>,
    // background task that periodically scans `/proc`, while active
    watcher: Option<JoinHandle<()>>,
}

impl SplitTunnel {
    /// Creates an inactive split tunnel.
    pub fn new() -> Self {
        Self {
            apps: Default::default(),
            watcher: None,
        }
    }

    /// Whether split tunneling can work on this computer: always on Linux.
    pub fn available() -> bool {
        true
    }

    /// Starts (or updates) tracking of the given executables. The mode and
    /// tunnel are handled by routing and the firewall on Linux.
    pub async fn start(
        &mut self,
        _mode: SplitMode,
        apps: &[PathBuf],
        _tunnel: Option<&TunnelNetConfig>,
    ) -> Result<()> {
        // canonical paths, to compare them with the `/proc/<pid>/exe` links
        let apps: HashSet<PathBuf> = apps
            .iter()
            .map(|p| p.canonicalize().unwrap_or_else(|_| p.clone()))
            .collect();
        let changed = *self.apps.read().unwrap() != apps;
        if changed && self.watcher.is_some() {
            // release of everything so processes of removed apps leave the cgroup
            self.stop().await?;
        }
        *self.apps.write().unwrap() = apps;

        // creation of the cgroup (blocking file system work), then start of the
        // watcher if it is not running yet; a running one picks up the new apps
        let group = tokio::task::spawn_blocking(prepare_group)
            .await
            .expect("prepare_group panicked")?;
        if self.watcher.is_none() {
            let apps = self.apps.clone();
            self.watcher = Some(tokio::spawn(async move {
                let mut tick = tokio::time::interval(SCAN_INTERVAL);
                loop {
                    tick.tick().await;
                    let apps = apps.read().unwrap().clone();
                    let group = group.clone();
                    let _ = tokio::task::spawn_blocking(move || scan(&apps, &group)).await;
                }
            }));
        }
        tracing::info!(
            apps = self.apps.read().unwrap().len(),
            "split tunnel active"
        );
        Ok(())
    }

    /// Stops tracking and moves every process back to the root cgroup.
    pub async fn stop(&mut self) -> Result<()> {
        if let Some(watcher) = self.watcher.take() {
            watcher.abort();
        }
        tokio::task::spawn_blocking(release_all)
            .await
            .expect("release_all panicked")?;
        Ok(())
    }

    /// Windows: the physical network changed; no-op elsewhere.
    pub async fn network_changed(&mut self) -> Result<()> {
        Ok(())
    }
}

impl Default for SplitTunnel {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for SplitTunnel {
    fn drop(&mut self) {
        // only the watcher is stopped: processes are released by `stop`
        if let Some(watcher) = self.watcher.take() {
            watcher.abort();
        }
    }
}

/// Mount point of an existing `net_cls` hierarchy, if the system has one.
fn find_hierarchy() -> Option<PathBuf> {
    let mounts = std::fs::read_to_string("/proc/mounts").ok()?;
    mounts.lines().find_map(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let (path, fstype, options) = (fields.get(1)?, fields.get(2)?, fields.get(3)?);
        (*fstype == "cgroup" && options.split(',').any(|o| o == "net_cls"))
            .then(|| PathBuf::from(path))
    })
}

/// Returns the `net_cls` hierarchy, mounting a private one at [`MOUNT_POINT`] when the
/// system has none (e.g. on unified cgroup v2-only systems).
fn ensure_hierarchy() -> io::Result<PathBuf> {
    if let Some(path) = find_hierarchy() {
        return Ok(path);
    }
    // mount of a v1 hierarchy with only the net_cls controller
    std::fs::create_dir_all(MOUNT_POINT)?;
    let source = CString::new("net_cls").expect("no NUL");
    let target = CString::new(MOUNT_POINT).expect("no NUL");
    let fstype = CString::new("cgroup").expect("no NUL");
    let data = CString::new("net_cls").expect("no NUL");
    // SAFETY: all arguments are valid NUL-terminated strings that outlive the call.
    let rc = unsafe {
        libc::mount(
            source.as_ptr(),
            target.as_ptr(),
            fstype.as_ptr(),
            0,
            data.as_ptr().cast(),
        )
    };
    if rc != 0 {
        let err = io::Error::last_os_error();
        return Err(io::Error::new(
            err.kind(),
            format!("cannot mount the net_cls cgroup: {err}"),
        ));
    }
    Ok(PathBuf::from(MOUNT_POINT))
}

/// Creates our cgroup if needed and sets its class id; returns its directory.
fn prepare_group() -> io::Result<PathBuf> {
    let group = ensure_hierarchy()?.join(GROUP);
    if !group.exists() {
        std::fs::create_dir(&group)?;
    }
    std::fs::write(group.join("net_cls.classid"), CLASSID.to_string())?;
    Ok(group)
}

/// Moves every process of our cgroup back to the root cgroup of the hierarchy.
fn release_all() -> io::Result<()> {
    let Some(root) = find_hierarchy() else {
        return Ok(());
    };
    let group = root.join(GROUP);
    let Ok(pids) = std::fs::read_to_string(group.join("cgroup.procs")) else {
        return Ok(());
    };
    for pid in pids.lines() {
        // the process may have exited meanwhile
        let _ = std::fs::write(root.join("cgroup.procs"), pid);
    }
    Ok(())
}

/// Moves into `group` every running process whose executable is one of `apps`.
fn scan(apps: &HashSet<PathBuf>, group: &Path) {
    if apps.is_empty() {
        return;
    }
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return;
    };
    for entry in entries.flatten() {
        // only the numeric entries of /proc are processes
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .filter(|s| s.bytes().all(|b| b.is_ascii_digit()))
        else {
            continue;
        };
        // no exe link: kernel thread, or no permission
        let Ok(exe) = std::fs::read_link(entry.path().join("exe")) else {
            continue;
        };
        let exe = running_exe(exe);
        if !apps.contains(&exe) || in_group(pid) {
            continue;
        }
        match std::fs::write(group.join("cgroup.procs"), pid) {
            Ok(()) => tracing::debug!(pid, exe = %exe.display(), "process added to split tunnel"),
            Err(err) => tracing::debug!(pid, "cannot move process: {err}"),
        }
    }
}

/// True when the process `pid` is already in our cgroup.
fn in_group(pid: &str) -> bool {
    let Ok(cgroups) = std::fs::read_to_string(format!("/proc/{pid}/cgroup")) else {
        return false;
    };
    cgroups.lines().any(is_our_group)
}

/// True when a line of `/proc/<pid>/cgroup` says the process is in our `net_cls`
/// cgroup. Lines look like `7:net_cls,net_prio:/submarine`.
fn is_our_group(line: &str) -> bool {
    let mut parts = line.splitn(3, ':');
    let (_, controllers, path) = (parts.next(), parts.next(), parts.next());
    controllers.is_some_and(|c| c.split(',').any(|c| c == "net_cls"))
        && path == Some(&format!("/{GROUP}"))
}

/// Path of the executable of a running process, as read from `/proc/<pid>/exe`.
///
/// NB: when the file on disk was replaced or removed while the process runs (e.g. an
/// app updated in place) the kernel appends " (deleted)" to the link: the suffix is
/// removed, so the process still matches the path chosen by the user.
fn running_exe(link: PathBuf) -> PathBuf {
    match link.to_str().and_then(|s| s.strip_suffix(" (deleted)")) {
        Some(path) => PathBuf::from(path),
        None => link,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // a replaced executable still matches its path; other links are left untouched
    #[test]
    fn strips_deleted_suffix_from_exe_links() {
        assert_eq!(
            running_exe(PathBuf::from("/usr/lib/firefox/firefox (deleted)")),
            PathBuf::from("/usr/lib/firefox/firefox")
        );
        assert_eq!(
            running_exe(PathBuf::from("/usr/bin/curl")),
            PathBuf::from("/usr/bin/curl")
        );
    }

    // only net_cls lines with our group path count, whatever the other controllers
    #[test]
    fn recognizes_membership_lines() {
        assert!(is_our_group("7:net_cls,net_prio:/submarine"));
        assert!(is_our_group("3:net_cls:/submarine"));
        assert!(!is_our_group("3:net_cls:/"));
        assert!(!is_our_group("0::/submarine"));
        assert!(!is_our_group("5:cpu:/submarine"));
    }
}

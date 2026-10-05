//! Per-app split tunneling through the Submarine split tunnel driver
//! (third_party/win-split-tunnel, a fork of Mullvad's).
//!
//! The driver is a non-PnP kernel service, created on first use from the
//! `.sys` shipped next to this executable. It redirects the chosen apps'
//! sockets to the "internet" address and blocks them on the "tunnel" one:
//! - exclude: tunnel = tunnel interface, internet = physical interface;
//! - include: the same addresses swapped, so the chosen apps are moved into
//!   the tunnel and blocked on the physical network (LAN included).
//!
//! Its WFP filters live in our sublayer (see `firewall.rs`) with higher
//! weights than ours, so excluded apps pass the kill switch.

use std::ffi::OsString;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use windows_service::service::{
    ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType, ServiceState, ServiceType,
};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_sys::Win32::Foundation::{
    CloseHandle, FILETIME, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    FreeMibTable, GetUnicastIpAddressTable, MIB_UNICASTIPADDRESS_ROW, MIB_UNICASTIPADDRESS_TABLE,
};
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6, AF_UNSPEC};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, OPEN_EXISTING, QueryDosDeviceW,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_NAME_NATIVE, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};

use super::firewall::{SUBLAYER_KEY, ensure_sublayer};
use super::route::physical_default_route;
use super::wide;
use crate::split_driver::{
    DriverState, IOCTL_CLEAR_CONFIGURATION, IOCTL_GET_STATE, IOCTL_INITIALIZE,
    IOCTL_REGISTER_IP_ADDRESSES, IOCTL_REGISTER_PROCESSES, IOCTL_SET_CONFIGURATION, IpAddresses,
    ProcessEntry, encode_configuration, encode_processes, encode_sublayers, to_device_path,
};
use crate::{NetError, Result, SplitMode, TunnelNetConfig};

/// Name of the kernel service that loads the driver.
pub const DRIVER_SERVICE: &str = "SubmarineSplitTunnel";
/// Driver image, shipped next to this executable.
const DRIVER_FILE: &str = "submarine-split-tunnel.sys";
/// Path of the driver's control device.
const DEVICE: &str = r"\\.\SUBMARINESPLITTUNNEL";

/// Per-app split tunneling. The driver is opened lazily on first use and
/// kept open afterwards.
#[derive(Default)]
pub struct SplitTunnel {
    /// Open control device, once the driver has been initialized.
    device: Option<Device>,
    /// Configuration currently sent to the driver, if splitting is on.
    active: Option<Active>,
}

/// Configuration sent to the driver, kept to skip redundant updates and to
/// recompute the addresses when the network changes.
struct Active {
    mode: SplitMode,
    tunnel: TunnelNetConfig,
    /// NT device paths of the chosen apps.
    paths: Vec<String>,
    /// Addresses registered with the driver, already swapped in include mode.
    ips: IpAddresses,
}

impl SplitTunnel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies the configuration. Without a tunnel there is nothing to
    /// split; the kill switch for included apps is handled by the firewall.
    /// With mode `Off` or no apps, splitting is stopped. Apps whose drive
    /// cannot be resolved are skipped.
    pub async fn start(
        &mut self,
        mode: SplitMode,
        apps: &[PathBuf],
        tunnel: Option<&TunnelNetConfig>,
    ) -> Result<()> {
        // nothing to split: stop instead
        let (Some(tunnel), false) = (tunnel, mode == SplitMode::Off || apps.is_empty()) else {
            return self.stop().await;
        };
        // the driver matches images by NT device path
        let paths: Vec<String> = apps.iter().filter_map(|p| device_path(p)).collect();
        let ips = driver_addresses(mode, tunnel)?;

        // the driver is not touched when nothing changed
        let unchanged = self
            .active
            .as_ref()
            .is_some_and(|a| a.mode == mode && a.paths == paths && a.ips == ips);
        if unchanged {
            return Ok(());
        }

        // addresses first, then the list of apps
        let device = self.device()?;
        device.ioctl(IOCTL_REGISTER_IP_ADDRESSES, &ips.encode())?;
        device.ioctl(IOCTL_SET_CONFIGURATION, &encode_configuration(&paths))?;
        // the driver state is only logged: failing to read it must NOT undo a configuration
        // that succeeded, or `active` would not be recorded and never cleaned up
        match device.state() {
            Ok(state) => {
                tracing::info!(?mode, apps = paths.len(), ?state, "split tunnel configured")
            }
            Err(err) => tracing::warn!(
                ?mode,
                apps = paths.len(),
                "split tunnel configured, state unknown: {err}"
            ),
        }
        self.active = Some(Active {
            mode,
            tunnel: tunnel.clone(),
            paths,
            ips,
        });
        Ok(())
    }

    /// Stops splitting. The driver stays loaded for the next use.
    pub async fn stop(&mut self) -> Result<()> {
        self.active = None;
        if let Some(device) = self.device.take() {
            device.ioctl(IOCTL_CLEAR_CONFIGURATION, &[])?;
        } else if let Ok(device) = Device::open() {
            // leftovers from a crashed run
            let _ = device.ioctl(IOCTL_CLEAR_CONFIGURATION, &[]);
        }
        Ok(())
    }

    /// The physical network changed: tell the driver its new addresses, if
    /// they differ from those registered.
    pub async fn network_changed(&mut self) -> Result<()> {
        let Some(active) = self.active.as_mut() else {
            return Ok(());
        };
        let ips = driver_addresses(active.mode, &active.tunnel)?;
        if ips != active.ips {
            active.ips = ips;
            let device = self.device.as_ref().expect("active implies an open device");
            device.ioctl(IOCTL_REGISTER_IP_ADDRESSES, &ips.encode())?;
        }
        Ok(())
    }

    /// Opens the driver, loading and initializing it if needed.
    ///
    /// # Errors
    /// Fails if the driver cannot be started or opened, or if after
    /// initialization it is not in the `Ready` or `Engaged` state.
    fn device(&mut self) -> Result<&Device> {
        if self.device.is_none() {
            ensure_driver_running()?;
            let device = Device::open()?;
            // a freshly started driver needs our sublayer and the running
            // processes; otherwise it was initialized by an earlier run
            if device.state()? == DriverState::Started {
                // the driver adds its filters to our sublayer, which must exist
                ensure_sublayer()?;
                device.ioctl(
                    IOCTL_INITIALIZE,
                    &encode_sublayers(guid_bytes(&SUBLAYER_KEY)),
                )?;
                device.ioctl(
                    IOCTL_REGISTER_PROCESSES,
                    &encode_processes(&process_snapshot()?),
                )?;
            }
            // only a ready (or already engaged) driver accepts a configuration
            match device.state()? {
                DriverState::Ready | DriverState::Engaged => {}
                state => {
                    return Err(NetError::Driver(format!(
                        "unexpected driver state {state:?}"
                    )));
                }
            }
            self.device = Some(device);
        }
        Ok(self.device.as_ref().expect("just opened"))
    }
}

/// Addresses to register, swapped for include mode: the first tunnel
/// address of each family and the addresses of the physical interface that
/// holds the default route.
fn driver_addresses(mode: SplitMode, tunnel: &TunnelNetConfig) -> Result<IpAddresses> {
    // the "internet" side is the interface of the physical default route
    let physical = physical_default_route(tunnel.if_index)?
        .ok_or_else(|| NetError::Driver("no physical network".into()))?;
    let (internet_v4, internet_v6) = interface_addresses(physical.InterfaceIndex)?;
    let ips = IpAddresses {
        tunnel_v4: tunnel.addresses.iter().find_map(|n| match n.addr() {
            IpAddr::V4(a) => Some(a),
            IpAddr::V6(_) => None,
        }),
        tunnel_v6: tunnel.addresses.iter().find_map(|n| match n.addr() {
            IpAddr::V6(a) => Some(a),
            IpAddr::V4(_) => None,
        }),
        internet_v4,
        internet_v6,
    };
    Ok(if mode == SplitMode::Include {
        ips.swapped()
    } else {
        ips
    })
}

/// First usable IPv4 and non-link-local IPv6 address of an interface,
/// skipping addresses marked `SkipAsSource`.
fn interface_addresses(
    if_index: u32,
) -> Result<(Option<std::net::Ipv4Addr>, Option<std::net::Ipv6Addr>)> {
    let mut table: *mut MIB_UNICASTIPADDRESS_TABLE = std::ptr::null_mut();
    // SAFETY: valid out pointer; freed below.
    let code = unsafe { GetUnicastIpAddressTable(AF_UNSPEC, &mut table) };
    if code != 0 {
        return Err(NetError::System {
            call: "GetUnicastIpAddressTable",
            code,
        });
    }
    // first match per family, among the interface's source addresses
    let (mut v4, mut v6) = (None, None);
    // SAFETY: the API returned a table with `NumEntries` rows, freed below. They are
    // read through raw pointers: the array is declared with a single element, and
    // `SkipAsSource` is a BOOLEAN that Windows may set to values other than 0 and 1,
    // which are not valid for a Rust `bool`, so no reference to either is created.
    unsafe {
        let first = (&raw const (*table).Table).cast::<MIB_UNICASTIPADDRESS_ROW>();
        for i in 0..(*table).NumEntries as usize {
            let row = first.add(i);
            let skip_as_source = (&raw const (*row).SkipAsSource).cast::<u8>().read() != 0;
            if (*row).InterfaceIndex != if_index || skip_as_source {
                continue;
            }
            // the family tag selects the valid union member
            let address = (*row).Address;
            match address.si_family {
                AF_INET if v4.is_none() => {
                    v4 = Some(std::net::Ipv4Addr::from(
                        address.Ipv4.sin_addr.S_un.S_addr.to_ne_bytes(),
                    ));
                }
                AF_INET6 if v6.is_none() => {
                    let addr = std::net::Ipv6Addr::from(address.Ipv6.sin6_addr.u.Byte);
                    if !addr.is_unicast_link_local() {
                        v6 = Some(addr);
                    }
                }
                _ => {}
            }
        }
    }
    // SAFETY: allocated by GetUnicastIpAddressTable.
    unsafe { FreeMibTable(table.cast()) };
    Ok((v4, v6))
}

/// Converts `C:\...` to `\Device\HarddiskVolumeN\...`, as the driver matches
/// images by NT device path. Returns `None` (with a warning when the drive is
/// unknown) if the path cannot be converted.
pub(super) fn device_path(path: &Path) -> Option<String> {
    // device name of the drive letter, e.g. `\Device\HarddiskVolume3` for `C:`
    let text = path.to_str()?;
    let drive = wide(text.get(..2)?);
    let mut buf = vec![0u16; 512];
    // SAFETY: valid input string and output buffer of the given size.
    let len = unsafe { QueryDosDeviceW(drive.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
    if len == 0 {
        tracing::warn!(path = %path.display(), "cannot resolve the drive of a split tunnel app");
        return None;
    }
    // the buffer holds a list of NUL-terminated names: the first one is used
    let device = String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0)?]);
    to_device_path(text, &device)
}

/// Every running process with parent and image path, as the driver needs
/// to classify processes started before it.
fn process_snapshot() -> Result<Vec<ProcessEntry>> {
    // walk of a Toolhelp snapshot, collecting pid, parent, image and creation time
    // SAFETY: plain call; the handle is closed below.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut found = Vec::new();
    // SAFETY: valid snapshot handle and entry with dwSize set.
    let mut ok = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while ok {
        let (image, created) = process_details(entry.th32ProcessID);
        found.push((
            entry.th32ProcessID,
            entry.th32ParentProcessID,
            image,
            created,
        ));
        // SAFETY: as above.
        ok = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    // SAFETY: handle from CreateToolhelp32Snapshot.
    unsafe { CloseHandle(snapshot) };

    // a parent created after its child means the parent PID was recycled; an
    // unknown creation time (0) is treated the same way
    let created: std::collections::HashMap<u32, u64> = found.iter().map(|p| (p.0, p.3)).collect();
    Ok(found
        .into_iter()
        .map(|(pid, parent, image, time)| {
            let parent_ok = created.get(&parent).is_some_and(|&t| t != 0 && t <= time);
            ProcessEntry {
                pid,
                parent_pid: if parent_ok { parent } else { 0 },
                image,
            }
        })
        .collect())
}

/// Image device path and creation time (FILETIME as u64) of a process;
/// empty/zero when not accessible.
fn process_details(pid: u32) -> (String, u64) {
    // SAFETY: plain call; the handle is closed below.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return (String::new(), 0);
    }
    // image path in NT device form, the same form used in the configuration
    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: valid handle and buffer.
    let image = if unsafe {
        QueryFullProcessImageNameW(handle, PROCESS_NAME_NATIVE, buf.as_mut_ptr(), &mut len)
    } != 0
    {
        String::from_utf16_lossy(&buf[..len as usize])
    } else {
        String::new()
    };
    // creation time, used to detect recycled parent PIDs
    let (mut created, mut other) = (FILETIME::default(), FILETIME::default());
    let (mut kernel, mut user) = (FILETIME::default(), FILETIME::default());
    // SAFETY: valid handle and out pointers.
    let time = if unsafe {
        GetProcessTimes(handle, &mut created, &mut other, &mut kernel, &mut user)
    } != 0
    {
        (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime)
    } else {
        0
    };
    // SAFETY: handle from OpenProcess.
    unsafe { CloseHandle(handle) };
    (image, time)
}

/// Creates the kernel service if needed and starts it, waiting up to 5
/// seconds for it to run.
fn ensure_driver_running() -> Result<()> {
    let err = |e: windows_service::Error| NetError::Driver(format!("driver service: {e}"));
    let manager = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )
    .map_err(err)?;
    // existing service, or a new on-demand one for the `.sys` next to this exe
    let access = ServiceAccess::QUERY_STATUS | ServiceAccess::START;
    let service = match manager.open_service(DRIVER_SERVICE, access) {
        Ok(service) => service,
        Err(_) => {
            let sys = std::env::current_exe()?.with_file_name(DRIVER_FILE);
            if !sys.exists() {
                return Err(NetError::Driver(format!("{} not found", sys.display())));
            }
            let info = ServiceInfo {
                name: DRIVER_SERVICE.into(),
                display_name: "Submarine Split Tunnel".into(),
                service_type: ServiceType::KERNEL_DRIVER,
                start_type: ServiceStartType::OnDemand,
                error_control: ServiceErrorControl::Normal,
                executable_path: sys,
                launch_arguments: vec![],
                dependencies: vec![],
                account_name: None,
                account_password: None,
            };
            manager.create_service(&info, access).map_err(err)?
        }
    };
    // start and poll every 100 ms, 50 times
    if service.query_status().map_err(err)?.current_state != ServiceState::Running {
        service.start::<OsString>(&[]).map_err(err)?;
        for _ in 0..50 {
            if service.query_status().map_err(err)?.current_state == ServiceState::Running {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        // usually an unsigned (test) driver refused by Windows
        return Err(NetError::Driver(
            "the driver did not start; on test machines enable test signing and trust the test certificate".into(),
        ));
    }
    Ok(())
}

/// Stops and deletes the driver service, e.g. when uninstalling.
pub fn remove_driver() -> Result<()> {
    let err = |e: windows_service::Error| NetError::Driver(format!("driver service: {e}"));
    let manager =
        ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT).map_err(err)?;
    let Ok(service) = manager.open_service(
        DRIVER_SERVICE,
        ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE,
    ) else {
        // never installed
        return Ok(());
    };
    // the stop may fail if the driver is not running: deletion proceeds anyway
    let _ = service.stop();
    service.delete().map_err(err)
}

/// In-memory layout of a GUID (little-endian Data1..Data3), as the driver
/// expects it.
fn guid_bytes(guid: &windows_sys::core::GUID) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&guid.data1.to_le_bytes());
    out[4..6].copy_from_slice(&guid.data2.to_le_bytes());
    out[6..8].copy_from_slice(&guid.data3.to_le_bytes());
    out[8..16].copy_from_slice(&guid.data4);
    out
}

/// Handle to the driver's control device, closed on drop.
struct Device(HANDLE);

// SAFETY: a device handle may be used from any thread.
unsafe impl Send for Device {}

impl Device {
    /// Opens the control device; fails if the driver is not running.
    fn open() -> Result<Self> {
        let name = wide(DEVICE);
        // SAFETY: valid NUL-terminated name; other arguments are plain values.
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(NetError::Driver(format!(
                "cannot open the split tunnel driver: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(Self(handle))
    }

    /// Current state of the driver.
    fn state(&self) -> Result<DriverState> {
        let mut out = [0u8; 8];
        self.ioctl_out(IOCTL_GET_STATE, &[], &mut out)?;
        Ok(DriverState::from(u64::from_le_bytes(out)))
    }

    /// Sends a request without output.
    fn ioctl(&self, code: u32, input: &[u8]) -> Result<()> {
        self.ioctl_out(code, input, &mut []).map(|_| ())
    }

    /// Sends a request and returns the number of bytes written to `output`.
    /// Empty buffers are passed as NULL.
    fn ioctl_out(&self, code: u32, input: &[u8], output: &mut [u8]) -> Result<u32> {
        let mut returned = 0u32;
        // SAFETY: buffers are valid for the given lengths; synchronous call.
        let ok = unsafe {
            DeviceIoControl(
                self.0,
                code,
                if input.is_empty() {
                    std::ptr::null()
                } else {
                    input.as_ptr().cast()
                },
                input.len() as u32,
                if output.is_empty() {
                    std::ptr::null_mut()
                } else {
                    output.as_mut_ptr().cast()
                },
                output.len() as u32,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(NetError::Driver(format!(
                "driver request {code:#x} failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(returned)
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        // SAFETY: handle from CreateFileW.
        unsafe { CloseHandle(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // the addresses of the loopback interface (index 1) are read from the table
    #[test]
    fn loopback_addresses() {
        let (v4, _) = interface_addresses(1).unwrap();
        assert_eq!(v4, Some(std::net::Ipv4Addr::LOCALHOST));
    }
}

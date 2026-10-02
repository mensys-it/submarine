//! Name (SSID) of the Wi-Fi network the computer is connected to, read for the
//! trusted networks rules of the daemon.
//!
//! - Linux: `iw dev`, or `nmcli` when `iw` is not installed;
//! - Windows: the WLAN API (`wlanapi.dll`);
//! - macOS: `networksetup -getairportnetwork` on the Wi-Fi device, or
//!   `ipconfig getsummary` when the former reports nothing.
//!
//! Every failure (no Wi-Fi hardware, tool missing, API error) means "no Wi-Fi
//! network": the rules are simply not applied.

/// Returns the SSID of the Wi-Fi network in use, `None` without one. With
/// several Wi-Fi interfaces the first connected one wins.
pub async fn current_ssid() -> Option<String> {
    platform::current_ssid().await
}

#[cfg(target_os = "linux")]
mod platform {
    use super::{parse_iw, parse_nmcli};

    /// Linux: `iw` reads the kernel state directly, `nmcli` is the fallback for
    /// systems that have NetworkManager but not `iw`.
    pub async fn current_ssid() -> Option<String> {
        if let Ok(out) = crate::run("iw", &["dev"]).await {
            return parse_iw(&out);
        }
        // NB: `--rescan no` keeps nmcli from scanning, which would be slow and
        // would disturb the connection every few seconds
        let out = crate::run(
            "nmcli",
            &[
                "-t",
                "-f",
                "ACTIVE,SSID",
                "device",
                "wifi",
                "list",
                "--rescan",
                "no",
            ],
        )
        .await
        .ok()?;
        parse_nmcli(&out)
    }
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::NetworkManagement::WiFi::{
        WLAN_CONNECTION_ATTRIBUTES, WLAN_INTERFACE_INFO_LIST, WlanCloseHandle, WlanEnumInterfaces,
        WlanFreeMemory, WlanOpenHandle, WlanQueryInterface, wlan_interface_state_connected,
        wlan_intf_opcode_current_connection,
    };

    /// Version 2 of the WLAN API, available since Windows Vista.
    const WLAN_API_VERSION_2: u32 = 2;

    /// Windows: the WLAN API is blocking, so it runs on the blocking pool.
    pub async fn current_ssid() -> Option<String> {
        tokio::task::spawn_blocking(query).await.ok().flatten()
    }

    /// SSID of the first connected wireless interface.
    fn query() -> Option<String> {
        let mut version = 0;
        let mut handle: HANDLE = std::ptr::null_mut();
        // SAFETY: valid out pointers; the reserved argument must be null.
        if unsafe {
            WlanOpenHandle(
                WLAN_API_VERSION_2,
                std::ptr::null(),
                &mut version,
                &mut handle,
            )
        } != 0
        {
            // e.g. the WLAN AutoConfig service is not running (no Wi-Fi hardware)
            return None;
        }
        let ssid = connected_ssid(handle);
        // SAFETY: `handle` was opened above and is closed exactly once.
        unsafe { WlanCloseHandle(handle, std::ptr::null()) };
        ssid
    }

    /// Walks the interfaces of an open WLAN handle.
    fn connected_ssid(handle: HANDLE) -> Option<String> {
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        // SAFETY: valid handle and out pointer; the list is freed below.
        if unsafe { WlanEnumInterfaces(handle, std::ptr::null(), &mut list) } != 0 {
            return None;
        }
        // SAFETY: on success `list` points to a header followed by
        // `dwNumberOfItems` entries, allocated by the API.
        let interfaces = unsafe {
            std::slice::from_raw_parts(
                (*list).InterfaceInfo.as_ptr(),
                (*list).dwNumberOfItems as usize,
            )
        };
        let ssid = interfaces
            .iter()
            .filter(|i| i.isState == wlan_interface_state_connected)
            .find_map(|i| {
                let mut size = 0;
                let mut data: *mut core::ffi::c_void = std::ptr::null_mut();
                // SAFETY: valid handle, GUID from the list and out pointers.
                let code = unsafe {
                    WlanQueryInterface(
                        handle,
                        &i.InterfaceGuid,
                        wlan_intf_opcode_current_connection,
                        std::ptr::null(),
                        &mut size,
                        &mut data,
                        std::ptr::null_mut(),
                    )
                };
                if code != 0 || data.is_null() {
                    return None;
                }
                // SAFETY: for this opcode the API returns a WLAN_CONNECTION_ATTRIBUTES,
                // copied out before the memory is freed.
                let attributes = unsafe { *(data as *const WLAN_CONNECTION_ATTRIBUTES) };
                // SAFETY: allocated by WlanQueryInterface and freed once.
                unsafe { WlanFreeMemory(data) };
                let ssid = attributes.wlanAssociationAttributes.dot11Ssid;
                let len = (ssid.uSSIDLength as usize).min(ssid.ucSSID.len());
                (len > 0).then(|| String::from_utf8_lossy(&ssid.ucSSID[..len]).into_owned())
            });
        // SAFETY: allocated by WlanEnumInterfaces and freed once, after its last use.
        unsafe { WlanFreeMemory(list as *const core::ffi::c_void) };
        ssid
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use tokio::process::Command;

    use crate::macos_text::{parse_airport_network, parse_ipconfig_ssid, parse_wifi_device};

    /// macOS: the Wi-Fi device first, then its network.
    ///
    /// NB: recent macOS versions may hide the SSID from processes without the
    /// location permission; then both tools report nothing and the rules are
    /// not applied.
    pub async fn current_ssid() -> Option<String> {
        let ports = output("networksetup", &["-listallhardwareports"]).await?;
        let device = parse_wifi_device(&ports)?;
        if let Some(ssid) = output("networksetup", &["-getairportnetwork", &device])
            .await
            .and_then(|out| parse_airport_network(&out))
        {
            return Some(ssid);
        }
        let summary = output("ipconfig", &["getsummary", &device]).await?;
        parse_ipconfig_ssid(&summary)
    }

    /// Stdout of a tool that exited successfully.
    async fn output(program: &str, args: &[&str]) -> Option<String> {
        let out = Command::new(program).args(args).output().await.ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod platform {
    /// Other platforms: Wi-Fi networks are not detected.
    pub async fn current_ssid() -> Option<String> {
        None
    }
}

/// SSID in the output of `iw dev`: the first `ssid <name>` line, with the
/// `\xNN` escapes `iw` uses for non-printable bytes decoded.
#[cfg(any(target_os = "linux", test))]
fn parse_iw(output: &str) -> Option<String> {
    output
        .lines()
        .find_map(|line| line.trim_start().strip_prefix("ssid "))
        .map(unescape_iw)
        .filter(|ssid| !ssid.is_empty())
}

/// Decodes the `\xNN` escapes of `iw` into bytes, then the bytes as UTF-8.
#[cfg(any(target_os = "linux", test))]
fn unescape_iw(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 2..i + 4)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], bytes.get(i + 1), hex) {
            (b'\\', Some(b'x'), Some(byte)) => {
                out.push(byte);
                i += 4;
            }
            (byte, _, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// SSID of the active network in the terse output of `nmcli` (`yes:<ssid>`
/// lines), where a `:` inside the name is escaped as `\:`.
#[cfg(any(target_os = "linux", test))]
fn parse_nmcli(output: &str) -> Option<String> {
    output
        .lines()
        .find_map(|line| line.strip_prefix("yes:"))
        .map(|ssid| ssid.replace("\\:", ":").replace("\\\\", "\\"))
        .filter(|ssid| !ssid.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    // the ssid of the first connected interface, indented as iw prints it
    #[test]
    fn iw_ssid_is_found() {
        let out =
            "phy#0\n\tInterface wlp2s0\n\t\tifindex 3\n\t\tssid Home Network\n\t\ttype managed\n";
        assert_eq!(parse_iw(out).as_deref(), Some("Home Network"));
    }

    // no ssid line: the interface is not connected
    #[test]
    fn iw_without_ssid_is_none() {
        assert_eq!(
            parse_iw("phy#0\n\tInterface wlp2s0\n\t\ttype managed\n"),
            None
        );
    }

    // non-ASCII names come escaped byte by byte
    #[test]
    fn iw_escapes_are_decoded() {
        let out = "\t\tssid Caf\\xc3\\xa8 \\x5cfree\n";
        assert_eq!(parse_iw(out).as_deref(), Some("Cafè \\free"));
    }

    // only the active line counts, and escaped colons are restored
    #[test]
    fn nmcli_active_network_is_found() {
        let out = "no:Neighbour\nyes:Office\\:5G\nno:Other\n";
        assert_eq!(parse_nmcli(out).as_deref(), Some("Office:5G"));
        assert_eq!(parse_nmcli("no:Neighbour\n"), None);
    }
}

//! Wire format of the Windows split tunnel driver, as defined in
//! third_party/win-split-tunnel/src/defs.
//!
//! The module only builds and parses byte buffers, so it is platform-neutral
//! and unit tested everywhere; the Windows backend (`windows/split.rs`) sends
//! these buffers to the driver through `DeviceIoControl`. The driver is 64-bit
//! only, so `SIZE_T` and `HANDLE` are 8 bytes and structures use natural
//! alignment.

use std::net::{Ipv4Addr, Ipv6Addr};

/// Device type of the driver's control device (`FILE_DEVICE_*` custom range).
const DEVICE_TYPE: u32 = 0x8000;
/// Transfer type for requests whose buffers are copied by the I/O manager.
const METHOD_BUFFERED: u32 = 0;
/// Transfer type for requests without buffers.
const METHOD_NEITHER: u32 = 3;

/// Rust equivalent of the `CTL_CODE` macro for our device type.
const fn ctl_code(function: u32, method: u32) -> u32 {
    // the access bits are FILE_ANY_ACCESS, i.e. 0
    (DEVICE_TYPE << 16) | (function << 2) | method
}

/// Initializes the driver with the WFP sublayers it must use.
pub(crate) const IOCTL_INITIALIZE: u32 = ctl_code(1, METHOD_BUFFERED);
/// Registers the processes that were already running when the driver started.
pub(crate) const IOCTL_REGISTER_PROCESSES: u32 = ctl_code(3, METHOD_BUFFERED);
/// Registers the tunnel and internet addresses (see [`IpAddresses`]).
pub(crate) const IOCTL_REGISTER_IP_ADDRESSES: u32 = ctl_code(4, METHOD_BUFFERED);
/// Sets the list of split applications (see [`encode_configuration`]).
pub(crate) const IOCTL_SET_CONFIGURATION: u32 = ctl_code(6, METHOD_BUFFERED);
/// Clears the list of split applications.
pub(crate) const IOCTL_CLEAR_CONFIGURATION: u32 = ctl_code(8, METHOD_NEITHER);
/// Returns the current [`DriverState`] as a little-endian u64.
pub(crate) const IOCTL_GET_STATE: u32 = ctl_code(9, METHOD_BUFFERED);

/// State of the driver, mirroring `ST_DRIVER_STATE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DriverState {
    None,
    Started,
    Initialized,
    Ready,
    Engaged,
    Zombie,
    /// A value this crate does not know, kept for diagnostics.
    Unknown(u64),
}

impl From<u64> for DriverState {
    fn from(value: u64) -> Self {
        match value {
            0 => Self::None,
            1 => Self::Started,
            2 => Self::Initialized,
            3 => Self::Ready,
            4 => Self::Engaged,
            5 => Self::Zombie,
            other => Self::Unknown(other),
        }
    }
}

/// Addresses registered with the driver, mirroring `ST_IP_ADDRESSES`: IN_ADDR
/// tunnel, IN_ADDR internet, IN6_ADDR tunnel, IN6_ADDR internet. Unspecified
/// addresses on the wire mean "not available" and map to `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct IpAddresses {
    /// IPv4 address of the side the split apps are blocked on.
    pub tunnel_v4: Option<Ipv4Addr>,
    /// IPv4 address the split apps are redirected to.
    pub internet_v4: Option<Ipv4Addr>,
    /// IPv6 address of the side the split apps are blocked on.
    pub tunnel_v6: Option<Ipv6Addr>,
    /// IPv6 address the split apps are redirected to.
    pub internet_v6: Option<Ipv6Addr>,
}

impl IpAddresses {
    /// Returns the addresses with the tunnel and internet roles exchanged.
    /// Include mode reuses the driver's exclusion logic with the roles
    /// swapped: the chosen apps are "redirected" to the tunnel and blocked
    /// on the physical network.
    pub fn swapped(self) -> Self {
        Self {
            tunnel_v4: self.internet_v4,
            internet_v4: self.tunnel_v4,
            tunnel_v6: self.internet_v6,
            internet_v6: self.tunnel_v6,
        }
    }

    /// Encodes the 40-byte `ST_IP_ADDRESSES` structure, writing missing
    /// addresses as unspecified (all zeros).
    pub fn encode(&self) -> Vec<u8> {
        let v4 = |a: Option<Ipv4Addr>| a.unwrap_or(Ipv4Addr::UNSPECIFIED).octets();
        let v6 = |a: Option<Ipv6Addr>| a.unwrap_or(Ipv6Addr::UNSPECIFIED).octets();
        let mut out = Vec::with_capacity(40);
        out.extend(v4(self.tunnel_v4));
        out.extend(v4(self.internet_v4));
        out.extend(v6(self.tunnel_v6));
        out.extend(v6(self.internet_v6));
        out
    }
}

/// Encodes `ST_SUBLAYER_GUIDS`: the baseline and the DNS sublayer, both set to
/// our own sublayer. `guid` is the 16-byte in-memory GUID layout
/// (little-endian Data1..Data3).
pub(crate) fn encode_sublayers(guid: [u8; 16]) -> Vec<u8> {
    [guid, guid].concat()
}

/// Encodes a string as UTF-16LE bytes, without a NUL terminator.
fn utf16(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

/// Encodes `ST_CONFIGURATION_HEADER` + `ST_CONFIGURATION_ENTRY[]` + strings:
/// the list of applications to split. Paths are NT device paths
/// (`\Device\HarddiskVolume3\...`); each entry holds the offset of its string
/// relative to the start of the string area.
pub(crate) fn encode_configuration(paths: &[String]) -> Vec<u8> {
    // header: SIZE_T entry count, SIZE_T total length
    const HEADER: usize = 16;
    // entry: SIZE_T offset, USHORT length, padding
    const ENTRY: usize = 16;
    let strings: Vec<Vec<u8>> = paths.iter().map(|p| utf16(p)).collect();
    let total = HEADER + ENTRY * strings.len() + strings.iter().map(Vec::len).sum::<usize>();

    // header
    let mut out = Vec::with_capacity(total);
    out.extend((strings.len() as u64).to_le_bytes());
    out.extend((total as u64).to_le_bytes());
    // one entry per path, with the string length in bytes
    let mut offset = 0u64;
    for s in &strings {
        out.extend(offset.to_le_bytes());
        out.extend((s.len() as u16).to_le_bytes());
        out.extend([0u8; 6]);
        offset += s.len() as u64;
    }
    // string area, in the same order as the entries
    for s in &strings {
        out.extend(s);
    }
    debug_assert_eq!(out.len(), total);
    out
}

/// A running process, as reported to the driver at initialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProcessEntry {
    /// Process id.
    pub pid: u32,
    /// Parent process id; 0 when unknown or when the PID was recycled.
    pub parent_pid: u32,
    /// NT device path of the image; empty when it could not be queried.
    pub image: String,
}

/// Encodes `ST_PROCESS_DISCOVERY_HEADER` + `ST_PROCESS_DISCOVERY_ENTRY[]` +
/// strings: the processes running before the driver was initialized. An entry
/// with an empty image gets offset 0 and length 0.
pub(crate) fn encode_processes(processes: &[ProcessEntry]) -> Vec<u8> {
    // header: SIZE_T entry count, SIZE_T total length
    const HEADER: usize = 16;
    // entry: HANDLE pid, HANDLE ppid, SIZE_T offset, USHORT length, padding
    const ENTRY: usize = 32;
    let strings: Vec<Vec<u8>> = processes.iter().map(|p| utf16(&p.image)).collect();
    let total = HEADER + ENTRY * processes.len() + strings.iter().map(Vec::len).sum::<usize>();

    // header
    let mut out = Vec::with_capacity(total);
    out.extend((processes.len() as u64).to_le_bytes());
    out.extend((total as u64).to_le_bytes());
    // one entry per process, PIDs widened to HANDLE size
    let mut offset = 0u64;
    for (process, s) in processes.iter().zip(&strings) {
        out.extend(u64::from(process.pid).to_le_bytes());
        out.extend(u64::from(process.parent_pid).to_le_bytes());
        out.extend(if s.is_empty() { 0 } else { offset }.to_le_bytes());
        out.extend((s.len() as u16).to_le_bytes());
        out.extend([0u8; 6]);
        offset += s.len() as u64;
    }
    // string area, in the same order as the entries
    for s in &strings {
        out.extend(s);
    }
    debug_assert_eq!(out.len(), total);
    out
}

/// Turns `C:\dir\app.exe` into `\Device\HarddiskVolume3\dir\app.exe`, given
/// the drive's device name as returned by `QueryDosDeviceW("C:")`.
/// Returns `None` for paths that do not start with a drive letter (e.g. UNC
/// paths).
pub(crate) fn to_device_path(dos_path: &str, drive_device: &str) -> Option<String> {
    let rest = dos_path.get(2..)?;
    let drive = dos_path.get(..2)?;
    (drive.ends_with(':') && rest.starts_with('\\')).then(|| format!("{drive_device}{rest}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // the codes match CTL_CODE(0x8000, function, method, FILE_ANY_ACCESS)
    #[test]
    fn ioctl_codes_match_ctl_code_macro() {
        // CTL_CODE(0x8000, 1, METHOD_BUFFERED, FILE_ANY_ACCESS)
        assert_eq!(IOCTL_INITIALIZE, 0x8000_0004);
        assert_eq!(IOCTL_REGISTER_PROCESSES, 0x8000_000C);
        assert_eq!(IOCTL_REGISTER_IP_ADDRESSES, 0x8000_0010);
        assert_eq!(IOCTL_SET_CONFIGURATION, 0x8000_0018);
        assert_eq!(IOCTL_CLEAR_CONFIGURATION, 0x8000_0023);
        assert_eq!(IOCTL_GET_STATE, 0x8000_0024);
    }

    // byte layout of the addresses and round trip of the role swap
    #[test]
    fn ip_addresses_layout_and_swap() {
        let ips = IpAddresses {
            tunnel_v4: Some("10.99.0.2".parse().unwrap()),
            internet_v4: Some("192.168.1.20".parse().unwrap()),
            tunnel_v6: None,
            internet_v6: Some("2001:db8::20".parse().unwrap()),
        };
        let raw = ips.encode();
        assert_eq!(raw.len(), 40);
        assert_eq!(&raw[0..4], &[10, 99, 0, 2]);
        assert_eq!(&raw[4..8], &[192, 168, 1, 20]);
        assert_eq!(&raw[8..24], &[0; 16]);
        assert_eq!(raw[24..26], [0x20, 0x01]);

        let swapped = ips.swapped();
        assert_eq!(swapped.tunnel_v4, ips.internet_v4);
        assert_eq!(swapped.internet_v6, None);
        assert_eq!(swapped.swapped(), ips);
    }

    // header, entry offsets/lengths and total size of the configuration
    #[test]
    fn configuration_layout() {
        let raw = encode_configuration(&[r"\Device\A\x.exe".into(), r"\Device\B\yy.exe".into()]);
        let u64_at = |i: usize| u64::from_le_bytes(raw[i..i + 8].try_into().unwrap());
        let u16_at = |i: usize| u16::from_le_bytes(raw[i..i + 2].try_into().unwrap());
        assert_eq!(u64_at(0), 2);
        assert_eq!(u64_at(8) as usize, raw.len());
        // first entry: offset 0, 15 UTF-16 units (30 bytes)
        assert_eq!(u64_at(16), 0);
        assert_eq!(u16_at(24), 30);
        // second entry: follows the first string
        assert_eq!(u64_at(32), 30);
        assert_eq!(u16_at(40), 32);
        assert_eq!(raw.len(), 16 + 32 + 30 + 32);
    }

    // process entries, including one whose image is unknown
    #[test]
    fn processes_layout() {
        let raw = encode_processes(&[
            ProcessEntry {
                pid: 4,
                parent_pid: 0,
                image: String::new(),
            },
            ProcessEntry {
                pid: 1234,
                parent_pid: 4,
                image: r"\Device\X\a.exe".into(),
            },
        ]);
        let u64_at = |i: usize| u64::from_le_bytes(raw[i..i + 8].try_into().unwrap());
        assert_eq!(u64_at(0), 2);
        assert_eq!(u64_at(8) as usize, raw.len());
        assert_eq!(u64_at(16), 4); // pid
        assert_eq!(u64_at(48), 1234);
        assert_eq!(u64_at(56), 4); // parent
        assert_eq!(u64_at(64), 0); // offset of the only string
        assert_eq!(raw.len(), 16 + 64 + 30);
    }

    // drive paths are converted, UNC paths and bare drives are rejected
    #[test]
    fn device_paths() {
        assert_eq!(
            to_device_path(r"C:\Program Files\App\app.exe", r"\Device\HarddiskVolume3").as_deref(),
            Some(r"\Device\HarddiskVolume3\Program Files\App\app.exe")
        );
        assert_eq!(to_device_path(r"\\server\share\a.exe", r"\Device\X"), None);
        assert_eq!(to_device_path("C:", r"\Device\X"), None);
    }
}

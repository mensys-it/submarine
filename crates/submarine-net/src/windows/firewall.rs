//! Kill switch on the Windows Filtering Platform.
//!
//! All filters live in our own persistent sublayer, at the ALE connect and
//! receive/accept layers: a low-weight "block everything" plus higher-weight
//! permits (loopback, tunnel interface, this service's own encrypted traffic,
//! DHCP, optionally LAN and DNS). Every change replaces the whole set inside a
//! single transaction.
//!
//! Filters are persistent: they survive a crash of the service and reboots,
//! like the nftables table on Linux, until the service removes them. The
//! uninstaller must run `submarine-daemon reset-firewall`.

use std::net::{Ipv4Addr, Ipv6Addr};
use std::ptr;

use windows_sys::Win32::Foundation::{
    FWP_E_ALREADY_EXISTS, FWP_E_FILTER_NOT_FOUND, FWP_E_IN_USE, FWP_E_NOT_FOUND,
    FWP_E_PROVIDER_NOT_FOUND, FWP_E_SUBLAYER_NOT_FOUND, HANDLE,
};
use windows_sys::Win32::NetworkManagement::WindowsFilteringPlatform::*;
use windows_sys::Win32::System::Rpc::RPC_C_AUTHN_WINNT;
use windows_sys::core::GUID;

use super::{luid_from_index, wide};
use crate::{FirewallPolicy, NetError, Result, SplitMode};

/// Key of our persistent WFP provider, owner of every filter we add.
const PROVIDER_KEY: GUID = GUID::from_u128(0x7d4f_0c52_8a1e_4f61_9d3b_5375_6d61_7269);
/// Key of our persistent WFP sublayer. Shared with the split tunnel driver,
/// which adds its filters here.
pub(super) const SUBLAYER_KEY: GUID = GUID::from_u128(0x2b91_e7a4_3c5d_4e08_a6f2_5375_6d61_7269);

// WFP returns HRESULTs as u32: the error codes we tolerate, converted once
const ALREADY_EXISTS: u32 = FWP_E_ALREADY_EXISTS as u32;
const FILTER_NOT_FOUND: u32 = FWP_E_FILTER_NOT_FOUND as u32;
const NOT_FOUND: u32 = FWP_E_NOT_FOUND as u32;
const SUBLAYER_NOT_FOUND: u32 = FWP_E_SUBLAYER_NOT_FOUND as u32;
const PROVIDER_NOT_FOUND: u32 = FWP_E_PROVIDER_NOT_FOUND as u32;
const IN_USE: u32 = FWP_E_IN_USE as u32;
// flags that make the provider and the sublayer persistent
const FWPM_PROVIDER_FLAG_PERSISTENT: u32 = 1;
const FWPM_SUBLAYER_FLAG_PERSISTENT: u32 = 1;

/// IP protocol number of UDP, for the DHCP filters.
const IPPROTO_UDP: u8 = 17;
// filter weights inside our sublayer: higher weights are evaluated first and
// the first matching filter decides
const WEIGHT_BLOCK: u8 = 0;
const WEIGHT_PERMIT: u8 = 10;
const WEIGHT_BLOCK_APP: u8 = 11;
const WEIGHT_BLOCK_DNS: u8 = 12;
const WEIGHT_PERMIT_APP_LAN: u8 = 12;
const WEIGHT_PERMIT_TUNNEL: u8 = 13;

/// IPv4 networks allowed by "allow LAN": private ranges, link-local,
/// multicast and broadcast.
const LAN_V4: &[(Ipv4Addr, u8)] = &[
    (Ipv4Addr::new(10, 0, 0, 0), 8),
    (Ipv4Addr::new(172, 16, 0, 0), 12),
    (Ipv4Addr::new(192, 168, 0, 0), 16),
    (Ipv4Addr::new(169, 254, 0, 0), 16),
    (Ipv4Addr::new(224, 0, 0, 0), 4),
    (Ipv4Addr::BROADCAST, 32),
];
/// IPv6 networks allowed by "allow LAN": link-local, unique local and
/// multicast.
const LAN_V6: &[(Ipv6Addr, u8)] = &[
    (Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0), 10),
    (Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 0), 7),
    (Ipv6Addr::new(0xff00, 0, 0, 0, 0, 0, 0, 0), 8),
];

/// Kill switch. It is stateless: the filters are found again in WFP by
/// provider and sublayer key.
pub struct Firewall;

impl Firewall {
    pub fn new() -> Self {
        Self
    }

    /// Replaces our filters with those for `policy`. WFP calls are blocking,
    /// so they run on the blocking thread pool.
    pub async fn apply(&mut self, policy: &FirewallPolicy) -> Result<()> {
        let policy = policy.clone();
        tokio::task::spawn_blocking(move || apply_blocking(&policy))
            .await
            .expect("firewall task panicked")
    }

    /// Removes our filters, then the sublayer and the provider unless they are
    /// still in use by the split tunnel driver's filters.
    pub async fn reset(&mut self) -> Result<()> {
        tokio::task::spawn_blocking(|| {
            let engine = Engine::open()?;
            engine.transaction(|engine| {
                engine.delete_our_filters()?;
                ignore(
                    // SAFETY: `engine.0` is an open engine handle and the key outlives the call.
                    engine.call("FwpmSubLayerDeleteByKey0", unsafe {
                        FwpmSubLayerDeleteByKey0(engine.0, &SUBLAYER_KEY)
                    }),
                    // IN_USE: still used by the split tunnel driver's filters
                    &[SUBLAYER_NOT_FOUND, IN_USE],
                )?;
                ignore(
                    // SAFETY: `engine.0` is an open engine handle and the key outlives the call.
                    engine.call("FwpmProviderDeleteByKey0", unsafe {
                        FwpmProviderDeleteByKey0(engine.0, &PROVIDER_KEY)
                    }),
                    &[PROVIDER_NOT_FOUND, IN_USE],
                )
            })
        })
        .await
        .expect("firewall task panicked")
    }
}

/// Creates our provider and sublayer if missing (the split tunnel driver
/// needs the sublayer before it can be initialized).
pub(super) fn ensure_sublayer() -> Result<()> {
    let engine = Engine::open()?;
    engine.transaction(|engine| engine.ensure_provider_and_sublayer())
}

impl Default for Firewall {
    fn default() -> Self {
        Self::new()
    }
}

/// Body of [`Firewall::apply`]: the whole filter set is replaced inside a
/// single transaction, so a failure leaves the previous set in place.
fn apply_blocking(p: &FirewallPolicy) -> Result<()> {
    let engine = Engine::open()?;
    engine.transaction(|engine| {
        // fresh start from an empty sublayer
        engine.ensure_provider_and_sublayer()?;
        engine.delete_our_filters()?;
        // WFP matches the tunnel interface by its LUID
        let tunnel_luid = p.tunnel_index.map(luid_from_index).transpose()?;
        // SAFETY: every NET_LUID_LH variant is a plain u64.
        let tunnel_luid = tunnel_luid.map(|luid| unsafe { luid.Value });

        // DNS leak protection: outbound DNS only through the tunnel (or loopback)
        if p.block_dns_leaks
            && let Some(luid) = tunnel_luid
        {
            for layer in [
                FWPM_LAYER_ALE_AUTH_CONNECT_V4,
                FWPM_LAYER_ALE_AUTH_CONNECT_V6,
            ] {
                let add = |name, weight, action, conds: &[Cond]| {
                    engine.add_filter(&layer, name, weight, action, conds)
                };
                add(
                    "Submarine: loopback",
                    WEIGHT_PERMIT_TUNNEL,
                    FWP_ACTION_PERMIT,
                    &[Cond::Loopback],
                )?;
                add(
                    "Submarine: tunnel",
                    WEIGHT_PERMIT_TUNNEL,
                    FWP_ACTION_PERMIT,
                    &[Cond::LocalInterface(luid)],
                )?;
                add(
                    "Submarine: DNS outside the tunnel",
                    WEIGHT_BLOCK_DNS,
                    FWP_ACTION_BLOCK,
                    &[Cond::RemotePort(53)],
                )?;
            }
        }
        // nothing else without the kill switch
        if !p.block {
            return Ok(());
        }
        if p.split == SplitMode::Include {
            // only the chosen apps are protected: with the tunnel up the driver binds
            // them to it, and they are blocked everywhere else
            // NB: the app filters stay with the tunnel up too. An app the driver does
            // not bind (driver not running, image not matched) would otherwise use the
            // physical network, unprotected, while the user expects it to be
            block_apps(engine, p, tunnel_luid)?;
            tracing::info!(
                apps = p.split_apps.len(),
                "firewall applied (chosen apps only)"
            );
            return Ok(());
        }

        // full kill switch: permits on top of a block-all, on every ALE layer
        let app_id = AppId::current_exe()?;

        // (layer, IPv6, outbound)
        let layers = [
            (FWPM_LAYER_ALE_AUTH_CONNECT_V4, false, true),
            (FWPM_LAYER_ALE_AUTH_RECV_ACCEPT_V4, false, false),
            (FWPM_LAYER_ALE_AUTH_CONNECT_V6, true, true),
            (FWPM_LAYER_ALE_AUTH_RECV_ACCEPT_V6, true, false),
        ];
        for (layer, v6, outbound) in layers {
            let permit = |name: &str, conds: &[Cond]| {
                engine.add_filter(&layer, name, WEIGHT_PERMIT, FWP_ACTION_PERMIT, conds)
            };
            permit("Submarine: loopback", &[Cond::Loopback])?;
            if let Some(luid) = tunnel_luid {
                permit("Submarine: tunnel", &[Cond::LocalInterface(luid)])?;
            }
            // encrypted tunnel traffic comes from this service
            permit("Submarine: service", &[Cond::AppId(app_id.blob())])?;

            // DHCP client and server ports of the layer's family
            let (client, server) = if v6 { (546, 547) } else { (68, 67) };
            permit(
                "Submarine: DHCP",
                &[
                    Cond::Protocol(IPPROTO_UDP),
                    Cond::LocalPort(client),
                    Cond::RemotePort(server),
                ],
            )?;
            if p.allow_lan {
                if v6 {
                    for &(addr, len) in LAN_V6 {
                        permit("Submarine: LAN", &[Cond::RemoteV6(addr, len)])?;
                    }
                } else {
                    for &(addr, len) in LAN_V4 {
                        permit("Submarine: LAN", &[Cond::RemoteV4(addr, len)])?;
                    }
                }
            }
            // plain DNS while the endpoint hostnames are resolved
            if p.allow_dns && outbound {
                permit("Submarine: DNS while connecting", &[Cond::RemotePort(53)])?;
            }
            // lowest weight: whatever is not permitted above is blocked
            engine.add_filter(
                &layer,
                "Submarine: block",
                WEIGHT_BLOCK,
                FWP_ACTION_BLOCK,
                &[],
            )?;
        }
        tracing::info!(
            allow_lan = p.allow_lan,
            tunnel = ?p.tunnel_index,
            dns_leaks = p.block_dns_leaks,
            "firewall applied"
        );
        Ok(())
    })
}

/// Include mode: the chosen apps may reach only loopback, the tunnel interface
/// `tunnel_luid` when there is one and, if allowed, the LAN. Apps whose id cannot
/// be computed (a path not on a local drive) are skipped with a warning.
fn block_apps(engine: &Engine, p: &FirewallPolicy, tunnel_luid: Option<u64>) -> Result<()> {
    // (layer, IPv6)
    let layers = [
        (FWPM_LAYER_ALE_AUTH_CONNECT_V4, false),
        (FWPM_LAYER_ALE_AUTH_RECV_ACCEPT_V4, false),
        (FWPM_LAYER_ALE_AUTH_CONNECT_V6, true),
        (FWPM_LAYER_ALE_AUTH_RECV_ACCEPT_V6, true),
    ];
    for path in &p.split_apps {
        let app = match AppId::from_path(path) {
            Ok(app) => app,
            Err(err) => {
                tracing::warn!(path = %path.display(), "cannot block app: {err}");
                continue;
            }
        };
        for (layer, v6) in layers {
            let app_cond = Cond::AppId(app.blob());
            // loopback for this app, above its block (the other apps are not blocked)
            engine.add_filter(
                &layer,
                "Submarine: app loopback",
                WEIGHT_PERMIT_TUNNEL,
                FWP_ACTION_PERMIT,
                &[Cond::AppId(app.blob()), Cond::Loopback],
            )?;
            // the tunnel for this app, above its block
            if let Some(luid) = tunnel_luid {
                engine.add_filter(
                    &layer,
                    "Submarine: app tunnel",
                    WEIGHT_PERMIT_TUNNEL,
                    FWP_ACTION_PERMIT,
                    &[Cond::AppId(app.blob()), Cond::LocalInterface(luid)],
                )?;
            }
            // LAN for this app, above its block
            if p.allow_lan {
                if v6 {
                    for &(addr, len) in LAN_V6 {
                        let conds = [Cond::AppId(app.blob()), Cond::RemoteV6(addr, len)];
                        engine.add_filter(
                            &layer,
                            "Submarine: app LAN",
                            WEIGHT_PERMIT_APP_LAN,
                            FWP_ACTION_PERMIT,
                            &conds,
                        )?;
                    }
                } else {
                    for &(addr, len) in LAN_V4 {
                        let conds = [Cond::AppId(app.blob()), Cond::RemoteV4(addr, len)];
                        engine.add_filter(
                            &layer,
                            "Submarine: app LAN",
                            WEIGHT_PERMIT_APP_LAN,
                            FWP_ACTION_PERMIT,
                            &conds,
                        )?;
                    }
                }
            }
            // everything else from this app is blocked
            engine.add_filter(
                &layer,
                "Submarine: block app",
                WEIGHT_BLOCK_APP,
                FWP_ACTION_BLOCK,
                &[app_cond],
            )?;
        }
    }
    Ok(())
}

/// Compares two GUIDs field by field (windows-sys `GUID` is not `PartialEq`).
fn same_guid(a: &GUID, b: &GUID) -> bool {
    (a.data1, a.data2, a.data3, a.data4) == (b.data1, b.data2, b.data3, b.data4)
}

/// Turns the system errors whose code is in `codes` into success.
fn ignore(result: Result<()>, codes: &[u32]) -> Result<()> {
    match result {
        Err(NetError::System { code, .. }) if codes.contains(&code) => Ok(()),
        other => other,
    }
}

/// An open WFP engine session, closed on drop.
struct Engine(HANDLE);

impl Engine {
    /// Opens a session on the local engine.
    fn open() -> Result<Self> {
        let session = FWPM_SESSION0::default();
        let mut handle: HANDLE = ptr::null_mut();
        // SAFETY: valid session and out pointers; NULL server means local.
        let code = unsafe {
            FwpmEngineOpen0(
                ptr::null(),
                RPC_C_AUTHN_WINNT,
                ptr::null(),
                &session,
                &mut handle,
            )
        };
        // wrapped before the check, so the handle is closed on error too
        let engine = Engine(handle);
        engine.call("FwpmEngineOpen0", code)?;
        Ok(engine)
    }

    /// Turns a WFP return code into a `Result`, naming the failed `call`.
    fn call(&self, call: &'static str, code: u32) -> Result<()> {
        if code == 0 {
            Ok(())
        } else {
            Err(NetError::System { call, code })
        }
    }

    /// Runs `body` inside a transaction: committed on success, aborted on error.
    fn transaction(&self, body: impl FnOnce(&Self) -> Result<()>) -> Result<()> {
        // SAFETY: `self.0` is an open engine handle.
        self.call("FwpmTransactionBegin0", unsafe {
            FwpmTransactionBegin0(self.0, 0)
        })?;
        match body(self) {
            // SAFETY: a transaction is open.
            Ok(()) => self.call("FwpmTransactionCommit0", unsafe {
                FwpmTransactionCommit0(self.0)
            }),
            Err(err) => {
                // SAFETY: a transaction is open.
                unsafe { FwpmTransactionAbort0(self.0) };
                Err(err)
            }
        }
    }

    /// Creates our persistent provider and sublayer; existing ones are kept.
    fn ensure_provider_and_sublayer(&self) -> Result<()> {
        // provider
        let mut name = wide("Submarine VPN");
        let mut provider_key = PROVIDER_KEY;
        let provider = FWPM_PROVIDER0 {
            providerKey: PROVIDER_KEY,
            displayData: FWPM_DISPLAY_DATA0 {
                name: name.as_mut_ptr(),
                description: ptr::null_mut(),
            },
            flags: FWPM_PROVIDER_FLAG_PERSISTENT,
            ..Default::default()
        };
        // SAFETY: `provider` and `name` outlive the call.
        let code = unsafe { FwpmProviderAdd0(self.0, &provider, ptr::null_mut()) };
        ignore(self.call("FwpmProviderAdd0", code), &[ALREADY_EXISTS])?;

        // sublayer, owned by the provider
        let sublayer = FWPM_SUBLAYER0 {
            subLayerKey: SUBLAYER_KEY,
            displayData: FWPM_DISPLAY_DATA0 {
                name: name.as_mut_ptr(),
                description: ptr::null_mut(),
            },
            flags: FWPM_SUBLAYER_FLAG_PERSISTENT,
            providerKey: &mut provider_key,
            // highest weight: evaluated before other sublayers
            weight: u16::MAX,
            ..Default::default()
        };
        // SAFETY: `sublayer`, `name` and `provider_key` outlive the call.
        let code = unsafe { FwpmSubLayerAdd0(self.0, &sublayer, ptr::null_mut()) };
        ignore(self.call("FwpmSubLayerAdd0", code), &[ALREADY_EXISTS])
    }

    /// Deletes every filter of our provider in our sublayer, leaving the split
    /// tunnel driver's filters in place.
    fn delete_our_filters(&self) -> Result<()> {
        // ids are collected first and the filters deleted once the
        // enumeration is over
        let mut enum_handle: HANDLE = ptr::null_mut();
        // SAFETY: NULL template enumerates every filter.
        let code = unsafe { FwpmFilterCreateEnumHandle0(self.0, ptr::null(), &mut enum_handle) };
        self.call("FwpmFilterCreateEnumHandle0", code)?;

        // enumeration in pages of 256; a short page is the last one
        let mut ours = Vec::new();
        let result = loop {
            let mut entries: *mut *mut FWPM_FILTER0 = ptr::null_mut();
            let mut count = 0u32;
            // SAFETY: valid handles and out pointers.
            let code =
                unsafe { FwpmFilterEnum0(self.0, enum_handle, 256, &mut entries, &mut count) };
            if let Err(e) = self.call("FwpmFilterEnum0", code) {
                break Err(e);
            }
            for i in 0..count as usize {
                // SAFETY: WFP returned `count` valid filter pointers.
                let filter = unsafe { &**entries.add(i) };
                // only ours: the split tunnel driver has filters in this sublayer too
                // SAFETY: a non-null provider key points to a GUID owned by the entry.
                let ours_provider = !filter.providerKey.is_null()
                    && same_guid(unsafe { &*filter.providerKey }, &PROVIDER_KEY);
                if ours_provider && same_guid(&filter.subLayerKey, &SUBLAYER_KEY) {
                    ours.push(filter.filterId);
                }
            }
            if !entries.is_null() {
                // SAFETY: memory allocated by FwpmFilterEnum0.
                unsafe { FwpmFreeMemory0(&mut entries as *mut _ as *mut *mut core::ffi::c_void) };
            }
            if count < 256 {
                break Ok(());
            }
        };
        // SAFETY: handle created above.
        unsafe { FwpmFilterDestroyEnumHandle0(self.0, enum_handle) };
        result?;

        // deletion; filters that vanished in the meantime are not an error
        for id in ours {
            // SAFETY: plain id.
            let code = unsafe { FwpmFilterDeleteById0(self.0, id) };
            ignore(
                self.call("FwpmFilterDeleteById0", code),
                &[FILTER_NOT_FOUND, NOT_FOUND],
            )?;
        }
        Ok(())
    }

    /// Adds a persistent filter to our sublayer.
    ///
    /// `layer`: WFP layer key. `weight`: weight inside the sublayer.
    /// `action`: `FWP_ACTION_PERMIT` or `FWP_ACTION_BLOCK`. `conds`: conditions,
    /// all of which must match; none matches every packet.
    fn add_filter(
        &self,
        layer: &GUID,
        name: &str,
        weight: u8,
        action: u32,
        conds: &[Cond],
    ) -> Result<()> {
        let mut name = wide(name);
        let mut provider_key = PROVIDER_KEY;
        // values referenced by the conditions must stay alive until the call returns
        let mut storage = CondStorage::default();
        let mut conditions: Vec<FWPM_FILTER_CONDITION0> =
            conds.iter().map(|c| c.to_condition(&mut storage)).collect();

        let filter = FWPM_FILTER0 {
            displayData: FWPM_DISPLAY_DATA0 {
                name: name.as_mut_ptr(),
                description: ptr::null_mut(),
            },
            flags: FWPM_FILTER_FLAG_PERSISTENT,
            providerKey: &mut provider_key,
            layerKey: *layer,
            subLayerKey: SUBLAYER_KEY,
            weight: FWP_VALUE0 {
                r#type: FWP_UINT8,
                Anonymous: FWP_VALUE0_0 { uint8: weight },
            },
            numFilterConditions: conditions.len() as u32,
            filterCondition: if conditions.is_empty() {
                ptr::null_mut()
            } else {
                conditions.as_mut_ptr()
            },
            action: FWPM_ACTION0 {
                r#type: action,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut id = 0u64;
        // SAFETY: the filter and everything it points to outlive the call.
        let code = unsafe { FwpmFilterAdd0(self.0, &filter, ptr::null_mut(), &mut id) };
        // explicit drop: keeps `storage` alive until after the call
        drop(storage);
        self.call("FwpmFilterAdd0", code)
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: handle opened by FwpmEngineOpen0.
            unsafe { FwpmEngineClose0(self.0) };
        }
    }
}

/// App id of an executable: the blob that WFP compares with the image of a process.
enum AppId {
    /// Returned by `FwpmGetAppIdFromFileName0`, freed on drop.
    Api(*mut FWP_BYTE_BLOB),
    /// Computed from the path; the blob points into the UTF-16 text, which is boxed
    /// with it so that both stay where they are.
    Computed(Box<(FWP_BYTE_BLOB, Vec<u16>)>),
}

impl AppId {
    /// App id of this service's executable, from the system API: its path is NOT
    /// user-controlled, and a wrong id would block the tunnel's own traffic.
    fn current_exe() -> Result<Self> {
        let path = wide(&std::env::current_exe()?.to_string_lossy());
        let mut blob: *mut FWP_BYTE_BLOB = ptr::null_mut();
        // SAFETY: valid path and out pointer.
        let code = unsafe { FwpmGetAppIdFromFileName0(path.as_ptr(), &mut blob) };
        if code != 0 {
            return Err(NetError::System {
                call: "FwpmGetAppIdFromFileName0",
                code,
            });
        }
        Ok(Self::Api(blob))
    }

    /// App id of the executable at `path`, a `X:\...` path chosen by the user.
    /// NB: computed WITHOUT opening the file, unlike `FwpmGetAppIdFromFileName0`. This
    /// service runs as SYSTEM, and a path through a junction and an object manager
    /// link, which any user can create, could make it open a UNC path and send the
    /// machine account's NTLM credentials to another host.
    fn from_path(path: &std::path::Path) -> Result<Self> {
        let device = super::split::device_path(path).ok_or_else(|| {
            NetError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "not a path on a local drive",
            ))
        })?;
        let text = crate::split_driver::app_id(&device);
        let mut owned = Box::new((
            FWP_BYTE_BLOB {
                size: (text.len() * 2) as u32,
                data: ptr::null_mut(),
            },
            text,
        ));
        owned.0.data = owned.1.as_mut_ptr().cast();
        Ok(Self::Computed(owned))
    }

    /// The blob to put in a filter condition, valid as long as `self`.
    fn blob(&self) -> *mut FWP_BYTE_BLOB {
        match self {
            Self::Api(blob) => *blob,
            Self::Computed(owned) => ptr::from_ref(&owned.0).cast_mut(),
        }
    }
}

impl Drop for AppId {
    fn drop(&mut self) {
        if let Self::Api(blob) = self {
            // SAFETY: allocated by FwpmGetAppIdFromFileName0 and freed only here.
            unsafe { FwpmFreeMemory0(blob as *mut _ as *mut *mut core::ffi::c_void) };
        }
    }
}

/// A filter condition, turned into a `FWPM_FILTER_CONDITION0` by
/// [`Cond::to_condition`].
enum Cond {
    /// Loopback traffic.
    Loopback,
    /// Traffic on the interface with this LUID.
    LocalInterface(u64),
    /// Traffic of the application with this app id, owned by an [`AppId`].
    AppId(*mut FWP_BYTE_BLOB),
    /// IP protocol number.
    Protocol(u8),
    LocalPort(u16),
    RemotePort(u16),
    /// Remote IPv4 network (address, prefix length).
    RemoteV4(Ipv4Addr, u8),
    /// Remote IPv6 network (address, prefix length).
    RemoteV6(Ipv6Addr, u8),
}

/// Values that conditions point to, owned for the duration of a WFP call.
/// Boxed so that pointers handed to WFP stay valid while the vectors grow.
#[derive(Default)]
#[allow(clippy::vec_box)]
struct CondStorage {
    u64s: Vec<Box<u64>>,
    v4: Vec<Box<FWP_V4_ADDR_AND_MASK>>,
    v6: Vec<Box<FWP_V6_ADDR_AND_MASK>>,
}

impl Cond {
    /// Builds the WFP condition. Values passed by pointer are stored in
    /// `storage`, which must outlive every use of the result.
    fn to_condition(&self, storage: &mut CondStorage) -> FWPM_FILTER_CONDITION0 {
        let value = |kind, data| FWP_CONDITION_VALUE0 {
            r#type: kind,
            Anonymous: data,
        };
        let (field, match_type, value) = match *self {
            Cond::Loopback => (
                FWPM_CONDITION_FLAGS,
                FWP_MATCH_FLAGS_ALL_SET,
                value(
                    FWP_UINT32,
                    FWP_CONDITION_VALUE0_0 {
                        uint32: FWP_CONDITION_FLAG_IS_LOOPBACK,
                    },
                ),
            ),
            Cond::LocalInterface(luid) => {
                let mut boxed = Box::new(luid);
                let ptr: *mut u64 = &mut *boxed;
                storage.u64s.push(boxed);
                (
                    FWPM_CONDITION_IP_LOCAL_INTERFACE,
                    FWP_MATCH_EQUAL,
                    value(FWP_UINT64, FWP_CONDITION_VALUE0_0 { uint64: ptr }),
                )
            }
            Cond::AppId(blob) => (
                FWPM_CONDITION_ALE_APP_ID,
                FWP_MATCH_EQUAL,
                value(
                    FWP_BYTE_BLOB_TYPE,
                    FWP_CONDITION_VALUE0_0 { byteBlob: blob },
                ),
            ),
            Cond::Protocol(proto) => (
                FWPM_CONDITION_IP_PROTOCOL,
                FWP_MATCH_EQUAL,
                value(FWP_UINT8, FWP_CONDITION_VALUE0_0 { uint8: proto }),
            ),
            Cond::LocalPort(port) => (
                FWPM_CONDITION_IP_LOCAL_PORT,
                FWP_MATCH_EQUAL,
                value(FWP_UINT16, FWP_CONDITION_VALUE0_0 { uint16: port }),
            ),
            Cond::RemotePort(port) => (
                FWPM_CONDITION_IP_REMOTE_PORT,
                FWP_MATCH_EQUAL,
                value(FWP_UINT16, FWP_CONDITION_VALUE0_0 { uint16: port }),
            ),
            Cond::RemoteV4(addr, len) => {
                // a /0 prefix needs a special case: a 32-bit shift would overflow
                let mask = if len == 0 {
                    0
                } else {
                    u32::MAX << (32 - u32::from(len))
                };
                // WFP takes IPv4 addresses in host byte order
                let mut boxed = Box::new(FWP_V4_ADDR_AND_MASK {
                    addr: u32::from(addr),
                    mask,
                });
                let ptr: *mut FWP_V4_ADDR_AND_MASK = &mut *boxed;
                storage.v4.push(boxed);
                (
                    FWPM_CONDITION_IP_REMOTE_ADDRESS,
                    FWP_MATCH_EQUAL,
                    value(FWP_V4_ADDR_MASK, FWP_CONDITION_VALUE0_0 { v4AddrMask: ptr }),
                )
            }
            Cond::RemoteV6(addr, len) => {
                let mut boxed = Box::new(FWP_V6_ADDR_AND_MASK {
                    addr: addr.octets(),
                    prefixLength: len,
                });
                let ptr: *mut FWP_V6_ADDR_AND_MASK = &mut *boxed;
                storage.v6.push(boxed);
                (
                    FWPM_CONDITION_IP_REMOTE_ADDRESS,
                    FWP_MATCH_EQUAL,
                    value(FWP_V6_ADDR_MASK, FWP_CONDITION_VALUE0_0 { v6AddrMask: ptr }),
                )
            }
        };
        FWPM_FILTER_CONDITION0 {
            fieldKey: field,
            matchType: match_type,
            conditionValue: value,
        }
    }
}

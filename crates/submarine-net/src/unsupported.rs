//! Fallback backend for the platforms without a real implementation (anything but
//! Linux, Windows and macOS). It exposes the same types as the real backends so the
//! daemon compiles everywhere: operations that would leave the system unprotected or
//! misconfigured fail with [`NetError::Unsupported`], while resets are no-ops.

use std::path::PathBuf;

use crate::{FirewallPolicy, NetError, Result, RouteOptions, SplitMode, TunnelNetConfig};

/// Linux-only concept, kept so callers compile on every platform.
pub const FWMARK: u32 = 0;
/// Linux-only concept, kept so callers compile on every platform.
pub const ROUTING_TABLE: u32 = 0;

/// Route manager that cannot route anything.
pub struct RouteManager;

impl RouteManager {
    /// Creates the route manager; never fails.
    pub fn new() -> Result<Self> {
        Ok(Self)
    }

    /// Follows physical network changes; only needed on Windows.
    pub async fn refresh(&mut self, _cfg: &TunnelNetConfig) -> Result<Option<u32>> {
        Ok(None)
    }

    /// Always fails with [`NetError::Unsupported`].
    pub async fn apply(&mut self, _cfg: &TunnelNetConfig, _opts: RouteOptions) -> Result<()> {
        Err(NetError::Unsupported)
    }

    /// Nothing to remove.
    pub async fn reset(&mut self) -> Result<()> {
        Ok(())
    }
}

/// DNS manager that cannot configure DNS.
#[derive(Default)]
pub struct DnsManager;

impl DnsManager {
    /// Creates the DNS manager.
    pub fn new() -> Self {
        Self
    }

    /// Always fails with [`NetError::Unsupported`].
    pub async fn apply(&mut self, _cfg: &TunnelNetConfig) -> Result<()> {
        Err(NetError::Unsupported)
    }

    /// Nothing to restore.
    pub async fn reset(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Firewall without kill switch or split tunneling support.
#[derive(Default)]
pub struct Firewall;

impl Firewall {
    /// Creates the firewall.
    pub fn new() -> Self {
        Self
    }

    /// Accepts only a policy that needs no rules at all.
    ///
    /// # Errors
    /// [`NetError::Unsupported`] when the kill switch or split tunneling is requested,
    /// so the user is NOT left believing that traffic is protected.
    pub async fn apply(&mut self, policy: &FirewallPolicy) -> Result<()> {
        if policy.block || policy.split != SplitMode::Off {
            return Err(NetError::Unsupported);
        }
        Ok(())
    }

    /// Nothing to remove.
    pub async fn reset(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Split tunnel that cannot track any app.
#[derive(Default)]
pub struct SplitTunnel;

impl SplitTunnel {
    /// Creates the split tunnel.
    pub fn new() -> Self {
        Self
    }

    /// Whether split tunneling can work on this computer: never here.
    pub fn available() -> bool {
        false
    }

    /// Always fails with [`NetError::Unsupported`].
    pub async fn start(
        &mut self,
        _mode: SplitMode,
        _apps: &[PathBuf],
        _tunnel: Option<&TunnelNetConfig>,
    ) -> Result<()> {
        Err(NetError::Unsupported)
    }

    /// Nothing to stop.
    pub async fn stop(&mut self) -> Result<()> {
        Ok(())
    }

    /// Windows: the physical network changed; no-op elsewhere.
    pub async fn network_changed(&mut self) -> Result<()> {
        Ok(())
    }
}

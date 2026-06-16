//! macOS backend, through the system tools: `route` for routes,
//! `networksetup` for DNS and `pfctl` for the kill switch. Per-app split
//! tunneling needs a Network Extension and is not available yet.

mod dns;
mod firewall;
mod route;

use std::path::PathBuf;
use std::process::Stdio;

use tokio::io::AsyncWriteExt;

pub use dns::DnsManager;
pub use firewall::Firewall;
pub use route::{FWMARK, ROUTING_TABLE, RouteManager};

use crate::{NetError, Result, SplitMode, TunnelNetConfig};

/// Persistent state (the daemon's default data dir): `networksetup` changes
/// survive reboots, so their backup must too.
const STATE_DIR: &str = "/Library/Application Support/Submarine";

/// Runs a command, optionally feeding `input` on stdin; returns stdout and stderr.
///
/// # Errors
/// [`NetError::Command`] with the trimmed stderr when the command exits with an error,
/// [`NetError::Io`] when it cannot be started.
async fn exec(program: &str, args: &[&str], input: Option<&str>) -> Result<(String, String)> {
    // stdin is piped only when there is something to feed
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(input) = input {
        child
            .stdin
            .take()
            .expect("stdin is piped")
            .write_all(input.as_bytes())
            .await?;
    }
    // both outputs are returned: some tools (e.g. `pfctl`) print useful data on stderr
    let output = child.wait_with_output().await?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if !output.status.success() {
        return Err(NetError::Command {
            command: format!("{program} {}", args.join(" ")),
            stderr: stderr.trim().to_owned(),
        });
    }
    Ok((stdout, stderr))
}

/// Per-app split tunneling, not available on macOS yet.
#[derive(Default)]
pub struct SplitTunnel;

impl SplitTunnel {
    /// Creates the split tunnel.
    pub fn new() -> Self {
        Self
    }

    /// Always fails with [`NetError::Unsupported`].
    pub async fn start(
        &mut self,
        _mode: SplitMode,
        _apps: &[PathBuf],
        _tunnel: Option<&TunnelNetConfig>,
    ) -> Result<()> {
        // per-app split tunneling on macOS needs a Network Extension, NOT implemented yet
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

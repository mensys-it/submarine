//! DNS through `networksetup`, like wg-quick on macOS: every enabled network
//! service gets the tunnel's DNS servers. The previous values are saved to a
//! file first, so they are restored even after a crash or a reboot.

use std::path::Path;

use super::{STATE_DIR, exec};
use crate::macos_text::{ServiceDns, format_backup, parse_backup, parse_list, parse_services};
use crate::{Result, TunnelNetConfig};

/// Saved DNS settings of every service, in the format of [`format_backup`].
const BACKUP: &str = "/Library/Application Support/Submarine/dns-backup";

/// Sets the tunnel's DNS on every enabled network service.
pub struct DnsManager;

impl DnsManager {
    /// Creates the DNS manager; all its state lives in the backup file.
    pub fn new() -> Self {
        Self
    }

    /// Applies the DNS servers and search domains of `cfg` to every enabled service,
    /// replacing any previous configuration. Without DNS servers the system
    /// configuration is left alone.
    pub async fn apply(&mut self, cfg: &TunnelNetConfig) -> Result<()> {
        // restore of the previous configuration, including that of a crashed run
        self.reset().await?;
        if cfg.dns_servers.is_empty() {
            return Ok(());
        }
        // read of the current settings of every enabled service
        let (out, _) = exec("networksetup", &["-listallnetworkservices"], None).await?;
        let mut backup = Vec::new();
        for service in parse_services(&out) {
            let (servers, _) = exec("networksetup", &["-getdnsservers", &service], None).await?;
            let (search, _) = exec("networksetup", &["-getsearchdomains", &service], None).await?;
            backup.push(ServiceDns {
                service,
                servers: parse_list(&servers),
                search: parse_list(&search),
            });
        }
        // the backup is written BEFORE any change, so a crash can always be undone
        tokio::fs::create_dir_all(STATE_DIR).await?;
        tokio::fs::write(BACKUP, format_backup(&backup)).await?;

        // the tunnel's settings on every service
        let servers: Vec<String> = cfg.dns_servers.iter().map(ToString::to_string).collect();
        for entry in &backup {
            set(&entry.service, &servers, &cfg.dns_search).await?;
        }
        tracing::info!(servers = ?cfg.dns_servers, services = backup.len(), "dns applied");
        Ok(())
    }

    /// Restores the saved settings, including those of a crashed run.
    pub async fn reset(&mut self) -> Result<()> {
        let Ok(text) = tokio::fs::read_to_string(BACKUP).await else {
            return Ok(());
        };
        for entry in parse_backup(&text) {
            // a service may have been removed meanwhile
            if let Err(err) = set(&entry.service, &entry.servers, &entry.search).await {
                tracing::warn!(service = %entry.service, "dns restore failed: {err}");
            }
        }
        if Path::new(BACKUP).exists() {
            tokio::fs::remove_file(BACKUP).await?;
        }
        tracing::info!("dns restored");
        Ok(())
    }
}

impl Default for DnsManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Sets the DNS servers and search domains of a network service.
/// An empty list means "back to automatic (DHCP)".
async fn set(service: &str, servers: &[String], search: &[String]) -> Result<()> {
    // `networksetup` takes the literal "Empty" to clear a list
    let empty = ["Empty".to_owned()];
    let values = |list: &[String]| {
        if list.is_empty() {
            empty.to_vec()
        } else {
            list.to_vec()
        }
    };
    let servers = values(servers);
    let search = values(search);

    let mut args = vec!["-setdnsservers", service];
    args.extend(servers.iter().map(String::as_str));
    exec("networksetup", &args, None).await?;
    let mut args = vec!["-setsearchdomains", service];
    args.extend(search.iter().map(String::as_str));
    exec("networksetup", &args, None).await?;
    Ok(())
}

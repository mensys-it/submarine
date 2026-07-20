//! Command line client of Submarine, mainly a development and diagnostic tool.
//!
//! It works in two ways:
//! - without the daemon, `up` brings a tunnel up directly from a `.conf` file,
//!   with its routes and DNS, until Ctrl+C (it needs the privileges of the daemon);
//! - through the daemon, every other command sends one IPC request on the socket
//!   (from `SUBMARINE_SOCKET` or the platform default) and prints the response as
//!   JSON.
//!
//! ```text
//! submarine-cli up <file.conf> [interface-name]
//! submarine-cli import <name> <file.conf>
//! submarine-cli list | status | disconnect
//! submarine-cli connect <tunnel-id> | delete <tunnel-id>
//! submarine-cli settings | set-settings '<json>'
//! submarine-cli logs | clear-logs
//! ```

use std::process::ExitCode;
use std::time::Duration;

use submarine_ipc::{Client, Request};
use submarine_net::{DnsManager, FWMARK, RouteManager, RouteOptions, TunnelNetConfig};
use submarine_tunnel::{Tunnel, TunnelOptions};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    // logs on stderr, filtered by RUST_LOG (default `info`)
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    // parsing of the command line into a daemon request; `up` runs on its own
    let args: Vec<String> = std::env::args().collect();
    let arg = |i: usize| args.get(i).map(String::as_str);
    let request = match (arg(1), arg(2), arg(3)) {
        (Some("up"), Some(path), name) => {
            return report(up(path, name.map(str::to_owned)).await);
        }
        (Some("import"), Some(name), Some(path)) => match std::fs::read_to_string(path) {
            Ok(config) => Request::ImportTunnel {
                name: name.into(),
                config,
            },
            Err(err) => return report(Err(err.into())),
        },
        (Some("list"), None, None) => Request::ListTunnels,
        (Some("status"), None, None) => Request::GetStatus,
        (Some("connect"), Some(id), None) => Request::Connect { id: id.into() },
        (Some("delete"), Some(id), None) => Request::DeleteTunnel { id: id.into() },
        (Some("disconnect"), None, None) => Request::Disconnect,
        (Some("settings"), None, None) => Request::GetSettings,
        (Some("logs"), None, None) => Request::GetLogs,
        (Some("clear-logs"), None, None) => Request::ClearLogs,
        (Some("set-settings"), Some(json), None) => match serde_json::from_str(json) {
            Ok(settings) => Request::SetSettings { settings },
            Err(err) => return report(Err(err.into())),
        },
        _ => {
            eprintln!(
                "usage: {0} up <file.conf> [interface-name]\n       {0} import <name> <file.conf> | list | status | connect <id> | delete <id> | disconnect\n       {0} settings | set-settings <json> | logs | clear-logs",
                args[0]
            );
            return ExitCode::from(2);
        }
    };
    report(call(request).await)
}

/// Sends `request` to the daemon and prints its response as pretty JSON.
///
/// # Errors
///
/// Fails if the daemon socket cannot be reached or the exchange fails.
async fn call(request: Request) -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::connect(&submarine_ipc::socket_path()).await?;
    let response = client.request(request).await?;
    println!("{}", serde_json::to_string_pretty(&response)?);
    Ok(())
}

/// Prints the error of `result`, if any, and turns it into the exit code.
fn report(result: Result<(), Box<dyn std::error::Error>>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Brings up the tunnel described by the `.conf` at `path`, with its routes and
/// DNS, and keeps it up until Ctrl+C or a tunnel failure.
///
/// The interface is called `name`, or `submarine0` by default. Routes and DNS are
/// ALWAYS reset before returning, even when applying them failed half-way.
///
/// # Errors
///
/// Fails if the file cannot be read or parsed, if the tunnel cannot start, or if
/// routes or DNS cannot be applied or reset.
async fn up(path: &str, name: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    // parsing of the configuration, whose warnings are only logged
    let parsed = submarine_config::parse(&std::fs::read_to_string(path)?)?;
    for warning in &parsed.warnings {
        tracing::warn!("{path}:{}: {}", warning.line, warning.message);
    }
    let config = parsed.config;

    // start of the tunnel, with the fwmark that keeps its own traffic off the tunnel
    let options = TunnelOptions {
        name: Some(name.unwrap_or_else(|| "submarine0".into())),
        fwmark: Some(FWMARK),
        endpoints: None,
    };
    let tunnel = Tunnel::start(&config, options).await?;
    // network parameters for routes and DNS
    let net = TunnelNetConfig {
        if_name: tunnel.interface_name().to_owned(),
        if_index: tunnel.interface_index(),
        addresses: config.interface.addresses.clone(),
        allowed_ips: config
            .peers
            .iter()
            .flat_map(|p| p.allowed_ips.iter().copied())
            .collect(),
        endpoints: tunnel.endpoints().to_vec(),
        dns_servers: config.interface.dns_servers.clone(),
        dns_search: config.interface.dns_search.clone(),
    };

    // routes and DNS applied, then wait until stopped
    let mut routes = RouteManager::new()?;
    let mut dns = DnsManager::new();
    let result = async {
        routes.apply(&net, RouteOptions::default()).await?;
        dns.apply(&net).await?;
        run_until_stopped(&tunnel).await;
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;

    // restore of the system, ALWAYS, even if applying failed half-way
    // NB: the interface is removed only after routes and DNS are reset
    dns.reset().await?;
    routes.reset().await?;
    drop(tunnel);
    result
}

/// Waits for Ctrl+C or a tunnel failure, logging the peer statistics every five
/// seconds in the meantime.
async fn run_until_stopped(tunnel: &Tunnel) {
    let mut failure = tunnel.failure();
    let mut report = tokio::time::interval(Duration::from_secs(5));
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = failure.changed() => {
                tracing::error!("tunnel failed: {:?}", *failure.borrow());
                break;
            }
            _ = report.tick() => {
                for peer in tunnel.stats() {
                    tracing::info!(
                        peer = %peer.public_key,
                        endpoint = ?peer.endpoint,
                        handshake = ?peer.last_handshake.and_then(|t| t.elapsed().ok()),
                        rx = peer.rx_bytes,
                        tx = peer.tx_bytes,
                    );
                }
            }
        }
    }
}

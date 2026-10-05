//! Privileged Submarine service. It owns the tunnels and every OS change made for
//! them (routes, DNS, firewall); the UI talks to it through `submarine-ipc`.
//!
//! This file holds the entry point: command line parsing, logging setup, the IPC
//! socket permissions and the main loop that runs the service until shutdown.
//!
//! Usage:
//!   submarine-daemon [--socket <path>] [--data-dir <path>]   run in the foreground
//!   submarine-daemon reset-firewall                          remove kill switch rules
//!   submarine-daemon service install|uninstall|run           Windows service (Windows only)
//!
//! `--socket` and `--data-dir` can also be set with SUBMARINE_SOCKET and
//! SUBMARINE_DATA_DIR.

mod logbuf;
mod server;
mod service;
mod store;
mod vault;
#[cfg(windows)]
mod winsvc;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use tracing_subscriber::{EnvFilter, Layer};

use crate::service::Service;
use crate::store::TunnelStore;

/// Group allowed to use the IPC socket. On macOS every local user is in `staff`,
/// mirroring Windows, where every interactive user can talk to the service.
#[cfg(all(unix, not(target_os = "macos")))]
const SOCKET_GROUP: &str = "submarine";
#[cfg(target_os = "macos")]
const SOCKET_GROUP: &str = "staff";

/// Size in bytes above which the log file is discarded at startup, so it cannot grow
/// without bound.
const MAX_LOG_SIZE: u64 = 10 * 1024 * 1024;

/// Runtime configuration of the daemon.
pub struct Config {
    /// Name or path of the IPC socket the daemon listens on.
    pub socket: String,
    /// Directory holding tunnels, settings and state.
    pub data_dir: PathBuf,
}

impl Config {
    /// Builds the configuration from the command line, falling back to the
    /// `SUBMARINE_SOCKET` / `SUBMARINE_DATA_DIR` environment variables and then
    /// to the platform defaults.
    fn from_args(args: &[String]) -> Self {
        // value following `flag` on the command line, otherwise the `env` variable
        let option = |flag: &str, env: &str| {
            args.iter()
                .position(|a| a == flag)
                .and_then(|i| args.get(i + 1).cloned())
                .or_else(|| std::env::var(env).ok())
        };
        Self {
            socket: option("--socket", "SUBMARINE_SOCKET")
                .unwrap_or_else(submarine_ipc::socket_path),
            data_dir: option("--data-dir", "SUBMARINE_DATA_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(default_data_dir),
        }
    }
}

/// Default data directory of the platform: `%ProgramData%\Submarine` on Windows,
/// `/Library/Application Support/Submarine` on macOS, `/var/lib/submarine` elsewhere.
fn default_data_dir() -> PathBuf {
    if cfg!(windows) {
        let base = std::env::var_os("ProgramData").unwrap_or_else(|| r"C:\ProgramData".into());
        PathBuf::from(base).join("Submarine")
    } else if cfg!(target_os = "macos") {
        "/Library/Application Support/Submarine".into()
    } else {
        "/var/lib/submarine".into()
    }
}

/// Initializes logging: to stderr, or to `file` when running without a console
/// (Windows service). Every event is also copied to the in-memory buffer served
/// to the UI (see `logbuf`). If `file` cannot be opened, stderr is used.
pub fn init_logging(file: Option<&Path>) {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    // level from RUST_LOG, `info` by default
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    // log file opened in append mode, discarded first when it is too large
    let log_file = file.and_then(|path| {
        if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_LOG_SIZE) {
            let _ = std::fs::remove_file(path);
        }
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()
    });
    // output layer: plain text to the file (no ANSI colors), or stderr
    let output = match log_file {
        Some(file) => tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(file))
            .boxed(),
        None => tracing_subscriber::fmt::layer().boxed(),
    };
    // registration of the global subscriber
    tracing_subscriber::registry()
        .with(filter)
        .with(output)
        .with(logbuf::BufferLayer)
        .init();

    // panics are logged too: a service has no stderr, so without this hook they
    // would leave no trace (the default hook still runs afterwards)
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        tracing::error!(thread = thread.name().unwrap_or("?"), "panic: {info}");
        default_hook(info);
    }));
}

/// Creates the multi-threaded tokio runtime.
///
/// # Panics
/// Panics if the runtime cannot be built.
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
}

/// Entry point: dispatches on the first argument (subcommand or options).
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let result = match args.get(1).map(String::as_str) {
        // removal of the kill switch rules, then exit
        Some("reset-firewall") => {
            init_logging(None);
            runtime().block_on(reset_firewall())
        }
        // Windows service management, handled entirely by `winsvc`
        #[cfg(windows)]
        Some("service") => return winsvc::command(args.get(2).map(String::as_str), &args),
        // usage
        Some("-h" | "--help") => {
            eprintln!(
                "usage: {0} [--socket <path>] [--data-dir <path>]\n       {0} reset-firewall",
                args[0]
            );
            #[cfg(windows)]
            eprintln!("       {} service install|uninstall|run", args[0]);
            return ExitCode::SUCCESS;
        }
        // default: daemon in the foreground until SIGTERM / Ctrl+C
        _ => {
            init_logging(None);
            runtime().block_on(run(Config::from_args(&args), shutdown_signal()))
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!("{err}");
            ExitCode::FAILURE
        }
    }
}

/// Runs the service until `shutdown` completes, then stops the tunnel.
///
/// Shared by the foreground mode and the Windows service, which only differ in
/// the `shutdown` future (signals vs. SCM stop request).
///
/// # Errors
/// Fails if the data directory or the socket cannot be set up, or if the IPC
/// server stops with an error.
pub async fn run(config: Config, shutdown: impl Future<Output = ()>) -> std::io::Result<()> {
    // storage and service: the service restores the previous connection, if any
    let store = TunnelStore::open(&config.data_dir)?;
    let service = Service::new(store).await;

    // IPC socket, restricted to the allowed group on Unix
    let listener = submarine_ipc::bind(&config.socket)?;
    #[cfg(unix)]
    secure_socket(&config.socket)?;
    tracing::info!(socket = %config.socket, data_dir = %config.data_dir.display(), "submarine daemon ready");

    // clients are served until the shutdown request
    tokio::select! {
        result = server::serve(listener, service.clone()) => result?,
        () = shutdown => tracing::info!("shutting down"),
    }
    service.shutdown().await;
    Ok(())
}

/// Removes every kill switch rule, e.g. before uninstalling.
///
/// # Errors
/// Fails if the firewall rules cannot be removed.
pub async fn reset_firewall() -> std::io::Result<()> {
    submarine_net::Firewall::new()
        .reset()
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    tracing::info!("firewall rules removed");
    Ok(())
}

/// Restricts the socket so that only root and members of `SOCKET_GROUP` may talk
/// to the daemon. If the group does not exist, only root can use the socket.
#[cfg(unix)]
fn secure_socket(path: &str) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::fs::PermissionsExt;

    // lookup of the group id
    let group = CString::new(SOCKET_GROUP).expect("no NUL in group name");
    // SAFETY: getgrnam receives a valid C string; the result is only read immediately.
    let gid = unsafe {
        let entry = libc::getgrnam(group.as_ptr());
        (!entry.is_null()).then(|| (*entry).gr_gid)
    };
    // group ownership of the socket and matching permissions
    let mode = match gid {
        Some(gid) => {
            let c_path = CString::new(path)?;
            // SAFETY: valid C string path; uid -1 leaves the owner unchanged.
            if unsafe { libc::chown(c_path.as_ptr(), u32::MAX, gid) } != 0 {
                return Err(std::io::Error::last_os_error());
            }
            0o660
        }
        None => {
            tracing::warn!("group `{SOCKET_GROUP}` not found: only root can use the socket");
            0o600
        }
    };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

/// Completes on SIGTERM or Ctrl+C (only Ctrl+C outside Unix).
///
/// # Panics
/// Panics if the SIGTERM handler cannot be installed.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
        tokio::select! {
            _ = term.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}

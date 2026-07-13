//! Windows service integration: install/uninstall through the Service Control
//! Manager (SCM), and the entry point the SCM calls (`service run`).
//!
//! The service runs as LocalSystem, starts automatically at boot and is restarted
//! by the SCM after a crash.

use std::ffi::OsString;
use std::process::ExitCode;
use std::time::Duration;

use windows_service::service::{
    ServiceAccess, ServiceAction, ServiceActionType, ServiceControl, ServiceControlAccept,
    ServiceErrorControl, ServiceExitCode, ServiceFailureActions, ServiceFailureResetPeriod,
    ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_service::{define_windows_service, service_dispatcher};

use crate::{Config, init_logging, reset_firewall, run, runtime};

/// Name the service is registered with in the SCM.
const SERVICE_NAME: &str = "Submarine";
/// The service runs in its own process.
const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;

/// Handles `service install|uninstall|run` and returns the process exit code
/// (2 for an unknown or missing action).
///
/// `action`: the subcommand; `args`: the full command line, used for the usage
/// message.
pub fn command(action: Option<&str>, args: &[String]) -> ExitCode {
    let result = match action {
        Some("install") => install(),
        Some("uninstall") => uninstall(),
        Some("run") => service_dispatcher::start(SERVICE_NAME, ffi_service_main),
        _ => {
            eprintln!("usage: {} service install|uninstall|run", args[0]);
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Registers the service (auto start, LocalSystem account) and starts it.
///
/// # Errors
/// Fails if the service cannot be created, configured or started, e.g. when not
/// running as administrator or when it is already installed.
fn install() -> windows_service::Result<()> {
    // connection to the SCM and creation of the service
    let manager = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )?;
    let info = ServiceInfo {
        name: SERVICE_NAME.into(),
        display_name: "Submarine VPN".into(),
        service_type: SERVICE_TYPE,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: std::env::current_exe().map_err(windows_service::Error::Winapi)?,
        launch_arguments: vec!["service".into(), "run".into()],
        dependencies: vec![],
        // no account: the service runs as LocalSystem
        account_name: None,
        account_password: None,
    };
    let service = manager.create_service(
        &info,
        ServiceAccess::CHANGE_CONFIG | ServiceAccess::START | ServiceAccess::QUERY_STATUS,
    )?;
    service.set_description("Manages Submarine VPN tunnels, routes, DNS and the kill switch.")?;
    // restart after a crash (up to three times, counter reset after an hour), so
    // the kill switch is re-evaluated quickly
    let restart = ServiceAction {
        action_type: ServiceActionType::Restart,
        delay: Duration::from_secs(2),
    };
    service.update_failure_actions(ServiceFailureActions {
        reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(3600)),
        reboot_msg: None,
        command: None,
        actions: Some(vec![restart.clone(), restart.clone(), restart]),
    })?;
    // first start, without waiting for the next boot
    service.start::<OsString>(&[])?;
    println!("service {SERVICE_NAME} installed and started");
    Ok(())
}

/// Stops and removes the service, then removes the split tunnel driver and every
/// kill switch rule.
///
/// # Errors
/// Fails if the service cannot be opened, deleted or the firewall reset. A
/// failure removing the split tunnel driver is only reported as a warning.
fn uninstall() -> windows_service::Result<()> {
    // connection to the SCM and opening of the service
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
    let service = manager.open_service(
        SERVICE_NAME,
        ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE,
    )?;
    // stop request, waiting up to 10 seconds for the service to stop
    let mut stopped = service.query_status()?.current_state == ServiceState::Stopped;
    if !stopped {
        let _ = service.stop();
        for _ in 0..50 {
            if service.query_status()?.current_state == ServiceState::Stopped {
                stopped = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    // NB: a service still running is only marked for deletion by Windows, and it may
    // still change the firewall after the reset below: the user is warned
    if !stopped {
        eprintln!(
            "warning: service {SERVICE_NAME} did not stop within 10 seconds, it will be \
             removed when it stops (or at the next reboot)"
        );
    }
    // removal of the service and of the split tunnel driver
    service.delete()?;
    if let Err(err) = submarine_net::remove_split_driver() {
        eprintln!("warning: split tunnel driver not removed: {err}");
    }
    // removal of the firewall rules: without the service nobody could lift a
    // kill switch block anymore
    runtime()
        .block_on(reset_firewall())
        .map_err(windows_service::Error::Winapi)?;
    println!("service {SERVICE_NAME} removed");
    Ok(())
}

// generation of the `extern "system"` entry point the SCM dispatcher calls,
// which forwards to `service_main`
define_windows_service!(ffi_service_main, service_main);

/// Service entry point, called by the SCM dispatcher on its own thread.
///
/// The SCM passes no useful arguments: the configuration comes from the
/// environment and the defaults. Logs go to `daemon.log` in the data directory.
fn service_main(_arguments: Vec<OsString>) {
    let config = Config::from_args(&[]);
    let _ = std::fs::create_dir_all(&config.data_dir);
    init_logging(Some(&config.data_dir.join("daemon.log")));
    if let Err(err) = run_service(config) {
        tracing::error!("service failed: {err}");
    }
}

/// Reports the service as running to the SCM, runs the daemon until a stop or
/// shutdown request, then reports it as stopped.
///
/// # Errors
/// Fails if the control handler cannot be registered or the status cannot be
/// reported. A failure of the daemon itself is only logged.
fn run_service(config: Config) -> windows_service::Result<()> {
    // control handler: stop and system shutdown trigger the daemon shutdown
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let handler = move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            let _ = stop_tx.send(true);
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    };
    // registration of the handler and helper reporting the state to the SCM
    let status = service_control_handler::register(SERVICE_NAME, handler)?;
    let set_state = |state, accept| {
        status.set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: state,
            controls_accepted: accept,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::from_secs(10),
            process_id: None,
        })
    };
    set_state(
        ServiceState::Running,
        ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
    )?;

    // daemon run until the handler signals the stop
    let mut stop_rx = stop_rx;
    let shutdown = async move {
        let _ = stop_rx.wait_for(|stop| *stop).await;
    };
    if let Err(err) = runtime().block_on(run(config, shutdown)) {
        tracing::error!("{err}");
    }
    set_state(ServiceState::Stopped, ServiceControlAccept::empty())
}

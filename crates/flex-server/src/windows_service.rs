//! Windows service integration for flex-server.
//!
//! This module is only compiled on Windows. It provides service management
//! (install, uninstall, start, stop, status) and the service entry point
//! used by the Service Control Manager.

#![cfg(windows)]

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use tokio::runtime::Runtime;
use tokio::sync::oneshot;
use tracing::{error, info};
use windows_service::define_windows_service;
use windows_service::service::{
    ServiceAccess, ServiceControl, ServiceControlAccept, ServiceErrorControl, ServiceExitCode,
    ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

use crate::config::Cli;
use crate::run_server;

const SERVICE_NAME: &str = "flex-server";
const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;

pub fn run() -> Result<()> {
    windows_service::service_dispatcher::start(SERVICE_NAME, ffi_service_main)
        .map_err(|e| anyhow!("failed to start service dispatcher: {e}"))
}

define_windows_service!(ffi_service_main, service_main);

fn service_main(_args: Vec<OsString>) {
    if let Err(err) = run_service() {
        error!(error = %err, "service failed");
    }
}

fn run_service() -> Result<()> {
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let shutdown_tx = std::sync::Mutex::new(Some(shutdown_tx));

    let event_handler = move |control_event: ServiceControl| -> ServiceControlHandlerResult {
        match control_event {
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            ServiceControl::Stop | ServiceControl::Preshutdown | ServiceControl::Shutdown => {
                if let Ok(mut tx) = shutdown_tx.lock() {
                    if let Some(tx) = tx.take() {
                        let _ = tx.send(());
                    }
                }
                ServiceControlHandlerResult::NoError
            }
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };

    let status_handle = service_control_handler::register(SERVICE_NAME, event_handler)
        .map_err(|e| anyhow!("failed to register service control handler: {e}"))?;

    // Tell SCM we're starting.
    status_handle
        .set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: ServiceState::StartPending,
            controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::from_secs(10),
            process_id: None,
        })
        .map_err(|e| anyhow!("failed to set service status to start pending: {e}"))?;

    // Build CLI from environment; the SCM passes minimal args, so use defaults/env.
    let cli = Cli::parse();

    let rt = Runtime::new().context("failed to create tokio runtime")?;
    rt.block_on(async {
        // Signal future that completes when we receive stop from SCM.
        let shutdown = async {
            let _ = shutdown_rx.await;
        };

        let result = run_server(cli, shutdown).await;
        if let Err(err) = &result {
            error!(error = %err, "server exited with error");
        }
        result
    })?;

    // Tell SCM we're stopping.
    status_handle
        .set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: ServiceState::StopPending,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::from_secs(5),
            process_id: None,
        })
        .map_err(|e| anyhow!("failed to set service status to stop pending: {e}"))?;

    // Mark stopped.
    status_handle
        .set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: ServiceState::Stopped,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::from_secs(0),
            process_id: None,
        })
        .map_err(|e| anyhow!("failed to set service status to stopped: {e}"))?;

    Ok(())
}

fn service_exe_path() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("failed to get current executable path")?;
    Ok(exe)
}

fn make_bin_path(exe: &Path, args: &[String]) -> String {
    use std::ffi::OsStr;
    let mut parts: Vec<String> = Vec::new();
    parts.push(quote_arg(exe.as_os_str()));
    for a in args {
        parts.push(quote_arg(OsStr::new(a)));
    }
    parts.join(" ")
}

fn quote_arg(s: &OsStr) -> String {
    let s = s.to_string_lossy();
    if s.chars().any(|c| c == ' ' || c == '\t' || c == '\"') {
        format!("\"{}\"", s.replace('\"', "\"\""))
    } else {
        s.into_owned()
    }
}

pub fn install(
    name: &str,
    display: &str,
    description: &str,
    config: Option<PathBuf>,
    working_dir: Option<PathBuf>,
    database: Option<PathBuf>,
    cache_dir: Option<PathBuf>,
) -> Result<()> {
    let manager_access = ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE;
    let manager = ServiceManager::local_computer(None::<&str>, manager_access)
        .context("failed to open service manager")?;

    let exe = service_exe_path()?;
    let mut args: Vec<String> = Vec::new();
    if let Some(cfg) = config {
        let cfg = cfg.canonicalize().unwrap_or(cfg);
        let wd = working_dir
            .clone()
            .or_else(|| cfg.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        args.push("--config".to_string());
        args.push(cfg.to_string_lossy().into_owned());
        if working_dir.is_none() {
            std::env::set_current_dir(&wd)?;
        }
    }
    if let Some(db) = database {
        args.push("--database".to_string());
        args.push(db.to_string_lossy().into_owned());
    }
    if let Some(cache) = cache_dir {
        args.push("--cache-dir".to_string());
        args.push(cache.to_string_lossy().into_owned());
    }

    let bin_path = make_bin_path(&exe, &args);
    let service_access = ServiceAccess::CHANGE_CONFIG
        | ServiceAccess::START
        | ServiceAccess::QUERY_STATUS
        | ServiceAccess::STOP;
    let service_info = ServiceInfo {
        name: name.to_string().into(),
        display_name: display.to_string().into(),
        service_type: SERVICE_TYPE,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: bin_path.clone().into(),
        launch_arguments: vec![],
        dependencies: vec![],
        account_name: None, // LocalSystem
        account_password: None,
    };
    let service = manager
        .create_service(&service_info, service_access)
        .context("failed to create service")?;
    // Set description if supported.
    let _ = service.set_description(description);
    info!("installed service {name}");
    Ok(())
}

pub fn uninstall(name: &str, force: bool) -> Result<()> {
    let manager_access = ServiceManagerAccess::CONNECT;
    let manager = ServiceManager::local_computer(None::<&str>, manager_access)
        .context("failed to open service manager")?;

    let service_access = ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE;
    let service = manager
        .open_service(name, service_access)
        .context("failed to open service; is it installed?")?;

    if force {
        let _ = stop(name);
    }

    // Stop if running.
    let status = service
        .query_status()
        .context("failed to query service status")?;
    if status.current_state == ServiceState::Running {
        service.stop().context("failed to stop service")?;
        // Wait briefly
        std::thread::sleep(Duration::from_millis(500));
    }

    service.delete().context("failed to delete service")?;
    info!("uninstalled service {name}");
    Ok(())
}

pub fn start(name: &str) -> Result<()> {
    let manager_access = ServiceManagerAccess::CONNECT;
    let manager = ServiceManager::local_computer(None::<&str>, manager_access)
        .context("failed to open service manager")?;

    let service_access = ServiceAccess::START | ServiceAccess::QUERY_STATUS;
    let service = manager
        .open_service(name, service_access)
        .context("failed to open service; is it installed?")?;

    service
        .start(&[] as &[&str])
        .context("failed to start service")?;
    info!("started service {name}");
    Ok(())
}

pub fn stop(name: &str) -> Result<()> {
    let manager_access = ServiceManagerAccess::CONNECT;
    let manager = ServiceManager::local_computer(None::<&str>, manager_access)
        .context("failed to open service manager")?;

    let service_access = ServiceAccess::STOP | ServiceAccess::QUERY_STATUS;
    let service = manager
        .open_service(name, service_access)
        .context("failed to open service; is it installed?")?;

    service.stop().context("failed to stop service")?;
    info!("stopped service {name}");
    Ok(())
}

pub fn status(name: &str) -> Result<()> {
    let manager_access = ServiceManagerAccess::CONNECT;
    let manager = ServiceManager::local_computer(None::<&str>, manager_access)
        .context("failed to open service manager")?;

    let service_access = ServiceAccess::QUERY_STATUS;
    let service = manager
        .open_service(name, service_access)
        .context("failed to open service; is it installed?")?;

    let status = service
        .query_status()
        .context("failed to query service status")?;
    let state = match status.current_state {
        ServiceState::Stopped => "stopped",
        ServiceState::StartPending => "start_pending",
        ServiceState::StopPending => "stop_pending",
        ServiceState::Running => "running",
        ServiceState::ContinuePending => "continue_pending",
        ServiceState::PausePending => "pause_pending",
        ServiceState::Paused => "paused",
    };
    info!(name, state, "service status");
    println!("{name}: {state}");
    Ok(())
}

//! OS specifics: default paths, system shutdown, Windows service (install / uninstall / run).
use std::path::PathBuf;

/// Where the service keeps its files and where clients connect.
pub struct Paths {
    pub config: PathBuf,
    /// state.json and trip-<time>.log
    pub state_dir: PathBuf,
    /// Unix socket path (Linux) or named pipe name (Windows).
    pub endpoint: String,
}

#[cfg(unix)]
pub fn default_paths() -> Paths {
    Paths {
        config: PathBuf::from("/etc/areko-gpupinwatch/config.toml"),
        state_dir: PathBuf::from("/run/areko-gpupinwatch"),
        endpoint: gpw_core::ipc::SOCKET_PATH.to_string(),
    }
}

#[cfg(windows)]
pub fn default_paths() -> Paths {
    let base = std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
        .join("Areko GPUPinWatch");
    Paths { config: base.join("config.toml"), state_dir: base, endpoint: gpw_core::ipc::PIPE_NAME.to_string() }
}

/// Powers the machine off now (stage 2).
#[cfg(unix)]
pub fn shutdown() -> Result<(), String> {
    let st = std::process::Command::new("systemctl").arg("poweroff").status().map_err(|e| e.to_string())?;
    if st.success() {
        Ok(())
    } else {
        Err(format!("systemctl poweroff: {st}"))
    }
}

#[cfg(windows)]
pub fn shutdown() -> Result<(), String> {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_NOT_ALL_ASSIGNED, HANDLE, LUID};
    use windows_sys::Win32::Security::{
        AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED, SE_SHUTDOWN_NAME,
        TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Shutdown::{
        InitiateSystemShutdownExW, SHTDN_REASON_MAJOR_HARDWARE, SHTDN_REASON_MINOR_POWER_SUPPLY,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    let err = |what: &str| format!("{what}: {}", std::io::Error::last_os_error());
    unsafe {
        // The service runs as LocalSystem, which holds SE_SHUTDOWN_NAME but has it disabled.
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token) == 0 {
            return Err(err("OpenProcessToken"));
        }
        let mut tp = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES { Luid: LUID { LowPart: 0, HighPart: 0 }, Attributes: SE_PRIVILEGE_ENABLED }],
        };
        if LookupPrivilegeValueW(std::ptr::null(), SE_SHUTDOWN_NAME, &mut tp.Privileges[0].Luid) == 0 {
            CloseHandle(token);
            return Err(err("LookupPrivilegeValueW"));
        }
        let ok = AdjustTokenPrivileges(token, 0, &tp, 0, std::ptr::null_mut(), std::ptr::null_mut());
        let last = GetLastError();
        CloseHandle(token);
        if ok == 0 || last == ERROR_NOT_ALL_ASSIGNED {
            return Err(err("AdjustTokenPrivileges"));
        }
        let msg: Vec<u16> = "Areko GPUPinWatch: 12V-2x6 connector overload, shutting down"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        if InitiateSystemShutdownExW(
            std::ptr::null(),
            msg.as_ptr(),
            0,
            1,
            0,
            SHTDN_REASON_MAJOR_HARDWARE | SHTDN_REASON_MINOR_POWER_SUPPLY,
        ) == 0
        {
            return Err(err("InitiateSystemShutdownExW"));
        }
    }
    Ok(())
}

/// Windows service: `areko-gpupinwatchd install | uninstall | service`.
#[cfg(windows)]
pub mod service {
    use std::ffi::OsString;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, OnceLock};
    use std::time::Duration;
    use windows_service::service::{
        ServiceAccess, ServiceControl, ServiceControlAccept, ServiceErrorControl, ServiceExitCode, ServiceInfo,
        ServiceStartType, ServiceState, ServiceStatus, ServiceType,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
    use windows_service::{define_windows_service, service_dispatcher};

    pub const SERVICE_NAME: &str = "AREKOGPUPinWatch";
    const DISPLAY_NAME: &str = "Areko GPUPinWatch";

    type Runner = fn(Arc<AtomicBool>) -> Result<(), String>;
    static RUNNER: OnceLock<Runner> = OnceLock::new();

    define_windows_service!(ffi_service_main, service_main);

    pub fn install() -> Result<(), String> {
        let mgr = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)
            .map_err(|e| e.to_string())?;
        let info = ServiceInfo {
            name: OsString::from(SERVICE_NAME),
            display_name: OsString::from(DISPLAY_NAME),
            service_type: ServiceType::OWN_PROCESS,
            start_type: ServiceStartType::AutoStart,
            error_control: ServiceErrorControl::Normal,
            executable_path: std::env::current_exe().map_err(|e| e.to_string())?,
            launch_arguments: vec![OsString::from("service")],
            dependencies: vec![],
            account_name: None, // LocalSystem
            account_password: None,
        };
        let svc = mgr
            .create_service(&info, ServiceAccess::QUERY_STATUS | ServiceAccess::START)
            .map_err(|e| e.to_string())?;
        svc.start::<&str>(&[]).map_err(|e| e.to_string())
    }

    pub fn uninstall() -> Result<(), String> {
        let mgr = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT).map_err(|e| e.to_string())?;
        let svc = mgr
            .open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE)
            .map_err(|e| e.to_string())?;
        if svc.query_status().map(|s| s.current_state != ServiceState::Stopped).unwrap_or(false) {
            let _ = svc.stop();
        }
        svc.delete().map_err(|e| e.to_string())
    }

    /// Called from `areko-gpupinwatchd service` (started by the SCM).
    pub fn dispatch(run: Runner) -> Result<(), String> {
        let _ = RUNNER.set(run);
        service_dispatcher::start(SERVICE_NAME, ffi_service_main).map_err(|e| e.to_string())
    }

    fn status(state: ServiceState, code: u32) -> ServiceStatus {
        ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: if state == ServiceState::Running {
                ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
            } else {
                ServiceControlAccept::empty()
            },
            exit_code: ServiceExitCode::Win32(code),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        }
    }

    fn service_main(_args: Vec<OsString>) {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let handler = move |ev| match ev {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                flag.store(true, Ordering::SeqCst);
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        };
        let Ok(handle) = service_control_handler::register(SERVICE_NAME, handler) else { return };
        let _ = handle.set_service_status(status(ServiceState::Running, 0));
        let code = match RUNNER.get().map(|run| run(stop)) {
            Some(Ok(())) => 0,
            Some(Err(e)) => {
                eprintln!("areko-gpupinwatchd: {e}");
                1
            }
            None => 1,
        };
        let _ = handle.set_service_status(status(ServiceState::Stopped, code));
    }
}

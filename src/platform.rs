use std::sync::OnceLock;

use windows::core::{w, BOOL, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
use windows::Win32::System::Console::{SetConsoleCtrlHandler, CTRL_CLOSE_EVENT};
use windows::Win32::System::SystemInformation::{GetVersionExW, OSVERSIONINFOW};
use windows::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcess, ProcessPowerThrottling, SetPriorityClass,
    SetProcessInformation, CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS, IDLE_PRIORITY_CLASS,
    PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
    PROCESS_POWER_THROTTLING_STATE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK,
};

pub fn windows_version_string() -> String {
    let mut info = OSVERSIONINFOW {
        dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };

    if unsafe { GetVersionExW(&mut info) }.is_err() {
        return "Windows".into();
    }

    let name = if info.dwMajorVersion >= 10 && info.dwBuildNumber >= 22000 {
        "Windows 11"
    } else if info.dwMajorVersion >= 10 {
        "Windows 10"
    } else {
        "Windows"
    };

    format!("{name} (build {})", info.dwBuildNumber)
}

static INSTANCE: OnceLock<usize> = OnceLock::new();

pub fn acquire_single_instance() -> bool {
    match unsafe { CreateMutexW(None, true, w!("Local\\real-rs")) } {
        Ok(handle) => {
            if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
                let _ = unsafe { CloseHandle(handle) };
                false
            } else {
                let _ = INSTANCE.set(handle.0 as usize);
                true
            }
        }
        Err(_) => true,
    }
}

fn release_single_instance() {
    if let Some(value) = INSTANCE.get().copied() {
        let _ = unsafe { CloseHandle(HANDLE(value as *mut core::ffi::c_void)) };
    }
}

pub fn install_close_handler() {
    unsafe {
        let _ = SetConsoleCtrlHandler(Some(handle_console_close), false);
        let _ = SetConsoleCtrlHandler(Some(handle_console_close), true);
    }
}

unsafe extern "system" fn handle_console_close(event: u32) -> BOOL {
    if event == CTRL_CLOSE_EVENT && crate::startup::is_enabled() {
        release_single_instance();
        spawn_tray_instance();
        return BOOL(1);
    }

    BOOL(0)
}

fn spawn_tray_instance() {
    use std::os::windows::process::CommandExt;

    let Ok(executable) = std::env::current_exe() else {
        return;
    };

    let _ = std::process::Command::new(executable)
        .arg("--tray")
        .creation_flags(DETACHED_PROCESS.0 | CREATE_NEW_PROCESS_GROUP.0)
        .spawn();
}

pub fn notify_already_running() {
    unsafe {
        MessageBoxW(
            None,
            w!("real-rs is already running."),
            w!("real-rs"),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

pub fn show_error(message: &str) {
    let text: Vec<u16> = message.encode_utf16().chain(std::iter::once(0)).collect();

    unsafe {
        MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            w!("real-rs"),
            MB_OK | MB_ICONERROR,
        );
    }
}

pub fn set_idle_priority() {
    unsafe {
        let _ = SetPriorityClass(GetCurrentProcess(), IDLE_PRIORITY_CLASS);
    }
}

pub fn set_background_mode(background: bool) {
    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        StateMask: if background {
            PROCESS_POWER_THROTTLING_EXECUTION_SPEED
        } else {
            0
        },
    };

    unsafe {
        let _ = SetProcessInformation(
            GetCurrentProcess(),
            ProcessPowerThrottling,
            &state as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        );
    }
}

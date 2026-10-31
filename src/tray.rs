use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, OnceLock};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_SETVERSION,
    NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetMessageW, KillTimer, LoadIconW, PostMessageW,
    PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SetForegroundWindow, SetTimer,
    TrackPopupMenu, TranslateMessage, HICON, IDI_APPLICATION, MF_CHECKED, MF_SEPARATOR, MF_STRING,
    TPM_RIGHTBUTTON, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_COMMAND, WM_CONTEXTMENU, WM_DESTROY,
    WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WM_TIMER, WNDCLASSW,
};

use crate::logger::Logger;
use crate::manager::Ui;
use crate::tui::Outcome;

const TRAY_CALLBACK: u32 = WM_APP + 1;
const TRAY_COMMAND: u32 = WM_APP + 2;
const RETRY_TIMER_ID: usize = 1;
const RETRY_INTERVAL_MS: u32 = 2_000;
const TRAY_ID: u32 = 1;
const ICON_RESOURCE_ID: u16 = 1;
const IDM_OPEN: usize = 1001;
const IDM_EXIT: usize = 1002;
const IDM_STARTUP: usize = 1003;

struct TrayState {
    logger: Arc<Logger>,
    ui: Ui,
    hwnd: AtomicIsize,
    icon: AtomicIsize,
    added: AtomicBool,
    taskbar_created: u32,
}

static TRAY_STATE: OnceLock<Arc<TrayState>> = OnceLock::new();

pub fn run(ui: Ui, logger: Arc<Logger>) -> Result<(), String> {
    unsafe {
        let module = GetModuleHandleW(None)
            .map_err(|error| format!("could not get the application module: {error}"))?;
        let instance = HINSTANCE(module.0);
        let class_name = w!("real-rs.TrayWindow");

        let mut window_class: WNDCLASSW = std::mem::zeroed();
        window_class.lpfnWndProc = Some(window_procedure);
        window_class.hInstance = instance;
        window_class.lpszClassName = class_name;

        if RegisterClassW(&window_class) == 0 {
            return Err("could not register the tray window class".into());
        }

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            w!("real-rs"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        )
        .map_err(|error| format!("could not create the tray window: {error}"))?;

        let icon = LoadIconW(Some(instance), PCWSTR(ICON_RESOURCE_ID as *const u16))
            .or_else(|_| LoadIconW(None, IDI_APPLICATION))
            .map_err(|error| format!("could not load the tray icon: {error}"))?;

        let taskbar_created = RegisterWindowMessageW(w!("TaskbarCreated"));

        let state = Arc::new(TrayState {
            logger,
            ui,
            hwnd: AtomicIsize::new(hwnd.0 as isize),
            icon: AtomicIsize::new(icon.0 as isize),
            added: AtomicBool::new(false),
            taskbar_created,
        });

        let _ = TRAY_STATE.set(state.clone());

        if !add_icon_with_retry(hwnd, &state) {
            state
                .logger
                .line("Warning: could not add the tray icon yet; retrying.");
        }

        let mut message: windows::Win32::UI::WindowsAndMessaging::MSG = std::mem::zeroed();
        loop {
            let result = GetMessageW(&mut message, None, 0, 0);
            if result.0 <= 0 {
                break;
            }

            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }

        remove_icon(&state);
        let _ = DestroyWindow(hwnd);
        Ok(())
    }
}

fn add_icon(state: &TrayState) -> bool {
    if state.added.swap(true, Ordering::SeqCst) {
        return true;
    }

    if notify_icon(state, NIM_ADD).as_bool() {
        let _ = notify_icon(state, NIM_SETVERSION);
        return true;
    }

    state.added.store(false, Ordering::SeqCst);
    false
}

fn add_icon_with_retry(hwnd: HWND, state: &TrayState) -> bool {
    if add_icon(state) {
        let _ = unsafe { KillTimer(Some(hwnd), RETRY_TIMER_ID) };
        true
    } else {
        let _ = unsafe { SetTimer(Some(hwnd), RETRY_TIMER_ID, RETRY_INTERVAL_MS, None) };
        false
    }
}

fn remove_icon(state: &TrayState) {
    if state.added.swap(false, Ordering::SeqCst) {
        let _ = notify_icon(state, NIM_DELETE);
    }
}

fn notify_icon(
    state: &TrayState,
    message: windows::Win32::UI::Shell::NOTIFY_ICON_MESSAGE,
) -> windows::core::BOOL {
    let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = HWND(state.hwnd.load(Ordering::SeqCst) as *mut core::ffi::c_void);
    data.uID = TRAY_ID;
    data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    data.uCallbackMessage = TRAY_CALLBACK;
    data.hIcon = HICON(state.icon.load(Ordering::SeqCst) as *mut core::ffi::c_void);
    data.Anonymous.uVersion = NOTIFYICON_VERSION_4;

    let tip: Vec<u16> = "real-rs - reduce audio latency"
        .encode_utf16()
        .take(127)
        .collect();
    data.szTip[..tip.len()].copy_from_slice(&tip);

    unsafe { Shell_NotifyIconW(message, &data) }
}

unsafe extern "system" fn window_procedure(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let state = TRAY_STATE.get();

    if let Some(state) = state {
        if state.taskbar_created != 0 && message == state.taskbar_created {
            state.added.store(false, Ordering::SeqCst);

            let _ = add_icon_with_retry(hwnd, state);

            return LRESULT(0);
        }
    }

    match message {
        TRAY_CALLBACK => {
            let event = (lparam.0 as u32) & 0xFFFF;
            if let Some(state) = state {
                match event {
                    WM_LBUTTONUP => open_console(hwnd, state),
                    WM_RBUTTONUP | WM_CONTEXTMENU => show_menu(hwnd),
                    _ => {}
                }
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let _ = unsafe { PostMessageW(Some(hwnd), TRAY_COMMAND, wparam, LPARAM(0)) };
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 == RETRY_TIMER_ID {
                if let Some(state) = state {
                    if !state.added.load(Ordering::SeqCst) && add_icon(state) {
                        let _ = unsafe { KillTimer(Some(hwnd), RETRY_TIMER_ID) };
                    }
                }
            }
            LRESULT(0)
        }
        TRAY_COMMAND => {
            if let Some(state) = state {
                match wparam.0 & 0xFFFF {
                    IDM_OPEN => open_console(hwnd, state),
                    IDM_STARTUP => {
                        let enabled = !crate::startup::is_enabled();

                        if let Err(error) = crate::startup::set_enabled(enabled) {
                            crate::platform::show_error(&format!(
                                "Could not update autostart: {error}"
                            ));
                        }
                    }
                    IDM_EXIT => {
                        let _ = unsafe { DestroyWindow(hwnd) };
                    }
                    _ => return DefWindowProcW(hwnd, message, wparam, lparam),
                }
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            if let Some(state) = state {
                remove_icon(state);
            }
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

fn open_console(hwnd: HWND, state: &TrayState) {
    remove_icon(state);

    if !state.logger.open_console() {
        crate::platform::show_error("Could not open the console window.");
        let _ = add_icon_with_retry(hwnd, state);
        return;
    }

    crate::platform::set_background_mode(false);

    match crate::tui::run(&state.ui, &state.logger) {
        Ok(Outcome::Quit) => {
            let _ = unsafe { DestroyWindow(hwnd) };
        }
        Ok(Outcome::Tray) => {
            crate::platform::set_background_mode(true);
            state.logger.close_console();
            let _ = add_icon_with_retry(hwnd, state);
        }
        Err(error) => {
            crate::platform::set_background_mode(true);
            state.logger.line(format!("ERROR: {error}"));
            crate::platform::show_error(&format!("real-rs could not continue: {error}"));
            state.logger.close_console();
            let _ = add_icon_with_retry(hwnd, state);
        }
    }
}

fn show_menu(hwnd: HWND) {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            return;
        };

        let _ = AppendMenuW(menu, MF_STRING, IDM_OPEN, w!("Open console"));

        let startup_flags = if crate::startup::is_enabled() {
            MF_STRING | MF_CHECKED
        } else {
            MF_STRING
        };
        let _ = AppendMenuW(menu, startup_flags, IDM_STARTUP, w!("Start with Windows"));

        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, IDM_EXIT, w!("Exit"));

        let mut point: POINT = std::mem::zeroed();
        let _ = GetCursorPos(&mut point);
        let _ = SetForegroundWindow(hwnd);
        let _ = TrackPopupMenu(menu, TPM_RIGHTBUTTON, point.x, point.y, None, hwnd, None);
        let _ = DestroyMenu(menu);
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
    }
}

use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::PCWSTR;
use windows::Win32::System::Console::{
    AllocConsole, AttachConsole, GetConsoleMode, GetConsoleProcessList, GetConsoleWindow,
    GetStdHandle, SetConsoleMode, SetConsoleTitleW, WriteConsoleW, ATTACH_PARENT_PROCESS,
    CONSOLE_MODE, ENABLE_VIRTUAL_TERMINAL_PROCESSING, STD_OUTPUT_HANDLE,
};
use windows::Win32::UI::WindowsAndMessaging::{SetForegroundWindow, ShowWindow, SW_HIDE, SW_SHOW};

pub struct Console {
    open: AtomicBool,
}

impl Console {
    pub const fn new() -> Self {
        Self {
            open: AtomicBool::new(false),
        }
    }

    pub fn attach(&self) -> bool {
        if self.open.load(Ordering::SeqCst) {
            return true;
        }

        let attached = unsafe {
            !GetConsoleWindow().0.is_null() || AttachConsole(ATTACH_PARENT_PROCESS).is_ok()
        };

        if attached {
            self.open.store(true, Ordering::SeqCst);
        }

        attached
    }

    pub fn open(&self) -> bool {
        if self.attach() {
            self.show();
            return true;
        }

        if unsafe { AllocConsole().is_ok() } {
            self.open.store(true, Ordering::SeqCst);
            self.show();
            return true;
        }

        false
    }

    pub fn set_title(&self, title: &str) {
        let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let _ = SetConsoleTitleW(PCWSTR(wide.as_ptr()));
        }
    }

    fn show(&self) {
        let window = unsafe { GetConsoleWindow() };
        if !window.0.is_null() {
            unsafe {
                let _ = ShowWindow(window, SW_SHOW);
                let _ = SetForegroundWindow(window);
            }
        }
    }

    fn hide_private_window(&self) {
        let mut processes = [0u32; 2];
        if unsafe { GetConsoleProcessList(&mut processes) } != 1 {
            return;
        }

        let window = unsafe { GetConsoleWindow() };
        if !window.0.is_null() {
            unsafe {
                let _ = ShowWindow(window, SW_HIDE);
            }
        }
    }

    pub fn close(&self) {
        if !self.open.load(Ordering::SeqCst) {
            return;
        }

        self.hide_private_window();
    }

    pub fn enable_virtual_terminal(&self) {
        unsafe {
            let Ok(handle) = GetStdHandle(STD_OUTPUT_HANDLE) else {
                return;
            };

            let mut mode = CONSOLE_MODE(0);
            if GetConsoleMode(handle, &mut mode).is_ok() {
                let _ = SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
            }
        }
    }

    pub fn write(&self, text: &str) {
        if text.is_empty() {
            return;
        }

        let wide: Vec<u16> = text.encode_utf16().collect();
        let mut written = 0u32;
        unsafe {
            if let Ok(handle) = GetStdHandle(STD_OUTPUT_HANDLE) {
                let _ = WriteConsoleW(handle, &wide, Some(&mut written), None);
            }
        }
    }
}

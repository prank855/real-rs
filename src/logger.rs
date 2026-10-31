use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::console::Console;

const BUFFER_LINES: usize = 256;

struct State {
    buffer: VecDeque<String>,
    console_open: bool,
    tui: bool,
}

pub struct Logger {
    console: Console,
    state: Mutex<State>,
}

impl Logger {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            console: Console::new(),
            state: Mutex::new(State {
                buffer: VecDeque::new(),
                console_open: false,
                tui: false,
            }),
        })
    }

    pub fn open_console(&self) -> bool {
        self.show_console(Console::open)
    }

    pub fn attach_console(&self) -> bool {
        self.show_console(Console::attach)
    }

    fn show_console(&self, open: impl FnOnce(&Console) -> bool) -> bool {
        let buffered: Vec<String> = {
            let mut state = self.state.lock().unwrap();
            if state.console_open {
                return true;
            }

            if !open(&self.console) {
                return false;
            }

            state.console_open = true;
            self.console.enable_virtual_terminal();
            crate::platform::install_close_handler();
            state.buffer.drain(..).collect()
        };

        for line in buffered {
            self.console.write(&line);
            self.console.write("\n");
        }

        true
    }

    pub fn close_console(&self) {
        let mut state = self.state.lock().unwrap();
        if state.console_open {
            self.console.close();
            state.console_open = false;
        }
    }

    pub fn set_console_title(&self, title: &str) {
        self.console.set_title(title);
    }

    pub fn start_tui(&self) {
        let mut state = self.state.lock().unwrap();
        state.tui = true;
        state.buffer.clear();
    }

    pub fn stop_tui(&self) {
        self.state.lock().unwrap().tui = false;
    }

    pub fn line(&self, message: impl AsRef<str>) {
        let message = message.as_ref();

        let output = {
            let mut state = self.state.lock().unwrap();

            if state.tui {
                return;
            }

            if state.console_open {
                format!("{message}\n")
            } else {
                state.buffer.push_back(message.to_string());

                while state.buffer.len() > BUFFER_LINES {
                    state.buffer.pop_front();
                }

                return;
            }
        };

        self.console.write(&output);
    }
}

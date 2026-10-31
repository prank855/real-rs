#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod console;
mod logger;
mod manager;
mod platform;
mod startup;
mod tray;
mod tui;

use logger::Logger;
use manager::Manager;

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let tray_only = std::env::args()
        .skip(1)
        .any(|argument| argument == "--tray");

    if !platform::acquire_single_instance() {
        if !tray_only {
            platform::notify_already_running();
        }
        return 0;
    }

    let logger = Logger::new();
    let mut show_tui = !tray_only && logger.open_console();
    platform::install_close_handler();

    platform::set_background_mode(!show_tui);
    platform::set_idle_priority();

    let manager = match Manager::start(logger.clone()) {
        Ok(manager) => manager,
        Err(error) => {
            logger.open_console();
            logger.line(format!("ERROR: Could not enable low-latency mode: {error}"));
            platform::show_error(&format!("Could not enable low-latency mode: {error}"));
            logger.close_console();
            return 1;
        }
    };

    let exit_code = loop {
        if show_tui {
            match tui::run(&manager.ui(), &logger) {
                Ok(tui::Outcome::Quit) => break 0,
                Ok(tui::Outcome::Tray) => {
                    logger.close_console();
                    platform::set_background_mode(true);
                    show_tui = false;
                }
                Err(error) => {
                    logger.line(format!("ERROR: {error}"));
                    platform::show_error(&format!("real-rs could not continue: {error}"));
                    break 1;
                }
            }
        } else {
            match tray::run(manager.ui(), logger.clone()) {
                Ok(()) => break 0,
                Err(error) => {
                    logger.attach_console();
                    logger.line(format!("ERROR: {error}"));
                    platform::show_error(&format!("real-rs could not continue: {error}"));
                    break 1;
                }
            }
        }
    };

    logger.close_console();
    manager.stop();
    exit_code
}

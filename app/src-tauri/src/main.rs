// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Err(err) = mahjong_copilot_lib::run() {
        eprintln!("failed to start Mahjong Copilot: {err}");
        std::process::exit(1);
    }
}

// No console window on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Err(e) = freedsp_studio::run() {
        eprintln!("FreeDSP Studio: {}", e);
        std::process::exit(1);
    }
}

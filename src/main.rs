#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod capture;
mod detection;
mod native;
mod overlay;
mod tracking;
mod worker;

fn main() {
    if let Err(error) = native::run() {
        native::report_fatal(&format!("{error:#}"));
        std::process::exit(1);
    }
}

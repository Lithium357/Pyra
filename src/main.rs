#![cfg_attr(all(not(debug_assertions), not(test)), windows_subsystem = "windows")]

mod capture;
mod detection;
mod label;
mod native;
mod overlay;
mod scanning;
mod tracking;
mod worker;

fn main() {
    if let Err(error) = native::run() {
        native::report_fatal(&format!("{error:#}"));
        std::process::exit(1);
    }
}

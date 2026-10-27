#![cfg_attr(all(not(debug_assertions), not(test)), windows_subsystem = "windows")]

mod analysis;
mod capture;
mod detection;
mod label;
mod native;
mod overlay;
#[cfg(test)]
mod private_validation;
mod scanning;
mod stylized;
mod tracking;
#[cfg(test)]
mod validation;
mod worker;

fn main() {
    if let Err(error) = native::run() {
        native::report_fatal(&format!("{error:#}"));
        std::process::exit(1);
    }
}

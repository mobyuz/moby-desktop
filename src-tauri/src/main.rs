// Release builds run as a GUI app without a console window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    moby_desktop_lib::run();
}

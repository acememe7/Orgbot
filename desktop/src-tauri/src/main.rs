//! Desktop shell entry point.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    gptbots_desktop_lib::run()
}

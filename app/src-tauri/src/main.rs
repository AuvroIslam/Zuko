// Zuko runs without a console window: Zuko is the whole UI.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    zuko_lib::run()
}

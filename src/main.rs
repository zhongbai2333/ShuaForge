#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() -> eframe::Result<()> {
    shuaforge_core::desktop::run()
}

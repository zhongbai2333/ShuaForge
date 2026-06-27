#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod ai;
mod app;
mod deck;
mod logging;
mod problem;
mod self_update;
mod store;
mod userscript_server;

use app::ShuaForgeApp;

fn main() -> eframe::Result<()> {
    let log_path = logging::init_app_logging();
    if let Some(path) = &log_path {
        log::info!("ShuaForge starting, log file: {}", path.display());
    }

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([980.0, 720.0])
            .with_min_inner_size([760.0, 560.0])
            .with_title("ShuaForge 刷题助手"),
        ..Default::default()
    };

    eframe::run_native(
        "ShuaForge 刷题助手",
        options,
        Box::new(|cc| Ok(Box::new(ShuaForgeApp::new(cc, log_path)))),
    )
}

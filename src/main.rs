mod ai;
mod app;
mod deck;
mod problem;
mod store;

use app::ShuaForgeApp;

fn main() -> eframe::Result<()> {
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
        Box::new(|cc| Ok(Box::new(ShuaForgeApp::new(cc)))),
    )
}

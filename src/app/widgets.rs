use eframe::egui;

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit_index = 0usize;
    while value >= 1024.0 && unit_index + 1 < UNITS.len() {
        value /= 1024.0;
        unit_index += 1;
    }
    if unit_index == 0 {
        format!("{} {}", bytes, UNITS[unit_index])
    } else {
        format!("{value:.1} {}", UNITS[unit_index])
    }
}

pub fn render_update_progress_bar(ui: &mut egui::Ui, downloaded: u64, total: u64) {
    if total > 0 {
        let progress = (downloaded as f32 / total as f32).clamp(0.0, 1.0);
        ui.add(
            egui::ProgressBar::new(progress)
                .desired_width(ui.available_width())
                .text(format!("下载进度 {:.0}%", progress * 100.0)),
        );
    } else {
        ui.add(
            egui::ProgressBar::new(0.35)
                .animate(true)
                .desired_width(ui.available_width())
                .text("正在下载..."),
        );
    }
}

pub fn grid_columns(ui: &egui::Ui, card_width: f32) -> usize {
    let gap = 12.0;
    ((ui.available_width() + gap) / (card_width + gap))
        .floor()
        .max(1.0) as usize
}

pub fn add_content_safe_area(ui: &mut egui::Ui, content_pad: f32, is_mobile: bool) {
    if is_mobile {
        ui.add_space(content_pad * 0.5);
    }
}

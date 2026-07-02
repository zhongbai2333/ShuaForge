use super::*;

impl ShuaForgeApp {
    pub(super) fn render_mobile_tabs(&mut self, ui: &mut egui::Ui) {
        let tab_width = ui.available_width() / 3.0;
        let selected_color = egui::Color32::from_rgb(100, 180, 255);
        let dim_color = ui.visuals().widgets.noninteractive.fg_stroke.color;
        let practice_available = self.deck.is_some() || self.has_saved_practice_session();

        let library_label = if self.view == AppView::Library {
            egui::RichText::new("首页")
                .color(selected_color)
                .strong()
                .size(16.0)
        } else {
            egui::RichText::new("首页").color(dim_color).size(15.0)
        };
        let practice_label = if self.view == AppView::Practice {
            egui::RichText::new("练习")
                .color(selected_color)
                .strong()
                .size(16.0)
        } else if practice_available {
            egui::RichText::new("练习").color(dim_color).size(15.0)
        } else {
            egui::RichText::new("练习")
                .color(dim_color.gamma_multiply(0.45))
                .size(15.0)
        };
        let settings_label = if self.view == AppView::Settings {
            egui::RichText::new("设置")
                .color(selected_color)
                .strong()
                .size(16.0)
        } else {
            egui::RichText::new("设置").color(dim_color).size(15.0)
        };

        ui.horizontal(|ui| {
            if ui
                .add_sized(
                    [tab_width, MOBILE_TOUCH_HEIGHT],
                    egui::Button::new(library_label).fill(egui::Color32::TRANSPARENT),
                )
                .clicked()
            {
                self.view = AppView::Library;
            }
            if ui
                .add_enabled(
                    practice_available,
                    egui::Button::new(practice_label)
                        .fill(egui::Color32::TRANSPARENT)
                        .min_size(egui::vec2(tab_width, MOBILE_TOUCH_HEIGHT)),
                )
                .clicked()
            {
                if self.deck.is_some() {
                    self.view = AppView::Practice;
                } else {
                    self.continue_latest_practice_session();
                }
            }
            if ui
                .add_sized(
                    [tab_width, MOBILE_TOUCH_HEIGHT],
                    egui::Button::new(settings_label).fill(egui::Color32::TRANSPARENT),
                )
                .clicked()
            {
                self.view = AppView::Settings;
            }
        });
    }
}

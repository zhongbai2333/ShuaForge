use super::markdown::render_markdown_text;
use super::{AnalysisDialogState, AnalysisProgressState, ChatMessage, ChatRole};
use eframe::egui;

pub(super) fn render_analysis_input_bar(
    ui: &mut egui::Ui,
    dialog: &mut AnalysisDialogState,
    enter_to_send: &mut bool,
) -> bool {
    let mut send_clicked = false;
    ui.horizontal(|ui| {
        ui.strong("继续追问");
        ui.small("Ctrl + Enter 发送");
    });
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .stroke(egui::Stroke::new(
            1.0,
            ui.visuals().widgets.noninteractive.bg_stroke.color,
        ))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let button_width = 64.0;
                let input_width =
                    (ui.available_width() - button_width - 8.0).clamp(80.0, ui.available_width());
                let response = ui.add(
                    egui::TextEdit::multiline(&mut dialog.input)
                        .desired_width(input_width)
                        .desired_rows(2)
                        .hint_text("追问分析细节、生成复习计划或让 AI 换一种说法…"),
                );
                *enter_to_send = response.has_focus()
                    && ui
                        .input(|input| input.modifiers.ctrl && input.key_pressed(egui::Key::Enter));
                let send_label = if dialog.is_loading {
                    "取消"
                } else {
                    "发送"
                };
                let send_button =
                    egui::Button::new(send_label).min_size(egui::vec2(button_width, 42.0));
                if ui.add_sized([button_width, 42.0], send_button).clicked() {
                    send_clicked = true;
                }
            });
        });
    ui.small("AI 会基于上方分析结果继续回答，不会重新导入题库。");
    send_clicked
}

pub(super) fn render_chat_message(ui: &mut egui::Ui, message: &ChatMessage) {
    let (label, accent, fill) = match message.role {
        ChatRole::User => (
            "用户",
            egui::Color32::from_rgb(120, 170, 255),
            if ui.visuals().dark_mode {
                egui::Color32::from_rgb(38, 68, 118)
            } else {
                egui::Color32::from_rgb(226, 238, 255)
            },
        ),
        ChatRole::Assistant => (
            "助手",
            egui::Color32::from_rgb(150, 210, 160),
            ui.visuals().faint_bg_color,
        ),
        ChatRole::Tool => (
            "工具调用",
            egui::Color32::from_rgb(220, 180, 110),
            if ui.visuals().dark_mode {
                egui::Color32::from_rgb(45, 43, 38)
            } else {
                egui::Color32::from_rgb(255, 244, 220)
            },
        ),
    };
    let available = ui.available_width().max(1.0);
    let max_bubble_width = (available * 0.86).clamp(160.0, 820.0).min(available - 16.0);
    let bubble_width = adaptive_chat_bubble_width(message, max_bubble_width);
    let layout = if message.role == ChatRole::User {
        egui::Layout::right_to_left(egui::Align::Min)
    } else {
        egui::Layout::left_to_right(egui::Align::Min)
    };
    ui.with_layout(layout, |ui| {
        egui::Frame::new()
            .fill(fill)
            .stroke(egui::Stroke::new(1.0, accent.gamma_multiply(0.55)))
            .corner_radius(egui::CornerRadius::same(12))
            .inner_margin(egui::Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.set_min_width(0.0);
                ui.set_max_width(bubble_width);
                ui.set_width(bubble_width);
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.set_width(bubble_width);
                    let header = if message.title == label {
                        label.to_owned()
                    } else {
                        format!("{label} · {}", message.title)
                    };
                    ui.colored_label(accent, header);
                    ui.add_space(3.0);
                    if message.role == ChatRole::Tool {
                        ui.collapsing("参数", |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt(("tool_message", &message.title))
                                .max_height(120.0)
                                .show(ui, |ui| {
                                    ui.monospace(&message.content);
                                });
                        });
                    } else {
                        ui.set_width((bubble_width - 24.0).max(80.0));
                        if message.role == ChatRole::Assistant {
                            render_markdown_text(
                                ui,
                                &message.content,
                                (bubble_width - 24.0).max(80.0),
                            );
                        } else {
                            ui.add(egui::Label::new(&message.content).wrap());
                        }
                    }
                });
            });
    });
}

pub(super) fn render_analysis_progress_bar(
    ui: &mut egui::Ui,
    progress: Option<&AnalysisProgressState>,
) {
    let Some(progress) = progress else { return };
    if progress.total == 0 {
        ui.add(
            egui::ProgressBar::new(0.6)
                .animate(true)
                .desired_width(ui.available_width())
                .text(progress.text()),
        );
    } else {
        ui.add(
            egui::ProgressBar::new(progress.fraction())
                .desired_width(ui.available_width())
                .text(progress.text()),
        );
    }
}

fn adaptive_chat_bubble_width(message: &ChatMessage, max_width: f32) -> f32 {
    let header_chars = message.title.chars().count() + 8;
    let longest_line_chars = message
        .content
        .lines()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0)
        .max(header_chars);
    let visible_chars = message.content.chars().count().max(longest_line_chars);
    let estimated_char_width = match message.role {
        ChatRole::Tool => 8.5,
        ChatRole::User | ChatRole::Assistant => 13.0,
    };
    let ideal_width = longest_line_chars as f32 * estimated_char_width + 42.0;
    let multiline_width = if visible_chars > 120 {
        max_width
    } else {
        ideal_width
    };
    multiline_width.clamp(128.0, max_width)
}

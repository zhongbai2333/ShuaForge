use crate::problem::{Problem, ProblemType};
use eframe::egui;
use std::collections::BTreeSet;

pub fn render_answer_input(
    ui: &mut egui::Ui,
    problem: &Problem,
    answer_input: &mut String,
    selected_single: &mut Option<String>,
    selected_multiple: &mut BTreeSet<String>,
    focused_choice_index: &mut usize,
    keyboard_choice_focus_visible: &mut bool,
) -> PracticeKeyboardActions {
    let mut actions = PracticeKeyboardActions::default();
    match problem.kind() {
        ProblemType::SingleChoice => {
            let options = problem.options();
            clamp_focused_choice(focused_choice_index, options.len());
            for (index, option) in options.iter().enumerate() {
                let focused = *keyboard_choice_focus_visible && index == *focused_choice_index;
                let label = format!("{}. {}", option.key, option.text);
                let response = render_single_choice_row(
                    ui,
                    selected_single,
                    option.key.clone(),
                    &label,
                    focused,
                );
                if response.clicked() {
                    *focused_choice_index = index;
                    *keyboard_choice_focus_visible = false;
                }
            }
        }
        ProblemType::MultipleChoice => {
            let options = problem.options();
            clamp_focused_choice(focused_choice_index, options.len());
            for (index, option) in options.iter().enumerate() {
                let focused = *keyboard_choice_focus_visible && index == *focused_choice_index;
                let label = format!("{}. {}", option.key, option.text);
                let response = render_multiple_choice_row(
                    ui,
                    selected_multiple,
                    option.key.clone(),
                    &label,
                    focused,
                );
                if response.clicked() {
                    *focused_choice_index = index;
                    *keyboard_choice_focus_visible = false;
                }
            }
        }
        ProblemType::Text => {
            let text_edit_id = egui::Id::new(("practice_text_answer", &problem.id));
            let response = ui.add(
                egui::TextEdit::multiline(answer_input)
                    .id_source(text_edit_id)
                    .desired_rows(4)
                    .hint_text("写下答案后按 Ctrl+Enter / Ctrl+I / Ctrl+S / Ctrl+D"),
            );
            if !response.has_focus() {
                response.request_focus();
            }
            if response.has_focus() {
                ui.input(|input| {
                    let ctrl_only = input.modifiers.ctrl
                        && !input.modifiers.alt
                        && !input.modifiers.shift
                        && !input.modifiers.mac_cmd;
                    if ctrl_only && input.key_pressed(egui::Key::Enter) {
                        actions.submit = true;
                    }
                    if ctrl_only && input.key_pressed(egui::Key::I) {
                        actions.solution_guide = true;
                    }
                    if ctrl_only && input.key_pressed(egui::Key::S) {
                        actions.skip = true;
                    }
                    if ctrl_only && input.key_pressed(egui::Key::D) {
                        actions.skip_without_requeue = true;
                    }
                });
            }
        }
    }
    actions
}

#[derive(Default)]
pub struct PracticeKeyboardActions {
    pub submit: bool,
    pub solution_guide: bool,
    pub skip: bool,
    pub skip_without_requeue: bool,
    pub back_to_library: bool,
}

pub fn handle_practice_keyboard(
    ui: &egui::Ui,
    problem: &Problem,
    selected_single: &mut Option<String>,
    selected_multiple: &mut BTreeSet<String>,
    focused_choice_index: &mut usize,
    keyboard_choice_focus_visible: &mut bool,
) -> PracticeKeyboardActions {
    let mut actions = PracticeKeyboardActions::default();
    if ui.ctx().wants_keyboard_input() {
        return actions;
    }

    ui.input(|input| {
        let ctrl_only = input.modifiers.ctrl
            && !input.modifiers.alt
            && !input.modifiers.shift
            && !input.modifiers.mac_cmd;
        if matches!(problem.kind(), ProblemType::Text) {
            if ctrl_only && input.key_pressed(egui::Key::Enter) {
                actions.submit = true;
            }
            if ctrl_only && input.key_pressed(egui::Key::I) {
                actions.solution_guide = true;
            }
            if ctrl_only && input.key_pressed(egui::Key::S) {
                actions.skip = true;
            }
            if ctrl_only && input.key_pressed(egui::Key::D) {
                actions.skip_without_requeue = true;
            }
            if !input.modifiers.any() && input.key_pressed(egui::Key::Escape) {
                actions.back_to_library = true;
            }
            return;
        }

        let plain_key = !input.modifiers.any();
        let options = problem.options();
        if !options.is_empty() {
            if *focused_choice_index >= options.len() {
                *focused_choice_index = options.len().saturating_sub(1);
            }
            if plain_key
                && (input.key_pressed(egui::Key::ArrowDown)
                    || input.key_pressed(egui::Key::ArrowRight))
            {
                *keyboard_choice_focus_visible = true;
                *focused_choice_index = (*focused_choice_index + 1).min(options.len() - 1);
            }
            if plain_key
                && (input.key_pressed(egui::Key::ArrowUp)
                    || input.key_pressed(egui::Key::ArrowLeft))
            {
                *keyboard_choice_focus_visible = true;
                *focused_choice_index = focused_choice_index.saturating_sub(1);
            }
            if plain_key && *keyboard_choice_focus_visible && input.key_pressed(egui::Key::Space) {
                let key = options[*focused_choice_index].key.clone();
                match problem.kind() {
                    ProblemType::SingleChoice => *selected_single = Some(key),
                    ProblemType::MultipleChoice => {
                        if !selected_multiple.remove(&key) {
                            selected_multiple.insert(key);
                        }
                    }
                    ProblemType::Text => {}
                }
            }
        }
        if plain_key && input.key_pressed(egui::Key::Enter) {
            actions.submit = true;
        }
        if plain_key && input.key_pressed(egui::Key::I) {
            actions.solution_guide = true;
        }
        if plain_key && input.key_pressed(egui::Key::S) {
            actions.skip = true;
        }
        if plain_key && input.key_pressed(egui::Key::D) {
            actions.skip_without_requeue = true;
        }
        if plain_key && input.key_pressed(egui::Key::Escape) {
            actions.back_to_library = true;
        }
    });
    actions
}

pub fn suppress_practice_tab_focus(ctx: &egui::Context) -> bool {
    ctx.input_mut(|input| {
        input.consume_key(egui::Modifiers::NONE, egui::Key::Tab)
            || input.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab)
            || input.key_pressed(egui::Key::Tab)
    })
}

pub fn clear_egui_keyboard_focus(ctx: &egui::Context) {
    ctx.memory_mut(|memory| {
        if let Some(focused) = memory.focused() {
            memory.surrender_focus(focused);
        }
        memory.stop_text_input();
    });
}

fn clamp_focused_choice(focused_choice_index: &mut usize, option_len: usize) {
    if option_len == 0 {
        *focused_choice_index = 0;
    } else if *focused_choice_index >= option_len {
        *focused_choice_index = option_len - 1;
    }
}

fn choice_row_fill(ui: &egui::Ui, focused: bool, selected: bool) -> egui::Color32 {
    if selected && focused {
        brighten_color(ui.visuals().selection.bg_fill, 1.28)
    } else if selected {
        ui.visuals().selection.bg_fill
    } else if focused {
        ui.visuals().widgets.hovered.bg_fill
    } else {
        egui::Color32::TRANSPARENT
    }
}

fn choice_row_stroke(ui: &egui::Ui, focused: bool, selected: bool) -> egui::Stroke {
    if selected && focused {
        egui::Stroke::new(
            1.5,
            brighten_color(ui.visuals().selection.stroke.color, 1.35),
        )
    } else if focused && !selected {
        egui::Stroke::new(1.0, ui.visuals().widgets.hovered.bg_stroke.color)
    } else if selected {
        egui::Stroke::new(1.0, ui.visuals().selection.stroke.color)
    } else {
        egui::Stroke::NONE
    }
}

fn choice_row_text_color(ui: &egui::Ui, focused: bool, selected: bool) -> egui::Color32 {
    if selected && focused {
        brighten_color(ui.visuals().selection.stroke.color, 1.25)
    } else if selected {
        ui.visuals().selection.stroke.color
    } else if focused {
        ui.visuals().widgets.hovered.fg_stroke.color
    } else {
        ui.visuals().widgets.noninteractive.fg_stroke.color
    }
}

fn brighten_color(color: egui::Color32, factor: f32) -> egui::Color32 {
    let brighten_channel = |channel: u8| ((channel as f32 * factor).round()).min(255.0) as u8;
    egui::Color32::from_rgba_premultiplied(
        brighten_channel(color.r()),
        brighten_channel(color.g()),
        brighten_channel(color.b()),
        color.a(),
    )
}

fn render_single_choice_row(
    ui: &mut egui::Ui,
    selected_single: &mut Option<String>,
    value: String,
    label: &str,
    focused: bool,
) -> egui::Response {
    let selected = selected_single.as_deref() == Some(value.as_str());
    let row_value = value.clone();
    let response = render_choice_row(ui, focused, selected, label, false);
    if response.clicked_by(egui::PointerButton::Primary) {
        *selected_single = Some(row_value);
    }
    response
}

fn render_multiple_choice_row(
    ui: &mut egui::Ui,
    selected_multiple: &mut BTreeSet<String>,
    value: String,
    label: &str,
    focused: bool,
) -> egui::Response {
    let mut selected = selected_multiple.contains(&value);
    let response = render_choice_row(ui, focused, selected, label, true);
    if response.clicked_by(egui::PointerButton::Primary) {
        selected = !selected;
    }
    if selected {
        selected_multiple.insert(value);
    } else {
        selected_multiple.remove(&value);
    }
    response
}

fn choice_row_text(label: &str, selected: bool, multiple: bool) -> String {
    let marker = match (multiple, selected) {
        (true, true) => "☑",
        (true, false) => "☐",
        (false, true) => "◉",
        (false, false) => "○",
    };
    format!("{marker}  {label}")
}

fn render_choice_row(
    ui: &mut egui::Ui,
    focused: bool,
    selected: bool,
    label: &str,
    multiple: bool,
) -> egui::Response {
    let row_height = ui.spacing().interact_size.y.max(44.0);
    let text = choice_row_text(label, selected, multiple);
    let text_color = choice_row_text_color(ui, focused, selected);
    let response = ui.add_sized(
        [ui.available_width(), row_height],
        egui::Button::new("")
            .fill(choice_row_fill(ui, focused, selected))
            .stroke(choice_row_stroke(ui, focused, selected))
            .corner_radius(egui::CornerRadius::same(3)),
    );

    let text_rect = response.rect.shrink2(egui::vec2(12.0, 0.0));
    ui.painter().text(
        egui::pos2(text_rect.left(), text_rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        egui::TextStyle::Button.resolve(ui.style()),
        text_color,
    );

    response
}

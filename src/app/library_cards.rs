use crate::problem::Problem;
use crate::store::{DeckCard, GroupCard, GroupDeckCard};
use eframe::egui;

pub const CARD_WIDTH: f32 = 300.0;
const CARD_INNER_MARGIN: f32 = 14.0;
const MOBILE_CARD_INNER_MARGIN: f32 = 8.0;
const MOBILE_CARD_MIN_WIDTH: f32 = 120.0;
const GROUP_CARD_MIN_HEIGHT: f32 = 190.0;
const DECK_CARD_MIN_HEIGHT: f32 = 228.0;
const MOBILE_GROUP_CARD_MIN_HEIGHT: f32 = 210.0;
const MOBILE_DECK_CARD_MIN_HEIGHT: f32 = 228.0;
const CARD_BUTTON_WIDTH: f32 = 82.0;
const CARD_BUTTON_HEIGHT: f32 = 24.0;
const MOBILE_CARD_BUTTON_TEXT_SIZE: f32 = 14.0;
const DECK_ACTION_COLUMNS: usize = 3;
const MOBILE_DECK_ACTION_COLUMNS: usize = 2;
const GROUP_ACTION_COLUMNS: usize = 2;
const CARD_ACTION_SPACING: f32 = 6.0;
const CARD_SELECT_SIZE: f32 = 18.0;
const CARD_SELECT_OFFSET: f32 = 8.0;

pub fn render_problem(ui: &mut egui::Ui, problem: &Problem) {
    let question = problem.question_text();
    ui.label(if question.is_empty() {
        &problem.prompt
    } else {
        &question
    });
}

pub fn render_library_deck_card(
    ui: &mut egui::Ui,
    card: &DeckCard,
    dragging: bool,
    selected: bool,
) -> (
    egui::Response,
    egui::Response,
    bool,
    bool,
    bool,
    bool,
    bool,
    bool,
) {
    render_deck_card(ui, card, dragging, selected, DeckCardUiKind::Desktop)
}

pub fn render_mobile_library_deck_card(
    ui: &mut egui::Ui,
    card: &DeckCard,
    dragging: bool,
    selected: bool,
) -> (
    egui::Response,
    egui::Response,
    bool,
    bool,
    bool,
    bool,
    bool,
    bool,
) {
    render_deck_card(ui, card, dragging, selected, DeckCardUiKind::Mobile)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeckCardUiKind {
    Desktop,
    Mobile,
}

fn render_deck_card(
    ui: &mut egui::Ui,
    card: &DeckCard,
    dragging: bool,
    selected: bool,
    ui_kind: DeckCardUiKind,
) -> (
    egui::Response,
    egui::Response,
    bool,
    bool,
    bool,
    bool,
    bool,
    bool,
) {
    let is_mobile = ui_kind == DeckCardUiKind::Mobile;
    let frame = egui::Frame::group(ui.style())
        .fill(if dragging {
            selection_fill(ui)
        } else {
            ui.visuals().faint_bg_color
        })
        .stroke(if dragging {
            egui::Stroke::new(2.0, accent_color(ui))
        } else {
            egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color)
        })
        .inner_margin(egui::Margin::same(if is_mobile {
            MOBILE_CARD_INNER_MARGIN as i8
        } else {
            CARD_INNER_MARGIN as i8
        }));
    let mut start_clicked = false;
    let mut preview_clicked = false;
    let mut analyze_clicked = false;
    let mut diagnose_clicked = false;
    let mut export_clicked = false;
    let mut selected_now = selected;
    let mut drag_response: Option<egui::Response> = None;
    let response = frame
        .show(ui, |ui| {
            let card_w = if is_mobile {
                mobile_card_content_width(ui)
            } else {
                card_content_width(ui)
            };
            let card_h = if is_mobile {
                MOBILE_DECK_CARD_MIN_HEIGHT
            } else {
                DECK_CARD_MIN_HEIGHT
            };
            ui.set_min_size(egui::vec2(card_w, card_h));
            ui.set_height(card_h);
            ui.set_max_width(card_w);
            ui.vertical_centered(|ui| {
                let info_response = ui
                    .vertical_centered(|ui| {
                        ui.heading("📘");
                        ui.strong(&card.name);
                        ui.small(format!("{} 道题", card.problem_count));
                        if is_mobile {
                            ui.small(format!("新增 {} · 更新 {}", card.inserted, card.updated));
                            ui.small(format!("更新时间：{}", card.updated_at));
                            ui.small(format!("来源：{}", compact_text(&card.source_path, 20)));
                        } else {
                            ui.small(format!("新增 {} · 更新 {}", card.inserted, card.updated));
                            ui.small(format!("更新时间：{}", card.updated_at));
                            ui.small(format!("来源：{}", compact_text(&card.source_path, 26)));
                            ui.small("拖动此区域可移动或删除题库");
                        }
                    })
                    .response
                    .interact(egui::Sense::click_and_drag());
                drag_response = Some(if dragging || info_response.dragged() {
                    info_response.on_hover_cursor(egui::CursorIcon::Grabbing)
                } else {
                    info_response
                });
                (
                    start_clicked,
                    preview_clicked,
                    analyze_clicked,
                    diagnose_clicked,
                    export_clicked,
                ) = render_card_actions(ui, Some("题库预览"), CardActionUiKind::from(ui_kind));
            });
        })
        .response;
    let checkbox_rect = egui::Rect::from_min_size(
        response.rect.min + egui::vec2(CARD_SELECT_OFFSET, CARD_SELECT_OFFSET),
        egui::vec2(CARD_SELECT_SIZE, CARD_SELECT_SIZE),
    );
    ui.put(
        checkbox_rect,
        egui::Checkbox::without_text(&mut selected_now),
    )
    .on_hover_text("多选题库");
    let drag_response = drag_response.unwrap_or_else(|| response.clone());
    (
        response,
        drag_response,
        start_clicked,
        preview_clicked,
        analyze_clicked,
        diagnose_clicked,
        export_clicked,
        selected_now != selected,
    )
}

pub fn render_group_card(
    ui: &mut egui::Ui,
    group: &GroupCard,
    group_decks: &[GroupDeckCard],
    hot: bool,
    dragging: bool,
) -> (
    egui::Response,
    egui::Response,
    bool,
    bool,
    bool,
    bool,
    Vec<i64>,
) {
    render_group_card_with_kind(
        ui,
        group,
        group_decks,
        hot,
        dragging,
        CardActionUiKind::Desktop,
    )
}

pub fn render_mobile_group_card(
    ui: &mut egui::Ui,
    group: &GroupCard,
    group_decks: &[GroupDeckCard],
    hot: bool,
    dragging: bool,
) -> (
    egui::Response,
    egui::Response,
    bool,
    bool,
    bool,
    bool,
    Vec<i64>,
) {
    render_group_card_with_kind(
        ui,
        group,
        group_decks,
        hot,
        dragging,
        CardActionUiKind::Mobile,
    )
}

fn render_group_card_with_kind(
    ui: &mut egui::Ui,
    group: &GroupCard,
    group_decks: &[GroupDeckCard],
    hot: bool,
    dragging: bool,
    ui_kind: CardActionUiKind,
) -> (
    egui::Response,
    egui::Response,
    bool,
    bool,
    bool,
    bool,
    Vec<i64>,
) {
    let is_mobile = ui_kind == CardActionUiKind::Mobile;
    let frame = egui::Frame::group(ui.style())
        .fill(if hot {
            success_fill(ui)
        } else {
            ui.visuals().faint_bg_color
        })
        .inner_margin(egui::Margin::same(if is_mobile {
            MOBILE_CARD_INNER_MARGIN as i8
        } else {
            CARD_INNER_MARGIN as i8
        }));
    let mut start_clicked = false;
    let mut analyze_clicked = false;
    let mut diagnose_clicked = false;
    let mut export_clicked = false;
    let mut remove_requests = Vec::new();
    let mut drag_response: Option<egui::Response> = None;
    let response = frame
        .show(ui, |ui| {
            let card_wg = if is_mobile {
                mobile_card_content_width(ui)
            } else {
                card_content_width(ui)
            };
            let card_h = if is_mobile {
                MOBILE_GROUP_CARD_MIN_HEIGHT
            } else {
                GROUP_CARD_MIN_HEIGHT
            };
            ui.set_min_size(egui::vec2(card_wg, card_h));
            ui.set_height(card_h);
            ui.set_max_width(card_wg);
            ui.vertical_centered(|ui| {
                let info_response = ui
                    .vertical_centered(|ui| {
                        ui.heading("📁");
                        ui.strong(&group.name);
                        ui.small(format!(
                            "{} 个题库 · {} 道题",
                            group.deck_count, group.problem_count
                        ));
                        if is_mobile {
                            ui.small(format!("更新时间：{}", group.updated_at));
                        } else {
                            ui.small(format!("更新时间：{}", group.updated_at));
                            ui.small("拖动此区域可删除题组");
                        }
                    })
                    .response
                    .interact(egui::Sense::click_and_drag());
                drag_response = Some(if dragging || info_response.dragged() {
                    info_response.on_hover_cursor(egui::CursorIcon::Grabbing)
                } else {
                    info_response
                });
                (
                    start_clicked,
                    _,
                    analyze_clicked,
                    diagnose_clicked,
                    export_clicked,
                ) = render_card_actions(ui, None, ui_kind);

                render_group_deck_list(ui, group.id, group_decks, is_mobile, &mut remove_requests);
            });
        })
        .response;
    let drag_response = drag_response.unwrap_or_else(|| response.clone());
    (
        response,
        drag_response,
        start_clicked,
        analyze_clicked,
        diagnose_clicked,
        export_clicked,
        remove_requests,
    )
}

fn render_group_deck_list(
    ui: &mut egui::Ui,
    group_id: i64,
    group_decks: &[GroupDeckCard],
    compact: bool,
    remove_requests: &mut Vec<i64>,
) {
    if compact {
        ui.add_space(4.0);
        ui.menu_button("题组内题库", |ui| {
            if group_decks.is_empty() {
                ui.small("暂无题库");
            } else {
                for deck in group_decks {
                    ui.horizontal(|ui| {
                        ui.small(format!("{}（{}题）", deck.name, deck.problem_count));
                        if ui.small_button("移出").clicked() {
                            remove_requests.push(deck.id);
                            ui.close();
                        }
                    });
                }
            }
        });
        return;
    }

    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);
    ui.small("题组内题库");
    if group_decks.is_empty() {
        ui.small("暂无题库，可从题库卡片拖入这里。");
        return;
    }

    egui::ScrollArea::vertical()
        .max_height(110.0)
        .id_salt(("group_decks", group_id))
        .show(ui, |ui| {
            for deck in group_decks {
                ui.horizontal(|ui| {
                    let name_width = (ui.available_width() - 42.0).max(80.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(name_width, 20.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.small(format!("{}（{}题）", deck.name, deck.problem_count));
                        },
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("移出").clicked() {
                            remove_requests.push(deck.id);
                        }
                    });
                });
            }
        });
}

pub fn render_trash_zone(ui: &mut egui::Ui, active: bool) -> egui::Response {
    let frame = egui::Frame::group(ui.style())
        .fill(if active {
            danger_fill(ui)
        } else {
            ui.visuals().faint_bg_color
        })
        .inner_margin(egui::Margin::same(12));
    frame
        .show(ui, |ui| {
            ui.set_min_size(egui::vec2(ui.available_width(), 72.0));
            ui.vertical_centered(|ui| {
                ui.heading("🗑 删除区");
                ui.label("将题库或题组拖到此处删除");
            });
        })
        .response
        .interact(egui::Sense::click())
}

fn render_card_actions(
    ui: &mut egui::Ui,
    preview_label: Option<&str>,
    ui_kind: CardActionUiKind,
) -> (bool, bool, bool, bool, bool) {
    let mut start_clicked = false;
    let mut preview_clicked = false;
    let mut analyze_clicked = false;
    let mut diagnose_clicked = false;
    let mut export_clicked = false;

    ui.add_space(6.0);
    if let Some(label) = preview_label {
        let mut actions = [
            ("开始答题", &mut start_clicked),
            (label, &mut preview_clicked),
            ("导出", &mut export_clicked),
            ("分析题目", &mut analyze_clicked),
            ("学习诊断", &mut diagnose_clicked),
        ];
        match ui_kind {
            CardActionUiKind::Desktop => {
                render_card_action_grid(ui, &mut actions, DECK_ACTION_COLUMNS);
            }
            CardActionUiKind::Mobile => {
                let mut mobile_actions = [
                    ("开始", &mut start_clicked),
                    ("预览", &mut preview_clicked),
                    ("导出", &mut export_clicked),
                    ("分析", &mut analyze_clicked),
                    ("诊断", &mut diagnose_clicked),
                ];
                render_mobile_card_action_grid(ui, &mut mobile_actions, MOBILE_DECK_ACTION_COLUMNS);
            }
        }
    } else {
        let mut actions = [
            ("开始答题", &mut start_clicked),
            ("导出", &mut export_clicked),
            ("分析题目", &mut analyze_clicked),
            ("学习诊断", &mut diagnose_clicked),
        ];
        match ui_kind {
            CardActionUiKind::Desktop => {
                render_card_action_grid(ui, &mut actions, GROUP_ACTION_COLUMNS);
            }
            CardActionUiKind::Mobile => {
                let mut mobile_actions = [
                    ("开始", &mut start_clicked),
                    ("导出", &mut export_clicked),
                    ("分析", &mut analyze_clicked),
                    ("诊断", &mut diagnose_clicked),
                ];
                render_mobile_card_action_grid(ui, &mut mobile_actions, GROUP_ACTION_COLUMNS);
            }
        }
    }

    (
        start_clicked,
        preview_clicked,
        analyze_clicked,
        diagnose_clicked,
        export_clicked,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CardActionUiKind {
    Desktop,
    Mobile,
}

impl From<DeckCardUiKind> for CardActionUiKind {
    fn from(value: DeckCardUiKind) -> Self {
        match value {
            DeckCardUiKind::Desktop => Self::Desktop,
            DeckCardUiKind::Mobile => Self::Mobile,
        }
    }
}

fn card_content_width(ui: &egui::Ui) -> f32 {
    let fixed = CARD_WIDTH - CARD_INNER_MARGIN * 2.0;
    let available = ui.available_width() - CARD_INNER_MARGIN * 2.0;
    if available >= fixed {
        fixed
    } else {
        available.max(220.0)
    }
}

fn mobile_card_content_width(ui: &egui::Ui) -> f32 {
    (ui.available_width() - MOBILE_CARD_INNER_MARGIN * 2.0).max(MOBILE_CARD_MIN_WIDTH)
}

fn render_card_action_grid(ui: &mut egui::Ui, actions: &mut [(&str, &mut bool)], columns: usize) {
    let available = ui.available_width();
    let grid_width =
        columns as f32 * CARD_BUTTON_WIDTH + columns.saturating_sub(1) as f32 * CARD_ACTION_SPACING;
    let grid_width = grid_width.min(available);
    let button_width = ((grid_width - columns.saturating_sub(1) as f32 * CARD_ACTION_SPACING)
        / columns as f32)
        .max(68.0);

    for row in actions.chunks_mut(columns) {
        let row_width = row.len() as f32 * button_width
            + row.len().saturating_sub(1) as f32 * CARD_ACTION_SPACING;
        let left_padding = ((available - row_width) / 2.0).max(0.0);

        ui.horizontal(|ui| {
            ui.add_space(left_padding);
            ui.spacing_mut().item_spacing.x = CARD_ACTION_SPACING;
            for (label, clicked) in row.iter_mut() {
                **clicked |= ui
                    .add_sized(
                        [button_width, CARD_BUTTON_HEIGHT],
                        egui::Button::new(*label),
                    )
                    .clicked();
            }
        });
    }
}

fn render_mobile_card_action_grid(
    ui: &mut egui::Ui,
    actions: &mut [(&str, &mut bool)],
    columns: usize,
) {
    let available = ui.available_width();
    let full_row_width = (columns as f32 * CARD_BUTTON_WIDTH
        + columns.saturating_sub(1) as f32 * CARD_ACTION_SPACING)
        .min(available);
    let button_width = ((full_row_width - columns.saturating_sub(1) as f32 * CARD_ACTION_SPACING)
        / columns as f32)
        .max(68.0);

    for (row_index, row) in actions.chunks_mut(columns).enumerate() {
        if row_index > 0 {
            ui.add_space(CARD_ACTION_SPACING);
        }
        let row_width = row.len() as f32 * button_width
            + row.len().saturating_sub(1) as f32 * CARD_ACTION_SPACING;
        let (row_rect, _) = ui.allocate_exact_size(
            egui::vec2(available, CARD_BUTTON_HEIGHT),
            egui::Sense::hover(),
        );
        let start_x = row_rect.center().x - row_width / 2.0;
        for (index, (label, clicked)) in row.iter_mut().enumerate() {
            let x = start_x + index as f32 * (button_width + CARD_ACTION_SPACING);
            let rect = egui::Rect::from_min_size(
                egui::pos2(x, row_rect.min.y),
                egui::vec2(button_width, CARD_BUTTON_HEIGHT),
            );
            **clicked |= ui
                .put(
                    rect,
                    egui::Button::new(
                        egui::RichText::new(*label).size(MOBILE_CARD_BUTTON_TEXT_SIZE),
                    ),
                )
                .clicked();
        }
    }
}

fn accent_color(ui: &egui::Ui) -> egui::Color32 {
    ui.visuals().selection.stroke.color
}

fn selection_fill(ui: &egui::Ui) -> egui::Color32 {
    ui.visuals()
        .selection
        .bg_fill
        .gamma_multiply(if ui.visuals().dark_mode { 0.75 } else { 0.45 })
}

fn success_fill(ui: &egui::Ui) -> egui::Color32 {
    if ui.visuals().dark_mode {
        egui::Color32::from_rgb(48, 86, 68)
    } else {
        egui::Color32::from_rgb(218, 242, 226)
    }
}

fn danger_fill(ui: &egui::Ui) -> egui::Color32 {
    if ui.visuals().dark_mode {
        egui::Color32::from_rgb(92, 36, 36)
    } else {
        egui::Color32::from_rgb(252, 224, 224)
    }
}

fn compact_text(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    let tail: String = value
        .chars()
        .rev()
        .take(max_chars.saturating_sub(1))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("…{tail}")
}

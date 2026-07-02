use super::*;

#[derive(Default)]
struct PracticeLayoutActions {
    submit: bool,
    solution_guide: bool,
    skip: bool,
    skip_without_requeue: bool,
    back_to_library: bool,
}

impl ShuaForgeApp {
    pub(super) fn render_desktop_ui(&mut self, ctx: &egui::Context) {
        let panel_fill = ctx.style().visuals.panel_fill;
        egui::TopBottomPanel::top("desktop_top_bar")
            .frame(
                egui::Frame::default()
                    .fill(panel_fill)
                    .inner_margin(egui::Margin::symmetric(
                        DESKTOP_TOP_BAR_PAD_X as i8,
                        DESKTOP_TOP_BAR_PAD_Y as i8,
                    )),
            )
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.heading("ShuaForge");
                    ui.label("轻量 Rust 刷题助手");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button(self.theme_button_icon())
                            .on_hover_text(self.theme_button_tooltip())
                            .clicked()
                        {
                            self.toggle_theme(ctx);
                        }
                        if ui.button("关于").clicked() {
                            self.show_about = true;
                        }
                    });
                });
            });

        if self.view == AppView::Library {
            egui::SidePanel::right("desktop_config_panel")
                .frame(
                    egui::Frame::default()
                        .fill(panel_fill)
                        .inner_margin(egui::Margin::same(DESKTOP_CONTENT_PAD as i8)),
                )
                .resizable(true)
                .default_width(260.0)
                .width_range(220.0..=380.0)
                .show(ctx, |ui| {
                    ui.set_min_width(0.0);
                    egui::ScrollArea::vertical()
                        .id_salt("desktop_config_panel_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            self.render_settings_panel(ui);
                        });
                });
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(panel_fill)
                    .inner_margin(egui::Margin::same(DESKTOP_CONTENT_PAD as i8)),
            )
            .show(ctx, |ui| {
                self.render_desktop_main_content(ui);
            });
    }

    pub(super) fn render_mobile_ui(&mut self, ctx: &egui::Context) {
        let panel_fill = ctx.style().visuals.panel_fill;
        egui::TopBottomPanel::top("mobile_top_bar")
            .frame(
                egui::Frame::default()
                    .fill(panel_fill)
                    .inner_margin(egui::Margin {
                        left: MOBILE_SIDE_SAFE as i8,
                        right: (MOBILE_SIDE_SAFE + 8.0) as i8,
                        top: MOBILE_TOP_SAFE as i8,
                        bottom: 6,
                    }),
            )
            .min_height(MOBILE_TOP_SAFE + 36.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("ShuaForge");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button(self.theme_button_icon())
                            .on_hover_text(self.theme_button_tooltip())
                            .clicked()
                        {
                            self.toggle_theme(ctx);
                        }
                        if ui.button("关于").clicked() {
                            self.show_about = true;
                        }
                    });
                });
            });

        egui::TopBottomPanel::bottom("mobile_tabs")
            .frame(
                egui::Frame::default()
                    .fill(panel_fill)
                    .inner_margin(egui::Margin {
                        left: MOBILE_SIDE_SAFE as i8,
                        right: MOBILE_SIDE_SAFE as i8,
                        top: 4,
                        bottom: MOBILE_BOTTOM_SAFE as i8,
                    }),
            )
            .min_height(MOBILE_TOUCH_HEIGHT + MOBILE_BOTTOM_SAFE + 4.0)
            .show(ctx, |ui| {
                self.render_mobile_tabs(ui);
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(panel_fill)
                    .inner_margin(egui::Margin {
                        left: MOBILE_SIDE_SAFE as i8,
                        right: MOBILE_SIDE_SAFE as i8,
                        top: MOBILE_CONTENT_PAD as i8,
                        bottom: MOBILE_CONTENT_PAD as i8,
                    }),
            )
            .show(ctx, |ui| {
                self.render_mobile_main_content(ui);
            });
    }

    fn render_desktop_main_content(&mut self, ui: &mut egui::Ui) {
        match self.view {
            AppView::Practice => {
                let actions = self.render_desktop_practice_layout(ui);
                self.apply_practice_layout_actions(actions);
            }
            AppView::Library | AppView::Settings => self.render_desktop_library_content(ui),
        }
    }

    fn render_mobile_main_content(&mut self, ui: &mut egui::Ui) {
        match self.view {
            AppView::Settings => self.render_mobile_settings_content(ui),
            AppView::Practice => {
                let actions = if mobile_practice_prefers_wide_layout(ui) {
                    self.render_desktop_practice_layout(ui)
                } else {
                    self.render_mobile_practice_layout(ui)
                };
                self.apply_practice_layout_actions(actions);
            }
            AppView::Library => self.render_mobile_library_content(ui),
        }
    }

    fn render_desktop_library_content(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .id_salt("desktop_library_home_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                add_content_safe_area(ui, MOBILE_CONTENT_PAD, false);
                self.render_library_home(ui);
                ui.add_space(MOBILE_CONTENT_PAD);
            });
    }

    fn render_mobile_library_content(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .id_salt("mobile_library_home_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                add_content_safe_area(ui, MOBILE_CONTENT_PAD, true);
                self.render_library_home(ui);
                ui.add_space(MOBILE_BOTTOM_SAFE + MOBILE_CONTENT_PAD);
            });
    }

    fn render_mobile_settings_content(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .id_salt("mobile_settings_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.render_settings_panel(ui);
            });
    }

    fn render_desktop_practice_layout(&mut self, ui: &mut egui::Ui) -> PracticeLayoutActions {
        let mut actions = PracticeLayoutActions::default();
        let Some(deck) = &self.deck else {
            self.view = AppView::Library;
            return actions;
        };

        let left_w = (ui.available_width() * DESKTOP_QUESTION_RATIO).max(380.0);
        let right_w = (ui.available_width() - left_w - DESKTOP_GAP).max(DESKTOP_ANALYSIS_MIN_WIDTH);
        let avail_h = ui.available_height();

        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(left_w, avail_h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("desktop_question_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let stats = deck.stats();
                            ui.horizontal(|ui| {
                                if ui.button("← 主页").clicked() {
                                    actions.back_to_library = true;
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "总{} 剩{} 对{} 错{}",
                                                stats.total,
                                                stats.remaining,
                                                stats.correct,
                                                stats.wrong
                                            ))
                                            .size(12.0),
                                        );
                                    },
                                );
                            });
                            let completed = stats.correct + stats.wrong;
                            let progress = if stats.total == 0 {
                                0.0
                            } else {
                                completed as f32 / stats.total as f32
                            };
                            ui.add(
                                egui::ProgressBar::new(progress)
                                    .desired_width(ui.available_width())
                                    .text(format!(
                                        "Lv.{} · {completed}/{}",
                                        stats.correct / 10 + 1,
                                        stats.total
                                    )),
                            );
                            ui.separator();

                            if deck.is_finished() {
                                ui.add_space(40.0);
                                ui.vertical_centered(|ui| {
                                    ui.heading("本轮练习已完成");
                                    ui.label("可返回题库主页选择新的练习内容。");
                                });
                            } else if let Some(problem) = deck.current() {
                                let keyboard_actions = handle_practice_keyboard(
                                    ui,
                                    problem,
                                    &mut self.selected_single,
                                    &mut self.selected_multiple,
                                    &mut self.focused_choice_index,
                                    &mut self.keyboard_choice_focus_visible,
                                );
                                actions.submit |= keyboard_actions.submit;
                                actions.solution_guide |= keyboard_actions.solution_guide;
                                actions.skip |= keyboard_actions.skip;
                                actions.skip_without_requeue |=
                                    keyboard_actions.skip_without_requeue;
                                actions.back_to_library |= keyboard_actions.back_to_library;

                                ui.horizontal(|ui| {
                                    ui.strong(
                                        egui::RichText::new(format!("题目 #{}", problem.id))
                                            .size(15.0),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.small(problem_display_type_label(problem));
                                        },
                                    );
                                });
                                let rendered_tags = visible_tags(&problem.tags);
                                if !rendered_tags.is_empty() {
                                    ui.horizontal_wrapped(|ui| {
                                        for tag in &rendered_tags {
                                            ui.label(
                                                egui::RichText::new(format!("# {tag}"))
                                                    .color(egui::Color32::from_rgb(108, 166, 255))
                                                    .size(12.0),
                                            );
                                        }
                                    });
                                }
                                ui.add_space(10.0);
                                egui::Frame::group(ui.style())
                                    .inner_margin(egui::Margin::same(12))
                                    .show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        render_problem(ui, problem);
                                    });
                                ui.add_space(14.0);
                                ui.strong("你的答案");
                                ui.add_space(4.0);
                                let input_actions = render_answer_input(
                                    ui,
                                    problem,
                                    &mut self.answer_input,
                                    &mut self.selected_single,
                                    &mut self.selected_multiple,
                                    &mut self.focused_choice_index,
                                    &mut self.keyboard_choice_focus_visible,
                                );
                                actions.submit |= input_actions.submit;
                                actions.solution_guide |= input_actions.solution_guide;
                                actions.skip |= input_actions.skip;
                                actions.skip_without_requeue |= input_actions.skip_without_requeue;
                                ui.add_space(10.0);
                                ui.horizontal(|ui| {
                                    let text_problem = matches!(problem.kind(), ProblemType::Text);
                                    let show_shortcuts = self.keyboard_choice_focus_visible;
                                    let submit_label =
                                        submit_button_label("✓ 提交", text_problem, show_shortcuts);
                                    let guide_label =
                                        guide_button_label("AI 引导", text_problem, show_shortcuts);
                                    let skip_label =
                                        skip_button_label("跳过", text_problem, show_shortcuts);
                                    let dismiss_label = dismiss_button_label(
                                        "不再出现",
                                        text_problem,
                                        show_shortcuts,
                                    );
                                    if ui
                                        .add_sized(
                                            [
                                                if show_shortcuts { 128.0 } else { 84.0 },
                                                DESKTOP_BUTTON_HEIGHT,
                                            ],
                                            egui::Button::new(submit_label),
                                        )
                                        .clicked_by(egui::PointerButton::Primary)
                                    {
                                        actions.submit = true;
                                    }
                                    ui.separator();
                                    if ui
                                        .add_sized(
                                            [
                                                if show_shortcuts { 116.0 } else { 84.0 },
                                                DESKTOP_BUTTON_HEIGHT,
                                            ],
                                            egui::Button::new(guide_label),
                                        )
                                        .clicked_by(egui::PointerButton::Primary)
                                    {
                                        actions.solution_guide = true;
                                    }
                                    if ui
                                        .add_sized(
                                            [
                                                if show_shortcuts { 104.0 } else { 64.0 },
                                                DESKTOP_BUTTON_HEIGHT,
                                            ],
                                            egui::Button::new(skip_label),
                                        )
                                        .clicked_by(egui::PointerButton::Primary)
                                    {
                                        actions.skip = true;
                                    }
                                    if ui
                                        .add_sized(
                                            [
                                                if show_shortcuts { 128.0 } else { 104.0 },
                                                DESKTOP_BUTTON_HEIGHT,
                                            ],
                                            egui::Button::new(dismiss_label),
                                        )
                                        .on_hover_text("跳过且本轮不再插回，适合依赖当时材料的题目")
                                        .clicked_by(egui::PointerButton::Primary)
                                    {
                                        actions.skip_without_requeue = true;
                                    }
                                });
                            }
                            ui.add_space(16.0);
                        });
                },
            );

            ui.separator();

            ui.allocate_ui_with_layout(
                egui::vec2(right_w, avail_h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::Frame::group(ui.style())
                        .inner_margin(egui::Margin::same(8))
                        .show(ui, |ui| {
                            ui.strong(
                                egui::RichText::new(if self.is_ai_loading {
                                    "解析与分析…"
                                } else {
                                    "解析与分析"
                                })
                                .size(14.0),
                            );
                            render_analysis_progress_bar(ui, self.analysis_progress.as_ref());
                            ui.separator();
                            egui::ScrollArea::vertical()
                                .id_salt("desktop_analysis_side_scroll")
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    render_markdown_text(ui, &self.analysis, ui.available_width());
                                });
                        });
                },
            );
        });

        actions
    }

    fn render_mobile_practice_layout(&mut self, ui: &mut egui::Ui) -> PracticeLayoutActions {
        let mut actions = PracticeLayoutActions::default();
        let Some(deck) = &self.deck else {
            self.view = AppView::Library;
            return actions;
        };

        egui::ScrollArea::vertical()
            .id_salt("mobile_practice_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                add_content_safe_area(ui, MOBILE_CONTENT_PAD, true);
                let stats = deck.stats();
                ui.label(format!(
                    "总{} 剩{} 对{} 错{}",
                    stats.total, stats.remaining, stats.correct, stats.wrong
                ));
                let completed = stats.correct + stats.wrong;
                let progress = if stats.total == 0 {
                    0.0
                } else {
                    completed as f32 / stats.total as f32
                };
                ui.add(
                    egui::ProgressBar::new(progress)
                        .desired_width(ui.available_width())
                        .text(format!(
                            "Lv.{} · {completed}/{}",
                            stats.correct / 10 + 1,
                            stats.total
                        )),
                );
                ui.separator();

                if deck.is_finished() {
                    ui.heading("本轮练习已完成");
                    ui.label("本轮练习已完成。可返回题库主页选择新的练习内容。");
                } else if let Some(problem) = deck.current() {
                    let keyboard_actions = handle_practice_keyboard(
                        ui,
                        problem,
                        &mut self.selected_single,
                        &mut self.selected_multiple,
                        &mut self.focused_choice_index,
                        &mut self.keyboard_choice_focus_visible,
                    );
                    actions.submit |= keyboard_actions.submit;
                    actions.solution_guide |= keyboard_actions.solution_guide;
                    actions.skip |= keyboard_actions.skip;
                    actions.skip_without_requeue |= keyboard_actions.skip_without_requeue;
                    actions.back_to_library |= keyboard_actions.back_to_library;

                    ui.horizontal(|ui| {
                        ui.strong(format!("题目 #{}", problem.id));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.small(problem_display_type_label(problem));
                        });
                    });
                    let rendered_tags = visible_tags(&problem.tags);
                    if !rendered_tags.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            for tag in &rendered_tags {
                                ui.label(
                                    egui::RichText::new(format!("# {tag}"))
                                        .color(egui::Color32::from_rgb(108, 166, 255)),
                                );
                            }
                        });
                    }
                    ui.add_space(8.0);
                    render_problem(ui, problem);

                    ui.add_space(12.0);
                    ui.label("你的答案");
                    let input_actions = render_answer_input(
                        ui,
                        problem,
                        &mut self.answer_input,
                        &mut self.selected_single,
                        &mut self.selected_multiple,
                        &mut self.focused_choice_index,
                        &mut self.keyboard_choice_focus_visible,
                    );
                    actions.submit |= input_actions.submit;
                    actions.solution_guide |= input_actions.solution_guide;
                    actions.skip |= input_actions.skip;
                    actions.skip_without_requeue |= input_actions.skip_without_requeue;

                    let text_problem = matches!(problem.kind(), ProblemType::Text);
                    let show_shortcuts = self.keyboard_choice_focus_visible;
                    let submit_label =
                        submit_button_label("提交答案", text_problem, show_shortcuts);
                    if ui
                        .add_sized(
                            [ui.available_width(), MOBILE_TOUCH_HEIGHT],
                            egui::Button::new(submit_label),
                        )
                        .clicked_by(egui::PointerButton::Primary)
                    {
                        actions.submit = true;
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let gap = ui.spacing().item_spacing.x;
                        let button_width = ((ui.available_width() - gap * 2.0) / 3.0).max(72.0);
                        let guide_label =
                            guide_button_label("AI 引导", text_problem, show_shortcuts);
                        if ui
                            .add_sized(
                                [button_width, MOBILE_TOUCH_HEIGHT],
                                egui::Button::new(guide_label),
                            )
                            .clicked_by(egui::PointerButton::Primary)
                        {
                            actions.solution_guide = true;
                        }
                        let skip_label = skip_button_label("跳过", text_problem, show_shortcuts);
                        if ui
                            .add_sized(
                                [button_width, MOBILE_TOUCH_HEIGHT],
                                egui::Button::new(skip_label),
                            )
                            .clicked_by(egui::PointerButton::Primary)
                        {
                            actions.skip = true;
                        }
                        let dismiss_label =
                            dismiss_button_label("不再出现", text_problem, show_shortcuts);
                        if ui
                            .add_sized(
                                [button_width, MOBILE_TOUCH_HEIGHT],
                                egui::Button::new(dismiss_label),
                            )
                            .on_hover_text("适合依赖课堂表格、当时材料等复习阶段无法作答的题目")
                            .clicked_by(egui::PointerButton::Primary)
                        {
                            actions.skip_without_requeue = true;
                        }
                    });
                }

                ui.add_space(18.0);
                egui::Frame::group(ui.style())
                    .inner_margin(egui::Margin::symmetric(12, 12))
                    .corner_radius(egui::CornerRadius::same(12))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.heading(if self.is_ai_loading {
                            "解析与分析…"
                        } else {
                            "解析与分析"
                        });
                        render_analysis_progress_bar(ui, self.analysis_progress.as_ref());
                        ui.add_space(8.0);
                        render_markdown_text(ui, &self.analysis, ui.available_width());
                        ui.add_space(4.0);
                    });
                ui.add_space(MOBILE_BOTTOM_SAFE + MOBILE_CONTENT_PAD);
            });

        actions
    }

    fn apply_practice_layout_actions(&mut self, actions: PracticeLayoutActions) {
        if actions.submit {
            self.submit_answer();
        }
        if actions.solution_guide {
            self.request_solution_guide();
        }
        if actions.back_to_library {
            self.back_to_library();
        }
        if actions.skip_without_requeue
            && let Some(deck) = &mut self.deck
        {
            deck.skip_without_requeue();
            self.clear_answer_inputs();
            self.guided_problem_id = None;
            self.persist_current_practice_session();
            self.status = "已跳过，当前题本轮不再插回题库。".into();
            return;
        }
        if actions.skip
            && let Some(deck) = &mut self.deck
        {
            deck.skip();
            self.clear_answer_inputs();
            self.guided_problem_id = None;
            self.persist_current_practice_session();
            self.status = "已跳过，题目已重新插回题库。".into();
        }
    }
}

fn mobile_practice_prefers_wide_layout(ui: &egui::Ui) -> bool {
    let size = ui.available_size();
    size.x >= 720.0 && size.x > size.y * 1.15
}

fn submit_button_label(base: &'static str, text_problem: bool, show_shortcut: bool) -> String {
    shortcut_button_label(base, text_problem, show_shortcut, "Ctrl+Enter", "Enter")
}

fn guide_button_label(base: &'static str, text_problem: bool, show_shortcut: bool) -> String {
    shortcut_button_label(base, text_problem, show_shortcut, "Ctrl+I", "I")
}

fn skip_button_label(base: &'static str, text_problem: bool, show_shortcut: bool) -> String {
    shortcut_button_label(base, text_problem, show_shortcut, "Ctrl+S", "S")
}

fn dismiss_button_label(base: &'static str, text_problem: bool, show_shortcut: bool) -> String {
    shortcut_button_label(base, text_problem, show_shortcut, "Ctrl+D", "D")
}

fn shortcut_button_label(
    base: &'static str,
    text_problem: bool,
    show_shortcut: bool,
    text_shortcut: &'static str,
    choice_shortcut: &'static str,
) -> String {
    if !show_shortcut {
        return base.to_owned();
    }
    let shortcut = if text_problem {
        text_shortcut
    } else {
        choice_shortcut
    };
    format!("{base} {shortcut}")
}

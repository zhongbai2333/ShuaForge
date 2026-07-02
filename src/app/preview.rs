use crate::problem::{Problem, ProblemAnswerSource, ProblemType, visible_tags};
use eframe::egui;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProblemPreviewAction {
    Start { problem_id: String, answer: String },
    Save { problem_id: String, answer: String },
    Cancel,
}

pub fn render_problem_preview_row(
    ui: &mut egui::Ui,
    index: usize,
    problem: &Problem,
    is_editing: bool,
    editing_answer: &mut String,
) -> Option<ProblemPreviewAction> {
    let mut action = None;
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong(format!("#{}", index + 1));
                ui.label(format!("ID：{}", problem.id));
                ui.separator();
                ui.label(problem_display_type_label(problem));
                ui.label(format!(
                    "答案来源：{}",
                    answer_source_label(problem.state.answer_source)
                ));
                if problem.needs_ai_review() {
                    ui.colored_label(egui::Color32::YELLOW, "需复核");
                }
                ui.separator();
                if !is_editing && ui.button("修改答案").clicked() {
                    action = Some(ProblemPreviewAction::Start {
                        problem_id: problem.id.clone(),
                        answer: problem.answer.clone(),
                    });
                }
            });

            let rendered_tags = visible_tags(&problem.tags);
            if !rendered_tags.is_empty() {
                ui.small(format!("标签：{}", rendered_tags.join("、")));
            }

            let question = problem.question_text();
            let prompt_preview = if question.trim().is_empty() {
                problem.prompt.trim()
            } else {
                question.trim()
            };
            ui.label(compact_text(prompt_preview, 180));

            match problem.kind() {
                ProblemType::SingleChoice | ProblemType::MultipleChoice => {
                    let options = problem.options();
                    if !options.is_empty() {
                        ui.small(format!(
                            "选项：{}",
                            options
                                .iter()
                                .take(6)
                                .map(|option| format!(
                                    "{}. {}",
                                    option.key,
                                    compact_text(&option.text, 28)
                                ))
                                .collect::<Vec<_>>()
                                .join("；")
                        ));
                    }
                }
                ProblemType::Text => {}
            }

            if is_editing {
                ui.label("人工修正标准答案");
                ui.add(
                    egui::TextEdit::multiline(editing_answer)
                        .desired_rows(2)
                        .hint_text("请输入人工确认后的标准答案，例如 A、AC、正确，或文本答案"),
                );
                ui.horizontal_wrapped(|ui| {
                    if ui.button("保存答案").clicked() {
                        action = Some(ProblemPreviewAction::Save {
                            problem_id: problem.id.clone(),
                            answer: editing_answer.clone(),
                        });
                    }
                    if ui.button("取消").clicked() {
                        action = Some(ProblemPreviewAction::Cancel);
                    }
                    ui.small("保存后会标记为人工确认，并清除需复核状态。");
                });
            } else {
                ui.small(format!(
                    "答案：{}{}",
                    compact_text(&display_answer_value(problem, &problem.answer), 80),
                    if problem.state.user_answer.trim().is_empty() {
                        String::new()
                    } else {
                        format!(
                            "；我的答案：{}",
                            compact_text(
                                &display_answer_value(problem, &problem.state.user_answer),
                                80
                            )
                        )
                    }
                ));
            }
        });
    action
}

fn problem_display_type_label(problem: &Problem) -> &'static str {
    if problem.is_judgement() {
        "判断题"
    } else {
        problem_type_label(problem.kind())
    }
}

fn problem_type_label(problem_type: ProblemType) -> &'static str {
    match problem_type {
        ProblemType::SingleChoice => "单选题",
        ProblemType::MultipleChoice => "多选题",
        ProblemType::Text => "文本题",
    }
}

fn display_answer_value(problem: &Problem, answer: &str) -> String {
    if !problem.is_judgement() {
        return answer.to_owned();
    }
    match answer.trim() {
        "A" | "a" | "对" | "正确" | "√" => "对".into(),
        "B" | "b" | "错" | "错误" | "×" => "错".into(),
        value => value.to_owned(),
    }
}

fn answer_source_label(source: ProblemAnswerSource) -> &'static str {
    match source {
        ProblemAnswerSource::Standard => "标准答案",
        ProblemAnswerSource::UserTemporary => "临时答案",
        ProblemAnswerSource::ScoreInferred => "得分推断",
        ProblemAnswerSource::AiReviewed => "AI复核",
        ProblemAnswerSource::ManualReviewed => "人工确认",
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

use base64::{Engine as _, engine::general_purpose};
use serde::{Deserialize, Deserializer, Serialize};
use std::{
    error::Error,
    fs,
    io::{Cursor, Read},
    path::Path,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProblemType {
    SingleChoice,
    MultipleChoice,
    Text,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProblemAnswerSource {
    #[default]
    Standard,
    UserTemporary,
    ScoreInferred,
    AiReviewed,
    ManualReviewed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProblemReviewStatus {
    #[default]
    None,
    Pending,
    Accepted,
    Unknown,
    Conflict,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProblemReviewVerdict {
    #[default]
    Unknown,
    Correct,
    Wrong,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProblemReviewConfidence {
    High,
    Medium,
    Low,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ProblemState {
    #[serde(default, deserialize_with = "deserialize_string_like")]
    pub user_answer: String,
    #[serde(default)]
    pub answer_source: ProblemAnswerSource,
    #[serde(default)]
    pub review_needed: bool,
    #[serde(default)]
    pub review_status: ProblemReviewStatus,
    #[serde(default)]
    pub review_verdict: ProblemReviewVerdict,
    #[serde(default)]
    pub review_confidence: ProblemReviewConfidence,
    #[serde(default, deserialize_with = "deserialize_string_like")]
    pub score_display: String,
}

impl ProblemState {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Problem {
    pub id: String,
    pub prompt: String,
    pub answer: String,
    #[serde(default)]
    pub explanation: String,
    #[serde(default, deserialize_with = "deserialize_tags")]
    pub tags: Vec<String>,
    #[serde(default)]
    pub problem_type: Option<ProblemType>,
    #[serde(default)]
    pub deck_name: Option<String>,
    #[serde(default)]
    pub deck_info: Option<String>,
    #[serde(default, deserialize_with = "deserialize_images")]
    pub images: Vec<ProblemImage>,
    #[serde(default, flatten)]
    pub state: ProblemState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProblemImage {
    pub filename: String,
    pub mime_type: String,
    pub base64: String,
    #[serde(default)]
    pub alt_text: String,
    #[serde(default)]
    pub source_url: String,
}

#[derive(Debug, Deserialize)]
struct ZipProblemBank {
    #[serde(default)]
    deck_name: String,
    #[serde(default)]
    deck_info: String,
    problems: Vec<Problem>,
}

#[derive(Debug, Deserialize)]
struct DebugSnapshotProblemBank {
    #[serde(default)]
    bank: DebugSnapshotBankInfo,
    problems: Vec<DebugSnapshotProblem>,
}

#[derive(Debug, Default, Deserialize)]
struct DebugSnapshotBankInfo {
    #[serde(default)]
    name: String,
    #[serde(default)]
    info: String,
}

#[derive(Debug, Deserialize)]
struct DebugSnapshotProblem {
    id: String,
    extracted: DebugSnapshotExtractedProblem,
}

#[derive(Debug, Deserialize)]
struct DebugSnapshotExtractedProblem {
    prompt: String,
    answer: String,
    #[serde(default)]
    explanation: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_images")]
    images: Vec<ProblemImage>,
    #[serde(default)]
    user_answer: String,
    #[serde(default)]
    answer_source: ProblemAnswerSource,
    #[serde(default)]
    review_needed: bool,
    #[serde(default)]
    review_status: ProblemReviewStatus,
    #[serde(default)]
    review_verdict: ProblemReviewVerdict,
    #[serde(default)]
    score_display: String,
}

impl Problem {
    pub fn needs_ai_review(&self) -> bool {
        self.state.review_needed
            || matches!(
                self.state.review_status,
                ProblemReviewStatus::Pending
                    | ProblemReviewStatus::Unknown
                    | ProblemReviewStatus::Conflict
            )
    }

    pub fn is_correct(&self, input: &str) -> bool {
        match self.kind() {
            ProblemType::MultipleChoice => {
                normalize_choice_answer(input) == normalize_choice_answer(&self.answer)
            }
            ProblemType::SingleChoice | ProblemType::Text => {
                normalize_answer(input) == normalize_answer(&self.answer)
            }
        }
    }

    pub fn kind(&self) -> ProblemType {
        self.problem_type
            .unwrap_or_else(|| infer_problem_type(self))
    }

    pub fn question_text(&self) -> String {
        let mut lines = self.prompt.lines();
        let mut question = Vec::new();

        for line in lines.by_ref() {
            if parse_option_line(line).is_some() {
                break;
            }
            question.push(line.trim());
        }

        question.join("\n").trim().to_owned()
    }

    pub fn options(&self) -> Vec<ChoiceOption> {
        self.prompt.lines().filter_map(parse_option_line).collect()
    }

    pub fn is_judgement(&self) -> bool {
        is_judgement_options(&self.options())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceOption {
    pub key: String,
    pub text: String,
}

pub fn load_problems(path: &Path) -> Result<Vec<Problem>, Box<dyn Error + Send + Sync>> {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
    {
        Some(ext) if ext == "json" => load_json(path),
        Some(ext) if ext == "csv" => load_csv(path),
        Some(ext) if ext == "zip" => load_zip(path),
        _ => Err("仅支持 .json / .csv / .zip 题库文件".into()),
    }
}

pub fn load_problems_from_json_text(
    text: &str,
) -> Result<Vec<Problem>, Box<dyn Error + Send + Sync>> {
    parse_json_problems(text)
}

pub fn load_problems_from_text(text: &str) -> Result<Vec<Problem>, Box<dyn Error + Send + Sync>> {
    let text = text.trim_start_matches('\u{feff}').trim();
    if text.is_empty() {
        return Err("题库文本为空".into());
    }

    match parse_json_problems(text) {
        Ok(problems) => Ok(problems),
        Err(json_err) => parse_csv_problems(text).map_err(|csv_err| {
            format!("无法按 JSON 或 CSV 解析题库：JSON：{json_err}；CSV：{csv_err}").into()
        }),
    }
}

pub fn normalize_problem(problem: Problem) -> Problem {
    clean_problem(problem)
}

fn load_json(path: &Path) -> Result<Vec<Problem>, Box<dyn Error + Send + Sync>> {
    let text = fs::read_to_string(path)?;
    parse_json_problems(&text)
}

fn load_csv(path: &Path) -> Result<Vec<Problem>, Box<dyn Error + Send + Sync>> {
    let mut reader = csv::Reader::from_path(path)?;
    let mut problems = Vec::new();

    for record in reader.deserialize::<Problem>() {
        problems.push(record?);
    }

    validate_problems(problems)
}

fn load_zip(path: &Path) -> Result<Vec<Problem>, Box<dyn Error + Send + Sync>> {
    let file = fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;

    if let Ok(text) = read_zip_text(&mut archive, "problems.json") {
        let mut problems = parse_json_problems(&text)?;
        hydrate_zip_images(&mut archive, &mut problems)?;
        return validate_problems(problems);
    }

    if let Ok(text) = read_zip_text(&mut archive, "problems.csv") {
        let mut problems = parse_csv_problems(&text)?;
        hydrate_zip_images(&mut archive, &mut problems)?;
        return validate_problems(problems);
    }

    Err("ZIP 题库中未找到 problems.json 或 problems.csv".into())
}

fn parse_json_problems(text: &str) -> Result<Vec<Problem>, Box<dyn Error + Send + Sync>> {
    if let Ok(mut bank) = serde_json::from_str::<ZipProblemBank>(text) {
        for problem in &mut bank.problems {
            if problem
                .deck_name
                .as_deref()
                .unwrap_or_default()
                .trim()
                .is_empty()
                && !bank.deck_name.trim().is_empty()
            {
                problem.deck_name = Some(bank.deck_name.clone());
            }
            if problem
                .deck_info
                .as_deref()
                .unwrap_or_default()
                .trim()
                .is_empty()
                && !bank.deck_info.trim().is_empty()
            {
                problem.deck_info = Some(bank.deck_info.clone());
            }
        }
        return validate_problems(bank.problems);
    }

    if let Ok(snapshot) = serde_json::from_str::<DebugSnapshotProblemBank>(text) {
        let problems = snapshot
            .problems
            .into_iter()
            .map(|item| debug_snapshot_problem_to_problem(item, &snapshot.bank))
            .collect::<Vec<_>>();
        return validate_problems(problems);
    }

    let problems: Vec<Problem> = serde_json::from_str(text)?;
    validate_problems(problems)
}

fn debug_snapshot_problem_to_problem(
    item: DebugSnapshotProblem,
    bank: &DebugSnapshotBankInfo,
) -> Problem {
    let extracted = item.extracted;
    Problem {
        id: item.id,
        prompt: extracted.prompt,
        answer: extracted.answer,
        explanation: extracted.explanation,
        tags: extracted.tags,
        problem_type: None,
        deck_name: (!bank.name.trim().is_empty()).then(|| bank.name.clone()),
        deck_info: (!bank.info.trim().is_empty()).then(|| bank.info.clone()),
        images: extracted.images,
        state: ProblemState {
            user_answer: extracted.user_answer,
            answer_source: extracted.answer_source,
            review_needed: extracted.review_needed,
            review_status: extracted.review_status,
            review_verdict: extracted.review_verdict,
            review_confidence: ProblemReviewConfidence::Unknown,
            score_display: extracted.score_display,
        },
    }
}

fn parse_csv_problems(text: &str) -> Result<Vec<Problem>, Box<dyn Error + Send + Sync>> {
    let mut reader = csv::Reader::from_reader(Cursor::new(text.as_bytes()));
    let mut problems = Vec::new();

    for record in reader.deserialize::<Problem>() {
        problems.push(record?);
    }

    validate_problems(problems)
}

fn read_zip_text(
    archive: &mut zip::ZipArchive<fs::File>,
    name: &str,
) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut file = archive.by_name(name)?;
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    Ok(text.trim_start_matches('\u{feff}').to_owned())
}

fn hydrate_zip_images(
    archive: &mut zip::ZipArchive<fs::File>,
    problems: &mut [Problem],
) -> Result<(), Box<dyn Error + Send + Sync>> {
    for problem in problems {
        for (index, image) in problem.images.iter_mut().enumerate() {
            if !image.base64.trim().is_empty() {
                continue;
            }
            let Some(bytes) = read_zip_image_bytes(archive, &problem.id, index, &image.filename)?
            else {
                continue;
            };
            image.base64 = general_purpose::STANDARD.encode(bytes);
        }
    }
    Ok(())
}

fn read_zip_image_bytes(
    archive: &mut zip::ZipArchive<fs::File>,
    problem_id: &str,
    image_index: usize,
    filename: &str,
) -> Result<Option<Vec<u8>>, Box<dyn Error + Send + Sync>> {
    let suffix = format!("-{}-{}", image_index + 1, filename.replace('\\', "/"));
    let fallback = format!("assets/{}", filename.replace('\\', "/"));
    let target_name = (0..archive.len()).find_map(|index| {
        let file = archive.by_index(index).ok()?;
        let name = file.name().replace('\\', "/");
        if name == fallback
            || name.ends_with(&suffix)
            || name.contains(problem_id) && name.ends_with(filename)
        {
            Some(file.name().to_owned())
        } else {
            None
        }
    });

    let Some(target_name) = target_name else {
        return Ok(None);
    };
    let mut file = archive.by_name(&target_name)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(Some(bytes))
}

fn validate_problems(problems: Vec<Problem>) -> Result<Vec<Problem>, Box<dyn Error + Send + Sync>> {
    if problems.is_empty() {
        return Err("题库为空".into());
    }

    for problem in &problems {
        if problem.id.trim().is_empty() {
            return Err("题目 id 不能为空".into());
        }
        if problem.prompt.trim().is_empty() {
            return Err(format!("题目 {} 的题干不能为空", problem.id).into());
        }
        if problem.answer.trim().is_empty() {
            return Err(format!("题目 {} 的答案不能为空", problem.id).into());
        }
    }

    Ok(problems.into_iter().map(clean_problem).collect())
}

fn normalize_answer(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

pub fn normalize_choice_answer(value: &str) -> String {
    let mut chars: Vec<String> = value
        .split(|ch: char| {
            ch == ','
                || ch == '，'
                || ch == '、'
                || ch == ';'
                || ch == '；'
                || ch == '#'
                || ch.is_whitespace()
        })
        .flat_map(|part| part.chars())
        .filter(|ch| ch.is_ascii_alphabetic())
        .map(|ch| ch.to_ascii_uppercase().to_string())
        .collect();
    chars.sort();
    chars.dedup();
    chars.join("")
}

fn clean_problem(mut problem: Problem) -> Problem {
    problem.tags = clean_tags(&problem.tags);
    normalize_embedded_answer_state(&mut problem);
    normalize_judgement_answer(&mut problem);
    problem.problem_type = Some(infer_problem_type(&problem));
    backfill_legacy_problem_state(&mut problem);
    repair_score_inferred_state(&mut problem);
    prune_system_tags(&mut problem.tags);
    problem
}

fn normalize_judgement_answer(problem: &mut Problem) {
    let options = problem.options();
    if !is_judgement_options(&options) {
        return;
    }

    match problem.answer.trim() {
        "对" | "正确" | "√" => problem.answer = "A".into(),
        "错" | "错误" | "×" => problem.answer = "B".into(),
        _ => {}
    }
}

fn is_judgement_options(options: &[ChoiceOption]) -> bool {
    if options.len() != 2 {
        return false;
    }
    let first = options
        .first()
        .map(|option| option.text.trim())
        .unwrap_or_default();
    let second = options
        .get(1)
        .map(|option| option.text.trim())
        .unwrap_or_default();
    matches!((first, second), ("对" | "正确" | "√", "错" | "错误" | "×"))
}

fn normalize_embedded_answer_state(problem: &mut Problem) {
    let (answer_without_score, score_display) = strip_trailing_score_line(&problem.answer);
    if answer_without_score != problem.answer {
        problem.answer = answer_without_score;
    }
    if problem.state.score_display.trim().is_empty()
        && let Some(score_display) = score_display
    {
        problem.state.score_display = score_display;
    }

    let (correct_answer, user_answer) = extract_labeled_answers(&problem.answer);

    if let Some(user_answer) = user_answer
        && problem.state.user_answer.trim().is_empty()
    {
        problem.state.user_answer = user_answer;
    }

    if let Some(correct_answer) = correct_answer {
        problem.answer = correct_answer;
    } else if let Some(user_answer) = extract_labeled_answers(&problem.answer).1 {
        problem.answer = user_answer;
    }
}

fn strip_trailing_score_line(value: &str) -> (String, Option<String>) {
    let mut lines = value.lines().collect::<Vec<_>>();
    let Some(last) = lines.last().map(|line| line.trim()) else {
        return (value.to_owned(), None);
    };
    let Some(score) = parse_score_line(last) else {
        return (value.to_owned(), None);
    };
    lines.pop();
    (lines.join("\n").trim().to_owned(), Some(score))
}

fn parse_score_line(value: &str) -> Option<String> {
    let value = value.trim().trim_matches(['*', ' ']);
    let number = value.strip_suffix('分')?.trim();
    if number.is_empty() {
        return None;
    }
    let mut seen_dot = false;
    if number.chars().all(|ch| {
        if ch == '.' {
            if seen_dot {
                false
            } else {
                seen_dot = true;
                true
            }
        } else {
            ch.is_ascii_digit()
        }
    }) {
        Some(number.to_owned())
    } else {
        None
    }
}

fn extract_labeled_answers(text: &str) -> (Option<String>, Option<String>) {
    let normalized = text.replace('*', "");
    let correct = extract_first_labeled_answer(&normalized, &["正确答案", "参考答案", "标准答案"]);
    let user = extract_first_labeled_answer(&normalized, &["我的答案", "你的答案", "作答答案"]);
    (correct, user)
}

fn extract_first_labeled_answer(text: &str, labels: &[&str]) -> Option<String> {
    const ALL_LABELS: &[&str] = &[
        "正确答案",
        "参考答案",
        "标准答案",
        "我的答案",
        "你的答案",
        "作答答案",
        "答案解析",
        "解析",
        "知识点",
        "考点",
        "标签",
        "得分",
    ];

    let mut best_match: Option<(usize, String)> = None;
    for label in labels {
        let mut start_at = 0usize;
        while let Some(found) = text[start_at..].find(label) {
            let index = start_at + found;
            let after_label = &text[index + label.len()..];
            let after_label = after_label.trim_start_matches([' ', '\t', ':', '：']);
            let end =
                next_labeled_section_index(after_label, ALL_LABELS).unwrap_or(after_label.len());
            if let Some(answer) = clean_labeled_answer_value(&after_label[..end]) {
                match &best_match {
                    Some((best_index, _)) if *best_index <= index => {}
                    _ => best_match = Some((index, answer)),
                }
                break;
            }
            start_at = index + label.len();
        }
    }
    best_match.map(|(_, answer)| answer)
}

fn next_labeled_section_index(text: &str, labels: &[&str]) -> Option<usize> {
    labels.iter().filter_map(|label| text.find(label)).min()
}

fn clean_labeled_answer_value(value: &str) -> Option<String> {
    let value = value
        .trim()
        .trim_matches('*')
        .trim_end_matches([';', '；'])
        .trim();
    if value.is_empty() {
        return None;
    }

    let normalized_choice = normalize_choice_answer(value);
    if !normalized_choice.is_empty() {
        return Some(normalized_choice);
    }

    if let Some((prefix, _)) = value.split_once([':', '：'])
        && prefix.trim().chars().count() == 1
    {
        let normalized_prefix = normalize_choice_answer(prefix.trim());
        if !normalized_prefix.is_empty() {
            return Some(normalized_prefix);
        }
    }

    Some(value.to_owned())
}

pub fn clean_tags(tags: &[String]) -> Vec<String> {
    let mut cleaned = Vec::new();
    for tag in tags {
        for item in tag.split(|ch: char| {
            ch == ','
                || ch == '，'
                || ch == '、'
                || ch == ';'
                || ch == '；'
                || ch == '\n'
                || ch == '\r'
        }) {
            let value = item
                .trim()
                .trim_start_matches('：')
                .trim_start_matches(':')
                .trim_start_matches("知识点")
                .trim_start_matches('：')
                .trim_start_matches(':')
                .trim();
            if value.is_empty() || value == "知识点" || value == "考点" || value == "标签" {
                continue;
            }
            if !cleaned.iter().any(|existing: &String| existing == value) {
                cleaned.push(value.to_owned());
            }
        }
    }
    cleaned
}

pub fn visible_tags(tags: &[String]) -> Vec<String> {
    let mut visible = Vec::new();
    for tag in tags {
        let Some(value) = normalize_visible_tag(tag) else {
            continue;
        };
        if !visible.iter().any(|existing: &String| existing == &value) {
            visible.push(value);
        }
    }
    visible
}

pub fn is_system_tag(tag: &str) -> bool {
    matches!(
        tag.trim(),
        "无标准答案"
            | "按得分推断"
            | "作答正确"
            | "作答错误"
            | "待复核"
            | "多选需复核"
            | "填空需复核"
            | "AI复核完成"
            | "AI复核无法判断"
            | "AI复核冲突"
            | "AI导入"
            | "未批改"
            | "AI批改"
            | "整卷得分反推"
            | "结果页显示规则反推"
    ) || tag.trim().starts_with("本题得分:")
        || tag.trim().starts_with("AI复核结论:")
}

fn normalize_visible_tag(tag: &str) -> Option<String> {
    let value = tag.trim();
    if value.is_empty() || is_system_tag(value) {
        return None;
    }
    if let Some(point) = value.strip_prefix("AI知识点:") {
        let point = point.trim();
        if point.is_empty() {
            None
        } else {
            Some(point.to_owned())
        }
    } else {
        Some(value.to_owned())
    }
}

fn prune_system_tags(tags: &mut Vec<String>) {
    tags.retain(|tag| !is_system_tag(tag));
}

fn backfill_legacy_problem_state(problem: &mut Problem) {
    if !problem.state.is_default() {
        return;
    }

    let mut state = ProblemState {
        score_display: legacy_score_display(&problem.tags).unwrap_or_default(),
        ..Default::default()
    };

    let has_pending_text = problem.explanation.contains("未批改：页面没有提供正确答案")
        || problem
            .explanation
            .contains("因页面未给出标准答案，需人工补全正确答案");
    let has_score_inferred = problem.tags.iter().any(|tag| tag == "按得分推断")
        || problem
            .explanation
            .contains("本题得分可用于判断该作答是否正确");
    let has_ai_review = problem.explanation.contains("AI复核结果：")
        || problem.tags.iter().any(|tag| {
            matches!(tag.as_str(), "AI复核完成" | "AI复核无法判断" | "AI复核冲突")
                || tag.starts_with("AI复核结论:")
        });

    if has_ai_review {
        state.answer_source = ProblemAnswerSource::AiReviewed;
        state.review_verdict = legacy_review_verdict(problem);
        state.review_confidence = legacy_review_confidence(problem);
        state.review_status = if problem.tags.iter().any(|tag| tag == "AI复核冲突")
            || problem.explanation.contains("冲突")
        {
            ProblemReviewStatus::Conflict
        } else if problem.tags.iter().any(|tag| tag == "AI复核无法判断")
            || problem.explanation.contains("未自动采用")
            || problem.explanation.contains("无法判断")
        {
            ProblemReviewStatus::Unknown
        } else {
            ProblemReviewStatus::Accepted
        };
        state.review_needed = state.review_status != ProblemReviewStatus::Accepted;
    } else if has_score_inferred {
        state.answer_source = ProblemAnswerSource::ScoreInferred;
        state.user_answer = problem.answer.clone();
        state.review_verdict = if problem.tags.iter().any(|tag| tag == "作答正确") {
            ProblemReviewVerdict::Correct
        } else if problem.tags.iter().any(|tag| tag == "作答错误") {
            ProblemReviewVerdict::Wrong
        } else {
            ProblemReviewVerdict::Unknown
        };
        state.review_needed = problem
            .tags
            .iter()
            .any(|tag| matches!(tag.as_str(), "待复核" | "多选需复核" | "填空需复核"))
            || state.review_verdict == ProblemReviewVerdict::Wrong
            || !matches!(problem.kind(), ProblemType::SingleChoice);
        state.review_status = if state.review_needed {
            ProblemReviewStatus::Pending
        } else {
            ProblemReviewStatus::None
        };
    } else if has_pending_text
        || problem.tags.iter().any(|tag| {
            matches!(
                tag.as_str(),
                "未批改" | "AI批改" | "待复核" | "多选需复核" | "填空需复核"
            )
        })
    {
        state.answer_source = ProblemAnswerSource::UserTemporary;
        state.user_answer = problem.answer.clone();
        state.review_needed = true;
        state.review_status = ProblemReviewStatus::Pending;
    }

    problem.state = state;
}

fn repair_score_inferred_state(problem: &mut Problem) {
    if matches!(
        problem.state.answer_source,
        ProblemAnswerSource::AiReviewed | ProblemAnswerSource::ManualReviewed
    ) {
        return;
    }
    let has_score_inferred_text = problem
        .explanation
        .contains("页面没有提供标准答案，已使用“我的答案”作为导出答案")
        || problem
            .explanation
            .contains("本题得分可用于判断该作答是否正确");
    if !has_score_inferred_text && problem.state.answer_source != ProblemAnswerSource::ScoreInferred
    {
        return;
    }

    if problem.state.user_answer.trim().is_empty() {
        problem.state.user_answer = problem.answer.clone();
    }
    problem.state.answer_source = ProblemAnswerSource::ScoreInferred;

    let score_value = problem
        .state
        .score_display
        .trim()
        .parse::<f64>()
        .ok()
        .unwrap_or(0.0);
    match problem.kind() {
        ProblemType::SingleChoice => {
            if score_value > 0.0 {
                problem.state.review_verdict = ProblemReviewVerdict::Correct;
                problem.state.review_needed = false;
                problem.state.review_status = ProblemReviewStatus::None;
            } else {
                problem.state.review_verdict = ProblemReviewVerdict::Wrong;
                problem.state.review_needed = true;
                problem.state.review_status = ProblemReviewStatus::Pending;
            }
        }
        ProblemType::MultipleChoice | ProblemType::Text => {
            problem.state.review_verdict = if score_value <= 0.0 {
                ProblemReviewVerdict::Wrong
            } else {
                ProblemReviewVerdict::Unknown
            };
            problem.state.review_needed = true;
            problem.state.review_status = ProblemReviewStatus::Pending;
        }
    }
}

fn legacy_score_display(tags: &[String]) -> Option<String> {
    tags.iter().find_map(|tag| {
        tag.strip_prefix("本题得分:")
            .map(str::trim)
            .map(|value| value.trim_end_matches('分').trim().to_owned())
            .filter(|value| !value.is_empty())
    })
}

fn legacy_review_verdict(problem: &Problem) -> ProblemReviewVerdict {
    if problem.tags.iter().any(|tag| tag == "作答正确")
        || problem.tags.iter().any(|tag| tag == "AI复核结论:正确")
        || problem.explanation.contains("AI复核结果：正确")
    {
        ProblemReviewVerdict::Correct
    } else if problem.tags.iter().any(|tag| tag == "作答错误")
        || problem.tags.iter().any(|tag| tag == "AI复核结论:错误")
        || problem.explanation.contains("AI复核结果：错误")
    {
        ProblemReviewVerdict::Wrong
    } else {
        ProblemReviewVerdict::Unknown
    }
}

fn legacy_review_confidence(problem: &Problem) -> ProblemReviewConfidence {
    if problem.explanation.contains("置信度：高") {
        ProblemReviewConfidence::High
    } else if problem.explanation.contains("置信度：中") {
        ProblemReviewConfidence::Medium
    } else if problem.explanation.contains("置信度：低") {
        ProblemReviewConfidence::Low
    } else {
        ProblemReviewConfidence::Unknown
    }
}

fn infer_problem_type(problem: &Problem) -> ProblemType {
    let answer = normalize_choice_answer(&problem.answer);
    let option_count = problem.options().len();

    if option_count >= 2 && (answer.len() > 1 || looks_like_multiple_choice(problem)) {
        ProblemType::MultipleChoice
    } else if option_count >= 2 && answer.len() == 1 {
        ProblemType::SingleChoice
    } else {
        ProblemType::Text
    }
}

fn looks_like_multiple_choice(problem: &Problem) -> bool {
    let question = problem.question_text();
    question.contains("哪些")
        || question.contains("多选")
        || question.contains("正确的有")
        || question.contains("包括")
        || question.contains("属于") && question.contains("有")
        || question.contains("因素有")
        || question.contains("方法有")
        || question.contains("例子")
}

fn parse_option_line(line: &str) -> Option<ChoiceOption> {
    let trimmed = line.trim();
    let mut chars = trimmed.chars();
    let key = chars.next()?.to_ascii_uppercase();
    if !key.is_ascii_alphabetic() {
        return None;
    }
    let separator = chars.next()?;
    if !matches!(separator, '.' | '、' | '．') {
        return None;
    }
    let text = chars.as_str().trim().to_owned();
    Some(ChoiceOption {
        key: key.to_string(),
        text,
    })
}

fn deserialize_tags<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Tags {
        List(Vec<String>),
        Text(String),
    }

    let tags = Option::<Tags>::deserialize(deserializer)?;
    Ok(match tags {
        Some(Tags::List(values)) => values,
        Some(Tags::Text(value)) => value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        None => Vec::new(),
    })
}

fn deserialize_images<'de, D>(deserializer: D) -> Result<Vec<ProblemImage>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Images {
        List(Vec<ProblemImage>),
        Text(String),
    }

    let images = Option::<Images>::deserialize(deserializer)?;
    Ok(match images {
        Some(Images::List(values)) => values,
        Some(Images::Text(value)) => {
            let value = value.trim();
            if value.is_empty() {
                Vec::new()
            } else {
                serde_json::from_str::<Vec<ProblemImage>>(value)
                    .map_err(serde::de::Error::custom)?
            }
        }
        None => Vec::new(),
    })
}

fn deserialize_string_like<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringLike {
        Text(String),
        Integer(i64),
        Unsigned(u64),
        Float(f64),
        Bool(bool),
    }

    let value = Option::<StringLike>::deserialize(deserializer)?;
    Ok(match value {
        Some(StringLike::Text(text)) => text,
        Some(StringLike::Integer(number)) => number.to_string(),
        Some(StringLike::Unsigned(number)) => number.to_string(),
        Some(StringLike::Float(number)) => {
            if number.fract() == 0.0 {
                format!("{number:.0}")
            } else {
                number.to_string()
            }
        }
        Some(StringLike::Bool(value)) => value.to_string(),
        None => String::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        Problem, clean_tags, deserialize_images, deserialize_tags, normalize_answer, visible_tags,
    };
    use serde::Deserialize;

    #[test]
    fn answers_are_case_and_space_insensitive() {
        let problem = Problem {
            id: "1".into(),
            prompt: "Rust 所有权英文？".into(),
            answer: "Ownership".into(),
            explanation: String::new(),
            tags: vec![],
            problem_type: None,
            deck_name: None,
            deck_info: None,
            images: vec![],
            state: super::ProblemState::default(),
        };

        assert!(problem.is_correct(" ownership "));
        assert_eq!(normalize_answer("  A   B  "), "a b");
    }

    #[test]
    fn tags_accept_comma_separated_text() {
        #[derive(Deserialize)]
        struct Row {
            #[serde(default, deserialize_with = "deserialize_tags")]
            tags: Vec<String>,
        }

        let row: Row = serde_json::from_str(r#"{"tags":"rust, ownership"}"#).unwrap();
        assert_eq!(row.tags, vec!["rust", "ownership"]);
    }

    #[test]
    fn images_accept_json_text_from_csv() {
        #[derive(Deserialize)]
        struct Row {
            #[serde(default, deserialize_with = "deserialize_images")]
            images: Vec<super::ProblemImage>,
        }

        let row: Row = serde_json::from_str(
            r#"{"images":"[{\"filename\":\"a.png\",\"mime_type\":\"image/png\",\"base64\":\"AA==\",\"alt_text\":\"图\",\"source_url\":\"https://example.test/a.png\"}]"}"#,
        )
        .unwrap();
        assert_eq!(row.images.len(), 1);
        assert_eq!(row.images[0].filename, "a.png");
        assert_eq!(row.images[0].source_url, "https://example.test/a.png");
    }

    #[test]
    fn text_import_accepts_json_problem_bank() {
        let json = r#"[{"id":"1","prompt":"1+1=?","answer":"2"}]"#;
        let problems = super::load_problems_from_text(json).expect("json text should parse");

        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].id, "1");
        assert_eq!(problems[0].answer, "2");
    }

    #[test]
    fn text_import_falls_back_to_csv() {
        let csv = "id,prompt,answer\n1,1+1=?,2\n";
        let problems = super::load_problems_from_text(csv).expect("csv text should parse");

        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].id, "1");
        assert_eq!(problems[0].prompt, "1+1=?");
    }

    #[test]
    fn image_source_url_is_backward_compatible() {
        let image: super::ProblemImage = serde_json::from_str(
            r#"{"filename":"a.png","mime_type":"image/png","base64":"AA==","alt_text":"图"}"#,
        )
        .unwrap();

        assert_eq!(image.source_url, "");
    }

    #[test]
    fn multiple_choice_answer_order_does_not_matter() {
        let problem = Problem {
            id: "1".into(),
            prompt: "多选\nA. 参数估计\nB. 绘图\nC. 假设检验".into(),
            answer: "A,C".into(),
            explanation: String::new(),
            tags: vec![],
            problem_type: Some(super::ProblemType::MultipleChoice),
            deck_name: None,
            deck_info: None,
            images: vec![],
            state: super::ProblemState::default(),
        };

        assert!(problem.is_correct("C A"));
    }

    #[test]
    fn tags_are_cleaned() {
        assert_eq!(
            clean_tags(&["：".into(), "知识点：\n测量尺度".into(), "测量尺度".into()]),
            vec!["测量尺度"]
        );
    }

    #[test]
    fn visible_tags_hide_system_statuses() {
        let tags = vec![
            "AI知识点:需求弹性".into(),
            "作答错误".into(),
            "待复核".into(),
            "本题得分:0分".into(),
            "统计学".into(),
            "AI复核结论:错误".into(),
        ];

        let visible = visible_tags(&tags);
        assert_eq!(visible, vec!["需求弹性", "统计学"]);
    }

    #[test]
    fn judgement_answers_are_normalized_to_choice_keys() {
        let problem = super::clean_problem(Problem {
            id: "judge".into(),
            prompt: "样本均值是总体均值的无偏估计。\nA. 对\nB. 错".into(),
            answer: "对".into(),
            explanation: String::new(),
            tags: vec![],
            problem_type: None,
            deck_name: None,
            deck_info: None,
            images: vec![],
            state: super::ProblemState::default(),
        });

        assert_eq!(problem.answer, "A");
        assert_eq!(problem.kind(), super::ProblemType::SingleChoice);
        assert!(problem.is_judgement());
    }

    #[test]
    fn judgement_answer_with_score_line_is_normalized() {
        let problem = super::normalize_problem(Problem {
            id: "judge-score".into(),
            prompt: "平均固定成本会随着产量增加而不断上升。\nA. 对\nB. 错".into(),
            answer: "错\n2分".into(),
            explanation: "页面没有提供标准答案，已使用“我的答案”作为导出答案；本题得分可用于判断该作答是否正确。 当前作答得分 2 分；填空/主观题评分规则不确定，需人工复核后再作为标准答案使用。".into(),
            tags: vec![],
            problem_type: Some(super::ProblemType::Text),
            deck_name: None,
            deck_info: None,
            images: vec![],
            state: super::ProblemState {
                user_answer: "错".into(),
                score_display: "2".into(),
                ..super::ProblemState::default()
            },
        });

        assert_eq!(problem.answer, "B");
        assert_eq!(problem.kind(), super::ProblemType::SingleChoice);
        assert!(problem.is_judgement());
        assert_eq!(
            problem.state.answer_source,
            super::ProblemAnswerSource::ScoreInferred
        );
        assert!(!problem.needs_ai_review());
    }

    #[test]
    fn multi_choice_keywords_infer_multiple_choice() {
        let problem = super::clean_problem(Problem {
            id: "multi".into(),
            prompt: "下列哪些是参数估计的评价标准(\nA. 无偏性\nB. 有效性\nC. 一致性\nD. 随机性"
                .into(),
            answer: "A".into(),
            explanation: "未批改：页面没有提供正确答案，已使用“我的答案”作为临时答案。".into(),
            tags: vec![],
            problem_type: None,
            deck_name: None,
            deck_info: None,
            images: vec![],
            state: super::ProblemState::default(),
        });

        assert_eq!(problem.kind(), super::ProblemType::MultipleChoice);
    }

    #[test]
    fn detects_pending_ai_review_problem() {
        let problem = super::normalize_problem(Problem {
            id: "pending".into(),
            prompt: "简答题".into(),
            answer: "我的旧答案".into(),
            explanation: "未批改：页面没有提供正确答案，已使用“我的答案”作为临时答案。".into(),
            tags: vec![],
            problem_type: None,
            deck_name: None,
            deck_info: None,
            images: vec![],
            state: super::ProblemState::default(),
        });

        assert!(problem.needs_ai_review());
    }

    #[test]
    fn score_inferred_correct_choice_does_not_need_ai_review() {
        let problem = super::normalize_problem(Problem {
            id: "score-correct".into(),
            prompt: "单选题\nA. 正确项\nB. 干扰项".into(),
            answer: "A".into(),
            explanation: "页面没有提供标准答案，已使用“我的答案”作为导出答案；本题得分可用于判断该作答是否正确。 当前作答得分 2.6 分，单选题可推断该答案正确。".into(),
            tags: vec!["无标准答案".into(), "按得分推断".into(), "本题得分:2.6分".into(), "作答正确".into()],
            problem_type: None,
            deck_name: None,
            deck_info: None,
            images: vec![],
            state: super::ProblemState::default(),
        });

        assert!(!problem.needs_ai_review());
    }

    #[test]
    fn score_inferred_wrong_choice_needs_ai_review() {
        let problem = super::normalize_problem(Problem {
            id: "score-wrong".into(),
            prompt: "单选题\nA. 正确项\nB. 错误项".into(),
            answer: "B".into(),
            explanation: "页面没有提供标准答案，已使用“我的答案”作为导出答案；本题得分可用于判断该作答是否正确。 当前作答得分 0 分，单选题可推断该作答错误；因页面未给出标准答案，需人工补全正确答案。".into(),
            tags: vec!["无标准答案".into(), "按得分推断".into(), "本题得分:0分".into(), "作答错误".into(), "待复核".into()],
            problem_type: None,
            deck_name: None,
            deck_info: None,
            images: vec![],
            state: super::ProblemState::default(),
        });

        assert!(problem.needs_ai_review());
    }

    #[test]
    fn mixed_answer_field_extracts_correct_and_user_answers() {
        let problem = super::normalize_problem(Problem {
            id: "judge-mixed".into(),
            prompt: "判断题\nA. 对\nB. 错".into(),
            answer: "我的答案:对\n正确答案:对".into(),
            explanation: String::new(),
            tags: vec![],
            problem_type: None,
            deck_name: None,
            deck_info: None,
            images: vec![],
            state: super::ProblemState::default(),
        });

        assert_eq!(problem.answer, "A");
        assert_eq!(problem.state.user_answer, "对");
        assert_eq!(problem.kind(), super::ProblemType::SingleChoice);
        assert!(problem.is_judgement());
    }

    #[test]
    fn csv_state_string_fields_accept_numeric_values() {
        let csv = concat!(
            "id,prompt,answer,explanation,tags,problem_type,deck_name,deck_info,images,user_answer,answer_source,review_needed,review_status,review_verdict,review_confidence,score_display\n",
            "p1,题目,A,,\"\",single_choice,,,\"[]\",42,score_inferred,true,pending,wrong,unknown,2.6\n"
        );

        let mut reader = csv::Reader::from_reader(std::io::Cursor::new(csv.as_bytes()));
        let problem = reader
            .deserialize::<Problem>()
            .next()
            .expect("csv row exists")
            .expect("csv row should deserialize");

        assert_eq!(problem.state.user_answer, "42");
        assert_eq!(problem.state.score_display, "2.6");
        assert_eq!(
            problem.state.answer_source,
            super::ProblemAnswerSource::ScoreInferred
        );
    }

    #[test]
    fn debug_snapshot_json_can_be_loaded_as_problem_bank() {
        let json = r#"
                {
                    "schema_version": 1,
                    "exporter_version": "0.1.5",
                    "bank": { "name": "1、导论", "info": "题量:1；满分:2.6" },
                    "problems": [
                        {
                            "id": "web-0001-demo",
                            "extracted": {
                                "prompt": "微观经济学的中心理论是\nA. 国民收入决定理论\nB. 价格理论",
                                "options": ["A. 国民收入决定理论", "B. 价格理论"],
                                "answer": "B",
                                "correct_answer": "B",
                                "user_answer": "B",
                                "score_display": "2.6",
                                "explanation": "",
                                "tags": ["微观经济学"],
                                "images": [
                                    {
                                        "filename": "p1.png",
                                        "mime_type": "image/png",
                                        "base64": "",
                                        "alt_text": "图1",
                                        "source_url": "https://example.test/p1.png"
                                    }
                                ],
                                "answer_source": "standard",
                                "review_needed": false,
                                "review_status": "none",
                                "review_verdict": "unknown"
                            },
                            "evidence": {},
                            "confidence": 0.98,
                            "warnings": []
                        }
                    ]
                }
                "#;

        let problems = super::parse_json_problems(json).expect("debug snapshot should parse");
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].id, "web-0001-demo");
        assert_eq!(problems[0].answer, "B");
        assert_eq!(problems[0].deck_name.as_deref(), Some("1、导论"));
        assert_eq!(problems[0].state.user_answer, "B");
        assert_eq!(problems[0].images.len(), 1);
        assert_eq!(
            problems[0].images[0].source_url,
            "https://example.test/p1.png"
        );
        assert_eq!(problems[0].kind(), super::ProblemType::SingleChoice);
    }
}

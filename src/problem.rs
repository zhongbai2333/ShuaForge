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

impl Problem {
    pub fn needs_ai_review(&self) -> bool {
        self.explanation.contains("未批改：页面没有提供正确答案")
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

    let problems: Vec<Problem> = serde_json::from_str(text)?;
    validate_problems(problems)
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
    normalize_judgement_answer(&mut problem);
    if problem.problem_type.is_none() {
        problem.problem_type = Some(infer_problem_type(&problem));
    }
    problem
}

fn normalize_judgement_answer(problem: &mut Problem) {
    if problem.options().len() != 2 {
        return;
    }

    let options = problem.options();
    let first = options
        .first()
        .map(|option| option.text.trim())
        .unwrap_or_default();
    let second = options
        .get(1)
        .map(|option| option.text.trim())
        .unwrap_or_default();
    if !matches!((first, second), ("对" | "正确" | "√", "错" | "错误" | "×")) {
        return;
    }

    match problem.answer.trim() {
        "对" | "正确" | "√" => problem.answer = "A".into(),
        "错" | "错误" | "×" => problem.answer = "B".into(),
        _ => {}
    }
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

#[cfg(test)]
mod tests {
    use super::{Problem, clean_tags, deserialize_images, deserialize_tags, normalize_answer};
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
        });

        assert_eq!(problem.answer, "A");
        assert_eq!(problem.kind(), super::ProblemType::SingleChoice);
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
        });

        assert_eq!(problem.kind(), super::ProblemType::MultipleChoice);
    }

    #[test]
    fn detects_pending_ai_review_problem() {
        let problem = Problem {
            id: "pending".into(),
            prompt: "简答题".into(),
            answer: "我的旧答案".into(),
            explanation: "未批改：页面没有提供正确答案，已使用“我的答案”作为临时答案。".into(),
            tags: vec![],
            problem_type: None,
            deck_name: None,
            deck_info: None,
            images: vec![],
        };

        assert!(problem.needs_ai_review());
    }
}

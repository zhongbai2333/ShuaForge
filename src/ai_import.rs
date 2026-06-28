use crate::{
    ai::AiConfig,
    problem::{Problem, ProblemType},
};
use calamine::{Reader, open_workbook_auto};
use serde::Deserialize;
use serde_json::json;
use std::{error::Error, fs, path::Path};

const MAX_IMPORT_TEXT_CHARS: usize = 80_000;

#[derive(Debug, Clone)]
pub struct AiImportResult {
    pub problems: Vec<Problem>,
    pub extracted_chars: usize,
}

#[derive(Debug, Deserialize)]
struct AiProblemImportEnvelope {
    problems: Vec<Problem>,
}

pub fn import_problem_bank_with_ai(
    config: &AiConfig,
    path: &Path,
) -> Result<AiImportResult, Box<dyn Error + Send + Sync>> {
    ensure_ai_import_config(config)?;
    let extracted_text = extract_problem_bank_text(path)?;
    if extracted_text.trim().is_empty() {
        return Err("文件中没有提取到可供 AI 识别的文本".into());
    }
    let extracted_chars = extracted_text.chars().count();
    let prompt = build_ai_import_prompt(path, &truncate_for_prompt(&extracted_text));
    let response_text = send_ai_import_prompt(config, prompt)?;
    let problems = parse_ai_import_response(&response_text)?;
    Ok(AiImportResult {
        problems,
        extracted_chars,
    })
}

fn ensure_ai_import_config(config: &AiConfig) -> Result<(), Box<dyn Error + Send + Sync>> {
    if !config.enabled {
        return Err("AI 导入需要先在设置中启用 AI".into());
    }
    if config.endpoint.trim().is_empty() || config.model.trim().is_empty() {
        return Err("AI 导入需要先填写 endpoint 和 model".into());
    }
    Ok(())
}

fn extract_problem_bank_text(path: &Path) -> Result<String, Box<dyn Error + Send + Sync>> {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("csv") => Ok(fs::read_to_string(path)?.trim_start_matches('\u{feff}').to_owned()),
        Some("xlsx") => extract_xlsx_text(path),
        Some("pdf") => Ok(pdf_extract::extract_text(path)?),
        _ => Err("AI 动态导入目前仅支持 .csv / .xlsx / .pdf".into()),
    }
}

fn extract_xlsx_text(path: &Path) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut workbook = open_workbook_auto(path)?;
    let mut output = String::new();
    for sheet_name in workbook.sheet_names().to_owned() {
        if let Ok(range) = workbook.worksheet_range(&sheet_name) {
            output.push_str(&format!("# Sheet: {sheet_name}\n"));
            for row in range.rows() {
                let cells = row
                    .iter()
                    .map(ToString::to_string)
                    .map(|value| value.replace(['\n', '\r', '\t'], " "))
                    .collect::<Vec<_>>();
                output.push_str(&cells.join("\t"));
                output.push('\n');
            }
            output.push('\n');
        }
    }
    Ok(output)
}

fn truncate_for_prompt(text: &str) -> String {
    let mut truncated = text.chars().take(MAX_IMPORT_TEXT_CHARS).collect::<String>();
    if text.chars().count() > MAX_IMPORT_TEXT_CHARS {
        truncated.push_str("\n\n[ShuaForge: 文件过长，以上为前部可解析内容，请尽量从已提供文本中提取题目。]");
    }
    truncated
}

fn build_ai_import_prompt(path: &Path, extracted_text: &str) -> String {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("unknown");
    format!(
        r#"你是 ShuaForge 题库导入器。请从用户提供的题库文件文本中提取题目，并只返回严格 JSON。

文件名：{filename}

输出格式必须是：
{{
  "problems": [
    {{
      "id": "稳定唯一ID，例如 ai-import-001",
      "prompt": "题干。选择题必须把选项写入 prompt，每个选项独立一行，如 A. ...",
      "answer": "标准答案。单选/判断使用 A/B/C；多选使用 ABC；填空/问答写文本答案",
      "explanation": "解析；没有则为空字符串",
      "tags": ["AI导入"],
      "problem_type": "single_choice|multiple_choice|text"
    }}
  ]
}}

规则：
1. 只能输出 JSON，不要 Markdown，不要代码块。
2. 不确定答案的题目不要导入。
3. 保留题干、选项、答案、解析和知识点标签。
4. 判断题请转换为两个选项：A. 对 / B. 错，并把 answer 转成 A 或 B。
5. id 在本次返回中必须唯一。
6. 如果原文件是表格，按列名/上下文推断题干、选项、答案、解析、题型。

文件文本如下：
{extracted_text}
"#
    )
}

fn send_ai_import_prompt(
    config: &AiConfig,
    prompt: String,
) -> Result<String, Box<dyn Error + Send + Sync>> {
    let body = json!({
        "model": config.model,
        "messages": [
            {"role": "user", "content": prompt}
        ],
        "stream": false
    })
    .to_string();
    let mut request = minreq::post(&config.endpoint)
        .with_header("content-type", "application/json")
        .with_timeout(config.timeout_secs.max(30))
        .with_body(body);
    if !config.api_key.trim().is_empty() {
        request = request.with_header("authorization", format!("Bearer {}", config.api_key.trim()));
    }
    let response = request.send()?;
    if response.status_code < 200 || response.status_code >= 300 {
        return Err(format!("AI 导入请求失败：HTTP {}", response.status_code).into());
    }
    Ok(response.as_str()?.to_owned())
}

fn parse_ai_import_response(text: &str) -> Result<Vec<Problem>, Box<dyn Error + Send + Sync>> {
    let content = extract_chat_content(text).unwrap_or_else(|| text.to_owned());
    let json_text = extract_json_object(&content).ok_or("AI 响应中没有找到 JSON 对象")?;
    let envelope: AiProblemImportEnvelope = serde_json::from_str(&json_text)?;
    validate_ai_import_problems(envelope.problems)
}

fn extract_chat_content(text: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    value
        .get("choices")?
        .as_array()?
        .first()?
        .get("message")?
        .get("content")?
        .as_str()
        .map(ToOwned::to_owned)
        .or_else(|| {
            value
                .get("choices")?
                .as_array()?
                .first()?
                .get("text")?
                .as_str()
                .map(ToOwned::to_owned)
        })
}

fn extract_json_object(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (start <= end).then(|| text[start..=end].to_owned())
}

fn validate_ai_import_problems(
    problems: Vec<Problem>,
) -> Result<Vec<Problem>, Box<dyn Error + Send + Sync>> {
    if problems.is_empty() {
        return Err("AI 没有提取到可导入题目".into());
    }
    let mut cleaned = Vec::new();
    for (index, mut problem) in problems.into_iter().enumerate() {
        if problem.id.trim().is_empty() {
            problem.id = format!("ai-import-{:04}", index + 1);
        }
        if problem.prompt.trim().is_empty() || problem.answer.trim().is_empty() {
            continue;
        }
        if problem.problem_type.is_none() {
            problem.problem_type = Some(ProblemType::Text);
        }
        if !problem.tags.iter().any(|tag| tag == "AI导入") {
            problem.tags.push("AI导入".into());
        }
        cleaned.push(problem);
    }
    if cleaned.is_empty() {
        return Err("AI 返回的题目缺少题干或答案，无法导入".into());
    }
    Ok(cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_chat_completion_json_problem_payload() {
        let response = r#"{
            "choices": [{"message": {"content": "{\"problems\":[{\"id\":\"p1\",\"prompt\":\"题干\\nA. 甲\\nB. 乙\",\"answer\":\"A\",\"explanation\":\"\",\"tags\":[\"经济学\"],\"problem_type\":\"single_choice\"}]}"}}]
        }"#;

        let problems = parse_ai_import_response(response).expect("parse response");

        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].id, "p1");
        assert!(problems[0].tags.contains(&"AI导入".into()));
    }
}
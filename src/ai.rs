use crate::{problem::Problem, store::AnswerRecord};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    error::Error,
    fs,
    io::{BufRead, BufReader},
    path::Path,
    sync::mpsc,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AiConfig {
    pub enabled: bool,
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

#[derive(Debug, Clone)]
pub struct AnalysisToolResult {
    pub tool_name: String,
    pub arguments_json: String,
    pub result: String,
}

#[derive(Debug, Clone)]
pub enum AnalysisStreamEvent {
    ToolCall { arguments_json: String },
    TextDelta(String),
    Finished,
    Failed(String),
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: String::new(),
            api_key: String::new(),
            model: String::new(),
            timeout_secs: default_timeout_secs(),
        }
    }
}

#[derive(Debug, Serialize)]
struct ExplainRequest<'a> {
    model: &'a str,
    prompt: String,
}

#[derive(Debug, Serialize)]
struct ChatCompletionRequest<'a> {
    model: &'a str,
    messages: Vec<ChatCompletionMessage<'a>>,
    stream: bool,
}

#[derive(Debug, Serialize)]
struct ChatCompletionMessage<'a> {
    role: &'a str,
    content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProblemSetToolRequest {
    tool: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    cursor: usize,
    #[serde(default = "default_problem_batch_limit")]
    limit: usize,
}

#[derive(Debug, Serialize)]
struct ProblemBatchResponse<'a> {
    cursor: usize,
    limit: usize,
    total: usize,
    read_count: usize,
    remaining_count: usize,
    read_fraction: f32,
    next_cursor: Option<usize>,
    problems: Vec<ProblemPreview<'a>>,
}

#[derive(Debug, Serialize)]
struct ProblemPreview<'a> {
    id: &'a str,
    problem_type: String,
    tags: &'a [String],
    prompt: &'a str,
    answer: &'a str,
    explanation: &'a str,
}

#[derive(Debug, Deserialize)]
struct ExplainResponse {
    explanation: Option<String>,
    choices: Option<Vec<Choice>>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: Option<Message>,
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Message {
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StreamResponse {
    choices: Option<Vec<StreamChoice>>,
}

#[derive(Debug, Deserialize)]
struct StreamChoice {
    delta: Option<StreamDelta>,
}

#[derive(Debug, Deserialize)]
struct StreamDelta {
    content: Option<String>,
}

pub fn load_ai_config(path: &Path) -> Result<AiConfig, Box<dyn Error + Send + Sync>> {
    let text = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&text)?)
}

pub fn save_ai_config(path: &Path, config: &AiConfig) -> Result<(), Box<dyn Error + Send + Sync>> {
    let text = serde_json::to_string_pretty(config)?;
    fs::write(path, text)?;
    Ok(())
}

pub fn explain_wrong_answer(
    config: &AiConfig,
    problem: &Problem,
    user_answer: &str,
) -> Result<String, Box<dyn Error + Send + Sync>> {
    if !config.enabled {
        return Ok(local_explanation(problem, user_answer));
    }

    if config.endpoint.trim().is_empty() || config.model.trim().is_empty() {
        return Ok("AI 已启用，但 endpoint/model 未配置。请先在配置区填写。".into());
    }

    let body = build_ai_request_body(config, build_prompt(problem, user_answer))?;
    let mut req = minreq::post(&config.endpoint)
        .with_header("content-type", "application/json")
        .with_timeout(config.timeout_secs)
        .with_body(body);

    if !config.api_key.trim().is_empty() {
        req = req.with_header("authorization", format!("Bearer {}", config.api_key.trim()));
    }

    let response = req.send()?;
    if response.status_code < 200 || response.status_code >= 300 {
        return Err(format_ai_http_error(
            response.status_code,
            &config.endpoint,
            response.as_str().ok(),
        )
        .into());
    }

    let text = response.as_str()?;
    extract_explanation(text)
        .or_else(|| Some(text.to_string()))
        .ok_or_else(|| "AI 响应为空".into())
}

pub fn review_answer(
    config: &AiConfig,
    problem: &Problem,
    user_answer: &str,
) -> Result<String, Box<dyn Error + Send + Sync>> {
    if !config.enabled {
        return Ok(local_review(problem, user_answer));
    }

    if config.endpoint.trim().is_empty() || config.model.trim().is_empty() {
        return Ok("AI 批改需要先启用并填写 endpoint/model。".into());
    }

    let body = build_ai_request_body(config, build_review_prompt(problem, user_answer))?;
    let mut req = minreq::post(&config.endpoint)
        .with_header("content-type", "application/json")
        .with_timeout(config.timeout_secs)
        .with_body(body);

    if !config.api_key.trim().is_empty() {
        req = req.with_header("authorization", format!("Bearer {}", config.api_key.trim()));
    }

    let response = req.send()?;
    if response.status_code < 200 || response.status_code >= 300 {
        return Err(format_ai_http_error(
            response.status_code,
            &config.endpoint,
            response.as_str().ok(),
        )
        .into());
    }

    let text = response.as_str()?;
    extract_explanation(text)
        .or_else(|| Some(text.to_string()))
        .ok_or_else(|| "AI 响应为空".into())
}

#[allow(dead_code)]
pub fn analyze_problem_set(
    config: &AiConfig,
    title: &str,
    problems: &[Problem],
) -> Result<String, Box<dyn Error + Send + Sync>> {
    if problems.is_empty() {
        return Ok("没有可分析的题目。".into());
    }

    if !config.enabled {
        return Ok(local_problem_set_analysis(title, problems));
    }

    if config.endpoint.trim().is_empty() || config.model.trim().is_empty() {
        return Ok("AI 分析需要先启用并填写 endpoint/model。".into());
    }

    match analyze_problem_set_with_tools(config, title, problems) {
        Ok(report) => Ok(report),
        Err(err) => Ok(format!(
            "AI 请求失败，已切换为本地分析。\n\n失败原因：{}\n\n{}",
            err,
            local_problem_set_analysis(title, problems)
        )),
    }
}

#[allow(dead_code)]
pub fn call_problem_set_analysis_tool(
    config: &AiConfig,
    title: &str,
    problems: &[Problem],
) -> Result<AnalysisToolResult, Box<dyn Error + Send + Sync>> {
    let arguments = json!({
        "title": title,
        "problem_count": problems.len(),
        "data_access": "tool_paging",
        "available_tools": ["shuaforge.list_problems", "shuaforge.get_problem"],
        "analysis_dimensions": ["problem_type", "difficulty", "knowledge_points", "practice_order"]
    });

    Ok(AnalysisToolResult {
        tool_name: "shuaforge.analyze_problem_set".into(),
        arguments_json: serde_json::to_string_pretty(&arguments)?,
        result: analyze_problem_set(config, title, problems)?,
    })
}

pub fn stream_problem_set_analysis_tool(
    config: AiConfig,
    title: String,
    problems: Vec<Problem>,
    sender: mpsc::Sender<AnalysisStreamEvent>,
) {
    let arguments = json!({
        "title": title,
        "problem_count": problems.len(),
        "data_access": "tool_paging",
        "available_tools": ["shuaforge.list_problems", "shuaforge.get_problem"],
        "stream": true,
        "analysis_dimensions": ["problem_type", "difficulty", "knowledge_points", "practice_order"]
    });
    let _ = sender.send(AnalysisStreamEvent::ToolCall {
        arguments_json: serde_json::to_string_pretty(&arguments).unwrap_or_else(|_| "{}".into()),
    });

    if problems.is_empty() {
        let _ = sender.send(AnalysisStreamEvent::TextDelta("没有可分析的题目。".into()));
        let _ = sender.send(AnalysisStreamEvent::Finished);
        return;
    }

    if !config.enabled || config.endpoint.trim().is_empty() || config.model.trim().is_empty() {
        let _ = sender.send(AnalysisStreamEvent::TextDelta(local_problem_set_analysis(
            &title, &problems,
        )));
        let _ = sender.send(AnalysisStreamEvent::Finished);
        return;
    }

    if let Err(err) = analyze_problem_set_with_tools_streaming(&config, &title, &problems, &sender)
    {
        let fallback = format!(
            "AI 请求失败，已切换为本地分析。\n\n失败原因：{}\n\n{}",
            err,
            local_problem_set_analysis(&title, &problems)
        );
        let _ = sender.send(AnalysisStreamEvent::Failed(err.to_string()));
        let _ = sender.send(AnalysisStreamEvent::TextDelta(fallback));
        let _ = sender.send(AnalysisStreamEvent::Finished);
    }
}

pub fn analyze_learning_gaps(
    config: &AiConfig,
    title: &str,
    records: &[AnswerRecord],
) -> Result<String, Box<dyn Error + Send + Sync>> {
    if records.is_empty() {
        return Ok("还没有这个范围内的答题记录。先刷一轮，再分析薄弱点会更准。".into());
    }

    if !config.enabled {
        return Ok(local_learning_gap_analysis(title, records));
    }

    if config.endpoint.trim().is_empty() || config.model.trim().is_empty() {
        return Ok("AI 薄弱点分析需要先启用并填写 endpoint/model。".into());
    }

    send_prompt(config, build_learning_gap_prompt(title, records))
}

pub fn call_learning_gap_analysis_tool(
    config: &AiConfig,
    title: &str,
    records: &[AnswerRecord],
) -> Result<AnalysisToolResult, Box<dyn Error + Send + Sync>> {
    let arguments = json!({
        "title": title,
        "answer_record_count": records.len(),
        "record_limit": 120,
        "analysis_dimensions": ["accuracy", "weak_points", "mistake_patterns", "next_practice_plan"]
    });

    Ok(AnalysisToolResult {
        tool_name: "shuaforge.analyze_learning_gaps".into(),
        arguments_json: serde_json::to_string_pretty(&arguments)?,
        result: analyze_learning_gaps(config, title, records)?,
    })
}

pub fn continue_analysis_chat(
    config: &AiConfig,
    title: &str,
    analysis_result: &str,
    user_message: &str,
) -> Result<String, Box<dyn Error + Send + Sync>> {
    if user_message.trim().is_empty() {
        return Ok("请输入要追问的内容。".into());
    }

    if !config.enabled {
        return Ok(format!(
            "当前未启用 AI，无法继续追问。\n\n当前分析对象：{title}\n你的问题：{}\n\n可启用 AI 配置后继续对话。",
            user_message.trim()
        ));
    }

    if config.endpoint.trim().is_empty() || config.model.trim().is_empty() {
        return Ok("继续对话需要先启用并填写 endpoint/model。".into());
    }

    send_prompt(
        config,
        format!(
            "你是 ShuaForge 的刷题助手，不要自称老师。以下是已经生成的分析结果，请基于该结果回答用户追问。不要编造不存在的数据。\n\n分析对象：{}\n\n已有分析：\n{}\n\n用户追问：{}",
            title,
            analysis_result,
            user_message.trim()
        ),
    )
}

fn build_prompt(problem: &Problem, user_answer: &str) -> String {
    format!(
        "你是刷题助手。请用中文简洁解释这道题为什么答错，并给出记忆要点。\n\n题目：{}\n用户答案：{}\n标准答案：{}\n已有解析：{}",
        problem.prompt,
        user_answer,
        problem.answer,
        if problem.explanation.trim().is_empty() {
            "无"
        } else {
            &problem.explanation
        }
    )
}

fn build_review_prompt(problem: &Problem, user_answer: &str) -> String {
    format!(
        "你是严谨但鼓励型的刷题助手，不要自称老师。页面没有提供标准答案，题库中保存的 answer 只是上次作答记录，不一定正确。请根据题干、选项和用户本次答案进行批改。\n\n要求：\n1. 先给出结论：正确 / 部分正确 / 错误 / 无法判断。\n2. 简要说明理由。\n3. 如果能推断出更合理答案，请给出参考答案。\n4. 给出复习建议。\n\n题目：{}\n\n用户本次答案：{}\n\n历史作答记录（仅供参考，可能错误）：{}",
        problem.prompt,
        user_answer.trim(),
        problem.answer
    )
}

fn send_prompt(config: &AiConfig, prompt: String) -> Result<String, Box<dyn Error + Send + Sync>> {
    let body = build_ai_request_body(config, prompt)?;
    let mut req = minreq::post(&config.endpoint)
        .with_header("content-type", "application/json")
        .with_timeout(config.timeout_secs)
        .with_body(body);

    if !config.api_key.trim().is_empty() {
        req = req.with_header("authorization", format!("Bearer {}", config.api_key.trim()));
    }

    let response = req.send()?;
    if response.status_code < 200 || response.status_code >= 300 {
        return Err(format_ai_http_error(
            response.status_code,
            &config.endpoint,
            response.as_str().ok(),
        )
        .into());
    }

    let text = response.as_str()?;
    extract_explanation(text)
        .or_else(|| Some(text.to_string()))
        .ok_or_else(|| "AI 响应为空".into())
}

fn send_chat_messages(
    config: &AiConfig,
    messages: Vec<ChatCompletionMessage<'_>>,
) -> Result<String, Box<dyn Error + Send + Sync>> {
    let body = if uses_chat_completions(&config.endpoint) {
        serde_json::to_string(&ChatCompletionRequest {
            model: &config.model,
            messages,
            stream: false,
        })?
    } else {
        let prompt = messages
            .into_iter()
            .map(|message| format!("{}:\n{}", message.role, message.content))
            .collect::<Vec<_>>()
            .join("\n\n---\n\n");
        serde_json::to_string(&ExplainRequest {
            model: &config.model,
            prompt,
        })?
    };

    let mut req = minreq::post(&config.endpoint)
        .with_header("content-type", "application/json")
        .with_timeout(config.timeout_secs)
        .with_body(body);

    if !config.api_key.trim().is_empty() {
        req = req.with_header("authorization", format!("Bearer {}", config.api_key.trim()));
    }

    let response = req.send()?;
    if response.status_code < 200 || response.status_code >= 300 {
        return Err(format_ai_http_error(
            response.status_code,
            &config.endpoint,
            response.as_str().ok(),
        )
        .into());
    }

    let text = response.as_str()?;
    extract_explanation(text)
        .or_else(|| Some(text.to_string()))
        .ok_or_else(|| "AI 响应为空".into())
}

fn send_chat_messages_streaming(
    config: &AiConfig,
    messages: Vec<ChatCompletionMessage<'_>>,
    mut on_delta: impl FnMut(String),
) -> Result<String, Box<dyn Error + Send + Sync>> {
    if !uses_chat_completions(&config.endpoint) {
        let text = send_chat_messages(config, messages)?;
        on_delta(text.clone());
        return Ok(text);
    }

    let body = serde_json::to_string(&ChatCompletionRequest {
        model: &config.model,
        messages,
        stream: true,
    })?;
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(
            config.timeout_secs.max(30),
        )))
        .build()
        .new_agent();
    let mut request = agent
        .post(&config.endpoint)
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .send(body)?;

    let status = request.status();
    if !status.is_success() {
        let body = request.body_mut().read_to_string().ok();
        return Err(format_ai_http_error(
            status.as_u16() as i32,
            &config.endpoint,
            body.as_deref(),
        )
        .into());
    }

    let reader = BufReader::new(request.into_body().into_reader());
    let mut full = String::new();
    for line in reader.lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        let Some(data) = line.strip_prefix("data:").map(str::trim) else {
            continue;
        };
        if data == "[DONE]" {
            break;
        }
        if let Some(delta) = parse_sse_content_delta(data) {
            full.push_str(&delta);
            on_delta(delta);
        }
    }

    Ok(full)
}

fn build_ai_request_body(
    config: &AiConfig,
    prompt: String,
) -> Result<String, Box<dyn Error + Send + Sync>> {
    if uses_chat_completions(&config.endpoint) {
        let request = ChatCompletionRequest {
            model: &config.model,
            messages: vec![ChatCompletionMessage {
                role: "user",
                content: prompt,
            }],
            stream: false,
        };
        Ok(serde_json::to_string(&request)?)
    } else {
        let request = ExplainRequest {
            model: &config.model,
            prompt,
        };
        Ok(serde_json::to_string(&request)?)
    }
}

fn uses_chat_completions(endpoint: &str) -> bool {
    let endpoint = endpoint.trim().to_ascii_lowercase();
    endpoint.contains("/chat/completions") || endpoint.contains("api.deepseek.com")
}

fn format_ai_http_error(status_code: i32, endpoint: &str, response_body: Option<&str>) -> String {
    let reason = match status_code {
        400 => {
            "请求格式不被服务接受。若使用 DeepSeek/OpenAI chat/completions，需要 messages 格式；请确认 Endpoint 与模型接口类型匹配。"
        }
        401 | 403 => "认证失败，请检查 API Key 或服务权限。",
        404 => {
            "接口地址不存在。通常是 Endpoint 路径填错，例如少了服务要求的 /v1/completions、/v1/chat/completions 或自定义代理路径。"
        }
        429 => "请求过于频繁或额度不足，请稍后重试或检查配额。",
        500..=599 => "AI 服务端异常，请稍后重试或检查服务日志。",
        _ => "AI 服务返回了非成功状态码。",
    };
    let body = response_body
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| {
            let snippet: String = value.chars().take(240).collect();
            format!("\n响应片段：{snippet}")
        })
        .unwrap_or_default();

    format!(
        "AI 请求失败：HTTP {status_code}\n{reason}\n当前 Endpoint：{}{body}",
        endpoint.trim()
    )
}

#[allow(dead_code)]
fn analyze_problem_set_with_tools(
    config: &AiConfig,
    title: &str,
    problems: &[Problem],
) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut messages = vec![
        ChatCompletionMessage {
            role: "system",
            content: problem_set_analysis_system_prompt(),
        },
        ChatCompletionMessage {
            role: "user",
            content: build_problem_set_analysis_prompt(title, problems),
        },
    ];

    for _ in 0..8 {
        let response = send_chat_messages(config, clone_chat_messages(&messages))?;
        let Some(request) = parse_problem_set_tool_request(&response) else {
            return Ok(response);
        };

        let tool_result = execute_problem_set_tool(problems, &request)?;
        messages.push(ChatCompletionMessage {
            role: "assistant",
            content: response,
        });
        messages.push(ChatCompletionMessage {
            role: "system",
            content: tool_result,
        });
    }

    Ok("AI 已多次请求题目数据但未生成最终报告。请缩小题组范围或稍后重试。".into())
}

fn analyze_problem_set_with_tools_streaming(
    config: &AiConfig,
    title: &str,
    problems: &[Problem],
    sender: &mpsc::Sender<AnalysisStreamEvent>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut messages = vec![
        ChatCompletionMessage {
            role: "system",
            content: problem_set_analysis_system_prompt(),
        },
        ChatCompletionMessage {
            role: "user",
            content: build_problem_set_analysis_prompt(title, problems),
        },
    ];

    for _ in 0..8 {
        let response = send_chat_messages(config, clone_chat_messages(&messages))?;
        let Some(request) = parse_problem_set_tool_request(&response) else {
            let _ = sender.send(AnalysisStreamEvent::TextDelta(response));
            let _ = sender.send(AnalysisStreamEvent::Finished);
            return Ok(());
        };

        let tool_result = execute_problem_set_tool(problems, &request)?;
        messages.push(ChatCompletionMessage {
            role: "assistant",
            content: response,
        });
        messages.push(ChatCompletionMessage {
            role: "system",
            content: tool_result,
        });
    }

    let mut emitted = false;
    send_chat_messages_streaming(config, clone_chat_messages(&messages), |delta| {
        emitted = true;
        let _ = sender.send(AnalysisStreamEvent::TextDelta(delta));
    })?;
    if !emitted {
        let _ = sender.send(AnalysisStreamEvent::TextDelta(
            "AI 未生成最终报告，请稍后重试。".into(),
        ));
    }
    let _ = sender.send(AnalysisStreamEvent::Finished);
    Ok(())
}

fn clone_chat_messages(
    messages: &[ChatCompletionMessage<'_>],
) -> Vec<ChatCompletionMessage<'static>> {
    messages
        .iter()
        .map(|message| ChatCompletionMessage {
            role: match message.role {
                "user" => "user",
                "assistant" => "assistant",
                "system" => "system",
                _ => "user",
            },
            content: message.content.clone(),
        })
        .collect()
}

fn default_problem_batch_limit() -> usize {
    30
}

fn build_problem_set_analysis_prompt(title: &str, problems: &[Problem]) -> String {
    let stats = problem_set_stats(problems);

    format!(
        "题组/题库「{}」总览如下。请按系统消息中的工具规则获取完整数据后再分析。\n\n题库总览：\n{}",
        title, stats
    )
}

fn problem_set_analysis_system_prompt() -> String {
    "你是 ShuaForge 的刷题助手，不要自称老师。\n\n这是一个两阶段任务：\n1. 先读取题库数据。\n2. 只有在已通过工具读取完全部题目后，才能输出最终 Markdown 报告。\n\n强制规则：\n- 如果还没读完全部题目，不要给最终分析，不要总结，不要推断难度和易混点。\n- 每次只能请求一页题目，且 `limit` 不超过 50。\n- 当工具结果里的 `next_cursor` 不是 null 时，说明还有未读取题目，必须继续请求下一页。\n- 只有当 `next_cursor` 为 null 且已读取题目数等于题库总数时，才能输出最终报告。\n- 最终报告中的“数据读取范围”必须明确写出是否已读取全部题目。\n- 题型数量和标签频次只能使用题库总览中的全量统计。\n- 難度、易混点、刷题顺序必须基于全部已读取题目，而不是少量样本。\n- 不要因为第一页数据足够就提前结束；题库有多少题，就尽量把全部页都读完。\n\n工具协议：\n- 拉取题目列表：只回复 JSON {\"tool\":\"shuaforge.list_problems\",\"cursor\":0,\"limit\":50}\n- 拉取单题详情：只回复 JSON {\"tool\":\"shuaforge.get_problem\",\"id\":\"题目ID\"}\n- 程序会把工具结果作为 system 消息插回对话；你应读取 system 消息后继续下一步。\n- 如果需要下一页，请使用上一次返回的 next_cursor 作为 cursor。".to_owned()
}

fn parse_problem_set_tool_request(text: &str) -> Option<ProblemSetToolRequest> {
    json_candidates(text).into_iter().find_map(|candidate| {
        let request = serde_json::from_str::<ProblemSetToolRequest>(&candidate).ok()?;
        if matches!(
            request.tool.as_str(),
            "shuaforge.list_problems" | "shuaforge.get_problem"
        ) {
            Some(request)
        } else {
            None
        }
    })
}

fn json_candidates(text: &str) -> Vec<String> {
    let trimmed = text.trim();
    let mut candidates = vec![trimmed.to_owned()];
    if let Some(fenced) = trimmed
        .split("```")
        .find(|part| part.trim_start().starts_with("json"))
    {
        candidates.push(fenced.trim_start_matches("json").trim().to_owned());
    }
    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}'))
        && start < end
    {
        candidates.push(trimmed[start..=end].to_owned());
    }
    candidates
}

fn parse_sse_content_delta(data: &str) -> Option<String> {
    let parsed = serde_json::from_str::<StreamResponse>(data).ok()?;
    parsed.choices?.into_iter().find_map(|choice| {
        choice
            .delta
            .and_then(|delta| delta.content)
            .filter(|content| !content.is_empty())
    })
}

fn execute_problem_set_tool(
    problems: &[Problem],
    request: &ProblemSetToolRequest,
) -> Result<String, Box<dyn Error + Send + Sync>> {
    match request.tool.as_str() {
        "shuaforge.list_problems" => list_problem_batch(problems, request.cursor, request.limit),
        "shuaforge.get_problem" => get_problem_detail(problems, request.id.as_deref()),
        _ => Ok(json!({ "error": "unknown tool" }).to_string()),
    }
}

fn list_problem_batch(
    problems: &[Problem],
    cursor: usize,
    limit: usize,
) -> Result<String, Box<dyn Error + Send + Sync>> {
    let limit = limit.clamp(1, 50);
    let end = cursor.saturating_add(limit).min(problems.len());
    let batch = if cursor < problems.len() {
        &problems[cursor..end]
    } else {
        &[]
    };
    let response = ProblemBatchResponse {
        cursor,
        limit,
        total: problems.len(),
        read_count: batch.len(),
        remaining_count: problems.len().saturating_sub(end),
        read_fraction: if problems.is_empty() {
            0.0
        } else {
            end as f32 / problems.len() as f32
        },
        next_cursor: (end < problems.len()).then_some(end),
        problems: batch.iter().map(problem_preview).collect(),
    };
    Ok(serde_json::to_string_pretty(&response)?)
}

fn get_problem_detail(
    problems: &[Problem],
    id: Option<&str>,
) -> Result<String, Box<dyn Error + Send + Sync>> {
    let Some(id) = id else {
        return Ok(json!({ "error": "missing id" }).to_string());
    };
    let Some(problem) = problems.iter().find(|problem| problem.id == id) else {
        return Ok(json!({ "error": "problem not found", "id": id }).to_string());
    };
    Ok(serde_json::to_string_pretty(&problem_preview(problem))?)
}

fn problem_preview(problem: &Problem) -> ProblemPreview<'_> {
    ProblemPreview {
        id: &problem.id,
        problem_type: format!("{:?}", problem.kind()),
        tags: &problem.tags,
        prompt: &problem.prompt,
        answer: &problem.answer,
        explanation: &problem.explanation,
    }
}

fn problem_set_stats(problems: &[Problem]) -> String {
    let mut single = 0;
    let mut multiple = 0;
    let mut text = 0;
    let mut tags = std::collections::BTreeMap::<String, usize>::new();

    for problem in problems {
        match problem.kind() {
            crate::problem::ProblemType::SingleChoice => single += 1,
            crate::problem::ProblemType::MultipleChoice => multiple += 1,
            crate::problem::ProblemType::Text => text += 1,
        }
        for tag in &problem.tags {
            *tags.entry(tag.clone()).or_default() += 1;
        }
    }

    let mut tag_counts = tags.into_iter().collect::<Vec<_>>();
    tag_counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let top_tags = tag_counts
        .into_iter()
        .take(12)
        .map(|(tag, count)| format!("{tag}({count})"))
        .collect::<Vec<_>>()
        .join("、");

    format!(
        "- 题目总数：{}\n- 题型分布（全量精确）：单选 {}，多选 {}，文本/简答 {}\n- 高频标签（全量，最多 12 个）：{}",
        problems.len(),
        single,
        multiple,
        text,
        if top_tags.is_empty() {
            "暂无标签"
        } else {
            &top_tags
        }
    )
}

fn build_learning_gap_prompt(title: &str, records: &[AnswerRecord]) -> String {
    let history = records
        .iter()
        .take(120)
        .map(|record| {
            format!(
                "时间: {} | 题目ID: {} | 是否正确: {} | 用户答案: {} | 参考答案: {}",
                record.answered_at,
                record.problem_id,
                if record.is_correct {
                    "正确"
                } else {
                    "错误"
                },
                record.user_answer,
                record.correct_answer
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "你是 ShuaForge 的学习诊断助手，不要自称老师。请基于用户在「{}」中的答题记录分析需要提高的内容。\n\n要求：\n1. 给出正确率和整体表现判断。\n2. 根据错题/反复错误推断薄弱知识点或题型。\n3. 给出 3-5 条具体提高建议。\n4. 给出下一轮练习策略。\n5. 不要编造不存在的题干，只基于记录做推断。\n6. 使用 Markdown 输出，语气应像学习助手，不要说“老师已分析完”。\n\n答题记录数量：{}\n记录如下：\n{}",
        title,
        records.len(),
        history
    )
}

fn local_problem_set_analysis(title: &str, problems: &[Problem]) -> String {
    let mut single = 0;
    let mut multiple = 0;
    let mut text = 0;
    let mut tags = std::collections::BTreeMap::<String, usize>::new();
    for problem in problems {
        match problem.kind() {
            crate::problem::ProblemType::SingleChoice => single += 1,
            crate::problem::ProblemType::MultipleChoice => multiple += 1,
            crate::problem::ProblemType::Text => text += 1,
        }
        for tag in &problem.tags {
            *tags.entry(tag.clone()).or_default() += 1;
        }
    }
    let mut tag_counts = tags.into_iter().collect::<Vec<_>>();
    tag_counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let top_tags = tag_counts
        .into_iter()
        .take(8)
        .map(|(tag, count)| format!("{tag}（{count}）"))
        .collect::<Vec<_>>()
        .join("、");

    format!(
        "## 题组/题库分析：{title}\n\n### 概览\n\n- 题目数量：{}\n- 分析方式：本地统计，仅做确定性汇总，不推断难度比例。\n\n### 题型分布\n\n| 题型 | 数量 |\n| --- | ---: |\n| 单选 | {single} |\n| 多选 | {multiple} |\n| 文本/简答 | {text} |\n\n### 知识点\n\n{}\n\n### 局限性\n\n本地模式无法可靠判断难度和易混点；启用 AI 后会基于样例题给出观察，但仍会明确区分全量统计和样例推断。\n\n### 刷题建议\n\n先按高频知识点刷一轮，再集中复盘错题。",
        problems.len(),
        if top_tags.is_empty() {
            "暂无标签"
        } else {
            &top_tags
        }
    )
}

fn local_learning_gap_analysis(title: &str, records: &[AnswerRecord]) -> String {
    let total = records.len();
    let wrong = records.iter().filter(|record| !record.is_correct).count();
    let correct = total.saturating_sub(wrong);
    let rate = if total == 0 {
        0.0
    } else {
        correct as f32 * 100.0 / total as f32
    };
    let recent_wrong = records
        .iter()
        .filter(|record| !record.is_correct)
        .take(8)
        .map(|record| record.problem_id.clone())
        .collect::<Vec<_>>()
        .join("、");

    format!(
        "学习诊断：{title}\n\n最近记录：{total} 条\n正确：{correct}，错误：{wrong}，正确率：{rate:.1}%\n近期错题：{}\n\n建议：优先复盘近期错题。启用 AI 后，可进一步分析知识点分布和提升建议。",
        if recent_wrong.is_empty() {
            "暂无"
        } else {
            &recent_wrong
        }
    )
}

fn extract_explanation(text: &str) -> Option<String> {
    let parsed = serde_json::from_str::<ExplainResponse>(text).ok()?;

    if let Some(explanation) = parsed.explanation.filter(|v| !v.trim().is_empty()) {
        return Some(explanation);
    }

    parsed.choices.and_then(|choices| {
        choices.into_iter().find_map(|choice| {
            choice
                .message
                .and_then(|message| message.content)
                .or(choice.text)
                .filter(|value| !value.trim().is_empty())
        })
    })
}

fn local_explanation(problem: &Problem, user_answer: &str) -> String {
    let base = if problem.explanation.trim().is_empty() {
        "暂无本地解析。可在题库中补充 explanation 字段，或启用 AI 获取解析。".to_string()
    } else {
        problem.explanation.clone()
    };

    format!(
        "回答错误。\n\n你的答案：{}\n标准答案：{}\n\n{}",
        user_answer.trim(),
        problem.answer,
        base
    )
}

fn local_review(problem: &Problem, user_answer: &str) -> String {
    format!(
        "该题没有标准答案，建议启用 AI 批改。\n\n你的本次答案：{}\n历史作答记录：{}\n\n启用 AI 后，可根据题干判断作答情况并给出参考答案。",
        user_answer.trim(),
        problem.answer
    )
}

fn default_timeout_secs() -> u64 {
    30
}

#[cfg(test)]
mod tests {
    use super::{
        AiConfig, build_ai_request_body, build_problem_set_analysis_prompt, build_review_prompt,
        call_learning_gap_analysis_tool, call_problem_set_analysis_tool, continue_analysis_chat,
        execute_problem_set_tool, extract_explanation, local_learning_gap_analysis,
        local_problem_set_analysis, parse_problem_set_tool_request,
        problem_set_analysis_system_prompt, uses_chat_completions,
    };
    use crate::problem::Problem;
    use crate::store::AnswerRecord;

    #[test]
    fn extracts_openai_like_response() {
        let text = r#"{"choices":[{"message":{"content":"解析内容"}}]}"#;
        assert_eq!(extract_explanation(text).as_deref(), Some("解析内容"));
    }

    #[test]
    fn extracts_simple_response() {
        let text = r#"{"explanation":"简单解析"}"#;
        assert_eq!(extract_explanation(text).as_deref(), Some("简单解析"));
    }

    #[test]
    fn deepseek_endpoint_uses_chat_messages_body() {
        let config = AiConfig {
            enabled: true,
            endpoint: "https://api.deepseek.com/chat/completions".into(),
            api_key: String::new(),
            model: "deepseek-chat".into(),
            timeout_secs: 30,
        };

        assert!(uses_chat_completions(&config.endpoint));
        let body = build_ai_request_body(&config, "你好".into()).expect("body should serialize");
        assert!(body.contains("\"messages\""));
        assert!(body.contains("\"role\":\"user\""));
        assert!(!body.contains("\"prompt\""));
    }

    #[test]
    fn review_prompt_treats_saved_answer_as_reference_only() {
        let problem = Problem {
            id: "1".into(),
            prompt: "简答题".into(),
            answer: "历史答案".into(),
            explanation: String::new(),
            tags: vec![],
            problem_type: None,
            deck_name: None,
            deck_info: None,
            images: vec![],
        };

        let prompt = build_review_prompt(&problem, "本次答案");
        assert!(prompt.contains("不一定正确"));
        assert!(prompt.contains("用户本次答案：本次答案"));
        assert!(prompt.contains("不要自称老师"));
    }

    #[test]
    fn local_problem_set_analysis_summarizes_types_and_tags() {
        let problem = Problem {
            id: "1".into(),
            prompt: "单选题\nA. 对\nB. 错".into(),
            answer: "A".into(),
            explanation: String::new(),
            tags: vec!["统计学".into()],
            problem_type: Some(crate::problem::ProblemType::SingleChoice),
            deck_name: None,
            deck_info: None,
            images: vec![],
        };

        let report = local_problem_set_analysis("测试题库", &[problem]);
        assert!(report.contains("| 单选 | 1 |"));
        assert!(report.contains("统计学"));
        assert!(report.contains("不推断难度比例"));
    }

    #[test]
    fn problem_set_prompt_uses_tool_paging_instead_of_embedded_samples() {
        let problems = (0..35)
            .map(|index| Problem {
                id: format!("p{index}"),
                prompt: "统计学是什么？\nA. 数据分析\nB. 绘画".into(),
                answer: "A".into(),
                explanation: String::new(),
                tags: vec!["统计学".into()],
                problem_type: Some(crate::problem::ProblemType::SingleChoice),
                deck_name: None,
                deck_info: None,
                images: vec![],
            })
            .collect::<Vec<_>>();

        let prompt = build_problem_set_analysis_prompt("测试题组", &problems);
        assert!(prompt.contains("题组/题库「测试题组」总览如下"));
        assert!(prompt.contains("题库总览"));
        assert!(!prompt.contains("统计学是什么？"));
        assert!(!prompt.contains("以下是 30 道样例题"));

        let system_prompt = problem_set_analysis_system_prompt();
        assert!(system_prompt.contains("已通过工具读取完全部题目"));
        assert!(system_prompt.contains("next_cursor` 不是 null"));
        assert!(system_prompt.contains("每次只能请求一页题目"));
        assert!(system_prompt.contains("shuaforge.list_problems"));
        assert!(system_prompt.contains("shuaforge.get_problem"));
    }

    #[test]
    fn problem_set_tool_request_parses_and_lists_batches() {
        let request = parse_problem_set_tool_request(
            r#"```json
{"tool":"shuaforge.list_problems","cursor":1,"limit":2}
```"#,
        )
        .expect("tool request should parse");
        let problems = (0..4)
            .map(|index| Problem {
                id: format!("p{index}"),
                prompt: format!("题目 {index}\nA. 是\nB. 否"),
                answer: "A".into(),
                explanation: String::new(),
                tags: vec![],
                problem_type: Some(crate::problem::ProblemType::SingleChoice),
                deck_name: None,
                deck_info: None,
                images: vec![],
            })
            .collect::<Vec<_>>();

        let result = execute_problem_set_tool(&problems, &request).expect("tool result");
        assert!(result.contains("\"cursor\": 1"));
        assert!(result.contains("\"next_cursor\": 3"));
        assert!(result.contains("题目 1"));
        assert!(result.contains("题目 2"));
        assert!(!result.contains("题目 0"));
    }

    #[test]
    fn local_learning_gap_analysis_reports_accuracy() {
        let records = vec![
            AnswerRecord {
                answered_at: "now".into(),
                problem_id: "p1".into(),
                user_answer: "A".into(),
                correct_answer: "A".into(),
                is_correct: true,
            },
            AnswerRecord {
                answered_at: "now".into(),
                problem_id: "p2".into(),
                user_answer: "B".into(),
                correct_answer: "A".into(),
                is_correct: false,
            },
        ];

        let report = local_learning_gap_analysis("测试题库", &records);
        assert!(report.contains("正确率：50.0%"));
        assert!(report.contains("p2"));
    }

    #[test]
    fn problem_set_analysis_tool_returns_tool_call_metadata() {
        let problem = Problem {
            id: "1".into(),
            prompt: "题目\nA. 是\nB. 否".into(),
            answer: "A".into(),
            explanation: String::new(),
            tags: vec![],
            problem_type: Some(crate::problem::ProblemType::SingleChoice),
            deck_name: None,
            deck_info: None,
            images: vec![],
        };

        let result = call_problem_set_analysis_tool(&AiConfig::default(), "测试", &[problem])
            .expect("tool call should succeed in local mode");
        assert_eq!(result.tool_name, "shuaforge.analyze_problem_set");
        assert!(result.arguments_json.contains("problem_count"));
        assert!(result.result.contains("题目数量"));
    }

    #[test]
    fn learning_gap_tool_returns_tool_call_metadata() {
        let records = vec![AnswerRecord {
            answered_at: "now".into(),
            problem_id: "p1".into(),
            user_answer: "B".into(),
            correct_answer: "A".into(),
            is_correct: false,
        }];

        let result = call_learning_gap_analysis_tool(&AiConfig::default(), "测试", &records)
            .expect("tool call should succeed in local mode");
        assert_eq!(result.tool_name, "shuaforge.analyze_learning_gaps");
        assert!(result.arguments_json.contains("answer_record_count"));
        assert!(result.result.contains("正确率"));
    }

    #[test]
    fn continue_analysis_chat_guides_when_ai_disabled() {
        let reply =
            continue_analysis_chat(&AiConfig::default(), "测试", "已有分析", "下一步怎么练")
                .expect("local chat fallback should succeed");
        assert!(reply.contains("当前未启用 AI"));
        assert!(reply.contains("下一步怎么练"));
    }
}

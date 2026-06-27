use serde_json::Value;
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const LOG_DIR_NAME: &str = "logs";
const LOG_FILE_NAME: &str = "shuaforge.log";
const LOG_ARCHIVE_NAME: &str = "shuaforge-logs.zip";

pub fn init_app_logging() -> Option<PathBuf> {
    let log_dir = default_log_dir().ok()?;
    if let Err(err) = fs::create_dir_all(&log_dir) {
        eprintln!(
            "failed to create log directory {}: {err}",
            log_dir.display()
        );
        return None;
    }

    let log_path = log_dir.join(LOG_FILE_NAME);
    let file = match fern::log_file(&log_path) {
        Ok(file) => file,
        Err(err) => {
            eprintln!("failed to open log file {}: {err}", log_path.display());
            return None;
        }
    };

    let dispatch = fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "{} [{}] {}: {}",
                now_rfc3339(),
                record.level(),
                record.target(),
                message
            ))
        })
        .level(log::LevelFilter::Info)
        .level_for("wgpu", log::LevelFilter::Warn)
        .level_for("naga", log::LevelFilter::Warn)
        .chain(file);

    if let Err(err) = dispatch.apply() {
        eprintln!("failed to initialize logging: {err}");
        return None;
    }

    Some(log_path)
}

pub fn default_log_dir() -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    let base = dirs::data_local_dir()
        .or_else(|| std::env::current_dir().ok())
        .ok_or("无法确定本地数据目录")?;
    Ok(base.join("ShuaForge").join(LOG_DIR_NAME))
}

pub fn export_logs_to(path: &Path) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let log_dir = default_log_dir()?;
    if !log_dir.exists() {
        return Err("日志目录尚不存在".into());
    }

    let file = fs::File::create(path)?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    let mut exported = 0usize;
    for entry in fs::read_dir(&log_dir)? {
        let entry = entry?;
        let entry_path = entry.path();
        if !entry_path.is_file() {
            continue;
        }
        let Some(file_name) = entry_path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !file_name.ends_with(".log") {
            continue;
        }

        zip.start_file(file_name, options)?;
        let mut log_file = fs::File::open(&entry_path)?;
        let mut buffer = Vec::new();
        log_file.read_to_end(&mut buffer)?;
        zip.write_all(&buffer)?;
        exported += 1;
    }

    if exported == 0 {
        zip.start_file("README.txt", options)?;
        zip.write_all("ShuaForge 当前没有可导出的 .log 文件。".as_bytes())?;
    }

    zip.finish()?;
    Ok(())
}

pub fn default_export_file_name() -> &'static str {
    LOG_ARCHIVE_NAME
}

pub fn redact_url(url: &str) -> String {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let (before_fragment, had_fragment) = trimmed
        .split_once('#')
        .map(|(head, _)| (head, true))
        .unwrap_or((trimmed, false));
    let mut redacted = before_fragment
        .split_once('?')
        .map(|(head, _)| format!("{head}?<redacted>"))
        .unwrap_or_else(|| before_fragment.to_owned());
    if had_fragment {
        redacted.push_str("#<redacted>");
    }
    redacted
}

pub fn redact_text(text: &str) -> String {
    let mut redacted = text.to_owned();
    for marker in [
        "Bearer ", "bearer ", "api_key=", "apikey=", "key=", "token=",
    ] {
        redacted = redact_after_marker(&redacted, marker);
    }
    redacted
}

pub fn redact_json_text(text: &str, max_chars: usize) -> String {
    let redacted = serde_json::from_str::<Value>(text)
        .map(|mut value| {
            redact_json_value(&mut value);
            serde_json::to_string(&value).unwrap_or_else(|_| redact_text(text))
        })
        .unwrap_or_else(|_| redact_text(text));
    truncate_chars(&redacted, max_chars)
}

pub fn summarize_text(text: &str, max_chars: usize) -> String {
    truncate_chars(&redact_text(text), max_chars)
}

fn redact_after_marker(text: &str, marker: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find(marker) {
        let (before, after_before) = rest.split_at(index);
        result.push_str(before);
        result.push_str(marker);
        result.push_str("<redacted>");
        let value_start = marker.len();
        let after_marker = &after_before[value_start..];
        let value_end = after_marker
            .find(|ch: char| ch.is_whitespace() || matches!(ch, '&' | '"' | '\'' | ',' | '}'))
            .unwrap_or(after_marker.len());
        rest = &after_marker[value_end..];
    }
    result.push_str(rest);
    result
}

fn redact_json_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, value) in map.iter_mut() {
                if is_sensitive_key(key) {
                    *value = Value::String("<redacted>".into());
                } else {
                    redact_json_value(value);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                redact_json_value(item);
            }
        }
        Value::String(text) => {
            *text = redact_text(text);
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn is_sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    matches!(
        key.as_str(),
        "api_key"
            | "apikey"
            | "authorization"
            | "password"
            | "passwd"
            | "secret"
            | "token"
            | "access_token"
            | "refresh_token"
            | "client_secret"
    ) || key.ends_with("_secret")
        || key.ends_with("_token")
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}…<truncated>")
    } else {
        truncated
    }
}

fn now_rfc3339() -> String {
    let now = OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc());
    now.format(&Rfc3339)
        .unwrap_or_else(|_| "unknown-time".into())
}

#[cfg(test)]
mod tests {
    use super::{redact_json_text, redact_text, redact_url};

    #[test]
    fn redacts_sensitive_url_parts() {
        assert_eq!(
            redact_url("https://example.test/path?api_key=abc#token"),
            "https://example.test/path?<redacted>#<redacted>"
        );
    }

    #[test]
    fn redacts_bearer_tokens_and_json_secrets() {
        assert_eq!(
            redact_text("Authorization: Bearer secret123"),
            "Authorization: Bearer <redacted>"
        );
        let redacted = redact_json_text(r#"{"api_key":"secret","model":"demo"}"#, 200);
        assert!(redacted.contains("<redacted>"));
        assert!(!redacted.contains("secret"));
        assert!(redacted.contains("demo"));
    }
}

use crate::problem::Problem;
use base64::{Engine as _, engine::general_purpose};
use serde::Serialize;
use std::{
    error::Error,
    fs,
    io::Write,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, Serialize)]
pub struct ExportProblemBank {
    pub deck_name: String,
    pub deck_info: String,
    pub exported_at: String,
    pub problem_count: usize,
    pub problems: Vec<Problem>,
}

impl ExportProblemBank {
    pub fn new(
        deck_name: impl Into<String>,
        deck_info: impl Into<String>,
        problems: Vec<Problem>,
    ) -> Self {
        let deck_name = deck_name.into();
        let deck_info = deck_info.into();
        let problems = problems
            .into_iter()
            .map(|mut problem| {
                if problem
                    .deck_name
                    .as_deref()
                    .unwrap_or_default()
                    .trim()
                    .is_empty()
                {
                    problem.deck_name = Some(deck_name.clone());
                }
                if problem
                    .deck_info
                    .as_deref()
                    .unwrap_or_default()
                    .trim()
                    .is_empty()
                    && !deck_info.trim().is_empty()
                {
                    problem.deck_info = Some(deck_info.clone());
                }
                problem
            })
            .collect::<Vec<_>>();

        Self {
            deck_name,
            deck_info,
            exported_at: now_iso_like(),
            problem_count: problems.len(),
            problems,
        }
    }
}

pub fn export_problem_bank_json(
    path: &Path,
    bank: &ExportProblemBank,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(bank)?;
    fs::write(path, text)?;
    Ok(())
}

pub fn export_problem_bank_zip(
    path: &Path,
    bank: &ExportProblemBank,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let file = fs::File::create(path)?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    zip.start_file("problems.json", options)?;
    zip.write_all(serde_json::to_string_pretty(bank)?.as_bytes())?;

    let mut asset_count = 0usize;
    for problem in &bank.problems {
        for (index, image) in problem.images.iter().enumerate() {
            let base64 = image.base64.trim();
            if base64.is_empty() {
                continue;
            }
            let Ok(bytes) = general_purpose::STANDARD.decode(base64) else {
                continue;
            };
            let filename = safe_zip_segment(&image.filename);
            let problem_id = safe_zip_segment(&problem.id);
            zip.start_file(
                format!("assets/{problem_id}-{}-{filename}", index + 1),
                options,
            )?;
            zip.write_all(&bytes)?;
            asset_count += 1;
        }
    }

    zip.start_file("README.txt", options)?;
    zip.write_all(
        format!(
            "ShuaForge 题库导出\n\n名称：{}\n题目数：{}\n图片资源：{}\n导出时间：{}\n\n可在 ShuaForge 中直接导入本 ZIP。\n",
            bank.deck_name, bank.problem_count, asset_count, bank.exported_at
        )
        .as_bytes(),
    )?;
    zip.finish()?;
    Ok(())
}

pub fn default_export_file_name(name: &str, extension: &str) -> String {
    format!(
        "{}-{}.{}",
        safe_file_stem(name).unwrap_or_else(|| "shuaforge-export".into()),
        compact_timestamp(),
        extension.trim_start_matches('.')
    )
}

fn safe_zip_segment(value: &str) -> String {
    safe_file_stem(value).unwrap_or_else(|| "unnamed".into())
}

fn safe_file_stem(value: &str) -> Option<String> {
    let cleaned = value
        .chars()
        .map(|ch| match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' | '\0' => '_',
            ch if ch.is_control() => '_',
            ch => ch,
        })
        .collect::<String>()
        .trim()
        .trim_matches('.')
        .to_owned();
    (!cleaned.is_empty()).then_some(cleaned)
}

fn compact_timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".into())
}

fn now_iso_like() -> String {
    let secs = compact_timestamp();
    format!("unix:{secs}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::problem::{Problem, ProblemImage, ProblemState};

    fn problem(id: &str) -> Problem {
        Problem {
            id: id.into(),
            prompt: "题干".into(),
            answer: "A".into(),
            explanation: String::new(),
            tags: vec![],
            problem_type: None,
            deck_name: None,
            deck_info: None,
            images: vec![],
            state: ProblemState::default(),
        }
    }

    #[test]
    fn bank_backfills_deck_metadata() {
        let bank = ExportProblemBank::new("第一章", "导出测试", vec![problem("p1")]);

        assert_eq!(bank.problem_count, 1);
        assert_eq!(bank.problems[0].deck_name.as_deref(), Some("第一章"));
        assert_eq!(bank.problems[0].deck_info.as_deref(), Some("导出测试"));
    }

    #[test]
    fn zip_export_contains_problem_json_and_assets() {
        let mut problem = problem("p/1");
        problem.images.push(ProblemImage {
            filename: "a:b.png".into(),
            mime_type: "image/png".into(),
            base64: general_purpose::STANDARD.encode([0u8, 1, 2, 3]),
            alt_text: String::new(),
            source_url: String::new(),
        });
        let bank = ExportProblemBank::new("测试题库", "", vec![problem]);
        let path =
            std::env::temp_dir().join(format!("shuaforge-export-test-{}.zip", compact_timestamp()));

        export_problem_bank_zip(&path, &bank).expect("zip export should succeed");

        let file = fs::File::open(&path).expect("zip file exists");
        let mut archive = zip::ZipArchive::new(file).expect("valid zip");
        assert!(archive.by_name("problems.json").is_ok());
        assert!(archive.by_name("assets/p_1-1-a_b.png").is_ok());

        let _ = fs::remove_file(path);
    }
}

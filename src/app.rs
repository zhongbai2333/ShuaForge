use crate::{
    ai::{
        AiConfig, AnalysisStreamEvent, AnalysisToolResult,
        annotate_problem_knowledge_points_with_progress, call_learning_gap_analysis_tool,
        continue_analysis_chat, explain_wrong_answer, guide_solution_process,
        pre_generate_explanations_with_progress, review_pending_problems_with_progress,
        stream_problem_set_analysis_tool,
    },
    ai_import::AiImportResult,
    deck::{PracticeDeck, PracticeDeckSnapshot, PracticeOrder, SubmitResult},
    lan_sync::{self, SyncImportSummary},
    problem::{
        Problem, ProblemAnswerSource, ProblemType, load_problems, load_problems_from_json_text,
        load_problems_from_text, normalize_choice_answer, visible_tags,
    },
    problem_export::{
        ExportProblemBank, default_export_file_name, export_problem_bank_json,
        export_problem_bank_zip,
    },
    self_update::{self, UpdateInfo, UpdateOutcome},
    store::{AnswerRecord, AppStore, DeckCard, GroupCard},
};
#[cfg(not(target_os = "android"))]
use crate::{
    ai::{load_ai_config, save_ai_config},
    ai_import::import_problem_bank_with_ai,
    logging, userscript_server,
};
mod analysis_ui;
mod fonts;
mod layout;
mod library_cards;
mod markdown;
mod navigation;
mod practice_input;
mod preview;
mod settings_ui;
mod widgets;
use analysis_ui::{render_analysis_input_bar, render_analysis_progress_bar, render_chat_message};
use eframe::egui;
use fonts::configure_fonts;
use library_cards::{
    CARD_WIDTH, render_group_card, render_library_deck_card, render_mobile_group_card,
    render_mobile_library_deck_card, render_problem, render_trash_zone,
};
use markdown::{FormulaRenderSettings, apply_formula_render_settings, render_markdown_text};
use practice_input::{
    clear_egui_keyboard_focus, handle_practice_keyboard, render_answer_input,
    suppress_practice_tab_focus,
};
use preview::{ProblemPreviewAction, render_problem_preview_row};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use std::{
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
};
use widgets::{add_content_safe_area, format_bytes, grid_columns, render_update_progress_bar};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppTheme {
    Dark,
    Light,
}

impl AppTheme {
    fn visuals(self) -> egui::Visuals {
        match self {
            AppTheme::Dark => egui::Visuals::dark(),
            AppTheme::Light => egui::Visuals::light(),
        }
    }

    fn apply_to_context(self, ctx: &egui::Context) {
        let mut visuals = self.visuals();
        visuals.override_text_color = None;
        ctx.set_visuals(visuals);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppView {
    Library,
    Practice,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppUiMode {
    Desktop,
    Mobile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LibraryDeckViewMode {
    Cards,
    List,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AnalysisKind {
    ProblemSet,
    LearningGap,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct AnalysisCacheKey {
    kind: AnalysisKind,
    title: String,
}

#[derive(Debug, Clone)]
enum AnalysisSource {
    Problems(Vec<Problem>),
    Records(Vec<AnswerRecord>),
}

#[derive(Debug, Clone)]
struct AnalysisDialogState {
    title: String,
    kind: AnalysisKind,
    messages: Vec<ChatMessage>,
    input: String,
    latest_result: String,
    is_loading: bool,
    open: bool,
}

#[derive(Debug, Clone)]
struct DeckPreviewState {
    deck_id: i64,
    deck_name: String,
    problems: Vec<Problem>,
    open: bool,
    editing_problem_id: Option<String>,
    editing_answer: String,
}

#[derive(Debug, Default, Clone)]
struct ImportQualitySummary {
    total: usize,
    standard: usize,
    user_temporary: usize,
    score_inferred: usize,
    ai_reviewed: usize,
    manual_reviewed: usize,
    review_needed: usize,
    single_choice: usize,
    multiple_choice: usize,
    text: usize,
}

#[derive(Debug, Default)]
struct ProblemImportBatchSummary {
    imported_files: usize,
    imported: usize,
    inserted: usize,
    updated: usize,
    failures: Vec<String>,
    last_imported_path: Option<PathBuf>,
    quality: ImportQualitySummary,
}

impl ImportQualitySummary {
    fn from_problems(problems: &[Problem]) -> Self {
        let mut summary = Self::default();
        for problem in problems {
            summary.total += 1;
            match problem.state.answer_source {
                ProblemAnswerSource::Standard => summary.standard += 1,
                ProblemAnswerSource::UserTemporary => summary.user_temporary += 1,
                ProblemAnswerSource::ScoreInferred => summary.score_inferred += 1,
                ProblemAnswerSource::AiReviewed => summary.ai_reviewed += 1,
                ProblemAnswerSource::ManualReviewed => summary.manual_reviewed += 1,
            }
            if problem.needs_ai_review() {
                summary.review_needed += 1;
            }
            match problem.kind() {
                ProblemType::SingleChoice => summary.single_choice += 1,
                ProblemType::MultipleChoice => summary.multiple_choice += 1,
                ProblemType::Text => summary.text += 1,
            }
        }
        summary
    }

    fn add(&mut self, other: &Self) {
        self.total += other.total;
        self.standard += other.standard;
        self.user_temporary += other.user_temporary;
        self.score_inferred += other.score_inferred;
        self.ai_reviewed += other.ai_reviewed;
        self.manual_reviewed += other.manual_reviewed;
        self.review_needed += other.review_needed;
        self.single_choice += other.single_choice;
        self.multiple_choice += other.multiple_choice;
        self.text += other.text;
    }

    fn describe(&self) -> String {
        if self.total == 0 {
            return String::new();
        }
        let mut parts = vec![format!(
            "题型：单选 {}，多选 {}，文本 {}",
            self.single_choice, self.multiple_choice, self.text
        )];
        parts.push(format!(
            "答案来源：标准 {}，临时 {}，得分推断 {}，AI复核 {}，人工确认 {}",
            self.standard,
            self.user_temporary,
            self.score_inferred,
            self.ai_reviewed,
            self.manual_reviewed
        ));
        if self.review_needed > 0 {
            parts.push(format!("需 AI/人工复核 {}", self.review_needed));
        } else {
            parts.push("无需复核".into());
        }
        parts.join("；")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ChatRole {
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatMessage {
    role: ChatRole,
    title: String,
    content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PersistedAnalysisCache {
    entries: Vec<PersistedAnalysisDialog>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PersistedAnalysisText {
    text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedAnalysisDialog {
    kind: AnalysisKind,
    title: String,
    messages: Vec<ChatMessage>,
    latest_result: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedPracticeSession {
    key: String,
    active_deck_name: Option<String>,
    guided_problem_id: Option<String>,
    deck: PracticeDeckSnapshot,
}

#[derive(Debug, Clone)]
struct AnalysisAsyncResponse {
    generation_id: u64,
    result: AnalysisToolResult,
}

#[derive(Debug, Clone)]
struct AnalysisStreamResponse {
    generation_id: u64,
    event: AnalysisStreamEvent,
}

#[derive(Debug, Clone)]
struct ChatAsyncResponse {
    generation_id: u64,
    message: String,
}

struct AiImportAsyncResponse {
    path: PathBuf,
    result: Result<AiImportResult, String>,
}

#[derive(Debug, Clone)]
struct BrowserSnapshotImportMeta {
    source_path: PathBuf,
    director_title: String,
}

struct BrowserSnapshotImport {
    path: PathBuf,
    director_title: String,
    problems: Vec<Problem>,
    quality: ImportQualitySummary,
    imported: usize,
    inserted: usize,
    updated: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AnalysisProgressState {
    label: String,
    current: usize,
    total: usize,
}

impl AnalysisProgressState {
    fn new(label: impl Into<String>, current: usize, total: usize) -> Self {
        Self {
            label: label.into(),
            current,
            total,
        }
    }

    fn fraction(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.current as f32 / self.total as f32).clamp(0.0, 1.0)
        }
    }

    fn text(&self) -> String {
        if self.total == 0 {
            self.label.clone()
        } else {
            format!(
                "{} · {}/{}",
                self.label,
                self.current.min(self.total),
                self.total
            )
        }
    }
}

#[derive(Debug, Clone)]
enum UpdateAsyncResponse {
    CheckFinished(Result<Option<UpdateInfo>, String>),
    ApplyProgress { downloaded: u64, total: u64 },
    ApplyFinished(Result<UpdateOutcome, String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UpdateCheckSource {
    #[cfg(not(target_os = "android"))]
    Startup,
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PersistedUpdateSettings {
    ignored_tag: Option<String>,
}

impl AnalysisDialogState {
    fn loading(
        title: String,
        kind: AnalysisKind,
        tool_name: &'static str,
        arguments_json: String,
    ) -> Self {
        let kind_label = match kind {
            AnalysisKind::ProblemSet => {
                "请作为 AI 总监审查这组题目的采集质量、待复核项和学习价值。"
            }
            AnalysisKind::LearningGap => "请根据答题记录分析需要提升的内容。",
        };
        Self {
            title,
            kind,
            messages: vec![
                ChatMessage {
                    role: ChatRole::User,
                    title: "用户".into(),
                    content: kind_label.into(),
                },
                ChatMessage {
                    role: ChatRole::Tool,
                    title: format!("Tool Call · {tool_name}"),
                    content: arguments_json,
                },
                ChatMessage {
                    role: ChatRole::Assistant,
                    title: "助手".into(),
                    content: "AI 总监正在审查，请稍候...".into(),
                },
            ],
            input: String::new(),
            latest_result: String::new(),
            is_loading: true,
            open: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedSettings {
    #[serde(default)]
    theme: PersistedTheme,
    #[serde(default)]
    practice_order: PersistedPracticeOrder,
    #[serde(default)]
    ai_config: AiConfig,
    #[serde(default)]
    formula_render: FormulaRenderSettings,
    #[serde(default)]
    library_deck_view: PersistedLibraryDeckViewMode,
}

impl Default for PersistedSettings {
    fn default() -> Self {
        Self {
            theme: PersistedTheme::Dark,
            practice_order: PersistedPracticeOrder::Shuffled,
            ai_config: AiConfig::default(),
            formula_render: FormulaRenderSettings::default(),
            library_deck_view: PersistedLibraryDeckViewMode::Cards,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
enum PersistedLibraryDeckViewMode {
    #[default]
    Cards,
    List,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
enum PersistedTheme {
    #[default]
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
enum PersistedPracticeOrder {
    Sequential,
    #[default]
    Shuffled,
}

impl From<AppTheme> for PersistedTheme {
    fn from(value: AppTheme) -> Self {
        match value {
            AppTheme::Dark => PersistedTheme::Dark,
            AppTheme::Light => PersistedTheme::Light,
        }
    }
}

impl From<PersistedTheme> for AppTheme {
    fn from(value: PersistedTheme) -> Self {
        match value {
            PersistedTheme::Dark => AppTheme::Dark,
            PersistedTheme::Light => AppTheme::Light,
        }
    }
}

impl From<PracticeOrder> for PersistedPracticeOrder {
    fn from(value: PracticeOrder) -> Self {
        match value {
            PracticeOrder::Sequential => PersistedPracticeOrder::Sequential,
            PracticeOrder::Shuffled => PersistedPracticeOrder::Shuffled,
        }
    }
}

impl From<PersistedPracticeOrder> for PracticeOrder {
    fn from(value: PersistedPracticeOrder) -> Self {
        match value {
            PersistedPracticeOrder::Sequential => PracticeOrder::Sequential,
            PersistedPracticeOrder::Shuffled => PracticeOrder::Shuffled,
        }
    }
}

impl From<LibraryDeckViewMode> for PersistedLibraryDeckViewMode {
    fn from(value: LibraryDeckViewMode) -> Self {
        match value {
            LibraryDeckViewMode::Cards => PersistedLibraryDeckViewMode::Cards,
            LibraryDeckViewMode::List => PersistedLibraryDeckViewMode::List,
        }
    }
}

impl From<PersistedLibraryDeckViewMode> for LibraryDeckViewMode {
    fn from(value: PersistedLibraryDeckViewMode) -> Self {
        match value {
            PersistedLibraryDeckViewMode::Cards => LibraryDeckViewMode::Cards,
            PersistedLibraryDeckViewMode::List => LibraryDeckViewMode::List,
        }
    }
}

const SETTINGS_KEY: &str = "app_settings";
const UPDATE_SETTINGS_KEY: &str = "update_settings";
const ANALYSIS_CACHE_KEY: &str = "analysis_dialog_cache";
const ANALYSIS_TEXT_CACHE_KEY: &str = "analysis_text_cache";
const PRACTICE_SESSION_KEY: &str = "practice_session:latest";

const MOBILE_SIDE_SAFE: f32 = 16.0;
const MOBILE_TOP_SAFE: f32 = 36.0;
const MOBILE_BOTTOM_SAFE: f32 = 20.0;
const MOBILE_TOUCH_HEIGHT: f32 = 44.0;
const MOBILE_CONTENT_PAD: f32 = 12.0;
const DESKTOP_CONTENT_PAD: f32 = 20.0;
const DESKTOP_TOP_BAR_PAD_X: f32 = 14.0;
const DESKTOP_TOP_BAR_PAD_Y: f32 = 8.0;
const DESKTOP_BUTTON_HEIGHT: f32 = 32.0;
const DESKTOP_QUESTION_RATIO: f32 = 0.54;
const DESKTOP_ANALYSIS_MIN_WIDTH: f32 = 260.0;
const MOBILE_LIBRARY_CARD_TARGET_WIDTH: f32 = 220.0;
const MOBILE_LIBRARY_CARD_MIN_WIDTH: f32 = 120.0;
const MOBILE_LIBRARY_CARD_GAP: f32 = 8.0;
const DESKTOP_GAP: f32 = 20.0;

struct ThirdPartyLicense {
    name: &'static str,
    license: &'static str,
    url: &'static str,
}

const THIRD_PARTY_LICENSES: &[ThirdPartyLicense] = &[
    ThirdPartyLicense {
        name: "base64",
        license: "MIT OR Apache-2.0",
        url: "https://github.com/marshallpierce/rust-base64",
    },
    ThirdPartyLicense {
        name: "csv",
        license: "Unlicense/MIT",
        url: "https://github.com/BurntSushi/rust-csv",
    },
    ThirdPartyLicense {
        name: "dirs",
        license: "MIT OR Apache-2.0",
        url: "https://github.com/soc/dirs-rs",
    },
    ThirdPartyLicense {
        name: "eframe / egui",
        license: "MIT OR Apache-2.0",
        url: "https://github.com/emilk/egui",
    },
    ThirdPartyLicense {
        name: "hex",
        license: "MIT OR Apache-2.0",
        url: "https://github.com/KokaKiwi/rust-hex",
    },
    ThirdPartyLicense {
        name: "minreq",
        license: "ISC",
        url: "https://github.com/neonmoe/minreq",
    },
    ThirdPartyLicense {
        name: "rand",
        license: "MIT OR Apache-2.0",
        url: "https://github.com/rust-random/rand",
    },
    ThirdPartyLicense {
        name: "resvg",
        license: "MIT OR Apache-2.0",
        url: "https://github.com/linebender/resvg",
    },
    ThirdPartyLicense {
        name: "rfd",
        license: "MIT",
        url: "https://github.com/PolyMeilex/rfd",
    },
    ThirdPartyLicense {
        name: "rusqlite",
        license: "MIT",
        url: "https://github.com/rusqlite/rusqlite",
    },
    ThirdPartyLicense {
        name: "serde",
        license: "MIT OR Apache-2.0",
        url: "https://github.com/serde-rs/serde",
    },
    ThirdPartyLicense {
        name: "serde_json",
        license: "MIT OR Apache-2.0",
        url: "https://github.com/serde-rs/json",
    },
    ThirdPartyLicense {
        name: "sha2",
        license: "MIT OR Apache-2.0",
        url: "https://github.com/RustCrypto/hashes",
    },
    ThirdPartyLicense {
        name: "tempfile",
        license: "MIT OR Apache-2.0",
        url: "https://github.com/Stebalien/tempfile",
    },
    ThirdPartyLicense {
        name: "ureq",
        license: "MIT OR Apache-2.0",
        url: "https://github.com/algesten/ureq",
    },
    ThirdPartyLicense {
        name: "webbrowser",
        license: "MIT OR Apache-2.0",
        url: "https://github.com/amodm/webbrowser-rs",
    },
    ThirdPartyLicense {
        name: "zip",
        license: "MIT",
        url: "https://github.com/zip-rs/zip2.git",
    },
    ThirdPartyLicense {
        name: "TypeScript",
        license: "Apache-2.0",
        url: "https://github.com/microsoft/TypeScript",
    },
];

pub struct ShuaForgeApp {
    ui_mode: AppUiMode,
    deck: Option<PracticeDeck>,
    view: AppView,
    store: Option<AppStore>,
    ai_config: AiConfig,
    answer_input: String,
    selected_single: Option<String>,
    selected_multiple: BTreeSet<String>,
    focused_choice_index: usize,
    keyboard_choice_focus_visible: bool,
    clear_practice_focus_next_frame: bool,
    status: String,
    analysis: String,
    loaded_path: Option<PathBuf>,
    ai_config_path: Option<PathBuf>,
    ai_receiver: Option<mpsc::Receiver<String>>,
    analysis_receiver: Option<mpsc::Receiver<AnalysisAsyncResponse>>,
    analysis_stream_receiver: Option<mpsc::Receiver<AnalysisStreamResponse>>,
    chat_receiver: Option<mpsc::Receiver<ChatAsyncResponse>>,
    ai_import_receiver: Option<mpsc::Receiver<Vec<AiImportAsyncResponse>>>,
    browser_snapshot_receiver: Option<mpsc::Receiver<String>>,
    sync_receiver: Option<mpsc::Receiver<Result<SyncImportSummary, String>>>,
    sync_server_addr: Option<String>,
    sync_status: String,
    update_receiver: Option<mpsc::Receiver<UpdateAsyncResponse>>,
    update_info: Option<UpdateInfo>,
    update_status: String,
    update_downloaded_bytes: u64,
    update_total_bytes: u64,
    is_update_checking: bool,
    is_update_applying: bool,
    update_check_source: Option<UpdateCheckSource>,
    show_update_prompt: bool,
    ignored_update_tag: Option<String>,
    analysis_generation_id: u64,
    analysis_progress: Option<AnalysisProgressState>,
    analysis_dialog: Option<AnalysisDialogState>,
    deck_preview: Option<DeckPreviewState>,
    analysis_dialogs: HashMap<AnalysisCacheKey, AnalysisDialogState>,
    active_analysis_key: Option<AnalysisCacheKey>,
    analysis_cache: HashMap<AnalysisCacheKey, AnalysisDialogState>,
    analysis_sources: HashMap<AnalysisCacheKey, AnalysisSource>,
    is_ai_loading: bool,
    is_ai_importing: bool,
    formula_render_settings: FormulaRenderSettings,
    theme: AppTheme,
    practice_order: PracticeOrder,
    library_deck_view: LibraryDeckViewMode,
    deck_cards: Vec<DeckCard>,
    group_cards: Vec<GroupCard>,
    active_deck_name: Option<String>,
    answer_history: Vec<AnswerRecord>,
    bank_count: usize,
    show_about: bool,
    show_text_import_dialog: bool,
    text_import_buffer: String,
    new_group_name: String,
    selected_deck_ids: BTreeSet<i64>,
    dragging_deck_id: Option<i64>,
    dragging_group_id: Option<i64>,
    guided_problem_id: Option<String>,
    log_path: Option<PathBuf>,
}

impl ShuaForgeApp {
    pub fn new(cc: &eframe::CreationContext<'_>, log_path: Option<PathBuf>) -> Self {
        Self::new_with_ui_mode(cc, log_path, AppUiMode::Desktop)
    }

    pub fn new_mobile(cc: &eframe::CreationContext<'_>) -> Self {
        Self::new_with_ui_mode(cc, None, AppUiMode::Mobile)
    }

    fn new_with_ui_mode(
        cc: &eframe::CreationContext<'_>,
        log_path: Option<PathBuf>,
        ui_mode: AppUiMode,
    ) -> Self {
        configure_fonts(&cc.egui_ctx, MOBILE_TOUCH_HEIGHT);
        log::info!("Initializing ShuaForge application state");

        let mut status = "请导入题库开始练习。支持 JSON / CSV / ZIP / 调试快照格式。".to_owned();
        let store = match AppStore::open_default() {
            Ok(store) => {
                log::info!("Opened local SQLite store");
                Some(store)
            }
            Err(err) => {
                log::error!("Failed to open local SQLite store: {err}");
                status = format!("本地数据库打开失败：{err}");
                None
            }
        };

        let settings = store
            .as_ref()
            .and_then(|store| load_persisted_settings(store).ok())
            .unwrap_or_default();
        let theme = AppTheme::from(settings.theme);
        let practice_order = PracticeOrder::from(settings.practice_order);
        let library_deck_view = LibraryDeckViewMode::from(settings.library_deck_view);
        apply_formula_render_settings(settings.formula_render.clone());
        let update_settings = store
            .as_ref()
            .and_then(|store| load_persisted_update_settings(store).ok())
            .unwrap_or_default();
        theme.apply_to_context(&cc.egui_ctx);

        let mut app = Self {
            ui_mode,
            deck: None,
            view: AppView::Library,
            store,
            ai_config: settings.ai_config,
            answer_input: String::new(),
            selected_single: None,
            selected_multiple: BTreeSet::new(),
            focused_choice_index: 0,
            keyboard_choice_focus_visible: false,
            clear_practice_focus_next_frame: false,
            status,
            analysis: "解析与学习分析结果将在这里显示。".into(),
            loaded_path: None,
            ai_config_path: None,
            ai_receiver: None,
            analysis_receiver: None,
            analysis_stream_receiver: None,
            chat_receiver: None,
            ai_import_receiver: None,
            browser_snapshot_receiver: None,
            sync_receiver: None,
            sync_server_addr: None,
            sync_status: "尚未启动局域网同步服务。".into(),
            update_receiver: None,
            update_info: None,
            update_status: "尚未检查更新。".into(),
            update_downloaded_bytes: 0,
            update_total_bytes: 0,
            is_update_checking: false,
            is_update_applying: false,
            update_check_source: None,
            show_update_prompt: false,
            ignored_update_tag: update_settings.ignored_tag,
            analysis_generation_id: 0,
            analysis_progress: None,
            analysis_dialog: None,
            deck_preview: None,
            analysis_dialogs: HashMap::new(),
            active_analysis_key: None,
            analysis_cache: HashMap::new(),
            analysis_sources: HashMap::new(),
            is_ai_loading: false,
            is_ai_importing: false,
            formula_render_settings: settings.formula_render,
            theme,
            practice_order,
            library_deck_view,
            deck_cards: Vec::new(),
            group_cards: Vec::new(),
            active_deck_name: None,
            answer_history: Vec::new(),
            bank_count: 0,
            show_about: false,
            show_text_import_dialog: false,
            text_import_buffer: String::new(),
            new_group_name: "新题组".into(),
            selected_deck_ids: BTreeSet::new(),
            dragging_deck_id: None,
            dragging_group_id: None,
            guided_problem_id: None,
            log_path,
        };
        app.load_analysis_cache();
        app.load_analysis_text_cache();
        app.refresh_store_state();
        #[cfg(not(target_os = "android"))]
        app.start_browser_bridge_on_startup();
        #[cfg(not(target_os = "android"))]
        app.start_update_check(UpdateCheckSource::Startup);
        app
    }

    #[cfg(not(target_os = "android"))]
    fn start_browser_bridge_on_startup(&mut self) {
        match userscript_server::ensure_bridge_running() {
            Ok(url) => {
                log::info!("Browser collection bridge auto-started: {url}");
            }
            Err(err) => {
                log::warn!("Failed to auto-start browser collection bridge: {err}");
                if self.status.starts_with("请导入题库") {
                    self.status = format!("浏览器采集服务自启动失败：{err}；仍可导入本地题库。");
                }
            }
        }
    }

    fn toggle_theme(&mut self, ctx: &egui::Context) {
        self.theme = match self.theme {
            AppTheme::Dark => AppTheme::Light,
            AppTheme::Light => AppTheme::Dark,
        };
        self.theme.apply_to_context(ctx);
        self.persist_settings();
    }

    fn current_settings(&self) -> PersistedSettings {
        PersistedSettings {
            theme: self.theme.into(),
            practice_order: self.practice_order.into(),
            ai_config: self.ai_config.clone(),
            formula_render: self.formula_render_settings.clone(),
            library_deck_view: self.library_deck_view.into(),
        }
    }

    fn persist_settings(&mut self) {
        let Some(store) = &self.store else { return };
        let Ok(value) = serde_json::to_string_pretty(&self.current_settings()) else {
            self.status = "设置序列化失败。".into();
            return;
        };
        if let Err(err) = store.set_setting(SETTINGS_KEY, &value) {
            self.status = format!("设置保存失败：{err}");
        }
        apply_formula_render_settings(self.formula_render_settings.clone());
    }

    fn theme_button_icon(&self) -> &'static str {
        match self.theme {
            AppTheme::Dark => "☀",
            AppTheme::Light => "🌙",
        }
    }

    fn theme_button_tooltip(&self) -> &'static str {
        match self.theme {
            AppTheme::Dark => "切换到浅色主题",
            AppTheme::Light => "切换到深色主题",
        }
    }

    fn open_url(&mut self, url: &str) {
        if let Err(err) = webbrowser::open(url) {
            log::warn!("Failed to open URL {url}: {err}");
            self.status = format!("打开链接失败：{err}");
        }
    }

    #[cfg(target_os = "android")]
    fn export_logs(&mut self) {
        self.status = "Android 端日志导出稍后接入系统分享/文件选择。".into();
    }

    #[cfg(not(target_os = "android"))]
    fn export_logs(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("日志压缩包", &["zip"])
            .set_file_name(logging::default_export_file_name())
            .save_file()
        else {
            return;
        };

        match logging::export_logs_to(&path) {
            Ok(()) => {
                log::info!("Exported logs to {}", path.display());
                self.status = format!("日志已导出到：{}", path.display());
            }
            Err(err) => {
                log::error!("Failed to export logs to {}: {err}", path.display());
                self.status = format!("日志导出失败：{err}");
            }
        }
    }

    fn start_update_check(&mut self, source: UpdateCheckSource) {
        if self.is_update_checking || self.is_update_applying {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        self.update_receiver = Some(receiver);
        self.is_update_checking = true;
        self.update_check_source = Some(source);
        self.update_status = "正在检查更新...".into();
        thread::spawn(move || {
            let result = self_update::check_latest_version().map_err(|err| err.to_string());
            let _ = sender.send(UpdateAsyncResponse::CheckFinished(result));
        });
    }

    fn start_apply_update(&mut self) {
        if self.is_update_applying {
            return;
        }
        self.show_update_prompt = false;
        self.is_update_applying = true;
        self.update_downloaded_bytes = 0;
        self.update_total_bytes = self.update_info.as_ref().map_or(0, |info| info.size);
        self.update_status = "正在下载并应用更新...".into();
        let (sender, receiver) = mpsc::channel();
        self.update_receiver = Some(receiver);
        thread::spawn(move || {
            let progress_sender = sender.clone();
            let on_progress: self_update::ProgressCallback = Box::new(move |downloaded, total| {
                let _ =
                    progress_sender.send(UpdateAsyncResponse::ApplyProgress { downloaded, total });
            });
            let result =
                self_update::perform_update(Some(on_progress)).map_err(|err| err.to_string());
            let _ = sender.send(UpdateAsyncResponse::ApplyFinished(result));
        });
    }

    fn ignore_current_update(&mut self) {
        let Some(info) = &self.update_info else {
            self.show_update_prompt = false;
            return;
        };
        self.ignored_update_tag = Some(info.tag_name.clone());
        self.show_update_prompt = false;
        self.update_status = format!("已忽略版本 {}。", info.tag_name);
        self.persist_update_settings();
    }

    fn persist_update_settings(&mut self) {
        let Some(store) = &self.store else { return };
        let Ok(value) = serde_json::to_string_pretty(&PersistedUpdateSettings {
            ignored_tag: self.ignored_update_tag.clone(),
        }) else {
            self.status = "更新设置序列化失败。".into();
            return;
        };
        if let Err(err) = store.set_setting(UPDATE_SETTINGS_KEY, &value) {
            self.status = format!("更新设置保存失败：{err}");
        }
    }

    fn poll_update(&mut self) {
        let Some(receiver) = &self.update_receiver else {
            return;
        };
        let mut responses = Vec::new();
        while let Ok(response) = receiver.try_recv() {
            responses.push(response);
        }
        if responses.is_empty() {
            return;
        }
        let mut clear_receiver = false;
        for response in responses {
            match response {
                UpdateAsyncResponse::CheckFinished(result) => {
                    clear_receiver = true;
                    self.is_update_checking = false;
                    let source = self
                        .update_check_source
                        .take()
                        .unwrap_or(UpdateCheckSource::Manual);
                    match result {
                        Ok(Some(info)) => {
                            let ignored =
                                self.ignored_update_tag.as_deref() == Some(&info.tag_name);
                            self.update_status = format!(
                                "发现新版本 {}（当前 {}）。",
                                info.tag_name,
                                self_update::current_tag()
                            );
                            self.update_info = Some(info);
                            self.show_update_prompt =
                                !ignored || source == UpdateCheckSource::Manual;
                        }
                        Ok(None) => {
                            self.update_info = None;
                            self.show_update_prompt = false;
                            self.update_status =
                                format!("已是最新版本 {}。", self_update::current_tag());
                        }
                        Err(err) => {
                            self.update_status = format!("检查更新失败：{err}");
                            if source == UpdateCheckSource::Manual {
                                self.status = self.update_status.clone();
                            }
                        }
                    }
                }
                UpdateAsyncResponse::ApplyProgress { downloaded, total } => {
                    self.update_downloaded_bytes = downloaded;
                    if total > 0 {
                        self.update_total_bytes = total;
                    }
                    self.update_status = if self.update_total_bytes > 0 {
                        format!(
                            "正在下载更新... {} / {}",
                            format_bytes(self.update_downloaded_bytes),
                            format_bytes(self.update_total_bytes)
                        )
                    } else {
                        format!(
                            "正在下载更新... {}",
                            format_bytes(self.update_downloaded_bytes)
                        )
                    };
                }
                UpdateAsyncResponse::ApplyFinished(result) => {
                    clear_receiver = true;
                    self.is_update_applying = false;
                    match result {
                        Ok(UpdateOutcome::UpToDate) => {
                            self.update_status = "已是最新版本。".into();
                        }
                        Ok(UpdateOutcome::Skipped) => {
                            self.update_status =
                                "开发构建或当前环境不适合自动替换，已跳过更新。".into();
                        }
                        Ok(UpdateOutcome::UpdateLaunched) => {
                            self.update_status = "更新流程已启动，请按提示完成。".into();
                        }
                        Err(err) => {
                            self.update_status = format!("更新失败：{err}");
                        }
                    }
                    self.status = self.update_status.clone();
                }
            }
        }
        if clear_receiver {
            self.update_receiver = None;
        }
    }

    fn start_lan_sync_server(&mut self) {
        match lan_sync::ensure_sync_server_running() {
            Ok(addr) => {
                self.sync_server_addr = Some(addr.clone());
                self.sync_status = format!("同步服务已启动：{addr}");
                self.status = "局域网同步服务已启动，其他设备稍等几秒即可发现。".into();
            }
            Err(err) => {
                self.sync_status = err.clone();
                self.status = err;
            }
        }
    }

    fn start_lan_sync_import(&mut self, addr: String, device_name: String) {
        if self.sync_receiver.is_some() {
            self.status = "正在同步中，请稍候。".into();
            return;
        }
        let (sender, receiver) = mpsc::channel();
        self.sync_receiver = Some(receiver);
        self.sync_status = format!("正在从 {device_name} 拉取题库...");
        self.status = self.sync_status.clone();
        thread::spawn(move || {
            let result = lan_sync::fetch_and_import_from_peer(&addr);
            let _ = sender.send(result);
        });
    }

    fn poll_lan_sync(&mut self) {
        let Some(receiver) = &self.sync_receiver else {
            return;
        };
        let Ok(result) = receiver.try_recv() else {
            return;
        };
        self.sync_receiver = None;
        match result {
            Ok(summary) => {
                self.refresh_store_state();
                self.sync_status = format!(
                    "同步完成：{} 个题库，{} 道题（新增 {}，更新 {}）。",
                    summary.decks, summary.imported, summary.inserted, summary.updated
                );
                self.status = self.sync_status.clone();
            }
            Err(err) => {
                self.sync_status = format!("同步失败：{err}");
                self.status = self.sync_status.clone();
            }
        }
    }

    #[cfg(target_os = "android")]
    fn import_problem_bank(&mut self) {
        match crate::mobile::android::request_problem_bank_file_picker() {
            Ok(()) => {
                self.status = "已打开系统文件选择器，请选择 JSON / CSV / ZIP 题库文件。".into();
            }
            Err(err) => {
                self.show_text_import_dialog = true;
                self.status = format!("{err} 可改用粘贴导入。 ");
            }
        }
    }

    #[cfg(not(target_os = "android"))]
    fn import_problem_bank(&mut self) {
        if let Some(paths) = rfd::FileDialog::new()
            .add_filter("题库 / 调试快照", &["json", "csv", "zip"])
            .pick_files()
        {
            if paths.is_empty() {
                return;
            }
            self.import_problem_bank_files(paths);
        }
    }

    #[cfg(target_os = "android")]
    fn request_browser_snapshot_import(&mut self) {
        self.status = "Android 端不支持油猴浏览器采集，请从桌面同步或导入题库文件。".into();
    }

    #[cfg(not(target_os = "android"))]
    fn request_browser_snapshot_import(&mut self) {
        if self.browser_snapshot_receiver.is_some() {
            self.status = "已向所有已连接的浏览器页面请求快照，请保持题库页面打开。".into();
            return;
        }
        match userscript_server::request_page_snapshot() {
            Ok(receiver) => {
                self.browser_snapshot_receiver = Some(receiver);
                self.status = "已向所有自动连接的浏览器页面请求题库快照，收到后会逐个导入。".into();
            }
            Err(err) => {
                self.status = format!("请求浏览器快照失败：{err}");
            }
        }
    }

    fn poll_browser_snapshot_import(&mut self) {
        let Some(receiver) = &self.browser_snapshot_receiver else {
            return;
        };
        let mut snapshots = Vec::new();
        let mut finished = false;
        loop {
            match receiver.try_recv() {
                Ok(snapshot_json) => snapshots.push(snapshot_json),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    finished = true;
                    break;
                }
            }
        }
        if snapshots.is_empty() {
            if finished {
                self.browser_snapshot_receiver = None;
            }
            return;
        }

        let mut imported_snapshots = 0usize;
        let mut imported = 0usize;
        let mut inserted = 0usize;
        let mut updated = 0usize;
        let mut failures = Vec::new();
        let mut last_imported_path = None;
        let mut quality = ImportQualitySummary::default();
        let mut review_requests = Vec::new();

        for snapshot_json in snapshots {
            match self.import_browser_snapshot_json(&snapshot_json) {
                Ok(Some(imported_snapshot)) => {
                    imported_snapshots += 1;
                    imported += imported_snapshot.imported;
                    inserted += imported_snapshot.inserted;
                    updated += imported_snapshot.updated;
                    quality.add(&imported_snapshot.quality);
                    last_imported_path = Some(imported_snapshot.path);
                    if imported_snapshot.imported > 0 {
                        review_requests
                            .push((imported_snapshot.director_title, imported_snapshot.problems));
                    }
                }
                Ok(None) => {}
                Err(err) => failures.push(err),
            }
        }

        if imported_snapshots > 0 || !failures.is_empty() {
            self.finish_problem_import_batch(ProblemImportBatchSummary {
                imported_files: imported_snapshots,
                imported,
                inserted,
                updated,
                failures,
                last_imported_path,
                quality,
            });
            for (title, problems) in review_requests {
                self.request_ai_director_review(title, problems);
            }
            if finished {
                self.browser_snapshot_receiver = None;
            } else if self.browser_snapshot_receiver.is_some() {
                self.status = format!(
                    "已导入 {imported_snapshots} 个浏览器页面快照；继续等待其他已连接页面回传。"
                );
            }
        }
    }

    fn import_browser_snapshot_json(
        &mut self,
        snapshot_json: &str,
    ) -> Result<Option<BrowserSnapshotImport>, String> {
        match load_problems_from_json_text(snapshot_json) {
            Ok(problems) => {
                if problems.is_empty() {
                    return Ok(None);
                }
                let quality = ImportQualitySummary::from_problems(&problems);
                let import_meta = browser_snapshot_import_meta(snapshot_json, &problems);
                let virtual_path = import_meta.source_path.clone();
                match self.save_imported_problems(&virtual_path, &problems) {
                    Ok(summary) => Ok(Some(BrowserSnapshotImport {
                        path: virtual_path,
                        director_title: import_meta.director_title,
                        problems,
                        quality,
                        imported: summary.imported,
                        inserted: summary.inserted,
                        updated: summary.updated,
                    })),
                    Err(err) => Err(format!("浏览器快照保存失败：{err}")),
                }
            }
            Err(err) => Err(format!("浏览器快照解析失败：{err}")),
        }
    }

    fn request_ai_director_review(&mut self, title: String, problems: Vec<Problem>) {
        if problems.is_empty() {
            return;
        }
        let review_count = problems
            .iter()
            .filter(|problem| problem.needs_ai_review())
            .count();
        let title = format!("AI总监审查 · {title}");
        self.clear_ai_analysis_records_for_titles(std::slice::from_ref(&title));
        self.request_problem_set_analysis(title.clone(), problems);
        self.status = if review_count > 0 {
            format!("已导入浏览器快照，并交给 AI 总监优先复核 {review_count} 道待确认题。")
        } else {
            "已导入浏览器快照，并交给 AI 总监做一致性审查。".into()
        };
    }

    fn import_problem_bank_files(&mut self, paths: Vec<PathBuf>) {
        let mut imported_files = 0usize;
        let mut imported = 0usize;
        let mut inserted = 0usize;
        let mut updated = 0usize;
        let mut failures = Vec::new();
        let mut last_imported_path = None;
        let mut quality = ImportQualitySummary::default();

        for path in paths {
            log::info!("Importing problem bank from {}", path.display());
            match load_problems(&path) {
                Ok(problems) => match self.save_imported_problems(&path, &problems) {
                    Ok(summary) => {
                        quality.add(&ImportQualitySummary::from_problems(&problems));
                        imported_files += 1;
                        imported += summary.imported;
                        inserted += summary.inserted;
                        updated += summary.updated;
                        last_imported_path = Some(path);
                    }
                    Err(err) => {
                        log::error!("Saving imported problems to SQLite failed: {err}");
                        failures.push(format!("{}：保存失败：{err}", path.display()));
                    }
                },
                Err(err) => {
                    log::error!("Problem bank import failed: {err}");
                    failures.push(format!("{}：{err}", path.display()));
                }
            }
        }

        self.finish_problem_import_batch(ProblemImportBatchSummary {
            imported_files,
            imported,
            inserted,
            updated,
            failures,
            last_imported_path,
            quality,
        });
    }

    #[cfg(target_os = "android")]
    fn poll_android_file_picker_import(&mut self) {
        while let Some(result) = crate::mobile::android::poll_problem_bank_file_picker_result() {
            match result {
                Ok(path) => self.import_problem_bank_files(vec![path]),
                Err(err) => {
                    self.status = err;
                }
            }
        }
    }

    fn import_problem_bank_with_ai_dialog(&mut self) {
        if self.is_ai_importing {
            self.status = "AI 正在导入题库，请稍候。".into();
            return;
        }
        #[cfg(target_os = "android")]
        {
            self.status = "Android 端 AI 导入入口已保留，下一步接入文本粘贴/系统文件选择。".into();
            return;
        }
        #[cfg(not(target_os = "android"))]
        if let Some(paths) = rfd::FileDialog::new()
            .add_filter("AI 可解析题库", &["csv", "xlsx", "pdf"])
            .pick_files()
        {
            if paths.is_empty() {
                return;
            }
            let config = self.ai_config.clone();
            let (sender, receiver) = mpsc::channel();
            self.ai_import_receiver = Some(receiver);
            self.is_ai_importing = true;
            self.status = if paths.len() == 1 {
                format!("AI 正在解析并导入题库：{}", paths[0].display())
            } else {
                format!("AI 正在批量解析并导入 {} 个题库文件。", paths.len())
            };
            thread::spawn(move || {
                let responses = paths
                    .into_iter()
                    .map(|path| {
                        let result = import_problem_bank_with_ai(&config, &path)
                            .map_err(|err| err.to_string());
                        AiImportAsyncResponse { path, result }
                    })
                    .collect();
                let _ = sender.send(responses);
            });
        }
    }

    fn save_imported_problems(
        &mut self,
        path: &Path,
        problems: &[Problem],
    ) -> Result<crate::store::ImportSummary, Box<dyn std::error::Error + Send + Sync>> {
        let (summary, deck_name) = {
            let Some(store) = self.store.as_mut() else {
                return Err("本地数据库不可用".into());
            };
            let summary = store.import_problems(problems, &path.display().to_string())?;
            let deck_name = store
                .deck_cards()?
                .into_iter()
                .find(|card| card.id == summary.deck_id)
                .map(|card| card.name);
            (summary, deck_name)
        };
        let titles = self.analysis_titles_affected_by_import(path, deck_name.as_deref());
        self.clear_ai_analysis_records_for_titles(&titles);
        log::info!(
            "Imported problem bank: imported={}, inserted={}, updated={}",
            summary.imported,
            summary.inserted,
            summary.updated
        );
        Ok(summary)
    }

    fn finish_problem_import_batch(&mut self, batch: ProblemImportBatchSummary) {
        if batch.imported_files > 0 {
            self.active_deck_name = None;
            self.loaded_path = batch.last_imported_path;
            self.deck = None;
            self.guided_problem_id = None;
            self.view = AppView::Library;
            self.refresh_store_state();
        }

        let mut status = if batch.imported_files == 0 {
            "没有成功导入题库。".to_owned()
        } else if batch.imported_files == 1 {
            format!(
                "导入 {} 道题：新增 {}，更新 {}。已保存到题库列表。",
                batch.imported, batch.inserted, batch.updated
            )
        } else {
            format!(
                "批量导入完成：成功 {} 个文件，共 {} 道题；新增 {}，更新 {}。",
                batch.imported_files, batch.imported, batch.inserted, batch.updated
            )
        };

        if !batch.failures.is_empty() {
            status.push_str(&format!(
                " 失败 {} 个：{}",
                batch.failures.len(),
                batch.failures.join("；")
            ));
        }
        let quality_text = batch.quality.describe();
        if !quality_text.is_empty() {
            status.push_str(&format!(" {quality_text}。"));
        }
        self.status = status;
    }

    fn import_problem_bank_text(&mut self) {
        let text = self.text_import_buffer.trim();
        if text.is_empty() {
            self.status = "请先粘贴 JSON 或 CSV 题库文本。".into();
            return;
        }

        match load_problems_from_text(text) {
            Ok(problems) => {
                let quality = ImportQualitySummary::from_problems(&problems);
                let virtual_path = PathBuf::from("粘贴导入题库");
                match self.save_imported_problems(&virtual_path, &problems) {
                    Ok(summary) => {
                        self.text_import_buffer.clear();
                        self.show_text_import_dialog = false;
                        self.finish_problem_import_batch(ProblemImportBatchSummary {
                            imported_files: 1,
                            imported: summary.imported,
                            inserted: summary.inserted,
                            updated: summary.updated,
                            last_imported_path: Some(virtual_path),
                            quality,
                            ..Default::default()
                        });
                    }
                    Err(err) => {
                        self.status = format!("保存粘贴题库失败：{err}");
                    }
                }
            }
            Err(err) => {
                self.status = format!("粘贴题库解析失败：{err}");
            }
        }
    }

    fn poll_ai_import(&mut self) {
        let Some(receiver) = &self.ai_import_receiver else {
            return;
        };
        let Ok(responses) = receiver.try_recv() else {
            return;
        };
        self.ai_import_receiver = None;
        self.is_ai_importing = false;
        let total_files = responses.len();
        let mut imported_files = 0usize;
        let mut imported = 0usize;
        let mut inserted = 0usize;
        let mut updated = 0usize;
        let mut extracted_chars = 0usize;
        let mut failures = Vec::new();
        let mut last_imported_path = None;
        let mut quality = ImportQualitySummary::default();

        for response in responses {
            match response.result {
                Ok(result) => {
                    let problem_count = result.problems.len();
                    extracted_chars += result.extracted_chars;
                    match self.save_imported_problems(&response.path, &result.problems) {
                        Ok(summary) => {
                            quality.add(&ImportQualitySummary::from_problems(&result.problems));
                            imported_files += 1;
                            imported += summary.imported;
                            inserted += summary.inserted;
                            updated += summary.updated;
                            last_imported_path = Some(response.path);
                            log::info!(
                                "AI imported problem bank: problems={}, extracted_chars={}",
                                problem_count,
                                result.extracted_chars
                            );
                        }
                        Err(err) => {
                            failures.push(format!("{}：保存失败：{err}", response.path.display()))
                        }
                    }
                }
                Err(err) => {
                    log::error!("AI problem bank import failed: {err}");
                    failures.push(format!("{}：{err}", response.path.display()));
                }
            }
        }

        let failure_suffix = if failures.is_empty() {
            String::new()
        } else {
            format!(" 失败 {} 个：{}", failures.len(), failures.join("；"))
        };

        self.finish_problem_import_batch(ProblemImportBatchSummary {
            imported_files,
            imported,
            inserted,
            updated,
            failures,
            last_imported_path,
            quality,
        });
        if imported_files > 0 {
            self.status = if total_files == 1 {
                format!(
                    "AI 导入完成：从约 {extracted_chars} 个字符中提取并导入 {imported} 道题。{failure_suffix}"
                )
            } else {
                format!(
                    "AI 批量导入完成：成功 {imported_files}/{total_files} 个文件，从约 {extracted_chars} 个字符中提取并导入 {imported} 道题；新增 {inserted}，更新 {updated}。{failure_suffix}"
                )
            };
        }
    }

    fn load_bank_from_store(&mut self) {
        let Some(store) = &self.store else { return };
        match store.load_all_problems() {
            Ok(problems) if !problems.is_empty() => {
                let count = problems.len();
                self.clear_latest_practice_session();
                self.deck = Some(PracticeDeck::with_order(problems, self.practice_order));
                self.guided_problem_id = None;
                self.active_deck_name = Some("全部题库".into());
                self.view = AppView::Practice;
                self.prepare_practice_entry();
                self.status = format!("已从全部题库载入 {count} 道题。");
            }
            Ok(_) => {}
            Err(err) => self.status = format!("读取本地题库失败：{err}"),
        }
    }

    fn refresh_store_state(&mut self) {
        let Some(store) = &self.store else { return };
        self.bank_count = store.problem_count().unwrap_or(0);
        self.deck_cards = store.deck_cards().unwrap_or_default();
        self.group_cards = store.group_cards().unwrap_or_default();
        self.answer_history = store.answer_history(8).unwrap_or_default();
        let available_deck_ids = self
            .deck_cards
            .iter()
            .map(|card| card.id)
            .collect::<BTreeSet<_>>();
        self.selected_deck_ids
            .retain(|deck_id| available_deck_ids.contains(deck_id));
    }

    fn set_deck_selected(&mut self, deck_id: i64, selected: bool) {
        if selected {
            self.selected_deck_ids.insert(deck_id);
        } else {
            self.selected_deck_ids.remove(&deck_id);
        }
    }

    fn deck_drag_ids(&self, deck_id: i64) -> Vec<i64> {
        if self.selected_deck_ids.contains(&deck_id) {
            self.selected_deck_ids.iter().copied().collect()
        } else {
            vec![deck_id]
        }
    }

    fn is_deck_dragging(&self, deck_id: i64) -> bool {
        let Some(dragging_deck_id) = self.dragging_deck_id else {
            return false;
        };
        dragging_deck_id == deck_id
            || (self.selected_deck_ids.contains(&dragging_deck_id)
                && self.selected_deck_ids.contains(&deck_id))
    }

    fn restart_from_store(&mut self) {
        self.load_bank_from_store();
        self.loaded_path = None;
        self.active_deck_name = Some("全部题库".into());
        self.prepare_practice_entry();
        self.analysis.clear();
    }

    fn start_deck_card(&mut self, deck_id: i64, deck_name: String) {
        let Some(store) = &self.store else {
            self.status = "本地数据库不可用。".into();
            return;
        };

        match store.load_deck_problems(deck_id) {
            Ok(problems) if !problems.is_empty() => {
                let count = problems.len();
                self.clear_latest_practice_session();
                self.deck = Some(PracticeDeck::with_order(problems, self.practice_order));
                self.guided_problem_id = None;
                self.active_deck_name = Some(deck_name.clone());
                self.loaded_path = None;
                self.view = AppView::Practice;
                self.prepare_practice_entry();
                self.analysis.clear();
                self.status = format!("已开始题库「{deck_name}」，共 {count} 道题。");
            }
            Ok(_) => self.status = format!("题库「{deck_name}」为空。"),
            Err(err) => self.status = format!("读取题库失败：{err}"),
        }
    }

    fn preview_deck_card(&mut self, deck_id: i64, deck_name: String) {
        let Some(store) = &self.store else {
            self.status = "本地数据库不可用。".into();
            return;
        };

        match store.load_deck_problems(deck_id) {
            Ok(problems) if !problems.is_empty() => {
                let count = problems.len();
                self.deck_preview = Some(DeckPreviewState {
                    deck_id,
                    deck_name: deck_name.clone(),
                    problems,
                    open: true,
                    editing_problem_id: None,
                    editing_answer: String::new(),
                });
                self.status = format!("正在预览题库「{deck_name}」，共 {count} 道题。");
            }
            Ok(_) => self.status = format!("题库「{deck_name}」为空。"),
            Err(err) => self.status = format!("读取题库预览失败：{err}"),
        }
    }

    #[cfg(target_os = "android")]
    fn export_problem_bank(&mut self, _title: String, _info: String, _problems: Vec<Problem>) {
        self.status = "Android 端题库导出稍后接入系统分享/文件保存。".into();
    }

    #[cfg(not(target_os = "android"))]
    fn export_problem_bank(&mut self, title: String, info: String, problems: Vec<Problem>) {
        if problems.is_empty() {
            self.status = format!("{title} 没有可导出的题目。");
            return;
        }

        let default_name = default_export_file_name(&title, "zip");
        let Some(path) = rfd::FileDialog::new()
            .add_filter("ShuaForge 题库包", &["zip"])
            .add_filter("JSON 题库", &["json"])
            .set_file_name(default_name)
            .save_file()
        else {
            return;
        };

        let bank = ExportProblemBank::new(title.clone(), info, problems);
        let result = match path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("json") => export_problem_bank_json(&path, &bank),
            _ => export_problem_bank_zip(&path, &bank),
        };

        match result {
            Ok(()) => {
                self.status = format!(
                    "已导出「{}」共 {} 道题到：{}",
                    bank.deck_name,
                    bank.problem_count,
                    path.display()
                );
            }
            Err(err) => {
                self.status = format!("导出「{title}」失败：{err}");
            }
        }
    }

    fn export_deck_card(&mut self, deck_id: i64, deck_name: String) {
        let Some(store) = &self.store else {
            self.status = "本地数据库不可用。".into();
            return;
        };

        match store.load_deck_problems(deck_id) {
            Ok(problems) => {
                let info = self
                    .deck_cards
                    .iter()
                    .find(|card| card.id == deck_id)
                    .map(|card| {
                        format!(
                            "来源：{}；更新时间：{}；新增 {}；更新 {}",
                            card.source_path, card.updated_at, card.inserted, card.updated
                        )
                    })
                    .unwrap_or_default();
                self.export_problem_bank(deck_name, info, problems);
            }
            Err(err) => self.status = format!("读取题库失败：{err}"),
        }
    }

    fn export_group_card(&mut self, group_id: i64, group_name: String) {
        let Some(store) = &self.store else {
            self.status = "本地数据库不可用。".into();
            return;
        };

        match store.load_group_problems(group_id) {
            Ok(problems) => {
                let info = store
                    .group_decks(group_id)
                    .map(|decks| {
                        let names = decks
                            .iter()
                            .map(|deck| format!("{}（{}题）", deck.name, deck.problem_count))
                            .collect::<Vec<_>>()
                            .join("；");
                        if names.is_empty() {
                            "题组导出：暂无题库明细".into()
                        } else {
                            format!("题组导出：包含 {} 个题库：{}", decks.len(), names)
                        }
                    })
                    .unwrap_or_default();
                self.export_problem_bank(format!("题组-{}", group_name), info, problems);
            }
            Err(err) => self.status = format!("读取题组失败：{err}"),
        }
    }

    fn render_deck_preview_dialog(&mut self, ctx: &egui::Context) {
        let Some(preview) = self.deck_preview.clone() else {
            return;
        };

        let mut open = preview.open;
        let mut close_requested = false;
        let mut start_requested = false;
        let mut analyze_requested = false;
        let mut export_requested = false;
        let mut editing_problem_id = preview.editing_problem_id.clone();
        let mut editing_answer = preview.editing_answer.clone();
        let mut save_answer: Option<(String, String)> = None;
        egui::Window::new(format!("题库预览 · {}", preview.deck_name))
            .id(egui::Id::new(("deck_preview", preview.deck_id)))
            .open(&mut open)
            .default_width(760.0)
            .default_height(620.0)
            .resizable(true)
            .show(ctx, |ui| {
                let quality = ImportQualitySummary::from_problems(&preview.problems);
                ui.horizontal_wrapped(|ui| {
                    ui.heading(&preview.deck_name);
                    ui.label(format!("共 {} 道题", preview.problems.len()));
                });
                ui.small(quality.describe());
                ui.horizontal_wrapped(|ui| {
                    if ui.button("开始练习此题库").clicked() {
                        start_requested = true;
                        close_requested = true;
                    }
                    if ui.button("AI总监审查此题库").clicked() {
                        analyze_requested = true;
                    }
                    if ui.button("导出题库").clicked() {
                        export_requested = true;
                    }
                    if ui.button("关闭").clicked() {
                        close_requested = true;
                    }
                });
                ui.separator();

                egui::ScrollArea::vertical()
                    .id_salt(("deck_preview_scroll", preview.deck_id))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (index, problem) in preview.problems.iter().enumerate() {
                            let is_editing = editing_problem_id.as_deref() == Some(&problem.id);
                            match render_problem_preview_row(
                                ui,
                                index,
                                problem,
                                is_editing,
                                &mut editing_answer,
                            ) {
                                Some(ProblemPreviewAction::Start { problem_id, answer }) => {
                                    editing_problem_id = Some(problem_id);
                                    editing_answer = answer;
                                }
                                Some(ProblemPreviewAction::Save { problem_id, answer }) => {
                                    save_answer = Some((problem_id, answer));
                                }
                                Some(ProblemPreviewAction::Cancel) => {
                                    editing_problem_id = None;
                                    editing_answer.clear();
                                }
                                None => {}
                            }
                            ui.add_space(8.0);
                        }
                    });
            });

        if start_requested {
            self.start_deck_card(preview.deck_id, preview.deck_name.clone());
        }
        if analyze_requested {
            self.request_problem_set_analysis(
                format!("题库：{}", preview.deck_name),
                preview.problems.clone(),
            );
        }
        if export_requested {
            self.export_deck_card(preview.deck_id, preview.deck_name.clone());
        }
        if let Some((problem_id, answer)) = save_answer {
            self.save_preview_problem_answer(
                preview.deck_id,
                &preview.deck_name,
                &problem_id,
                &answer,
            );
            editing_problem_id = None;
            editing_answer.clear();
        }

        if close_requested || !open || start_requested {
            self.deck_preview = None;
        } else if let Some(preview) = &mut self.deck_preview {
            preview.open = open;
            preview.editing_problem_id = editing_problem_id;
            preview.editing_answer = editing_answer;
        }
    }

    fn save_preview_problem_answer(
        &mut self,
        deck_id: i64,
        deck_name: &str,
        problem_id: &str,
        answer: &str,
    ) {
        if answer.trim().is_empty() {
            self.status = "人工修正答案不能为空。".into();
            return;
        }

        let result = if let Some(store) = &mut self.store {
            store
                .update_problem_manual_answer(problem_id, answer)
                .and_then(|updated| {
                    let problems = store.load_deck_problems(deck_id)?;
                    Ok((updated, problems))
                })
        } else {
            Err("本地数据库不可用。".into())
        };

        match result {
            Ok((0, _)) => {
                self.status = format!("未找到题目 {problem_id}，答案未修改。");
            }
            Ok((_, problems)) => {
                if let Some(preview) = &mut self.deck_preview
                    && preview.deck_id == deck_id
                {
                    preview.problems = problems;
                    preview.editing_problem_id = None;
                    preview.editing_answer.clear();
                }
                let cleared =
                    self.clear_ai_analysis_records_for_titles(&[format!("题库：{deck_name}")]);
                self.status = if cleared > 0 {
                    format!("已人工修正题目 {problem_id} 的标准答案，并清理旧 AI 总监缓存。")
                } else {
                    format!("已人工修正题目 {problem_id} 的标准答案。")
                };
            }
            Err(err) => {
                self.status = format!("人工修正答案失败：{err}");
            }
        }
    }

    fn start_group_card(&mut self, group_id: i64, group_name: String) {
        let Some(store) = &self.store else {
            self.status = "本地数据库不可用。".into();
            return;
        };

        match store.load_group_problems(group_id) {
            Ok(problems) if !problems.is_empty() => {
                let count = problems.len();
                self.clear_latest_practice_session();
                self.deck = Some(PracticeDeck::with_order(problems, self.practice_order));
                self.guided_problem_id = None;
                self.active_deck_name = Some(format!("题组：{group_name}"));
                self.loaded_path = None;
                self.view = AppView::Practice;
                self.prepare_practice_entry();
                self.analysis.clear();
                self.status = format!("已开始题组「{group_name}」，共 {count} 道题。");
            }
            Ok(_) => self.status = format!("题组「{group_name}」还没有题库。"),
            Err(err) => self.status = format!("读取题组失败：{err}"),
        }
    }

    fn create_group(&mut self) {
        let Some(store) = &self.store else {
            self.status = "本地数据库不可用。".into();
            return;
        };
        match store.create_group(&self.new_group_name) {
            Ok(_) => {
                self.status = format!("已创建题组「{}」。", self.new_group_name.trim());
                self.new_group_name = "新题组".into();
                self.refresh_store_state();
            }
            Err(err) => self.status = format!("创建题组失败：{err}"),
        }
    }

    fn add_deck_to_group(&mut self, deck_id: i64, group_id: i64, group_name: &str) {
        let Some(store) = &self.store else {
            self.status = "本地数据库不可用。".into();
            return;
        };
        match store.add_deck_to_group(group_id, deck_id) {
            Ok(()) => {
                self.status = format!("已把题库加入题组「{group_name}」。");
                self.dragging_deck_id = None;
                self.refresh_store_state();
            }
            Err(err) => self.status = format!("加入题组失败：{err}"),
        }
    }

    fn add_decks_to_group(&mut self, deck_ids: &[i64], group_id: i64, group_name: &str) {
        if deck_ids.is_empty() {
            return;
        }
        if let [deck_id] = deck_ids {
            self.add_deck_to_group(*deck_id, group_id, group_name);
            return;
        }
        let Some(store) = &self.store else {
            self.status = "本地数据库不可用。".into();
            return;
        };
        let mut added = 0usize;
        let mut failed = 0usize;
        for deck_id in deck_ids {
            match store.add_deck_to_group(group_id, *deck_id) {
                Ok(()) => added += 1,
                Err(_) => failed += 1,
            }
        }
        self.dragging_deck_id = None;
        self.refresh_store_state();
        self.status = if failed == 0 {
            format!("已把 {added} 个题库加入题组「{group_name}」。")
        } else {
            format!("已把 {added} 个题库加入题组「{group_name}」，{failed} 个失败。")
        };
    }

    fn remove_deck_from_group(&mut self, deck_id: i64, group_id: i64, group_name: &str) {
        let Some(store) = &self.store else {
            self.status = "本地数据库不可用。".into();
            return;
        };
        match store.remove_deck_from_group(group_id, deck_id) {
            Ok(()) => {
                self.status = format!("已将题库从题组「{group_name}」移出。");
                self.refresh_store_state();
            }
            Err(err) => self.status = format!("移出题组失败：{err}"),
        }
    }

    fn delete_deck(&mut self, deck_id: i64, deck_name: &str) {
        let analysis_titles = self.analysis_titles_affected_by_deck(deck_id, deck_name);
        let Some(store) = self.store.as_mut() else {
            self.status = "本地数据库不可用。".into();
            return;
        };

        let delete_result = store.delete_deck(deck_id);
        match delete_result {
            Ok(()) => {
                let cleared = self.clear_ai_analysis_records_for_titles(&analysis_titles);
                self.status = if cleared > 0 {
                    format!("已删除题库「{deck_name}」，并清理 {cleared} 条相关 AI 分析记录。")
                } else {
                    format!("已删除题库「{deck_name}」。")
                };
                self.dragging_deck_id = None;
                self.dragging_group_id = None;
                if self
                    .deck_preview
                    .as_ref()
                    .is_some_and(|preview| preview.deck_id == deck_id)
                {
                    self.deck_preview = None;
                }
                self.refresh_store_state();
            }
            Err(err) => self.status = format!("删除题库失败：{err}"),
        }
    }

    fn delete_decks(&mut self, deck_ids: &[i64]) {
        if deck_ids.is_empty() {
            return;
        }
        if let [deck_id] = deck_ids
            && let Some(card) = self
                .deck_cards
                .iter()
                .find(|card| card.id == *deck_id)
                .cloned()
        {
            self.delete_deck(card.id, &card.name);
            return;
        }
        let cards = deck_ids
            .iter()
            .filter_map(|deck_id| {
                self.deck_cards
                    .iter()
                    .find(|card| card.id == *deck_id)
                    .cloned()
            })
            .collect::<Vec<_>>();
        let mut deleted = 0usize;
        let mut failed = 0usize;
        for card in cards {
            let analysis_titles = self.analysis_titles_affected_by_deck(card.id, &card.name);
            let Some(store) = self.store.as_mut() else {
                self.status = "本地数据库不可用。".into();
                return;
            };
            match store.delete_deck(card.id) {
                Ok(()) => {
                    deleted += 1;
                    self.clear_ai_analysis_records_for_titles(&analysis_titles);
                    if self
                        .deck_preview
                        .as_ref()
                        .is_some_and(|preview| preview.deck_id == card.id)
                    {
                        self.deck_preview = None;
                    }
                }
                Err(_) => failed += 1,
            }
        }
        self.dragging_deck_id = None;
        self.dragging_group_id = None;
        for deck_id in deck_ids {
            self.selected_deck_ids.remove(deck_id);
        }
        self.refresh_store_state();
        self.status = if failed == 0 {
            format!("已删除 {deleted} 个题库。")
        } else {
            format!("已删除 {deleted} 个题库，{failed} 个删除失败。")
        };
    }

    fn delete_group(&mut self, group_id: i64, group_name: &str) {
        let analysis_titles = vec![format!("题组：{group_name}")];
        let Some(store) = &self.store else {
            self.status = "本地数据库不可用。".into();
            return;
        };

        let delete_result = store.delete_group(group_id);
        match delete_result {
            Ok(()) => {
                let cleared = self.clear_ai_analysis_records_for_titles(&analysis_titles);
                self.status = if cleared > 0 {
                    format!("已删除题组「{group_name}」，并清理 {cleared} 条相关 AI 分析记录。")
                } else {
                    format!("已删除题组「{group_name}」。")
                };
                self.dragging_group_id = None;
                self.dragging_deck_id = None;
                self.refresh_store_state();
            }
            Err(err) => self.status = format!("删除题组失败：{err}"),
        }
    }

    fn analysis_titles_affected_by_deck(&self, deck_id: i64, deck_name: &str) -> Vec<String> {
        let mut titles = vec![format!("题库：{deck_name}")];
        if let Some(card) = self.deck_cards.iter().find(|card| card.id == deck_id)
            && let Some(title) = import_director_analysis_title(
                &PathBuf::from(&card.source_path),
                Some(card.name.as_str()),
            )
        {
            titles.push(title);
        }
        let Some(store) = &self.store else {
            return titles;
        };
        for group in &self.group_cards {
            let Ok(group_decks) = store.group_decks(group.id) else {
                continue;
            };
            if group_decks.iter().any(|deck| deck.id == deck_id) {
                titles.push(format!("题组：{}", group.name));
            }
        }
        titles
    }

    fn analysis_titles_affected_by_import(
        &self,
        path: &Path,
        deck_name: Option<&str>,
    ) -> Vec<String> {
        let mut titles = Vec::new();
        if let Some(deck_name) = deck_name.map(str::trim).filter(|name| !name.is_empty()) {
            titles.push(format!("题库：{deck_name}"));
        }
        if let Some(title) = import_director_analysis_title(path, deck_name) {
            titles.push(title);
        }
        titles
    }

    fn clear_ai_analysis_records_for_titles(&mut self, titles: &[String]) -> usize {
        if titles.is_empty() {
            return 0;
        }

        let should_remove = |key: &AnalysisCacheKey| titles.iter().any(|title| title == &key.title);
        let mut removed = 0usize;
        let mut latest_text_removed = false;

        let cache_keys = self
            .analysis_cache
            .keys()
            .filter(|key| should_remove(key))
            .cloned()
            .collect::<Vec<_>>();
        for key in cache_keys {
            if let Some(dialog) = self.analysis_cache.remove(&key) {
                latest_text_removed |= !dialog.latest_result.trim().is_empty()
                    && dialog.latest_result == self.analysis;
                removed += 1;
            }
        }

        let dialog_keys = self
            .analysis_dialogs
            .keys()
            .filter(|key| should_remove(key))
            .cloned()
            .collect::<Vec<_>>();
        for key in dialog_keys {
            if let Some(dialog) = self.analysis_dialogs.remove(&key) {
                latest_text_removed |= !dialog.latest_result.trim().is_empty()
                    && dialog.latest_result == self.analysis;
            }
        }

        let source_keys = self
            .analysis_sources
            .keys()
            .filter(|key| should_remove(key))
            .cloned()
            .collect::<Vec<_>>();
        for key in source_keys {
            self.analysis_sources.remove(&key);
        }

        if self
            .analysis_dialog
            .as_ref()
            .is_some_and(|dialog| titles.iter().any(|title| title == &dialog.title))
        {
            if let Some(dialog) = &self.analysis_dialog {
                latest_text_removed |= !dialog.latest_result.trim().is_empty()
                    && dialog.latest_result == self.analysis;
                if !self.analysis_cache.contains_key(&AnalysisCacheKey {
                    kind: dialog.kind,
                    title: dialog.title.clone(),
                }) {
                    removed += 1;
                }
            }
            self.analysis_dialog = None;
        }

        if self.active_analysis_key.as_ref().is_some_and(should_remove) {
            self.active_analysis_key = None;
            latest_text_removed = true;
        }

        if latest_text_removed {
            self.analysis.clear();
            self.persist_analysis_text();
        }
        if removed > 0 {
            self.persist_analysis_cache();
        }
        removed
    }

    fn remove_deck_from_all_groups(&mut self, deck_id: i64, deck_name: &str) {
        let Some(store) = &self.store else {
            self.status = "本地数据库不可用。".into();
            return;
        };
        let groups = self.group_cards.clone();
        let mut removed = 0usize;
        for group in groups {
            if let Ok(group_decks) = store.group_decks(group.id)
                && group_decks.iter().any(|item| item.id == deck_id)
                && store.remove_deck_from_group(group.id, deck_id).is_ok()
            {
                removed += 1;
            }
        }
        self.refresh_store_state();
        self.status = if removed == 0 {
            format!("题库「{deck_name}」本来就没有加入任何题组。")
        } else {
            format!("已将题库「{deck_name}」从 {removed} 个题组中移出。")
        };
    }

    fn back_to_library(&mut self) {
        self.persist_current_practice_session();
        self.view = AppView::Library;
        self.deck = None;
        self.active_deck_name = None;
        self.guided_problem_id = None;
        self.clear_answer_inputs();
        self.reset_keyboard_choice_focus_mode();
        self.analysis.clear();
        self.refresh_store_state();
    }

    fn clear_answer_inputs(&mut self) {
        self.answer_input.clear();
        self.selected_single = None;
        self.selected_multiple.clear();
        self.focused_choice_index = 0;
    }

    fn reset_keyboard_choice_focus_mode(&mut self) {
        self.keyboard_choice_focus_visible = false;
    }

    fn prepare_practice_entry(&mut self) {
        self.clear_answer_inputs();
        self.reset_keyboard_choice_focus_mode();
    }

    fn current_practice_session_key(&self) -> Option<String> {
        self.deck.as_ref()?;
        Some(PRACTICE_SESSION_KEY.to_owned())
    }

    fn has_saved_practice_session(&self) -> bool {
        self.store
            .as_ref()
            .and_then(|store| store.get_setting(PRACTICE_SESSION_KEY).ok().flatten())
            .is_some()
    }

    fn load_practice_session(&mut self) -> Option<PracticeDeck> {
        let store = self.store.as_ref()?;
        let value = store.get_setting(PRACTICE_SESSION_KEY).ok().flatten()?;
        let session = serde_json::from_str::<PersistedPracticeSession>(&value).ok()?;
        if session.key != PRACTICE_SESSION_KEY {
            return None;
        }
        self.active_deck_name = session.active_deck_name;
        self.guided_problem_id = session.guided_problem_id;
        log::info!("Restored latest practice session");
        Some(PracticeDeck::from_snapshot(session.deck))
    }

    fn continue_latest_practice_session(&mut self) {
        match self.load_practice_session() {
            Some(deck) => {
                self.deck = Some(deck);
                self.view = AppView::Practice;
                self.prepare_practice_entry();
                self.analysis.clear();
                self.status = "已恢复上次退出/返回前的练习进度。".into();
            }
            None => {
                self.status = "没有可继续的练习进度。".into();
            }
        }
    }

    fn persist_current_practice_session(&mut self) {
        let Some(key) = self.current_practice_session_key() else {
            return;
        };
        let Some(deck) = &self.deck else {
            return;
        };
        let Some(store) = &self.store else {
            return;
        };
        let session = PersistedPracticeSession {
            key: key.clone(),
            active_deck_name: self.active_deck_name.clone(),
            guided_problem_id: self.guided_problem_id.clone(),
            deck: deck.snapshot(),
        };
        let Ok(value) = serde_json::to_string(&session) else {
            return;
        };
        if let Err(err) = store.set_setting(&key, &value) {
            self.status = format!("练习进度保存失败：{err}");
        }
    }

    fn clear_current_practice_session_if_finished(&mut self) {
        let Some(deck) = &self.deck else {
            return;
        };
        if !deck.is_finished() {
            return;
        }
        let Some(key) = self.current_practice_session_key() else {
            return;
        };
        if let Some(store) = &self.store {
            let _ = store.delete_setting(&key);
        }
    }

    fn clear_latest_practice_session(&mut self) {
        if let Some(store) = &self.store {
            let _ = store.delete_setting(PRACTICE_SESSION_KEY);
        }
    }

    fn load_ai_config(&mut self) {
        #[cfg(target_os = "android")]
        {
            self.status = "Android 端暂不支持从文件导入 AI 配置，请直接在设置页填写。".into();
            return;
        }
        #[cfg(not(target_os = "android"))]
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("AI 配置", &["json"])
            .pick_file()
        {
            match load_ai_config(&path) {
                Ok(config) => {
                    self.ai_config = config;
                    self.ai_config_path = Some(path);
                    self.persist_settings();
                    self.status = "AI 配置已载入。".into();
                }
                Err(err) => self.status = format!("AI 配置载入失败：{err}"),
            }
        }
    }

    fn save_ai_config(&mut self) {
        #[cfg(target_os = "android")]
        {
            self.status = "Android 端暂不支持导出 AI 配置文件。".into();
            return;
        }
        #[cfg(not(target_os = "android"))]
        let path = self.ai_config_path.clone().or_else(|| {
            rfd::FileDialog::new()
                .add_filter("AI 配置", &["json"])
                .set_file_name("ai-config.json")
                .save_file()
        });

        #[cfg(not(target_os = "android"))]
        {
            let Some(path) = path else { return };
            match save_ai_config(&path, &self.ai_config) {
                Ok(()) => {
                    self.ai_config_path = Some(path);
                    self.persist_settings();
                    self.status = "AI 配置已保存。".into();
                }
                Err(err) => self.status = format!("AI 配置保存失败：{err}"),
            }
        }
    }

    fn submit_answer(&mut self) {
        let Some(problem) = self.deck.as_ref().and_then(|deck| deck.current()).cloned() else {
            self.status = "请先导入题库。".into();
            return;
        };

        let user_answer = self.current_user_answer(&problem);
        if user_answer.trim().is_empty() {
            self.status = "请输入答案后再提交。".into();
            return;
        }

        let Some(deck) = self.deck.as_mut() else {
            self.status = "请先导入题库。".into();
            return;
        };

        if problem.needs_ai_review() {
            self.status = "该题尚未完成 AI 复核，暂不判题也不计错。请回到题库主页点击“AI分析”，统一复核后再练习。".into();
            self.analysis = format!(
                "这道题缺少可靠标准答案，当前答案不会被判错。\n\n你的输入：{}\n\n建议先对所在题库/题组运行 AI 分析；分析阶段会统一复核待复核题目，并把可确认的标准答案保存到题库。",
                user_answer.trim()
            );
            self.persist_current_practice_session();
            self.refresh_store_state();
            return;
        }

        let keep_after_submit = self.guided_problem_id.as_deref() == Some(problem.id.as_str());
        let result = if keep_after_submit {
            deck.submit_and_requeue(&user_answer)
        } else {
            deck.submit(&user_answer)
        };

        match result {
            SubmitResult::Correct => {
                self.record_answer(&problem, &user_answer, true);
                if keep_after_submit {
                    self.status = "回答正确；由于你查看过 AI 解题过程，本题已重新插回题库。".into();
                    self.analysis = "回答正确；本题已保留在当前练习队列中，稍后会再次出现。".into();
                } else {
                    self.status = "回答正确，已从题库移除。".into();
                    self.analysis = "回答正确，本题已从当前练习队列移除。".into();
                }
            }
            SubmitResult::Wrong {
                expected,
                explanation,
            } => {
                self.record_answer(&problem, &user_answer, false);
                self.status = format!("回答错误，已重新加入题库。标准答案：{expected}");
                if explanation.trim().is_empty() {
                    self.analysis = "本题暂无预生成解析，正在临时请求 AI 解析...".into();
                    self.request_ai_analysis(problem, user_answer);
                } else {
                    self.analysis = explanation;
                }
            }
            SubmitResult::NoCurrentProblem => self.status = "当前没有题目。".into(),
        }

        self.clear_answer_inputs();
        self.guided_problem_id = None;
        if self.deck.as_ref().is_some_and(PracticeDeck::is_finished) {
            self.clear_current_practice_session_if_finished();
        } else {
            self.persist_current_practice_session();
        }
        self.refresh_store_state();
    }

    fn current_user_answer(&self, problem: &Problem) -> String {
        match problem.kind() {
            ProblemType::SingleChoice => self.selected_single.clone().unwrap_or_default(),
            ProblemType::MultipleChoice => normalize_choice_answer(
                &self
                    .selected_multiple
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(","),
            ),
            ProblemType::Text => self.answer_input.clone(),
        }
    }

    fn record_answer(&mut self, problem: &Problem, user_answer: &str, is_correct: bool) {
        if let Some(store) = &self.store
            && let Err(err) =
                store.record_answer(&problem.id, user_answer, &problem.answer, is_correct)
        {
            self.status = format!("答题记录保存失败：{err}");
        }
    }

    fn request_ai_analysis(&mut self, problem: Problem, user_answer: String) {
        let config = self.ai_config.clone();
        let (sender, receiver) = mpsc::channel();
        self.ai_receiver = Some(receiver);
        self.is_ai_loading = true;

        thread::spawn(move || {
            let message = match explain_wrong_answer(&config, &problem, &user_answer) {
                Ok(text) => text,
                Err(err) => format!("AI 解析失败：{err}"),
            };
            let _ = sender.send(message);
        });
    }

    fn request_solution_guide(&mut self) {
        if self.is_ai_loading {
            self.status = "AI 正在生成中，请稍等当前请求完成。".into();
            return;
        }

        let Some(problem) = self.deck.as_ref().and_then(|deck| deck.current()).cloned() else {
            self.status = "当前没有可讲解的题目。".into();
            return;
        };

        if self.guided_problem_id.as_deref() == Some(problem.id.as_str()) {
            self.status = "AI 引导已经为当前题发起，请等待结果。".into();
            return;
        }

        if let Some(deck) = &mut self.deck {
            deck.requeue_current_without_advancing();
        }
        self.guided_problem_id = Some(problem.id.clone());
        self.persist_current_practice_session();
        self.status = "AI 正在生成不泄答案的做题过程；本题已重新插回题库，但当前不会跳题。".into();
        self.analysis = "AI 引导生成中...".into();

        let config = self.ai_config.clone();
        let (sender, receiver) = mpsc::channel();
        self.ai_receiver = Some(receiver);
        self.is_ai_loading = true;
        thread::spawn(move || {
            let message = match guide_solution_process(&config, &problem) {
                Ok(text) => text,
                Err(err) => format!("AI 引导失败：{err}"),
            };
            let _ = sender.send(message);
        });
    }

    fn analyze_deck_problems(&mut self, deck_id: i64, deck_name: String) {
        let Some(store) = &self.store else {
            self.status = "本地数据库不可用。".into();
            return;
        };

        match store.load_deck_problems(deck_id) {
            Ok(problems) => {
                self.request_problem_set_analysis(format!("题库：{deck_name}"), problems)
            }
            Err(err) => self.status = format!("读取题库失败：{err}"),
        }
    }

    fn analyze_group_problems(&mut self, group_id: i64, group_name: String) {
        let Some(store) = &self.store else {
            self.status = "本地数据库不可用。".into();
            return;
        };

        match store.load_group_problems(group_id) {
            Ok(problems) => {
                self.request_problem_set_analysis(format!("题组：{group_name}"), problems)
            }
            Err(err) => self.status = format!("读取题组失败：{err}"),
        }
    }

    fn analyze_deck_learning_gaps(&mut self, deck_id: i64, deck_name: String) {
        let Some(store) = &self.store else {
            self.status = "本地数据库不可用。".into();
            return;
        };

        match store.deck_answer_history(deck_id, 120) {
            Ok(records) => {
                self.request_learning_gap_analysis(format!("题库：{deck_name}"), records)
            }
            Err(err) => self.status = format!("读取答题历史失败：{err}"),
        }
    }

    fn analyze_group_learning_gaps(&mut self, group_id: i64, group_name: String) {
        let Some(store) = &self.store else {
            self.status = "本地数据库不可用。".into();
            return;
        };

        match store.group_answer_history(group_id, 120) {
            Ok(records) => {
                self.request_learning_gap_analysis(format!("题组：{group_name}"), records)
            }
            Err(err) => self.status = format!("读取答题历史失败：{err}"),
        }
    }

    fn request_problem_set_analysis(&mut self, title: String, problems: Vec<Problem>) {
        if problems.is_empty() {
            self.status = format!("{title} 没有可分析的题目。");
            return;
        }
        let key = AnalysisCacheKey {
            kind: AnalysisKind::ProblemSet,
            title: title.clone(),
        };
        self.analysis_sources
            .insert(key.clone(), AnalysisSource::Problems(problems.clone()));
        self.stash_active_analysis_dialog();
        if let Some(dialog) = self.analysis_cache.get(&key).cloned() {
            let dialog = AnalysisDialogState {
                open: true,
                ..dialog
            };
            self.analysis_dialogs.insert(key.clone(), dialog.clone());
            self.analysis_dialog = Some(dialog);
            self.active_analysis_key = Some(key);
            self.status = format!("已打开{title}的分析对话。");
            return;
        }
        self.start_problem_set_analysis(title, problems);
    }

    fn start_problem_set_analysis(&mut self, title: String, problems: Vec<Problem>) {
        self.finish_active_loading_dialog_as_switched();
        let generation_id = self.next_analysis_generation();
        let key = AnalysisCacheKey {
            kind: AnalysisKind::ProblemSet,
            title: title.clone(),
        };
        let config = self.ai_config.clone();
        let (sender, receiver) = mpsc::channel();
        self.analysis_stream_receiver = Some(receiver);
        let dialog = AnalysisDialogState::loading(
            title.clone(),
            AnalysisKind::ProblemSet,
            "shuaforge.ai_director_review_problem_set",
            serde_json::json!({
                "title": title,
                "problem_count": problems.len(),
                "data_access": "local_compact_full_dataset",
                "available_tools": ["shuaforge.local_compact_problem_set"],
                "review_pipeline": ["deterministic_cleanup", "pending_problem_ai_review", "knowledge_point_annotation", "director_quality_report"],
                "analysis_dimensions": ["data_quality", "answer_source", "review_status", "problem_type", "knowledge_points", "practice_order"]
            })
            .to_string(),
        );
        self.analysis_dialogs.insert(key.clone(), dialog.clone());
        self.analysis_dialog = Some(dialog);
        self.active_analysis_key = Some(key);
        self.status = format!("AI 总监正在审查{title}...");
        self.analysis = "AI 总监正在审查题库质量，请稍候...".into();
        self.analysis_progress = Some(AnalysisProgressState::new(
            "准备 AI 总监审查",
            0,
            problems.len(),
        ));

        thread::spawn(move || {
            let (event_sender, event_receiver) = mpsc::channel();
            let forward_sender = sender.clone();
            let forward_handle = thread::spawn(move || {
                for event in event_receiver {
                    let finished = matches!(event, AnalysisStreamEvent::Finished);
                    let _ = forward_sender.send(AnalysisStreamResponse {
                        generation_id,
                        event,
                    });
                    if finished {
                        break;
                    }
                }
            });
            let review_progress_sender = event_sender.clone();
            let problems = review_pending_problems_with_progress(
                &config,
                problems,
                Some(&move |current, total| {
                    let _ = review_progress_sender.send(AnalysisStreamEvent::Progress {
                        label: "AI 统一复核待复核题目".into(),
                        current,
                        total,
                    });
                }),
            );
            let _ = event_sender.send(AnalysisStreamEvent::ReviewCompleted(problems.clone()));
            let explanation_progress_sender = event_sender.clone();
            let problems = pre_generate_explanations_with_progress(
                &config,
                problems,
                Some(&move |current, total| {
                    let _ = explanation_progress_sender.send(AnalysisStreamEvent::Progress {
                        label: "AI 并发预生成题目解析".into(),
                        current,
                        total,
                    });
                }),
            );
            let _ = event_sender.send(AnalysisStreamEvent::ExplanationsGenerated(problems.clone()));
            let progress_sender = event_sender.clone();
            let problems = annotate_problem_knowledge_points_with_progress(
                &config,
                problems,
                Some(&move |current, total| {
                    let _ = progress_sender.send(AnalysisStreamEvent::Progress {
                        label: "AI 知识点标注".into(),
                        current,
                        total,
                    });
                }),
            );
            let _ = event_sender.send(AnalysisStreamEvent::Progress {
                label: "AI 总监整理审查数据".into(),
                current: 1,
                total: 1,
            });
            let _ = event_sender.send(AnalysisStreamEvent::KnowledgePointsAnnotated(
                problems.clone(),
            ));
            stream_problem_set_analysis_tool(config, title, problems, event_sender);
            let _ = forward_handle.join();
        });
    }

    fn start_knowledge_point_reanalysis(&mut self) {
        let Some(dialog) = &self.analysis_dialog else {
            return;
        };
        if dialog.is_loading || dialog.kind != AnalysisKind::ProblemSet {
            return;
        }
        let key = AnalysisCacheKey {
            kind: dialog.kind,
            title: dialog.title.clone(),
        };
        let Some(AnalysisSource::Problems(problems)) = self.analysis_sources.get(&key).cloned()
        else {
            self.status = "当前对话缺少原始题目，无法重分析知识点。".into();
            return;
        };

        let generation_id = self.next_analysis_generation();
        let config = self.ai_config.clone();
        let (sender, receiver) = mpsc::channel();
        self.analysis_stream_receiver = Some(receiver);
        self.analysis_progress = Some(AnalysisProgressState::new(
            "准备重分析知识点",
            0,
            problems.len(),
        ));
        self.status = format!("正在重分析{}的 AI 知识点...", key.title);
        if let Some(dialog) = &mut self.analysis_dialog {
            dialog.is_loading = true;
        }

        thread::spawn(move || {
            let progress_sender = sender.clone();
            let problems = annotate_problem_knowledge_points_with_progress(
                &config,
                problems,
                Some(&move |current, total| {
                    let _ = progress_sender.send(AnalysisStreamResponse {
                        generation_id,
                        event: AnalysisStreamEvent::Progress {
                            label: "AI 知识点重分析".into(),
                            current,
                            total,
                        },
                    });
                }),
            );
            let _ = sender.send(AnalysisStreamResponse {
                generation_id,
                event: AnalysisStreamEvent::KnowledgePointsAnnotated(problems),
            });
            let _ = sender.send(AnalysisStreamResponse {
                generation_id,
                event: AnalysisStreamEvent::Finished,
            });
        });
    }

    fn start_ai_review_check(&mut self) {
        let Some(dialog) = &self.analysis_dialog else {
            return;
        };
        if dialog.is_loading || dialog.kind != AnalysisKind::ProblemSet {
            return;
        }
        let key = AnalysisCacheKey {
            kind: dialog.kind,
            title: dialog.title.clone(),
        };
        let Some(AnalysisSource::Problems(problems)) = self.analysis_sources.get(&key).cloned()
        else {
            self.status = "当前对话缺少原始题目，无法执行 AI 核查。".into();
            return;
        };

        let pending_count = problems
            .iter()
            .filter(|problem| problem.needs_ai_review())
            .count();
        if pending_count == 0 {
            self.status = format!("{} 没有需要 AI 核查的题目。", key.title);
            return;
        }

        let generation_id = self.next_analysis_generation();
        let config = self.ai_config.clone();
        let (sender, receiver) = mpsc::channel();
        self.analysis_stream_receiver = Some(receiver);
        self.analysis_progress = Some(AnalysisProgressState::new(
            "准备 AI 核查待复核题目",
            0,
            pending_count,
        ));
        self.status = format!("正在核查{}的待复核题目...", key.title);
        if let Some(dialog) = &mut self.analysis_dialog {
            dialog.is_loading = true;
        }

        thread::spawn(move || {
            let progress_sender = sender.clone();
            let problems = review_pending_problems_with_progress(
                &config,
                problems,
                Some(&move |current, total| {
                    let _ = progress_sender.send(AnalysisStreamResponse {
                        generation_id,
                        event: AnalysisStreamEvent::Progress {
                            label: "AI 核查待复核题目".into(),
                            current,
                            total,
                        },
                    });
                }),
            );
            let _ = sender.send(AnalysisStreamResponse {
                generation_id,
                event: AnalysisStreamEvent::ReviewCompleted(problems),
            });
            let _ = sender.send(AnalysisStreamResponse {
                generation_id,
                event: AnalysisStreamEvent::Finished,
            });
        });
    }

    fn request_learning_gap_analysis(&mut self, title: String, records: Vec<AnswerRecord>) {
        let key = AnalysisCacheKey {
            kind: AnalysisKind::LearningGap,
            title: title.clone(),
        };
        self.analysis_sources
            .insert(key.clone(), AnalysisSource::Records(records.clone()));
        self.stash_active_analysis_dialog();
        if let Some(dialog) = self.analysis_cache.get(&key).cloned() {
            let dialog = AnalysisDialogState {
                open: true,
                ..dialog
            };
            self.analysis_dialogs.insert(key.clone(), dialog.clone());
            self.analysis_dialog = Some(dialog);
            self.active_analysis_key = Some(key);
            self.status = format!("已打开{title}的学习诊断对话。");
            return;
        }
        self.start_learning_gap_analysis(title, records);
    }

    fn start_learning_gap_analysis(&mut self, title: String, records: Vec<AnswerRecord>) {
        self.finish_active_loading_dialog_as_switched();
        let generation_id = self.next_analysis_generation();
        let key = AnalysisCacheKey {
            kind: AnalysisKind::LearningGap,
            title: title.clone(),
        };
        let config = self.ai_config.clone();
        let (sender, receiver) = mpsc::channel();
        self.analysis_receiver = Some(receiver);
        let dialog = AnalysisDialogState::loading(
            title.clone(),
            AnalysisKind::LearningGap,
            "shuaforge.analyze_learning_gaps",
            serde_json::json!({
                "title": title,
                "answer_record_count": records.len(),
                "record_limit": 120,
                "analysis_dimensions": ["accuracy", "weak_points", "mistake_patterns", "next_practice_plan"]
            })
            .to_string(),
        );
        self.analysis_dialogs.insert(key.clone(), dialog.clone());
        self.analysis_dialog = Some(dialog);
        self.active_analysis_key = Some(key);
        self.status = format!("正在分析{title}的答题表现...");
        self.analysis = "正在生成学习诊断，请稍候...".into();
        self.analysis_progress = Some(AnalysisProgressState::new("AI 正在生成学习诊断", 0, 0));

        thread::spawn(move || {
            let result = match call_learning_gap_analysis_tool(&config, &title, &records) {
                Ok(result) => result,
                Err(err) => AnalysisToolResult {
                    tool_name: "shuaforge.analyze_learning_gaps".into(),
                    arguments_json: "{}".into(),
                    result: format!("学习诊断失败：{err}"),
                },
            };
            let _ = sender.send(AnalysisAsyncResponse {
                generation_id,
                result,
            });
        });
    }

    fn stash_active_analysis_dialog(&mut self) {
        let Some(dialog) = self.analysis_dialog.take() else {
            return;
        };
        let key = AnalysisCacheKey {
            kind: dialog.kind,
            title: dialog.title.clone(),
        };
        self.analysis_dialogs.insert(key.clone(), dialog);
        self.active_analysis_key = Some(key);
    }

    fn finish_active_loading_dialog_as_switched(&mut self) {
        let Some(dialog) = &mut self.analysis_dialog else {
            return;
        };
        if !dialog.is_loading {
            return;
        }
        dialog.is_loading = false;
        if let Some(assistant_message) = dialog.messages.iter_mut().rev().find(|message| {
            message.role == ChatRole::Assistant && message.content == "正在分析，请稍候..."
        }) {
            assistant_message.content = "已切换到新的 AI 分析窗口，本次生成已停止。".into();
        }
        self.sync_active_analysis_dialog_to_pool();
    }

    fn set_active_analysis_dialog(&mut self, key: AnalysisCacheKey) {
        if self.active_analysis_key.as_ref() == Some(&key) {
            return;
        }
        self.stash_active_analysis_dialog();
        if let Some(mut dialog) = self.analysis_dialogs.remove(&key) {
            dialog.open = true;
            self.analysis_dialog = Some(dialog);
            self.active_analysis_key = Some(key);
        }
    }

    fn poll_ai(&mut self) {
        if let Some(receiver) = &self.ai_receiver
            && let Ok(message) = receiver.try_recv()
        {
            self.analysis = message;
            self.is_ai_loading = false;
            self.ai_receiver = None;
        }
        if let Some(receiver) = &self.analysis_receiver
            && let Ok(response) = receiver.try_recv()
        {
            if response.generation_id != self.analysis_generation_id {
                self.analysis_receiver = None;
                return;
            }
            let message = response.result;
            self.analysis = message.result.clone();
            self.status = "分析完成。".into();
            if let Some(dialog) = &mut self.analysis_dialog {
                if let Some(tool_message) = dialog
                    .messages
                    .iter_mut()
                    .find(|message| message.role == ChatRole::Tool)
                {
                    tool_message.title = format!("Tool Call · {}", message.tool_name);
                    tool_message.content = message.arguments_json;
                }
                if let Some(assistant_message) = dialog
                    .messages
                    .iter_mut()
                    .rev()
                    .find(|message| message.role == ChatRole::Assistant)
                {
                    assistant_message.content = self.analysis.clone();
                }
                dialog.latest_result = self.analysis.clone();
                dialog.is_loading = false;
                self.persist_analysis_text();
                self.cache_current_analysis_dialog();
            }
            self.analysis_receiver = None;
        }
        let mut stream_events = Vec::new();
        let mut clear_stream_receiver = false;
        if let Some(receiver) = &self.analysis_stream_receiver {
            while let Ok(response) = receiver.try_recv() {
                if response.generation_id != self.analysis_generation_id {
                    clear_stream_receiver = true;
                    break;
                }
                stream_events.push(response.event);
            }
        }
        for event in stream_events {
            match event {
                AnalysisStreamEvent::Progress {
                    label,
                    current,
                    total,
                } => {
                    self.analysis_progress =
                        Some(AnalysisProgressState::new(label, current, total));
                }
                AnalysisStreamEvent::KnowledgePointsAnnotated(problems) => {
                    if let Some(store) = &mut self.store {
                        match store.update_problem_tags(&problems) {
                            Ok(updated) if updated > 0 => {
                                self.status = format!(
                                    "已保存 {updated} 道题的 AI 知识点标签，正在生成分析..."
                                );
                            }
                            Ok(_) => {
                                self.status = "AI 知识点标签无需更新，正在生成分析...".into();
                            }
                            Err(err) => {
                                self.status = format!("AI 知识点标签保存失败：{err}");
                            }
                        }
                    }
                    if let Some(dialog) = &self.analysis_dialog {
                        let key = AnalysisCacheKey {
                            kind: dialog.kind,
                            title: dialog.title.clone(),
                        };
                        if matches!(dialog.kind, AnalysisKind::ProblemSet) {
                            self.analysis_sources
                                .insert(key, AnalysisSource::Problems(problems));
                        }
                    }
                }
                AnalysisStreamEvent::ExplanationsGenerated(problems) => {
                    if let Some(store) = &mut self.store {
                        match store.update_problem_explanations(&problems) {
                            Ok(updated) if updated > 0 => {
                                self.status = format!(
                                    "已保存 {updated} 道题的 AI 预生成解析，正在继续分析..."
                                );
                            }
                            Ok(_) => {
                                self.status = "AI 预生成解析无需更新，正在继续分析...".into();
                            }
                            Err(err) => {
                                self.status = format!("AI 预生成解析保存失败：{err}");
                            }
                        }
                    }
                    if let Some(dialog) = &self.analysis_dialog {
                        let key = AnalysisCacheKey {
                            kind: dialog.kind,
                            title: dialog.title.clone(),
                        };
                        if matches!(dialog.kind, AnalysisKind::ProblemSet) {
                            self.analysis_sources
                                .insert(key, AnalysisSource::Problems(problems));
                        }
                    }
                }
                AnalysisStreamEvent::ReviewCompleted(problems) => {
                    let review_summary = ai_review_action_summary(&problems);
                    let persisted_result = if let Some(store) = &mut self.store {
                        store.update_ai_review_accepted_answers(&problems)
                    } else {
                        Ok(0)
                    };
                    self.status = match persisted_result {
                        Ok(persisted) if persisted > 0 => {
                            format!(
                                "AI 复核已写回 {persisted} 道高置信参考答案：{review_summary}，正在继续生成总监报告..."
                            )
                        }
                        Ok(_) => {
                            format!(
                                "AI 复核未产生可安全写回的参考答案：{review_summary}，正在继续生成总监报告..."
                            )
                        }
                        Err(err) => format!(
                            "AI 复核答案写回失败：{err}；{review_summary}，仍将继续生成总监报告..."
                        ),
                    };
                    if let Some(dialog) = &self.analysis_dialog {
                        let key = AnalysisCacheKey {
                            kind: dialog.kind,
                            title: dialog.title.clone(),
                        };
                        if matches!(dialog.kind, AnalysisKind::ProblemSet) {
                            self.analysis_sources
                                .insert(key, AnalysisSource::Problems(problems));
                        }
                    }
                }
                AnalysisStreamEvent::ToolCall { arguments_json } => {
                    self.analysis_progress =
                        Some(AnalysisProgressState::new("AI 总监正在生成审查报告", 0, 0));
                    if let Some(dialog) = &mut self.analysis_dialog
                        && let Some(tool_message) = dialog
                            .messages
                            .iter_mut()
                            .find(|message| message.role == ChatRole::Tool)
                    {
                        tool_message.title =
                            "Tool Call · shuaforge.ai_director_review_problem_set".into();
                        tool_message.content = arguments_json;
                    }
                }
                AnalysisStreamEvent::TextDelta(delta) => {
                    self.analysis_progress =
                        Some(AnalysisProgressState::new("AI 总监正在生成审查报告", 0, 0));
                    self.analysis.push_str(&delta);
                    if let Some(dialog) = &mut self.analysis_dialog {
                        if let Some(assistant_message) = dialog
                            .messages
                            .iter_mut()
                            .rev()
                            .find(|message| message.role == ChatRole::Assistant)
                        {
                            if assistant_message.content == "正在分析，请稍候..."
                                || assistant_message.content == "AI 总监正在审查，请稍候..."
                            {
                                assistant_message.content.clear();
                            }
                            assistant_message.content.push_str(&delta);
                        }
                        dialog.latest_result = self.analysis.clone();
                    }
                    self.persist_analysis_text();
                }
                AnalysisStreamEvent::Finished => {
                    self.status = "AI 总监审查完成。".into();
                    self.analysis_progress = None;
                    if let Some(dialog) = &mut self.analysis_dialog {
                        dialog.is_loading = false;
                    }
                    self.persist_analysis_text();
                    self.cache_current_analysis_dialog();
                    clear_stream_receiver = true;
                }
                AnalysisStreamEvent::Failed(reason) => {
                    self.status = format!("AI 分析失败，已尝试回退：{reason}");
                    self.analysis_progress =
                        Some(AnalysisProgressState::new("正在生成本地回退分析", 0, 0));
                }
            }
        }
        if clear_stream_receiver {
            self.analysis_stream_receiver = None;
        }
        if let Some(receiver) = &self.chat_receiver
            && let Ok(response) = receiver.try_recv()
        {
            if response.generation_id != self.analysis_generation_id {
                self.chat_receiver = None;
                return;
            }
            if let Some(dialog) = &mut self.analysis_dialog {
                if let Some(assistant_message) = dialog.messages.iter_mut().rev().find(|message| {
                    message.role == ChatRole::Assistant && message.content == "正在回复，请稍候..."
                }) {
                    assistant_message.content = response.message;
                }
                dialog.is_loading = false;
                self.cache_current_analysis_dialog();
            }
            self.analysis_progress = None;
            self.chat_receiver = None;
        }
        self.sync_active_analysis_dialog_to_pool();
    }

    fn sync_active_analysis_dialog_to_pool(&mut self) {
        let Some(dialog) = &self.analysis_dialog else {
            return;
        };
        let key = AnalysisCacheKey {
            kind: dialog.kind,
            title: dialog.title.clone(),
        };
        self.analysis_dialogs.insert(key.clone(), dialog.clone());
        self.active_analysis_key = Some(key);
    }

    fn cache_current_analysis_dialog(&mut self) {
        let Some(dialog) = &self.analysis_dialog else {
            return;
        };
        let mut cached = dialog.clone();
        cached.open = false;
        let key = AnalysisCacheKey {
            kind: cached.kind,
            title: cached.title.clone(),
        };
        self.analysis_cache.insert(key, cached);
        self.persist_analysis_cache();
        self.persist_analysis_text();
    }

    fn next_analysis_generation(&mut self) -> u64 {
        self.analysis_generation_id = self.analysis_generation_id.wrapping_add(1).max(1);
        self.analysis_generation_id
    }

    fn cancel_analysis_work(&mut self) {
        self.next_analysis_generation();
        self.analysis_receiver = None;
        self.analysis_stream_receiver = None;
        self.chat_receiver = None;
        self.analysis_progress = None;
        if let Some(dialog) = &mut self.analysis_dialog {
            dialog.is_loading = false;
            if let Some(assistant_message) = dialog.messages.iter_mut().rev().find(|message| {
                message.role == ChatRole::Assistant
                    && (message.content == "正在分析，请稍候..."
                        || message.content == "正在回复，请稍候...")
            }) {
                assistant_message.content = "已取消生成。".into();
            }
        }
        self.status = "已取消当前 AI 生成。".into();
        self.cache_current_analysis_dialog();
    }

    fn load_analysis_cache(&mut self) {
        let Some(store) = &self.store else { return };
        let Ok(Some(value)) = store.get_setting(ANALYSIS_CACHE_KEY) else {
            return;
        };
        let Ok(cache) = serde_json::from_str::<PersistedAnalysisCache>(&value) else {
            return;
        };
        self.analysis_cache.clear();
        for entry in cache.entries {
            let key = AnalysisCacheKey {
                kind: entry.kind,
                title: entry.title.clone(),
            };
            self.analysis_cache.insert(
                key,
                AnalysisDialogState {
                    title: entry.title,
                    kind: entry.kind,
                    messages: entry.messages,
                    input: String::new(),
                    latest_result: entry.latest_result,
                    is_loading: false,
                    open: false,
                },
            );
        }
    }

    fn load_analysis_text_cache(&mut self) {
        let Some(store) = &self.store else { return };
        let Ok(Some(value)) = store.get_setting(ANALYSIS_TEXT_CACHE_KEY) else {
            return;
        };
        let Ok(snapshot) = serde_json::from_str::<PersistedAnalysisText>(&value) else {
            return;
        };
        if !snapshot.text.trim().is_empty() {
            self.analysis = snapshot.text;
        }
    }

    fn persist_analysis_cache(&mut self) {
        let Some(store) = &self.store else { return };
        let entries = self
            .analysis_cache
            .values()
            .map(|dialog| PersistedAnalysisDialog {
                kind: dialog.kind,
                title: dialog.title.clone(),
                messages: dialog.messages.clone(),
                latest_result: dialog.latest_result.clone(),
            })
            .collect::<Vec<_>>();
        let Ok(value) = serde_json::to_string(&PersistedAnalysisCache { entries }) else {
            return;
        };
        if let Err(err) = store.set_setting(ANALYSIS_CACHE_KEY, &value) {
            self.status = format!("分析对话缓存保存失败：{err}");
        }
    }

    fn persist_analysis_text(&mut self) {
        let Some(store) = &self.store else { return };
        let Ok(value) = serde_json::to_string(&PersistedAnalysisText {
            text: self.analysis.clone(),
        }) else {
            return;
        };
        if let Err(err) = store.set_setting(ANALYSIS_TEXT_CACHE_KEY, &value) {
            self.status = format!("分析结果缓存保存失败：{err}");
        }
    }

    fn start_new_analysis_dialog(&mut self) {
        let Some(dialog) = &self.analysis_dialog else {
            return;
        };
        if dialog.is_loading {
            return;
        }
        let key = AnalysisCacheKey {
            kind: dialog.kind,
            title: dialog.title.clone(),
        };
        self.analysis_cache.remove(&key);
        self.persist_analysis_cache();
        match self.analysis_sources.get(&key).cloned() {
            Some(AnalysisSource::Problems(problems)) => {
                self.start_problem_set_analysis(key.title, problems);
            }
            Some(AnalysisSource::Records(records)) => {
                self.start_learning_gap_analysis(key.title, records);
            }
            None => {
                self.status = "当前对话缺少原始分析上下文，无法新建对话。".into();
            }
        }
    }

    fn render_library_home(&mut self, ui: &mut egui::Ui) {
        let narrow = self.ui_mode == AppUiMode::Mobile;
        let compact_cards = ui.available_width() < 600.0;
        ui.horizontal_wrapped(|ui| {
            ui.heading("题库主页");
            if !narrow {
                ui.label("集中管理题库与题组");
            }
        });
        ui.label("可将题库加入题组，按章节、课程或专题组织练习内容。");
        ui.add_space(10.0);

        if narrow {
            if ui
                .add_sized(
                    [ui.available_width(), MOBILE_TOUCH_HEIGHT],
                    egui::Button::new("导入题库"),
                )
                .clicked()
            {
                self.import_problem_bank();
            }
            if ui
                .add_sized(
                    [ui.available_width(), MOBILE_TOUCH_HEIGHT],
                    egui::Button::new("AI 导入"),
                )
                .clicked()
            {
                self.import_problem_bank_with_ai_dialog();
            }
            if self.is_ai_importing {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("AI 导入中");
                });
            }
            if ui
                .add_sized(
                    [ui.available_width(), MOBILE_TOUCH_HEIGHT],
                    egui::Button::new("练习全部题库"),
                )
                .clicked()
            {
                self.restart_from_store();
            }
            if self.has_saved_practice_session()
                && ui
                    .add_sized(
                        [ui.available_width(), MOBILE_TOUCH_HEIGHT],
                        egui::Button::new("继续上次练习"),
                    )
                    .clicked()
            {
                self.continue_latest_practice_session();
            }
            ui.horizontal(|ui| {
                ui.add_sized(
                    [ui.available_width() - 82.0, MOBILE_TOUCH_HEIGHT],
                    egui::TextEdit::singleline(&mut self.new_group_name).hint_text("新题组名称"),
                );
                if ui
                    .add_sized([76.0, MOBILE_TOUCH_HEIGHT], egui::Button::new("创建"))
                    .clicked()
                {
                    self.create_group();
                }
            });
        } else {
            ui.horizontal_wrapped(|ui| {
                if ui.button("导入新题库").clicked() {
                    self.import_problem_bank();
                }
                if ui.button("从浏览器获取题库").clicked() {
                    self.request_browser_snapshot_import();
                }
                if ui
                    .add_enabled(!self.is_ai_importing, egui::Button::new("AI导入题库"))
                    .clicked()
                {
                    self.import_problem_bank_with_ai_dialog();
                }
                if self.is_ai_importing {
                    ui.spinner();
                    ui.label("AI 导入中");
                }
                if ui.button("练习全部题库").clicked() {
                    self.restart_from_store();
                }
                if self.has_saved_practice_session() && ui.button("继续上次练习").clicked() {
                    self.continue_latest_practice_session();
                }
                ui.separator();
                ui.label("新题组");
                ui.text_edit_singleline(&mut self.new_group_name);
                if ui.button("创建题组").clicked() {
                    self.create_group();
                }
            });
        }

        ui.add_space(10.0);
        let trash_response = render_trash_zone(
            ui,
            self.dragging_deck_id.is_some() || self.dragging_group_id.is_some(),
        );
        if trash_response.hovered() && ui.input(|input| input.pointer.any_released()) {
            if let Some(deck_id) = self.dragging_deck_id.take() {
                let deck_ids = self.deck_drag_ids(deck_id);
                self.delete_decks(&deck_ids);
            }
            if let Some(group_id) = self.dragging_group_id.take()
                && let Some(group) = self
                    .group_cards
                    .iter()
                    .find(|group| group.id == group_id)
                    .cloned()
            {
                self.delete_group(group.id, &group.name);
            }
        }

        ui.separator();
        ui.heading("题组");
        if self.group_cards.is_empty() {
            ui.small("暂无题组。创建题组后，可将多个题库合并练习。");
        } else {
            let groups = self.group_cards.clone();
            let columns = if self.ui_mode == AppUiMode::Mobile {
                mobile_library_card_columns(ui, groups.len())
            } else if compact_cards {
                1
            } else {
                grid_columns(ui, CARD_WIDTH)
            };
            let gap = if self.ui_mode == AppUiMode::Mobile {
                MOBILE_LIBRARY_CARD_GAP
            } else {
                12.0
            };
            let mobile_cell_width = if self.ui_mode == AppUiMode::Mobile {
                Some(
                    ((ui.available_width() - gap * (columns.saturating_sub(1) as f32))
                        / columns as f32)
                        .max(MOBILE_LIBRARY_CARD_MIN_WIDTH),
                )
            } else {
                None
            };
            for row in groups.chunks(columns) {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for group in row {
                        let group = group.clone();
                        let group_decks = self
                            .store
                            .as_ref()
                            .and_then(|store| store.group_decks(group.id).ok())
                            .unwrap_or_default();
                        let (
                            response,
                            drag_response,
                            start_clicked,
                            analyze_clicked,
                            diagnose_clicked,
                            export_clicked,
                            remove_requests,
                        ) = if self.ui_mode == AppUiMode::Mobile {
                            let cell_width = mobile_cell_width.unwrap_or(ui.available_width());
                            ui.allocate_ui_with_layout(
                                egui::vec2(cell_width, 0.0),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    render_mobile_group_card(
                                        ui,
                                        &group,
                                        &group_decks,
                                        self.dragging_deck_id.is_some(),
                                        self.dragging_group_id == Some(group.id),
                                    )
                                },
                            )
                            .inner
                        } else {
                            render_group_card(
                                ui,
                                &group,
                                &group_decks,
                                self.dragging_deck_id.is_some(),
                                self.dragging_group_id == Some(group.id),
                            )
                        };
                        if drag_response.drag_started() || drag_response.dragged() {
                            self.dragging_group_id = Some(group.id);
                        }
                        if start_clicked {
                            self.start_group_card(group.id, group.name.clone());
                        }
                        if analyze_clicked {
                            self.analyze_group_problems(group.id, group.name.clone());
                        }
                        if diagnose_clicked {
                            self.analyze_group_learning_gaps(group.id, group.name.clone());
                        }
                        if export_clicked {
                            self.export_group_card(group.id, group.name.clone());
                        }
                        for deck_id in remove_requests {
                            self.remove_deck_from_group(deck_id, group.id, &group.name);
                        }
                        response.context_menu(|ui| {
                            if ui.button("导出题组").clicked() {
                                self.export_group_card(group.id, group.name.clone());
                                ui.close();
                            }
                            if ui.button("分析题组题目").clicked() {
                                self.analyze_group_problems(group.id, group.name.clone());
                                ui.close();
                            }
                            if ui.button("分析答题薄弱点").clicked() {
                                self.analyze_group_learning_gaps(group.id, group.name.clone());
                                ui.close();
                            }
                            ui.separator();
                            if let Some(store) = &self.store
                                && let Ok(group_decks) = store.group_decks(group.id)
                            {
                                if group_decks.is_empty() {
                                    ui.small("题组内还没有题库。");
                                } else {
                                    ui.menu_button("从题组移出题库", |ui| {
                                        for deck in &group_decks {
                                            if ui
                                                .button(format!(
                                                    "{}（{}题）",
                                                    deck.name, deck.problem_count
                                                ))
                                                .clicked()
                                            {
                                                self.remove_deck_from_group(
                                                    deck.id,
                                                    group.id,
                                                    &group.name,
                                                );
                                                ui.close();
                                            }
                                        }
                                    });
                                }
                            }
                        });

                        if let Some(deck_id) = self.dragging_deck_id
                            && response.hovered()
                            && ui.input(|input| input.pointer.any_released())
                        {
                            let deck_ids = self.deck_drag_ids(deck_id);
                            self.add_decks_to_group(&deck_ids, group.id, &group.name);
                        }
                    }
                });
                ui.add_space(12.0);
            }
        }

        ui.separator();
        self.render_library_deck_section(ui, compact_cards);

        if ui.input(|input| input.pointer.any_released()) {
            self.dragging_deck_id = None;
            self.dragging_group_id = None;
        }
    }

    fn render_library_deck_section(&mut self, ui: &mut egui::Ui, compact_cards: bool) {
        ui.horizontal_wrapped(|ui| {
            ui.heading("题库");
            if !self.deck_cards.is_empty() {
                ui.small(format!("{} 个题库", self.deck_cards.len()));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let list_selected = self.library_deck_view == LibraryDeckViewMode::List;
                let cards_selected = self.library_deck_view == LibraryDeckViewMode::Cards;
                if ui.selectable_label(list_selected, "☰ 列表").clicked() {
                    self.library_deck_view = LibraryDeckViewMode::List;
                    self.persist_settings();
                }
                if ui.selectable_label(cards_selected, "▦ 小图标").clicked() {
                    self.library_deck_view = LibraryDeckViewMode::Cards;
                    self.persist_settings();
                }
            });
        });

        if self.deck_cards.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(60.0);
                ui.heading("暂无题库");
                ui.label("导入题库后，可在此开始练习、分析题目或查看学习诊断。");
            });
            return;
        }

        match self.library_deck_view {
            LibraryDeckViewMode::Cards => self.render_library_deck_cards(ui, compact_cards),
            LibraryDeckViewMode::List => self.render_library_deck_list(ui),
        }
    }

    fn render_library_deck_cards(&mut self, ui: &mut egui::Ui, compact_cards: bool) {
        let cards = self.deck_cards.clone();
        let columns = if self.ui_mode == AppUiMode::Mobile {
            mobile_library_card_columns(ui, cards.len())
        } else if compact_cards {
            1
        } else {
            grid_columns(ui, CARD_WIDTH)
        };
        let gap = if self.ui_mode == AppUiMode::Mobile {
            MOBILE_LIBRARY_CARD_GAP
        } else {
            12.0
        };
        let mobile_cell_width = if self.ui_mode == AppUiMode::Mobile {
            Some(
                ((ui.available_width() - gap * (columns.saturating_sub(1) as f32))
                    / columns as f32)
                    .max(MOBILE_LIBRARY_CARD_MIN_WIDTH),
            )
        } else {
            None
        };
        for row in cards.chunks(columns) {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for card in row {
                    let card = card.clone();
                    let (
                        response,
                        drag_response,
                        start_clicked,
                        preview_clicked,
                        analyze_clicked,
                        diagnose_clicked,
                        export_clicked,
                        selection_changed,
                    ) = if self.ui_mode == AppUiMode::Mobile {
                        let cell_width = mobile_cell_width.unwrap_or(ui.available_width());
                        ui.allocate_ui_with_layout(
                            egui::vec2(cell_width, 0.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                render_mobile_library_deck_card(
                                    ui,
                                    &card,
                                    self.is_deck_dragging(card.id),
                                    self.selected_deck_ids.contains(&card.id),
                                )
                            },
                        )
                        .inner
                    } else {
                        render_library_deck_card(
                            ui,
                            &card,
                            self.is_deck_dragging(card.id),
                            self.selected_deck_ids.contains(&card.id),
                        )
                    };
                    if selection_changed {
                        self.set_deck_selected(card.id, !self.selected_deck_ids.contains(&card.id));
                    }
                    if drag_response.drag_started() || drag_response.dragged() {
                        self.dragging_deck_id = Some(card.id);
                    }
                    self.apply_deck_card_actions(
                        &card,
                        start_clicked,
                        preview_clicked,
                        analyze_clicked,
                        diagnose_clicked,
                        export_clicked,
                    );
                    self.render_deck_context_menu(&response, &card);
                    if response.hovered() && ui.input(|input| input.pointer.any_released()) {
                        self.dragging_deck_id = None;
                    }
                }
            });
            ui.add_space(12.0);
        }
    }

    fn render_library_deck_list(&mut self, ui: &mut egui::Ui) {
        let cards = self.deck_cards.clone();
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.strong("题库");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.small("操作");
                    });
                });
                ui.separator();
                for card in cards {
                    self.render_library_deck_list_row(ui, card);
                    ui.separator();
                }
            });
    }

    fn render_library_deck_list_row(&mut self, ui: &mut egui::Ui, card: DeckCard) {
        let row_height = if self.ui_mode == AppUiMode::Mobile {
            104.0
        } else {
            58.0
        };
        let response = ui
            .allocate_ui_with_layout(
                egui::vec2(ui.available_width(), row_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    if self.ui_mode == AppUiMode::Mobile {
                        self.render_mobile_deck_list_row_content(ui, &card);
                    } else {
                        self.render_desktop_deck_list_row_content(ui, &card);
                    }
                },
            )
            .response
            .interact(egui::Sense::click_and_drag());
        if response.drag_started() || response.dragged() {
            self.dragging_deck_id = Some(card.id);
        }
        self.render_deck_context_menu(&response, &card);
        if response.hovered() && ui.input(|input| input.pointer.any_released()) {
            self.dragging_deck_id = None;
        }
    }

    fn render_desktop_deck_list_row_content(&mut self, ui: &mut egui::Ui, card: &DeckCard) {
        const LIST_ACTION_WIDTH: f32 = 230.0;
        const LIST_ACTION_GAP: f32 = 12.0;
        const LIST_CHECKBOX_WIDTH: f32 = 24.0;
        let available = ui.available_width();
        let action_width = LIST_ACTION_WIDTH.min(available * 0.46);
        let info_width =
            (available - LIST_CHECKBOX_WIDTH - action_width - LIST_ACTION_GAP).max(120.0);
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(LIST_CHECKBOX_WIDTH, ui.available_height()),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    let mut selected = self.selected_deck_ids.contains(&card.id);
                    if ui
                        .checkbox(&mut selected, "")
                        .on_hover_text("多选题库")
                        .changed()
                    {
                        self.set_deck_selected(card.id, selected);
                    }
                },
            );
            ui.allocate_ui_with_layout(
                egui::vec2(info_width, ui.available_height()),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.strong(&card.name);
                    ui.small(format!(
                        "{} 道题 · 新增 {} · 更新 {} · {} · {}",
                        card.problem_count,
                        card.inserted,
                        card.updated,
                        card.updated_at,
                        compact_middle(&card.source_path, 30)
                    ));
                },
            );
            ui.add_space(LIST_ACTION_GAP);
            ui.allocate_ui_with_layout(
                egui::vec2(action_width, ui.available_height()),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    let export_clicked = ui.small_button("导出").clicked();
                    let diagnose_clicked = ui.small_button("诊断").clicked();
                    let analyze_clicked = ui.small_button("分析").clicked();
                    let preview_clicked = ui.small_button("预览").clicked();
                    let start_clicked = ui.small_button("开始").clicked();
                    self.apply_deck_card_actions(
                        card,
                        start_clicked,
                        preview_clicked,
                        analyze_clicked,
                        diagnose_clicked,
                        export_clicked,
                    );
                },
            );
        });
    }

    fn render_mobile_deck_list_row_content(&mut self, ui: &mut egui::Ui, card: &DeckCard) {
        ui.horizontal(|ui| {
            let mut selected = self.selected_deck_ids.contains(&card.id);
            if ui
                .checkbox(&mut selected, "")
                .on_hover_text("多选题库")
                .changed()
            {
                self.set_deck_selected(card.id, selected);
            }
            ui.strong(&card.name);
        });
        ui.small(format!(
            "{} 道题 · 新增 {} · 更新 {}",
            card.problem_count, card.inserted, card.updated
        ));
        ui.small(format!(
            "{} · {}",
            card.updated_at,
            compact_middle(&card.source_path, 24)
        ));
        ui.horizontal_wrapped(|ui| {
            let start_clicked = ui.small_button("开始").clicked();
            let preview_clicked = ui.small_button("预览").clicked();
            let analyze_clicked = ui.small_button("分析").clicked();
            let diagnose_clicked = ui.small_button("诊断").clicked();
            let export_clicked = ui.small_button("导出").clicked();
            self.apply_deck_card_actions(
                card,
                start_clicked,
                preview_clicked,
                analyze_clicked,
                diagnose_clicked,
                export_clicked,
            );
        });
    }

    fn apply_deck_card_actions(
        &mut self,
        card: &DeckCard,
        start_clicked: bool,
        preview_clicked: bool,
        analyze_clicked: bool,
        diagnose_clicked: bool,
        export_clicked: bool,
    ) {
        if start_clicked {
            self.start_deck_card(card.id, card.name.clone());
        }
        if preview_clicked {
            self.preview_deck_card(card.id, card.name.clone());
        }
        if analyze_clicked {
            self.analyze_deck_problems(card.id, card.name.clone());
        }
        if diagnose_clicked {
            self.analyze_deck_learning_gaps(card.id, card.name.clone());
        }
        if export_clicked {
            self.export_deck_card(card.id, card.name.clone());
        }
    }

    fn render_deck_context_menu(&mut self, response: &egui::Response, card: &DeckCard) {
        response.context_menu(|ui| {
            if ui.button("导出题库").clicked() {
                self.export_deck_card(card.id, card.name.clone());
                ui.close();
            }
            if ui.button("分析题库题目").clicked() {
                self.analyze_deck_problems(card.id, card.name.clone());
                ui.close();
            }
            if ui.button("预览题库").clicked() {
                self.preview_deck_card(card.id, card.name.clone());
                ui.close();
            }
            if ui.button("分析答题薄弱点").clicked() {
                self.analyze_deck_learning_gaps(card.id, card.name.clone());
                ui.close();
            }
            ui.separator();
            if ui.button("从所有题组移出").clicked() {
                self.remove_deck_from_all_groups(card.id, &card.name);
                ui.close();
            }
        });
    }

    fn render_analysis_dialog(&mut self, ctx: &egui::Context) {
        let active_before_render = self.active_analysis_key.clone();
        let mut keys = self.analysis_dialogs.keys().cloned().collect::<Vec<_>>();
        if let Some(dialog) = &self.analysis_dialog {
            let key = AnalysisCacheKey {
                kind: dialog.kind,
                title: dialog.title.clone(),
            };
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        keys.sort_by(|left, right| {
            left.kind
                .cmp(&right.kind)
                .then_with(|| left.title.cmp(&right.title))
        });
        for key in keys {
            self.render_one_analysis_dialog(ctx, key);
        }
        if let Some(key) = active_before_render
            && self.active_analysis_key.as_ref() != Some(&key)
            && self.analysis_dialogs.contains_key(&key)
        {
            self.set_active_analysis_dialog(key);
        }
    }

    fn render_one_analysis_dialog(&mut self, ctx: &egui::Context, key: AnalysisCacheKey) {
        self.set_active_analysis_dialog(key.clone());
        let Some(dialog) = &self.analysis_dialog else {
            return;
        };
        let title = match dialog.kind {
            AnalysisKind::ProblemSet => format!("题目分析 - {}", dialog.title),
            AnalysisKind::LearningGap => format!("学习诊断 - {}", dialog.title),
        };
        let mut send_message = false;
        let mut new_dialog_requested = false;
        let mut knowledge_reanalysis_requested = false;
        let mut ai_review_check_requested = false;
        let mut should_close = false;
        let viewport_id = egui::ViewportId::from_hash_of(("shuaforge_analysis_dialog", &key));
        let window_index = self
            .analysis_dialogs
            .keys()
            .filter(|existing| *existing < &key)
            .count() as f32;
        let builder = egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size([660.0, 560.0])
            .with_position([80.0 + window_index * 28.0, 80.0 + window_index * 28.0])
            .with_min_inner_size([520.0, 420.0])
            .with_resizable(true);

        ctx.show_viewport_immediate(viewport_id, builder, |ctx, class| {
            if ctx.input(|input| input.viewport().close_requested()) {
                should_close = true;
            }

            let render_content = |ui: &mut egui::Ui| {
                if let Some(dialog) = &mut self.analysis_dialog {
                    ui.horizontal(|ui| {
                        ui.heading("对话");
                        if dialog.is_loading {
                            ui.spinner();
                            ui.label("生成中");
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add_enabled(!dialog.is_loading, egui::Button::new("新对话"))
                                .clicked()
                            {
                                new_dialog_requested = true;
                            }
                            if dialog.kind == AnalysisKind::ProblemSet
                                && ui
                                    .add_enabled(
                                        !dialog.is_loading,
                                        egui::Button::new("AI知识点重分析"),
                                    )
                                    .clicked()
                            {
                                knowledge_reanalysis_requested = true;
                            }
                            if dialog.kind == AnalysisKind::ProblemSet
                                && ui
                                    .add_enabled(
                                        !dialog.is_loading,
                                        egui::Button::new("AI总监核查"),
                                    )
                                    .clicked()
                            {
                                ai_review_check_requested = true;
                            }
                        });
                    });
                    render_analysis_progress_bar(ui, self.analysis_progress.as_ref());
                    ui.separator();

                    let input_area_height = 126.0;
                    let message_area_height =
                        (ui.available_height() - input_area_height).max(120.0);
                    let scroll_id = format!("analysis_chat_messages_{}", dialog.title);
                    egui::ScrollArea::vertical()
                        .id_salt(scroll_id)
                        .auto_shrink([false, false])
                        .max_height(message_area_height)
                        .min_scrolled_height(message_area_height)
                        .show(ui, |ui| {
                            for message in &dialog.messages {
                                render_chat_message(ui, message);
                                ui.add_space(8.0);
                            }
                        });

                    ui.separator();
                    let mut enter_to_send = false;
                    if render_analysis_input_bar(ui, dialog, &mut enter_to_send) {
                        send_message = true;
                    }
                    if enter_to_send {
                        send_message = true;
                    }
                }
            };

            match class {
                egui::ViewportClass::Embedded => {
                    egui::CentralPanel::default().show(ctx, render_content);
                }
                egui::ViewportClass::Root
                | egui::ViewportClass::Deferred
                | egui::ViewportClass::Immediate => {
                    egui::CentralPanel::default().show(ctx, render_content);
                }
            }
        });
        if let Some(dialog) = &mut self.analysis_dialog {
            dialog.open = !should_close;
            self.analysis_dialogs.insert(key.clone(), dialog.clone());
        }
        if should_close {
            self.close_analysis_dialog_by_key(&key);
        }
        if send_message {
            if self
                .analysis_dialog
                .as_ref()
                .is_some_and(|dialog| dialog.is_loading)
            {
                self.cancel_analysis_work();
            } else {
                self.send_analysis_chat_message();
            }
        }
        if new_dialog_requested {
            self.start_new_analysis_dialog();
        }
        if knowledge_reanalysis_requested {
            self.start_knowledge_point_reanalysis();
        }
        if ai_review_check_requested {
            self.start_ai_review_check();
        }
        if self
            .analysis_dialogs
            .get(&key)
            .is_some_and(|dialog| !dialog.open && !dialog.is_loading)
        {
            self.close_analysis_dialog_by_key(&key);
        }
    }

    fn close_analysis_dialog_by_key(&mut self, key: &AnalysisCacheKey) {
        let mut dialog = if self.active_analysis_key.as_ref() == Some(key) {
            self.analysis_dialog.take()
        } else {
            self.analysis_dialogs.remove(key)
        };
        if let Some(dialog) = dialog.as_mut() {
            dialog.open = false;
            if dialog.is_loading {
                dialog.is_loading = false;
                if let Some(assistant_message) = dialog.messages.iter_mut().rev().find(|message| {
                    message.role == ChatRole::Assistant
                        && (message.content == "正在分析，请稍候..."
                            || message.content == "正在回复，请稍候...")
                }) {
                    assistant_message.content = "此 AI 分析窗口已关闭。".into();
                }
            }
            let mut cached = dialog.clone();
            cached.open = false;
            self.analysis_cache.insert(key.clone(), cached);
            self.persist_analysis_cache();
        }
        self.analysis_dialogs.remove(key);
        if self.active_analysis_key.as_ref() == Some(key) {
            self.active_analysis_key = None;
            self.analysis_dialog = None;
        }
    }

    fn render_about_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_about {
            return;
        }

        let mut open = self.show_about;
        egui::Window::new("关于")
            .open(&mut open)
            .default_width(420.0)
            .default_height(560.0)
            .resizable(false)
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(8.0);
                    ui.heading("ShuaForge");
                    ui.label("轻量 Rust 桌面刷题助手");
                    ui.add_space(4.0);
                    ui.label(format!("v{}", env!("CARGO_PKG_VERSION")));
                    ui.small("作者：ShuaForge contributors");
                    ui.add_space(8.0);

                    if ui.button("GitHub 仓库").clicked() {
                        self.open_url("https://github.com/zhongbai2333/ShuaForge");
                    }
                    ui.small("许可证：MIT License");
                });

                ui.separator();
                ui.heading("日志");
                if let Some(path) = &self.log_path {
                    ui.small(format!("当前日志：{}", path.display()));
                } else {
                    ui.small("日志文件尚未成功初始化。");
                }
                if ui.button("导出日志").clicked() {
                    self.export_logs();
                }

                ui.separator();
                ui.heading("更新");
                ui.label(&self.update_status);
                if self.is_update_checking {
                    ui.add(
                        egui::ProgressBar::new(0.3)
                            .animate(true)
                            .desired_width(ui.available_width())
                            .text("正在检查更新..."),
                    );
                }
                if self.is_update_applying {
                    render_update_progress_bar(
                        ui,
                        self.update_downloaded_bytes,
                        self.update_total_bytes,
                    );
                }
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(
                            !self.is_update_checking && !self.is_update_applying,
                            egui::Button::new("检查更新"),
                        )
                        .clicked()
                    {
                        self.start_update_check(UpdateCheckSource::Manual);
                    }
                    if self.update_info.is_some()
                        && ui
                            .add_enabled(
                                !self.is_update_checking && !self.is_update_applying,
                                egui::Button::new("立即更新"),
                            )
                            .clicked()
                    {
                        self.start_apply_update();
                    }
                });

                ui.separator();
                ui.heading("开源许可");
                ui.small("以下为 ShuaForge 直接使用的第三方库及其包元数据声明的许可证。");
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .id_salt("about_license_scroll")
                    .max_height(250.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for notice in THIRD_PARTY_LICENSES {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(notice.name).strong());
                                ui.small(format!("({})", notice.license));
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui.small_button("查看").clicked() {
                                            self.open_url(notice.url);
                                        }
                                    },
                                );
                            });
                        }
                    });
            });

        self.show_about = open && self.show_about;
    }

    fn render_text_import_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_text_import_dialog {
            return;
        }

        let mut open = self.show_text_import_dialog;
        let mut should_import = false;
        let mut should_clear = false;
        egui::Window::new("粘贴导入题库")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(420.0)
            .show(ctx, |ui| {
                ui.label(
                    "将 JSON 或 CSV 题库内容粘贴到下方。适合手机端从聊天、文件或浏览器复制后导入。",
                );
                ui.add_space(8.0);
                ui.add(
                    egui::TextEdit::multiline(&mut self.text_import_buffer)
                        .hint_text("粘贴题库 JSON / CSV 文本…")
                        .desired_rows(12)
                        .desired_width(ui.available_width()),
                );
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    if ui.button("导入").clicked() {
                        should_import = true;
                    }
                    if ui.button("清空").clicked() {
                        should_clear = true;
                    }
                    if ui.button("取消").clicked() {
                        self.show_text_import_dialog = false;
                    }
                });
            });

        if should_clear {
            self.text_import_buffer.clear();
        }
        if should_import {
            self.import_problem_bank_text();
        } else {
            self.show_text_import_dialog = open && self.show_text_import_dialog;
        }
    }

    fn render_update_prompt(&mut self, ctx: &egui::Context) {
        if !self.show_update_prompt {
            return;
        }

        let Some(info) = self.update_info.clone() else {
            self.show_update_prompt = false;
            return;
        };
        let title = format!("发现新版本 {}", info.tag_name);
        egui::Window::new("发现更新")
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .show(ctx, |ui| {
                ui.heading(title);
                ui.label(format!("当前版本：{}", self_update::current_tag()));
                ui.label(format!("最新版本：{}", info.tag_name));
                if !info.release_name.trim().is_empty() {
                    ui.small(format!("Release：{}", info.release_name));
                }
                ui.small(format!("资产：{}", info.asset_name));
                if self.is_update_applying {
                    ui.add_space(8.0);
                    ui.label(&self.update_status);
                    render_update_progress_bar(
                        ui,
                        self.update_downloaded_bytes,
                        self.update_total_bytes,
                    );
                }
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(!self.is_update_applying, egui::Button::new("更新"))
                        .clicked()
                    {
                        self.start_apply_update();
                    }
                    if ui.button("此版本不再提醒").clicked() {
                        self.ignore_current_update();
                    }
                });
            });
    }

    fn send_analysis_chat_message(&mut self) {
        let (title, latest_result, input) = {
            let Some(dialog) = &mut self.analysis_dialog else {
                return;
            };
            if dialog.is_loading {
                return;
            }
            let input = dialog.input.trim().to_owned();
            if input.is_empty() {
                return;
            }
            dialog.input.clear();
            dialog.messages.push(ChatMessage {
                role: ChatRole::User,
                title: "用户".into(),
                content: input.clone(),
            });
            dialog.messages.push(ChatMessage {
                role: ChatRole::Assistant,
                title: "助手".into(),
                content: "正在回复，请稍候...".into(),
            });
            dialog.is_loading = true;
            (dialog.title.clone(), dialog.latest_result.clone(), input)
        };

        let generation_id = self.next_analysis_generation();
        let config = self.ai_config.clone();
        let (sender, receiver) = mpsc::channel();
        self.chat_receiver = Some(receiver);
        thread::spawn(move || {
            let message = match continue_analysis_chat(&config, &title, &latest_result, &input) {
                Ok(text) => text,
                Err(err) => format!("回复失败：{err}"),
            };
            let _ = sender.send(ChatAsyncResponse {
                generation_id,
                message,
            });
        });
    }
}

fn load_persisted_settings(
    store: &AppStore,
) -> Result<PersistedSettings, Box<dyn std::error::Error + Send + Sync>> {
    let Some(value) = store.get_setting(SETTINGS_KEY)? else {
        return Ok(PersistedSettings::default());
    };
    Ok(serde_json::from_str(&value)?)
}

fn compact_middle(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars {
        return value.to_owned();
    }
    if max_chars <= 3 {
        return "…".to_owned();
    }
    let head_len = (max_chars - 1) / 2;
    let tail_len = max_chars - 1 - head_len;
    let head: String = value.chars().take(head_len).collect();
    let tail: String = value
        .chars()
        .rev()
        .take(tail_len)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{head}…{tail}")
}

fn load_persisted_update_settings(
    store: &AppStore,
) -> Result<PersistedUpdateSettings, Box<dyn std::error::Error + Send + Sync>> {
    let Some(value) = store.get_setting(UPDATE_SETTINGS_KEY)? else {
        return Ok(PersistedUpdateSettings::default());
    };
    Ok(serde_json::from_str(&value)?)
}

fn browser_snapshot_import_meta(
    snapshot_json: &str,
    problems: &[Problem],
) -> BrowserSnapshotImportMeta {
    let value = serde_json::from_str::<serde_json::Value>(snapshot_json).ok();
    let bank_name = value
        .as_ref()
        .and_then(|value| value.pointer("/bank/name"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            problems
                .iter()
                .find_map(|problem| problem.deck_name.as_deref())
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(ToOwned::to_owned)
        })
        .or_else(|| {
            value
                .as_ref()
                .and_then(|value| value.get("title"))
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|title| !title.is_empty())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| "浏览器页面快照".into());
    let url = value
        .as_ref()
        .and_then(|value| value.get("url"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .unwrap_or_default();
    let fingerprint_seed = if url.is_empty() {
        format!("{bank_name}\n{snapshot_json}")
    } else {
        format!("{bank_name}\n{url}")
    };
    let hash = hex::encode(&Sha256::digest(fingerprint_seed.as_bytes())[..8]);
    let safe_name = safe_virtual_source_name(&bank_name);
    BrowserSnapshotImportMeta {
        source_path: PathBuf::from(format!("browser-snapshot-{safe_name}-{hash}.json")),
        director_title: format!("浏览器页面快照 · {bank_name}"),
    }
}

fn safe_virtual_source_name(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else if ch.is_whitespace()
                || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
            {
                '-'
            } else {
                ch
            }
        })
        .collect::<String>();
    let compact = sanitized
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let trimmed = compact.trim_matches('-');
    if trimmed.is_empty() {
        "browser".into()
    } else {
        trimmed.chars().take(48).collect()
    }
}

fn import_director_analysis_title(path: &Path, deck_name: Option<&str>) -> Option<String> {
    let source = path.to_string_lossy();
    if source == "浏览器页面快照" {
        return Some("AI总监审查 · 浏览器页面快照".into());
    }
    if source.starts_with("browser-snapshot-") {
        let name = deck_name
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or("浏览器页面快照");
        return Some(format!("AI总监审查 · 浏览器页面快照 · {name}"));
    }
    None
}

fn mobile_library_card_columns(ui: &egui::Ui, item_count: usize) -> usize {
    mobile_library_card_columns_for_width(ui.available_width(), item_count)
}

fn mobile_library_card_columns_for_width(available_width: f32, item_count: usize) -> usize {
    if item_count == 0 {
        return 1;
    }
    // Return the number of layout slots, not clamped to the item count. This keeps
    // a sparse final row (or a single card) card-sized instead of stretching it
    // across the full mobile/Pad width.
    ((available_width + MOBILE_LIBRARY_CARD_GAP)
        / (MOBILE_LIBRARY_CARD_TARGET_WIDTH + MOBILE_LIBRARY_CARD_GAP))
        .floor()
        .max(2.0) as usize
}

impl eframe::App for ShuaForgeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.view == AppView::Practice {
            let tab_pressed = suppress_practice_tab_focus(ctx);
            if tab_pressed || self.clear_practice_focus_next_frame {
                clear_egui_keyboard_focus(ctx);
            }
            self.clear_practice_focus_next_frame = tab_pressed;
        } else {
            self.clear_practice_focus_next_frame = false;
        }

        self.poll_ai();
        self.poll_ai_import();
        self.poll_browser_snapshot_import();
        #[cfg(target_os = "android")]
        self.poll_android_file_picker_import();
        self.poll_lan_sync();
        self.poll_update();
        if self.is_ai_loading {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if self
            .analysis_dialog
            .as_ref()
            .is_some_and(|dialog| dialog.is_loading)
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if self.is_update_checking || self.is_update_applying {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if self.browser_snapshot_receiver.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        if self.sync_receiver.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        if self.dragging_deck_id.is_some() || self.dragging_group_id.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
        self.render_analysis_dialog(ctx);
        self.render_deck_preview_dialog(ctx);
        self.render_about_dialog(ctx);
        self.render_text_import_dialog(ctx);
        self.render_update_prompt(ctx);

        match self.ui_mode {
            AppUiMode::Desktop => self.render_desktop_ui(ctx),
            AppUiMode::Mobile => self.render_mobile_ui(ctx),
        }
    }
}

fn problem_type_label(problem_type: ProblemType) -> &'static str {
    match problem_type {
        ProblemType::SingleChoice => "单选题",
        ProblemType::MultipleChoice => "多选题",
        ProblemType::Text => "文本题",
    }
}

fn problem_display_type_label(problem: &Problem) -> &'static str {
    if problem.is_judgement() {
        "判断题"
    } else {
        problem_type_label(problem.kind())
    }
}

fn ai_review_action_summary(problems: &[Problem]) -> String {
    let mut accepted = 0usize;
    let mut pending = 0usize;
    let mut conflict = 0usize;
    for problem in problems {
        if problem.state.answer_source == ProblemAnswerSource::AiReviewed
            && problem.state.review_status == crate::problem::ProblemReviewStatus::Accepted
        {
            accepted += 1;
        }
        if problem.needs_ai_review() {
            pending += 1;
        }
        if problem.state.review_status == crate::problem::ProblemReviewStatus::Conflict {
            conflict += 1;
        }
    }
    format!("自动采纳 {accepted}，仍待确认 {pending}，冲突 {conflict}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::problem::ProblemState;

    fn test_problem(id: &str, deck_name: &str) -> Problem {
        Problem {
            id: id.into(),
            prompt: format!("题目 {id}"),
            answer: "A".into(),
            explanation: String::new(),
            tags: vec![],
            problem_type: Some(ProblemType::SingleChoice),
            deck_name: Some(deck_name.into()),
            deck_info: None,
            images: vec![],
            state: ProblemState::default(),
        }
    }

    #[test]
    fn browser_snapshot_source_path_is_stable_and_distinguishes_decks() {
        let first_snapshot = r#"{
            "url": "https://example.test/homework/1",
            "title": "作业详情",
            "bank": { "name": "第一章", "info": "" }
        }"#;
        let second_snapshot = r#"{
            "url": "https://example.test/homework/2",
            "title": "作业详情",
            "bank": { "name": "第二章", "info": "" }
        }"#;

        let first = browser_snapshot_import_meta(first_snapshot, &[test_problem("p1", "第一章")]);
        let first_again =
            browser_snapshot_import_meta(first_snapshot, &[test_problem("p1", "第一章")]);
        let second = browser_snapshot_import_meta(second_snapshot, &[test_problem("p2", "第二章")]);

        assert_eq!(first.source_path, first_again.source_path);
        assert_ne!(first.source_path, second.source_path);
        assert_eq!(first.director_title, "浏览器页面快照 · 第一章");
        assert_eq!(second.director_title, "浏览器页面快照 · 第二章");
    }

    #[test]
    fn mobile_library_card_columns_are_adaptive_with_two_column_minimum() {
        assert_eq!(mobile_library_card_columns_for_width(320.0, 1), 2);
        assert_eq!(mobile_library_card_columns_for_width(320.0, 8), 2);
        assert_eq!(mobile_library_card_columns_for_width(760.0, 8), 3);
        assert_eq!(mobile_library_card_columns_for_width(1200.0, 3), 5);
    }

    #[test]
    fn browser_snapshot_import_title_uses_deck_name_for_new_virtual_sources() {
        let path = PathBuf::from("browser-snapshot-第一章-abcdef.json");

        assert_eq!(
            import_director_analysis_title(&path, Some("第一章")).as_deref(),
            Some("AI总监审查 · 浏览器页面快照 · 第一章")
        );
        assert_eq!(
            import_director_analysis_title(&PathBuf::from("浏览器页面快照"), None).as_deref(),
            Some("AI总监审查 · 浏览器页面快照")
        );
    }
}

use crate::{
    ai::{
        AiConfig, AnalysisStreamEvent, AnalysisToolResult, call_learning_gap_analysis_tool,
        continue_analysis_chat, explain_wrong_answer, load_ai_config, review_answer,
        save_ai_config, stream_problem_set_analysis_tool,
    },
    deck::{PracticeDeck, PracticeOrder, SubmitResult},
    problem::{Problem, ProblemType, load_problems, normalize_choice_answer},
    self_update::{self, UpdateInfo, UpdateOutcome},
    store::{AnswerRecord, AppStore, DeckCard, GroupCard},
    userscript_server,
};
use eframe::egui::{self, FontData, FontDefinitions, FontFamily};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::{path::PathBuf, sync::mpsc, thread};

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppView {
    Library,
    Practice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AnalysisKind {
    ProblemSet,
    LearningGap,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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

#[derive(Debug, Clone)]
enum UpdateAsyncResponse {
    CheckFinished(Result<Option<UpdateInfo>, String>),
    ApplyFinished(Result<UpdateOutcome, String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UpdateCheckSource {
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
            AnalysisKind::ProblemSet => "请分析这组题目的题型、难度和知识点。",
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
                    content: "正在分析，请稍候...".into(),
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
}

impl Default for PersistedSettings {
    fn default() -> Self {
        Self {
            theme: PersistedTheme::Dark,
            practice_order: PersistedPracticeOrder::Shuffled,
            ai_config: AiConfig::default(),
        }
    }
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

const SETTINGS_KEY: &str = "app_settings";
const UPDATE_SETTINGS_KEY: &str = "update_settings";
const ANALYSIS_CACHE_KEY: &str = "analysis_dialog_cache";
const ANALYSIS_TEXT_CACHE_KEY: &str = "analysis_text_cache";
const CARD_WIDTH: f32 = 300.0;
const GROUP_CARD_HEIGHT: f32 = 158.0;
const DECK_CARD_HEIGHT: f32 = 188.0;
const CARD_BUTTON_WIDTH: f32 = 76.0;

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
    deck: Option<PracticeDeck>,
    view: AppView,
    store: Option<AppStore>,
    ai_config: AiConfig,
    answer_input: String,
    selected_single: Option<String>,
    selected_multiple: BTreeSet<String>,
    status: String,
    analysis: String,
    loaded_path: Option<PathBuf>,
    ai_config_path: Option<PathBuf>,
    ai_receiver: Option<mpsc::Receiver<String>>,
    analysis_receiver: Option<mpsc::Receiver<AnalysisAsyncResponse>>,
    analysis_stream_receiver: Option<mpsc::Receiver<AnalysisStreamResponse>>,
    chat_receiver: Option<mpsc::Receiver<ChatAsyncResponse>>,
    update_receiver: Option<mpsc::Receiver<UpdateAsyncResponse>>,
    update_info: Option<UpdateInfo>,
    update_status: String,
    is_update_checking: bool,
    is_update_applying: bool,
    update_check_source: Option<UpdateCheckSource>,
    show_update_prompt: bool,
    ignored_update_tag: Option<String>,
    analysis_generation_id: u64,
    analysis_dialog: Option<AnalysisDialogState>,
    analysis_cache: HashMap<AnalysisCacheKey, AnalysisDialogState>,
    analysis_sources: HashMap<AnalysisCacheKey, AnalysisSource>,
    is_ai_loading: bool,
    theme: AppTheme,
    practice_order: PracticeOrder,
    deck_cards: Vec<DeckCard>,
    group_cards: Vec<GroupCard>,
    active_deck_name: Option<String>,
    answer_history: Vec<AnswerRecord>,
    bank_count: usize,
    show_about: bool,
    new_group_name: String,
    dragging_deck_id: Option<i64>,
    dragging_group_id: Option<i64>,
}

impl ShuaForgeApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        configure_fonts(&cc.egui_ctx);
        cc.egui_ctx.set_visuals(egui::Visuals::dark());

        let mut status = "请导入题库开始练习。支持 JSON / CSV / ZIP 格式。".to_owned();
        let store = match AppStore::open_default() {
            Ok(store) => Some(store),
            Err(err) => {
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
        let update_settings = store
            .as_ref()
            .and_then(|store| load_persisted_update_settings(store).ok())
            .unwrap_or_default();
        cc.egui_ctx.set_visuals(theme.visuals());

        let mut app = Self {
            deck: None,
            view: AppView::Library,
            store,
            ai_config: settings.ai_config,
            answer_input: String::new(),
            selected_single: None,
            selected_multiple: BTreeSet::new(),
            status,
            analysis: "解析与学习分析结果将在这里显示。".into(),
            loaded_path: None,
            ai_config_path: None,
            ai_receiver: None,
            analysis_receiver: None,
            analysis_stream_receiver: None,
            chat_receiver: None,
            update_receiver: None,
            update_info: None,
            update_status: "尚未检查更新。".into(),
            is_update_checking: false,
            is_update_applying: false,
            update_check_source: None,
            show_update_prompt: false,
            ignored_update_tag: update_settings.ignored_tag,
            analysis_generation_id: 0,
            analysis_dialog: None,
            analysis_cache: HashMap::new(),
            analysis_sources: HashMap::new(),
            is_ai_loading: false,
            theme,
            practice_order,
            deck_cards: Vec::new(),
            group_cards: Vec::new(),
            active_deck_name: None,
            answer_history: Vec::new(),
            bank_count: 0,
            show_about: false,
            new_group_name: "新题组".into(),
            dragging_deck_id: None,
            dragging_group_id: None,
        };
        app.load_analysis_cache();
        app.load_analysis_text_cache();
        app.refresh_store_state();
        app.start_update_check(UpdateCheckSource::Startup);
        app
    }

    fn toggle_theme(&mut self, ctx: &egui::Context) {
        self.theme = match self.theme {
            AppTheme::Dark => AppTheme::Light,
            AppTheme::Light => AppTheme::Dark,
        };
        ctx.set_visuals(self.theme.visuals());
        self.persist_settings();
    }

    fn current_settings(&self) -> PersistedSettings {
        PersistedSettings {
            theme: self.theme.into(),
            practice_order: self.practice_order.into(),
            ai_config: self.ai_config.clone(),
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
            self.status = format!("打开链接失败：{err}");
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
        self.update_status = "正在下载并应用更新...".into();
        let (sender, receiver) = mpsc::channel();
        self.update_receiver = Some(receiver);
        thread::spawn(move || {
            let result = self_update::perform_update(None).map_err(|err| err.to_string());
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
        let Ok(response) = receiver.try_recv() else {
            return;
        };
        self.update_receiver = None;
        match response {
            UpdateAsyncResponse::CheckFinished(result) => {
                self.is_update_checking = false;
                let source = self
                    .update_check_source
                    .take()
                    .unwrap_or(UpdateCheckSource::Manual);
                match result {
                    Ok(Some(info)) => {
                        let ignored = self.ignored_update_tag.as_deref() == Some(&info.tag_name);
                        self.update_status = format!(
                            "发现新版本 {}（当前 {}）。",
                            info.tag_name,
                            self_update::current_tag()
                        );
                        self.update_info = Some(info);
                        self.show_update_prompt = !ignored || source == UpdateCheckSource::Manual;
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
            UpdateAsyncResponse::ApplyFinished(result) => {
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

    fn import_problem_bank(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("题库", &["json", "csv", "zip"])
            .pick_file()
        {
            match load_problems(&path) {
                Ok(problems) => self.import_and_load_problems(path, problems),
                Err(err) => self.status = format!("导入失败：{err}"),
            }
        }
    }

    fn import_and_load_problems(&mut self, path: PathBuf, problems: Vec<Problem>) {
        if let Some(store) = self.store.as_mut() {
            match store.import_problems(&problems, &path.display().to_string()) {
                Ok(summary) => {
                    self.status = format!(
                        "导入 {} 道题：新增 {}，更新 {}。已保存到题库列表。",
                        summary.imported, summary.inserted, summary.updated
                    );
                    self.active_deck_name = None;
                }
                Err(err) => self.status = format!("题库已读入，但保存 SQLite 失败：{err}"),
            }
            self.refresh_store_state();
        }
        self.loaded_path = Some(path);
        self.deck = None;
        self.view = AppView::Library;
    }

    fn load_bank_from_store(&mut self) {
        let Some(store) = &self.store else { return };
        match store.load_all_problems() {
            Ok(problems) if !problems.is_empty() => {
                let count = problems.len();
                self.deck = Some(PracticeDeck::with_order(problems, self.practice_order));
                self.active_deck_name = Some("全部题库".into());
                self.view = AppView::Practice;
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
    }

    fn restart_from_store(&mut self) {
        self.load_bank_from_store();
        self.loaded_path = None;
        self.active_deck_name = Some("全部题库".into());
        self.clear_answer_inputs();
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
                self.deck = Some(PracticeDeck::with_order(problems, self.practice_order));
                self.active_deck_name = Some(deck_name.clone());
                self.loaded_path = None;
                self.view = AppView::Practice;
                self.clear_answer_inputs();
                self.analysis.clear();
                self.status = format!("已开始题库「{deck_name}」，共 {count} 道题。");
            }
            Ok(_) => self.status = format!("题库「{deck_name}」为空。"),
            Err(err) => self.status = format!("读取题库失败：{err}"),
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
                self.deck = Some(PracticeDeck::with_order(problems, self.practice_order));
                self.active_deck_name = Some(format!("题组：{group_name}"));
                self.loaded_path = None;
                self.view = AppView::Practice;
                self.clear_answer_inputs();
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
        let Some(store) = self.store.as_mut() else {
            self.status = "本地数据库不可用。".into();
            return;
        };

        match store.delete_deck(deck_id) {
            Ok(()) => {
                self.status = format!("已删除题库「{deck_name}」。");
                self.dragging_deck_id = None;
                self.dragging_group_id = None;
                self.refresh_store_state();
            }
            Err(err) => self.status = format!("删除题库失败：{err}"),
        }
    }

    fn delete_group(&mut self, group_id: i64, group_name: &str) {
        let Some(store) = &self.store else {
            self.status = "本地数据库不可用。".into();
            return;
        };

        match store.delete_group(group_id) {
            Ok(()) => {
                self.status = format!("已删除题组「{group_name}」。");
                self.dragging_group_id = None;
                self.dragging_deck_id = None;
                self.refresh_store_state();
            }
            Err(err) => self.status = format!("删除题组失败：{err}"),
        }
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
        self.view = AppView::Library;
        self.deck = None;
        self.active_deck_name = None;
        self.clear_answer_inputs();
        self.analysis.clear();
        self.refresh_store_state();
    }

    fn clear_answer_inputs(&mut self) {
        self.answer_input.clear();
        self.selected_single = None;
        self.selected_multiple.clear();
    }

    fn load_ai_config(&mut self) {
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
        let path = self.ai_config_path.clone().or_else(|| {
            rfd::FileDialog::new()
                .add_filter("AI 配置", &["json"])
                .set_file_name("ai-config.json")
                .save_file()
        });

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
            deck.skip();
            self.record_answer(&problem, &user_answer, false);
            self.status = "该题没有标准答案，已交给 AI 批改，并放回队尾继续复习。".into();
            self.analysis = "AI 批改中...".into();
            self.request_ai_review(problem, user_answer);
            self.clear_answer_inputs();
            self.refresh_store_state();
            return;
        }

        match deck.submit(&user_answer) {
            SubmitResult::Correct => {
                self.record_answer(&problem, &user_answer, true);
                self.status = "回答正确，已从题库移除。".into();
                self.analysis = "回答正确，本题已从当前练习队列移除。".into();
            }
            SubmitResult::Wrong {
                expected,
                explanation,
            } => {
                self.record_answer(&problem, &user_answer, false);
                self.status = format!("回答错误，已重新加入题库。标准答案：{expected}");
                self.analysis = if explanation.trim().is_empty() {
                    "正在准备解析...".into()
                } else {
                    explanation
                };
                self.request_ai_analysis(problem, user_answer);
            }
            SubmitResult::NoCurrentProblem => self.status = "当前没有题目。".into(),
        }

        self.clear_answer_inputs();
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

    fn request_ai_review(&mut self, problem: Problem, user_answer: String) {
        let config = self.ai_config.clone();
        let (sender, receiver) = mpsc::channel();
        self.ai_receiver = Some(receiver);
        self.is_ai_loading = true;

        thread::spawn(move || {
            let message = match review_answer(&config, &problem, &user_answer) {
                Ok(text) => text,
                Err(err) => format!("AI 批改失败：{err}"),
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
        if let Some(dialog) = self.analysis_cache.get(&key).cloned() {
            self.analysis_dialog = Some(AnalysisDialogState {
                open: true,
                ..dialog
            });
            self.status = format!("已打开{title}的分析对话。");
            return;
        }
        self.start_problem_set_analysis(title, problems);
    }

    fn start_problem_set_analysis(&mut self, title: String, problems: Vec<Problem>) {
        let generation_id = self.next_analysis_generation();
        let config = self.ai_config.clone();
        let (sender, receiver) = mpsc::channel();
        self.analysis_stream_receiver = Some(receiver);
        self.analysis_dialog = Some(AnalysisDialogState::loading(
            title.clone(),
            AnalysisKind::ProblemSet,
            "shuaforge.analyze_problem_set",
            serde_json::json!({
                "title": title,
                "problem_count": problems.len(),
                "data_access": "tool_paging",
                "available_tools": ["shuaforge.list_problems", "shuaforge.get_problem"],
                "analysis_dimensions": ["problem_type", "difficulty", "knowledge_points", "practice_order"]
            })
            .to_string(),
        ));
        self.status = format!("正在分析{title}...");
        self.analysis = "正在生成题目分析，请稍候...".into();

        thread::spawn(move || {
            let (event_sender, event_receiver) = mpsc::channel();
            stream_problem_set_analysis_tool(config, title, problems, event_sender);
            for event in event_receiver {
                let finished = matches!(event, AnalysisStreamEvent::Finished);
                let _ = sender.send(AnalysisStreamResponse {
                    generation_id,
                    event,
                });
                if finished {
                    break;
                }
            }
        });
    }

    fn request_learning_gap_analysis(&mut self, title: String, records: Vec<AnswerRecord>) {
        let key = AnalysisCacheKey {
            kind: AnalysisKind::LearningGap,
            title: title.clone(),
        };
        self.analysis_sources
            .insert(key.clone(), AnalysisSource::Records(records.clone()));
        if let Some(dialog) = self.analysis_cache.get(&key).cloned() {
            self.analysis_dialog = Some(AnalysisDialogState {
                open: true,
                ..dialog
            });
            self.status = format!("已打开{title}的学习诊断对话。");
            return;
        }
        self.start_learning_gap_analysis(title, records);
    }

    fn start_learning_gap_analysis(&mut self, title: String, records: Vec<AnswerRecord>) {
        let generation_id = self.next_analysis_generation();
        let config = self.ai_config.clone();
        let (sender, receiver) = mpsc::channel();
        self.analysis_receiver = Some(receiver);
        self.analysis_dialog = Some(AnalysisDialogState::loading(
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
        ));
        self.status = format!("正在分析{title}的答题表现...");
        self.analysis = "正在生成学习诊断，请稍候...".into();

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
                AnalysisStreamEvent::ToolCall { arguments_json } => {
                    if let Some(dialog) = &mut self.analysis_dialog
                        && let Some(tool_message) = dialog
                            .messages
                            .iter_mut()
                            .find(|message| message.role == ChatRole::Tool)
                    {
                        tool_message.title = "Tool Call · shuaforge.analyze_problem_set".into();
                        tool_message.content = arguments_json;
                    }
                }
                AnalysisStreamEvent::TextDelta(delta) => {
                    self.analysis.push_str(&delta);
                    if let Some(dialog) = &mut self.analysis_dialog {
                        if let Some(assistant_message) = dialog
                            .messages
                            .iter_mut()
                            .rev()
                            .find(|message| message.role == ChatRole::Assistant)
                        {
                            if assistant_message.content == "正在分析，请稍候..." {
                                assistant_message.content.clear();
                            }
                            assistant_message.content.push_str(&delta);
                        }
                        dialog.latest_result = self.analysis.clone();
                    }
                    self.persist_analysis_text();
                }
                AnalysisStreamEvent::Finished => {
                    self.status = "分析完成。".into();
                    if let Some(dialog) = &mut self.analysis_dialog {
                        dialog.is_loading = false;
                    }
                    self.persist_analysis_text();
                    self.cache_current_analysis_dialog();
                    clear_stream_receiver = true;
                }
                AnalysisStreamEvent::Failed(reason) => {
                    self.status = format!("AI 分析失败，已尝试回退：{reason}");
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
            self.chat_receiver = None;
        }
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
        ui.horizontal_wrapped(|ui| {
            ui.heading("题库主页");
            ui.label("集中管理题库与题组");
        });
        ui.label("可将题库加入题组，按章节、课程或专题组织练习内容。");
        ui.add_space(10.0);

        ui.horizontal_wrapped(|ui| {
            if ui.button("导入新题库").clicked() {
                self.import_problem_bank();
            }
            if ui.button("练习全部题库").clicked() {
                self.restart_from_store();
            }
            ui.separator();
            ui.label("新题组");
            ui.text_edit_singleline(&mut self.new_group_name);
            if ui.button("创建题组").clicked() {
                self.create_group();
            }
        });

        ui.add_space(10.0);
        let trash_response = render_trash_zone(
            ui,
            self.dragging_deck_id.is_some() || self.dragging_group_id.is_some(),
        );
        if trash_response.hovered() && ui.input(|input| input.pointer.any_released()) {
            if let Some(deck_id) = self.dragging_deck_id.take()
                && let Some(card) = self
                    .deck_cards
                    .iter()
                    .find(|card| card.id == deck_id)
                    .cloned()
            {
                self.delete_deck(card.id, &card.name);
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
            let columns = grid_columns(ui, CARD_WIDTH);
            egui::Grid::new("group_bookshelf_grid")
                .num_columns(columns)
                .spacing(egui::vec2(12.0, 12.0))
                .show(ui, |ui| {
                    for (index, group) in groups.into_iter().enumerate() {
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
                            remove_requests,
                        ) = render_group_card(
                            ui,
                            &group,
                            &group_decks,
                            self.dragging_deck_id.is_some(),
                            self.dragging_group_id == Some(group.id),
                        );
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
                        for deck_id in remove_requests {
                            self.remove_deck_from_group(deck_id, group.id, &group.name);
                        }
                        response.context_menu(|ui| {
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
                            self.add_deck_to_group(deck_id, group.id, &group.name);
                        }
                        if (index + 1) % columns == 0 {
                            ui.end_row();
                        }
                    }
                });
        }

        ui.separator();
        ui.heading("题库");
        if self.deck_cards.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(60.0);
                ui.heading("暂无题库");
                ui.label("导入题库后，可在此开始练习、分析题目或查看学习诊断。");
            });
        } else {
            let cards = self.deck_cards.clone();
            let columns = grid_columns(ui, CARD_WIDTH);
            egui::Grid::new("deck_bookshelf_grid")
                .num_columns(columns)
                .spacing(egui::vec2(12.0, 12.0))
                .show(ui, |ui| {
                    for (index, card) in cards.into_iter().enumerate() {
                        let (
                            response,
                            drag_response,
                            start_clicked,
                            analyze_clicked,
                            diagnose_clicked,
                        ) = render_library_deck_card(
                            ui,
                            &card,
                            self.dragging_deck_id == Some(card.id),
                        );
                        if drag_response.drag_started() || drag_response.dragged() {
                            self.dragging_deck_id = Some(card.id);
                        }
                        if start_clicked {
                            self.start_deck_card(card.id, card.name.clone());
                        }
                        if analyze_clicked {
                            self.analyze_deck_problems(card.id, card.name.clone());
                        }
                        if diagnose_clicked {
                            self.analyze_deck_learning_gaps(card.id, card.name.clone());
                        }
                        response.context_menu(|ui| {
                            if ui.button("分析题库题目").clicked() {
                                self.analyze_deck_problems(card.id, card.name.clone());
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
                        if response.hovered() && ui.input(|input| input.pointer.any_released()) {
                            self.dragging_deck_id = None;
                        }
                        if (index + 1) % columns == 0 {
                            ui.end_row();
                        }
                    }
                });
        }

        if ui.input(|input| input.pointer.any_released()) {
            self.dragging_deck_id = None;
            self.dragging_group_id = None;
        }
    }

    fn render_analysis_dialog(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &self.analysis_dialog else {
            return;
        };
        let title = match dialog.kind {
            AnalysisKind::ProblemSet => format!("题目分析 - {}", dialog.title),
            AnalysisKind::LearningGap => format!("学习诊断 - {}", dialog.title),
        };
        let mut send_message = false;
        let mut new_dialog_requested = false;
        let mut should_close = false;
        let viewport_id = egui::ViewportId::from_hash_of("shuaforge_analysis_dialog");
        let builder = egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size([660.0, 560.0])
            .with_min_inner_size([520.0, 420.0])
            .with_resizable(true);

        ctx.show_viewport_immediate(viewport_id, builder, |ctx, class| {
            if ctx.input(|input| input.viewport().close_requested()) {
                should_close = true;
            }

            let mut render_content = |ui: &mut egui::Ui| {
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
                        });
                    });
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
                    egui::Window::new("分析对话")
                        .resizable(true)
                        .default_width(660.0)
                        .default_height(560.0)
                        .show(ctx, &mut render_content);
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
        }
        if should_close {
            if self
                .analysis_dialog
                .as_ref()
                .is_some_and(|dialog| dialog.is_loading)
            {
                self.cancel_analysis_work();
            } else {
                self.cache_current_analysis_dialog();
            }
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
        if let Some(dialog) = &self.analysis_dialog
            && !dialog.open
            && !dialog.is_loading
        {
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
                ui.heading("更新");
                ui.label(&self.update_status);
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

fn render_analysis_input_bar(
    ui: &mut egui::Ui,
    dialog: &mut AnalysisDialogState,
    enter_to_send: &mut bool,
) -> bool {
    let mut send_clicked = false;
    ui.horizontal(|ui| {
        ui.strong("继续追问");
        ui.small("Ctrl + Enter 发送");
    });
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .stroke(egui::Stroke::new(
            1.0,
            ui.visuals().widgets.noninteractive.bg_stroke.color,
        ))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let button_width = 64.0;
                let input_width =
                    (ui.available_width() - button_width - 8.0).clamp(80.0, ui.available_width());
                let response = ui.add(
                    egui::TextEdit::multiline(&mut dialog.input)
                        .desired_width(input_width)
                        .desired_rows(2)
                        .hint_text("追问分析细节、生成复习计划或让 AI 换一种说法…"),
                );
                *enter_to_send = response.has_focus()
                    && ui
                        .input(|input| input.modifiers.ctrl && input.key_pressed(egui::Key::Enter));
                let send_label = if dialog.is_loading {
                    "取消"
                } else {
                    "发送"
                };
                let send_button =
                    egui::Button::new(send_label).min_size(egui::vec2(button_width, 42.0));
                if ui.add_sized([button_width, 42.0], send_button).clicked() {
                    send_clicked = true;
                }
            });
        });
    ui.small("AI 会基于上方分析结果继续回答，不会重新导入题库。");
    send_clicked
}

fn render_chat_message(ui: &mut egui::Ui, message: &ChatMessage) {
    let (label, accent, fill) = match message.role {
        ChatRole::User => (
            "用户",
            egui::Color32::from_rgb(120, 170, 255),
            egui::Color32::from_rgb(38, 68, 118),
        ),
        ChatRole::Assistant => (
            "助手",
            egui::Color32::from_rgb(150, 210, 160),
            ui.visuals().faint_bg_color,
        ),
        ChatRole::Tool => (
            "工具调用",
            egui::Color32::from_rgb(220, 180, 110),
            egui::Color32::from_rgb(45, 43, 38),
        ),
    };
    let available = ui.available_width().max(1.0);
    let max_bubble_width = (available * 0.86).clamp(160.0, 820.0).min(available - 16.0);
    let bubble_width = adaptive_chat_bubble_width(message, max_bubble_width);
    let layout = if message.role == ChatRole::User {
        egui::Layout::right_to_left(egui::Align::Min)
    } else {
        egui::Layout::left_to_right(egui::Align::Min)
    };
    ui.with_layout(layout, |ui| {
        egui::Frame::new()
            .fill(fill)
            .stroke(egui::Stroke::new(1.0, accent.gamma_multiply(0.55)))
            .corner_radius(egui::CornerRadius::same(12))
            .inner_margin(egui::Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.set_min_width(0.0);
                ui.set_max_width(bubble_width);
                ui.set_width(bubble_width);
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.set_width(bubble_width);
                    let header = if message.title == label {
                        label.to_owned()
                    } else {
                        format!("{label} · {}", message.title)
                    };
                    ui.colored_label(accent, header);
                    ui.add_space(3.0);
                    if message.role == ChatRole::Tool {
                        ui.collapsing("参数", |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt(("tool_message", &message.title))
                                .max_height(120.0)
                                .show(ui, |ui| {
                                    ui.monospace(&message.content);
                                });
                        });
                    } else {
                        ui.set_width((bubble_width - 24.0).max(80.0));
                        if message.role == ChatRole::Assistant {
                            render_markdown_text(
                                ui,
                                &message.content,
                                (bubble_width - 24.0).max(80.0),
                            );
                        } else {
                            ui.add(egui::Label::new(&message.content).wrap());
                        }
                    }
                });
            });
    });
}

fn adaptive_chat_bubble_width(message: &ChatMessage, max_width: f32) -> f32 {
    let header_chars = message.title.chars().count() + 8;
    let longest_line_chars = message
        .content
        .lines()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0)
        .max(header_chars);
    let visible_chars = message.content.chars().count().max(longest_line_chars);
    let estimated_char_width = match message.role {
        ChatRole::Tool => 8.5,
        ChatRole::User | ChatRole::Assistant => 13.0,
    };
    let ideal_width = longest_line_chars as f32 * estimated_char_width + 42.0;
    let multiline_width = if visible_chars > 120 {
        max_width
    } else {
        ideal_width
    };
    multiline_width.clamp(128.0, max_width)
}

fn render_markdown_text(ui: &mut egui::Ui, text: &str, content_width: f32) {
    let normalized = normalize_display_symbols(text);
    let lines = normalized.lines().collect::<Vec<_>>();
    let mut index = 0;
    while index < lines.len() {
        if let Some((table, consumed)) = parse_markdown_table(&lines[index..]) {
            render_markdown_table(ui, &table, content_width);
            index += consumed;
            continue;
        }

        let raw_line = lines[index];
        index += 1;
        let line = raw_line.trim_end();
        let trimmed = line.trim();
        if trimmed.is_empty() {
            ui.add_space(6.0);
            continue;
        }
        if trimmed == "---" || trimmed == "***" {
            ui.separator();
            continue;
        }

        if let Some(heading) = trimmed.strip_prefix("#### ") {
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(strip_markdown_inline(heading))
                    .strong()
                    .size(16.0),
            );
            continue;
        }
        if let Some(heading) = trimmed.strip_prefix("### ") {
            ui.add_space(3.0);
            ui.label(
                egui::RichText::new(strip_markdown_inline(heading))
                    .strong()
                    .size(18.0),
            );
            continue;
        }
        if let Some(heading) = trimmed.strip_prefix("## ") {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(strip_markdown_inline(heading))
                    .strong()
                    .size(20.0),
            );
            continue;
        }
        if let Some(heading) = trimmed.strip_prefix("# ") {
            ui.add_space(5.0);
            ui.label(
                egui::RichText::new(strip_markdown_inline(heading))
                    .strong()
                    .size(22.0),
            );
            continue;
        }

        if let Some(item) = markdown_list_item(trimmed) {
            ui.horizontal_wrapped(|ui| {
                ui.label("•");
                render_inline_markdown(ui, item);
            });
            continue;
        }

        render_inline_markdown(ui, trimmed);
    }
}

fn parse_markdown_table(lines: &[&str]) -> Option<(Vec<Vec<String>>, usize)> {
    if lines.len() < 2 {
        return None;
    }
    let header = parse_table_row(lines[0])?;
    if header.len() < 2 || !is_table_separator(lines[1], header.len()) {
        return None;
    }

    let mut rows = vec![header];
    let mut consumed = 2;
    for line in lines.iter().skip(2) {
        let Some(row) = parse_table_row(line) else {
            break;
        };
        if row.len() < 2 {
            break;
        }
        rows.push(row);
        consumed += 1;
    }

    Some((rows, consumed))
}

fn parse_table_row(line: &str) -> Option<Vec<String>> {
    let trimmed = line.trim();
    if !trimmed.contains('|') {
        return None;
    }
    let trimmed = trimmed.trim_matches('|');
    let cells = trimmed
        .split('|')
        .map(|cell| cell.trim().to_owned())
        .collect::<Vec<_>>();
    (cells.len() >= 2).then_some(cells)
}

fn is_table_separator(line: &str, min_columns: usize) -> bool {
    let Some(cells) = parse_table_row(line) else {
        return false;
    };
    cells.len() >= min_columns
        && cells.iter().all(|cell| {
            let cell = cell.trim();
            !cell.is_empty() && cell.chars().all(|ch| ch == '-' || ch == ':' || ch == ' ')
        })
}

fn render_markdown_table(ui: &mut egui::Ui, rows: &[Vec<String>], content_width: f32) {
    ui.add_space(4.0);
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color.gamma_multiply(0.55))
        .stroke(egui::Stroke::new(
            1.0,
            ui.visuals().widgets.noninteractive.bg_stroke.color,
        ))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(content_width);
            ui.set_min_width(content_width);
            egui::ScrollArea::horizontal()
                .id_salt((
                    "markdown_table",
                    rows.len(),
                    rows.first().map_or(0, Vec::len),
                ))
                .max_width(content_width)
                .show(ui, |ui| {
                    egui::Grid::new((
                        "markdown_table_grid",
                        rows.len(),
                        rows.first().map_or(0, Vec::len),
                    ))
                    .striped(true)
                    .spacing(egui::vec2(14.0, 6.0))
                    .show(ui, |ui| {
                        for (row_index, row) in rows.iter().enumerate() {
                            for cell in row {
                                if row_index == 0 {
                                    ui.label(
                                        egui::RichText::new(strip_markdown_inline(cell)).strong(),
                                    );
                                } else {
                                    ui.label(strip_markdown_inline(cell));
                                }
                            }
                            ui.end_row();
                        }
                    });
                });
        });
}
fn normalize_display_symbols(text: &str) -> String {
    text.chars()
        .map(|ch| match ch {
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2212}' => '-',
            '\u{00a0}' => ' ',
            _ => ch,
        })
        .collect()
}

fn markdown_list_item(line: &str) -> Option<&str> {
    line.strip_prefix("*   ")
        .or_else(|| line.strip_prefix("* "))
        .or_else(|| line.strip_prefix("- "))
        .or_else(|| {
            let (number, rest) = line.split_once(".  ")?;
            number.chars().all(|ch| ch.is_ascii_digit()).then_some(rest)
        })
        .or_else(|| {
            let (number, rest) = line.split_once(". ")?;
            number.chars().all(|ch| ch.is_ascii_digit()).then_some(rest)
        })
}

fn render_inline_markdown(ui: &mut egui::Ui, line: &str) {
    ui.horizontal_wrapped(|ui| {
        for (index, segment) in line.split("**").enumerate() {
            if segment.is_empty() {
                continue;
            }
            render_code_segments(ui, segment, index % 2 == 1);
        }
    });
}

fn render_code_segments(ui: &mut egui::Ui, text: &str, strong: bool) {
    for (index, segment) in text.split('`').enumerate() {
        if segment.is_empty() {
            continue;
        }
        let mut rich = egui::RichText::new(segment.to_owned());
        if strong {
            rich = rich.strong();
        }
        if index % 2 == 1 {
            rich = rich
                .monospace()
                .background_color(ui.visuals().extreme_bg_color);
        }
        ui.label(rich);
    }
}

fn strip_markdown_inline(text: &str) -> String {
    text.replace("**", "").replace('`', "")
}

fn grid_columns(ui: &egui::Ui, card_width: f32) -> usize {
    let gap = 12.0;
    ((ui.available_width() + gap) / (card_width + gap))
        .floor()
        .max(1.0) as usize
}

fn load_persisted_settings(
    store: &AppStore,
) -> Result<PersistedSettings, Box<dyn std::error::Error + Send + Sync>> {
    let Some(value) = store.get_setting(SETTINGS_KEY)? else {
        return Ok(PersistedSettings::default());
    };
    Ok(serde_json::from_str(&value)?)
}

fn load_persisted_update_settings(
    store: &AppStore,
) -> Result<PersistedUpdateSettings, Box<dyn std::error::Error + Send + Sync>> {
    let Some(value) = store.get_setting(UPDATE_SETTINGS_KEY)? else {
        return Ok(PersistedUpdateSettings::default());
    };
    Ok(serde_json::from_str(&value)?)
}

impl eframe::App for ShuaForgeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_ai();
        self.poll_update();
        if self.is_ai_loading {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if self.is_update_checking || self.is_update_applying {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if self.dragging_deck_id.is_some() || self.dragging_group_id.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
        self.render_analysis_dialog(ctx);
        self.render_about_dialog(ctx);
        self.render_update_prompt(ctx);

        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
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
            egui::SidePanel::right("config_panel")
                .resizable(true)
                .default_width(300.0)
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("config_panel_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.heading("练习设置");
                            let mut shuffled = self.practice_order == PracticeOrder::Shuffled;
                            if ui.checkbox(&mut shuffled, "乱序练习").changed() {
                                self.practice_order = if shuffled {
                                    PracticeOrder::Shuffled
                                } else {
                                    PracticeOrder::Sequential
                                };
                                self.status = if shuffled {
                                    "已切换为乱序练习。"
                                } else {
                                    "已切换为顺序练习。"
                                }
                                .into();
                                self.persist_settings();
                            }

                            ui.separator();
                            ui.heading("AI 配置");
                            ui.horizontal_wrapped(|ui| {
                                if ui.button("导入 JSON").clicked() {
                                    self.load_ai_config();
                                }
                                if ui.button("导出 JSON").clicked() {
                                    self.save_ai_config();
                                }
                            });
                            ui.small(
                                "配置会自动保存到本机数据库。JSON 导入/导出可用于备份或迁移配置。",
                            );
                            let mut settings_changed = false;
                            settings_changed |= ui
                                .checkbox(&mut self.ai_config.enabled, "启用 AI 错题解析")
                                .changed();
                            ui.label("Endpoint");
                            settings_changed |= ui
                                .text_edit_singleline(&mut self.ai_config.endpoint)
                                .changed();
                            ui.label("Model");
                            settings_changed |=
                                ui.text_edit_singleline(&mut self.ai_config.model).changed();
                            ui.label("API Key（仅保存在本机 SQLite 设置中）");
                            settings_changed |= ui
                                .add(
                                    egui::TextEdit::singleline(&mut self.ai_config.api_key)
                                        .password(true),
                                )
                                .changed();
                            settings_changed |= ui
                                .add(
                                    egui::Slider::new(&mut self.ai_config.timeout_secs, 5..=120)
                                        .text("超时秒数"),
                                )
                                .changed();
                            if settings_changed {
                                self.persist_settings();
                            }

                            ui.separator();
                            ui.heading("状态");
                            ui.label(&self.status);
                            ui.label(format!("全部题目：{} 道", self.bank_count));
                            ui.label(format!("题库：{} 个", self.deck_cards.len()));
                            ui.label(format!("题组：{} 个", self.group_cards.len()));
                            if let Some(name) = &self.active_deck_name {
                                ui.label(format!("当前题库：{name}"));
                            }

                            if let Some(path) = &self.loaded_path {
                                ui.small(format!("题库：{}", path.display()));
                            }
                            if let Some(path) = &self.ai_config_path {
                                ui.small(format!("AI 配置：{}", path.display()));
                            }

                            ui.separator();
                            ui.heading("浏览器导出脚本");
                            if ui.button("安装题库导出油猴脚本").clicked() {
                                match userscript_server::open_userscript_install_page() {
                                    Ok(url) => {
                                        self.status = format!(
                                            "已在浏览器打开脚本安装页：{url}。如果没有弹出安装页，请确认已安装 Tampermonkey / Violentmonkey / 脚本猫。"
                                        );
                                    }
                                    Err(err) => {
                                        self.status = err;
                                    }
                                }
                            }
                            ui.small(
                                "会启动本机 127.0.0.1 临时服务并打开 .user.js 安装页；浏览器扩展仍需要你确认安装。",
                            );

                            ui.collapsing("答题历史", |ui| {
                                for record in &self.answer_history {
                                    ui.small(format!(
                                        "{} {} {}：{} / {}",
                                        record.answered_at,
                                        if record.is_correct { "✅" } else { "❌" },
                                        record.problem_id,
                                        record.user_answer,
                                        record.correct_answer
                                    ));
                                }
                            });
                        });
                });
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            let mut submit_requested = false;
            let mut skip_requested = false;
            let mut back_to_library_requested = false;

            if self.view == AppView::Practice {
                let Some(deck) = &self.deck else {
                    self.view = AppView::Library;
                    return;
                };
                let stats = deck.stats();
                ui.horizontal(|ui| {
                    if ui.button("← 题库主页").clicked() {
                        back_to_library_requested = true;
                    }
                    ui.separator();
                    ui.label(format!("总题数：{}", stats.total));
                    ui.label(format!("剩余：{}", stats.remaining));
                    ui.label(format!("答对：{}", stats.correct));
                    ui.label(format!("答错：{}", stats.wrong));
                });
                ui.separator();

                if deck.is_finished() {
                    ui.heading("本轮练习已完成");
                    ui.label("本轮练习已完成。可返回题库主页选择新的练习内容。");
                } else if let Some(problem) = deck.current() {
                    ui.heading(format!("题目 #{}", problem.id));
                    ui.small(format!("题型：{}", problem_type_label(problem.kind())));
                    if !problem.tags.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            for tag in &problem.tags {
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
                    let submit_shortcut = render_answer_input(
                        ui,
                        problem,
                        &mut self.answer_input,
                        &mut self.selected_single,
                        &mut self.selected_multiple,
                    );

                    ui.horizontal(|ui| {
                        if ui.button("提交答案").clicked() || submit_shortcut {
                            submit_requested = true;
                        }
                        if ui.button("跳过并放回队尾").clicked() {
                            skip_requested = true;
                        }
                    });
                }
            } else {
                egui::ScrollArea::vertical()
                    .id_salt("library_home_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        self.render_library_home(ui);
                    });
            }

            if submit_requested {
                self.submit_answer();
            }
            if back_to_library_requested {
                self.back_to_library();
            }
            if skip_requested && let Some(deck) = &mut self.deck {
                deck.skip();
                self.clear_answer_inputs();
                self.status = "已跳过，题目放回队尾。".into();
            }

            ui.separator();
            ui.heading(if self.is_ai_loading {
                "解析与分析（生成中...）"
            } else {
                "解析与分析"
            });
            egui::ScrollArea::vertical()
                .id_salt("wrong_answer_analysis")
                .max_height(220.0)
                .show(ui, |ui| {
                    render_markdown_text(ui, &self.analysis, ui.available_width());
                });
        });
    }
}

fn render_problem(ui: &mut egui::Ui, problem: &Problem) {
    egui::ScrollArea::vertical()
        .id_salt(("problem_prompt", &problem.id))
        .max_height(180.0)
        .show(ui, |ui| {
            let question = problem.question_text();
            ui.label(if question.is_empty() {
                &problem.prompt
            } else {
                &question
            });
        });
}

fn render_library_deck_card(
    ui: &mut egui::Ui,
    card: &DeckCard,
    dragging: bool,
) -> (egui::Response, egui::Response, bool, bool, bool) {
    let frame = egui::Frame::group(ui.style())
        .fill(if dragging {
            egui::Color32::from_rgb(48, 69, 120)
        } else {
            ui.visuals().extreme_bg_color
        })
        .stroke(if dragging {
            egui::Stroke::new(2.0, egui::Color32::from_rgb(120, 170, 255))
        } else {
            egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color)
        })
        .inner_margin(egui::Margin::same(14));
    let mut start_clicked = false;
    let mut analyze_clicked = false;
    let mut diagnose_clicked = false;
    let mut drag_response: Option<egui::Response> = None;
    let response = frame
        .show(ui, |ui| {
            ui.set_min_size(egui::vec2(CARD_WIDTH, DECK_CARD_HEIGHT));
            ui.set_max_width(CARD_WIDTH);
            ui.vertical_centered(|ui| {
                let info_response = ui
                    .vertical_centered(|ui| {
                        ui.heading("📘");
                        ui.strong(&card.name);
                        ui.small(format!("{} 道题", card.problem_count));
                        ui.small(format!("新增 {} · 更新 {}", card.inserted, card.updated));
                        ui.small(format!("更新时间：{}", card.updated_at));
                        ui.small(format!("来源：{}", compact_text(&card.source_path, 26)));
                        ui.small("拖动此区域可移动或删除题库");
                    })
                    .response
                    .interact(egui::Sense::click_and_drag());
                drag_response = Some(if dragging || info_response.dragged() {
                    info_response.on_hover_cursor(egui::CursorIcon::Grabbing)
                } else {
                    info_response
                });
                (start_clicked, analyze_clicked, diagnose_clicked) = render_card_actions(ui);
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
    )
}

fn render_group_card(
    ui: &mut egui::Ui,
    group: &GroupCard,
    group_decks: &[crate::store::GroupDeckCard],
    hot: bool,
    dragging: bool,
) -> (egui::Response, egui::Response, bool, bool, bool, Vec<i64>) {
    let frame = egui::Frame::group(ui.style())
        .fill(if hot {
            egui::Color32::from_rgb(48, 86, 68)
        } else {
            ui.visuals().faint_bg_color
        })
        .inner_margin(egui::Margin::same(14));
    let mut start_clicked = false;
    let mut analyze_clicked = false;
    let mut diagnose_clicked = false;
    let mut remove_requests = Vec::new();
    let mut drag_response: Option<egui::Response> = None;
    let response = frame
        .show(ui, |ui| {
            ui.set_min_size(egui::vec2(CARD_WIDTH, GROUP_CARD_HEIGHT));
            ui.set_max_width(CARD_WIDTH);
            ui.vertical_centered(|ui| {
                let info_response = ui
                    .vertical_centered(|ui| {
                        ui.heading("📁");
                        ui.strong(&group.name);
                        ui.small(format!(
                            "{} 个题库 · {} 道题",
                            group.deck_count, group.problem_count
                        ));
                        ui.small(format!("更新时间：{}", group.updated_at));
                        ui.small("拖动此区域可删除题组");
                    })
                    .response
                    .interact(egui::Sense::click_and_drag());
                drag_response = Some(if dragging || info_response.dragged() {
                    info_response.on_hover_cursor(egui::CursorIcon::Grabbing)
                } else {
                    info_response
                });
                (start_clicked, analyze_clicked, diagnose_clicked) = render_card_actions(ui);

                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);
                ui.small("题组内题库");
                if group_decks.is_empty() {
                    ui.small("暂无题库，可从题库卡片拖入这里。");
                } else {
                    egui::ScrollArea::vertical()
                        .max_height(110.0)
                        .id_salt(("group_decks", group.id))
                        .show(ui, |ui| {
                            for deck in group_decks {
                                ui.horizontal(|ui| {
                                    ui.small(format!("{}（{}题）", deck.name, deck.problem_count));
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui.small_button("移出").clicked() {
                                                remove_requests.push(deck.id);
                                            }
                                        },
                                    );
                                });
                            }
                        });
                }
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
        remove_requests,
    )
}

fn render_card_actions(ui: &mut egui::Ui) -> (bool, bool, bool) {
    let spacing = 6.0;
    let total_width = CARD_BUTTON_WIDTH * 3.0 + spacing * 2.0;
    let left_padding = ((ui.available_width() - total_width) / 2.0).max(0.0);
    let mut start_clicked = false;
    let mut analyze_clicked = false;
    let mut diagnose_clicked = false;

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(left_padding);
        ui.spacing_mut().item_spacing.x = spacing;
        start_clicked = ui
            .add_sized([CARD_BUTTON_WIDTH, 24.0], egui::Button::new("开始答题"))
            .clicked();
        analyze_clicked = ui
            .add_sized([CARD_BUTTON_WIDTH, 24.0], egui::Button::new("分析题目"))
            .clicked();
        diagnose_clicked = ui
            .add_sized([CARD_BUTTON_WIDTH, 24.0], egui::Button::new("学习诊断"))
            .clicked();
    });

    (start_clicked, analyze_clicked, diagnose_clicked)
}

fn render_trash_zone(ui: &mut egui::Ui, active: bool) -> egui::Response {
    let frame = egui::Frame::group(ui.style())
        .fill(if active {
            egui::Color32::from_rgb(92, 36, 36)
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

fn render_answer_input(
    ui: &mut egui::Ui,
    problem: &Problem,
    answer_input: &mut String,
    selected_single: &mut Option<String>,
    selected_multiple: &mut BTreeSet<String>,
) -> bool {
    match problem.kind() {
        ProblemType::SingleChoice => {
            for option in problem.options() {
                ui.radio_value(
                    selected_single,
                    Some(option.key.clone()),
                    format!("{}. {}", option.key, option.text),
                );
            }
            false
        }
        ProblemType::MultipleChoice => {
            for option in problem.options() {
                let mut checked = selected_multiple.contains(&option.key);
                if ui
                    .checkbox(&mut checked, format!("{}. {}", option.key, option.text))
                    .changed()
                {
                    if checked {
                        selected_multiple.insert(option.key.clone());
                    } else {
                        selected_multiple.remove(&option.key);
                    }
                }
            }
            false
        }
        ProblemType::Text => {
            let response = ui.add(
                egui::TextEdit::multiline(answer_input)
                    .desired_rows(4)
                    .hint_text("写下答案后按 Ctrl+Enter 或点击提交"),
            );
            response.has_focus()
                && ui.input(|input| input.modifiers.ctrl && input.key_pressed(egui::Key::Enter))
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

fn configure_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    for (index, (name, data)) in load_system_fonts().into_iter().enumerate() {
        fonts.font_data.insert(name.clone(), data.into());

        if index == 0 {
            fonts
                .families
                .entry(FontFamily::Proportional)
                .or_default()
                .insert(0, name.clone());
            fonts
                .families
                .entry(FontFamily::Monospace)
                .or_default()
                .insert(0, name);
        } else {
            fonts
                .families
                .entry(FontFamily::Proportional)
                .or_default()
                .push(name.clone());
            fonts
                .families
                .entry(FontFamily::Monospace)
                .or_default()
                .push(name);
        }
    }

    ctx.set_fonts(fonts);

    let mut style = (*ctx.style()).clone();
    style.text_styles = [
        (egui::TextStyle::Heading, egui::FontId::proportional(26.0)),
        (egui::TextStyle::Body, egui::FontId::proportional(16.0)),
        (egui::TextStyle::Button, egui::FontId::proportional(15.0)),
        (egui::TextStyle::Small, egui::FontId::proportional(13.0)),
        (egui::TextStyle::Monospace, egui::FontId::monospace(14.0)),
    ]
    .into();
    ctx.set_style(style);
}

fn load_system_fonts() -> Vec<(String, FontData)> {
    let candidates = [
        ("Microsoft YaHei", r"C:\Windows\Fonts\msyh.ttc"),
        ("Microsoft YaHei UI", r"C:\Windows\Fonts\msyh.ttf"),
        ("Segoe UI Symbol", r"C:\Windows\Fonts\seguisym.ttf"),
        ("Segoe UI Emoji", r"C:\Windows\Fonts\seguiemj.ttf"),
        ("Arial Unicode MS", r"C:\Windows\Fonts\arialuni.ttf"),
        ("SimHei", r"C:\Windows\Fonts\simhei.ttf"),
        ("SimSun", r"C:\Windows\Fonts\simsun.ttc"),
        ("Noto Sans CJK", r"C:\Windows\Fonts\NotoSansCJK-Regular.ttc"),
    ];

    candidates
        .iter()
        .filter_map(|(name, path)| {
            std::fs::read(path)
                .ok()
                .map(|bytes| ((*name).to_owned(), FontData::from_owned(bytes)))
        })
        .collect()
}

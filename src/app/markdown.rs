use eframe::egui::{self, ColorImage, TextureHandle};
use resvg::{tiny_skia, usvg};
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    sync::{Mutex, OnceLock},
    thread,
};

const REMOTE_FORMULA_PLACEHOLDER: &str = "{latex}";
const CODECOGS_SVG_URL_TEMPLATE: &str = "https://latex.codecogs.com/svg.image?{latex}";
const CODECOGS_DARK_SVG_URL_TEMPLATE: &str =
    "https://latex.codecogs.com/svg.image?\\color{white}{%20{latex}%20}";
const EMBEDDED_MISANS_FONT: &[u8] = include_bytes!("../../assets/fonts/MiSans-Regular.otf");
const EMBEDDED_NOTO_SANS_SC_FONT: &[u8] = include_bytes!("../../assets/fonts/NotoSansSC-VF.ttf");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FormulaRenderSettings {
    #[serde(default)]
    pub enable_remote: bool,
    #[serde(default = "default_formula_render_url_template")]
    pub remote_url_template: String,
    #[serde(default = "default_formula_render_timeout_secs")]
    pub remote_timeout_secs: u64,
}

impl Default for FormulaRenderSettings {
    fn default() -> Self {
        Self {
            enable_remote: false,
            remote_url_template: default_formula_render_url_template(),
            remote_timeout_secs: default_formula_render_timeout_secs(),
        }
    }
}

pub const FORMULA_RENDER_PRESETS: &[(&str, &str)] = &[
    (
        "CodeCogs SVG（公共免费服务，不保证可用）",
        CODECOGS_SVG_URL_TEMPLATE,
    ),
    (
        "CodeCogs SVG 白色公式（公共免费服务，不保证可用）",
        CODECOGS_DARK_SVG_URL_TEMPLATE,
    ),
];

thread_local! {
    static FORMULA_RENDER_CACHE: RefCell<FormulaRenderCache> = RefCell::new(FormulaRenderCache::default());
    static FORMULA_TEXTURE_CACHE: RefCell<HashMap<FormulaTextureCacheKey, TextureHandle>> = RefCell::new(HashMap::new());
    static FORMULA_RENDER_SETTINGS: RefCell<FormulaRenderSettings> = RefCell::new(FormulaRenderSettings::default());
}

static REMOTE_FORMULA_CACHE: OnceLock<Mutex<HashMap<RemoteFormulaCacheKey, RemoteFormulaState>>> =
    OnceLock::new();

pub fn apply_formula_render_settings(settings: FormulaRenderSettings) {
    FORMULA_RENDER_SETTINGS.with(|current| {
        let mut current = current.borrow_mut();
        if *current != settings {
            *current = settings;
            clear_formula_render_caches();
        }
    });
}

pub fn clear_formula_render_caches() {
    FORMULA_RENDER_CACHE.with(|cache| cache.borrow_mut().fallback_outputs.clear());
    FORMULA_TEXTURE_CACHE.with(|cache| cache.borrow_mut().clear());
    remote_formula_cache()
        .lock()
        .expect("remote formula cache")
        .clear();
}

fn remote_formula_cache() -> &'static Mutex<HashMap<RemoteFormulaCacheKey, RemoteFormulaState>> {
    REMOTE_FORMULA_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn default_formula_render_url_template() -> String {
    CODECOGS_SVG_URL_TEMPLATE.to_owned()
}

fn default_formula_render_timeout_secs() -> u64 {
    6
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct FormulaCacheKey {
    latex: String,
    display: bool,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct FormulaTextureCacheKey {
    label: String,
    display: bool,
    size: [usize; 2],
    rgba_hash: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RenderedFormula {
    readable_text: String,
    image: Option<RenderedFormulaImage>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RenderedFormulaImage {
    size: [usize; 2],
    rgba: Vec<u8>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct RemoteFormulaCacheKey {
    latex: String,
    display: bool,
    url_template: String,
}

#[derive(Clone, Debug)]
enum RemoteFormulaState {
    Pending,
    Ready(Option<RenderedFormulaImage>),
}

trait FormulaRenderer {
    fn render_formula(&mut self, latex: &str, display: bool) -> Option<RenderedFormula>;
}

#[derive(Default)]
struct FormulaRenderCache {
    fallback_outputs: HashMap<FormulaCacheKey, RenderedFormula>,
}

impl FormulaRenderCache {
    fn render_with<R: FormulaRenderer>(
        &mut self,
        renderer: &mut R,
        latex: &str,
        display: bool,
    ) -> RenderedFormula {
        let key = FormulaCacheKey {
            latex: latex.trim().to_owned(),
            display,
        };
        if let Some(rendered) = self.fallback_outputs.get(&key) {
            return rendered.clone();
        }
        let rendered = renderer
            .render_formula(&key.latex, display)
            .unwrap_or_else(|| RenderedFormula {
                readable_text: key.latex.clone(),
                image: None,
            });
        self.fallback_outputs.insert(key, rendered.clone());
        rendered
    }
}

#[derive(Default)]
struct FallbackFormulaRenderer;

impl FormulaRenderer for FallbackFormulaRenderer {
    fn render_formula(&mut self, latex: &str, display: bool) -> Option<RenderedFormula> {
        let readable_text = if display {
            latex_to_readable_text_with_mode(latex, true)
        } else {
            latex_to_readable_text(latex)
        };
        Some(RenderedFormula {
            image: render_formula_svg_image(&readable_text, display),
            readable_text,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
enum InlineSegment<'a> {
    Text(&'a str),
    Formula { latex: &'a str, display: bool },
}

pub fn render_markdown_text(ui: &mut egui::Ui, text: &str, content_width: f32) {
    let normalized = normalize_display_symbols(text);
    let lines = normalized.lines().collect::<Vec<_>>();
    let mut index = 0;
    while index < lines.len() {
        if let Some((formula, consumed)) = parse_latex_block(&lines[index..]) {
            render_latex_block(ui, &formula, content_width);
            index += consumed;
            continue;
        }

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

fn parse_latex_block(lines: &[&str]) -> Option<(String, usize)> {
    let first = lines.first()?.trim();
    if first.starts_with("\\[") {
        return collect_delimited_latex(lines, "\\[", "\\]");
    }
    if first.starts_with("$$") {
        return collect_delimited_latex(lines, "$$", "$$");
    }
    None
}

fn collect_delimited_latex(lines: &[&str], open: &str, close: &str) -> Option<(String, usize)> {
    let mut formula = String::new();
    for (index, line) in lines.iter().enumerate() {
        let mut part = line.trim();
        if index == 0 {
            part = part.strip_prefix(open)?.trim_start();
        }
        let ended = if open == close && index == 0 {
            part.ends_with(close) && part.len() > close.len()
        } else {
            part.ends_with(close)
        };
        if ended {
            part = part.strip_suffix(close).unwrap_or(part).trim_end();
            if !part.is_empty() {
                if !formula.is_empty() {
                    formula.push(' ');
                }
                formula.push_str(part);
            }
            return Some((formula, index + 1));
        }
        if !part.is_empty() {
            if !formula.is_empty() {
                formula.push(' ');
            }
            formula.push_str(part);
        }
    }
    None
}

fn render_latex_block(ui: &mut egui::Ui, formula: &str, content_width: f32) {
    let rendered = render_formula_cached(ui.ctx(), formula, true);
    ui.add_space(4.0);
    egui::Frame::new()
        .fill(markdown_block_fill(ui))
        .stroke(egui::Stroke::new(
            1.0,
            ui.visuals().widgets.noninteractive.bg_stroke.color,
        ))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(content_width);
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new("公式")
                        .strong()
                        .color(egui::Color32::from_rgb(120, 170, 255)),
                );
                render_formula_output(ui, &rendered, true);
            });
        });
}

fn latex_to_readable_text(formula: &str) -> String {
    latex_to_readable_text_with_mode(formula, false)
}

fn latex_to_readable_text_with_mode(formula: &str, display: bool) -> String {
    let mut text = formula.trim().to_owned();
    text = replace_latex_hat(&text);
    text = replace_latex_bar(&text);
    text = replace_latex_frac(&text);
    let replacements = [
        ("\\text", ""),
        ("\\sum", "∑"),
        ("\\prod", "∏"),
        ("\\infty", "∞"),
        ("\\leq", "≤"),
        ("\\geq", "≥"),
        ("\\neq", "≠"),
        ("\\approx", "≈"),
        ("\\alpha", "α"),
        ("\\beta", "β"),
        ("\\gamma", "γ"),
        ("\\delta", "δ"),
        ("\\mu", "μ"),
        ("\\sigma", "σ"),
        ("\\times", "×"),
        ("\\cdot", "·"),
        ("\\%", "%"),
        ("\\left", ""),
        ("\\right", ""),
        ("\\mathrm", ""),
        ("\\mathbf", ""),
        ("\\quad", " "),
        ("\\,", " "),
    ];
    for (from, to) in replacements {
        text = text.replace(from, to);
    }
    text = text.replace(['{', '}'], "");
    if display && text.contains('\n') {
        text.lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n")
            .trim_matches('\n')
            .to_owned()
    } else {
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

fn replace_latex_hat(input: &str) -> String {
    let mut text = input.to_owned();
    for command in ["\\widehat", "\\hat"] {
        while let Some(start) = text.find(&format!("{command}{{")) {
            let argument_start = start + command.len();
            let Some((argument, argument_end)) = read_braced_group(&text, argument_start) else {
                break;
            };
            text.replace_range(start..argument_end, &hat_text(&argument));
        }
    }
    text
}

fn hat_text(text: &str) -> String {
    let readable = latex_to_readable_text(text);
    let trimmed = readable.trim();
    if trimmed.is_empty() {
        "̂".to_owned()
    } else {
        format!("{trimmed}̂")
    }
}

fn replace_latex_bar(input: &str) -> String {
    let mut text = input.to_owned();
    while let Some(start) = text.find("\\bar{") {
        let argument_start = start + "\\bar".len();
        let Some((argument, argument_end)) = read_braced_group(&text, argument_start) else {
            break;
        };
        text.replace_range(start..argument_end, &overline_text(&argument));
    }
    while let Some(start) = text.find("\\overline{") {
        let argument_start = start + "\\overline".len();
        let Some((argument, argument_end)) = read_braced_group(&text, argument_start) else {
            break;
        };
        text.replace_range(start..argument_end, &overline_text(&argument));
    }
    text
}

fn overline_text(text: &str) -> String {
    text.chars()
        .flat_map(|ch| [ch, '\u{0305}'])
        .collect::<String>()
}

fn replace_latex_frac(input: &str) -> String {
    let mut text = input.to_owned();
    while let Some(start) = text.find("\\frac{") {
        let numerator_start = start + "\\frac".len();
        let Some((numerator, numerator_end)) = read_braced_group(&text, numerator_start) else {
            break;
        };
        let Some((denominator, denominator_end)) = read_braced_group(&text, numerator_end) else {
            break;
        };
        let numerator = latex_to_readable_text_with_mode(&numerator, false);
        let denominator = latex_to_readable_text_with_mode(&denominator, false);
        let replacement = render_plain_fraction_text(&numerator, &denominator);
        text.replace_range(start..denominator_end, &replacement);
    }
    text
}

fn render_plain_fraction_text(numerator: &str, denominator: &str) -> String {
    let numerator = numerator.trim();
    let denominator = denominator.trim();
    format!("({numerator})/({denominator})")
}

fn read_braced_group(text: &str, open_index: usize) -> Option<(String, usize)> {
    let bytes = text.as_bytes();
    if bytes.get(open_index).copied()? != b'{' {
        return None;
    }
    let mut depth = 0usize;
    let content_start = open_index + 1;
    for (offset, ch) in text[open_index..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let end = open_index + offset;
                    return Some((text[content_start..end].to_owned(), end + 1));
                }
            }
            _ => {}
        }
    }
    None
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
        .fill(markdown_block_fill(ui))
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
        if index % 2 == 1 {
            let mut rich = egui::RichText::new(segment.to_owned())
                .monospace()
                .background_color(inline_code_fill(ui));
            if strong {
                rich = rich.strong();
            }
            ui.add(egui::Label::new(rich).wrap());
        } else {
            render_inline_formula_segments(ui, segment, strong);
        }
    }
}

fn render_inline_formula_segments(ui: &mut egui::Ui, text: &str, strong: bool) {
    for segment in parse_inline_formula_segments(text) {
        match segment {
            InlineSegment::Text(text) => {
                if text.is_empty() {
                    continue;
                }
                let mut rich = egui::RichText::new(text.to_owned());
                if strong {
                    rich = rich.strong();
                }
                ui.add(egui::Label::new(rich).wrap());
            }
            InlineSegment::Formula { latex, display } => render_inline_formula(ui, latex, display),
        }
    }
}

fn render_inline_formula(ui: &mut egui::Ui, latex: &str, display: bool) {
    let rendered = render_formula_cached(ui.ctx(), latex, display);
    render_formula_output(ui, &rendered, display);
}

fn render_formula_output(ui: &mut egui::Ui, rendered: &RenderedFormula, display: bool) -> bool {
    if let Some(image) = &rendered.image
        && let Some(texture) = load_formula_texture(ui, image, &rendered.readable_text, display)
    {
        let size = egui::vec2(image.size[0] as f32, image.size[1] as f32);
        if display {
            ui.add(egui::Image::new((texture.id(), size)));
        } else {
            paint_inline_formula_texture(ui, &texture, size);
        }
        return true;
    }

    let accent = if display {
        egui::Color32::from_rgb(120, 170, 255)
    } else {
        egui::Color32::from_rgb(150, 190, 255)
    };
    ui.add(
        egui::Label::new(
            egui::RichText::new(rendered.readable_text.clone())
                .monospace()
                .color(accent)
                .background_color(inline_code_fill(ui)),
        )
        .wrap(),
    );
    false
}

fn markdown_block_fill(ui: &egui::Ui) -> egui::Color32 {
    if ui.visuals().dark_mode {
        ui.visuals().extreme_bg_color.gamma_multiply(0.55)
    } else {
        ui.visuals().faint_bg_color
    }
}

fn inline_code_fill(ui: &egui::Ui) -> egui::Color32 {
    if ui.visuals().dark_mode {
        ui.visuals().extreme_bg_color.gamma_multiply(0.65)
    } else {
        ui.visuals().faint_bg_color
    }
}

fn paint_inline_formula_texture(ui: &mut egui::Ui, texture: &TextureHandle, size: egui::Vec2) {
    let row_height = ui.text_style_height(&egui::TextStyle::Body);
    let target_height = row_height * 1.08;
    let scale = if size.y > 0.0 {
        (target_height / size.y).min(1.0)
    } else {
        1.0
    };
    let target_size = egui::vec2((size.x * scale).max(1.0), (size.y * scale).max(1.0));
    let (rect, _) = ui.allocate_exact_size(target_size, egui::Sense::empty());
    let lift = (target_size.y - row_height).max(0.0) * 0.45 + 1.0;
    let paint_rect = rect.translate(egui::vec2(0.0, -lift));
    ui.painter().image(
        texture.id(),
        paint_rect,
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
}

fn render_formula_svg_image(readable_text: &str, display: bool) -> Option<RenderedFormulaImage> {
    let lines = readable_text.lines().collect::<Vec<_>>();
    let lines = if lines.is_empty() {
        vec![readable_text]
    } else {
        lines
    };
    let font_size = if display { 24.0 } else { 16.0 };
    let line_height = font_size * if display { 1.35 } else { 1.08 };
    let padding_x = if display { 14.0 } else { 8.0 };
    let padding_y = if display { 10.0 } else { 1.0 };
    let max_chars = lines
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(1)
        .max(1);
    let width = ((max_chars as f32 * font_size * 0.62) + padding_x * 2.0)
        .ceil()
        .max(24.0) as u32;
    let height = ((lines.len() as f32 * line_height) + padding_y * 2.0)
        .ceil()
        .max(if display { 18.0 } else { 12.0 }) as u32;
    let mut body = String::new();
    for (index, line) in lines.iter().enumerate() {
        let y = padding_y + font_size + index as f32 * line_height;
        body.push_str(&format!(
            r#"<text x="{padding_x:.1}" y="{y:.1}">{}</text>"#,
            escape_svg_text(line)
        ));
    }
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}"><style>text{{font-family:'MiSans','Noto Sans SC','Segoe UI Symbol','DejaVu Sans Mono','Noto Sans Math','Arial Unicode MS',monospace;font-size:{font_size}px;fill:#96BEFF;white-space:pre;}}</style>{body}</svg>"#
    );
    rasterize_svg(&svg, width, height)
}

fn get_async_remote_formula_image(
    ctx: &egui::Context,
    latex: &str,
    display: bool,
) -> Option<RenderedFormulaImage> {
    let settings = FORMULA_RENDER_SETTINGS.with(|settings| settings.borrow().clone());
    if !settings.enable_remote || settings.remote_url_template.trim().is_empty() {
        return None;
    }
    let key = RemoteFormulaCacheKey {
        latex: latex.trim().to_owned(),
        display,
        url_template: settings.remote_url_template.trim().to_owned(),
    };
    {
        let mut cache = remote_formula_cache().lock().expect("remote formula cache");
        match cache.get(&key) {
            Some(RemoteFormulaState::Ready(image)) => return image.clone(),
            Some(RemoteFormulaState::Pending) => return None,
            None => {
                cache.insert(key.clone(), RemoteFormulaState::Pending);
            }
        }
    }

    let repaint_ctx = ctx.clone();
    thread::spawn(move || {
        let image = render_remote_formula_image_with_settings(&settings, &key.latex, key.display);
        remote_formula_cache()
            .lock()
            .expect("remote formula cache")
            .insert(key, RemoteFormulaState::Ready(image));
        repaint_ctx.request_repaint();
    });
    None
}

fn render_remote_formula_image_with_settings(
    settings: &FormulaRenderSettings,
    latex: &str,
    display: bool,
) -> Option<RenderedFormulaImage> {
    if !settings.enable_remote || settings.remote_url_template.trim().is_empty() {
        return None;
    }
    let url = build_remote_formula_url(&settings.remote_url_template, latex, display)?;
    let response = minreq::get(url)
        .with_timeout(settings.remote_timeout_secs.max(1))
        .send()
        .ok()?;
    if response.status_code < 200 || response.status_code >= 300 {
        return None;
    }
    let svg = response.as_str().ok()?.trim();
    if !looks_like_svg(svg) {
        return None;
    }
    rasterize_svg_auto_size(svg)
}

fn build_remote_formula_url(template: &str, latex: &str, display: bool) -> Option<String> {
    let latex = if display {
        format!(r"\displaystyle {latex}")
    } else {
        latex.to_owned()
    };
    let encoded = percent_encode_url_component(&latex);
    let template = template.trim();
    if template.contains(REMOTE_FORMULA_PLACEHOLDER) {
        Some(template.replace(REMOTE_FORMULA_PLACEHOLDER, &encoded))
    } else if template.contains('?') {
        Some(format!("{template}&latex={encoded}"))
    } else {
        Some(format!("{template}?latex={encoded}"))
    }
}

fn percent_encode_url_component(input: &str) -> String {
    input
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            _ => format!("%{byte:02X}").chars().collect::<Vec<_>>(),
        })
        .collect()
}

fn looks_like_svg(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with("<svg") || trimmed.starts_with("<?xml") && trimmed.contains("<svg")
}

fn rasterize_svg_auto_size(svg: &str) -> Option<RenderedFormulaImage> {
    let options = formula_svg_options();
    let tree = usvg::Tree::from_str(svg, &options).ok()?;
    let size = tree.size().to_int_size();
    let width = size.width().max(1);
    let height = size.height().max(1);
    rasterize_tree(&tree, width, height)
}

fn rasterize_svg(svg: &str, width: u32, height: u32) -> Option<RenderedFormulaImage> {
    let options = formula_svg_options();
    let tree = usvg::Tree::from_str(svg, &options).ok()?;
    rasterize_tree(&tree, width, height)
}

fn formula_svg_options() -> usvg::Options<'static> {
    let mut options = usvg::Options {
        font_family: "MiSans".to_owned(),
        ..Default::default()
    };
    let fontdb = options.fontdb_mut();
    fontdb.load_system_fonts();
    fontdb.load_font_data(EMBEDDED_MISANS_FONT.to_vec());
    fontdb.load_font_data(EMBEDDED_NOTO_SANS_SC_FONT.to_vec());
    options
}

fn rasterize_tree(tree: &usvg::Tree, width: u32, height: u32) -> Option<RenderedFormulaImage> {
    let mut pixmap = tiny_skia::Pixmap::new(width, height)?;
    resvg::render(tree, tiny_skia::Transform::identity(), &mut pixmap.as_mut());
    Some(RenderedFormulaImage {
        size: [width as usize, height as usize],
        rgba: premultiplied_to_unmultiplied_rgba(pixmap.data()),
    })
}

fn premultiplied_to_unmultiplied_rgba(data: &[u8]) -> Vec<u8> {
    data.chunks_exact(4)
        .flat_map(|pixel| {
            let alpha = pixel[3];
            if alpha == 0 {
                [0, 0, 0, 0]
            } else {
                let unpremultiply = |channel: u8| {
                    ((u16::from(channel) * 255 + u16::from(alpha) / 2) / u16::from(alpha)).min(255)
                        as u8
                };
                [
                    unpremultiply(pixel[0]),
                    unpremultiply(pixel[1]),
                    unpremultiply(pixel[2]),
                    alpha,
                ]
            }
        })
        .collect()
}

fn escape_svg_text(text: &str) -> String {
    text.chars()
        .flat_map(|ch| match ch {
            '&' => "&amp;".chars().collect::<Vec<_>>(),
            '<' => "&lt;".chars().collect::<Vec<_>>(),
            '>' => "&gt;".chars().collect::<Vec<_>>(),
            '"' => "&quot;".chars().collect::<Vec<_>>(),
            '\'' => "&apos;".chars().collect::<Vec<_>>(),
            _ => vec![ch],
        })
        .collect()
}

fn load_formula_texture(
    ui: &mut egui::Ui,
    image: &RenderedFormulaImage,
    readable_text: &str,
    display: bool,
) -> Option<TextureHandle> {
    let key = FormulaTextureCacheKey {
        label: readable_text.to_owned(),
        display,
        size: image.size,
        rgba_hash: hash_formula_rgba(&image.rgba),
    };
    if let Some(texture) = FORMULA_TEXTURE_CACHE.with(|cache| cache.borrow().get(&key).cloned()) {
        return Some(texture);
    }
    let color_image = ColorImage::from_rgba_unmultiplied(image.size, &image.rgba);
    let texture = ui.ctx().load_texture(
        format!(
            "formula:{}:{}",
            if display { "display" } else { "inline" },
            readable_text
        ),
        color_image,
        egui::TextureOptions::LINEAR,
    );
    FORMULA_TEXTURE_CACHE.with(|cache| {
        cache.borrow_mut().insert(key, texture.clone());
    });
    Some(texture)
}

fn hash_formula_rgba(rgba: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    rgba.hash(&mut hasher);
    hasher.finish()
}

fn render_formula_cached(ctx: &egui::Context, latex: &str, display: bool) -> RenderedFormula {
    let mut rendered = FORMULA_RENDER_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let mut renderer = FallbackFormulaRenderer;
        cache.render_with(&mut renderer, latex, display)
    });
    if let Some(remote_image) = get_async_remote_formula_image(ctx, latex, display) {
        rendered.image = Some(remote_image);
    }
    rendered
}

fn parse_inline_formula_segments(text: &str) -> Vec<InlineSegment<'_>> {
    let mut segments = Vec::new();
    let mut cursor = 0usize;
    while cursor < text.len() {
        let Some(formula) = find_next_inline_formula(text, cursor) else {
            segments.push(InlineSegment::Text(&text[cursor..]));
            break;
        };

        if formula.start > cursor {
            segments.push(InlineSegment::Text(&text[cursor..formula.start]));
        }
        segments.push(InlineSegment::Formula {
            latex: &text[formula.content_start..formula.content_end],
            display: formula.display,
        });
        cursor = formula.end;
    }
    if text.is_empty() {
        segments.push(InlineSegment::Text(""));
    }
    segments
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct InlineFormulaMatch {
    start: usize,
    end: usize,
    content_start: usize,
    content_end: usize,
    display: bool,
}

fn find_next_inline_formula(text: &str, cursor: usize) -> Option<InlineFormulaMatch> {
    let mut candidates = [
        find_delimited_formula(text, cursor, r"\(", r"\)", false),
        find_delimited_formula(text, cursor, r"\[", r"\]", true),
        find_delimited_formula(text, cursor, "$$", "$$", true),
        find_dollar_formula(text, cursor),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    candidates.sort_by_key(|candidate| candidate.start);
    candidates.into_iter().next()
}

fn find_delimited_formula(
    text: &str,
    cursor: usize,
    open: &str,
    close: &str,
    display: bool,
) -> Option<InlineFormulaMatch> {
    let start = find_unescaped_str(text, open, cursor)?;
    let content_start = start + open.len();
    let close_start = find_unescaped_str(text, close, content_start)?;
    (close_start > content_start).then_some(InlineFormulaMatch {
        start,
        end: close_start + close.len(),
        content_start,
        content_end: close_start,
        display,
    })
}

fn find_dollar_formula(text: &str, mut cursor: usize) -> Option<InlineFormulaMatch> {
    while let Some(start) = find_unescaped_str(text, "$", cursor) {
        if text[start..].starts_with("$$") {
            cursor = start + 2;
            continue;
        }
        let content_start = start + 1;
        let close_start = find_unescaped_str(text, "$", content_start)?;
        if text[close_start..].starts_with("$$") {
            cursor = close_start + 2;
            continue;
        }
        let content = &text[content_start..close_start];
        if looks_like_latex_math(content) {
            return Some(InlineFormulaMatch {
                start,
                end: close_start + 1,
                content_start,
                content_end: close_start,
                display: false,
            });
        }
        cursor = close_start + 1;
    }
    None
}

fn find_unescaped_str(text: &str, needle: &str, cursor: usize) -> Option<usize> {
    let mut search_from = cursor;
    while search_from <= text.len() {
        let relative = text[search_from..].find(needle)?;
        let absolute = search_from + relative;
        if !is_escaped(text, absolute) {
            return Some(absolute);
        }
        search_from = absolute + needle.len();
    }
    None
}

fn is_escaped(text: &str, index: usize) -> bool {
    let mut backslashes = 0usize;
    for ch in text[..index].chars().rev() {
        if ch == '\\' {
            backslashes += 1;
        } else {
            break;
        }
    }
    backslashes % 2 == 1
}

fn looks_like_latex_math(content: &str) -> bool {
    let trimmed = content.trim();
    !trimmed.is_empty()
        && trimmed.chars().any(|ch| {
            matches!(
                ch,
                '\\' | '_' | '^' | '=' | '+' | '-' | '*' | '/' | '<' | '>' | '{' | '}'
            )
        })
}

fn strip_markdown_inline(text: &str) -> String {
    text.replace("**", "").replace('`', "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct CountingRenderer {
        calls: usize,
    }

    impl FormulaRenderer for CountingRenderer {
        fn render_formula(&mut self, latex: &str, display: bool) -> Option<RenderedFormula> {
            self.calls += 1;
            Some(RenderedFormula {
                image: None,
                readable_text: format!("{}:{latex}", if display { "display" } else { "inline" }),
            })
        }
    }

    #[test]
    fn parses_bracketed_latex_block() {
        let lines = [
            r"\[ \text{行百分比} = \frac{\text{行内单元格频数}}{\text{该行合计}} \times 100\% \]",
            "下一段",
        ];

        let (formula, consumed) = parse_latex_block(&lines).expect("latex block");

        assert_eq!(consumed, 1);
        assert!(formula.contains(r"\frac"));
    }

    #[test]
    fn latex_fraction_is_rendered_as_readable_text() {
        let text = latex_to_readable_text(
            r"\text{行百分比} = \frac{\text{行内单元格频数}}{\text{该行合计}} \times 100\%",
        );

        assert!(text.contains("行百分比"));
        assert!(text.contains("行内单元格频数"));
        assert!(text.contains("该行合计"));
        assert!(text.contains('×'));
        assert!(text.contains("100%"));
    }

    #[test]
    fn fraction_fallback_uses_plain_single_line_text() {
        let text = latex_to_readable_text(r"\frac{a}{b} + \frac{x_i}{n}");

        assert!(text.contains("(a)/(b)"));
        assert!(text.contains("(x_i)/(n)"));
        assert!(!text.contains('─'));
    }

    #[test]
    fn nested_fraction_fallback_stays_single_line() {
        let text = latex_to_readable_text(r"\frac{\frac{a}{b}}{c + d}");

        assert_eq!(text, "((a)/(b))/(c + d)");
        assert!(!text.contains('\n'));
    }

    #[test]
    fn display_fraction_no_longer_uses_multiline_bar() {
        let text = latex_to_readable_text_with_mode(
            r"\frac{\text{行内单元格频数}}{\text{该行合计}}",
            true,
        );

        assert!(text.contains("行内单元格频数"));
        assert!(text.contains("该行合计"));
        assert!(!text.contains('─'));
        assert_eq!(text.lines().count(), 1);
    }

    #[test]
    fn formula_svg_image_rasterizes_to_rgba_pixels() {
        let image = render_formula_svg_image("a/b", false).expect("formula image");

        assert!(image.size[0] > 0);
        assert!(image.size[1] > 0);
        assert_eq!(image.rgba.len(), image.size[0] * image.size[1] * 4);
        assert!(image.rgba.chunks_exact(4).any(|pixel| pixel[3] > 0));
    }

    #[test]
    fn formula_svg_uses_embedded_fonts_for_non_ascii_text() {
        let image = render_formula_svg_image("行百分比 μ p^", false).expect("formula image");

        assert!(image.size[0] > 0);
        assert!(image.size[1] > 0);
        assert!(image.rgba.chunks_exact(4).any(|pixel| pixel[3] > 0));
    }

    #[test]
    fn inline_formula_svg_stays_near_text_line_height() {
        let inline = render_formula_svg_image("H_0", false).expect("inline formula image");
        let display = render_formula_svg_image("H_0", true).expect("display formula image");

        assert!(
            inline.size[1] <= 20,
            "inline formula is too tall: {}px",
            inline.size[1]
        );
        assert!(display.size[1] > inline.size[1]);
    }

    #[test]
    fn formula_svg_text_is_escaped() {
        assert_eq!(
            escape_svg_text("a < b && c > d"),
            "a &lt; b &amp;&amp; c &gt; d"
        );
    }

    #[test]
    fn combining_hat_is_preserved_for_font_rendering() {
        let text = latex_to_readable_text("p̂ = 0.5");

        assert_eq!(text, "p̂ = 0.5");
        assert!(text.contains('\u{0302}'));
    }

    #[test]
    fn latex_hat_is_converted_to_combining_hat() {
        assert_eq!(latex_to_readable_text(r"\hat{p}"), "p̂");
        assert_eq!(latex_to_readable_text(r"\widehat{AB}"), "AB̂");
    }

    #[test]
    fn remote_formula_url_replaces_latex_placeholder() {
        let url = build_remote_formula_url(CODECOGS_SVG_URL_TEMPLATE, r"H_0 \le \mu_0", false)
            .expect("remote url");

        assert_eq!(
            url,
            "https://latex.codecogs.com/svg.image?H_0%20%5Cle%20%5Cmu_0"
        );
    }

    #[test]
    fn remote_formula_url_adds_latex_query_when_no_placeholder() {
        let url = build_remote_formula_url("https://example.test/render", r"x^2", false)
            .expect("remote url");

        assert_eq!(url, "https://example.test/render?latex=x%5E2");
    }

    #[test]
    fn display_remote_formula_url_adds_displaystyle() {
        let url = build_remote_formula_url(
            "https://example.test/render?eq={latex}",
            r"\frac{a}{b}",
            true,
        )
        .expect("remote url");

        assert_eq!(
            url,
            "https://example.test/render?eq=%5Cdisplaystyle%20%5Cfrac%7Ba%7D%7Bb%7D"
        );
    }

    #[test]
    fn svg_detection_accepts_xml_preamble() {
        assert!(looks_like_svg(
            r#"<svg xmlns="http://www.w3.org/2000/svg"></svg>"#
        ));
        assert!(looks_like_svg(
            r#"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg"></svg>"#
        ));
        assert!(!looks_like_svg("not svg"));
    }

    #[test]
    fn fallback_renderer_includes_svg_image_when_possible() {
        let mut renderer = FallbackFormulaRenderer;
        let rendered = renderer
            .render_formula(r"\frac{a}{b}", false)
            .expect("rendered formula");

        assert_eq!(rendered.readable_text, "(a)/(b)");
        assert!(rendered.image.is_some());
    }

    #[test]
    fn common_inline_math_commands_are_readable_fallback() {
        let text = latex_to_readable_text(r"\sum (x_i - \bar{x})^2");

        assert!(text.contains('∑'));
        assert!(text.contains("x̅"));
        assert!(text.contains("x_i"));
    }

    #[test]
    fn parses_parenthesized_inline_latex_segments() {
        let segments =
            parse_inline_formula_segments(r"离差平方和（\(\sum (x_i - \bar{x})^2\)）通常大于零");

        assert_eq!(
            segments,
            vec![
                InlineSegment::Text("离差平方和（"),
                InlineSegment::Formula {
                    latex: r"\sum (x_i - \bar{x})^2",
                    display: false,
                },
                InlineSegment::Text("）通常大于零"),
            ]
        );
    }

    #[test]
    fn parses_inline_and_display_latex_in_one_line() {
        let segments = parse_inline_formula_segments(r"A $x_i^2$ B \[\frac{a}{b}\] C");

        assert_eq!(
            segments,
            vec![
                InlineSegment::Text("A "),
                InlineSegment::Formula {
                    latex: "x_i^2",
                    display: false,
                },
                InlineSegment::Text(" B "),
                InlineSegment::Formula {
                    latex: r"\frac{a}{b}",
                    display: true,
                },
                InlineSegment::Text(" C"),
            ]
        );
    }

    #[test]
    fn ignores_escaped_and_non_math_dollar_text() {
        let segments =
            parse_inline_formula_segments(r"价格是 \$5，变量是 $x_i$，普通 $abc$ 不处理");

        assert_eq!(
            segments,
            vec![
                InlineSegment::Text(r"价格是 \$5，变量是 "),
                InlineSegment::Formula {
                    latex: "x_i",
                    display: false,
                },
                InlineSegment::Text("，普通 $abc$ 不处理"),
            ]
        );
    }

    #[test]
    fn formula_cache_reuses_same_renderer_output() {
        let mut cache = FormulaRenderCache::default();
        let mut renderer = CountingRenderer::default();

        let first = cache.render_with(&mut renderer, r"\sum x_i", false);
        let second = cache.render_with(&mut renderer, r" \sum x_i ", false);
        let third = cache.render_with(&mut renderer, r"\sum x_i", true);

        assert_eq!(first, second);
        assert_ne!(first, third);
        assert_eq!(renderer.calls, 2);
    }

    #[test]
    fn remote_formula_cache_can_store_ready_image() {
        clear_formula_render_caches();
        let key = RemoteFormulaCacheKey {
            latex: r"x^2".to_owned(),
            display: false,
            url_template: CODECOGS_SVG_URL_TEMPLATE.to_owned(),
        };
        let image = RenderedFormulaImage {
            size: [1, 1],
            rgba: vec![255, 255, 255, 255],
        };

        remote_formula_cache()
            .lock()
            .expect("remote formula cache")
            .insert(key.clone(), RemoteFormulaState::Ready(Some(image.clone())));

        let stored = remote_formula_cache()
            .lock()
            .expect("remote formula cache")
            .get(&key)
            .cloned();
        assert!(matches!(stored, Some(RemoteFormulaState::Ready(Some(value))) if value == image));
        clear_formula_render_caches();
    }

    #[test]
    fn disabled_remote_formula_rendering_does_not_enqueue_work() {
        clear_formula_render_caches();
        apply_formula_render_settings(FormulaRenderSettings::default());
        let ctx = egui::Context::default();

        assert!(get_async_remote_formula_image(&ctx, r"x^2", false).is_none());
        assert!(
            remote_formula_cache()
                .lock()
                .expect("remote formula cache")
                .is_empty()
        );
    }
}

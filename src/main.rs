#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use eframe::egui;
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontFamily, FontId, RichText, Stroke};
use i18n::{tr, Lang};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

mod i18n;
mod editor;
mod recovery;
mod large_editor;
mod text_tools;
mod changes;
mod linkage;
mod shortcuts;
mod line_commands;
mod json_plugin;
mod workspace_theme;
mod whitespace;
mod terminal;
mod updater;
mod file_open;
mod text_conversion;
mod web_tools;
mod curl_plugin;
mod windows;
mod new_window;

const HEX_BYTES_PER_ROW: usize = 16;
const BINARY_SAMPLE_SIZE: usize = 8192;

/// 字体链是否包含控制图片字形（␀ ␊ ␍ 等），决定"显示不可见内容"的降级渲染。
static CONTROL_PICTURES: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

fn control_pictures_ok() -> bool {
    *CONTROL_PICTURES.get().unwrap_or(&false)
}

static OPEN_REQUEST: AtomicBool = AtomicBool::new(false);

fn main() -> eframe::Result {
    new_window::initialize();
    if updater::run_helper_if_requested() { return Ok(()); }
    let _runtime_lock = match updater::hold_runtime_lock() {
        Ok(lock) => lock,
        Err(error) => { eprintln!("Unable to initialize runtime lock: {error}"); return Ok(()); }
    };
    let _window_lock = match new_window::lock_current() {
        Ok(lock) => lock,
        Err(error) => { eprintln!("Unable to lock window session: {error}"); return Ok(()); }
    };
    let paths = file_open::argument_paths(std::env::args_os().skip(1));
    #[cfg(target_os = "macos")]
    let _file_open_handler = file_open::macos::install();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("LiteText")
        .with_icon(app_icon())
        .with_min_inner_size([640.0, 420.0])
        .with_inner_size([1080.0, 720.0]);
    // 恢复上次关闭时的窗口大小与位置
    if let Some(ws) = load_window_state() {
        viewport = viewport.with_inner_size(ws.size).with_position(ws.pos);
    }
    let options = eframe::NativeOptions {
        viewport,
        persistence_path: new_window::directory().map(|p| p.join("app.ron")),
        ..Default::default()
    };
    eframe::run_native(
        "LiteText",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
            #[cfg(target_os = "macos")]
            file_open::macos::bind(&cc.egui_ctx);
            setup_fonts(&cc.egui_ctx);
            let mut app: LiteTextApp = cc
                .storage
                .and_then(|s| eframe::get_value(s, eframe::APP_KEY))
                .or_else(new_window::seed)
                .unwrap_or_default();
            app.restore_session();
            app.open_paths(paths);
            Ok(Box::new(app))
        }),
    )
}

fn app_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../assets/app-icon.png"))
        .expect("embedded LiteText icon must be a valid PNG")
}

// ---------------------------------------------------------------------------
// 主题与调色板
// ---------------------------------------------------------------------------

/// Legacy configuration only; active themes live in workspace_theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Theme {
    #[default]
    Dark,
    Light,
    System,
}

/// 一套完整的界面配色。视图函数全部从调色板取色，不使用硬编码颜色。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Palette {
    dark: bool,
    accent: Color32,
    accent_hover: Color32,
    danger: Color32,
    recovery_pending: Color32,
    recovery_backed_up: Color32,
    bg_window: Color32,
    bg_panel: Color32,
    bg_elevated: Color32,
    bg_inset: Color32,
    text_main: Color32,
    text_faint: Color32,
}

static DARK: Palette = Palette {
    dark: true,
    accent: Color32::from_rgb(0x5B, 0x9D, 0xFF),
    accent_hover: Color32::from_rgb(0x74, 0xAF, 0xFF),
    danger: Color32::from_rgb(0xE8, 0x6A, 0x5E),
    recovery_pending: Color32::from_rgb(0xBD, 0x9C, 0x52),
    recovery_backed_up: Color32::from_rgb(0x68, 0xA1, 0x7B),
    bg_window: Color32::from_rgb(0x16, 0x18, 0x1D),
    bg_panel: Color32::from_rgb(0x1B, 0x1E, 0x24),
    bg_elevated: Color32::from_rgb(0x25, 0x29, 0x31),
    bg_inset: Color32::from_rgb(0x11, 0x13, 0x18),
    text_main: Color32::from_rgb(0xDE, 0xE3, 0xEC),
    text_faint: Color32::from_rgb(0x77, 0x80, 0x92),
};

static LIGHT: Palette = Palette {
    dark: false,
    accent: Color32::from_rgb(0x3D, 0x7E, 0xEB),
    accent_hover: Color32::from_rgb(0x5B, 0x93, 0xF0),
    danger: Color32::from_rgb(0xD9, 0x50, 0x3F),
    recovery_pending: Color32::from_rgb(0xAC, 0x87, 0x36),
    recovery_backed_up: Color32::from_rgb(0x53, 0x8D, 0x65),
    bg_window: Color32::from_rgb(0xF2, 0xF3, 0xF5),
    bg_panel: Color32::from_rgb(0xFA, 0xFB, 0xFC),
    bg_elevated: Color32::from_rgb(0xE9, 0xEC, 0xF0),
    bg_inset: Color32::from_rgb(0xE4, 0xE7, 0xEB),
    text_main: Color32::from_rgb(0x24, 0x28, 0x2F),
    text_faint: Color32::from_rgb(0x8A, 0x91, 0x9E),
};

fn apply_style(ctx: &egui::Context, pal: &Palette) {
    let mut style = (*ctx.style()).clone();
    // dark_mode alone does not change egui's inherited light widget fills.
    // Rebase all states before applying LiteText's palette on every switch.
    style.visuals = if pal.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    let v = &mut style.visuals;
    v.dark_mode = pal.dark;
    v.window_fill = pal.bg_window;
    v.panel_fill = pal.bg_window;
    v.extreme_bg_color = pal.bg_inset;
    v.faint_bg_color = pal.bg_elevated;
    v.override_text_color = Some(pal.text_main);
    v.selection.bg_fill = pal.accent.gamma_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0_f32, pal.accent);
    v.widgets.noninteractive.bg_stroke = Stroke::NONE;
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, pal.text_main);
    // 幽灵按钮：常态无底色无边框，悬停淡显，沉浸感
    v.widgets.inactive.bg_fill = Color32::TRANSPARENT;
    v.widgets.inactive.weak_bg_fill = pal.bg_elevated;
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, pal.text_main);
    v.widgets.hovered.bg_fill = pal.bg_elevated;
    v.widgets.hovered.weak_bg_fill = pal.bg_elevated;
    v.widgets.hovered.bg_stroke = Stroke::NONE;
    v.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, if pal.dark { Color32::WHITE } else { pal.text_main });
    v.widgets.active.bg_fill = pal.accent.gamma_multiply(0.35);
    v.widgets.active.weak_bg_fill = pal.accent.gamma_multiply(0.35);
    v.widgets.active.fg_stroke = Stroke::new(1.0_f32, if pal.dark { Color32::WHITE } else { pal.text_main });
    // 下拉框展开态与弹层描边（两套主题都取调色板）
    v.widgets.open.weak_bg_fill = pal.bg_elevated;
    v.widgets.open.bg_fill = pal.bg_elevated;
    v.widgets.open.fg_stroke = Stroke::new(1.0_f32, pal.text_main);
    v.window_stroke = Stroke::new(1.0_f32, pal.bg_elevated);
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(12.0, 5.0);
    ctx.set_style(style);
}

/// 加载系统中文字体，保证中文界面在各平台正常渲染。
///
/// 根因修复：Hiragino 的 hhea.lineGap 高达 0.5em，使实现行高膨胀为 1.5 倍，
/// egui 行盒居中时会把中文基线推高约 3pt（实测），这是"中文比英文数字高"的根源。
/// 读入后直接在内存中把 lineGap 置 0，行盒恢复正常，中文基线与拉丁自然对齐。
/// 剩余的微小偏差（中文墨迹中心 0.376em vs 数字 0.313em）用 y_offset_factor 光学校正。
fn setup_fonts(ctx: &egui::Context) {
    const FONT_REGULAR: &[u8] = include_bytes!("../assets/fonts/HarmonyOS_Sans_Regular.ttf");
    const FONT_MEDIUM: &[u8] = include_bytes!("../assets/fonts/HarmonyOS_Sans_Medium.ttf");

    let mut fonts = egui::FontDefinitions::default();

    // 内置 HarmonyOS Sans：Proportional 首位 + 品牌 Medium 单独成族
    fonts.font_data.insert(
        "harmonyos".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(FONT_REGULAR)),
    );
    fonts.font_data.insert(
        "harmonyos-medium".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(FONT_MEDIUM)),
    );
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "harmonyos".to_owned());
    fonts
        .families
        .entry(FontFamily::Name("brand".into()))
        .or_default()
        .push("harmonyos-medium".to_owned());

    // 系统中文字体回退（egui 默认字体不含 CJK 字形）
    const CJK_CANDIDATES: &[&str] = &[
        "/System/Library/Fonts/Hiragino Sans GB.ttc",             // macOS 简体中文
        "/System/Library/Fonts/STHeiti Light.ttc",                // macOS 旧版
        "/System/Library/Fonts/STHeiti Medium.ttc",
        "/System/Library/Fonts/PingFang.ttc",                     // 部分 macOS 版本
        "C:\\Windows\\Fonts\\msyh.ttc",                           // Windows 微软雅黑
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", // Debian/Ubuntu
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",      // Arch
    ];
    for path in CJK_CANDIDATES {
        if let Some(data) = load_validated_cjk(path) {
            fonts
                .font_data
                .insert("cjk".to_owned(), std::sync::Arc::new(data));
            for family in [FontFamily::Proportional, FontFamily::Monospace] {
                fonts.families.entry(family).or_default().push("cjk".to_owned());
            }
            break;
        }
    }

    // 符号字体回退：提供控制图片字形（␀ ␊ ␍ 等，用于"显示不可见内容"）
    const SYMBOL_CANDIDATES: &[&str] = &[
        "/System/Library/Fonts/Apple Symbols.ttf",                     // macOS
        "C:\\Windows\\Fonts\\seguisym.ttf",                            // Windows
        "/usr/share/fonts/truetype/noto/NotoSansSymbols2-Regular.ttf", // Linux
    ];
    let mut symbols_ok = false;
    for path in SYMBOL_CANDIDATES {
        if let Ok(bytes) = std::fs::read(path)
            && let Ok(font) = ab_glyph::FontVec::try_from_vec_and_index(bytes.clone(), 0)
            && ab_glyph::Font::glyph_id(&font, '␀').0 != 0
        {
            fonts.font_data.insert(
                "symbols".to_owned(),
                std::sync::Arc::new(egui::FontData::from_owned(bytes)),
            );
            for family in [FontFamily::Proportional, FontFamily::Monospace] {
                fonts.families.entry(family).or_default().push("symbols".to_owned());
            }
            symbols_ok = true;
            break;
        }
    }
    let _ = CONTROL_PICTURES.set(symbols_ok);

    ctx.set_fonts(fonts);
}

/// 读取并校验一个系统字体文件：必须能被 ab_glyph 解析，且真的包含中文字形。
fn load_validated_cjk(path: &str) -> Option<egui::FontData> {
    const CJK_Y_OFFSET_FACTOR: f32 = 0.04;
    let mut bytes = std::fs::read(path).ok()?;
    let font = ab_glyph::FontVec::try_from_vec_and_index(bytes.clone(), 0).ok()?;
    if ab_glyph::Font::glyph_id(&font, '中').0 == 0 {
        return None;
    }
    patch_cjk_line_gap(&mut bytes);
    let mut data = egui::FontData::from_owned(bytes);
    data.tweak.y_offset_factor = CJK_Y_OFFSET_FACTOR;
    Some(data)
}

/// 将 sfnt/TTC 字体中 hhea 表的 lineGap 置 0。解析失败则原样返回（不影响使用）。
fn patch_cjk_line_gap(data: &mut [u8]) {
    let Some(base) = sfnt_base(data) else { return };
    if base + 6 > data.len() {
        return;
    }
    let num_tables = u16::from_be_bytes([data[base + 4], data[base + 5]]) as usize;
    for i in 0..num_tables {
        let rec = base + 12 + i * 16;
        if rec + 16 > data.len() {
            return;
        }
        if &data[rec..rec + 4] == b"hhea" {
            let off = u32::from_be_bytes([data[rec + 8], data[rec + 9], data[rec + 10], data[rec + 11]])
                as usize;
            if off + 10 <= data.len() {
                data[off + 8..off + 10].copy_from_slice(&[0, 0]);
            }
            return;
        }
    }
}

/// 返回 sfnt（含 TTC 首个字体）表目录的起始偏移。
fn sfnt_base(data: &[u8]) -> Option<usize> {
    if data.len() < 12 {
        return None;
    }
    if &data[0..4] == b"ttcf" {
        Some(u32::from_be_bytes([data[12], data[13], data[14], data[15]]) as usize)
    } else {
        Some(0)
    }
}

// ---------------------------------------------------------------------------
// 窗口几何持久化
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Default)]
struct WindowState {
    size: [f32; 2],
    pos: [f32; 2],
}

fn window_state_path() -> Option<PathBuf> {
    eframe::storage_dir("LiteText").map(|d| d.join("window.json"))
}

fn load_window_state() -> Option<WindowState> {
    let data = std::fs::read_to_string(window_state_path()?).ok()?;
    serde_json::from_str(&data).ok()
}

fn save_window_state(ws: &WindowState) {
    let Some(path) = window_state_path() else { return };
    if let Ok(json) = serde_json::to_string_pretty(ws) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, json);
    }
}

// ---------------------------------------------------------------------------
// 状态
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct LiteTextApp {
    lang: Lang,
    #[serde(rename = "theme", skip_serializing)]
    legacy_theme: Option<Theme>,
    workspace_theme: workspace_theme::WorkspaceTheme,
    workspace_font_size: f32,
    tab_mode: whitespace::TabMode,
    terminal: terminal::Settings,
    updates: updater::Settings,
    #[serde(skip)]
    updater: updater::State,
    #[serde(skip)]
    terminal_installed: Option<Vec<terminal::Installed>>,
    #[serde(skip)]
    terminal_pending: Option<std::sync::mpsc::Receiver<Result<(), String>>>,
    #[serde(skip)]
    workspace_theme_page: bool,
    shortcut_keys: shortcuts::Keymap,
    #[serde(skip)]
    shortcut_recording: Option<shortcuts::Action>,
    #[serde(skip)]
    shortcut_error: Option<String>,
    // HEX 视图偏好（hex_upper 改名以丢弃旧版本可能存错的小写默认值）
    #[serde(rename = "hex_case_upper")]
    hex_upper: bool,
    dim_zeros: bool,
    show_ascii: bool,
    show_invisible: bool,
    /// Data Inspector 面板宽度
    inspector_width: f32,
    /// 最近打开的文件（最多 10 条，最新的在前）
    recent_files: Vec<PathBuf>,
    /// 当前激活的文件 id
    active_id: Option<u64>,
    #[serde(skip)]
    applied: Option<Palette>,
    #[serde(skip)]
    show_settings: bool,
    #[serde(skip)]
    shortcuts_page: bool,
    #[serde(skip)]
    show_menu: bool,
    #[serde(skip)]
    menu_recent_open: bool,
    #[serde(skip)]
    menu_plugins_open: bool,
    #[serde(skip)]
    menu_whitespace_open: bool,
    #[serde(skip)]
    menu_conversion_open: bool,
    #[serde(skip)]
    show_curl_menu: bool,
    #[cfg(test)]
    #[serde(skip)]
    new_window_requested: bool,
    #[serde(skip)]
    plugin_error: Option<String>,
    #[serde(skip)]
    focus_editor: Option<u64>,
    /// 选中的字节区间（含端点）
    #[serde(skip)]
    selection: Option<(usize, usize)>,
    /// 拖拽选择的起始锚点
    #[serde(skip)]
    drag_anchor: Option<usize>,
    /// Data Inspector 双击弹窗展示的完整内容
    #[serde(skip)]
    inspector_popup: Option<String>,
    /// 弹窗弹出位置（双击点）
    #[serde(skip)]
    popup_pos: Option<egui::Pos2>,
    /// 弹窗打开时刻（用于"点击外部关闭"的防误触）
    #[serde(skip)]
    popup_opened_at: Option<f64>,
    /// 正在拖拽排序的标签 id
    #[serde(skip)]
    tab_drag_id: Option<u64>,
    /// 拖拽开始时指针相对标签左缘的偏移
    #[serde(skip)]
    tab_grab_dx: f32,
    /// 右侧工具栏实测宽度（上一帧），用于标签区自适应
    #[serde(skip)]
    right_controls_w: f32,
    /// 已打开的文件标签页（按打开顺序）
    #[serde(skip)]
    files: Vec<LoadedFile>,
    #[serde(skip)]
    next_file_id: u64,
    /// 最近一帧的窗口尺寸（外框），用于退出时持久化
    #[serde(skip)]
    window_size: Option<[f32; 2]>,
    /// 最近一帧的窗口位置（外框左上角）
    #[serde(skip)]
    window_pos: Option<[f32; 2]>,
    #[serde(skip)]
    last_recovery: Option<std::time::Instant>,
    #[serde(skip)]
    recovery_key: Option<RecoveryKey>,
    #[serde(skip)]
    recovery_stamp: Option<(u64, std::time::SystemTime)>,
    /// Content versions actually included in the last successful local recovery write.
    #[serde(skip)]
    recovery_versions: std::collections::HashMap<u64, (u64, u64)>,
    #[serde(skip)]
    io_error: Option<String>,
    #[serde(skip)]
    pending_close: Option<TabAction>,
    #[serde(skip)]
    pending_save: Option<linkage::PendingSave>,
    #[serde(skip)]
    last_disk_check: Option<std::time::Instant>,
    #[serde(skip)]
    last_checked_tab: Option<u64>,
    #[serde(skip)]
    had_focus: bool,
    #[cfg(test)]
    #[serde(skip)]
    recovery_path_override: Option<PathBuf>,
}

impl Default for LiteTextApp {
    fn default() -> Self {
        Self {
            lang: Lang::default(),
            legacy_theme: None,
            workspace_theme: Default::default(),
            workspace_font_size: 13.5,
            tab_mode: Default::default(),
            terminal: Default::default(),
            updates: Default::default(),
            updater: Default::default(),
            terminal_installed: None,
            terminal_pending: None,
            workspace_theme_page: false,
            shortcut_keys: Default::default(),
            shortcut_recording: None,
            shortcut_error: None,
            hex_upper: true,
            dim_zeros: false,
            show_ascii: true,
            show_invisible: false,
            applied: None,
            show_settings: false,
            shortcuts_page: false,
            show_menu: false,
            menu_recent_open: false,
            menu_plugins_open: false,
            menu_whitespace_open: false,
            menu_conversion_open: false,
            show_curl_menu: false,
            #[cfg(test)]
            new_window_requested: false,
            plugin_error: None,
            focus_editor: None,
            selection: None,
            drag_anchor: None,
            inspector_popup: None,
            inspector_width: 190.0,
            recent_files: Vec::new(),
            active_id: None,
            popup_opened_at: None,
            tab_drag_id: None,
            tab_grab_dx: 0.0,
            right_controls_w: 105.0,
            popup_pos: None,
            files: Vec::new(),
            next_file_id: 0,
            window_size: None,
            window_pos: None,
            last_recovery: None,
            recovery_key: None,
            recovery_stamp: None,
            recovery_versions: Default::default(),
            io_error: None,
            pending_close: None,
            pending_save: None,
            last_disk_check: None,
            last_checked_tab: None,
            had_focus: false,
            #[cfg(test)]
            recovery_path_override: None,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct LoadedFile {
    id: u64,
    path: PathBuf,
    name: String,
    // 内容不入持久化存储：恢复会话时从磁盘重读
    #[serde(skip)]
    bytes: Vec<u8>,
    is_binary: bool,
    #[serde(skip)]
    lines: Vec<String>,
    view: ViewMode,
    #[serde(default)]
    language_override: Option<String>,
    /// 所有可编辑文本的工作缓冲区。新建文件的 path 为空。
    #[serde(skip)]
    draft_text: Option<String>,
    #[serde(skip)]
    editor_state: editor::EditorState,
    #[serde(skip)]
    encoding: recovery::Encoding,
    /// 上次从原文件读取/手动保存的字节，用于修改判断和外部变更保护。
    #[serde(skip)]
    saved_bytes: Vec<u8>,
    #[serde(skip)]
    character_count: usize,
    #[serde(skip)]
    saved_large_revision: Option<u64>,
    #[serde(skip)]
    bytes_dirty: bool,
    #[serde(skip)]
    content_revision: u64,
    #[serde(skip)]
    saved_encoding: recovery::Encoding,
    #[serde(skip)]
    source_path: Option<PathBuf>,
    #[serde(skip)]
    disk: linkage::DiskState,
}

impl LoadedFile {
    fn prepare_changes(&mut self) {
        if self.editor_state.changes.baseline.is_none()
            && (!self.path.as_os_str().is_empty() || self.source_path.is_some())
            && self.draft_text.is_some()
        {
            let text = self.saved_encoding.decode_as(&self.saved_bytes)
                .unwrap_or_else(|| recovery::Encoding::decode(&self.saved_bytes).1);
            self.editor_state.changes.baseline = Some(changes::lines(&text));
            self.editor_state.changes.revision = None;
        }
        if !self.editor_state.large.active && self.editor_state.changes.revision != Some(self.content_revision)
            && let Some(text) = &self.draft_text {
            let current = changes::lines(text);
            self.editor_state.changes.update(&current, self.content_revision);
        }
    }

    fn sync_draft(&mut self) {
        if let Some(text) = &self.draft_text {
            self.bytes = self.encoding.encode(text).unwrap_or_else(|_| text.as_bytes().to_vec());
            self.lines = if text.len() < large_editor::THRESHOLD { text.split('\n').map(str::to_owned).collect() } else { Vec::new() };
            self.character_count = text.chars().count();
            if self.editor_state.large.active {
                let tools = self.editor_state.tools.clone();
                self.editor_state = editor::EditorState::default();
                self.editor_state.tools = tools;
            }
            else {
                self.editor_state.invalidate();
                self.editor_state.large.set_encoding(text, recovery::Encoding::Utf8);
            }
            self.saved_large_revision = None;
            self.bytes_dirty = false;
            self.content_revision += 1;
        }
        self.prepare_changes();
    }

    fn is_modified(&self) -> bool {
        self.draft_text.is_some() && (self.path.as_os_str().is_empty() || self.encoding != self.saved_encoding || if self.editor_state.large.active {
            self.saved_large_revision != Some(self.editor_state.large.revision)
        } else { self.bytes != self.saved_bytes })
    }

    /// User-visible unsaved changes, shared by tab tinting and close prompts.
    /// Empty scratch tabs still need recovery, but have no user content to lose.
    /// A detached file is not a scratch tab: clearing it is an unsaved change.
    fn has_unsaved_changes(&self) -> bool {
        let empty_scratch = self.path.as_os_str().is_empty()
            && self.source_path.is_none()
            && self.draft_text.as_ref().is_some_and(|text| text.is_empty());
        !empty_scratch && self.is_modified()
    }

    fn byte_len(&self) -> usize {
        if !self.bytes_dirty { return self.bytes.len(); }
        let text = self.draft_text.as_ref().expect("text buffer");
        match self.encoding {
            recovery::Encoding::Utf16Le | recovery::Encoding::Utf16Be => 2 + self.editor_state.large.utf16_units * 2,
            recovery::Encoding::Utf8Bom => 3 + text.len(),
            encoding if encoding.is_legacy() && self.editor_state.large.legacy_invalid == 0 => self.editor_state.large.legacy_bytes,
            _ => text.len(),
        }
    }

    fn ensure_bytes(&mut self) {
        if self.bytes_dirty {
            let text = self.draft_text.as_ref().expect("text buffer");
            self.bytes = self.encoding.encode(text).unwrap_or_else(|_| text.as_bytes().to_vec());
            self.bytes_dirty = false;
        }
    }

    fn line_count(&self) -> usize {
        if self.editor_state.large.active { self.editor_state.large.line_count() }
        else if let Some(text) = &self.draft_text { text.bytes().filter(|&b| b == b'\n').count() + 1 }
        else { self.lines.len() }
    }

    #[cfg(test)]
    fn change_encoding(&mut self, encoding: recovery::Encoding) -> std::io::Result<()> {
        if encoding == self.encoding { return Ok(()); }
        let text = self.draft_text.as_ref().ok_or_else(|| std::io::Error::other("Binary files have no text encoding"))?;
        if self.encoding == recovery::Encoding::Lossy {
            return Err(std::io::Error::other("The source encoding is unknown or contains decoding errors; refusing to convert replacement characters"));
        }
        // Validate before touching any buffer, history, baseline or recovery metadata.
        let bytes = encoding.encode(text)?;
        if !self.editor_state.large.active && self.saved_large_revision.is_none() {
            self.saved_large_revision = (!self.is_modified()).then_some(0);
        }
        self.editor_state.large.set_encoding(text, encoding);
        self.encoding = encoding;
        self.bytes = bytes;
        self.bytes_dirty = false;
        self.content_revision += 1;
        Ok(())
    }
}

#[derive(PartialEq, Clone, Copy, Serialize, Deserialize)]
enum ViewMode {
    Text,
    Hex,
}

/// 标签页右键菜单动作。
enum TabAction {
    Close(u64),
    CloseOthers(u64),
    CloseLeft(u64),
    CloseRight(u64),
    OpenFolder(u64),
}

enum MenuAction {
    Open,
    NewWindow,
    Recent(PathBuf),
    Settings,
    FormatJson,
    Whitespace(whitespace::Action),
    Conversion(text_conversion::Action),
    WebTool(web_tools::Action),
    CurlMenu,
}

type RecoveryKey = (Option<u64>, Vec<(u64, PathBuf, String, ViewMode, u64, u64, Option<String>)>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecoveryIndicator {
    Idle,
    Pending,
    BackedUp,
}

impl RecoveryIndicator {
    fn color(self, pal: &Palette) -> Color32 {
        match self {
            Self::Idle => pal.text_faint.gamma_multiply(0.45),
            Self::Pending => pal.recovery_pending,
            Self::BackedUp => pal.recovery_backed_up,
        }
    }
}

impl LiteTextApp {
    fn recovery_indicator(&self, file: &LoadedFile) -> RecoveryIndicator {
        // Untitled buffers need recovery even before their first edit, but their
        // empty initial state should not look like an unsaved user change.
        let untouched_blank = file.path.as_os_str().is_empty()
            && file.source_path.is_none()
            && file.draft_text.as_deref() == Some("")
            && file.encoding == file.saved_encoding;
        if !file.is_modified() || untouched_blank {
            RecoveryIndicator::Idle
        } else if self.recovery_versions.get(&file.id)
            == Some(&(file.content_revision, file.editor_state.large.revision)) {
            RecoveryIndicator::BackedUp
        } else {
            RecoveryIndicator::Pending
        }
    }

    fn remember_recovered_versions(&mut self) {
        self.recovery_versions = self.files.iter()
            .map(|file| (file.id, (file.content_revision, file.editor_state.large.revision)))
            .collect();
    }

    fn write_recovery(&mut self) -> std::io::Result<()> {
        let path = self.recovery_path().ok_or_else(|| std::io::Error::other("Application storage directory unavailable"))?;
        let key = (self.active_id, self.files.iter().map(|f| (f.id, f.path.clone(), f.name.clone(), f.view,
            f.content_revision, f.editor_state.large.revision, f.language_override.clone())).collect());
        let stamp = |path: &std::path::Path| std::fs::metadata(path).ok().and_then(|m| m.modified().ok().map(|t| (m.len(), t)));
        let external_change = stamp(&path) != self.recovery_stamp;
        if self.recovery_key.as_ref() != Some(&key) || external_change {
            // Never show green for a failed write (including a damaged/deleted record).
            self.recovery_versions.clear();
            let validate = self.recovery_key.is_none() || external_change;
            self.recovery_key = None;
            recovery::store_buffers(&path, &self.files, self.active_id, validate)?;
            self.recovery_key = Some(key);
            self.recovery_stamp = stamp(&path);
            self.remember_recovered_versions();
        }
        self.last_recovery = Some(std::time::Instant::now());
        Ok(())
    }

    fn recovery_path(&self) -> Option<PathBuf> {
        #[cfg(test)]
        if let Some(path) = &self.recovery_path_override {
            return Some(path.clone());
        }
        recovery::session_path()
    }

    fn backup_or_report(&mut self) -> bool {
        match self.write_recovery() {
            Ok(()) => true,
            Err(error) => {
                self.io_error = Some(format!("{}: {error}", tr(self.lang).recovery_failed));
                // Avoid retrying a failed write every frame.
                self.last_recovery = Some(std::time::Instant::now());
                false
            }
        }
    }

    fn save_file(&mut self, id: u64) -> bool {
        if let Some(file) = self.files.iter_mut().find(|f| f.id == id) {
            linkage::check(file, true);
            if file.disk.pending.is_some() { return false; }
        }
        let Some(file) = self.files.iter().find(|f| f.id == id) else { return false };
        if file.draft_text.is_none() { return true; }
        let target = if file.path.as_os_str().is_empty() {
            let Some(path) = rfd::FileDialog::new().set_file_name(format!("{}.txt", file.name)).save_file() else { return false };
            path
        } else { file.path.clone() };
        if self.files.iter().any(|f| f.id != id && !f.path.as_os_str().is_empty()
            && std::fs::canonicalize(&f.path).unwrap_or_else(|_| f.path.clone())
                == std::fs::canonicalize(&target).unwrap_or_else(|_| target.clone()))
        {
            self.io_error = Some(format!("{}: file is already open in another tab", tr(self.lang).save_failed));
            return false;
        }
        let detached = self.files.iter().find(|f| f.id == id).is_some_and(|f| f.path.as_os_str().is_empty());
        if detached {
            match std::fs::read(&target) {
                Ok(expected) => { self.pending_save = Some(linkage::PendingSave { id, target, expected }); return false; }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                Err(error) => { self.io_error = Some(format!("{}: {error}", tr(self.lang).save_failed)); return false; }
            }
        }
        self.commit_save(id, &target, None)
    }

    fn commit_save(&mut self, id: u64, target: &std::path::Path, expected: Option<&[u8]>) -> bool {
        if self.files.iter().any(|f| f.id != id && !f.path.as_os_str().is_empty() && linkage::resolved(&f.path) == linkage::resolved(target)) {
            self.io_error = Some(format!("{}: file is already open in another tab", tr(self.lang).save_failed));
            return false;
        }
        let Some(file) = self.files.iter_mut().find(|f| f.id == id) else { return false; };
        match recovery::save_document_checked(file, target, expected) {
            Ok(()) => {
                let path = file.path.clone();
                self.recent_files.retain(|p| p != &path);
                self.recent_files.insert(0, path);
                self.recent_files.truncate(10);
                self.backup_or_report();
                true
            }
            Err(error) => {
                linkage::check(file, true);
                self.io_error = Some(format!("{}: {error}", tr(self.lang).save_failed));
                false
            }
        }
    }

    fn closing_ids(&self, action: &TabAction) -> Vec<u64> {
        let target = match action { TabAction::Close(id) | TabAction::CloseOthers(id) | TabAction::CloseLeft(id) | TabAction::CloseRight(id) => *id, TabAction::OpenFolder(_) => return Vec::new() };
        let Some(pos) = self.files.iter().position(|f| f.id == target) else { return Vec::new() };
        self.files.iter().enumerate().filter(|(i, f)| match action {
            TabAction::Close(_) => f.id == target,
            TabAction::CloseOthers(_) => f.id != target,
            TabAction::CloseLeft(_) => *i < pos,
            TabAction::CloseRight(_) => *i > pos,
            TabAction::OpenFolder(_) => false,
        }).map(|(_, f)| f.id).collect()
    }

    fn apply_tab_action(&mut self, action: TabAction) {
        match action {
            TabAction::Close(id) => self.close_tab(id),
            TabAction::CloseOthers(id) => self.close_others(id),
            TabAction::CloseLeft(id) => self.close_left(id),
            TabAction::CloseRight(id) => self.close_right(id),
            TabAction::OpenFolder(id) => {
                if let Some(file) = self.files.iter().find(|f| f.id == id)
                    && let Err(error) = open_file_folder(&file.path)
                {
                    self.io_error = Some(format!("{}: {error}", tr(self.lang).open_folder));
                }
                return;
            }
        }
        self.backup_or_report();
    }

    fn request_tab_action(&mut self, action: TabAction) {
        let ids = self.closing_ids(&action);
        if self.files.iter().any(|f| ids.contains(&f.id) && f.has_unsaved_changes()) {
            self.pending_close = Some(action);
        } else {
            self.apply_tab_action(action);
        }
    }

    fn file_dialogs(&mut self, ctx: &egui::Context) {
        let t = tr(self.lang);
        if let Some(index) = self.files.iter().position(|f| f.disk.pending.is_some()) {
            let id = self.files[index].id;
            let file = &self.files[index];
            let mut choice = 0;
            egui::Window::new(t.external_title).id(egui::Id::new("external_change"))
                .collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(file.path.to_string_lossy());
                    match file.disk.pending.as_ref().unwrap() {
                        linkage::External::Changed => { ui.label(t.external_changed); }
                        linkage::External::Missing => { ui.label(t.external_missing); }
                        linkage::External::Unreadable(error) => { ui.label(t.external_unreadable); ui.label(error); }
                    }
                    if file.is_modified() { ui.colored_label(self.workspace_theme.palette(ctx.system_theme()).danger, t.reload_warning); }
                    ui.horizontal(|ui| {
                        if ui.button(t.reload_disk).clicked() { choice = 1; }
                        if ui.button(t.detach_copy).clicked() { choice = 2; }
                    });
                });
            if choice == 1 {
                // Preserve the current working version before any destructive replacement.
                let archive = self.recovery_path().map(|p| p.with_file_name(format!("before-reload-{}-{}.json", id,
                    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos())));
                let result = archive.ok_or_else(|| std::io::Error::other("Recovery storage unavailable"))
                    .and_then(|path| recovery::archive_buffers(&path, std::slice::from_ref(&self.files[index]), Some(id)))
                    .and_then(|_| linkage::reload(&mut self.files[index]));
                match result {
                    Ok(()) => { self.selection = None; self.drag_anchor = None; self.inspector_popup = None; self.backup_or_report(); }
                    Err(error) => self.io_error = Some(format!("{}: {error}", t.reload_failed)),
                }
            } else if choice == 2 {
                linkage::detach(&mut self.files[index]);
                self.backup_or_report();
            }
        }
        if let Some(pending) = &self.pending_save {
            let mut choice = 0;
            egui::Window::new(t.save).id(egui::Id::new("overwrite_target"))
                .collapsible(false).resizable(false).show(ctx, |ui| {
                    ui.label(t.overwrite_target);
                    ui.label(pending.target.to_string_lossy());
                    ui.horizontal(|ui| {
                        if ui.button(t.confirm_overwrite).clicked() { choice = 1; }
                        if ui.button(t.cancel).clicked() { choice = 2; }
                    });
                });
            if choice != 0 {
                let pending = self.pending_save.take().unwrap();
                if choice == 1 { self.commit_save(pending.id, &pending.target, Some(&pending.expected)); }
            }
        }
        if self.pending_close.is_some() && self.pending_save.is_none() && !self.files.iter().any(|f| f.disk.pending.is_some()) {
            let mut choice = 0;
            egui::Window::new(t.save).id(egui::Id::new("unsaved_close"))
                .collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(t.close_unsaved);
                    ui.horizontal(|ui| {
                        if ui.button(t.save_and_close).clicked() { choice = 1; }
                        let pal = self.workspace_theme.palette(ctx.system_theme());
                        if ui.button(RichText::new(t.discard).color(pal.danger)).clicked() { choice = 2; }
                        if ui.button(t.cancel).clicked() { choice = 3; }
                    });
                });
            if choice != 0 {
                let action = self.pending_close.take().unwrap();
                if choice == 1 {
                    let ids = self.closing_ids(&action);
                    let dirty: Vec<_> = self.files.iter().filter(|f| ids.contains(&f.id) && f.has_unsaved_changes()).map(|f| f.id).collect();
                    if dirty.into_iter().all(|id| self.save_file(id)) {
                        self.apply_tab_action(action);
                    } else {
                        self.pending_close = Some(action);
                    }
                } else if choice == 2 {
                    self.apply_tab_action(action);
                }
            }
        }
        if let Some(error) = self.io_error.clone() {
            egui::Window::new(t.save).id(egui::Id::new("io_error"))
                .collapsible(false).resizable(false).show(ctx, |ui| {
                    ui.label(error);
                    if ui.button(t.done).clicked() { self.io_error = None; }
                });
        }
    }

    fn new_document(&mut self) {
        let id = self.next_file_id;
        self.next_file_id += 1;
        let mut file = LoadedFile {
            id,
            path: PathBuf::new(),
            name: format!("{} {}", tr(self.lang).untitled, id + 1),
            bytes: Vec::new(),
            is_binary: false,
            lines: Vec::new(),
            view: ViewMode::Text,
            language_override: None,
            draft_text: Some(String::new()),
            editor_state: editor::EditorState::default(),
            encoding: recovery::Encoding::default(),
            saved_bytes: Vec::new(),
            character_count: 0,
            saved_large_revision: None,
            bytes_dirty: false,
            content_revision: 0,
            saved_encoding: recovery::Encoding::default(),
            source_path: None,
            disk: linkage::DiskState::default(),
        };
        file.sync_draft();
        self.files.push(file);
        self.active_id = Some(id);
        self.selection = None;
        self.drag_anchor = None;
        self.inspector_popup = None;
        self.focus_editor = Some(id);
    }

    /// 打开文件（多选）：已存在的只激活，其余追加为新标签并激活。
    fn open_paths(&mut self, paths: Vec<PathBuf>) {
        for path in paths {
            let path = std::fs::canonicalize(&path).unwrap_or(path);
            if let Some(existing) = self.files.iter().find(|f| f.path == path) {
                self.active_id = Some(existing.id);
            } else {
                let Ok(bytes) = std::fs::read(&path) else { continue };
                let is_binary = detect_binary(&bytes);
                let (encoding, text) = if is_binary {
                    (recovery::Encoding::default(), String::new())
                } else {
                    recovery::Encoding::decode(&bytes)
                };
                let lines: Vec<String> = if is_binary || text.len() >= large_editor::THRESHOLD {
                    Vec::new()
                } else {
                    text.split('\n').map(str::to_owned).collect()
                };
                let id = self.next_file_id;
                self.next_file_id += 1;
                let character_count = text.chars().count();
                self.files.push(LoadedFile {
                    id,
                    path: path.clone(),
                    name: path
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    saved_bytes: bytes.clone(),
                    bytes,
                    is_binary,
                    lines,
                    view: if is_binary { ViewMode::Hex } else { ViewMode::Text },
                    language_override: None,
                    draft_text: (!is_binary).then_some(text),
                    editor_state: editor::EditorState::default(),
                    encoding,
                    character_count,
                    saved_large_revision: None,
                    bytes_dirty: false,
                    content_revision: 0,
                    saved_encoding: encoding,
                    source_path: None,
                    disk: linkage::DiskState::default(),
                });
                self.active_id = Some(id);
                self.selection = None;
                self.drag_anchor = None;
                self.inspector_popup = None;
            }
            // 最近打开：去重置顶，最多 10 条
            self.recent_files.retain(|p| p != &path);
            self.recent_files.insert(0, path.clone());
            self.recent_files.truncate(10);
        }
    }

    /// 启动时恢复上次会话：从磁盘重读文件内容，磁盘上已不存在的标签丢弃。
    fn restore_session(&mut self) {
        if let Some(path) = self.recovery_path() {
            match recovery::load(&path) {
                Ok(Some(session)) => {
                    self.files = session.restore();
                    self.active_id = session.active_id;
                    self.remember_recovered_versions();
                }
                Ok(None) => {}
                Err(error) => self.io_error = Some(format!("{}: {error}", tr(self.lang).recovery_failed)),
            }
        }
        self.next_file_id = self.files.iter().map(|f| f.id).max().unwrap_or(0) + 1;
        self.recent_files.retain(|p| p.exists());
        if !self.files.iter().any(|f| Some(f.id) == self.active_id) {
            self.active_id = self.files.first().map(|f| f.id);
        }
        self.last_recovery = Some(std::time::Instant::now());
    }

    fn open_dialog(&mut self) {
        if let Some(paths) = rfd::FileDialog::new().pick_files() {
            self.open_paths(paths);
        }
    }

    fn active(&self) -> Option<&LoadedFile> {
        self.files.iter().find(|f| Some(f.id) == self.active_id)
    }

    fn active_mut(&mut self) -> Option<&mut LoadedFile> {
        self.files
            .iter_mut()
            .find(|f| Some(f.id) == self.active_id)
    }

    /// 若激活的文件已不存在，回退到 fallback 并清空临时状态。
    fn ensure_active_exists(&mut self, fallback: u64) {
        let exists = self
            .active_id
            .is_some_and(|a| self.files.iter().any(|f| f.id == a));
        if !exists {
            self.active_id = Some(fallback);
            self.selection = None;
            self.drag_anchor = None;
            self.inspector_popup = None;
        }
    }

    fn close_tab(&mut self, id: u64) {
        if let Some(pos) = self.files.iter().position(|f| f.id == id) {
            self.files.remove(pos);
            if self.active_id == Some(id) {
                // 优先激活右侧邻位，否则最后一个（左侧）
                self.active_id = self
                    .files
                    .get(pos)
                    .or_else(|| self.files.last())
                    .map(|f| f.id);
            }
            self.selection = None;
            self.drag_anchor = None;
            self.inspector_popup = None;
        }
    }

    fn close_others(&mut self, id: u64) {
        self.files.retain(|f| f.id == id);
        self.active_id = Some(id);
        self.selection = None;
        self.drag_anchor = None;
        self.inspector_popup = None;
    }

    fn close_left(&mut self, id: u64) {
        if let Some(pos) = self.files.iter().position(|f| f.id == id) {
            self.files = self
                .files
                .drain(..)
                .enumerate()
                .filter(|(i, f)| *i >= pos || f.id == id)
                .map(|(_, f)| f)
                .collect();
            self.ensure_active_exists(id);
        }
    }

    fn close_right(&mut self, id: u64) {
        if let Some(pos) = self.files.iter().position(|f| f.id == id) {
            self.files = self
                .files
                .drain(..)
                .enumerate()
                .filter(|(i, f)| *i <= pos || f.id == id)
                .map(|(_, f)| f)
                .collect();
            self.ensure_active_exists(id);
        }
    }

    /// 标签标题：存在同名文件时用左起第一个不同的目录组件消歧（如 a/../d.txt）。
    fn tab_title(&self, id: u64) -> String {
        let Some(file) = self.files.iter().find(|f| f.id == id) else {
            return String::new();
        };
        let name = &file.name;
        if file.source_path.is_some() { return format!("{} · {}", name, tr(self.lang).temporary_copy); }
        let same_name_count = self.files.iter().filter(|f| f.name == *name).count();
        if same_name_count <= 1 {
            return name.clone();
        }
        let comps = |f: &LoadedFile| -> Vec<String> {
            f.path
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect()
        };
        if let Some(other) = self.files.iter().find(|f| f.name == *name && f.id != id) {
            let a = comps(file);
            let b = comps(other);
            let dirs = a.len().saturating_sub(1).min(b.len().saturating_sub(1));
            for i in 0..dirs {
                if a[i] != b[i] {
                    return format!("{}/../{name}", a[i]);
                }
            }
        }
        // 深层路径才不同：退回用直接父目录名
        let parent = file
            .path
            .parent()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if parent.is_empty() {
            name.clone()
        } else {
            format!("{parent}/../{name}")
        }
    }
}

// ---------------------------------------------------------------------------
// 文件检测
// ---------------------------------------------------------------------------

/// 启发式判断是否为二进制文件：开头 8KB 内出现 NUL 字节即视为二进制。
fn detect_binary(bytes: &[u8]) -> bool {
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        return false;
    }
    let sample = &bytes[..bytes.len().min(BINARY_SAMPLE_SIZE)];
    sample.contains(&0)
}

fn decode_utf16(bytes: &[u8], little_endian: bool) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            if little_endian {
                u16::from_le_bytes([pair[0], pair[1]])
            } else {
                u16::from_be_bytes([pair[0], pair[1]])
            }
        })
        .collect();
    String::from_utf16_lossy(&units)
}

const TAB_GAP: f32 = 8.0;
const TAB_TITLE_PADDING: f32 = 12.0;

/// 标签宽度：扣除间距后均分，上限 160px、下限 90px（超出则横向滚动）。
fn tab_width_for(file_count: usize, area_w: f32) -> f32 {
    let n = file_count.max(1) as f32;
    ((area_w - (n - 1.0) * TAB_GAP) / n).clamp(90.0, 160.0)
}

/// 指针所在的目标槽位（0..=others_len），超出部分收拢到边界。
fn tab_slot_for_pointer(pointer_x: f32, origin_x: f32, tab_step: f32, others_len: usize) -> usize {
    (((pointer_x - origin_x) / tab_step).floor() as isize)
        .clamp(0, others_len as isize) as usize
}

/// 非拖拽标签的槽位 x 坐标：在动画间隙位置腾出一个槽位。
fn layout_tab_xs(others_len: usize, origin_x: f32, tab_step: f32, gap_x: Option<f32>) -> Vec<f32> {
    let mut xs = Vec::with_capacity(others_len);
    let mut x = origin_x;
    let mut gap_drawn = false;
    for _ in 0..others_len {
        if let Some(gx) = gap_x
            && !gap_drawn
            && x + tab_step * 0.5 > gx
        {
            x += tab_step;
            gap_drawn = true;
        }
        xs.push(x);
        x += tab_step;
    }
    xs
}

/// 超宽标题保留开头，按实际排版宽度在末尾追加三个点。
fn elide_tab_title(title: &str, max_width: f32, measure: impl Fn(&str) -> f32) -> String {
    if measure(title) <= max_width {
        return title.to_owned();
    }
    if measure("...") > max_width {
        return String::new();
    }
    let starts: Vec<usize> = title.char_indices().map(|(i, _)| i).chain([title.len()]).collect();
    let mut lo = 0;
    let mut hi = starts.len() - 1;
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        if measure(&format!("{}...", &title[..starts[mid]])) <= max_width {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    format!("{}...", &title[..starts[lo]])
}

fn paint_tab_title(painter: &egui::Painter, rect: egui::Rect, title: &str, color: Color32) {
    let font = FontId::proportional(12.0);
    let text_rect = rect.shrink2(egui::vec2(TAB_TITLE_PADDING, 0.0));
    let title = elide_tab_title(title, text_rect.width().max(0.0), |text| {
        painter.layout_no_wrap(text.to_owned(), font.clone(), color).size().x
    });
    painter.with_clip_rect(painter.clip_rect().intersect(text_rect)).text(
        text_rect.center(),
        egui::Align2::CENTER_CENTER,
        title,
        font,
        color,
    );
}

/// 落位：把拖拽标签插入目标槽位。
fn insert_id(others: &[u64], id: u64, slot: usize) -> Vec<u64> {
    let slot = slot.min(others.len());
    let mut out = Vec::with_capacity(others.len() + 1);
    out.extend_from_slice(&others[..slot]);
    out.push(id);
    out.extend_from_slice(&others[slot..]);
    out
}

/// 激活标签两侧外弯圆弧半径。
const TAB_FILLET_RADIUS: f32 = 8.0;

/// 激活标签两侧外弯圆弧的形状：[(填充矩形, 打底圆), ...] 左右各一。
fn tab_fillet_shapes(rect: &egui::Rect) -> [(egui::Rect, egui::Pos2); 2] {
    let r = TAB_FILLET_RADIUS;
    let fy = rect.bottom() - r;
    [
        (
            egui::Rect::from_min_max(
                egui::pos2(rect.left() - r, fy),
                egui::pos2(rect.left(), rect.bottom()),
            ),
            egui::pos2(rect.left() - r, fy),
        ),
        (
            egui::Rect::from_min_max(
                egui::pos2(rect.right(), fy),
                egui::pos2(rect.right() + r, rect.bottom()),
            ),
            egui::pos2(rect.right() + r, fy),
        ),
    ]
}

fn open_file_folder(path: &std::path::Path) -> std::io::Result<()> {
    let parent = file_parent_folder(path)?;
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = std::process::Command::new("explorer");
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = std::process::Command::new("xdg-open");
    command.arg(parent).spawn()?;
    Ok(())
}

fn file_parent_folder(path: &std::path::Path) -> std::io::Result<PathBuf> {
    if path.as_os_str().is_empty() {
        return Err(std::io::Error::other("Untitled document has no folder"));
    }
    let absolute = std::path::absolute(path)?;
    let parent = absolute.parent().ok_or_else(|| std::io::Error::other("File has no parent folder"))?;
    Ok(parent.to_path_buf())
}

fn path_copy_label(ui: &mut egui::Ui, path: &str, pal: &Palette, hint: &str) -> egui::Response {
    let font = FontId::proportional(12.0);
    let galley = ui.painter().layout_no_wrap(path.to_owned(), font.clone(), pal.text_faint);
    let width = galley.size().x.min(ui.available_width()).max(0.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, galley.size().y), egui::Sense::click());
    let display = elide_tab_title(path, width, |text| ui.painter().layout_no_wrap(text.to_owned(), font.clone(), pal.text_faint).size().x);
    ui.painter_at(rect).text(rect.left_center(), egui::Align2::LEFT_CENTER, display, font, pal.text_faint);
    let response = response
        .on_hover_text(format!("{path}\n{hint}"))
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let toast_id = egui::Id::new(("copied_path_toast", path));
    let now = ui.input(|i| i.time);
    if response.clicked() {
        ui.ctx().copy_text(path.to_owned());
        ui.ctx().data_mut(|d| d.insert_temp(toast_id, now));
    }
    if let Some(start) = ui.ctx().data(|d| d.get_temp::<f64>(toast_id)) {
        let age = now - start;
        if (0.0..1.05).contains(&age) {
            let text = if hint == tr(Lang::ZhCn).copy_path { "已复制文件地址" } else { "File path copied" };
            egui::Area::new(toast_id.with("bubble"))
                .order(egui::Order::Tooltip).interactable(false)
                .pivot(egui::Align2::RIGHT_BOTTOM)
                .fixed_pos(rect.right_top() - egui::vec2(0.0, 10.0 + (age as f32 * 10.0)))
                .show(ui.ctx(), |ui| {
                    egui::Frame::new().fill(pal.bg_elevated).stroke(Stroke::new(1.0_f32, pal.accent))
                        .corner_radius(7.0).inner_margin(egui::Margin::symmetric(12, 7))
                        .show(ui, |ui| { ui.label(RichText::new(text).color(pal.text_main).size(13.0)); });
                });
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(16));
        } else { ui.ctx().data_mut(|d| d.remove::<f64>(toast_id)); }
    }
    response
}

fn format_size(size: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = size as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{size} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

// ---------------------------------------------------------------------------
// UI 主循环
// ---------------------------------------------------------------------------

impl eframe::App for LiteTextApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.backup_or_report();
        eframe::set_value(storage, eframe::APP_KEY, self);
        // 退出/自动保存时记录窗口几何
        if let (Some(size), Some(pos)) = (self.window_size, self.window_pos) {
            save_window_state(&WindowState { size, pos });
        }
    }

    fn auto_save_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(5)
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Err(error) = self.write_recovery() {
            eprintln!("Recovery save on exit failed: {error}");
        }
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        self.workspace_font_size = if self.workspace_font_size.is_finite() {
            self.workspace_font_size.clamp(9.0, 32.0)
        } else { 13.5 };
        self.poll_terminal();
        self.poll_updates();
        // 跟踪窗口几何（外框），供退出时持久化
        if let Some(rect) = ctx.input(|i| i.viewport().outer_rect) {
            self.window_size = Some([rect.width(), rect.height()]);
            self.window_pos = Some([rect.left(), rect.top()]);
        }

        let cmd_s = self.handle_shortcuts(ctx);
        #[cfg(target_os = "macos")]
        {
            let paths = file_open::macos::take();
            if !paths.is_empty() {
                self.open_paths(paths);
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
        }
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if OPEN_REQUEST.swap(false, Ordering::Relaxed) {
            self.open_dialog();
        }
        let dropped_paths: Vec<PathBuf> = dropped
            .iter()
            .filter_map(|f| f.path.clone())
            .collect();
        if !dropped_paths.is_empty() {
            self.open_paths(dropped_paths);
        }

        // 主题：仅在生效配色变化时重设样式（跟随系统时可自动切换）
        let system_theme = ctx.system_theme();
        self.migrate_legacy_theme();
        let palette = self.workspace_theme.palette(system_theme);
        let pal = &palette;
        if self.applied.as_ref() != Some(pal) {
            apply_style(ctx, pal);
            self.applied = Some(palette);
        }

        self.top_bar(ctx, pal);
        let focused = ctx.input(|i| i.focused);
        let force = (focused && !self.had_focus) || self.last_checked_tab != self.active_id;
        if force || self.last_disk_check.is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(2)) {
            for file in &mut self.files { linkage::check(file, force); }
            self.last_disk_check = Some(std::time::Instant::now());
            self.last_checked_tab = self.active_id;
        }
        self.had_focus = focused;

        if let Some(file) = self.active_mut() && file.view == ViewMode::Hex { file.ensure_bytes(); }

        let in_hex = self
            .active()
            .is_some_and(|f| f.view == ViewMode::Hex && !f.bytes.is_empty());
        let active_id = self.active_id;
        let shortcuts_blocked = self.shortcuts_blocked();
        if let Some(file) = self.files.iter_mut().find(|f| Some(f.id) == active_id) {
            file.editor_state.font_size = self.workspace_font_size;
            workspace_theme::configure(file, &self.workspace_theme, pal.dark);
        }

        // Reserve bottom panels before the central editor; otherwise they cover its horizontal scrollbar.
        self.status_bar(ctx, pal);
        if in_hex {
            self.hex_toolbar(ctx, pal);
        }

        // 右侧 Data Inspector：在顶栏之后添加，标题与 Address 表头同一水平线对齐
        if in_hex {
            self.data_inspector(ctx, pal);
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(pal.bg_window).inner_margin(egui::Margin::ZERO))
            .show(ctx, |ui| {
                if shortcuts_blocked { ui.disable(); }
                let file = active_id.and_then(|id| self.files.iter_mut().find(|f| f.id == id));
                match file {
                    None => welcome_screen(ui, self, pal),
                    Some(file) => match file.view {
                        ViewMode::Text if file.draft_text.is_some() => {
                            let focus = self.focus_editor == Some(file.id);
                            draft_editor(ui, file, pal, focus, self.lang);
                            if focus {
                                self.focus_editor = None;
                            }
                        }
                        ViewMode::Text => text_view(ui, file, pal),
                        ViewMode::Hex => hex_view(
                            ui,
                            file,
                            pal,
                            &mut self.selection,
                            &mut self.drag_anchor,
                            &HexViewOpts {
                                font_size: self.workspace_font_size,
                                upper: self.hex_upper,
                                dim_zero: self.dim_zeros,
                                show_ascii: self.show_ascii,
                                show_invisible: self.show_invisible,
                            },
                        ),
                    },
                }
            });

        self.settings_window(ctx, pal);
        self.plugin_error_window(ctx);
        self.curl_target_menu(ctx, pal);
        self.inspector_popup_window(ctx, pal);
        // Handle save after TextEdit so this frame's input is included.
        if cmd_s && let Some(id) = self.active_id {
            self.save_file(id);
            ctx.request_repaint();
        }
        if self.last_recovery.is_none_or(|last| last.elapsed() >= std::time::Duration::from_secs(5)) {
            self.backup_or_report();
            ctx.request_repaint();
        }
        ctx.request_repaint_after(std::time::Duration::from_secs(2));
        if ctx.input(|i| i.viewport().close_requested()) && !self.backup_or_report() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        self.file_dialogs(ctx);
        updater::acknowledge_startup();
        new_window::restore_closed_windows();
        self.finish_update_restart(ctx, frame);
    }
}

impl LiteTextApp {
    fn app_menu(&mut self, ctx: &egui::Context, anchor: egui::Rect, pal: &Palette) {
        if !self.show_menu {
            self.menu_recent_open = false;
            self.menu_plugins_open = false;
            self.menu_whitespace_open = false;
            self.menu_conversion_open = false;
            return;
        }
        let t = tr(self.lang);
        let mut action = None;
        let mut recent_row = egui::Rect::NOTHING;
        let mut plugins_row = egui::Rect::NOTHING;
        let mut whitespace_row = egui::Rect::NOTHING;
        let mut conversion_row = egui::Rect::NOTHING;
        let new_window_label = if self.lang == Lang::ZhCn { "新窗口" } else { "New window" };
        let conversion_label = if self.lang == Lang::ZhCn { "‹  文本转换" } else { "‹  Text conversion" };
        let plugins_label = format!("‹  {}", t.plugins);
        let whitespace_label = if self.lang == Lang::ZhCn { "‹  空白符处理" } else { "‹  Whitespace" };
        let recent_label = format!("‹  {}", t.open_recent);
        let menu = egui::Area::new(egui::Id::new("app_menu"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::RIGHT_TOP)
            .fixed_pos(egui::pos2(anchor.right(), anchor.bottom() + 6.0))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(pal.bg_panel)
                    .stroke(Stroke::new(1.0_f32, pal.text_faint.gamma_multiply(0.55)))
                    .corner_radius(8.0)
                    .inner_margin(egui::Margin::same(6))
                    .show(ui, |ui| {
                        // Including the frame, longest label + one character on each side.
                        let labels = [t.menu_open, new_window_label, &recent_label, &plugins_label, whitespace_label, conversion_label, t.settings];
                        let width = app_menu_width(ui, &labels, pal);
                        ui.set_width(width);
                        if app_menu_row(ui, t.menu_open, false, pal).on_hover_text(t.open_file).clicked() {
                            action = Some(MenuAction::Open);
                        }
                        app_menu_separator(ui, pal);
                        if app_menu_row(ui, new_window_label, false, pal).clicked() {
                            action = Some(MenuAction::NewWindow);
                        }
                        app_menu_separator(ui, pal);
                        let recent = app_menu_row(ui, &recent_label, self.menu_recent_open, pal);
                        recent_row = recent.rect;
                        if recent.hovered() && !self.menu_recent_open {
                            self.menu_recent_open = true;
                            self.menu_plugins_open = false;
                            self.menu_whitespace_open = false;
                            self.menu_conversion_open = false;
                            self.recent_files.retain(|p| p.is_file());
                        }
                        app_menu_separator(ui, pal);
                        let plugins = app_menu_row(ui, &plugins_label, self.menu_plugins_open, pal);
                        plugins_row = plugins.rect;
                        if plugins.hovered() || plugins.clicked() {
                            self.menu_plugins_open = true;
                            self.menu_recent_open = false;
                            self.menu_whitespace_open = false;
                            self.menu_conversion_open = false;
                        }
                        app_menu_separator(ui, pal);
                        let whitespace = app_menu_row(ui, whitespace_label, self.menu_whitespace_open, pal);
                        whitespace_row = whitespace.rect;
                        if whitespace.hovered() || whitespace.clicked() {
                            self.menu_whitespace_open = true;
                            self.menu_conversion_open = false;
                            self.menu_recent_open = false;
                            self.menu_plugins_open = false;
                        }
                        app_menu_separator(ui, pal);
                        let conversion = app_menu_row(ui, conversion_label, self.menu_conversion_open, pal);
                        conversion_row = conversion.rect;
                        if conversion.hovered() || conversion.clicked() {
                            self.menu_conversion_open = true;
                            self.menu_whitespace_open = false;
                            self.menu_recent_open = false;
                            self.menu_plugins_open = false;
                        }
                        app_menu_separator(ui, pal);
                        if app_menu_row(ui, t.settings, false, pal).clicked() {
                            action = Some(MenuAction::Settings);
                        }
                    });
            });
        let mut submenu_rect = None;
        if self.menu_conversion_open {
            let submenu = egui::Area::new(egui::Id::new("app_conversion_submenu"))
                .order(egui::Order::Foreground).pivot(egui::Align2::RIGHT_TOP)
                .fixed_pos(egui::pos2(menu.response.rect.left() - 4.0, conversion_row.top() - 6.0))
                .show(ctx, |ui| {
                    egui::Frame::new().fill(pal.bg_panel)
                        .stroke(Stroke::new(1.0_f32, pal.text_faint.gamma_multiply(0.55)))
                        .corner_radius(8.0).inner_margin(egui::Margin::same(6))
                        .show(ui, |ui| {
                            let labels: Vec<_> = text_conversion::ACTIONS.iter().map(|action| action.label(self.lang == Lang::ZhCn)).collect();
                            ui.set_width(app_menu_width(ui, &labels, pal));
                            let enabled = !self.shortcuts_blocked() && self.active().is_some_and(|f| f.view == ViewMode::Text && !f.is_binary && f.draft_text.is_some());
                            ui.add_enabled_ui(enabled, |ui| {
                                for (index, operation) in text_conversion::ACTIONS.into_iter().enumerate() {
                                    if app_menu_row(ui, labels[index], false, pal).clicked() {
                                        action = Some(MenuAction::Conversion(operation));
                                    }
                                }
                            });
                        });
                });
            submenu_rect = Some(submenu.response.rect);
            let bridge = egui::Rect::from_min_max(
                egui::pos2(submenu.response.rect.right().min(conversion_row.left()), conversion_row.top()),
                conversion_row.right_bottom(),
            );
            if ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| {
                !conversion_row.contains(p) && !submenu.response.rect.contains(p) && !bridge.contains(p)
            }) { self.menu_conversion_open = false; }
        }
        if self.menu_whitespace_open {
            let submenu = egui::Area::new(egui::Id::new("app_whitespace_submenu"))
                .order(egui::Order::Foreground).pivot(egui::Align2::RIGHT_TOP)
                .fixed_pos(egui::pos2(menu.response.rect.left() - 4.0, whitespace_row.top() - 6.0))
                .show(ctx, |ui| {
                    egui::Frame::new().fill(pal.bg_panel)
                        .stroke(Stroke::new(1.0_f32, pal.text_faint.gamma_multiply(0.55)))
                        .corner_radius(8.0).inner_margin(egui::Margin::same(6))
                        .show(ui, |ui| {
                            let labels: Vec<_> = whitespace::ACTIONS.iter().map(|action| action.label(self.lang == Lang::ZhCn)).collect();
                            ui.set_width(app_menu_width(ui, &labels, pal));
                            let enabled = !self.shortcuts_blocked() && self.active().is_some_and(|f| f.view == ViewMode::Text && !f.is_binary && f.draft_text.is_some());
                            ui.add_enabled_ui(enabled, |ui| {
                                for (index, operation) in whitespace::ACTIONS.into_iter().enumerate() {
                                    if app_menu_row(ui, labels[index], false, pal).clicked() {
                                        action = Some(MenuAction::Whitespace(operation));
                                    }
                                }
                            });
                        });
                });
            submenu_rect = Some(submenu.response.rect);
            let bridge = egui::Rect::from_min_max(
                egui::pos2(submenu.response.rect.right().min(whitespace_row.left()), whitespace_row.top()),
                whitespace_row.right_bottom(),
            );
            if ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| {
                !whitespace_row.contains(p) && !submenu.response.rect.contains(p) && !bridge.contains(p)
            }) { self.menu_whitespace_open = false; }
        }
        if self.menu_plugins_open {
            let submenu = egui::Area::new(egui::Id::new("app_plugins_submenu"))
                .order(egui::Order::Foreground)
                .pivot(egui::Align2::RIGHT_TOP)
                .fixed_pos(egui::pos2(menu.response.rect.left() - 4.0, plugins_row.top() - 6.0))
                .show(ctx, |ui| {
                    egui::Frame::new().fill(pal.bg_panel)
                        .stroke(Stroke::new(1.0_f32, pal.text_faint.gamma_multiply(0.55)))
                        .corner_radius(8.0).inner_margin(egui::Margin::same(6))
                        .show(ui, |ui| {
                            let curl_label = if self.lang == Lang::ZhCn { "curl解析  ›" } else { "Parse cURL  ›" };
                            ui.set_width(app_menu_width(ui, &["Format JSON", "Format HTML", "Format Header", "Format Cookie", curl_label], pal));
                            let enabled = !self.shortcuts_blocked() && self.active().is_some_and(|f| f.view == ViewMode::Text && !f.is_binary && f.draft_text.is_some());
                            ui.add_enabled_ui(enabled, |ui| {
                                if app_menu_row(ui, "Format JSON", false, pal).clicked() {
                                    action = Some(MenuAction::FormatJson);
                                }
                                for tool in [web_tools::Action::Html, web_tools::Action::Header, web_tools::Action::Cookie] {
                                    if app_menu_row(ui, tool.label(), false, pal).clicked() { action = Some(MenuAction::WebTool(tool)); }
                                }
                                app_menu_separator(ui, pal);
                                if app_menu_row(ui, curl_label, false, pal).clicked() { action = Some(MenuAction::CurlMenu); }
                            });
                        });
                });
            submenu_rect = Some(submenu.response.rect);
            let bridge = egui::Rect::from_min_max(
                egui::pos2(submenu.response.rect.right().min(plugins_row.left()), plugins_row.top()),
                plugins_row.right_bottom(),
            );
            if ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| {
                !plugins_row.contains(p) && !submenu.response.rect.contains(p) && !bridge.contains(p)
            }) { self.menu_plugins_open = false; }
        }
        if self.menu_recent_open {
            let submenu = egui::Area::new(egui::Id::new("app_recent_submenu"))
                .order(egui::Order::Foreground)
                .pivot(egui::Align2::RIGHT_TOP)
                .fixed_pos(egui::pos2(menu.response.rect.left() - 4.0, recent_row.top() - 6.0))
                .show(ctx, |ui| {
                    egui::Frame::new()
                        .fill(pal.bg_panel)
                        .stroke(Stroke::new(1.0_f32, pal.text_faint.gamma_multiply(0.55)))
                        .corner_radius(8.0)
                        .inner_margin(egui::Margin::same(6))
                        .show(ui, |ui| {
                            ui.set_width(260.0);
                            egui::ScrollArea::vertical()
                                .id_salt("menu_recent_files")
                                .max_height(300.0)
                                .auto_shrink([false, true])
                                .show(ui, |ui| {
                                    if self.recent_files.is_empty() {
                                        ui.add(egui::Label::new(RichText::new(t.recent_empty).size(12.0).color(pal.text_faint)).selectable(false));
                                    }
                                    for (index, path) in self.recent_files.iter().enumerate() {
                                        if index > 0 {
                                            app_menu_separator(ui, pal);
                                        }
                                        let name = path.file_name().unwrap_or_default().to_string_lossy();
                                        if app_menu_row(ui, &name, false, pal)
                                            .on_hover_text(path.to_string_lossy())
                                            .clicked()
                                        {
                                            action = Some(MenuAction::Recent(path.clone()));
                                        }
                                    }
                                });
                        });
                });
            submenu_rect = Some(submenu.response.rect);
            // 将两菜单间的空隙纳入安全区，指针横移时不会提前收起子菜单。
            let bridge = egui::Rect::from_min_max(
                egui::pos2(submenu.response.rect.right().min(recent_row.left()), recent_row.top()),
                egui::pos2(recent_row.right(), recent_row.bottom()),
            );
            if ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| {
                !recent_row.contains(p) && !submenu.response.rect.contains(p) && !bridge.contains(p)
            }) {
                self.menu_recent_open = false;
            }
        }
        let dismiss = ctx.input(|i| {
            i.key_pressed(egui::Key::Escape)
                || (i.pointer.any_click() && i.pointer.interact_pos().is_some_and(|p| {
                    !anchor.contains(p) && !menu.response.rect.contains(p)
                        && !submenu_rect.is_some_and(|rect| rect.contains(p))
                }))
        });
        if action.is_some() || dismiss {
            self.show_menu = false;
            self.menu_recent_open = false;
            self.menu_plugins_open = false;
            self.menu_whitespace_open = false;
            self.menu_conversion_open = false;
        }
        match action {
            Some(MenuAction::Open) => OPEN_REQUEST.store(true, Ordering::Relaxed),
            Some(MenuAction::NewWindow) => self.open_new_window(),
            Some(MenuAction::Recent(path)) => self.open_paths(vec![path]),
            Some(MenuAction::Settings) => { self.show_settings = true; self.shortcuts_page = false; self.workspace_theme_page = false; },
            Some(MenuAction::FormatJson) if !self.shortcuts_blocked() => self.format_active_json(ctx),
            Some(MenuAction::FormatJson) => {},
            Some(MenuAction::Whitespace(operation)) if !self.shortcuts_blocked() => {
                let mode = self.tab_mode;
                if let Some(file) = self.active_mut() { whitespace::apply(file, ctx, operation, mode); }
            }
            Some(MenuAction::Whitespace(_)) => {},
            Some(MenuAction::Conversion(operation)) => self.convert_active_text(ctx, operation),
            Some(MenuAction::WebTool(operation)) => self.apply_web_tool(ctx, operation),
            Some(MenuAction::CurlMenu) => self.show_curl_menu = true,
            None => {}
        }
    }

    fn top_bar(&mut self, ctx: &egui::Context, pal: &Palette) {
        let t = tr(self.lang);
        egui::TopBottomPanel::top("top_bar")
            .exact_height(38.0)
            .frame(
                egui::Frame::new()
                    .fill(pal.bg_panel)
                    .inner_margin(egui::Margin::symmetric(14, 0)),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    // ── 左：文件标签页（均分宽度 + 最小宽 + 悬停滚轮查看被遮标签）──
                    let area_w = (ui.available_width() - self.right_controls_w - 10.0).max(60.0);
                    let tab_w = tab_width_for(self.files.len(), area_w);
                    let tab_step = tab_w + TAB_GAP;
                    let tabs_width = (ui.available_width() - self.right_controls_w - 10.0).max(60.0);
                    ui.scope(|ui| {
                    // 仅标签区将纵向滚轮映射为横向滚动，不影响正文或右侧控件。
                    ui.style_mut().always_scroll_the_only_direction = true;
                    egui::ScrollArea::horizontal()
                        .id_salt("file_tabs")
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                        .auto_shrink([false, true])
                        .max_width(tabs_width)
                        .show(ui, |ui| {
                            ui.set_min_height(38.0);
                            let origin_x = ui.cursor().left();
                            let content_top = ui.cursor().top();
                            let pointer = ui.input(|i| i.pointer.interact_pos());
                            let dragging_id = self.tab_drag_id;

                            let ids: Vec<u64> = self.files.iter().map(|f| f.id).collect();
                            // 拖拽中的标签不占槽位，其余按顺序排布
                            let others: Vec<u64> = ids
                                .iter()
                                .filter(|&&id| Some(id) != dragging_id)
                                .copied()
                                .collect();

                            // 拖拽时计算目标槽位，间隙位置做平滑动画
                            let hover_slot = dragging_id.and_then(|_| {
                                pointer.map(|p| {
                                    tab_slot_for_pointer(p.x, origin_x, tab_step, others.len())
                                })
                            });
                            let gap_x = hover_slot.map(|s| {
                                origin_x + ui.ctx().animate_value_with_time(
                                    egui::Id::new("tab_gap"),
                                    s as f32 * tab_step,
                                    0.15,
                                )
                            });

                            // 非拖拽标签：跳过动画间隙后的槽位 x 坐标
                            let xs = layout_tab_xs(others.len(), origin_x, tab_step, gap_x);

                            // 间隙槽位可视化（含末尾间隙）
                            if let Some(gx) = gap_x {
                                let gap_rect = egui::Rect::from_min_size(
                                    egui::pos2(gx, content_top + 6.0),
                                    egui::vec2(tab_w, 26.0),
                                );
                                ui.painter().rect_filled(
                                    gap_rect,
                                    6.0,
                                    pal.accent.gamma_multiply(0.15),
                                );
                            }

                            // 非拖拽标签
                            for (i, &id) in others.iter().enumerate() {
                                let active = self.active_id == Some(id);
                                let dirty = self.files.iter().any(|f| f.id == id && f.has_unsaved_changes());
                                let title = self.tab_title(id);
                                let path = self
                                    .files
                                    .iter()
                                    .find(|f| f.id == id)
                                    .map(|f| if f.path.as_os_str().is_empty() {
                                        t.unsaved.to_owned()
                                    } else {
                                        f.path.to_string_lossy().into_owned()
                                    })
                                    .unwrap_or_default();

                                // 激活标签贯通到底部，与内容区"流下来"衔接
                                let rect = if active {
                                    egui::Rect::from_min_max(
                                        egui::pos2(xs[i], content_top + 6.0),
                                        egui::pos2(xs[i] + tab_w, content_top + 38.0),
                                    )
                                } else {
                                    egui::Rect::from_min_max(
                                        egui::pos2(xs[i], content_top + 6.0),
                                        egui::pos2(xs[i] + tab_w, content_top + 32.0),
                                    )
                                };
                                let (rect, mut resp) = (rect, ui.interact(
                                    rect,
                                    egui::Id::new("file_tab").with(id),
                                    egui::Sense::click_and_drag(),
                                ));
                                resp = resp.on_hover_text(path);
                                let p = ui.painter_at(rect.expand2(egui::vec2(TAB_FILLET_RADIUS, 0.0)));

                                if active {
                                    // 贯通底部的激活标签 + 两侧外弯圆弧
                                    p.rect_filled(
                                        rect,
                                        egui::CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 },
                                        pal.bg_window,
                                    );
                                    for (fill_rect, circle_center) in tab_fillet_shapes(&rect) {
                                        p.rect_filled(fill_rect, 0.0, pal.bg_window);
                                        p.circle_filled(circle_center, TAB_FILLET_RADIUS, pal.bg_panel);
                                    }
                                } else {
                                    // 非激活：独立圆角与轻描边，避免相邻标签粘连。
                                    p.rect_filled(
                                        rect,
                                        6.0,
                                        pal.bg_elevated,
                                    );
                                    if resp.hovered() {
                                        p.rect_filled(
                                            rect,
                                            6.0,
                                            pal.bg_inset,
                                        );
                                    }
                                    p.rect_stroke(
                                        rect,
                                        6.0,
                                        Stroke::new(1.0_f32, pal.text_faint.gamma_multiply(0.25)),
                                        egui::StrokeKind::Inside,
                                    );
                                }
                                if dirty { p.rect_filled(rect.shrink(1.0), 6.0, pal.danger.gamma_multiply(if active { 0.12 } else { 0.08 })); }
                                paint_tab_title(
                                    &p,
                                    rect,
                                    &title,
                                    if active { pal.text_main } else { pal.text_faint },
                                );

                                let mut switch = false;
                                let mut action: Option<TabAction> = None;
                                if resp.clicked() && !active {
                                    switch = true;
                                }
                                if resp.double_clicked() {
                                    action = Some(TabAction::Close(id));
                                }
                                resp.context_menu(|menu| {
                                    let has_path = self.files.iter().any(|f| f.id == id && !f.path.as_os_str().is_empty());
                                    if menu.add_enabled(has_path, egui::Button::new(t.open_folder)).clicked() {
                                        action = Some(TabAction::OpenFolder(id));
                                        menu.close_menu();
                                    }
                                    menu.separator();
                                    if menu.button(t.close).clicked() {
                                        action = Some(TabAction::Close(id));
                                        menu.close_menu();
                                    }
                                    if menu.button(t.close_others).clicked() {
                                        action = Some(TabAction::CloseOthers(id));
                                        menu.close_menu();
                                    }
                                    if menu.button(t.close_left).clicked() {
                                        action = Some(TabAction::CloseLeft(id));
                                        menu.close_menu();
                                    }
                                    if menu.button(t.close_right).clicked() {
                                        action = Some(TabAction::CloseRight(id));
                                        menu.close_menu();
                                    }
                                });
                                if switch {
                                    self.active_id = Some(id);
                                    self.selection = None;
                                    self.drag_anchor = None;
                                    self.inspector_popup = None;
                                }

                                if resp.drag_started() && self.tab_drag_id.is_none() {
                                    self.tab_drag_id = Some(id);
                                    if let Some(p) = resp.interact_pointer_pos() {
                                        self.tab_grab_dx = p.x - rect.left();
                                    }
                                }
                                if self.tab_drag_id == Some(id) {
                                    resp = resp.on_hover_cursor(egui::CursorIcon::Move);
                                }
                                if resp.drag_stopped() {
                                    self.tab_drag_id = None;
                                }

                                if let Some(action) = action {
                                    self.request_tab_action(action);
                                }
                            }

                            // 拖拽中的标签：跟随指针悬浮显示
                            if let (Some(drag_id), Some(p)) = (dragging_id, pointer) {
                                let title = self.tab_title(drag_id);
                                let float_rect = egui::Rect::from_min_size(
                                    egui::pos2(p.x - self.tab_grab_dx, content_top + 2.0),
                                    egui::vec2(tab_w, 26.0),
                                );
                                let resp = ui.interact(
                                    float_rect,
                                    egui::Id::new("file_tab").with(drag_id),
                                    egui::Sense::click_and_drag(),
                                );
                                let p2 = ui.painter_at(float_rect);
                                p2.rect_filled(
                                    float_rect,
                                    egui::CornerRadius { nw: 6, ne: 6, sw: 6, se: 6 },
                                    pal.bg_elevated,
                                );
                                p2.rect_stroke(
                                    float_rect,
                                    egui::CornerRadius { nw: 6, ne: 6, sw: 6, se: 6 },
                                    Stroke::new(1.2_f32, pal.accent),
                                    egui::StrokeKind::Inside,
                                );
                                if self.files.iter().any(|f| f.id == drag_id && f.has_unsaved_changes()) {
                                    p2.rect_filled(float_rect.shrink(1.0), 6.0, pal.danger.gamma_multiply(0.12));
                                }
                                paint_tab_title(
                                    &p2,
                                    float_rect,
                                    &title,
                                    pal.text_main,
                                );
                                let _ = resp;

                                // 松手落位：插入目标槽位
                                if resp.drag_stopped() {
                                    if let Some(slot) = hover_slot {
                                        let order = insert_id(&others, drag_id, slot);
                                        let mut old: std::collections::HashMap<_, _> = std::mem::take(&mut self.files)
                                            .into_iter().map(|file| (file.id, file)).collect();
                                        self.files = order
                                            .iter()
                                            .filter_map(|oid| old.remove(oid))
                                            .collect();
                                    }
                                    self.tab_drag_id = None;
                                    self.tab_grab_dx = 0.0;
                                }
                            }
                            // 手动绘制也要声明内容宽度，标签超出视口时才能横向滚动。
                            let content_w = (ids.len() as f32 * tab_step - TAB_GAP).max(0.0);
                            ui.allocate_space(egui::vec2(content_w, 38.0));
                        });
                    });

                    ui.separator();

                    // ── 右侧控件（从右到左：菜单 / 新建 / 视图切换）──
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let menu_button = settings_icon_button(ui, &mut self.show_menu, t.menu, pal);
                        self.app_menu(ui.ctx(), menu_button.rect, pal);

                        if new_file_icon_button(ui, t.new_file, pal).clicked() {
                            self.new_document();
                        }

                        // 单状态图标切换：TXT / 010，无背景融入界面。
                        // 可切换时外圈有强调色描边；二进制固定 010，灰显、无描边、点击无效。
                        if let Some(file) = self.active() {
                            let is_binary = file.is_binary;
                            let enabled = !is_binary;
                            let show_hex = is_binary || file.view == ViewMode::Hex;
                            let mut toggled = false;

                            let (rect, mut resp) = ui.allocate_exact_size(
                                egui::vec2(26.0, 24.0),
                                egui::Sense::click(),
                            );
                            resp = resp.on_hover_text(t.view_toggle);
                            let hovered = enabled && resp.hovered();
                            if enabled {
                                resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                            }
                            let p = ui.painter();
                            if enabled {
                                // 外圈高亮描边
                                p.rect_stroke(
                                    rect,
                                    6.0,
                                    Stroke::new(
                                        if hovered { 1.6_f32 } else { 1.2_f32 },
                                        if hovered { pal.accent_hover } else { pal.accent },
                                    ),
                                    egui::StrokeKind::Inside,
                                );
                                if hovered {
                                    p.rect_filled(rect, 6.0, pal.accent.gamma_multiply(0.15));
                                }
                            }
                            let color = if enabled {
                                if hovered { pal.accent_hover } else { pal.text_main }
                            } else {
                                pal.text_faint
                            };
                            let label = if show_hex { "010" } else { "TXT" };
                            p.text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                label,
                                FontId::monospace(10.5),
                                color,
                            );
                            if resp.clicked() && enabled {
                                toggled = true;
                            }

                            if toggled {
                                if let Some(f) = self.active_mut() {
                                    f.view = if f.view == ViewMode::Text {
                                        ViewMode::Hex
                                    } else {
                                        ViewMode::Text
                                    };
                                }
                                self.selection = None;
                                self.drag_anchor = None;
                            }
                        }

                        if self.active().is_some() && terminal::button(ui, pal, self.lang).clicked() {
                            self.open_terminal(ui.ctx());
                        }

                        if let Some(file) = self.active_mut()
                            && file.view == ViewMode::Text && file.draft_text.is_some()
                        {
                            text_tools::toolbar(ui, file, pal, t);
                        }

                        // 实测右侧控件总宽（下一帧供标签区自适应）
                        self.right_controls_w = ui.min_rect().width() + 12.0;
                    });
                });
            });
    }

    fn status_bar(&mut self, ctx: &egui::Context, pal: &Palette) {
        let Some(file) = self.active() else { return };
        let t = tr(self.lang);
        let info = if file.is_binary {
            t.binary_info(file.bytes.len())
        } else {
            t.characters_info.replace("{n}", &file.character_count.to_string())
        };
        let size_label = format_size(file.byte_len() as u64);
        let indicator = self.recovery_indicator(file);
        let path = (!file.path.as_os_str().is_empty()).then(|| std::path::absolute(&file.path)
            .unwrap_or_else(|_| file.path.clone()).to_string_lossy().into_owned());
        let language_enabled = !self.shortcuts_blocked() && !file.is_binary;
        let mut language_change = None;
        let mut tab_change = None;
        egui::TopBottomPanel::bottom("status_bar")
            .frame(egui::Frame::new().fill(pal.bg_panel).inner_margin(egui::Margin::symmetric(14, 5)))
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    language_change = workspace_theme::language_picker(ui, file, self.lang, pal, language_enabled);
                    ui.separator();
                    ui.label(RichText::new(&size_label).color(pal.accent).size(12.0));
                    ui.separator();
                    ui.label(RichText::new(&info).color(pal.text_faint).size(12.0));
                    ui.separator();
                    if file.draft_text.is_some() {
                        ui.add(egui::Label::new(RichText::new(if file.encoding == recovery::Encoding::Lossy { t.encoding_unknown } else { file.encoding.name() }).color(pal.text_faint).size(12.0)).selectable(false));
                    } else {
                        ui.add(egui::Label::new(RichText::new(t.encoding_binary).color(pal.text_faint).size(12.0)).selectable(false));
                    }
                    ui.separator();
                    ui.add(egui::Label::new(RichText::new(if file.editor_state.selected_characters == 0 {
                        t.cursor_line.replace("{n}", &(file.editor_state.cursor_line + 1).to_string())
                    } else {
                        t.selected_characters.replace("{n}", &file.editor_state.selected_characters.to_string())
                    }).color(pal.text_faint).size(12.0)).selectable(false));
                    ui.separator();
                    tab_change = whitespace::picker(ui, self.tab_mode, self.lang, pal, language_enabled);

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 14.0), egui::Sense::hover());
                        // A quiet 8-point dot: no glow, animation, label or tooltip.
                        ui.painter().circle_filled(rect.center(), 4.0, indicator.color(pal));
                        if let Some(path) = &path {
                            path_copy_label(ui, path, pal, t.copy_path);
                        } else if let Some(source) = &file.source_path {
                            ui.add(egui::Label::new(RichText::new(t.temporary_copy).color(pal.text_faint).size(12.0)).selectable(false))
                                .on_hover_text(format!("{}: {}", t.temporary_copy, source.display()));
                        }
                    });
                });
            }).response.on_hover_text(t.lines_info(file.line_count()));
        if let Some(language) = language_change {
            let theme = self.workspace_theme.clone();
            if let Some(file) = self.active_mut() {
                file.language_override = language;
                workspace_theme::configure(file, &theme, pal.dark);
            }
            ctx.request_repaint();
        }
        if let Some(mode) = tab_change { self.tab_mode = mode; }
    }

    fn settings_window(&mut self, ctx: &egui::Context, pal: &Palette) {
        if !self.show_settings {
            return;
        }
        let recording = self.shortcut_recording.is_some();
        if recording { self.capture_shortcut(ctx); }
        let t = tr(self.lang);
        let mut open = self.show_settings;
        let mut done_clicked = false;
        egui::Window::new(RichText::new(t.settings).size(15.0).strong())
            .id(egui::Id::new("system_settings"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .fixed_size(if self.shortcuts_page {
                egui::vec2(
                    (ctx.screen_rect().width() - 56.0).clamp(320.0, 1240.0),
                    (ctx.screen_rect().height() - 64.0).max(300.0),
                )
            } else if self.workspace_theme_page {
                egui::vec2((ctx.screen_rect().width() - 56.0).clamp(320.0, 1120.0), (ctx.screen_rect().height() - 100.0).clamp(260.0, 620.0))
            } else { egui::vec2((ctx.screen_rect().width() - 56.0).clamp(320.0, 460.0), (ctx.screen_rect().height() - 100.0).clamp(180.0, 560.0)) })
            .frame(
                egui::Frame::new()
                    .fill(pal.bg_panel)
                    .corner_radius(10.0)
                    .stroke(Stroke::new(1.0_f32, pal.bg_elevated))
                    .inner_margin(egui::Margin::same(12)),
            )
            .show(ctx, |ui| {
                ui.memory_mut(|memory| memory.set_modal_layer(ui.layer_id()));
                if self.shortcuts_page {
                    self.shortcuts_ui(ui, pal);
                } else if self.workspace_theme_page {
                    self.workspace_theme_ui(ui, pal);
                } else {
                    egui::ScrollArea::vertical().id_salt("settings_home_scroll")
                        .max_height((ctx.screen_rect().height() - 110.0).max(180.0))
                        .show(ui, |ui| {
                    section_header(ui, pal, t.general);
                    ui.add_space(4.0);

                    // 标签与控件两端对齐，明暗主题共用自研下拉组件。
                    ui.horizontal(|ui| {
                        ui.set_min_width(ui.available_width());
                        ui.label(RichText::new(t.language).size(13.0));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let options = Lang::ALL.iter()
                                .map(|l| (l.native_name().to_owned(), *l)).collect();
                            if let Some(lang) = dropdown(
                                ui, pal, egui::Id::new("settings_lang"),
                                self.lang.native_name().to_owned(), options, 160.0,
                            ) {
                                self.lang = lang;
                            }
                        });
                    });
                    ui.add_space(6.0);
                    ui.label(RichText::new(t.language_desc).size(11.0).color(pal.text_faint));
                    ui.add_space(8.0);
                    self.terminal_settings_ui(ui, pal);
                    ui.add_space(8.0);
                    ui.separator();
                    let workspace = ui.add_sized([ui.available_width(), 30.0], egui::Button::new(format!("{}    ›", t.workspace_theme)));
                    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("settings_workspace_theme_rect"), workspace.rect));
                    if workspace.clicked() { self.workspace_theme_page = true; }
                    ui.add_space(10.0);
                    ui.separator();
                    let shortcuts = ui.add_sized(
                        [ui.available_width(), 30.0],
                        egui::Button::new(format!("{}    ›", t.shortcuts)),
                    );
                    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("settings_shortcuts_rect"), shortcuts.rect));
                    if shortcuts.clicked() { self.shortcuts_page = true; }
                    ui.add_space(10.0);
                    ui.separator();
                    self.update_settings_ui(ui, pal);
                    });
                }
            });
        if !recording && shortcuts::take_binding(ctx, self.shortcut_keys.binding(shortcuts::Action::Settings)) { done_clicked = true; }
        if !recording && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            if self.shortcuts_page { self.shortcuts_page = false; }
            else if self.workspace_theme_page { self.workspace_theme_page = false; }
            else { done_clicked = true; }
        }
        self.show_settings = open && !done_clicked;
        if !self.show_settings { self.shortcut_recording = None; self.shortcut_error = None; }
        if self.applied.as_ref() != Some(&self.workspace_theme.palette(ctx.system_theme())) { ctx.request_repaint(); }
    }
}

// ---------------------------------------------------------------------------
// 通用组件
// ---------------------------------------------------------------------------

/// 设置分组标题。
fn section_header(ui: &mut egui::Ui, pal: &Palette, title: &str) {
    ui.add_space(2.0);
    ui.label(RichText::new(title).size(11.0).strong().color(pal.text_faint));
    ui.add_space(2.0);
}

/// 现代胶囊按钮：primary 为强调色实底，secondary 为安静幽灵样式。
fn pill_button(ui: &mut egui::Ui, label: &str, primary: bool, pal: &Palette) -> egui::Response {
    let text_color = if primary { Color32::WHITE } else { pal.text_main };
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), FontId::proportional(13.0), text_color);
    let size = egui::vec2(galley.size().x + 28.0, (galley.size().y + 10.0).max(26.0));
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);

    let fill = if primary {
        if resp.hovered() { pal.accent_hover } else { pal.accent }
    } else if resp.hovered() {
        pal.bg_elevated
    } else {
        Color32::TRANSPARENT
    };
    if fill != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, size.y * 0.5, fill);
    }
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, text_color);
    resp
}

/// 极简单页纸图标：细线轮廓与右上折角。
fn new_file_icon_button(ui: &mut egui::Ui, tooltip: &str, pal: &Palette) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(30.0, 26.0), egui::Sense::click());
    let resp = resp.on_hover_text(tooltip).on_hover_cursor(egui::CursorIcon::PointingHand);
    let color = if resp.hovered() { pal.accent_hover } else { pal.text_faint };
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, 6.0, pal.bg_elevated);
    }
    let icon = egui::Rect::from_center_size(rect.center(), egui::vec2(14.0, 18.0));
    let fold = 4.0;
    let stroke = Stroke::new(1.4_f32, color);
    p.add(egui::Shape::closed_line(vec![
        icon.left_top(),
        icon.right_top() - egui::vec2(fold, 0.0),
        icon.right_top() + egui::vec2(0.0, fold),
        icon.right_bottom(),
        icon.left_bottom(),
    ], stroke));
    p.add(egui::Shape::line(vec![
        icon.right_top() - egui::vec2(fold, 0.0),
        icon.right_top() + egui::vec2(-fold, fold),
        icon.right_top() + egui::vec2(0.0, fold),
    ], stroke));
    resp
}

fn app_menu_separator(ui: &mut egui::Ui, pal: &Palette) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().line_segment(
        [egui::pos2(rect.left() + 4.0, rect.center().y), egui::pos2(rect.right() - 4.0, rect.center().y)],
        Stroke::new(1.0_f32, pal.text_faint.gamma_multiply(0.55)),
    );
}

fn app_menu_row(ui: &mut egui::Ui, label: &str, active: bool, pal: &Palette) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 28.0), egui::Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if resp.hovered() || active {
        ui.painter().rect_filled(rect, 5.0, pal.bg_elevated);
    }
    let font = FontId::proportional(13.0);
    let color = if active { pal.accent } else { pal.text_main };
    let padding = app_menu_padding(ui, pal);
    let text = elide_tab_title(label, (rect.width() - 2.0 * padding).max(0.0), |text| {
        ui.painter().layout_no_wrap(text.to_owned(), font.clone(), color).size().x
    });
    ui.painter_at(rect).text(egui::pos2(rect.left() + padding, rect.center().y), egui::Align2::LEFT_CENTER, text, font, color);
    resp
}

fn app_menu_padding(ui: &egui::Ui, pal: &Palette) -> f32 {
    // Six points are already provided by the menu frame on each side.
    (ui.painter().layout_no_wrap("字".to_owned(), FontId::proportional(13.0), pal.text_main).size().x - 6.0).max(0.0)
}

fn app_menu_width(ui: &egui::Ui, labels: &[&str], pal: &Palette) -> f32 {
    labels.iter().map(|label| ui.painter().layout_no_wrap((*label).to_owned(), FontId::proportional(13.0), pal.text_main).size().x)
        .fold(0.0f32, f32::max) + 2.0 * app_menu_padding(ui, pal)
}

/// 设置“调节杆”图标按钮。painter 矢量绘制。
fn settings_icon_button(ui: &mut egui::Ui, open: &mut bool, tooltip: &str, pal: &Palette) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(28.0, 24.0), egui::Sense::click());
    let resp = resp.on_hover_text(tooltip).on_hover_cursor(egui::CursorIcon::PointingHand);

    let active = *open;
    if resp.hovered() {
        ui.painter().rect_filled(rect, 6.0, pal.bg_elevated);
    }
    if active {
        ui.painter().rect_filled(rect, 6.0, pal.accent.gamma_multiply(0.25));
    }

    let color = if active {
        pal.accent
    } else if resp.hovered() {
        pal.text_main
    } else {
        pal.text_faint
    };
    let stroke = Stroke::new(1.6_f32, color);
    let p = ui.painter();

    // 三条横线 + 错落的圆点，模拟设置滑杆
    for i in 0..3 {
        let y = rect.top() + rect.height() * (0.28 + 0.22 * i as f32);
        let left = egui::pos2(rect.left() + 5.0, y);
        let right = egui::pos2(rect.right() - 5.0, y);
        p.line_segment([left, right], stroke);
        let knob_x = match i {
            0 => rect.left() + rect.width() * 0.62,
            1 => rect.left() + rect.width() * 0.34,
            _ => rect.left() + rect.width() * 0.70,
        };
        p.circle_filled(egui::pos2(knob_x, y), 2.6, color);
    }

    if resp.clicked() {
        *open = !*open;
    }
    resp
}

// ---------------------------------------------------------------------------
// HEX 工具栏与 Data Inspector
// ---------------------------------------------------------------------------

impl LiteTextApp {
    /// HEX 模式底部工具栏：全部为图标开关。
    fn hex_toolbar(&mut self, ctx: &egui::Context, pal: &Palette) {
        let t = tr(self.lang);
        egui::TopBottomPanel::bottom("hex_toolbar")
            .frame(egui::Frame::new().fill(pal.bg_panel).inner_margin(egui::Margin::symmetric(14, 4)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let upper = self.hex_upper;
                    if toolbar_toggle(ui, self.hex_upper, t.hex_case, pal, move |p, r, c| {
                        let label = if upper { "AA" } else { "aa" };
                        p.text(r.center(), egui::Align2::CENTER_CENTER, label, FontId::monospace(12.0), c);
                    })
                    .clicked()
                    {
                        self.hex_upper = !self.hex_upper;
                    }
                    if toolbar_toggle(ui, self.dim_zeros, t.dim_zeros, pal, |p, r, c| {
                        let dim = Color32::from_rgba_premultiplied(
                            c.r() / 3,
                            c.g() / 3,
                            c.b() / 3,
                            255,
                        );
                        p.text(
                            r.center() - egui::vec2(4.5, 0.0),
                            egui::Align2::CENTER_CENTER,
                            "0",
                            FontId::monospace(12.0),
                            dim,
                        );
                        p.text(
                            r.center() + egui::vec2(4.5, 0.0),
                            egui::Align2::CENTER_CENTER,
                            "0",
                            FontId::monospace(12.0),
                            c,
                        );
                    })
                    .clicked()
                    {
                        self.dim_zeros = !self.dim_zeros;
                    }
                    if toolbar_toggle(ui, !self.show_ascii, t.toggle_ascii, pal, |p, r, c| {
                        p.text(r.center(), egui::Align2::CENTER_CENTER, "A", FontId::proportional(12.0), c);
                        p.line_segment(
                            [egui::pos2(r.left() + 6.0, r.bottom() - 6.0), egui::pos2(r.right() - 6.0, r.top() + 6.0)],
                            Stroke::new(1.4_f32, c),
                        );
                    })
                    .clicked()
                    {
                        self.show_ascii = !self.show_ascii;
                    }
                    if toolbar_toggle(ui, self.show_invisible, t.show_invisible, pal, |p, r, c| {
                        let dim = Color32::from_rgba_premultiplied(
                            c.r() / 2,
                            c.g() / 2,
                            c.b() / 2,
                            255,
                        );
                        for (dx, filled) in [(-8.0, true), (0.0, false), (8.0, true)] {
                            let center = r.center() + egui::vec2(dx, 0.0);
                            if filled {
                                p.circle_filled(center, 2.0, dim);
                            } else {
                                p.circle_stroke(center, 3.0, Stroke::new(1.4_f32, c));
                            }
                        }
                    })
                    .clicked()
                    {
                        self.show_invisible = !self.show_invisible;
                    }
                });
            });
    }

    /// HEX 模式右侧 Data Inspector：按选中起始字节的小端序解释。
    /// 面板边距与 HEX 表头一致，标题与 "Address" 表头同行对齐。
    fn data_inspector(&mut self, ctx: &egui::Context, pal: &Palette) {
        let t = tr(self.lang);
        let Some(active_id) = self.active_id else { return };
        let Some(file) = self.files.iter().find(|f| f.id == active_id) else { return };
        let selection = self.selection;

        // 拖拽调宽上限：面板左缘最多到达 ASCII 列完整显示内容的右缘 + 20px
        // （字符列：10 地址 + 49 十六进制 + 1 中缝 + 16 ASCII = 76）
        let probe = ctx.fonts(|f| {
            f.layout_no_wrap("0000000000".to_owned(), FontId::monospace(self.workspace_font_size - 0.5), Color32::WHITE)
        });
        let char_w = probe
            .rows
            .first()
            .and_then(|r| {
                let g = &r.glyphs;
                (g.len() >= 2).then(|| g[1].pos.x - g[0].pos.x)
            })
            .unwrap_or(8.0);
        let ascii_right = 14.0 + 76.0 * char_w;
        let min_width = 180.0;
        let max_width = (ctx.screen_rect().width() - ascii_right - 40.0).max(min_width);
        let width = self.inspector_width.clamp(min_width, max_width);

        egui::SidePanel::right("data_inspector")
            .exact_width(width)
            .resizable(false)
            .frame(egui::Frame::new().fill(pal.bg_panel).inner_margin(egui::Margin::symmetric(14, 4)))
            .show(ctx, |ui| {
                ui.label(RichText::new(t.data_inspector).size(13.0).strong().color(pal.text_faint));
                ui.separator();
                ui.add_space(2.0);

                let bytes = &file.bytes;
                let start = selection.map(|(s, _)| s).unwrap_or(0);
                let count = selection.map(|(s, e)| e - s + 1).unwrap_or(0);

                // 小端序读取 n 字节
                let le = |n: usize| -> Option<u128> {
                    (start + n <= bytes.len()).then(|| {
                        (0..n).rev().fold(0u128, |acc, i| (acc << 8) | bytes[start + i] as u128)
                    })
                };
                // 纵向滚动兜底：行数较多时面板内可滚动
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        // 值行：一行一个类型，超宽省略，双击弹窗，右键复制值
                        macro_rules! row {
                            ($label:expr, $value:expr) => {
                                if let Some((value, pos)) =
                                    inspector_value_row(ui, pal, t, $label, $value, char_w)
                                {
                                    self.inspector_popup = Some(value);
                                    self.popup_pos = Some(pos);
                                }
                            };
                        }

                        row!(
                            "Offset",
                            if bytes.is_empty() { None } else { Some(format!("{start:08X}")) }
                        );
                        row!("u8", le(1).map(|v| (v as u8).to_string()));
                        row!("i8", le(1).map(|v| (v as i8).to_string()));
                        row!("u16", le(2).map(|v| (v as u16).to_string()));
                        row!("i16", le(2).map(|v| (v as i16).to_string()));
                        row!("u32", le(4).map(|v| (v as u32).to_string()));
                        row!("i32", le(4).map(|v| (v as i32).to_string()));
                        row!("f32", le(4).map(|v| f32::from_bits(v as u32).to_string()));
                        row!("u64", le(8).map(|v| (v as u64).to_string()));
                        row!("i64", le(8).map(|v| (v as i64).to_string()));
                        row!("f64", le(8).map(|v| f64::from_bits(v as u64).to_string()));
                        row!("Length", Some(format!("{count}")));

                        // 选中字节的字符串预览
                        ui.add_space(8.0);
                        let full: Option<String> = if count > 0 && start + count <= bytes.len() {
                            Some(String::from_utf8_lossy(&bytes[start..start + count]).into_owned())
                        } else {
                            None
                        };
                        row!("String", full);
                    });

                // 自研宽度调节：面板左缘拖拽条（注册在内容之上，不会被内容抢事件）
                let content_left = ui.min_rect().left();
                let strip = egui::Rect::from_min_max(
                    egui::pos2(content_left - 4.0, ui.min_rect().top()),
                    egui::pos2(content_left + 4.0, ui.max_rect().bottom()),
                );
                let divider = ui.interact(
                    strip,
                    egui::Id::new("inspector_divider"),
                    egui::Sense::drag(),
                );
                if divider.hovered() || divider.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeWest);
                    ui.painter().line_segment(
                        [
                            egui::pos2(content_left, strip.top()),
                            egui::pos2(content_left, strip.bottom()),
                        ],
                        Stroke::new(1.5_f32, pal.accent),
                    );
                }
                if divider.dragged() {
                    // 面板在右侧：向右拖 = 变窄
                    self.inspector_width = (self.inspector_width - divider.drag_delta().x)
                        .clamp(min_width, max_width);
                }
            });
    }

    /// Inspector 双击弹出的完整内容窗口：可全选复制，或一键复制。
    /// 点击窗口外部自动关闭。
    fn inspector_popup_window(&mut self, ctx: &egui::Context, pal: &Palette) {
        let Some(value) = self.inspector_popup.clone() else {
            self.popup_opened_at = None;
            return;
        };
        // 记录打开时刻：防止打开弹窗的那次双击立即触发"点击外部关闭"
        let now = ctx.input(|i| i.time);
        if self.popup_opened_at.is_none() {
            self.popup_opened_at = Some(now);
        }
        let t = tr(self.lang);
        // 淡入 + 上滑动画：fade ∈ [0,1]
        let fade = self
            .popup_opened_at
            .map(|opened| (((now - opened) / 0.15).clamp(0.0, 1.0)) as f32)
            .unwrap_or(1.0);
        // 弹窗位置跟随双击点（限制在屏幕范围内）
        let screen = ctx.screen_rect();
        let pos = self
            .popup_pos
            .map(|p| {
                egui::pos2(
                    (p.x - 20.0).clamp(screen.left() + 8.0, screen.right() - 368.0),
                    (p.y + 8.0).clamp(screen.top() + 46.0, screen.bottom() - 120.0),
                )
            })
            .unwrap_or(egui::pos2(screen.center().x - 180.0, screen.bottom() - 160.0));
        let mut copy = false;
        let slide = (1.0 - fade) * 10.0;
        let window_response = egui::Window::new(RichText::new(t.value_detail).size(14.0).strong())
            .fixed_pos(egui::pos2(pos.x, pos.y + slide))
            .pivot(egui::Align2::LEFT_TOP)
            .collapsible(false)
            .resizable(false)
            .fixed_size([360.0, 0.0])
            .frame(
                egui::Frame::new()
                    .fill(pal.bg_panel)
                    .corner_radius(10.0)
                    .stroke(Stroke::new(1.0_f32, pal.bg_elevated))
                    .inner_margin(egui::Margin::same(12)),
            )
            .show(ctx, |ui| {
                ui.set_opacity(fade);
                // 可选中、自动换行的完整内容
                ui.add(
                    egui::Label::new(
                        RichText::new(&value).size(12.5).color(pal.text_main).monospace(),
                    )
                    .wrap(),
                );
                ui.add_space(8.0);
                if pill_button(ui, t.copy, true, pal).clicked() {
                    ui.ctx().copy_text(value.clone());
                    copy = true;
                }
            });
        let window_rect = window_response.map(|r| r.response.rect);

        // 点击外部关闭（打开后 0.3 秒内不判定，防误触）
        let click_outside = ctx.input(|i| i.pointer.any_click())
            && ctx
                .input(|i| i.pointer.interact_pos())
                .and_then(|p| window_rect.map(|r| !r.contains(p)))
                .unwrap_or(false)
            && self
                .popup_opened_at
                .is_some_and(|opened| now - opened > 0.3);
        if copy || click_outside {
            self.inspector_popup = None;
            self.popup_opened_at = None;
        }
    }
}

/// Inspector 值行：标签与值同行；超宽按面板宽度省略；
/// 双击返回完整值（由调用方弹窗展示）；右键"复制值"复制完整内容。
fn inspector_value_row(
    ui: &mut egui::Ui,
    pal: &Palette,
    t: &i18n::Strings,
    label: &str,
    value: Option<String>,
    char_w: f32,
) -> Option<(String, egui::Pos2)> {
    let mut popup: Option<(String, egui::Pos2)> = None;
    ui.horizontal(|ui| {
        ui.monospace(RichText::new(label).size(12.0).color(pal.text_faint));
        ui.add_space(4.0);
        let Some(full) = value else {
            ui.monospace(RichText::new("—").size(12.0).color(pal.text_faint));
            return;
        };

        // 值显示宽度 ≈ 剩余宽度
        let max_chars = (((ui.available_width() - 8.0) / char_w) as usize).max(6);
        let truncated = full.chars().count() > max_chars;
        let display = if truncated {
            let mut s: String = full.chars().take(max_chars.saturating_sub(1)).collect();
            s.push('…');
            s
        } else {
            full.clone()
        };

        let resp = ui.monospace(RichText::new(display).size(12.0).color(pal.text_main));
        let resp = if truncated { resp.on_hover_text(t.value_hint) } else { resp };
        if resp.double_clicked()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            popup = Some((full.clone(), pos));
        }
        resp.context_menu(|menu| {
            if menu.button(t.copy_value).clicked() {
                ui.ctx().copy_text(full.clone());
                menu.close_menu();
            }
        });
    });
    popup
}

/// 自研下拉选择框：芯片按钮 + 自绘弹层，颜色全部取自调色板（明暗主题一致）。
/// 返回用户点选的新值。
fn dropdown<S: Clone + PartialEq + 'static>(
    ui: &mut egui::Ui,
    pal: &Palette,
    id: egui::Id,
    selected: String,
    options: Vec<(String, S)>,
    width: f32,
) -> Option<S> {
    let mut picked: Option<S> = None;
    let open = ui.memory(|m| m.is_popup_open(id));

    // 芯片按钮：选中值 + 下拉三角
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 26.0), egui::Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let p = ui.painter();
    p.rect_filled(
        rect,
        6.0,
        if open { pal.accent.gamma_multiply(0.25) } else { pal.bg_elevated },
    );
    p.text(
        egui::pos2(rect.left() + 10.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        &selected,
        FontId::proportional(13.0),
        pal.text_main,
    );
    let tri = egui::Rect::from_center_size(
        egui::pos2(rect.right() - 14.0, rect.center().y),
        egui::vec2(10.0, 5.0),
    );
    let pts = if open {
        vec![tri.left_bottom(), tri.right_bottom(), tri.center_top()]
    } else {
        vec![tri.left_top(), tri.right_top(), tri.center_bottom()]
    };
    p.add(egui::Shape::convex_polygon(pts, pal.text_main, Stroke::NONE));

    if resp.clicked() {
        ui.memory_mut(|m| if open { m.close_popup() } else { m.open_popup(id) });
    }

    // 弹层列表
    if open {
        let area_id = id.with("popup");
        egui::Area::new(area_id)
            .fixed_pos(egui::pos2(rect.left(), rect.bottom() + 4.0))
            .order(egui::Order::Foreground)
            .default_size([width, 0.0])
            .show(ui.ctx(), |ui| {
                egui::Frame::new()
                    .fill(pal.bg_panel)
                    .corner_radius(8.0)
                    .stroke(Stroke::new(1.0_f32, pal.bg_elevated))
                    .inner_margin(egui::Margin::same(4))
                    .show(ui, |ui| {
                        ui.set_min_width(width - 8.0);
                        for (name, value) in &options {
                            let is_selected = *name == selected;
                            let (r, rresp) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), 24.0),
                                egui::Sense::click(),
                            );
                            let rresp = rresp.on_hover_cursor(egui::CursorIcon::PointingHand);
                            let pr = ui.painter_at(r);
                            if is_selected || rresp.hovered() {
                                pr.rect_filled(
                                    r,
                                    4.0,
                                    if is_selected {
                                        pal.accent.gamma_multiply(0.25)
                                    } else {
                                        pal.bg_elevated
                                    },
                                );
                            }
                            pr.text(
                                egui::pos2(r.left() + 6.0, r.center().y),
                                egui::Align2::LEFT_CENTER,
                                name,
                                FontId::proportional(12.5),
                                if is_selected { pal.accent } else { pal.text_main },
                            );
                            if rresp.clicked() {
                                picked = Some(value.clone());
                                ui.memory_mut(|m| m.close_popup());
                            }
                        }
                    });
            });

        // 点击按钮与弹层之外的区域关闭
        if ui.input(|i| i.pointer.any_click()) {
            let pointer = ui.input(|i| i.pointer.interact_pos());
            let in_button = pointer.is_some_and(|p| rect.contains(p));
            let in_popup = pointer.is_some_and(|p| {
                ui.ctx()
                    .read_response(area_id)
                    .is_some_and(|r| r.rect.contains(p))
            });
            if !in_button && !in_popup {
                ui.memory_mut(|m| m.close_popup());
            }
        }
    }

    picked
}

/// 工具栏图标开关：active 高亮底色，图标由 draw 回调以当前颜色绘制。
fn toolbar_toggle(
    ui: &mut egui::Ui,
    active: bool,
    tooltip: &str,
    pal: &Palette,
    draw: impl FnOnce(&egui::Painter, egui::Rect, Color32),
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::click());
    let resp = resp.on_hover_text(tooltip).on_hover_cursor(egui::CursorIcon::PointingHand);
    let hovered = resp.hovered();
    if active || hovered {
        let bg = if active {
            pal.accent.gamma_multiply(0.25)
        } else {
            pal.bg_elevated
        };
        ui.painter().rect_filled(rect, 6.0, bg);
    }
    let color = if active {
        pal.accent
    } else if hovered {
        pal.text_main
    } else {
        pal.text_faint
    };
    draw(ui.painter(), rect, color);
    resp
}

// ---------------------------------------------------------------------------
// 视图
// ---------------------------------------------------------------------------

/// 欢迎页整体位于中部略偏下，兼顾上方留白与底部按钮的空间。
fn welcome_rects(bounds: egui::Rect) -> (egui::Rect, egui::Rect) {
    let width = (bounds.width() - 48.0).clamp(0.0, 880.0);
    let gap = if width < 720.0 { 24.0 } else { 40.0 };
    let recent_w = (width * 0.32).min(240.0);
    let left_w = (width - gap - recent_w).max(0.0);
    let left = bounds.center().x - width * 0.5;
    let top = bounds.center().y - 169.0;
    (
        egui::Rect::from_min_size(egui::pos2(left, top), egui::vec2(left_w, 380.0)),
        egui::Rect::from_min_size(
            egui::pos2(left + left_w + gap, top + 30.0),
            egui::vec2(recent_w, 320.0),
        ),
    )
}

fn welcome_screen(ui: &mut egui::Ui, app: &mut LiteTextApp, pal: &Palette) {
    let t = tr(app.lang);

    // 自动清理磁盘上已不存在的最近文件
    let missing = app.recent_files.iter().any(|p| !p.exists());
    if missing {
        app.recent_files.retain(|p| p.exists());
    }

    // 小窗口允许纵向滚动，正常窗口中两栏始终围绕内容区中心布局。
    let height = ui.available_height().max(412.0);
    egui::ScrollArea::vertical()
        .id_salt("welcome_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let (bounds, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), height),
                egui::Sense::hover(),
            );
            let (left_rect, recent_rect) = welcome_rects(bounds);
            // 下移后，小窗口需要为底部按钮和提示保留可滚动的空间。
            ui.set_min_height(height.max(left_rect.bottom() - bounds.top() + 16.0));
            ui.allocate_new_ui(
                egui::UiBuilder::new()
                    .max_rect(left_rect)
                    .layout(egui::Layout::top_down(egui::Align::Center)),
                |ui| {
                ui.vertical_centered(|ui| {
                    // 品牌直接绘制，避免 Label 的文本选区与横向布局导致左上角错位。
                    let brand = FontId::new(42.0, FontFamily::Name("brand".into()));
                    let lite = ui.painter().layout_no_wrap("Lite".to_owned(), brand.clone(), pal.text_main);
                    let text = ui.painter().layout_no_wrap("Text".to_owned(), brand, pal.accent);
                    let (rect, _) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), 54.0),
                        egui::Sense::hover(),
                    );
                    let pos = egui::pos2(
                        rect.center().x - (lite.size().x + text.size().x) * 0.5,
                        rect.center().y - lite.size().y * 0.5,
                    );
                    let text_pos = pos + egui::vec2(lite.size().x, 0.0);
                    ui.painter().galley(pos, lite, pal.text_main);
                    ui.painter().galley(text_pos, text, pal.accent);
                    ui.label(RichText::new(t.tagline).color(pal.text_faint).size(14.0));
                    ui.add_space(36.0);

                    // 虚线拖放区域
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(ui.available_width().min(380.0), 110.0), egui::Sense::hover());
                    ui.painter().rect_stroke(
                        rect,
                        10.0,
                        Stroke::new(1.2_f32, pal.text_faint.gamma_multiply(0.6)),
                        egui::StrokeKind::Inside,
                    );
                    ui.painter().text(
                        rect.center() - egui::vec2(0.0, 16.0),
                        egui::Align2::CENTER_CENTER,
                        "↓",
                        FontId::proportional(28.0),
                        pal.accent,
                    );
                    ui.painter().text(
                        rect.center() + egui::vec2(0.0, 18.0),
                        egui::Align2::CENTER_CENTER,
                        t.drop_hint,
                        FontId::proportional(14.0),
                        pal.text_main,
                    );

                    ui.add_space(18.0);
                    ui.label(RichText::new(t.or).color(pal.text_faint).size(12.0));
                    ui.add_space(6.0);
                    if pill_button(ui, t.choose_file, true, pal).clicked() {
                        OPEN_REQUEST.store(true, Ordering::Relaxed);
                    }
                    ui.add_space(24.0);
                    ui.label(
                        RichText::new(t.footer_hint)
                            .color(pal.text_faint.gamma_multiply(0.8))
                            .size(11.5),
                    );
                });
            });

        // ── 右：最近打开的面板，属于居中组合而非贴在窗口右缘 ──
        ui.allocate_new_ui(
            egui::UiBuilder::new()
                .max_rect(recent_rect)
                .layout(egui::Layout::top_down(egui::Align::LEFT)),
            |ui| {
            egui::Frame::new()
                .fill(pal.bg_panel)
                .corner_radius(10.0)
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.set_width((recent_rect.width() - 20.0).max(0.0));
                    ui.set_min_height(recent_rect.height() - 20.0);
                    ui.label(RichText::new(t.recent).size(12.0).strong().color(pal.text_faint));
                    ui.add_space(4.0);

                    egui::ScrollArea::vertical()
                        .id_salt("welcome_recent")
                        .max_height(260.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                    if app.recent_files.is_empty() {
                        ui.add_space(10.0);
                        ui.label(
                            RichText::new(t.recent_empty)
                                .size(11.5)
                                .color(pal.text_faint.gamma_multiply(0.8)),
                        );
                    }
                    let paths: Vec<PathBuf> = app.recent_files.clone();
                    for path in paths {
                        let name = path
                            .file_name()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        let dir = path
                            .parent()
                            .map(|p| p.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        let (rect, resp) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), 34.0),
                            egui::Sense::click(),
                        );
                        let resp = resp.on_hover_text(path.to_string_lossy().into_owned());
                        let p = ui.painter_at(rect);
                        if resp.hovered() {
                            p.rect_filled(rect, 6.0, pal.bg_elevated);
                        }
                        p.text(
                            egui::pos2(rect.left() + 8.0, rect.top() + 10.0),
                            egui::Align2::LEFT_CENTER,
                            name,
                            FontId::proportional(12.0),
                            pal.text_main,
                        );
                        p.text(
                            egui::pos2(rect.left() + 8.0, rect.bottom() - 8.0),
                            egui::Align2::LEFT_CENTER,
                            dir,
                            FontId::proportional(10.0),
                            pal.text_faint,
                        );
                        if resp.clicked() {
                            app.open_paths(vec![path.clone()]);
                        }
                    }
                    });
                });
            },
        );
    });
}

fn draft_editor(ui: &mut egui::Ui, file: &mut LoadedFile, pal: &Palette, focus: bool, lang: Lang) {
    if let Some(render) = &file.editor_state.tools.workspace {
        ui.painter().rect_filled(ui.available_rect_before_wrap(), 0.0, render.colors.background);
    }
    file.prepare_changes();
    text_tools::find_bar(ui, file, pal, tr(lang));
    file.editor_state.folds.sync(file.draft_text.as_deref().expect("text buffer"), (file.content_revision, file.editor_state.large.revision));
    if !file.editor_state.large.active && file.draft_text.as_ref().is_some_and(|t| t.len() >= large_editor::THRESHOLD)
        && file.saved_large_revision.is_none()
    {
        file.saved_large_revision = (!file.is_modified()).then_some(0);
    }
    file.editor_state.large.set_encoding(file.draft_text.as_ref().expect("text buffer"), file.encoding);
    let before_selection = (file.editor_state.selected_characters, file.editor_state.cursor_line);
    if editor::show(
        ui,
        file.draft_text.as_mut().expect("草稿编辑器需要文本缓冲区"),
        &mut file.editor_state,
        file.id,
        pal,
        focus,
        tr(lang),
    ) {
        ui.ctx().request_repaint();
        file.editor_state.tools.current = None;
        if file.editor_state.large.active {
            file.character_count = file.editor_state.large.characters;
            file.bytes_dirty = true;
            // The baseline remains available for conflict checks; do not retain a stale HEX copy.
            file.bytes = Vec::new();
            file.lines = Vec::new();
        } else { file.sync_draft(); }
    }
    if before_selection != (file.editor_state.selected_characters, file.editor_state.cursor_line) { ui.ctx().request_repaint(); }
}

fn text_view(ui: &mut egui::Ui, file: &LoadedFile, pal: &Palette) {
    if file.lines.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label(RichText::new(tr(Lang::default()).empty_file).color(pal.text_faint));
        });
        return;
    }
    const ROW_HEIGHT: f32 = 19.0;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_rows(ui, ROW_HEIGHT, file.lines.len(), |ui, range| {
            for i in range {
                ui.horizontal(|row| {
                    row.monospace(
                        RichText::new(format!("{:>6}  ", i + 1))
                            .color(pal.text_faint.gamma_multiply(0.7))
                            .size(12.0),
                    );
                    row.monospace(RichText::new(file.lines[i].as_str()).size(13.5).color(pal.text_main));
                });
            }
        });
}

/// HEX 视图选项（由顶部工具栏控制）。
#[derive(Clone, Copy)]
struct HexViewOpts {
    font_size: f32,
    upper: bool,
    dim_zero: bool,
    show_ascii: bool,
    show_invisible: bool,
}

/// 按大小写格式化一个字节。
fn hex_byte(b: u8, upper: bool) -> String {
    if upper { format!("{b:02X}") } else { format!("{b:02x}") }
}

/// ASCII 列字符：可打印字符原样；不可见内容由开关决定显示控制图片或占位符。
fn ascii_char(b: u8, show_invisible: bool) -> char {
    match b {
        0x20..=0x7E => b as char,
        0x00..=0x1F if show_invisible && control_pictures_ok() => {
            char::from_u32(0x2400 + b as u32).unwrap_or('·')
        }
        0x7F if show_invisible && control_pictures_ok() => '\u{2421}',
        _ => '·',
    }
}

/// HEX 视图：冻结表头（Address / 00-0F / ASCII）+ 拖拽范围选择 + 两侧联动高亮。
/// 点击地址列选中整行；点击/拖拽内容字节则在两列同步高亮。
#[allow(clippy::too_many_arguments)]
fn hex_view(
    ui: &mut egui::Ui,
    file: &LoadedFile,
    pal: &Palette,
    selection: &mut Option<(usize, usize)>,
    drag_anchor: &mut Option<usize>,
    opts: &HexViewOpts,
) {
    if file.bytes.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label(RichText::new(tr(Lang::default()).empty_file).color(pal.text_faint));
        });
        return;
    }

    let font_size = opts.font_size - 0.5;
    let mono = FontId::monospace(font_size);
    let row_height = editor::row_height(ui, &mono) + 3.0;
    editor::scroll_style(ui, pal);
    // 字符间距必须取自真实排版的字形位置差（egui 逐字形做物理像素取整），
    // 用单字符 galley 的宽度测量会得到未取整值，导致高亮与字形错位。
    let probe = ui.painter().layout_no_wrap("0000000000".to_owned(), mono.clone(), pal.text_main);
    let char_w = probe
        .rows
        .first()
        .and_then(|r| {
            let g = &r.glyphs;
            (g.len() >= 2).then(|| g[1].pos.x - g[0].pos.x)
        })
        .unwrap_or_else(|| probe.size().x);
    const ADDR_CHARS: f32 = 10.0; // 8 位地址 + 2 空格
    const HEX_CHARS: f32 = 49.0; // 16 字节 × 3 字符 + 中缝 1 空格
    let addr_w = ADDR_CHARS * char_w;
    let hex_x = addr_w;
    let ascii_x = (ADDR_CHARS + HEX_CHARS + 1.0) * char_w;

    // ── 冻结表头：不随内容滚动，普通标签不可选择 ──
    let mut header = format!("{:<10}", "Address");
    for i in 0..HEX_BYTES_PER_ROW {
        if i == 8 {
            header.push(' ');
        }
        header.push_str(&hex_byte(i as u8, opts.upper));
        header.push(' ');
    }
    if opts.show_ascii {
        header.push_str(" ASCII");
    }
    egui::Frame::new()
        .fill(pal.bg_panel)
        .inner_margin(egui::Margin::symmetric(14, 4))
        .show(ui, |ui| {
            ui.monospace(RichText::new(header).size(font_size).color(pal.text_faint));
        });
    let span = ui.min_rect().x_range();
    let line_y = ui.min_rect().bottom();
    ui.painter().line_segment(
        [egui::pos2(span.min, line_y), egui::pos2(span.max, line_y)],
        Stroke::new(1.0_f32, pal.bg_inset),
    );

    let total_rows = file.bytes.len().div_ceil(HEX_BYTES_PER_ROW);
    // 右侧留出间距：避免内容滚动条与 Inspector 分隔条重叠导致光标样式抖动
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 0,
            right: 12,
            top: 0,
            bottom: 0,
        })
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                .auto_shrink([false, false])
                .show_rows(ui, row_height, total_rows, |ui, range| {
                    egui::Frame::new()
                        .inner_margin(egui::Margin {
                            left: 14,
                            right: 14,
                            top: 0,
                            bottom: 0,
                        })
                        .show(ui, |ui| {
                            for row in range {
                        let start = row * HEX_BYTES_PER_ROW;
                        let end = (start + HEX_BYTES_PER_ROW).min(file.bytes.len());
                        let chunk = &file.bytes[start..end];
                        // 本行与选中区间的重叠列 [c0, c1]（含端点）
                        let sel_cols = overlap_cols(*selection, row, chunk.len());

                        let (rect, resp) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), row_height),
                            egui::Sense::click_and_drag(),
                        );
                        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                        let p = ui.painter_at(rect);

                        // 悬停底色
                        if resp.hovered() && sel_cols.is_none() {
                            p.rect_filled(rect, 3.0, pal.bg_elevated.gamma_multiply(0.5));
                        }

                        // 选中区间的两侧高亮背景（连续区段一次绘制）
                        if let Some((c0, c1)) = sel_cols {
                            let x0 = rect.left() + hex_x + hex_col_char_x(c0) as f32 * char_w;
                            let x1 =
                                rect.left() + hex_x + (hex_col_char_x(c1) as f32 + 2.0) * char_w;
                            p.rect_filled(
                                egui::Rect::from_min_max(
                                    egui::pos2(x0, rect.top() + 1.0),
                                    egui::pos2(x1, rect.bottom() - 1.0),
                                ),
                                2.0,
                                pal.accent,
                            );
                            if opts.show_ascii {
                                let ax0 = rect.left() + ascii_x + c0 as f32 * char_w;
                                let ax1 = rect.left() + ascii_x + (c1 + 1) as f32 * char_w;
                                p.rect_filled(
                                    egui::Rect::from_min_max(
                                        egui::pos2(ax0, rect.top() + 1.0),
                                        egui::pos2(ax1, rect.bottom() - 1.0),
                                    ),
                                    2.0,
                                    pal.accent,
                                );
                            }
                        }

                        // 整行内容：单个 LayoutJob 保证列对齐
                        let mut job = LayoutJob::default();
                        job.append(
                            &format!("{start:08X}  "),
                            0.0,
                            TextFormat::simple(mono.clone(), pal.accent.gamma_multiply(0.9)),
                        );
                        for idx in 0..HEX_BYTES_PER_ROW {
                            if idx == 8 {
                                job.append(
                                    " ",
                                    0.0,
                                    TextFormat::simple(mono.clone(), Color32::TRANSPARENT),
                                );
                            }
                            let text = if idx < chunk.len() {
                                format!("{} ", hex_byte(chunk[idx], opts.upper))
                            } else {
                                "   ".to_owned()
                            };
                            let mut color = if opts.dim_zero && chunk.get(idx) == Some(&0) {
                                pal.text_faint.gamma_multiply(0.55)
                            } else {
                                pal.text_main
                            };
                            if sel_cols.is_some_and(|(c0, c1)| idx >= c0 && idx <= c1) {
                                color = Color32::WHITE;
                            }
                            job.append(&text, 0.0, TextFormat::simple(mono.clone(), color));
                        }
                        if opts.show_ascii {
                            job.append(
                                " ",
                                0.0,
                                TextFormat::simple(mono.clone(), Color32::TRANSPARENT),
                            );
                            for idx in 0..HEX_BYTES_PER_ROW {
                                let ch = chunk
                                    .get(idx)
                                    .map(|b| ascii_char(*b, opts.show_invisible))
                                    .unwrap_or(' ');
                                let mut color = pal.text_faint;
                                if opts.dim_zero && chunk.get(idx) == Some(&0) {
                                    color = pal.text_faint.gamma_multiply(0.55);
                                }
                                if sel_cols.is_some_and(|(c0, c1)| idx >= c0 && idx <= c1) {
                                    color = Color32::WHITE;
                                }
                                job.append(&ch.to_string(), 0.0, TextFormat::simple(mono.clone(), color));
                            }
                        }
                        let galley = ui.painter().layout_job(job);
                        let y_off = (row_height - galley.size().y) * 0.5;
                        p.galley(egui::pos2(rect.left(), rect.top() + y_off), galley, pal.text_main);

                        // 交互：点击选择，拖拽扩展区间
                        if resp.drag_started()
                            && let Some(b) = byte_at(
                                resp.interact_pointer_pos(),
                                rect.left(),
                                row,
                                chunk.len(),
                                char_w,
                                hex_x,
                                ascii_x,
                                opts.show_ascii,
                            )
                        {
                            *drag_anchor = Some(b);
                            *selection = Some((b, b));
                        }
                        if resp.dragged()
                            && let (Some(a), Some(b)) = (
                                *drag_anchor,
                                byte_at(
                                    resp.interact_pointer_pos(),
                                    rect.left(),
                                    row,
                                    chunk.len(),
                                    char_w,
                                    hex_x,
                                    ascii_x,
                                    opts.show_ascii,
                                ),
                            )
                        {
                            *selection = Some((a.min(b), a.max(b)));
                        }
                        if resp.drag_stopped() {
                            *drag_anchor = None;
                        }
                        if resp.clicked() {
                            let px = resp
                                .interact_pointer_pos()
                                .unwrap_or(egui::pos2(rect.left(), rect.center().y))
                                .x
                                - rect.left();
                            if px < addr_w {
                                // 地址列：整行区间
                                let rs = row * HEX_BYTES_PER_ROW;
                                let re = rs + chunk.len() - 1;
                                *selection = if *selection == Some((rs, re)) {
                                    None
                                } else {
                                    Some((rs, re))
                                };
                                *drag_anchor = None;
                            } else if let Some(b) = byte_at(
                                resp.interact_pointer_pos(),
                                rect.left(),
                                row,
                                chunk.len(),
                                char_w,
                                hex_x,
                                ascii_x,
                                opts.show_ascii,
                            ) {
                                if *selection == Some((b, b)) && drag_anchor.is_none() {
                                    *selection = None; // 重复点击同一字节取消选中
                                } else {
                                    *selection = Some((b, b));
                                    *drag_anchor = Some(b);
                                }
                            } else {
                                *selection = None;
                            }
                        }
                    }
                });
            });
        });
}

/// 十六进制列中第 col 字节的字符偏移（每字节 3 字符，第 8 字节后多 1 空格）。
fn hex_col_char_x(col: usize) -> usize {
    col * 3 + if col >= 8 { 1 } else { 0 }
}

/// 选中区间在本行的列范围 [c0, c1]，无交集时返回 None。
fn overlap_cols(
    selection: Option<(usize, usize)>,
    row: usize,
    len: usize,
) -> Option<(usize, usize)> {
    let (s, e) = selection?;
    let rs = row * HEX_BYTES_PER_ROW;
    let re = rs + len - 1;
    if e < rs || s > re {
        return None;
    }
    Some((s.saturating_sub(rs), (e - rs).min(len - 1)))
}

/// 把行内横向位置换算为字节索引；不在内容区返回 None。
#[allow(clippy::too_many_arguments)]
fn byte_at(
    pointer: Option<egui::Pos2>,
    row_left: f32,
    row: usize,
    len: usize,
    char_w: f32,
    hex_x: f32,
    ascii_x: f32,
    show_ascii: bool,
) -> Option<usize> {
    let px = pointer?.x - row_left;
    let col = if px >= hex_x && px < ascii_x - char_w {
        let ci = ((px - hex_x) / char_w) as usize;
        if ci < 24 {
            ci / 3
        } else if ci == 24 {
            7
        } else {
            (8 + (ci - 25) / 3).min(15)
        }
    } else if show_ascii && px >= ascii_x {
        (((px - ascii_x) / char_w) as usize).min(15)
    } else {
        return None;
    };
    (col < len).then(|| row * HEX_BYTES_PER_ROW + col)
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_icon_is_valid_and_has_a_bold_centered_t() {
        let icon = app_icon();
        assert_eq!((icon.width, icon.height), (512, 512));
        assert_eq!(icon.rgba.len(), 512 * 512 * 4);
        let pixel = |x: usize, y: usize| &icon.rgba[(y * 512 + x) * 4..][..4];
        assert_eq!(pixel(0, 0), [0, 0, 0, 0]);
        assert_eq!(pixel(256, 100), [14, 17, 22, 255]);
        for (x, y) in [(160, 170), (350, 170), (240, 300), (270, 360)] {
            assert_eq!(pixel(x, y), [248, 250, 252, 255]);
        }
        assert_eq!(pixel(200, 300), pixel(310, 300));
        assert_eq!(pixel(200, 300), pixel(256, 100));
    }

    #[test]
    fn recovery_dot_tracks_each_document_and_only_successful_current_versions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("original.txt");
        let snapshot = dir.path().join("session.json");
        std::fs::write(&path, "original").unwrap();
        let mut app = LiteTextApp { recovery_path_override: Some(snapshot.clone()), ..Default::default() };
        app.new_document();
        let blank_id = app.active_id.unwrap();
        assert_eq!(app.recovery_indicator(app.active().unwrap()), RecoveryIndicator::Idle);
        app.write_recovery().unwrap();
        assert_eq!(app.recovery_indicator(app.active().unwrap()), RecoveryIndicator::Idle);
        app.open_paths(vec![path.clone()]);
        let file_id = app.active_id.unwrap();
        assert_eq!(app.recovery_indicator(app.active().unwrap()), RecoveryIndicator::Idle);
        let ctx = egui::Context::default();
        text_tools::replace_range(app.active_mut().unwrap(), &ctx, 0..0, "edit ".to_owned());
        assert_eq!(app.recovery_indicator(app.active().unwrap()), RecoveryIndicator::Pending);
        app.write_recovery().unwrap();
        assert_eq!(app.recovery_indicator(app.active().unwrap()), RecoveryIndicator::BackedUp);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "original");
        app.active_id = Some(blank_id);
        assert_eq!(app.recovery_indicator(app.active().unwrap()), RecoveryIndicator::Idle);
        text_tools::replace_range(app.active_mut().unwrap(), &ctx, 0..0, "new text".to_owned());
        assert_eq!(app.recovery_indicator(app.active().unwrap()), RecoveryIndicator::Pending);
        app.active_id = Some(file_id);
        assert_eq!(app.recovery_indicator(app.active().unwrap()), RecoveryIndicator::BackedUp);
        app.write_recovery().unwrap();
        text_tools::replace_range(app.active_mut().unwrap(), &ctx, 0..0, "more ".to_owned());
        assert_eq!(app.recovery_indicator(app.active().unwrap()), RecoveryIndicator::Pending);
        app.write_recovery().unwrap();
        assert_eq!(app.recovery_indicator(app.active().unwrap()), RecoveryIndicator::BackedUp);
        assert!(app.commit_save(file_id, &path, None));
        assert_eq!(app.recovery_indicator(app.active().unwrap()), RecoveryIndicator::Idle);
        app.write_recovery().unwrap();
        assert_eq!(app.recovery_indicator(app.active().unwrap()), RecoveryIndicator::Idle);

        let mut restored = LiteTextApp { recovery_path_override: Some(snapshot), ..Default::default() };
        restored.restore_session();
        restored.active_id = Some(blank_id);
        assert_eq!(restored.recovery_indicator(restored.active().unwrap()), RecoveryIndicator::BackedUp);
        restored.active_id = Some(file_id);
        assert_eq!(restored.recovery_indicator(restored.active().unwrap()), RecoveryIndicator::Idle);

        // Returning to the saved contents also turns the dot off without a disk write.
        text_tools::replace_range(app.active_mut().unwrap(), &ctx, 0..0, "x".to_owned());
        text_tools::replace_range(app.active_mut().unwrap(), &ctx, 0..1, String::new());
        assert_eq!(app.recovery_indicator(app.active().unwrap()), RecoveryIndicator::Idle);
    }

    #[test]
    fn recovery_dot_does_not_turn_green_after_failed_backup_or_save_and_tracks_large_edits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.txt");
        let snapshot = dir.path().join("session.json");
        let original = "original line\n".repeat(25_000);
        std::fs::write(&path, &original).unwrap();
        let mut app = LiteTextApp { recovery_path_override: Some(snapshot.clone()), ..Default::default() };
        app.open_paths(vec![path.clone()]);
        let ctx = egui::Context::default();
        text_tools::replace_range(app.active_mut().unwrap(), &ctx, 0..0, "first".to_owned());
        app.write_recovery().unwrap();
        assert_eq!(app.recovery_indicator(app.active().unwrap()), RecoveryIndicator::BackedUp);
        let revision = app.active().unwrap().content_revision;
        text_tools::replace_range(app.active_mut().unwrap(), &ctx, 0..0, "second".to_owned());
        assert_eq!(app.active().unwrap().content_revision, revision, "large edits use a separate revision");
        assert_eq!(app.recovery_indicator(app.active().unwrap()), RecoveryIndicator::Pending);
        let blocked = dir.path().join("not-a-directory");
        std::fs::write(&blocked, b"blocked").unwrap();
        app.recovery_path_override = Some(blocked.join("session.json"));
        assert!(app.write_recovery().is_err());
        assert_eq!(app.recovery_indicator(app.active().unwrap()), RecoveryIndicator::Pending);
        app.recovery_path_override = Some(snapshot);
        app.write_recovery().unwrap();
        assert_eq!(app.recovery_indicator(app.active().unwrap()), RecoveryIndicator::BackedUp);
        let id = app.active_id.unwrap();
        std::fs::write(&path, "external change").unwrap();
        assert!(!app.commit_save(id, &path, None));
        assert_eq!(app.recovery_indicator(app.active().unwrap()), RecoveryIndicator::BackedUp);
        std::fs::write(&path, &original).unwrap();
        assert!(app.commit_save(id, &path, None));
        assert_eq!(app.recovery_indicator(app.active().unwrap()), RecoveryIndicator::Idle);
    }

    #[test]
    fn recovery_dot_is_small_muted_and_has_no_backup_text_in_either_theme() {
        for pal in [&DARK, &LIGHT] {
            let dir = tempfile::tempdir().unwrap();
            let mut app = LiteTextApp { recovery_path_override: Some(dir.path().join("session.json")), ..Default::default() };
            let ctx = egui::Context::default();
            app.new_document();
            for expected in [RecoveryIndicator::Idle, RecoveryIndicator::Pending, RecoveryIndicator::BackedUp] {
                match expected {
                    RecoveryIndicator::Idle => {},
                    RecoveryIndicator::Pending => text_tools::replace_range(app.active_mut().unwrap(), &ctx, 0..0, "text".to_owned()),
                    RecoveryIndicator::BackedUp => app.write_recovery().unwrap(),
                }
                let output = ctx.run(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO, egui::vec2(1080.0, 720.0))), ..Default::default() }, |ctx| app.status_bar(ctx, pal));
                let mut dots = 0;
                for shape in output.shapes {
                    match shape.shape {
                        egui::Shape::Circle(circle) if circle.fill == expected.color(pal) => {
                            dots += 1;
                            assert_eq!(circle.radius, 4.0);
                            assert!(circle.center.x > 1050.0);
                        },
                        egui::Shape::Text(text) => {
                            assert!(!text.galley.text().contains("备份"));
                            assert!(!text.galley.text().contains("backed up"));
                            assert_ne!(text.galley.text(), tr(app.lang).unsaved);
                        },
                        _ => {},
                    }
                }
                assert_eq!(dots, 1);
            }
        }
    }

    #[test]
    fn scratch_tab_tint_tracks_unsaved_content_in_both_themes_and_drag_preview() {
        for pal in [&DARK, &LIGHT] {
            for large in [false, true] {
                for mode in ["active", "inactive", "dragging"] {
                    let dir = tempfile::tempdir().unwrap();
                    let path = dir.path().join("document.txt");
                    let mut app = LiteTextApp::default();
                    app.new_document();
                    let id = app.active_id.unwrap();
                    app.new_document();
                    if mode != "inactive" { app.active_id = Some(id); }
                    let ctx = egui::Context::default();
                    let tinted = |app: &mut LiteTextApp| {
                        app.tab_drag_id = (mode == "dragging").then_some(id);
                        let output = ctx.run(egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1080.0, 720.0))),
                            events: vec![egui::Event::PointerMoved(egui::pos2(220.0, 24.0))],
                            ..Default::default()
                        }, |ctx| app.top_bar(ctx, pal));
                        output.shapes.iter().any(|shape| matches!(&shape.shape,
                            egui::Shape::Rect(rect) if rect.fill == pal.danger.gamma_multiply(if mode == "inactive" { 0.08 } else { 0.12 })))
                    };
                    assert!(!tinted(&mut app), "new empty tab: {mode}");
                    let original = "content\n".repeat(if large { 40_000 } else { 1 });
                    let file = app.files.iter_mut().find(|f| f.id == id).unwrap();
                    text_tools::replace_range(file, &ctx, 0..0, original.clone());
                    assert!(tinted(&mut app), "unsaved content: {mode}");
                    let file = app.files.iter_mut().find(|f| f.id == id).unwrap();
                    text_tools::replace_range(file, &ctx, 0..original.len(), String::new());
                    assert!(!tinted(&mut app), "cleared scratch tab: {mode}");
                    let file = app.files.iter_mut().find(|f| f.id == id).unwrap();
                    text_tools::replace_range(file, &ctx, 0..0, original.clone());
                    recovery::save_document(file, &path).unwrap();
                    assert!(!tinted(&mut app), "saved tab: {mode}");
                    let file = app.files.iter_mut().find(|f| f.id == id).unwrap();
                    text_tools::replace_range(file, &ctx, 0..original.len(), String::new());
                    assert!(tinted(&mut app), "clearing a saved file is still an edit: {mode}");
                    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
                    recovery::save_document(app.files.iter_mut().find(|f| f.id == id).unwrap(), &path).unwrap();
                    assert!(!tinted(&mut app), "saved empty file: {mode}");
                }
            }
        }
    }

    #[test]
    fn saved_file_tab_gains_subtle_background_when_modified_and_clears_after_save() {
        for pal in [&DARK, &LIGHT] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("document.txt");
            std::fs::write(&path, b"original").unwrap();
            let mut app = LiteTextApp::default();
            app.open_paths(vec![path.clone()]);
            let id = app.active_id.unwrap();
            let ctx = egui::Context::default();
            let render = |app: &mut LiteTextApp| ctx.run(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO, egui::vec2(1080.0, 720.0))), ..Default::default() }, |ctx| app.top_bar(ctx, pal));
            let tinted = |output: egui::FullOutput, opacity| output.shapes.iter().any(|s| matches!(&s.shape,
                egui::Shape::Rect(r) if r.fill == pal.danger.gamma_multiply(opacity)));
            assert!(!tinted(render(&mut app), 0.12));
            let file = app.active_mut().unwrap();
            file.draft_text = Some("modified".to_owned()); file.sync_draft();
            assert!(tinted(render(&mut app), 0.12));
            app.new_document();
            assert!(tinted(render(&mut app), 0.08));
            recovery::save_document(app.files.iter_mut().find(|f| f.id == id).unwrap(), &path).unwrap();
            assert!(!tinted(render(&mut app), 0.08));
            app.active_id = Some(id);
            assert!(!tinted(render(&mut app), 0.12));
        }
    }

    #[test]
    fn status_encoding_matches_status_text_height_and_palette_in_both_themes() {
        for pal in [&DARK, &LIGHT] {
            let ctx = egui::Context::default();
            setup_fonts(&ctx);
            apply_style(&ctx, pal);
            let mut app = LiteTextApp::default();
            app.new_document();
            let mut editor_bottom = 0.0;
            for _ in 0..2 {
                let output = ctx.run(egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1080.0, 720.0))),
                    ..Default::default()
                }, |ctx| {
                    app.status_bar(ctx, pal);
                    egui::CentralPanel::default().show(ctx, |ui| { editor_bottom = ui.max_rect().bottom(); });
                });
                let mut encoding_center = None;
                let mut size_center = None;
                let mut encoding_x = None;
                let mut size_x = None;
                let mut count_x = None;
                for shape in output.shapes {
                    if let egui::Shape::Text(text) = shape.shape {
                        if text.galley.text() == "UTF-8" {
                            assert_eq!(text.galley.job.sections[0].format.color, pal.text_faint);
                            encoding_center = Some(text.pos.y + text.galley.size().y * 0.5);
                            encoding_x = Some(text.pos.x);
                        }
                        if text.galley.text() == "0 B" {
                            size_center = Some(text.pos.y + text.galley.size().y * 0.5);
                            size_x = Some(text.pos.x);
                        }
                        if text.galley.text() == "0 字" { count_x = Some(text.pos.x); }
                    }
                }
                assert!((encoding_center.unwrap() - size_center.unwrap()).abs() < 1.0);
                assert!(size_x.unwrap() < count_x.unwrap() && count_x.unwrap() < encoding_x.unwrap());
                assert!(editor_bottom > 675.0, "encoding picker must not inflate the status bar");
            }
        }
    }

    #[test]
    fn status_encoding_is_read_only_and_selection_count_follows_it() {
        let ctx = egui::Context::default();
        let mut app = LiteTextApp::default();
        app.new_document();
        app.active_mut().unwrap().draft_text = Some("中文".to_owned());
        app.active_mut().unwrap().sync_draft();
        app.active_mut().unwrap().editor_state.selected_characters = 2;
        let mut encoding_pos = None;
        for frame in 0..5 {
            let events = if [2, 3].contains(&frame) {
                let pos = encoding_pos.unwrap();
                vec![egui::Event::PointerMoved(pos), egui::Event::PointerButton { pos,
                    button: egui::PointerButton::Primary, pressed: frame == 2,
                    modifiers: egui::Modifiers::default() }]
            } else { vec![] };
            let output = ctx.run(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1080.0, 720.0))),
                events, ..Default::default()
            }, |ctx| app.status_bar(ctx, &DARK));
            let mut selected_x = None;
            for shape in output.shapes {
                if let egui::Shape::Text(text) = shape.shape {
                    if text.galley.text() == "UTF-8" {
                        encoding_pos = Some(text.pos + text.galley.size() * 0.5);
                    }
                    assert_ne!(text.galley.text(), "GBK");
                    if text.galley.text() == "选中2字" { selected_x = Some(text.pos.x); }
                }
            }
            assert!(selected_x.unwrap() > encoding_pos.unwrap().x);
        }
        assert_eq!(app.active().unwrap().encoding, recovery::Encoding::Utf8);
        assert_eq!(app.active().unwrap().draft_text.as_deref(), Some("中文"));
    }

    #[test]
    fn status_switches_between_logical_cursor_line_and_selected_characters() {
        for lang in [Lang::ZhCn, Lang::En] {
            let ctx = egui::Context::default();
            setup_fonts(&ctx);
            let mut app = LiteTextApp { lang, ..Default::default() };
            app.new_document();
            for (line, count) in [(0, 0), (999, 0), (999, 22), (7, 0)] {
                let state = &mut app.active_mut().unwrap().editor_state;
                state.cursor_line = line;
                state.selected_characters = count;
                let expected = if count == 0 { tr(lang).cursor_line.replace("{n}", &(line + 1).to_string()) }
                    else { tr(lang).selected_characters.replace("{n}", &count.to_string()) };
                let output = ctx.run(egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1080.0, 720.0))),
                    ..Default::default()
                }, |ctx| app.status_bar(ctx, &DARK));
                assert!(output.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text() == expected)));
            }
        }
    }

    #[test]
    fn hex_zoom_updates_header_and_body_fonts_without_changing_bytes_or_selection() {
        let ctx = egui::Context::default();
        let mut app = LiteTextApp::default();
        app.new_document();
        let file = app.active_mut().unwrap();
        file.draft_text = Some("abcdef0123456789".repeat(100));
        file.sync_draft();
        let bytes = file.bytes.clone();
        let mut selection = Some((2, 4));
        let mut anchor = None;
        for size in [9.0, 13.5, 32.0] {
            let output = ctx.run(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1080.0, 720.0))),
                ..Default::default()
            }, |ctx| { egui::CentralPanel::default().show(ctx, |ui| {
                hex_view(ui, file, &DARK, &mut selection, &mut anchor, &HexViewOpts {
                    font_size: size, upper: true, dim_zero: false, show_ascii: true, show_invisible: false,
                });
            }); });
            let mut header = false;
            let mut body = false;
            for shape in output.shapes {
                if let egui::Shape::Text(t) = shape.shape
                    && (t.galley.text().starts_with("Address") || t.galley.text().starts_with("00000000")) {
                        assert_eq!(t.galley.job.sections[0].format.font_id.size, size - 0.5);
                        header |= t.galley.text().starts_with("Address");
                        body |= t.galley.text().starts_with("00000000");
                }
            }
            assert!(header && body);
            assert_eq!(selection, Some((2, 4)));
            assert_eq!(file.bytes, bytes);
        }
    }

    #[test]
    fn folder_action_resolves_parent_and_does_not_close_tabs() {
        assert!(file_parent_folder(std::path::Path::new("")).is_err());
        let parent = file_parent_folder(std::path::Path::new("docs/PROGRESS.md")).unwrap();
        assert!(parent.is_absolute());
        assert!(parent.ends_with("docs"));
        let mut app = LiteTextApp::default();
        app.new_document();
        assert!(app.closing_ids(&TabAction::OpenFolder(app.active_id.unwrap())).is_empty());
        assert_eq!(app.files.len(), 1);
    }

    #[test]
    fn status_bar_hides_filename_and_path_click_copies_without_selection() {
        let ctx = egui::Context::default();
        let mut app = LiteTextApp::default();
        app.new_document();
        let file = app.active_mut().unwrap();
        file.path = PathBuf::from(if cfg!(windows) {
            r"C:\example\directory\document.txt"
        } else {
            "/example/directory/document.txt"
        });
        let displayed_path = std::path::absolute(&file.path).unwrap().display().to_string();
        file.name = "document.txt".to_owned();
        file.draft_text = Some("中文hello".to_owned());
        file.sync_draft();
        let mut click = None;
        let mut copied = false;
        for frame in 0..4 {
            let events = if frame >= 2 {
                let pos = click.expect("文件路径应已在状态栏渲染");
                vec![egui::Event::PointerMoved(pos), egui::Event::PointerButton {
                    pos, button: egui::PointerButton::Primary, pressed: frame == 2,
                    modifiers: egui::Modifiers::default(),
                }]
            } else { vec![] };
            let output = ctx.run(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1080.0, 720.0))),
                events, ..Default::default()
            }, |ctx| {
                app.status_bar(ctx, &DARK);
                egui::CentralPanel::default().show(ctx, |ui| {
                    assert!(ui.max_rect().bottom() < 700.0, "状态栏不应遮挡编辑区与横向滚动条");
                });
            });
            for shape in output.shapes {
                if let egui::Shape::Text(text) = shape.shape {
                    assert_ne!(text.galley.text(), "document.txt");
                    if text.galley.text() == displayed_path {
                        click = Some(text.pos + text.galley.size() * 0.5);
                    }
                }
            }
            copied |= output.platform_output.commands.iter().any(|cmd| matches!(cmd, egui::OutputCommand::CopyText(text) if text == &displayed_path));
        }
        assert!(copied);
        let start = ctx.data(|d| d.get_temp::<f64>(egui::Id::new(("copied_path_toast", displayed_path.as_str())))).unwrap();
        for (age, visible) in [(0.3, true), (1.2, false)] {
            let mut last = None;
            for _ in 0..2 {
                last = Some(ctx.run(egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1080.0, 720.0))),
                    time: Some(start + age), ..Default::default()
                }, |ctx| app.status_bar(ctx, &DARK)));
            }
            let bubble = last.unwrap().shapes.into_iter().find_map(|s| match s.shape {
                egui::Shape::Text(t) if t.galley.text() == "已复制文件地址" => Some(t.pos), _ => None,
            });
            assert_eq!(bubble.is_some(), visible);
            if let Some(pos) = bubble { assert!(pos.y < click.unwrap().y - 10.0); }
        }
        assert_eq!(app.active().unwrap().character_count, 7);
    }

    #[test]
    #[ignore = "manual benchmark using an external user-provided log"]
    fn benchmark_user_log_editor() {
        let path = std::env::var_os("LITETEXT_BENCH_FILE").expect("LITETEXT_BENCH_FILE");
        let original = std::fs::read(&path).unwrap();
        let mut app = LiteTextApp::default();
        let started = std::time::Instant::now();
        app.open_paths(vec![PathBuf::from(&path)]);
        eprintln!("open: {:?}", started.elapsed());
        let ctx = egui::Context::default();
        setup_fonts(&ctx);
        for frame in 0..12 {
            workspace_theme::configure(app.active_mut().unwrap(), &workspace_theme::WorkspaceTheme::default(), true);
            let start = std::time::Instant::now();
            let output = ctx.run(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1080.0, 720.0))),
                time: Some(frame as f64),
                events: match frame {
                    7 => vec![egui::Event::Text("benchmark edit".to_owned())],
                    9 => vec![egui::Event::PointerMoved(egui::pos2(200.0, 200.0)), egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point, delta: egui::vec2(-100.0, -5000.0), modifiers: egui::Modifiers::default(),
                    }],
                    _ => vec![],
                },
                ..Default::default()
            }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    draft_editor(ui, app.active_mut().unwrap(), &DARK, frame == 6, Lang::ZhCn);
                });
            });
            let _ = ctx.tessellate(output.shapes, output.pixels_per_point);
            eprintln!("frame {frame}: {:?}", start.elapsed());
        }
        assert!(app.active().unwrap().draft_text.as_ref().unwrap().contains("benchmark edit"));
        assert_eq!(std::fs::read(path).unwrap(), original, "基准测试不允许修改用户日志");
    }

    #[test]
    fn tab_bar_scrolls_horizontally_with_mouse_wheel_only_when_hovered() {
        let scroll_distance = |pointer: egui::Pos2, delta: egui::Vec2, count: usize| {
            let ctx = egui::Context::default();
            let mut app = LiteTextApp::default();
            for _ in 0..count {
                app.new_document();
            }
            let first_title = app.files[0].name.clone();
            let mut first_x = None;
            let mut last_x = None;
            for frame in 0..8 {
                let mut events = vec![egui::Event::PointerMoved(pointer)];
                if frame == 2 {
                    events.push(egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta,
                        modifiers: egui::Modifiers::default(),
                    });
                }
                let output = ctx.run(egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1080.0, 720.0))),
                    time: Some(frame as f64 / 60.0),
                    events,
                    ..Default::default()
                }, |ctx| app.top_bar(ctx, &DARK));
                for shape in output.shapes {
                    if let egui::Shape::Text(text) = shape.shape
                        && text.galley.text() == first_title
                    {
                        if frame == 1 {
                            first_x = Some(text.pos.x);
                        }
                        last_x = Some(text.pos.x);
                    }
                }
            }
            first_x.unwrap() - last_x.unwrap()
        };
        let hover = egui::pos2(100.0, 20.0);
        assert!(scroll_distance(hover, egui::vec2(0.0, -120.0), 20) > 50.0);
        assert!(scroll_distance(hover, egui::vec2(-120.0, 0.0), 20) > 50.0);
        assert_eq!(scroll_distance(egui::pos2(100.0, 200.0), egui::vec2(0.0, -120.0), 20), 0.0);
        assert_eq!(scroll_distance(egui::pos2(1050.0, 20.0), egui::vec2(0.0, -120.0), 20), 0.0);
        assert_eq!(scroll_distance(hover, egui::vec2(0.0, -120.0), 2), 0.0);
    }

    #[test]
    fn new_documents_are_independent_and_not_recent_files() {
        let mut app = LiteTextApp::default();
        app.new_document();
        let first_id = app.active_id.unwrap();
        app.active_mut().unwrap().draft_text = Some("第一份草稿\nhello".to_owned());
        app.active_mut().unwrap().sync_draft();
        app.new_document();
        let second = app.active().unwrap();
        assert_ne!(second.id, first_id);
        assert_ne!(app.files[0].name, second.name);
        assert_eq!(second.draft_text.as_deref(), Some(""));
        assert_eq!(second.lines, vec![""]);
        assert!(second.bytes.is_empty());
        assert!(second.path.as_os_str().is_empty());
        assert_eq!(app.focus_editor, Some(second.id));
        assert!(app.recent_files.is_empty());
        assert_eq!(app.files[0].draft_text.as_deref(), Some("第一份草稿\nhello"));
    }

    #[test]
    fn draft_editor_accepts_input_and_keeps_hex_bytes_in_sync() {
        let ctx = egui::Context::default();
        let mut app = LiteTextApp::default();
        app.new_document();
        for frame in 0..3 {
            let _ = ctx.run(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1080.0, 720.0))),
                events: if frame == 2 { vec![egui::Event::Text("hello\nworld\n".to_owned())] } else { vec![] },
                ..Default::default()
            }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    draft_editor(ui, app.active_mut().unwrap(), &DARK, frame == 0, Lang::ZhCn);
                });
            });
        }
        let file = app.active().unwrap();
        assert_eq!(file.draft_text.as_deref(), Some("hello\nworld\n"));
        assert_eq!(file.bytes, b"hello\nworld\n");
        assert_eq!(file.lines, vec!["hello", "world", ""]);
    }

    #[test]
    fn recent_files_open_on_hover_in_side_menu_and_accept_clicks() {
        let ctx = egui::Context::default();
        setup_fonts(&ctx);
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let mut app = LiteTextApp {
            show_menu: true,
            recent_files: vec![path.clone()],
            ..Default::default()
        };
        let anchor = egui::Rect::from_min_size(egui::pos2(1030.0, 7.0), egui::vec2(28.0, 24.0));
        let mut recent_pos = None;
        let mut file_pos = None;
        for frame in 0..7 {
            let mut events = Vec::new();
            if frame >= 2 {
                let pos = if frame < 4 { recent_pos.unwrap() } else { file_pos.unwrap() };
                events.push(egui::Event::PointerMoved(pos));
                if frame >= 5 {
                    events.push(egui::Event::PointerButton {
                        pos, button: egui::PointerButton::Primary, pressed: frame == 5,
                        modifiers: egui::Modifiers::default(),
                    });
                }
            }
            let output = ctx.run(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1080.0, 720.0))),
                events,
                ..Default::default()
            }, |ctx| app.app_menu(ctx, anchor, &DARK));
            for shape in output.shapes {
                if let egui::Shape::Text(text) = shape.shape {
                    if text.galley.text().contains(tr(app.lang).open_recent) {
                        recent_pos = Some(text.pos + egui::vec2(20.0, 8.0));
                    }
                    if text.galley.text() == "Cargo.toml" {
                        file_pos = Some(text.pos + egui::vec2(12.0, 8.0));
                    }
                }
            }
            if frame == 3 {
                assert!(app.menu_recent_open, "无需点击就应展开最近文件");
                let recent = recent_pos.unwrap();
                let file = file_pos.expect("子菜单应显示最近文件");
                assert!(file.x < recent.x - 200.0, "子菜单应出现在主菜单左侧");
                assert!((file.y - recent.y).abs() < 10.0, "子菜单应与最近文件行对齐");
            }
            if frame == 4 {
                assert!(app.menu_recent_open, "移入子菜单时应保持展开");
            }
        }
        assert!(!app.show_menu);
        assert_eq!(app.active().unwrap().path, path.canonicalize().unwrap());
    }

    #[test]
    fn app_menu_opens_below_button_and_requests_empty_new_window() {
        let ctx = egui::Context::default();
        setup_fonts(&ctx);
        let mut app = LiteTextApp { show_menu: true, ..Default::default() };
        let anchor = egui::Rect::from_min_size(egui::pos2(1030.0, 7.0), egui::vec2(28.0, 24.0));
        let mut new_pos = None;
        for frame in 0..4 {
            let events = if frame >= 2 {
                let pos = new_pos.expect("新建菜单项应已渲染");
                vec![egui::Event::PointerMoved(pos), egui::Event::PointerButton {
                    pos, button: egui::PointerButton::Primary, pressed: frame == 2,
                    modifiers: egui::Modifiers::default(),
                }]
            } else { vec![] };
            let output = ctx.run(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1080.0, 720.0))),
                events,
                ..Default::default()
            }, |ctx| {
                apply_style(ctx, &DARK);
                app.app_menu(ctx, anchor, &DARK);
            });
            if frame == 1 {
                for shape in output.shapes {
                    if let egui::Shape::Text(text) = shape.shape {
                        assert!(text.pos.y > anchor.bottom());
                        if text.galley.text() == "新窗口" {
                            new_pos = Some(text.pos + egui::vec2(8.0, 8.0));
                        }
                    }
                }
                assert!(!app.show_settings);
            }
        }
        assert!(!app.show_menu);
        assert!(app.new_window_requested);
        assert!(app.files.is_empty());
    }

    #[test]
    fn welcome_rendered_content_is_vertically_centered() {
        let ctx = egui::Context::default();
        setup_fonts(&ctx);
        let mut app = LiteTextApp::default();
        // 覆盖连续帧，避免只验证首帧、遗漏滚动区布局稳定后的偏移。
        for frame in 0..5 {
        let mut content_center_y = 0.0;
        let output = ctx.run(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1080.0, 720.0))),
            ..Default::default()
        }, |ctx| {
            apply_style(ctx, &DARK);
            app.top_bar(ctx, &DARK);
            egui::CentralPanel::default()
                .frame(egui::Frame::new().inner_margin(egui::Margin::ZERO))
                .show(ctx, |ui| {
                    content_center_y = ui.available_rect_before_wrap().center().y;
                    welcome_screen(ui, &mut app, &DARK);
                });
        });
        let mut tagline_y = None;
        let mut recent_y = None;
        for shape in output.shapes {
            if let egui::Shape::Text(text) = shape.shape {
                if text.galley.text() == tr(app.lang).tagline {
                    tagline_y = Some(text.pos.y + text.galley.size().y * 0.5);
                }
                if text.galley.text() == tr(app.lang).recent {
                    recent_y = Some(text.pos.y + text.galley.size().y * 0.5);
                }
            }
        }
        let tagline_y = tagline_y.expect("欢迎说明应实际渲染");
        let recent_y = recent_y.expect("最近打开标题应实际渲染");
        assert!((tagline_y - (content_center_y - 100.0)).abs() < 5.0, "frame {frame}");
        assert!((recent_y - (content_center_y - 122.0)).abs() < 5.0, "frame {frame}");
        }
    }

    #[test]
    fn welcome_columns_are_centered_and_inset() {
        for size in [egui::vec2(640.0, 412.0), egui::vec2(1080.0, 682.0), egui::vec2(1600.0, 900.0)] {
            let bounds = egui::Rect::from_min_size(egui::pos2(0.0, 38.0), size);
            let (left, recent) = welcome_rects(bounds);
            assert!((left.left() + recent.right() - 2.0 * bounds.center().x).abs() < 0.001);
            assert!(left.left() >= bounds.left() + 24.0);
            assert!(recent.right() <= bounds.right() - 24.0);
            assert!(recent.left() - left.right() >= 24.0);
            assert!((recent.top() - (bounds.center().y - 139.0)).abs() < 0.001);
            assert!((left.top() - (bounds.center().y - 169.0)).abs() < 0.001);
            assert!(left.top() >= bounds.top());
        }
    }

    #[test]
    fn tab_width_shares_area_with_bounds() {
        assert_eq!(tab_width_for(2, 1000.0), 160.0); // 均分超上限 → 上限
        assert_eq!(tab_width_for(10, 1072.0), 100.0); // 扣除 9 个间距后正好均分
        let width = tab_width_for(10, 1000.0);
        assert!((width * 10.0 + TAB_GAP * 9.0 - 1000.0).abs() < 0.001);
        assert_eq!(tab_width_for(20, 1000.0), 90.0); // 低于下限 → 下限（触发滚动）
    }

    #[test]
    fn tab_titles_keep_prefix_within_width() {
        let measure = |s: &str| s.chars().count() as f32;
        assert_eq!(elide_tab_title("notes.txt", 9.0, measure), "notes.txt");
        assert_eq!(elide_tab_title("project/notes.txt", 10.0, measure), "project...");
        assert_eq!(elide_tab_title("very-long-filename.txt", 8.0, measure), "very-...");
        assert_eq!(elide_tab_title("目录/中文阅读笔记.txt", 9.0, measure), "目录/中文阅...");
        assert_eq!(elide_tab_title("notes.txt", 3.0, measure), "...");
        assert_eq!(elide_tab_title("notes.txt", 2.0, measure), "");
        assert_eq!(elide_tab_title("notes.txt", 0.0, measure), "");
    }

    #[test]
    fn tab_titles_use_measured_width_instead_of_character_count() {
        let measure = |s: &str| s.chars().map(|c| if c.is_ascii() { 1.0 } else { 2.0 }).sum();
        let title = elide_tab_title("目录/中文笔记.txt", 10.0, measure);
        assert_eq!(title, "目录/中...");
        assert!(measure(&title) <= 10.0);
    }

    #[test]
    fn tab_spacing_and_drag_gap_share_coordinates() {
        let origin = 14.0;
        let width = 100.0;
        let step = width + TAB_GAP;
        let xs = layout_tab_xs(3, origin, step, None);
        assert_eq!(xs, vec![14.0, 122.0, 230.0]);
        assert_eq!(xs[1] - (xs[0] + width), TAB_GAP);
        let gap = origin + step;
        assert_eq!(layout_tab_xs(3, origin, step, Some(gap)), vec![14.0, 230.0, 338.0]);
        assert_eq!(tab_slot_for_pointer(gap + width / 2.0, origin, step, 3), 1);
    }

    #[test]
    fn hover_slot_clamps_to_bounds() {
        assert_eq!(tab_slot_for_pointer(-50.0, 0.0, 100.0, 3), 0); // 左越界
        assert_eq!(tab_slot_for_pointer(50.0, 0.0, 100.0, 3), 0); // 第 0 格
        assert_eq!(tab_slot_for_pointer(150.0, 0.0, 100.0, 3), 1); // 第 1 格
        assert_eq!(tab_slot_for_pointer(999.0, 0.0, 100.0, 3), 3); // 右越界 → 末尾之后
    }

    #[test]
    fn gap_skips_exactly_one_slot() {
        // 无间隙：紧密排列
        assert_eq!(layout_tab_xs(3, 0.0, 100.0, None), vec![0.0, 100.0, 200.0]);
        // 间隙在第 1 槽：第 0 个不动，后面顺移
        assert_eq!(
            layout_tab_xs(3, 0.0, 100.0, Some(100.0)),
            vec![0.0, 200.0, 300.0]
        );
        // 间隙在最前：全部顺移
        assert_eq!(
            layout_tab_xs(3, 0.0, 100.0, Some(0.0)),
            vec![100.0, 200.0, 300.0]
        );
        // 间隙在最后：顺序不变（末尾留空）
        assert_eq!(
            layout_tab_xs(3, 0.0, 100.0, Some(300.0)),
            vec![0.0, 100.0, 200.0]
        );
        // 动画中间态（间隙滑到半格处）：首个中心越过 gx 的槽位让位
        assert_eq!(
            layout_tab_xs(3, 0.0, 100.0, Some(60.0)),
            vec![0.0, 200.0, 300.0]
        );
    }

    #[test]
    fn insert_id_places_at_slot() {
        assert_eq!(insert_id(&[1, 2, 3], 9, 1), vec![1, 9, 2, 3]);
        assert_eq!(insert_id(&[1, 2, 3], 9, 0), vec![9, 1, 2, 3]);
        assert_eq!(insert_id(&[1, 2, 3], 9, 3), vec![1, 2, 3, 9]);
        // 越界槽位收拢
        assert_eq!(insert_id(&[1, 2, 3], 9, 99), vec![1, 2, 3, 9]);
    }

    #[test]
    fn fillets_hug_bottom_corners() {
        let rect = egui::Rect::from_min_max(egui::pos2(100.0, 6.0), egui::pos2(200.0, 38.0));
        let [(l_rect, l_circle), (r_rect, r_circle)] = tab_fillet_shapes(&rect);
        let r = TAB_FILLET_RADIUS; // 8.0
        // 左：矩形贴在标签左下角外侧，打底圆心在其左上
        assert_eq!(l_rect.min, egui::pos2(100.0 - r, 38.0 - r));
        assert_eq!(l_rect.max, egui::pos2(100.0, 38.0));
        assert_eq!(l_circle, egui::pos2(100.0 - r, 38.0 - r));
        // 右：镜像
        assert_eq!(r_rect.min, egui::pos2(200.0, 38.0 - r));
        assert_eq!(r_rect.max, egui::pos2(200.0 + r, 38.0));
        assert_eq!(r_circle, egui::pos2(200.0 + r, 38.0 - r));
    }
}

//! Lightweight Markdown workspace rendering.
//!
//! The editor deliberately keeps Markdown as plain text; this module adds
//! Typora-like presentation on top of the same draft buffer. It is not a
//! CommonMark implementation: it covers headings, paragraphs, lists, quotes,
//! fenced code, rules, simple tables and the common inline styles.
use crate::{Lang, LoadedFile, Palette};
use eframe::egui;
use egui::{Color32, FontFamily, FontId, Stroke};
use serde::{Deserialize, Serialize};
use std::ops::Range;
use std::path::{Path, PathBuf};
#[path = "markdown_table_ops.rs"]
mod table_ops;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Mode {
    #[default]
    Source,
    Live,
    Split,
}

impl Mode {
    pub(crate) fn label(self, zh: bool) -> &'static str {
        match (self, zh) {
            (Self::Source, true) => "源码",
            (Self::Source, false) => "Source",
            (Self::Live, true) => "实时渲染",
            (Self::Live, false) => "Live render",
            (Self::Split, true) => "对比视图",
            (Self::Split, false) => "Split view",
        }
    }

    const ALL: [Self; 3] = [Self::Source, Self::Live, Self::Split];
}

#[derive(Clone, Default)]
pub(crate) struct State {
    pub(crate) editing: Option<Editing>,
    /// Live tables keep their grid while only one source cell is editable.
    pub(crate) cell_editing: Option<CellEditing>,
    /// Open "insert table" size picker, kept across frames while its window is
    /// visible.
    pub(crate) table_picker: Option<TablePicker>,
    /// Heading byte offset requested by the right-hand outline.
    pub(crate) toc_target: Option<usize>,
    outline: OutlineState,
    /// Byte offset waiting for the platform's paste event after the rendered
    /// right-click menu asked for the clipboard.
    pub(crate) paste_at: Option<usize>,
    /// Shared vertical offset of the split view's two panes.
    pub(crate) split_offset: f32,
    /// Frames left before the split view may force its shared offset again
    /// after a find / go-to-line reveal. egui applies a scroll request on the
    /// following frame, so the forced offset must wait for it to land.
    pub(crate) reveal_frames: u8,
    /// Decoded local images, keyed by the Markdown source string.
    images: std::collections::HashMap<String, ImageEntry>,
}

#[derive(Clone)]
enum ImageEntry {
    Ready {
        texture: egui::TextureHandle,
        size: [usize; 2],
    },
    /// Remote image download/decode running on a worker thread.
    Loading {
        receiver: std::sync::Arc<
            std::sync::Mutex<std::sync::mpsc::Receiver<Result<DecodedImage, ()>>>,
        >,
    },
    Failed,
}

struct DecodedImage {
    rgba: Vec<u8>,
    size: [usize; 2],
}

#[derive(Clone)]
pub(crate) struct Editing {
    range: Range<usize>,
    text: String,
    focus: bool,
    fresh: bool,
    /// Whether editing started on a code block. An unfinished fence typed
    /// into a paragraph must stay visible as text instead of being parsed as
    /// a code block that swallows everything below it.
    code: bool,
    /// Character offset within `text` where the click landed. Applied once
    /// before the first frame so editing starts at the clicked position.
    cursor: Option<usize>,
}

#[derive(Clone)]
pub(crate) struct CellEditing {
    table: usize,
    row: usize,
    column: usize,
    source: Range<usize>,
    text: String,
    committed: String,
    focus: bool,
}

/// Small modal-style size picker opened from the live-render context menu.
#[derive(Clone)]
pub(crate) struct TablePicker {
    /// Document byte offset where the finished table is inserted.
    pub(crate) at: usize,
    pub(crate) rows: usize,
    pub(crate) columns: usize,
}

#[derive(Clone)]
enum InsertAction {
    Table { rows: usize, columns: usize },
    Callout(CalloutKind),
    /// Insert one empty line before or after the current source line. The
    /// line boundaries are captured when the menu opens so the command keeps
    /// working after the menu has taken focus away from the editor.
    BlankLine {
        line_start: usize,
        line_end: usize,
        before: bool,
    },
    /// Cut a whole rendered block, including one blank-line separator.
    Cut { start: usize, end: usize },
    /// Insert the clipboard after this byte offset once the platform delivers
    /// the paste event. The rendered view has no focused text box of its own.
    Paste { at: usize },
    /// Insert a formatted local timestamp at the caret of the inline editor
    /// or at the end of the right-clicked block.
    InsertTime { at: usize, stamp: String },
}

/// GitHub-style alert kinds (`> [!NOTE]` …). Their Markdown stays portable;
/// only the presentation is LiteText-specific.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CalloutKind {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

impl CalloutKind {
    const ALL: [Self; 5] = [
        Self::Note,
        Self::Tip,
        Self::Important,
        Self::Warning,
        Self::Caution,
    ];

    fn marker(self) -> &'static str {
        match self {
            Self::Note => "NOTE",
            Self::Tip => "TIP",
            Self::Important => "IMPORTANT",
            Self::Warning => "WARNING",
            Self::Caution => "CAUTION",
        }
    }

    fn label(self, zh: bool) -> &'static str {
        match (self, zh) {
            (Self::Note, true) => "提醒",
            (Self::Note, false) => "Note",
            (Self::Tip, true) => "建议",
            (Self::Tip, false) => "Tip",
            (Self::Important, true) => "重要",
            (Self::Important, false) => "Important",
            (Self::Warning, true) => "警告",
            (Self::Warning, false) => "Warning",
            (Self::Caution, true) => "注意",
            (Self::Caution, false) => "Caution",
        }
    }

    fn color(self, pal: &Palette) -> Color32 {
        match self {
            Self::Note => pal.accent,
            Self::Tip => pal.recovery_backed_up,
            Self::Important => pal.accent_hover,
            Self::Warning => pal.recovery_pending,
            Self::Caution => pal.danger,
        }
    }

    fn template(self, zh: bool) -> String {
        let placeholder = if zh {
            format!("在这里输入{}内容", self.label(true))
        } else {
            format!("Write {} content here", self.label(false).to_lowercase())
        };
        format!("> [!{}]\n> {placeholder}", self.marker())
    }
}

fn callout_kind(trimmed: &str) -> Option<CalloutKind> {
    let rest = trimmed.strip_prefix('>')?.trim_start();
    let rest = rest.strip_prefix("[!")?;
    let end = rest.find(']')?;
    let marker = rest[..end].trim();
    CalloutKind::ALL
        .into_iter()
        .find(|kind| kind.marker().eq_ignore_ascii_case(marker))
}

/// Markdown is only offered when the effective workspace language is Markdown.
pub(crate) fn is_markdown(file: &LoadedFile) -> bool {
    if file.draft_text.is_none() {
        return false;
    }
    // A user-picked language always wins. Otherwise the extension is enough;
    // going through `effective_language` would load the whole syntect syntax
    // set just to answer a boolean, which showed up as a first-open hitch.
    if let Some(language) = &file.language_override {
        return language.to_ascii_lowercase().contains("markdown");
    }
    const MARKDOWN_EXTENSIONS: &[&str] = &["md", "mdown", "markdown", "markdn"];
    let path = file.source_path.as_deref().unwrap_or(&file.path);
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            MARKDOWN_EXTENSIONS
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
        })
}

/// A compact icon-only status-bar control. Returns the chosen mode.
pub(crate) fn picker(
    ui: &mut egui::Ui,
    file: &LoadedFile,
    lang: Lang,
    pal: &Palette,
    enabled: bool,
) -> Option<Mode> {
    let zh = lang == Lang::ZhCn;
    let id = egui::Id::new(("markdown_mode", file.id));
    let (rect, _) = ui.allocate_exact_size(egui::vec2(24.0, 14.0), egui::Sense::hover());
    let hit = rect.expand2(egui::vec2(3.0, 5.0)).intersect(ui.clip_rect());
    let response = ui
        .interact(hit, id, egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let response = response.on_hover_text(if zh {
        format!(
            "Markdown 视图：{}（点击切换）",
            file.markdown_mode.label(true)
        )
    } else {
        format!(
            "Markdown view: {} (click to change)",
            file.markdown_mode.label(false)
        )
    });
    ui.ctx()
        .data_mut(|data| data.insert_temp(id.with("rect"), rect));
    if !enabled && ui.memory(|memory| memory.is_popup_open(id)) {
        ui.memory_mut(|memory| memory.close_popup());
    }
    let mut open = enabled && ui.memory(|memory| memory.is_popup_open(id));
    if enabled {
        // A permanent accent tint marks the Markdown view control as the
        // active workspace feature; hover/open just strengthens it.
        let tint = if response.hovered() || open { 0.34 } else { 0.18 };
        ui.painter().rect_filled(
            rect.expand2(egui::vec2(2.0, 3.0)),
            4.0,
            crate::opaque_tint(pal.bg_panel, pal.accent, tint),
        );
    }
    paint_icon(
        ui.painter(),
        egui::Rect::from_center_size(rect.center(), egui::vec2(15.0, 13.0)),
        file.markdown_mode,
        // The bottom bar is muted by design; the Markdown affordance is the
        // one control that should read as an active theme-colored feature.
        if enabled { pal.accent } else { pal.text_faint },
    );
    if enabled && response.clicked() {
        open = !open;
        ui.memory_mut(|memory| {
            if open {
                memory.open_popup(id)
            } else {
                memory.close_popup()
            }
        });
    }
    if !open {
        return None;
    }
    let mut picked = None;
    let popup = egui::Area::new(id.with("popup"))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::LEFT_BOTTOM)
        .fixed_pos(rect.left_top() - egui::vec2(0.0, 8.0))
        // Opaque from the first visible frame; the document sits directly behind.
        .fade_in(false)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style())
                .fill(pal.bg_panel)
                .show(ui, |ui| {
                    // Fit the widest label instead of a fixed 168 px: the
                    // popup sits over the document and empty panel space
                    // looks like a stray dark block.
                    let font = egui::FontId::proportional(13.0);
                    let label_w = Mode::ALL
                        .iter()
                        .map(|mode| {
                            ui.painter()
                                .layout_no_wrap(mode.label(zh).to_owned(), font.clone(), pal.text_main)
                                .size()
                                .x
                        })
                        .fold(0.0_f32, f32::max);
                    let width = (34.0 + label_w + 14.0).clamp(96.0, 248.0);
                    ui.set_width(width);
                    for mode in Mode::ALL {
                        let (row, response) =
                            ui.allocate_exact_size(egui::vec2(width, 28.0), egui::Sense::click());
                        let selected = file.markdown_mode == mode;
                        ui.ctx()
                            .data_mut(|data| data.insert_temp(id.with(mode.label(false)), row));
                        if selected || response.hovered() {
                            ui.painter().rect_filled(
                                row,
                                4.0,
                                if selected { pal.bg_elevated } else { pal.bg_inset },
                            );
                        }
                        paint_icon(
                            ui.painter(),
                            egui::Rect::from_center_size(
                                egui::pos2(row.left() + 16.0, row.center().y),
                                egui::vec2(16.0, 14.0),
                            ),
                            mode,
                            if selected { pal.accent } else { pal.text_main },
                        );
                        ui.painter().text(
                            egui::pos2(row.left() + 34.0, row.center().y),
                            egui::Align2::LEFT_CENTER,
                            mode.label(zh),
                            font.clone(),
                            if selected {
                                pal.text_main
                            } else {
                                pal.text_faint
                            },
                        );
                        if response.clicked() {
                            picked = Some(mode);
                        }
                    }
                });
        });
    ui.ctx()
        .data_mut(|data| data.insert_temp(id.with("popup_rect"), popup.response.rect));
    if picked.is_some()
        || ui.input(|input| {
            input.key_pressed(egui::Key::Escape)
                || input.pointer.any_click()
                    && input
                        .pointer
                        .interact_pos()
                        .is_some_and(|pos| !hit.contains(pos) && !popup.response.rect.contains(pos))
        })
    {
        ui.memory_mut(|memory| memory.close_popup());
    }
    picked
}

fn paint_icon(painter: &egui::Painter, rect: egui::Rect, mode: Mode, color: Color32) {
    let stroke = Stroke::new(1.4_f32, color);
    match mode {
        Mode::Source => {
            // A plain text sheet with left-aligned lines.
            for (index, width) in [0.62, 0.48, 0.55].into_iter().enumerate() {
                let y = rect.top() + 3.0 + index as f32 * 3.5;
                painter.line_segment(
                    [
                        egui::pos2(rect.left() + 2.0, y),
                        egui::pos2(rect.left() + 2.0 + rect.width() * width, y),
                    ],
                    stroke,
                );
            }
        }
        Mode::Live => {
            // A rendered sheet: a heading bar plus body lines.
            painter.rect_stroke(
                rect,
                2.0,
                Stroke::new(1.2_f32, color),
                egui::StrokeKind::Inside,
            );
            painter.rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(rect.left() + 2.5, rect.top() + 2.5),
                    egui::vec2(rect.width() * 0.55, 3.0),
                ),
                1.0,
                color,
            );
            for (index, width) in [0.72, 0.55].into_iter().enumerate() {
                let y = rect.top() + 8.0 + index as f32 * 2.7;
                painter.line_segment(
                    [
                        egui::pos2(rect.left() + 2.5, y),
                        egui::pos2(rect.left() + 2.5 + rect.width() * width, y),
                    ],
                    Stroke::new(1.1_f32, color),
                );
            }
        }
        Mode::Split => {
            // Source lines on the left, a filled render on the right.
            painter.rect_stroke(
                rect,
                2.0,
                Stroke::new(1.2_f32, color),
                egui::StrokeKind::Inside,
            );
            let divider = rect.center().x;
            painter.line_segment(
                [
                    egui::pos2(divider, rect.top() + 1.0),
                    egui::pos2(divider, rect.bottom() - 1.0),
                ],
                Stroke::new(1.0_f32, color),
            );
            for index in 0..3 {
                let y = rect.top() + 3.5 + index as f32 * 2.7;
                painter.line_segment(
                    [
                        egui::pos2(rect.left() + 2.5, y),
                        egui::pos2(divider - 1.5, y),
                    ],
                    Stroke::new(1.0_f32, color),
                );
            }
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(divider + 2.0, rect.top() + 3.0),
                    egui::pos2(rect.right() - 2.5, rect.top() + 6.0),
                ),
                1.0,
                color,
            );
            painter.line_segment(
                [
                    egui::pos2(divider + 2.0, rect.top() + 8.5),
                    egui::pos2(rect.right() - 3.0, rect.top() + 8.5),
                ],
                Stroke::new(1.0_f32, color),
            );
        }
    }
}

/// Rendered, click-to-edit Markdown. Returns true when the draft changed.
/// Character offset (egui cursor units) to a byte offset in `text`.
pub(crate) fn char_to_byte(text: &str, char_offset: usize) -> usize {
    text.char_indices()
        .nth(char_offset)
        .map_or(text.len(), |(index, _)| index)
}

/// Index of the block that owns `byte`, falling back to the last block that
/// starts before it (for matches exactly on a block boundary).
fn block_at(blocks: &[Block], byte: usize) -> Option<usize> {
    blocks
        .iter()
        .position(|block| block.range.start <= byte && byte < block.range.end)
        .or_else(|| {
            blocks
                .iter()
                .rposition(|block| block.range.start <= byte)
        })
}

/// Vertical band of the line that contains `byte` inside an already rendered
/// block. Centering a whole block is not enough: a match deep inside a long
/// code block or table would stay far outside the viewport.
fn match_band(rect: egui::Rect, text: &str, block: &Block, byte: usize) -> egui::Rect {
    let lines = block.line_count.max(1) as f32;
    let line = text[block.range.start..byte.min(block.range.end)]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count() as f32;
    let center = rect.top() + rect.height() * ((line + 0.5) / lines).clamp(0.0, 1.0);
    let band = (rect.height() / lines).max(10.0);
    egui::Rect::from_min_max(
        egui::pos2(rect.left(), (center - band * 0.5).max(rect.top())),
        egui::pos2(rect.right(), (center + band * 0.5).min(rect.bottom())),
    )
}

/// Replace fence markers inside an edited paragraph with same-length plain
/// characters so block discovery keeps the following lines visible. The
/// returned copy is never rendered or saved; only its block ranges are used.
fn mask_editing_fences(text: &str, range: &Range<usize>) -> String {
    let start = range.start.min(text.len());
    let end = range.end.min(text.len());
    let mut masked = text.to_owned();
    let mut offset = start;
    while offset < end {
        let line_end = text[offset..]
            .find('\n')
            .map_or(text.len(), |index| offset + index)
            .min(end);
        let line = &text[offset..line_end];
        let trimmed = line.trim_start();
        let lead = line.len() - trimmed.len();
        if lead <= 3 && (trimmed.starts_with("```") || trimmed.starts_with("~~~")) {
            // A same-length blank line both keeps byte ranges valid and gives
            // the parser a hard block boundary: the following paragraphs stay
            // visible instead of becoming the body of an unfinished fence.
            masked.replace_range(offset..line_end, &" ".repeat(line.len()));
        }
        if line_end >= end {
            break;
        }
        offset = line_end + 1;
    }
    masked
}

pub(crate) fn live_editor(
    ui: &mut egui::Ui,
    file: &mut LoadedFile,
    pal: &Palette,
    _focus: bool,
    lang: Lang,
) -> bool {
    let Some(mut text) = file.draft_text.take() else {
        return false;
    };
    // A paste requested from the rendered right-click menu has no focused
    // text box to land in: consume the platform's paste event here and insert
    // it as a new block after the one that opened the menu.
    let mut paste_at = file.markdown.paste_at.take();
    let mut paste_changed = false;
    if let Some(at) = paste_at {
        let pasted = ui.input_mut(|input| {
            let mut pasted = None;
            input.events.retain(|event| {
                if pasted.is_none()
                    && let egui::Event::Paste(value) = event
                {
                    pasted = Some(value.clone());
                    false
                } else {
                    true
                }
            });
            pasted
        });
        if let Some(pasted) = pasted {
            let pasted = pasted.replace("\r\n", "\n");
            let pasted = pasted.trim_end_matches('\n');
            if !pasted.is_empty() {
                let at = at.min(text.len());
                insert_block(&mut text, at, pasted);
                paste_changed = true;
            }
            paste_at = None;
        } else {
            paste_at = Some(at);
        }
    }
    let file_id = file.id;
    let body = body_size(file);
    let avail = ui.available_width().max(240.0);
    let mut editing = file.markdown.editing.take();
    let mut cell_editing = file.markdown.cell_editing.take();
    // External undo/source edits are authoritative; never replay a stale cell.
    if cell_editing.as_ref().is_some_and(|edit| {
        text.get(edit.source.clone()) != Some(edit.committed.as_str())
    }) {
        cell_editing = None;
    }
    // While a paragraph is edited, an unfinished fence must not be parsed as
    // a code block: that would hide every following block until the user
    // types the space that completes it. Mask the markers in a same-length
    // copy used only for block discovery, so all block byte ranges still
    // address the real buffer.
    let masked = editing
        .as_ref()
        .filter(|edit| !edit.code)
        .map(|edit| mask_editing_fences(&text, &edit.range));
    let blocks = parse(masked.as_deref().unwrap_or(&text));
    let headings: Vec<(usize, u8, String)> = blocks
        .iter()
        .filter_map(|block| match block.kind {
            Kind::Heading(level) => Some((
                block.range.start,
                level,
                heading_text(&text[block.range.clone()]),
            )),
            _ => None,
        })
        .collect();
    // The outline lives in the right margin, so wide windows gain navigation
    // while the reading column stays where it was.
    let toc_visible = !headings.is_empty() && avail >= 1040.0;
    let toc_width = if toc_visible { 200.0 } else { 0.0 };
    let toc_gap = if toc_visible { 16.0 } else { 0.0 };
    let body_avail = (avail - toc_width - toc_gap).max(240.0);
    let content = (body_avail - 48.0).clamp(240.0, 860.0);
    let left = ((body_avail - content) * 0.5).max(18.0);
    let base = image_base(file);
    ensure_images(
        ui.ctx(),
        &mut file.markdown.images,
        &base,
        &blocks,
    );
    let images = &file.markdown.images;
    let mut table_picker = file.markdown.table_picker.take();
    let mut lang_change: Option<(usize, String)> = None;
    let mut toc_target = file.markdown.toc_target.take();
    let mut outline = file.markdown.outline.clone();
    let mut insert_action: Option<(usize, InsertAction)> = None;
    // Set when a typed fence opener was completed; the caret moves into the
    // freshly created code block after the document text is replaced.
    let mut fence_focus: Option<(usize, String, usize)> = None;
    // Find / go-to-line targets are expressed in source character offsets.
    // The rendered view consumes them; there is no TextEdit to do it for us.
    let jump = file
        .editor_state
        .pending_selection
        .take()
        .map(|range| char_to_byte(&text, range.start.min(range.end)));
    let target = jump.and_then(|byte| block_at(&blocks, byte));
    let highlight = file
        .editor_state
        .tools
        .find_open
        .then(|| file.editor_state.tools.current.clone())
        .flatten()
        .filter(|range| !range.is_empty());
    let mut pending: Option<(Range<usize>, String)> = None;
    let mut clicked: Option<(Range<usize>, String, usize, usize, bool)> = None;
    let mut changed = false;
    let mut cursor_line = file.editor_state.cursor_line;
    let mut selected = file.editor_state.selected_characters;
    let scroll_id = egui::Id::new(("markdown_live_scroll", file_id));

    let panel_height = ui.available_height();
    let mut toc_click = None;
    let mut heading_positions = Vec::with_capacity(headings.len());
    let live_scroll = ui
        .horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let scroll = ui
                .allocate_ui_with_layout(
                    egui::vec2(body_avail, panel_height),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt(scroll_id)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
            ui.add_space(18.0);
            ui.horizontal(|ui| {
                ui.add_space(left);
                ui.vertical(|ui| {
                    ui.set_width(content);
                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                    for (index, block) in blocks.iter().enumerate() {
                        let raw = &text[block.range.clone()];
                        // While editing, the text box owns its whole byte range.
                        // Reparsing may split or merge that range after Enter or
                        // deletion; render the box once at the containing block
                        // and skip the parsed blocks that begin inside it.
                        let editing_range = editing.as_ref().map(|edit| edit.range.clone());
                        let is_editing = editing_range.as_ref().is_some_and(|range| {
                            block.range.start == range.start
                                || block.range.start <= range.start
                                    && range.start < block.range.end
                        });
                        let inside_edit = editing_range
                            .as_ref()
                            .is_some_and(|range| block.range.start > range.start && block.range.start < range.end);
                        if inside_edit {
                            continue;
                        }
                        if matches!(block.kind, Kind::Table) && !is_editing {
                            let response = live_table(ui, raw, block.range.start, content, body, pal,
                                file_id, lang, highlight.as_ref().and_then(|range| block_highlight(block, raw, range)).as_ref(),
                                &mut cell_editing, &mut pending);
                            if cell_editing.is_some() { editing = None; }
                            if pending.is_some() { changed = true; }
                            #[cfg(test)]
                            ui.ctx().data_mut(|data| data.insert_temp(
                                egui::Id::new(("markdown_block", file_id, block.range.start)), response.rect));
                            if let Some(byte) = jump.filter(|_| target == Some(index)) {
                                ui.scroll_to_rect_animation(match_band(response.rect, &text, block, byte),
                                    Some(egui::Align::Center), egui::style::ScrollAnimation::none());
                            }
                            response.context_menu(|menu| insert_context_menu(menu, lang, block.range.end,
                                (block.range.start, block.range.end), &mut insert_action, &mut table_picker, Some(raw)));
                            continue;
                        }
                        if is_editing {
                            let edit = editing.as_mut().expect("checked above");
                            let edit_id = egui::Id::new((
                                "markdown_edit",
                                file_id,
                                edit.range.start,
                            ));
                            let code = edit.code;
                            let code_body = code.then(|| CodeBodyEdit::new(&edit.text));
                            let mut body_text = code_body.as_ref().map(|body| body.text.clone());
                            let language = fence_info_raw(edit.text.lines().next().unwrap_or(""));
                            // Place the caret at the clicked character before
                            // the first frame; the state is read by TextEdit.
                            if edit.focus
                                && let Some(cursor) = edit.cursor.take()
                            {
                                let mut state =
                                    egui::TextEdit::load_state(ui.ctx(), edit_id).unwrap_or_default();
                                state.cursor.set_char_range(Some(egui::text::CCursorRange::one(
                                    egui::text::CCursor::new(code_body.as_ref().map_or(cursor, |body| body.body_cursor(&edit.text, cursor))),
                                )));
                                state.store(ui.ctx(), edit_id);
                            }
                            // Continue a list or quote on Enter. Only consume the key when the
                            // current line actually has a prefix to continue, so ordinary
                            // paragraphs keep the default TextEdit behaviour.
                            if !code && ui.memory(|memory| memory.has_focus(edit_id))
                                && !ui.input(|input| {
                                    input.events.iter().any(|event| {
                                        matches!(event, egui::Event::Ime(_))
                                    })
                                })
                            {
                                let cursor_chars = egui::TextEdit::load_state(ui.ctx(), edit_id)
                                    .and_then(|state| state.cursor.char_range())
                                    .map_or(edit.text.chars().count(), |range| {
                                        range.primary.index
                                    });
                                let cursor_byte = edit
                                    .text
                                    .char_indices()
                                    .nth(cursor_chars)
                                    .map_or(edit.text.len(), |(index, _)| index);
                                let line_start =
                                    edit.text[..cursor_byte].rfind('\n').map_or(0, |index| index + 1);
                                let line_end = edit.text[cursor_byte..]
                                    .find('\n')
                                    .map_or(edit.text.len(), |index| cursor_byte + index);
                                let action =
                                    editing_enter_action(&edit.text[line_start..line_end]);
                                if let Some((insertion, exit)) = action
                                    && ui.input_mut(|input| {
                                        input.consume_key(
                                            egui::Modifiers::NONE,
                                            egui::Key::Enter,
                                        )
                                    })
                                {
                                    let cursor_byte = if exit {
                                        edit.text.replace_range(line_start..line_end, "");
                                        line_start
                                    } else {
                                        edit.text.insert_str(cursor_byte, &insertion);
                                        cursor_byte + insertion.len()
                                    };
                                    let cursor_chars =
                                        edit.text[..cursor_byte].chars().count();
                                    let mut state = egui::TextEdit::load_state(ui.ctx(), edit_id)
                                        .unwrap_or_default();
                                    state.cursor.set_char_range(Some(
                                        egui::text::CCursorRange::one(
                                            egui::text::CCursor::new(cursor_chars),
                                        ),
                                    ));
                                    state.store(ui.ctx(), edit_id);
                                    pending = Some((edit.range.clone(), edit.text.clone()));
                                    changed = true;
                                }
                            }
                            let mut layouter = |ui: &egui::Ui, text: &str, width: f32| {
                                let job = if matches!(block.kind, Kind::Table) {
                                    let layout = table_layout(ui, text, body, pal, None);
                                    let mut job = plain_job(text, FontId::proportional(body), pal.text_main, f32::INFINITY);
                                    let height = (layout.height() / line_ranges(text).len().max(1) as f32).max(body * BODY_LINE_HEIGHT);
                                    for section in &mut job.sections { section.format.line_height = Some(height); }
                                    job
                                } else if code {
                                    code_body_edit_job(text, &language, body, width, pal)
                                } else { editing_job_for(
                                    text,
                                    &FontId::proportional(body),
                                    pal.text_main,
                                    pal,
                                    width,
                                    false,
                                ) };
                                #[cfg(test)]
                                ui.ctx().data_mut(|data| {
                                    data.insert_temp(
                                        egui::Id::new(("markdown_edit_job_text", file_id)),
                                        job.text.clone(),
                                    )
                                });
                                ui.fonts(|fonts| fonts.layout_job(job))
                            };
                            let (top, bottom) = editing_space(&block.kind, body);
                            ui.add_space(top);
                            let heading = matches!(block.kind, Kind::Heading(_));
                            let boxed = matches!(
                                block.kind,
                                Kind::Quote | Kind::Callout(_) | Kind::Code(_) | Kind::Table
                            );
                            let side = editing_inset(&block.kind);
                            // A code block keeps the rendered box geometry: the
                            // grey background starts at the block top and ends
                            // 12 px below the last code row, so entering the
                            // editor does not change the box height.
                            let (frame_top, frame_bottom) = if code { (6, 12) } else { (0, 0) };
                            // `TextEdit` enforces at least one row of its font's
                            // row height. Match the code body's monospace row so
                            // a single-line block keeps exactly the rendered box
                            // height instead of growing by the proportional row.
                            let edit_font = if code {
                                FontId::monospace((body - 1.0).max(9.0))
                            } else {
                                FontId::proportional(body)
                            };
                            crate::undo_settings::configure(ui.ctx(), edit_id, body_text.as_deref().unwrap_or(&edit.text));
                            let (output, frame_rect) = if boxed {
                                let framed = egui::Frame::new()
                                    .fill(pal.bg_inset)
                                    .corner_radius(6.0)
                                    .inner_margin(egui::Margin {
                                        left: side,
                                        right: side,
                                        top: frame_top,
                                        bottom: frame_bottom,
                                    })
                                    .show(ui, |ui| {
                                        let table = matches!(block.kind, Kind::Table);
                                        if table {
                                            ui.spacing_mut().scroll = egui::style::ScrollStyle::floating();
                                            let height = table_layout(ui, &edit.text, body, pal, None).height();
                                            return egui::ScrollArea::horizontal()
                                                .id_salt(("markdown_table_edit_scroll", file_id, block.range.start))
                                                .auto_shrink([false, true])
                                                .show(ui, |ui| {
                                                    ui.set_min_height(height);
                                                    egui::TextEdit::multiline(&mut edit.text)
                                                        .id(edit_id).font(edit_font).text_color(pal.text_main)
                                                        .frame(false).margin(egui::Margin::ZERO)
                                                        .desired_width((content - 2.0 * side as f32).max(24.0))
                                                        .desired_rows(1).layouter(&mut layouter).show(ui)
                                                }).inner;
                                        }
                                        egui::TextEdit::multiline(body_text.as_mut().unwrap_or(&mut edit.text))
                                            .id(edit_id)
                                            .font(edit_font)
                                            .text_color(pal.text_main)
                                            .frame(false)
                                            .margin(egui::Margin::ZERO)
                                            .desired_width((content - 2.0 * side as f32).max(24.0))
                                            .desired_rows(1)
                                            .horizontal_align(egui::Align::LEFT)
                                            .layouter(&mut layouter)
                                            .show(ui)
                                    });
                                (framed.inner, Some(framed.response.rect))
                            } else if let Kind::List { ordered, marker } = &block.kind {
                                let indent = editing_inset(&block.kind) as f32;
                                let output = ui
                                    .horizontal(|ui| {
                                        ui.add_space(indent);
                                        egui::TextEdit::multiline(&mut edit.text)
                                            .id(edit_id)
                                            .font(FontId::proportional(body))
                                            .text_color(pal.text_main)
                                            .frame(false)
                                            .margin(egui::Margin::ZERO)
                                            .desired_width((content - indent).max(24.0))
                                            .desired_rows(1)
                                            .horizontal_align(egui::Align::LEFT)
                                            .layouter(&mut layouter)
                                            .show(ui)
                                    })
                                    .inner;
                                paint_list_marker(ui, &output, *ordered, marker, body, pal);
                                (output, None)
                            } else {
                                let output = egui::TextEdit::multiline(&mut edit.text)
                                    .id(edit_id)
                                    .font(FontId::proportional(body))
                                    .text_color(pal.text_main)
                                    .frame(false)
                                    .margin(egui::Margin::ZERO)
                                    .desired_width(content)
                                    .desired_rows(1)
                                    .horizontal_align(if heading {
                                        egui::Align::Center
                                    } else {
                                        egui::Align::LEFT
                                    })
                                    .layouter(&mut layouter)
                                    .show(ui);
                                (output, None)
                            };
                            if let (Some(body), Some(edited)) = (&code_body, &body_text) {
                                body.commit(&mut edit.text, edited);
                            }
                            #[cfg(not(test))]
                            let _ = frame_rect;
                            if code {
                                #[cfg(test)]
                                if let Some(rect) = frame_rect {
                                    ui.ctx().data_mut(|data| {
                                        data.insert_temp(
                                            egui::Id::new((
                                                "markdown_code_edit_box",
                                                file_id,
                                                block.range.start,
                                            )),
                                            rect,
                                        )
                                    });
                                }
                                let row = code_language_row(
                                    ui,
                                    content,
                                    file_id,
                                    block.range.start,
                                    &fence_info_raw(edit.text.lines().next().unwrap_or("")),
                                    body,
                                    pal,
                                    true,
                                );
                                if row.changed.is_some() {
                                    lang_change = row.changed;
                                }
                            }
                            let response = output.response;
                            if matches!(block.kind, Kind::Heading(_)) {
                                heading_positions.push((block.range.start, response.rect.top()));
                            }
                            if toc_target == Some(block.range.start) {
                                ui.scroll_to_rect_animation(
                                    response.rect,
                                    Some(egui::Align::TOP),
                                    egui::style::ScrollAnimation::none(),
                                );
                                toc_target = None;
                            }
                            #[cfg(test)]
                            ui.ctx().data_mut(|data| {
                                data.insert_temp(
                                    egui::Id::new(("markdown_edit_rect", file_id)),
                                    response.rect,
                                )
                            });
                            let edit_cursor_chars = output.state.cursor.char_range()
                                .map_or(0, |range| range.primary.index);
                            let edit_cursor = if code {
                                CodeBodyEdit::new(&edit.text).raw_byte(&edit.text, edit_cursor_chars)
                            } else {
                                char_to_byte(&edit.text, edit_cursor_chars)
                            };
                            let edit_at = edit.range.start + edit_cursor;
                            // Insert relative to the whole block, not the
                            // source line under the caret: a code block stays
                            // one element even when the caret is inside it.
                            response.context_menu(|menu| {
                                insert_context_menu(
                                    menu,
                                    lang,
                                    edit_at,
                                    (block.range.start, block.range.end),
                                    &mut insert_action,
                                    &mut table_picker,
                                    None,
                                );
                            });
                            if let Some(byte) = jump.filter(|_| target == Some(index)) {
                                ui.scroll_to_rect_animation(
                                    match_band(response.rect, &text, block, byte),
                                    Some(egui::Align::Center),
                                    egui::style::ScrollAnimation::none(),
                                );
                                cursor_line = block.line_start
                                    + text[block.range.start..byte.min(block.range.end)]
                                        .bytes()
                                        .filter(|byte| *byte == b'\n')
                                        .count();
                            }
                            if edit.focus {
                                response.request_focus();
                                edit.focus = false;
                            }
                            // Ignore the focus report from the request frame
                            // itself; only an actual later focus loss ends editing.
                            let fresh = edit.fresh;
                            edit.fresh = false;
                            if let Some(range) = output.state.cursor.char_range() {
                                // CCursor indices are character offsets, not bytes.
                                let byte = edit_cursor;
                                cursor_line = block.line_start
                                    + edit.text[..byte].bytes().filter(|byte| *byte == b'\n').count();
                                selected = range.primary.index.abs_diff(range.secondary.index);
                            }
                            if response.changed() {
                                let shortcut = if code {
                                    None
                                } else {
                                    fence_shortcut(&edit.text, edit_cursor)
                                };
                                if let Some(shortcut) = shortcut {
                                    pending = Some((edit.range.clone(), shortcut.replacement));
                                    fence_focus = Some((
                                        edit.range.start + shortcut.code_at,
                                        shortcut.code_text,
                                        shortcut.cursor,
                                    ));
                                } else {
                                    pending = Some((edit.range.clone(), edit.text.clone()));
                                }
                                changed = true;
                            }
                            // A right-click menu temporarily takes focus away
                            // from the editor. Keep the block in edit mode so
                            // the menu's insertion point (the caret) stays
                            // valid; an actual click elsewhere still ends it.
                            if !fresh
                                && response.lost_focus()
                                && !response.context_menu_opened()
                                && !ui.memory(|memory| {
                                    memory.has_focus(egui::Id::new((
                                        "markdown_code_lang",
                                        file_id,
                                        block.range.start,
                                    )))
                                })
                                || ui.input(|input| input.key_pressed(egui::Key::Escape))
                            {
                                editing = None;
                            }
                            ui.add_space(bottom);
                        } else {
                            let (response, lang_row) =
                                render_block(
                                    ui,
                                    block,
                                    raw,
                                    content,
                                    pal,
                                    body,
                                    images,
                                    false,
                                    highlight.clone(),
                                    lang,
                                    true,
                                    file_id,
                                );
                            if lang_row.changed.is_some() {
                                lang_change = lang_row.changed;
                            }
                            if matches!(block.kind, Kind::Heading(_)) {
                                heading_positions.push((block.range.start, response.rect.top()));
                            }
                            if toc_target == Some(block.range.start) {
                                ui.scroll_to_rect_animation(
                                    response.rect,
                                    Some(egui::Align::TOP),
                                    egui::style::ScrollAnimation::none(),
                                );
                                toc_target = None;
                            }
                            if let Some(byte) = jump.filter(|_| target == Some(index)) {
                                // Reveal the matched line itself, not just the
                                // block that contains it.
                                ui.scroll_to_rect_animation(
                                    match_band(response.rect, &text, block, byte),
                                    Some(egui::Align::Center),
                                    egui::style::ScrollAnimation::none(),
                                );
                                cursor_line = block.line_start
                                    + text[block.range.start..byte.min(block.range.end)]
                                        .bytes()
                                        .filter(|byte| *byte == b'\n')
                                        .count();
                            }
                            #[cfg(test)]
                            ui.ctx().data_mut(|data| {
                                data.insert_temp(
                                    egui::Id::new(("markdown_block", file_id, block.range.start)),
                                    response.rect,
                                )
                            });
                            if response.clicked() && !lang_row.used {
                                let pos = response
                                    .interact_pointer_pos()
                                    .unwrap_or_else(|| response.rect.center());
                                let cursor = editing_cursor_from_click(
                                    ui,
                                    raw,
                                    pos - response.rect.min + egui::vec2(if matches!(block.kind, Kind::Table) {
                                        ui.ctx().data(|data| data.get_temp::<f32>(egui::Id::new(("markdown_table_offset", file_id, block.range.start)))).unwrap_or(0.0)
                                    } else { 0.0 }, 0.0),
                                    &block.kind,
                                    content,
                                    body,
                                    pal,
                                );
                                clicked = Some((
                                    block.range.clone(),
                                    raw.to_owned(),
                                    block.line_start,
                                    cursor,
                                    matches!(block.kind, Kind::Code(_)),
                                ));
                            }
                            response.context_menu(|menu| {
                                insert_context_menu(
                                    menu,
                                    lang,
                                    block.range.end,
                                    (block.range.start, block.range.end),
                                    &mut insert_action,
                                    &mut table_picker,
                                    Some(raw),
                                );
                            });
                        }
                    }
                    ui.add_space(40.0);
                });
            });
        })
    },
)
.inner;
            if toc_visible {
                ui.add_space(toc_gap);
                toc_click = ui
                    .allocate_ui_with_layout(
                        egui::vec2(toc_width, panel_height),
                        egui::Layout::top_down(egui::Align::LEFT),
                        |ui| {
                            let editing_heading = editing.as_ref().and_then(|edit| {
                                headings.iter().find(|(at, _, _)| *at == edit.range.start)
                                    .filter(|(at, _, _)| ui.memory(|m| m.has_focus(
                                        egui::Id::new(("markdown_edit", file_id, *at)))))
                                    .map(|(at, _, _)| *at)
                            });
                            let at_bottom = scroll.state.offset.y > 0.0
                                && scroll.state.offset.y + scroll.inner_rect.height()
                                    >= scroll.content_size.y - 1.0;
                            let manual_scroll = ui.input(|input| {
                                let over_body = input.pointer.hover_pos().is_some_and(|pos| scroll.inner_rect.contains(pos));
                                over_body && (input.raw_scroll_delta.y != 0.0 || input.pointer.primary_down())
                                    || input.key_pressed(egui::Key::PageDown)
                                    || input.key_pressed(egui::Key::PageUp)
                            });
                            let active = outline.update(
                                &heading_positions, scroll.inner_rect, at_bottom,
                                editing_heading, manual_scroll,
                            );
                            heading_outline(ui, &headings, active, pal, body)
                        },
                    )
                    .inner;
            }
            scroll
        })
        .inner;
    if let Some(at) = toc_click {
        toc_target = Some(at);
        outline.clicked = Some(at);
        outline.active = Some(at);
        ui.ctx().request_repaint();
    }
    #[cfg(test)]
    ui.ctx().data_mut(|data| {
        data.insert_temp(
            egui::Id::new(("markdown_live_scroll_y", file_id)),
            live_scroll.state.offset.y,
        );
        data.insert_temp(
            egui::Id::new(("markdown_live_viewport", file_id)),
            live_scroll.inner_rect,
        );
    });
    #[cfg(not(test))]
    let _ = live_scroll;

    if let Some((range, replacement)) = pending {
        let new_len = replacement.len();
        text.replace_range(range.clone(), &replacement);
        if let Some(edit) = cell_editing.as_mut() {
            let delta = new_len as isize - range.len() as isize;
            if edit.source == range {
                edit.source.end = range.start + new_len;
            } else if edit.source.start >= range.end {
                edit.source = edit.source.start.saturating_add_signed(delta)..edit.source.end.saturating_add_signed(delta);
            }
        }
        if fence_focus.is_none()
            && let Some(edit) = editing.as_mut()
        {
            edit.range = range.start..range.start + new_len;
        }
    }
    if let Some((start, code_text, cursor)) = fence_focus {
        let end = start + code_text.len();
        editing = Some(Editing {
            range: start..end,
            text: code_text,
            focus: true,
            fresh: true,
            code: true,
            cursor: Some(cursor),
        });
        cursor_line = text[..start].bytes().filter(|byte| *byte == b'\n').count();
        selected = 0;
        changed = true;
    }
    if let Some((at, info)) = lang_change.take()
        && let Some(delta) = set_code_language(&mut text, at, &info)
    {
        if let Some(edit) = editing.as_mut()
            && edit.range.start == at
            && set_code_language(&mut edit.text, 0, &info).is_some()
        {
            edit.range.end =
                (edit.range.end as isize + delta).max(edit.range.start as isize) as usize;
        }
        changed = true;
    }

    if let Some(mut picker) = table_picker.take() {
        let zh = lang == Lang::ZhCn;
        let mut open = true;
        let mut confirmed = false;
        egui::Window::new(if zh { "插入表格" } else { "Insert table" })
            .id(egui::Id::new(("markdown_table_picker", file_id)))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ui.ctx(), |ui| {
                ui.horizontal(|ui| {
                    ui.label(if zh { "行" } else { "Rows" });
                    ui.add(egui::DragValue::new(&mut picker.rows).range(1..=50));
                    ui.label(if zh { "列" } else { "Columns" });
                    ui.add(egui::DragValue::new(&mut picker.columns).range(1..=20));
                });
                ui.label(format!("{} × {}", picker.rows, picker.columns));
                let button = ui.add(
                    egui::Button::new(if zh { "插入" } else { "Insert" })
                        .min_size(egui::vec2(88.0, 24.0)),
                );
                #[cfg(test)]
                ui.ctx().data_mut(|data| {
                    data.insert_temp(
                        egui::Id::new(("markdown_table_insert", picker.at)),
                        button.rect,
                    )
                });
                if button.clicked() {
                    confirmed = true;
                }
            });
        if confirmed {
            insert_action = Some((
                picker.at,
                InsertAction::Table {
                    rows: picker.rows,
                    columns: picker.columns,
                },
            ));
        } else if open {
            table_picker = Some(picker);
        }
    }

    if let Some((range, raw, line, cursor, code)) = clicked {
        editing = Some(Editing {
            range,
            text: raw,
            focus: true,
            fresh: true,
            code,
            cursor: Some(cursor),
        });
        cursor_line = line;
        selected = 0;
    }
    if let Some((at, action)) = insert_action {
        match action {
            InsertAction::Table { rows, columns } => {
                let block = table_template(rows, columns, lang == Lang::ZhCn);
                insert_block(&mut text, at, &block);
                // The insertion becomes the authoritative buffer; a live
                // TextEdit still holding the old block text must not write
                // over it.
                editing = None;
                selected = 0;
                changed = true;
            }
            InsertAction::Callout(kind) => {
                let block = kind.template(lang == Lang::ZhCn);
                insert_block(&mut text, at, &block);
                editing = None;
                selected = 0;
                changed = true;
            }
            InsertAction::BlankLine {
                line_start,
                line_end,
                before,
            } => {
                let (insert_at, blank_start) =
                    blank_line_position(&text, line_start, line_end, before);
                text.insert(insert_at, '\n');
                // Start editing the new empty line so the caret is exactly
                // where the inserted line begins.
                editing = Some(Editing {
                    range: blank_start..blank_start,
                    text: String::new(),
                    focus: true,
                    fresh: true,
                    code: false,
                    cursor: Some(0),
                });
                cursor_line = text[..blank_start]
                    .bytes()
                    .filter(|byte| *byte == b'\n')
                    .count();
                selected = 0;
                changed = true;
            }
            InsertAction::Cut { start, end } => {
                remove_block(&mut text, start, end);
                editing = None;
                selected = 0;
                changed = true;
            }
            InsertAction::Paste { at } => {
                // The clipboard text can only be obtained asynchronously from
                // the platform; wait for the paste event on a later frame.
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                paste_at = Some(at);
            }
            InsertAction::InsertTime { at, stamp } => {
                text.insert_str(at, &stamp);
                // The insertion becomes the authoritative buffer; a live
                // TextEdit still holding the old block text must not write
                // over it.
                editing = None;
                selected = 0;
                changed = true;
            }
        }
    }
    if let Some(edit) = cell_editing.as_ref() {
        cursor_line = text[..edit.source.start.min(text.len())].bytes().filter(|byte| *byte == b'\n').count();
    }
    file.editor_state.cursor_line = cursor_line;
    file.editor_state.selected_characters = selected;
    if editing.is_some() || paste_changed {
        cell_editing = None;
    }
    file.markdown.editing = editing;
    file.markdown.cell_editing = cell_editing;
    file.markdown.table_picker = table_picker;
    file.markdown.toc_target = toc_target;
    file.markdown.outline = outline;
    file.markdown.paste_at = paste_at;
    file.draft_text = Some(text);
    changed || paste_changed
}

/// Read-only rendered Markdown. `line_heights` maps each source line to the
/// height of its rendered block for the side-by-side source pane.
pub(crate) fn split_editor(
    ui: &mut egui::Ui,
    file: &mut LoadedFile,
    pal: &Palette,
    focus: bool,
    lang: Lang,
) -> bool {
    let Some(mut text) = file.draft_text.take() else {
        return false;
    };
    // The editable source pane below may change `text` while this frame is
    // still being built. Parse, measure and render the preview from one
    // snapshot so a block byte range can never be applied to a different
    // (possibly shorter) string and split a multi-byte glyph.
    let preview = text.clone();
    let file_id = file.id;
    let body = body_size(file);
    const LEFT_PADDING: f32 = 14.0;
    const RIGHT_PADDING: f32 = 24.0;
    const SCROLLBAR_GUTTER: f32 = 12.0;
    let available = ui.available_rect_before_wrap();
    // Never force the split layout wider than the window: a fixed 320 px
    // minimum used to push the preview past the right edge on narrow windows.
    let total_width = available.width().max(1.0);
    let left_width = (total_width * 0.5).floor();
    let right_width = (total_width - left_width - 8.0).max(120.0);
    let right_content =
        (right_width - LEFT_PADDING - RIGHT_PADDING - SCROLLBAR_GUTTER).max(80.0);
    let base = image_base(file);
    let blocks = parse(&preview);
    ensure_images(ui.ctx(), &mut file.markdown.images, &base, &blocks);
    let images = &file.markdown.images;
    // The preview side never wraps code, tables or HTML, so those blocks are
    // measured at their natural width and can drive horizontal scrolling.
    let widest_content = natural_width(ui, &blocks, &preview, pal, body);
    // Only long, non-wrapping blocks need a horizontal range. The scrollable
    // page is deliberately wider than its widest line: keep the normal right
    // padding plus one full CJK glyph of blank space, so scrolling fully right
    // never leaves the longest line touching the pane edge.
    let trailing_gap = RIGHT_PADDING + body;
    let scroll_width = if widest_content > right_content + 0.5 {
        widest_content + trailing_gap
    } else {
        right_content
    };
    let (heights, fonts) =
        line_layout(ui, &preview, pal, right_content, body, images, true);
    // The find bar edits and navigates the source buffer; mirror its current
    // match on the rendered side so both panes point at the same element.
    let highlight = file
        .editor_state
        .tools
        .find_open
        .then(|| file.editor_state.tools.current.clone())
        .flatten()
        .filter(|range| !range.is_empty());
    let mut changed = false;
    let ctx = ui.ctx().clone();

    // Decide which pane drives the shared offset. The pane under the pointer
    // wins; otherwise a keyboard-driven change in the source pane does. Pane
    // rects come from the previous frame so the first frame simply shows both
    // panes at their own saved offsets.
    let pointer = ctx.pointer_hover_pos();
    let left_rect = ctx
        .data(|data| data.get_temp::<egui::Rect>(egui::Id::new(("markdown_split_left", file_id))));
    let right_rect = ctx
        .data(|data| data.get_temp::<egui::Rect>(egui::Id::new(("markdown_split_right", file_id))));
    let in_left = pointer.is_some_and(|p| left_rect.is_some_and(|rect| rect.contains(p)));
    let in_right = pointer.is_some_and(|p| right_rect.is_some_and(|rect| rect.contains(p)));
    // A find / go-to-line jump must reveal its match in the editable source
    // pane. While the find box (top-right) is hovered the pointer counts as
    // "in the preview", so the forced shared offset would otherwise cancel the
    // reveal; skip it for this frame and let the panes follow the new offset.
    let pending_jump = file.editor_state.pending_selection.is_some();
    if pending_jump {
        file.markdown.reveal_frames = 2;
    }
    let revealing = pending_jump || file.markdown.reveal_frames > 0;
    file.markdown.reveal_frames = file.markdown.reveal_frames.saturating_sub(1);
    let mut offset = file.markdown.split_offset;
    let mut left_is_source = false;

    ui.horizontal_top(|ui| {
        // The panes and the divider are sized explicitly; egui's default item
        // spacing would add another ~8 px and push the preview past the window.
        ui.spacing_mut().item_spacing.x = 0.0;
        let left = ui.allocate_ui_with_layout(
            egui::vec2(left_width, available.height()),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                if !heights.is_empty() {
                    changed = crate::editor::show_synced(
                        ui,
                        &mut text,
                        &mut file.editor_state,
                        file_id,
                        pal,
                        focus,
                        crate::tr(lang),
                        &heights,
                        &fonts,
                        (!revealing && in_right).then_some(offset),
                    );
                } else {
                    changed = crate::editor::show(
                        ui,
                        &mut text,
                        &mut file.editor_state,
                        file_id,
                        pal,
                        focus,
                        crate::tr(lang),
                    );
                }
            },
        );
        if revealing || !in_right {
            // Hovering the source pane (or moving its cursor) makes it
            // authoritative; any visible change updates the shared offset.
            // A find jump is authoritative for this frame even when the pointer
            // is over the find box, which sits inside the preview pane's rect.
            if let Some(current) = crate::editor::last_scroll_offset(&ctx, file_id)
                && (revealing || in_left || (current - offset).abs() > 0.5)
            {
                offset = current;
                left_is_source = true;
            }
        }
        let (divider, _) =
            ui.allocate_exact_size(egui::vec2(8.0, available.height()), egui::Sense::hover());
        ui.painter().rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(divider.center().x, divider.top()),
                egui::vec2(1.0, divider.height()),
            ),
            0.0,
            pal.bg_elevated,
        );
        let right = ui.allocate_ui_with_layout(
            egui::vec2(right_width, available.height()),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                let mut area = egui::ScrollArea::both()
                    .id_salt(("markdown_split_scroll", file_id))
                    .auto_shrink([false, false]);
                if in_left || left_is_source || !in_right {
                    area = area.vertical_scroll_offset(offset);
                }
                let output = area.show(ui, |ui| {
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        ui.add_space(LEFT_PADDING);
                        ui.vertical(|ui| {
                            ui.set_width(right_content);
                            ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                            for block in &blocks {
                                let raw = &preview[block.range.clone()];
                                let _ = render_block(
                                    ui,
                                    block,
                                    raw,
                                    right_content,
                                    pal,
                                    body,
                                    images,
                                    true,
                                    highlight.clone(),
                                    lang,
                                    false,
                                    file_id,
                                );
                            }
                            // Report the natural width of non-wrapping blocks
                            // so the pane gets a horizontal scroll range, and
                            // keep a right margin inside that range.
                            ui.set_min_width(scroll_width);
                            ui.add_space(40.0);
                        });
                    });
                });
                #[cfg(test)]
                ctx.data_mut(|data| {
                    data.insert_temp(
                        egui::Id::new(("markdown_split_geometry", file_id)),
                        (output.content_size, output.inner_rect, output.state.offset.x),
                    )
                });
                if in_right {
                    offset = output.state.offset.y;
                }
            },
        );
        ctx.data_mut(|data| {
            data.insert_temp(egui::Id::new(("markdown_split_left", file_id)), left.response.rect);
            data.insert_temp(egui::Id::new(("markdown_split_right", file_id)), right.response.rect);
        });
    });
    file.markdown.split_offset = offset;
    file.draft_text = Some(text);
    changed
}

fn body_size(file: &LoadedFile) -> f32 {
    if file.editor_state.font_size > 0.0 {
        (file.editor_state.font_size + 1.5).clamp(14.0, 30.0)
    } else {
        15.0
    }
}

fn heading_size(level: u8, body: f32) -> f32 {
    match level {
        1 => body * 2.05,
        2 => body * 1.65,
        3 => body * 1.38,
        4 => body * 1.18,
        5 => body * 1.06,
        _ => body,
    }
}

fn heading_font(level: u8, body: f32) -> FontId {
    FontId::new(heading_size(level, body), FontFamily::Name("brand".into()))
}

fn heading_space(level: u8, body: f32) -> (f32, f32) {
    let top = match level {
        1 => body * 0.9,
        2 => body * 0.75,
        _ => body * 0.55,
    };
    let bottom = match level {
        1 | 2 => body * 0.5,
        _ => body * 0.32,
    };
    (top, bottom)
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Kind {
    Heading(u8),
    Paragraph,
    List { ordered: bool, marker: String },
    Quote,
    Callout(CalloutKind),
    Code(Option<String>),
    Rule,
    Table,
    Html,
    Image { alt: String, src: String },
    Blank,
}

#[derive(Clone, Debug)]
struct Block {
    range: Range<usize>,
    line_start: usize,
    line_count: usize,
    kind: Kind,
}

fn line_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            let end = if index > start && text.as_bytes()[index - 1] == b'\r' {
                index - 1
            } else {
                index
            };
            out.push((start, end));
            start = index + 1;
        }
    }
    out.push((start, text.len()));
    out
}

fn parse(text: &str) -> Vec<Block> {
    let lines = line_ranges(text);
    let mut blocks = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let raw = &text[lines[index].0..lines[index].1];
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            blocks.push(Block {
                range: lines[index].0..lines[index].1,
                line_start: index,
                line_count: 1,
                kind: Kind::Blank,
            });
            index += 1;
            continue;
        }
        if let Some(fence) = fence_marker(trimmed) {
            let mut end = index + 1;
            while end < lines.len() {
                let candidate = text[lines[end].0..lines[end].1].trim();
                if fence_marker(candidate).is_some() && candidate.starts_with(fence) {
                    end += 1;
                    break;
                }
                end += 1;
            }
            let info = trimmed
                .trim_start_matches('`')
                .trim_start_matches('~')
                .trim();
            blocks.push(block(
                &lines,
                index,
                end,
                text,
                Kind::Code((!info.is_empty()).then(|| info.to_owned())),
            ));
            index = end;
            continue;
        }
        if let Some(level) = heading_level(trimmed) {
            blocks.push(block(&lines, index, index + 1, text, Kind::Heading(level)));
            index += 1;
            continue;
        }
        if is_rule(trimmed) {
            blocks.push(block(&lines, index, index + 1, text, Kind::Rule));
            index += 1;
            continue;
        }
        if trimmed.starts_with('>') {
            let mut end = index + 1;
            while end < lines.len() {
                let candidate = text[lines[end].0..lines[end].1].trim_start();
                if !candidate.starts_with('>') {
                    break;
                }
                end += 1;
            }
            let kind = callout_kind(trimmed)
                .map_or(Kind::Quote, Kind::Callout);
            blocks.push(block(&lines, index, end, text, kind));
            index = end;
            continue;
        }
        if let Some((ordered, marker)) = list_marker(trimmed) {
            let mut end = index + 1;
            while end < lines.len() {
                let candidate = &text[lines[end].0..lines[end].1];
                let candidate_trimmed = candidate.trim();
                if candidate_trimmed.is_empty()
                    || heading_level(candidate_trimmed).is_some()
                    || fence_marker(candidate_trimmed).is_some()
                    || candidate_trimmed.starts_with('>')
                    || is_rule(candidate_trimmed)
                    || list_marker(candidate_trimmed).is_some()
                {
                    break;
                }
                // Only indented continuation lines belong to the item.
                if !candidate.starts_with(' ') && !candidate.starts_with('\t') {
                    break;
                }
                end += 1;
            }
            blocks.push(block(
                &lines,
                index,
                end,
                text,
                Kind::List { ordered, marker },
            ));
            index = end;
            continue;
        }
        if index + 1 < lines.len()
            && table_start(raw, &text[lines[index + 1].0..lines[index + 1].1])
        {
            let mut end = index + 2;
            while end < lines.len() && text[lines[end].0..lines[end].1].contains('|') {
                end += 1;
            }
            blocks.push(block(&lines, index, end, text, Kind::Table));
            index = end;
            continue;
        }
        if trimmed.starts_with('<') && trimmed.ends_with('>') {
            let kind = match html_image_line(trimmed) {
                Some((alt, src)) => Kind::Image { alt, src },
                None => Kind::Html,
            };
            blocks.push(block(&lines, index, index + 1, text, kind));
            index += 1;
            continue;
        }
        if let Some((alt, src)) = image_line(trimmed) {
            blocks.push(block(
                &lines,
                index,
                index + 1,
                text,
                Kind::Image { alt, src },
            ));
            index += 1;
            continue;
        }
        let mut end = index + 1;
        while end < lines.len() {
            let candidate = &text[lines[end].0..lines[end].1];
            let candidate_trimmed = candidate.trim();
            if candidate_trimmed.is_empty()
                || heading_level(candidate_trimmed).is_some()
                || fence_marker(candidate_trimmed).is_some()
                || candidate_trimmed.starts_with('>')
                || is_rule(candidate_trimmed)
                || list_marker(candidate_trimmed).is_some()
                || candidate_trimmed.starts_with('<') && candidate_trimmed.ends_with('>')
                || image_line(candidate_trimmed).is_some()
                || end + 1 < lines.len()
                    && table_start(candidate, &text[lines[end + 1].0..lines[end + 1].1])
            {
                break;
            }
            end += 1;
        }
        blocks.push(block(&lines, index, end, text, Kind::Paragraph));
        index = end;
    }
    blocks
}

fn block(lines: &[(usize, usize)], start: usize, end: usize, text: &str, kind: Kind) -> Block {
    let end = end.min(lines.len()).max(start + 1);
    let _ = text;
    Block {
        range: lines[start].0..lines[end - 1].1,
        line_start: start,
        line_count: end - start,
        kind,
    }
}

fn fence_marker(trimmed: &str) -> Option<&'static str> {
    if trimmed.starts_with("```") {
        Some("```")
    } else if trimmed.starts_with("~~~") {
        Some("~~~")
    } else {
        None
    }
}

fn heading_level(trimmed: &str) -> Option<u8> {
    let level = trimmed.bytes().take_while(|byte| *byte == b'#').count();
    (1..=6)
        .contains(&level)
        .then_some(level as u8)
        .filter(|_| trimmed[level..].starts_with(' ') || trimmed.len() == level)
}

fn is_rule(trimmed: &str) -> bool {
    let mut chars = trimmed.chars().filter(|c| !c.is_whitespace());
    let Some(first) = chars.next() else {
        return false;
    };
    matches!(first, '-' | '*' | '_') && chars.clone().count() >= 2 && chars.all(|c| c == first)
}

fn list_marker(trimmed: &str) -> Option<(bool, String)> {
    let mut chars = trimmed.char_indices();
    let (_, first) = chars.next()?;
    if matches!(first, '-' | '*' | '+') {
        let rest = &trimmed[1..];
        return rest.starts_with(' ').then(|| (false, first.to_string()));
    }
    let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 && digits <= 3 {
        let rest = &trimmed[digits..];
        if let Some(rest) = rest.strip_prefix('.')
            && rest.starts_with(' ')
        {
            return Some((true, trimmed[..digits + 1].to_owned()));
        }
    }
    None
}

/// A whole line holding a single image: `![alt](path "optional title")`.
fn image_line(trimmed: &str) -> Option<(String, String)> {
    let rest = trimmed.strip_prefix("![")?;
    let close = rest.find("](")?;
    let alt = rest[..close].to_owned();
    let tail = &rest[close + 2..];
    let end = tail.rfind(')')?;
    if end + 1 != tail.len() {
        return None;
    }
    let mut target = tail[..end].trim();
    if target.starts_with('<') && target.ends_with('>') {
        target = target[1..target.len() - 1].trim();
    } else if target.ends_with('"') || target.ends_with('\'') {
        // Drop a quoted title after the path.
        let quote = target.chars().last()?;
        if let Some(start) = target[..target.len() - 1].rfind(quote)
            && start > 0
        {
            target = target[..start].trim_end();
        }
    }
    (!target.is_empty()).then(|| (alt, target.to_owned()))
}

/// A standalone HTML image tag, common in exported Markdown:
/// `<img src="picture.png" alt="...">`.
fn html_image_line(trimmed: &str) -> Option<(String, String)> {
    let lower = trimmed.to_ascii_lowercase();
    if !lower.starts_with("<img") || !lower.ends_with('>') {
        return None;
    }
    let src = html_attribute(trimmed, "src")?;
    let alt = html_attribute(trimmed, "alt").unwrap_or_default();
    (!src.is_empty()).then_some((alt, src))
}

fn html_attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let start = lower
        .find(&format!("{name}="))
        .map(|index| index + name.len() + 1)?;
    let rest = &tag[start..];
    if let Some(quote) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') {
        let body = &rest[1..];
        let end = body.find(quote)?;
        Some(body[..end].to_owned())
    } else {
        let end = rest
            .find(|c: char| c.is_whitespace() || c == '>')
            .unwrap_or(rest.len());
        Some(rest[..end].to_owned())
    }
}

/// Byte ranges into the original row: escaped pipes and paired code spans
/// never split cells. No normalized copy is written back to the draft.
fn table_cells(line: &str) -> Vec<Range<usize>> {
    let mut pipes = Vec::new();
    let mut i = 0;
    let bytes = line.as_bytes();
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => { i = (i + 2).min(bytes.len()); }
            b'`' => {
                if let Some((len, _, _)) = backtick_span(&line[i..]) { i += len; }
                else { i += bytes[i..].iter().take_while(|b| **b == b'`').count(); }
            }
            b'|' => { pipes.push(i); i += 1; }
            _ => { i += 1; }
        }
    }
    let mut ranges = Vec::with_capacity(pipes.len() + 1);
    let mut start = 0;
    for end in pipes { ranges.push(start..end); start = end + 1; }
    ranges.push(start..line.len());
    if ranges.len() > 1 && line[ranges[0].clone()].trim().is_empty() { ranges.remove(0); }
    if ranges.len() > 1 && line[ranges.last().unwrap().clone()].trim().is_empty() { ranges.pop(); }
    for range in &mut ranges {
        let cell = &line[range.clone()];
        let lead = cell.len() - cell.trim_start().len();
        let len = cell.trim().len();
        range.start += lead;
        range.end = range.start + len;
    }
    ranges
}

fn table_alignments(line: &str) -> Option<Vec<egui::Align>> {
    table_cells(line).into_iter().map(|range| {
        let cell = &line[range];
        let dashes = cell.strip_prefix(':').unwrap_or(cell);
        let dashes = dashes.strip_suffix(':').unwrap_or(dashes);
        (dashes.len() >= 3 && dashes.bytes().all(|b| b == b'-')).then_some(
            if cell.starts_with(':') && cell.ends_with(':') { egui::Align::Center }
            else if cell.ends_with(':') { egui::Align::RIGHT }
            else { egui::Align::LEFT }
        )
    }).collect()
}

fn table_start(header: &str, separator: &str) -> bool {
    header.contains('|') && table_alignments(separator)
        .is_some_and(|align| align.len() == table_cells(header).len())
}

struct TableCellLayout {
    source: Range<usize>,
    galley: std::sync::Arc<egui::Galley>,
}
struct TableLayout {
    rows: Vec<Vec<TableCellLayout>>,
    columns: Vec<f32>,
    align: Vec<egui::Align>,
    row_height: f32,
}
impl TableLayout {
    fn width(&self) -> f32 { self.columns.iter().sum() }
    fn height(&self) -> f32 { self.rows.len() as f32 * self.row_height }
}

fn table_cell_job(raw: &str, range: Range<usize>, header: bool, body: f32,
    pal: &Palette, highlight: Option<&Range<usize>>) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    let font = if header { FontId::new(body, FontFamily::Name("brand".into())) }
        else { FontId::proportional(body) };
    let base = egui::TextFormat { font_id: font.clone(), color: pal.text_main, ..Default::default() };
    push_inline_fmt(&mut job, &raw[range.clone()], &base, pal, highlight, range.start);
    if job.sections.is_empty() { job.append("", 0.0, base); }
    apply_body_line_height(&mut job, &font);
    job
}

fn table_layout(ui: &egui::Ui, raw: &str, body: f32, pal: &Palette,
    highlight: Option<&Range<usize>>) -> TableLayout {
    let lines = line_ranges(raw);
    let align = lines.get(1).and_then(|&(s, e)| table_alignments(&raw[s..e])).unwrap_or_default();
    let mut columns = vec![48.0_f32; align.len()];
    let mut rows = Vec::new();
    let mut row_height = body * BODY_LINE_HEIGHT + 14.0;
    for (index, &(start, end)) in lines.iter().enumerate() {
        if index == 1 || (index > 1 && raw[start..end].trim().is_empty()) { continue; }
        let mut cells = table_cells(&raw[start..end]);
        cells.resize(align.len(), end-start..end-start);
        cells.truncate(align.len());
        let row = cells.into_iter().enumerate().map(|(column, range)| {
            let source = start + range.start..start + range.end;
            let job = table_cell_job(raw, source.clone(), index == 0, body, pal, highlight);
            let galley = ui.fonts(|fonts| fonts.layout_job(job));
            columns[column] = columns[column].max(galley.size().x + 20.0);
            row_height = row_height.max(galley.size().y + 14.0);
            TableCellLayout { source, galley }
        }).collect();
        rows.push(row);
    }
    // Editing exposes the separator too; reserve enough room for every source
    // row without changing the following block's position on entry.
    if !rows.is_empty() {
        row_height = row_height.max(lines.len() as f32 * body * BODY_LINE_HEIGHT / rows.len() as f32);
        // egui rounds each TextEdit source row to whole points. Reserve the
        // same total in the grid, including fractional editor font sizes.
        let per_source = (row_height * rows.len() as f32 / lines.len() as f32).ceil();
        row_height = per_source * lines.len() as f32 / rows.len() as f32;
    }
    TableLayout { rows, columns, align, row_height }
}

/// Serialize only the edited cell. Existing escapes are retained byte-for-byte;
/// new pipes (including pasted ones) cannot turn into column separators.
fn table_cell_source(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut slashes = 0;
    for ch in text.chars() {
        if ch == '|' && slashes % 2 == 0 { result.push('\\'); }
        result.push(if ch == '\r' || ch == '\n' { ' ' } else { ch });
        slashes = if ch == '\\' { slashes + 1 } else { 0 };
    }
    // With no padding before the row delimiter, an odd trailing slash would
    // escape that delimiter and merge the following column into this one.
    if slashes % 2 != 0 { result.push('\\'); }
    result
}

#[allow(clippy::too_many_arguments)]
fn live_table(ui: &mut egui::Ui, raw: &str, table: usize, width: f32, body: f32,
    pal: &Palette, file_id: u64, lang: Lang, highlight: Option<&Range<usize>>, editing: &mut Option<CellEditing>,
    pending: &mut Option<(Range<usize>, String)>) -> egui::Response {
    let layout = table_layout(ui, raw, body, pal, highlight);
    ui.add_space(5.0);
    ui.spacing_mut().scroll = egui::style::ScrollStyle::floating();
    let scroll = egui::ScrollArea::horizontal()
        .id_salt(("markdown_table_scroll", file_id, table))
        .drag_to_scroll(false).auto_shrink([false, true])
        .max_width(width)
        .show(ui, |ui| {
            let (rect, response) = ui.allocate_exact_size(
                egui::vec2(layout.width(), layout.height()), egui::Sense::hover());
            paint_table(ui, &layout, rect.min, pal);
            let mut next = None;
            let mut finish = false;
            let mut structural = None;
            for (row, cells) in layout.rows.iter().enumerate() {
                let mut x = rect.left();
                for (column, _cell) in cells.iter().enumerate() {
                    let cell_rect = egui::Rect::from_min_size(
                        egui::pos2(x, rect.top() + row as f32 * layout.row_height),
                        egui::vec2(layout.columns[column], layout.row_height));
                    x += layout.columns[column];
                    #[cfg(test)]
                    ui.ctx().data_mut(|data| data.insert_temp(
                        egui::Id::new(("markdown_cell", file_id, table, row, column)), cell_rect));
                    let active = editing.as_ref().is_some_and(|edit|
                        edit.table == table && edit.row == row && edit.column == column);
                    if active {
                        let edit = editing.as_mut().unwrap();
                        let id = egui::Id::new(("markdown_cell_edit", file_id, table, row, column));
                        let focused = ui.memory(|memory| memory.has_focus(id));
                        if ui.input(|input| input.key_pressed(egui::Key::Escape)) { finish = true; }
                        if focused && !ui.input(|input| input.events.iter().any(|e| matches!(e, egui::Event::Ime(_)))) {
                            if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                                || input.key_pressed(egui::Key::Escape)) { finish = true; }
                            let backwards = ui.input_mut(|input| input.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab));
                            if backwards || ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Tab)) {
                                let count = layout.rows.len() * layout.columns.len();
                                let current = row * layout.columns.len() + column;
                                let index = if backwards { (current + count - 1) % count } else { (current + 1) % count };
                                next = Some((index / layout.columns.len(), index % layout.columns.len()));
                            }
                        }
                        // Cover just the active glyphs, never the grid lines.
                        ui.painter().rect_filled(cell_rect.shrink(1.0), 0.0,
                            if row == 0 { pal.bg_elevated } else { pal.bg_inset });
                        let font = if row == 0 { FontId::new(body, FontFamily::Name("brand".into())) }
                            else { FontId::proportional(body) };
                        crate::undo_settings::configure(ui.ctx(), id, &edit.text);
                        // Use the exact preview origin and body line metrics;
                        // ui.put over a tall cell otherwise centers a natural
                        // font row differently from the rendered table galley.
                        let free = (layout.columns[column] - 20.0 - _cell.galley.size().x).max(0.0);
                        let offset = match layout.align[column] { egui::Align::Center => free * 0.5, egui::Align::Max => free, _ => 0.0 };
                        let origin = cell_rect.min + egui::vec2(10.0 + offset, 7.0);
                        let edit_rect = egui::Rect::from_min_size(origin,
                            egui::vec2((cell_rect.right() - origin.x - 4.0).max(12.0), _cell.galley.size().y));
                        let mut layouter = |ui: &egui::Ui, text: &str, _width: f32| {
                            let mut job = egui::text::LayoutJob::default();
                            job.wrap.max_width = f32::INFINITY;
                            let base = egui::TextFormat {
                                font_id: font.clone(), color: pal.text_main,
                                line_height: Some(body * BODY_LINE_HEIGHT), ..Default::default()
                            };
                            // Keep paired inline syntax rendered while the raw
                            // characters remain available for caret/undo. An
                            // incomplete delimiter falls back to visible source.
                            push_inline_editing(&mut job, text, &base, pal);
                            if job.sections.is_empty() { job.append("", 0.0, base); }
                            apply_body_line_height(&mut job, &font);
                            ui.fonts(|fonts| fonts.layout_job(job))
                        };
                        let output = ui.put(edit_rect,
                            egui::TextEdit::singleline(&mut edit.text).id(id).font(font.clone())
                                .text_color(pal.text_main).frame(false).margin(egui::Margin::ZERO)
                                .vertical_align(egui::Align::TOP)
                                .layouter(&mut layouter).desired_width(edit_rect.width()));
                        let cell_menu_open = output.context_menu_opened();
                        output.context_menu(|menu| {
                            if let Some(action) = table_structure_menu(menu, file_id, table, row, column,
                                layout.rows.len(), layout.columns.len(), lang) { structural = Some((row, column, action)); }
                        });
                        if edit.focus { output.request_focus(); edit.focus = false; }
                        if output.changed() {
                            let replacement = table_cell_source(&edit.text);
                            edit.committed = replacement.clone();
                            *pending = Some((edit.source.clone(), replacement));
                        }
                        if focused && !cell_menu_open && ui.input(|input| input.pointer.any_pressed())
                            && ui.input(|input| input.pointer.interact_pos().is_some_and(|pos| !cell_rect.contains(pos))) {
                            finish = true;
                        }
                    } else {
                        let hit = ui.interact(cell_rect, egui::Id::new(("markdown_cell_hit", file_id, table, row, column)), egui::Sense::click());
                        if hit.clicked() { next = Some((row, column)); }
                        hit.context_menu(|menu| {
                            if let Some(action) = table_structure_menu(menu, file_id, table, row, column,
                                layout.rows.len(), layout.columns.len(), lang) { structural = Some((row, column, action)); }
                        });
                    }
                }
            }
            if let Some((row, column, action)) = structural
                && let Some(replacement) = table_ops::edit(raw, row, column, action)
            {
                    *pending = Some((table..table + raw.len(), replacement));
                    *editing = None;
                    next = None;
                    ui.ctx().request_repaint();
            }
            if finish { *editing = None; }
            if let Some((row, column)) = next {
                let source = layout.rows[row][column].source.clone();
                *editing = Some(CellEditing { table, row, column,
                    text: raw[source.clone()].to_owned(),
                    committed: raw[source.clone()].to_owned(),
                    source: table + source.start..table + source.end, focus: true });
                ui.ctx().request_repaint();
            }
            response
        });
    ui.ctx().data_mut(|data| data.insert_temp(
        egui::Id::new(("markdown_table_offset", file_id, table)), scroll.state.offset.x));
    ui.add_space(6.0);
    scroll.inner
}

#[allow(clippy::too_many_arguments)]
fn table_structure_menu(ui: &mut egui::Ui, file: u64, table: usize, row: usize, column: usize,
    rows: usize, columns: usize, lang: Lang) -> Option<table_ops::Action> {
    let zh = lang == Lang::ZhCn;
    let mut action = None;
    for (kind, label, entries) in [
        ("row", if zh { "插入行" } else { "Insert row" }, [
            (if zh { "上方插入一行" } else { "Insert row above" }, table_ops::Action::RowAbove),
            (if zh { "下方插入一行" } else { "Insert row below" }, table_ops::Action::RowBelow)]),
        ("column", if zh { "插入列" } else { "Insert column" }, [
            (if zh { "左侧插入一列" } else { "Insert column left" }, table_ops::Action::ColumnLeft),
            (if zh { "右侧插入一列" } else { "Insert column right" }, table_ops::Action::ColumnRight)]),
    ] {
        let response = ui.menu_button(label, |menu| {
            for (index, (label, value)) in entries.into_iter().enumerate() {
                let response = menu.button(label);
                menu.ctx().data_mut(|d| d.insert_temp(egui::Id::new(("table_insert_action", file, table, kind, index)), response.rect));
                if response.clicked() { action = Some(value); menu.close_menu(); }
            }
        });
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new(("table_insert_menu", file, table, kind)), response.response.rect));
    }
    ui.separator();
    let delete_row = ui.add_enabled(row > 0 || rows > 1,
        egui::Button::new(if zh { "删除当前行" } else { "Delete current row" }));
    let delete_column = ui.add_enabled(columns > 1,
        egui::Button::new(if zh { "删除当前列" } else { "Delete current column" }));
    ui.ctx().data_mut(|d| {
        d.insert_temp(egui::Id::new(("table_delete_row", file, table)), delete_row.rect);
        d.insert_temp(egui::Id::new(("table_delete_column", file, table)), delete_column.rect);
    });
    if delete_row.clicked() { action = Some(table_ops::Action::DeleteRow); }
    if delete_column.clicked() { action = Some(table_ops::Action::DeleteColumn); }
    if action.is_some() { ui.close_menu(); }
    let _ = column;
    action
}

fn paint_table(ui: &egui::Ui, layout: &TableLayout, origin: egui::Pos2, pal: &Palette) {
    let rect = egui::Rect::from_min_size(origin, egui::vec2(layout.width(), layout.height()));
    ui.painter().rect_filled(rect, 0.0, pal.bg_inset);
    let stroke = Stroke::new(1.0, pal.text_faint.gamma_multiply(0.35));
    for (row_index, row) in layout.rows.iter().enumerate() {
        let y = origin.y + row_index as f32 * layout.row_height;
        if row_index == 0 {
            ui.painter().rect_filled(egui::Rect::from_min_size(origin, egui::vec2(layout.width(), layout.row_height)), 0.0, pal.bg_elevated);
        }
        if y + layout.row_height >= ui.clip_rect().top() && y <= ui.clip_rect().bottom() {
            let mut x = origin.x;
            for (column, cell) in row.iter().enumerate() {
                let free = (layout.columns[column] - 20.0 - cell.galley.size().x).max(0.0);
                let offset = match layout.align[column] { egui::Align::Center => free * 0.5, egui::Align::Max => free, _ => 0.0 };
                ui.painter().galley(egui::pos2(x + 10.0 + offset, y + 7.0), cell.galley.clone(), pal.text_main);
                x += layout.columns[column];
            }
            ui.painter().line_segment([egui::pos2(origin.x, y), egui::pos2(rect.right(), y)], stroke);
        }
    }
    let mut x = origin.x;
    for width in layout.columns.iter().chain(std::iter::once(&0.0)) {
        ui.painter().line_segment([egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())], stroke);
        x += width;
    }
    ui.painter().line_segment([rect.left_bottom(), rect.right_bottom()], stroke);
}

fn heading_text(raw: &str) -> String {
    let trimmed = raw.trim();
    let level = trimmed.bytes().take_while(|byte| *byte == b'#').count();
    trimmed[level..]
        .trim()
        .trim_end_matches('#')
        .trim()
        .to_owned()
}

#[derive(Clone, Default)]
struct OutlineState {
    active: Option<usize>,
    clicked: Option<usize>,
}

impl OutlineState {
    fn update(&mut self, headings: &[(usize, f32)], viewport: egui::Rect,
        at_bottom: bool, editing: Option<usize>, manual_scroll: bool,
    ) -> Option<usize> {
        if manual_scroll { self.clicked = None; }
        let valid = |at: &usize| headings.iter().any(|(id, _)| id == at);
        self.clicked = self.clicked.filter(valid);
        let forced = editing.filter(valid).or(self.clicked);
        let line = viewport.top() + viewport.height() * 0.2;
        let candidate = forced.or_else(|| {
            if at_bottom {
                headings.iter().rev().find(|(_, top)| *top < viewport.bottom()).map(|(at, _)| *at)
            } else {
                headings.iter().rev().find(|(_, top)| *top <= line)
                    .or_else(|| headings.first()).map(|(at, _)| *at)
            }
        });
        if forced.is_none() && !at_bottom
            && let (Some(previous), Some(next)) = (self.active, candidate)
        {
            let old = headings.iter().position(|(at, _)| *at == previous);
            let new = headings.iter().position(|(at, _)| *at == next);
            if let (Some(old), Some(new)) = (old, new) {
                // Shrink hysteresis for tightly packed headings, rather
                // than making a short section impossible to select.
                let boundary = if new > old { new } else { old };
                let mut gap = if boundary > 0 {
                    (headings[boundary].1 - headings[boundary - 1].1).abs()
                } else { 64.0 };
                if boundary + 1 < headings.len() {
                    gap = gap.min((headings[boundary + 1].1 - headings[boundary].1).abs());
                }
                let buffer = 16.0_f32.min(gap * 0.25);
                if new > old && headings[new].1 > line - buffer
                    || new < old && headings[old].1 < line + buffer
                {
                    return self.active;
                }
            }
        }
        self.active = candidate;
        candidate
    }
}

/// The last heading above the reading line owns the current section, including
/// its paragraphs. Before the first heading enters that line, select it anyway.
/// Positions come from this frame's layout, so scrolling, resizing and inline
/// editing all update the outline without relying on a previous click.
#[cfg(test)]
fn active_outline_heading(headings: &[(usize, f32)], viewport_top: f32) -> Option<usize> {
    let reading_line = viewport_top + 18.0;
    headings
        .iter()
        .rev()
        .find(|(_, top)| *top <= reading_line)
        .or_else(|| headings.first())
        .map(|(at, _)| *at)
}

/// Any heading level may own the current section. A focused heading wins;
/// at the document bottom, later visible headings cannot reach the top reading
/// line, so use the last visible heading instead of leaving its parent lit.
#[cfg(test)]
fn outline_heading_for_view(
    headings: &[(usize, f32)], viewport: egui::Rect, at_bottom: bool,
    editing_heading: Option<usize>,
) -> Option<usize> {
    editing_heading.or_else(|| {
        if at_bottom {
            headings.iter().rev().find(|(_, top)| *top < viewport.bottom()).map(|(at, _)| *at)
        } else {
            active_outline_heading(headings, viewport.top())
        }
    })
}

/// Right-hand document outline for the live render. Headings are indented by
/// level and clicking one returns its block byte offset.
fn heading_outline(
    ui: &mut egui::Ui,
    headings: &[(usize, u8, String)],
    active: Option<usize>,
    pal: &Palette,
    body: f32,
) -> Option<usize> {
    let mut clicked = None;
    let width = ui.available_width();
    ui.vertical(|ui| {
        ui.set_width(width);
        ui.add_space(18.0);
        egui::ScrollArea::vertical()
            .id_salt("markdown_outline")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                for (at, level, text) in headings {
                    ui.horizontal(|ui| {
                        ui.add_space((*level as f32 - 1.0) * 10.0);
                        let selected = active == Some(*at);
                        let mut label = egui::RichText::new(text)
                            .size(body * 0.82)
                            .color(if selected { pal.accent } else { pal.text_faint });
                        if selected {
                            label = label.strong();
                        }
                        let response = ui.add(
                            egui::Label::new(label)
                            .sense(egui::Sense::click())
                            .truncate(),
                        );
                        if response.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if response.clicked() {
                            clicked = Some(*at);
                        }
                    });
                    ui.add_space(2.0);
                }
            });
    });
    clicked
}

fn list_text(raw: &str) -> (String, String) {
    let trimmed = raw.trim_start();
    if let Some((ordered, marker)) = list_marker(trimmed) {
        let body = trimmed[marker.len()..].trim_start().to_owned();
        let _ = ordered;
        (marker, body)
    } else {
        (String::new(), trimmed.to_owned())
    }
}

fn quote_text(raw: &str) -> String {
    raw.lines()
        .map(|line| {
            line.trim_start()
                .strip_prefix('>')
                .unwrap_or(line)
                .trim_start()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Callout body without the `[!TYPE]` marker line and quote prefixes.
fn callout_body(raw: &str) -> String {
    let quoted = quote_text(raw);
    let mut out = String::new();
    for (index, line) in quoted.lines().enumerate() {
        let line = if index == 0 {
            line.trim_start()
                .strip_prefix("[!")
                .and_then(|rest| rest.split_once(']').map(|(_, rest)| rest.trim_start()))
                .unwrap_or(line)
        } else {
            line
        };
        if index == 0 && line.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
    }
    out
}

/// A Markdown table with a header row and `rows - 1` empty body rows.
fn table_template(rows: usize, columns: usize, zh: bool) -> String {
    let columns = columns.clamp(1, 20);
    let rows = rows.clamp(1, 50);
    let mut out = String::new();
    out.push('|');
    for column in 1..=columns {
        if zh {
            out.push_str(&format!(" 列 {column} |"));
        } else {
            out.push_str(&format!(" Column {column} |"));
        }
    }
    out.push('\n');
    out.push('|');
    for _ in 0..columns {
        out.push_str(" --- |");
    }
    for _ in 1..rows {
        out.push('\n');
        out.push('|');
        for _ in 0..columns {
            out.push_str("  |");
        }
    }
    out
}

/// Insert a standalone Markdown block at a document byte offset, adding only
/// the blank lines needed to keep it a separate block. Returns the inserted
/// block's byte range (without the surrounding separators).
fn insert_block(text: &mut String, at: usize, block: &str) -> Range<usize> {
    let at = at.min(text.len());
    let before = &text[..at];
    let after = &text[at..];
    let mut insertion = String::new();
    let newlines_before = before.chars().rev().take_while(|character| *character == '\n').count();
    if !before.is_empty() && newlines_before == 0 {
        insertion.push_str("\n\n");
    } else if newlines_before == 1 {
        insertion.push('\n');
    }
    let start = at + insertion.len();
    insertion.push_str(block);
    let newlines_after = after.chars().take_while(|character| *character == '\n').count();
    if !after.is_empty() {
        if newlines_after == 0 {
            insertion.push_str("\n\n");
        } else if newlines_after == 1 {
            insertion.push('\n');
        }
    } else if !insertion.ends_with('\n') {
        insertion.push('\n');
    }
    text.insert_str(at, &insertion);
    start..start + block.len()
}

/// Length of the line break starting at `at` (`\r\n` or `\n`).
fn line_break_at(text: &str, at: usize) -> Option<usize> {
    match text.as_bytes().get(at) {
        Some(b'\r') if text.as_bytes().get(at + 1) == Some(&b'\n') => Some(2),
        Some(b'\n') => Some(1),
        _ => None,
    }
}

/// Length of the line break ending at `at` (`\r\n` or `\n`).
fn line_break_before(text: &str, at: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    match at.checked_sub(1).and_then(|index| bytes.get(index)) {
        Some(b'\n') if at >= 2 && bytes.get(at - 2) == Some(&b'\r') => Some(2),
        Some(b'\n') => Some(1),
        _ => None,
    }
}

/// Remove a whole block and one blank-line separator, so cutting a paragraph
/// does not leave a growing run of empty lines behind.
fn remove_block(text: &mut String, start: usize, end: usize) {
    let mut from = start.min(text.len());
    let mut to = end.min(text.len());
    // Take the block's own terminator plus one following blank line.
    if let Some(len) = line_break_at(text, to) {
        to += len;
    }
    if let Some(len) = line_break_at(text, to) {
        to += len;
    }
    if to >= text.len()
        && let Some(last) = line_break_before(text, from)
        && line_break_before(text, from - last).is_some()
    {
        // At the end of the document keep the previous line's terminator but
        // drop the separator blank line before the removed block.
        from -= last;
    }
    let to = to.min(text.len());
    if from < to {
        text.replace_range(from..to, "");
    }
}

/// Byte position where a blank line is inserted, plus where that new blank
/// line starts. `line_end` excludes the terminator, so "after" steps past an
/// existing `\n` (and the `\r` of CRLF) before inserting.
fn blank_line_position(
    text: &str,
    line_start: usize,
    line_end: usize,
    before: bool,
) -> (usize, usize) {
    if before {
        return (line_start, line_start);
    }
    let line_end = line_end.min(text.len());
    match text[line_end..].find('\n') {
        Some(offset) => {
            let insert_at = line_end + offset + 1;
            (insert_at, insert_at)
        }
        // Last line without a terminator: the inserted newline terminates the
        // current line, so the new blank line begins one byte later.
        None => (line_end, line_end + 1),
    }
}

fn insert_context_menu(
    menu: &mut egui::Ui,
    lang: Lang,
    at: usize,
    line: (usize, usize),
    action: &mut Option<(usize, InsertAction)>,
    picker: &mut Option<TablePicker>,
    clipboard: Option<&str>,
) {
    let zh = lang == Lang::ZhCn;
    let t = crate::i18n::tr(lang);
    // Rendered blocks are not selectable text boxes, so the clipboard actions
    // work on the whole block that was right-clicked: copy/cut its Markdown,
    // paste after it. The inline editor keeps its own text shortcuts and
    // passes `None` here.
    if let Some(contents) = clipboard {
        crate::editor::line_menu_style(menu, t.copy_lines, t.paste);
        let copy = crate::editor::line_menu_button(menu, t.copy_lines);
        #[cfg(test)]
        menu.ctx().data_mut(|data| {
            data.insert_temp(egui::Id::new(("markdown_menu_copy", at)), copy.rect)
        });
        if copy.clicked() {
            menu.ctx().copy_text(contents.to_owned());
            menu.close_menu();
        }
        let cut = crate::editor::line_menu_button(menu, t.cut);
        #[cfg(test)]
        menu.ctx().data_mut(|data| {
            data.insert_temp(egui::Id::new(("markdown_menu_cut", at)), cut.rect)
        });
        if cut.clicked() {
            menu.ctx().copy_text(contents.to_owned());
            *action = Some((
                at,
                InsertAction::Cut {
                    start: line.0,
                    end: line.1,
                },
            ));
            menu.close_menu();
        }
        let paste = crate::editor::line_menu_button(menu, t.paste);
        #[cfg(test)]
        menu.ctx().data_mut(|data| {
            data.insert_temp(egui::Id::new(("markdown_menu_paste", at)), paste.rect)
        });
        if paste.clicked() {
            *action = Some((at, InsertAction::Paste { at: line.1 }));
            menu.close_menu();
        }
        menu.separator();
    }
    let table_label = if zh { "插入表格…" } else { "Insert table…" };
    let callout_label = if zh { "插入警告框" } else { "Insert callout" };
    let blank_label = if zh { "插入空行" } else { "Insert blank line" };
    let before_label = if zh { "当前行前" } else { "Before current line" };
    let after_label = if zh { "当前行后" } else { "After current line" };
    crate::editor::line_menu_style(menu, table_label, blank_label);
    if let Some(stamp) = crate::insert_time::submenu(menu, t.insert_time, t.unix_timestamp) {
        *action = Some((at, InsertAction::InsertTime { at, stamp }));
        menu.close_menu();
    }
    let table = crate::editor::line_menu_button(menu, table_label);
    #[cfg(test)]
    menu.ctx().data_mut(|data| {
        data.insert_temp(egui::Id::new(("markdown_menu_table", at)), table.rect)
    });
    if table.clicked() {
        *picker = Some(TablePicker { at, rows: 3, columns: 3 });
        menu.close_menu();
    }
    let callout = menu.menu_button(callout_label, |menu| {
        for kind in CalloutKind::ALL {
            let button = crate::editor::line_menu_button(menu, kind.label(zh));
            #[cfg(test)]
            menu.ctx().data_mut(|data| {
                data.insert_temp(
                    egui::Id::new(("markdown_menu_callout", at, kind.marker())),
                    button.rect,
                )
            });
            if button.clicked() {
                *action = Some((at, InsertAction::Callout(kind)));
                menu.close_menu();
            }
        }
    });
    #[cfg(test)]
    menu.ctx().data_mut(|data| {
        data.insert_temp(
            egui::Id::new(("markdown_menu_callout_header", at)),
            callout.response.rect,
        )
    });
    let blank = menu.menu_button(blank_label, |menu| {
        let before = crate::editor::line_menu_button(menu, before_label);
        #[cfg(test)]
        menu.ctx().data_mut(|data| {
            data.insert_temp(egui::Id::new(("markdown_menu_blank_before", at)), before.rect);
            data.insert_temp(egui::Id::new("markdown_menu_blank_before_last"), before.rect);
        });
        if before.clicked() {
            *action = Some((
                at,
                InsertAction::BlankLine {
                    line_start: line.0,
                    line_end: line.1,
                    before: true,
                },
            ));
            menu.close_menu();
        }
        let after = crate::editor::line_menu_button(menu, after_label);
        #[cfg(test)]
        menu.ctx().data_mut(|data| {
            data.insert_temp(egui::Id::new(("markdown_menu_blank_after", at)), after.rect);
            data.insert_temp(egui::Id::new("markdown_menu_blank_after_last"), after.rect);
        });
        if after.clicked() {
            *action = Some((
                at,
                InsertAction::BlankLine {
                    line_start: line.0,
                    line_end: line.1,
                    before: false,
                },
            ));
            menu.close_menu();
        }
    });
    #[cfg(test)]
    menu.ctx().data_mut(|data| {
        data.insert_temp(
            egui::Id::new(("markdown_menu_blank_header", at)),
            blank.response.rect,
        );
        data.insert_temp(
            egui::Id::new("markdown_menu_blank_header_last"),
            blank.response.rect,
        );
    });
    let _ = (callout, blank);
}

fn plain_job(text: &str, font: FontId, color: Color32, width: f32) -> egui::text::LayoutJob {
    plain_job_highlighted(text, font, color, width, None, &crate::DARK)
}

/// Search-hit background: the accent hue at partial alpha, so it reads like a
/// text selection instead of a box drawn over the layout.
fn highlight_color(pal: &Palette) -> Color32 {
    let color = pal.accent;
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 92)
}

/// Apply the find-hit background to an already highlighted layout job,
/// splitting token sections at the hit boundaries.
fn highlight_sections(
    job: &mut egui::text::LayoutJob,
    range: Range<usize>,
    pal: &Palette,
) {
    let color = highlight_color(pal);
    let sections = std::mem::take(&mut job.sections);
    let mut out = Vec::with_capacity(sections.len() + 2);
    for section in sections {
        let start = section.byte_range.start;
        let end = section.byte_range.end;
        if end <= range.start || start >= range.end {
            out.push(section);
            continue;
        }
        for (from, to, hit) in [
            (start, range.start.min(end), false),
            (start.max(range.start), end.min(range.end), true),
            (range.end.max(start), end, false),
        ] {
            if from < to {
                let mut section = section.clone();
                section.byte_range = from..to;
                if hit {
                    section.format.background = color;
                }
                out.push(section);
            }
        }
    }
    job.sections = out;
}

/// Append one rendered run, splitting it when a source range only covers part
/// of it. Derived text (links, images) falls back to highlighting the run.
fn append_mapped(
    job: &mut egui::text::LayoutJob,
    rendered: &str,
    source: Range<usize>,
    mut format: egui::TextFormat,
    highlight: Option<&Range<usize>>,
    pal: &Palette,
) {
    let Some(hit) = highlight.filter(|hit| hit.start < source.end && source.start < hit.end) else {
        job.append(rendered, 0.0, format);
        return;
    };
    let start = hit.start.saturating_sub(source.start).min(rendered.len());
    let end = hit.end.saturating_sub(source.start).min(rendered.len());
    if start >= end || !rendered.is_char_boundary(start) || !rendered.is_char_boundary(end) {
        format.background = highlight_color(pal);
        job.append(rendered, 0.0, format);
        return;
    }
    if start > 0 {
        job.append(&rendered[..start], 0.0, format.clone());
    }
    let mut hot = format.clone();
    hot.background = highlight_color(pal);
    job.append(&rendered[start..end], 0.0, hot);
    if end < rendered.len() {
        job.append(&rendered[end..], 0.0, format);
    }
}

/// Plain text job with an optional highlighted range (in its own bytes). Code
/// blocks, tables and HTML use this so the matched characters themselves are
/// tinted.
fn plain_job_highlighted(
    text: &str,
    font: FontId,
    color: Color32,
    width: f32,
    highlight: Option<Range<usize>>,
    pal: &Palette,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = width.max(24.0);
    append_mapped(
        &mut job,
        text,
        0..text.len(),
        egui::TextFormat {
            font_id: font,
            color,
            ..Default::default()
        },
        highlight.as_ref(),
        pal,
    );
    job
}

fn inline_job(
    text: &str,
    font: FontId,
    color: Color32,
    pal: &Palette,
    width: f32,
    highlight: Option<Range<usize>>,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = width.max(24.0);
    push_inline(&mut job, text, &font, color, pal, highlight.as_ref());
    apply_body_line_height(&mut job, &font);
    job
}

/// Body line height, as a multiple of the body font size.
///
/// The bundled font's natural row height leaves barely a pixel between rows.
/// Inline code paints its background one point beyond the glyph box, so with
/// the natural height a code span on one line visually touches the line above;
/// an inline code span alone on a wrapped row also made that row shorter than
/// its neighbours because the monospace font has a smaller row height. A
/// slightly taller, uniform row fixes both without changing the text position
/// inside the row (egui bottom-aligns glyphs).
const BODY_LINE_HEIGHT: f32 = 1.35;

/// Give every body-size section the same line height so rows that contain only
/// inline code, emphasis or markers keep the height of ordinary prose rows.
/// Larger (heading) fonts and deliberately collapsed rows (a hidden fence)
/// keep their own metrics.
fn apply_body_line_height(job: &mut egui::text::LayoutJob, body: &FontId) {
    let height = body.size * BODY_LINE_HEIGHT;
    for section in &mut job.sections {
        if section.format.font_id.size <= body.size {
            section.format.line_height = Some(height);
        }
    }
}

fn push_inline(
    job: &mut egui::text::LayoutJob,
    text: &str,
    font: &FontId,
    color: Color32,
    pal: &Palette,
    highlight: Option<&Range<usize>>,
) {
    let base = egui::TextFormat {
        font_id: font.clone(),
        color,
        ..Default::default()
    };
    push_inline_fmt(job, text, &base, pal, highlight, 0);
}

/// Rendered inline Markdown on top of `base`. `origin` is the byte offset of
/// `text` inside the string the caller highlights, so nested markup (emphasis,
/// link labels) still maps find matches to the right characters.
fn push_inline_fmt(
    job: &mut egui::text::LayoutJob,
    text: &str,
    base: &egui::TextFormat,
    pal: &Palette,
    highlight: Option<&Range<usize>>,
    origin: usize,
) {
    // Consecutive plain characters share one layout section. Appending them
    // one by one used to create a section per character, which made the first
    // Markdown frame (and every resize) unnecessarily expensive.
    let mut plain_start = 0;
    let mut index = 0;
    while index < text.len() {
        let rest = &text[index..];
        if rest.starts_with('\\') && rest.len() > 1 {
            if plain_start < index {
                append_mapped(
                    job,
                    &text[plain_start..index],
                    origin + plain_start..origin + index,
                    base.clone(),
                    highlight,
                    pal,
                );
            }
            let next = rest[1..].chars().next().unwrap();
            append_mapped(
                job,
                &next.to_string(),
                origin + index..origin + index + 1 + next.len_utf8(),
                base.clone(),
                highlight,
                pal,
            );
            index += 1 + next.len_utf8();
            plain_start = index;
            continue;
        }
        if rest.starts_with("![")
            && let Some((consumed, label)) = link_parts(rest, "![")
        {
            if plain_start < index {
                append_mapped(
                    job,
                    &text[plain_start..index],
                    origin + plain_start..origin + index,
                    base.clone(),
                    highlight,
                    pal,
                );
            }
            let mut format = egui::TextFormat {
                font_id: base.font_id.clone(),
                color: pal.accent,
                ..Default::default()
            };
            if highlight.is_some_and(|hit| {
                hit.start < origin + index + consumed && origin + index < hit.end
            }) {
                format.background = highlight_color(pal);
            }
            job.append(&format!("[图片 {label}]"), 0.0, format);
            index += consumed;
            plain_start = index;
            continue;
        }
        if rest.starts_with('[')
            && let Some((consumed, label)) = link_parts(rest, "[")
        {
            if plain_start < index {
                append_mapped(
                    job,
                    &text[plain_start..index],
                    origin + plain_start..origin + index,
                    base.clone(),
                    highlight,
                    pal,
                );
            }
            let format = egui::TextFormat {
                font_id: base.font_id.clone(),
                color: pal.accent,
                underline: Stroke::new(1.0_f32, pal.accent),
                ..Default::default()
            };
            push_inline_fmt(job, label, &format, pal, highlight, origin + index + 1);
            index += consumed;
            plain_start = index;
            continue;
        }
        if rest.starts_with('`')
            && let Some((consumed, code_start, code_len)) = backtick_span(rest)
        {
            if plain_start < index {
                append_mapped(
                    job,
                    &text[plain_start..index],
                    origin + plain_start..origin + index,
                    base.clone(),
                    highlight,
                    pal,
                );
            }
            let code = &rest[code_start..code_start + code_len];
            let start = origin + index + code_start;
            append_mapped(
                job,
                code,
                start..start + code.len(),
                egui::TextFormat {
                    font_id: FontId::monospace((base.font_id.size - 1.0).max(10.0)),
                    color: pal.text_main,
                    background: pal.bg_elevated,
                    ..Default::default()
                },
                highlight,
                pal,
            );
            index += consumed;
            plain_start = index;
            continue;
        }
        if rest.starts_with("**") || rest.starts_with("__") {
            let marker = &rest[..2];
            if let Some((consumed, inner)) = delimited(rest, marker, marker) {
                if plain_start < index {
                    append_mapped(
                        job,
                        &text[plain_start..index],
                        origin + plain_start..origin + index,
                        base.clone(),
                        highlight,
                        pal,
                    );
                }
                let bold = egui::TextFormat {
                    font_id: FontId::new(
                        base.font_id.size,
                        FontFamily::Name("brand".into()),
                    ),
                    color: base.color,
                    ..Default::default()
                };
                push_inline_fmt(
                    job,
                    inner,
                    &bold,
                    pal,
                    highlight,
                    origin + index + marker.len(),
                );
                index += consumed;
                plain_start = index;
                continue;
            }
        }
        if rest.starts_with("~~")
            && let Some((consumed, inner)) = delimited(rest, "~~", "~~")
        {
            if plain_start < index {
                append_mapped(
                    job,
                    &text[plain_start..index],
                    origin + plain_start..origin + index,
                    base.clone(),
                    highlight,
                    pal,
                );
            }
            let strike = egui::TextFormat {
                font_id: base.font_id.clone(),
                color: pal.text_faint,
                strikethrough: Stroke::new(1.0_f32, pal.text_faint),
                ..Default::default()
            };
            push_inline_fmt(job, inner, &strike, pal, highlight, origin + index + 2);
            index += consumed;
            plain_start = index;
            continue;
        }
        if rest.starts_with('*') || rest.starts_with('_') {
            let marker = &rest[..1];
            if let Some((consumed, inner)) = delimited(rest, marker, marker) {
                if plain_start < index {
                    append_mapped(
                        job,
                        &text[plain_start..index],
                        origin + plain_start..origin + index,
                        base.clone(),
                        highlight,
                        pal,
                    );
                }
                let italic = egui::TextFormat {
                    font_id: base.font_id.clone(),
                    color: base.color,
                    italics: true,
                    ..Default::default()
                };
                push_inline_fmt(job, inner, &italic, pal, highlight, origin + index + 1);
                index += consumed;
                plain_start = index;
                continue;
            }
        }
        let next = rest.chars().next().unwrap();
        index += next.len_utf8();
    }
    if plain_start < text.len() {
        append_mapped(
            job,
            &text[plain_start..],
            origin + plain_start..origin + text.len(),
            base.clone(),
            highlight,
            pal,
        );
    }
}

/// Left inset used by the editing layout so entering edit mode keeps the
/// rendered text at the same horizontal position: list bullets, quote bars and
/// code panel padding all reserve space before the text.
fn editing_inset(kind: &Kind) -> i8 {
    match kind {
        Kind::List { .. } => 22,
        Kind::Quote | Kind::Callout(_) => 14,
        Kind::Code(_) => 10,
        Kind::Table => 8,
        _ => 0,
    }
}

/// Draw the list bullet or ordinal in the left inset while editing. The
/// source marker itself stays hidden so the item text keeps the same x as the
/// rendered block, but the number must remain visible.
fn paint_list_marker(
    ui: &egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    ordered: bool,
    marker: &str,
    body: f32,
    pal: &Palette,
) {
    paint_list_marker_at(ui, output.galley_pos, &output.galley, ordered, marker, body, pal);
}

fn paint_list_marker_at(
    ui: &egui::Ui,
    text_pos: egui::Pos2,
    text: &egui::Galley,
    ordered: bool,
    marker: &str,
    body: f32,
    pal: &Palette,
) {
    let Some(row) = text.rows.first() else { return; };
    let galley = ui.painter().layout_no_wrap(
        if ordered { marker.to_owned() } else { "•".to_owned() },
        FontId::proportional(body),
        pal.accent,
    );
    // Rendered lists draw the marker in a 4 px offset + 18 px slot and center
    // it; the editing galley starts after the 22 px inset, so the slot centre
    // sits 9 px left of the text (13 px right of the content origin).
    let pos = egui::pos2(
        text_pos.x - 9.0 - galley.size().x * 0.5,
        text_pos.y + row.rect.center().y - galley.size().y * 0.5,
    );
    ui.painter().galley(pos, galley, pal.accent);
}

fn code_lang_height(body: f32) -> f32 {
    // The language field uses a slightly larger font than body annotations, so
    // leave enough room for its pill without growing the row afterwards.
    (body * 1.2).max(20.0)
}

/// Result of drawing a code block's language row.
#[derive(Default)]
struct CodeLangRow {
    /// User-typed fence info and the block's opening-fence byte offset.
    changed: Option<(usize, String)>,
    /// Whether the field owned the pointer or focus this frame, so clicking it
    /// must not also switch the block into text editing.
    used: bool,
}

/// Full info string of an opening fence line, e.g. `rust,ignore` in
/// `````rust,ignore``.
fn fence_info_raw(line: &str) -> String {
    let trimmed = line.trim_start();
    let fence = trimmed
        .bytes()
        .take_while(|byte| *byte == b'`' || *byte == b'~')
        .count();
    if fence == 0 {
        return String::new();
    }
    trimmed[fence..].trim().to_owned()
}

/// Fence info is free-form user input: keep it on one line and strip leading
/// fence characters that would otherwise merge into the opening fence.
fn sanitize_fence_info(info: &str) -> String {
    let mut value: String = info
        .chars()
        .map(|character| match character {
            '\n' | '\r' => ' ',
            other => other,
        })
        .collect();
    value = value.trim().to_owned();
    while value.starts_with('`') || value.starts_with('~') {
        value.remove(0);
    }
    value.trim().to_owned()
}

/// Language row below a fenced code block, right-aligned and outside the code
/// box. The live view renders a small free-form field: typing a built-in name,
/// extension or alias applies that syntax, while unknown input stays plain
/// text. The read-only split preview keeps a plain label.
#[allow(clippy::too_many_arguments)]
fn code_language_row(
    ui: &mut egui::Ui,
    width: f32,
    file_id: u64,
    at: usize,
    info: &str,
    body: f32,
    pal: &Palette,
    editable: bool,
) -> CodeLangRow {
    let height = code_lang_height(body);
    let font = FontId::proportional((body * 0.8).max(10.0));
    // A minimum inner width keeps short labels (`txt`, `rust`, `swift`) the
    // same size while still growing for longer names.
    let min_width = (font.size * 4.0).max(44.0);
    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            ui.add_space(2.0);
            if editable {
                let id = egui::Id::new(("markdown_code_lang", file_id, at));
                let focused = ui
                    .ctx()
                    .read_response(id)
                    .is_some_and(|response| response.has_focus());
                // The pill hugs its text but never drops below the minimum, so
                // 3/4/5-character labels share one width. Longer names grow
                // until the reading column, then scroll inside the field.
                let hint = "txt";
                let displayed = if info.trim().is_empty() { hint } else { info };
                let text_width = ui
                    .painter()
                    .layout_no_wrap(displayed.to_owned(), font.clone(), pal.text_faint)
                    .size()
                    .x;
                let field_width = (text_width + 10.0)
                    .max(min_width)
                    .min((width - 12.0).max(min_width));
                let mut value = info.to_owned();
                let response = egui::Frame::new()
                    .fill(if focused { pal.bg_elevated } else { pal.bg_panel })
                    .corner_radius(4.0)
                    .inner_margin(egui::Margin::symmetric(6, 1))
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut value)
                                .id(id)
                                .font(font)
                                .text_color(if focused {
                                    pal.text_main
                                } else {
                                    pal.text_faint
                                })
                                .frame(false)
                                .margin(egui::Margin::ZERO)
                                .desired_width(field_width)
                                .horizontal_align(egui::Align::Center)
                                .hint_text(hint),
                        )
                    })
                    .inner;
                #[cfg(test)]
                ui.ctx().data_mut(|data| {
                    data.insert_temp(
                        egui::Id::new(("markdown_code_lang_rect", file_id, at)),
                        response.rect,
                    )
                });
                let changed = response
                    .changed()
                    .then(|| sanitize_fence_info(&value))
                    .filter(|value| value != info)
                    .map(|value| (at, value));
                CodeLangRow {
                    changed,
                    used: response.has_focus() || response.clicked(),
                }
            } else {
                let label = if info.trim().is_empty() {
                    "txt".to_owned()
                } else {
                    info.to_owned()
                };
                let galley = ui.painter().layout_no_wrap(label, font, pal.text_faint);
                let size = egui::vec2(galley.size().x.max(min_width - 12.0) + 14.0, height);
                let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                ui.painter().rect_filled(rect, 4.0, pal.bg_panel);
                ui.painter()
                    .galley(rect.center() - galley.size() * 0.5, galley, pal.text_faint);
                CodeLangRow::default()
            }
        },
    )
    .inner
}

/// Replace a fenced block's info string, returning the byte-length delta.
fn set_code_language(text: &mut String, at: usize, language: &str) -> Option<isize> {
    let at = at.min(text.len());
    let end = text[at..]
        .find('\n')
        .map_or(text.len(), |index| at + index);
    let line = text[at..end].to_owned();
    let trimmed = line.trim_start();
    let fence = fence_marker(trimmed)?;
    let indent = &line[..line.len() - trimmed.len()];
    let language = sanitize_fence_info(language);
    if fence_info_raw(trimmed) == language {
        return Some(0);
    }
    let new_line = format!("{indent}{fence}{language}");
    let delta = new_line.len() as isize - line.len() as isize;
    text.replace_range(at..end, &new_line);
    Some(delta)
}

/// What Enter should do while editing a list or quote line. Returns the text
/// to insert and whether the current empty item should be removed instead.
fn editing_enter_action(line: &str) -> Option<(String, bool)> {
    let trimmed = line.trim_start();
    let indent = &line[..line.len() - trimmed.len()];
    if let Some((ordered, marker)) = list_marker(trimmed) {
        let content = trimmed[marker.len()..].trim_start();
        if content.is_empty() {
            return Some((String::new(), true));
        }
        let marker = if ordered {
            next_list_marker(&marker)
        } else {
            marker
        };
        let marker = if content.starts_with("[ ] ")
            || content.starts_with("[x] ")
            || content.starts_with("[X] ")
        {
            format!("{marker} [ ]")
        } else {
            marker
        };
        return Some((format!("\n{indent}{marker} "), false));
    }
    if let Some(after) = trimmed.strip_prefix('>') {
        if after.trim().is_empty() {
            return Some((String::new(), true));
        }
        return Some((format!("\n{indent}> "), false));
    }
    None
}

fn next_list_marker(marker: &str) -> String {
    let digits = marker.bytes().take_while(u8::is_ascii_digit).count();
    let (number, rest) = marker.split_at(digits);
    match number.parse::<u32>() {
        Ok(number) => format!("{}{}", number + 1, rest),
        Err(_) => marker.to_owned(),
    }
}

/// Spacing around the active block while it is being edited. Kept close to the
/// rendered block so switching into editing does not make the page jump.
fn editing_space(kind: &Kind, body: f32) -> (f32, f32) {
    match kind {
        Kind::Heading(level) => heading_space(*level, body),
        Kind::Paragraph => (0.0, body * 0.62),
        Kind::List { .. } => (0.0, 4.0),
        Kind::Quote => (6.0, 8.0),
        // The code frame carries its own 6 px top and 12 px bottom padding,
        // matching the rendered box exactly.
        Kind::Code(_) => (0.0, 0.0),
        Kind::Table => (5.0, 6.0),
        Kind::Html => (0.0, 6.0),
        _ => (0.0, 0.0),
    }
}

/// Kind of every source line, used to keep the active block's raw text styled
/// exactly like its rendered counterpart.
fn editing_line_kinds(text: &str) -> Vec<Kind> {
    let mut kinds = Vec::new();
    for block in parse(text) {
        while kinds.len() < block.line_start {
            kinds.push(Kind::Blank);
        }
        for _ in 0..block.line_count {
            kinds.push(block.kind.clone());
        }
    }
    kinds
}

/// Syntax-highlighted body of a code block while it is being edited. The
/// highlight job is built once for the whole block so multi-line constructs
/// keep their parser context, then spliced into the edit job line by line.
struct EditingCode {
    job: egui::text::LayoutJob,
    /// Byte offset of the next body line inside `job.text`.
    offset: usize,
    /// First section that may still overlap the next line.
    cursor: usize,
    /// Format for characters no token covers (newlines, indentation gaps).
    fallback: egui::TextFormat,
}

impl EditingCode {
    fn new(body: &str, language: &str, font: &FontId, width: f32, pal: &Palette) -> Self {
        let font = FontId::monospace((font.size - 1.0).max(9.0));
        let fallback = egui::TextFormat {
            font_id: font.clone(),
            color: pal.text_main,
            ..Default::default()
        };
        let job = crate::workspace_theme::code_job(body, language, font, width, pal.dark);
        Self {
            job,
            offset: 0,
            cursor: 0,
            fallback,
        }
    }

    /// Append one source line, reusing the block's syntax sections so colors
    /// stay visible while editing.
    fn push_line(&mut self, job: &mut egui::text::LayoutJob, line: &str) {
        let text_len = self.job.text.len();
        if self.offset >= text_len {
            job.append(line, 0.0, self.fallback.clone());
            return;
        }
        let start = self.offset;
        let end = (start + line.len()).min(text_len);
        while self.cursor < self.job.sections.len()
            && self.job.sections[self.cursor].byte_range.end <= start
        {
            self.cursor += 1;
        }
        let mut at = start;
        let mut index = self.cursor;
        while index < self.job.sections.len() {
            let section = &self.job.sections[index];
            if section.byte_range.start >= end {
                break;
            }
            let from = section.byte_range.start.max(at);
            let to = section.byte_range.end.min(end);
            if from > at {
                job.append(&self.job.text[at..from], 0.0, section.format.clone());
            }
            if from < to {
                job.append(&self.job.text[from..to], 0.0, section.format.clone());
                at = to;
            }
            if at >= end {
                break;
            }
            index += 1;
        }
        self.cursor = index;
        if at < end {
            job.append(&self.job.text[at..end], 0.0, self.fallback.clone());
        }
        // Body lines are joined with a single newline in `code_body`.
        self.offset = end + 1;
    }
}

/// Body of a fenced block, given the line range between its fences.
/// Mirrors `code_body` for the raw segment slices.
fn code_body_between(segments: &[&str], start: usize, end: usize) -> String {
    segments[start..end]
        .iter()
        .map(|segment| {
            let without_newline = segment.strip_suffix('\n').unwrap_or(segment);
            without_newline.strip_suffix('\r').unwrap_or(without_newline)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The TextEdit owns only visible code, never the fence rows. Keep the raw
/// envelope in `Editing.text` so language edits and document commits retain
/// their source spelling (including CRLF and the closing fence's newline).
struct CodeBodyEdit {
    range: Range<usize>,
    text: String,
    crlf: bool,
}

impl CodeBodyEdit {
    fn new(raw: &str) -> Self {
        let start = raw.find('\n').map_or(raw.len(), |at| at + 1);
        let last_end = raw.strip_suffix('\n').unwrap_or(raw).len();
        let last_start = raw[..last_end].rfind('\n').map_or(0, |at| at + 1);
        let closed = last_start >= start
            && fence_marker(raw[last_start..last_end].trim()).is_some();
        let mut end = if closed { last_start } else { raw.len() };
        // The newline separating body from closing fence is structural, not
        // an extra editable blank row. Actual trailing blank body rows remain.
        if closed && end > start && raw.as_bytes()[end - 1] == b'\n' {
            end -= 1;
            if end > start && raw.as_bytes()[end - 1] == b'\r' { end -= 1; }
        }
        Self {
            range: start..end,
            text: raw[start..end].replace("\r\n", "\n"),
            crlf: raw[..start].ends_with("\r\n"),
        }
    }

    fn raw_byte(&self, raw: &str, chars: usize) -> usize {
        let body = &raw[self.range.clone()];
        let mut count = 0;
        for (byte, ch) in body.char_indices() {
            if count == chars { return self.range.start + byte; }
            if ch == '\r' && body[byte..].starts_with("\r\n") { continue; }
            count += 1;
        }
        self.range.end
    }

    fn body_cursor(&self, raw: &str, raw_chars: usize) -> usize {
        let byte = char_to_byte(raw, raw_chars).clamp(self.range.start, self.range.end);
        raw[self.range.start..byte].replace("\r\n", "\n").chars().count()
    }

    fn commit(&self, raw: &mut String, edited: &str) {
        // Replace only the changed span: untouched source, even mixed line
        // endings, remains byte-for-byte intact. This also handles TextEdit undo.
        let prefix = self.text.chars().zip(edited.chars()).take_while(|(a, b)| a == b).count();
        let suffix = self.text.chars().rev().take(self.text.chars().count() - prefix)
            .zip(edited.chars().rev().take(edited.chars().count() - prefix))
            .take_while(|(a, b)| a == b).count();
        let old_end = self.text.chars().count() - suffix;
        let new_start = char_to_byte(edited, prefix);
        let new_end = char_to_byte(edited, edited.chars().count() - suffix);
        let replacement = if self.crlf {
            edited[new_start..new_end].replace('\n', "\r\n")
        } else { edited[new_start..new_end].to_owned() };
        let range = self.raw_byte(raw, prefix)..self.raw_byte(raw, old_end);
        let needs_separator = self.range.is_empty() && !edited.is_empty()
            && !raw[self.range.end..].starts_with(['\r', '\n'])
            && fence_marker(raw[self.range.end..].trim()).is_some();
        let replacement = if needs_separator {
            format!("{replacement}{}", if self.crlf { "\r\n" } else { "\n" })
        } else { replacement };
        raw.replace_range(range, &replacement);
    }
}

fn code_body_edit_job(text: &str, language: &str, body: f32, width: f32, pal: &Palette) -> egui::text::LayoutJob {
    let font = FontId::monospace((body - 1.0).max(9.0));
    let mut job = crate::workspace_theme::code_job(text, language, font.clone(), width, pal.dark);
    if text.is_empty() {
        job.append("", 0.0, egui::TextFormat { font_id: font, color: pal.text_main, ..Default::default() });
    }
    job
}

/// Result of completing a fence opener typed in the live editor.
struct FenceShortcut {
    /// Whole replacement for the block being edited.
    replacement: String,
    /// Byte offset of the code block inside `replacement`.
    code_at: usize,
    /// The complete fenced block (`open`, blank body row, `close`).
    code_text: String,
    /// Character offset inside `code_text` where the caret belongs.
    cursor: usize,
}

/// Detect a typed fence opener confirmed by a trailing space or by Enter,
/// e.g. ```` ```rust ```` or ```` ```rust `<enter>`. The confirmation is the
/// user's signal that the info string is complete, so it is safe to finish the
/// block and put the caret inside.
///
/// `cursor` is the caret's byte position inside `text`. Completion only fires
/// when the whole line is just the fence plus an optional info string and the
/// caret sits at the end of it (space) or at the start of the line created by
/// Enter, so ordinary prose that starts with backticks is left alone.
fn fence_shortcut(text: &str, cursor: usize) -> Option<FenceShortcut> {
    let cursor = cursor.min(text.len());
    if !text.is_char_boundary(cursor) {
        return None;
    }
    let line_start = text[..cursor].rfind('\n').map_or(0, |index| index + 1);
    let line_end = text[cursor..]
        .find('\n')
        .map_or(text.len(), |index| cursor + index);
    // A trailing space confirms the opener in place. Enter instead leaves the
    // caret at the start of the next line, with the opener just above it.
    let (opener_start, opener_end) = if cursor == line_end && text[..cursor].ends_with(' ') {
        (line_start, line_end)
    } else if cursor == line_start && line_start > 0 {
        let opener_end = line_start - 1;
        let opener_start = text[..opener_end]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        (opener_start, opener_end)
    } else {
        return None;
    };
    let raw_line = &text[opener_start..opener_end];
    let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
    let trimmed = line.trim_end_matches([' ', '\t']);
    let indent = &trimmed[..trimmed.len() - trimmed.trim_start_matches([' ', '\t']).len()];
    // List/quote indentation stays put; deeper indentation is ordinary prose.
    if indent.len() > 3 {
        return None;
    }
    let body = &trimmed[indent.len()..];
    let fence = if body.starts_with("```") {
        "```"
    } else if body.starts_with("~~~") {
        "~~~"
    } else {
        return None;
    };
    // A fence already present before this line means the user is editing an
    // existing block, not opening a new one.
    if text[..opener_start]
        .lines()
        .any(|line| fence_marker(line.trim()).is_some())
    {
        return None;
    }
    let info = body[fence.len()..].trim();
    if info.contains(['`', '~']) {
        return None;
    }
    let open = format!("{indent}{fence}{info}");
    let code_text = format!("{open}\n\n{indent}{fence}");
    let before = &text[..opener_start];
    let after = &text[cursor..];
    let mut replacement = String::with_capacity(before.len() + code_text.len() + after.len() + 1);
    replacement.push_str(before);
    let code_at = replacement.len();
    replacement.push_str(&code_text);
    // Pressing Enter already consumed the line break that separated the opener
    // from the caret; keep any following content on its own line.
    if after.is_empty() || !after.starts_with('\n') {
        replacement.push('\n');
    }
    replacement.push_str(after);
    let cursor = code_text[..open.len() + 1].chars().count();
    Some(FenceShortcut {
        replacement,
        code_at,
        code_text,
        cursor,
    })
}

/// Lay out the raw Markdown of the block under edit. Every source character is
/// preserved so `TextEdit` keeps a correct caret/selection/IME mapping; only
/// the presentation changes: rendered fonts, colors and backgrounds, with the
/// syntax markers dimmed instead of removed.
fn editing_job(
    text: &str,
    font: &FontId,
    color: Color32,
    pal: &Palette,
    width: f32,
) -> egui::text::LayoutJob {
    editing_job_for(text, font, color, pal, width, true)
}

/// `code_block` says whether editing started on a code block. While the user
/// is still typing an opener into a paragraph, the incomplete fence must stay
/// visible instead of being hidden as if it were an existing code block.
fn editing_job_for(
    text: &str,
    font: &FontId,
    color: Color32,
    pal: &Palette,
    width: f32,
    code_block: bool,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = width.max(24.0);
    let base = egui::TextFormat {
        font_id: font.clone(),
        color,
        // Body rows use the same taller line height as the rendered view, so
        // entering and leaving the inline editor does not move any text.
        line_height: Some(font.size * BODY_LINE_HEIGHT),
        ..Default::default()
    };
    if text.is_empty() {
        // An empty job otherwise falls back to egui's default text format,
        // which gives the caret a different height from this Markdown row.
        job.append("", 0.0, base.clone());
    }
    let kinds = editing_line_kinds(text);
    let segments: Vec<&str> = text.split_inclusive('\n').collect();
    let mut code: Option<EditingCode> = None;
    let mut code_close: Option<usize> = None;
    for (line_index, segment) in segments.iter().enumerate() {
        let without_newline = segment.strip_suffix('\n').unwrap_or(segment);
        let (line, carriage) = match without_newline.strip_suffix('\r') {
            Some(line) => (line, true),
            None => (without_newline, false),
        };
        let mut kind = kinds.get(line_index).cloned().unwrap_or(Kind::Paragraph);
        if !code_block && matches!(kind, Kind::Code(_)) {
            // An unterminated fence typed into a paragraph is still being
            // written; keep its backticks visible until the shortcut fires.
            kind = Kind::Paragraph;
        }
        let mut newline = base.clone();
        if let Kind::Code(language) = &kind {
            let block_start = line_index == 0
                || !matches!(kinds.get(line_index - 1), Some(Kind::Code(_)));
            if block_start {
                // Opening fence: find this block's last line so interior
                // fence-looking text stays part of the code body.
                let mut end = line_index + 1;
                while end < segments.len() && matches!(kinds.get(end), Some(Kind::Code(_))) {
                    end += 1;
                }
                let last = segments.get(end.saturating_sub(1)).copied().unwrap_or("");
                let closing = (end > line_index + 1
                    && fence_marker(
                        last.strip_suffix('\n')
                            .unwrap_or(last)
                            .strip_suffix('\r')
                            .unwrap_or(last)
                            .trim(),
                    )
                    .is_some())
                .then_some(end - 1);
                let body_end = closing.unwrap_or(end);
                let body = code_body_between(&segments, line_index + 1, body_end);
                code = Some(EditingCode::new(
                    &body,
                    language.as_deref().unwrap_or(""),
                    font,
                    width,
                    pal,
                ));
                code_close = closing;
                job.append(line, 0.0, hidden_line_format(&base));
                newline = hidden_line_format(&base);
            } else if Some(line_index) == code_close {
                // Closing fence: hidden, and ends the highlighted body.
                job.append(line, 0.0, hidden_line_format(&base));
                code = None;
                code_close = None;
                newline = hidden_line_format(&base);
            } else {
                let state = code.as_mut().expect("a code block starts with a fence");
                state.push_line(&mut job, line);
                // Empty code rows still need the monospace row height.
                newline = state.fallback.clone();
            }
        } else if !code_block && fence_marker(line.trim()).is_some() {
            // An opener being typed into a paragraph is plain text until the
            // shortcut completes it; keep every backtick visible.
            job.append(line, 0.0, base.clone());
        } else {
            push_editing_line(&mut job, line, &base, pal, &kind);
        }
        if carriage {
            job.append("\r", 0.0, newline.clone());
        }
        if without_newline.len() < segment.len() {
            job.append("\n", 0.0, newline);
        }
    }
    job
}

fn push_editing_line(
    job: &mut egui::text::LayoutJob,
    line: &str,
    base: &egui::TextFormat,
    pal: &Palette,
    kind: &Kind,
) {
    if line.is_empty() {
        return;
    }
    let lead = line.len() - line.trim_start().len();
    match kind {
        Kind::Heading(level) => {
            let trimmed = &line[lead..];
            let hashes = trimmed.bytes().take_while(|byte| *byte == b'#').count();
            if hashes == 0
                || hashes != *level as usize
                || !trimmed[hashes..].starts_with(' ')
            {
                push_inline_editing(job, line, base, pal);
                return;
            }
            job.append(&line[..lead + hashes], 0.0, marker_format(base, pal, true));
            let after_hashes = &line[lead + hashes..];
            let spaces = after_hashes.len() - after_hashes.trim_start().len();
            job.append(
                &after_hashes[..spaces],
                0.0,
                marker_format(base, pal, true),
            );
            let content_all = &after_hashes[spaces..];
            let content = content_all.trim_end().trim_end_matches('#').trim_end();
            let heading = egui::TextFormat {
                font_id: heading_font(*level, base.font_id.size),
                color: base.color,
                ..Default::default()
            };
            push_inline_editing(job, content, &heading, pal);
            if content.len() < content_all.len() {
                job.append(
                    &content_all[content.len()..],
                    0.0,
                    marker_format(base, pal, true),
                );
            }
        }
        Kind::List { .. } => {
            let trimmed = &line[lead..];
            let Some((_, marker)) = list_marker(trimmed) else {
                push_inline_editing(job, line, base, pal);
                return;
            };
            let marker_end = lead + marker.len();
            let after_marker = &line[marker_end..];
            let spaces = after_marker.len() - after_marker.trim_start().len();
            let mut format = marker_format(base, pal, false);
            format.color = pal.accent.gamma_multiply(0.9);
            job.append(&line[..marker_end + spaces], 0.0, format);
            push_inline_editing(job, &line[marker_end + spaces..], base, pal);
        }
        Kind::Quote | Kind::Callout(_) => {
            let trimmed = &line[lead..];
            let Some(after) = trimmed.strip_prefix('>') else {
                push_inline_editing(job, line, base, pal);
                return;
            };
            let spaces = after.len() - after.trim_start().len();
            let prefix_end = lead + 1 + spaces;
            let mut format = marker_format(base, pal, false);
            format.color = pal.accent.gamma_multiply(0.8);
            job.append(&line[..prefix_end], 0.0, format);
            let mut quote = base.clone();
            quote.color = pal.text_faint;
            push_inline_editing(job, &line[prefix_end..], &quote, pal);
        }
        Kind::Code(_) => {
            let mut format = base.clone();
            format.font_id = FontId::monospace((base.font_id.size - 1.0).max(9.0));
            if fence_marker(line.trim()).is_some() {
                // Fences disappear entirely while editing: no markers and no
                // extra blank rows, so the code stays where it was rendered.
                format = hidden_line_format(&format);
            }
            job.append(line, 0.0, format);
        }
        Kind::Table => {
            let mut format = base.clone();
            format.font_id = FontId::monospace((base.font_id.size - 1.5).max(9.0));
            // Rendered tables use the monospace row height; keep the editing
            // box the same height.
            format.line_height = None;
            job.append(line, 0.0, format);
        }
        Kind::Rule => {
            // Rendered rules are a fixed 16 pt row; keep the editing row the
            // same height instead of inheriting the taller body line height.
            let mut format = marker_format(base, pal, false);
            format.line_height = Some(16.0);
            job.append(line, 0.0, format);
        }
        Kind::Html => {
            let mut format = base.clone();
            format.font_id = FontId::monospace((base.font_id.size - 1.5).max(9.0));
            format.line_height = None;
            job.append(line, 0.0, format);
        }
        Kind::Blank | Kind::Paragraph | Kind::Image { .. } => {
            push_inline_editing(job, line, base, pal);
        }
    }
}

fn hidden_line_format(base: &egui::TextFormat) -> egui::TextFormat {
    let mut format = base.clone();
    format.color = egui::Color32::TRANSPARENT;
    format.font_id.size = 0.1;
    format.line_height = Some(0.1);
    format
}

fn marker_format(
    base: &egui::TextFormat,
    _pal: &Palette,
    _small: bool,
) -> egui::TextFormat {
    // Markers stay in the buffer so caret, selection and IME mapping remain
    // exact, but they take no visible width. Otherwise entering edit mode
    // pushes the rendered text to the right by the marker width.
    let mut format = base.clone();
    format.color = egui::Color32::TRANSPARENT;
    format.font_id.size = 0.1;
    // Keep a normal body row height even when a line contains only markers.
    format.line_height = Some(base.font_id.size * BODY_LINE_HEIGHT);
    format
}

fn push_inline_editing(
    job: &mut egui::text::LayoutJob,
    text: &str,
    base: &egui::TextFormat,
    pal: &Palette,
) {
    let mut plain_start = 0;
    let mut index = 0;
    while index < text.len() {
        let rest = &text[index..];
        if rest.starts_with('\\') && rest.len() > 1 {
            job.append(&text[plain_start..index], 0.0, base.clone());
            let next = rest[1..].chars().next().unwrap();
            job.append("\\", 0.0, marker_format(base, pal, true));
            job.append(&next.to_string(), 0.0, base.clone());
            index += 1 + next.len_utf8();
            plain_start = index;
            continue;
        }
        if rest.starts_with("![")
            && let Some((consumed, label)) = link_parts(rest, "![")
        {
            job.append(&text[plain_start..index], 0.0, base.clone());
            job.append("![", 0.0, marker_format(base, pal, true));
            let mut label_format = base.clone();
            label_format.color = pal.accent;
            label_format.underline = Stroke::new(1.0_f32, pal.accent);
            job.append(label, 0.0, label_format);
            let rest_of_markup = &text[index + 2 + label.len()..index + consumed];
            job.append(rest_of_markup, 0.0, marker_format(base, pal, true));
            index += consumed;
            plain_start = index;
            continue;
        }
        if rest.starts_with('[')
            && let Some((consumed, label)) = link_parts(rest, "[")
        {
            job.append(&text[plain_start..index], 0.0, base.clone());
            job.append("[", 0.0, marker_format(base, pal, true));
            let mut label_format = base.clone();
            label_format.color = pal.accent;
            label_format.underline = Stroke::new(1.0_f32, pal.accent);
            push_inline_editing(job, label, &label_format, pal);
            let rest_of_markup = &text[index + 1 + label.len()..index + consumed];
            job.append(rest_of_markup, 0.0, marker_format(base, pal, true));
            index += consumed;
            plain_start = index;
            continue;
        }
        if rest.starts_with('`')
            && let Some((consumed, code_start, code_len)) = backtick_span(rest)
        {
            job.append(&text[plain_start..index], 0.0, base.clone());
            job.append(&rest[..code_start], 0.0, marker_format(base, pal, true));
            let mut code_format = base.clone();
            code_format.font_id = FontId::monospace((base.font_id.size - 1.0).max(9.0));
            code_format.background = pal.bg_elevated;
            job.append(
                &rest[code_start..code_start + code_len],
                0.0,
                code_format,
            );
            job.append(
                &rest[code_start + code_len..consumed],
                0.0,
                marker_format(base, pal, true),
            );
            index += consumed;
            plain_start = index;
            continue;
        }
        if (rest.starts_with("**") || rest.starts_with("__"))
            && let Some((consumed, inner)) = delimited(rest, &rest[..2], &rest[..2])
        {
            job.append(&text[plain_start..index], 0.0, base.clone());
            job.append(&rest[..2], 0.0, marker_format(base, pal, true));
            let mut bold = base.clone();
            bold.font_id = FontId::new(base.font_id.size, FontFamily::Name("brand".into()));
            push_inline_editing(job, inner, &bold, pal);
            job.append(&rest[..2], 0.0, marker_format(base, pal, true));
            index += consumed;
            plain_start = index;
            continue;
        }
        if rest.starts_with("~~")
            && let Some((consumed, inner)) = delimited(rest, "~~", "~~")
        {
            job.append(&text[plain_start..index], 0.0, base.clone());
            job.append("~~", 0.0, marker_format(base, pal, true));
            let mut strike = base.clone();
            strike.color = pal.text_faint;
            strike.strikethrough = Stroke::new(1.0_f32, pal.text_faint);
            push_inline_editing(job, inner, &strike, pal);
            job.append("~~", 0.0, marker_format(base, pal, true));
            index += consumed;
            plain_start = index;
            continue;
        }
        if (rest.starts_with('*') || rest.starts_with('_'))
            && let Some((consumed, inner)) = delimited(rest, &rest[..1], &rest[..1])
        {
            job.append(&text[plain_start..index], 0.0, base.clone());
            job.append(&rest[..1], 0.0, marker_format(base, pal, true));
            let mut italic = base.clone();
            italic.italics = true;
            push_inline_editing(job, inner, &italic, pal);
            job.append(&rest[..1], 0.0, marker_format(base, pal, true));
            index += consumed;
            plain_start = index;
            continue;
        }
        index += rest.chars().next().unwrap().len_utf8();
    }
    if plain_start < text.len() {
        job.append(&text[plain_start..], 0.0, base.clone());
    }
}

/// Translate a rendered cell cursor through the source-preserving inline
/// parser, never through hidden delimiters or link destinations.
fn table_visible_cursor_byte(raw: &str, cursor: usize, body: f32, pal: &Palette) -> usize {
    let mut job = egui::text::LayoutJob::default();
    let base = egui::TextFormat {
        font_id: FontId::proportional(body), color: pal.text_main, ..Default::default()
    };
    push_inline_editing(&mut job, raw, &base, pal);
    debug_assert_eq!(job.text, raw);
    let mut visible = 0;
    let mut end = 0;
    for section in &job.sections {
        if section.format.color == Color32::TRANSPARENT { continue; }
        for (byte, ch) in raw[section.byte_range.clone()].char_indices() {
            let byte = section.byte_range.start + byte;
            if visible == cursor { return byte; }
            visible += 1;
            end = byte + ch.len_utf8();
        }
    }
    end
}

/// Map a click inside a rendered block back to a character offset in its raw
/// Markdown. The editing layout keeps every source character, so a galley hit
/// test is enough.
fn editing_cursor_from_click(
    ui: &egui::Ui,
    raw: &str,
    relative: egui::Vec2,
    kind: &Kind,
    content: f32,
    body: f32,
    pal: &Palette,
) -> usize {
    if matches!(kind, Kind::Table) {
        let layout = table_layout(ui, raw, body, pal, None);
        if layout.rows.is_empty() { return 0; }
        let row = (((relative.y - 5.0).max(0.0) / layout.row_height) as usize).min(layout.rows.len() - 1);
        let mut x = 0.0;
        for (column, cell) in layout.rows[row].iter().enumerate() {
            let right = x + layout.columns[column];
            if relative.x <= right || column + 1 == layout.columns.len() {
                let free = (layout.columns[column] - 20.0 - cell.galley.size().x).max(0.0);
                let aligned = match layout.align[column] { egui::Align::Center => free * 0.5, egui::Align::Max => free, _ => 0.0 };
                let cursor = cell.galley.cursor_from_pos(egui::vec2(relative.x - x - 10.0 - aligned, 0.0)).ccursor.index;
                // The source-preserving inline job identifies hidden syntax
                // explicitly. Matching characters greedily against raw text can
                // otherwise land in a hidden URL containing the same words.
                let cell_raw = &raw[cell.source.clone()];
                let byte = table_visible_cursor_byte(cell_raw, cursor, body, pal);
                return raw[..cell.source.start + byte].chars().count();
            }
            x = right;
        }
        return 0;
    }
    let (top, _) = editing_space(kind, body);
    let (width, inset) = match kind {
        Kind::Quote | Kind::Callout(_) => (
            (content - 28.0).max(24.0),
            egui::vec2(14.0, top),
        ),
        Kind::Code(_) => (
            (content - 20.0).max(24.0),
            egui::vec2(10.0, top + 6.0),
        ),
        Kind::Table => (
            (content - 16.0).max(24.0),
            egui::vec2(8.0, top),
        ),
        Kind::List { .. } => ((content - 22.0).max(24.0), egui::vec2(22.0, top)),
        _ => (content, egui::vec2(0.0, top)),
    };
    let relative = egui::vec2(relative.x.max(0.0), relative.y.max(0.0));
    let relative = egui::vec2(
        (relative.x - inset.x).max(0.0),
        (relative.y - inset.y).max(0.0),
    );
    let code_body = matches!(kind, Kind::Code(_)).then(|| CodeBodyEdit::new(raw));
    let job = if let Some(code) = &code_body {
        code_body_edit_job(&code.text, &fence_info_raw(raw.lines().next().unwrap_or("")), body, width, pal)
    } else { editing_job(raw, &FontId::proportional(body), pal.text_main, pal, width) };
    let galley = ui.fonts(|fonts| fonts.layout_job(job));
    let relative = if matches!(kind, Kind::Heading(_)) {
        let centered = ((width - galley.size().x) * 0.5).max(0.0);
        egui::vec2((relative.x - centered).max(0.0), relative.y)
    } else {
        relative
    };
    let cursor = galley.cursor_from_pos(relative).ccursor.index;
    code_body.map_or(cursor, |code| raw[..code.raw_byte(raw, cursor)].chars().count())
}

/// Returns the consumed byte length and the inner text for a paired delimiter.
fn delimited<'a>(text: &'a str, open: &str, close: &str) -> Option<(usize, &'a str)> {
    let rest = text.strip_prefix(open)?;
    let end = rest.find(close)?;
    Some((open.len() + end + close.len(), &rest[..end]))
}

/// Backtick run at the start of `text` and the content it encloses, as
/// `(consumed, content_start, content_len)`.
///
/// A lone backtick closes at the next backtick; runs of two or more close at a
/// run of the same length. An unmatched run is ordinary text: it stays visible
/// so typing `` ``` `` or a literal pair never makes characters vanish.
fn backtick_span(text: &str) -> Option<(usize, usize, usize)> {
    let open = text.bytes().take_while(|byte| *byte == b'`').count();
    if open == 0 {
        return None;
    }
    let mut offset = open;
    while offset < text.len() {
        let Some(index) = text[offset..].find('`') else {
            break;
        };
        let start = offset + index;
        let run = text[start..]
            .bytes()
            .take_while(|byte| *byte == b'`')
            .count();
        if open == 1 {
            // A lone backtick closes at the first backtick; any further
            // backticks on the line are handled as their own run.
            return Some((start + 1, open, start - open));
        }
        if run == open {
            return Some((start + run, open, start - open));
        }
        offset = start + run;
    }
    None
}

/// `[label](target)` and `![alt](source)`: keep the label, consume the target.
fn link_parts<'a>(text: &'a str, open: &str) -> Option<(usize, &'a str)> {
    let (mut consumed, label) = delimited(text, open, "]")?;
    if text[consumed..].starts_with('(')
        && let Some(end) = text[consumed..].find(')')
    {
        consumed += end + 1;
    }
    Some((consumed, label))
}

fn galley_height(ui: &egui::Ui, job: egui::text::LayoutJob) -> f32 {
    ui.fonts(|fonts| fonts.layout_job(job)).size().y
}

fn galley_width(ui: &egui::Ui, job: egui::text::LayoutJob) -> f32 {
    ui.fonts(|fonts| fonts.layout_job(job)).size().x
}

/// Natural width of a block when its content must not wrap. Only code, tables
/// and HTML use this; prose keeps wrapping at the reading-column width.
fn block_natural_width(
    ui: &egui::Ui,
    block: &Block,
    raw: &str,
    pal: &Palette,
    body: f32,
) -> f32 {
    match &block.kind {
        Kind::Code(_) => {
            galley_width(
                ui,
                plain_job(
                    &code_body(raw),
                    FontId::monospace(body - 1.0),
                    pal.text_main,
                    f32::INFINITY,
                ),
            ) + 20.0
        }
        Kind::Table => table_layout(ui, raw, body, pal, None).width(),
        Kind::Html => galley_width(
            ui,
            plain_job(
                raw.trim(),
                FontId::monospace(body - 1.5),
                pal.text_faint,
                f32::INFINITY,
            ),
        ),
        _ => 0.0,
    }
}

/// Widest line drawn by a non-wrapping block, including the inset used when
/// the text is placed inside its background panel.
fn natural_width(ui: &egui::Ui, blocks: &[Block], text: &str, pal: &Palette, body: f32) -> f32 {
    blocks
        .iter()
        .map(|block| block_natural_width(ui, block, &text[block.range.clone()], pal, body))
        .fold(0.0_f32, f32::max)
}

type Images = std::collections::HashMap<String, ImageEntry>;

/// Directory that relative Markdown image paths are resolved against.
fn image_base(file: &LoadedFile) -> PathBuf {
    let path = file.source_path.as_deref().unwrap_or(&file.path);
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::path::absolute(parent).unwrap_or_else(|_| parent.to_path_buf())
}

fn resolve_image_path(base: &Path, src: &str) -> Option<PathBuf> {
    if src.is_empty() {
        return None;
    }
    let raw = Path::new(src);
    if raw.is_absolute() {
        return Some(raw.to_path_buf());
    }
    // Only local files: remote images are intentionally not fetched, matching
    // the app's offline-by-default behavior.
    if src.contains("://") && !src.starts_with("file://") {
        return None;
    }
    if let Ok(url) = url::Url::parse(src)
        && url.scheme() == "file"
        && let Ok(path) = url.to_file_path()
    {
        return Some(path);
    }
    let base = url::Url::from_directory_path(base).ok()?;
    let url = base.join(src).ok()?;
    (url.scheme() == "file")
        .then(|| url.to_file_path().ok())
        .flatten()
}

fn is_remote_image(src: &str) -> bool {
    let lower = src.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

fn checked_dimensions(width: u32, height: u32) -> bool {
    width > 0
        && height > 0
        && width <= 16_384
        && height <= 16_384
        && u64::from(width) * u64::from(height) <= 40_000_000
}

fn upload_image(ctx: &egui::Context, label: &str, image: DecodedImage) -> ImageEntry {
    let texture = ctx.load_texture(
        format!("markdown-image:{label}"),
        egui::ColorImage::from_rgba_unmultiplied(image.size, &image.rgba),
        egui::TextureOptions::LINEAR,
    );
    ImageEntry::Ready {
        texture,
        size: image.size,
    }
}

fn load_image(ctx: &egui::Context, base: &Path, src: &str) -> ImageEntry {
    if is_remote_image(src) {
        return start_remote_image(ctx, src);
    }
    let Some(path) = resolve_image_path(base, src) else {
        return ImageEntry::Failed;
    };
    // Check the header first so a corrupt or enormous file never allocates a
    // full RGBA buffer just to fail.
    let Ok((width, height)) = image::image_dimensions(&path) else {
        return ImageEntry::Failed;
    };
    if !checked_dimensions(width, height) {
        return ImageEntry::Failed;
    }
    let Ok(image) = image::open(&path) else {
        return ImageEntry::Failed;
    };
    let rgba = image.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    upload_image(
        ctx,
        &path.display().to_string(),
        DecodedImage {
            rgba: rgba.into_raw(),
            size,
        },
    )
}

/// Remote images are fetched and decoded off the UI thread; the frame that
/// first sees the block only registers a placeholder and schedules repaints.
fn start_remote_image(ctx: &egui::Context, src: &str) -> ImageEntry {
    let (sender, receiver) = std::sync::mpsc::channel();
    let url = src.to_owned();
    if std::thread::Builder::new()
        .name("markdown-image".to_owned())
        .spawn(move || {
            let _ = sender.send(fetch_remote_image(&url));
        })
        .is_err()
    {
        return ImageEntry::Failed;
    }
    ctx.request_repaint();
    ImageEntry::Loading {
        receiver: std::sync::Arc::new(std::sync::Mutex::new(receiver)),
    }
}

fn fetch_remote_image(src: &str) -> Result<DecodedImage, ()> {
    const MAX_BYTES: usize = 32 * 1024 * 1024;
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(25))
        .user_agent("LiteText")
        .build()
        .map_err(|_| ())?;
    let response = client.get(src).send().map_err(|_| ())?;
    if !response.status().is_success() {
        return Err(());
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_BYTES as u64)
    {
        return Err(());
    }
    let bytes = response.bytes().map_err(|_| ())?;
    if bytes.is_empty() || bytes.len() > MAX_BYTES {
        return Err(());
    }
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes.as_ref()))
        .with_guessed_format()
        .map_err(|_| ())?;
    let (width, height) = reader.into_dimensions().map_err(|_| ())?;
    if !checked_dimensions(width, height) {
        return Err(());
    }
    let image = image::load_from_memory(bytes.as_ref()).map_err(|_| ())?;
    let rgba = image.to_rgba8();
    Ok(DecodedImage {
        rgba: rgba.as_raw().clone(),
        size: [rgba.width() as usize, rgba.height() as usize],
    })
}

/// Decode local images and poll pending remote downloads. Entries are cached
/// per document, including failures, so a missing URL is not retried on every
/// frame.
fn ensure_images(ctx: &egui::Context, images: &mut Images, base: &Path, blocks: &[Block]) {
    let sources: Vec<String> = blocks
        .iter()
        .filter_map(|block| match &block.kind {
            Kind::Image { src, .. } => Some(src.clone()),
            _ => None,
        })
        .collect();
    let mut waiting = false;
    for src in sources {
        let updated = match images.get(&src) {
            Some(ImageEntry::Loading { receiver }) => {
                match receiver.lock().ok().map(|receiver| receiver.try_recv()) {
                    Some(Ok(Ok(image))) => Some(upload_image(ctx, &src, image)),
                    Some(Ok(Err(()))) | Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                        Some(ImageEntry::Failed)
                    }
                    Some(Err(std::sync::mpsc::TryRecvError::Empty)) => {
                        waiting = true;
                        None
                    }
                    None => Some(ImageEntry::Failed),
                }
            }
            Some(_) => None,
            None => Some(load_image(ctx, base, &src)),
        };
        if let Some(entry) = updated {
            images.insert(src, entry);
        }
    }
    if waiting {
        ctx.request_repaint_after(std::time::Duration::from_millis(120));
    }
}

fn image_display(size: [usize; 2], width: f32) -> egui::Vec2 {
    let source_w = size[0].max(1) as f32;
    let source_h = size[1].max(1) as f32;
    // Fit the reading column without enlarging small images.
    let scale = (width.max(32.0) / source_w).min(1.0);
    egui::vec2((source_w * scale).round(), (source_h * scale).round())
}

fn image_display_size(images: &Images, src: &str, width: f32, body: f32) -> egui::Vec2 {
    match images.get(src) {
        Some(ImageEntry::Ready { size, .. }) => image_display(*size, width),
        _ => egui::vec2(width, body * 1.2),
    }
}

fn image_block_height(images: &Images, src: &str, width: f32, body: f32) -> f32 {
    image_display_size(images, src, width, body).y + body * 0.9
}

fn image_failure_label(alt: &str, src: &str) -> String {
    if alt.is_empty() {
        format!("[image] {src}")
    } else {
        format!("[image: {alt}] {src}")
    }
}

/// One empty source line still needs a full editor row in the split view:
/// every row paints its own line number and shorter rows make consecutive
/// numbers overlap. Live rendering reserves its editable caret row up front
/// so clicking an empty line never changes the following block's position.
fn blank_line_height(ui: &egui::Ui, body: f32, split: bool) -> f32 {
    if !split {
        // A live blank line is clickable and becomes a one-row TextEdit. Keep
        // its resting height equal to that row instead of a collapsed gap.
        let font = FontId::proportional(body);
        let mut job = egui::text::LayoutJob::default();
        job.append("", 0.0, egui::TextFormat {
            font_id: font.clone(),
            line_height: Some(body * BODY_LINE_HEIGHT),
            ..Default::default()
        });
        return galley_height(ui, job).max(ui.fonts(|fonts| fonts.row_height(&font)));
    }
    // `body` is clamped for very large editor fonts, so size this from the
    // upper bound instead: the row must never be shorter than the gutter
    // glyphs that are painted into it.
    let editor_font = FontId::monospace(body + 1.5);
    ui.fonts(|fonts| fonts.row_height(&editor_font)).max(body)
}

#[allow(clippy::too_many_arguments)]
fn measure_block(
    ui: &egui::Ui,
    block: &Block,
    raw: &str,
    width: f32,
    pal: &Palette,
    body: f32,
    images: &Images,
    nowrap: bool,
) -> f32 {
    match &block.kind {
        Kind::Blank => blank_line_height(ui, body, nowrap),
        Kind::Heading(level) => {
            let (top, bottom) = heading_space(*level, body);
            let job = plain_job(
                &heading_text(raw),
                heading_font(*level, body),
                pal.text_main,
                width,
            );
            galley_height(ui, job) + top + bottom
        }
        Kind::Paragraph => {
            let job = inline_job(
                raw.trim(),
                FontId::proportional(body),
                pal.text_main,
                pal,
                width,
                None,
            );
            galley_height(ui, job) + body * 0.62
        }
        Kind::List { .. } => {
            let (_, item) = list_text(raw);
            let job = inline_job(
                &item,
                FontId::proportional(body),
                pal.text_main,
                pal,
                width - 22.0,
                None,
            );
            galley_height(ui, job).max(body + 4.0) + 4.0
        }
        Kind::Quote => {
            let job = inline_job(
                &quote_text(raw),
                FontId::proportional(body),
                pal.text_faint,
                pal,
                width - 24.0,
                None,
            );
            galley_height(ui, job) + 14.0
        }
        Kind::Callout(_) => {
            let job = inline_job(
                &callout_body(raw),
                FontId::proportional(body),
                pal.text_main,
                pal,
                width - 28.0,
                None,
            );
            galley_height(ui, job) + body * 1.2 + 20.0
        }
        Kind::Code(language) => {
            let text = code_body(raw);
            let job = plain_job(
                &text,
                FontId::monospace(body - 1.0),
                pal.text_main,
                if nowrap { f32::INFINITY } else { width - 20.0 },
            );
            let _ = language;
            // TextEdit reserves at least one font row even with desired_rows(1).
            // A one-line galley is pixel-rounded and can be shorter than that
            // fractional row height. Reserve the same minimum in preview, or
            // entering edit moves both the box bottom and every following block.
            let row_height = ui.fonts(|fonts| fonts.row_height(&FontId::monospace(body - 1.0)));
            galley_height(ui, job).max(row_height) + 18.0 + code_lang_height(body)
        }
        Kind::Rule => 16.0,
        Kind::Table => table_layout(ui, raw, body, pal, None).height() + 11.0,
        Kind::Html => {
            let job = plain_job(
                raw.trim(),
                FontId::monospace(body - 1.5),
                pal.text_faint,
                if nowrap { f32::INFINITY } else { width },
            );
            galley_height(ui, job) + 6.0
        }
        Kind::Image { alt, src } => {
            if matches!(images.get(src), Some(ImageEntry::Ready { .. })) {
                image_block_height(images, src, width, body)
            } else {
                let job = plain_job(
                    &image_failure_label(alt, src),
                    FontId::proportional(body * 0.85),
                    pal.text_faint,
                    width,
                );
                galley_height(ui, job) + body * 0.9
            }
        }
    }
}

fn code_body(raw: &str) -> String {
    let mut lines = raw.lines();
    lines.next();
    let mut body: Vec<&str> = lines.collect();
    if body
        .last()
        .is_some_and(|line| fence_marker(line.trim()).is_some())
    {
        body.pop();
    }
    body.join("\n")
}

/// Map a source byte offset inside `raw` to the byte offset of the text that
/// is actually laid out for that block kind.
fn rendered_offset(raw: &str, kind: &Kind, byte: usize) -> Option<usize> {
    match kind {
        Kind::Heading(level) => {
            let lead = raw.len() - raw.trim_start().len();
            let trimmed = raw.trim();
            let level = (*level as usize).min(trimmed.len());
            let after = level + (trimmed[level..].len() - trimmed[level..].trim_start().len());
            let start = lead + after;
            (byte >= start).then(|| byte - start)
        }
        Kind::Paragraph | Kind::Html => {
            let lead = raw.len() - raw.trim_start().len();
            (byte >= lead).then(|| byte - lead)
        }
        Kind::List { .. } => {
            let lead = raw.len() - raw.trim_start().len();
            let trimmed = raw.trim_start();
            let marker = list_marker(trimmed).map(|(_, marker)| marker).unwrap_or_default();
            let rest = &trimmed[marker.len()..];
            let spaces = rest.len() - rest.trim_start().len();
            let start = lead + marker.len() + spaces;
            (byte >= start).then(|| byte - start)
        }
        Kind::Quote => {
            let mut consumed = 0;
            let mut out = 0;
            for line in raw.split_inclusive('\n') {
                if byte < consumed + line.len() {
                    let within = byte - consumed;
                    let stripped = line.trim_start();
                    let stripped = stripped.strip_prefix('>').unwrap_or(stripped);
                    let lead = line.len() - stripped.len();
                    let body = stripped.trim_start();
                    let lead = lead + (stripped.len() - body.len());
                    return (within >= lead).then(|| out + within - lead);
                }
                let rendered = line
                    .trim_start()
                    .strip_prefix('>')
                    .unwrap_or(line)
                    .trim_start();
                out += rendered.trim_end_matches(['\r', '\n']).len();
                if line.ends_with('\n') {
                    out += 1;
                }
                consumed += line.len();
            }
            None
        }
        Kind::Callout(_) => {
            // The first line is the `[!TYPE]` marker and renders as a
            // localized title, so source offsets on it have no body target.
            let mut consumed = 0;
            let mut out = 0;
            let mut marker_line = true;
            for line in raw.split_inclusive('\n') {
                if byte < consumed + line.len() {
                    if marker_line {
                        return None;
                    }
                    let within = byte - consumed;
                    let stripped = line.trim_start();
                    let stripped = stripped.strip_prefix('>').unwrap_or(stripped);
                    let lead = line.len() - stripped.len();
                    let body = stripped.trim_start();
                    let lead = lead + (stripped.len() - body.len());
                    return (within >= lead).then(|| out + within - lead);
                }
                if !marker_line {
                    let rendered = line
                        .trim_start()
                        .strip_prefix('>')
                        .unwrap_or(line)
                        .trim_start();
                    out += rendered.trim_end_matches(['\r', '\n']).len();
                    if line.ends_with('\n') {
                        out += 1;
                    }
                }
                consumed += line.len();
                marker_line = false;
            }
            None
        }
        Kind::Code(_) => {
            let first = raw.lines().next().map_or(0, str::len);
            let start = first + usize::from(raw.len() > first);
            (byte >= start).then(|| byte - start)
        }
        Kind::Table => Some(byte),
        _ => None,
    }
}

/// Byte range of a document-level find hit within the block's rendered text.
fn block_highlight(block: &Block, raw: &str, range: &Range<usize>) -> Option<Range<usize>> {
    if range.start < block.range.start || range.start >= block.range.end {
        return None;
    }
    let rel = range.start - block.range.start;
    let offset = rendered_offset(raw, &block.kind, rel)?;
    let len = (range.end.min(block.range.end) - block.range.start).saturating_sub(rel);
    let end = offset + len.max(1);
    (offset < end).then_some(offset..end)
}

#[allow(clippy::too_many_arguments)]
fn render_block(
    ui: &mut egui::Ui,
    block: &Block,
    raw: &str,
    width: f32,
    pal: &Palette,
    body: f32,
    images: &Images,
    nowrap: bool,
    highlight: Option<Range<usize>>,
    lang: Lang,
    editable: bool,
    file_id: u64,
) -> (egui::Response, CodeLangRow) {
    let height = measure_block(ui, block, raw, width, pal, body, images, nowrap);
    let mut lang_row = CodeLangRow::default();
    let hit = highlight
        .as_ref()
        .and_then(|range| block_highlight(block, raw, range));
    let content_width = if nowrap {
        block_natural_width(ui, block, raw, pal, body).max(width)
    } else {
        width
    };
    let inner = ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        egui::Layout::top_down(egui::Align::LEFT),
        |ui| {
            ui.set_min_size(egui::vec2(width, height));
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
            let rect = ui.max_rect();
            // A non-wrapping block may be wider than the reading column; its
            // background must follow the full content, not the clip width.
            match &block.kind {
                Kind::Blank => {}
                Kind::Heading(level) => {
                    let (top, _) = heading_space(*level, body);
                    ui.add_space(top);
                    // Headings form a centered title band inside the reading
                    // column; body content stays left-aligned.
                    ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                        ui.add(
                            egui::Label::new(plain_job_highlighted(
                                &heading_text(raw),
                                heading_font(*level, body),
                                pal.text_main,
                                width,
                                hit.clone(),
                                pal,
                            ))
                            // Center the whole title band, not the glyphs a
                            // second time inside its galley. TextEdit centers
                            // a left-aligned galley too; matching that avoids
                            // pixel-rounding differences on odd-width titles.
                            .halign(egui::Align::LEFT)
                            .selectable(false),
                        );
                    });
                }
                Kind::Paragraph => {
                    ui.add(
                        egui::Label::new(inline_job(
                            raw.trim(),
                            FontId::proportional(body),
                            pal.text_main,
                            pal,
                            width,
                            hit.clone(),
                        ))
                        .selectable(false),
                    );
                    ui.add_space(body * 0.62);
                }
                Kind::List { ordered, marker } => {
                    let (_, item) = list_text(raw);
                    let job = inline_job(
                        &item, FontId::proportional(body), pal.text_main, pal,
                        width - 22.0, hit.clone(),
                    );
                    let galley = ui.fonts(|fonts| fonts.layout_job(job));
                    let text_pos = rect.min + egui::vec2(22.0, 0.0);
                    paint_list_marker_at(ui, text_pos, &galley, *ordered, marker, body, pal);
                    ui.painter().galley(text_pos, galley, pal.text_main);
                    ui.add_space(4.0);
                }
                Kind::Quote => {
                    ui.painter().rect_filled(rect, 4.0, pal.bg_inset);
                    ui.painter().rect_filled(
                        egui::Rect::from_min_size(rect.min, egui::vec2(3.0, rect.height())),
                        1.0,
                        pal.accent,
                    );
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.add_space(14.0);
                        ui.add(
                            egui::Label::new(inline_job(
                                &quote_text(raw),
                                FontId::proportional(body),
                                pal.text_faint,
                                pal,
                                width - 24.0,
                                hit.clone(),
                            ))
                            .selectable(false),
                        );
                    });
                    ui.add_space(8.0);
                }
                Kind::Callout(kind) => {
                    let color = kind.color(pal);
                    ui.painter().rect_filled(rect, 6.0, pal.bg_inset);
                    ui.painter().rect_filled(
                        egui::Rect::from_min_size(
                            rect.min,
                            egui::vec2(3.0, rect.height()),
                        ),
                        1.0,
                        color,
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.add_space(14.0);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(kind.label(lang == Lang::ZhCn))
                                    .font(FontId::new(
                                        body * 0.9,
                                        FontFamily::Name("brand".into()),
                                    ))
                                    .color(color),
                            )
                            .selectable(false),
                        );
                    });
                    ui.add_space(4.0);
                    let text = callout_body(raw);
                    if !text.is_empty() {
                        ui.horizontal(|ui| {
                            ui.add_space(14.0);
                            ui.add(
                                egui::Label::new(inline_job(
                                    &text,
                                    FontId::proportional(body),
                                    pal.text_main,
                                    pal,
                                    width - 28.0,
                                    hit.clone(),
                                ))
                                .selectable(false),
                            );
                        });
                    }
                    ui.add_space(8.0);
                }
                Kind::Code(language) => {
                    let lang_h = code_lang_height(body);
                    let body_rect = egui::Rect::from_min_size(
                        rect.min,
                        egui::vec2(content_width, (rect.height() - lang_h).max(0.0)),
                    );
                    ui.painter().rect_filled(body_rect, 6.0, pal.bg_inset);
                    #[cfg(test)]
                    ui.ctx().data_mut(|data| {
                        data.insert_temp(
                            egui::Id::new(("markdown_code_box", file_id, block.range.start)),
                            body_rect,
                        )
                    });
                    let text = code_body(raw);
                    let mut job = crate::workspace_theme::code_job(
                        &text,
                        language.as_deref().unwrap_or(""),
                        FontId::monospace(body - 1.0),
                        if nowrap { f32::INFINITY } else { width - 20.0 },
                        pal.dark,
                    );
                    if let Some(hit) = hit.clone() {
                        highlight_sections(&mut job, hit, pal);
                    }
                    // A horizontal UI centers short labels in at least the
                    // interaction row height. That adds a font-dependent y
                    // offset to single-line code, unlike the top-aligned
                    // body-only TextEdit. Paint at its exact content origin so
                    // both the glyph baselines and pixels stay put on entry.
                    let galley = ui.fonts(|fonts| fonts.layout_job(job));
                    ui.painter().galley(
                        rect.min + egui::vec2(10.0, 6.0),
                        galley,
                        pal.text_main,
                    );
                    let row_top = rect.top() + (rect.height() - lang_h).max(0.0);
                    ui.add_space((row_top - ui.cursor().top()).max(0.0));
                    lang_row = code_language_row(
                        ui,
                        rect.width(),
                        file_id,
                        block.range.start,
                        &fence_info_raw(raw.lines().next().unwrap_or("")),
                        body,
                        pal,
                        editable,
                    );
                }
                Kind::Rule => {
                    let y = rect.center().y;
                    ui.painter().line_segment(
                        [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                        Stroke::new(1.0_f32, pal.text_faint.gamma_multiply(0.55)),
                    );
                }
                Kind::Table => {
                    let layout = table_layout(ui, raw, body, pal, hit.as_ref());
                    ui.add_space(5.0);
                    if !nowrap && layout.width() > width {
                        // Floating bars do not change measured block height.
                        ui.spacing_mut().scroll = egui::style::ScrollStyle::floating();
                        let scroll = egui::ScrollArea::horizontal()
                            .id_salt(("markdown_table_scroll", file_id, block.range.start))
                            .drag_to_scroll(false)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(layout.width(), layout.height()), egui::Sense::hover());
                                paint_table(ui, &layout, rect.min, pal);
                            });
                        ui.ctx().data_mut(|data| data.insert_temp(
                            egui::Id::new(("markdown_table_offset", file_id, block.range.start)), scroll.state.offset.x));
                    } else {
                        ui.ctx().data_mut(|data| data.insert_temp(
                            egui::Id::new(("markdown_table_offset", file_id, block.range.start)), 0.0_f32));
                        paint_table(ui, &layout, rect.min + egui::vec2(0.0, 5.0), pal);
                    }
                }
                Kind::Html => {
                    ui.add(
                        egui::Label::new(plain_job_highlighted(
                            raw.trim(),
                            FontId::monospace(body - 1.5),
                            pal.text_faint,
                            if nowrap { f32::INFINITY } else { width },
                            hit.clone(),
                            pal,
                        ))
                        .selectable(false),
                    );
                    ui.add_space(6.0);
                }
                Kind::Image { alt, src } => match images.get(src) {
                    Some(ImageEntry::Ready { texture, size }) => {
                        ui.add_space(4.0);
                        let display = image_display(*size, width);
                        ui.add(
                            egui::Image::new(egui::load::SizedTexture::new(
                                texture.id(),
                                display,
                            ))
                            .corner_radius(4.0),
                        );
                        ui.add_space(body * 0.45);
                    }
                    _ => {
                        ui.add_space(4.0);
                        ui.add(
                            egui::Label::new(plain_job(
                                &image_failure_label(alt, src),
                                FontId::proportional(body * 0.85),
                                pal.text_faint,
                                width,
                            ))
                            .selectable(false),
                        );
                        ui.add_space(body * 0.45);
                    }
                },
            }
        },
    );
    (
        inner.response.interact(egui::Sense::click()),
        lang_row,
    )
}

/// Rendered heights for every source line. Used by the split view to stretch
/// the editable source pane to the same vertical rhythm as the preview.
/// Return the per-source-line height and font size of the rendered blocks.
/// Besides stretching each source row, the font size grows for headings so the
/// source line visually fills the same block as the preview.
fn line_layout(
    ui: &egui::Ui,
    text: &str,
    pal: &Palette,
    width: f32,
    body: f32,
    images: &Images,
    nowrap: bool,
) -> (Vec<f32>, Vec<f32>) {
    let lines = line_ranges(text);
    let mut heights: Vec<f32> = vec![0.0; lines.len()];
    let mut fonts: Vec<f32> = vec![body; lines.len()];
    for block in parse(text) {
        let raw = &text[block.range.clone()];
        let height = measure_block(ui, &block, raw, width, pal, body, images, nowrap);
        let font = match &block.kind {
            Kind::Heading(level) => heading_size(*level, body),
            Kind::Code(_) => body - 1.0,
            Kind::Table => body - 1.5,
            Kind::Html => body - 1.5,
            _ => body,
        };
        let per_line = height / block.line_count as f32;
        for line in block.line_start..(block.line_start + block.line_count).min(heights.len()) {
            heights[line] = per_line.max(1.0);
            fonts[line] = font;
        }
    }
    // Never leave a line without a height, even if a parser branch skipped it.
    let fallback = blank_line_height(ui, body, true);
    for line in 0..heights.len() {
        if heights[line] <= 0.0 {
            heights[line] = fallback;
        }
        fonts[line] = fonts[line].max(1.0);
    }
    (heights, fonts)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_parser_preserves_cells_alignment_and_source() {
        for newline in ["\n", "\r\n"] {
            for outer in [false, true] {
                let row = |s: &str| if outer { format!("| {s} |") } else { s.to_owned() };
                let text = [row("中文 | **标题** | 数量"), row(":--- | :---: | ---:"),
                    row(r"甲\|乙 | ``a|b`` | *二*")].join(newline);
                let original = text.clone();
                let blocks = parse(&text);
                assert_eq!(blocks.len(), 1);
                assert_eq!(blocks[0].kind, Kind::Table);
                assert_eq!(&text[blocks[0].range.clone()], text);
                assert_eq!(table_alignments(&row(":--- | :---: | ---:")).unwrap(),
                    [egui::Align::LEFT, egui::Align::Center, egui::Align::RIGHT]);
                let last = text.lines().last().unwrap();
                let cells: Vec<_> = table_cells(last).into_iter().map(|r| &last[r]).collect();
                assert_eq!(cells, [r"甲\|乙", "``a|b``", "*二*"]);
                assert_eq!(text, original);
            }
        }
        for invalid in ["a | b\n--- | --", "a | b\n---", "a | b\n:--: | ---", "a | b\n--- | --- | ---"] {
            assert!(!parse(invalid).iter().any(|b| b.kind == Kind::Table), "{invalid}");
        }
        assert_eq!(parse("paragraph\na | b\n--- | ---\nx | y")[1].kind, Kind::Table);
    }

    #[test]
    fn table_live_click_edit_keeps_following_layout_and_source() {
        for font_size in [12.5, 16.0, 28.5] {
            for row in ["甲乙 | value".to_owned(), format!("{} | value", "宽内容".repeat(80))] {
                let source = format!("中文 | Right\n:--- | ---:\n{row}\n\nFollowing paragraph.");
                let ctx = egui::Context::default();
                crate::setup_fonts(&ctx);
                let mut app = markdown_app();
                let file = app.active_mut().unwrap();
                file.draft_text = Some(source.clone());
                file.sync_draft();
                file.editor_state.font_size = font_size;
                file.markdown_mode = Mode::Live;
                for _ in 0..3 { draw(&ctx, &mut app, vec![]); }
                let id = app.active_id.unwrap();
                let rect = |at| ctx.data(|data| data.get_temp::<egui::Rect>(egui::Id::new(("markdown_block", id, at)))).unwrap();
                let following = source.find("Following").unwrap();
                let before = rect(following);
                click(&ctx, &mut app, rect(0).min + egui::vec2(20.0, 15.0));
                assert!(app.active().unwrap().markdown.cell_editing.is_some());
                assert!(app.active().unwrap().markdown.editing.is_none());
                for _ in 0..3 { draw(&ctx, &mut app, vec![]); }
                assert!((before.top() - rect(following).top()).abs() < 1.1,
                    "font {font_size}: before {before:?}, after {:?}", rect(following));
                assert_eq!(app.active().unwrap().draft_text.as_deref(), Some(source.as_str()));
                draw(&ctx, &mut app, vec![egui::Event::Key {
                    key: egui::Key::Escape, physical_key: None, pressed: true,
                    repeat: false, modifiers: egui::Modifiers::NONE,
                }]);
                draw(&ctx, &mut app, vec![]);
                assert!(app.active().unwrap().markdown.cell_editing.is_none());
                assert!((before.top() - rect(following).top()).abs() < 1.1);
                assert_eq!(app.active().unwrap().draft_text.as_deref(), Some(source.as_str()));
            }
        }
    }

    #[test]
    fn table_inline_code_stays_styled_until_a_backtick_is_deleted() {
        let ctx = egui::Context::default(); crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        let source = "| API | Note |\n| --- | --- |\n| `https://api.example.com` | keep |";
        let file = app.active_mut().unwrap(); file.draft_text = Some(source.into()); file.sync_draft(); file.markdown_mode = Mode::Live;
        for _ in 0..3 { draw(&ctx, &mut app, vec![]); }
        let id = app.active_id.unwrap();
        let cell = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("markdown_cell", id, 0usize, 1usize, 0usize)))).unwrap();
        click(&ctx, &mut app, cell.center());
        for _ in 0..3 { draw(&ctx, &mut app, vec![]); }
        let inspect = |output: egui::FullOutput| output.shapes.iter().rev().find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text().contains("https://api.example.com") && text.galley.text().starts_with('`') => Some(text.galley.job.clone()), _ => None,
        }).unwrap();
        let job = inspect(draw(&ctx, &mut app, vec![]));
        assert!(job.sections.iter().filter(|section| job.text[section.byte_range.clone()].contains('`'))
            .all(|section| section.format.color == Color32::TRANSPARENT));
        assert!(job.sections.iter().any(|section| section.format.font_id.family == FontFamily::Monospace
            && section.format.background != Color32::TRANSPARENT));
        let edit_id = egui::Id::new(("markdown_cell_edit", id, 0usize, 1usize, 0usize));
        let mut state = egui::TextEdit::load_state(&ctx, edit_id).unwrap();
        state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(1)))); state.store(&ctx, edit_id);
        draw(&ctx, &mut app, vec![egui::Event::Key { key: egui::Key::Backspace, physical_key: None, pressed: true,
            repeat: false, modifiers: Default::default() }]);
        let result = draw(&ctx, &mut app, vec![]);
        assert!(result.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Text(text) if text.galley.text() == "https://api.example.com`"
                && text.galley.job.sections.iter().all(|s| s.format.color != Color32::TRANSPARENT))));
        assert!(app.active().unwrap().draft_text.as_deref().unwrap().contains("| https://api.example.com` | keep |"));
    }

    #[test]
    fn table_cell_glyph_baselines_stay_put_on_entry() {
        for size in [12.5, 16.0, 22.0, 28.5] {
            for row in [0usize, 1] {
                let ctx = egui::Context::default(); crate::setup_fonts(&ctx);
                let mut app = markdown_app();
                let source = "| Header中文 | Other |\n| --- | --- |\n| value中文 | end |";
                let file = app.active_mut().unwrap(); file.draft_text = Some(source.into()); file.sync_draft();
                file.markdown_mode = Mode::Live; file.editor_state.font_size = size;
                for _ in 0..3 { draw(&ctx, &mut app, vec![]); }
                let label = if row == 0 { "Header中文" } else { "value中文" };
                let positions = |output: egui::FullOutput| {
                    output.shapes.iter().rev().find_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) if text.galley.text() == label => Some(text.galley.rows.iter()
                            .flat_map(|r| r.glyphs.iter().map(|g| text.pos + g.pos.to_vec2())).collect::<Vec<_>>()),
                        _ => None,
                    }).unwrap()
                };
                let before = positions(draw(&ctx, &mut app, vec![]));
                let id = app.active_id.unwrap();
                let cell = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("markdown_cell", id, 0usize, row, 0usize)))).unwrap();
                click(&ctx, &mut app, cell.center());
                for _ in 0..3 { draw(&ctx, &mut app, vec![]); }
                let after = positions(draw(&ctx, &mut app, vec![]));
                assert_eq!(before.len(), after.len());
                for (a,b) in before.iter().zip(after) { assert!(a.distance(b) < 0.25, "size={size} row={row}: {a:?} -> {b:?}"); }
            }
        }
    }

    #[test]
    fn table_right_click_insert_submenu_changes_column_count() {
        let ctx = egui::Context::default(); crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        let file = app.active_mut().unwrap(); file.draft_text = Some("| A | B |\n| --- | --- |\n| one | two |".into());
        file.sync_draft(); file.markdown_mode = Mode::Live;
        for _ in 0..3 { draw(&ctx, &mut app, vec![]); }
        let id = app.active_id.unwrap();
        let cell = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("markdown_cell", id, 0usize, 1usize, 0usize)))).unwrap();
        for pressed in [true, false] { draw(&ctx, &mut app, vec![egui::Event::PointerMoved(cell.center()), egui::Event::PointerButton {
            pos: cell.center(), button: egui::PointerButton::Secondary, pressed, modifiers: Default::default(),
        }]); }
        for _ in 0..3 { draw(&ctx, &mut app, vec![]); }
        let submenu = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("table_insert_menu", id, 0usize, "column")))).unwrap();
        for _ in 0..4 { draw(&ctx, &mut app, vec![egui::Event::PointerMoved(submenu.center())]); }
        let action = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("table_insert_action", id, 0usize, "column", 1usize)))).unwrap();
        click(&ctx, &mut app, action.center());
        for _ in 0..3 { draw(&ctx, &mut app, vec![]); }
        let source = app.active().unwrap().draft_text.as_deref().unwrap();
        assert_eq!(table_cells(source.lines().next().unwrap()).len(), 3, "{source}");
        assert!(source.contains("one")); assert!(source.contains("two"));
    }

    #[test]
    fn table_right_click_delete_targets_the_clicked_cell_and_preserves_surroundings() {
        for active in [false, true] {
            let ctx = egui::Context::default(); crate::setup_fonts(&ctx);
            let mut app = markdown_app();
            let source = "before\n\n| A | B |\n| --- | --- |\n| keep | one |\n| remove | two |\n\nafter";
            let start = source.find("| A").unwrap();
            let file = app.active_mut().unwrap(); file.draft_text = Some(source.into()); file.sync_draft(); file.markdown_mode = Mode::Live;
            for _ in 0..3 { draw(&ctx, &mut app, vec![]); }
            let id = app.active_id.unwrap();
            let cell = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("markdown_cell", id, start, 2usize, 1usize)))).unwrap();
            if active { click(&ctx, &mut app, cell.center()); for _ in 0..3 { draw(&ctx, &mut app, vec![]); } }
            for pressed in [true, false] { draw(&ctx, &mut app, vec![egui::Event::PointerMoved(cell.center()), egui::Event::PointerButton {
                pos: cell.center(), button: egui::PointerButton::Secondary, pressed, modifiers: Default::default(),
            }]); }
            for _ in 0..3 { draw(&ctx, &mut app, vec![]); }
            let row = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("table_delete_row", id, start)))).unwrap();
            click(&ctx, &mut app, row.center());
            for _ in 0..3 { draw(&ctx, &mut app, vec![]); }
            let result = app.active().unwrap().draft_text.as_deref().unwrap();
            assert!(result.starts_with("before\n\n| A | B |")); assert!(result.ends_with("\n\nafter"));
            assert!(result.contains("| keep | one |")); assert!(!result.contains("remove"), "active={active}: {result}");
            assert!(parse(result).iter().any(|block| matches!(block.kind, Kind::Table)));
        }
    }

    #[test]
    fn table_cell_live_input_tab_escape_preserves_other_source_bytes() {
        let source = "| 中文 | **Right** |\r\n| :--- | ---: |\r\n| 甲\\|乙 | `二` |\r\n\r\nFollowing.";
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        let file = app.active_mut().unwrap();
        file.draft_text = Some(source.to_owned());
        file.sync_draft();
        file.markdown_mode = Mode::Live;
        for _ in 0..3 { draw(&ctx, &mut app, vec![]); }
        let id = app.active_id.unwrap();
        let cell = ctx.data(|data| data.get_temp::<egui::Rect>(
            egui::Id::new(("markdown_cell", id, 0usize, 1usize, 0usize)))).unwrap();
        click(&ctx, &mut app, cell.center());
        draw(&ctx, &mut app, vec![]);
        assert!(app.active().unwrap().markdown.editing.is_none());
        assert_eq!(app.active().unwrap().markdown.cell_editing.as_ref().unwrap().text, r"甲\|乙");
        draw(&ctx, &mut app, vec![egui::Event::Text("新|字".into())]);
        let edited = app.active().unwrap().draft_text.clone().unwrap();
        assert!(edited.contains(r"新\|字"), "{edited:?}");
        assert!(edited.starts_with("| 中文 | **Right** |\r\n| :--- | ---: |\r\n| "));
        assert!(edited.ends_with(" | `二` |\r\n\r\nFollowing."));
        assert_eq!(parse(&edited)[0].kind, Kind::Table);
        let key = |key, modifiers| egui::Event::Key { key, physical_key: None,
            pressed: true, repeat: false, modifiers };
        let command = egui::Modifiers { command: true, mac_cmd: true, ..Default::default() };
        draw(&ctx, &mut app, vec![]);
        draw(&ctx, &mut app, vec![key(egui::Key::Z, command)]);
        assert_eq!(app.active().unwrap().draft_text.as_deref(), Some(source));
        assert!(app.active().unwrap().markdown.cell_editing.is_some());
        draw(&ctx, &mut app, vec![egui::Event::Text("新|字".into())]);
        let edited = app.active().unwrap().draft_text.clone().unwrap();
        draw(&ctx, &mut app, vec![key(egui::Key::Tab, egui::Modifiers::NONE)]);
        draw(&ctx, &mut app, vec![]);
        let next = app.active().unwrap().markdown.cell_editing.as_ref().unwrap();
        assert_eq!((next.row, next.column, next.text.as_str()), (1, 1, "`二`"));
        assert_eq!(&edited[next.source.clone()], "`二`");
        draw(&ctx, &mut app, vec![key(egui::Key::Tab, egui::Modifiers::SHIFT)]);
        draw(&ctx, &mut app, vec![]);
        assert_eq!(app.active().unwrap().markdown.cell_editing.as_ref().unwrap().column, 0);
        draw(&ctx, &mut app, vec![key(egui::Key::Escape, egui::Modifiers::NONE)]);
        assert!(app.active().unwrap().markdown.cell_editing.is_none());
        assert_eq!(app.active().unwrap().draft_text.as_deref(), Some(edited.as_str()));
        assert_eq!(table_cell_source(r"中\|文|值\\|末"), r"中\|文\|值\\\|末");
        click(&ctx, &mut app, cell.center());
        draw(&ctx, &mut app, vec![]);
        draw(&ctx, &mut app, vec![key(egui::Key::Enter, egui::Modifiers::NONE)]);
        assert!(app.active().unwrap().markdown.cell_editing.is_none());
        assert_eq!(app.active().unwrap().draft_text.as_deref(), Some(edited.as_str()));
        assert_eq!(table_cell_source("末\\"), "末\\\\");
        assert_eq!(table_cells(&format!("{}|next", table_cell_source("末\\"))).len(), 2);
    }

    #[test]
    fn table_click_mapping_skips_hidden_link_destinations() {
        for cell in ["[x](world)world", "[x](abc)abc", "[x](hello world) world", "[中](中文)中文", "**[x](world)**world"] {
            let suffix = if cell.ends_with("中文") { "中文" } else if cell.ends_with("abc") { "abc" } else { "world" };
            let expected = cell.rfind(suffix).unwrap();
            let visible_prefix = inline_job(&cell[..expected], FontId::proportional(16.0), crate::DARK.text_main,
                &crate::DARK, f32::INFINITY, None).text.chars().count();
            assert_eq!(table_visible_cursor_byte(cell, visible_prefix, 16.0, &crate::DARK), expected, "{cell}");
            assert_eq!(table_visible_cursor_byte(cell, visible_prefix + suffix.chars().count(), 16.0, &crate::DARK), cell.len(), "{cell}");
        }
    }

    #[test]
    fn table_header_is_bold_and_cells_render_inline_styles() {
        let pal = crate::DARK;
        let text = "中文 *强调* `code` [link](target)";
        let job = table_cell_job(text, 0..text.len(), true, 16.0, &pal, None);
        assert_eq!(job.text, "中文 强调 code link");
        assert!(job.sections.iter().filter(|s| !matches!(s.format.font_id.family, FontFamily::Monospace))
            .all(|s| s.format.font_id.family == FontFamily::Name("brand".into())));
        let text = r"甲\|乙 **粗体**";
        assert_eq!(table_cell_job(text, 0..text.len(), false, 16.0, &pal, None).text, "甲|乙 粗体");
    }

    #[test]
    fn table_grid_measurement_click_and_long_table_are_consistent() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let text = "| 中文 | Right |\n| :--- | ---: |\n| 甲乙 | **value** |";
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let pal = crate::DARK;
                let body = 16.0;
                let block = parse(text).remove(0);
                let layout = table_layout(ui, text, body, &pal, None);
                assert_eq!(layout.rows.len(), 2);
                assert_eq!(layout.rows[1][1].galley.text(), "value");
                let height = measure_block(ui, &block, text, 160.0, &pal, body, &Images::default(), false);
                assert_eq!(height, measure_block(ui, &block, text, 160.0, &pal, body, &Images::default(), true));
                let (lines, _) = line_layout(ui, text, &pal, 160.0, body, &Images::default(), true);
                assert!((lines.iter().sum::<f32>() - height).abs() < 0.01);
                let cursor = editing_cursor_from_click(ui, text,
                    egui::vec2(10.0, 5.0 + layout.row_height + 7.0), &Kind::Table, 160.0, body, &pal);
                assert_eq!(char_to_byte(text, cursor), text.find("甲乙").unwrap());
                let (response, _) = render_block(ui, &block, text, 160.0, &pal, body, &Images::default(), false, None, Lang::ZhCn, true, 1);
                assert!((response.rect.height() - height).abs() < 1.0, "{} vs {height}", response.rect.height());
                let long = format!("a | b\n--- | ---\n{}", "中文 | `a|b`\n".repeat(2000));
                let long = long.trim_end();
                let layout = table_layout(ui, long, body, &pal, None);
                assert_eq!(layout.rows.len(), 2001);
                assert!(layout.height().is_finite());
                assert_eq!(parse(long).len(), 1);
            });
        });
    }

    #[test]
    fn markdown_extension_is_recognised_by_the_workspace_theme() {
        for name in ["note.md", "note.markdown", "NOTE.MD"] {
            assert_eq!(
                crate::workspace_theme::language(std::path::Path::new(name), false),
                "Markdown",
                "{name}"
            );
        }
    }

    #[test]
    fn markdown_detection_follows_extension_or_language_override() {
        let mut app = markdown_app();
        let file = app.active_mut().unwrap();
        assert!(is_markdown(file));
        file.path = std::path::PathBuf::from("note.markdown");
        assert!(is_markdown(file));
        file.path = std::path::PathBuf::from("note.txt");
        assert!(!is_markdown(file));
        file.language_override = Some("Markdown".to_owned());
        assert!(is_markdown(file));
        file.path = std::path::PathBuf::from("note.md");
        file.language_override = Some("text".to_owned());
        assert!(!is_markdown(file));
        file.language_override = None;
        file.path = std::path::PathBuf::from("untitled");
        file.source_path = Some(std::path::PathBuf::from("README.md"));
        assert!(is_markdown(file));
    }

    #[test]
    fn parser_recognises_github_callouts_and_keeps_plain_quotes() {
        let text = "> [!NOTE]\n> first line\n> second **bold**\n\n> [!WARNING]\n> care\n\n> ordinary\n";
        let blocks = parse(text);
        let callouts: Vec<CalloutKind> = blocks
            .iter()
            .filter_map(|block| match block.kind {
                Kind::Callout(kind) => Some(kind),
                _ => None,
            })
            .collect();
        assert_eq!(callouts, [CalloutKind::Note, CalloutKind::Warning]);
        assert!(blocks.iter().any(|block| block.kind == Kind::Quote));
        let note = blocks
            .iter()
            .find(|block| block.kind == Kind::Callout(CalloutKind::Note))
            .unwrap();
        assert_eq!(
            callout_body(&text[note.range.clone()]),
            "first line\nsecond **bold**"
        );
        assert_eq!(
            CalloutKind::Note.template(true),
            "> [!NOTE]\n> 在这里输入提醒内容"
        );
    }

    #[test]
    fn table_and_callout_templates_insert_as_standalone_blocks() {
        let mut text = "intro\n\noutro\n".to_owned();
        let at = "intro".len();
        let callout = CalloutKind::Warning.template(true);
        let range = insert_block(&mut text, at, &callout);
        assert_eq!(&text[range], callout.as_str());
        assert!(
            text.starts_with("intro\n\n> [!WARNING]\n> 在这里输入警告内容\n\noutro\n"),
            "{text:?}"
        );

        let table = table_template(3, 2, true);
        assert_eq!(
            table,
            "| 列 1 | 列 2 |\n| --- | --- |\n|  |  |\n|  |  |"
        );
        let mut text = "intro\n\noutro\n".to_owned();
        let range = insert_block(&mut text, at, &table);
        assert_eq!(&text[range], table.as_str());
        assert!(
            text.contains("\n\n| 列 1 | 列 2 |\n| --- | --- |\n|  |  |\n|  |  |\n\noutro"),
            "{text:?}"
        );
    }

    #[test]
    fn inline_job_keeps_plain_text_in_one_section() {
        let job = inline_job(
            "plain text without any markers",
            FontId::proportional(15.0),
            Color32::WHITE,
            &crate::DARK,
            400.0,
            None,
        );
        assert_eq!(job.text, "plain text without any markers");
        assert_eq!(
            job.sections.len(),
            1,
            "a plain run must not create one section per character"
        );
    }

    #[test]
    fn inline_job_splits_a_run_only_at_highlight_boundaries() {
        let job = inline_job(
            "hello needle world",
            FontId::proportional(15.0),
            Color32::WHITE,
            &crate::DARK,
            400.0,
            Some(6..12),
        );
        assert_eq!(job.text, "hello needle world");
        assert_eq!(job.sections.len(), 3);
        let highlight = highlight_color(&crate::DARK);
        assert_eq!(job.sections[1].format.background, highlight);
        assert_eq!(
            &job.text[job.sections[1].byte_range.clone()],
            "needle"
        );
    }

    #[test]
    fn parser_recognises_common_blocks() {
        let text = "# Title\n\n- one\n- two\n\n> quote\n\n```rust\nfn main() {}\n```\n\n---\n\n![dot](images/dot.png)\n";
        let blocks = parse(text);
        let kinds: Vec<_> = blocks.iter().map(|block| block.kind.clone()).collect();
        assert!(kinds.contains(&Kind::Heading(1)));
        assert!(kinds.contains(&Kind::Image {
            alt: "dot".to_owned(),
            src: "images/dot.png".to_owned()
        }));
        let html = parse("<img src=\"pic.png\" alt=\"shot\">\n");
        assert_eq!(
            html[0].kind,
            Kind::Image {
                alt: "shot".to_owned(),
                src: "pic.png".to_owned()
            }
        );
        assert!(
            kinds
                .iter()
                .filter(|kind| matches!(kind, Kind::List { .. }))
                .count()
                >= 2
        );
        assert!(kinds.contains(&Kind::Quote));
        assert!(kinds.contains(&Kind::Code(Some("rust".to_owned()))));
        assert!(kinds.contains(&Kind::Rule));
    }

    #[test]
    fn inline_job_removes_markdown_markers() {
        let job = inline_job(
            "plain **bold** and `code` and [link](https://example.com)",
            FontId::proportional(15.0),
            Color32::WHITE,
            &crate::DARK,
            400.0,
            None,
        );
        assert_eq!(job.text, "plain bold and code and link");
    }

    #[test]
    fn inline_code_lines_keep_the_body_line_height() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let _ = ctx.run(Default::default(), |_| {});
        let body = 16.0;
        // The code span lands alone on the second row: with the monospace font
        // it used to be two pixels shorter than a normal row, pulling it onto
        // the line above.
        let job = inline_job(
            "plain words here\n`code`",
            FontId::proportional(body),
            Color32::WHITE,
            &crate::DARK,
            400.0,
            None,
        );
        let galley = ctx.fonts(|fonts| fonts.layout_job(job));
        assert_eq!(galley.rows.len(), 2, "the hard break must create two rows");
        let [plain, code] = [&galley.rows[0], &galley.rows[1]];
        assert_eq!(
            plain.rect.height(),
            code.rect.height(),
            "an inline-code-only row must keep the body row height"
        );
        let gap = |rows: &[egui::epaint::text::Row]| {
            let bottom = rows[0]
                .glyphs
                .iter()
                .map(|glyph| glyph.logical_rect().bottom())
                .fold(f32::MIN, f32::max);
            let top = rows[1]
                .glyphs
                .iter()
                .map(|glyph| glyph.logical_rect().top())
                .fold(f32::MAX, f32::min);
            top - bottom
        };
        let reference = inline_job(
            "plain words here\nplain words here",
            FontId::proportional(body),
            Color32::WHITE,
            &crate::DARK,
            400.0,
            None,
        );
        let reference = ctx.fonts(|fonts| fonts.layout_job(reference));
        let (code_gap, plain_gap) = (gap(&galley.rows), gap(&reference.rows));
        assert!(
            code_gap > 0.0 && (code_gap - plain_gap).abs() < 0.2,
            "the code line must sit like a plain line: {code_gap} vs {plain_gap}"
        );
    }

    #[test]
    fn editing_and_rendered_paragraphs_share_line_heights() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let _ = ctx.run(Default::default(), |_| {});
        let pal = crate::DARK;
        let body = 16.0;
        let text = "plain words here\n`code`";
        let rendered = inline_job(
            text,
            FontId::proportional(body),
            pal.text_main,
            &pal,
            400.0,
            None,
        );
        let editing = editing_job_for(
            text,
            &FontId::proportional(body),
            pal.text_main,
            &pal,
            400.0,
            false,
        );
        let heights = |job| {
            let galley = ctx.fonts(|fonts| fonts.layout_job(job));
            galley
                .rows
                .iter()
                .map(|row| row.rect.height())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            heights(rendered),
            heights(editing),
            "entering the inline editor must not move either line"
        );
    }

    #[test]
    fn inline_job_keeps_unmatched_backticks_and_hides_paired_markers() {
        for (text, shown) in [
            ("``", "``"),
            ("```", "```"),
            ("````", "````"),
            ("a``b", "a``b"),
            ("`` tail", "`` tail"),
            ("head ``", "head ``"),
            ("``a``", "a"),
            ("```a```", "a"),
            ("**``**", "``"),
            ("*``*", "``"),
            ("~~``~~", "``"),
            ("[``](https://example.com)", "``"),
            ("**`a`**", "a"),
            ("`a`", "a"),
            ("`a``", "a`"),
            ("`a` `b`", "a b"),
            ("`", "`"),
        ] {
            let job = inline_job(
                text,
                FontId::proportional(15.0),
                Color32::WHITE,
                &crate::DARK,
                400.0,
                None,
            );
            assert_eq!(job.text, shown, "input {text:?}");
        }
    }

    #[test]
    fn editing_job_keeps_unmatched_backticks_visible() {
        let pal = crate::DARK;
        for text in ["hello ``", "**``**", "~~``~~", "*``*", "x```"] {
            let job = editing_job_for(
                text,
                &FontId::proportional(15.0),
                pal.text_main,
                &pal,
                400.0,
                false,
            );
            assert_eq!(
                job.text, text,
                "every source byte must stay for caret mapping"
            );
            for section in &job.sections {
                let part = &job.text[section.byte_range.clone()];
                if part.contains('`') {
                    assert_ne!(
                        section.format.color,
                        egui::Color32::TRANSPARENT,
                        "{text:?} / {part:?} must stay visible while unmatched"
                    );
                }
            }
        }
        for text in ["hello `x`", "**`x`**", "*`x`*", "[`x`](https://example.com)"] {
            let job = editing_job_for(
                text,
                &FontId::proportional(15.0),
                pal.text_main,
                &pal,
                400.0,
                false,
            );
            assert_eq!(job.text, text);
            for section in &job.sections {
                let part = &job.text[section.byte_range.clone()];
                if part.contains('`') {
                    assert_eq!(
                        section.format.color,
                        egui::Color32::TRANSPARENT,
                        "{text:?} / {part:?} must hide paired markers"
                    );
                }
            }
        }
    }

    #[test]
    fn editing_enter_continues_lists_and_quotes_or_exits_empty_items() {
        assert_eq!(
            editing_enter_action("- item"),
            Some(("\n- ".to_owned(), false))
        );
        assert_eq!(
            editing_enter_action("  3. item"),
            Some(("\n  4. ".to_owned(), false))
        );
        assert_eq!(
            editing_enter_action("- [ ] task"),
            Some(("\n- [ ] ".to_owned(), false))
        );
        assert_eq!(
            editing_enter_action("> quote"),
            Some(("\n> ".to_owned(), false))
        );
        assert_eq!(
            editing_enter_action("-   "),
            Some((String::new(), true)),
            "an empty list item ends the list"
        );
        assert_eq!(editing_enter_action("> "), Some((String::new(), true)));
        assert_eq!(editing_enter_action("plain paragraph"), None);
    }

    #[test]
    fn editing_job_preserves_every_source_character() {
        let cases = [
            "# Heading\n\nA **bold** and *italic* and `code` and [link](https://example.com).\n\n- item\n- [ ] task\n\n> quote\n\n```rust\nfn main() {}\n```\n\n| a | b |\n| --- | --- |\n\n---\n",
            "中文 **粗体** 与 `代码`\r\n第二行\r\n",
            "plain",
            "**unclosed and ***triple*** and \\*escaped\\* and [label]",
            "",
            "\n\n",
        ];
        for text in cases {
            let job = editing_job(
                text,
                &FontId::proportional(15.0),
                Color32::WHITE,
                &crate::DARK,
                480.0,
            );
            assert_eq!(
                job.text, text,
                "the editing layout must stay a 1:1 copy of the buffer"
            );
            let covered: usize = job.sections.iter().map(|section| section.byte_range.len()).sum();
            assert_eq!(covered, text.len(), "every byte must be laid out once");
        }
    }

    #[test]
    fn editing_job_styles_headings_and_inline_runs_but_keeps_markers() {
        let body = 15.0;
        let pal = crate::DARK;
        let text = "# Heading text";
        let job = editing_job(text, &FontId::proportional(body), pal.text_main, &pal, 480.0);
        assert_eq!(job.text, text);
        let marker = job
            .sections
            .iter()
            .find(|section| section.byte_range.start == 0 && !section.byte_range.is_empty())
            .unwrap();
        assert_eq!(marker.format.color, egui::Color32::TRANSPARENT);
        assert!(
            marker.format.font_id.size < 0.5,
            "markers must not move the rendered text"
        );
        let heading = job
            .sections
            .iter()
            .find(|section| text[section.byte_range.clone()].starts_with("Heading"))
            .unwrap();
        assert!(heading.format.font_id.size > body);
        assert_eq!(
            heading.format.font_id.family,
            FontFamily::Name("brand".into())
        );

        let text = "a **bold** b";
        let job = editing_job(text, &FontId::proportional(body), pal.text_main, &pal, 480.0);
        assert_eq!(job.text, text);
        let bold = job
            .sections
            .iter()
            .find(|section| &text[section.byte_range.clone()] == "bold")
            .unwrap();
        assert_eq!(bold.format.font_id.family, FontFamily::Name("brand".into()));

        let text = "a `code` b";
        let job = editing_job(text, &FontId::proportional(body), pal.text_main, &pal, 480.0);
        let code = job
            .sections
            .iter()
            .find(|section| &text[section.byte_range.clone()] == "code")
            .unwrap();
        assert_eq!(code.format.background, pal.bg_elevated);
    }

    #[test]
    fn line_heights_cover_every_source_line() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let text = "# Heading\n\ntext\n\n```\ncode\n```\n";
        let lines = line_ranges(text).len();
        let mut heights = Vec::new();
        let mut fonts = Vec::new();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 700.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    (heights, fonts) =
                        line_layout(ui, text, &crate::DARK, 420.0, 15.0, &Images::new(), true);
                });
            },
        );
        assert_eq!(heights.len(), lines);
        assert!(heights.iter().all(|height| *height > 0.0));
        // The heading line must be taller than an ordinary paragraph line.
        assert!(heights[0] > heights[2]);
        assert!(
            fonts[0] > fonts[2] * 1.5,
            "heading font must grow with its block: {:?}",
            fonts
        );
    }

    #[test]
    fn split_source_edit_never_slices_the_preview_with_stale_blocks() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        let file = app.active_mut().unwrap();
        // The paragraph ends on a multi-byte glyph, so inserting one ASCII
        // byte at the start makes the old block end fall inside that glyph.
        file.draft_text = Some("中文\n\nend\n".to_owned());
        file.sync_draft();
        file.markdown_mode = Mode::Split;
        file.editor_state.pending_selection = Some(0..0);
        let run = |ctx: &egui::Context,
                   app: &mut crate::LiteTextApp,
                   events: Vec<egui::Event>| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000.0, 720.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        split_editor(ui, app.active_mut().unwrap(), &crate::DARK, true, Lang::ZhCn);
                    });
                },
            )
        };
        for _ in 0..3 {
            let _ = run(&ctx, &mut app, vec![]);
        }
        let _ = run(&ctx, &mut app, vec![egui::Event::Text("x".to_owned())]);
        for _ in 0..2 {
            let _ = run(&ctx, &mut app, vec![]);
        }
        assert_eq!(app.active().unwrap().draft_text.as_deref(), Some("x中文\n\nend\n"));
    }

#[test]
    fn split_blank_lines_keep_a_readable_gutter_row() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let text = "a\n\n\n\nb\n";
        // body size and the matching source editor font, including the clamped
        // extremes where the rendered body no longer tracks the font exactly.
        for (body, editor_font) in [(14.0_f32, 10.0_f32), (15.0, 13.5), (30.0, 32.0)] {
            let mut heights = Vec::new();
            let mut live_heights = Vec::new();
            let mut gutter_row = 0.0;
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 700.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        (heights, _) = line_layout(
                            ui, text, &crate::DARK, 420.0, body, &Images::new(), true,
                        );
                        (live_heights, _) = line_layout(
                            ui, text, &crate::DARK, 420.0, body, &Images::new(), false,
                        );
                        gutter_row =
                            crate::editor::row_height(ui, &FontId::monospace(editor_font - 1.0));
                    });
                },
            );
            let blanks = &heights[1..4];
            assert!(
                blanks.iter().all(|height| *height >= gutter_row),
                "body {body}: blank rows must fit their line numbers: {blanks:?} < row {gutter_row}"
            );
            assert!(
                blanks.iter().all(|height| (*height - blanks[0]).abs() < 0.01),
                "consecutive blank lines keep one height: {blanks:?}"
            );
            // Live blank rows must fit their caret before editing begins.
            assert!(live_heights[1..4].iter().all(|height| *height >= body * BODY_LINE_HEIGHT - 1.0));
        }
    }

#[test]
    fn split_source_text_scales_to_the_rendered_block_font() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        app.active_mut().unwrap().markdown_mode = Mode::Split;
        let mut output = None;
        for _ in 0..3 {
            output = Some(draw(&ctx, &mut app, vec![]));
        }
        let (mut heading, mut body) = (0.0_f32, 0.0_f32);
        for shape in output.unwrap().shapes {
            let egui::Shape::Text(text) = shape.shape else {
                continue;
            };
            let galley = text.galley;
            if !galley.job.text.starts_with("# Heading") {
                continue;
            }
            for row in &galley.rows {
                let content: String = row.glyphs.iter().map(|glyph| glyph.chr).collect();
                let height = row
                    .glyphs
                    .iter()
                    .map(|glyph| glyph.font_height)
                    .fold(0.0_f32, f32::max);
                if content.trim_start().starts_with('#') {
                    heading = height;
                } else if content.contains("A paragraph") {
                    body = height;
                }
            }
        }
        assert!(body > 0.0, "body source row must be rendered");
        assert!(
            heading > body * 1.5,
            "source heading glyphs must grow with the rendered block: heading {heading}, body {body}"
        );
    }

    #[test]
    fn split_preview_has_horizontal_scroll_for_long_code_lines() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        let long = "let value = some_function(argument_one, argument_two, argument_three, argument_four, argument_five, argument_six, argument_seven, argument_eight);";
        let file = app.active_mut().unwrap();
        file.draft_text = Some(format!("# Long line\n\n```rust\n{long}\n```\n"));
        file.sync_draft();
        app.active_mut().unwrap().markdown_mode = Mode::Split;
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let file_id = app.active_id.unwrap();
        let geometry = || {
            ctx.data(|data| {
                data.get_temp::<(egui::Vec2, egui::Rect, f32)>(egui::Id::new((
                    "markdown_split_geometry",
                    file_id,
                )))
            })
            .expect("split preview must publish its scroll geometry")
        };
        let (content, inner, _) = geometry();
        assert!(
            content.x > inner.width() + 10.0,
            "long code must provide horizontal scroll range: content {:?}, viewport {:?}",
            content,
            inner.size()
        );

        // Shift+wheel over the preview scrolls it horizontally.
        let target = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new(("markdown_split_right", file_id)))
            })
            .unwrap()
            .center();
        draw(&ctx, &mut app, vec![egui::Event::PointerMoved(target)]);
        for _ in 0..40 {
            draw(
                &ctx,
                &mut app,
                vec![egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -80.0),
                    modifiers: egui::Modifiers {
                        shift: true,
                        ..Default::default()
                    },
                }],
            );
        }
        let mut output = None;
        for _ in 0..8 {
            output = Some(draw(&ctx, &mut app, vec![]));
        }
        assert!(
            geometry().2 > 0.5,
            "the preview must actually scroll right, got {}",
            geometry().2
        );
        // Fully scrolled right, the page must still extend one CJK glyph past
        // the widest line instead of ending on it.
        let inner_rect = geometry().1;
        let mut code_right = 0.0_f32;
        for shape in output.unwrap().shapes {
            if let egui::Shape::Text(text) = shape.shape
                // The source pane renders the same text; only measure the
                // preview's copy.
                && text.pos.x >= inner_rect.left() - 1.0
                && text.galley.text().contains("some_function")
            {
                code_right = code_right.max(text.pos.x + text.galley.size().x);
            }
        }
        let inner_right = inner_rect.right();
        let cjk = crate::markdown::body_size(app.active().unwrap());
        assert!(
            inner_right - code_right >= cjk,
            "the page must stay at least one CJK glyph wider than its longest line: \
             right {code_right}, edge {inner_right}, glyph {cjk}"
        );
    }

    #[test]
    fn split_preview_has_no_horizontal_range_for_short_content() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        app.active_mut().unwrap().markdown_mode = Mode::Split;
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let file_id = app.active_id.unwrap();
        let (content, inner, offset) = ctx
            .data(|data| {
                data.get_temp::<(egui::Vec2, egui::Rect, f32)>(egui::Id::new((
                    "markdown_split_geometry",
                    file_id,
                )))
            })
            .unwrap();
        assert!(
            content.x <= inner.width() + 1.0 && offset == 0.0,
            "wrapped text must not create a horizontal scrollbar: content {:?}, viewport {:?}",
            content,
            inner.size()
        );
        // The preview pane must end inside the window instead of under its
        // right edge; this regressed when egui's default item spacing was
        // added on top of the explicitly sized panes.
        let right = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new(("markdown_split_right", file_id)))
            })
            .unwrap();
        assert!(
            right.right() <= 1000.5,
            "preview pane must stay inside the 1000 px window: {:?}",
            right
        );
    }

    #[test]
    fn remote_image_urls_are_detected_separately_from_local_paths() {
        assert!(is_remote_image("https://static.litetools.top/a.png"));
        assert!(is_remote_image("HTTP://example.com/a.png"));
        assert!(!is_remote_image("images/a.png"));
        assert!(!is_remote_image("/absolute/a.png"));
        assert!(!is_remote_image("file:///tmp/a.png"));
    }

    #[test]
    fn remote_images_are_fetched_and_decoded() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(6, 4, image::Rgba([10, 20, 30, 255]))
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let body = encoded.into_inner();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut request = [0_u8; 1024];
                let _ = stream.read(&mut request);
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        let image = fetch_remote_image(&format!("http://{address}/dot.png"))
            .expect("local HTTP image must decode");
        assert_eq!(image.size, [6, 4]);
    }

    #[test]
    fn remote_images_load_asynchronously_in_live_view() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(8, 5, image::Rgba([90, 40, 10, 255]))
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let body = encoded.into_inner();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut request = [0_u8; 1024];
                let _ = stream.read(&mut request);
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        let file = app.active_mut().unwrap();
        file.path = std::path::PathBuf::from("remote.md");
        file.name = "remote.md".to_owned();
        file.draft_text = Some(format!("![remote](http://{address}/dot.png)\n"));
        file.sync_draft();
        app.active_mut().unwrap().markdown_mode = Mode::Live;
        let url = format!("http://{address}/dot.png");
        for _ in 0..100 {
            draw(&ctx, &mut app, vec![]);
            if matches!(
                app.active().unwrap().markdown.images.get(&url),
                Some(ImageEntry::Ready { .. })
            ) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        match app.active().unwrap().markdown.images.get(&url) {
            Some(ImageEntry::Ready { size, .. }) => assert_eq!(*size, [8, 5]),
            _ => panic!("remote image must finish loading without blocking the UI"),
        }
    }

    #[test]
    fn local_images_decode_and_render_in_split_view() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("dot.png");
        image::RgbaImage::from_pixel(40, 20, image::Rgba([200, 60, 40, 255]))
            .save(&png)
            .unwrap();
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        let file = app.active_mut().unwrap();
        file.path = dir.path().join("note.md");
        file.name = "note.md".to_owned();
        file.draft_text = Some("# Title\n\n![dot](dot.png)\n\nAfter.\n".to_owned());
        file.sync_draft();
        app.active_mut().unwrap().markdown_mode = Mode::Split;
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let images = &app.active().unwrap().markdown.images;
        match images.get("dot.png") {
            Some(ImageEntry::Ready { size, .. }) => assert_eq!(*size, [40, 20]),
            _ => panic!("local image must decode"),
        }

        // A missing file becomes a cached placeholder label, not a panic or a
        // repeated decode attempt on every frame.
        let file = app.active_mut().unwrap();
        file.draft_text = Some("![gone](missing.png)\n".to_owned());
        file.sync_draft();
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        assert!(matches!(
            app.active().unwrap().markdown.images.get("missing.png"),
            Some(ImageEntry::Failed)
        ));
    }

    #[test]
    fn opening_a_markdown_file_uses_the_last_chosen_mode() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("a.md");
        let second = dir.path().join("b.md");
        std::fs::write(&first, "# A\n").unwrap();
        std::fs::write(&second, "# B\n").unwrap();
        let mut app = crate::LiteTextApp {
            lang: Lang::ZhCn,
            ..Default::default()
        };
        app.open_paths(vec![first]);
        assert_eq!(app.active().unwrap().markdown_mode, Mode::Source);
        app.markdown_mode = Mode::Live;
        app.open_paths(vec![second]);
        assert_eq!(
            app.active().unwrap().markdown_mode,
            Mode::Live,
            "new Markdown documents must reuse the last selected mode"
        );
    }

    fn markdown_app() -> crate::LiteTextApp {
        let mut app = crate::LiteTextApp {
            lang: Lang::ZhCn,
            ..Default::default()
        };
        app.new_document();
        let file = app.active_mut().unwrap();
        file.path = std::path::PathBuf::from("test.md");
        file.name = "test.md".to_owned();
        file.draft_text = Some(
            "# Heading\n\nA paragraph with **bold** and `code`.\n\n- item one\n- item two\n\n```rust\nfn main() {}\n```\n"
                .to_owned(),
        );
        file.sync_draft();
        crate::workspace_theme::configure(
            file,
            &crate::workspace_theme::WorkspaceTheme::default(),
            true,
        );
        app
    }

    fn draw(
        ctx: &egui::Context,
        app: &mut crate::LiteTextApp,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        draw_width(ctx, app, events, 1000.0)
    }

    fn draw_width(
        ctx: &egui::Context,
        app: &mut crate::LiteTextApp,
        events: Vec<egui::Event>,
        width: f32,
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 720.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    crate::draft_editor(
                        ui,
                        app.active_mut().unwrap(),
                        &crate::DARK,
                        false,
                        Lang::ZhCn,
                    );
                });
            },
        )
    }

    fn find_text_where(
        output: &egui::FullOutput,
        predicate: impl Fn(&str, egui::Pos2) -> bool,
    ) -> Option<(egui::Pos2, egui::Vec2)> {
        output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(text)
                if predicate(text.galley.text(), text.pos) =>
            {
                Some((
                    text.pos + text.galley.rect.left_top().to_vec2(),
                    text.galley.size(),
                ))
            }
            _ => None,
        })
    }

    /// Text sections that would paint a backtick: markers hidden with a
    /// transparent color are ignored, since they keep caret mapping only.
    fn visible_backticks(output: &egui::FullOutput, near: &str) -> Vec<String> {
        let mut found = Vec::new();
        for shape in &output.shapes {
            let egui::Shape::Text(text) = &shape.shape else {
                continue;
            };
            let job = &text.galley.job;
            if !job.text.contains(near) {
                continue;
            }
            for section in &job.sections {
                let part = &job.text[section.byte_range.clone()];
                if part.contains('`') && section.format.color != egui::Color32::TRANSPARENT {
                    found.push(part.to_owned());
                }
            }
        }
        found
    }

    fn click(ctx: &egui::Context, app: &mut crate::LiteTextApp, pos: egui::Pos2) {
        click_width(ctx, app, pos, 1000.0);
    }

    fn click_width(
        ctx: &egui::Context,
        app: &mut crate::LiteTextApp,
        pos: egui::Pos2,
        width: f32,
    ) {
        for pressed in [true, false] {
            draw_width(
                ctx,
                app,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
                width,
            );
        }
    }

    #[test]
    fn entering_edit_keeps_paragraph_text_in_place() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        app.active_mut().unwrap().markdown_mode = Mode::Live;
        let mut output = None;
        for _ in 0..3 {
            output = Some(draw(&ctx, &mut app, vec![]));
        }
        let output = output.unwrap();
        let file_id = app.active_id.unwrap();
        let text = app.active().unwrap().draft_text.clone().unwrap();
        let paragraph = parse(&text)
            .into_iter()
            .find(|block| block.kind == Kind::Paragraph)
            .unwrap();
        let before = find_text_where(&output, |text, _| {
            text == "A paragraph with bold and code."
        })
        .unwrap()
        .0
        .x;
        let rect = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new((
                    "markdown_block",
                    file_id,
                    paragraph.range.start,
                )))
            })
            .unwrap();
        click(&ctx, &mut app, rect.center());
        draw(&ctx, &mut app, vec![]);
        let output = draw(&ctx, &mut app, vec![]);
        let after = find_text_where(&output, |text, _| {
            text.contains("A paragraph with **bold**")
        })
        .unwrap()
        .0
        .x;
        assert!(
            (before - after).abs() < 1.5,
            "entering edit mode must not push the paragraph right: rendered {before}, editing {after}"
        );
    }

    #[test]
    fn entering_edit_keeps_headings_and_list_items_in_place() {
        for (source, rendered, editing_prefix) in [
            ("# Heading\n\nbody\n", "Heading", "# Heading"),
            ("- item one\n- item two\n", "item one", "- item one"),
            ("1. first\n2. second\n", "first", "1. first"),
        ] {
            let ctx = egui::Context::default();
            crate::setup_fonts(&ctx);
            let mut app = markdown_app();
            app.active_mut().unwrap().draft_text = Some(source.into());
            app.active_mut().unwrap().sync_draft();
            app.active_mut().unwrap().markdown_mode = Mode::Live;
            let mut output = None;
            for _ in 0..3 {
                output = Some(draw(&ctx, &mut app, vec![]));
            }
            let before_output = output.unwrap();
            let before = find_text_where(&before_output, |text, _| text == rendered)
                .unwrap()
                .0;
            let file_id = app.active_id.unwrap();
            let text = app.active().unwrap().draft_text.clone().unwrap();
            let block = parse(&text)
                .into_iter()
                .find(|block| {
                    matches!(block.kind, Kind::Heading(_) | Kind::List { .. })
                        && matches!(block.kind, Kind::Heading(_))
                            == editing_prefix.starts_with('#')
                })
                .unwrap();
            let rect = ctx
                .data(|data| {
                    data.get_temp::<egui::Rect>(egui::Id::new((
                        "markdown_block",
                        file_id,
                        block.range.start,
                    )))
                })
                .unwrap();
            click(&ctx, &mut app, rect.center());
            draw(&ctx, &mut app, vec![]);
            let output = draw(&ctx, &mut app, vec![]);
            let after =
                find_text_where(&output, |text, _| text.starts_with(editing_prefix))
                    .unwrap()
                    .0;
            assert!(
                (before.x - after.x).abs() < 1.5 && (before.y - after.y).abs() < 2.0,
                "editing must keep {editing_prefix:?} in place: rendered {before:?}, editing {after:?}"
            );
            if editing_prefix.starts_with(|c: char| c.is_ascii_digit()) {
                let number = find_text_where(&output, |text, _| text == "1.")
                    .unwrap_or_else(|| panic!("editing must keep the ordinal visible"));
                assert!(
                    number.0.x < after.x,
                    "the ordinal belongs in the left inset: number {number:?}, text {after:?}"
                );
            }
        }
    }

    #[test]
    fn entering_code_edit_keeps_the_code_in_place() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        app.active_mut().unwrap().draft_text =
            Some("```rust\nfn main() {}\n```\n".into());
        app.active_mut().unwrap().sync_draft();
        app.active_mut().unwrap().markdown_mode = Mode::Live;
        let mut output = None;
        for _ in 0..3 {
            output = Some(draw(&ctx, &mut app, vec![]));
        }
        let before = find_text_where(&output.unwrap(), |text, _| text == "fn main() {}")
            .unwrap()
            .0
            .y;
        let file_id = app.active_id.unwrap();
        let text = app.active().unwrap().draft_text.clone().unwrap();
        let block = parse(&text)
            .into_iter()
            .find(|block| matches!(block.kind, Kind::Code(_)))
            .unwrap();
        let rect = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new((
                    "markdown_block",
                    file_id,
                    block.range.start,
                )))
            })
            .unwrap();
        click(&ctx, &mut app, rect.center());
        draw(&ctx, &mut app, vec![]);
        let output = draw(&ctx, &mut app, vec![]);
        let after = find_text_where(&output, |text, _| {
            text.starts_with("fn main()")
        })
        .unwrap()
        .0
        .y;
        assert!(
            (before - after).abs() < 2.0,
            "entering code edit must not move the code: rendered {before}, editing {after}"
        );
    }

    #[test]
    fn code_body_adapter_preserves_envelopes_unicode_crlf_and_blank_lines() {
        for raw in [
            "```bash\ncodesign -f -s - /Applications/IDA\\ Professional\\ 9.0.app/Contents/MacOS/libida32.dylib\n```",
            "  ~~~~rust  \r\n你好🙂\r\nsecond\r\n\r\n  ~~~~  \r\n",
            "```\n\n```\n",
        ] {
            let body = CodeBodyEdit::new(raw);
            let mut committed = raw.to_owned();
            body.commit(&mut committed, &body.text);
            assert_eq!(committed, raw, "no-op must preserve the exact source");
            let edited = format!("{}\n新🙂", body.text);
            body.commit(&mut committed, &edited);
            let expected_body = if body.crlf { edited.replace('\n', "\r\n") } else { edited.clone() };
            assert_eq!(committed, format!("{}{}{}", &raw[..body.range.start], expected_body, &raw[body.range.end..]));
            let undo = CodeBodyEdit::new(&committed);
            undo.commit(&mut committed, &body.text);
            assert_eq!(committed, raw);
            for cursor in 0..=body.text.chars().count() {
                let byte = body.raw_byte(raw, cursor);
                assert_eq!(body.body_cursor(raw, raw[..byte].chars().count()), cursor);
            }
        }
    }

    #[test]
    fn code_body_edits_empty_blocks_and_crlf_boundaries() {
        for (raw, edited, expected) in [
            ("```\n```", "x", "```\nx\n```"),
            ("```\n\n```", "x", "```\nx\n```"),
            ("```\nx\n```", "", "```\n\n```"),
            ("```\r\na\r\nb\r\n```", "ab", "```\r\nab\r\n```"),
            ("```\r\na\r\nb\r\n```", "ax\nb", "```\r\nax\r\nb\r\n```"),
        ] {
            let body = CodeBodyEdit::new(raw);
            let mut actual = raw.to_owned();
            body.commit(&mut actual, edited);
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn live_code_body_never_places_the_caret_on_hidden_fences() {
        for source in [
            "```bash\ncodesign -f -s - /Applications/IDA\\ Professional\\ 9.0.app/Contents/MacOS/libida32.dylib\n```",
            "```rust\nlet x = 1;\n// 你好🙂\n```\n",
            "~~~~rust  \r\n你好🙂\r\nsecond\r\n~~~~  \r\n",
        ] {
            let ctx = egui::Context::default();
            crate::setup_fonts(&ctx);
            let mut app = markdown_app();
            let file = app.active_mut().unwrap();
            file.draft_text = Some(source.into());
            file.sync_draft();
            file.markdown_mode = Mode::Live;
            for _ in 0..3 { draw(&ctx, &mut app, vec![]); }
            let id = app.active_id.unwrap();
            let block = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("markdown_block", id, 0usize)))).unwrap();
            click(&ctx, &mut app, block.center());
            for _ in 0..3 { draw(&ctx, &mut app, vec![]); }
            let body = CodeBodyEdit::new(source);
            let job_text = ctx.data(|d| d.get_temp::<String>(egui::Id::new(("markdown_edit_job_text", id)))).unwrap();
            assert_eq!(job_text, body.text, "TextEdit must contain only real visible body characters");
            let edit_id = egui::Id::new(("markdown_edit", id, 0usize));
            let rect = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("markdown_edit_rect", id)))).unwrap();
            click(&ctx, &mut app, rect.right_bottom() - egui::vec2(1.0, 0.1));
            draw(&ctx, &mut app, vec![]);
            let cursor = egui::TextEdit::load_state(&ctx, edit_id).unwrap().cursor.char_range().unwrap().primary.index;
            assert_eq!(cursor, body.text.chars().count(), "bottom-right click must end on the last body row, not a tiny closing-fence row");
            // Synthetic clicks share egui's default timestamp and can be
            // interpreted as a double-click word selection. Collapse it before typing.
            let mut state = egui::TextEdit::load_state(&ctx, edit_id).unwrap();
            state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(cursor))));
            state.store(&ctx, edit_id);
            draw(&ctx, &mut app, vec![egui::Event::Text("追加🙂".into())]);
            let mut expected = source.to_owned();
            body.commit(&mut expected, &format!("{}追加🙂", body.text));
            assert_eq!(app.active().unwrap().draft_text.as_deref(), Some(expected.as_str()));
            draw(&ctx, &mut app, vec![egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }]);
            let mut entered = source.to_owned();
            body.commit(&mut entered, &format!("{}追加🙂\n", body.text));
            assert_eq!(app.active().unwrap().draft_text.as_deref(), Some(entered.as_str()));
            let job = code_body_edit_job(&body.text, "bash", 15.0, 480.0, &crate::DARK);
            assert!(job.sections.iter().all(|s| s.format.font_id.size >= 9.0 && s.format.color != Color32::TRANSPARENT));
        }
    }

    #[test]
    fn editing_hides_code_fences_without_leaving_blank_rows() {
        let body = 15.0;
        let pal = crate::DARK;
        let text = "```rust\nfn main() {}\n```\n";
        let job = editing_job(
            text,
            &FontId::proportional(body),
            pal.text_main,
            &pal,
            480.0,
        );
        assert_eq!(job.text, text);
        for fence in ["```rust", "```"] {
            let section = job
                .sections
                .iter()
                .find(|section| &job.text[section.byte_range.clone()] == fence)
                .unwrap();
            assert_eq!(section.format.color, egui::Color32::TRANSPARENT);
            assert!(
                section.format.font_id.size < 0.5
                    && section.format.line_height == Some(0.1),
                "fence rows must not show markers or reserve a blank row"
            );
        }
    }

    #[test]
    fn editing_code_keeps_syntax_colors() {
        let body = 15.0;
        let pal = crate::DARK;
        let text = "```rust\nfn main() {\n    let x = 1;\n}\n```\n";
        let job = editing_job(
            text,
            &FontId::proportional(body),
            pal.text_main,
            &pal,
            480.0,
        );
        assert_eq!(job.text, text);
        let code_start = job.text.find("fn main").unwrap();
        let code_end = job.text.find("```\n").unwrap_or(job.text.len());
        let mut colors = std::collections::HashSet::new();
        for section in &job.sections {
            if section.format.color == egui::Color32::TRANSPARENT {
                continue;
            }
            if section.byte_range.start < code_end && section.byte_range.end > code_start {
                colors.insert(section.format.color);
            }
        }
        assert!(
            colors.len() > 1,
            "code body must keep its syntax colors while editing, got {colors:?}"
        );
    }

    #[test]
    fn list_marker_and_text_keep_the_same_vertical_position_when_editing() {
        for size in [12.5, 16.0, 22.0, 28.5] {
            for source in ["1. first item", "12. first item", "- first item", "1. first item\n   continued line"] {
                let ctx = egui::Context::default();
                crate::setup_fonts(&ctx);
                let mut app = markdown_app();
                let file = app.active_mut().unwrap();
                file.draft_text = Some(source.into()); file.sync_draft();
                file.markdown_mode = Mode::Live; file.editor_state.font_size = size;
                for _ in 0..3 { draw(&ctx, &mut app, vec![]); }
                let before = draw(&ctx, &mut app, vec![]);
                let marker = if source.starts_with("12.") { "12." } else if source.starts_with('-') { "•" } else { "1." };
                let mark_before = find_text_where(&before, |text, _| text == marker).unwrap().0;
                let body_before = find_text_where(&before, |text, _| text.starts_with("first item")).unwrap().0;
                let id = app.active_id.unwrap();
                let rect = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("markdown_block", id, 0usize)))).unwrap();
                click(&ctx, &mut app, rect.center());
                for _ in 0..3 { draw(&ctx, &mut app, vec![]); }
                let after = draw(&ctx, &mut app, vec![]);
                let mark_after = find_text_where(&after, |text, _| text == marker).unwrap().0;
                let body_after = find_text_where(&after, |text, _| text.contains("first item")).unwrap().0;
                assert!((mark_before.y - mark_after.y).abs() < 0.1, "{size} {source}: marker {mark_before:?} -> {mark_after:?}");
                assert!((body_before.y - body_after.y).abs() < 0.1, "{size} {source}: body {body_before:?} -> {body_after:?}");
            }
        }
    }

    #[test]
    fn clicking_heading_before_image_while_editing_another_block_keeps_heading_target() {
        let source = "# First\n\nparagraph\n\n## 打开app-享受底层\n\n![image](missing-image.png)\n";
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        let file = app.active_mut().unwrap();
        file.draft_text = Some(source.into()); file.sync_draft(); file.markdown_mode = Mode::Live;
        let image = upload_image(&ctx, "test-heading-image", DecodedImage { rgba: vec![255; 600 * 800 * 4], size: [600, 800] });
        app.active_mut().unwrap().markdown.images.insert("missing-image.png".into(), image);
        for _ in 0..3 { draw(&ctx, &mut app, vec![]); }
        let id = app.active_id.unwrap();
        let rect = |start| ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("markdown_block", id, start)))).unwrap();
        let paragraph = source.find("paragraph").unwrap();
        let heading = source.find("## 打开").unwrap();
        click(&ctx, &mut app, rect(paragraph).center());
        for _ in 0..3 { draw(&ctx, &mut app, vec![]); }
        click(&ctx, &mut app, rect(heading).center());
        for _ in 0..3 { draw(&ctx, &mut app, vec![]); }
        let editing = app.active().unwrap().markdown.editing.as_ref().unwrap();
        assert_eq!(editing.range.start, heading);
        assert!(editing.text.starts_with("## 打开app"));
        let state = egui::TextEdit::load_state(&ctx, egui::Id::new(("markdown_edit", id, heading))).unwrap();
        assert!(state.cursor.char_range().unwrap().primary.index <= editing.text.chars().count());
        // A zero-length edit on the separator belongs only to that blank
        // block, never also to the preceding heading's end boundary.
        let separator = source.find("\n\n![image]").unwrap() + 1;
        app.active_mut().unwrap().markdown.editing = Some(Editing {
            range: separator..separator, text: String::new(), focus: true,
            fresh: true, code: false, cursor: Some(0),
        });
        for _ in 0..3 { draw(&ctx, &mut app, vec![]); }
        click(&ctx, &mut app, rect(heading).center());
        for _ in 0..3 { draw(&ctx, &mut app, vec![]); }
        assert_eq!(app.active().unwrap().markdown.editing.as_ref().unwrap().range.start, heading);
    }

    #[test]
    fn editing_blank_line_keeps_following_content_in_place() {
        for size in [10.0, 14.0, 22.0, 32.0] {
            for source in ["before\n\nafter", "before\n   \nafter", "before\n\n\nafter"] {
                let ctx = egui::Context::default();
                crate::setup_fonts(&ctx);
                let mut app = markdown_app();
                let file = app.active_mut().unwrap();
                file.draft_text = Some(source.into());
                file.sync_draft();
                file.markdown_mode = Mode::Live;
                file.editor_state.font_size = size;
                for _ in 0..3 { draw(&ctx, &mut app, vec![]); }
                let id = app.active_id.unwrap();
                let blocks = parse(source);
                let blank = blocks.iter().find(|b| b.kind == Kind::Blank).unwrap();
                let after = blocks.iter().find(|b| &source[b.range.clone()] == "after").unwrap();
                let rect = |start| ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("markdown_block", id, start)))).unwrap();
                let before = rect(after.range.start);
                click(&ctx, &mut app, rect(blank.range.start).center());
                for _ in 0..3 { draw(&ctx, &mut app, vec![]); }
                assert!(app.active().unwrap().markdown.editing.is_some());
                let edited = rect(after.range.start);
                assert!((edited.top() - before.top()).abs() < 0.5,
                    "size={size} source={source:?}: before={before:?} edited={edited:?}");
            }
        }
    }

    #[test]
    fn editing_code_box_matches_the_rendered_height_for_one_and_many_lines() {
        for app_fonts in [false, true] {
            for font_size in [12.5, 13.5, 16.0, 22.0, 28.5] {
                for code in [
                    "```rust\nfn main() {}\n```\n",
                    "```\nplain\n```\n",
                    "```rust\nfn main() {\n    let x = 1;\n}\n```\n",
                    "```python\nx = 1\n```\n",
                    "```\n\n```\n",
                ] {
                    let text = format!("{code}Following paragraph.\n");
                    let ctx = egui::Context::default();
                    if app_fonts {
                        crate::setup_fonts(&ctx);
                    }
                    let mut app = markdown_app();
                    app.active_mut().unwrap().draft_text = Some(text.to_owned());
                    app.active_mut().unwrap().sync_draft();
                    app.active_mut().unwrap().editor_state.font_size = font_size;
                    app.active_mut().unwrap().markdown_mode = Mode::Live;
                    for _ in 0..3 {
                        draw(&ctx, &mut app, vec![]);
                    }
                    let file_id = app.active_id.unwrap();
                    let source = app.active().unwrap().draft_text.clone().unwrap();
                    let block = parse(&source)
                        .into_iter()
                        .find(|block| matches!(block.kind, Kind::Code(_)))
                        .unwrap();
                    let rendered = ctx
                        .data(|data| {
                            data.get_temp::<egui::Rect>(egui::Id::new((
                                "markdown_code_box",
                                file_id,
                                block.range.start,
                            )))
                        })
                        .unwrap();
                    let block_rect = ctx
                        .data(|data| {
                            data.get_temp::<egui::Rect>(egui::Id::new((
                                "markdown_block",
                                file_id,
                                block.range.start,
                            )))
                        })
                        .unwrap();
                    let following_at = parse(&source)
                        .into_iter()
                        .find(|block| matches!(block.kind, Kind::Paragraph))
                        .unwrap()
                        .range
                        .start;
                    let following_rect = || {
                        ctx.data(|data| {
                            data.get_temp::<egui::Rect>(egui::Id::new((
                                "markdown_block",
                                file_id,
                                following_at,
                            )))
                        })
                        .unwrap()
                    };
                    let following_before = following_rect();
                    click(&ctx, &mut app, rendered.center());
                    assert!(app.active().unwrap().markdown.editing.is_some());
                    for frame in 0..3 {
                        draw(&ctx, &mut app, vec![]);
                        let following_after = following_rect();
                        assert!(
                            (following_before.top() - following_after.top()).abs() < 0.05,
                            "font {font_size}, {code:?}, frame {frame}: following block moved: before {following_before:?}, after {following_after:?}; code block {block_rect:?}"
                        );
                    }
                    let editing = ctx
                        .data(|data| {
                            data.get_temp::<egui::Rect>(egui::Id::new((
                                "markdown_code_edit_box",
                                file_id,
                                block.range.start,
                            )))
                        })
                        .unwrap();
                    assert!(
                        (editing.height() - rendered.height()).abs() < 0.05,
                        "{text:?} edit box must keep the rendered height: rendered {rendered:?}, editing {editing:?}"
                    );
                    assert!(
                        (editing.top() - rendered.top()).abs() < 0.05,
                        "{text:?} edit box must start where the rendered box starts: rendered {rendered:?}, editing {editing:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn editing_code_preserves_painted_glyph_positions() {
        // Compare actual text shapes, baselines and glyph mesh vertices, not
        // just the stable background box. Include the long bash command that
        // exposed the single-row jump, and mixed-height multiline content.
        for app_fonts in [false, true] {
            for font_size in [10.0, 12.5, 16.0, 22.0, 28.5, 32.0] {
                for body in [
                    "codesign --force --sign - app",
                    "codesign --force --deep --sign - /Applications/LiteText.app --preserve-metadata=identifier,entitlements,requirements,flags,runtime",
                    "codesign --force --deep --sign - /Applications/LiteText.app\necho signed",
                    "codesign --force --sign - app\n\necho 签名完成",
                ] {
                    let source = format!("```bash\n{body}\n```\nFollowing paragraph.\n");
                    let ctx = egui::Context::default();
                    if app_fonts { crate::setup_fonts(&ctx); }
                    let mut app = markdown_app();
                    let file = app.active_mut().unwrap();
                    file.draft_text = Some(source);
                    file.sync_draft();
                    file.editor_state.font_size = font_size;
                    file.markdown_mode = Mode::Live;
                    for _ in 0..3 { draw(&ctx, &mut app, vec![]); }
                    let snapshot = |output: &egui::FullOutput| {
                        let text = output.shapes.iter().find_map(|shape| match &shape.shape {
                            egui::Shape::Text(text) if text.galley.text() == body => Some(text),
                            _ => None,
                        }).expect("code body must be painted as its own text shape");
                        let baselines: Vec<_> = text.galley.rows.iter().flat_map(|row| {
                            row.glyphs.iter().map(|glyph| (glyph.chr, text.pos + glyph.pos.to_vec2()))
                        }).collect();
                        let vertices: Vec<_> = text.galley.rows.iter().flat_map(|row| {
                            row.visuals.mesh.vertices[row.visuals.glyph_vertex_range.clone()]
                                .iter().map(|vertex| text.pos + vertex.pos.to_vec2())
                        }).collect();
                        (text.pos, baselines, vertices)
                    };
                    let rendered = snapshot(&draw(&ctx, &mut app, vec![]));
                    let id = app.active_id.unwrap();
                    let rect = ctx.data(|data| data.get_temp::<egui::Rect>(egui::Id::new((
                        "markdown_code_box", id, 0_usize,
                    )))).unwrap();
                    click(&ctx, &mut app, rect.left_top() + egui::vec2(20.0, 10.0));
                    assert!(app.active().unwrap().markdown.editing.is_some());
                    for frame in 0..3 {
                        let edited = snapshot(&draw(&ctx, &mut app, vec![]));
                        let context = format!("fonts={app_fonts}, size={font_size}, frame={frame}, body={body:?}, rendered origin={:?}, edited origin={:?}", rendered.0, edited.0);
                        assert_eq!(rendered.1.len(), edited.1.len(), "{context}");
                        for ((before_char, before), (after_char, after)) in rendered.1.iter().zip(&edited.1) {
                            assert_eq!(before_char, after_char, "{context}");
                            assert!(before.distance(*after) < 0.05, "glyph {before_char:?}: {before:?} -> {after:?}; {context}");
                        }
                        assert_eq!(rendered.2.len(), edited.2.len(), "{context}");
                        for (before, after) in rendered.2.iter().zip(&edited.2) {
                            assert!(before.distance(*after) < 0.05, "painted glyph vertex {before:?} -> {after:?}; {context}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn empty_code_language_hint_is_txt_and_the_pill_hugs_its_text() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        app.active_mut().unwrap().draft_text = Some("```\nplain\n```\n".into());
        app.active_mut().unwrap().sync_draft();
        app.active_mut().unwrap().markdown_mode = Mode::Live;
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let file_id = app.active_id.unwrap();
        let text = app.active().unwrap().draft_text.clone().unwrap();
        let at = parse(&text)[0].range.start;
        let field = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new((
                    "markdown_code_lang_rect",
                    file_id,
                    at,
                )))
            })
            .unwrap();
        let output = draw(&ctx, &mut app, vec![]);
        find_text_where(&output, |text, pos| text == "txt" && field.contains(pos))
            .expect("an empty language field shows the txt hint");
        assert!(
            !output.shapes.iter().any(|shape| match &shape.shape {
                egui::Shape::Text(text) => text.galley.text().contains("纯文本"),
                _ => false,
            }),
            "the language hint must not be localized"
        );
        let pill = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect)
                    if rect.rect.contains(field.center())
                        && rect.rect.width() < 120.0
                        && rect.rect.height() < 40.0
                        && rect.fill != egui::Color32::TRANSPARENT =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .expect("the language field paints its own pill");
        assert!(
            pill.width() < 70.0,
            "the pill must use the compact minimum width for the short hint: {pill:?}"
        );
    }

    #[test]
    fn short_code_language_labels_share_one_minimum_width() {
        let mut widths = Vec::new();
        for label in ["txt", "rust", "swift"] {
            let ctx = egui::Context::default();
            crate::setup_fonts(&ctx);
            let mut app = markdown_app();
            app.active_mut().unwrap().draft_text = Some(format!("```{label}\ncode\n```\n"));
            app.active_mut().unwrap().sync_draft();
            app.active_mut().unwrap().markdown_mode = Mode::Live;
            for _ in 0..3 {
                draw(&ctx, &mut app, vec![]);
            }
            let file_id = app.active_id.unwrap();
            let text = app.active().unwrap().draft_text.clone().unwrap();
            let at = parse(&text)[0].range.start;
            let rect = ctx
                .data(|data| {
                    data.get_temp::<egui::Rect>(egui::Id::new((
                        "markdown_code_lang_rect",
                        file_id,
                        at,
                    )))
                })
                .unwrap();
            widths.push(rect.width());
        }
        assert!(
            widths.windows(2).all(|pair| (pair[0] - pair[1]).abs() < 0.5),
            "3-5 character labels must share one width, got {widths:?}"
        );
        assert!(
            widths[0] >= 44.0,
            "the field needs a minimum width, got {widths:?}"
        );
    }

    #[test]
    fn code_language_field_sits_outside_the_box_and_applies_typed_syntaxes() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        app.active_mut().unwrap().draft_text = Some("```rust\nfn main() {}\n```\n".into());
        app.active_mut().unwrap().sync_draft();
        app.active_mut().unwrap().markdown_mode = Mode::Live;
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let file_id = app.active_id.unwrap();
        let at = parse(&app.active().unwrap().draft_text.clone().unwrap())[0]
            .range
            .start;
        let field = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new((
                    "markdown_code_lang_rect",
                    file_id,
                    at,
                )))
            })
            .expect("the code block must show a language field");
        let code_box = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new((
                    "markdown_code_box",
                    file_id,
                    at,
                )))
            })
            .expect("the code block must paint its box");
        assert!(
            field.top() >= code_box.bottom(),
            "the language field belongs below the code box: field {field:?}, box {code_box:?}"
        );
        assert!(
            field.right() > code_box.right() - 40.0,
            "the language field stays right-aligned"
        );
        let output = draw(&ctx, &mut app, vec![]);
        find_text_where(&output, |text, pos| text == "rust" && field.contains(pos))
            .expect("the field must show the current fence info");
        let pill = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect)
                    if rect.rect.contains(field.center())
                        && rect.rect.width() < 300.0
                        && rect.rect.height() < 40.0
                        && rect.fill != egui::Color32::TRANSPARENT =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .expect("the language field paints its own pill");
        assert!(
            pill.top() >= code_box.bottom() - 0.5,
            "the language pill must sit outside the code box: pill {pill:?}, box {code_box:?}"
        );
        assert!(
            pill.width() <= field.width() + 20.0 && pill.width() < 70.0,
            "the pill must hug the input: pill {pill:?}, field {field:?}"
        );
        click(&ctx, &mut app, field.center());
        draw(&ctx, &mut app, vec![]);
        assert!(
            app.active().unwrap().markdown.editing.is_none(),
            "clicking the language field must not open block editing"
        );

        let id = egui::Id::new(("markdown_code_lang", file_id, at));
        let replace = |ctx: &egui::Context, end: usize| {
            let mut state =
                egui::TextEdit::load_state(ctx, id).unwrap_or_default();
            state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(end),
            )));
            state.store(ctx, id);
            ctx.memory_mut(|memory| memory.request_focus(id));
        };
        replace(&ctx, 4);
        draw(&ctx, &mut app, vec![]);
        draw(
            &ctx,
            &mut app,
            vec![egui::Event::Text("py".into())],
        );
        for _ in 0..2 {
            draw(&ctx, &mut app, vec![]);
        }
        let text = app.active().unwrap().draft_text.clone().unwrap();
        assert_eq!(text.lines().next(), Some("```py"), "{text:?}");
        assert_eq!(crate::workspace_theme::code_syntax_name("py"), "Python");

        replace(&ctx, 2);
        draw(&ctx, &mut app, vec![]);
        draw(
            &ctx,
            &mut app,
            vec![egui::Event::Text("not-a-language".into())],
        );
        for _ in 0..2 {
            draw(&ctx, &mut app, vec![]);
        }
        let text = app.active().unwrap().draft_text.clone().unwrap();
        assert_eq!(text.lines().next(), Some("```not-a-language"), "{text:?}");
        assert_eq!(
            crate::workspace_theme::code_syntax_name("not-a-language"),
            "Plain Text"
        );
    }

    #[test]
    fn editing_code_language_field_keeps_the_editor_open() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        app.active_mut().unwrap().draft_text = Some("```rust\nfn main() {}\n```\n".into());
        app.active_mut().unwrap().sync_draft();
        app.active_mut().unwrap().markdown_mode = Mode::Live;
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let file_id = app.active_id.unwrap();
        let at = parse(&app.active().unwrap().draft_text.clone().unwrap())[0]
            .range
            .start;
        let rect = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new((
                    "markdown_block",
                    file_id,
                    at,
                )))
            })
            .unwrap();
        click(&ctx, &mut app, rect.center());
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        assert!(
            app.active().unwrap().markdown.editing.is_some(),
            "clicking the block enters code editing"
        );
        let id = egui::Id::new(("markdown_code_lang", file_id, at));
        let mut state = egui::TextEdit::load_state(&ctx, id).unwrap_or_default();
        state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(0),
            egui::text::CCursor::new(4),
        )));
        state.store(&ctx, id);
        ctx.memory_mut(|memory| memory.request_focus(id));
        draw(&ctx, &mut app, vec![]);
        draw(&ctx, &mut app, vec![egui::Event::Text("js".into())]);
        for _ in 0..2 {
            draw(&ctx, &mut app, vec![]);
        }
        assert!(
            app.active().unwrap().markdown.editing.is_some(),
            "typing a language must not close the code editor"
        );
        let text = app.active().unwrap().draft_text.clone().unwrap();
        assert_eq!(text.lines().next(), Some("```js"), "{text:?}");
    }

    #[test]
    fn outline_lists_headings_and_scrolls_to_the_clicked_one() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        let source = (0..24)
            .map(|index| format!("## Section {index}\n\nParagraph {index}.\n\n"))
            .collect::<String>();
        app.active_mut().unwrap().draft_text = Some(source);
        app.active_mut().unwrap().sync_draft();
        app.active_mut().unwrap().markdown_mode = Mode::Live;
        let mut output = None;
        for _ in 0..3 {
            output = Some(draw_width(&ctx, &mut app, vec![], 1400.0));
        }
        let output = output.unwrap();
        assert!(find_text_where(&output, |text, _| {
            matches!(text, "目录" | "Outline" | "Table of contents")
        }).is_none(), "the outline must contain only heading entries");
        let (pos, size) = find_text_where(&output, |text, pos| {
            text == "Section 12" && pos.x > 1100.0
        })
        .expect("wide live view must show the right-hand outline");
        for _ in 0..2 {
            draw_width(&ctx, &mut app, vec![], 1400.0);
        }
        click_width(&ctx, &mut app, pos + size * 0.5, 1400.0);
        for _ in 0..3 {
            draw_width(&ctx, &mut app, vec![], 1400.0);
        }
        let file_id = app.active_id.unwrap();
        let offset = ctx
            .data(|data| {
                data.get_temp::<f32>(egui::Id::new((
                    "markdown_live_scroll_y",
                    file_id,
                )))
            })
            .unwrap();
        assert!(
            offset > 100.0,
            "clicking an outline item must scroll to that heading: {offset}"
        );
        assert!(app.active().unwrap().markdown.toc_target.is_none());
        let output = draw_width(&ctx, &mut app, vec![], 1400.0);
        let active = output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.pos.x > 1100.0
                && text.galley.job.sections.iter().any(|section| {
                    section.format.color == crate::DARK.accent
                }) => Some(text.galley.text()),
            _ => None,
        });
        assert_eq!(active, Some("Section 12"), "jumped-to heading must become highlighted");
    }

    #[test]
    fn outline_tracks_wheel_scrolling_without_clicking() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        let source = (0..24)
            .map(|index| format!("## Section {index}\n\n{}\n\n", format!("Paragraph {index}. ").repeat(40)))
            .collect::<String>();
        let headings: Vec<_> = parse(&source).into_iter()
            .filter(|block| matches!(block.kind, Kind::Heading(_)))
            .map(|block| block.range.start)
            .collect();
        app.active_mut().unwrap().draft_text = Some(source);
        app.active_mut().unwrap().sync_draft();
        app.active_mut().unwrap().markdown_mode = Mode::Live;
        let highlighted = |output: &egui::FullOutput| {
            output.shapes.iter().filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.pos.x > 1100.0
                    && text.galley.job.sections.iter().any(|section| {
                        section.format.color == crate::DARK.accent
                    }) => Some(text.galley.text().to_owned()),
                _ => None,
            }).collect::<Vec<_>>()
        };
        for _ in 0..3 {
            draw_width(&ctx, &mut app, vec![], 1400.0);
        }
        let output = draw_width(&ctx, &mut app, vec![], 1400.0);
        assert_eq!(highlighted(&output), vec!["Section 0"]);
        let file_id = app.active_id.unwrap();
        let viewport = ctx.data(|data| data.get_temp::<egui::Rect>(
            egui::Id::new(("markdown_live_viewport", file_id)),
        )).unwrap();
        for delta in [-600.0, 600.0] {
            draw_width(&ctx, &mut app, vec![
                egui::Event::PointerMoved(viewport.center()),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, delta),
                    modifiers: egui::Modifiers::NONE,
                },
            ], 1400.0);
            for _ in 0..12 {
                draw_width(&ctx, &mut app, vec![], 1400.0);
            }
            let output = draw_width(&ctx, &mut app, vec![], 1400.0);
            let expected = ctx.data(|data| {
                headings.iter().enumerate().rev().find_map(|(index, at)| {
                    let rect = data.get_temp::<egui::Rect>(egui::Id::new((
                        "markdown_block", file_id, *at,
                    ))).unwrap();
                    (rect.top() <= viewport.top() + viewport.height() * 0.2).then_some(index)
                }).unwrap_or(0)
            });
            if delta < 0.0 {
                assert!(expected > 0, "wheel scrolling must reach a later section");
            } else {
                assert_eq!(expected, 0, "scrolling back must restore the first section");
            }
            assert_eq!(highlighted(&output), vec![format!("Section {expected}")]);
            assert!(app.active().unwrap().markdown.toc_target.is_none());
        }
    }

    #[test]
    fn outline_hysteresis_stabilizes_close_headings_and_clicks() {
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 500.0));
        let mut state = OutlineState::default();
        assert_eq!(state.update(&[(0, 0.0), (1, 110.0), (2, 140.0)], viewport, false, None, false), Some(0));
        assert_eq!(state.update(&[(0, -20.0), (1, 95.0), (2, 125.0)], viewport, false, None, true), Some(0));
        assert_eq!(state.update(&[(0, -30.0), (1, 85.0), (2, 115.0)], viewport, false, None, true), Some(1));
        // Reversing a few points around the boundary keeps the current item.
        assert_eq!(state.update(&[(0, -10.0), (1, 102.0), (2, 132.0)], viewport, false, None, true), Some(1));
        assert_eq!(state.update(&[(0, 0.0), (1, 120.0), (2, 150.0)], viewport, false, None, true), Some(0));
        state.clicked = Some(1);
        assert_eq!(state.update(&[(0, -300.0), (1, -200.0), (2, 20.0)], viewport, true, None, false), Some(1));
        assert_eq!(state.update(&[(0, -300.0), (1, -200.0), (2, 20.0)], viewport, true, None, true), Some(2));
        assert_eq!(state.update(&[(0, -300.0), (1, -200.0), (2, 20.0)], viewport, true, Some(0), false), Some(0));
    }

    #[test]
    fn outline_highlights_nested_headings_at_bottom_and_while_editing() {
        let headings = [(0, -100.0), (100, 140.0), (200, 300.0), (300, 800.0)];
        let viewport = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(800.0, 600.0));
        assert_eq!(outline_heading_for_view(&headings, viewport, false, None), Some(0));
        assert_eq!(outline_heading_for_view(&headings, viewport, true, None), Some(200));
        assert_eq!(outline_heading_for_view(&headings, viewport, true, Some(100)), Some(100));
        assert_eq!(outline_heading_for_view(&[], viewport, false, None), None);
    }

    #[test]
    fn outline_reading_line_keeps_the_current_section_between_headings() {
        let headings = [(0, 30.0), (100, 300.0), (200, 700.0)];
        assert_eq!(active_outline_heading(&[], 0.0), None);
        assert_eq!(active_outline_heading(&headings, 0.0), Some(0));
        assert_eq!(active_outline_heading(&headings, 200.0), Some(0));
        assert_eq!(active_outline_heading(&headings, 282.0), Some(100));
        assert_eq!(active_outline_heading(&headings, 600.0), Some(100));
        assert_eq!(active_outline_heading(&headings, 900.0), Some(200));
    }

    #[test]
    fn split_view_scrolls_both_panes_together() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        let file = app.active_mut().unwrap();
        file.draft_text = Some(
            (0..80)
                .map(|index| format!("## Section {index}\n\nParagraph {index} with **bold** text.\n\n"))
                .collect(),
        );
        file.markdown_mode = Mode::Split;
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let file_id = app.active_id.unwrap();
        let right = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new(("markdown_split_right", file_id)))
            })
            .expect("split view must publish its right pane");
        let target = right.center();
        draw(&ctx, &mut app, vec![egui::Event::PointerMoved(target)]);
        // Stay away from the bottom clamp: the two panes measure their content
        // independently and can differ by a few pixels at the very end.
        for _ in 0..3 {
            draw(
                &ctx,
                &mut app,
                vec![egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -60.0),
                    modifiers: Default::default(),
                }],
            );
        }
        let offset = app.active().unwrap().markdown.split_offset;
        assert!(offset > 0.0, "wheel over the preview must scroll it: {offset}");
        // egui animates wheel scrolling over several frames; settle first so
        // the source pane is compared against a stationary preview.
        for _ in 0..20 {
            draw(&ctx, &mut app, vec![]);
        }
        let left = crate::editor::last_scroll_offset(&ctx, file_id).unwrap_or(-1.0);
        assert!(
            (left - app.active().unwrap().markdown.split_offset).abs() < 2.0,
            "source pane must follow the preview: left {left}, shared {}",
            app.active().unwrap().markdown.split_offset
        );
    }

    #[test]
    fn live_render_makes_headings_visibly_larger_than_body_text() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        app.active_mut().unwrap().markdown_mode = Mode::Live;
        let mut output = None;
        for _ in 0..3 {
            output = Some(draw(&ctx, &mut app, vec![]));
        }
        let (mut heading, mut body) = (0.0_f32, 0.0_f32);
        for shape in output.unwrap().shapes {
            if let egui::Shape::Text(text) = shape.shape {
                if text.galley.text() == "Heading" {
                    heading = text.galley.rect.height();
                }
                if text.galley.text().contains("A paragraph with") {
                    body = text.galley.rect.height();
                }
            }
        }
        assert!(body > 0.0, "body paragraph must be rendered");
        assert!(
            heading > body * 1.5,
            "h1 must be visibly larger: heading {heading}, body {body}"
        );
    }

    #[test]
    fn callouts_render_localized_titles_and_body_text() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        app.active_mut().unwrap().draft_text =
            Some("> [!WARNING]\n> 小心 **这个**\n".into());
        app.active_mut().unwrap().sync_draft();
        app.active_mut().unwrap().markdown_mode = Mode::Live;
        let mut output = None;
        for _ in 0..3 {
            output = Some(draw(&ctx, &mut app, vec![]));
        }
        let texts: Vec<String> = output
            .unwrap()
            .shapes
            .into_iter()
            .filter_map(|shape| match shape.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect();
        assert!(texts.iter().any(|text| text == "警告"), "{texts:?}");
        assert!(texts.iter().any(|text| text == "小心 这个"), "{texts:?}");
        assert!(
            !texts.iter().any(|text| text.contains("[!WARNING]")),
            "the marker line is presentation, not body text: {texts:?}"
        );
    }

    #[test]
    fn fence_shortcut_requires_a_standalone_opener_and_one_trailing_space() {
        let done = fence_shortcut("```py ", 6).expect("a standalone opener completes");
        assert_eq!(done.code_text, "```py\n\n```");
        assert_eq!(done.replacement, "```py\n\n```\n");
        assert_eq!(done.cursor, 6);

        assert!(fence_shortcut("~~~rust ", 8).is_some());
        assert!(fence_shortcut("``` ", 4).is_some(), "an empty info string is valid");
        assert!(fence_shortcut("```py", 5).is_none(), "the space confirms completion");
        assert!(fence_shortcut("text ```py ", 11).is_none(), "only a standalone fence completes");
        assert!(fence_shortcut("    ```py ", 10).is_none(), "deep indentation is prose");
        assert!(
            fence_shortcut("```py\nalready ", 14).is_none(),
            "an existing fenced block must not nest another opener"
        );
    }

    #[test]
    fn fence_shortcut_accepts_enter_after_a_standalone_opener() {
        let done = fence_shortcut("```py\n", 6).expect("Enter after the opener completes");
        assert_eq!(done.code_text, "```py\n\n```");
        assert_eq!(done.replacement, "```py\n\n```\n");
        assert_eq!(done.cursor, 6);

        let with_following = fence_shortcut("alpha\n```py\nomega", 12);
        let done = with_following.expect("the following block must survive");
        assert_eq!(done.replacement, "alpha\n```py\n\n```\nomega");
        assert_eq!(done.code_at, 6);

        assert!(
            fence_shortcut("```py", 5).is_none(),
            "the opener is complete only after Enter or a space"
        );
        assert!(
            fence_shortcut("text ```py\n", 11).is_none(),
            "only a standalone opener completes"
        );
    }

    #[test]
    fn typing_a_fence_opener_and_space_creates_a_complete_code_block() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        app.active_mut().unwrap().draft_text = Some("alpha\n\nomega\n".into());
        app.active_mut().unwrap().sync_draft();
        app.active_mut().unwrap().markdown_mode = Mode::Live;
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let file_id = app.active_id.unwrap();
        let block = parse(&app.active().unwrap().draft_text.clone().unwrap())
            .into_iter()
            .find(|block| block.kind == Kind::Blank && block.range.start > 0)
            .unwrap();
        let rect = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new((
                    "markdown_block",
                    file_id,
                    block.range.start,
                )))
            })
            .unwrap();
        click(&ctx, &mut app, rect.center());
        for _ in 0..2 {
            draw(&ctx, &mut app, vec![]);
        }
        draw(
            &ctx,
            &mut app,
            vec![egui::Event::Text("```py".into())],
        );
        let mut output = None;
        for _ in 0..2 {
            output = Some(draw(&ctx, &mut app, vec![]));
        }
        assert!(
            find_text_where(output.as_ref().unwrap(), |text, _| text == "```py").is_some(),
            "the unfinished fence must stay visible while it is being typed"
        );
        assert!(
            find_text_where(output.as_ref().unwrap(), |text, _| text == "omega").is_some(),
            "an unfinished fence must not hide the following blocks"
        );
        draw(
            &ctx,
            &mut app,
            vec![egui::Event::Text(" ".into())],
        );
        for _ in 0..2 {
            draw(&ctx, &mut app, vec![]);
        }
        let text = app.active().unwrap().draft_text.clone().unwrap();
        assert_eq!(text, "alpha\n```py\n\n```\n\nomega\n", "{text:?}");
        let editing = app
            .active()
            .unwrap()
            .markdown
            .editing
            .as_ref()
            .expect("the completed block stays in edit mode");
        assert_eq!(editing.range, 6..16, "editing owns just the code block");

        draw(
            &ctx,
            &mut app,
            vec![egui::Event::Text("print(1)".into())],
        );
        for _ in 0..2 {
            draw(&ctx, &mut app, vec![]);
        }
        let text = app.active().unwrap().draft_text.clone().unwrap();
        assert!(
            text.contains("```py\nprint(1)\n```"),
            "typed code lands inside the block: {text:?}"
        );
    }

    #[test]
    fn live_edit_keeps_unmatched_backticks_visible() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        app.active_mut().unwrap().draft_text = Some("hello\n\nomega\n".into());
        app.active_mut().unwrap().sync_draft();
        app.active_mut().unwrap().markdown_mode = Mode::Live;
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let file_id = app.active_id.unwrap();
        let paragraph = parse(&app.active().unwrap().draft_text.clone().unwrap())
            .into_iter()
            .find(|block| block.kind == Kind::Paragraph)
            .unwrap();
        let rect = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new((
                    "markdown_block",
                    file_id,
                    paragraph.range.start,
                )))
            })
            .unwrap();
        click(&ctx, &mut app, rect.center());
        for _ in 0..2 {
            draw(&ctx, &mut app, vec![]);
        }
        draw(&ctx, &mut app, vec![egui::Event::Text("`".into())]);
        let mut output = None;
        for _ in 0..2 {
            output = Some(draw(&ctx, &mut app, vec![]));
        }
        assert!(
            !visible_backticks(output.as_ref().unwrap(), "hello").is_empty(),
            "a lone backtick must still be visible"
        );
        draw(&ctx, &mut app, vec![egui::Event::Text("`".into())]);
        let mut output = None;
        for _ in 0..3 {
            output = Some(draw(&ctx, &mut app, vec![]));
        }
        assert!(
            !visible_backticks(output.as_ref().unwrap(), "hello").is_empty(),
            "two unclosed backticks must stay visible while editing"
        );
        draw(&ctx, &mut app, vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        let output = draw(&ctx, &mut app, vec![]);
        assert!(
            !visible_backticks(&output, "hello").is_empty(),
            "two unclosed backticks must stay visible when rendered"
        );
    }

    #[test]
    fn typing_a_fence_opener_and_enter_creates_a_complete_code_block() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        app.active_mut().unwrap().draft_text = Some("alpha\n\nomega\n".into());
        app.active_mut().unwrap().sync_draft();
        app.active_mut().unwrap().markdown_mode = Mode::Live;
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let file_id = app.active_id.unwrap();
        let block = parse(&app.active().unwrap().draft_text.clone().unwrap())
            .into_iter()
            .find(|block| block.kind == Kind::Blank && block.range.start > 0)
            .unwrap();
        let rect = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new((
                    "markdown_block",
                    file_id,
                    block.range.start,
                )))
            })
            .unwrap();
        click(&ctx, &mut app, rect.center());
        for _ in 0..2 {
            draw(&ctx, &mut app, vec![]);
        }
        draw(&ctx, &mut app, vec![egui::Event::Text("```py".into())]);
        for _ in 0..2 {
            draw(&ctx, &mut app, vec![]);
        }
        draw(&ctx, &mut app, vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        for _ in 0..2 {
            draw(&ctx, &mut app, vec![]);
        }
        let text = app.active().unwrap().draft_text.clone().unwrap();
        assert_eq!(text, "alpha\n```py\n\n```\n\nomega\n", "{text:?}");
        let editing = app
            .active()
            .unwrap()
            .markdown
            .editing
            .as_ref()
            .expect("the completed block stays in edit mode");
        assert_eq!(editing.range, 6..16, "editing owns just the code block");
        draw(&ctx, &mut app, vec![egui::Event::Text("print(1)".into())]);
        for _ in 0..2 {
            draw(&ctx, &mut app, vec![]);
        }
        let text = app.active().unwrap().draft_text.clone().unwrap();
        assert!(
            text.contains("```py\nprint(1)\n```"),
            "typed code lands inside the block: {text:?}"
        );
    }

    #[test]
    fn live_context_menu_inserts_a_blank_line_before_the_current_block() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        app.active_mut().unwrap().draft_text = Some("alpha\n\nomega\n".into());
        app.active_mut().unwrap().sync_draft();
        app.active_mut().unwrap().markdown_mode = Mode::Live;
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let file_id = app.active_id.unwrap();
        let paragraph = parse(&app.active().unwrap().draft_text.clone().unwrap())
            .into_iter()
            .find(|block| block.kind == Kind::Paragraph)
            .unwrap();
        let at = paragraph.range.end;
        let rect = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new((
                    "markdown_block",
                    file_id,
                    paragraph.range.start,
                )))
            })
            .unwrap();
        for pressed in [true, false] {
            draw(
                &ctx,
                &mut app,
                vec![
                    egui::Event::PointerMoved(rect.center()),
                    egui::Event::PointerButton {
                        pos: rect.center(),
                        button: egui::PointerButton::Secondary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let header = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new((
                    "markdown_menu_blank_header",
                    at,
                )))
            })
            .expect("the context menu must expose insert blank line");
        for pressed in [true, false] {
            draw(
                &ctx,
                &mut app,
                vec![
                    egui::Event::PointerMoved(header.center()),
                    egui::Event::PointerButton {
                        pos: header.center(),
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        for _ in 0..2 {
            draw(&ctx, &mut app, vec![]);
        }
        let before = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new((
                    "markdown_menu_blank_before",
                    at,
                )))
            })
            .expect("the submenu must expose the before command");
        for pressed in [true, false] {
            draw(
                &ctx,
                &mut app,
                vec![
                    egui::Event::PointerMoved(before.center()),
                    egui::Event::PointerButton {
                        pos: before.center(),
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        for _ in 0..2 {
            draw(&ctx, &mut app, vec![]);
        }
        let text = app.active().unwrap().draft_text.clone().unwrap();
        assert_eq!(text, "\nalpha\n\nomega\n", "{text:?}");
        let editing = app.active().unwrap().markdown.editing.as_ref().unwrap();
        assert_eq!(editing.range, 0..0, "the new blank line is being edited");
        draw(
            &ctx,
            &mut app,
            vec![egui::Event::Text("new".into())],
        );
        for _ in 0..2 {
            draw(&ctx, &mut app, vec![]);
        }
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some("new\nalpha\n\nomega\n"),
            "typing must land on the inserted line"
        );
    }

    #[test]
    fn live_context_menu_inserts_a_blank_line_after_the_current_block() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        app.active_mut().unwrap().draft_text = Some("alpha\n\nomega\n".into());
        app.active_mut().unwrap().sync_draft();
        app.active_mut().unwrap().markdown_mode = Mode::Live;
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let file_id = app.active_id.unwrap();
        let paragraph = parse(&app.active().unwrap().draft_text.clone().unwrap())
            .into_iter()
            .find(|block| block.kind == Kind::Paragraph)
            .unwrap();
        let at = paragraph.range.end;
        let rect = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new((
                    "markdown_block",
                    file_id,
                    paragraph.range.start,
                )))
            })
            .unwrap();
        for pressed in [true, false] {
            draw(
                &ctx,
                &mut app,
                vec![
                    egui::Event::PointerMoved(rect.center()),
                    egui::Event::PointerButton {
                        pos: rect.center(),
                        button: egui::PointerButton::Secondary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let header = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new((
                    "markdown_menu_blank_header",
                    at,
                )))
            })
            .unwrap();
        for pressed in [true, false] {
            draw(
                &ctx,
                &mut app,
                vec![
                    egui::Event::PointerMoved(header.center()),
                    egui::Event::PointerButton {
                        pos: header.center(),
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        for _ in 0..2 {
            draw(&ctx, &mut app, vec![]);
        }
        let after = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new((
                    "markdown_menu_blank_after",
                    at,
                )))
            })
            .expect("the submenu must expose the after command");
        for pressed in [true, false] {
            draw(
                &ctx,
                &mut app,
                vec![
                    egui::Event::PointerMoved(after.center()),
                    egui::Event::PointerButton {
                        pos: after.center(),
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        for _ in 0..2 {
            draw(&ctx, &mut app, vec![]);
        }
        let text = app.active().unwrap().draft_text.clone().unwrap();
        assert_eq!(text, "alpha\n\n\nomega\n", "{text:?}");
        let editing = app.active().unwrap().markdown.editing.as_ref().unwrap();
        assert_eq!(editing.range, 6..6);
        draw(
            &ctx,
            &mut app,
            vec![egui::Event::Text("gap".into())],
        );
        for _ in 0..2 {
            draw(&ctx, &mut app, vec![]);
        }
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some("alpha\ngap\n\nomega\n"),
            "typing must land on the inserted line"
        );
    }

    #[test]
    fn live_edit_context_menu_inserts_a_blank_line_after_the_caret_line() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        app.active_mut().unwrap().draft_text = Some("alpha beta\n\nomega\n".into());
        app.active_mut().unwrap().sync_draft();
        app.active_mut().unwrap().markdown_mode = Mode::Live;
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let file_id = app.active_id.unwrap();
        let paragraph = parse(&app.active().unwrap().draft_text.clone().unwrap())
            .into_iter()
            .find(|block| block.kind == Kind::Paragraph)
            .unwrap();
        let at = paragraph.range.end;
        let rect = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new((
                    "markdown_block",
                    file_id,
                    paragraph.range.start,
                )))
            })
            .unwrap();
        click(&ctx, &mut app, rect.center());
        draw(&ctx, &mut app, vec![]);
        let edit_rect = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new((
                    "markdown_edit_rect",
                    file_id,
                )))
            })
            .expect("editing must be active");
        for pressed in [true, false] {
            draw(
                &ctx,
                &mut app,
                vec![
                    egui::Event::PointerMoved(edit_rect.center()),
                    egui::Event::PointerButton {
                        pos: edit_rect.center(),
                        button: egui::PointerButton::Secondary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let header = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new((
                    "markdown_menu_blank_header",
                    at,
                )))
            })
            .unwrap();
        for pressed in [true, false] {
            draw(
                &ctx,
                &mut app,
                vec![
                    egui::Event::PointerMoved(header.center()),
                    egui::Event::PointerButton {
                        pos: header.center(),
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        for _ in 0..2 {
            draw(&ctx, &mut app, vec![]);
        }
        let after = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new((
                    "markdown_menu_blank_after",
                    at,
                )))
            })
            .unwrap();
        for pressed in [true, false] {
            draw(
                &ctx,
                &mut app,
                vec![
                    egui::Event::PointerMoved(after.center()),
                    egui::Event::PointerButton {
                        pos: after.center(),
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        for _ in 0..2 {
            draw(&ctx, &mut app, vec![]);
        }
        let text = app.active().unwrap().draft_text.clone().unwrap();
        assert_eq!(text, "alpha beta\n\n\nomega\n", "{text:?}");
        let editing = app.active().unwrap().markdown.editing.as_ref().unwrap();
        assert_eq!(editing.range, 11..11);
        draw(
            &ctx,
            &mut app,
            vec![egui::Event::Text("gap".into())],
        );
        for _ in 0..2 {
            draw(&ctx, &mut app, vec![]);
        }
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some("alpha beta\ngap\n\nomega\n"),
            "typing must land on the inserted line"
        );
    }

    #[test]
    fn live_edit_blank_line_uses_the_whole_code_block_not_the_caret_line() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        app.active_mut().unwrap().draft_text =
            Some("```rust\nfn main() {}\n```\nomega\n".into());
        app.active_mut().unwrap().sync_draft();
        app.active_mut().unwrap().markdown_mode = Mode::Live;
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let file_id = app.active_id.unwrap();
        let block = parse(&app.active().unwrap().draft_text.clone().unwrap())
            .into_iter()
            .find(|block| matches!(block.kind, Kind::Code(_)))
            .unwrap();
        let rect = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new((
                    "markdown_block",
                    file_id,
                    block.range.start,
                )))
            })
            .unwrap();
        click(&ctx, &mut app, rect.center());
        for _ in 0..2 {
            draw(&ctx, &mut app, vec![]);
        }
        let edit_rect = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new((
                    "markdown_edit_rect",
                    file_id,
                )))
            })
            .expect("the code block must be editable");
        for pressed in [true, false] {
            draw(
                &ctx,
                &mut app,
                vec![
                    egui::Event::PointerMoved(edit_rect.center()),
                    egui::Event::PointerButton {
                        pos: edit_rect.center(),
                        button: egui::PointerButton::Secondary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let header = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new(
                    "markdown_menu_blank_header_last",
                ))
            })
            .expect("the editor context menu must expose insert blank line");
        for pressed in [true, false] {
            draw(
                &ctx,
                &mut app,
                vec![
                    egui::Event::PointerMoved(header.center()),
                    egui::Event::PointerButton {
                        pos: header.center(),
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        for _ in 0..2 {
            draw(&ctx, &mut app, vec![]);
        }
        let after = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new(
                    "markdown_menu_blank_after_last",
                ))
            })
            .expect("the submenu must expose the after command");
        for pressed in [true, false] {
            draw(
                &ctx,
                &mut app,
                vec![
                    egui::Event::PointerMoved(after.center()),
                    egui::Event::PointerButton {
                        pos: after.center(),
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        for _ in 0..2 {
            draw(&ctx, &mut app, vec![]);
        }
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some("```rust\nfn main() {}\n```\n\nomega\n"),
            "the blank line must go after the whole fenced block"
        );
        let editing = app.active().unwrap().markdown.editing.as_ref().unwrap();
        assert_eq!(editing.range, 25..25);
    }

    #[test]
    fn live_context_menu_inserts_a_table_at_the_clicked_block() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        app.active_mut().unwrap().draft_text = Some("alpha\n\nomega\n".into());
        app.active_mut().unwrap().sync_draft();
        app.active_mut().unwrap().markdown_mode = Mode::Live;
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let file_id = app.active_id.unwrap();
        let text = app.active().unwrap().draft_text.clone().unwrap();
        let paragraph = parse(&text)
            .into_iter()
            .find(|block| block.kind == Kind::Paragraph)
            .unwrap();
        let at = paragraph.range.end;
        let rect = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new((
                    "markdown_block",
                    file_id,
                    paragraph.range.start,
                )))
            })
            .unwrap();
        for pressed in [true, false] {
            draw(
                &ctx,
                &mut app,
                vec![
                    egui::Event::PointerMoved(rect.center()),
                    egui::Event::PointerButton {
                        pos: rect.center(),
                        button: egui::PointerButton::Secondary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let menu = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new(("markdown_menu_table", at)))
            })
            .expect("the context menu must expose the table command");
        for pressed in [true, false] {
            draw(
                &ctx,
                &mut app,
                vec![
                    egui::Event::PointerMoved(menu.center()),
                    egui::Event::PointerButton {
                        pos: menu.center(),
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        for _ in 0..2 {
            draw(&ctx, &mut app, vec![]);
        }
        assert!(
            app.active().unwrap().markdown.table_picker.is_some(),
            "choosing the table command opens the size picker"
        );
        let insert = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new(("markdown_table_insert", at)))
            })
            .expect("the size picker must expose an insert button");
        for pressed in [true, false] {
            draw(
                &ctx,
                &mut app,
                vec![
                    egui::Event::PointerMoved(insert.center()),
                    egui::Event::PointerButton {
                        pos: insert.center(),
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        draw(&ctx, &mut app, vec![]);
        let text = app.active().unwrap().draft_text.clone().unwrap();
        assert!(
            text.starts_with("alpha\n\n| 列 1 |"),
            "the table must land after the right-clicked block: {text:?}"
        );
        assert!(text.contains("| --- | --- | --- |"), "{text:?}");
        assert!(app.active().unwrap().markdown.table_picker.is_none());
    }

    #[test]
    fn live_context_menu_copies_and_cuts_the_clicked_block() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        app.active_mut().unwrap().draft_text = Some("alpha\n\nomega\n".into());
        app.active_mut().unwrap().sync_draft();
        app.active_mut().unwrap().markdown_mode = Mode::Live;
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let file_id = app.active_id.unwrap();
        let paragraph = parse(&app.active().unwrap().draft_text.clone().unwrap())
            .into_iter()
            .find(|block| block.kind == Kind::Paragraph)
            .unwrap();
        let at = paragraph.range.end;
        let rect = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new((
                    "markdown_block",
                    file_id,
                    paragraph.range.start,
                )))
            })
            .unwrap();
        let open_menu = |ctx: &egui::Context, app: &mut crate::LiteTextApp| {
            for pressed in [true, false] {
                draw(
                    ctx,
                    app,
                    vec![
                        egui::Event::PointerMoved(rect.center()),
                        egui::Event::PointerButton {
                            pos: rect.center(),
                            button: egui::PointerButton::Secondary,
                            pressed,
                            modifiers: Default::default(),
                        },
                    ],
                );
            }
            for _ in 0..3 {
                draw(ctx, app, vec![]);
            }
        };
        open_menu(&ctx, &mut app);
        let copy = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new(("markdown_menu_copy", at)))
            })
            .expect("the context menu must expose copy");
        let mut output = None;
        for pressed in [true, false] {
            output = Some(draw(
                &ctx,
                &mut app,
                vec![
                    egui::Event::PointerMoved(copy.center()),
                    egui::Event::PointerButton {
                        pos: copy.center(),
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            ));
        }
        assert!(
            output.unwrap().platform_output.commands.iter().any(
                |command| matches!(command, egui::OutputCommand::CopyText(text) if text == "alpha")
            ),
            "copy must put the whole block's Markdown on the clipboard"
        );
        open_menu(&ctx, &mut app);
        let cut = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new(("markdown_menu_cut", at)))
            })
            .expect("the context menu must expose cut");
        for pressed in [true, false] {
            draw(
                &ctx,
                &mut app,
                vec![
                    egui::Event::PointerMoved(cut.center()),
                    egui::Event::PointerButton {
                        pos: cut.center(),
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        for _ in 0..2 {
            draw(&ctx, &mut app, vec![]);
        }
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some("omega\n"),
            "cut must remove the block and its blank-line separator"
        );
    }

    #[test]
    fn live_context_menu_pastes_after_the_clicked_block() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        app.active_mut().unwrap().draft_text = Some("alpha\n\nomega\n".into());
        app.active_mut().unwrap().sync_draft();
        app.active_mut().unwrap().markdown_mode = Mode::Live;
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let file_id = app.active_id.unwrap();
        let paragraph = parse(&app.active().unwrap().draft_text.clone().unwrap())
            .into_iter()
            .find(|block| block.kind == Kind::Paragraph)
            .unwrap();
        let at = paragraph.range.end;
        let rect = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new((
                    "markdown_block",
                    file_id,
                    paragraph.range.start,
                )))
            })
            .unwrap();
        for pressed in [true, false] {
            draw(
                &ctx,
                &mut app,
                vec![
                    egui::Event::PointerMoved(rect.center()),
                    egui::Event::PointerButton {
                        pos: rect.center(),
                        button: egui::PointerButton::Secondary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let paste = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new(("markdown_menu_paste", at)))
            })
            .expect("the context menu must expose paste");
        let mut output = None;
        for pressed in [true, false] {
            output = Some(draw(
                &ctx,
                &mut app,
                vec![
                    egui::Event::PointerMoved(paste.center()),
                    egui::Event::PointerButton {
                        pos: paste.center(),
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            ));
        }
        assert!(
            output.unwrap().viewport_output.values().any(|viewport| viewport
                .commands
                .contains(&egui::ViewportCommand::RequestPaste)),
            "paste must ask the platform for clipboard contents"
        );
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some("alpha\n\nomega\n"),
            "nothing changes until the platform delivers the text"
        );
        draw(
            &ctx,
            &mut app,
            vec![egui::Event::Paste("beta\ngamma".into())],
        );
        for _ in 0..2 {
            draw(&ctx, &mut app, vec![]);
        }
        let text = app.active().unwrap().draft_text.clone().unwrap();
        assert!(text.contains("alpha\n\nbeta\ngamma"), "{text:?}");
        assert!(app.active().unwrap().markdown.paste_at.is_none());
    }

    #[test]
    fn live_edit_context_menu_inserts_at_the_caret() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        app.active_mut().unwrap().draft_text = Some("alpha beta\n\nomega\n".into());
        app.active_mut().unwrap().sync_draft();
        app.active_mut().unwrap().markdown_mode = Mode::Live;
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let file_id = app.active_id.unwrap();
        let text = app.active().unwrap().draft_text.clone().unwrap();
        let paragraph = parse(&text)
            .into_iter()
            .find(|block| block.kind == Kind::Paragraph)
            .unwrap();
        let at = paragraph.range.end;
        let rect = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new((
                    "markdown_block",
                    file_id,
                    paragraph.range.start,
                )))
            })
            .unwrap();
        for pressed in [true, false] {
            draw(
                &ctx,
                &mut app,
                vec![
                    egui::Event::PointerMoved(rect.center()),
                    egui::Event::PointerButton {
                        pos: rect.center(),
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        draw(&ctx, &mut app, vec![]);
        draw(
            &ctx,
            &mut app,
            vec![egui::Event::Key {
                key: egui::Key::End,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        draw(&ctx, &mut app, vec![]);
        let edit_rect = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new((
                    "markdown_edit_rect",
                    file_id,
                )))
            })
            .expect("editing must be active");
        for pressed in [true, false] {
            draw(
                &ctx,
                &mut app,
                vec![
                    egui::Event::PointerMoved(edit_rect.center()),
                    egui::Event::PointerButton {
                        pos: edit_rect.center(),
                        button: egui::PointerButton::Secondary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let menu = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new(("markdown_menu_table", at)))
            })
            .expect("the editor must expose the insert menu");
        for pressed in [true, false] {
            draw(
                &ctx,
                &mut app,
                vec![
                    egui::Event::PointerMoved(menu.center()),
                    egui::Event::PointerButton {
                        pos: menu.center(),
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        for _ in 0..2 {
            draw(&ctx, &mut app, vec![]);
        }
        let insert = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new(("markdown_table_insert", at)))
            })
            .expect("the size picker must be open");
        for pressed in [true, false] {
            draw(
                &ctx,
                &mut app,
                vec![
                    egui::Event::PointerMoved(insert.center()),
                    egui::Event::PointerButton {
                        pos: insert.center(),
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        draw(&ctx, &mut app, vec![]);
        let text = app.active().unwrap().draft_text.clone().unwrap();
        assert!(
            text.starts_with("alpha beta\n\n| 列 1 |"),
            "the table must be inserted at the caret: {text:?}"
        );
        assert!(
            app.active().unwrap().markdown.editing.is_none(),
            "inserting closes the editor that owned the caret"
        );
    }

    #[test]
    fn live_edit_context_menu_inserts_the_current_time_at_the_caret() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        app.active_mut().unwrap().draft_text = Some("alpha beta\n\nomega\n".into());
        app.active_mut().unwrap().sync_draft();
        app.active_mut().unwrap().markdown_mode = Mode::Live;
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let file_id = app.active_id.unwrap();
        let text = app.active().unwrap().draft_text.clone().unwrap();
        let paragraph = parse(&text)
            .into_iter()
            .find(|block| block.kind == Kind::Paragraph)
            .unwrap();
        let rect = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new((
                    "markdown_block",
                    file_id,
                    paragraph.range.start,
                )))
            })
            .unwrap();
        // Enter the inline editor, then move the caret to the end of the block.
        for pressed in [true, false] {
            draw(
                &ctx,
                &mut app,
                vec![
                    egui::Event::PointerMoved(rect.center()),
                    egui::Event::PointerButton {
                        pos: rect.center(),
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::default(),
                    },
                ],
            );
        }
        draw(&ctx, &mut app, vec![]);
        draw(
            &ctx,
            &mut app,
            vec![egui::Event::Key {
                key: egui::Key::End,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            }],
        );
        draw(&ctx, &mut app, vec![]);
        let edit_rect = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new(("markdown_edit_rect", file_id)))
            })
            .expect("editing must be active");
        for pressed in [true, false] {
            draw(
                &ctx,
                &mut app,
                vec![
                    egui::Event::PointerMoved(edit_rect.center()),
                    egui::Event::PointerButton {
                        pos: edit_rect.center(),
                        button: egui::PointerButton::Secondary,
                        pressed,
                        modifiers: egui::Modifiers::default(),
                    },
                ],
            );
        }
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let header = ctx
            .data(|data| data.get_temp::<egui::Rect>(egui::Id::new("insert_time_header")))
            .expect("the context menu must expose the time submenu");
        for pressed in [true, false] {
            draw(
                &ctx,
                &mut app,
                vec![
                    egui::Event::PointerMoved(header.center()),
                    egui::Event::PointerButton {
                        pos: header.center(),
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::default(),
                    },
                ],
            );
        }
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let template = "YYYY-mm-dd";
        let item = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new(("insert_time_template", template)))
            })
            .expect("the submenu must list the templates");
        for pressed in [true, false] {
            draw(
                &ctx,
                &mut app,
                vec![
                    egui::Event::PointerMoved(item.center()),
                    egui::Event::PointerButton {
                        pos: item.center(),
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::default(),
                    },
                ],
            );
        }
        draw(&ctx, &mut app, vec![]);
        let text = app.active().unwrap().draft_text.clone().unwrap();
        let stamped = regex::Regex::new(r"^alpha beta\d{4}-\d{2}-\d{2}\n\nomega\n$").unwrap();
        assert!(
            stamped.is_match(&text),
            "the stamp must be inserted at the caret: {text:?}"
        );
        assert!(
            app.active().unwrap().markdown.editing.is_none(),
            "inserting closes the editor that owned the caret"
        );
    }

    #[test]
    fn live_context_menu_inserts_a_callout_at_the_clicked_block() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        app.active_mut().unwrap().draft_text = Some("alpha\n\nomega\n".into());
        app.active_mut().unwrap().sync_draft();
        app.active_mut().unwrap().markdown_mode = Mode::Live;
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let file_id = app.active_id.unwrap();
        let text = app.active().unwrap().draft_text.clone().unwrap();
        let paragraph = parse(&text)
            .into_iter()
            .find(|block| block.kind == Kind::Paragraph)
            .unwrap();
        let at = paragraph.range.end;
        let rect = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new((
                    "markdown_block",
                    file_id,
                    paragraph.range.start,
                )))
            })
            .unwrap();
        for pressed in [true, false] {
            draw(
                &ctx,
                &mut app,
                vec![
                    egui::Event::PointerMoved(rect.center()),
                    egui::Event::PointerButton {
                        pos: rect.center(),
                        button: egui::PointerButton::Secondary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let callout = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new((
                    "markdown_menu_callout_header",
                    at,
                )))
            })
            .expect("the context menu must expose the callout submenu");
        for pressed in [true, false] {
            draw(
                &ctx,
                &mut app,
                vec![
                    egui::Event::PointerMoved(callout.center()),
                    egui::Event::PointerButton {
                        pos: callout.center(),
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let warning = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new((
                    "markdown_menu_callout",
                    at,
                    "WARNING",
                )))
            })
            .expect("the submenu must list the warning callout");
        for pressed in [true, false] {
            draw(
                &ctx,
                &mut app,
                vec![
                    egui::Event::PointerMoved(warning.center()),
                    egui::Event::PointerButton {
                        pos: warning.center(),
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        draw(&ctx, &mut app, vec![]);
        let text = app.active().unwrap().draft_text.clone().unwrap();
        assert!(
            text.starts_with("alpha\n\n> [!WARNING]\n> 在这里输入警告内容"),
            "{text:?}"
        );
        assert!(text.ends_with("\n\nomega\n"), "{text:?}");
    }

    #[test]
    fn mixed_heading_glyph_positions_match_live_preview_and_edit() {
        // Compare actual laid-out glyphs (including the CJK fallback), not just
        // TextShape origins: hidden syntax can move the visible run within it.
        fn glyphs(output: &egui::FullOutput, source: &str) -> Vec<(char, egui::Pos2, f32)> {
            let text = output.shapes.iter().find_map(|shape| {
                if let egui::Shape::Text(text) = &shape.shape {
                    (text.galley.text() == source).then_some(text)
                } else {
                    None
                }
            }).unwrap_or_else(|| panic!("missing heading {source:?}"));
            text.galley.rows.iter().flat_map(|row| {
                row.glyphs.iter().filter(|glyph| !glyph.chr.is_whitespace() && glyph.chr != '#')
                    .map(|glyph| (glyph.chr, text.pos + glyph.pos.to_vec2(), glyph.advance_width))
            }).collect()
        }

        for level in 1..=4 {
            for (title, scale, font_size) in [
                ("打开app-享受底层", 1.0, 13.5),
                ("English中文 app测试", 1.0, 13.5),
                ("打开app-享受底层", 2.0, 13.5),
                ("打开app-享受底层", 1.0, 16.0),
                ("打开app-享受底层", 2.0, 20.0),
            ] {
                let ctx = egui::Context::default();
                ctx.set_pixels_per_point(scale);
                crate::setup_fonts(&ctx);
                let mut app = markdown_app();
                let source = format!("{} {title}", "#".repeat(level));
                {
                    let file = app.active_mut().unwrap();
                    file.editor_state.font_size = font_size;
                    file.draft_text = Some(format!("{source}\n\nbody\n"));
                    file.sync_draft();
                    file.markdown_mode = Mode::Live;
                }
                for _ in 0..2 { draw(&ctx, &mut app, vec![]); }
                let output = draw(&ctx, &mut app, vec![]);
                let before = glyphs(&output, title);
                let block = parse(&source).remove(0);
                let file_id = app.active_id.unwrap();
                let rect = ctx.data(|data| data.get_temp::<egui::Rect>(egui::Id::new((
                    "markdown_block", file_id, block.range.start,
                )))).unwrap();
                let left = before.first().unwrap().1.x;
                let right = before.last().unwrap().1.x + before.last().unwrap().2;
                assert!(((left + right) * 0.5 - rect.center().x).abs() < 1.0,
                    "H{level} glyphs must be centered: {left}..{right}, block {rect:?}");
                // Exercise the actual Live transition as well as glyph layout.
                click(&ctx, &mut app, rect.center());
                draw(&ctx, &mut app, vec![]);
                let output = draw(&ctx, &mut app, vec![]);
                let after = glyphs(&output, &source);
                assert_eq!(before.len(), after.len());
                for (preview, edit) in before.iter().zip(&after) {
                    assert_eq!(preview.0, edit.0);
                    assert!((preview.2 - edit.2).abs() < 0.01,
                        "H{level} {:?} advance jumped: {preview:?} -> {edit:?}", preview.0);
                    // Font fallback and pixel snapping differ between OSes.
                    // Allow a subpixel displacement, but still catch the old
                    // half-pixel centering regression at every display scale.
                    let delta_pixels = preview.1.distance(edit.1) * scale;
                    assert!(delta_pixels < 0.25,
                        "H{level} {:?} position jumped by {delta_pixels:.6}px (scale={scale}, size={font_size}): ({:.6}, {:.6}) -> ({:.6}, {:.6})",
                        preview.0, preview.1.x, preview.1.y, edit.1.x, edit.1.y);
                }
            }
        }
    }

    #[test]
    fn live_headings_are_centered_in_the_reading_column() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        app.active_mut().unwrap().markdown_mode = Mode::Live;
        let mut output = None;
        for _ in 0..3 {
            output = Some(draw(&ctx, &mut app, vec![]));
        }
        let (mut heading, mut paragraph) = (None, None);
        for shape in output.unwrap().shapes {
            if let egui::Shape::Text(text) = shape.shape {
                let center = text.pos.x + text.galley.rect.center().x;
                if text.galley.text() == "Heading" {
                    heading = Some(center);
                }
                if text.galley.text().contains("A paragraph with") {
                    paragraph = Some(center);
                }
            }
        }
        let heading = heading.expect("heading must be rendered");
        let paragraph = paragraph.expect("paragraph must be rendered");
        assert!(
            heading > paragraph + 80.0,
            "headings must be centered, not left-aligned with body text: heading {heading}, paragraph {paragraph}"
        );
        assert!(
            (heading - 500.0).abs() < 30.0,
            "heading should sit near the reading-column center: {heading}"
        );
    }

    #[test]
    fn live_and_split_modes_render_without_touching_the_text() {
        for mode in [Mode::Live, Mode::Split] {
            let ctx = egui::Context::default();
            crate::setup_fonts(&ctx);
            let mut app = markdown_app();
            let original = app.active().unwrap().draft_text.clone().unwrap();
            app.active_mut().unwrap().markdown_mode = mode;
            for _ in 0..3 {
                draw(&ctx, &mut app, vec![]);
            }
            let file = app.active().unwrap();
            assert_eq!(file.draft_text.as_deref(), Some(original.as_str()));
            assert!(file.markdown.editing.is_none());
        }
    }

    #[test]
    fn clicking_a_live_block_edits_and_writes_back() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        app.active_mut().unwrap().markdown_mode = Mode::Live;
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let file_id = app.active_id.unwrap();
        let blocks = parse(app.active().unwrap().draft_text.as_deref().unwrap());
        let paragraph = blocks
            .iter()
            .find(|block| block.kind == Kind::Paragraph)
            .unwrap();
        let rect = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new((
                    "markdown_block",
                    file_id,
                    paragraph.range.start,
                )))
            })
            .expect("paragraph must be rendered");
        let target = rect.center();
        for pressed in [true, false] {
            draw(
                &ctx,
                &mut app,
                vec![
                    egui::Event::PointerMoved(target),
                    egui::Event::PointerButton {
                        pos: target,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        draw(&ctx, &mut app, vec![]);
        assert!(
            app.active().unwrap().markdown.editing.is_some(),
            "clicking a rendered block must start source editing"
        );
        let edit_text = ctx
            .data(|data| {
                data.get_temp::<String>(egui::Id::new((
                    "markdown_edit_job_text",
                    file_id,
                )))
            })
            .expect("the editing layouter must run");
        assert!(
            edit_text.contains("**bold**") && edit_text.contains("`code`"),
            "editing must keep the raw markers while styling them: {edit_text:?}"
        );
        draw(&ctx, &mut app, vec![egui::Event::Text("XY".to_owned())]);
        draw(&ctx, &mut app, vec![]);
        let text = app.active().unwrap().draft_text.clone().unwrap();
        assert!(
            text.contains("XY"),
            "typed text must reach the draft buffer: {text}"
        );
    }

    #[test]
    fn clicking_a_live_block_places_the_caret_at_the_click() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        app.active_mut().unwrap().draft_text = Some("alpha beta gamma\n".into());
        app.active_mut().unwrap().sync_draft();
        app.active_mut().unwrap().markdown_mode = Mode::Live;
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let file_id = app.active_id.unwrap();
        let blocks = parse(app.active().unwrap().draft_text.as_deref().unwrap());
        let paragraph = blocks
            .iter()
            .find(|block| block.kind == Kind::Paragraph)
            .unwrap();
        let rect = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new((
                    "markdown_block",
                    file_id,
                    paragraph.range.start,
                )))
            })
            .expect("paragraph must be rendered");
        let target = egui::pos2(rect.left() + 1.0, rect.top() + 2.0);
        for pressed in [true, false] {
            draw(
                &ctx,
                &mut app,
                vec![
                    egui::Event::PointerMoved(target),
                    egui::Event::PointerButton {
                        pos: target,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        draw(&ctx, &mut app, vec![]);
        let edit_rect = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new((
                    "markdown_edit_rect",
                    file_id,
                )))
            })
            .expect("the editing widget must be laid out");
        assert!(
            edit_rect.height() <= rect.height() + 4.0,
            "editing a paragraph must not add a tall empty area: rendered {} vs editing {}",
            rect.height(),
            edit_rect.height()
        );
        draw(&ctx, &mut app, vec![egui::Event::Text("Z".to_owned())]);
        draw(&ctx, &mut app, vec![]);
        let text = app.active().unwrap().draft_text.clone().unwrap();
        assert!(
            text.starts_with("Zalpha"),
            "typing must start where the click landed: {text:?}"
        );
    }

    #[test]
    fn enter_in_a_live_list_continues_the_marker() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        app.active_mut().unwrap().draft_text = Some("- one\n- two\n\nparagraph\n".into());
        app.active_mut().unwrap().sync_draft();
        app.active_mut().unwrap().markdown_mode = Mode::Live;
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let file_id = app.active_id.unwrap();
        let blocks = parse(app.active().unwrap().draft_text.as_deref().unwrap());
        let list = blocks
            .iter()
            .find(|block| matches!(block.kind, Kind::List { .. }))
            .unwrap();
        let rect = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new((
                    "markdown_block",
                    file_id,
                    list.range.start,
                )))
            })
            .expect("list must be rendered");
        let target = rect.center();
        for pressed in [true, false] {
            draw(
                &ctx,
                &mut app,
                vec![
                    egui::Event::PointerMoved(target),
                    egui::Event::PointerButton {
                        pos: target,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        draw(&ctx, &mut app, vec![]);
        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        };
        draw(&ctx, &mut app, vec![key(egui::Key::End)]);
        draw(&ctx, &mut app, vec![]);
        draw(&ctx, &mut app, vec![key(egui::Key::Enter)]);
        draw(&ctx, &mut app, vec![egui::Event::Text("three".into())]);
        draw(&ctx, &mut app, vec![]);
        let text = app.active().unwrap().draft_text.clone().unwrap();
        assert!(
            text.contains("- three"),
            "Enter in a list item must continue the marker: {text:?}"
        );
    }

    #[test]
    fn picker_is_only_available_for_markdown_and_opens_upwards() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        let file_id = app.active_id.unwrap();
        for _ in 0..2 {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000.0, 720.0),
                    )),
                    ..Default::default()
                },
                |ctx| app.status_bar(ctx, &crate::DARK),
            );
        }
        let id = egui::Id::new(("markdown_mode", file_id));
        let rect = ctx
            .data(|data| data.get_temp::<egui::Rect>(id.with("rect")))
            .expect("Markdown tabs need a view picker");
        assert_eq!(app.active().unwrap().markdown_mode, Mode::Source);
        let ending = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new("status_line_ending").with("rect"))
            })
            .expect("line-ending picker must be rendered");
        assert!(
            rect.left() > ending.right(),
            "Markdown picker must sit to the right of the TAB/LF controls"
        );
        let target = rect.center() - egui::vec2(0.0, 9.0);
        for pressed in [true, false] {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000.0, 720.0),
                    )),
                    events: vec![
                        egui::Event::PointerMoved(target),
                        egui::Event::PointerButton {
                            pos: target,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: Default::default(),
                        },
                    ],
                    ..Default::default()
                },
                |ctx| app.status_bar(ctx, &crate::DARK),
            );
        }
        let mut output = None;
        for _ in 0..2 {
            output = Some(ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000.0, 720.0),
                    )),
                    ..Default::default()
                },
                |ctx| app.status_bar(ctx, &crate::DARK),
            ));
        }
        let output = output.unwrap();
        assert!(ctx.memory(|memory| memory.is_popup_open(id)));
        let popup = ctx
            .data(|data| data.get_temp::<egui::Rect>(id.with("popup_rect")))
            .unwrap();
        assert!(
            popup.bottom() <= rect.top(),
            "Markdown menu must open upwards"
        );
        assert!(
            popup.width() < 150.0,
            "Markdown menu should fit its labels, got {}",
            popup.width()
        );
        // The collapsed picker icon uses the theme accent, not the faint
        // status-bar text color.
        assert!(
            output.shapes.iter().any(|shape| matches!(
                &shape.shape,
                egui::Shape::LineSegment { stroke, .. } if stroke.color == crate::DARK.accent
            )),
            "Markdown picker icon must be theme-colored"
        );
        let live = ctx
            .data(|data| data.get_temp::<egui::Rect>(id.with(Mode::Live.label(false))))
            .expect("Live render row must be laid out");
        let live_target = live.center();
        for pressed in [true, false] {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000.0, 720.0),
                    )),
                    events: vec![
                        egui::Event::PointerMoved(live_target),
                        egui::Event::PointerButton {
                            pos: live_target,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: Default::default(),
                        },
                    ],
                    ..Default::default()
                },
                |ctx| app.status_bar(ctx, &crate::DARK),
            );
        }
        assert_eq!(app.active().unwrap().markdown_mode, Mode::Live);
        assert_eq!(
            app.markdown_mode,
            Mode::Live,
            "the chosen mode must be remembered for new files"
        );

        // A plain text file must not show the picker. Use a fresh context so a
        // stale temp rect from the Markdown frame cannot mask the result.
        app.active_mut().unwrap().path = std::path::PathBuf::from("test.txt");
        app.active_mut().unwrap().name = "test.txt".to_owned();
        let text_ctx = egui::Context::default();
        crate::setup_fonts(&text_ctx);
        for _ in 0..2 {
            let _ = text_ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000.0, 720.0),
                    )),
                    ..Default::default()
                },
                |ctx| app.status_bar(ctx, &crate::DARK),
            );
        }
        let hidden = text_ctx
            .data(|data| data.get_temp::<egui::Rect>(id.with("rect")))
            .is_none();
        assert!(hidden, "text files must not get a Markdown view picker");
    }

    #[test]
    fn split_preview_page_is_wider_than_the_longest_line() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        let long = "let value = some_function(argument_one, argument_two, argument_three, argument_four, argument_five, argument_six, argument_seven, argument_eight);";
        let file = app.active_mut().unwrap();
        file.draft_text = Some(format!(
            "# Long lines\n\n```rust\n{long}\n```\n\n| a | b |\n|---|---|\n| {long} | x |\n"
        ));
        file.sync_draft();
        app.active_mut().unwrap().markdown_mode = Mode::Split;
        let mut output = None;
        for _ in 0..3 {
            output = Some(draw(&ctx, &mut app, vec![]));
        }
        let file_id = app.active_id.unwrap();
        let (content, inner, _) = ctx
            .data(|data| {
                data.get_temp::<(egui::Vec2, egui::Rect, f32)>(egui::Id::new((
                    "markdown_split_geometry",
                    file_id,
                )))
            })
            .expect("split preview must publish its scroll geometry");
        let cjk = crate::markdown::body_size(app.active().unwrap());
        let mut widest = 0.0_f32;
        let mut lines = 0;
        for shape in output.unwrap().shapes {
            if let egui::Shape::Text(text) = shape.shape
                // Ignore the source pane, which contains the same text.
                && text.pos.x >= inner.left() - 1.0
                && text.galley.text().contains("some_function")
            {
                widest = widest.max(text.pos.x - inner.left() + text.galley.size().x);
                lines += 1;
            }
        }
        // Code is visible; a grid row below the viewport is intentionally
        // culled. Validate the table's non-wrapping galley independently.
        assert!(lines >= 1, "preview long lines were not measured: {lines}");
        let source = app.active().unwrap().draft_text.as_ref().unwrap();
        let table = parse(source).into_iter().find(|b| b.kind == Kind::Table).unwrap();
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let layout = table_layout(ui, &source[table.range.clone()], cjk, &crate::DARK, None);
                assert_eq!(layout.rows[1][0].galley.rows.len(), 1);
                let expected = inline_job(long, FontId::proportional(cjk), crate::DARK.text_main,
                    &crate::DARK, f32::INFINITY, None).text;
                assert_eq!(layout.rows[1][0].galley.text(), expected);
            });
        });
        // The page is the scrollable width, not just the widest block: it must
        // keep one whole CJK glyph of blank space past the longest line.
        assert!(
            content.x - widest >= cjk,
            "page must be at least one CJK glyph wider than its longest line: \
             page {}, widest {widest}, glyph {cjk}",
            content.x
        );
        assert!(
            content.x > inner.width() + cjk,
            "page {} must exceed the viewport {} by a glyph",
            content.x,
            inner.width()
        );
    }
    fn long_markdown(body: &str) -> String {
        let mut text = String::new();
        for index in 0..80 {
            text.push_str(&format!(
                "## Section {index}\n\nParagraph number {index} with some text.\n\n"
            ));
        }
        text.push_str(body);
        text
    }

    #[test]
    fn live_preview_find_reveals_the_matching_block() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        let text = long_markdown("The needle sits at the end.\n");
        let needle_line = text[..text.find("needle").unwrap()]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count();
        let file = app.active_mut().unwrap();
        file.draft_text = Some(text.clone());
        file.sync_draft();
        file.markdown_mode = Mode::Live;
        file.editor_state.tools.find_open = true;
        file.editor_state.tools.query = "needle".to_owned();
        file.editor_state.tools.pending_action = Some(2);
        for _ in 0..6 {
            draw(&ctx, &mut app, vec![]);
        }
        let file_id = app.active_id.unwrap();
        let offset = ctx
            .data(|data| {
                data.get_temp::<f32>(egui::Id::new(("markdown_live_scroll_y", file_id)))
            })
            .expect("live preview must publish its scroll offset");
        assert!(
            offset > 0.0,
            "the live preview must scroll to the matched block, offset {offset}"
        );
        let file = app.active().unwrap();
        assert!(
            file.editor_state.pending_selection.is_none(),
            "live mode must consume the jump target instead of leaving it pending"
        );
        assert_eq!(
            file.editor_state.cursor_line, needle_line,
            "the status bar must follow the revealed match"
        );
        assert_eq!(file.draft_text.as_deref(), Some(text.as_str()));
    }

    #[test]
    fn split_find_reveals_the_source_pane_even_under_the_find_box() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        let text = long_markdown("The needle sits at the end.\n");
        let file = app.active_mut().unwrap();
        file.draft_text = Some(text);
        file.sync_draft();
        file.markdown_mode = Mode::Split;
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let file_id = app.active_id.unwrap();
        // The find overlay sits over the preview pane; a pointer there used to
        // force the shared offset back and cancel the reveal.
        let target = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new(("markdown_split_right", file_id)))
            })
            .unwrap()
            .center();
        draw(&ctx, &mut app, vec![egui::Event::PointerMoved(target)]);
        let file = app.active_mut().unwrap();
        file.editor_state.tools.find_open = true;
        file.editor_state.tools.query = "needle".to_owned();
        file.editor_state.tools.pending_action = Some(2);
        for _ in 0..4 {
            draw(&ctx, &mut app, vec![]);
        }
        let source = crate::editor::last_scroll_offset(&ctx, file_id)
            .expect("the source pane must publish its offset");
        assert!(
            source > 0.0,
            "find must reveal the match in the editable source pane, offset {source}"
        );
        let file = app.active().unwrap();
        assert!(file.editor_state.pending_selection.is_none());
        assert!(
            (file.markdown.split_offset - source).abs() < 1.0,
            "the preview must follow the revealed source offset: split {}, source {source}",
            file.markdown.split_offset
        );
    }
    #[test]
    fn live_preview_find_lands_inside_a_tall_code_block() {
        // Middle of the block must be centered; the very last line can only
        // scroll to the clamped bottom but must still be inside the viewport.
        for (needle_index, should_center) in [(150, true), (295, false)] {
            let ctx = egui::Context::default();
            crate::setup_fonts(&ctx);
            let mut app = markdown_app();
            let mut text = String::from("# Tall code\n\n```rust\n");
            for index in 0..300 {
                if index == needle_index {
                    text.push_str("let needle = find_me();\n");
                } else {
                    text.push_str(&format!("let value_{index} = {index};\n"));
                }
            }
            text.push_str("```\n");
            let needle_line = text[..text.find("needle").unwrap()]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count();
            let file = app.active_mut().unwrap();
            file.draft_text = Some(text);
            file.sync_draft();
            file.markdown_mode = Mode::Live;
            file.editor_state.tools.find_open = true;
            file.editor_state.tools.query = "needle".to_owned();
            file.editor_state.tools.pending_action = Some(2);
            let mut output = None;
            for _ in 0..6 {
                output = Some(draw(&ctx, &mut app, vec![]));
            }
            let file_id = app.active_id.unwrap();
            let viewport = ctx
                .data(|data| {
                    data.get_temp::<egui::Rect>(egui::Id::new((
                        "markdown_live_viewport",
                        file_id,
                    )))
                })
                .expect("live viewport");
            let offset = ctx
                .data(|data| {
                    data.get_temp::<f32>(egui::Id::new(("markdown_live_scroll_y", file_id)))
                })
                .unwrap();
            let mut needle_y = None;
            for shape in output.unwrap().shapes {
                if let egui::Shape::Text(shape) = shape.shape
                    // The find box itself also contains the query; measure the
                    // rendered code block instead.
                    && shape.galley.text().contains("value_")
                {
                    for row in &shape.galley.rows {
                        let line: String = row.glyphs.iter().map(|glyph| glyph.chr).collect();
                        if line.contains("needle") {
                            needle_y = Some(shape.pos.y + row.rect.center().y);
                        }
                    }
                }
            }
            let needle_y = needle_y.expect("the matched code line must be rendered");
            assert!(offset > 0.0, "needle {needle_index}: preview must scroll, offset {offset}");
            assert!(
                viewport.top() - 1.0 <= needle_y && needle_y <= viewport.bottom() + 1.0,
                "needle {needle_index}: find must land on the matched line inside a tall \
                 block: line y {needle_y}, viewport {viewport:?}"
            );
            if should_center {
                assert!(
                    (needle_y - viewport.center().y).abs() < viewport.height() * 0.25,
                    "needle {needle_index}: the matched line should sit near the middle of \
                     the preview: line y {needle_y}, viewport {viewport:?}"
                );
            }
            assert_eq!(app.active().unwrap().editor_state.cursor_line, needle_line);
        }
    }
    #[test]
    fn live_preview_enter_in_find_field_jumps_and_tints_the_match() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        let text = long_markdown("The needle sits at the end.\n");
        let file = app.active_mut().unwrap();
        file.draft_text = Some(text);
        file.sync_draft();
        file.markdown_mode = Mode::Live;
        file.editor_state.tools.find_open = true;
        file.editor_state.tools.focus_find = Some(false);
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        // A singleline TextEdit surrenders focus on Enter; the find bar must
        // still treat that frame as "search next" (this was the live-mode bug).
        draw(&ctx, &mut app, vec![egui::Event::Text("needle".to_owned())]);
        draw(
            &ctx,
            &mut app,
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        let mut output = None;
        for _ in 0..5 {
            output = Some(draw(&ctx, &mut app, vec![]));
        }
        let file_id = app.active_id.unwrap();
        let offset = ctx
            .data(|data| data.get_temp::<f32>(egui::Id::new(("markdown_live_scroll_y", file_id))))
            .expect("live scroll offset");
        assert!(
            offset > 0.0,
            "Enter in the find field must jump the live preview, offset {offset}"
        );
        assert!(
            app.active().unwrap().editor_state.tools.current.is_some(),
            "the search must locate a match"
        );
        let highlight = highlight_color(&crate::DARK);
        let mut tinted = false;
        for shape in output.unwrap().shapes {
            if let egui::Shape::Text(shape) = shape.shape
                && shape.galley.text().contains("The needle sits")
            {
                tinted |= shape
                    .galley
                    .job
                    .sections
                    .iter()
                    .any(|section| section.format.background == highlight);
            }
        }
        assert!(
            tinted,
            "the matched word must be tinted in the rendered paragraph"
        );
    }

    #[test]
    fn code_block_search_hit_tints_only_the_matched_characters() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        let file = app.active_mut().unwrap();
        file.draft_text = Some(
            "# Code\n\n```rust\nlet before = 1;\nlet needle = 2;\nlet after = 3;\n```\n"
                .to_owned(),
        );
        file.sync_draft();
        file.markdown_mode = Mode::Live;
        file.editor_state.tools.find_open = true;
        file.editor_state.tools.query = "needle".to_owned();
        file.editor_state.tools.pending_action = Some(2);
        let mut output = None;
        for _ in 0..3 {
            output = Some(draw(&ctx, &mut app, vec![]));
        }
        let highlight = highlight_color(&crate::DARK);
        let mut tinted: Vec<String> = Vec::new();
        for shape in output.unwrap().shapes {
            if let egui::Shape::Text(shape) = shape.shape
                && shape.galley.text().contains("let needle = 2;")
            {
                let job = &shape.galley.job;
                for section in &job.sections {
                    if section.format.background == highlight {
                        tinted.push(job.text[section.byte_range.clone()].to_owned());
                    }
                }
            }
        }
        assert_eq!(
            tinted,
            vec!["needle".to_owned()],
            "only the matched token may be tinted, got {tinted:?}"
        );
    }

    #[test]
    fn live_preview_repeated_enter_advances_through_matches() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = markdown_app();
        let mut text = String::from("# Top\n\nThe needle first sits at the top.\n\n");
        text.push_str(&long_markdown("The needle second sits at the end.\n"));
        let file = app.active_mut().unwrap();
        file.draft_text = Some(text);
        file.sync_draft();
        file.markdown_mode = Mode::Live;
        file.editor_state.tools.find_open = true;
        file.editor_state.tools.focus_find = Some(false);
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        draw(&ctx, &mut app, vec![egui::Event::Text("needle".to_owned())]);

        // Type a query, press Enter, then press Enter again: the second press
        // must move on to the next match. Rendered modes have no editor
        // selection, so the previous match has to be remembered explicitly.
        let observe = |app: &mut crate::LiteTextApp, ctx: &egui::Context, press: bool| {
            if press {
                draw(
                    ctx,
                    app,
                    vec![egui::Event::Key {
                        key: egui::Key::Enter,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: Default::default(),
                    }],
                );
            }
            let mut output = None;
            for _ in 0..4 {
                output = Some(draw(ctx, app, vec![]));
            }
            let file_id = app.active_id.unwrap();
            let offset = ctx
                .data(|data| {
                    data.get_temp::<f32>(egui::Id::new(("markdown_live_scroll_y", file_id)))
                })
                .expect("live scroll offset");
            let viewport = ctx
                .data(|data| {
                    data.get_temp::<egui::Rect>(egui::Id::new(("markdown_live_viewport", file_id)))
                })
                .expect("live viewport");
            let highlight = highlight_color(&crate::DARK);
            let mut tinted = Vec::new();
            for shape in output.unwrap().shapes {
                if let egui::Shape::Text(shape) = shape.shape {
                    let job = &shape.galley.job;
                    let hot: Vec<_> = job
                        .sections
                        .iter()
                        .filter(|section| section.format.background == highlight)
                        .map(|section| section.byte_range.clone())
                        .collect();
                    if hot.is_empty() {
                        continue;
                    }
                    let mut base = 0;
                    for row in &shape.galley.rows {
                        let len: usize = row.glyphs.iter().map(|glyph| glyph.chr.len_utf8()).sum();
                        let row_end = base + len + usize::from(row.ends_with_newline);
                        if hot.iter().any(|hit| hit.start >= base && hit.start < row_end) {
                            let y = shape.pos.y + row.rect.center().y;
                            assert!(
                                viewport.top() <= y && y <= viewport.bottom(),
                                "matched row at {y} must be inside {viewport:?}"
                            );
                            tinted.push(job.text[base..row_end].trim_end().to_owned());
                        }
                        base = row_end;
                    }
                }
            }
            let file = app.active().unwrap();
            (file.editor_state.tools.current.clone(), offset, tinted)
        };

        let (first, first_offset, first_tinted) = observe(&mut app, &ctx, true);
        let first = first.expect("first match");
        assert!(
            first_tinted.iter().any(|row| row.contains("The needle first")),
            "first press must tint the first match, got {first_tinted:?}"
        );

        let (second, second_offset, second_tinted) = observe(&mut app, &ctx, true);
        let second = second.expect("second match");
        assert!(
            second.start > first.start,
            "Enter must advance past {first:?}, still at {second:?}"
        );
        assert_ne!(
            first_offset, second_offset,
            "the preview must scroll to the next match"
        );
        assert!(
            second_tinted
                .iter()
                .any(|row| row.contains("The needle second")),
            "second press must tint the second match, got {second_tinted:?}"
        );
    }





}

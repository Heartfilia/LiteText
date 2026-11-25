use crate::{Palette, i18n::Strings};
use eframe::egui;
use egui::{FontId, Stroke};
use std::ops::Range;

pub(crate) const GUTTER_NUMBER_GAP: f32 = 3.0;
const EDITOR_LEFT_PADDING: f32 = 4.0;

/// Keep the number/fold lane distinct without changing the workspace hue.
pub(crate) fn gutter_background(background: egui::Color32) -> egui::Color32 {
    let darker = |channel: u8| channel.saturating_sub((channel / 16).max(8));
    egui::Color32::from_rgb(darker(background.r()), darker(background.g()), darker(background.b()))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WordClass {
    Han,
    Word,
    Space,
    Other,
}

fn word_class(c: char) -> WordClass {
    if matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF
        | 0x20000..=0x2FA1F | 0x30000..=0x3347F)
        || c == '〇'
    {
        WordClass::Han
    } else if c.is_alphanumeric() || c == '_' {
        WordClass::Word
    } else if c.is_whitespace() && c != '\r' && c != '\n' {
        WordClass::Space
    } else {
        WordClass::Other
    }
}

/// Character range of the word-like run at `index` (punctuation is its own
/// one-character run, whitespace runs together).
pub(crate) fn word_range(text: &str, index: usize) -> Range<usize> {
    let chars: Vec<_> = text.chars().collect();
    if index >= chars.len() {
        return chars.len()..chars.len();
    }
    let class = word_class(chars[index]);
    if class == WordClass::Other {
        return index..index + 1;
    }
    let mut start = index;
    let mut end = index + 1;
    while start > 0 && word_class(chars[start - 1]) == class {
        start -= 1;
    }
    while end < chars.len() && word_class(chars[end]) == class {
        end += 1;
    }
    start..end
}

/// Override egui's ASCII-only word selection in both document editors. Rebuild
/// only the double-click frame so selection paint, copy and cursor agree at once.
pub(crate) fn select_word_on_double_click(
    ui: &egui::Ui,
    output: &mut egui::text_edit::TextEditOutput,
    text: &str,
    layout: impl FnOnce() -> std::sync::Arc<egui::Galley>,
) {
    if !output.response.double_clicked() || output.response.triple_clicked() {
        return;
    }
    let Some(pointer) = output.response.interact_pointer_pos() else {
        return;
    };
    let local = pointer - output.galley_pos;
    let hit = output.galley.cursor_from_pos(local);
    let row = &output.galley.rows[hit.rcursor.row];
    let mut index = hit.ccursor.index;
    // cursor_from_pos rounds to the closest caret. On the right half of a
    // glyph, select that glyph, not the punctuation/word after it.
    if hit.rcursor.column > 0 && local.x < row.x_offset(hit.rcursor.column) {
        index = index.saturating_sub(1);
    }
    let range = word_range(text, index);
    let cursor = egui::text::CCursorRange::two(
        egui::text::CCursor::new(range.start),
        egui::text::CCursor::new(range.end),
    );
    output.state.cursor.set_char_range(Some(cursor));
    output.state.clone().store(ui.ctx(), output.response.id);
    output.cursor_range = Some(egui::text::CursorRange {
        primary: output.galley.from_ccursor(cursor.primary),
        secondary: output.galley.from_ccursor(cursor.secondary),
    });
    let old = output.galley.clone();
    let mut galley = layout();
    egui::text_selection::visuals::paint_text_selection(
        &mut galley,
        ui.visuals(),
        output.cursor_range.as_ref().unwrap(),
        None,
    );
    ui.ctx().graphics_mut(|graphics| {
        let list = graphics.entry(ui.layer_id());
        let indices: Vec<_> = list
            .all_entries()
            .enumerate()
            .filter_map(|(index, shape)| match &shape.shape {
                egui::Shape::Text(t) if std::sync::Arc::ptr_eq(&t.galley, &old) => Some(index),
                _ => None,
            })
            .collect();
        for index in indices {
            list.mutate_shape(egui::layers::ShapeIdx(index), |shape| {
                if let egui::Shape::Text(t) = &mut shape.shape {
                    t.galley = galley.clone();
                }
            });
        }
    });
    output.galley = galley;
    ui.ctx().request_repaint();
}

/// TextEdit hides selection on focus loss (e.g. opening the app/plugin menu).
/// Retain the document selection visually without stealing focus from menus.
pub(crate) fn paint_unfocused_selection(ui: &egui::Ui, output: &mut egui::text_edit::TextEditOutput) {
    if output.response.has_focus() { return; }
    let Some(range) = output.state.cursor.char_range().filter(|range| range.primary.index != range.secondary.index) else { return; };
    let old = output.galley.clone();
    let mut galley = std::sync::Arc::new((*old).clone());
    let cursor = egui::text::CursorRange {
        primary: galley.from_ccursor(range.primary), secondary: galley.from_ccursor(range.secondary),
    };
    egui::text_selection::visuals::paint_text_selection(&mut galley, ui.visuals(), &cursor, None);
    ui.ctx().graphics_mut(|graphics| {
        let list = graphics.entry(ui.layer_id());
        let indices: Vec<_> = list.all_entries().enumerate().filter_map(|(index, shape)| match &shape.shape {
            egui::Shape::Text(text) if std::sync::Arc::ptr_eq(&text.galley, &old) => Some(index), _ => None,
        }).collect();
        for index in indices { list.mutate_shape(egui::layers::ShapeIdx(index), |shape| {
            if let egui::Shape::Text(text) = &mut shape.shape { text.galley = galley.clone(); }
        }); }
    });
    output.galley = galley;
    output.cursor_range = Some(cursor);
}

/// ScrollArea includes the frozen gutter in its viewport, but TextEdit does not
/// know that it is covered. Reserve that width on the left of its reveal target.
/// Only reveal on cursor/edit changes, so manual scrolling can leave it offscreen.
pub(crate) fn reveal_cursor(
    ui: &egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    previous: Option<egui::text::CCursorRange>,
    gutter_width: f32,
    explicit: bool,
) {
    let cursor = output.state.cursor.char_range();
    if (explicit
        || (output.response.has_focus() && (output.response.changed() || cursor != previous)))
        && let Some(cursor) = cursor
    {
        let mut target = output
            .galley
            .pos_from_ccursor(cursor.primary)
            .translate(output.galley_pos.to_vec2());
        target.min.x -= gutter_width + EDITOR_LEFT_PADDING;
        if explicit {
            // Find / go-to-line jumps must land on the target in this frame:
            // the split view re-applies its shared offset on the next frame,
            // which would freeze an animated scroll halfway.
            ui.scroll_to_rect_animation(
                target,
                Some(egui::Align::Center),
                egui::style::ScrollAnimation::none(),
            );
        } else {
            ui.scroll_to_rect(target, None);
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct EditorState {
    /// Inclusive logical line indices, independent of the text editor's character selection.
    pub(crate) selection: Option<(usize, usize)>,
    pub(crate) anchor: Option<usize>,
    pub(crate) large: crate::large_editor::State,
    pub(crate) tools: crate::text_tools::Tools,
    pub(crate) cursor: Option<egui::text::CCursorRange>,
    pub(crate) selected_characters: usize,
    pub(crate) cursor_line: usize,
    pub(crate) font_size: f32,
    pub(crate) pending_selection: Option<Range<usize>>,
    /// Allow bottom whitespace after folding so a shorter document does not pull
    /// the existing viewport upward merely to keep its old bottom edge filled.
    pub(crate) fold_scroll_extent: f32,
    pub(crate) focus_requested: bool,
    pub(crate) changes: crate::changes::Changes,
    pub(crate) folds: crate::json_plugin::Folds,
    pub(crate) syntax_cache: std::sync::Arc<std::sync::Mutex<crate::workspace_theme::SyntaxCache>>,
    pub(crate) line_ending: crate::line_endings::LineEnding,
    pub(crate) composing: bool,
}

impl EditorState {
    pub(crate) fn gutter_font(&self) -> FontId {
        let mut font = self.font();
        font.size -= 1.0;
        font
    }

    pub(crate) fn font(&self) -> FontId {
        FontId::monospace(if self.font_size > 0.0 {
            self.font_size
        } else {
            13.5
        })
    }

    pub(crate) fn invalidate(&mut self) {
        self.large.indexed_len = None;
        self.large.invalidate_window();
    }
}

/// Match epaint's per-row physical-pixel rounding, not the raw font metric.
pub(crate) fn row_height(ui: &egui::Ui, font: &FontId) -> f32 {
    let ppp = ui.ctx().pixels_per_point();
    (ui.fonts(|f| f.row_height(font)) * ppp).round() / ppp
}

/// Compact line actions, independent of document zoom and editor row spacing.
pub(crate) fn line_menu_style(ui: &mut egui::Ui, copy: &str, clear: &str) {
    ui.spacing_mut().item_spacing.y = 2.0;
    ui.spacing_mut().button_padding = egui::vec2(8.0, 3.0);
    ui.spacing_mut().interact_size.y = 24.0;
    let font = egui::FontId::proportional(13.0);
    let width = [copy, clear].into_iter().map(|label|
        ui.painter().layout_no_wrap(label.to_owned(), font.clone(), ui.visuals().text_color()).size().x
    ).fold(0.0_f32, f32::max) + 16.0;
    ui.set_min_width(width);
}

pub(crate) fn line_menu_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(egui::Button::new(egui::RichText::new(label).size(13.0)).wrap_mode(egui::TextWrapMode::Extend))
}

pub(crate) fn scroll_style(ui: &mut egui::Ui, pal: &Palette) {
    let mut style = egui::style::ScrollStyle::solid();
    style.foreground_color = true;
    style.bar_width = 10.0;
    ui.style_mut().spacing.scroll = style;
    let v = ui.visuals_mut();
    v.widgets.inactive.fg_stroke.color = pal.text_faint.gamma_multiply(0.45);
    v.widgets.hovered.fg_stroke.color = pal.text_faint.gamma_multiply(0.65);
    v.widgets.active.fg_stroke.color = pal.text_faint.gamma_multiply(0.85);
}

/// Byte ranges exclude line terminators; even a blank document has one line.
pub(crate) fn line_ranges(text: &str) -> Vec<Range<usize>> {
    let mut offset = 0;
    text.split('\n')
        .map(|line| {
            let start = offset;
            offset += line.len() + 1;
            let end = start + line.strip_suffix('\r').unwrap_or(line).len();
            start..end
        })
        .collect()
}

pub(crate) fn copy_lines(text: &str, selection: (usize, usize)) -> String {
    line_ranges(text)
        .into_iter()
        .skip(selection.0)
        .take(selection.1 - selection.0 + 1)
        .map(|range| &text[range])
        .collect::<Vec<_>>()
        .join("\n")
}

/// Clear selected line contents without deleting their line breaks or shifting other lines.
pub(crate) fn clear_lines(text: &mut String, selection: (usize, usize)) -> usize {
    let ranges = line_ranges(text);
    let start = ranges[selection.0].start;
    for range in ranges
        .into_iter()
        .skip(selection.0)
        .take(selection.1 - selection.0 + 1)
        .rev()
    {
        text.replace_range(range, "");
    }
    text[..start].chars().count()
}

/// Reserved line-number columns: two for 1–99 lines, four for 100–9999, then
/// exactly as many columns as the highest line number needs (5 for 99999, 6
/// for 666666, …).
pub(crate) fn gutter_columns(line_count: usize) -> usize {
    match line_count.max(1) {
        1..=99 => 2,
        100..=9999 => 4,
        lines => lines.ilog10() as usize + 1,
    }
}

/// Reserve digits for original logical line numbers, not the visible row count.
/// The fold lane is allocated separately and must not also become left padding.
pub(crate) fn gutter_number_width(line_count: usize, digit_width: f32) -> f32 {
    let left_padding = GUTTER_NUMBER_GAP;
    left_padding + gutter_columns(line_count) as f32 * digit_width + GUTTER_NUMBER_GAP
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn show(
    ui: &mut egui::Ui,
    text: &mut String,
    state: &mut EditorState,
    file_id: u64,
    pal: &Palette,
    focus: bool,
    t: &Strings,
) -> bool {
    show_impl(ui, text, state, file_id, pal, focus, t, None, None)
}

/// Byte offset of an egui character index.
fn byte_of_char(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map_or(text.len(), |(index, _)| index)
}

/// Byte range of a non-empty TextEdit selection.
fn selected_bytes(
    text: &str,
    selection: Option<egui::text::CCursorRange>,
) -> Option<Range<usize>> {
    let selection = selection?;
    let start = byte_of_char(text, selection.primary.index.min(selection.secondary.index));
    let end = byte_of_char(text, selection.primary.index.max(selection.secondary.index));
    (start < end).then_some(start..end)
}

/// Byte range of the word under a collapsed caret, so "jump to next match"
/// also works when the user right-clicks a word instead of selecting it first.
pub(crate) fn word_bytes_at(text: &str, char_index: usize) -> Option<Range<usize>> {
    let range = word_range(text, char_index);
    if range.is_empty() {
        return None;
    }
    let start = byte_of_char(text, range.start);
    let end = byte_of_char(text, range.end);
    (start < end && !text[start..end].chars().all(char::is_whitespace)).then_some(start..end)
}

/// Byte range for a character-index selection; `None` when it is empty.
fn selection_bytes(text: &str, first: usize, second: usize) -> Option<Range<usize>> {
    let start = byte_of_char(text, first.min(second));
    let end = byte_of_char(text, first.max(second));
    (start < end).then_some(start..end)
}

/// Next occurrence of `needle` outside `selection`, wrapping around the
/// document. The result is a character range ready for `pending_selection`.
pub(crate) fn next_occurrence(
    text: &str,
    needle: &str,
    selection: Range<usize>,
) -> Option<Range<usize>> {
    if needle.is_empty() || selection.start > selection.end || selection.end > text.len() {
        return None;
    }
    let hit = text[selection.end..]
        .find(needle)
        .map(|at| selection.end + at)
        .or_else(|| text[..selection.start].find(needle))?;
    let start = text[..hit].chars().count();
    Some(start..start + needle.chars().count())
}

/// Last vertical scroll offset of the document editor, used to keep the
/// Markdown split view's two panes aligned.
pub(crate) fn last_scroll_offset(ctx: &egui::Context, file_id: u64) -> Option<f32> {
    ctx.data(|data| data.get_temp::<f32>(egui::Id::new(("editor_scroll_y", file_id))))
}

/// Editor variant used by the Markdown split view. Each logical source line
/// receives the height of its rendered counterpart, so both panes share a
/// vertical rhythm while the left pane stays fully editable.
#[allow(clippy::too_many_arguments)]
pub(crate) fn show_synced(
    ui: &mut egui::Ui,
    text: &mut String,
    state: &mut EditorState,
    file_id: u64,
    pal: &Palette,
    focus: bool,
    t: &Strings,
    line_heights: &[f32],
    font_sizes: &[f32],
    scroll_y: Option<f32>,
) -> bool {
    show_impl(
        ui,
        text,
        state,
        file_id,
        pal,
        focus,
        t,
        Some((line_heights, font_sizes)),
        scroll_y,
    )
}

#[allow(clippy::too_many_arguments)]
fn show_impl(
    ui: &mut egui::Ui,
    text: &mut String,
    state: &mut EditorState,
    file_id: u64,
    pal: &Palette,
    focus: bool,
    t: &Strings,
    line_layout: Option<(&[f32], &[f32])>,
    scroll_y: Option<f32>,
) -> bool {
    if line_layout.is_none() && (crate::large_editor::needs_windowing(text) || state.large.active) {
        return crate::large_editor::show(ui, text, state, file_id, pal, focus, t);
    }
    let bounds = ui.available_rect_before_wrap();
    let editor_id = egui::Id::new("draft_editor").with(file_id);
    // egui collapses a text selection as soon as any pointer button is
    // pressed on the widget, including the secondary button that opens the
    // context menu. Keep the pre-click selection so the menu can still copy,
    // cut or jump from the text the user actually selected. Clear it when the
    // press started without a selection so a later menu never acts on a stale
    // range.
    if ui.input(|input| input.pointer.button_pressed(egui::PointerButton::Secondary)) {
        let id = egui::Id::new(("editor_menu_selection", file_id));
        let range = egui::TextEdit::load_state(ui.ctx(), editor_id)
            .and_then(|state| state.cursor.char_range())
            .filter(|range| range.primary.index != range.secondary.index)
            .map(|range| {
                (
                    range.primary.index.min(range.secondary.index),
                    range.primary.index.max(range.secondary.index),
                )
            });
        ui.ctx().data_mut(|data| match range {
            Some(range) => {
                data.insert_temp(id, range);
            }
            None => {
                data.remove_temp::<(usize, usize)>(id);
            }
        });
    }
    if ui.is_enabled() { state.line_ending.remap_enter(ui.ctx(), editor_id, &mut state.composing, text, state.cursor); }
    let fold_selection = state
        .folds
        .prepare_input(ui, state.pending_selection.is_some(), editor_id);
    if state.pending_selection.is_some() && !fold_selection || !state.folds.has_collapsed() {
        state.fold_scroll_extent = 0.0;
    }
    let folds = state.folds.clone();
    let fold_w = folds.width();
    scroll_style(ui, pal);
    let font = state.font();
    let gutter_font = state.gutter_font();
    let tools = state.tools.clone();
    if let Some(render) = &tools.workspace {
        render.apply_style(ui);
    }
    let colors = tools.workspace.as_ref().map(|r| r.colors);
    let gutter_bg = gutter_background(colors.map_or(pal.bg_window, |c| c.background));
    let before_layout = (!tools.marks.is_empty() || tools.current.is_some()).then(|| text.clone());
    let line_count = text.bytes().filter(|&b| b == b'\n').count() + 1;
    // Use the rounded glyph advance from a multi-digit galley, not a single glyph's width.
    let probe = ui.painter().layout_no_wrap(
        "0000000000".to_owned(),
        gutter_font.clone(),
        colors.map_or(pal.text_faint, |c| c.gutter),
    );
    let digit_w = probe
        .rows
        .first()
        .and_then(|row| (row.glyphs.len() >= 2).then(|| row.glyphs[1].pos.x - row.glyphs[0].pos.x))
        .unwrap_or(8.0);
    let number_w = gutter_number_width(line_count, digit_w);
    let gutter_w = number_w + fold_w;
    let gutter = egui::Rect::from_min_max(
        bounds.min,
        egui::pos2(bounds.left() + gutter_w, bounds.bottom()),
    );
    ui.painter()
        .rect_filled(gutter, 0.0, gutter_bg);
    let mut changed = false;
    let mut fold_pointer = false;
    let mut viewport_offset = egui::Vec2::ZERO;
    let mut depth_action = None;
    let mut depth_offset = None;
    let mut scroll_area = egui::ScrollArea::both()
        .id_salt(("draft_scroll", file_id))
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
        .scroll_bar_rect(egui::Rect::from_min_max(
            egui::pos2(gutter.right() + 1.0, bounds.top()),
            bounds.max,
        ))
        .auto_shrink([false, false]);
    if let Some(offset) = scroll_y {
        scroll_area = scroll_area.vertical_scroll_offset(offset);
    }
    let scroll_output = scroll_area
        .show_viewport(ui, |ui, viewport| {
            viewport_offset = viewport.min.to_vec2();
            let origin = ui.cursor().min;
            let viewport_clip = ui.clip_rect();
            let editor_rect = egui::Rect::from_min_size(
                origin + egui::vec2(gutter_w + EDITOR_LEFT_PADDING, 12.0),
                egui::vec2(
                    (viewport.width() - gutter_w - EDITOR_LEFT_PADDING - 12.0).max(24.0),
                    (viewport.height() - 24.0).max(24.0),
                ),
            );
            let editor_id = egui::Id::new("draft_editor").with(file_id);
            let mut layouter = |ui: &egui::Ui, text: &str, width: f32| {
                let mut current_tools = tools.clone();
                if let Some(before) = &before_layout
                    && before != text
                {
                    let (removed, inserted) = crate::large_editor::difference(before, text);
                    current_tools.apply_edit(removed, inserted.len());
                }
                let mut job = crate::text_tools::layout(
                    text,
                    font.clone(),
                    pal,
                    // In the split view each logical line must stay one row so
                    // its stretched height lines up with the rendered block.
                    if tools.wrap && line_layout.is_none() { (width - if tools.whitespace { 12.0 } else { 0.0 }).max(24.0) } else { f32::INFINITY },
                    &current_tools,
                );
                if let Some((heights, fonts)) = line_layout {
                    apply_line_heights(&mut job, heights, fonts);
                }
                folds.layout(ui.painter().layout_job(job), 0)
            };
            let reveal = state.pending_selection.is_some() && !fold_selection;
            if let Some(range) = state.pending_selection.take() {
                let mut edit = egui::TextEdit::load_state(ui.ctx(), editor_id).unwrap_or_default();
                edit.cursor
                    .set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(range.start),
                        egui::text::CCursor::new(range.end),
                    )));
                edit.store(ui.ctx(), editor_id);
            }
            crate::undo_settings::configure(ui.ctx(), editor_id, text);
            let previous = egui::TextEdit::load_state(ui.ctx(), editor_id)
                .and_then(|edit| edit.cursor.char_range());
            let rule_viewport = egui::Rect::from_min_max(
                egui::pos2(gutter.right() + 1.0, viewport_clip.top()), viewport_clip.max);
            let rule_painter = ui.painter().with_clip_rect(rule_viewport);
            let rule_slot = crate::notebook::slot(&rule_painter, tools.workspace.as_ref());
            let editor_output =
                ui.allocate_new_ui(egui::UiBuilder::new().max_rect(editor_rect), |ui| {
                    // Keep the gutter frozen during horizontal scrolling.
                    ui.set_clip_rect(ui.clip_rect().intersect(egui::Rect::from_min_max(
                        egui::pos2(gutter.right() + 1.0, bounds.top()),
                        viewport_clip.max,
                    )));
                    let mut output = egui::TextEdit::multiline(&mut *text)
                        .id(editor_id)
                        .font(font.clone())
                        .text_color(colors.map_or(pal.text_main, |c| c.text))
                        .frame(false)
                        .margin(egui::Margin::ZERO)
                        .desired_width(editor_rect.width())
                        .min_size(editor_rect.size())
                        .layouter(&mut layouter)
                        .lock_focus(true)
                        .show(ui);
                    select_word_on_double_click(ui, &mut output, text, || {
                        layouter(ui, text, editor_rect.width())
                    });
                    output
                });
            // Propagate the full text extent to the scroll area, not just the viewport-sized child.
            let mut output = editor_output.inner;
            crate::notebook::paint_rows(&rule_painter, rule_slot, tools.workspace.as_ref(),
                &output.galley, output.galley_pos, rule_viewport);
            // egui collapses a text selection on any pointer press, including
            // the secondary click that opens the context menu. When the user
            // right-clicks inside the selection, restore it so the highlight
            // stays visible while the menu is open and copy/cut still target
            // the text the user picked.
            if ui.input(|input| input.pointer.button_pressed(egui::PointerButton::Secondary))
                && let Some(saved) = previous
                && saved.primary.index != saved.secondary.index
                && let Some(clicked) = output.state.cursor.char_range()
            {
                let start = saved.primary.index.min(saved.secondary.index);
                let end = saved.primary.index.max(saved.secondary.index);
                if start <= clicked.primary.index && clicked.primary.index <= end {
                    output.state.cursor.set_char_range(Some(saved));
                    output.state.clone().store(ui.ctx(), editor_id);
                    ui.ctx().request_repaint();
                }
            }
            paint_unfocused_selection(ui, &mut output);
            if let Some(render) = &tools.workspace {
                render.paint_selected_text(ui, &output);
            }
            ui.set_min_width(gutter_w + EDITOR_LEFT_PADDING + output.galley.size().x + if tools.whitespace { 24.0 } else { 12.0 });
            if focus {
                output.response.request_focus();
            }
            changed |= output.response.changed();
            state.cursor = output.state.cursor.char_range();
            state.cursor_line = state.cursor.map_or(0, |c| {
                text.chars()
                    .take(c.primary.index)
                    .filter(|&c| c == '\n')
                    .count()
            });
            state.tools.handle_marker(ui, &output, text, 0);
            if !fold_selection { reveal_cursor(ui, &output, previous, gutter_w, reveal); }
            if tools.whitespace {
                crate::text_tools::paint_whitespace(ui, &output.galley, output.galley_pos, pal, "");
            }
            folds.paint_summaries(ui, &output.galley, output.galley_pos, 0, pal);
            let mut logical = 0;
            let row_lines: Vec<_> = output
                .galley
                .rows
                .iter()
                .map(|row| {
                    let line = logical;
                    if row.ends_with_newline {
                        logical += 1;
                    }
                    line
                })
                .collect();
            let last_line = line_count - 1;
            if let Some((start, end)) = state.selection {
                state.selection = Some((start.min(last_line), end.min(last_line)));
            }
            if output.response.clicked() || output.response.changed() {
                state.selection = None;
                state.anchor = None;
            }

            let number_gutter = egui::Rect::from_min_max(
                gutter.min,
                egui::pos2(gutter.left() + number_w, gutter.bottom()),
            );
            let gutter_clip = number_gutter.intersect(ui.clip_rect());
            let response = ui.interact(
                gutter_clip,
                egui::Id::new("editor_gutter").with(file_id),
                egui::Sense::click_and_drag(),
            );
            let pointer_line = response.interact_pointer_pos().map(|p| {
                output
                    .galley
                    .rows
                    .partition_point(|row| output.galley_pos.y + row.rect.bottom() <= p.y)
                    .min(row_lines.len() - 1)
            });
            let pointer_line = pointer_line.map(|row| row_lines[row]);
            let start_line = if response.drag_started() {
                ui.input(|i| i.pointer.press_origin())
                    .map(|p| {
                        output
                            .galley
                            .rows
                            .partition_point(|row| output.galley_pos.y + row.rect.bottom() <= p.y)
                            .min(row_lines.len() - 1)
                    })
                    .map(|row| row_lines[row])
            } else {
                pointer_line
            };
            if (response.clicked() || response.drag_started())
                && let Some(line) = start_line
            {
                let anchor = if ui.input(|i| i.modifiers.shift) {
                    state.anchor.unwrap_or(line)
                } else {
                    line
                };
                state.anchor = Some(anchor);
                state.selection = Some((anchor.min(line), anchor.max(line)));
            }
            if response.dragged()
                && let (Some(anchor), Some(line)) = (state.anchor, pointer_line)
            {
                state.selection = Some((anchor.min(line), anchor.max(line)));
            }
            if !folds.enabled && response.secondary_clicked()
                && let Some(line) = pointer_line
                && !state
                    .selection
                    .is_some_and(|(start, end)| start <= line && line <= end)
            {
                state.selection = Some((line, line));
                state.anchor = Some(line);
            }

            let painter = ui.painter().with_clip_rect(gutter_clip);
            let line_box = row_height(ui, &font);
            let top_aligned = line_layout.is_some();
            for (row_index, row) in output.galley.rows.iter().enumerate() {
                let line = row_lines[row_index];
                if folds.hidden(line) {
                    continue;
                }
                let row_rect = egui::Rect::from_min_max(
                    egui::pos2(gutter.left(), output.galley_pos.y + row.rect.top()),
                    egui::pos2(
                        number_gutter.right(),
                        output.galley_pos.y + row.rect.bottom(),
                    ),
                );
                if !row_rect.intersects(gutter_clip) {
                    continue;
                }
                let selected = state
                    .selection
                    .is_some_and(|(start, end)| start <= line && line <= end);
                if selected {
                    painter.rect_filled(
                        row_rect,
                        0.0,
                        colors.map_or(pal.accent.gamma_multiply(0.25), |c| c.selection),
                    );
                }
                let modified = state.changes.is_marked(line);
                if modified && !selected {
                    painter.rect_filled(row_rect, 0.0, pal.accent.gamma_multiply(0.10));
                    painter.line_segment(
                        [row_rect.left_top(), row_rect.left_bottom()],
                        Stroke::new(2.0_f32, pal.accent),
                    );
                }
                // Painted numbers are never part of a selectable text widget.
                if row_index == 0 || row_lines[row_index - 1] != line {
                    painter.text(
                        egui::pos2(
                            number_gutter.right() - GUTTER_NUMBER_GAP,
                            gutter_number_center(
                                row_rect,
                                // Enlarged source fonts must move the number to
                                // the center of the glyphs, not the body font.
                                row.glyphs
                                    .iter()
                                    .map(|glyph| glyph.font_height)
                                    .fold(0.0_f32, f32::max)
                                    .max(line_box),
                                top_aligned,
                            ),
                        ),
                        egui::Align2::RIGHT_CENTER,
                        (line + 1).to_string(),
                        gutter_font.clone(),
                        if selected || modified {
                            pal.accent
                        } else {
                            colors.map_or(pal.text_faint, |c| c.gutter)
                        },
                    );
                }
            }

            if fold_w > 0.0 {
                let lane = egui::Rect::from_min_max(
                    egui::pos2(number_gutter.right(), gutter.top()),
                    gutter.max,
                );
                ui.painter()
                    .rect_filled(lane, 0.0, gutter_bg);
                for block in folds.blocks.iter() {
                    if folds.hidden(block.start) {
                        continue;
                    }
                    let Some((row_index, _)) = row_lines
                        .iter()
                        .enumerate()
                        .find(|(_, line)| **line == block.start)
                    else {
                        continue;
                    };
                    let row = &output.galley.rows[row_index];
                    let rect = egui::Rect::from_min_max(
                        egui::pos2(lane.left(), output.galley_pos.y + row.rect.top()),
                        egui::pos2(lane.right(), output.galley_pos.y + row.rect.bottom()),
                    );
                    let menu_rect = egui::Rect::from_min_max(
                        egui::pos2(number_gutter.left(), rect.top()), rect.max);
                    if menu_rect.intersects(ui.clip_rect()) {
                        fold_pointer |= ui.input(|i| i.pointer.interact_pos().is_some_and(|p| menu_rect.contains(p))
                            && (i.pointer.secondary_down() || i.pointer.button_released(egui::PointerButton::Secondary)));
                        if crate::json_plugin::depth_menu(menu_rect, file_id, block.start,
                            folds.collapsed(block.start), t.encoding_binary == "二进制", &response) {
                            depth_action = Some((block.start, row_index));
                        }
                    }
                    fold_pointer |= rect.intersects(lane.intersect(ui.clip_rect())) && ui.input(|input| {
                        input.pointer.interact_pos().is_some_and(|pos| rect.contains(pos))
                            && (input.pointer.any_down() || input.pointer.any_released())
                    });
                    if rect.intersects(lane.intersect(ui.clip_rect()))
                        && crate::json_plugin::fold_button(
                            ui,
                            rect,
                            file_id,
                            block.start,
                            folds.collapsed(block.start),
                            pal,
                        )
                    {
                        state.folds.toggle(block.start);
                        log::info!(target: "litetext::fold", "Toggle node: tab_id={file_id}; line={}; collapsed={}", block.start + 1, state.folds.collapsed(block.start));
                        state.fold_scroll_extent = viewport.bottom();
                        let start = text
                            .split_inclusive('\n')
                            .take(block.start)
                            .map(|l| l.chars().count())
                            .sum::<usize>();
                        state.pending_selection = Some(start..start);
                        state.selection = None;
                        ui.ctx().request_repaint();
                    }
                }
            }

            if !folds.enabled { response.context_menu(|menu| {
                let selection = state.selection.unwrap_or((0, 0));
                let label = if selection.0 == selection.1 {
                    t.copy_line
                } else {
                    t.copy_lines
                };
                line_menu_style(menu, t.select_all, t.clear_content);
                if line_menu_button(menu, label).clicked() {
                    menu.ctx().copy_text(copy_lines(text, selection));
                    menu.close_menu();
                }
                if line_menu_button(menu, t.cut).clicked() {
                    menu.ctx().copy_text(copy_lines(text, selection));
                    let before = text.clone();
                    let cursor = output.state.cursor.char_range().unwrap_or_default();
                    let mut undoer = output.state.undoer();
                    undoer.add_undo(&(cursor, before.clone()));
                    let index = clear_lines(text, selection);
                    let cursor = egui::text::CCursorRange::one(egui::text::CCursor::new(index));
                    output.state.cursor.set_char_range(Some(cursor));
                    undoer.add_undo(&(cursor, text.clone()));
                    output.state.set_undoer(undoer);
                    output.state.clone().store(menu.ctx(), editor_id);
                    changed |= *text != before;
                    menu.close_menu();
                }
                if line_menu_button(menu, t.paste).clicked() {
                    // Ask the platform for the clipboard contents and deliver
                    // them as a paste event to the (re-focused) editor.
                    menu.ctx().memory_mut(|memory| memory.request_focus(editor_id));
                    menu.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                    menu.close_menu();
                }
                if line_menu_button(menu, t.select_all).clicked() {
                    let mut edit = output.state.clone();
                    let chars = text.chars().count();
                    edit.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        egui::text::CCursor::new(chars),
                    )));
                    state.cursor = edit.cursor.char_range();
                    edit.store(menu.ctx(), editor_id);
                    state.selection = None;
                    menu.close_menu();
                }
                menu.separator();
                if line_menu_button(menu, t.clear_content).clicked() {
                    let before = text.clone();
                    let cursor = output.state.cursor.char_range().unwrap_or_default();
                    let mut undoer = output.state.undoer();
                    undoer.add_undo(&(cursor, before.clone()));
                    let index = clear_lines(text, selection);
                    let cursor = egui::text::CCursorRange::one(egui::text::CCursor::new(index));
                    output.state.cursor.set_char_range(Some(cursor));
                    undoer.add_undo(&(cursor, text.clone()));
                    output.state.set_undoer(undoer);
                    output.state.clone().store(menu.ctx(), editor_id);
                    changed |= *text != before;
                    menu.close_menu();
                }
            }); }
            output.response.context_menu(|menu| {
                let bytes = menu
                    .ctx()
                    .data(|data| {
                        data.get_temp::<(usize, usize)>(egui::Id::new((
                            "editor_menu_selection",
                            file_id,
                        )))
                    })
                    .and_then(|(start, end)| selection_bytes(text, start, end))
                    .or_else(|| selected_bytes(text, output.state.cursor.char_range()));
                let label = if bytes.is_some() {
                    t.copy_lines
                } else {
                    t.copy_line
                };
                line_menu_style(menu, t.select_all, t.cut);
                if line_menu_button(menu, label).clicked() {
                    let copied = bytes
                        .as_ref()
                        .map(|range| text[range.clone()].to_owned())
                        .unwrap_or_else(|| {
                            copy_lines(text, (state.cursor_line, state.cursor_line))
                        });
                    menu.ctx().copy_text(copied);
                    menu.close_menu();
                }
                if line_menu_button(menu, t.cut).clicked() {
                    let before = text.clone();
                    let cursor = output.state.cursor.char_range().unwrap_or_default();
                    let mut undoer = output.state.undoer();
                    undoer.add_undo(&(cursor, before.clone()));
                    let (index, copied) = if let Some(range) = &bytes {
                        let index = text[..range.start].chars().count();
                        let copied = text[range.clone()].to_owned();
                        text.replace_range(range.clone(), "");
                        (index, copied)
                    } else {
                        let line = (state.cursor_line, state.cursor_line);
                        (clear_lines(text, line), copy_lines(&before, line))
                    };
                    menu.ctx().copy_text(copied);
                    let cursor = egui::text::CCursorRange::one(
                        egui::text::CCursor::new(index),
                    );
                    output.state.cursor.set_char_range(Some(cursor));
                    undoer.add_undo(&(cursor, text.clone()));
                    output.state.set_undoer(undoer);
                    output.state.clone().store(menu.ctx(), editor_id);
                    changed = true;
                    menu.close_menu();
                }
                if line_menu_button(menu, t.paste).clicked() {
                    menu.ctx().memory_mut(|memory| memory.request_focus(editor_id));
                    menu.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                    menu.close_menu();
                }
                if line_menu_button(menu, t.select_all).clicked() {
                    let mut edit = output.state.clone();
                    let chars = text.chars().count();
                    edit.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        egui::text::CCursor::new(chars),
                    )));
                    state.cursor = edit.cursor.char_range();
                    edit.store(menu.ctx(), editor_id);
                    state.selection = None;
                    menu.close_menu();
                }
                menu.separator();
                if let Some(stamp) = crate::insert_time::submenu(menu, t.insert_time, t.unix_timestamp) {
                    // Insert the local time at the caret, replacing any
                    // selection, as one undoable edit. The menu holds focus,
                    // so hand it back for the caret to be painted.
                    let before = text.clone();
                    let cursor = output.state.cursor.char_range().unwrap_or_default();
                    let mut undoer = output.state.undoer();
                    undoer.add_undo(&(cursor, before.clone()));
                    let start = cursor.primary.index.min(cursor.secondary.index);
                    let end = cursor.primary.index.max(cursor.secondary.index);
                    let start_byte = crate::markdown::char_to_byte(text, start);
                    let end_byte = crate::markdown::char_to_byte(text, end);
                    text.replace_range(start_byte..end_byte, &stamp);
                    let after = egui::text::CCursorRange::one(egui::text::CCursor::new(
                        start + stamp.chars().count(),
                    ));
                    output.state.cursor.set_char_range(Some(after));
                    undoer.add_undo(&(after, text.clone()));
                    output.state.set_undoer(undoer);
                    output.state.clone().store(menu.ctx(), editor_id);
                    state.cursor = Some(after);
                    state.selection = None;
                    menu.ctx().memory_mut(|memory| memory.request_focus(editor_id));
                    changed = true;
                    menu.close_menu();
                }
                let jump_range = bytes.clone().or_else(|| {
                    let cursor = output.state.cursor.char_range()?;
                    (cursor.primary.index == cursor.secondary.index)
                        .then(|| word_bytes_at(text, cursor.primary.index))
                        .flatten()
                });
                let next_jump = jump_range
                    .as_ref()
                    .and_then(|range| next_occurrence(text, &text[range.clone()], range.clone()));
                let jump = menu.add_enabled(
                    next_jump.is_some(),
                    egui::Button::new(egui::RichText::new(t.jump_next).size(13.0))
                        .wrap_mode(egui::TextWrapMode::Extend),
                );
                if jump.clicked() && let Some(next) = next_jump {
                    // Move the selection to the next hit and reveal it without
                    // opening the find bar; wrapping keeps this usable as a
                    // symbol-usage jump. Without a selection the word under
                    // the right-clicked caret is the needle. The context menu
                    // holds focus, so hand it back to the editor or the caret
                    // would not be painted at the end of the new match.
                    menu.ctx().memory_mut(|memory| memory.request_focus(editor_id));
                    // Store the range immediately as well: the widget may have
                    // stored a collapsed cursor earlier in this frame, and the
                    // caret must be drawn at the end of the new match on the
                    // first frame after the menu closes.
                    let mut edit = output.state.clone();
                    edit.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(next.start),
                        egui::text::CCursor::new(next.end),
                    )));
                    let cursor = edit.cursor.char_range();
                    edit.store(menu.ctx(), editor_id);
                    state.selection = None;
                    state.cursor = cursor;
                    state.pending_selection = Some(next);
                    menu.ctx().request_repaint();
                    menu.close_menu();
                }
            });
            if let Some((line, row_index)) = depth_action {
                state.folds.toggle_depth(line);
                log::info!(target: "litetext::fold", "Toggle depth: tab_id={file_id}; line={}; collapsed={}", line + 1, state.folds.collapsed(line));
                // Re-layout once, then compensate for same-depth blocks above
                // the clicked header. Numeric offset alone cannot anchor it.
                let galley = ui.fonts(|fonts| fonts.layout_job((*output.galley.job).clone()));
                let galley = state.folds.layout(galley, 0);
                let delta = galley.rows[row_index].rect.top() - output.galley.rows[row_index].rect.top();
                let offset = depth_scroll_offset(viewport, delta, galley.size().y + 24.0);
                depth_offset = Some(offset);
                // Batch folding may remove most of the document, including
                // nodes above this header. Keep the real document origin and
                // scrollbar range instead of manufacturing scrollable space.
                state.fold_scroll_extent = 0.0;
                let start = text.split_inclusive('\n').take(line).map(|l| l.chars().count()).sum();
                state.pending_selection = Some(start..start);
                ui.ctx().request_repaint();
            }
            ui.set_min_height(state.fold_scroll_extent);
        });
    let scroll_output = finish_fold_scroll(ui.ctx(), scroll_output, depth_offset.or((fold_pointer || fold_selection).then_some(viewport_offset)));
    ui.ctx().data_mut(|data| {
        data.insert_temp(
            egui::Id::new(("editor_scroll_y", file_id)),
            scroll_output.state.offset.y,
        )
    });
    #[cfg(test)]
    ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("editor_scroll_geometry", file_id)), (scroll_output.id, scroll_output.state.offset, scroll_output.inner_rect)));
    #[cfg(test)]
    ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("editor_content_geometry", file_id)), (gutter, scroll_output.content_size)));
    #[cfg(not(test))]
    let _ = scroll_output;
    state.selected_characters = if let Some(selection) = state.selection {
        copy_lines(text, selection).chars().count()
    } else {
        state
            .cursor
            .map_or(0, |c| c.primary.index.abs_diff(c.secondary.index))
    };
    if changed && let Some(before) = before_layout {
        let (removed, inserted) = crate::large_editor::difference(&before, text);
        state.tools.apply_edit(removed, inserted.len());
    }
    ui.painter().line_segment(
        [gutter.right_top(), gutter.right_bottom()],
        Stroke::new(1.0_f32, pal.text_faint.gamma_multiply(0.55)),
    );
    changed
}

/// TextEdit may request an implicit cursor reveal even on the press frame of a
/// gutter click. A display-only fold must cancel that target and its animation,
/// not just restore the numeric offset while leaving a pending target alive.
pub(crate) fn finish_fold_scroll<R>(
    ctx: &egui::Context,
    mut output: egui::scroll_area::ScrollAreaOutput<R>,
    offset: Option<egui::Vec2>,
) -> egui::scroll_area::ScrollAreaOutput<R> {
    if let Some(offset) = offset {
        output.state = egui::scroll_area::State::default();
        output.state.offset = offset;
        output.state.store(ctx, output.id);
    }
    output
}

/// Split-view rows are stretched to the rendered block height and the source
/// text is top-aligned inside them, so a vertically centered number would sit
/// far below its own line. Keep the number centered on the first text line.
fn gutter_number_center(row: egui::Rect, line_box: f32, top_aligned: bool) -> f32 {
    if top_aligned {
        row.top() + line_box.min(row.height()) * 0.5
    } else {
        row.center().y
    }
}

pub(crate) fn depth_scroll_offset(
    viewport: egui::Rect,
    delta: f32,
    content_height: f32,
) -> egui::Vec2 {
    let max_offset = (content_height - viewport.height()).max(0.0);
    egui::vec2(viewport.left(), (viewport.top() + delta).clamp(0.0, max_offset))
}

/// Split-view line heights are applied to the syntax-highlighted layout job
/// instead of post-processing the galley: egui then places glyphs, cursors and
/// selections using the stretched rows from the start.
fn apply_line_heights(job: &mut egui::text::LayoutJob, heights: &[f32], fonts: &[f32]) {
    if heights.is_empty() || job.sections.is_empty() {
        return;
    }
    let mut starts = vec![0usize];
    for (index, byte) in job.text.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(index + 1);
        }
    }
    let line_end = |line: usize| starts.get(line + 1).copied().unwrap_or(job.text.len());
    let mut sections = Vec::with_capacity(job.sections.len());
    for section in job.sections.drain(..) {
        let mut start = section.byte_range.start;
        let mut first = true;
        while start < section.byte_range.end {
            let line = starts.partition_point(|offset| *offset <= start).saturating_sub(1);
            let end = section.byte_range.end.min(line_end(line).max(start + 1));
            let mut format = section.format.clone();
            // A new line typed in split view arrives before the next measurement
            // pass; fall back to the editor font height instead of a 1 px row.
            let fallback = (format.font_id.size + 4.0).max(1.0);
            format.line_height = Some(heights.get(line).copied().unwrap_or(fallback).max(1.0));
            // Tall rendered blocks should align the source line at the top of
            // its stretched row instead of sinking to the row baseline.
            format.valign = egui::Align::TOP;
            // Headings and code inherit the rendered block's font size; only
            // the family (monospace) stays that of the source editor.
            if let Some(size) = fonts.get(line).copied().filter(|size| *size > 0.0) {
                format.font_id.size = size;
            }
            sections.push(egui::text::LayoutSection {
                leading_space: if first { section.leading_space } else { 0.0 },
                byte_range: start..end,
                format,
            });
            start = end;
            first = false;
        }
    }
    job.sections = sections;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_gutter_numbers_follow_the_top_aligned_source_line() {
        let row = egui::Rect::from_min_size(egui::pos2(0.0, 100.0), egui::vec2(40.0, 120.0));
        assert!((gutter_number_center(row, 20.0, true) - 110.0).abs() < 0.01);
        // Ordinary editor rows still center the number vertically.
        assert!((gutter_number_center(row, 20.0, false) - 160.0).abs() < 0.01);
    }

    #[test]
    fn split_line_heights_stretch_layout_rows_before_placement() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let mut job = egui::text::LayoutJob::simple(
                    "heading\nbody\n".to_owned(),
                    FontId::monospace(14.0),
                    egui::Color32::WHITE,
                    400.0,
                );
                apply_line_heights(&mut job, &[60.0, 22.0, 12.0], &[30.0, 14.0, 14.0]);
                let galley = ui.fonts(|fonts| fonts.layout_job(job));
                assert!(galley.rows[0].rect.height() >= 59.0, "{:?}", galley.rows[0].rect);
                assert!(galley.rows[1].rect.height() >= 21.0, "{:?}", galley.rows[1].rect);
                assert!(galley.rows[0].rect.bottom() <= galley.rows[1].rect.top() + 0.5);
                // The heading row must also carry bigger glyphs, not just a
                // taller empty row above body-sized text.
                let heading = galley.rows[0]
                    .glyphs
                    .iter()
                    .map(|glyph| glyph.font_height)
                    .fold(0.0_f32, f32::max);
                let body = galley.rows[1]
                    .glyphs
                    .iter()
                    .map(|glyph| glyph.font_height)
                    .fold(0.0_f32, f32::max);
                assert!(heading > body * 1.5, "heading {heading}, body {body}");
            });
        });
    }

    #[test]
    fn scrollbars_only_take_space_in_directions_with_overflow() {
        let viewport = |text: &str| {
            let mut h = Harness::new(text);
            h.ctx.style_mut(|s| s.animation_time = 0.0);
            for _ in 0..5 { h.run(vec![]); }
            h.ctx.data(|d| d.get_temp::<(egui::Id, egui::Vec2, egui::Rect)>(
                egui::Id::new(("editor_scroll_geometry", 1u64)))).unwrap().2
        };
        let fitting = viewport("hello");
        let vertical = viewport(&"short\n".repeat(100));
        let horizontal = viewport(&"long ".repeat(500));
        let both = viewport(&format!("{}\n", "long ".repeat(500)).repeat(100));
        assert!((fitting.bottom() - 600.0).abs() < 1.0, "{fitting:?}");
        assert!((fitting.right() - 800.0).abs() < 1.0, "{fitting:?}");
        assert!(vertical.right() < fitting.right());
        assert!((vertical.bottom() - fitting.bottom()).abs() < 1.0);
        assert!(horizontal.bottom() < fitting.bottom());
        assert!((horizontal.right() - fitting.right()).abs() < 1.0);
        assert!(both.right() < fitting.right() && both.bottom() < fitting.bottom());
    }

    #[test]
    fn scrollbars_stay_muted_and_neutral_in_both_themes() {
        for pal in [&crate::DARK, &crate::LIGHT] {
            let ctx = egui::Context::default();
            let _ = ctx.run(Default::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    scroll_style(ui, pal);
                    let v = ui.visuals();
                    assert_eq!(
                        v.widgets.inactive.fg_stroke.color,
                        pal.text_faint.gamma_multiply(0.45)
                    );
                    assert_eq!(
                        v.widgets.hovered.fg_stroke.color,
                        pal.text_faint.gamma_multiply(0.65)
                    );
                    assert_eq!(
                        v.widgets.active.fg_stroke.color,
                        pal.text_faint.gamma_multiply(0.85)
                    );
                    assert!(!ui.spacing().scroll.floating);
                });
            });
        }
    }

    #[test]
    fn deep_jsonl_rows_align_and_scrollbar_thumb_tracks_progress_at_all_scales() {
        for large in [false, true] {
            for ppp in [1.0, 1.25, 2.0] {
                let original = (0..1000)
                    .map(|i| {
                        format!(
                            "{{\"row\":{i},\"中文🙂\":\"{}\"}}\n",
                            "x".repeat(if large { 300 } else { 12 })
                        )
                    })
                    .collect::<String>();
                let mut h = Harness::new(&original);
                crate::setup_fonts(&h.ctx);
                h.ctx.set_pixels_per_point(ppp);
                h.ctx.style_mut(|s| s.animation_time = 0.0);
                for size in [13.5, 10.0, 32.0, 13.5] {
                    h.state.font_size = size;
                    for _ in 0..3 {
                        h.run(vec![]);
                    }
                    let mut thumb_tops = Vec::new();
                    for fraction in [0.0_f32, 0.5, 0.95] {
                        h.ctx.data_mut(|d| {
                            d.insert_temp(egui::Id::new("test_scroll_fraction"), fraction)
                        });
                        let output = h.run(vec![]);
                        let document = output
                            .shapes
                            .iter()
                            .find_map(|s| match &s.shape {
                                egui::Shape::Text(t) if t.galley.text().contains("\"row\"") => {
                                    Some(t)
                                }
                                _ => None,
                            })
                            .unwrap();
                        let base = document
                            .galley
                            .text()
                            .split("\"row\":")
                            .nth(1)
                            .unwrap()
                            .split(',')
                            .next()
                            .unwrap()
                            .parse::<usize>()
                            .unwrap();
                        let mut checked = 0;
                        for shape in &output.shapes {
                            if let egui::Shape::Text(number) = &shape.shape
                                && let Ok(line) = number.galley.text().parse::<usize>()
                                && number.pos.y > 20.0
                                && number.pos.y < 550.0
                            {
                                let row = &document.galley.rows[line - 1 - base];
                                let expected = document.pos.y + row.rect.center().y;
                                let actual = number.pos.y + number.galley.rows[0].rect.center().y;
                                assert_eq!(number.galley.job.sections[0].format.font_id.size, size - 1.0);
                                assert_eq!(document.galley.job.sections[0].format.font_id.size, size);
                                let right_gap = document.pos.x - (number.pos.x + number.galley.size().x);
                                assert!(number.pos.x + 1.0 / ppp >= right_gap,
                                    "left padding must balance document gap: large={large}, size={size}, line={line}, left={}, right={right_gap}", number.pos.x);
                                assert!(
                                    (actual - expected).abs() < 1.1 / ppp,
                                    "large={large}, ppp={ppp}, size={size}, line={line}: {actual} != {expected}"
                                );
                                checked += 1;
                            }
                        }
                        assert!(checked > 5);
                        let thumb = output
                            .shapes
                            .iter()
                            .find_map(|s| match &s.shape {
                                egui::Shape::Rect(r)
                                    if r.fill == crate::DARK.text_faint.gamma_multiply(0.45)
                                        && r.rect.width() <= 11.0
                                        && r.rect.right() > 780.0
                                        && r.rect.height() > 10.0 =>
                                {
                                    Some(r.rect)
                                }
                                _ => None,
                            })
                            .expect("Visible muted vertical scrollbar thumb");
                        thumb_tops.push(thumb.top());
                        if large {
                            assert!(h.state.large.laid_out_bytes < 64 * 1024);
                        }
                    }
                    assert!(
                        thumb_tops[0] < thumb_tops[1] && thumb_tops[1] < thumb_tops[2],
                        "large={large} ppp={ppp} size={size}: {thumb_tops:?}"
                    );
                }
                assert_eq!(h.text, original);
            }
        }
    }

    #[test]
    fn cursor_line_uses_logical_unicode_lines_in_both_editors() {
        for large in [false, true] {
            let text = format!(
                "中文🙂\r\n{}\nlast{}",
                "longword".repeat(200),
                if large {
                    "\npadding".repeat(40000)
                } else {
                    String::new()
                }
            );
            let mut h = Harness::new(&text);
            h.state.tools.wrap = true;
            let at = text[..text.find("last").unwrap()].chars().count();
            h.state.pending_selection = Some(at..at);
            h.run(vec![]);
            assert_eq!(h.state.cursor_line, 2);
            assert_eq!(h.state.selected_characters, 0);
            h.state.pending_selection = Some(at..at + 4);
            h.run(vec![]);
            assert_eq!(h.state.selected_characters, 4);
            assert_eq!(h.text, text);
        }
    }

    #[test]
    fn deep_pending_cursor_reveals_the_requested_large_document_line() {
        let text = (0..1000)
            .map(|i| format!("row {i}: {}\n", "中文".repeat(100)))
            .collect::<String>();
        let mut h = Harness::new(&text);
        let at = text[..text.find("row 990:").unwrap()].chars().count();
        h.state.pending_selection = Some(at..at);
        for _ in 0..30 {
            h.run(vec![]);
        }
        let output = h.run(vec![]);
        let row = output
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == "991" => Some(t.pos.y),
                _ => None,
            })
            .expect("Requested deep line number should be rendered");
        assert!((0.0..580.0).contains(&row));
        assert_eq!(h.state.cursor_line, 990);
        assert!(h.state.large.laid_out_bytes < 64 * 1024);
        assert_eq!(h.text, text);
    }

    #[test]
    fn unicode_words_stop_at_ascii_and_chinese_punctuation() {
        let text = r#"{"message":"中文内容,English_word42！测试。"}"#;
        for expected in ["中文内容", "English_word42", "测试", ",", "！", "。", "\""] {
            let start = text[..text.find(expected).unwrap()].chars().count();
            for offset in 0..expected.chars().count() {
                let range = word_range(text, start + offset);
                assert_eq!(
                    text.chars()
                        .skip(range.start)
                        .take(range.len())
                        .collect::<String>(),
                    expected
                );
            }
        }
        assert_eq!(word_range("", 0), 0..0);
        assert_eq!(word_range("中文", 2), 2..2);
        for (text, index, expected) in [
            ("中文English", 1, "中文"),
            ("中文English", 3, "English"),
            ("甲\r\n乙", 0, "甲"),
            ("甲\r\n乙", 3, "乙"),
            ("café!", 2, "café"),
            ("𠀀中文,", 0, "𠀀中文"),
        ] {
            let range = word_range(text, index);
            assert_eq!(
                text.chars()
                    .skip(range.start)
                    .take(range.len())
                    .collect::<String>(),
                expected
            );
        }
    }

    #[test]
    fn real_double_click_selects_and_copies_words_in_both_editors() {
        let sample = r#"{"msg":"中文内容,English_word42！测试。"}"#;
        for (large, wrap) in [(false, false), (false, true), (true, false), (true, true)] {
            for (expected, offset) in [
                ("中文内容", 0),
                ("中文内容", 3),
                ("English_word42", 4),
                ("测试", 1),
                (",", 0),
            ] {
                let prefix = if large {
                    "padding\n".repeat(20)
                } else {
                    String::new()
                };
                let suffix = if large {
                    "\npadding".repeat(40000)
                } else {
                    "\nend".into()
                };
                let original = format!("{prefix}{sample}{suffix}");
                let mut harness = Harness::new(&original);
                harness.state.tools.wrap = wrap;
                let start = prefix.chars().count()
                    + sample[..sample.find(expected).unwrap()].chars().count();
                for _ in 0..4 {
                    harness.run(vec![]);
                }
                let output = harness.run(vec![]);
                let (pos, galley) = output
                    .shapes
                    .iter()
                    .find_map(|s| match &s.shape {
                        egui::Shape::Text(t) if t.galley.text().contains(sample) => {
                            Some((t.pos, t.galley.clone()))
                        }
                        _ => None,
                    })
                    .expect("Document text must be rendered");
                let local_start = galley.text()[..galley.text().find(expected).unwrap()]
                    .chars()
                    .count();
                let a = galley.pos_from_ccursor(egui::text::CCursor::new(local_start + offset));
                let b = galley.pos_from_ccursor(egui::text::CCursor::new(local_start + offset + 1));
                // Hit the glyph's right half to exercise nearest-caret rounding.
                let pointer =
                    pos + egui::vec2(a.center().x * 0.25 + b.center().x * 0.75, a.center().y);
                for _ in 0..2 {
                    harness.button(pointer, egui::PointerButton::Primary, true);
                    harness.button(pointer, egui::PointerButton::Primary, false);
                }
                let cursor = harness.state.cursor.unwrap();
                assert_eq!(
                    cursor.secondary.index.min(cursor.primary.index),
                    start,
                    "large={large} {expected}"
                );
                assert_eq!(
                    cursor.secondary.index.max(cursor.primary.index),
                    start + expected.chars().count(),
                    "large={large} {expected} pointer={pointer:?}"
                );
                assert_eq!(harness.state.selected_characters, expected.chars().count());
                let copied = harness.run(vec![egui::Event::Copy]);
                assert!(
                    copied.platform_output.commands.iter().any(
                        |c| matches!(c, egui::OutputCommand::CopyText(value) if value == expected)
                    ),
                    "large={large} {expected}"
                );
                assert_eq!(harness.text, original);
                if large {
                    assert!(harness.state.large.laid_out_bytes < 64 * 1024);
                }
                harness.run(vec![egui::Event::Text("replacement".into())]);
                let at = original.find(expected).unwrap();
                let mut replaced = original.clone();
                replaced.replace_range(at..at + expected.len(), "replacement");
                assert_eq!(harness.text, replaced);
                harness.run(vec![egui::Event::Key {
                    key: egui::Key::Z,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::COMMAND,
                }]);
                assert_eq!(harness.text, original);
            }
        }
    }

    #[test]
    fn wrapping_whitespace_and_unicode_selection_do_not_modify_document() {
        let original = format!("{}\r\n中文🙂\t end", "longword".repeat(100));
        let mut harness = Harness::new(&original);
        harness.state.tools.wrap = true;
        harness.state.tools.whitespace = true;
        harness.state.pending_selection = Some(802..805);
        let output = harness.run(vec![]);
        assert_eq!(harness.text, original);
        assert_eq!(harness.state.selected_characters, 3);
        let galley = output
            .shapes
            .iter()
            .find_map(|s| {
                if let egui::Shape::Text(t) = &s.shape {
                    (t.galley.text() == original).then_some(&t.galley)
                } else {
                    None
                }
            })
            .unwrap();
        assert!(galley.rows.len() > 2);
        assert!(galley.size().x < 800.0);
    }

    #[test]
    fn gutter_width_uses_two_four_then_exact_columns() {
        for (lines, columns) in [
            (0, 2),
            (1, 2),
            (9, 2),
            (10, 2),
            (99, 2),
            (100, 4),
            (999, 4),
            (1000, 4),
            (9999, 4),
            (10000, 5),
            (99999, 5),
            (100000, 6),
            (666666, 6),
            (1000000, 7),
        ] {
            assert_eq!(gutter_columns(lines), columns);
        }
    }

    #[test]
    fn rendered_gutter_grows_and_shrinks_with_total_line_count() {
        let mut harness = Harness::new("");
        let first = harness.row_pos(0).x;
        harness.text = "\n".repeat(9);
        let ten = harness.row_pos(0).x;
        harness.text = "\n".repeat(99);
        let hundred = harness.row_pos(0).x;
        assert!(first < 32.0, "增加左留白后仍保持紧凑行号栏");
        assert_eq!(ten, first);
        assert!(hundred > ten);
        harness.text = "\n".repeat(999);
        assert_eq!(harness.row_pos(0).x, hundred);
        harness.text.clear();
        assert_eq!(harness.row_pos(0).x, first);
    }

    #[test]
    fn gutter_left_padding_does_not_duplicate_the_separate_fold_lane() {
        for count in [1usize, 10, 100, 1000, 10000] {
            for digit_width in [5.0, 8.0, 19.0] {
                let width = gutter_number_width(count, digit_width);
                let reserved_digits = gutter_columns(count) as f32 * digit_width;
                let left = width - GUTTER_NUMBER_GAP - reserved_digits;
                let right = GUTTER_NUMBER_GAP;
                assert_eq!(left, 3.0);
                assert_eq!(width, 3.0 + reserved_digits + 3.0);
                assert!((left - right).abs() < 0.01);
            }
        }
    }

    #[test]
    fn gutter_border_has_three_point_number_gap_and_compact_text_padding() {
        let mut harness = Harness::new("hello");
        let output = harness.run(vec![]);
        let mut number_right = None;
        let mut text_left = None;
        let mut border_x = None;
        for shape in output.shapes {
            match shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "1" => {
                    number_right = Some(text.pos.x + text.galley.size().x);
                }
                egui::Shape::Text(text) if text.galley.text() == "hello" => {
                    text_left = Some(text.pos.x);
                }
                egui::Shape::LineSegment { points, .. }
                    if points[0].x == points[1].x && points[1].y - points[0].y > 500.0 =>
                {
                    border_x = Some(points[0].x);
                }
                _ => {}
            }
        }
        let border = border_x.expect("行号分隔线应渲染");
        assert!((border - number_right.unwrap() - 3.0).abs() < 0.01);
        assert!((text_left.unwrap() - border - 4.0).abs() < 0.01);
    }

    #[test]
    fn line_actions_preserve_unicode_empty_lines_and_line_breaks() {
        assert_eq!(line_ranges(""), vec![0..0]);
        assert_eq!(
            copy_lines("第一行\n第二行\n\nlast", (0, 2)),
            "第一行\n第二行\n"
        );
        let mut text = "keep\r\n中文🙂\r\nthird\r\nlast".to_owned();
        assert_eq!(copy_lines(&text, (1, 2)), "中文🙂\nthird");
        assert_eq!(clear_lines(&mut text, (1, 2)), 6);
        assert_eq!(text, "keep\r\n\r\n\r\nlast");
        let mut text = "only".to_owned();
        clear_lines(&mut text, (0, 0));
        assert_eq!(text, "");
    }

    struct Harness {
        ctx: egui::Context,
        text: String,
        state: EditorState,
        frame: usize,
        lang: crate::Lang,
    }

    impl Harness {
        fn new(text: &str) -> Self {
            let mut harness = Self {
                ctx: egui::Context::default(),
                text: text.to_owned(),
                state: EditorState::default(),
                frame: 0,
                lang: crate::Lang::En,
            };
            harness.run(vec![]);
            harness.run(vec![]);
            harness
        }

        fn run(&mut self, events: Vec<egui::Event>) -> egui::FullOutput {
            let output = self.ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 600.0),
                    )),
                    time: Some(self.frame as f64 / 60.0),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    crate::apply_style(ctx, &crate::DARK);
                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE)
                        .show(ctx, |ui| {
                            if let Some(fraction) = ctx.data_mut(|d| {
                                d.remove_temp::<f32>(egui::Id::new("test_scroll_fraction"))
                            }) {
                                let id =
                                    ui.make_persistent_id(egui::Id::new(("draft_scroll", 1u64)));
                                let mut scroll =
                                    egui::scroll_area::State::load(ctx, id).unwrap_or_default();
                                scroll.offset.y =
                                    row_height(ui, &self.state.font()) * 1000.0 * fraction;
                                scroll.store(ctx, id);
                            }
                            show(
                                ui,
                                &mut self.text,
                                &mut self.state,
                                1,
                                &crate::DARK,
                                false,
                                crate::tr(self.lang),
                            );
                        });
                },
            );
            self.frame += 1;
            output
        }

        fn button(
            &mut self,
            pos: egui::Pos2,
            button: egui::PointerButton,
            pressed: bool,
        ) -> egui::FullOutput {
            self.run(vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button,
                    pressed,
                    modifiers: egui::Modifiers::default(),
                },
            ])
        }

        fn row_pos(&mut self, line: usize) -> egui::Pos2 {
            text_pos(self.run(vec![]), &(line + 1).to_string()).expect("行号应渲染")
        }

        fn select_rows(&mut self, first: usize, last: usize) {
            let first = self.row_pos(first);
            let last = self.row_pos(last);
            self.button(first, egui::PointerButton::Primary, true);
            self.run(vec![egui::Event::PointerMoved(last)]);
            self.button(last, egui::PointerButton::Primary, false);
        }

        fn open_menu(&mut self, line: usize) -> egui::FullOutput {
            let pos = self.row_pos(line);
            self.button(pos, egui::PointerButton::Secondary, true);
            self.button(pos, egui::PointerButton::Secondary, false);
            self.run(vec![])
        }

        fn open_body(&mut self, pos: egui::Pos2) -> egui::FullOutput {
            self.button(pos, egui::PointerButton::Secondary, true);
            self.button(pos, egui::PointerButton::Secondary, false);
            self.run(vec![])
        }

        fn click_menu(&mut self, output: egui::FullOutput, label: &str) -> egui::FullOutput {
            let pos = text_pos(output, label).expect("右键菜单项应渲染");
            self.button(pos, egui::PointerButton::Primary, true);
            self.button(pos, egui::PointerButton::Primary, false)
        }
    }

    fn text_pos(output: egui::FullOutput, label: &str) -> Option<egui::Pos2> {
        output.shapes.into_iter().find_map(|shape| {
            if let egui::Shape::Text(text) = shape.shape
                && text.galley.text() == label
            {
                Some(text.pos + text.galley.size() * 0.5)
            } else {
                None
            }
        })
    }

    #[test]
    fn line_menus_are_compact_single_line_and_copy_selected_content() {
        for large in [false, true] {
            for lang in [crate::Lang::ZhCn, crate::Lang::En] {
                let source = if large { "alpha\nbeta\n".repeat(30000) } else { "alpha\nbeta\ngamma".into() };
                let mut harness = Harness::new(&source);
                crate::setup_fonts(&harness.ctx);
                harness.lang = lang;
                harness.state.font_size = 32.0; // menu must not inherit document zoom
                harness.select_rows(0, 1);
                let output = harness.open_menu(0);
                let t = crate::tr(lang);
                let text = |label| output.shapes.iter().find_map(|s| match &s.shape {
                    egui::Shape::Text(text) if text.galley.text() == label => Some(text),
                    _ => None,
                }).unwrap();
                let copy = text(t.copy_lines);
                let cut = text(t.cut);
                let clear = text(t.clear_content);
                assert_eq!(copy.galley.rows.len(), 1);
                assert_eq!(cut.galley.rows.len(), 1);
                assert_eq!(clear.galley.rows.len(), 1);
                let gap = cut.pos.y - (copy.pos.y + copy.galley.size().y);
                assert!((0.0..=14.0).contains(&gap), "{lang:?}, large={large}: menu gap={gap}");
                assert_eq!(copy.galley.job.sections[0].format.font_id.size, 13.0);
                let result = harness.click_menu(output, t.copy_lines);
                assert!(result.platform_output.commands.iter().any(|cmd|
                    matches!(cmd, egui::OutputCommand::CopyText(text) if text == "alpha\nbeta")));
            }
        }
    }

    #[test]
    fn blank_editor_has_first_line_and_right_click_copies_only_line_contents() {
        let mut blank = Harness::new("");
        assert!(blank.row_pos(0).x < 54.0);
        let mut harness = Harness::new("alpha\nbeta\ngamma");
        let menu = harness.open_menu(1);
        let result = harness.click_menu(menu, "Copy This Line");
        assert_eq!(harness.state.selection, Some((1, 1)));
        assert!(
            result
                .platform_output
                .commands
                .iter()
                .any(|cmd| matches!(cmd, egui::OutputCommand::CopyText(text) if text == "beta"))
        );
    }

    #[test]
    fn context_menu_offers_cut_paste_and_select_all() {
        let t = crate::i18n::tr(crate::Lang::En);
        let mut harness = Harness::new("alpha\nbeta");
        let menu = harness.open_menu(0);
        let labels: Vec<_> = menu
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.text()),
                _ => None,
            })
            .collect();
        for label in [t.cut, t.paste, t.select_all] {
            assert!(labels.contains(&label), "missing {label:?}: {labels:?}");
        }
        harness.click_menu(menu, t.select_all);
        let editor =
            egui::TextEdit::load_state(&harness.ctx, egui::Id::new("draft_editor").with(1u64))
                .unwrap();
        let range = editor.cursor.char_range().unwrap();
        assert_eq!(
            (
                range.primary.index.min(range.secondary.index),
                range.primary.index.max(range.secondary.index),
            ),
            (0, "alpha\nbeta".chars().count())
        );

        let mut harness = Harness::new("alpha\nbeta");
        let menu = harness.open_menu(0);
        let result = harness.click_menu(menu, t.paste);
        assert!(
            result
                .viewport_output
                .values()
                .any(|viewport| viewport
                    .commands
                    .contains(&egui::ViewportCommand::RequestPaste)),
            "paste must ask the platform for clipboard contents"
        );

        let mut harness = Harness::new("alpha\nbeta");
        let menu = harness.open_menu(0);
        let result = harness.click_menu(menu, t.cut);
        assert!(result.platform_output.commands.iter().any(
            |command| matches!(command, egui::OutputCommand::CopyText(text) if text == "alpha")
        ));
        assert_eq!(harness.text, "\nbeta");
    }

    #[test]
    fn body_context_menu_offers_basic_editing_actions() {
        let t = crate::i18n::tr(crate::Lang::En);
        let mut harness = Harness::new("alpha\nbeta");
        crate::setup_fonts(&harness.ctx);
        let pos = text_pos(harness.run(vec![]), "alpha\nbeta")
            .expect("body text must be rendered");
        for pressed in [true, false] {
            harness.button(pos, egui::PointerButton::Secondary, pressed);
        }
        let menu = harness.run(vec![]);
        let labels: Vec<_> = menu
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.text()),
                _ => None,
            })
            .collect();
        for label in [t.copy_line, t.cut, t.paste, t.select_all] {
            assert!(labels.contains(&label), "missing {label:?}: {labels:?}");
        }
        assert!(!labels.contains(&t.clear_content));
        assert_eq!(
            menu.shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == t.jump_next => {
                        Some(text.fallback_color)
                    }
                    _ => None,
                })
                .count(),
            1,
            "the jump item must be listed even without a selection"
        );
        harness.click_menu(menu, t.select_all);
        let editor =
            egui::TextEdit::load_state(&harness.ctx, egui::Id::new("draft_editor").with(1u64))
                .unwrap();
        let range = editor.cursor.char_range().unwrap();
        assert_eq!(
            (
                range.primary.index.min(range.secondary.index),
                range.primary.index.max(range.secondary.index),
            ),
            (0, "alpha\nbeta".chars().count())
        );
    }

    #[test]
    fn body_menu_copy_keeps_the_selection_collapsed_by_right_click() {
        let t = crate::i18n::tr(crate::Lang::En);
        let source = "alpha beta alpha";
        let mut harness = Harness::new(source);
        crate::setup_fonts(&harness.ctx);
        let editor_id = egui::Id::new("draft_editor").with(1u64);
        let mut edit =
            egui::TextEdit::load_state(&harness.ctx, editor_id).unwrap_or_default();
        edit.cursor.set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(0),
            egui::text::CCursor::new(5),
        )));
        edit.store(&harness.ctx, editor_id);
        harness.state.cursor = Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(0),
            egui::text::CCursor::new(5),
        ));
        harness.run(vec![]);
        let pos = text_pos(harness.run(vec![]), source).expect("body text");
        let menu = harness.open_body(pos);
        let result = harness.click_menu(menu, t.copy_lines);
        assert!(
            result
                .platform_output
                .commands
                .iter()
                .any(|command| matches!(command, egui::OutputCommand::CopyText(text) if text == "alpha")),
            "right-clicking must still copy the selected text, not the whole line"
        );
    }

    #[test]
    fn right_click_inside_a_selection_keeps_it_highlighted() {
        let source = "alpha beta alpha";
        let mut harness = Harness::new(source);
        crate::setup_fonts(&harness.ctx);
        let editor_id = egui::Id::new("draft_editor").with(1u64);
        let selected = egui::text::CCursorRange::two(
            egui::text::CCursor::new(0),
            egui::text::CCursor::new(5),
        );
        let mut edit =
            egui::TextEdit::load_state(&harness.ctx, editor_id).unwrap_or_default();
        edit.cursor.set_char_range(Some(selected));
        edit.store(&harness.ctx, editor_id);
        harness.state.cursor = Some(selected);
        harness.run(vec![]);
        let pos = text_pos(harness.run(vec![]), source).expect("body text");
        // `text_pos` is the galley centre; step left into the selected word.
        let inside = pos - egui::vec2(45.0, 0.0);
        let _menu = harness.open_body(inside);
        let state =
            egui::TextEdit::load_state(&harness.ctx, editor_id).expect("editor state");
        let range = state.cursor.char_range().expect("selection must survive");
        assert_eq!(
            (
                range.primary.index.min(range.secondary.index),
                range.primary.index.max(range.secondary.index),
            ),
            (0, 5),
            "right-clicking inside the selection must keep it highlighted"
        );
    }

    #[test]
    fn next_occurrence_wraps_and_skips_the_current_selection() {
        let text = "foo bar foo baz foo";
        assert_eq!(next_occurrence(text, "foo", 0..3), Some(8..11));
        assert_eq!(next_occurrence(text, "foo", 8..11), Some(16..19));
        assert_eq!(next_occurrence(text, "foo", 16..19), Some(0..3));
        assert_eq!(next_occurrence(text, "missing", 0..3), None);
        assert_eq!(next_occurrence(text, "", 0..0), None);
        assert_eq!(next_occurrence("only", "only", 0..4), None);
    }

    #[test]
    fn body_menu_jump_next_selects_the_following_occurrence_and_wraps() {
        let t = crate::i18n::tr(crate::Lang::En);
        let source = "beta alpha beta gamma";
        let mut harness = Harness::new(source);
        crate::setup_fonts(&harness.ctx);
        let editor_id = egui::Id::new("draft_editor").with(1u64);
        let select = |harness: &mut Harness, start: usize, end: usize| {
            let mut edit =
                egui::TextEdit::load_state(&harness.ctx, editor_id).unwrap_or_default();
            edit.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(start),
                egui::text::CCursor::new(end),
            )));
            edit.store(&harness.ctx, editor_id);
            harness.state.cursor = Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(start),
                egui::text::CCursor::new(end),
            ));
            harness.run(vec![]);
        };
        let selection = |harness: &Harness| {
            let edit = egui::TextEdit::load_state(&harness.ctx, editor_id).unwrap();
            let range = edit.cursor.char_range().unwrap();
            (
                range.primary.index.min(range.secondary.index),
                range.primary.index.max(range.secondary.index),
            )
        };
        select(&mut harness, 0, 4);
        let pos = text_pos(harness.run(vec![]), source).expect("body text");
        let menu = harness.open_body(pos);
        harness.click_menu(menu, t.jump_next);
        harness.run(vec![]);
        assert_eq!(selection(&harness), (11, 15));

        let menu = harness.open_body(pos);
        harness.click_menu(menu, t.jump_next);
        harness.run(vec![]);
        assert_eq!(selection(&harness), (0, 4), "jumping must wrap to the top");
    }

    #[test]
    fn body_menu_insert_time_at_the_caret_and_replaces_the_selection() {
        let t = crate::i18n::tr(crate::Lang::En);
        let source = "alpha beta";
        let mut harness = Harness::new(source);
        crate::setup_fonts(&harness.ctx);
        let editor_id = egui::Id::new("draft_editor").with(1u64);
        let select = |harness: &mut Harness, start: usize, end: usize| {
            let mut edit =
                egui::TextEdit::load_state(&harness.ctx, editor_id).unwrap_or_default();
            edit.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(start),
                egui::text::CCursor::new(end),
            )));
            edit.store(&harness.ctx, editor_id);
            harness.state.cursor = Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(start),
                egui::text::CCursor::new(end),
            ));
            harness.run(vec![]);
        };
        select(&mut harness, 0, 5);
        let pos = text_pos(harness.run(vec![]), source).expect("body text");
        let menu = harness.open_body(pos);
        let _ = harness.click_menu(menu, t.insert_time);
        let _ = harness.run(vec![]);
        let submenu = harness.run(vec![]);
        harness.click_menu(submenu, "YYYY-mm-dd HH:MM:SS");
        harness.run(vec![]);
        let stamp = harness.text[..19].to_owned();
        let full = regex::Regex::new(r"^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$").unwrap();
        assert!(full.is_match(&stamp), "unexpected stamp: {stamp:?}");
        assert_eq!(&harness.text[19..], " beta", "the selection must be replaced");
        let state = egui::TextEdit::load_state(&harness.ctx, editor_id).unwrap();
        let range = state.cursor.char_range().expect("insert must leave a caret");
        assert_eq!(range.primary.index, 19, "the caret must follow the stamp");
        assert!(
            harness.ctx.memory(|memory| memory.has_focus(editor_id)),
            "focus returns so the caret is painted"
        );
        // The stamp is one undoable edit.
        harness.run(vec![egui::Event::Key {
            key: egui::Key::Z,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        }]);
        harness.run(vec![]);
        assert_eq!(harness.text, source);
    }

    #[test]
    fn large_editor_body_menu_inserts_time_at_the_caret() {
        let t = crate::i18n::tr(crate::Lang::En);
        let source = "alpha beta gamma";
        let mut harness = Harness::new(source);
        crate::setup_fonts(&harness.ctx);
        harness.state.large.active = true;
        harness.run(vec![]);
        harness.state.large.set_tool_cursor(egui::text::CCursorRange::two(
            egui::text::CCursor::new(6),
            egui::text::CCursor::new(11),
        ));
        harness.run(vec![]);
        let body = text_pos(harness.run(vec![]), source).expect("large editor body");
        let menu = harness.open_body(body);
        let _ = harness.click_menu(menu, t.insert_time);
        let _ = harness.run(vec![]);
        let submenu = harness.run(vec![]);
        harness.click_menu(submenu, "YYYY/mm/dd");
        harness.run(vec![]);
        let date = regex::Regex::new(r"^alpha \d{4}/\d{2}/\d{2}gamma$").unwrap();
        assert!(date.is_match(&harness.text), "unexpected text: {:?}", harness.text);
        assert_eq!(
            harness.state.large.tool_cursor().map(|cursor| cursor.primary.index),
            Some(16),
            "the caret must follow the inserted stamp"
        );
    }

    #[test]
    fn large_editor_body_menu_jumps_to_the_following_occurrence() {
        let t = crate::i18n::tr(crate::Lang::En);
        let source = "alpha beta alpha gamma";
        let mut harness = Harness::new(source);
        crate::setup_fonts(&harness.ctx);
        harness.state.large.active = true;
        harness.run(vec![]);
        harness.state.large.set_tool_cursor(egui::text::CCursorRange::two(
            egui::text::CCursor::new(0),
            egui::text::CCursor::new(5),
        ));
        harness.run(vec![]);
        let body = text_pos(harness.run(vec![]), source).expect("large editor body");
        let menu = harness.open_body(body);
        harness.click_menu(menu, t.jump_next);
        harness.run(vec![]);
        assert_eq!(
            harness.state.large.tool_cursor(),
            Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(11),
                egui::text::CCursor::new(16),
            ))
        );
    }

    #[test]
    fn large_editor_jump_next_uses_the_word_under_the_caret_without_a_selection() {
        let t = crate::i18n::tr(crate::Lang::En);
        let source = "alpha beta alpha gamma";
        let mut harness = Harness::new(source);
        crate::setup_fonts(&harness.ctx);
        harness.state.large.active = true;
        harness.run(vec![]);
        harness.state.large.set_tool_cursor(egui::text::CCursorRange::two(
            egui::text::CCursor::new(11),
            egui::text::CCursor::new(11),
        ));
        harness.run(vec![]);
        let body = text_pos(harness.run(vec![]), source).expect("large editor body");
        let menu = harness.open_body(body);
        harness.click_menu(menu, t.jump_next);
        harness.run(vec![]);
        assert_eq!(
            harness.state.large.tool_cursor(),
            Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(5),
            )),
            "a collapsed caret must still jump to the next matching word"
        );
    }

    #[test]
    fn large_editor_body_menu_copies_and_cuts_the_selection() {
        let t = crate::i18n::tr(crate::Lang::En);
        let mut harness = Harness::new("alpha\nbeta");
        crate::setup_fonts(&harness.ctx);
        harness.state.large.active = true;
        harness.run(vec![]);
        let body = {
            let output = harness.run(vec![]);
            text_pos(output, "alpha\nbeta").expect("large editor body must be rendered")
        };
        for pressed in [true, false] {
            harness.button(body, egui::PointerButton::Secondary, pressed);
        }
        let menu = harness.run(vec![]);
        let labels: Vec<_> = menu
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.text()),
                _ => None,
            })
            .collect();
        for label in [t.copy_line, t.cut, t.paste, t.select_all] {
            assert!(labels.contains(&label), "missing {label:?}: {labels:?}");
        }
        assert!(!labels.contains(&t.clear_content));

        let menu = harness.open_body(body);
        let result = harness.click_menu(menu, t.paste);
        assert!(
            result
                .viewport_output
                .values()
                .any(|viewport| viewport
                    .commands
                    .contains(&egui::ViewportCommand::RequestPaste)),
            "paste must ask the platform for clipboard contents"
        );

        let menu = harness.open_body(body);
        harness.click_menu(menu, t.select_all);
        assert_eq!(
            harness.state.large.tool_cursor().unwrap(),
            egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(harness.text.chars().count()),
            )
        );

        let menu = harness.open_body(body);
        let result = harness.click_menu(menu, t.cut);
        assert!(result.platform_output.commands.iter().any(
            |command| matches!(command, egui::OutputCommand::CopyText(text) if text == "alpha\nbeta")
        ));
        harness.run(vec![]);
        assert_eq!(harness.text, "");
    }

    #[test]
    fn gutter_drag_selection_copies_multiple_lines_and_clears_without_deleting_rows() {
        let mut harness = Harness::new("alpha\nbeta\ngamma\ndelta");
        harness.select_rows(1, 2);
        assert_eq!(harness.state.selection, Some((1, 2)));
        let menu = harness.open_menu(1);
        let result = harness.click_menu(menu, "Copy Contents");
        assert!(result.platform_output.commands.iter().any(
            |cmd| matches!(cmd, egui::OutputCommand::CopyText(text) if text == "beta\ngamma")
        ));
        let menu = harness.open_menu(2);
        harness.click_menu(menu, "Clear Contents");
        assert_eq!(harness.text, "alpha\n\n\ndelta");
        let editor =
            egui::TextEdit::load_state(&harness.ctx, egui::Id::new("draft_editor").with(1u64))
                .unwrap();
        let current = (editor.cursor.char_range().unwrap(), harness.text.clone());
        assert_eq!(
            editor.undoer().undo(&current).unwrap().1,
            "alpha\nbeta\ngamma\ndelta"
        );
    }

    #[test]
    fn right_click_outside_selected_lines_targets_only_the_clicked_line() {
        let mut harness = Harness::new("alpha\nbeta\ngamma\ndelta");
        harness.select_rows(0, 1);
        harness.open_menu(3);
        assert_eq!(harness.state.selection, Some((3, 3)));
    }

    #[test]
    fn gutter_numbers_follow_vertical_scroll_but_stay_fixed_horizontally() {
        let content = (0..100)
            .map(|i| format!("line {i} {}", "x".repeat(200)))
            .collect::<Vec<_>>()
            .join("\n");
        let mut harness = Harness::new(&content);
        let before = harness.row_pos(10);
        harness.run(vec![
            egui::Event::PointerMoved(egui::pos2(200.0, 100.0)),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(-100.0, -70.0),
                modifiers: egui::Modifiers::default(),
            },
        ]);
        for _ in 0..40 {
            harness.run(vec![]);
        }
        let after = harness.row_pos(10);
        assert!((before.x - after.x).abs() < 0.001);
        assert!(after.y < before.y);
        // Hit-testing must use the scrolled row coordinates, not an unscrolled line-height estimate.
        harness.button(after, egui::PointerButton::Primary, true);
        harness.button(after, egui::PointerButton::Primary, false);
        assert_eq!(harness.state.selection, Some((10, 10)));
    }

    #[test]
    fn active_pen_marks_multiple_mouse_selections_and_deleted_text_loses_its_color() {
        for large in [false, true] {
            let content = format!(
                "abcdefghij{}",
                "\nshort".repeat(if large { 50_000 } else { 0 })
            );
            let mut harness = Harness::new(&content);
            harness.state.tools.pen = Some(0);
            let output = harness.run(vec![]);
            let (pos, glyphs) = output
                .shapes
                .into_iter()
                .find_map(|s| {
                    if let egui::Shape::Text(t) = s.shape
                        && t.galley.text().starts_with("abcdefghij")
                    {
                        Some((t.pos, t.galley.rows[0].glyphs.clone()))
                    } else {
                        None
                    }
                })
                .unwrap();
            let position = |index: usize| pos + egui::vec2(glyphs[index].pos.x, 6.0);
            for (start, end) in [(0, 3), (5, 8)] {
                harness.button(position(start), egui::PointerButton::Primary, true);
                harness.run(vec![egui::Event::PointerMoved(position(end))]);
                harness.button(position(end), egui::PointerButton::Primary, false);
            }
            assert_eq!(
                harness.state.tools.marks,
                [
                    crate::text_tools::Mark {
                        range: 0..3,
                        color: 0
                    },
                    crate::text_tools::Mark {
                        range: 5..8,
                        color: 0
                    }
                ]
            );
            assert_eq!(harness.text, content);
            // The second marked selection is still selected: delete it, leaving the first.
            harness.run(vec![egui::Event::Key {
                key: egui::Key::Backspace,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }]);
            assert_eq!(
                harness.state.tools.marks,
                [crate::text_tools::Mark {
                    range: 0..3,
                    color: 0
                }]
            );
        }
    }

    #[test]
    fn line_start_shortcuts_reveal_text_beyond_frozen_gutter_in_both_editors() {
        let mac = egui::Modifiers {
            mac_cmd: true,
            command: true,
            ..Default::default()
        };
        for large in [false, true] {
            for (os, start_key, end_key, modifiers) in [
                (
                    egui::os::OperatingSystem::Windows,
                    egui::Key::Home,
                    egui::Key::End,
                    egui::Modifiers::NONE,
                ),
                (
                    egui::os::OperatingSystem::Mac,
                    egui::Key::ArrowLeft,
                    egui::Key::ArrowRight,
                    mac,
                ),
                (
                    egui::os::OperatingSystem::Mac,
                    egui::Key::A,
                    egui::Key::E,
                    egui::Modifiers::CTRL,
                ),
            ] {
                let line = "abcdefghij".repeat(150);
                let content = if large {
                    format!("{line}\n{}", "short line\n".repeat(30_000))
                } else {
                    line.clone()
                };
                let mut harness = Harness::new(&content);
                harness.ctx.set_os(os);
                let initial = harness.run(vec![]);
                let line_start = |output: egui::FullOutput| {
                    output
                        .shapes
                        .into_iter()
                        .find_map(|shape| {
                            if let egui::Shape::Text(text) = shape.shape
                                && text.galley.text().starts_with(&line)
                            {
                                Some((text.pos.x, shape.clip_rect.left()))
                            } else {
                                None
                            }
                        })
                        .unwrap()
                };
                let (initial_x, clip_left) = line_start(initial);
                let click = egui::pos2(initial_x + 3.0, 18.0);
                harness.button(click, egui::PointerButton::Primary, true);
                harness.button(click, egui::PointerButton::Primary, false);
                let key = |key| egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                };
                harness.run(vec![key(end_key)]);
                for _ in 0..40 {
                    harness.run(vec![]);
                }
                assert_eq!(harness.state.cursor.unwrap().primary.index, line.len());
                let (scrolled_x, _) = line_start(harness.run(vec![]));
                assert!(scrolled_x < initial_x - 100.0);
                harness.run(vec![key(start_key)]);
                for _ in 0..40 {
                    harness.run(vec![]);
                }
                assert_eq!(harness.state.cursor.unwrap().primary.index, 0);
                let (revealed_x, _) = line_start(harness.run(vec![]));
                assert!(
                    revealed_x >= clip_left,
                    "line start {revealed_x} hidden behind gutter {clip_left}, initial={initial_x}, large={large}, os={os:?}"
                );
                assert!((revealed_x - initial_x).abs() < 0.5);
                assert_eq!(harness.text, content);

                // No cursor move: deliberate wheel scrolling must not snap back.
                harness.run(vec![
                    egui::Event::PointerMoved(egui::pos2(200.0, 100.0)),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(-200.0, 0.0),
                        modifiers: egui::Modifiers::NONE,
                    },
                ]);
                for _ in 0..40 {
                    harness.run(vec![]);
                }
                assert!(line_start(harness.run(vec![])).0 < initial_x - 150.0);
            }
        }
    }

    #[test]
    fn long_line_scrolls_horizontally_without_moving_gutter() {
        let content = "abcdefghij".repeat(150);
        let mut harness = Harness::new(&content);
        let before_number = harness.row_pos(0);
        let before = text_pos(harness.run(vec![]), &content).unwrap();
        harness.run(vec![
            egui::Event::PointerMoved(egui::pos2(200.0, 100.0)),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(-200.0, 0.0),
                modifiers: egui::Modifiers::default(),
            },
        ]);
        for _ in 0..40 {
            harness.run(vec![]);
        }
        let after = text_pos(harness.run(vec![]), &content).unwrap();
        assert!(after.x < before.x - 150.0);
        assert_eq!(harness.row_pos(0).x, before_number.x);
    }

    #[test]
    fn jump_next_scrolls_the_far_match_into_view() {
        let t = crate::i18n::tr(crate::Lang::En);
        let source = format!("needle\n{}\nend needle", "filler line\n".repeat(200));
        let mut harness = Harness::new(&source);
        crate::setup_fonts(&harness.ctx);
        let editor_id = egui::Id::new("draft_editor").with(1u64);
        let mut edit =
            egui::TextEdit::load_state(&harness.ctx, editor_id).unwrap_or_default();
        edit.cursor.set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(0),
            egui::text::CCursor::new(6),
        )));
        edit.store(&harness.ctx, editor_id);
        harness.state.cursor = Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(0),
            egui::text::CCursor::new(6),
        ));
        for _ in 0..3 {
            harness.run(vec![]);
        }
        let output = harness.run(vec![]);
        let (pos, galley) = output
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text().contains("needle") => {
                    Some((t.pos, t.galley.clone()))
                }
                _ => None,
            })
            .expect("body text");
        let pos = pos + galley.pos_from_ccursor(egui::text::CCursor::new(0)).center().to_vec2();
        let menu = harness.open_body(pos);
        harness.click_menu(menu, t.jump_next);
        for _ in 0..3 {
            harness.run(vec![]);
        }
        let expected = source[..source.rfind("needle").unwrap()].chars().count();
        assert_eq!(
            harness.state.cursor,
            Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(expected),
                egui::text::CCursor::new(expected + 6),
            ))
        );
        assert!(
            crate::editor::last_scroll_offset(&harness.ctx, 1).unwrap_or(0.0) > 0.0,
            "the jumped-to match must be scrolled into view"
        );
        assert!(
            harness.ctx.memory(|memory| memory.has_focus(editor_id)),
            "the editor must get focus back so the caret is visible at the match end"
        );
    }

    #[test]
    fn jump_next_uses_the_word_under_a_collapsed_right_click() {
        let t = crate::i18n::tr(crate::Lang::En);
        let source = "alpha beta alpha gamma";
        let mut harness = Harness::new(source);
        crate::setup_fonts(&harness.ctx);
        let editor_id = egui::Id::new("draft_editor").with(1u64);
        harness.run(vec![]);
        let output = harness.run(vec![]);
        let (pos, galley) = output
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text().contains(source) => {
                    Some((t.pos, t.galley.clone()))
                }
                _ => None,
            })
            .expect("body text");
        // No selection at all: right-click inside the second "alpha".
        let at = galley
            .pos_from_ccursor(egui::text::CCursor::new(11))
            .center();
        let menu = harness.open_body(pos + at.to_vec2());
        harness.click_menu(menu, t.jump_next);
        harness.run(vec![]);
        let state = egui::TextEdit::load_state(&harness.ctx, editor_id).unwrap();
        let range = state.cursor.char_range().expect("jump must select a match");
        assert_eq!(
            (
                range.primary.index.min(range.secondary.index),
                range.primary.index.max(range.secondary.index),
            ),
            (0, 5),
            "jumping from the second word must wrap to the first"
        );
        assert_eq!(
            range.primary.index, 5,
            "the caret must sit at the tail of the matched word"
        );
    }
}

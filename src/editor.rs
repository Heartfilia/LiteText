use crate::{Palette, i18n::Strings};
use eframe::egui;
use egui::{FontId, Stroke};
use std::ops::Range;

const GUTTER_NUMBER_GAP: f32 = 2.0;
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

fn word_range(text: &str, index: usize) -> Range<usize> {
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
        ui.scroll_to_rect(target, explicit.then_some(egui::Align::Center));
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
    pub(crate) focus_requested: bool,
    pub(crate) changes: crate::changes::Changes,
    pub(crate) folds: crate::json_plugin::Folds,
    pub(crate) syntax_cache: std::sync::Arc<std::sync::Mutex<crate::workspace_theme::SyntaxCache>>,
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

pub(crate) fn gutter_columns(line_count: usize) -> usize {
    let digits = line_count.max(1).ilog10() as usize + 1;
    digits.div_ceil(2) * 2
}

/// Leave at least as much space before the widest number as between the
/// number and the document, including the optional fold controls.
pub(crate) fn gutter_number_width(line_count: usize, digit_width: f32, fold_width: f32) -> f32 {
    let left_padding = GUTTER_NUMBER_GAP + EDITOR_LEFT_PADDING + fold_width;
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
    if text.len() >= crate::large_editor::THRESHOLD || state.large.active {
        return crate::large_editor::show(ui, text, state, file_id, pal, focus, t);
    }
    let bounds = ui.available_rect_before_wrap();
    let editor_id = egui::Id::new("draft_editor").with(file_id);
    state
        .folds
        .prepare_input(ui, state.pending_selection.is_some(), editor_id);
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
    let number_w = gutter_number_width(line_count, digit_w, fold_w);
    let gutter_w = number_w + fold_w;
    let gutter = egui::Rect::from_min_max(
        bounds.min,
        egui::pos2(bounds.left() + gutter_w, bounds.bottom()),
    );
    ui.painter()
        .rect_filled(gutter, 0.0, gutter_bg);
    let mut changed = false;
    egui::ScrollArea::both()
        .id_salt(("draft_scroll", file_id))
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
        .scroll_bar_rect(egui::Rect::from_min_max(
            egui::pos2(gutter.right() + 1.0, bounds.top()),
            bounds.max,
        ))
        .auto_shrink([false, false])
        .show_viewport(ui, |ui, _| {
            let origin = ui.cursor().min;
            let viewport_clip = ui.clip_rect();
            let editor_rect = egui::Rect::from_min_size(
                origin + egui::vec2(gutter_w + EDITOR_LEFT_PADDING, 12.0),
                egui::vec2(
                    (bounds.width() - gutter_w - EDITOR_LEFT_PADDING - 12.0).max(24.0),
                    (bounds.height() - 24.0).max(24.0),
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
                folds.layout(
                    ui.painter().layout_job(crate::text_tools::layout(
                        text,
                        font.clone(),
                        pal,
                        if tools.wrap { width } else { f32::INFINITY },
                        &current_tools,
                    )),
                    0,
                )
            };
            let reveal = state.pending_selection.is_some();
            if let Some(range) = state.pending_selection.take() {
                let mut edit = egui::TextEdit::load_state(ui.ctx(), editor_id).unwrap_or_default();
                edit.cursor
                    .set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(range.start),
                        egui::text::CCursor::new(range.end),
                    )));
                edit.store(ui.ctx(), editor_id);
            }
            let previous = egui::TextEdit::load_state(ui.ctx(), editor_id)
                .and_then(|edit| edit.cursor.char_range());
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
            if let Some(render) = &tools.workspace {
                render.paint_selected_text(ui, &output);
            }
            ui.set_min_width(gutter_w + EDITOR_LEFT_PADDING + output.galley.size().x + 12.0);
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
            reveal_cursor(ui, &output, previous, gutter_w, reveal);
            if tools.whitespace {
                crate::text_tools::paint_whitespace(ui, &output.galley, output.galley_pos, pal);
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
            if response.secondary_clicked()
                && let Some(line) = pointer_line
                && !state
                    .selection
                    .is_some_and(|(start, end)| start <= line && line <= end)
            {
                state.selection = Some((line, line));
                state.anchor = Some(line);
            }

            let painter = ui.painter().with_clip_rect(gutter_clip);
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
                            row_rect.center().y,
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
                for block in &folds.blocks {
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

            response.context_menu(|menu| {
                let selection = state.selection.unwrap_or((0, 0));
                let label = if selection.0 == selection.1 {
                    t.copy_line
                } else {
                    t.copy_lines
                };
                line_menu_style(menu, label, t.clear_content);
                if line_menu_button(menu, label).clicked() {
                    menu.ctx().copy_text(copy_lines(text, selection));
                    menu.close_menu();
                }
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
            });
        });
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn gutter_width_rounds_line_digits_up_to_even_columns() {
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
            (10000, 6),
            (999999, 6),
            (1000000, 8),
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
    fn gutter_left_padding_balances_right_gap_including_fold_lane() {
        for count in [1usize, 10, 100, 1000, 10000] {
            for digit_width in [5.0, 8.0, 19.0] {
                for folds in [0.0, 16.0] {
                    let width = gutter_number_width(count, digit_width, folds);
                    let reserved_digits = gutter_columns(count) as f32 * digit_width;
                    let left = width - GUTTER_NUMBER_GAP - reserved_digits;
                    let right = GUTTER_NUMBER_GAP + folds + EDITOR_LEFT_PADDING;
                    assert!((left - right).abs() < 0.01);
                }
            }
        }
    }

    #[test]
    fn gutter_border_has_two_point_number_gap_and_compact_text_padding() {
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
        assert!((border - number_right.unwrap() - 2.0).abs() < 0.01);
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
                let clear = text(t.clear_content);
                assert_eq!(copy.galley.rows.len(), 1);
                assert_eq!(clear.galley.rows.len(), 1);
                let gap = clear.pos.y - (copy.pos.y + copy.galley.size().y);
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
}

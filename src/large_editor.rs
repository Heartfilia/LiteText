//! Viewport-windowed editing: only nearby lines are laid out by TextEdit.
use crate::{Palette, editor, i18n::Strings};
use eframe::egui;
use egui::{FontId, Stroke};
use std::ops::Range;

pub(crate) const THRESHOLD: usize = 256 * 1024;

#[derive(Clone, Default)]
pub(crate) struct State {
    pub(crate) active: bool,
    pub(crate) indexed_len: Option<usize>,
    ranges: Vec<Range<usize>>,
    max_columns: usize,
    window: String,
    window_lines: Range<usize>,
    window_bytes: Range<usize>,
    char_start: usize,
    cursor: Option<egui::text::CCursorRange>,
    char_starts: Vec<usize>,
    widths: Vec<usize>,
    pub(crate) characters: usize,
    pub(crate) utf16_units: usize,
    metrics_encoding: Option<crate::recovery::Encoding>,
    pub(crate) legacy_bytes: usize,
    pub(crate) legacy_invalid: usize,
    pub(crate) revision: u64,
    next_revision: u64,
    undo: Vec<Edit>,
    redo: Vec<Edit>,
    wrap_key: Option<(u32, usize)>,
    visual_starts: Vec<usize>,
    visual_rows: Vec<usize>,
    visual_width: Option<u32>,
    fold_generation: Option<u64>,
    visible_lines: Vec<usize>,
    fingerprints: Vec<u64>,
    tool_edits: Vec<(Range<usize>, usize)>,
    #[cfg(test)]
    pub(crate) laid_out_bytes: usize,
}

const HISTORY_BYTES: usize = 16 * 1024 * 1024;
// A small navigation cushion, not hundreds of potentially long JSONL rows.
const OVERSCAN_LINES: usize = 12;

#[derive(Clone)]
struct Edit {
    start: usize,
    removed: String,
    inserted: String,
    before: u64,
    after: u64,
    cursor_before: Option<egui::text::CCursorRange>,
    cursor_after: Option<egui::text::CCursorRange>,
}

fn columns(text: &str) -> usize {
    text.chars()
        .map(|c| {
            if c == '\t' {
                4
            } else if c.is_ascii() {
                1
            } else {
                2
            }
        })
        .sum()
}

/// The smallest UTF-8-boundary replacement between two viewport buffers.
pub(crate) fn difference(old: &str, new: &str) -> (Range<usize>, Range<usize>) {
    let mut prefix = old
        .bytes()
        .zip(new.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(prefix) || !new.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let mut suffix = old[prefix..]
        .bytes()
        .rev()
        .zip(new[prefix..].bytes().rev())
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(old.len() - suffix) || !new.is_char_boundary(new.len() - suffix) {
        suffix -= 1;
    }
    (prefix..old.len() - suffix, prefix..new.len() - suffix)
}

impl State {
    pub(crate) fn take_tool_edits(&mut self) -> Vec<(Range<usize>, usize)> {
        std::mem::take(&mut self.tool_edits)
    }
    pub(crate) fn external_replace(
        &mut self,
        text: &mut String,
        range: Range<usize>,
        inserted: String,
    ) {
        self.index(text);
        let start = text[..range.start].chars().count();
        let after = Some(egui::text::CCursorRange::one(egui::text::CCursor::new(
            start + inserted.chars().count(),
        )));
        self.record(text, range, inserted, self.cursor, after);
        self.cursor = after;
    }
    pub(crate) fn invalidate_window(&mut self) {
        self.window_lines = 0..0;
    }
    pub(crate) fn set_tool_cursor(&mut self, cursor: egui::text::CCursorRange) {
        self.cursor = Some(cursor);
        if let Some(edit) = self.undo.last_mut() {
            edit.cursor_after = Some(cursor);
        }
    }
    fn index(&mut self, text: &str) {
        if self.indexed_len == Some(text.len()) {
            return;
        }
        self.ranges = editor::line_ranges(text);
        self.fingerprints = crate::changes::lines(text);
        self.visual_rows.clear();
        self.char_starts.clear();
        let mut chars = 0;
        for (i, range) in self.ranges.iter().enumerate() {
            self.char_starts.push(chars);
            let end = self.ranges.get(i + 1).map_or(text.len(), |r| r.start);
            chars += text[range.start..end].chars().count();
        }
        self.characters = chars;
        self.utf16_units = text.chars().map(char::len_utf16).sum();
        self.widths = self
            .ranges
            .iter()
            .map(|r| columns(&text[r.clone()]))
            .collect();
        self.max_columns = self.widths.iter().copied().max().unwrap_or(0);
        self.indexed_len = Some(text.len());
        self.wrap_key = None;
    }

    fn line_for_char(&self, character: usize) -> usize {
        self.char_starts
            .partition_point(|&start| start <= character)
            .saturating_sub(1)
    }

    /// Rescan touched logical lines only; shift the compact suffix metadata, not its text.
    fn replace(&mut self, text: &mut String, range: Range<usize>, replacement: &str) {
        self.tool_edits.push((range.clone(), replacement.len()));
        self.index(text);
        let first = self
            .ranges
            .partition_point(|r| r.start <= range.start)
            .saturating_sub(1);
        let last = self
            .ranges
            .partition_point(|r| r.start <= range.end)
            .saturating_sub(1);
        let start = self.ranges[first].start;
        let old_end = self.ranges.get(last + 1).map_or(text.len(), |r| r.start);
        let has_suffix = last + 1 < self.ranges.len();
        let byte_delta = replacement.len() as isize - range.len() as isize;
        let char_delta =
            replacement.chars().count() as isize - text[range.clone()].chars().count() as isize;
        let units_delta = replacement.chars().map(char::len_utf16).sum::<usize>() as isize
            - text[range.clone()]
                .chars()
                .map(char::len_utf16)
                .sum::<usize>() as isize;
        if let Some(encoding) = self.metrics_encoding {
            let (added, added_invalid) = encoding.legacy_metrics(replacement);
            let (removed, removed_invalid) = encoding.legacy_metrics(&text[range.clone()]);
            self.legacy_bytes = self.legacy_bytes + added - removed;
            self.legacy_invalid = self.legacy_invalid + added_invalid - removed_invalid;
        }
        text.replace_range(range, replacement);
        let end = old_end.saturating_add_signed(byte_delta);
        let mut ranges = editor::line_ranges(&text[start..end]);
        if has_suffix {
            ranges.pop();
        }
        let mut chars = self.char_starts[first];
        let mut starts = Vec::with_capacity(ranges.len());
        let mut widths = Vec::with_capacity(ranges.len());
        let mut fingerprints = Vec::with_capacity(ranges.len());
        for i in 0..ranges.len() {
            let next = ranges.get(i + 1).map_or(end - start, |r| r.start);
            let hash_end = if next > ranges[i].end && text.as_bytes()[start + next - 1] == b'\n' {
                next - 1
            } else {
                next
            };
            fingerprints.push(crate::changes::fingerprint(
                &text[start + ranges[i].start..start + hash_end],
            ));
            starts.push(chars);
            chars += text[start + ranges[i].start..start + next].chars().count();
            widths.push(columns(
                &text[start + ranges[i].start..start + ranges[i].end],
            ));
        }
        for r in &mut ranges {
            r.start += start;
            r.end += start;
        }
        for r in &mut self.ranges[last + 1..] {
            r.start = r.start.saturating_add_signed(byte_delta);
            r.end = r.end.saturating_add_signed(byte_delta);
        }
        for c in &mut self.char_starts[last + 1..] {
            *c = c.saturating_add_signed(char_delta);
        }
        let new_lines = ranges.len();
        self.ranges.splice(first..=last, ranges);
        self.char_starts.splice(first..=last, starts);
        self.widths.splice(first..=last, widths);
        self.fingerprints.splice(first..=last, fingerprints);
        if self.visual_rows.len() > last {
            self.visual_rows
                .splice(first..=last, std::iter::repeat_n(0, new_lines));
        }
        self.max_columns = self.widths.iter().copied().max().unwrap_or(0);
        self.characters = self.characters.saturating_add_signed(char_delta);
        self.utf16_units = self.utf16_units.saturating_add_signed(units_delta);
        self.indexed_len = Some(text.len());
        self.wrap_key = None;
        self.invalidate_window();
    }

    fn record(
        &mut self,
        text: &mut String,
        range: Range<usize>,
        inserted: String,
        before: Option<egui::text::CCursorRange>,
        after: Option<egui::text::CCursorRange>,
    ) {
        self.next_revision += 1;
        let edit = Edit {
            start: range.start,
            removed: text[range.clone()].to_owned(),
            inserted,
            before: self.revision,
            after: self.next_revision,
            cursor_before: before,
            cursor_after: after,
        };
        self.replace(text, range, &edit.inserted);
        self.revision = edit.after;
        self.redo.clear();
        self.undo.push(edit);
        let mut size: usize = self
            .undo
            .iter()
            .map(|e| e.removed.len() + e.inserted.len())
            .sum();
        while self.undo.len() > 1 && (size > HISTORY_BYTES || self.undo.len() > 1000) {
            let removed = self.undo.remove(0);
            size -= removed.removed.len() + removed.inserted.len();
        }
    }

    fn undo(&mut self, text: &mut String, redo: bool) -> bool {
        let Some(edit) = (if redo { &mut self.redo } else { &mut self.undo }).pop() else {
            return false;
        };
        if redo {
            self.replace(
                text,
                edit.start..edit.start + edit.removed.len(),
                &edit.inserted,
            );
            self.revision = edit.after;
            self.cursor = edit.cursor_after;
            self.undo.push(edit);
        } else {
            self.replace(
                text,
                edit.start..edit.start + edit.inserted.len(),
                &edit.removed,
            );
            self.revision = edit.before;
            self.cursor = edit.cursor_before;
            self.redo.push(edit);
        }
        true
    }

    pub(crate) fn line_count(&self) -> usize {
        self.ranges.len()
    }

    pub(crate) fn set_encoding(&mut self, text: &str, encoding: crate::recovery::Encoding) {
        let encoding = encoding.is_legacy().then_some(encoding);
        if self.metrics_encoding != encoding {
            self.metrics_encoding = encoding;
            (self.legacy_bytes, self.legacy_invalid) =
                encoding.map_or((0, 0), |e| e.legacy_metrics(text));
        }
    }

    fn clear_lines(&mut self, text: &mut String, selection: (usize, usize)) {
        let range = self.ranges[selection.0].start..self.ranges[selection.1].end;
        let mut cleared = String::new();
        for line in selection.0..selection.1 {
            cleared.push_str(&text[self.ranges[line].end..self.ranges[line + 1].start]);
        }
        let cursor = Some(egui::text::CCursorRange::one(egui::text::CCursor::new(
            self.char_starts[selection.0],
        )));
        self.record(text, range, cleared, self.cursor, cursor);
        self.cursor = cursor;
    }

    fn visual_index(
        &mut self,
        ui: &egui::Ui,
        text: &str,
        font: &FontId,
        pal: &Palette,
        width: f32,
        wrap: bool,
    ) {
        let key = (if wrap { width.to_bits() } else { 0 }, self.ranges.len());
        if self.wrap_key == Some(key) {
            return;
        }
        // A width change invalidates every layout; edits invalidate touched lines only.
        if self.visual_width != Some(key.0) || self.visual_rows.len() != self.ranges.len() {
            self.visual_rows = vec![0; self.ranges.len()];
        }
        self.visual_width = Some(key.0);
        let mut rows = 0;
        self.visual_starts.clear();
        for (i, range) in self.ranges.iter().enumerate() {
            self.visual_starts.push(rows);
            if self.visual_rows[i] == 0 {
                self.visual_rows[i] = if wrap {
                    let mut job = egui::text::LayoutJob::simple(
                        text[range.clone()].to_owned(),
                        font.clone(),
                        pal.text_main,
                        width,
                    );
                    job.wrap.break_anywhere = true;
                    ui.painter().layout_job(job).rows.len().max(1)
                } else {
                    1
                };
            }
            rows += self.visual_rows[i];
        }
        self.visual_starts.push(rows);
        self.fold_generation = None;
        self.wrap_key = Some(key);
    }

    fn fold_index(&mut self, folds: &crate::json_plugin::Folds) {
        if self.fold_generation == Some(folds.generation) {
            return;
        }
        let mut rows = 0;
        self.visual_starts.clear();
        self.visible_lines.clear();
        for (line, height) in self.visual_rows.iter().enumerate() {
            self.visual_starts.push(rows);
            if !folds.hidden(line) {
                self.visible_lines.push(line);
                rows += height;
            }
        }
        self.visual_starts.push(rows);
        self.fold_generation = Some(folds.generation);
    }
}

fn shift_cursor(cursor: egui::text::CCursorRange, delta: isize) -> egui::text::CCursorRange {
    let shift = |mut c: egui::text::CCursor| {
        c.index = c.index.saturating_add_signed(delta);
        c
    };
    egui::text::CCursorRange::two(shift(cursor.secondary), shift(cursor.primary))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn show(
    ui: &mut egui::Ui,
    text: &mut String,
    state: &mut editor::EditorState,
    file_id: u64,
    pal: &Palette,
    focus: bool,
    t: &Strings,
) -> bool {
    let bounds = ui.available_rect_before_wrap();
    state.large.active = true;
    let id = egui::Id::new("large_editor").with(file_id);
    state
        .folds
        .prepare_input(ui, state.pending_selection.is_some(), id);
    let folds = state.folds.clone();
    let fold_w = folds.width();
    ui.style_mut().spacing.scroll = egui::style::ScrollStyle::solid();
    let font = FontId::monospace(13.5);
    let mut tools = state.tools.clone();
    if let Some(render) = &tools.workspace {
        render.apply_style(ui);
    }
    let colors = tools.workspace.as_ref().map(|r| r.colors);
    let row_height = ui.fonts(|f| f.row_height(&font));
    let probe = ui
        .painter()
        .layout_no_wrap("0000000000".to_owned(), font.clone(), pal.text_main);
    let digit_w = probe.rows[0].glyphs[1].pos.x - probe.rows[0].glyphs[0].pos.x;
    state.large.index(text);
    state
        .changes
        .update(&state.large.fingerprints, state.large.revision);
    let count = state.large.ranges.len();
    let wrap_width =
        (bounds.width() - editor::gutter_columns(count) as f32 * digit_w - fold_w - 40.0).max(24.0);
    state
        .large
        .visual_index(ui, text, &font, pal, wrap_width, tools.wrap);
    state.large.fold_index(&folds);
    if let Some((start, end)) = state.selection {
        state.selection = Some((start.min(count - 1), end.min(count - 1)));
    }
    let number_w = editor::gutter_columns(count) as f32 * digit_w + 2.0;
    let gutter_w = number_w + fold_w;
    let gutter = egui::Rect::from_min_max(
        bounds.min,
        egui::pos2(bounds.left() + gutter_w, bounds.bottom()),
    );
    ui.painter()
        .rect_filled(gutter, 0.0, colors.map_or(pal.bg_panel, |c| c.background));
    let id = egui::Id::new("large_editor").with(file_id);
    let mut changed = false;
    // Document-level undo remains valid even after the visible text window moves.
    if ui.is_enabled() && ui.memory(|m| m.has_focus(id)) {
        let (undo, redo) = ui.input_mut(|i| {
            let redo = i.consume_key(
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                egui::Key::Z,
            ) || i.consume_key(egui::Modifiers::COMMAND, egui::Key::Y);
            let undo = i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z);
            (undo, redo)
        });
        if undo || redo {
            changed = state.large.undo(text, redo);
        }
    }
    if changed {
        for (range, inserted) in state.large.take_tool_edits() {
            state.tools.apply_edit(range, inserted);
        }
        tools = state.tools.clone();
        state
            .large
            .visual_index(ui, text, &font, pal, wrap_width, tools.wrap);
        state.large.fold_index(&folds);
    }
    let reveal = state.pending_selection.is_some();
    if let Some(range) = state.pending_selection.take() {
        state.large.cursor = Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(range.start),
            egui::text::CCursor::new(range.end),
        ));
        state.large.invalidate_window();
        let line = state.large.line_for_char(range.start);
        let scroll_id = ui.make_persistent_id(("draft_scroll", file_id));
        let mut scroll = egui::scroll_area::State::load(ui.ctx(), scroll_id).unwrap_or_default();
        scroll.offset.y =
            state.large.visual_starts.get(line).copied().unwrap_or(line) as f32 * row_height;
        scroll.store(ui.ctx(), scroll_id);
    }
    let count = state.large.ranges.len();
    let mut line_action = None;
    if let Some((start, end)) = state.selection {
        state.selection = Some((start.min(count - 1), end.min(count - 1)));
    }
    egui::ScrollArea::both()
        .id_salt(("draft_scroll", file_id))
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
        .scroll_bar_rect(egui::Rect::from_min_max(
            egui::pos2(gutter.right() + 1.0, bounds.top()),
            bounds.max,
        ))
        .auto_shrink([false, false])
        .show_viewport(ui, |ui, viewport| {
            let origin = ui.cursor().min;
            let at_row = |row: usize| {
                let at = state
                    .large
                    .visible_lines
                    .partition_point(|&line| state.large.visual_starts[line] <= row)
                    .saturating_sub(1);
                state.large.visible_lines.get(at).copied().unwrap_or(0)
            };
            let first = at_row(((viewport.top() - 12.0).max(0.0) / row_height) as usize);
            let visible_end =
                (at_row((viewport.bottom() / row_height).ceil() as usize + 2) + 1).min(count);
            let desired =
                first.saturating_sub(OVERSCAN_LINES)..(visible_end + OVERSCAN_LINES).min(count);
            let keyboard_input = ui.input(|i| {
                i.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::Text(_)
                            | egui::Event::Paste(_)
                            | egui::Event::Copy
                            | egui::Event::Cut
                            | egui::Event::Key { pressed: true, .. }
                    )
                })
            });
            let pointer_down = ui.input(|i| i.pointer.primary_down());
            let focused = ui.is_enabled() && ui.memory(|m| m.has_focus(id));
            let select_all = focused
                && ui.input(|i| {
                    i.events.iter().any(|event| {
                        matches!(event,
                            egui::Event::Key { key: egui::Key::A, pressed: true, modifiers, .. }
                                if modifiers.matches_exact(egui::Modifiers::COMMAND)
                        )
                    })
                });
            let mut window = desired;
            let cached = &state.large.window_lines;
            if cached.start <= first.saturating_sub(2)
                && cached.end >= (visible_end + 2).min(count)
                && cached.len() <= window.len() + 2 * OVERSCAN_LINES
            {
                // Reuse the same text/galley for small scroll steps until the
                // viewport nears its edge, instead of re-highlighting each row.
                window = cached.clone();
            }
            if select_all {
                window = 0..count;
            }
            // Keep a focused off-screen cursor valid when typing after scrolling away.
            if focused
                && (keyboard_input || pointer_down)
                && let Some(cursor) = state.large.cursor
            {
                let line = state.large.line_for_char(cursor.primary.index);
                if keyboard_input && (line < window.start || line >= window.end) {
                    window =
                        line.saturating_sub(OVERSCAN_LINES)..(line + OVERSCAN_LINES + 1).min(count);
                } else if pointer_down {
                    window.start = window.start.min(line);
                    window.end = window.end.max((line + 1).min(count));
                }
            }
            if focused
                && let Some(cursor) = state.large.cursor
                && cursor.primary.index != cursor.secondary.index
            {
                // Preserve explicit selections across viewport changes, including Select All + Copy.
                for cursor in [cursor.primary, cursor.secondary] {
                    let line = state.large.line_for_char(cursor.index);
                    window.start = window.start.min(line);
                    window.end = window.end.max((line + 1).min(count));
                }
            }
            let start = state.large.ranges[window.start].start;
            // Exclude the boundary newline; edits replace exactly this byte range.
            let end = state.large.ranges[window.end - 1].end;
            let bytes = start..end;
            if state.large.window_lines != window || state.large.window_bytes != bytes || changed {
                state.large.window = text[bytes.clone()].to_owned();
                state.large.window_lines = window.clone();
                state.large.window_bytes = bytes.clone();
                let offset = state.large.char_starts[window.start];
                state.large.char_start = offset;
                let mut edit_state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
                if let Some(cursor) = state.large.cursor {
                    edit_state
                        .cursor
                        .set_char_range(Some(shift_cursor(cursor, -(offset as isize))));
                }
                edit_state.clear_undoer();
                edit_state.store(ui.ctx(), id);
            }
            let char_start = state.large.char_start;
            let content_w = if tools.wrap {
                wrap_width + gutter_w + 16.0
            } else {
                (state.large.max_columns as f32 * digit_w + gutter_w + 20.0)
                    .max(bounds.width() - 16.0)
            };
            let rect = egui::Rect::from_min_size(
                origin
                    + egui::vec2(
                        gutter_w + 4.0,
                        12.0 + state
                            .large
                            .visual_starts
                            .get(window.start)
                            .copied()
                            .unwrap_or(window.start) as f32
                            * row_height,
                    ),
                egui::vec2((content_w - gutter_w - 16.0).max(24.0), row_height),
            );
            let before_layout = (!tools.marks.is_empty() || tools.current.is_some())
                .then(|| state.large.window.clone());
            if let Some(render) = &tools.workspace {
                render.prepare_prefix(text, bytes.start);
            }
            let mut layouter = |ui: &egui::Ui, text: &str, _: f32| {
                let mut current_tools = tools.clone();
                if let Some(before) = &before_layout
                    && before != text
                {
                    let (removed, inserted) = difference(before, text);
                    current_tools.apply_edit(
                        bytes.start + removed.start..bytes.start + removed.end,
                        inserted.len(),
                    );
                }
                let mut job = crate::text_tools::layout_window(
                    text,
                    font.clone(),
                    pal,
                    if tools.wrap {
                        wrap_width
                    } else {
                        f32::INFINITY
                    },
                    &current_tools,
                    bytes.start,
                );
                for section in &mut job.sections {
                    section.format.line_height = Some(row_height);
                }
                folds.layout(ui.painter().layout_job(job), window.start)
            };
            let cursor_before = state.large.cursor;
            let previous =
                egui::TextEdit::load_state(ui.ctx(), id).and_then(|edit| edit.cursor.char_range());
            let output = ui
                .allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| {
                    ui.set_clip_rect(ui.clip_rect().intersect(egui::Rect::from_min_max(
                        egui::pos2(gutter.right() + 1.0, bounds.top()),
                        bounds.max,
                    )));
                    egui::TextEdit::multiline(&mut state.large.window)
                        .id(id)
                        .font(font.clone())
                        .text_color(colors.map_or(pal.text_main, |c| c.text))
                        .frame(false)
                        .margin(egui::Margin::ZERO)
                        .desired_width(rect.width())
                        .desired_rows(1)
                        .layouter(&mut layouter)
                        .lock_focus(true)
                        .show(ui)
                })
                .inner;
            if let Some(render) = &tools.workspace {
                render.paint_selected_text(ui, &output);
            }
            if focus {
                output.response.request_focus();
            }
            editor::reveal_cursor(ui, &output, previous, gutter_w, reveal);
            if tools.whitespace {
                crate::text_tools::paint_whitespace(ui, &output.galley, output.galley_pos, pal);
            }
            folds.paint_summaries(ui, &output.galley, output.galley_pos, window.start, pal);
            if output.response.has_focus()
                && (output.response.clicked()
                    || output.response.changed()
                    || keyboard_input
                    || pointer_down)
                && let Some(cursor) = output.state.cursor.char_range()
            {
                state.large.cursor = Some(shift_cursor(cursor, char_start as isize));
            }
            #[cfg(test)]
            {
                state.large.laid_out_bytes = state.large.window.len();
            }
            if output.response.changed() {
                let (old, new) = difference(&text[bytes.clone()], &state.large.window);
                let inserted = state.large.window[new].to_owned();
                state.large.record(
                    text,
                    bytes.start + old.start..bytes.start + old.end,
                    inserted,
                    cursor_before,
                    state.large.cursor,
                );
                changed = true;
                state.selection = None;
            }
            if output.response.clicked() {
                state.selection = None;
            }
            state.cursor = state.large.cursor;
            state.tools.handle_marker(ui, &output, text, char_start);
            ui.set_min_size(egui::vec2(
                content_w,
                24.0 + state.large.visual_starts.last().copied().unwrap_or(count) as f32
                    * row_height,
            ));
            let number_gutter = egui::Rect::from_min_max(
                gutter.min,
                egui::pos2(gutter.left() + number_w, gutter.bottom()),
            );
            let gutter_clip = number_gutter.intersect(ui.clip_rect());
            let response = ui.interact(
                gutter_clip,
                id.with("gutter"),
                egui::Sense::click_and_drag(),
            );
            let line_at = |p: egui::Pos2| {
                let row = ((p.y - origin.y - 12.0).max(0.0) / row_height) as usize;
                let at = state
                    .large
                    .visible_lines
                    .partition_point(|&line| state.large.visual_starts[line] <= row)
                    .saturating_sub(1);
                state.large.visible_lines.get(at).copied().unwrap_or(0)
            };
            if (response.clicked() || response.drag_started())
                && let Some(p) = if response.drag_started() {
                    ui.input(|i| i.pointer.press_origin())
                } else {
                    response.interact_pointer_pos()
                }
            {
                let line = line_at(p);
                let anchor = if ui.input(|i| i.modifiers.shift) {
                    state.anchor.unwrap_or(line)
                } else {
                    line
                };
                state.anchor = Some(anchor);
                state.selection = Some((anchor.min(line), anchor.max(line)));
            }
            if response.dragged()
                && let (Some(anchor), Some(p)) = (state.anchor, response.interact_pointer_pos())
            {
                let line = line_at(p);
                state.selection = Some((anchor.min(line), anchor.max(line)));
            }
            if response.secondary_clicked()
                && let Some(p) = response.interact_pointer_pos()
            {
                let line = line_at(p);
                if !state.selection.is_some_and(|(a, b)| a <= line && line <= b) {
                    state.selection = Some((line, line));
                    state.anchor = Some(line);
                }
            }
            let painter = ui.painter().with_clip_rect(gutter_clip);
            for line in first..visible_end {
                if folds.hidden(line) {
                    continue;
                }
                let rect = egui::Rect::from_min_size(
                    egui::pos2(
                        gutter.left(),
                        origin.y
                            + 12.0
                            + state.large.visual_starts.get(line).copied().unwrap_or(line) as f32
                                * row_height,
                    ),
                    egui::vec2(number_w, row_height),
                );
                let selected = state.selection.is_some_and(|(a, b)| a <= line && line <= b);
                if selected {
                    painter.rect_filled(
                        rect,
                        0.0,
                        colors.map_or(pal.accent.gamma_multiply(0.25), |c| c.selection),
                    );
                }
                let modified = state.changes.is_marked(line);
                if modified && !selected {
                    painter.rect_filled(rect, 0.0, pal.accent.gamma_multiply(0.10));
                    painter.line_segment(
                        [rect.left_top(), rect.left_bottom()],
                        Stroke::new(2.0, pal.accent),
                    );
                }
                painter.text(
                    egui::pos2(number_gutter.right() - 2.0, rect.center().y),
                    egui::Align2::RIGHT_CENTER,
                    (line + 1).to_string(),
                    font.clone(),
                    if selected || modified {
                        pal.accent
                    } else {
                        colors.map_or(pal.text_faint, |c| c.gutter)
                    },
                );
            }
            if fold_w > 0.0 {
                let lane = egui::Rect::from_min_max(
                    egui::pos2(number_gutter.right(), gutter.top()),
                    gutter.max,
                );
                ui.painter()
                    .rect_filled(lane, 0.0, colors.map_or(pal.bg_panel, |c| c.background));
                for block in &folds.blocks {
                    if folds.hidden(block.start)
                        || block.start < first
                        || block.start >= visible_end
                    {
                        continue;
                    }
                    let rect = egui::Rect::from_min_size(
                        egui::pos2(
                            lane.left(),
                            origin.y
                                + 12.0
                                + state.large.visual_starts[block.start] as f32 * row_height,
                        ),
                        egui::vec2(fold_w, row_height),
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
                        let start = state.large.char_starts[block.start];
                        state.pending_selection = Some(start..start);
                        state.selection = None;
                        ui.ctx().request_repaint();
                    }
                }
            }
            response.context_menu(|ui| {
                let selection = state.selection.unwrap_or((0, 0));
                if ui
                    .button(if selection.0 == selection.1 {
                        t.copy_line
                    } else {
                        t.copy_lines
                    })
                    .clicked()
                {
                    let copied = state.large.ranges[selection.0..=selection.1]
                        .iter()
                        .map(|r| &text[r.clone()])
                        .collect::<Vec<_>>()
                        .join("\n");
                    ui.ctx().copy_text(copied);
                    ui.close_menu();
                }
                ui.separator();
                if ui.button(t.clear_content).clicked() {
                    line_action = Some(selection);
                    ui.close_menu();
                }
            });
        });
    if let Some(selection) = line_action {
        state.large.clear_lines(text, selection);
        changed = true;
    }
    for (range, inserted) in state.large.tool_edits.drain(..) {
        state.tools.apply_edit(range, inserted);
    }
    ui.painter().line_segment(
        [gutter.right_top(), gutter.right_bottom()],
        Stroke::new(1.0, pal.text_faint.gamma_multiply(0.55)),
    );
    state.selected_characters = if let Some((a, b)) = state.selection {
        let a = a.min(state.large.ranges.len() - 1);
        let b = b.min(state.large.ranges.len() - 1);
        (a..=b)
            .map(|i| {
                let next_byte = state
                    .large
                    .ranges
                    .get(i + 1)
                    .map_or(text.len(), |r| r.start);
                let next_char = state
                    .large
                    .char_starts
                    .get(i + 1)
                    .copied()
                    .unwrap_or(state.large.characters);
                next_char - state.large.char_starts[i] - (next_byte - state.large.ranges[i].end)
            })
            .sum::<usize>()
            + b
            - a
    } else {
        state
            .large
            .cursor
            .map_or(0, |c| c.primary.index.abs_diff(c.secondary.index))
    };
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonl_viewport_is_bounded_reused_for_small_scrolls_and_editable_at_depth() {
        let mut app = crate::LiteTextApp::default();
        app.new_document();
        let row = format!("{{\"payload\":\"{}\",\"count\":42}}\n", "x".repeat(1000));
        let original = row.repeat(2000);
        app.active_mut().unwrap().draft_text = Some(original.clone());
        app.active_mut().unwrap().language_override = Some("JSON".to_owned());
        let ctx = egui::Context::default();
        let draw = |app: &mut crate::LiteTextApp, events| {
            crate::workspace_theme::configure(app.active_mut().unwrap(), &Default::default(), true);
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1080.0, 720.0),
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
                            true,
                            crate::Lang::En,
                        );
                    });
                },
            );
        };
        draw(&mut app, vec![]);
        let initial = app
            .active()
            .unwrap()
            .editor_state
            .large
            .window_lines
            .clone();
        assert!(app.active().unwrap().editor_state.large.laid_out_bytes < 64 * 1024);
        draw(
            &mut app,
            vec![
                egui::Event::PointerMoved(egui::pos2(300.0, 300.0)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -18.0),
                    modifiers: Default::default(),
                },
            ],
        );
        assert_eq!(
            app.active().unwrap().editor_state.large.window_lines,
            initial
        );
        let at = row.chars().count() * 1800;
        app.active_mut().unwrap().editor_state.pending_selection = Some(at..at);
        draw(
            &mut app,
            vec![egui::Event::Key {
                key: egui::Key::ArrowRight,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        assert!(app.active().unwrap().editor_state.large.window_lines.start > 1700);
        assert!(app.active().unwrap().editor_state.large.laid_out_bytes < 90 * 1024);
        draw(&mut app, vec![egui::Event::Text(" ".to_owned())]);
        assert_ne!(
            app.active().unwrap().draft_text.as_deref(),
            Some(original.as_str())
        );
        draw(
            &mut app,
            vec![egui::Event::Key {
                key: egui::Key::Z,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            }],
        );
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some(original.as_str())
        );
    }

    #[test]
    fn large_wrap_offsets_and_selection_survive_edit_and_undo() {
        let original = format!("{}\r\n中文🙂\r\n", "abcdef".repeat(60)).repeat(1000);
        let mut text = original.clone();
        let mut state = editor::EditorState::default();
        state.tools.wrap = true;
        state.tools.whitespace = true;
        state.pending_selection = Some(362..365);
        let ctx = egui::Context::default();
        for frame in 0..3 {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(500.0, 600.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        editor::show(
                            ui,
                            &mut text,
                            &mut state,
                            1,
                            &crate::DARK,
                            frame == 0,
                            crate::tr(crate::Lang::ZhCn),
                        );
                    });
                },
            );
        }
        assert_eq!(state.selected_characters, 3);
        assert_eq!(text, original);
        assert!(state.large.visual_starts[1] > 1);
        let before = state.large.visual_starts.clone();
        state
            .large
            .external_replace(&mut text, 0..0, "\n".to_owned());
        state.large.undo(&mut text, false);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.large.visual_index(
                    ui,
                    &text,
                    &FontId::monospace(13.5),
                    &crate::DARK,
                    f32::from_bits(state.large.visual_width.unwrap()),
                    true,
                )
            });
        });
        assert_eq!(state.large.visual_starts, before);
        assert_eq!(text, original);
    }

    #[test]
    fn legacy_size_and_invalid_counts_update_incrementally_and_through_undo() {
        use crate::recovery::Encoding;
        let mut text = "中文\r\nhello".repeat(30_000);
        let mut state = State::default();
        state.index(&text);
        state.set_encoding(&text, Encoding::Gbk);
        assert_eq!(
            state.legacy_bytes,
            Encoding::Gbk.encode(&text).unwrap().len()
        );
        state.record(&mut text, 0..3, "简体".to_owned(), None, None);
        assert_eq!(
            state.legacy_bytes,
            Encoding::Gbk.encode(&text).unwrap().len()
        );
        state.record(&mut text, 0..0, "🙂".to_owned(), None, None);
        assert_eq!(state.legacy_invalid, 1);
        assert!(Encoding::Gbk.encode(&text).is_err());
        state.undo(&mut text, false);
        assert_eq!(state.legacy_invalid, 0);
        assert_eq!(
            state.legacy_bytes,
            Encoding::Gbk.encode(&text).unwrap().len()
        );
        state.set_encoding(&text, Encoding::Gb18030);
        state.undo(&mut text, true);
        assert_eq!(state.legacy_invalid, 0);
        assert_eq!(
            state.legacy_bytes,
            Encoding::Gb18030.encode(&text).unwrap().len()
        );
    }

    #[test]
    fn difference_preserves_multibyte_boundaries_and_clear_keeps_line_breaks() {
        for old in ["", "中🙂abc", "中文\r\n🙂", "a中a"] {
            for new in ["", "中新abc", "中🙃abc", "\n", "a🙂a"] {
                let (removed, inserted) = difference(old, new);
                let mut result = old.to_owned();
                result.replace_range(removed, &new[inserted]);
                assert_eq!(result, new);
            }
        }
        let original = "before\r\n中文🙂\r\nabc\nlast\r\n";
        let mut text = original.to_owned();
        let mut state = State::default();
        state.index(&text);
        state.clear_lines(&mut text, (1, 3));
        assert_eq!(text, "before\r\n\r\n\n\r\n");
        assert_index(&state, &text);
        assert!(state.undo(&mut text, false));
        assert_eq!(text, original);
    }

    fn assert_index(state: &State, text: &str) {
        assert_eq!(state.fingerprints, crate::changes::lines(text));
        assert_eq!(state.ranges, editor::line_ranges(text));
        assert_eq!(state.characters, text.chars().count());
        assert_eq!(
            state.utf16_units,
            text.chars().map(char::len_utf16).sum::<usize>()
        );
        for (i, range) in state.ranges.iter().enumerate() {
            assert_eq!(state.char_starts[i], text[..range.start].chars().count());
            assert_eq!(state.widths[i], columns(&text[range.clone()]));
            assert_eq!(state.line_for_char(state.char_starts[i]), i);
        }
    }

    #[test]
    fn incremental_index_matches_full_scan_for_unicode_crlf_and_line_merges() {
        let original = "中文🙂\r\n\nabc\tdef\r\n末尾\r".repeat(12);
        let mut text = original.clone();
        let mut state = State::default();
        state.index(&text);
        let mut seed = 42u64;
        for i in 0..300 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let boundaries = text
                .char_indices()
                .map(|(b, _)| b)
                .chain(std::iter::once(text.len()))
                .collect::<Vec<_>>();
            let a = seed as usize % boundaries.len();
            let b = (a + i % 5).min(boundaries.len() - 1);
            let insert = ["中🙂", "\r", "\n", "\r\n\n", "", "\tabc"][i % 6];
            state.record(
                &mut text,
                boundaries[a]..boundaries[b],
                insert.to_owned(),
                None,
                None,
            );
            assert_index(&state, &text);
        }
        for _ in 0..300 {
            assert!(state.undo(&mut text, false));
            assert_index(&state, &text);
        }
        assert_eq!(text, original);
        assert_eq!(state.revision, 0);
        for _ in 0..300 {
            assert!(state.undo(&mut text, true));
            assert_index(&state, &text);
        }
    }

    #[test]
    fn delta_history_does_not_retain_full_documents_and_branch_revisions_are_unique() {
        let mut text = "line 中文🙂\r\n".repeat(100_000);
        let mut state = State::default();
        state.index(&text);
        for i in 0..100 {
            state.record(&mut text, i..i, "x".to_owned(), None, None);
        }
        assert_eq!(
            state
                .undo
                .iter()
                .map(|e| e.removed.len() + e.inserted.len())
                .sum::<usize>(),
            100
        );
        let revision = state.revision;
        assert!(state.undo(&mut text, false));
        state.record(&mut text, 0..0, "y".to_owned(), None, None);
        assert!(state.revision > revision);
        assert!(state.redo.is_empty());
    }

    #[test]
    #[ignore = "manual synthetic large-buffer benchmark"]
    fn benchmark_large_incremental_edits() {
        for mb in [3, 32, 128] {
            let line = format!("中文🙂 {}\r\n", "x".repeat(180));
            let mut text = line.repeat(mb * 1024 * 1024 / line.len());
            let mut state = State::default();
            let start = std::time::Instant::now();
            state.index(&text);
            let indexing = start.elapsed();
            text.reserve(1024);
            let mut times = Vec::new();
            for i in 0..20 {
                let offset = state.ranges[state.ranges.len() / 2].start;
                let start = std::time::Instant::now();
                state.record(
                    &mut text,
                    offset..offset,
                    if i % 2 == 0 { "中" } else { "\n" }.to_owned(),
                    None,
                    None,
                );
                times.push(start.elapsed());
            }
            times.sort();
            eprintln!(
                "{mb} MiB: index {indexing:?}, middle-edit median {:?}, max {:?}, history {} bytes",
                times[10],
                times[19],
                state
                    .undo
                    .iter()
                    .map(|e| e.removed.len() + e.inserted.len())
                    .sum::<usize>()
            );
            assert_eq!(state.characters, text.chars().count());
            assert_eq!(state.ranges, editor::line_ranges(&text));
        }
    }

    #[test]
    fn select_all_copies_entire_document_not_only_the_visible_window() {
        let original = "中文 line\r\n".repeat(25000);
        let mut text = original.clone();
        let mut state = editor::EditorState::default();
        let ctx = egui::Context::default();
        let mut copied = false;
        for frame in 0..5 {
            let events = match frame {
                2 => vec![egui::Event::Key {
                    key: egui::Key::A,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::COMMAND,
                }],
                3 => vec![egui::Event::Copy],
                _ => vec![],
            };
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 600.0),
                    )),
                    modifiers: if frame == 2 {
                        egui::Modifiers::COMMAND
                    } else {
                        egui::Modifiers::default()
                    },
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        editor::show(
                            ui,
                            &mut text,
                            &mut state,
                            1,
                            &crate::DARK,
                            frame == 0,
                            crate::tr(crate::Lang::En),
                        );
                    });
                },
            );
            copied |= output.platform_output.commands.iter().any(
                |cmd| matches!(cmd, egui::OutputCommand::CopyText(value) if value == &original),
            );
        }
        assert!(copied);
        assert_eq!(text, original);
    }

    #[test]
    fn large_document_layout_is_bounded_and_editing_undo_preserves_other_lines() {
        let original = (0..18000)
            .map(|i| format!("row {i}: 中文 {}\n", "x".repeat(100)))
            .collect::<String>();
        let mut text = original.clone();
        let mut state = editor::EditorState::default();
        let ctx = egui::Context::default();
        let mut last = None;
        for frame in 0..7 {
            let events = match frame {
                3 => vec![egui::Event::Text("inserted".to_owned())],
                5 => vec![egui::Event::Key {
                    key: egui::Key::Z,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::COMMAND,
                }],
                _ => vec![],
            };
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 600.0),
                    )),
                    modifiers: if frame == 5 {
                        egui::Modifiers::COMMAND
                    } else {
                        egui::Modifiers::default()
                    },
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        editor::show(
                            ui,
                            &mut text,
                            &mut state,
                            1,
                            &crate::DARK,
                            frame == 1,
                            crate::tr(crate::Lang::En),
                        );
                    });
                },
            );
            if frame == 2 {
                assert!(state.large.laid_out_bytes < original.len() / 10);
            }
            if frame == 3 {
                assert_eq!(text.len(), original.len() + "inserted".len());
                assert!(text.ends_with(&original[original.len() - 1000..]));
            }
            last = Some(output);
        }
        assert_eq!(text, original);
        assert!(last.is_some());
    }

    #[test]
    fn scrolling_to_middle_then_editing_uses_global_line_offsets() {
        let mut text = (0..18000)
            .map(|i| format!("row {i}: {}\n", "x".repeat(100)))
            .collect::<String>();
        let original = text.clone();
        let mut state = editor::EditorState::default();
        let ctx = egui::Context::default();
        let mut target = None;
        let mut expected_start = 0;
        for frame in 0..10 {
            let mut events = vec![];
            if frame == 2 {
                events = vec![
                    egui::Event::PointerMoved(egui::pos2(200.0, 200.0)),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(-100.0, -5000.0),
                        modifiers: egui::Modifiers::default(),
                    },
                ];
            }
            if frame == 6 || frame == 7 {
                let pos = target.unwrap();
                events = vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: frame == 6,
                        modifiers: egui::Modifiers::default(),
                    },
                ];
            }
            if frame == 8 {
                events = vec![egui::Event::Text("INSERTED".to_owned())];
            }
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 600.0),
                    )),
                    time: Some(frame as f64),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        editor::show(
                            ui,
                            &mut text,
                            &mut state,
                            1,
                            &crate::DARK,
                            false,
                            crate::tr(crate::Lang::En),
                        );
                    });
                },
            );
            if frame == 5 {
                assert!(state.large.window_lines.start > 100);
                expected_start = state.large.window_bytes.start;
                for shape in output.shapes {
                    if let egui::Shape::Text(text) = shape.shape
                        && text.galley.text().chars().all(|c| c.is_ascii_digit())
                        && text.pos.y > 150.0
                        && text.pos.y < 200.0
                    {
                        target = Some(egui::pos2(150.0, text.pos.y + 5.0));
                        break;
                    }
                }
            }
        }
        assert_eq!(text.len(), original.len() + 8);
        assert_eq!(text[..expected_start], original[..expected_start]);
        assert!(text[expected_start..].contains("INSERTED"));
        assert!(text.ends_with(&original[original.len() - 1000..]));
    }
}

//! Viewport-windowed editing: only nearby lines are laid out by TextEdit.
use crate::{Palette, editor, i18n::Strings};
use eframe::egui;
use egui::{FontId, Stroke};
use std::ops::Range;

pub(crate) const THRESHOLD: usize = 256 * 1024;

pub(crate) fn needs_windowing(text: &str) -> bool {
    text.len() >= THRESHOLD || text.len() > LAYOUT_BYTES && text.split('\n').any(|line| line.len() > LAYOUT_BYTES)
}

#[derive(Clone, Default)]
pub(crate) struct State {
    pub(crate) active: bool,
    pub(crate) indexed_len: Option<usize>,
    ranges: Vec<Range<usize>>,
    max_columns: usize,
    window: String,
    window_lines: Range<usize>,
    window_bytes: Range<usize>,
    projection: Vec<ProjectedLine>,
    projection_generation: Option<u64>,
    char_start: usize,
    cursor: Option<egui::text::CCursorRange>,
    char_starts: Vec<usize>,
    widths: Vec<usize>,
    pub(crate) characters: usize,
    pub(crate) newlines: crate::line_endings::Counts,
    pub(crate) utf16_units: usize,
    metrics_encoding: Option<crate::recovery::Encoding>,
    pub(crate) legacy_bytes: usize,
    pub(crate) legacy_invalid: usize,
    pub(crate) revision: u64,
    next_revision: u64,
    undo: Vec<Edit>,
    undo_limit: usize,
    redo: Vec<Edit>,
    wrap_key: Option<(u32, usize)>,
    visual_starts: Vec<usize>,
    visual_rows: Vec<usize>,
    visual_width: Option<u32>,
    font_key: Option<(u32, u32)>,
    fold_generation: Option<u64>,
    visible_lines: Vec<usize>,
    fingerprints: Vec<u64>,
    tool_edits: Vec<(Range<usize>, usize)>,
    ime_selection: Option<egui::text::CCursorRange>,
    layout_limited: bool,
    wrap_incomplete: bool,
    #[cfg(test)]
    pub(crate) laid_out_bytes: usize,
    #[cfg(test)]
    pub(crate) window_builds: usize,
}

const HISTORY_BYTES: usize = 16 * 1024 * 1024;
// A small navigation cushion, not hundreds of potentially long JSONL rows.
const OVERSCAN_LINES: usize = 12;
const LAYOUT_BYTES: usize = 64 * 1024;
const LONG_LINE_COLUMNS: usize = 32 * 1024;

/// One visible source line in a compact display buffer. Document cursor and
/// byte coordinates always remain in the original, unprojected text.
#[derive(Clone)]
struct ProjectedLine {
    line: usize,
    local_char: usize,
    global_char: usize,
    local_byte: usize,
    global_byte: usize,
}

fn project_cursor(cursor: egui::text::CCursorRange, lines: &[ProjectedLine], to_global: bool) -> egui::text::CCursorRange {
    let map = |mut c: egui::text::CCursor| {
        let at = lines.partition_point(|l| if to_global { l.local_char } else { l.global_char } <= c.index).saturating_sub(1);
        if let Some(line) = lines.get(at) {
            let (from, to) = if to_global { (line.local_char, line.global_char) } else { (line.global_char, line.local_char) };
            c.index = to + c.index.saturating_sub(from);
        }
        c
    };
    egui::text::CCursorRange::two(map(cursor.secondary), map(cursor.primary))
}

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
    #[cfg(test)]
    pub(crate) fn wrapping_pending(&self) -> bool { self.wrap_incomplete }
    /// Install a worker-built document/index without rescanning the live text.
    /// Keep the existing delta history, with precisely one new undo point.
    pub(crate) fn commit_prepared(&mut self, text: &mut String, mut prepared: PreparedReplacement) {
        let _timing = crate::timing::phase("batch.publish");
        self.next_revision += 1;
        prepared.edit.before = self.revision;
        prepared.edit.after = self.next_revision;
        prepared.edit.cursor_before = self.cursor;
        prepared.state.revision = self.next_revision;
        prepared.state.next_revision = self.next_revision;
        prepared.state.cursor = prepared.edit.cursor_after;
        prepared.state.undo_limit = self.undo_limit;
        prepared.state.undo = std::mem::take(&mut self.undo);
        prepared.state.undo.push(prepared.edit);
        prepared.state.trim_history();
        *text = prepared.text;
        *self = prepared.state;
    }
    pub(crate) fn prepare_loaded(&mut self, text: &str) {
        self.index(text);
        self.active = true;
    }
    /// Import native undo points once when switching renderers; retain bounded
    /// deltas afterwards, not the native editor's full document snapshots.
    pub(crate) fn import_history(
        &mut self, ctx: &egui::Context, id: u64, text: &str,
        cursor: Option<egui::text::CCursorRange>, saved: Option<&str>,
    ) -> Option<u64> {
        let id = egui::Id::new("draft_editor").with(id);
        let mut native = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
        *self = Self::default();
        let mut undoer = native.undoer();
        let mut current = (cursor.unwrap_or_default(), text.to_owned());
        let mut points = vec![current.clone()];
        while let Some(previous) = undoer.undo(&current).cloned() {
            if previous.1 != current.1 { points.push(previous.clone()); }
            current = previous;
        }
        points.reverse();
        let mut working = points[0].1.clone();
        let mut before = points[0].0;
        let mut saved_revision = (saved == Some(working.as_str())).then_some(0);
        for (after, next) in points.into_iter().skip(1) {
            let (old, new) = difference(&working, &next);
            self.record(&mut working, old, next[new].to_owned(), Some(before), Some(after));
            if saved == Some(working.as_str()) { saved_revision = Some(self.revision); }
            before = after;
        }
        self.index(text);
        self.cursor = cursor;
        self.tool_edits.clear(); // These edits already happened in the native editor.
        native.clear_undoer();
        native.store(ctx, id);
        saved_revision
    }
    pub(crate) fn line_start(&mut self, text: &str, line: usize) -> Option<usize> {
        self.index(text);
        self.char_starts.get(line).copied()
    }
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
        self.projection_generation = None;
    }
    pub(crate) fn set_tool_cursor(&mut self, cursor: egui::text::CCursorRange) {
        self.cursor = Some(cursor);
        if let Some(edit) = self.undo.last_mut() {
            edit.cursor_after = Some(cursor);
        }
    }
    pub(crate) fn tool_cursor(&self) -> Option<egui::text::CCursorRange> { self.cursor }
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
        self.newlines = crate::line_endings::Counts::of(text);
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

    pub(crate) fn byte_for_char(&self, text: &str, character: usize) -> usize {
        let line = self.line_for_char(character);
        let byte = self.ranges.get(line).map_or(0, |range| range.start);
        byte + crate::text_tools::byte_at(&text[byte..], character.saturating_sub(self.char_starts.get(line).copied().unwrap_or(0)))
    }

    fn global_selection_input(&mut self, text: &mut String, ctx: &egui::Context) -> bool {
        let mut changed = false;
        let events = ctx.input_mut(|i| std::mem::take(&mut i.events));
        let mut retained = Vec::new();
        for event in events {
            if matches!(&event, egui::Event::Ime(egui::ImeEvent::Disabled)) { self.ime_selection = None; }
            if matches!(&event, egui::Event::Key { key: egui::Key::A, pressed: true, modifiers, .. } if modifiers.matches_exact(egui::Modifiers::COMMAND)) {
                self.cursor = Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(self.characters)));
                continue;
            }
            if matches!(&event, egui::Event::Ime(egui::ImeEvent::Commit(_))) && self.ime_selection.is_some() {
                self.cursor = self.ime_selection.take();
            }
            let cursor = self.cursor.unwrap_or_default();
            if self.layout_limited
                && let egui::Event::Key { key: egui::Key::Home | egui::Key::End, pressed: true, modifiers, .. } = &event {
                let line = self.line_for_char(cursor.primary.index);
                let primary = egui::text::CCursor::new(self.char_starts[line] + if matches!(&event, egui::Event::Key { key: egui::Key::End, .. }) { text[self.ranges[line].clone()].chars().count() } else { 0 });
                self.cursor = Some(egui::text::CCursorRange::two(if modifiers.shift { cursor.secondary } else { primary }, primary));
                self.invalidate_window();
                continue;
            }
            // A bounded local selection remains native. Wider selections are
            // document state, never a request to lay out all selected text.
            if cursor.primary.index.abs_diff(cursor.secondary.index) < 4096
                && !self.layout_limited {
                retained.push(event);
                continue;
            }
            let mut first_character = cursor.primary.index.min(cursor.secondary.index);
            let mut last_character = cursor.primary.index.max(cursor.secondary.index);
            let empty = first_character == last_character;
            if empty && self.layout_limited && matches!(&event, egui::Event::Copy | egui::Event::Cut) {
                let line = self.line_for_char(first_character);
                let start = self.ranges[line].start;
                let end = self.ranges.get(line + 1).map_or(text.len(), |range| range.start);
                ctx.copy_text(text[start..end].to_owned());
                if matches!(event, egui::Event::Cut) {
                    let after = egui::text::CCursorRange::one(egui::text::CCursor::new(self.char_starts[line]));
                    self.record(text, start..end, String::new(), Some(cursor), Some(after));
                    self.cursor = Some(after);
                    changed = true;
                }
                continue;
            }
            if empty && let egui::Event::Key { key, pressed: true, .. } = &event {
                match key {
                    egui::Key::Backspace => first_character = first_character.saturating_sub(1),
                    egui::Key::Delete => last_character = (last_character + 1).min(self.characters),
                    _ => {}
                }
            }
            let start = self.byte_for_char(text, first_character);
            let end = self.byte_for_char(text, last_character);
            let inserted = match &event {
                egui::Event::Copy => { ctx.copy_text(text[start..end].to_owned()); continue; }
                egui::Event::Cut => { ctx.copy_text(text[start..end].to_owned()); Some(String::new()) }
                egui::Event::Text(value) | egui::Event::Paste(value) => Some(value.clone()),
                egui::Event::Ime(egui::ImeEvent::Enabled | egui::ImeEvent::Preedit(_)) => {
                    self.ime_selection = Some(cursor);
                    continue;
                }
                egui::Event::Ime(egui::ImeEvent::Commit(value)) => Some(value.clone()),
                egui::Event::Ime(egui::ImeEvent::Disabled) => { self.ime_selection = None; continue; }
                egui::Event::Key { key: egui::Key::Backspace | egui::Key::Delete, pressed: true, .. } => Some(String::new()),
                egui::Event::Key { key, pressed: true, modifiers, .. }
                    if matches!(key, egui::Key::ArrowLeft | egui::Key::ArrowRight | egui::Key::ArrowUp | egui::Key::ArrowDown | egui::Key::Home | egui::Key::End | egui::Key::PageUp | egui::Key::PageDown | egui::Key::Escape) => {
                    let mut primary = cursor.primary.index;
                    let line = self.line_for_char(primary);
                    let line_start = self.char_starts[line];
                    let line_end = line_start + text[self.ranges[line].clone()].chars().count();
                    primary = match key {
                        egui::Key::ArrowLeft => if modifiers.shift || empty { primary.saturating_sub(1) } else { cursor.primary.index.min(cursor.secondary.index) },
                        egui::Key::ArrowRight => if modifiers.shift || empty { (primary + 1).min(self.characters) } else { cursor.primary.index.max(cursor.secondary.index) },
                        egui::Key::Home => line_start,
                        egui::Key::End => line_end,
                        egui::Key::Escape => primary,
                        _ => {
                            let step = if matches!(key, egui::Key::PageUp | egui::Key::PageDown) { 20 } else { 1 };
                            let target = if matches!(key, egui::Key::ArrowUp | egui::Key::PageUp) { line.saturating_sub(step) } else { (line + step).min(self.ranges.len() - 1) };
                            self.char_starts[target] + (primary - line_start).min(text[self.ranges[target].clone()].chars().count())
                        }
                    };
                    self.cursor = Some(egui::text::CCursorRange::two(if modifiers.shift && *key != egui::Key::Escape { cursor.secondary } else { egui::text::CCursor::new(primary) }, egui::text::CCursor::new(primary)));
                    self.invalidate_window();
                    continue;
                }
                _ => None,
            };
            if let Some(inserted) = inserted {
                let after = egui::text::CCursorRange::one(egui::text::CCursor::new(first_character + inserted.chars().count()));
                self.record(text, start..end, inserted, Some(cursor), Some(after));
                self.cursor = Some(after);
                changed = true;
            } else { retained.push(event); }
        }
        ctx.input_mut(|i| i.events = retained);
        changed
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
        // Include one byte on each side to account for edits splitting/joining CRLF.
        let local_start = range.start.saturating_sub(1);
        let local_end = (range.end + 1).min(text.len());
        // Counts only inspect bytes; the context need not be a UTF-8 boundary.
        let previous = local_start.checked_sub(1).map(|at| text.as_bytes()[at]);
        let old_counts = crate::line_endings::Counts::from_bytes(text.as_bytes()[local_start..local_end].iter().copied(), previous);
        let new_counts = crate::line_endings::Counts::from_bytes(
            text.as_bytes()[local_start..range.start].iter().copied()
                .chain(replacement.bytes()).chain(text.as_bytes()[range.end..local_end].iter().copied()), previous);
        self.newlines.lf = self.newlines.lf + new_counts.lf - old_counts.lf;
        self.newlines.crlf = self.newlines.crlf + new_counts.crlf - old_counts.crlf;
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
        self.trim_history();
    }

    fn trim_history(&mut self) {
        let mut size: usize = self
            .undo
            .iter()
            .map(|e| e.removed.len() + e.inserted.len())
            .sum();
        while self.undo.len() > 1 && (size > HISTORY_BYTES || self.undo.len() > if self.undo_limit == 0 { crate::undo_settings::DEFAULT } else { self.undo_limit }) {
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
    pub(crate) fn install_encoding_metrics(&mut self, encoding: crate::recovery::Encoding, metrics: (usize, usize)) {
        self.metrics_encoding = encoding.is_legacy().then_some(encoding);
        (self.legacy_bytes, self.legacy_invalid) = metrics;
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
        let font_key = (font.size.to_bits(), ui.ctx().pixels_per_point().to_bits());
        if self.font_key != Some(font_key) {
            self.font_key = Some(font_key);
            self.wrap_key = None;
            self.visual_width = None;
            self.invalidate_window();
        }
        let key = (if wrap { width.to_bits() } else { 0 }, self.ranges.len());
        if self.wrap_key == Some(key) {
            return;
        }
        // A width change invalidates every layout; edits invalidate touched lines only.
        if self.visual_width != Some(key.0) || self.visual_rows.len() != self.ranges.len() {
            self.visual_rows = vec![0; self.ranges.len()];
        }
        self.visual_width = Some(key.0);
        let started = std::time::Instant::now();
        let _timing = crate::timing::phase("layout.wrap_slice");
        let mut deferred = false;
        let mut rows = 0;
        self.visual_starts.clear();
        for (i, range) in self.ranges.iter().enumerate() {
            self.visual_starts.push(rows);
            if self.visual_rows[i] == 0 {
                if wrap && started.elapsed() >= std::time::Duration::from_millis(3) {
                    deferred = true;
                    rows += 1; // Placeholder, never cached as a completed layout.
                    continue;
                }
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
        self.wrap_incomplete = deferred;
        self.wrap_key = (!deferred).then_some(key);
        if deferred { ui.ctx().request_repaint_after(std::time::Duration::from_millis(16)); }
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
                rows += (*height).max(1);
            }
        }
        self.visual_starts.push(rows);
        self.fold_generation = Some(folds.generation);
    }
}

pub(crate) struct PreparedReplacement {
    text: String,
    state: State,
    edit: Edit,
}

pub(crate) fn prepare_replacement(
    source: &str, range: Range<usize>, inserted: String,
    cursor: egui::text::CCursorRange, encoding: crate::recovery::Encoding,
) -> PreparedReplacement {
    let _timing = crate::timing::phase("batch.output_and_index");
    let mut text = String::with_capacity(source.len() - range.len() + inserted.len());
    text.push_str(&source[..range.start]);
    text.push_str(&inserted);
    text.push_str(&source[range.end..]);
    let mut state = State::default();
    state.prepare_loaded(&text);
    state.set_encoding(&text, encoding);
    PreparedReplacement { text, state, edit: Edit {
        start: range.start, removed: source[range].to_owned(), inserted,
        before: 0, after: 0, cursor_before: None, cursor_after: Some(cursor),
    } }
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
    state.large.active = true;
    state.large.undo_limit = crate::undo_settings::limit(ui.ctx());
    state.large.trim_history();
    if state.large.redo.len() > state.large.undo_limit {
        let excess = state.large.redo.len() - state.large.undo_limit;
        state.large.redo.drain(..excess);
    }
    let id = egui::Id::new("large_editor").with(file_id);
    if ui.is_enabled() { state.line_ending.remap_enter(ui.ctx(), id, &mut state.composing, text, state.large.cursor); }
    let fold_selection = state
        .folds
        .prepare_input(ui, state.pending_selection.is_some(), id);
    if state.pending_selection.is_some() && !fold_selection || !state.folds.has_collapsed() {
        state.fold_scroll_extent = 0.0;
    }
    let folds = state.folds.clone();
    let fold_w = folds.width();
    editor::scroll_style(ui, pal);
    let font = state.font();
    let gutter_font = state.gutter_font();
    let mut tools = state.tools.clone();
    if let Some(render) = &tools.workspace {
        render.apply_style(ui);
    }
    let colors = tools.workspace.as_ref().map(|r| r.colors);
    let gutter_bg = editor::gutter_background(colors.map_or(pal.bg_window, |c| c.background));
    let row_height = editor::row_height(ui, &font);
    let probe = ui
        .painter()
        .layout_no_wrap("0000000000".to_owned(), font.clone(), pal.text_main);
    let digit_w = probe.rows[0].glyphs[1].pos.x - probe.rows[0].glyphs[0].pos.x;
    let number_probe = ui.painter().layout_no_wrap("0000000000".to_owned(), gutter_font.clone(), pal.text_faint);
    let number_digit_w = number_probe.rows[0].glyphs[1].pos.x - number_probe.rows[0].glyphs[0].pos.x;
    state.large.index(text);
    let long_lines = state.large.max_columns > LONG_LINE_COLUMNS;
    if long_lines { tools.wrap = false; }
    if long_lines || state.large.wrap_incomplete {
        let zh = t.encoding_binary == "二进制";
        ui.add(egui::Label::new(egui::RichText::new(if long_lines {
            if zh { "长行保护：暂停自动换行，按光标附近分段显示；全文搜索／复制／保存不截断。使用首尾导航或查找定位。" }
            else { "Long-line protection: wrap paused; display is chunked near the caret. Search, copy and save retain all text. Use document navigation or Find." }
        } else if zh { "正在分帧计算自动换行…" } else { "Preparing wrapped layout across frames…" }).small().color(pal.text_faint)).wrap());
    }
    let bounds = ui.available_rect_before_wrap();
    state
        .changes
        .update(&state.large.fingerprints, state.large.revision);
    let count = state.large.ranges.len();
    let number_w = editor::gutter_number_width(count, number_digit_w);
    let wrap_width =
        (bounds.width() - number_w - fold_w - 38.0 - if tools.whitespace { 12.0 } else { 0.0 }).max(24.0);
    state
        .large
        .visual_index(ui, text, &font, pal, wrap_width, tools.wrap);
    state.large.fold_index(&folds);
    if let Some((start, end)) = state.selection {
        state.selection = Some((start.min(count - 1), end.min(count - 1)));
    }
    let gutter_w = number_w + fold_w;
    let gutter = egui::Rect::from_min_max(
        bounds.min,
        egui::pos2(bounds.left() + gutter_w, bounds.bottom()),
    );
    ui.painter()
        .rect_filled(gutter, 0.0, gutter_bg);
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
    // Whole-document navigation must not be delegated to the viewport fragment.
    if ui.is_enabled() && ui.memory(|m| m.has_focus(id)) {
        let mut moves = Vec::new();
        ui.input_mut(|i| i.events.retain(|event| {
            if let egui::Event::Key { key, pressed: true, modifiers, .. } = event {
                let end = match key {
                    egui::Key::Home if modifiers.ctrl && !modifiers.alt => Some(false),
                    egui::Key::End if modifiers.ctrl && !modifiers.alt => Some(true),
                    egui::Key::ArrowUp if modifiers.command && !modifiers.alt => Some(false),
                    egui::Key::ArrowDown if modifiers.command && !modifiers.alt => Some(true),
                    _ => None,
                };
                if let Some(end) = end { moves.push((end, modifiers.shift)); return false; }
            }
            true
        }));
        for (end, extend) in moves {
            let primary = egui::text::CCursor::new(if end { state.large.characters } else { 0 });
            let secondary = if extend { state.large.cursor.unwrap_or_default().secondary } else { primary };
            let cursor = egui::text::CCursorRange::two(secondary, primary);
            state.large.cursor = Some(cursor);
            state.pending_selection = Some(secondary.index..primary.index);
            state.selection = None;
            state.folds.expand_all();
            ui.ctx().request_repaint();
        }
        state.large.fold_index(&state.folds);
    }
    let folds = state.folds.clone();
    if ui.is_enabled() && ui.memory(|m| m.has_focus(id)) {
        changed |= state.large.global_selection_input(text, ui.ctx());
        state.large.visual_index(ui, text, &font, pal, wrap_width, tools.wrap);
        state.large.fold_index(&folds);
    }
    let reveal = state.pending_selection.is_some() && !fold_selection;
    if let Some(range) = state.pending_selection.take() {
        state.large.cursor = Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(range.start),
            egui::text::CCursor::new(range.end),
        ));
        state.large.invalidate_window();
        if !fold_selection {
            let line = state.large.line_for_char(range.end);
            let scroll_id = ui.make_persistent_id(egui::Id::new(("draft_scroll", file_id)));
            let mut scroll = egui::scroll_area::State::load(ui.ctx(), scroll_id).unwrap_or_default();
            scroll.offset.y =
                state.large.visual_starts.get(line).copied().unwrap_or(line) as f32 * row_height;
            scroll.store(ui.ctx(), scroll_id);
        }
    }
    let count = state.large.ranges.len();
    let mut line_action = None;
    let mut fold_pointer = false;
    let mut viewport_offset = egui::Vec2::ZERO;
    let mut depth_action = None;
    let mut depth_offset = None;
    if let Some((start, end)) = state.selection {
        state.selection = Some((start.min(count - 1), end.min(count - 1)));
    }
    let scroll_output = egui::ScrollArea::both()
        .id_salt(("draft_scroll", file_id))
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
        .scroll_bar_rect(egui::Rect::from_min_max(
            egui::pos2(gutter.right() + 1.0, bounds.top()),
            bounds.max,
        ))
        .auto_shrink([false, false])
        .show_viewport(ui, |ui, viewport| {
            viewport_offset = viewport.min.to_vec2();
            let origin = ui.cursor().min;
            let viewport_clip = ui.clip_rect();
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
            let visible_first = state.large.visible_lines.partition_point(|&line| line < first);
            let visible_last = state.large.visible_lines.partition_point(|&line| line < visible_end);
            // Overscan visible rows, never a large span of hidden logical lines.
            let window_first = visible_first.saturating_sub(OVERSCAN_LINES);
            let window_last = (visible_last + OVERSCAN_LINES).min(state.large.visible_lines.len());
            let desired = state.large.visible_lines[window_first]
                ..state.large.visible_lines[window_last.saturating_sub(1)] + 1;
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
            // Keep a focused off-screen cursor valid when typing after scrolling away.
            if focused
                && (keyboard_input || pointer_down)
                && let Some(cursor) = state.large.cursor
            {
                let line = state.large.line_for_char(cursor.primary.index);
                if keyboard_input && (line < window.start || line >= window.end) {
                    window =
                        line.saturating_sub(OVERSCAN_LINES)..(line + OVERSCAN_LINES + 1).min(count);
                }
            }
            if focused
                && let Some(cursor) = state.large.cursor
                && cursor.primary.index != cursor.secondary.index
                && cursor.primary.index.abs_diff(cursor.secondary.index) < 4096
            {
                // Preserve explicit selections across viewport changes, including Select All + Copy.
                for cursor in [cursor.primary, cursor.secondary] {
                    let line = state.large.line_for_char(cursor.index);
                    window.start = window.start.min(line);
                    window.end = window.end.max((line + 1).min(count));
                }
            }
            let mut start = state.large.ranges[window.start].start;
            // Exclude the boundary newline; edits replace exactly this byte range.
            let mut end = state.large.ranges[window.end - 1].end;
            if long_lines && end - start > LAYOUT_BYTES {
                let primary = state.large.cursor.unwrap_or_default().primary.index;
                let caret = state.large.byte_for_char(text, primary);
                let target = if caret >= start && caret <= end { caret } else { start };
                // Keep at least a half-page before the caret; chunk boundaries
                // are UTF-8 safe and do not become document/newline boundaries.
                start = target.saturating_sub(LAYOUT_BYTES / 2).max(start);
                while !text.is_char_boundary(start) { start += 1; }
                end = (start + LAYOUT_BYTES).min(end);
                while !text.is_char_boundary(end) { end -= 1; }
                if end == caret && caret > start {
                    start = end.saturating_sub(LAYOUT_BYTES);
                    while !text.is_char_boundary(start) { start += 1; }
                }
                window.start = state.large.ranges.partition_point(|range| range.start <= start).saturating_sub(1);
                window.end = (state.large.ranges.partition_point(|range| range.start < end)).max(window.start + 1);
            }
            let bytes = start..end;
            let projected = folds.has_collapsed() && !long_lines;
            state.large.layout_limited = start != state.large.ranges[window.start].start || end != state.large.ranges[window.end - 1].end;
            let wide_selection = state.large.cursor.is_some_and(|cursor| cursor.primary.index.abs_diff(cursor.secondary.index) >= 4096 || state.large.layout_limited && cursor.primary.index != cursor.secondary.index);
            if state.large.window_lines != window || state.large.window_bytes != bytes || changed || wide_selection
                || state.large.projection_generation != Some(folds.generation) {
                #[cfg(test)]
                { state.large.window_builds += 1; }
                state.large.projection.clear();
                if projected {
                    let mut display = String::new();
                    let mut characters = 0;
                    let first_visible = state.large.visible_lines.partition_point(|&l| l < window.start);
                    let end_visible = state.large.visible_lines.partition_point(|&l| l < window.end);
                    for &line in &state.large.visible_lines[first_visible..end_visible] {
                        if !state.large.projection.is_empty() { display.push('\n'); characters += 1; }
                        state.large.projection.push(ProjectedLine {
                            line, local_char: characters, global_char: state.large.char_starts[line],
                            local_byte: display.len(), global_byte: state.large.ranges[line].start,
                        });
                        // Keep a CR before LF for whitespace hints and exact
                        // character mappings, but omit the boundary newline.
                        let end = state.large.ranges.get(line + 1).map_or(text.len(), |r| r.start - 1);
                        let value = &text[state.large.ranges[line].start..end];
                        display.push_str(value);
                        characters += value.chars().count();
                    }
                    state.large.window = display;
                } else {
                    state.large.window = text[bytes.clone()].to_owned();
                }
                state.large.projection_generation = Some(folds.generation);
                state.large.window_lines = window.clone();
                state.large.window_bytes = bytes.clone();
                let offset = state.large.char_starts[window.start] + text[state.large.ranges[window.start].start..start].chars().count();
                state.large.char_start = offset;
                let mut edit_state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
                if let Some(cursor) = state.large.cursor {
                    let mut local = if projected { project_cursor(cursor, &state.large.projection, false) }
                        else { shift_cursor(cursor, -(offset as isize)) };
                    let length = state.large.window.chars().count();
                    local.primary.index = local.primary.index.min(length);
                    local.secondary.index = local.secondary.index.min(length);
                    edit_state.cursor.set_char_range(Some(local));
                }
                edit_state.clear_undoer();
                edit_state.store(ui.ctx(), id);
            }
            let char_start = state.large.char_start;
            // Project highlight byte ranges once; syntax tokenization now sees
            // only visible JSON lines instead of all intervening hidden text.
            let mut layout_tools = tools.clone();
            if projected {
                let project_ranges = |range: &Range<usize>| -> Vec<Range<usize>> {
                    state.large.projection.iter().enumerate().filter_map(|(i, line)| {
                        let local_end = state.large.projection.get(i + 1).map_or(state.large.window.len(), |l| l.local_byte.saturating_sub(1));
                        let start = range.start.max(line.global_byte);
                        let end = range.end.min(line.global_byte + local_end - line.local_byte);
                        (start < end).then_some(line.local_byte + start.saturating_sub(line.global_byte)..line.local_byte + end.saturating_sub(line.global_byte))
                    }).collect()
                };
                layout_tools.marks = tools.marks.iter().flat_map(|m| project_ranges(&m.range).into_iter().map(|range| crate::text_tools::Mark { range, color: m.color })).collect();
                layout_tools.current = tools.current.as_ref().and_then(|r| project_ranges(r).into_iter().next());
            }
            let content_w = if long_lines {
                (columns(&state.large.window) as f32 * digit_w + gutter_w + 20.0).max(bounds.width() - 16.0)
            } else if tools.wrap {
                wrap_width + gutter_w + 16.0
            } else {
                (state.large.max_columns as f32 * digit_w + gutter_w + if tools.whitespace { 32.0 } else { 20.0 })
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
            let projected_before = projected.then(|| state.large.window.clone());
            if let Some(render) = &mut tools.workspace
                && !render.prepare_prefix(text, bytes.start)
            {
                render.syntax = false;
                ui.ctx().request_repaint();
            }
            let mut layouter = |ui: &egui::Ui, text: &str, _: f32| {
                let mut current_tools = layout_tools.clone();
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
                    if projected { 0 } else { bytes.start },
                );
                for section in &mut job.sections {
                    section.format.line_height = Some(row_height);
                }
                let galley = ui.painter().layout_job(job);
                if projected { galley } else { folds.layout(galley, window.start) }
            };
            let cursor_before = state.large.cursor;
            let pointer_extends = ui.input(|input| input.modifiers.shift || input.pointer.primary_down() && !input.pointer.any_pressed());
            let previous =
                egui::TextEdit::load_state(ui.ctx(), id).and_then(|edit| edit.cursor.char_range());
            let rule_viewport = egui::Rect::from_min_max(
                egui::pos2(gutter.right() + 1.0, viewport_clip.top()), viewport_clip.max);
            let rule_painter = ui.painter().with_clip_rect(rule_viewport);
            let rule_slot = crate::notebook::slot(&rule_painter, tools.workspace.as_ref());
            let mut output = ui
                .allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| {
                    ui.set_clip_rect(ui.clip_rect().intersect(egui::Rect::from_min_max(
                        egui::pos2(gutter.right() + 1.0, bounds.top()),
                        viewport_clip.max,
                    )));
                    let mut output = egui::TextEdit::multiline(&mut state.large.window)
                        .id(id)
                        .font(font.clone())
                        .text_color(colors.map_or(pal.text_main, |c| c.text))
                        .frame(false)
                        .margin(egui::Margin::ZERO)
                        .desired_width(rect.width())
                        .desired_rows(1)
                        .layouter(&mut layouter)
                        .lock_focus(true)
                        .show(ui);
                    editor::select_word_on_double_click(
                        ui,
                        &mut output,
                        &state.large.window,
                        || layouter(ui, &state.large.window, rect.width()),
                    );
                    output
                })
                .inner;
            crate::notebook::paint_rows(&rule_painter, rule_slot, tools.workspace.as_ref(),
                &output.galley, output.galley_pos, rule_viewport);
            // egui collapses a selection on any pointer press, including the
            // right-click that opens the context menu. Restore it when the
            // right-click lands inside the selection so the highlight stays
            // visible and copy/cut still target the selected text.
            if ui.input(|input| input.pointer.button_pressed(egui::PointerButton::Secondary))
                && let Some(saved) = previous
                && saved.primary.index != saved.secondary.index
                && let Some(clicked) = output.state.cursor.char_range()
            {
                let start = saved.primary.index.min(saved.secondary.index);
                let end = saved.primary.index.max(saved.secondary.index);
                if start <= clicked.primary.index && clicked.primary.index <= end {
                    output.state.cursor.set_char_range(Some(saved));
                    output.state.clone().store(ui.ctx(), id);
                    // Keep the document-level cursor in sync immediately: the
                    // normal focus/click update below only runs on the press
                    // frame and would otherwise use the collapsed range.
                    let global = if projected {
                        project_cursor(saved, &state.large.projection, true)
                    } else {
                        shift_cursor(saved, char_start as isize)
                    };
                    state.large.cursor = Some(global);
                    ui.ctx().request_repaint();
                }
            }
            editor::paint_unfocused_selection(ui, &mut output);
            if let Some(render) = &tools.workspace {
                render.paint_selected_text(ui, &output);
            } else {
                crate::text_tools::paint_selected_text(ui, &output, ui.visuals().selection.stroke.color);
            }
            if focus {
                output.response.request_focus();
            }
            if !fold_selection { editor::reveal_cursor(ui, &output, previous, gutter_w, reveal); }
            if tools.whitespace && !state.large.layout_limited {
                crate::text_tools::paint_whitespace(ui, &output.galley, output.galley_pos, pal, &text[bytes.end..]);
            }
            if projected {
                let mut logical = 0;
                for row in &output.galley.rows {
                    if let Some(line) = state.large.projection.get(logical)
                        && folds.collapsed(line.line) && row.ends_with_newline {
                        ui.painter().text(output.galley_pos + row.rect.right_center().to_vec2() + egui::vec2(8.0, 0.0),
                            egui::Align2::LEFT_CENTER, "…", FontId::monospace(13.5), pal.text_faint);
                    }
                    if row.ends_with_newline { logical += 1; }
                }
            } else { folds.paint_summaries(ui, &output.galley, output.galley_pos, window.start, pal); }
            if output.response.has_focus()
                && (output.response.clicked()
                    || output.response.changed()
                    || pointer_down
                    || (!wide_selection && keyboard_input))
                && let Some(cursor) = output.state.cursor.char_range()
            {
                let mut global = if projected { project_cursor(cursor, &state.large.projection, true) }
                    else { shift_cursor(cursor, char_start as isize) };
                if pointer_extends && let Some(before) = cursor_before {
                    global.secondary = before.secondary;
                }
                state.large.cursor = Some(global);
            }
            #[cfg(test)]
            {
                state.large.laid_out_bytes = state.large.window.len();
            }
            if output.response.changed() {
                let before = projected_before.as_deref().unwrap_or(&text[bytes.clone()]);
                let (old, new) = difference(before, &state.large.window);
                let inserted = state.large.window[new].to_owned();
                let removed = if projected {
                    let global_byte = |local: usize| {
                        let at = state.large.projection.partition_point(|l| l.local_byte <= local).saturating_sub(1);
                        let line = &state.large.projection[at];
                        line.global_byte + local.saturating_sub(line.local_byte)
                    };
                    global_byte(old.start)..global_byte(old.end)
                } else { bytes.start + old.start..bytes.start + old.end };
                state.large.record(
                    text,
                    removed,
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
            state.tools.handle_marker_mapped(ui, &output, text, |index| {
                if projected { project_cursor(egui::text::CCursorRange::one(egui::text::CCursor::new(index)), &state.large.projection, true).primary.index }
                else { index + char_start }
            });
            ui.expand_to_include_rect(egui::Rect::from_min_size(origin, egui::vec2(
                content_w,
                (24.0 + state.large.visual_starts.last().copied().unwrap_or(count) as f32
                    * row_height).max(state.fold_scroll_extent),
            )));
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
            if !folds.enabled && response.secondary_clicked()
                && let Some(p) = response.interact_pointer_pos()
            {
                let line = line_at(p);
                if !state.selection.is_some_and(|(a, b)| a <= line && line <= b) {
                    state.selection = Some((line, line));
                    state.anchor = Some(line);
                }
            }
            let painter = ui.painter().with_clip_rect(gutter_clip);
            for &line in &state.large.visible_lines[visible_first..visible_last] {
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
                        Stroke::new(2.0_f32, pal.accent),
                    );
                }
                painter.text(
                    egui::pos2(number_gutter.right() - editor::GUTTER_NUMBER_GAP, rect.center().y),
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
            if fold_w > 0.0 {
                let lane = egui::Rect::from_min_max(
                    egui::pos2(number_gutter.right(), gutter.top()),
                    gutter.max,
                );
                ui.painter()
                    .rect_filled(lane, 0.0, gutter_bg);
                for &line in &state.large.visible_lines[visible_first..visible_last] {
                    let Ok(index) = folds.blocks.binary_search_by_key(&line, |b| b.start) else { continue };
                    let block = &folds.blocks[index];
                    let rect = egui::Rect::from_min_size(
                        egui::pos2(
                            lane.left(),
                            origin.y
                                + 12.0
                                + state.large.visual_starts[block.start] as f32 * row_height,
                        ),
                        egui::vec2(fold_w, row_height),
                    );
                    let menu_rect = egui::Rect::from_min_max(
                        egui::pos2(number_gutter.left(), rect.top()), rect.max);
                    if menu_rect.intersects(ui.clip_rect()) {
                        fold_pointer |= ui.input(|i| i.pointer.interact_pos().is_some_and(|p| menu_rect.contains(p))
                            && (i.pointer.secondary_down() || i.pointer.button_released(egui::PointerButton::Secondary)));
                        if crate::json_plugin::depth_menu(menu_rect, file_id, block.start,
                            folds.collapsed(block.start), t.encoding_binary == "二进制", &response) {
                            depth_action = Some(block.start);
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
                        let start = state.large.char_starts[block.start];
                        state.pending_selection = Some(start..start);
                        state.selection = None;
                        ui.ctx().request_repaint();
                    }
                }
            }
            if !folds.enabled { response.context_menu(|ui| {
                let selection = state.selection.unwrap_or((0, 0));
                let label = if selection.0 == selection.1 {
                        t.copy_line
                    } else {
                        t.copy_lines
                    };
                editor::line_menu_style(ui, t.select_all, t.clear_content);
                let copied = || {
                    state.large.ranges[selection.0..=selection.1]
                        .iter()
                        .map(|r| &text[r.clone()])
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                if editor::line_menu_button(ui, label).clicked() {
                    ui.ctx().copy_text(copied());
                    ui.close_menu();
                }
                if editor::line_menu_button(ui, t.cut).clicked() {
                    ui.ctx().copy_text(copied());
                    line_action = Some(selection);
                    ui.close_menu();
                }
                if editor::line_menu_button(ui, t.paste).clicked() {
                    ui.ctx().memory_mut(|memory| memory.request_focus(id));
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                    ui.close_menu();
                }
                if editor::line_menu_button(ui, t.select_all).clicked() {
                    state.large.cursor = Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        egui::text::CCursor::new(state.large.characters),
                    ));
                    state.large.invalidate_window();
                    ui.close_menu();
                }
                ui.separator();
                if editor::line_menu_button(ui, t.clear_content).clicked() {
                    line_action = Some(selection);
                    ui.close_menu();
                }
            }); }
            output.response.context_menu(|ui| {
                let cursor = state.large.cursor.unwrap_or_default();
                let start = cursor.primary.index.min(cursor.secondary.index);
                let end = cursor.primary.index.max(cursor.secondary.index);
                let has_selection = start < end;
                let line = state.large.line_for_char(start);
                let copied = || {
                    if has_selection {
                        let range = state.large.byte_for_char(text, start)
                            ..state.large.byte_for_char(text, end);
                        text[range].to_owned()
                    } else {
                        state.large.ranges.get(line)
                            .map_or_else(String::new, |range| text[range.clone()].to_owned())
                    }
                };
                editor::line_menu_style(ui, t.select_all, t.cut);
                if editor::line_menu_button(
                    ui,
                    if has_selection { t.copy_lines } else { t.copy_line },
                )
                .clicked()
                {
                    ui.ctx().copy_text(copied());
                    ui.close_menu();
                }
                if editor::line_menu_button(ui, t.cut).clicked() {
                    ui.ctx().copy_text(copied());
                    if has_selection {
                        let range = state.large.byte_for_char(text, start)
                            ..state.large.byte_for_char(text, end);
                        let after = Some(egui::text::CCursorRange::one(
                            egui::text::CCursor::new(start),
                        ));
                        state.large.record(text, range, String::new(), Some(cursor), after);
                        state.large.cursor = after;
                        changed = true;
                    } else {
                        line_action = Some((line, line));
                    }
                    ui.close_menu();
                }
                if editor::line_menu_button(ui, t.paste).clicked() {
                    ui.ctx().memory_mut(|memory| memory.request_focus(id));
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                    ui.close_menu();
                }
                if editor::line_menu_button(ui, t.select_all).clicked() {
                    state.large.cursor = Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        egui::text::CCursor::new(state.large.characters),
                    ));
                    state.large.invalidate_window();
                    ui.close_menu();
                }
                ui.separator();
                if let Some(stamp) = crate::insert_time::submenu(ui, t.insert_time, t.unix_timestamp) {
                    // Insert the local time at the caret, replacing any
                    // selection, as one undoable edit of the large buffer.
                    ui.ctx().memory_mut(|memory| memory.request_focus(id));
                    let start_byte = state.large.byte_for_char(text, start);
                    let end_byte = state.large.byte_for_char(text, end);
                    let after = Some(egui::text::CCursorRange::one(
                        egui::text::CCursor::new(start + stamp.chars().count()),
                    ));
                    state.large.record(text, start_byte..end_byte, stamp, Some(cursor), after);
                    state.large.cursor = after;
                    changed = true;
                    ui.close_menu();
                }
                let jump_range = if has_selection {
                    Some(
                        state.large.byte_for_char(text, start)
                            ..state.large.byte_for_char(text, end),
                    )
                } else {
                    editor::word_bytes_at(text, start)
                };
                let next_jump = jump_range.as_ref().and_then(|range| {
                    editor::next_occurrence(text, &text[range.clone()], range.clone())
                });
                let jump = ui.add_enabled(
                    next_jump.is_some(),
                    egui::Button::new(egui::RichText::new(t.jump_next).size(13.0))
                        .wrap_mode(egui::TextWrapMode::Extend),
                );
                if jump.clicked() && let Some(next) = next_jump {
                    // The menu owns focus; give it back so the caret is drawn
                    // at the end of the new match.
                    ui.ctx().memory_mut(|memory| memory.request_focus(id));
                    state.large.cursor = Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(next.start),
                        egui::text::CCursor::new(next.end),
                    ));
                    state.pending_selection = Some(next);
                    state.selection = None;
                    ui.ctx().request_repaint();
                    ui.close_menu();
                }
            });
            if let Some(line) = depth_action {
                let before = state.large.visual_starts[line];
                state.folds.toggle_depth(line);
                log::info!(target: "litetext::fold", "Toggle depth: tab_id={file_id}; line={}; collapsed={}", line + 1, state.folds.collapsed(line));
                state.large.fold_index(&state.folds);
                let delta = (state.large.visual_starts[line] as f32 - before as f32) * row_height;
                let height = 24.0 + state.large.visual_starts.last().copied().unwrap_or(0) as f32 * row_height;
                let offset = editor::depth_scroll_offset(viewport, delta, height);
                depth_offset = Some(offset);
                state.fold_scroll_extent = 0.0;
                let start = state.large.char_starts[line];
                state.pending_selection = Some(start..start);
                ui.ctx().request_repaint();
            }
        });
    let scroll_output = editor::finish_fold_scroll(ui.ctx(), scroll_output, depth_offset.or((fold_pointer || fold_selection).then_some(viewport_offset)));
    #[cfg(test)]
    ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("editor_scroll_geometry", file_id)), (scroll_output.id, scroll_output.state.offset, scroll_output.inner_rect)));
    #[cfg(test)]
    ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("editor_content_geometry", file_id)), (gutter, scroll_output.content_size)));
    #[cfg(not(test))]
    let _ = scroll_output;
    if let Some(selection) = line_action {
        state.large.clear_lines(text, selection);
        changed = true;
    }
    for (range, inserted) in state.large.tool_edits.drain(..) {
        state.tools.apply_edit(range, inserted);
    }
    ui.painter().line_segment(
        [gutter.right_top(), gutter.right_bottom()],
        Stroke::new(1.0_f32, pal.text_faint.gamma_multiply(0.55)),
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
    state.cursor_line = state
        .large
        .cursor
        .map_or(0, |c| state.large.line_for_char(c.primary.index));
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sliced_wrapping_eventually_has_exact_rows_and_never_caches_placeholders() {
        let ctx = egui::Context::default();
        let text: String = (0..2000).map(|line| format!("{line} {}\n", "value ".repeat(line % 24 + 1))).collect();
        let mut state = State::default();
        state.prepare_loaded(&text);
        for _ in 0..400 {
            let _ = ctx.run(Default::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| state.visual_index(ui, &text, &FontId::monospace(13.5), &crate::DARK, 250.0, true));
            });
            if !state.wrap_incomplete { break; }
            assert!(state.wrap_key.is_none());
        }
        assert!(!state.wrap_incomplete);
        assert!(state.visual_rows.iter().all(|rows| *rows > 0));
        assert_eq!(state.visual_starts.last().copied(), Some(state.visual_rows.iter().sum()));
        assert!(state.visual_rows.iter().any(|rows| *rows > 1));
    }

    #[test]
    fn incremental_newline_counts_match_full_scan_through_crlf_boundary_edits_and_undo() {
        for source in ["\r\n\n中🙂\r\nend", "\n\r\nx\n", "", "abc\r"] {
            for start in 0..=source.len() {
                if !source.is_char_boundary(start) { continue; }
                for end in start..=source.len() {
                    if !source.is_char_boundary(end) { continue; }
                    for inserted in ["", "\r", "\n", "\r\n", "中🙂\n"] {
                        let mut text = source.to_owned();
                        let mut state = State::default();
                        state.index(&text);
                        state.record(&mut text, start..end, inserted.into(), None, None);
                        assert_eq!(state.newlines, crate::line_endings::Counts::of(&text), "{source:?} {start}..{end} {inserted:?}");
                        state.undo(&mut text, false);
                        assert_eq!(text, source);
                        assert_eq!(state.newlines, crate::line_endings::Counts::of(source));
                    }
                }
            }
        }
    }

    #[test]
    fn zoom_rebuilds_wrapped_visual_index_without_changing_document_or_cursor() {
        let text = "中文 hello world 🙂 ".repeat(60) + "\nlast";
        let mut state = State::default();
        state.index(&text);
        let ctx = egui::Context::default();
        let mut row_counts = Vec::new();
        for size in [10.0, 32.0, 10.0] {
            let _ = ctx.run(Default::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.visual_index(
                        ui,
                        &text,
                        &FontId::monospace(size),
                        &crate::DARK,
                        400.0,
                        true,
                    );
                    row_counts.push(state.visual_starts.last().copied().unwrap());
                });
            });
        }
        assert!(row_counts[0] < row_counts[1]);
        assert_eq!(row_counts[0], row_counts[2]);
        assert_eq!(state.characters, text.chars().count());
        assert_eq!(state.revision, 0);
    }

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
        let mut state = State { undo_limit: 500, ..Default::default() };
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
    fn configurable_large_history_limits_records() {
        let mut text = "abc".to_owned();
        let mut state = State { undo_limit: 3, ..Default::default() }; state.index(&text);
        for _ in 0..10 { state.record(&mut text, 0..0, "x".into(), None, None); }
        assert_eq!(state.undo.len(), 3);
        for _ in 0..3 { assert!(state.undo(&mut text, false)); }
        assert!(!state.undo(&mut text, false));
    }

    #[test]
    fn delta_history_does_not_retain_full_documents_and_branch_revisions_are_unique() {
        let mut text = "line 中文🙂\r\n".repeat(100_000);
        let mut state = State { undo_limit: 100, ..Default::default() };
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

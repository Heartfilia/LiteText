//! Lossless JSON whitespace formatting and display-only, logical-line folding.
use crate::{Lang, LiteTextApp, ViewMode, text_tools};
use eframe::egui;
use std::{collections::BTreeSet, sync::Arc};

/// Validate without materializing a Value (which would lose duplicate keys and
/// number spellings), then copy every non-whitespace token verbatim.
pub(crate) fn format_json(input: &str) -> Result<String, serde_json::Error> {
    let source = input.strip_prefix('\u{feff}').unwrap_or(input);
    let mut deserializer = serde_json::Deserializer::from_str(source);
    serde::Deserialize::deserialize(&mut deserializer).map(|_: serde::de::IgnoredAny| ())?;
    deserializer.end()?;
    let newline = if input.contains("\r\n") { "\r\n" } else { "\n" };
    let tokens: Vec<char> = source.chars().collect();
    let mut output = String::with_capacity(source.len());
    let mut depth = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    let mut empty = Vec::new();
    let line = |out: &mut String, depth: usize| {
        out.push_str(newline);
        for _ in 0..depth {
            out.push_str("  ");
        }
    };
    if input.starts_with('\u{feff}') {
        output.push('\u{feff}');
    }
    for (index, &ch) in tokens.iter().enumerate() {
        if quoted {
            output.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                quoted = false;
            }
            continue;
        }
        match ch {
            '"' => {
                quoted = true;
                output.push(ch);
            }
            '{' | '[' => {
                output.push(ch);
                depth += 1;
                let close = if ch == '{' { '}' } else { ']' };
                let is_empty = tokens[index + 1..]
                    .iter()
                    .find(|c| !c.is_ascii_whitespace())
                    == Some(&close);
                empty.push(is_empty);
                if !is_empty {
                    line(&mut output, depth);
                }
            }
            '}' | ']' => {
                depth -= 1;
                if !empty.pop().unwrap_or(true) {
                    line(&mut output, depth);
                }
                output.push(ch);
            }
            ',' => {
                output.push(ch);
                line(&mut output, depth);
            }
            ':' => output.push_str(": "),
            ' ' | '\t' | '\r' | '\n' => {}
            _ => output.push(ch),
        }
    }
    if input.ends_with('\n') {
        output.push_str(newline);
    }
    Ok(output)
}

impl LiteTextApp {
    pub(crate) fn format_active_json(&mut self, ctx: &egui::Context) {
        let zh = self.lang == Lang::ZhCn;
        let result = self.active_mut().ok_or_else(|| if zh {
            "请先打开一个文本工作区".to_owned()
        } else { "Open a text workspace first".to_owned() }).and_then(|file| {
            if file.view != ViewMode::Text || file.draft_text.is_none() {
                return Err(if zh { "当前工作区不是可编辑的文本，请切换到文本视图" } else { "Switch to an editable text workspace" }.to_owned());
            }
            let text = file.draft_text.as_ref().unwrap();
            let formatted = format_json(text).map_err(|error| if zh {
                format!("JSON 格式化失败：第 {} 行，第 {} 列\n{error}\n原内容未更改。", error.line(), error.column())
            } else { format!("JSON formatting failed at line {}, column {}\n{error}\nThe original content was not changed.", error.line(), error.column()) })?;
            if &formatted != text {
                let length = text.len();
                text_tools::replace_range_with_cursor(file, ctx, 0..length, formatted, Some(egui::text::CCursorRange::default()));
            }
            file.editor_state.selection = None;
            file.editor_state.pending_selection = Some(0..0);
            file.editor_state.folds.enabled = true;
            file.language_override = Some("JSON".to_owned());
            file.editor_state.folds.invalidate();
            Ok(file.id)
        });
        match result {
            Ok(id) => {
                self.focus_editor = Some(id);
                self.plugin_error = None;
            }
            Err(error) => self.plugin_error = Some(error),
        }
        ctx.request_repaint();
    }

    pub(crate) fn plugin_error_window(&mut self, ctx: &egui::Context) {
        let Some(error) = self.plugin_error.clone() else {
            return;
        };
        let mut open = true;
        egui::Window::new(if self.lang == Lang::ZhCn { "文本工具" } else { "Text tools" })
            .id(egui::Id::new("plugin_error"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(error);
                if ui.button(crate::tr(self.lang).done).clicked() {
                    self.plugin_error = None;
                }
            });
        if !open || ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.plugin_error = None;
        }
    }
}

pub(crate) const FOLD_WIDTH: f32 = 20.0;

#[derive(Clone, Debug)]
pub(crate) struct Block {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

#[derive(Clone, Default)]
pub(crate) struct Folds {
    pub(crate) enabled: bool,
    key: Option<(u64, u64)>,
    pub(crate) blocks: Vec<Block>,
    collapsed: BTreeSet<usize>,
    hidden: Vec<bool>,
    internal_selection: bool,
    pub(crate) generation: u64,
}

impl Folds {
    pub(crate) fn invalidate(&mut self) {
        self.key = None;
    }

    pub(crate) fn sync(&mut self, text: &str, key: (u64, u64)) {
        if !self.enabled || self.key == Some(key) {
            return;
        }
        self.key = Some(key);
        self.blocks.clear();
        self.collapsed.clear();
        self.hidden = vec![false; text.bytes().filter(|&c| c == b'\n').count() + 1];
        let mut stack = Vec::new();
        let (mut line, mut quoted, mut escaped) = (0, false, false);
        for ch in text.chars() {
            if quoted {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    quoted = false;
                }
            } else {
                match ch {
                    '"' => quoted = true,
                    '{' | '[' => stack.push((ch, line)),
                    '}' | ']' => {
                        let Some((open, start)) = stack.pop() else {
                            self.blocks.clear();
                            break;
                        };
                        if (open == '{') != (ch == '}') {
                            self.blocks.clear();
                            break;
                        }
                        if start < line {
                            self.blocks.push(Block { start, end: line });
                        }
                    }
                    _ => {}
                }
            }
            if ch == '\n' {
                line += 1;
            }
        }
        if !stack.is_empty() || quoted {
            self.blocks.clear();
        }
        self.blocks.sort_by_key(|b| b.start);
        self.generation += 1;
    }

    pub(crate) fn hidden(&self, line: usize) -> bool {
        self.hidden.get(line).copied().unwrap_or(false)
    }
    pub(crate) fn collapsed(&self, line: usize) -> bool {
        self.collapsed.contains(&line)
    }
    pub(crate) fn width(&self) -> f32 {
        if self.enabled { FOLD_WIDTH } else { 0.0 }
    }

    pub(crate) fn expand_all(&mut self) {
        if self.collapsed.is_empty() {
            return;
        }
        self.collapsed.clear();
        self.hidden.fill(false);
        self.generation += 1;
    }

    pub(crate) fn prepare_input(&mut self, ui: &egui::Ui, pending: bool, id: egui::Id) {
        let internal = std::mem::take(&mut self.internal_selection);
        let keyboard = ui.is_enabled()
            && ui.memory(|m| m.has_focus(id))
            && ui.input(|i| {
                i.events.iter().any(|e| {
                    matches!(
                        e,
                        egui::Event::Text(_)
                            | egui::Event::Paste(_)
                            | egui::Event::Cut
                            | egui::Event::Key { pressed: true, .. }
                    )
                })
            });
        if (pending && !internal) || keyboard {
            self.expand_all();
        }
    }

    pub(crate) fn toggle(&mut self, line: usize) {
        if !self.collapsed.remove(&line) {
            self.collapsed.insert(line);
        }
        self.hidden.fill(false);
        for block in &self.blocks {
            if self.collapsed.contains(&block.start) {
                self.hidden[block.start + 1..=block.end].fill(true);
            }
        }
        self.internal_selection = true;
        self.generation += 1;
    }

    pub(crate) fn paint_summaries(
        &self,
        ui: &egui::Ui,
        galley: &egui::Galley,
        pos: egui::Pos2,
        first_line: usize,
        pal: &crate::Palette,
    ) {
        let mut line = first_line;
        for row in &galley.rows {
            if !self.hidden(line) && self.collapsed(line) && row.ends_with_newline {
                ui.painter().text(
                    pos + row.rect.right_center().to_vec2() + egui::vec2(8.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    "…",
                    egui::FontId::monospace(13.5),
                    pal.text_faint,
                );
            }
            if row.ends_with_newline {
                line += 1;
            }
        }
    }

    /// Retain original glyph/character counts so selection, copy, search and undo
    /// use document coordinates. Hidden rows have no mesh and consume no height.
    pub(crate) fn layout(
        &self,
        mut galley: Arc<egui::Galley>,
        first_line: usize,
    ) -> Arc<egui::Galley> {
        if self.collapsed.is_empty() {
            return galley;
        }
        let layout = Arc::make_mut(&mut galley);
        let (mut line, mut y) = (first_line, 0.0);
        let mut bounds = egui::Rect::NOTHING;
        let mut max_width = 0.0f32;
        for row in &mut layout.rows {
            let height = if self.hidden(line) {
                0.0
            } else {
                row.rect.height()
            };
            let delta = y - row.rect.top();
            for glyph in &mut row.glyphs {
                glyph.pos.y += delta;
            }
            if height == 0.0 {
                row.visuals = Default::default();
            } else {
                for vertex in &mut row.visuals.mesh.vertices {
                    vertex.pos.y += delta;
                }
                row.visuals.mesh_bounds = row.visuals.mesh_bounds.translate(egui::vec2(0.0, delta));
                bounds = bounds.union(row.visuals.mesh_bounds);
                max_width = max_width.max(row.rect.right());
            }
            row.rect.min.y = y;
            row.rect.max.y = y + height;
            y += height;
            if row.ends_with_newline {
                line += 1;
            }
        }
        layout.rect.max = egui::pos2(max_width, y);
        layout.mesh_bounds = bounds;
        layout.num_vertices = layout
            .rows
            .iter()
            .map(|r| r.visuals.mesh.vertices.len())
            .sum();
        layout.num_indices = layout
            .rows
            .iter()
            .map(|r| r.visuals.mesh.indices.len())
            .sum();
        galley
    }
}

/// A fixed, independent control lane between line numbers and text.
pub(crate) fn fold_button(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    file_id: u64,
    line: usize,
    collapsed: bool,
    pal: &crate::Palette,
) -> bool {
    let rect = egui::Rect::from_center_size(rect.center(), egui::vec2(11.0, 11.0));
    let response = ui
        .interact(
            rect,
            egui::Id::new(("json_fold", file_id, line)),
            egui::Sense::click(),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(if collapsed {
            "Expand JSON / 展开"
        } else {
            "Collapse JSON / 折叠"
        });
    let color = if response.hovered() {
        pal.accent
    } else {
        pal.text_faint
    };
    let p = ui.painter_at(rect.expand(1.0));
    p.rect_filled(rect, 1.0, pal.bg_panel);
    p.rect_stroke(
        rect,
        1.0,
        egui::Stroke::new(1.0_f32, color),
        egui::StrokeKind::Inside,
    );
    p.line_segment(
        [
            rect.center() - egui::vec2(3.0, 0.0),
            rect.center() + egui::vec2(3.0, 0.0),
        ],
        egui::Stroke::new(1.0_f32, color),
    );
    if collapsed {
        p.line_segment(
            [
                rect.center() - egui::vec2(0.0, 3.0),
                rect.center() + egui::vec2(0.0, 3.0),
            ],
            egui::Stroke::new(1.0_f32, color),
        );
    }
    ui.ctx()
        .data_mut(|d| d.insert_temp(egui::Id::new(("json_fold_rect", file_id, line)), rect));
    response.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DARK, LIGHT, LiteTextApp, recovery};

    fn raw(events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1080.0, 720.0),
            )),
            events,
            ..Default::default()
        }
    }

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> Vec<egui::Event> {
        [true, false]
            .into_iter()
            .map(|pressed| egui::Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers,
            })
            .collect()
    }

    fn button(pos: egui::Pos2, pressed: bool) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            },
        ]
    }

    #[test]
    fn json_formatting_is_lossless_and_preserves_order_numbers_duplicates_and_strings() {
        let input = r#" {"z":9007199254740993000000000000000000001,"a":1.234567890123456789,"z":1e9999,"s":"[} , : \\\"中🙂","empty":[{},[]],"bool":true,"null":null} "#;
        let formatted = format_json(input).unwrap();
        assert!(formatted.starts_with("{\n  \"z\": 9007199254740993000000000000000000001,"));
        assert!(formatted.contains("\"a\": 1.234567890123456789"));
        assert!(formatted.contains("\"z\": 1e9999"));
        assert!(formatted.contains(r#""s": "[} , : \\\"中🙂""#));
        assert!(formatted.contains("\"empty\": [\n    {},\n    []\n  ]"));
        assert_eq!(format_json(&formatted).unwrap(), formatted);
        for input in ["{}", "[]", "null", "true", "123", "\"hello\""] {
            assert_eq!(format_json(input).unwrap(), input);
        }
        assert_eq!(
            format_json("\u{feff}{\r\n\"a\":1\r\n}\r\n").unwrap(),
            "\u{feff}{\r\n  \"a\": 1\r\n}\r\n"
        );
        for invalid in [
            "",
            "{",
            "{a:1}",
            "{'a':1}",
            "[1,]",
            "{} trailing",
            "[1 2]",
            "{\"a\": NaN}",
        ] {
            assert!(format_json(invalid).is_err(), "{invalid}");
        }
        // Validation is iterative and accepts deeply nested, otherwise valid data.
        let nested = format!("{}0{}", "[".repeat(200), "]".repeat(200));
        assert!(format_json(&nested).is_ok());
    }

    #[test]
    fn format_active_workspace_is_one_undoable_edit_and_failure_keeps_original() {
        for large in [false, true] {
            let ctx = egui::Context::default();
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("original.json");
            let original = format!(
                "{{\"first\":[1,2],\"last\":\"{}\"}}",
                "x".repeat(if large { 300_000 } else { 2 })
            );
            std::fs::write(&path, &original).unwrap();
            let mut app = LiteTextApp {
                recovery_path_override: Some(dir.path().join("session.json")),
                ..Default::default()
            };
            app.new_document();
            let untouched = app.active_id.unwrap();
            app.open_paths(vec![path.clone()]);
            let id = app.active_id.unwrap();
            let run = |app: &mut LiteTextApp, events| {
                ctx.run(raw(events), |ctx| {
                    app.handle_shortcuts(ctx);
                    egui::CentralPanel::default().show(ctx, |ui| {
                        crate::draft_editor(ui, app.active_mut().unwrap(), &DARK, true, Lang::En)
                    });
                })
            };
            run(&mut app, vec![]);
            app.format_active_json(&ctx);
            assert!(app.plugin_error.is_none());
            let formatted = format_json(&original).unwrap();
            assert_eq!(
                app.active().unwrap().draft_text.as_deref(),
                Some(formatted.as_str())
            );
            assert!(app.active().unwrap().has_unsaved_changes());
            assert_eq!(
                app.files
                    .iter()
                    .find(|f| f.id == untouched)
                    .unwrap()
                    .draft_text
                    .as_deref(),
                Some("")
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
            run(&mut app, vec![]);
            run(&mut app, key(egui::Key::Z, egui::Modifiers::COMMAND));
            assert_eq!(
                app.active().unwrap().draft_text.as_deref(),
                Some(original.as_str())
            );
            run(
                &mut app,
                key(
                    egui::Key::Z,
                    egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                ),
            );
            assert_eq!(
                app.active().unwrap().draft_text.as_deref(),
                Some(formatted.as_str())
            );
            assert_eq!(app.active_id, Some(id));
            recovery::save_document(app.active_mut().unwrap(), &path).unwrap();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), formatted);
            let file = app.active_mut().unwrap();
            let len = file.draft_text.as_ref().unwrap().len();
            text_tools::replace_range(file, &ctx, 0..len, "{\n  \"bad\":\n}".to_owned());
            let before = file.draft_text.clone();
            app.format_active_json(&ctx);
            assert_eq!(app.active().unwrap().draft_text, before);
            assert!(app.plugin_error.as_deref().unwrap().contains("第 3 行"));
            assert!(app.shortcuts_blocked());
        }
        let mut app = LiteTextApp::default();
        app.format_active_json(&egui::Context::default());
        assert!(app.plugin_error.is_some());
        app.new_document();
        app.active_mut().unwrap().view = ViewMode::Hex;
        app.format_active_json(&egui::Context::default());
        assert!(app.plugin_error.as_deref().unwrap().contains("文本"));
    }

    #[test]
    fn folding_detects_nested_blocks_without_matching_brackets_inside_strings() {
        let text = format_json(r#"{"s":"[}\\\"","a":[{"nested":1}],"b":[]}"#).unwrap();
        let mut folds = Folds {
            enabled: true,
            ..Default::default()
        };
        folds.sync(&text, (1, 0));
        assert_eq!(folds.blocks.len(), 3);
        let child = folds.blocks[1].start;
        folds.toggle(child);
        assert!(!folds.hidden(child));
        assert!(folds.hidden(child + 1));
        folds.toggle(0);
        assert!(folds.hidden(child));
        folds.toggle(0);
        assert!(!folds.hidden(child));
        assert!(folds.collapsed(child));
        folds.expand_all();
        assert!(folds.hidden.iter().all(|hidden| !hidden));
        folds.toggle(0);
        folds.sync(&text, (2, 0));
        assert!(!folds.collapsed(0));
    }

    #[test]
    fn formatting_across_large_threshold_keeps_a_single_undo_history() {
        let ctx = egui::Context::default();
        let mut app = LiteTextApp::default();
        app.new_document();
        let original = format!("[{}0]", "1,".repeat(60_000));
        assert!(original.len() < crate::large_editor::THRESHOLD);
        app.active_mut().unwrap().draft_text = Some(original.clone());
        app.active_mut().unwrap().sync_draft();
        app.format_active_json(&ctx);
        assert!(app.active().unwrap().editor_state.large.active);
        assert!(
            app.active().unwrap().draft_text.as_ref().unwrap().len()
                >= crate::large_editor::THRESHOLD
        );
        let run = |app: &mut LiteTextApp, events| {
            ctx.run(raw(events), |ctx| {
                app.handle_shortcuts(ctx);
                egui::CentralPanel::default().show(ctx, |ui| {
                    crate::draft_editor(ui, app.active_mut().unwrap(), &DARK, true, Lang::En)
                });
            })
        };
        run(&mut app, vec![]);
        run(&mut app, key(egui::Key::Z, egui::Modifiers::COMMAND));
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some(original.as_str())
        );
    }

    #[test]
    fn many_line_json_folded_root_remains_visible_and_saving_keeps_hidden_data() {
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("folded.json");
        let mut app = LiteTextApp::default();
        app.new_document();
        app.active_mut().unwrap().draft_text =
            Some(format!("[{}0]", "{\"a\":[1,2]},".repeat(20_000)));
        app.active_mut().unwrap().sync_draft();
        app.format_active_json(&ctx);
        let formatted = app.active().unwrap().draft_text.clone().unwrap();
        let id = app.active_id.unwrap();
        let run = |app: &mut LiteTextApp, events| {
            ctx.run(raw(events), |ctx| {
                app.handle_shortcuts(ctx);
                egui::CentralPanel::default().show(ctx, |ui| {
                    crate::draft_editor(ui, app.active_mut().unwrap(), &DARK, false, Lang::En)
                });
            })
        };
        run(&mut app, vec![]);
        run(&mut app, vec![]);
        let rect = ctx
            .data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("json_fold_rect", id, 0usize))))
            .unwrap();
        for pressed in [true, false] {
            run(&mut app, button(rect.center(), pressed));
        }
        for _ in 0..3 {
            let output = run(&mut app, vec![]);
            assert!(app.active().unwrap().editor_state.folds.collapsed(0));
            assert!(output.shapes.iter().any(|s| matches!(&s.shape,
                egui::Shape::Text(text) if text.galley.text().starts_with('[') && text.galley.size().y < 30.0)));
        }
        recovery::save_document(app.active_mut().unwrap(), &path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), formatted);
        assert!(app.active().unwrap().editor_state.folds.collapsed(0));
    }

    #[test]
    fn folding_buttons_hide_rows_keep_data_and_expand_for_edit_search_and_copy() {
        for pal in [&DARK, &LIGHT] {
            for large in [false, true] {
                let ctx = egui::Context::default();
                let mut app = LiteTextApp::default();
                app.new_document();
                let original = format!(
                    "{{\"items\":[{{\"name\":\"中🙂\"}},2],\"tail\":\"{}\"}}",
                    "x".repeat(if large { 300_000 } else { 1 })
                );
                app.active_mut().unwrap().draft_text = Some(original);
                app.active_mut().unwrap().sync_draft();
                app.format_active_json(&ctx);
                let formatted = app.active().unwrap().draft_text.clone().unwrap();
                let id = app.active_id.unwrap();
                let run = |app: &mut LiteTextApp, events| {
                    ctx.run(raw(events), |ctx| {
                        app.handle_shortcuts(ctx);
                        egui::CentralPanel::default().show(ctx, |ui| {
                            crate::draft_editor(ui, app.active_mut().unwrap(), pal, false, Lang::En)
                        });
                    })
                };
                let fold_rect = || {
                    ctx.data(|d| {
                        d.get_temp::<egui::Rect>(egui::Id::new(("json_fold_rect", id, 0usize)))
                    })
                    .unwrap()
                };
                run(&mut app, vec![]);
                let layout = run(&mut app, vec![]);
                let rect = fold_rect();
                // Number, fold control and body occupy separate, non-overlapping columns.
                let text_rect = |matches: fn(&str) -> bool| layout.shapes.iter().find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if matches(text.galley.text()) =>
                        Some(egui::Rect::from_min_size(text.pos, text.galley.size())),
                    _ => None,
                }).unwrap();
                let number = text_rect(|s| s == "1");
                let body = text_rect(|s| s.starts_with('{'));
                assert!(number.right() < rect.left() && rect.right() < body.left());
                for pressed in [true, false] {
                    run(&mut app, button(rect.center(), pressed));
                }
                let output = run(&mut app, vec![]);
                assert!(app.active().unwrap().editor_state.folds.collapsed(0));
                assert_eq!(
                    app.active().unwrap().draft_text.as_deref(),
                    Some(formatted.as_str())
                );
                let galley = output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) if text.galley.text().starts_with('{') => {
                            Some(text.galley.clone())
                        }
                        _ => None,
                    })
                    .unwrap();
                assert!(
                    galley
                        .rows
                        .iter()
                        .skip(1)
                        .all(|r| r.rect.height() == 0.0 && r.visuals.mesh.is_empty())
                );
                assert!(galley.size().y < 30.0);
                for pressed in [true, false] {
                    run(&mut app, button(fold_rect().center(), pressed));
                }
                run(&mut app, vec![]);
                assert!(!app.active().unwrap().editor_state.folds.collapsed(0));
                // Collapse again, then navigate to a hidden search result.
                for pressed in [true, false] {
                    run(&mut app, button(fold_rect().center(), pressed));
                }
                run(&mut app, vec![]);
                assert!(app.active().unwrap().editor_state.folds.collapsed(0));
                app.active_mut().unwrap().editor_state.pending_selection = Some(30..32);
                run(&mut app, vec![]);
                assert!(!app.active().unwrap().editor_state.folds.collapsed(0));
                for pressed in [true, false] {
                    run(&mut app, button(fold_rect().center(), pressed));
                }
                run(&mut app, vec![]);
                let editor_id = egui::Id::new(if large {
                    "large_editor"
                } else {
                    "draft_editor"
                })
                .with(id);
                ctx.memory_mut(|m| m.request_focus(editor_id));
                run(&mut app, key(egui::Key::A, egui::Modifiers::COMMAND));
                let output = run(&mut app, vec![egui::Event::Copy]);
                assert!(output.platform_output.commands.iter().any(|command| matches!(command, egui::OutputCommand::CopyText(value) if value == &formatted)));
                assert!(!app.active().unwrap().editor_state.folds.collapsed(0));
                assert_eq!(
                    app.active().unwrap().draft_text.as_deref(),
                    Some(formatted.as_str())
                );
            }
        }
    }

    #[test]
    fn plugin_menu_opens_on_hover_formats_current_tab_and_width_fits_labels() {
        for lang in [Lang::ZhCn, Lang::En] {
            let ctx = egui::Context::default();
            crate::setup_fonts(&ctx);
            let mut app = LiteTextApp {
                lang,
                show_menu: true,
                ..Default::default()
            };
            app.new_document();
            app.active_mut().unwrap().draft_text = Some("{\"ok\":[1,2]}".to_owned());
            app.active_mut().unwrap().sync_draft();
            let anchor = egui::Rect::from_min_size(egui::pos2(1030.0, 7.0), egui::vec2(28.0, 24.0));
            let run = |app: &mut LiteTextApp, events| {
                ctx.run(raw(events), |ctx| {
                    crate::apply_style(ctx, &DARK);
                    app.app_menu(ctx, anchor, &DARK);
                })
            };
            run(&mut app, vec![]);
            let output = run(&mut app, vec![]);
            let label = format!("‹  {}", crate::tr(lang).plugins);
            let pos = output
                .shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::Shape::Text(text) if text.galley.text() == label => {
                        Some(text.pos + egui::vec2(8.0, 8.0))
                    }
                    _ => None,
                })
                .unwrap();
            run(&mut app, vec![egui::Event::PointerMoved(pos)]);
            let output = run(&mut app, vec![egui::Event::PointerMoved(pos)]);
            assert!(app.menu_plugins_open);
            assert!(!app.menu_recent_open);
            let plugin = output
                .shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::Shape::Text(text) if text.galley.text() == "Format JSON" => {
                        Some(text.pos + egui::vec2(8.0, 8.0))
                    }
                    _ => None,
                })
                .unwrap();
            assert!(plugin.x < pos.x);
            for pressed in [true, false] {
                run(&mut app, button(plugin, pressed));
            }
            assert!(!app.show_menu);
            assert_eq!(
                app.active().unwrap().draft_text.as_deref(),
                Some("{\n  \"ok\": [\n    1,\n    2\n  ]\n}")
            );
            assert!(app.active().unwrap().editor_state.folds.enabled);
            let _ = ctx.run(raw(vec![]), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let labels = ["a", "Longest menu label"];
                    let width = crate::app_menu_width(ui, &labels, &DARK);
                    let longest = ui
                        .painter()
                        .layout_no_wrap(
                            labels[1].to_owned(),
                            egui::FontId::proportional(13.0),
                            DARK.text_main,
                        )
                        .size()
                        .x;
                    assert_eq!(width, longest + 2.0 * crate::app_menu_padding(ui, &DARK));
                });
            });
        }
    }
}

use crate::{Palette, i18n::Strings};
use eframe::egui;
use egui::{FontId, Stroke};
use std::ops::Range;

const GUTTER_NUMBER_GAP: f32 = 2.0;
const EDITOR_LEFT_PADDING: f32 = 4.0;

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
    pub(crate) pending_selection: Option<Range<usize>>,
    pub(crate) changes: crate::changes::Changes,
    pub(crate) folds: crate::json_plugin::Folds,
    pub(crate) syntax_cache: std::sync::Arc<std::sync::Mutex<crate::workspace_theme::SyntaxCache>>,
}

impl EditorState {
    pub(crate) fn invalidate(&mut self) {
        self.large.indexed_len = None;
        self.large.invalidate_window();
    }
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
    ui.style_mut().spacing.scroll = egui::style::ScrollStyle::solid();
    let font = FontId::monospace(13.5);
    let tools = state.tools.clone();
    if let Some(render) = &tools.workspace {
        render.apply_style(ui);
    }
    let colors = tools.workspace.as_ref().map(|r| r.colors);
    let before_layout = (!tools.marks.is_empty() || tools.current.is_some()).then(|| text.clone());
    let line_count = text.bytes().filter(|&b| b == b'\n').count() + 1;
    // Use the rounded glyph advance from a multi-digit galley, not a single glyph's width.
    let probe = ui.painter().layout_no_wrap(
        "0000000000".to_owned(),
        font.clone(),
        colors.map_or(pal.text_faint, |c| c.gutter),
    );
    let digit_w = probe
        .rows
        .first()
        .and_then(|row| (row.glyphs.len() >= 2).then(|| row.glyphs[1].pos.x - row.glyphs[0].pos.x))
        .unwrap_or(8.0);
    let number_w = gutter_columns(line_count) as f32 * digit_w + GUTTER_NUMBER_GAP;
    let gutter_w = number_w + fold_w;
    let gutter = egui::Rect::from_min_max(
        bounds.min,
        egui::pos2(bounds.left() + gutter_w, bounds.bottom()),
    );
    ui.painter()
        .rect_filled(gutter, 0.0, colors.map_or(pal.bg_panel, |c| c.background));
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
                        bounds.max,
                    )));
                    egui::TextEdit::multiline(text)
                        .id(editor_id)
                        .font(font.clone())
                        .text_color(colors.map_or(pal.text_main, |c| c.text))
                        .frame(false)
                        .margin(egui::Margin::ZERO)
                        .desired_width(editor_rect.width())
                        .min_size(editor_rect.size())
                        .layouter(&mut layouter)
                        .lock_focus(true)
                        .show(ui)
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
                        font.clone(),
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
                    .rect_filled(lane, 0.0, colors.map_or(pal.bg_panel, |c| c.background));
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
                if menu.button(label).clicked() {
                    menu.ctx().copy_text(copy_lines(text, selection));
                    menu.close_menu();
                }
                menu.separator();
                if menu.button(t.clear_content).clicked() {
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
        assert!(first < 20.0, "空白页不再保留 54px 的宽行号栏");
        assert_eq!(ten, first);
        assert!(hundred > ten);
        harness.text = "\n".repeat(999);
        assert_eq!(harness.row_pos(0).x, hundred);
        harness.text.clear();
        assert_eq!(harness.row_pos(0).x, first);
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
    }

    impl Harness {
        fn new(text: &str) -> Self {
            let mut harness = Self {
                ctx: egui::Context::default(),
                text: text.to_owned(),
                state: EditorState::default(),
                frame: 0,
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
                            show(
                                ui,
                                &mut self.text,
                                &mut self.state,
                                1,
                                &crate::DARK,
                                false,
                                crate::tr(crate::Lang::En),
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
        let result = harness.click_menu(menu, "Copy Line Contents");
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

//! Text-only controls, range-based highlighters and Unicode/regex find/replace.
use crate::Palette;
use eframe::egui;
use std::ops::Range;
#[path = "background_search.rs"]
pub(crate) mod background_search;

pub(crate) fn toolbar(
    ui: &mut egui::Ui,
    file: &mut crate::LoadedFile,
    pal: &Palette,
    t: &crate::i18n::Strings,
    rendered: bool,
) {
    // Wrap, whitespace and the marker pen only affect the source TextEdit.
    // Rendered Markdown (live / split) keeps their icons visible but disabled;
    // find & replace stays available in every mode.
    if rendered {
        file.editor_state.tools.pen_menu = false;
    }
    // Keep the text-tool group in the same visual order in both layouts:
    // wrap, whitespace, pen, find. Only the surrounding toolbar is mirrored.
    let mut tools = [
        (t.find_replace, file.editor_state.tools.find_open, 0),
        (t.mark, file.editor_state.tools.pen.is_some(), 1),
        (t.whitespace, file.editor_state.tools.whitespace, 2),
        (t.word_wrap, file.editor_state.tools.wrap, 3),
    ];
    if ui.layout().main_dir() == egui::Direction::LeftToRight {
        tools.reverse();
    }
    for (hint, active, action) in tools {
        let disabled = rendered && action != 0;
        let response = if disabled {
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::hover());
            // Visible but dimmed: the icon stays, the control cannot be used.
            paint_icon(
                ui.painter(),
                rect,
                pal.text_faint.gamma_multiply(0.35),
                action,
            );
            response
        } else {
            crate::toolbar_toggle(ui, active, hint, pal, |p, rect, color| {
                let color = if action == 1 {
                    file.editor_state.tools.pen.map(pen_color).unwrap_or(color)
                } else {
                    color
                };
                paint_icon(p, rect, color, if action == 1 && file.editor_state.tools.pen == Some(CONCEAL) { 10 } else { action });
            })
        };
        ui.ctx().data_mut(|d| {
            d.insert_temp(
                egui::Id::new(("text_tool_rect", file.id, action)),
                response.rect,
            )
        });
        if response.clicked() && !disabled {
            match action {
                0 => {
                    let tools = &mut file.editor_state.tools;
                    tools.find_open = !tools.find_open;
                    // Reopening starts from the compact find-only layout.
                    if !tools.find_open {
                        tools.replace_open = false;
                    }
                },
                1 => file.editor_state.tools.pen_menu = !file.editor_state.tools.pen_menu,
                2 => file.editor_state.tools.whitespace = !file.editor_state.tools.whitespace,
                _ => file.editor_state.tools.wrap = !file.editor_state.tools.wrap,
            }
        }
        if action == 1 && !disabled && file.editor_state.tools.pen_menu {
            let popup = egui::Area::new(egui::Id::new(("pen_menu", file.id)))
                .order(egui::Order::Foreground)
                .pivot(egui::Align2::RIGHT_TOP)
                .fixed_pos(response.rect.right_bottom() + egui::vec2(0.0, 6.0))
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
                            for (index, name) in t.marker_colors.iter().enumerate() {
                                ui.horizontal(|ui| {
                                    let selected = file.editor_state.tools.pen == Some(index);
                                    let button = marker_icon_button(
                                        ui,
                                        if index == CONCEAL { 10 } else { 1 },
                                        name,
                                        pen_color(index),
                                        selected,
                                        egui::vec2(40.0, 24.0),
                                        pal,
                                    );
                                    ui.ctx().data_mut(|d| {
                                        d.insert_temp(
                                            egui::Id::new(("pen_color_rect", file.id, index)),
                                            button.rect,
                                        )
                                    });
                                    if button.clicked() {
                                        file.editor_state.tools.pen =
                                            if selected { None } else { Some(index) };
                                        file.editor_state.tools.pen_menu = false;
                                        if let Some(text) = file.draft_text.as_deref() {
                                            file.editor_state.tools.mark_selection(text, file.editor_state.cursor);
                                        }
                                    }
                                    let clear = marker_icon_button(
                                        ui,
                                        6,
                                        &format!("{} · {}", t.clear_color, name),
                                        pen_color(index),
                                        false,
                                        egui::vec2(26.0, 24.0),
                                        pal,
                                    );
                                    ui.ctx().data_mut(|d| {
                                        d.insert_temp(
                                            egui::Id::new(("pen_clear_rect", file.id, index)),
                                            clear.rect,
                                        )
                                    });
                                    if clear.clicked() {
                                        file.editor_state
                                            .tools
                                            .marks
                                            .retain(|mark| mark.color != index);
                                    }
                                });
                            }
                            ui.separator();
                            let clear_all = ui
                                .with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                                    marker_icon_button(
                                        ui,
                                        9,
                                        t.clear_marks,
                                        pal.text_main,
                                        false,
                                        egui::vec2(28.0, 28.0),
                                        pal,
                                    )
                                })
                                .inner;
                            ui.ctx().data_mut(|d| {
                                d.insert_temp(
                                    egui::Id::new(("pen_clear_all_rect", file.id)),
                                    clear_all.rect,
                                )
                            });
                            if clear_all.clicked() {
                                file.editor_state.tools.marks.clear();
                                file.editor_state.tools.pen = None;
                            }
                        });
                    });
                });
            ui.ctx().data_mut(|d| {
                d.insert_temp(
                    egui::Id::new(("pen_menu_rect", file.id)),
                    popup.response.rect,
                )
            });
            if ui.input(|i| {
                i.key_pressed(egui::Key::Escape)
                    || (i.pointer.any_pressed()
                        && i.pointer.interact_pos().is_some_and(|p| {
                            !popup.response.rect.contains(p) && !response.rect.contains(p)
                        }))
            }) {
                file.editor_state.tools.pen_menu = false;
            }
        }
    }
}

fn paint_icon(p: &egui::Painter, rect: egui::Rect, color: egui::Color32, icon: usize) {
    paint_icon_scaled(p, rect, color, icon, 1.0);
}

fn paint_icon_scaled(
    p: &egui::Painter,
    rect: egui::Rect,
    color: egui::Color32,
    icon: usize,
    scale: f32,
) {
    let c = rect.center();
    let point = |x: f32, y: f32| c + egui::vec2(x, y) * scale;
    let stroke = egui::Stroke::new(1.4 * scale, color);
    let line = |a: [f32; 2], b: [f32; 2]| {
        p.line_segment([point(a[0], a[1]), point(b[0], b[1])], stroke);
    };
    match icon {
        0 => {
            // Magnifier + replace arrow.
            p.circle_stroke(point(-2.0, -2.0), 4.5 * scale, stroke);
            line([1.5, 1.5], [6.0, 6.0]);
            line([-7.0, 6.0], [-1.0, 6.0]);
            line([-3.0, 4.0], [-1.0, 6.0]);
            line([-3.0, 8.0], [-1.0, 6.0]);
        }
        1 => {
            p.add(egui::Shape::closed_line(
                vec![
                    point(-5.0, 2.0),
                    point(2.0, -5.0),
                    point(6.0, -1.0),
                    point(-1.0, 6.0),
                ],
                stroke,
            ));
            line([-6.0, 7.0], [-1.0, 7.0]);
        }
        10 => {
            // Solid nib/body distinguishes the opaque cover from translucent pens.
            p.add(egui::Shape::convex_polygon(vec![
                point(-5.0, 2.0), point(2.0, -5.0), point(6.0, -1.0), point(-1.0, 6.0),
            ], color, egui::Stroke::NONE));
            p.rect_filled(egui::Rect::from_min_max(point(-6.0, 6.0), point(0.0, 8.0)), 0.5, color);
        }
        2 => {
            p.text(
                c,
                egui::Align2::CENTER_CENTER,
                "¶",
                egui::FontId::proportional(15.0 * scale),
                color,
            );
        }
        3 => {
            line([-7.0, -5.0], [7.0, -5.0]);
            line([-7.0, 0.0], [3.0, 0.0]);
            p.add(egui::Shape::line(
                vec![
                    point(4.0, 0.0),
                    point(7.0, 0.0),
                    point(7.0, 5.0),
                    point(-1.0, 5.0),
                ],
                stroke,
            ));
            line([1.0, 3.0], [-1.0, 5.0]);
            line([1.0, 7.0], [-1.0, 5.0]);
            line([-7.0, 5.0], [-4.0, 5.0]);
        }
        4 | 5 => {
            line([-6.0, -4.0], [5.0, -4.0]);
            line([2.0, -7.0], [5.0, -4.0]);
            line([2.0, -1.0], [5.0, -4.0]);
            line([6.0, 4.0], [-5.0, 4.0]);
            line([-2.0, 1.0], [-5.0, 4.0]);
            line([-2.0, 7.0], [-5.0, 4.0]);
            if icon == 5 {
                p.circle_filled(point(8.0, -7.0), 1.5 * scale, color);
            }
        }
        6 => {
            // Eraser, used for clearing one marker color.
            p.add(egui::Shape::closed_line(
                vec![
                    point(-6.0, 2.0),
                    point(1.0, -5.0),
                    point(6.0, 0.0),
                    point(-1.0, 7.0),
                    point(-3.0, 7.0),
                ],
                stroke,
            ));
            line([-3.0, -1.0], [2.0, 4.0]);
            line([-3.0, 7.0], [7.0, 7.0]);
        }
        7 | 8 => {
            // Previous/next match, deliberately horizontal.
            let direction = if icon == 7 { -1.0 } else { 1.0 };
            line([-6.0 * direction, 0.0], [6.0 * direction, 0.0]);
            line([2.0 * direction, -4.0], [6.0 * direction, 0.0]);
            line([2.0 * direction, 4.0], [6.0 * direction, 0.0]);
        }
        9 => {
            // Trash bin: lid, handle, tapered body and two inner slots.
            line([-7.0, -4.0], [7.0, -4.0]);
            p.add(egui::Shape::line(
                vec![
                    point(-3.0, -4.0),
                    point(-3.0, -7.0),
                    point(3.0, -7.0),
                    point(3.0, -4.0),
                ],
                stroke,
            ));
            p.add(egui::Shape::line(
                vec![
                    point(-5.0, -4.0),
                    point(-4.0, 7.0),
                    point(4.0, 7.0),
                    point(5.0, -4.0),
                ],
                stroke,
            ));
            line([-2.0, -1.0], [-2.0, 4.0]);
            line([2.0, -1.0], [2.0, 4.0]);
        }
        _ => {}
    }
}

/// Compact, color-coded controls for the marker palette, without visible labels.
fn marker_icon_button(
    ui: &mut egui::Ui,
    icon: usize,
    hint: &str,
    color: egui::Color32,
    selected: bool,
    size: egui::Vec2,
    pal: &Palette,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), selected, hint)
    });
    ui.painter().rect_filled(
        rect,
        4.0,
        if selected {
            color.gamma_multiply(0.2)
        } else {
            pal.bg_elevated
        },
    );
    if selected || response.hovered() || response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            4.0,
            egui::Stroke::new(1.0_f32, color.gamma_multiply(if selected { 0.8 } else { 0.4 })),
            egui::StrokeKind::Inside,
        );
    }
    paint_icon_scaled(ui.painter(), rect, color, icon, size.min_elem() / 24.0);
    response
        .on_hover_text(hint)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn icon_button(ui: &mut egui::Ui, icon: usize, hint: &str, pal: &Palette) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(26.0, 24.0), egui::Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, 4.0, pal.bg_elevated);
    }
    paint_icon(ui.painter(), rect, pal.text_main, icon);
    response.on_hover_text(hint)
}

/// Inline search options, not standalone raised buttons: quiet until hovered or enabled.
fn search_option(
    ui: &mut egui::Ui,
    label: &str,
    hint: &str,
    selected: bool,
    pal: &Palette,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(24.0, 20.0), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            ui.is_enabled(),
            selected,
            label,
        )
    });
    if selected {
        ui.painter()
            .rect_filled(rect.shrink(1.0), 3.0, pal.accent.gamma_multiply(0.18));
        ui.painter().rect_stroke(
            rect.shrink(1.0),
            3.0,
            egui::Stroke::new(1.0_f32, pal.accent.gamma_multiply(0.5)),
            egui::StrokeKind::Inside,
        );
    } else if response.hovered() {
        ui.painter()
            .rect_filled(rect.shrink(1.0), 3.0, pal.bg_elevated);
    }
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(11.0),
        if selected { pal.accent } else { pal.text_faint },
    );
    response.on_hover_text(hint)
}

/// Display real line breaks as faint symbols, with one glyph per source character
/// so TextEdit's character-based cursor, selection and scrolling stay aligned.
fn search_input_layout(text: &str, font: egui::FontId, pal: &Palette) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob {
        break_on_newline: false,
        ..Default::default()
    };
    let normal = egui::TextFormat::simple(font.clone(), pal.text_main);
    let marker = egui::TextFormat::simple(font, pal.text_faint);
    let mut start = 0;
    for (offset, ch) in text.char_indices() {
        let symbol = match ch {
            '\n' => "↵",
            '\r' => "␍",
            _ => continue,
        };
        if start < offset {
            job.append(&text[start..offset], 0.0, normal.clone());
        }
        job.append(symbol, 0.0, marker.clone());
        start = offset + ch.len_utf8();
    }
    if start < text.len() || job.sections.is_empty() {
        job.append(&text[start..], 0.0, normal);
    }
    job
}

/// One outlined field with an editable, clipped text region and a separate
/// trailing action region. Long input scrolls inside TextEdit, never under icons.
#[allow(clippy::too_many_arguments)]
fn input_with_actions(
    ui: &mut egui::Ui,
    id: egui::Id,
    text: &mut String,
    hint: &str,
    width: f32,
    options: &mut [(&str, &str, bool)],
    actions: [(usize, &str); 2],
    pal: &Palette,
) -> (egui::Response, [egui::Response; 2], egui::Rect) {
    let output = egui::Frame::new()
        .fill(pal.bg_inset)
        .stroke(egui::Stroke::new(1.0_f32, pal.text_faint.gamma_multiply(0.3)))
        .corner_radius(5.0)
        .inner_margin(egui::Margin::symmetric(5, 3))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.allocate_ui_with_layout(
                egui::vec2(width - 10.0, 24.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    let font = egui::TextStyle::Body.resolve(ui.style());
                    let mut layouter = |ui: &egui::Ui, text: &str, _: f32| {
                        ui.fonts(|f| f.layout_job(search_input_layout(text, font.clone(), pal)))
                    };
                    let response = ui.add_sized(
                        egui::vec2((width - 78.0 - options.len() as f32 * 28.0).max(24.0), 24.0),
                        egui::TextEdit::singleline(text)
                            .id(id)
                            .hint_text(hint)
                            .frame(false)
                            .layouter(&mut layouter)
                            .margin(egui::Margin::symmetric(2, 3)),
                    );
                    ui.add_space(6.0);
                    let (divider, _) =
                        ui.allocate_exact_size(egui::vec2(1.0, 18.0), egui::Sense::hover());
                    ui.painter().line_segment(
                        [divider.center_top(), divider.center_bottom()],
                        egui::Stroke::new(1.0_f32, pal.text_faint.gamma_multiply(0.25)),
                    );
                    ui.add_space(5.0);
                    for (label, hint, selected) in options {
                        let button = search_option(ui, label, hint, *selected, pal);
                        ui.ctx()
                            .data_mut(|d| d.insert_temp(id.with(*label), button.rect));
                        if button.clicked() {
                            *selected = !*selected;
                        }
                        ui.add_space(4.0);
                    }
                    let first = icon_button(ui, actions[0].0, actions[0].1, pal);
                    let second = icon_button(ui, actions[1].0, actions[1].1, pal);
                    (response, [first, second])
                },
            )
            .inner
        });
    (output.inner.0, output.inner.1, output.response.rect)
}

pub(crate) fn replace_range(
    file: &mut crate::LoadedFile,
    ctx: &egui::Context,
    range: Range<usize>,
    replacement: String,
) {
    replace_range_with_cursor(file, ctx, range, replacement, None);
}

pub(crate) fn replace_range_with_cursor(
    file: &mut crate::LoadedFile,
    ctx: &egui::Context,
    range: Range<usize>,
    replacement: String,
    after: Option<egui::text::CCursorRange>,
) {
    file.prepare_changes();
    if !file.editor_state.large.active && file.draft_text.as_ref().is_some_and(|text|
        text.len().saturating_sub(range.len()).saturating_add(replacement.len()) >= crate::large_editor::THRESHOLD
        || text.len() >= crate::large_editor::THRESHOLD) {
        file.activate_large(ctx);
    }
    let text = file.draft_text.as_mut().expect("text document");
    if text
        .len()
        .saturating_sub(range.len())
        .saturating_add(replacement.len())
        >= crate::large_editor::THRESHOLD
        || text.len() >= crate::large_editor::THRESHOLD
        || file.editor_state.large.active
    {
        file.editor_state.large.set_encoding(text, file.encoding);
        file.editor_state
            .large
            .external_replace(text, range, replacement);
        if let Some(after) = after {
            file.editor_state.large.set_tool_cursor(after);
        }
        file.character_count = file.editor_state.large.characters;
        file.bytes = Vec::new();
        file.lines = Vec::new();
        file.bytes_dirty = true;
        for (range, inserted) in file.editor_state.large.take_tool_edits() {
            file.editor_state.tools.apply_edit(range, inserted);
        }
    } else {
        let id = egui::Id::new("draft_editor").with(file.id);
        let mut edit = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
        let cursor = edit.cursor.char_range().unwrap_or_default();
        let mut undoer = edit.undoer();
        undoer.add_undo(&(cursor, text.clone()));
        let start = text[..range.start].chars().count();
        let end = start + replacement.chars().count();
        file.editor_state
            .tools
            .apply_edit(range.clone(), replacement.len());
        text.replace_range(range, &replacement);
        let after =
            after.unwrap_or_else(|| egui::text::CCursorRange::one(egui::text::CCursor::new(end)));
        edit.cursor.set_char_range(Some(after));
        undoer.add_undo(&(after, text.clone()));
        edit.set_undoer(undoer);
        edit.store(ctx, id);
        file.editor_state.cursor = Some(after);
        file.sync_draft();
    }
    file.editor_state.selection = None;
    file.editor_state.tools.current = None;
}

/// Small chevron that unfolds or folds the replace row. It is intentionally
/// shorter than a normal button (14 px) so it reads as a handle attached to
/// the find field rather than another toolbar action.
fn find_replace_toggle(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    expanded: bool,
    pal: &Palette,
    file_id: u64,
    t: &crate::i18n::Strings,
) -> bool {
    let response = ui
        .interact(
            rect,
            egui::Id::new(("find_replace_toggle", file_id)),
            egui::Sense::click(),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(t.find_replace);
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        4.0,
        if response.hovered() {
            crate::opaque_tint(pal.bg_panel, pal.text_faint, 0.20)
        } else {
            crate::opaque_tint(pal.bg_panel, pal.text_faint, 0.08)
        },
    );
    let color = if response.hovered() {
        pal.text_main
    } else {
        pal.text_faint
    };
    let center = rect.center();
    let half_width = 4.0;
    // Collapsed points down (more content below); expanded points up (fold it
    // away again).
    let tip = if expanded { -2.0 } else { 2.0 };
    let stroke = egui::Stroke::new(1.5_f32, color);
    painter.line_segment(
        [
            egui::pos2(center.x - half_width, center.y - tip),
            egui::pos2(center.x, center.y + tip),
        ],
        stroke,
    );
    painter.line_segment(
        [
            egui::pos2(center.x, center.y + tip),
            egui::pos2(center.x + half_width, center.y - tip),
        ],
        stroke,
    );
    ui.ctx().data_mut(|d| {
        d.insert_temp(
            egui::Id::new(("find_replace_toggle_rect", file_id)),
            rect,
        )
    });
    response.clicked()
}

pub(crate) fn find_bar(
    ui: &mut egui::Ui,
    file: &mut crate::LoadedFile,
    pal: &Palette,
    t: &crate::i18n::Strings,
) {
    if !ui.is_enabled() {
        return;
    }
    let focus = file.editor_state.tools.focus_find.take();
    let managed = ui.ctx().data(|d| {
        d.get_temp::<bool>(egui::Id::new("app_keymap_managed"))
            .unwrap_or(false)
    });
    let shortcut = focus == Some(false)
        || (!managed
            && crate::shortcuts::take_binding(ui.ctx(), crate::shortcuts::Action::Find.binding()));
    if shortcut {
        // Opening find from a non-empty editor selection should seed the query
        // once. Keep the existing query when the bar is already open/focused.
        if !file.editor_state.tools.find_open
            && let Some(cursor) = file.editor_state.cursor
            && cursor.primary.index != cursor.secondary.index
            && let Some(text) = file.draft_text.as_deref()
        {
            let start = cursor.primary.index.min(cursor.secondary.index);
            let end = cursor.primary.index.max(cursor.secondary.index);
            let byte = |index| {
                if file.editor_state.large.active && file.editor_state.large.indexed_len == Some(text.len()) {
                    file.editor_state.large.byte_for_char(text, index)
                } else { byte_at(text, index) }
            };
            file.editor_state.tools.query = text.get(byte(start)..byte(end)).unwrap_or_default().to_owned();
        }
        file.editor_state.tools.find_open = true;
    }
    if !file.editor_state.tools.find_open {
        return;
    }
    file.editor_state
        .tools
        .invalidate_search_if_changed((file.content_revision, file.editor_state.large.revision));
    let mut action = file.editor_state.tools.pending_action.take().unwrap_or(0);
    // `Focus replace` and the replace shortcuts act on the replacement field,
    // so they must unfold the hidden replace row first.
    if focus == Some(true) || matches!(action, 3 | 4) {
        file.editor_state.tools.replace_open = true;
    }
    let mut jump = false;
    let bounds = ui.available_rect_before_wrap();
    let width = (bounds.width() - 16.0).min(500.0);
    let overlay = egui::Area::new(egui::Id::new(("find_overlay", file.id)))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::RIGHT_TOP)
        .fixed_pos(bounds.right_top() + egui::vec2(-8.0, 8.0))
        .default_size(egui::vec2(width, 90.0))
        .movable(false)
        .fade_in(false)
        .show(ui.ctx(), |ui| {
            ui.set_width(width);
            ui.with_layout(egui::Layout::top_down(egui::Align::Max), |ui| {
                egui::Frame::new()
                    .fill(pal.bg_panel)
                    .stroke(egui::Stroke::new(1.0_f32, pal.text_faint.gamma_multiply(0.2)))
                    .corner_radius(6.0)
                    .inner_margin(egui::Margin::same(6))
                    .show(ui, |ui| {
                        ui.set_width((width - 12.0).max(200.0));
                        ui.spacing_mut().item_spacing = egui::vec2(8.0, 4.0);
                        ui.allocate_ui_with_layout(
                            egui::vec2(ui.available_width(), 32.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                let tools = &mut file.editor_state.tools;
                                let mut options = [
                                    ("Aa", t.ignore_case, tools.ignore_case),
                                    (".*", t.regex, tools.regex),
                                ];
                                let (response, [previous, next], field) = input_with_actions(
                                    ui,
                                    egui::Id::new(("find_query", file.id)),
                                    &mut tools.query,
                                    t.find,
                                    (ui.available_width() - 2.0).max(140.0),
                                    &mut options,
                                    [(7, "Shift+Enter"), (8, "Enter")],
                                    pal,
                                );
                                ui.ctx().data_mut(|d| {
                                    d.insert_temp(
                                        egui::Id::new(("find_field_rect", file.id)),
                                        field,
                                    );
                                    d.insert_temp(
                                        egui::Id::new(("find_input_rect", file.id)),
                                        response.rect,
                                    );
                                    d.insert_temp(
                                        egui::Id::new(("find_previous_rect", file.id)),
                                        previous.rect,
                                    );
                                    d.insert_temp(
                                        egui::Id::new(("find_next_rect", file.id)),
                                        next.rect,
                                    );
                                });
                                if shortcut {
                                    response.request_focus();
                                }
                                if response.changed() {
                                    tools.search_key = None;
                                    tools.invalidate_search_if_changed((
                                        file.content_revision,
                                        file.editor_state.large.revision,
                                    ));
                                }
                                if tools.ignore_case != options[0].2 || tools.regex != options[1].2
                                {
                                    tools.ignore_case = options[0].2;
                                    tools.regex = options[1].2;
                                    tools.search_key = None;
                                    tools.invalidate_search_if_changed((
                                        file.content_revision,
                                        file.editor_state.large.revision,
                                    ));
                                }
                                if previous.clicked() {
                                    action = 1;
                                }
                                if next.clicked() {
                                    action = 2;
                                }
                                // A singleline TextEdit surrenders focus on Enter,
                                // so `has_focus` is already false in the Enter
                                // frame; accept the just-lost focus as well and
                                // keep the field focused for repeated stepping.
                                if (response.has_focus() || response.lost_focus())
                                    && ui.input(|i| i.key_pressed(egui::Key::Enter))
                                {
                                    action = if ui.input(|i| i.modifiers.shift) {
                                        1
                                    } else {
                                        2
                                    };
                                    response.request_focus();
                                }
                            },
                        );
                        if file.editor_state.tools.replace_open {
                            ui.allocate_ui_with_layout(
                                egui::vec2(ui.available_width(), 32.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    let tools = &mut file.editor_state.tools;
                                    let (response, [single, all], field) = input_with_actions(
                                        ui,
                                        egui::Id::new(("find_replacement", file.id)),
                                        &mut tools.replacement,
                                        t.replace,
                                        (ui.available_width() - 2.0).max(140.0),
                                        &mut [],
                                        [(4, t.replace), (5, t.replace_all)],
                                        pal,
                                    );
                                    ui.ctx().data_mut(|d| {
                                        d.insert_temp(
                                            egui::Id::new(("replace_rect", file.id)),
                                            single.rect,
                                        );
                                        d.insert_temp(
                                            egui::Id::new(("replace_all_rect", file.id)),
                                            all.rect,
                                        );
                                        d.insert_temp(
                                            egui::Id::new(("replace_field_rect", file.id)),
                                            field,
                                        );
                                        d.insert_temp(
                                            egui::Id::new(("replace_input_rect", file.id)),
                                            response.rect,
                                        );
                                    });
                                    if focus == Some(true) {
                                        response.request_focus();
                                    }
                                    if single.clicked() {
                                        action = 3;
                                    }
                                    if all.clicked() {
                                        action = 4;
                                    }
                                },
                            );
                            // Collapse handle: a slim chevron in its own strip,
                            // deliberately shorter than a regular button.
                            let (strip, _) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), 16.0),
                                egui::Sense::hover(),
                            );
                            let toggle = egui::Rect::from_center_size(
                                strip.center(),
                                egui::vec2(30.0, 14.0),
                            );
                            if find_replace_toggle(ui, toggle, true, pal, file.id, t) {
                                file.editor_state.tools.replace_open = false;
                            }
                        } else {
                            let row = ui.allocate_ui_with_layout(
                                egui::vec2(ui.available_width(), 24.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    let tools = &mut file.editor_state.tools;
                                    let count = ui
                                        .add(egui::Button::new(t.count_label).selected(tools.count))
                                        .on_hover_text(t.count_matches);
                                    ui.ctx().data_mut(|d| {
                                        d.insert_temp(
                                            egui::Id::new(("find_count_rect", file.id)),
                                            count.rect,
                                        )
                                    });
                                    if count.clicked() {
                                        action = 5;
                                    }
                                    if (tools.count_result.is_some() || tools.not_found)
                                        && tools.search_error.is_none()
                                    {
                                        let total = tools.count_result.unwrap_or(0);
                                        let result = if total == 0 {
                                            t.count_not_found.to_owned()
                                        } else {
                                            total.to_string()
                                        };
                                        let style = if total == 0 {
                                            egui::RichText::new(result).small().color(pal.text_faint)
                                        } else {
                                            egui::RichText::new(result)
                                                .size(16.0)
                                                .strong()
                                                .color(pal.accent)
                                        };
                                        let response = ui.label(style);
                                        ui.ctx().data_mut(|d| {
                                            d.insert_temp(
                                                egui::Id::new(("find_count_result_rect", file.id)),
                                                response.rect,
                                            )
                                        });
                                    }
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        let button = ui.button(t.go_to_line);
                                        let input = ui.add(egui::TextEdit::singleline(&mut tools.jump_line)
                                            .id(egui::Id::new(("jump_line", file.id)))
                                            .desired_width(72.0).char_limit(20));
                                        tools.jump_line.retain(|c| c.is_ascii_digit());
                                        ui.ctx().data_mut(|d| {
                                            d.insert_temp(egui::Id::new(("jump_line_input_rect", file.id)), input.rect);
                                            d.insert_temp(egui::Id::new(("jump_line_button_rect", file.id)), button.rect);
                                        });
                                        if button.clicked() || ((input.has_focus() || input.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter))) { jump = true; }
                                    });
                                },
                            );
                            // Expand handle sits in the middle of the count/jump
                            // row, right underneath the find field. On narrow
                            // panels the count and jump clusters can meet in
                            // the middle, so the handle moves into its own
                            // slim strip instead of covering an input.
                            let row_rect = row.response.rect;
                            let cluster = |key: &str| {
                                ui.ctx().data(|d| {
                                    d.get_temp::<egui::Rect>(egui::Id::new((key, file.id)))
                                })
                            };
                            let gap = cluster("find_count_rect")
                                .zip(cluster("jump_line_input_rect"))
                                .map(|(left, right)| (left.right(), right.left()));
                            let toggle = match gap {
                                Some((left_edge, right_edge))
                                    if right_edge - left_edge >= 46.0 =>
                                {
                                    let center_x = row_rect.center().x;
                                    // Prefer the panel middle, clamped into the
                                    // free gap between the two clusters.
                                    let center_x =
                                        center_x.clamp(left_edge + 23.0, right_edge - 23.0);
                                    egui::Rect::from_center_size(
                                        egui::pos2(center_x, row_rect.center().y),
                                        egui::vec2(30.0, 16.0),
                                    )
                                }
                                _ => {
                                    let (strip, _) = ui.allocate_exact_size(
                                        egui::vec2(ui.available_width(), 16.0),
                                        egui::Sense::hover(),
                                    );
                                    egui::Rect::from_center_size(
                                        strip.center(),
                                        egui::vec2(30.0, 14.0),
                                    )
                                }
                            };
                            if find_replace_toggle(ui, toggle, false, pal, file.id, t) {
                                file.editor_state.tools.replace_open = true;
                            }
                        }
                        let tools = &mut file.editor_state.tools;
                        if let Some(error) = &tools.search_error {
                            ui.label(egui::RichText::new(error).small().color(pal.danger));
                        }
                    });
            });
        });
    ui.ctx().data_mut(|d| {
        d.insert_temp(
            egui::Id::new(("find_overlay_rect", file.id)),
            overlay.response.rect,
        )
    });
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        file.editor_state.tools.find_open = false;
        file.editor_state.tools.replace_open = false;
    }
    if action != 0 {
        if file.draft_text.as_ref().is_some_and(|text| text.len() >= crate::large_editor::THRESHOLD) {
            background_search::start(file, ui.ctx(), action);
        } else if action == 5 {
            file.editor_state.tools.count_all(
                file.draft_text.as_deref().unwrap_or_default(),
                (file.content_revision, file.editor_state.large.revision),
            );
        } else {
            search_action(file, ui.ctx(), action);
        }
        ui.ctx().request_repaint();
    }
    if jump { jump_to_line(file, ui.ctx()); }
}

fn jump_to_line(file: &mut crate::LoadedFile, ctx: &egui::Context) {
    let Ok(line) = file.editor_state.tools.jump_line.parse::<usize>() else { return; };
    if line == 0 { return; }
    let Some(text) = file.draft_text.as_deref() else { return; };
    let start = if text.len() >= crate::large_editor::THRESHOLD || file.editor_state.large.active {
        file.editor_state.large.line_start(text, line - 1)
    } else if line == 1 { Some(0) } else {
        text.char_indices().filter(|(_, ch)| *ch == '\n').nth(line - 2).map(|(at, _)| text[..at + 1].chars().count())
    };
    let Some(start) = start else { return; };
    // A fold toggle may leave an internal selection marker for this frame;
    // explicit navigation must reveal its target regardless of that marker.
    file.editor_state.folds.expand_all();
    file.editor_state.selection = None;
    file.editor_state.anchor = None;
    file.editor_state.tools.current = None;
    file.editor_state.pending_selection = Some(start..start);
    file.editor_state.focus_requested = true;
    ctx.request_repaint();
}

/// Intercept bulk paste before TextEdit can lay out the entire inserted payload
/// in a small editor (or a viewport window). One delta undo; original files stay
/// untouched, and the cursor is revealed through the normal large editor path.
pub(crate) fn bulk_paste(file: &mut crate::LoadedFile, ctx: &egui::Context, focus: bool) -> bool {
    let id = egui::Id::new(if file.editor_state.large.active { "large_editor" } else { "draft_editor" }).with(file.id);
    if !focus && !ctx.memory(|m| m.has_focus(id)) { return false; }
    let Some(text) = file.draft_text.as_deref() else { return false; };
    let Some((event, payload)) = ctx.input(|i| i.events.iter().enumerate().find_map(|(index, event)| {
        if let egui::Event::Paste(payload) = event
            && payload.len() >= 64 * 1024
            && text.len().saturating_add(payload.len()) >= crate::large_editor::THRESHOLD
        { Some((index, payload.clone())) } else { None }
    })) else { return false; };
    let selection = file.editor_state.pending_selection.clone().unwrap_or_else(|| {
        let cursor = file.editor_state.cursor.unwrap_or_default();
        cursor.primary.index.min(cursor.secondary.index)..cursor.primary.index.max(cursor.secondary.index)
    });
    let range = byte_at(text, selection.start)..byte_at(text, selection.end);
    let end = selection.start + payload.chars().count();
    ctx.input_mut(|i| { i.events.remove(event); });
    replace_range_with_cursor(file, ctx, range, payload, Some(egui::text::CCursorRange::one(egui::text::CCursor::new(end))));
    file.editor_state.pending_selection = Some(end..end);
    file.editor_state.focus_requested = true;
    ctx.request_repaint();
    true
}

fn search_action(file: &mut crate::LoadedFile, ctx: &egui::Context, action: usize) {
    if file.draft_text.as_ref().is_some_and(|text| text.len() >= crate::large_editor::THRESHOLD) {
        background_search::start(file, ctx, action);
        return;
    }
    let revision = (file.content_revision, file.editor_state.large.revision);
    if !file.editor_state.tools.prepare_matcher(revision) {
        return;
    }
    if action == 4 {
        replace_all(file, ctx);
        return;
    }
    let text = file.draft_text.as_deref().unwrap();
    let selected = file.editor_state.cursor.map(|c| {
        byte_at(text, c.primary.index.min(c.secondary.index))
            ..byte_at(text, c.primary.index.max(c.secondary.index))
    });
    // In the rendered modes the preview owns the highlight and there is no
    // text selection to compare against. Keeping the current match there is
    // what lets Enter / "next" advance instead of restarting at the first hit.
    let rendered = crate::markdown::is_markdown(file)
        && file.markdown_mode != crate::markdown::Mode::Source
        && !file.editor_state.large.active
        && !crate::large_editor::needs_windowing(text);
    let current = file
        .editor_state
        .tools
        .current
        .clone()
        .filter(|r| rendered || Some(r) == selected.as_ref());
    if action == 3 {
        // Never replace an unlocated query or an unrelated manual selection.
        if let Some(range) = current
            && file
                .editor_state
                .tools
                .matcher
                .as_ref()
                .and_then(|m| m.find_at(text, range.start))
                .is_some_and(|m| m.range() == range)
        {
            let replacement = file.editor_state.tools.replacement_for(text, &range);
            // Replacing selects the next hit right away, like the mainstream
            // editors: each press replaces the current match and lands on the
            // following one, so repeated replaces need no extra "next".
            let advance = file
                .editor_state
                .tools
                .find_one_after_edit(text, &range, &replacement);
            replace_range(file, ctx, range, replacement);
            // The edit bumped the revision; re-anchor so the advanced
            // `current` is not dropped as stale by the next action.
            let revision = (file.content_revision, file.editor_state.large.revision);
            file.editor_state.tools.reanchor(revision);
            match advance {
                Some((found, characters)) => {
                    file.editor_state.selection = None;
                    file.editor_state.pending_selection = Some(characters);
                    file.editor_state.tools.current = Some(found);
                    file.editor_state.tools.not_found = false;
                }
                None => {
                    file.editor_state.tools.current = None;
                    file.editor_state.tools.not_found = true;
                }
            }
        }
        return;
    }
    let from = if let Some(range) = &current {
        if action == 1 { range.start } else { range.end }
    } else {
        file.editor_state
            .cursor
            .map_or(0, |c| byte_at(text, c.primary.index))
    };
    let found = file.editor_state.tools.find_one(
        text,
        from,
        action == 1,
        current.as_ref().is_some_and(|r| r.is_empty()),
    );
    file.editor_state.tools.not_found = found.is_none();
    if let Some(range) = &found {
        file.editor_state.selection = None;
        let start = text[..range.start].chars().count();
        file.editor_state.pending_selection =
            Some(start..start + text[range.clone()].chars().count());
    }
    file.editor_state.tools.current = found;
}

#[derive(Clone, Default)]
pub(crate) struct Tools {
    pub(crate) workspace: Option<crate::workspace_theme::Render>,
    pub focus_find: Option<bool>,
    pub pending_action: Option<usize>,
    pub wrap: bool,
    pub whitespace: bool,
    pub pen: Option<usize>,
    pub pen_menu: bool,
    marker_anchor: Option<usize>,
    pub marks: Vec<Mark>,
    pub ignore_case: bool,
    pub count: bool,
    count_result: Option<usize>,
    pub regex: bool,
    pub find_open: bool,
    /// Whether the replace row is unfolded below the find field. Collapsed by
    /// default so the panel only shows the query and the count/jump row.
    pub replace_open: bool,
    pub query: String,
    pub replacement: String,
    pub jump_line: String,
    pub current: Option<Range<usize>>,
    pub not_found: bool,
    search_key: Option<(String, bool, bool, u64, u64)>,
    matcher: Option<regex::Regex>,
    pub matches: std::sync::Arc<Vec<Range<usize>>>,
    pub search_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Mark {
    pub range: Range<usize>,
    pub color: usize,
}

/// Visual-only concealment, not redaction: source text and copying are unchanged.
pub(crate) const CONCEAL: usize = 5;

pub(crate) fn pen_color(index: usize) -> egui::Color32 {
    let colors = [
        (220, 91, 91),
        (204, 170, 65),
        (91, 170, 110),
        (88, 150, 220),
        (169, 115, 210),
        (128, 128, 128), // Neutral palette icon; the actual cover follows the theme.
    ];
    let (r, g, b) = colors[index];
    egui::Color32::from_rgb(r, g, b)
}

impl Tools {
    /// Keep a pen gesture alive independently of TextEdit hover/focus on release.
    /// Releasing just beyond a glyph/viewport must not silently lose the mark.
    pub(crate) fn handle_marker(
        &mut self,
        ui: &egui::Ui,
        output: &egui::text_edit::TextEditOutput,
        text: &str,
        character_offset: usize,
    ) {
        self.handle_marker_mapped(ui, output, text, |index| index + character_offset);
    }

    pub(crate) fn handle_marker_mapped(
        &mut self,
        ui: &egui::Ui,
        output: &egui::text_edit::TextEditOutput,
        text: &str,
        character: impl Fn(usize) -> usize,
    ) {
        if self.pen.is_none() {
            self.marker_anchor = None;
            return;
        }
        let (pressed, released, position, shift, keyboard) = ui.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_released(),
                i.pointer.interact_pos(),
                i.modifiers.shift,
                i.modifiers.shift
                    && i.events
                        .iter()
                        .any(|e| matches!(e, egui::Event::Key { pressed: true, .. })),
            )
        });
        if pressed && output.response.contains_pointer() && output.response.has_focus() {
            self.marker_anchor = position.map(|pos| {
                if shift && let Some(cursor) = output.state.cursor.char_range() {
                    character(cursor.secondary.index)
                } else {
                    character(output
                        .galley
                        .cursor_from_pos(pos - output.galley_pos)
                        .ccursor
                        .index)
                }
            });
        }
        if released
            && let Some(anchor) = self.marker_anchor.take()
            && let Some(pos) = position
        {
            let end = character(output
                .galley
                .cursor_from_pos(pos - output.galley_pos)
                .ccursor
                .index);
            self.mark_selection(
                text,
                Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(anchor),
                    egui::text::CCursor::new(end),
                )),
            );
            ui.ctx().request_repaint();
        } else if keyboard && output.response.has_focus() && !output.response.changed() {
            let cursor = output.state.cursor.char_range().map(|mut cursor| {
                cursor.primary.index = character(cursor.primary.index);
                cursor.secondary.index = character(cursor.secondary.index);
                cursor
            });
            self.mark_selection(text, cursor);
            ui.ctx().request_repaint();
        }
    }

    pub(crate) fn apply_edit(&mut self, removed: Range<usize>, inserted: usize) {
        let delta = inserted as isize - removed.len() as isize;
        self.marks.retain_mut(|mark| {
            if mark.range.end <= removed.start {
                return true;
            }
            if mark.range.start >= removed.end {
                mark.range.start = mark.range.start.saturating_add_signed(delta);
                mark.range.end = mark.range.end.saturating_add_signed(delta);
                return true;
            }
            let start = mark.range.start.min(removed.start);
            let end = if mark.range.end > removed.end {
                mark.range.end.saturating_add_signed(delta)
            } else {
                removed.start
            };
            mark.range = start..end;
            !mark.range.is_empty()
        });
        self.search_key = None;
        self.matches = Default::default();
        self.current = None;
        self.count_result = None;
        self.count = false;
    }

    pub(crate) fn mark_selection(&mut self, text: &str, cursor: Option<egui::text::CCursorRange>) {
        if let (Some(color), Some(cursor)) = (self.pen, cursor) {
            let a = cursor.primary.index.min(cursor.secondary.index);
            let b = cursor.primary.index.max(cursor.secondary.index);
            if a != b {
                let range = byte_at(text, a)..byte_at(text, b);
                // Recolor overlapping portions; never paint unrelated equal strings.
                let mut kept = Vec::new();
                for mark in self.marks.drain(..) {
                    if mark.range.end <= range.start || mark.range.start >= range.end {
                        kept.push(mark);
                    } else {
                        if mark.range.start < range.start {
                            kept.push(Mark {
                                range: mark.range.start..range.start,
                                color: mark.color,
                            });
                        }
                        if mark.range.end > range.end {
                            kept.push(Mark {
                                range: range.end..mark.range.end,
                                color: mark.color,
                            });
                        }
                    }
                }
                kept.push(Mark { range, color });
                kept.sort_by_key(|m| m.range.start);
                self.marks = kept;
            }
        }
    }

    fn invalidate_search_if_changed(&mut self, revision: (u64, u64)) {
        let key = (
            self.query.clone(),
            self.ignore_case,
            self.regex,
            revision.0,
            revision.1,
        );
        if self.search_key.as_ref() == Some(&key) {
            return;
        }
        self.search_key = Some(key);
        self.matches = Default::default();
        self.search_error = None;
        self.matcher = None;
        self.current = None;
        self.count_result = None;
        self.count = false;
        self.not_found = false;
    }

    /// Re-anchor the search state to the current revision after a
    /// programmatic edit, so a freshly located `current` survives the next
    /// action's staleness check. The matcher is text-independent and stays.
    pub(crate) fn reanchor(&mut self, revision: (u64, u64)) {
        self.search_key = Some((
            self.query.clone(),
            self.ignore_case,
            self.regex,
            revision.0,
            revision.1,
        ));
    }

    fn prepare_matcher(&mut self, revision: (u64, u64)) -> bool {
        self.invalidate_search_if_changed(revision);
        if self.query.is_empty() {
            return false;
        }
        if self.matcher.is_some() {
            return true;
        }
        let pattern = if self.regex {
            self.query.clone()
        } else {
            regex::escape(&self.query)
        };
        match regex::RegexBuilder::new(&pattern)
            .case_insensitive(self.ignore_case)
            .build()
        {
            Ok(matcher) => {
                self.matcher = Some(matcher);
            }
            Err(error) => self.search_error = Some(error.to_string()),
        }
        self.matcher.is_some()
    }

    fn count_all(&mut self, text: &str, revision: (u64, u64)) {
        let ready = self.prepare_matcher(revision);
        self.count = true;
        self.count_result = if ready {
            Some(self.matcher.as_ref().unwrap().find_iter(text).count())
        } else {
            self.query.is_empty().then_some(0)
        };
    }

    /// Full enumeration is reserved for explicit count/replace-all actions.
    fn refresh_search(&mut self, text: &str, revision: (u64, u64)) {
        if self.prepare_matcher(revision) {
            self.matches = std::sync::Arc::new(
                self.matcher
                    .as_ref()
                    .unwrap()
                    .find_iter(text)
                    .map(|m| m.range())
                    .collect(),
            );
        }
    }

    /// The next match after a single replacement, evaluated on the text the
    /// replacement is about to be applied to. Positions after the edit shift
    /// by the length delta; the search wraps to the top when the tail holds
    /// no further hit. Returns the byte range in the *new* coordinates plus
    /// the matching character range for the editor selection.
    pub(crate) fn find_one_after_edit(
        &self,
        text: &str,
        replaced: &Range<usize>,
        insertion: &str,
    ) -> Option<(Range<usize>, Range<usize>)> {
        let delta = insertion.len() as isize - replaced.len() as isize;
        let removed_chars = text[replaced.clone()].chars().count();
        let inserted_chars = insertion.chars().count();
        let (old_start, len, in_tail) = if !self.regex && !self.ignore_case {
            let query = self.query.as_str();
            let tail = &text[replaced.end..];
            if let Some(at) = tail.find(query) {
                (replaced.end + at, query.len(), true)
            } else {
                let head = &text[..replaced.start];
                let at = head.find(query)?;
                (at, query.len(), false)
            }
        } else {
            let matcher = self.matcher.as_ref()?;
            let after = matcher.find_iter(text).find(|m| m.start() >= replaced.end);
            if let Some(found) = after {
                (found.start(), found.end() - found.start(), true)
            } else {
                let found = matcher.find_iter(text).find(|m| m.end() <= replaced.start)?;
                (found.start(), found.end() - found.start(), false)
            }
        };
        let start = if in_tail {
            (old_start as isize + delta) as usize
        } else {
            old_start
        };
        let bytes = start..start + len;
        let matched_chars = text[old_start..old_start + len].chars().count();
        let prefix = if in_tail {
            text[..old_start].chars().count() - removed_chars + inserted_chars
        } else {
            text[..old_start].chars().count()
        };
        Some((bytes, prefix..prefix + matched_chars))
    }

    fn find_one(
        &self,
        text: &str,
        from: usize,
        backwards: bool,
        skip_empty: bool,
    ) -> Option<Range<usize>> {        if !self.regex && !self.ignore_case {
            let start = if backwards {
                text[..from]
                    .rfind(&self.query)
                    .or_else(|| text[from..].rfind(&self.query).map(|p| from + p))
            } else {
                text[from..]
                    .find(&self.query)
                    .map(|p| from + p)
                    .or_else(|| text[..from].find(&self.query))
            }?;
            return Some(start..start + self.query.len());
        }
        let matcher = self.matcher.as_ref()?;
        if backwards {
            // Reverse regex search is not offered by this engine. Stream matches
            // up to the cursor, retaining only the latest, rather than a result list.
            matcher
                .find_iter(text)
                .take_while(|m| m.start() < from)
                .last()
                .or_else(|| matcher.find_iter(text).last())
                .map(|m| m.range())
        } else {
            let start = if skip_empty && from < text.len() {
                from + text[from..].chars().next()?.len_utf8()
            } else if skip_empty {
                return matcher.find(text).map(|m| m.range());
            } else {
                from
            };
            matcher
                .find_at(text, start)
                .or_else(|| matcher.find(text))
                .map(|m| m.range())
        }
    }

    fn replacement_for(&self, text: &str, range: &Range<usize>) -> String {
        if self.regex
            && let Some(matcher) = &self.matcher
            && let Some(captures) = matcher.captures_at(text, range.start)
        {
            let mut result = String::new();
            captures.expand(&self.replacement, &mut result);
            return result;
        }
        self.replacement.clone()
    }
}

#[cfg(test)]
fn navigate(matches: &[Range<usize>], from: usize, backwards: bool) -> Option<Range<usize>> {
    if backwards {
        matches
            .iter()
            .rev()
            .find(|r| r.start < from)
            .or_else(|| matches.last())
            .cloned()
    } else {
        matches
            .iter()
            .find(|r| r.start >= from)
            .or_else(|| matches.first())
            .cloned()
    }
}

fn replace_all(file: &mut crate::LoadedFile, ctx: &egui::Context) {
    file.editor_state.tools.refresh_search(
        file.draft_text.as_deref().unwrap(),
        (file.content_revision, file.editor_state.large.revision),
    );
    if file.editor_state.tools.search_error.is_some() {
        return;
    }
    if file.editor_state.tools.matches.is_empty() {
        file.editor_state.tools.not_found = true;
        return;
    }
    let text = file.draft_text.as_deref().unwrap();
    let tools = &mut file.editor_state.tools;
    let replacements: Vec<_> = tools
        .matches
        .iter()
        .map(|range| (range.clone(), tools.replacement_for(text, range)))
        .collect();
    if text.len() >= 2 * 1024 * 1024 && replacements.len() > 1000 {
        crate::line_commands::commit_edits(file, ctx, replacements, None);
        return;
    }
    let mut result = String::with_capacity(text.len());
    let mut offset = 0;
    for (range, replacement) in &replacements {
        result.push_str(&text[offset..range.start]);
        result.push_str(replacement);
        offset = range.end;
    }
    result.push_str(&text[offset..]);
    let (old, new) = crate::large_editor::difference(text, &result);
    let inserted = result[new].to_owned();
    // Update marks per match, but commit the replacement as one undoable edit.
    for (range, replacement) in replacements.into_iter().rev() {
        tools.apply_edit(range, replacement.len());
    }
    let marks = std::mem::take(&mut tools.marks);
    replace_range(file, ctx, old, inserted);
    file.editor_state.tools.marks = marks;
}

pub(crate) fn byte_at(text: &str, character: usize) -> usize {
    text.char_indices()
        .nth(character)
        .map_or(text.len(), |(b, _)| b)
}

pub(crate) fn layout(
    text: &str,
    font: egui::FontId,
    pal: &Palette,
    width: f32,
    tools: &Tools,
) -> egui::text::LayoutJob {
    layout_window(text, font, pal, width, tools, 0)
}

pub(crate) fn layout_window(
    text: &str,
    font: egui::FontId,
    pal: &Palette,
    width: f32,
    tools: &Tools,
    offset: usize,
) -> egui::text::LayoutJob {
    let mut job = if let Some(render) = &tools.workspace {
        render.layout(text, font, width, offset)
    } else {
        egui::text::LayoutJob::simple(text.to_owned(), font, pal.text_main, width)
    };
    job.wrap.break_anywhere = tools.wrap;
    if tools.marks.is_empty() && (!tools.find_open || tools.current.is_none()) {
        return job;
    }
    let local = |range: &Range<usize>| {
        let start = range.start.max(offset);
        let end = range.end.min(offset + text.len());
        (start < end).then_some(start.saturating_sub(offset)..end.saturating_sub(offset))
    };
    let mut highlights = Vec::new();
    if tools.find_open
        && let Some(range) = &tools.current
        && let Some(range) = local(range)
    {
        highlights.push((
            range,
            tools
                .workspace
                .as_ref()
                .map_or(pal.accent.gamma_multiply(0.2), |r| r.colors.search),
            false,
        ));
    }
    // Marker colors take priority over search backgrounds, with exact boundaries.
    let cover = tools.workspace.as_ref().map_or(pal.text_main, |r| r.colors.text).to_opaque();
    highlights.extend(tools.marks.iter().filter_map(|m| {
        local(&m.range).map(|r| (r, if m.color == CONCEAL {
            cover
        } else {
            pen_color(m.color).gamma_multiply(0.30)
        }, m.color == CONCEAL))
    }));
    let mut events = Vec::with_capacity(highlights.len() * 2 + 2);
    for (index, (range, _, _)) in highlights.iter().enumerate() {
        events.push((range.start, true, index));
        events.push((range.end, false, index));
    }
    // Split background overlays at syntax boundaries rather than replacing the
    // foreground token formats. Markers > search; selection is painted by TextEdit.
    let syntax_sections = job.sections.clone();
    for section in &syntax_sections {
        events.push((section.byte_range.start, false, usize::MAX));
        events.push((section.byte_range.end, false, usize::MAX));
    }
    events.sort_unstable();
    let base = job.sections[0].format.clone();
    job.sections.clear();
    let mut active = std::collections::BTreeSet::<usize>::new();
    let mut start = 0;
    let mut syntax_index = 0;
    for (position, enter, index) in
        events
            .into_iter()
            .chain(std::iter::once((text.len(), false, usize::MAX)))
    {
        if position > start {
            while syntax_index + 1 < syntax_sections.len()
                && syntax_sections[syntax_index].byte_range.end <= start
            {
                syntax_index += 1;
            }
            let mut format = syntax_sections
                .get(syntax_index)
                .map_or_else(|| base.clone(), |s| s.format.clone());
            if let Some(&index) = active.last() {
                format.background = highlights[index].1;
            }
            if active.iter().any(|&index| highlights[index].2) {
                // Transparent glyph quads retain cursor geometry and can be recolored
                // by the selection painter. Unlike same-color glyphs, even italic
                // overhang outside the background rectangle cannot leak ink.
                format.color = egui::Color32::TRANSPARENT;
                format.background = cover;
                format.underline = egui::Stroke::NONE;
                format.strikethrough = egui::Stroke::NONE;
            }
            job.sections.push(egui::text::LayoutSection {
                leading_space: 0.0,
                byte_range: start..position,
                format,
            });
            start = position;
        }
        if enter {
            active.insert(index);
        } else {
            active.remove(&index);
        }
    }
    if job.sections.is_empty() {
        job.sections.push(egui::text::LayoutSection {
            leading_space: 0.0,
            byte_range: 0..text.len(),
            format: base,
        });
    }
    job
}

/// Recolor only selected glyph quads in this frame, never the cached galley.
/// TextEdit already inserts its selection background above the opaque cover.
/// Keep retained (unfocused) selections usable as well as active mouse drags.
pub(crate) fn paint_selected_text(
    ui: &egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    color: egui::Color32,
) {
    let Some(cursor) = output.state.cursor.char_range() else {
        return;
    };
    let selection = cursor.primary.index.min(cursor.secondary.index)
        ..cursor.primary.index.max(cursor.secondary.index);
    if selection.is_empty() {
        return;
    }
    ui.ctx().graphics_mut(|graphics| {
        let list = graphics.entry(ui.layer_id());
        let indices: Vec<_> = list
            .all_entries()
            .enumerate()
            .filter_map(|(index, shape)| match &shape.shape {
                egui::Shape::Text(text)
                    if text.pos == output.galley_pos
                        && text.galley.text() == output.galley.text() =>
                {
                    Some(index)
                }
                _ => None,
            })
            .collect();
        for index in indices {
            list.mutate_shape(egui::layers::ShapeIdx(index), |shape| {
                let egui::Shape::Text(text) = &mut shape.shape else {
                    return;
                };
                let galley = std::sync::Arc::make_mut(&mut text.galley);
                let mut offset = 0;
                for row in &mut galley.rows {
                    let start = selection.start.saturating_sub(offset).min(row.glyphs.len());
                    let end = selection.end.saturating_sub(offset).min(row.glyphs.len());
                    if start < end {
                        let glyphs = row.glyphs.iter().enumerate()
                            .filter(|(_, glyph)| !glyph.uv_rect.is_nothing());
                        let range = row.visuals.glyph_vertex_range.clone();
                        for ((index, _), quad) in glyphs.zip(row.visuals.mesh.vertices[range].as_chunks_mut::<4>().0) {
                            if start <= index && index < end {
                                for vertex in quad {
                                    vertex.color = color;
                                }
                            }
                        }
                    }
                    offset += row.char_count_including_newline();
                }
            });
        }
    });
}

/// Paint whitespace hints over existing glyph positions; source and cursor offsets stay intact.
pub(crate) fn paint_whitespace(
    ui: &egui::Ui,
    galley: &egui::Galley,
    pos: egui::Pos2,
    pal: &Palette,
    following: &str,
) {
    let painter = ui.painter();
    for (row_index, row) in galley.rows.iter().enumerate() {
        if row.rect.height() == 0.0 || !row.rect.translate(pos.to_vec2()).intersects(ui.clip_rect())
        {
            continue;
        }
        for (index, glyph) in row.glyphs.iter().enumerate() {
            // Conceal formatting also covers whitespace overlays; otherwise
            // dots/arrows would leak through the opaque visual mask.
            if (glyph.section_index as usize) < galley.job.sections.len()
                && galley.job.sections[glyph.section_index as usize].format.color == egui::Color32::TRANSPARENT
            { continue; }
            let hint = match glyph.chr {
                ' ' => "·",
                '\t' => "→",
                '\r' if row.ends_with_newline && index + 1 == row.glyphs.len() => continue,
                '\r' => "␍",
                _ => continue,
            };
            painter.text(
                pos + glyph.pos.to_vec2() + egui::vec2(0.0, -2.0),
                egui::Align2::LEFT_BOTTOM,
                hint,
                egui::FontId::monospace(11.0),
                pal.text_faint,
            );
        }
        // A viewport fragment excludes its boundary terminator, but the final
        // visible line still needs the original document's LF/CRLF hint.
        let boundary = (row_index + 1 == galley.rows.len()).then_some(following);
        let boundary_ending = boundary.and_then(|tail| {
            if tail.starts_with("\r\n") { Some(true) }
            else if tail.starts_with('\n') { Some(false) }
            else { None }
        });
        let concealed_end = row.glyphs.last().is_some_and(|glyph|
            galley.job.sections.get(glyph.section_index as usize)
                .is_some_and(|section| section.format.color == egui::Color32::TRANSPARENT));
        if (row.ends_with_newline || boundary_ending.is_some()) && !concealed_end {
            let crlf = boundary_ending.unwrap_or_else(|| row.glyphs.last().is_some_and(|g| g.chr == '\r'));
            let marker = pos + row.rect.right_center().to_vec2();
            painter.text(
                marker + egui::vec2(0.0, if crlf { 2.0 } else { 0.0 }),
                egui::Align2::LEFT_CENTER,
                "↵",
                egui::FontId::monospace(11.0),
                pal.text_faint,
            );
            if crlf {
                painter.text(
                    marker + egui::vec2(3.5, -1.5),
                    egui::Align2::CENTER_BOTTOM,
                    "CR",
                    egui::FontId::monospace(7.0),
                    pal.text_faint,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn jump_line_expands_folds_and_handles_trailing_empty_line() {
        for large in [false, true] {
            let ctx = egui::Context::default();
            let mut app = crate::LiteTextApp::default();
            app.new_document();
            let text = format!("{{\n{}  \"tail\": true\n}}\n", "  \"key\": 123,\n".repeat(if large { 25_000 } else { 3 }));
            let file = app.active_mut().unwrap();
            file.draft_text = Some(text.clone());
            file.sync_draft();
            file.language_override = Some("JSON".into());
            let draw = |app: &mut crate::LiteTextApp| {
                let _ = ctx.run(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1080.0, 720.0))), ..Default::default() }, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| crate::draft_editor(ui, app.active_mut().unwrap(), &crate::DARK, false, crate::Lang::En));
                });
            };
            draw(&mut app);
            app.active_mut().unwrap().editor_state.folds.toggle(0);
            assert!(app.active().unwrap().editor_state.folds.hidden(1));
            app.active_mut().unwrap().editor_state.tools.jump_line = "2".into();
            jump_to_line(app.active_mut().unwrap(), &ctx);
            draw(&mut app);
            assert!(!app.active().unwrap().editor_state.folds.hidden(1));
            assert_eq!(app.active().unwrap().editor_state.cursor.unwrap().primary.index, 2);
            app.active_mut().unwrap().editor_state.tools.jump_line = (text.bytes().filter(|&b| b == b'\n').count() + 1).to_string();
            jump_to_line(app.active_mut().unwrap(), &ctx);
            draw(&mut app);
            assert_eq!(app.active().unwrap().editor_state.cursor.unwrap().primary.index, text.chars().count());
            assert_eq!(app.active().unwrap().draft_text.as_deref(), Some(text.as_str()));
        }
    }

    #[test]
    fn bulk_html_paste_uses_viewport_editor_and_one_undo_without_saving() {
        use super::*;
        for (existing_large, replace) in [(false, false), (false, true), (true, false), (true, true)] {
            let ctx = egui::Context::default();
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("original.txt");
            let original = if existing_large { "old text\n".repeat(40_000) } else { "old text".into() };
            std::fs::write(&path, &original).unwrap();
            let mut app = crate::LiteTextApp::default();
            app.open_paths(vec![path.clone()]);
            app.active_mut().unwrap().language_override = Some("Python".into());
            let payload = "<div data-cost={\"renderCost\":\"0.4\",\"dataCost\":1}>中文</div>\n".repeat(12_000);
            let draw = |app: &mut crate::LiteTextApp, events| {
                crate::workspace_theme::configure(app.active_mut().unwrap(), &crate::workspace_theme::WorkspaceTheme::default(), true);
                let _ = ctx.run(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1080.0,720.0))), events, ..Default::default() }, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| crate::draft_editor(ui, app.active_mut().unwrap(), &crate::DARK, true, crate::Lang::En));
                });
            };
            draw(&mut app, vec![]);
            draw(&mut app, vec![]);
            let len = original.chars().count();
            app.active_mut().unwrap().editor_state.pending_selection = Some(if replace { 0..len } else { len..len });
            let started = std::time::Instant::now();
            draw(&mut app, vec![egui::Event::Paste(payload.clone())]);
            eprintln!("bulk HTML paste {} bytes: {:?}", payload.len(), started.elapsed());
            let file = app.active().unwrap();
            assert_eq!(file.draft_text.as_deref(), Some(if replace { payload.clone() } else { format!("{original}{payload}") }.as_str()));
            assert!(file.editor_state.large.active);
            assert!(file.editor_state.large.laid_out_bytes < 16 * 1024, "paste must not lay out the whole payload");
            draw(&mut app, vec![]);
            draw(&mut app, vec![egui::Event::Key { key: egui::Key::Z, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND }]);
            assert_eq!(app.active().unwrap().draft_text.as_deref(), Some(original.as_str()));
            assert_eq!(std::fs::read_to_string(path).unwrap(), original);
        }
    }

    #[test]
    fn jump_line_controls_filter_digits_align_right_and_reveal_logical_line_start() {
        use super::*;
        for large in [false, true] {
            for lang in [crate::Lang::ZhCn, crate::Lang::En] {
                let ctx = egui::Context::default();
                crate::setup_fonts(&ctx);
                let mut app = crate::LiteTextApp::default();
                app.new_document();
                let text = "中文🙂\r\nsecond line\r\n\r\n".repeat(if large { 15000 } else { 4 });
                app.active_mut().unwrap().draft_text = Some(text.clone());
                app.active_mut().unwrap().sync_draft();
                app.active_mut().unwrap().editor_state.tools.find_open = true;
                let draw = |app: &mut crate::LiteTextApp, events| {
                    ctx.run(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0,480.0))), events, ..Default::default() }, |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| crate::draft_editor(ui, app.active_mut().unwrap(), &crate::DARK, false, lang));
                    })
                };
                for _ in 0..3 { draw(&mut app, vec![]); }
                let id = app.active_id.unwrap();
                let rect = |key: &str| ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new((key,id)))).unwrap();
                let input = rect("jump_line_input_rect");
                let button = rect("jump_line_button_rect");
                assert!(input.left() > rect("find_count_rect").right());
                assert!(button.left() > input.right());
                assert!(rect("find_overlay_rect").contains_rect(button));
                assert!((button.center().y - rect("find_count_rect").center().y).abs() < 3.0);
                for pressed in [true, false] { draw(&mut app, vec![egui::Event::PointerMoved(input.center()), egui::Event::PointerButton { pos: input.center(), button: egui::PointerButton::Primary, pressed, modifiers: Default::default() }]); }
                let line = if large { 30002 } else { 2 };
                draw(&mut app, vec![egui::Event::Text(format!("a{line}x"))]);
                assert_eq!(app.active().unwrap().editor_state.tools.jump_line, line.to_string());
                draw(&mut app, vec![egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }]);
                draw(&mut app, vec![]);
                let expected = text.char_indices().filter(|(_,c)| *c == '\n').nth(line - 2).map(|(at,_)| text[..at + 1].chars().count()).unwrap();
                let file = app.active().unwrap();
                let cursor = file.editor_state.cursor.unwrap();
                assert_eq!(cursor.primary.index, expected);
                assert_eq!(cursor.secondary.index, expected);
                assert_eq!(file.editor_state.cursor_line, line - 1);
                assert_eq!(file.draft_text.as_deref(), Some(text.as_str()));
                for invalid in ["0", "99999999999999999999", "999999"] {
                    app.active_mut().unwrap().editor_state.tools.jump_line = invalid.into();
                    jump_to_line(app.active_mut().unwrap(), &ctx);
                    assert!(app.active().unwrap().editor_state.pending_selection.is_none());
                }
                app.active_mut().unwrap().editor_state.tools.jump_line = "1".into();
                let button = rect("jump_line_button_rect");
                for pressed in [true, false] { draw(&mut app, vec![egui::Event::PointerMoved(button.center()), egui::Event::PointerButton { pos: button.center(), button: egui::PointerButton::Primary, pressed, modifiers: Default::default() }]); }
                draw(&mut app, vec![]);
                assert_eq!(app.active().unwrap().editor_state.cursor.unwrap().primary.index, 0);
            }
        }
    }
    use super::*;

    fn cursor(start: usize, end: usize) -> Option<egui::text::CCursorRange> {
        Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(start),
            egui::text::CCursor::new(end),
        ))
    }

    #[test]
    fn search_input_line_breaks_are_faint_and_preserve_character_positions() {
        for pal in [&crate::DARK, &crate::LIGHT] {
            for text in ["", "普通文字", "中a\r\nb\n尾", "\n\n", "\\n"] {
                let job = search_input_layout(text, egui::FontId::proportional(13.0), pal);
                assert!(!job.break_on_newline);
                assert_eq!(job.text.chars().count(), text.chars().count());
                assert_eq!(job.text, text.replace('\r', "␍").replace('\n', "↵"));
                for section in &job.sections {
                    let content = &job.text[section.byte_range.clone()];
                    let is_marker = content == "↵" || content == "␍";
                    assert_eq!(
                        section.format.color,
                        if is_marker {
                            pal.text_faint
                        } else {
                            pal.text_main
                        }
                    );
                }
            }
        }
    }

    #[test]
    fn search_fields_show_pasted_newlines_without_changing_copy_or_delete() {
        for is_query in [true, false] {
            let ctx = egui::Context::default();
            let id = egui::Id::new("newline_input");
            let mut text = String::new();
            let run = |text: &mut String, events| {
                ctx.run(
                    egui::RawInput {
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            let mut options = [("Aa", "Case", false), (".*", "Regex", false)];
                            input_with_actions(
                                ui,
                                id,
                                text,
                                "Input",
                                500.0,
                                if is_query { &mut options } else { &mut [] },
                                if is_query {
                                    [(7, "Previous"), (8, "Next")]
                                } else {
                                    [(4, "Replace"), (5, "Replace all")]
                                },
                                &crate::DARK,
                            );
                        });
                    },
                )
            };
            run(&mut text, vec![]);
            ctx.memory_mut(|memory| memory.request_focus(id));
            let original = "中a\r\nb\n尾";
            let output = run(&mut text, vec![egui::Event::Paste(original.to_owned())]);
            assert_eq!(text, original);
            assert!(output.shapes.iter().any(|s| matches!(&s.shape,
                egui::Shape::Text(t) if t.galley.text() == "中a␍↵b↵尾" && t.galley.rows.len() == 1
            )));
            let mut state = egui::TextEdit::load_state(&ctx, id).unwrap();
            assert_eq!(
                state.cursor.char_range().unwrap().primary.index,
                original.chars().count()
            );
            state
                .cursor
                .set_char_range(cursor(0, original.chars().count()));
            state.store(&ctx, id);
            let output = run(&mut text, vec![egui::Event::Copy]);
            assert!(
                output
                    .platform_output
                    .commands
                    .iter()
                    .any(|cmd| matches!(cmd,
                        egui::OutputCommand::CopyText(value) if value == original
                    ))
            );
            let mut state = egui::TextEdit::load_state(&ctx, id).unwrap();
            state.cursor.set_char_range(cursor(2, 4)); // Select the actual CRLF, not display glyph bytes.
            state.store(&ctx, id);
            run(
                &mut text,
                vec![egui::Event::Key {
                    key: egui::Key::Backspace,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                }],
            );
            assert_eq!(text, "中ab\n尾");
        }
    }

    #[test]
    fn choosing_pen_from_toolbar_keeps_it_active_across_repeated_editor_drags() {
        for large in [false, true] {
            let ctx = egui::Context::default();
            let mut app = crate::LiteTextApp::default();
            app.new_document();
            let id = app.active_id.unwrap();
            let original = format!(
                "abcdefghij\nklmnopqrst\nuvwxyz0123{}",
                "\nshort".repeat(if large { 50_000 } else { 0 })
            );
            app.active_mut().unwrap().draft_text = Some(original.clone());
            app.active_mut().unwrap().sync_draft();
            let mut frame = 0;
            let mut run = |app: &mut crate::LiteTextApp, events| {
                frame += 1;
                ctx.run(
                    egui::RawInput {
                        time: Some(frame as f64 * 0.12),
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1080.0, 720.0),
                        )),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        app.top_bar(ctx, &crate::DARK);
                        egui::CentralPanel::default().show(ctx, |ui| {
                            crate::draft_editor(
                                ui,
                                app.active_mut().unwrap(),
                                &crate::DARK,
                                false,
                                crate::Lang::En,
                            )
                        });
                    },
                )
            };
            let button = |pos, pressed| {
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        pressed,
                        button: egui::PointerButton::Primary,
                        modifiers: Default::default(),
                    },
                ]
            };
            run(&mut app, vec![]);
            run(&mut app, vec![]);
            let pen = ctx
                .data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("text_tool_rect", id, 1i32))))
                .unwrap()
                .center();
            run(&mut app, button(pen, true));
            run(&mut app, button(pen, false));
            run(&mut app, vec![]);
            let red = ctx
                .data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("pen_color_rect", id, 0usize))))
                .unwrap()
                .center();
            run(&mut app, button(red, true));
            run(&mut app, button(red, false));
            let output = run(&mut app, vec![]);
            let (pos, rows) = output
                .shapes
                .into_iter()
                .find_map(|s| {
                    if let egui::Shape::Text(t) = s.shape
                        && t.galley.text().starts_with("abcdefghij")
                    {
                        Some((t.pos, t.galley.rows.clone()))
                    } else {
                        None
                    }
                })
                .unwrap();
            let point = |row: usize, column: usize| {
                pos + egui::vec2(rows[row].glyphs[column].pos.x, rows[row].rect.center().y)
            };
            for (row, a, b) in [(0, 0, 3), (1, 2, 5), (2, 4, 8), (0, 5, 8)] {
                run(&mut app, button(point(row, a), true));
                run(&mut app, vec![egui::Event::PointerMoved(point(row, b))]);
                run(&mut app, button(point(row, b), false));
                run(&mut app, vec![]);
                assert_eq!(app.active().unwrap().editor_state.tools.pen, Some(0));
            }
            assert_eq!(
                app.active().unwrap().editor_state.tools.marks.len(),
                4,
                "large={large}"
            );
            // A release outside TextEdit (e.g. over the frozen gutter) still
            // commits the gesture and must not disarm the next selection.
            let outside = egui::pos2(0.0, point(1, 0).y);
            run(&mut app, button(point(1, 9), true));
            run(&mut app, vec![egui::Event::PointerMoved(outside)]);
            run(&mut app, button(outside, false));
            assert!(
                app.active()
                    .unwrap()
                    .editor_state
                    .tools
                    .marks
                    .iter()
                    .any(|m| m.range == (11..20))
            );
            run(&mut app, button(point(2, 0), true));
            run(&mut app, vec![egui::Event::PointerMoved(point(2, 2))]);
            run(&mut app, button(point(2, 2), false));
            assert!(
                app.active()
                    .unwrap()
                    .editor_state
                    .tools
                    .marks
                    .iter()
                    .any(|m| m.range == (22..24))
            );
            assert_eq!(app.active().unwrap().editor_state.tools.pen, Some(0));
            assert_eq!(
                app.active().unwrap().draft_text.as_deref(),
                Some(original.as_str())
            );
        }
    }

    #[test]
    fn pen_dropdown_uses_compact_colored_icons_and_centers_the_trash_button() {
        for pal in [&crate::DARK, &crate::LIGHT] {
            let ctx = egui::Context::default();
            ctx.style_mut(|style| style.animation_time = 0.0);
            let mut app = crate::LiteTextApp::default();
            app.new_document();
            let file = app.active_mut().unwrap();
            let id = file.id;
            file.editor_state.tools.pen_menu = true;
            file.editor_state.tools.pen = Some(2);
            let mut run = || {
                ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1080.0, 720.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| app.top_bar(ctx, pal),
                )
            };
            run();
            let output = run();
            let menu = ctx
                .data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("pen_menu_rect", id))))
                .unwrap();
            // Only tooltip/accessibility labels remain; none are painted in the menu.
            assert!(!output.shapes.iter().any(|s| matches!(&s.shape,
                egui::Shape::Text(text) if menu.contains(text.pos)
            )));
            let mut last_row_bottom = 0.0;
            for index in 0..5usize {
                let rect = |key| {
                    ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new((key, id, index))))
                        .unwrap()
                };
                let pen = rect("pen_color_rect");
                let eraser = rect("pen_clear_rect");
                assert_eq!(pen.size(), egui::vec2(40.0, 24.0));
                assert_eq!(eraser.size(), egui::vec2(26.0, 24.0));
                assert!(eraser.left() > pen.right());
                assert!(eraser.left() - pen.right() <= 4.0);
                assert!((eraser.center().y - pen.center().y).abs() < 1.0);
                assert!(pen.top() > last_row_bottom);
                last_row_bottom = pen.bottom();
                for button in [pen, eraser] {
                    assert!(menu.contains_rect(button));
                    let path = output
                        .shapes
                        .iter()
                        .find_map(|s| match &s.shape {
                            egui::Shape::Path(path)
                                if path.closed
                                    && path.points.iter().all(|p| button.contains(*p)) =>
                            {
                                Some(path)
                            }
                            _ => None,
                        })
                        .expect("colored pen/eraser outline");
                    assert_eq!(
                        path.stroke.color,
                        egui::epaint::ColorMode::Solid(pen_color(index))
                    );
                    let bounds = egui::Rect::from_points(&path.points);
                    assert!((10.0..=14.0).contains(&bounds.width()));
                    assert!((10.0..=14.0).contains(&bounds.height()));
                    assert!((bounds.center().x - button.center().x).abs() < 2.0);
                }
                assert!(output.shapes.iter().any(|s| matches!(&s.shape,
                    egui::Shape::Rect(r) if r.rect == pen
                        && r.fill == if index == 2 { pen_color(index).gamma_multiply(0.2) } else { pal.bg_elevated }
                )));
            }
            let trash = ctx
                .data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("pen_clear_all_rect", id))))
                .unwrap();
            assert_eq!(trash.size(), egui::vec2(28.0, 28.0));
            assert!(trash.top() > last_row_bottom);
            assert!(
                (trash.center().x - menu.center().x).abs() < 1.0,
                "menu={menu:?}, trash={trash:?}"
            );
            assert!(output.shapes.iter().any(|s| matches!(&s.shape,
                egui::Shape::Path(path) if !path.closed && path.points.len() == 4
                    && path.points.iter().all(|p| trash.contains(*p))
            )));
        }
    }

    #[test]
    fn pen_dropdown_selects_color_clears_only_that_color_and_clears_all() {
        let ctx = egui::Context::default();
        let mut app = crate::LiteTextApp::default();
        app.new_document();
        let id = app.active_id.unwrap();
        let run = |app: &mut crate::LiteTextApp, events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1080.0, 720.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| app.top_bar(ctx, &crate::DARK),
            )
        };
        let click = |app: &mut crate::LiteTextApp, pos| {
            for pressed in [true, false] {
                run(
                    app,
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            pressed,
                            button: egui::PointerButton::Primary,
                            modifiers: Default::default(),
                        },
                    ],
                );
            }
            run(app, vec![]);
        };
        run(&mut app, vec![]);
        run(&mut app, vec![]);
        let rect = ctx
            .data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("text_tool_rect", id, 1i32))))
            .unwrap();
        click(&mut app, rect.center());
        run(&mut app, vec![]);
        let red = ctx
            .data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("pen_color_rect", id, 0usize))))
            .unwrap();
        assert!(red.top() > rect.bottom());
        click(&mut app, red.center());
        assert_eq!(app.active().unwrap().editor_state.tools.pen, Some(0));
        assert!(!app.active().unwrap().editor_state.tools.pen_menu);
        // Selecting the active color again still exits pen mode.
        click(&mut app, rect.center());
        click(&mut app, red.center());
        assert!(app.active().unwrap().editor_state.tools.pen.is_none());
        click(&mut app, rect.center());
        click(&mut app, red.center());
        assert_eq!(app.active().unwrap().editor_state.tools.pen, Some(0));
        let tools = &mut app.active_mut().unwrap().editor_state.tools;
        tools.mark_selection("abcdef", cursor(0, 2));
        tools.mark_selection("abcdef", cursor(4, 6));
        tools.pen = Some(2);
        tools.mark_selection("abcdef", cursor(2, 4));
        click(&mut app, rect.center());
        run(&mut app, vec![]);
        let clear = ctx
            .data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("pen_clear_rect", id, 0usize))))
            .unwrap();
        click(&mut app, clear.center());
        assert_eq!(
            app.active().unwrap().editor_state.tools.marks,
            [Mark {
                range: 2..4,
                color: 2
            }]
        );
        // The sixth pen can apply the already-retained editor selection, and
        // its dedicated eraser must leave ordinary markers alone.
        app.active_mut().unwrap().draft_text = Some("abcdef".into());
        app.active_mut().unwrap().editor_state.cursor = cursor(0, 2);
        let conceal = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("pen_color_rect", id, CONCEAL)))).unwrap();
        click(&mut app, conceal.center());
        assert_eq!(app.active().unwrap().editor_state.tools.pen, Some(CONCEAL));
        assert!(app.active().unwrap().editor_state.tools.marks.contains(&Mark { range: 0..2, color: CONCEAL }));
        click(&mut app, rect.center());
        run(&mut app, vec![]);
        let clear_conceal = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("pen_clear_rect", id, CONCEAL)))).unwrap();
        click(&mut app, clear_conceal.center());
        assert_eq!(app.active().unwrap().editor_state.tools.marks, [Mark { range: 2..4, color: 2 }]);
        let all = ctx
            .data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("pen_clear_all_rect", id))))
            .unwrap();
        click(&mut app, all.center());
        assert!(app.active().unwrap().editor_state.tools.marks.is_empty());
        assert!(app.active().unwrap().editor_state.tools.pen.is_none());
    }

    #[test]
    fn floating_find_panel_never_moves_editor_and_owns_input_over_the_document() {
        for large in [false, true] {
            for wrap in [false, true] {
                let ctx = egui::Context::default();
                let mut app = crate::LiteTextApp::default();
                app.new_document();
                let original = format!(
                    "first line {}\nsecond line{}",
                    "abc ".repeat(100),
                    "\nshort".repeat(if large { 50_000 } else { 0 })
                );
                let file = app.active_mut().unwrap();
                file.draft_text = Some(original.clone());
                file.sync_draft();
                file.editor_state.tools.wrap = wrap;
                let id = file.id;
                let run = |app: &mut crate::LiteTextApp, events| {
                    ctx.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(1080.0, 720.0),
                            )),
                            events,
                            ..Default::default()
                        },
                        |ctx| {
                            app.top_bar(ctx, &crate::DARK);
                            egui::CentralPanel::default().show(ctx, |ui| {
                                crate::draft_editor(
                                    ui,
                                    app.active_mut().unwrap(),
                                    &crate::DARK,
                                    false,
                                    crate::Lang::En,
                                );
                            });
                        },
                    )
                };
                let geometry = |output: egui::FullOutput| {
                    let text = output
                        .shapes
                        .iter()
                        .find_map(|s| {
                            if let egui::Shape::Text(t) = &s.shape {
                                t.galley
                                    .text()
                                    .starts_with("first line")
                                    .then_some((t.pos, t.galley.size()))
                            } else {
                                None
                            }
                        })
                        .unwrap();
                    let number = output
                        .shapes
                        .iter()
                        .find_map(|s| {
                            if let egui::Shape::Text(t) = &s.shape {
                                (t.galley.text() == "1").then_some(t.pos)
                            } else {
                                None
                            }
                        })
                        .unwrap();
                    (text, number)
                };
                run(&mut app, vec![]);
                let before = geometry(run(&mut app, vec![]));
                app.active_mut().unwrap().editor_state.tools.find_open = true;
                for _ in 0..3 {
                    assert_eq!(geometry(run(&mut app, vec![])), before);
                }
                let input = ctx
                    .data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("find_input_rect", id))))
                    .unwrap();
                let overlay = ctx
                    .data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("find_overlay_rect", id))))
                    .unwrap();
                assert!(overlay.contains_rect(input));
                assert!(overlay.top() > before.1.y - 20.0 && overlay.right() < 1080.0);
                for pressed in [true, false] {
                    run(
                        &mut app,
                        vec![
                            egui::Event::PointerMoved(input.center()),
                            egui::Event::PointerButton {
                                pos: input.center(),
                                pressed,
                                button: egui::PointerButton::Primary,
                                modifiers: Default::default(),
                            },
                        ],
                    );
                }
                run(&mut app, vec![egui::Event::Text("needle".to_owned())]);
                assert_eq!(app.active().unwrap().editor_state.tools.query, "needle");
                assert_eq!(
                    app.active().unwrap().draft_text.as_deref(),
                    Some(original.as_str())
                );
                app.active_mut().unwrap().editor_state.tools.find_open = false;
                assert_eq!(geometry(run(&mut app, vec![])), before);
            }
        }
    }

    #[test]
    fn inline_search_options_are_quiet_when_off_and_accented_when_selected() {
        for pal in [&crate::DARK, &crate::LIGHT] {
            for selected in [false, true] {
                let ctx = egui::Context::default();
                let output = ctx.run(egui::RawInput::default(), |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let response = search_option(ui, "Aa", "Ignore case", selected, pal);
                        assert_eq!(response.rect.size(), egui::vec2(24.0, 20.0));
                    });
                });
                let backgrounds = output
                    .shapes
                    .iter()
                    .filter(|s| {
                        matches!(&s.shape, egui::Shape::Rect(r)
                    if r.fill == pal.accent.gamma_multiply(0.18))
                    })
                    .count();
                assert_eq!(backgrounds, usize::from(selected));
                assert!(output.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Text(t)
                    if t.galley.text() == "Aa" && t.fallback_color == if selected { pal.accent } else { pal.text_faint })));
            }
        }
    }

    #[test]
    fn find_panel_toggle_never_overlaps_the_count_or_jump_controls() {
        for width in [320.0, 400.0, 1080.0] {
            let ctx = egui::Context::default();
            crate::setup_fonts(&ctx);
            let mut app = crate::LiteTextApp::default();
            app.new_document();
            let file = app.active_mut().unwrap();
            file.draft_text = Some("A a A".to_owned());
            file.sync_draft();
            file.editor_state.tools.find_open = true;
            file.editor_state.tools.query = "a".to_owned();
            file.editor_state.tools.count = true;
            file.editor_state.tools.count_all(
                file.draft_text.as_deref().unwrap(),
                (file.content_revision, 0),
            );
            let mut run = || {
                ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 600.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            find_bar(ui, app.active_mut().unwrap(), &crate::DARK, crate::tr(crate::Lang::En))
                        });
                    },
                )
            };
            run();
            run();
            let id = app.active_id.unwrap();
            let rect = |key: &str| {
                ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new((key, id))))
                    .unwrap()
            };
            let toggle = rect("find_replace_toggle_rect");
            assert!(!toggle.intersects(rect("jump_line_input_rect")), "width={width}: {toggle:?}");
            assert!(!toggle.intersects(rect("jump_line_button_rect")), "width={width}: {toggle:?}");
            assert!(!toggle.intersects(rect("find_count_rect")), "width={width}: {toggle:?}");
            if let Some(result) = ctx.data(|d| {
                d.get_temp::<egui::Rect>(egui::Id::new(("find_count_result_rect", id)))
            }) {
                assert!(!toggle.intersects(result), "width={width}: {toggle:?} vs {result:?}");
            }
            assert!(
                rect("find_overlay_rect").contains_rect(toggle),
                "width={width}: the toggle must stay inside the panel"
            );
            assert!(
                !app.active().unwrap().editor_state.tools.replace_open,
                "the toggle must not be clickable through another widget"
            );
        }
    }

    #[test]
    fn find_panel_starts_compact_and_the_chevron_unfolds_the_replace_row() {
        let ctx = egui::Context::default();
        let mut app = crate::LiteTextApp::default();
        app.new_document();
        let file = app.active_mut().unwrap();
        file.draft_text = Some("A a A".to_owned());
        file.sync_draft();
        file.editor_state.tools = Tools {
            find_open: true,
            query: "a".to_owned(),
            replacement: "new".to_owned(),
            count: true,
            ignore_case: true,
            ..Default::default()
        };
        file.editor_state.tools.count_all(
            file.draft_text.as_deref().unwrap(),
            (file.content_revision, 0),
        );
        let run = |file: &mut crate::LoadedFile, events: Vec<egui::Event>| {
            ctx.run(
                egui::RawInput {
                    events,
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1080.0, 720.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        find_bar(ui, file, &crate::DARK, crate::tr(crate::Lang::En))
                    });
                },
            )
        };
        let click = |file: &mut crate::LoadedFile, pos: egui::Pos2| {
            for pressed in [true, false] {
                run(
                    file,
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                );
            }
        };
        let texts = |output: &egui::FullOutput| {
            output
                .shapes
                .iter()
                .filter_map(|s| {
                    if let egui::Shape::Text(t) = &s.shape {
                        Some((t.galley.text().to_owned(), t.pos))
                    } else {
                        None
                    }
                })
                .collect::<std::collections::HashMap<_, _>>()
        };
        let id = file.id;
        let rect = |key: &str| {
            ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new((key, id))))
                .unwrap()
        };
        let option_rect = |label: &str| {
            ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("find_query", id)).with(label)))
                .unwrap()
        };

        // Default state: find field plus count/jump row, replace row folded.
        assert!(!file.editor_state.tools.replace_open);
        run(file, vec![]);
        let output = run(file, vec![]);
        let positions = texts(&output);
        assert!(positions["a"].x > 500.0);
        for label in ["Aa", "Count", ".*", "3"] {
            assert!(positions.contains_key(label), "missing {label}");
        }
        for label in ["new", "Replace", "Replace all"] {
            assert!(!positions.contains_key(label), "unexpected {label}");
        }
        let find_field = rect("find_field_rect");
        let find_input = rect("find_input_rect");
        let aa = option_rect("Aa");
        let reg = option_rect(".*");
        assert!(find_field.contains_rect(aa));
        assert!(find_field.contains_rect(reg));
        assert!(find_input.right() + 10.0 <= aa.left());
        assert!(aa.right() < reg.left() && reg.right() < rect("find_previous_rect").left());
        let count = rect("find_count_rect");
        let result = rect("find_count_result_rect");
        assert!(result.left() > count.right());
        assert!((result.center().y - count.center().y).abs() < 2.0);
        let jump_input = rect("jump_line_input_rect");
        let jump_button = rect("jump_line_button_rect");
        assert!(jump_input.left() > count.right());
        assert!(jump_button.left() > jump_input.right());
        assert!(rect("find_overlay_rect").contains_rect(jump_button));
        assert!((jump_button.center().y - count.center().y).abs() < 3.0);

        // The expand handle is a slim chevron in the middle of that row, not a
        // full-height button.
        let overlay = rect("find_overlay_rect");
        let toggle = rect("find_replace_toggle_rect");
        assert!(toggle.height() < 24.0, "toggle must be slim: {toggle:?}");
        assert!(toggle.width() <= 40.0, "toggle must be small: {toggle:?}");
        assert!(
            (toggle.center().x - overlay.center().x).abs() < 2.5,
            "toggle must be centered: {toggle:?}, overlay={overlay:?}"
        );
        assert!(
            (toggle.center().y - count.center().y).abs() < 3.0,
            "toggle must sit in the count/jump row: {toggle:?}"
        );
        let compact_height = overlay.height();

        // Expanding reveals the replace row and hides count/jump.
        click(file, toggle.center());
        assert!(file.editor_state.tools.replace_open);
        let output = run(file, vec![]);
        let positions = texts(&output);
        assert!(positions.contains_key("new"));
        assert!(positions["new"].y > positions["a"].y);
        assert!((positions["new"].x - positions["a"].x).abs() < 1.0);
        for label in ["Count", "3"] {
            assert!(!positions.contains_key(label), "count row must hide: {label}");
        }
        let replace_field = rect("replace_field_rect");
        let replace_input = rect("replace_input_rect");
        for (name, input, first, second) in [
            (
                "find",
                find_input,
                rect("find_previous_rect"),
                rect("find_next_rect"),
            ),
            (
                "replace",
                replace_input,
                rect("replace_rect"),
                rect("replace_all_rect"),
            ),
        ] {
            let field = if name == "find" { find_field } else { replace_field };
            let a = first;
            let b = second;
            assert!(field.contains_rect(input) && field.contains_rect(a) && field.contains_rect(b));
            assert!(
                a.left() >= input.right() + 10.0,
                "{name} actions need a separated trailing region"
            );
            assert!(b.left() >= a.right());
            assert!((a.center().y - input.center().y).abs() < 2.0);
        }
        assert!(
            (find_field.right() - replace_field.right()).abs() < 1.0,
            "the two fields must share their right edge"
        );
        for field in [find_field, replace_field] {
            let overlay = rect("find_overlay_rect");
            assert!(
                overlay.right() - field.right() < 16.0,
                "unused right-side space: overlay={overlay:?}, field={field:?}"
            );
            assert!(field.width() > overlay.width() - 24.0);
        }
        let overlay = rect("find_overlay_rect");
        assert!(
            overlay.height() > compact_height + 10.0,
            "the expanded panel must be taller"
        );
        let collapse = rect("find_replace_toggle_rect");
        assert!(collapse.height() < 24.0);
        assert!(
            collapse.top() > replace_input.bottom(),
            "the collapse handle belongs under the replace field"
        );
        assert!(
            (collapse.center().x - overlay.center().x).abs() < 2.5,
            "the collapse handle must stay centered"
        );

        // Toggling again folds it back to the default layout.
        click(file, collapse.center());
        assert!(!file.editor_state.tools.replace_open);
        let positions = texts(&run(file, vec![]));
        assert!(!positions.contains_key("new"));
        assert!(positions.contains_key("Count"));

        // Long text is clipped to the editable region, not drawn beneath actions.
        file.editor_state.tools.query = "long input ".repeat(100);
        let output = run(file, vec![]);
        let first_action = rect("find_previous_rect");
        let text_shape = output.shapes.iter().find(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text().starts_with("long input"))).unwrap();
        assert!(text_shape.clip_rect.right() < first_action.left() - 5.0);
    }

    #[test]
    fn embedded_search_option_buttons_toggle_and_count_uses_chinese_label() {
        let ctx = egui::Context::default();
        let mut app = crate::LiteTextApp::default();
        app.new_document();
        let file = app.active_mut().unwrap();
        file.editor_state.tools.find_open = true;
        let run = |file: &mut crate::LoadedFile, events| {
            ctx.run(
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
                        find_bar(ui, file, &crate::DARK, crate::tr(crate::Lang::ZhCn))
                    });
                },
            )
        };
        run(file, vec![]);
        let output = run(file, vec![]);
        assert!(
            output
                .shapes
                .iter()
                .any(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text() == "计数"))
        );
        for label in ["Aa", ".*", "计数"] {
            let rect = ctx
                .data(|d| {
                    if label == "计数" {
                        d.get_temp::<egui::Rect>(egui::Id::new(("find_count_rect", file.id)))
                    } else {
                        d.get_temp::<egui::Rect>(egui::Id::new(("find_query", file.id)).with(label))
                    }
                })
                .unwrap();
            for pressed in [true, false] {
                run(
                    file,
                    vec![
                        egui::Event::PointerMoved(rect.center()),
                        egui::Event::PointerButton {
                            pos: rect.center(),
                            pressed,
                            button: egui::PointerButton::Primary,
                            modifiers: Default::default(),
                        },
                    ],
                );
            }
        }
        assert!(
            file.editor_state.tools.ignore_case
                && file.editor_state.tools.regex
                && file.editor_state.tools.count
        );
        let output = run(file, vec![]);
        assert!(
            output
                .shapes
                .iter()
                .any(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text() == "未查询到"))
        );
        file.draft_text = Some("hello HELLO".to_owned());
        file.sync_draft();
        file.editor_state.tools.query = "hello".to_owned();
        file.editor_state.tools.count_all(
            file.draft_text.as_deref().unwrap(),
            (file.content_revision, 0),
        );
        let output = run(file, vec![]);
        assert!(
            output
                .shapes
                .iter()
                .any(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text() == "2"))
        );
        file.editor_state.tools.query = "missing".to_owned();
        file.editor_state.tools.count_all(
            file.draft_text.as_deref().unwrap(),
            (file.content_revision, 0),
        );
        let output = run(file, vec![]);
        assert!(
            output
                .shapes
                .iter()
                .any(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text() == "未查询到"))
        );
    }

    #[test]
    fn search_is_on_demand_cursor_based_and_only_current_match_is_highlighted() {
        for large in [false, true] {
            let ctx = egui::Context::default();
            let mut app = crate::LiteTextApp::default();
            app.new_document();
            let file = app.active_mut().unwrap();
            let original = format!(
                "hit middle hit end hit{}",
                "\nunchanged".repeat(if large { 30_000 } else { 0 })
            );
            file.draft_text = Some(original.clone());
            file.sync_draft();
            file.editor_state.tools.find_open = true;
            file.editor_state.tools.query = "hit".to_owned();
            file.editor_state.tools.replacement = "NEW".to_owned();
            // Just rendering with a query must not even compile a matcher.
            for _ in 0..3 {
                let _ = ctx.run(egui::RawInput::default(), |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        find_bar(ui, file, &crate::DARK, crate::tr(crate::Lang::En))
                    });
                });
            }
            assert!(file.editor_state.tools.matcher.is_none());
            assert!(file.editor_state.tools.matches.is_empty());
            assert!(file.editor_state.tools.current.is_none());
            file.editor_state.cursor = cursor(5, 5);
            let run = |app: &mut crate::LiteTextApp, action| {
                search_action(app.active_mut().unwrap(), &ctx, action);
                background_search::drain(app, &ctx);
            };
            run(&mut app, 3); // Single replace without a located result does nothing.
            let file = app.active_mut().unwrap();
            assert_eq!(file.draft_text.as_deref(), Some(original.as_str()));
            run(&mut app, 2);
            let file = app.active_mut().unwrap();
            assert_eq!(file.editor_state.tools.current, Some(11..14));
            assert!(file.editor_state.tools.matches.is_empty());
            let job = layout(
                "hit middle hit end hit",
                egui::FontId::monospace(13.5),
                &crate::DARK,
                f32::INFINITY,
                &file.editor_state.tools,
            );
            let painted: Vec<_> = job
                .sections
                .iter()
                .filter(|s| s.format.background != egui::Color32::TRANSPARENT)
                .map(|s| s.byte_range.clone())
                .collect();
            assert_eq!(painted.len(), 1);
            assert_eq!(painted[0], 11..14);
            // Mimic the editor applying the pending result selection.
            file.editor_state.cursor = cursor(11, 14);
            file.editor_state.pending_selection = None;
            run(&mut app, 2);
            let file = app.active_mut().unwrap();
            assert_eq!(file.editor_state.tools.current, Some(19..22));
            file.editor_state.cursor = cursor(19, 22);
            file.editor_state.pending_selection = None;
            run(&mut app, 1);
            let file = app.active_mut().unwrap();
            assert_eq!(file.editor_state.tools.current, Some(11..14));
            file.editor_state.cursor = cursor(11, 14);
            file.editor_state.pending_selection = None;
            run(&mut app, 3);
            let file = app.active_mut().unwrap();
            assert!(
                file.draft_text
                    .as_deref()
                    .unwrap()
                    .starts_with("hit middle NEW end hit")
            );
            // Replacing selects the next hit right away, so the pending
            // selection points at the remaining "hit" behind "end".
            assert_eq!(file.editor_state.tools.current, Some(19..22));
            let pending = file.editor_state.pending_selection.clone().unwrap();
            assert_eq!((pending.start, pending.end), (19, 22));
            // The second replace consumes that hit without extra navigation.
            file.editor_state.cursor = cursor(19, 22);
            file.editor_state.pending_selection = None;
            run(&mut app, 3);
            let file = app.active_mut().unwrap();
            assert!(
                file.draft_text.as_deref().unwrap().starts_with("hit middle NEW end NEW"),
                "the second replace must advance: {:?}",
                file.draft_text
            );
            // Wrapping lands back on the first hit.
            assert_eq!(file.editor_state.tools.current, Some(0..3));
            let revision = (file.content_revision, file.editor_state.large.revision);
            file.editor_state
                .tools
                .count_all(file.draft_text.as_deref().unwrap(), revision);
            assert_eq!(file.editor_state.tools.count_result, Some(1));
            assert!(file.editor_state.tools.matches.is_empty());
            file.editor_state.tools.query = "middle".to_owned();
            file.editor_state
                .tools
                .invalidate_search_if_changed(revision);
            assert!(file.editor_state.tools.matcher.is_none());
            assert!(file.editor_state.tools.count_result.is_none());
        }
    }

    #[test]
    fn on_demand_regex_navigation_wraps_and_advances_zero_width_results() {
        let mut tools = Tools {
            query: "^|$".to_owned(),
            regex: true,
            ..Default::default()
        };
        assert!(tools.prepare_matcher((0, 0)));
        assert_eq!(tools.find_one("中文", 0, false, false), Some(0..0));
        assert_eq!(tools.find_one("中文", 0, false, true), Some(6..6));
        assert_eq!(tools.find_one("中文", 6, false, true), Some(0..0));
        assert_eq!(tools.find_one("中文", 6, true, true), Some(0..0));
        assert_eq!(tools.find_one("中文", 0, true, true), Some(6..6));
        assert!(tools.matches.is_empty());
    }

    #[test]
    fn five_color_marks_are_positional_recolorable_and_follow_unicode_edits() {
        let text = "中文 abc 中文 xyz";
        let mut tools = Tools {
            pen: Some(0),
            ..Default::default()
        };
        tools.mark_selection(text, cursor(0, 2));
        tools.mark_selection(text, cursor(3, 6));
        assert_eq!(tools.marks.len(), 2);
        assert_eq!(
            tools.marks[0],
            Mark {
                range: 0..6,
                color: 0
            }
        );
        tools.pen = Some(2);
        tools.mark_selection(text, cursor(7, 9));
        assert_eq!(
            tools.marks[2],
            Mark {
                range: 11..17,
                color: 2
            }
        );
        tools.marks.retain(|mark| mark.color != 0);
        assert_eq!(
            tools.marks,
            [Mark {
                range: 11..17,
                color: 2
            }]
        );
        tools.apply_edit(0..0, 3);
        assert_eq!(tools.marks[0].range, 14..20);
        tools.apply_edit(14..17, 0);
        assert_eq!(tools.marks[0].range, 14..17);
        tools.apply_edit(14..17, 0);
        assert!(tools.marks.is_empty());
        for color in 0..5 {
            tools.pen = Some(color);
            tools.mark_selection("abcde", cursor(color, color + 1));
        }
        assert_eq!(tools.marks.len(), 5);
        tools.pen = Some(0);
        tools.mark_selection("abcde", cursor(1, 4));
        assert_eq!(
            tools
                .marks
                .iter()
                .map(|m| (m.range.clone(), m.color))
                .collect::<Vec<_>>(),
            [(0..1, 0), (1..4, 0), (4..5, 4)]
        );
    }

    #[test]
    fn conceal_pen_icon_is_solid_not_an_outline() {
        let ctx = egui::Context::default();
        let output = ctx.run(Default::default(), |ctx| { egui::CentralPanel::default().show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(24.0,24.0), egui::Sense::hover());
            paint_icon(ui.painter(), rect, egui::Color32::WHITE, 10);
        }); });
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Path(path) if path.closed && path.fill == egui::Color32::WHITE)));
    }

    #[test]
    fn conceal_window_offsets_edits_and_clear_preserve_original() {
        let original = "前缀\n中🙂secret尾";
        let offset = "前缀\n".len();
        let mut tools = Tools { pen: Some(CONCEAL), find_open: true,
            current: Some(offset..original.len()), ..Default::default() };
        tools.mark_selection(original, cursor(3, 12));
        for pal in [&crate::DARK, &crate::LIGHT] {
            let job = layout_window(&original[offset..], egui::FontId::monospace(14.0), pal,
                200.0, &tools, offset);
            assert_eq!(job.text, &original[offset..]);
            for section in &job.sections {
                assert_eq!(section.format.color, egui::Color32::TRANSPARENT);
                assert_eq!(section.format.background.a(), 255);
                assert_eq!(section.format.background, pal.text_main.to_opaque());
                assert_eq!(section.format.underline, egui::Stroke::NONE);
            }
        }
        tools.apply_edit(0..0, 4);
        assert_eq!(tools.marks[0].range.start, offset + 4);
        tools.apply_edit(offset + 4..offset + 7, 0);
        assert_eq!(tools.marks[0].range.end, original.len() + 1);
        tools.marks.retain(|mark| mark.color != CONCEAL);
        assert!(tools.marks.is_empty());
        let job = layout(original, egui::FontId::monospace(14.0), &crate::DARK,
            200.0, &tools);
        assert!(job.sections.iter().all(|s| s.format.color != egui::Color32::TRANSPARENT));
        assert_eq!(original, "前缀\n中🙂secret尾");
    }

    #[test]
    fn conceal_both_editors_reveal_only_selection_copy_original_and_rehide() {
        for large in [false, true] {
            for pal in [&crate::DARK, &crate::LIGHT] {
                for workspace in [false, true] {
                    let ctx = egui::Context::default();
                    let mut app = crate::LiteTextApp::default();
                    app.new_document();
                    let original = format!("abcdef secret\n{}", "padding\n".repeat(if large { 35_000 } else { 0 }));
                    let file = app.active_mut().unwrap();
                    file.draft_text = Some(original.clone());
                    file.editor_state.tools.pen = Some(CONCEAL);
                    file.editor_state.tools.mark_selection(&original, cursor(0, 6));
                    file.editor_state.tools.pen = None;
                    if workspace {
                        crate::workspace_theme::configure(file, &Default::default(), std::ptr::eq(pal, &crate::DARK));
                    }
                    let id = egui::Id::new(if large { "large_editor" } else { "draft_editor" }).with(file.id);
                    let draw = |app: &mut crate::LiteTextApp, events| {
                        ctx.run(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0))), events, ..Default::default() }, |ctx| {
                            egui::CentralPanel::default().show(ctx, |ui| {
                                crate::draft_editor(ui, app.active_mut().unwrap(), pal, true, crate::i18n::Lang::En);
                            });
                        })
                    };
                    draw(&mut app, vec![]);
                    ctx.memory_mut(|m| m.request_focus(id));
                    for selection in [0..0, 1..3, 0..0] {
                        app.active_mut().unwrap().editor_state.pending_selection = Some(selection.clone());
                        let frame = draw(&mut app, vec![]);
                        let painted = frame.shapes.iter().find_map(|shape| match &shape.shape {
                            egui::Shape::Text(text) if text.galley.text().starts_with("abcdef secret") => Some(&text.galley),
                            _ => None,
                        }).unwrap();
                        let row = &painted.rows[0];
                        let quads = row.visuals.mesh.vertices[row.visuals.glyph_vertex_range.clone()].as_chunks::<4>().0;
                        for (index, quad) in quads.iter().take(6).enumerate() {
                            assert_eq!(quad[0].color.a() != 0, selection.contains(&index), "large={large} workspace={workspace} index={index}");
                        }
                        assert!(row.visuals.mesh.vertices[..row.visuals.glyph_vertex_range.start].iter().any(|v| v.color.a() == 255));
                        assert_eq!(app.active().unwrap().draft_text.as_deref(), Some(original.as_str()));
                    }
                    // Exercise a real mouse drag over the concealed text too.
                    let frame = draw(&mut app, vec![]);
                    let (pos, galley) = frame.shapes.iter().find_map(|s| match &s.shape {
                        egui::Shape::Text(t) if t.galley.text().starts_with("abcdef secret") => Some((t.pos, &t.galley)), _ => None,
                    }).unwrap();
                    let point = |column| pos + egui::vec2(galley.rows[0].x_offset(column), galley.rows[0].rect.center().y);
                    draw(&mut app, vec![egui::Event::PointerMoved(point(1)), egui::Event::PointerButton { pos: point(1), button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }]);
                    draw(&mut app, vec![egui::Event::PointerMoved(point(3))]);
                    draw(&mut app, vec![egui::Event::PointerButton { pos: point(3), button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }]);
                    let frame = draw(&mut app, vec![]);
                    let painted = frame.shapes.iter().find_map(|s| match &s.shape {
                        egui::Shape::Text(t) if t.galley.text().starts_with("abcdef secret") => Some(&t.galley), _ => None,
                    }).unwrap();
                    let row = &painted.rows[0];
                    for (index, quad) in row.visuals.mesh.vertices[row.visuals.glyph_vertex_range.clone()].as_chunks::<4>().0.iter().take(6).enumerate() {
                        assert_eq!(quad[0].color.a() != 0, (1..3).contains(&index));
                    }
                    let frame = draw(&mut app, vec![egui::Event::Copy]);
                    assert!(frame.platform_output.commands.iter().any(|command| matches!(command, egui::OutputCommand::CopyText(text) if text == "bc")));
                    app.active_mut().unwrap().editor_state.tools.marks.clear();
                    let frame = draw(&mut app, vec![]);
                    let galley = frame.shapes.iter().find_map(|s| match &s.shape { egui::Shape::Text(t) if t.galley.text().starts_with("abcdef secret") => Some(&t.galley), _ => None }).unwrap();
                    let row = &galley.rows[0];
                    assert!(row.visuals.mesh.vertices[row.visuals.glyph_vertex_range.clone()].iter().all(|v| v.color.a() != 0));
                }
            }
        }
    }

    #[test]
    fn regex_case_counts_invalid_patterns_and_capture_replacements() {
        let mut tools = Tools {
            query: "hello".to_owned(),
            ..Default::default()
        };
        let text = "Hello HELLO hello 中文42 中文7";
        tools.refresh_search(text, (0, 0));
        assert_eq!(tools.matches.len(), 1);
        tools.ignore_case = true;
        tools.refresh_search(text, (0, 0));
        assert_eq!(tools.matches.len(), 3);
        tools.regex = true;
        tools.query = "中文(?P<n>\\d+)".to_owned();
        tools.replacement = "数字${n}".to_owned();
        tools.refresh_search(text, (0, 0));
        assert_eq!(tools.matches.len(), 2);
        assert_eq!(tools.replacement_for(text, &tools.matches[0]), "数字42");
        tools.query = "[".to_owned();
        tools.refresh_search(text, (0, 0));
        assert!(tools.search_error.is_some());
        assert!(tools.matches.is_empty());
        tools.query = "^|$".to_owned();
        tools.refresh_search(text, (0, 0));
        assert_eq!(tools.matches.as_ref(), &[0..0, text.len()..text.len()]);
        tools.regex = false;
        tools.query = "[".to_owned();
        tools.refresh_search("[ [", (1, 0));
        assert_eq!(tools.matches.len(), 2);
        assert!(tools.search_error.is_none());
    }

    #[test]
    fn marker_window_uses_global_offsets_and_search_does_not_override_pen_color() {
        let text = "前缀\n中文 red\n中文";
        let mut tools = Tools {
            pen: Some(4),
            find_open: true,
            query: "中文".to_owned(),
            ..Default::default()
        };
        tools.mark_selection(text, cursor(3, 5));
        tools.refresh_search(text, (0, 0));
        let offset = "前缀\n".len();
        tools.current = tools.matches.last().cloned();
        let job = layout_window(
            &text[offset..],
            egui::FontId::monospace(13.5),
            &crate::DARK,
            f32::INFINITY,
            &tools,
            offset,
        );
        assert_eq!(job.sections[0].byte_range, 0..6);
        assert_eq!(
            job.sections[0].format.background,
            pen_color(4).gamma_multiply(0.3)
        );
        assert!(
            job.sections
                .iter()
                .any(|s| s.format.background == crate::DARK.accent.gamma_multiply(0.2))
        );
        // Replacing marked text with different-width Unicode cannot leave invalid byte ranges.
        tools.apply_edit(offset..offset + 6, 4);
        assert!(tools.marks.is_empty());
        let updated = text.replacen("中文", "🙂", 1);
        let job = layout_window(
            &updated[offset..],
            egui::FontId::monospace(13.5),
            &crate::DARK,
            f32::INFINITY,
            &tools,
            offset,
        );
        for section in job.sections {
            assert!(updated[offset..].get(section.byte_range).is_some());
        }
    }

    #[test]
    fn regex_replace_all_is_one_undoable_edit_and_preserves_marks_between_matches() {
        for large in [false, true] {
            let ctx = egui::Context::default();
            let mut app = crate::LiteTextApp::default();
            app.new_document();
            let file = app.active_mut().unwrap();
            let original = format!(
                "a1 keep a2{}",
                "\nunchanged".repeat(if large { 30_000 } else { 0 })
            );
            file.draft_text = Some(original.clone());
            file.sync_draft();
            file.editor_state.tools = Tools {
                pen: Some(1),
                query: "a(\\d)".to_owned(),
                regex: true,
                replacement: "b${1}b".to_owned(),
                ..Default::default()
            };
            file.editor_state
                .tools
                .mark_selection(&original, cursor(3, 7));
            file.editor_state
                .tools
                .refresh_search(&original, (file.content_revision, 0));
            replace_all(file, &ctx);
            assert!(
                file.draft_text
                    .as_deref()
                    .unwrap()
                    .starts_with("b1b keep b2b")
            );
            assert_eq!(
                file.editor_state.tools.marks,
                [Mark {
                    range: 4..8,
                    color: 1
                }]
            );
            for frame in 0..2 {
                let _ = ctx.run(egui::RawInput::default(), |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        crate::draft_editor(ui, file, &crate::DARK, frame == 0, crate::Lang::En)
                    });
                });
            }
            let _ = ctx.run(
                egui::RawInput {
                    modifiers: egui::Modifiers::COMMAND,
                    events: vec![egui::Event::Key {
                        key: egui::Key::Z,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::COMMAND,
                    }],
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        crate::draft_editor(ui, file, &crate::DARK, false, crate::Lang::En)
                    });
                },
            );
            assert_eq!(file.draft_text.as_deref(), Some(original.as_str()));
        }
    }

    #[test]
    fn find_shortcut_seeds_nonempty_selection_once() {
        let ctx = egui::Context::default(); crate::setup_fonts(&ctx);
        let mut app = crate::LiteTextApp::default(); app.new_document();
        let file = app.active_mut().unwrap(); file.draft_text = Some("before selected text after".into());
        file.editor_state.cursor = Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(7), egui::text::CCursor::new(20)));
        let _ = ctx.run(egui::RawInput { events: vec![egui::Event::Key {
            key: egui::Key::F, physical_key: None, pressed: true, repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        }], ..Default::default() }, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| crate::draft_editor(ui, app.active_mut().unwrap(), &crate::DARK, false, crate::Lang::En));
        });
        assert_eq!(app.active().unwrap().editor_state.tools.query, "selected text");
        assert!(app.active().unwrap().editor_state.tools.find_open);
        app.active_mut().unwrap().editor_state.tools.query = "custom".into();
        app.active_mut().unwrap().editor_state.tools.find_open = true;
        app.active_mut().unwrap().editor_state.cursor = Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(0), egui::text::CCursor::new(6)));
        let _ = ctx.run(egui::RawInput { events: vec![egui::Event::Key {
            key: egui::Key::F, physical_key: None, pressed: true, repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        }], ..Default::default() }, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| crate::draft_editor(ui, app.active_mut().unwrap(), &crate::DARK, false, crate::Lang::En));
        });
        assert_eq!(app.active().unwrap().editor_state.tools.query, "custom");
    }

    #[test]
    fn find_bar_navigates_and_replaces_the_current_match() {
        let ctx = egui::Context::default();
        let mut app = crate::LiteTextApp::default();
        app.new_document();
        let file = app.active_mut().unwrap();
        file.draft_text = Some("中文🙂 中文🙂".to_owned());
        file.sync_draft();
        file.editor_state.tools.find_open = true;
        file.editor_state.tools.replace_open = true;
        file.editor_state.tools.query = "中文".to_owned();
        file.editor_state.tools.replacement = "替换".to_owned();
        let mut next = None;
        let mut replace = None;
        for frame in 0..8 {
            let events = if [2, 3, 5, 6].contains(&frame) {
                let pos = if frame < 4 {
                    next.unwrap()
                } else {
                    replace.unwrap()
                };
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: frame == 2 || frame == 5,
                        modifiers: Default::default(),
                    },
                ]
            } else {
                vec![]
            };
            let _output = ctx.run(
                egui::RawInput {
                    events,
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1080.0, 720.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        crate::draft_editor(ui, file, &crate::DARK, false, crate::Lang::ZhCn)
                    });
                },
            );
            next = ctx
                .data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("find_next_rect", file.id))))
                .map(|r| r.center())
                .or(next);
            replace = ctx
                .data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("replace_rect", file.id))))
                .map(|r| r.center())
                .or(replace);
            if frame == 4 {
                assert_eq!(file.editor_state.selected_characters, 2);
            }
        }
        assert_eq!(file.draft_text.as_deref(), Some("替换🙂 中文🙂"));
        assert!(file.is_modified());
    }

    #[test]
    fn text_toolbar_buttons_toggle_and_precede_view_switch() {
        let ctx = egui::Context::default();
        let mut app = crate::LiteTextApp { vertical_tabs: false, ..Default::default() };
        app.new_document();
        let mut wrap_pos = None;
        for frame in 0..4 {
            let events = if frame >= 2 {
                let pos = wrap_pos.unwrap();
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: frame == 2,
                        modifiers: Default::default(),
                    },
                ]
            } else {
                vec![]
            };
            let output = ctx.run(
                egui::RawInput {
                    events,
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1080.0, 720.0),
                    )),
                    ..Default::default()
                },
                |ctx| app.top_bar(ctx, &crate::DARK),
            );
            let mut positions = std::collections::HashMap::new();
            for shape in output.shapes {
                if let egui::Shape::Text(text) = shape.shape {
                    positions.insert(text.galley.text().to_owned(), text.pos.x);
                }
            }
            let rect = |action: i32| {
                ctx.data(|d| {
                    d.get_temp::<egui::Rect>(egui::Id::new((
                        "text_tool_rect",
                        app.active_id.unwrap(),
                        action,
                    )))
                })
                .unwrap()
            };
            wrap_pos = Some(rect(3).center());
            assert!(
                rect(3).center().x < rect(2).center().x && rect(2).center().x < rect(1).center().x
            );
            assert!(
                rect(1).center().x < rect(0).center().x && rect(0).center().x < positions["TXT"]
            );
        }
        assert!(app.active().unwrap().editor_state.tools.wrap);
        app.active_mut().unwrap().view = crate::ViewMode::Hex;
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            app.top_bar(ctx, &crate::DARK)
        });
        assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(t) if ["↵", "¶", "M", "⌕"].contains(&t.galley.text()))));
    }

    #[test]
    fn literal_unicode_search_wraps_both_directions() {
        let text = "a中文🙂 b中文🙂";
        let mut tools = Tools {
            query: "中文🙂".to_owned(),
            ..Default::default()
        };
        tools.refresh_search(text, (0, 0));
        let first = navigate(&tools.matches, 0, false).unwrap();
        let second = navigate(&tools.matches, first.end, false).unwrap();
        assert!(second.start > first.start);
        assert_eq!(
            navigate(&tools.matches, second.end, false),
            Some(first.clone())
        );
        assert_eq!(navigate(&tools.matches, second.start, true), Some(first));
        assert_eq!(navigate(&tools.matches, 0, true), Some(second));
        for query in ["", "missing"] {
            tools.query = query.to_owned();
            tools.refresh_search(text, (0, 0));
            assert_eq!(navigate(&tools.matches, 0, false), None);
        }
    }

    #[test]
    fn display_options_and_marking_preserve_text_and_highlight_matches() {
        let text = "中文 a\t\r\n中文";
        let tools = Tools {
            wrap: true,
            whitespace: true,
            marks: vec![
                Mark {
                    range: 0..6,
                    color: 0,
                },
                Mark {
                    range: 12..18,
                    color: 2,
                },
            ],
            ..Default::default()
        };
        let job = layout(
            text,
            egui::FontId::monospace(13.5),
            &crate::DARK,
            100.0,
            &tools,
        );
        assert_eq!(job.text, text);
        assert_eq!(job.wrap.max_width, 100.0);
        assert!(job.wrap.break_anywhere);
        assert_eq!(
            job.sections
                .iter()
                .filter(|s| s.format.background != egui::Color32::TRANSPARENT)
                .count(),
            2
        );
    }

    #[test]
    fn replacement_preserves_encoding_disk_and_view_options_and_can_undo() {
        for large in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("sample.txt");
            let original = "中文🙂\r\nend".repeat(if large { 30_000 } else { 1 });
            std::fs::write(&path, original.as_bytes()).unwrap();
            let mut app = crate::LiteTextApp::default();
            app.open_paths(vec![path.clone()]);
            let ctx = egui::Context::default();
            let file = app.active_mut().unwrap();
            file.editor_state.tools.whitespace = true;
            replace_range(file, &ctx, 0..6, "替换".to_owned());
            assert!(file.is_modified());
            assert!(file.editor_state.tools.whitespace);
            assert_eq!(file.encoding, crate::recovery::Encoding::Utf8);
            assert!(file.draft_text.as_ref().unwrap().starts_with("替换🙂\r\n"));
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    crate::draft_editor(ui, file, &crate::DARK, true, crate::Lang::ZhCn)
                });
            });
            let _ = ctx.run(
                egui::RawInput {
                    modifiers: egui::Modifiers::COMMAND,
                    events: vec![egui::Event::Key {
                        key: egui::Key::Z,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::COMMAND,
                    }],
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        crate::draft_editor(ui, file, &crate::DARK, false, crate::Lang::ZhCn)
                    });
                },
            );
            assert_eq!(file.draft_text.as_deref(), Some(original.as_str()));
            assert_eq!(std::fs::read(&path).unwrap(), original.as_bytes());
        }
    }
    #[test]
    fn rendered_markdown_disables_source_only_toolbar_buttons() {
        let ctx = egui::Context::default();
        let mut app = crate::LiteTextApp::default();
        app.new_document();
        {
            let file = app.active_mut().unwrap();
            file.path = std::path::PathBuf::from("toolbar.md");
            file.name = "toolbar.md".to_owned();
            file.draft_text = Some("# Title\n\nBody\n".to_owned());
            file.sync_draft();
            crate::workspace_theme::configure(
                file,
                &crate::workspace_theme::WorkspaceTheme::default(),
                true,
            );
            file.markdown_mode = crate::markdown::Mode::Live;
        }
        let draw = |app: &mut crate::LiteTextApp, events: Vec<egui::Event>| {
            ctx.run(
                egui::RawInput {
                    events,
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1080.0, 720.0),
                    )),
                    ..Default::default()
                },
                |ctx| app.top_bar(ctx, &crate::DARK),
            )
        };
        let click = |app: &mut crate::LiteTextApp, pos: egui::Pos2| {
            for pressed in [true, false] {
                draw(
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
                );
            }
        };
        let file_id = app.active_id.unwrap();
        let rect = |action: i32| {
            ctx.data(|d| {
                d.get_temp::<egui::Rect>(egui::Id::new(("text_tool_rect", file_id, action)))
            })
            .unwrap()
        };
        // Dimmest line stroke alpha of the icon inside its button rect.
        let icon_alpha = |output: egui::FullOutput, button: egui::Rect| {
            output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::LineSegment { points, stroke }
                        if button.contains(points[0]) && button.contains(points[1]) =>
                    {
                        Some(stroke.color.a())
                    }
                    _ => None,
                })
                .min()
                .unwrap_or(255)
        };

        draw(&mut app, vec![]);
        draw(&mut app, vec![]);
        let wrap_button = rect(3);
        let dim = icon_alpha(draw(&mut app, vec![egui::Event::PointerMoved(egui::pos2(5.0, 5.0))]), wrap_button);
        assert!(
            dim < 200,
            "wrap icon must be dimmed in a rendered Markdown mode, alpha {dim}"
        );

        let (whitespace_button, pen_button, find_button) =
            (rect(2), rect(1), rect(0));
        click(&mut app, wrap_button.center());
        assert!(
            !app.active().unwrap().editor_state.tools.wrap,
            "wrap must not toggle while Markdown is rendered"
        );
        click(&mut app, whitespace_button.center());
        assert!(
            !app.active().unwrap().editor_state.tools.whitespace,
            "whitespace must not toggle while Markdown is rendered"
        );
        click(&mut app, pen_button.center());
        assert!(
            app.active().unwrap().editor_state.tools.pen.is_none()
                && !app.active().unwrap().editor_state.tools.pen_menu,
            "the marker pen must stay unavailable while Markdown is rendered"
        );
        // Find & replace stays usable.
        click(&mut app, find_button.center());
        assert!(app.active().unwrap().editor_state.tools.find_open);

        app.active_mut().unwrap().markdown_mode = crate::markdown::Mode::Source;
        draw(&mut app, vec![]);
        click(&mut app, wrap_button.center());
        assert!(
            app.active().unwrap().editor_state.tools.wrap,
            "source mode must restore the wrap toggle"
        );
    }
}

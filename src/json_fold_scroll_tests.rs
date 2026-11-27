//! Display-only fold clicks must not become navigation/reveal requests.
use super::tests::{button, raw};
use super::*;
use crate::{DARK, LiteTextApp};

fn draw(ctx: &egui::Context, app: &mut LiteTextApp, events: Vec<egui::Event>) -> egui::FullOutput {
    ctx.run(raw(events), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            crate::draft_editor(ui, app.active_mut().unwrap(), &DARK, false, Lang::En);
        });
    })
}

fn geometry(ctx: &egui::Context, id: u64) -> (egui::Id, egui::Vec2, egui::Rect) {
    ctx.data(|data| data.get_temp(egui::Id::new(("editor_scroll_geometry", id))))
        .unwrap()
}

fn content_geometry(ctx: &egui::Context, id: u64) -> (egui::Rect, egui::Vec2) {
    ctx.data(|d| d.get_temp(egui::Id::new(("editor_content_geometry", id))))
        .unwrap()
}

#[test]
fn folded_large_json_layout_is_bounded_and_pointer_selection_maps_to_source() {
    for ending in ["\n", "\r\n"] {
        let ctx = egui::Context::default();
        let mut app = LiteTextApp::default();
        app.new_document();
        let object = format!(
            " {{\n{}  \"tail\": true\n }},\n",
            "  \"value\": \"中🙂 hidden text\",\n".repeat(400)
        );
        let text = format!("[\n{} null\n]\n", object.repeat(200)).replace('\n', ending);
        let file = app.active_mut().unwrap();
        file.draft_text = Some(text.clone());
        file.sync_draft();
        file.language_override = Some("JSON".into());
        let id = file.id;
        draw(&ctx, &mut app, vec![]);
        app.active_mut().unwrap().editor_state.folds.toggle_depth(1);
        for _ in 0..4 {
            draw(&ctx, &mut app, vec![]);
        }
        let builds = app.active().unwrap().editor_state.large.window_builds;
        for _ in 0..8 {
            draw(&ctx, &mut app, vec![]);
        }
        assert_eq!(
            app.active().unwrap().editor_state.large.window_builds,
            builds,
            "unchanged folded viewport must reuse its display buffer"
        );
        let started = std::time::Instant::now();
        for top in [true, false, true] {
            scroll_extreme(&ctx, &mut app, id, top);
            assert!(
                app.active().unwrap().editor_state.large.laid_out_bytes < 4096,
                "hidden text reached TextEdit: {}",
                app.active().unwrap().editor_state.large.laid_out_bytes
            );
        }
        eprintln!(
            "folded {} bytes: 3 scroll passes {:?}; last layout {} bytes",
            text.len(),
            started.elapsed(),
            app.active().unwrap().editor_state.large.laid_out_bytes
        );
        let second = 1 + object.lines().count();
        let rect = ctx
            .data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("json_depth_rect", id, second))))
            .unwrap();
        let pos = egui::pos2(rect.right() + 25.0, rect.center().y);
        for pressed in [true, false] {
            draw(&ctx, &mut app, button(pos, pressed));
        }
        let file = app.active().unwrap();
        assert_eq!(file.editor_state.cursor_line, second);
        let cursor = file.editor_state.cursor.unwrap();
        let source_start: usize = text
            .split_inclusive('\n')
            .take(second)
            .map(|l| l.chars().count())
            .sum();
        assert!((source_start..source_start + 5).contains(&cursor.primary.index));
        assert!(file.editor_state.folds.has_collapsed());
        draw(&ctx, &mut app, vec![egui::Event::Text("X".into())]);
        let mut expected = text.clone();
        let byte = crate::text_tools::byte_at(&text, cursor.primary.index);
        expected.insert(byte, 'X');
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some(expected.as_str())
        );
        assert!(!app.active().unwrap().editor_state.folds.has_collapsed());
        draw(
            &ctx,
            &mut app,
            super::tests::key(egui::Key::Z, egui::Modifiers::COMMAND),
        );
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some(text.as_str())
        );
        // A pointer selection across two projected headers must still copy the
        // full source span, including hidden lines, rather than display text.
        draw(&ctx, &mut app, vec![]); // Synchronize folds after the undo revision.
        app.active_mut().unwrap().editor_state.folds.toggle_depth(1);
        scroll_extreme(&ctx, &mut app, id, true);
        let mut input = raw(vec![]);
        input.time = Some(100.0);
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                crate::draft_editor(ui, app.active_mut().unwrap(), &DARK, false, Lang::En);
            });
        });
        let first_rect = ctx
            .data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("json_depth_rect", id, 1usize))))
            .unwrap();
        let second_rect = ctx
            .data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("json_depth_rect", id, second))))
            .unwrap();
        draw(
            &ctx,
            &mut app,
            button(
                egui::pos2(first_rect.right() + 5.0, first_rect.center().y),
                true,
            ),
        );
        let end = egui::pos2(second_rect.right() + 25.0, second_rect.center().y);
        draw(&ctx, &mut app, vec![egui::Event::PointerMoved(end)]);
        draw(&ctx, &mut app, button(end, false));
        let cursor = app.active().unwrap().editor_state.cursor.unwrap();
        let start =
            crate::text_tools::byte_at(&text, cursor.primary.index.min(cursor.secondary.index));
        let end =
            crate::text_tools::byte_at(&text, cursor.primary.index.max(cursor.secondary.index));
        assert!(
            end - start > 4096,
            "selection {cursor:?}, first={first_rect:?} second={second_rect:?}"
        );
        let output = draw(&ctx, &mut app, vec![egui::Event::Copy]);
        assert!(output.platform_output.commands.iter().any(
            |c| matches!(c, egui::OutputCommand::CopyText(value) if value == &text[start..end])
        ));
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some(text.as_str())
        );
    }
}

#[test]
fn depth_menu_labels_stay_single_line_at_narrow_window_edges() {
    for zh in [false, true] {
        for collapsed in [false, true] {
            let ctx = egui::Context::default();
            crate::setup_fonts(&ctx);
            let label = match (zh, collapsed) {
                (true, false) => "折叠当前层级",
                (true, true) => "展开该层级",
                (false, false) => "Collapse this level",
                (false, true) => "Expand this level",
            };
            let rect = egui::Rect::from_min_size(egui::pos2(220.0, 40.0), egui::vec2(55.0, 24.0));
            let draw_menu = |events| {
                ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(300.0, 200.0),
                        )),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            let response = ui.interact(
                                rect,
                                egui::Id::new("menu_test_gutter"),
                                egui::Sense::click(),
                            );
                            depth_menu(rect, 1, 0, collapsed, zh, &response);
                        });
                    },
                )
            };
            for _ in 0..3 {
                draw_menu(vec![]);
            }
            for pressed in [true, false] {
                draw_menu(secondary(rect.center(), pressed));
            }
            let output = draw_menu(vec![]);
            let text = output
                .shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::Shape::Text(t) if t.galley.text() == label => Some(t),
                    _ => None,
                })
                .expect("full menu label must render");
            assert_eq!(text.galley.rows.len(), 1);
            assert!(text.pos.x >= 0.0 && text.pos.x + text.galley.size().x <= 300.0);
        }
    }
}

#[test]
fn fold_snapshots_share_indices_and_mutations_do_not_change_older_snapshot() {
    let mut folds = Folds::default();
    folds.set_enabled(true);
    folds.sync("[\n {\n  \"a\": 1\n },\n {\n  \"b\": 2\n }\n]", (1, 1));
    folds.toggle(1);
    let snapshot = folds.clone();
    assert!(std::sync::Arc::ptr_eq(&folds.blocks, &snapshot.blocks));
    folds.toggle_depth(4);
    assert!(folds.collapsed(4) && !snapshot.collapsed(4));
    assert!(folds.hidden(5) && !snapshot.hidden(5));
    assert!(snapshot.collapsed(1) && snapshot.hidden(2));
}

fn choose_depth(ctx: &egui::Context, app: &mut LiteTextApp, id: u64, line: usize) {
    let rect = ctx
        .data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("json_depth_rect", id, line))))
        .unwrap();
    let pos = egui::pos2(rect.left() + 4.0, rect.center().y);
    for pressed in [true, false] {
        draw(ctx, app, secondary(pos, pressed));
    }
    draw(ctx, app, vec![]);
    let choice = ctx
        .data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("json_depth_choice", id))))
        .unwrap_or_else(|| {
            panic!(
                "no choice for line={line}, rect={rect:?}, viewport={:?}",
                geometry(ctx, id)
            )
        });
    for pressed in [true, false] {
        draw(ctx, app, button(choice.center(), pressed));
    }
    for _ in 0..8 {
        draw(ctx, app, vec![]);
    }
}

fn scroll_extreme(
    ctx: &egui::Context,
    app: &mut LiteTextApp,
    id: u64,
    top: bool,
) -> egui::FullOutput {
    draw(
        ctx,
        app,
        vec![
            egui::Event::PointerMoved(geometry(ctx, id).2.center()),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, if top { 1e7 } else { -1e7 }),
                modifiers: Default::default(),
            },
        ],
    );
    for _ in 0..30 {
        draw(ctx, app, vec![]);
    }
    draw(ctx, app, vec![])
}

#[test]
fn massive_depth_fold_keeps_compact_gutter_real_scroll_range_and_line_one_at_top() {
    for large in [false, true] {
        for wrap in [false, true] {
            for siblings in [3, 300] {
                let ctx = egui::Context::default();
                let mut app = LiteTextApp::default();
                app.new_document();
                let value = if large {
                    "1234567890".repeat(30)
                } else {
                    "1".into()
                };
                let object = format!(
                    " {{\n{}  \"last\": 0\n }},\n",
                    format!("  \"x\": \"{value}\",\n").repeat(if siblings == 3 {
                        1000
                    } else {
                        40
                    })
                );
                let text = format!("[\n{} null\n]\n", object.repeat(siblings));
                let file = app.active_mut().unwrap();
                file.draft_text = Some(text.clone());
                file.sync_draft();
                file.language_override = Some("JSON".into());
                file.editor_state.tools.wrap = wrap;
                file.editor_state.font_size = 19.5;
                let id = file.id;
                for _ in 0..4 {
                    draw(&ctx, &mut app, vec![]);
                }
                assert_eq!(app.active().unwrap().editor_state.large.active, large);
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
                while app.active().unwrap().editor_state.large.wrapping_pending() {
                    draw(&ctx, &mut app, vec![]);
                    assert!(std::time::Instant::now() < deadline);
                }
                let blocks = app.active().unwrap().editor_state.folds.blocks.clone();
                let line = blocks.iter().filter(|b| b.depth == 1).nth(1).unwrap().start;
                let start = text
                    .split_inclusive('\n')
                    .take(line.saturating_sub(6))
                    .map(|l| l.chars().count())
                    .sum();
                app.active_mut().unwrap().editor_state.pending_selection = Some(start..start);
                for _ in 0..20 {
                    draw(&ctx, &mut app, vec![]);
                }
                // Wrapped navigation can leave this long-distance animation
                // in flight. Position the actual header before clicking it.
                let rect = ctx
                    .data(|d| {
                        d.get_temp::<egui::Rect>(egui::Id::new(("json_depth_rect", id, line)))
                    })
                    .unwrap();
                let (scroll_id, offset, viewport) = geometry(&ctx, id);
                let mut scroll = egui::scroll_area::State::default();
                scroll.offset = egui::vec2(
                    offset.x,
                    (offset.y + rect.center().y - viewport.center().y).max(0.0),
                );
                scroll.store(&ctx, scroll_id);
                for _ in 0..4 {
                    draw(&ctx, &mut app, vec![]);
                }
                let initial_gutter = content_geometry(&ctx, id).0;
                choose_depth(&ctx, &mut app, id, line);
                assert!(app.active().unwrap().editor_state.folds.collapsed(line));
                let output = scroll_extreme(&ctx, &mut app, id, true);
                let (_, offset, viewport) = geometry(&ctx, id);
                let (gutter, size) = content_geometry(&ctx, id);
                assert!(offset.y < 1.0);
                assert_eq!(gutter.width(), initial_gutter.width());
                let one = output
                    .shapes
                    .iter()
                    .find_map(|s| match &s.shape {
                        egui::Shape::Text(t) if t.galley.text() == "1" => Some(t.pos),
                        _ => None,
                    })
                    .unwrap();
                assert!(
                    (one.y - viewport.top() - 12.0).abs() < 2.0,
                    "line one displaced: large={large} wrap={wrap} siblings={siblings}: {one:?} {viewport:?}"
                );
                let digits = crate::editor::gutter_columns(text.lines().count() + 1);
                let digit_w = ctx.fonts(|f| {
                    let probe = f.layout_no_wrap(
                        "0000000000".into(),
                        app.active().unwrap().editor_state.gutter_font(),
                        DARK.text_faint,
                    );
                    probe.rows[0].glyphs[1].pos.x - probe.rows[0].glyphs[0].pos.x
                });
                assert!(
                    (gutter.width() - (6.0 + digits as f32 * digit_w + FOLD_WIDTH)).abs() < 1.0
                );
                assert!(
                    size.y <= (24.0 + (siblings + 4) as f32 * 26.0 + viewport.height() * 0.5)
                        .max(viewport.height() + 4.0),
                    "scroll extent includes hidden rows: large={large} wrap={wrap} siblings={siblings} {size:?}"
                );
                let bottom_output = scroll_extreme(&ctx, &mut app, id, false);
                let (_, bottom, viewport) = geometry(&ctx, id);
                let max = (content_geometry(&ctx, id).1.y - viewport.height()).max(0.0);
                assert!((bottom.y - max).abs() < 1.0);
                for shape in &bottom_output.shapes {
                    if let egui::Shape::Text(t) = &shape.shape
                        && t.galley.text().parse::<usize>().is_ok()
                        && viewport.contains(t.pos)
                    {
                        assert!(t.pos.x >= gutter.left());
                        assert!(t.pos.x + t.galley.size().x <= gutter.right() - FOLD_WIDTH + 0.1);
                    }
                }
                scroll_extreme(&ctx, &mut app, id, true);
                choose_depth(&ctx, &mut app, id, 1);
                assert!(!app.active().unwrap().editor_state.folds.has_collapsed());
                scroll_extreme(&ctx, &mut app, id, true);
                choose_depth(&ctx, &mut app, id, 0);
                assert!(app.active().unwrap().editor_state.folds.collapsed(0));
                scroll_extreme(&ctx, &mut app, id, false);
                assert!(
                    geometry(&ctx, id).1.y < 4.0,
                    "collapsed root retained hidden scroll range"
                );
                assert_eq!(
                    app.active().unwrap().draft_text.as_deref(),
                    Some(text.as_str())
                );
            }
        }
    }
}

fn visible_numbers(
    output: &egui::FullOutput,
    viewport: egui::Rect,
    before_line: usize,
) -> Vec<(String, f32)> {
    output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text)
                if viewport.contains(text.pos)
                    && text
                        .galley
                        .text()
                        .parse::<usize>()
                        .is_ok_and(|number| number <= before_line) =>
            {
                Some((text.galley.text().to_owned(), text.pos.y))
            }
            _ => None,
        })
        .collect()
}

fn exercise(text: &str, wrap: bool) {
    let ctx = egui::Context::default();
    let mut app = LiteTextApp::default();
    app.new_document();
    let file = app.active_mut().unwrap();
    file.draft_text = Some(text.to_owned());
    file.sync_draft();
    file.language_override = Some("JSON".into());
    file.editor_state.tools.wrap = wrap;
    let id = file.id;
    for _ in 0..4 {
        draw(&ctx, &mut app, vec![]);
    }
    // Settle sliced wrapping before checking stability rather than mixing two changes.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while app.active().unwrap().editor_state.large.wrapping_pending() {
        draw(&ctx, &mut app, vec![]);
        assert!(std::time::Instant::now() < deadline);
    }
    let blocks = app.active().unwrap().editor_state.folds.blocks.clone();
    // Exercise deep blocks, including those which hide almost all remaining content.
    let candidates: Vec<_> = blocks
        .iter()
        .filter(|block| block.start > 30 && block.end > block.start + 2)
        .step_by((blocks.len() / 8).max(1))
        .take(8)
        .cloned()
        .collect();
    assert!(!candidates.is_empty());
    for block in candidates {
        let file = app.active_mut().unwrap();
        let start: usize = text
            .split_inclusive('\n')
            .take(block.start.saturating_sub(8))
            .map(|line| line.chars().count())
            .sum();
        file.editor_state.pending_selection = Some(start..start);
        for _ in 0..20 {
            draw(&ctx, &mut app, vec![]);
        }
        let settled = draw(&ctx, &mut app, vec![]);
        let key = egui::Id::new(("json_fold_rect", id, block.start));
        let rect = ctx.data(|data| data.get_temp::<egui::Rect>(key)).unwrap();
        let (_, before, viewport) = geometry(&ctx, id);
        let numbers = visible_numbers(&settled, viewport, block.start);
        assert!(!numbers.is_empty());
        assert!(
            viewport.contains(rect.center()),
            "fold {} not visible: {rect:?} {viewport:?}",
            block.start
        );
        for collapsed in [true, false] {
            let rect = ctx.data(|data| data.get_temp::<egui::Rect>(key)).unwrap();
            for pressed in [true, false] {
                draw(&ctx, &mut app, button(rect.center(), pressed));
                assert!(
                    (geometry(&ctx, id).1.y - before.y).abs() < 1.0,
                    "fold {} wrap={wrap} press={pressed} changed viewport",
                    block.start
                );
            }
            for frame in 0..8 {
                let output = draw(&ctx, &mut app, vec![]);
                assert_eq!(
                    app.active()
                        .unwrap()
                        .editor_state
                        .folds
                        .collapsed(block.start),
                    collapsed
                );
                let (_, after, _) = geometry(&ctx, id);
                assert!(
                    (after.x - before.x).abs() < 1.0,
                    "horizontal position changed"
                );
                assert!(
                    (after.y - before.y).abs() < 1.0,
                    "fold {} wrap={wrap} collapsed={collapsed} frame={frame}: {before:?} -> {after:?}",
                    block.start
                );
                assert_eq!(
                    visible_numbers(&output, viewport, block.start),
                    numbers,
                    "line numbers above fold moved"
                );
                let after_rect = ctx.data(|data| data.get_temp::<egui::Rect>(key)).unwrap();
                assert!(
                    (after_rect.top() - rect.top()).abs() < 1.0,
                    "fold header moved: {rect:?} -> {after_rect:?}"
                );
            }
        }
    }
    // Explicit navigation still expands folds and reveals the requested position.
    app.active_mut().unwrap().editor_state.pending_selection = Some(0..0);
    for _ in 0..20 {
        draw(&ctx, &mut app, vec![]);
    }
    assert!(geometry(&ctx, id).1.y < 1.0);
    assert_eq!(app.active().unwrap().editor_state.fold_scroll_extent, 0.0);
    assert_eq!(app.active().unwrap().draft_text.as_deref(), Some(text));
}

fn secondary(pos: egui::Pos2, pressed: bool) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(pos),
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: Default::default(),
        },
    ]
}

fn exercise_depth_menu(text: &str, wrap: bool) {
    let ctx = egui::Context::default();
    let mut app = LiteTextApp::default();
    app.new_document();
    let file = app.active_mut().unwrap();
    file.draft_text = Some(text.to_owned());
    file.sync_draft();
    file.language_override = Some("JSON".into());
    file.editor_state.tools.wrap = wrap;
    let id = file.id;
    for _ in 0..4 {
        draw(&ctx, &mut app, vec![]);
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while app.active().unwrap().editor_state.large.wrapping_pending() {
        draw(&ctx, &mut app, vec![]);
        assert!(std::time::Instant::now() < deadline);
    }
    let blocks = app.active().unwrap().editor_state.folds.blocks.clone();
    let candidates: Vec<_> = blocks
        .iter()
        .filter(|b| {
            b.start > 30
                && b.end > b.start + 2
                && blocks.iter().any(|a| a.depth == b.depth && a.end < b.start)
        })
        .collect();
    assert!(!candidates.is_empty());
    for block in candidates
        .iter()
        .step_by((candidates.len() / 3).max(1))
        .take(3)
    {
        let start: usize = text
            .split_inclusive('\n')
            .take(block.start.saturating_sub(6))
            .map(|l| l.chars().count())
            .sum();
        app.active_mut().unwrap().editor_state.pending_selection = Some(start..start);
        for _ in 0..20 {
            draw(&ctx, &mut app, vec![]);
        }
        let key = egui::Id::new(("json_depth_rect", id, block.start));
        let original = ctx.data(|d| d.get_temp::<egui::Rect>(key)).unwrap();
        assert!(geometry(&ctx, id).2.contains(original.center()));
        let gutter_before = content_geometry(&ctx, id).0;
        // Deliberately unrelated selected line must not become the menu target.
        app.active_mut().unwrap().editor_state.selection = Some((0, 0));
        for collapsed in [true, false] {
            let rect = ctx.data(|d| d.get_temp::<egui::Rect>(key)).unwrap();
            let before = geometry(&ctx, id).1;
            let pos = egui::pos2(rect.left() + 4.0, rect.center().y);
            for pressed in [true, false] {
                draw(&ctx, &mut app, secondary(pos, pressed));
            }
            let output = draw(&ctx, &mut app, vec![]);
            let labels: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(t) => Some(t.galley.text()),
                    _ => None,
                })
                .collect();
            assert!(
                labels.contains(&if collapsed {
                    "Collapse this level"
                } else {
                    "Expand this level"
                }),
                "{labels:?}"
            );
            let t = crate::i18n::tr(Lang::En);
            assert!(!labels.contains(&t.copy_line) && !labels.contains(&t.clear_content));
            assert_eq!(app.active().unwrap().editor_state.selection, Some((0, 0)));
            assert!((geometry(&ctx, id).1.y - before.y).abs() < 1.0);
            let choice = ctx
                .data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("json_depth_choice", id))))
                .unwrap();
            for pressed in [true, false] {
                draw(&ctx, &mut app, button(choice.center(), pressed));
            }
            for _ in 0..8 {
                ctx.data_mut(|d| d.remove::<egui::Rect>(key));
                draw(&ctx, &mut app, vec![]);
                let after = ctx.data(|d| d.get_temp::<egui::Rect>(key)).unwrap();
                let (_, offset, viewport) = geometry(&ctx, id);
                let (gutter, size) = content_geometry(&ctx, id);
                let max = (size.y - viewport.height()).max(0.0);
                assert!(offset.y >= 0.0 && offset.y <= max + 1.0);
                if offset.y > 1.0 && offset.y < max - 1.0 {
                    assert!(
                        (after.top() - rect.top()).abs() < 1.0,
                        "header {} wrap={wrap} collapsed={collapsed}: {rect:?} -> {after:?}",
                        block.start
                    );
                }
                assert!(
                    viewport.contains(after.center()),
                    "header must remain visible"
                );
                assert_eq!(gutter.width(), gutter_before.width());
                assert_eq!(app.active().unwrap().editor_state.fold_scroll_extent, 0.0);
            }
            let folds = &app.active().unwrap().editor_state.folds;
            for other in blocks.iter() {
                assert_eq!(
                    folds.collapsed(other.start),
                    collapsed && other.depth == block.depth
                );
            }
            assert_eq!(app.active().unwrap().editor_state.selection, Some((0, 0)));
        }
    }
    app.active_mut().unwrap().editor_state.pending_selection = Some(0..0);
    for _ in 0..20 {
        draw(&ctx, &mut app, vec![]);
    }
    assert!(geometry(&ctx, id).1.y < 1.0);
    assert_eq!(app.active().unwrap().draft_text.as_deref(), Some(text));
}

#[test]
fn json_nonheader_lines_have_no_menu_while_other_languages_keep_line_commands() {
    for large in [false, true] {
        let ctx = egui::Context::default();
        let mut app = LiteTextApp::default();
        app.new_document();
        let text = format!(
            "[\n{}  null\n]\n",
            "  {\n    \"value\": 1\n  },\n".repeat(if large { 15000 } else { 4 })
        );
        let file = app.active_mut().unwrap();
        file.draft_text = Some(text.clone());
        file.sync_draft();
        let id = file.id;
        for json in [true, false] {
            app.active_mut().unwrap().language_override =
                Some(if json { "JSON" } else { "Plain Text" }.into());
            for _ in 0..4 {
                draw(&ctx, &mut app, vec![]);
            }
            assert_eq!(app.active().unwrap().editor_state.large.active, large);
            let output = draw(&ctx, &mut app, vec![]);
            let pos = output
                .shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::Shape::Text(t) if t.galley.text() == "3" => {
                        Some(t.pos + egui::vec2(2.0, 6.0))
                    }
                    _ => None,
                })
                .unwrap();
            for pressed in [true, false] {
                draw(&ctx, &mut app, secondary(pos, pressed));
            }
            let output = draw(&ctx, &mut app, vec![]);
            let t = crate::i18n::tr(Lang::En);
            let labels: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(t) => Some(t.galley.text()),
                    _ => None,
                })
                .collect();
            assert_eq!(labels.contains(&t.copy_line), !json, "{labels:?}");
            assert_eq!(labels.contains(&t.clear_content), !json);
            assert!(!labels.contains(&"Collapse this level"));
            assert!(!labels.contains(&"Expand this level"));
            if json {
                assert!(ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("json_depth_choice", id)))).is_none());
                assert!(app.active().unwrap().editor_state.selection.is_none());
            }
        }
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some(text.as_str())
        );
    }
}

#[test]
fn json_depth_menu_anchors_within_real_scroll_bounds_and_targets_whole_depth() {
    for large in [false, true] {
        let object = format!(
            "  {{\n    \"child\": [\n{}      null\n    ],\n    \"tail\": true\n  }},\n",
            format!("      \"{}\",\n", "中🙂1234567890".repeat(12)).repeat(16)
        );
        let text = format!(
            "[\n{}  null\n]\n",
            object.repeat(if large { 200 } else { 8 })
        );
        assert_eq!(crate::large_editor::needs_windowing(&text), large);
        for wrap in [false, true] {
            exercise_depth_menu(&text, wrap);
        }
    }
}

#[test]
fn depth_operation_retains_nested_state_and_counts_same_line_ancestors() {
    let text = "[\n {\"a\": [\n  {\n   \"quoted\": \"[{}]\\\"\"\n  }\n ]},\n {\n  \"b\": [\n   1\n  ]\n }\n]";
    let mut folds = Folds::default();
    folds.set_enabled(true);
    folds.sync(text, (1, 1));
    let depths: Vec<_> = folds.blocks.iter().map(|b| (b.start, b.depth)).collect();
    assert_eq!(depths, [(0, 0), (1, 1), (2, 3), (6, 1), (7, 2)]);
    folds.toggle(2);
    let generation = folds.generation;
    folds.toggle_depth(6);
    assert_eq!(folds.generation, generation + 1);
    assert!(folds.collapsed(1) && folds.collapsed(6) && folds.collapsed(2));
    assert!(folds.hidden(2));
    folds.toggle_depth(1);
    assert!(!folds.collapsed(1) && !folds.collapsed(6));
    assert!(folds.collapsed(2) && folds.hidden(3));
    assert!(!folds.hidden(2));
    let generation = folds.generation;
    folds.toggle_depth(3); // No header, no mutation.
    assert_eq!(folds.generation, generation);
}

#[test]
fn fold_click_keeps_scrolled_viewport_stable_in_normal_and_large_json() {
    for large in [false, true] {
        let object = format!(
            "  {{\n{}    \"tail\": true\n  }},\n",
            "    \"value\": \"中🙂1234567890\",\n".repeat(80)
        );
        let text = format!(
            "[\n{}  null\n]\n",
            object.repeat(if large { 160 } else { 6 })
        );
        for wrap in [false, true] {
            exercise(&text, wrap);
        }
    }
}

#[test]
#[ignore = "Requires an explicitly provided read-only JSON fixture"]
fn user_json_fold_scroll_stays_stable_without_modifying_source() {
    let path = std::env::var_os("LITETEXT_FOLD_FILE").expect("set LITETEXT_FOLD_FILE");
    let before = std::fs::read(&path).unwrap();
    let metadata = std::fs::metadata(&path).unwrap();
    let text = std::str::from_utf8(&before).unwrap();
    for wrap in [false, true] {
        exercise(text, wrap);
        exercise_depth_menu(text, wrap);
    }
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        metadata.modified().unwrap()
    );
}

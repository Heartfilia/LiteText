//! Input-level regressions shared by the ordinary and viewport editors.
use super::*;

fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

fn draw(
    ctx: &egui::Context,
    app: &mut LiteTextApp,
    mut events: Vec<egui::Event>,
) -> egui::FullOutput {
    let modifiers = events
        .iter()
        .find_map(|e| match e {
            egui::Event::Key { modifiers, .. } => Some(*modifiers),
            _ => None,
        })
        .unwrap_or_default();
    let releases: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            egui::Event::Key { key, modifiers, .. } => Some(egui::Event::Key {
                key: *key,
                modifiers: *modifiers,
                physical_key: None,
                pressed: false,
                repeat: false,
            }),
            _ => None,
        })
        .collect();
    events.extend(releases);
    ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            events,
            modifiers,
            ..Default::default()
        },
        |ctx| {
            app.handle_shortcuts(ctx);
            egui::CentralPanel::default().show(ctx, |ui| {
                draft_editor(ui, app.active_mut().unwrap(), &DARK, false, Lang::En)
            });
        },
    )
}

fn select(ctx: &egui::Context, app: &mut LiteTextApp, start: usize, end: usize) {
    app.active_mut().unwrap().editor_state.pending_selection = Some(start..end);
    draw(ctx, app, vec![]);
    let file = app.active().unwrap();
    ctx.memory_mut(|m| {
        m.request_focus(
            egui::Id::new(if file.editor_state.large.active {
                "large_editor"
            } else {
                "draft_editor"
            })
            .with(file.id),
        )
    });
    draw(ctx, app, vec![]);
}

fn document(text: &str) -> LiteTextApp {
    let mut app = LiteTextApp::default();
    app.new_document();
    app.active_mut().unwrap().draft_text = Some(text.to_owned());
    app.active_mut().unwrap().sync_draft();
    app
}

#[test]
fn very_long_line_layout_is_bounded_and_tail_edit_copy_and_undo_keep_full_text() {
    let source = "中文🙂abcdef".repeat(80_000);
    let ctx = egui::Context::default();
    let mut app = document(&source);
    app.active_mut().unwrap().editor_state.tools.wrap = true;
    let end = source.chars().count();
    select(&ctx, &mut app, end, end);
    assert!(app.active().unwrap().editor_state.large.laid_out_bytes <= 64 * 1024);
    draw(&ctx, &mut app, vec![egui::Event::Text("尾".into())]);
    assert_eq!(
        app.active().unwrap().draft_text.as_ref().unwrap(),
        &format!("{source}尾")
    );
    draw(
        &ctx,
        &mut app,
        vec![key(egui::Key::Z, egui::Modifiers::COMMAND)],
    );
    assert_eq!(
        app.active().unwrap().draft_text.as_deref(),
        Some(source.as_str())
    );
    draw(
        &ctx,
        &mut app,
        vec![key(egui::Key::A, egui::Modifiers::COMMAND)],
    );
    let output = draw(&ctx, &mut app, vec![egui::Event::Copy]);
    assert!(output.platform_output.commands.iter().any(
        |command| matches!(command, egui::OutputCommand::CopyText(value) if value == &source)
    ));
    assert!(app.active().unwrap().editor_state.large.laid_out_bytes <= 64 * 1024);
}

#[test]
fn background_search_replacement_keeps_single_undo_and_global_cursor() {
    let source = "中hit42\r\n".repeat(40_000);
    let ctx = egui::Context::default();
    let mut app = document(&source);
    select(&ctx, &mut app, 0, 0);
    let tools = &mut app.active_mut().unwrap().editor_state.tools;
    tools.query = "hit(\\d+)".into();
    tools.replacement = "value$1".into();
    tools.regex = true;
    text_tools::background_search::start(app.active_mut().unwrap(), &ctx, 4);
    text_tools::background_search::drain(&mut app, &ctx);
    assert_eq!(
        app.active().unwrap().draft_text.as_deref(),
        Some("中value42\r\n".repeat(40_000).as_str())
    );
    draw(&ctx, &mut app, vec![]);
    draw(
        &ctx,
        &mut app,
        vec![key(egui::Key::Z, egui::Modifiers::COMMAND)],
    );
    assert_eq!(
        app.active().unwrap().draft_text.as_deref(),
        Some(source.as_str())
    );
    draw(
        &ctx,
        &mut app,
        vec![key(
            egui::Key::Z,
            egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
        )],
    );
    assert_eq!(
        app.active().unwrap().draft_text.as_deref(),
        Some("中value42\r\n".repeat(40_000).as_str())
    );
}

#[test]
fn multiline_indent_preserves_direction_newlines_boundary_and_single_undo() {
    for large in [false, true] {
        for ending in ["\n", "\r\n"] {
            for mode in [
                whitespace::TabMode::Default,
                whitespace::TabMode::Spaces2,
                whitespace::TabMode::Spaces4,
            ] {
                for reverse in [false, true] {
                    let original = format!(
                        "中{ending}🙂{ending}tail{ending}{}",
                        "padding\n".repeat(if large { 40_000 } else { 0 })
                    );
                    let ctx = egui::Context::default();
                    let mut app = document(&original);
                    app.tab_mode = mode;
                    let end = format!("中{ending}🙂{ending}").chars().count();
                    let (anchor, primary) = if reverse { (end, 0) } else { (0, end) };
                    select(&ctx, &mut app, anchor, primary);
                    draw(
                        &ctx,
                        &mut app,
                        vec![key(egui::Key::Tab, egui::Modifiers::NONE)],
                    );
                    let unit = if mode == whitespace::TabMode::Default {
                        "\t".to_owned()
                    } else {
                        " ".repeat(mode.width())
                    };
                    let expected = format!(
                        "{unit}中{ending}{unit}🙂{ending}tail{ending}{}",
                        "padding\n".repeat(if large { 40_000 } else { 0 })
                    );
                    assert_eq!(
                        app.active().unwrap().draft_text.as_deref(),
                        Some(expected.as_str())
                    );
                    let cursor = app.active().unwrap().editor_state.cursor.unwrap();
                    assert_eq!(cursor.primary.index < cursor.secondary.index, reverse);
                    draw(
                        &ctx,
                        &mut app,
                        vec![key(egui::Key::Z, egui::Modifiers::COMMAND)],
                    );
                    assert_eq!(
                        app.active().unwrap().draft_text.as_deref(),
                        Some(original.as_str())
                    );
                    draw(
                        &ctx,
                        &mut app,
                        vec![key(
                            egui::Key::Z,
                            egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                        )],
                    );
                    draw(
                        &ctx,
                        &mut app,
                        vec![key(egui::Key::Tab, egui::Modifiers::SHIFT)],
                    );
                    assert_eq!(
                        app.active().unwrap().draft_text.as_deref(),
                        Some(original.as_str())
                    );
                }
            }
        }
    }
}

#[test]
fn shift_tab_removes_one_partial_indent_and_enter_inherits_only_caret_prefix() {
    for large in [false, true] {
        for ending in ["\n", "\r\n"] {
            let ctx = egui::Context::default();
            let original = format!(
                "  \t中文{ending}{}",
                format!("padding{ending}").repeat(if large { 40_000 } else { 0 })
            );
            let mut app = document(&original);
            select(&ctx, &mut app, 5, 5);
            draw(
                &ctx,
                &mut app,
                vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
            );
            assert!(
                app.active()
                    .unwrap()
                    .draft_text
                    .as_ref()
                    .unwrap()
                    .starts_with(&format!("  \t中文{ending}  \t{ending}"))
            );
            draw(
                &ctx,
                &mut app,
                vec![key(egui::Key::Z, egui::Modifiers::COMMAND)],
            );
            select(&ctx, &mut app, 1, 1);
            draw(
                &ctx,
                &mut app,
                vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
            );
            assert!(
                app.active()
                    .unwrap()
                    .draft_text
                    .as_ref()
                    .unwrap()
                    .starts_with(&format!(" {ending}  \t中文"))
            );
            draw(
                &ctx,
                &mut app,
                vec![key(egui::Key::Z, egui::Modifiers::COMMAND)],
            );
            select(&ctx, &mut app, 0, 0);
            draw(
                &ctx,
                &mut app,
                vec![key(egui::Key::Tab, egui::Modifiers::SHIFT)],
            );
            assert!(
                app.active()
                    .unwrap()
                    .draft_text
                    .as_ref()
                    .unwrap()
                    .starts_with("\t中文")
            );
            draw(
                &ctx,
                &mut app,
                vec![key(egui::Key::Tab, egui::Modifiers::SHIFT)],
            );
            assert!(
                app.active()
                    .unwrap()
                    .draft_text
                    .as_ref()
                    .unwrap()
                    .starts_with("中文")
            );
        }
    }
}

#[test]
fn huge_selection_copy_cut_replace_undo_and_navigation_keep_layout_bounded() {
    let source = "中文🙂 line\r\n".repeat(50_000);
    for operation in [
        egui::Event::Cut,
        egui::Event::Paste("replacement".into()),
        key(egui::Key::Delete, egui::Modifiers::NONE),
    ] {
        let ctx = egui::Context::default();
        let mut app = document(&source);
        select(&ctx, &mut app, 0, 0);
        draw(
            &ctx,
            &mut app,
            vec![key(egui::Key::A, egui::Modifiers::COMMAND)],
        );
        assert!(app.active().unwrap().editor_state.large.laid_out_bytes < 64 * 1024);
        let output = draw(&ctx, &mut app, vec![egui::Event::Copy]);
        assert!(output.platform_output.commands.iter().any(
            |command| matches!(command, egui::OutputCommand::CopyText(text) if text == &source)
        ));
        draw(&ctx, &mut app, vec![operation.clone()]);
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some(if matches!(operation, egui::Event::Paste(_)) {
                "replacement"
            } else {
                ""
            })
        );
        draw(
            &ctx,
            &mut app,
            vec![key(egui::Key::Z, egui::Modifiers::COMMAND)],
        );
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some(source.as_str())
        );
        assert!(app.active().unwrap().editor_state.large.laid_out_bytes < 64 * 1024);
        draw(
            &ctx,
            &mut app,
            vec![key(egui::Key::ArrowLeft, egui::Modifiers::NONE)],
        );
        assert_eq!(
            app.active()
                .unwrap()
                .editor_state
                .cursor
                .unwrap()
                .primary
                .index,
            0
        );
        draw(
            &ctx,
            &mut app,
            vec![key(
                egui::Key::ArrowDown,
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
            )],
        );
        assert!(app.active().unwrap().editor_state.large.laid_out_bytes < 64 * 1024);
    }
}

#[test]
fn background_line_batch_is_atomic_version_checked_and_undoable() {
    let original = format!("{}\n", "x".repeat(800)).repeat(3000);
    for stale in [false, true] {
        let ctx = egui::Context::default();
        let mut app = document(&original);
        select(&ctx, &mut app, 0, 0);
        let edits = original
            .match_indices('\n')
            .map(|(at, _)| (at..at, "\r".to_owned()))
            .collect();
        line_commands::commit_edits(app.active_mut().unwrap(), &ctx, edits, None);
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some(original.as_str())
        );
        if stale {
            app.active_mut().unwrap().content_revision += 1;
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let _ = ctx.run(Default::default(), |ctx| app.poll_batch(ctx));
            if app.pending_batch.is_none() {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        let expected = if stale {
            original.clone()
        } else {
            original.replace('\n', "\r\n")
        };
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some(expected.as_str())
        );
        if !stale {
            draw(
                &ctx,
                &mut app,
                vec![key(egui::Key::Z, egui::Modifiers::COMMAND)],
            );
            assert_eq!(
                app.active().unwrap().draft_text.as_deref(),
                Some(original.as_str())
            );
        }
    }
}

#[test]
fn huge_selection_ime_commit_replaces_full_selection_without_full_layout() {
    let source = "中文🙂\r\n".repeat(50_000);
    let ctx = egui::Context::default();
    let mut app = document(&source);
    select(&ctx, &mut app, 0, 0);
    draw(
        &ctx,
        &mut app,
        vec![key(egui::Key::A, egui::Modifiers::COMMAND)],
    );
    draw(
        &ctx,
        &mut app,
        vec![egui::Event::Ime(egui::ImeEvent::Enabled)],
    );
    draw(
        &ctx,
        &mut app,
        vec![egui::Event::Ime(egui::ImeEvent::Preedit("新".into()))],
    );
    draw(
        &ctx,
        &mut app,
        vec![egui::Event::Ime(egui::ImeEvent::Commit("新".into()))],
    );
    assert_eq!(app.active().unwrap().draft_text.as_deref(), Some("新"));
    draw(
        &ctx,
        &mut app,
        vec![key(egui::Key::Z, egui::Modifiers::COMMAND)],
    );
    assert!(app.active().unwrap().draft_text.as_deref() == Some(source.as_str()));
    assert!(app.active().unwrap().editor_state.large.laid_out_bytes < 64 * 1024);
}

#[test]
fn cancelling_prepared_batch_keeps_content_and_newline_preference() {
    let original = format!("{}\n", "x".repeat(800)).repeat(3000);
    let ctx = egui::Context::default();
    let mut app = document(&original);
    select(&ctx, &mut app, 0, 0);
    line_endings::convert(
        app.active_mut().unwrap(),
        &ctx,
        line_endings::LineEnding::Crlf,
    );
    type Handoff = std::sync::Arc<std::sync::Mutex<Option<line_commands::PendingBatch>>>;
    // Cancel the handoff before any worker result is allowed to publish.
    let handoff = ctx
        .data_mut(|data| data.remove_temp::<Handoff>(egui::Id::new("prepared_line_batch")))
        .unwrap();
    drop(handoff.lock().unwrap().take());
    let _ = ctx.run(Default::default(), |ctx| app.poll_batch(ctx));
    assert_eq!(
        app.active().unwrap().line_ending,
        line_endings::LineEnding::Lf
    );
    assert!(app.active().unwrap().draft_text.as_deref() == Some(original.as_str()));
    assert!(app.pending_batch.is_none());
}

#[test]
fn enter_preserves_document_endings_and_does_not_rewrite_paste_or_search() {
    for large in [false, true] {
        for ending in ["\n", "\r\n"] {
            let ctx = egui::Context::default();
            let original = format!(
                "a{ending}b{ending}{}",
                format!("padding{ending}").repeat(if large { 40_000 } else { 1 })
            );
            let mut app = document(&original);
            select(&ctx, &mut app, 1, 1);
            draw(
                &ctx,
                &mut app,
                vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
            );
            let expected = format!("a{ending}{}", &original[1..]);
            assert_eq!(
                app.active().unwrap().draft_text.as_deref(),
                Some(expected.as_str())
            );
            draw(
                &ctx,
                &mut app,
                vec![key(egui::Key::Z, egui::Modifiers::COMMAND)],
            );
            assert_eq!(
                app.active().unwrap().draft_text.as_deref(),
                Some(original.as_str())
            );
            draw(
                &ctx,
                &mut app,
                vec![key(
                    egui::Key::Z,
                    egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                )],
            );
            assert_eq!(
                app.active().unwrap().draft_text.as_deref(),
                Some(expected.as_str())
            );
            draw(&ctx, &mut app, vec![egui::Event::Paste("p\nq\r\nr".into())]);
            assert!(
                app.active()
                    .unwrap()
                    .draft_text
                    .as_ref()
                    .unwrap()
                    .contains("p\nq\r\nr")
            );
            let before = app.active().unwrap().draft_text.clone();
            draw(
                &ctx,
                &mut app,
                vec![key(egui::Key::F, egui::Modifiers::COMMAND)],
            );
            draw(
                &ctx,
                &mut app,
                vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
            );
            assert_eq!(app.active().unwrap().draft_text, before);
        }
    }
}

#[test]
fn line_ending_conversion_is_active_only_undoable_preserves_original_and_recovers() {
    for large in [false, true] {
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mixed.txt");
        let original = "中文\r\nsecond\nthird\r\n".to_owned()
            + &"padding\n".repeat(if large { 40_000 } else { 0 });
        std::fs::write(&path, &original).unwrap();
        let mut app = LiteTextApp::default();
        app.open_paths(vec![path.clone()]);
        select(&ctx, &mut app, 0, 2);
        line_endings::convert(
            app.active_mut().unwrap(),
            &ctx,
            line_endings::LineEnding::Crlf,
        );
        let converted = app.active().unwrap().draft_text.clone().unwrap();
        assert_eq!(line_endings::Counts::of(&converted).lf, 0);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        let cursor = app.active().unwrap().editor_state.cursor.unwrap();
        assert_eq!(
            cursor.primary.index.min(cursor.secondary.index)
                ..cursor.primary.index.max(cursor.secondary.index),
            0..2
        );
        draw(
            &ctx,
            &mut app,
            vec![key(egui::Key::Z, egui::Modifiers::COMMAND)],
        );
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some(original.as_str())
        );
        draw(
            &ctx,
            &mut app,
            vec![key(
                egui::Key::Z,
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
            )],
        );
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some(converted.as_str())
        );
        let snapshot = dir.path().join("session.json");
        recovery::store_buffers(&snapshot, &app.files, app.active_id, false).unwrap();
        let files = recovery::load(&snapshot).unwrap().unwrap().restore();
        assert_eq!(files[0].line_ending, line_endings::LineEnding::Crlf);
        assert_eq!(files[0].draft_text.as_deref(), Some(converted.as_str()));
        recovery::save_document(app.active_mut().unwrap(), &path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), converted);
        app.new_document();
        line_endings::convert(
            app.active_mut().unwrap(),
            &ctx,
            line_endings::LineEnding::Crlf,
        );
        assert_eq!(app.files[0].draft_text.as_deref(), Some(converted.as_str()));
        recovery::store_buffers(&snapshot, &app.files, app.active_id, false).unwrap();
        let files = recovery::load(&snapshot).unwrap().unwrap().restore();
        assert_eq!(files[1].line_ending, line_endings::LineEnding::Crlf);
    }
}

#[test]
fn whitespace_hints_distinguish_actual_endings_without_changing_text() {
    for large in [false, true] {
        let ctx = egui::Context::default();
        let original =
            "a\r\nb\nc\r\n".to_owned() + &"padding\n".repeat(if large { 40_000 } else { 0 });
        let mut app = document(&original);
        app.active_mut().unwrap().editor_state.tools.whitespace = true;
        draw(&ctx, &mut app, vec![]);
        let output = draw(&ctx, &mut app, vec![]);
        for label in ["↵", "CR"] {
            assert!(
                output
                    .shapes
                    .iter()
                    .any(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text() == label)),
                "large={large}: {label}"
            );
        }
        assert!(!output.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Text(t) if ["LF", "CRLF"].contains(&t.galley.text()))));
        let arrows: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == "↵" => Some(t),
                _ => None,
            })
            .collect();
        let crs: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == "CR" => Some(t),
                _ => None,
            })
            .collect();
        assert!(arrows.len() > crs.len(), "LF has an arrow without CR");
        for cr in crs {
            assert!(
                arrows.iter().any(|arrow| {
                    let cr_rect = egui::Rect::from_min_size(cr.pos, cr.galley.size());
                    let arrow_rect = egui::Rect::from_min_size(arrow.pos, arrow.galley.size());
                    (cr_rect.center().x - arrow_rect.center().x).abs() < 3.0
                        && cr_rect.center().y < arrow_rect.center().y
                        && arrow_rect.center().y - cr_rect.center().y < 14.0
                        && cr.galley.size().y < arrow.galley.size().y
                }),
                "CR is smaller and above its return arrow"
            );
        }
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some(original.as_str())
        );
        select(&ctx, &mut app, 0, 0);
        draw(&ctx, &mut app, vec![egui::Event::Text("edit".into())]);
        draw(
            &ctx,
            &mut app,
            vec![key(egui::Key::Z, egui::Modifiers::COMMAND)],
        );
        app.active_mut().unwrap().editor_state.tools.whitespace = false;
        let output = draw(&ctx, &mut app, vec![]);
        assert!(!output.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Text(t) if ["↵", "CR", "LF", "CRLF"].contains(&t.galley.text()))));
    }
}

#[test]
fn status_line_ending_picker_follows_tab_and_converts_only_after_selection() {
    for lang in [Lang::ZhCn, Lang::En] {
        for pal in [&DARK, &LIGHT] {
            for width in [640.0, 1080.0] {
                let ctx = egui::Context::default();
                setup_fonts(&ctx);
                let mut app = document("a\n中文\r\n");
                app.lang = lang;
                let run = |app: &mut LiteTextApp, events| {
                    ctx.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 720.0),
                            )),
                            events,
                            ..Default::default()
                        },
                        |ctx| app.status_bar(ctx, pal),
                    )
                };
                run(&mut app, vec![]);
                let id = egui::Id::new("status_line_ending");
                let rect = ctx
                    .data(|d| d.get_temp::<egui::Rect>(id.with("rect")))
                    .unwrap();
                let tab = ctx
                    .data(|d| {
                        d.get_temp::<egui::Rect>(egui::Id::new("status_tab_mode").with("rect"))
                    })
                    .unwrap();
                assert!(rect.left() > tab.right());
                assert!(
                    rect.right() < width - 8.0,
                    "status picker clipped: {lang:?} {width} {rect:?}"
                );
                let click = |point: egui::Pos2, pressed| {
                    vec![
                        egui::Event::PointerMoved(point),
                        egui::Event::PointerButton {
                            pos: point,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: Default::default(),
                        },
                    ]
                };
                for pressed in [true, false] {
                    run(&mut app, click(rect.center(), pressed));
                }
                run(&mut app, vec![]);
                assert_eq!(
                    app.active().unwrap().draft_text.as_deref(),
                    Some("a\n中文\r\n")
                );
                let choice = ctx
                    .data(|d| d.get_temp::<egui::Rect>(id.with("CRLF")))
                    .unwrap();
                assert!(choice.bottom() < rect.top());
                for pressed in [true, false] {
                    run(&mut app, click(choice.center(), pressed));
                }
                assert_eq!(
                    app.active().unwrap().draft_text.as_deref(),
                    Some("a\r\n中文\r\n")
                );
                assert_eq!(
                    app.active().unwrap().line_ending,
                    line_endings::LineEnding::Crlf
                );
            }
        }
    }
}

#[test]
fn crlf_enter_does_not_remap_ime_confirmation() {
    let ctx = egui::Context::default();
    let mut app = document("a\r\nb");
    select(&ctx, &mut app, 1, 1);
    draw(
        &ctx,
        &mut app,
        vec![egui::Event::Ime(egui::ImeEvent::Enabled)],
    );
    draw(
        &ctx,
        &mut app,
        vec![egui::Event::Ime(egui::ImeEvent::Preedit("中".into()))],
    );
    draw(
        &ctx,
        &mut app,
        vec![
            egui::Event::Ime(egui::ImeEvent::Commit("中".into())),
            key(egui::Key::Enter, egui::Modifiers::NONE),
        ],
    );
    assert_eq!(app.active().unwrap().newline_counts().crlf, 1);
    assert_eq!(app.active().unwrap().newline_counts().lf, 0);
}

#[test]
fn ordinary_typing_across_threshold_keeps_undo_redo_cursor_and_save_baseline() {
    for paste in [false, true] {
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("threshold.txt");
        let original = "line\n".repeat(52_428) + "xy"; // 262142 bytes
        assert_eq!(original.len(), large_editor::THRESHOLD - 2);
        std::fs::write(&path, &original).unwrap();
        let mut app = LiteTextApp::default();
        app.open_paths(vec![path.clone()]);
        select(&ctx, &mut app, 2, 2);
        crate::text_tools::replace_range(app.active_mut().unwrap(), &ctx, 0..1, "L".into());
        let previous = app.active().unwrap().draft_text.clone().unwrap();
        select(&ctx, &mut app, 2, 2);
        let event = if paste {
            egui::Event::Paste("中🙂".into())
        } else {
            egui::Event::Text("中🙂".into())
        };
        draw(&ctx, &mut app, vec![event]);
        let edited = app.active().unwrap().draft_text.clone().unwrap();
        assert!(app.active().unwrap().editor_state.large.active);
        assert_eq!(
            app.active()
                .unwrap()
                .editor_state
                .cursor
                .unwrap()
                .primary
                .index,
            4
        );
        draw(&ctx, &mut app, vec![]);
        draw(
            &ctx,
            &mut app,
            vec![key(egui::Key::Z, egui::Modifiers::COMMAND)],
        );
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some(previous.as_str())
        );
        draw(
            &ctx,
            &mut app,
            vec![key(egui::Key::Z, egui::Modifiers::COMMAND)],
        );
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some(original.as_str())
        );
        assert!(!app.active().unwrap().is_modified());
        for _ in 0..2 {
            draw(
                &ctx,
                &mut app,
                vec![key(
                    egui::Key::Z,
                    egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                )],
            );
        }
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some(edited.as_str())
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }
}

#[test]
fn large_document_navigation_uses_global_endpoints_and_shift_anchor() {
    for (start_key, end_key, modifiers) in [
        (
            egui::Key::ArrowUp,
            egui::Key::ArrowDown,
            egui::Modifiers::COMMAND,
        ),
        (egui::Key::Home, egui::Key::End, egui::Modifiers::CTRL),
    ] {
        let ctx = egui::Context::default();
        let source = "中文🙂 line\r\n".repeat(30_000);
        let length = source.chars().count();
        let mut app = document(&source);
        select(&ctx, &mut app, 20, 20);
        draw(&ctx, &mut app, vec![key(end_key, modifiers)]);
        assert_eq!(
            app.active()
                .unwrap()
                .editor_state
                .cursor
                .unwrap()
                .primary
                .index,
            length
        );
        draw(
            &ctx,
            &mut app,
            vec![key(start_key, modifiers | egui::Modifiers::SHIFT)],
        );
        let cursor = app.active().unwrap().editor_state.cursor.unwrap();
        assert_eq!(cursor.primary.index, 0);
        assert_eq!(cursor.secondary.index, length);
        let output = draw(&ctx, &mut app, vec![egui::Event::Copy]);
        assert!(output.platform_output.commands.iter().any(
            |command| matches!(command, egui::OutputCommand::CopyText(text) if text == &source)
        ));
        draw(&ctx, &mut app, vec![key(start_key, modifiers)]);
        assert_eq!(
            app.active()
                .unwrap()
                .editor_state
                .cursor
                .unwrap()
                .primary
                .index,
            0
        );
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some(source.as_str())
        );
    }
}

#[test]
fn end_navigation_expands_folds_and_keeps_cursor_editable() {
    let ctx = egui::Context::default();
    let mut app = document(&format!("[{}0]", "1,".repeat(70_000)));
    app.format_active_json(&ctx);
    select(&ctx, &mut app, 0, 0);
    app.active_mut().unwrap().editor_state.folds.toggle(0);
    let length = app
        .active()
        .unwrap()
        .draft_text
        .as_ref()
        .unwrap()
        .chars()
        .count();
    draw(
        &ctx,
        &mut app,
        vec![key(egui::Key::ArrowDown, egui::Modifiers::COMMAND)],
    );
    assert_eq!(
        app.active()
            .unwrap()
            .editor_state
            .cursor
            .unwrap()
            .primary
            .index,
        length
    );
    assert!(!app.active().unwrap().editor_state.folds.collapsed(0));
    draw(&ctx, &mut app, vec![egui::Event::Text("x".into())]);
    assert!(
        app.active()
            .unwrap()
            .draft_text
            .as_ref()
            .unwrap()
            .ends_with("]x")
    );
}

#[test]
fn newline_rendering_marks_fragment_boundary_but_not_soft_wrapping() {
    let ctx = egui::Context::default();
    let _ = ctx.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let galley = ui.painter().layout(
                "a long line wraps here".to_owned(),
                egui::FontId::monospace(13.0),
                DARK.text_main,
                35.0,
            );
            assert!(galley.rows.len() > 1);
            text_tools::paint_whitespace(ui, &galley, egui::pos2(30.0, 30.0), &DARK, "\r\nnext");
        });
    });
    let output = ctx.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let galley = ui.painter().layout(
                "a long line wraps here".to_owned(),
                egui::FontId::monospace(13.0),
                DARK.text_main,
                35.0,
            );
            text_tools::paint_whitespace(ui, &galley, egui::pos2(30.0, 30.0), &DARK, "\r\nnext");
        });
    });
    let hints: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::Shape::Text(t) if ["↵", "CR", "LF", "CRLF"].contains(&t.galley.text()) => {
                Some(t.galley.text())
            }
            _ => None,
        })
        .collect();
    assert_eq!(hints, ["↵", "CR"]);
}

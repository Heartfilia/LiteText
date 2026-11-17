use super::*;

fn poll(app: &mut LiteTextApp, ctx: &egui::Context) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let _ = ctx.run(Default::default(), |ctx| app.reload_dialog(ctx));
        if app
            .reload_task
            .as_ref()
            .is_none_or(|task| task.receiver.is_none())
        {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
}

fn app(dir: &std::path::Path) -> LiteTextApp {
    LiteTextApp {
        recovery_path_override: Some(dir.join("session.json")),
        ..Default::default()
    }
}

#[test]
fn reload_confirmation_ui_cancel_and_confirm_are_transactional() {
    for confirm in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.txt");
        std::fs::write(&path, "original").unwrap();
        let mut app = app(dir.path());
        app.lang = Lang::En;
        app.open_paths(vec![path.clone()]);
        let ctx = egui::Context::default();
        let id = app.active_id.unwrap();
        crate::text_tools::replace_range(app.active_mut().unwrap(), &ctx, 0..0, "edited ".into());
        std::fs::write(&path, "latest").unwrap();
        app.request_reload(id, false);
        let cancelled = app.reload_task.as_ref().unwrap().cancel.clone();
        let mut position = None;
        for frame in 0..4 {
            let events = if frame >= 2 {
                let pos = position.unwrap();
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
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 600.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| app.reload_dialog(ctx),
            );
            if frame == 1 {
                position = output.shapes.iter().find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text)
                        if text.galley.text()
                            == if confirm {
                                "Reload"
                            } else {
                                crate::tr(app.lang).cancel
                            } =>
                    {
                        Some(text.pos + text.galley.size() * 0.5)
                    }
                    _ => None,
                });
                assert!(position.is_some());
            }
        }
        if confirm {
            poll(&mut app, &ctx);
        }
        assert!(app.reload_task.is_none());
        assert!(cancelled.load(Ordering::Relaxed));
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some(if confirm { "latest" } else { "edited original" })
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), "latest");
    }
}

#[test]
fn reload_confirmation_cancel_and_failure_preserve_content_history_and_association() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "original\r\n").unwrap();
    let mut app = app(dir.path());
    app.open_paths(vec![path.clone()]);
    let ctx = egui::Context::default();
    let id = app.active_id.unwrap();
    let original_path = app.active().unwrap().path.clone();
    crate::text_tools::replace_range(app.active_mut().unwrap(), &ctx, 0..0, "edited ".into());
    let original = app.active().unwrap().draft_text.clone();
    app.request_reload(id, false);
    assert!(app.reload_task.as_ref().unwrap().confirm);
    assert!(app.reload_task.as_ref().unwrap().receiver.is_none());
    assert_eq!(app.active().unwrap().draft_text, original);
    app.reload_task = None;
    std::fs::remove_file(&path).unwrap();
    app.request_reload(id, false);
    app.begin_reload();
    poll(&mut app, &ctx);
    assert!(app.reload_task.as_ref().unwrap().error.is_some());
    assert_eq!(app.active().unwrap().draft_text, original);
    assert_eq!(app.active().unwrap().path, original_path);
    let native = egui::TextEdit::load_state(&ctx, egui::Id::new("draft_editor").with(id)).unwrap();
    assert!(native.undoer().has_undo(&(
        native.cursor.char_range().unwrap_or_default(),
        original.unwrap()
    )));
}

#[test]
fn background_reload_preserves_read_encoding_preferences_and_line_without_stealing_focus() {
    for large in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gbk.txt");
        let encoding = recovery::Encoding::Gbk;
        let original = "中文\r\n".repeat(if large { 50_000 } else { 3 });
        std::fs::write(&path, encoding.encode(&original).unwrap()).unwrap();
        let mut app = app(dir.path());
        app.queue_open(vec![path.clone()]);
        // For this test the existing document is explicitly decoded by the shared loader.
        app.cancel_open();
        let mut loaded = background_io::load_with_encoding(
            &path,
            &AtomicBool::new(false),
            &AtomicU64::new(0),
            &AtomicU64::new(0),
            Some(encoding),
        )
        .unwrap();
        loaded.id = 1;
        loaded.editor_state.cursor_line = 2;
        loaded.editor_state.tools.wrap = true;
        loaded.editor_state.tools.whitespace = true;
        loaded.language_override = Some("Plain Text".into());
        app.files.push(loaded);
        app.active_id = Some(1);
        app.next_file_id = 2;
        // An unsaved save-encoding choice must not change the read encoding.
        app.active_mut()
            .unwrap()
            .change_encoding(recovery::Encoding::Utf8)
            .unwrap();
        let updated = "新内容\r\n".repeat(if large { 50_000 } else { 4 });
        std::fs::write(&path, encoding.encode(&updated).unwrap()).unwrap();
        app.request_reload(1, true);
        app.new_document();
        let selected = app.active_id;
        let ctx = egui::Context::default();
        poll(&mut app, &ctx);
        assert!(app.reload_task.is_none());
        assert_eq!(app.active_id, selected);
        let file = &app.files[0];
        assert_eq!(file.encoding, encoding);
        assert_eq!(file.saved_encoding, encoding);
        assert_eq!(file.draft_text.as_deref(), Some(updated.as_str()));
        assert!(file.editor_state.tools.wrap && file.editor_state.tools.whitespace);
        assert_eq!(file.language_override.as_deref(), Some("Plain Text"));
        assert_eq!(file.editor_state.pending_selection, Some(10..10));
        assert!(!file.is_modified());
        assert_eq!(file.id, 1);
        assert!(
            !egui::TextEdit::load_state(&ctx, egui::Id::new("draft_editor").with(1))
                .unwrap()
                .undoer()
                .has_undo(&(egui::text::CCursorRange::default(), updated))
        );
    }
}

#[test]
fn stale_reload_results_and_replaced_paths_cannot_overwrite_working_copy() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "original").unwrap();
    let mut app = app(dir.path());
    app.open_paths(vec![path.clone()]);
    let ctx = egui::Context::default();
    for stale in [true, false] {
        app.request_reload(app.active_id.unwrap(), true);
        let receiver = app.reload_task.as_ref().unwrap().receiver.as_ref().unwrap();
        let file = receiver.recv().unwrap().unwrap();
        let (sender, receiver) = mpsc::channel();
        sender.send(Ok(file)).unwrap();
        app.reload_task.as_mut().unwrap().receiver = Some(receiver);
        if stale {
            app.active_mut().unwrap().content_revision += 1;
        } else {
            std::fs::write(&path, "different length").unwrap();
        }
        poll(&mut app, &ctx);
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some("original")
        );
        if stale {
            assert!(app.reload_task.is_none());
        } else {
            assert!(app.reload_task.as_ref().unwrap().error.is_some());
        }
    }
}

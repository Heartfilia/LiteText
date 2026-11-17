use super::*;

fn drain(app: &mut LiteTextApp, ctx: &egui::Context) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while app.encoding_task.is_some() {
        let _ = ctx.run(Default::default(), |ctx| app.encoding_dialog(ctx));
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
}

#[test]
fn explicit_encoding_round_trips_bom_and_rejects_invalid_or_mismatched_input() {
    for encoding in Encoding::CHOICES {
        let text = if encoding == Encoding::Windows1252 {
            "café\r\n"
        } else if encoding == Encoding::ShiftJis {
            "日本語\r\n"
        } else {
            "中文\r\n"
        };
        let bytes = encoding.encode(text).unwrap();
        assert_eq!(
            encoding.decode_explicit(&bytes).unwrap(),
            text,
            "{encoding:?}"
        );
        assert_eq!(
            encoding
                .encode(&encoding.decode_explicit(&bytes).unwrap())
                .unwrap(),
            bytes
        );
    }
    for encoding in [
        Encoding::Utf8,
        Encoding::Utf8Bom,
        Encoding::Utf16Le,
        Encoding::Utf16Be,
    ] {
        assert!(encoding.decode_explicit(&[0xff]).is_err());
    }
    assert!(
        Encoding::Utf8
            .decode_explicit(&Encoding::Utf8Bom.encode("text").unwrap())
            .is_err()
    );
    assert!(Encoding::Utf8Bom.decode_explicit(b"plain").is_err());
    assert!(
        Encoding::Utf16Le
            .decode_explicit(&[0xff, 0xfe, 0x00, 0xd8])
            .is_err()
    );
}

#[test]
fn reopen_rejects_disk_change_before_publish_and_offers_retry_without_touching_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "original").unwrap();
    let mut app = LiteTextApp::default();
    app.open_paths(vec![path.clone()]);
    app.request_encoding(true, Encoding::Utf8);
    app.active_mut().unwrap().pinned = true;
    app.begin_encoding();
    let result = app
        .encoding_task
        .as_ref()
        .unwrap()
        .receiver
        .as_ref()
        .unwrap()
        .recv()
        .unwrap();
    let (sender, receiver) = mpsc::channel();
    sender.send(result).unwrap();
    app.encoding_task.as_mut().unwrap().receiver = Some(receiver);
    std::fs::write(&path, "latest content").unwrap();
    let ctx = egui::Context::default();
    let _ = ctx.run(Default::default(), |ctx| app.encoding_dialog(ctx));
    assert!(app.encoding_task.as_ref().unwrap().error.is_some());
    assert!(app.encoding_task.as_ref().unwrap().receiver.is_none());
    assert_eq!(
        app.active().unwrap().draft_text.as_deref(),
        Some("original")
    );
    app.begin_encoding();
    drain(&mut app, &ctx);
    assert_eq!(
        app.active().unwrap().draft_text.as_deref(),
        Some("latest content")
    );
    assert!(app.active().unwrap().pinned);
}

#[test]
fn conversion_is_transactional_preserves_undo_and_only_manual_save_changes_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.txt");
    std::fs::write(&path, "中文\r\n").unwrap();
    let ctx = egui::Context::default();
    let mut app = LiteTextApp {
        recovery_path_override: Some(dir.path().join("session.json")),
        ..Default::default()
    };
    app.open_paths(vec![path.clone()]);
    crate::text_tools::replace_range(app.active_mut().unwrap(), &ctx, 0..0, "新".into());
    app.request_encoding(false, Encoding::Gbk);
    app.begin_encoding();
    drain(&mut app, &ctx);
    let file = app.active().unwrap();
    assert_eq!(file.encoding, Encoding::Gbk);
    assert_eq!(file.saved_encoding, Encoding::Utf8);
    assert_eq!(file.draft_text.as_deref(), Some("新中文\r\n"));
    assert!(file.is_modified());
    assert_eq!(std::fs::read(&path).unwrap(), "中文\r\n".as_bytes());
    let native =
        egui::TextEdit::load_state(&ctx, egui::Id::new("draft_editor").with(file.id)).unwrap();
    let mut undoer = native.undoer();
    let point = (
        native.cursor.char_range().unwrap_or_default(),
        "新中文\r\n".to_owned(),
    );
    assert_eq!(undoer.undo(&point).unwrap().1, "中文\r\n");
    app.write_recovery().unwrap();
    let restored = crate::recovery::load(&dir.path().join("session.json"))
        .unwrap()
        .unwrap()
        .restore();
    assert_eq!(restored[0].encoding, Encoding::Gbk);
    assert_eq!(restored[0].saved_encoding, Encoding::Utf8);
    app.request_encoding(false, Encoding::Windows1252);
    app.begin_encoding();
    drain(&mut app, &ctx);
    assert!(app.io_error.is_some());
    assert_eq!(app.active().unwrap().encoding, Encoding::Gbk);
    assert_eq!(
        app.active().unwrap().draft_text.as_deref(),
        Some("新中文\r\n")
    );
    crate::recovery::save_document(app.active_mut().unwrap(), &path).unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        Encoding::Gbk.encode("新中文\r\n").unwrap()
    );
}

#[test]
fn reopen_lossy_input_is_confirmed_cancellable_versioned_and_failure_safe() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gbk.txt");
    let bytes = Encoding::Gbk.encode("中文\r\n").unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let ctx = egui::Context::default();
    let mut app = LiteTextApp::default();
    app.open_paths(vec![path.clone()]);
    assert_eq!(app.active().unwrap().encoding, Encoding::Lossy);
    let initial = app.active().unwrap().draft_text.clone();
    app.request_encoding(true, Encoding::Gbk);
    assert!(app.encoding_task.as_ref().unwrap().receiver.is_none());
    assert_eq!(app.active().unwrap().draft_text, initial);
    app.encoding_task = None;
    for stale in [true, false] {
        app.request_encoding(true, Encoding::Gbk);
        app.begin_encoding();
        if stale {
            app.active_mut().unwrap().content_revision += 1;
        }
        drain(&mut app, &ctx);
        if stale {
            assert_eq!(app.active().unwrap().encoding, Encoding::Lossy);
        }
    }
    assert_eq!(app.active().unwrap().encoding, Encoding::Gbk);
    assert_eq!(
        app.active().unwrap().draft_text.as_deref(),
        Some("中文\r\n")
    );
    assert!(!app.active().unwrap().is_modified());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    app.request_encoding(true, Encoding::Utf8);
    app.begin_encoding();
    drain(&mut app, &ctx);
    assert!(app.io_error.is_some());
    assert_eq!(app.active().unwrap().encoding, Encoding::Gbk);
    assert_eq!(
        app.active().unwrap().draft_text.as_deref(),
        Some("中文\r\n")
    );
}

#[test]
fn encoding_conversion_cancellation_and_focus_change_never_publish_wrong_target() {
    let ctx = egui::Context::default();
    let mut app = LiteTextApp::default();
    app.new_document();
    let first = app.active_id;
    app.active_mut().unwrap().draft_text = Some("中文\n".repeat(60_000));
    app.active_mut().unwrap().sync_draft();
    app.request_encoding(false, Encoding::Gb18030);
    app.begin_encoding();
    app.encoding_task = None;
    assert_eq!(app.active().unwrap().encoding, Encoding::Utf8);
    app.request_encoding(false, Encoding::Utf8Bom);
    app.begin_encoding();
    app.new_document();
    let second = app.active_id;
    drain(&mut app, &ctx);
    assert_eq!(app.active_id, second);
    assert_eq!(
        app.files
            .iter()
            .find(|file| Some(file.id) == first)
            .unwrap()
            .encoding,
        Encoding::Utf8Bom
    );
    assert_eq!(app.active().unwrap().encoding, Encoding::Utf8);
}

#[test]
fn status_picker_routes_conversion_to_confirmation_and_cancel_keeps_working_copy() {
    for lang in [Lang::ZhCn, Lang::En] {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        ctx.style_mut(|style| style.animation_time = 0.0);
        let mut app = LiteTextApp {
            lang,
            ..Default::default()
        };
        app.new_document();
        app.active_mut().unwrap().draft_text = Some("中文🙂\r\n".into());
        app.active_mut().unwrap().sync_draft();
        let draw = |app: &mut LiteTextApp, events| {
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
                    app.status_bar(ctx, &crate::DARK);
                    app.encoding_dialog(ctx);
                },
            )
        };
        let click = |app: &mut LiteTextApp, rect: egui::Rect| {
            for pressed in [true, false] {
                draw(
                    app,
                    vec![
                        egui::Event::PointerMoved(rect.center()),
                        egui::Event::PointerButton {
                            pos: rect.center(),
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: Default::default(),
                        },
                    ],
                );
            }
        };
        for _ in 0..2 {
            draw(&mut app, vec![]);
        }
        let rect = ctx
            .data(|data| data.get_temp::<egui::Rect>(egui::Id::new("status_encoding").with("rect")))
            .unwrap();
        click(&mut app, rect);
        draw(&mut app, vec![]);
        let choice = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(
                    egui::Id::new("status_encoding").with((false, Encoding::Utf8Bom.name())),
                )
            })
            .unwrap();
        click(&mut app, choice);
        draw(&mut app, vec![]);
        assert!(app.encoding_task.is_some());
        assert_eq!(app.active().unwrap().encoding, Encoding::Utf8);
        let cancel = ctx
            .data(|data| data.get_temp::<egui::Rect>(egui::Id::new("encoding_cancel_rect")))
            .unwrap();
        click(&mut app, cancel);
        assert!(app.encoding_task.is_none());
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some("中文🙂\r\n")
        );
        assert_eq!(app.active().unwrap().encoding, Encoding::Utf8);
    }
}

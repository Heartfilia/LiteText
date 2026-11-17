use super::*;

#[test]
fn hidden_entry_requires_five_consecutive_clicks_without_enabling_recording() {
    let mut app = LiteTextApp::default();
    assert!(!app.diagnostics_settings.unlocked && !app.diagnostics_settings.enabled);
    for time in [0.0, 0.2, 0.4, 0.6] {
        app.diagnostics_version_click(time);
    }
    assert!(!app.diagnostics_settings.unlocked);
    app.diagnostics_version_click(4.0); // Break the sequence.
    assert!(!app.diagnostics_settings.unlocked);
    for time in [4.2, 4.4, 4.6, 4.8] {
        app.diagnostics_version_click(time);
    }
    assert!(app.diagnostics_settings.unlocked && !app.diagnostics_settings.enabled);
    app.diagnostic_event("ERROR", "test", "disabled message");
    assert!(app.diagnostics.buffer.lines.is_empty());
    let restored: LiteTextApp =
        serde_json::from_str(&serde_json::to_string(&app).unwrap()).unwrap();
    assert!(!restored.diagnostics_settings.unlocked && !restored.diagnostics_settings.enabled);
    assert!(!restored.diagnostics.page);
}

#[test]
fn each_launch_hides_diagnostics_and_ignores_legacy_enabled_settings() {
    let mut legacy = serde_json::to_value(LiteTextApp::default()).unwrap();
    legacy["diagnostics_settings"] = serde_json::json!({ "unlocked": true, "enabled": true });
    let mut app: LiteTextApp = serde_json::from_value(legacy).unwrap();
    assert!(!app.diagnostics_settings.unlocked && !app.diagnostics_settings.enabled);

    // Startup is also defensive if a caller supplies an already enabled app.
    app.diagnostics_settings = Settings {
        unlocked: true,
        enabled: true,
    };
    app.diagnostics.page = true;
    app.diagnostic_event("WARN", "test", "previous run");
    app.initialize_diagnostics();
    assert!(!app.diagnostics_settings.unlocked && !app.diagnostics_settings.enabled);
    assert!(!app.diagnostics.page && app.diagnostics.buffer.lines.is_empty());
    app.show_settings = true;
    app.terminal_installed = Some(vec![]);
    let ctx = egui::Context::default();
    for _ in 0..4 {
        draw(&ctx, &mut app, vec![]);
    }
    assert!(
        ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new("settings_diagnostics_rect")))
            .is_none()
    );
    for time in [1.0, 1.2, 1.4, 1.6, 1.8] {
        app.diagnostics_version_click(time);
    }
    assert!(app.diagnostics_settings.unlocked && !app.diagnostics_settings.enabled);
    app.set_diagnostic_recording(true);
    app.show_settings = false;
    draw(&ctx, &mut app, vec![]);
    app.show_settings = true;
    draw(&ctx, &mut app, vec![]);
    assert!(app.diagnostics_settings.unlocked && app.diagnostics_settings.enabled);
}

#[test]
fn bounded_buffer_redacts_secrets_and_disabled_recording_preserves_existing_logs() {
    let mut app = LiteTextApp {
        diagnostics_settings: Settings {
            unlocked: true,
            enabled: true,
        },
        ..Default::default()
    };
    app.diagnostic_event(
        "WARN",
        "test",
        "failure /Users/private/person/file.txt password=secret",
    );
    app.diagnostic_event("ERROR", "test", "token=super-secret");
    assert!(!app.diagnostics.buffer.text().contains("super-secret"));
    assert!(!app.diagnostics.buffer.text().contains("/Users/private"));
    assert!(app.diagnostics.buffer.text().contains("[redacted]"));
    for _ in 0..MAX_ENTRIES + 20 {
        app.diagnostic_event("INFO", "test", &"🙂".repeat(3000));
    }
    assert!(app.diagnostics.buffer.lines.len() <= MAX_ENTRIES);
    assert!(app.diagnostics.buffer.bytes <= MAX_BYTES);
    assert!(app.diagnostics.buffer.dropped > 0);
    let before = app.diagnostics.buffer.text();
    app.diagnostics_settings.enabled = false;
    app.diagnostic_event("ERROR", "test", "must not appear");
    app.io_error = Some("private input".into());
    app.observe_diagnostics();
    assert_eq!(app.diagnostics.buffer.text(), before);
    assert!(framework_target("egui::context"));
    assert!(!framework_target("reqwest::client"));
}

#[test]
fn runtime_logger_captures_gui_warnings_and_respects_disabled_and_private_targets() {
    let logger = RuntimeLogger {
        enabled: AtomicBool::new(false),
        buffer: Mutex::new(Buffer::default()),
    };
    let record = |target: &'static str| {
        log::Record::builder()
            .args(format_args!("GUI warning /Users/private/path"))
            .level(log::Level::Warn)
            .target(target)
            .build()
    };
    log::Log::log(&logger, &record("egui::context"));
    assert!(logger.buffer.lock().unwrap().lines.is_empty());
    logger.enabled.store(true, Ordering::Relaxed);
    log::Log::log(&logger, &record("reqwest::client"));
    assert!(logger.buffer.lock().unwrap().lines.is_empty());
    log::Log::log(&logger, &record("egui::context"));
    let buffer = logger.buffer.lock().unwrap();
    assert_eq!(buffer.lines.len(), 1);
    assert!(buffer.text().contains("GUI warning"));
    assert!(!buffer.text().contains("/Users/private"));
}

#[test]
fn slow_frame_warnings_are_opt_in_and_rate_limited_and_hidden_enable_is_rejected() {
    let mut app = LiteTextApp::default();
    app.diagnostic_frame(std::time::Duration::from_secs(1));
    assert!(app.diagnostics.buffer.lines.is_empty());
    app.diagnostics_settings.enabled = true;
    app.initialize_diagnostics();
    assert!(!app.diagnostics_settings.enabled);
    app.diagnostics_settings.unlocked = true;
    app.set_diagnostic_recording(true);
    app.diagnostic_frame(std::time::Duration::from_millis(150));
    app.diagnostic_frame(std::time::Duration::from_millis(180));
    assert_eq!(
        app.diagnostics
            .buffer
            .text()
            .matches("Slow UI frame")
            .count(),
        1
    );
    app.set_diagnostic_recording(false);
    let before = app.diagnostics.buffer.text();
    app.diagnostic_frame(std::time::Duration::from_secs(2));
    assert_eq!(app.diagnostics.buffer.text(), before);
}

#[test]
fn captures_unpin_and_errors_once_without_paths_or_document_contents() {
    let mut app = LiteTextApp::default();
    app.new_document();
    let id = app.active_id.unwrap();
    app.active_mut().unwrap().draft_text = Some("SECRET DOCUMENT".into());
    app.diagnostics_settings = Settings {
        unlocked: true,
        enabled: true,
    };
    app.toggle_pin(id);
    app.toggle_pin(id);
    app.io_error = Some("/Users/person/SECRET FILE".into());
    app.observe_diagnostics();
    app.observe_diagnostics();
    let text = app.diagnostics.buffer.text();
    assert!(text.contains("Unpinned tab"));
    assert_eq!(text.matches("IO error displayed").count(), 1);
    assert!(!text.contains("SECRET"));
    app.report_save_failure(
        id,
        "/private/sensitive.txt".into(),
        std::io::Error::from(std::io::ErrorKind::PermissionDenied),
    );
    assert!(app.diagnostics.buffer.text().contains("PermissionDenied"));
    let serialized = serde_json::to_string(&app).unwrap();
    assert!(!serialized.contains("Unpinned tab"));
    assert!(!serialized.contains("diagnostics_settings"));
    let mut restored: LiteTextApp = serde_json::from_str(&serialized).unwrap();
    restored.initialize_diagnostics();
    assert!(!restored.diagnostics_settings.unlocked && !restored.diagnostics_settings.enabled);
    assert!(!restored.diagnostics.buffer.text().contains("Unpinned tab"));
}

#[test]
fn painted_transient_id_collisions_are_captured_without_arbitrary_widget_text() {
    let ctx = egui::Context::default();
    let mut app = LiteTextApp {
        diagnostics_settings: Settings {
            unlocked: true,
            enabled: true,
        },
        ..Default::default()
    };
    let _ = ctx.run(Default::default(), |ctx| {
        let id = egui::Id::new("duplicate");
        ctx.check_for_id_clash(
            id,
            egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(30.0, 30.0)),
            "PRIVATE DOCUMENT",
        );
        ctx.check_for_id_clash(
            id,
            egui::Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(30.0, 30.0)),
            "PRIVATE DOCUMENT",
        );
        app.capture_gui_diagnostics(ctx);
    });
    let text = app.diagnostics.buffer.text();
    assert!(text.contains("Widget ID collision"));
    assert!(!text.contains("PRIVATE DOCUMENT"));
}

fn draw(ctx: &egui::Context, app: &mut LiteTextApp, events: Vec<egui::Event>) -> egui::FullOutput {
    ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 800.0),
            )),
            events,
            ..Default::default()
        },
        |ctx| {
            app.settings_window(ctx, &crate::DARK);
        },
    )
}

fn rect(ctx: &egui::Context, name: &str) -> egui::Rect {
    ctx.data(|d| d.get_temp(egui::Id::new(name))).unwrap()
}

fn click(ctx: &egui::Context, app: &mut LiteTextApp, name: &str) {
    let pos = rect(ctx, name).center();
    for pressed in [true, false] {
        draw(
            ctx,
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
}

#[test]
fn real_settings_version_unlock_toggle_copy_clear_and_back() {
    for lang in [Lang::ZhCn, Lang::En] {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = LiteTextApp {
            lang,
            show_settings: true,
            terminal_installed: Some(vec![]),
            ..Default::default()
        };
        for _ in 0..4 {
            draw(&ctx, &mut app, vec![]);
        }
        assert!(
            ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new("settings_diagnostics_rect")))
                .is_none()
        );
        // Version is near the bottom of the settings home scroll area.
        let pos = rect(&ctx, "settings_home_scroll_rect").center();
        draw(
            &ctx,
            &mut app,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -10000.0),
                    modifiers: Default::default(),
                },
            ],
        );
        for _ in 0..30 {
            draw(&ctx, &mut app, vec![]);
        }
        for _ in 0..4 {
            click(&ctx, &mut app, "settings_version_rect");
        }
        assert!(!app.diagnostics_settings.unlocked);
        click(&ctx, &mut app, "settings_version_rect");
        assert!(app.diagnostics_settings.unlocked && !app.diagnostics_settings.enabled);
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        click(&ctx, &mut app, "settings_diagnostics_rect");
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        assert!(app.diagnostics.page);
        click(&ctx, &mut app, "diagnostics_toggle_rect");
        assert!(app.diagnostics_settings.enabled);
        app.diagnostic_event("WARN", "test", "A brief warning was captured");
        let pos = rect(&ctx, "diagnostics_copy_rect").center();
        draw(
            &ctx,
            &mut app,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
            ],
        );
        let output = draw(
            &ctx,
            &mut app,
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
        );
        assert!(output.platform_output.commands.iter().any(
            |c| matches!(c, egui::OutputCommand::CopyText(text) if text.contains("A brief warning"))
        ));
        click(&ctx, &mut app, "diagnostics_toggle_rect");
        assert!(!app.diagnostics_settings.enabled);
        click(&ctx, &mut app, "diagnostics_clear_rect");
        assert!(app.diagnostics.buffer.lines.is_empty());
        click(&ctx, &mut app, "diagnostics_back_rect");
        assert!(!app.diagnostics.page);
    }
}

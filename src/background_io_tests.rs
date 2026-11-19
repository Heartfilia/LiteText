use super::*;

fn drain(app: &mut LiteTextApp, ctx: &egui::Context) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        app.poll_background_io(ctx);
        if app.background_io.opening.is_none()
            && app.background_io.queue.is_empty()
            && app.background_io.recovery.is_none()
            && app.background_io.capturing.is_none()
            && app.background_io.disk.is_none()
        {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "worker timed out");
        std::thread::yield_now();
    }
}

#[test]
fn recovery_capture_is_sliced_utf8_safe_and_discards_partially_copied_versions() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app(dir.path());
    app.new_document();
    let text = "中文🙂\r\n".repeat(300_000);
    app.active_mut().unwrap().draft_text = Some(text.clone());
    app.active_mut().unwrap().sync_draft();
    app.start_background_recovery();
    app.poll_background_io(&egui::Context::default());
    assert!(app.background_io.capturing.is_some());
    assert!(app.background_io.recovery.is_none());
    app.active_mut().unwrap().draft_text = Some("new revision".into());
    app.active_mut().unwrap().sync_draft();
    drain(&mut app, &egui::Context::default());
    assert!(!dir.path().join("session.json").exists());
    app.active_mut().unwrap().draft_text = Some(text.clone());
    app.active_mut().unwrap().sync_draft();
    app.start_background_recovery();
    drain(&mut app, &egui::Context::default());
    assert_eq!(
        recovery::load(&dir.path().join("session.json"))
            .unwrap()
            .unwrap()
            .restore()[0]
            .draft_text
            .as_deref(),
        Some(text.as_str())
    );
}

fn app(dir: &Path) -> LiteTextApp {
    LiteTextApp {
        recovery_path_override: Some(dir.join("session.json")),
        ..Default::default()
    }
}

#[test]
fn stable_loader_rejects_in_place_changes_and_path_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file.txt");
    for replace in [false, true] {
        std::fs::write(&path, "initial data").unwrap();
        let result = load_inner(
            &path,
            &AtomicBool::new(false),
            &AtomicU64::new(0),
            &AtomicU64::new(0),
            &AtomicU8::new(0),
            None,
            || {
                if replace {
                    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
                    std::fs::rename(&path, dir.path().join("old.txt")).unwrap();
                    std::fs::write(&path, "new contents").unwrap();
                    // Same length and modification time: identity must detect replacement.
                    std::fs::File::options()
                        .write(true)
                        .open(&path)
                        .unwrap()
                        .set_times(std::fs::FileTimes::new().set_modified(modified))
                        .unwrap();
                } else {
                    std::fs::write(&path, "changed content length").unwrap();
                }
            },
        );
        let Err(error) = result else {
            panic!("loader accepted changed file (path replacement: {replace})");
        };
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
    }
}

#[test]
fn late_open_results_revalidate_disk_before_publication_and_retry_latest() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "initial").unwrap();
    let mut app = app(dir.path());
    let ctx = egui::Context::default();
    app.queue_open(vec![path.clone()]);
    app.poll_background_io(&ctx);
    let file = app
        .background_io
        .opening
        .as_ref()
        .unwrap()
        .receiver
        .recv()
        .unwrap()
        .unwrap();
    let (sender, receiver) = mpsc::channel();
    sender.send(Ok(file)).unwrap();
    app.background_io.opening.as_mut().unwrap().receiver = receiver;
    std::fs::write(&path, "latest version").unwrap();
    drain(&mut app, &ctx);
    assert!(app.files.is_empty());
    assert_eq!(app.open_errors.len(), 1);
    app.retry_open_errors();
    drain(&mut app, &ctx);
    assert!(app.open_errors.is_empty());
    assert_eq!(app.files[0].draft_text.as_deref(), Some("latest version"));
    assert_eq!(app.active_id, None);
}

#[test]
fn queue_focus_stays_with_user_across_later_items_and_new_retry_tabs() {
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<_> = (0..3)
        .map(|i| dir.path().join(format!("{i}.txt")))
        .collect();
    for path in &paths {
        std::fs::write(path, "contents").unwrap();
    }
    let mut app = app(dir.path());
    let ctx = egui::Context::default();
    app.new_document();
    let original = app.active_id;
    app.queue_open(paths[..2].to_vec());
    app.poll_background_io(&ctx);
    // Switching while the first read is pending must also protect the second item.
    app.new_document();
    let selected = app.active_id;
    drain(&mut app, &ctx);
    assert_eq!(app.active_id, selected);
    assert_eq!(app.files.len(), 4);
    // Retry a genuinely new tab, not merely an already-open path.
    app.open_errors.push(crate::file_actions::OpenError {
        path: paths[2].clone(),
        reason: "previous failure".into(),
    });
    app.retry_open_errors();
    drain(&mut app, &ctx);
    assert_eq!(app.active_id, selected);
    assert_eq!(app.files.len(), 5);
    // A fresh explicit batch can take focus again when the user leaves it alone.
    app.active_id = original;
    let path = dir.path().join("new.txt");
    std::fs::write(&path, "new").unwrap();
    app.queue_open(vec![path]);
    drain(&mut app, &ctx);
    assert_eq!(app.active_id, app.files.last().map(|file| file.id));
}

#[test]
fn skip_current_keeps_queue_and_failed_retry_never_switches_existing_tabs() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("first.txt");
    let second = dir.path().join("second.txt");
    std::fs::write(&first, "first").unwrap();
    std::fs::write(&second, "second").unwrap();
    let mut app = app(dir.path());
    let ctx = egui::Context::default();
    app.queue_open(vec![first.clone(), second.clone()]);
    app.poll_background_io(&ctx);
    let cancel = app.background_io.opening.as_ref().unwrap().cancel.clone();
    app.skip_open();
    assert!(cancel.load(Ordering::Relaxed));
    assert_eq!(app.background_io.completed, 1);
    assert_eq!(app.background_io.queue.len(), 1);
    drain(&mut app, &ctx);
    assert_eq!(app.files.len(), 1);
    assert_eq!(app.active().unwrap().draft_text.as_deref(), Some("second"));
    app.new_document();
    let selected = app.active_id;
    app.open_errors.push(crate::file_actions::OpenError {
        path: second,
        reason: "old failure".into(),
    });
    app.retry_open_errors();
    drain(&mut app, &ctx);
    assert_eq!(app.active_id, selected);
    assert_eq!(app.files.len(), 2);
}

#[test]
fn startup_restore_prepares_large_indices_and_close_barrier_keeps_session() {
    let dir = tempfile::tempdir().unwrap();
    let mut original = app(dir.path());
    original.new_document();
    let text = "中文🙂\r\n".repeat(60_000);
    original.active_mut().unwrap().draft_text = Some(text.clone());
    original.active_mut().unwrap().sync_draft();
    original.write_recovery().unwrap();
    for barrier in [false, true] {
        let mut restored = app(dir.path());
        restored.start_restore(dir.path().join("session.json"), None);
        assert!(restored.restoring());
        assert!(restored.shortcuts_blocked());
        restored.start_background_recovery();
        assert!(restored.background_io.recovery.is_none());
        if barrier {
            restored.write_recovery().unwrap();
        } else {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while restored.restoring() {
                restored.poll_background_io(&egui::Context::default());
                assert!(std::time::Instant::now() < deadline);
                std::thread::yield_now();
            }
        }
        let file = restored.active().unwrap();
        assert_eq!(file.draft_text.as_deref(), Some(text.as_str()));
        assert!(file.editor_state.large.active);
        assert!(file.is_modified());
        assert_eq!(file.editor_state.large.characters, text.chars().count());
    }
}

#[test]
fn restore_indexes_only_the_active_tab() {
    let dir = tempfile::tempdir().unwrap();
    let source = "中文 line\r\n".repeat(40_000);
    let first = dir.path().join("first.txt");
    let second = dir.path().join("second.txt");
    std::fs::write(&first, &source).unwrap();
    std::fs::write(&second, &source).unwrap();
    let mut original = app(dir.path());
    original.open_paths(vec![first, second]);
    original.active_id = Some(original.files[0].id);
    original.write_recovery().unwrap();

    let mut restored = app(dir.path());
    restored.start_restore(dir.path().join("session.json"), None);
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while restored.restoring() {
        restored.poll_background_io(&ctx);
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(restored.files.len(), 2);
    let active = restored.active_id.unwrap();
    for file in &restored.files {
        if file.id == active {
            assert!(
                file.editor_state.large.active,
                "the visible tab must be indexed before the first frame"
            );
            assert!(file.editor_state.changes.baseline.is_some());
        } else {
            assert!(
                !file.editor_state.large.active,
                "inactive tabs must not pay for their large-buffer index at startup"
            );
            assert!(
                file.editor_state.changes.baseline.is_none(),
                "inactive tabs must not build a line baseline at startup"
            );
        }
    }
}

#[test]
fn background_open_wakes_the_ui_before_the_fallback_poll() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wake.txt");
    std::fs::write(&path, "hello").unwrap();
    let ctx = egui::Context::default();
    let immediate = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = immediate.clone();
    ctx.set_request_repaint_callback(move |info| {
        if info.delay.is_zero() {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    });
    let mut app = app(dir.path());
    app.queue_open(vec![path]);
    app.poll_background_io(&ctx); // starts the worker; fallback poll is delayed 30 ms
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !immediate.load(std::sync::atomic::Ordering::SeqCst) {
        assert!(std::time::Instant::now() < deadline, "worker never woke the UI");
        std::thread::yield_now();
    }
    app.poll_background_io(&ctx);
    assert_eq!(app.files.len(), 1);
    assert_eq!(app.active().unwrap().draft_text.as_deref(), Some("hello"));
}

#[test]
fn background_open_prepares_indices_batches_failures_and_does_not_steal_focus() {
    let dir = tempfile::tempdir().unwrap();
    let good = dir.path().join("large.txt");
    let source = "中文\r\n".repeat(60_000);
    std::fs::write(&good, &source).unwrap();
    let mut app = app(dir.path());
    app.new_document();
    let ctx = egui::Context::default();
    app.queue_open(vec![
        good.clone(),
        good.clone(),
        dir.path().join("missing.txt"),
    ]);
    app.poll_background_io(&ctx);
    app.new_document();
    let selected = app.active_id;
    drain(&mut app, &ctx);
    assert_eq!(app.active_id, selected);
    assert_eq!(app.files.len(), 3);
    let file = app.files.last().unwrap();
    assert!(file.editor_state.large.active);
    assert_eq!(file.editor_state.large.characters, source.chars().count());
    assert_eq!(file.draft_text.as_deref(), Some(source.as_str()));
    assert!(!file.is_modified());
    assert!(file.editor_state.changes.baseline.is_some());
    assert_eq!(app.open_errors.len(), 1);
    assert_eq!(app.recent_files, [good.canonicalize().unwrap()]);
}

#[test]
fn cancel_loading_ignores_late_result_and_keeps_closed_tabs_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "hello").unwrap();
    let mut app = app(dir.path());
    let ctx = egui::Context::default();
    app.queue_open(vec![path]);
    app.poll_background_io(&ctx);
    app.cancel_open();
    drain(&mut app, &ctx);
    assert!(app.files.is_empty());
    assert!(app.open_errors.is_empty());
    assert!(app.recent_files.is_empty());
}

#[test]
fn staged_recovery_rejects_newer_edits_closed_tabs_and_sync_barriers() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app(dir.path());
    let ctx = egui::Context::default();
    app.new_document();
    app.active_mut().unwrap().draft_text = Some("old".into());
    app.active_mut().unwrap().sync_draft();
    app.start_background_recovery();
    app.active_mut().unwrap().draft_text = Some("new".into());
    app.active_mut().unwrap().sync_draft();
    drain(&mut app, &ctx);
    assert!(!dir.path().join("session.json").exists());
    app.start_background_recovery();
    drain(&mut app, &ctx);
    assert_eq!(
        recovery::load(&dir.path().join("session.json"))
            .unwrap()
            .unwrap()
            .restore()[0]
            .draft_text
            .as_deref(),
        Some("new")
    );
    assert_eq!(
        app.recovery_indicator(app.active().unwrap()),
        crate::RecoveryIndicator::BackedUp
    );
    app.active_mut().unwrap().draft_text = Some("late".into());
    app.active_mut().unwrap().sync_draft();
    app.start_background_recovery();
    app.close_tab(app.active_id.unwrap());
    app.write_recovery().unwrap();
    drain(&mut app, &ctx);
    assert!(
        !recovery::load(&dir.path().join("session.json"))
            .unwrap()
            .unwrap()
            .has_files()
    );
    assert!(app.recovery_versions.is_empty());
}

#[test]
fn staged_recovery_failures_never_report_current_buffer_as_backed_up() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app(dir.path());
    std::fs::write(dir.path().join("blocker"), "file").unwrap();
    app.recovery_path_override = Some(dir.path().join("blocker/session.json"));
    app.new_document();
    app.active_mut().unwrap().draft_text = Some("keep".into());
    app.active_mut().unwrap().sync_draft();
    app.start_background_recovery();
    drain(&mut app, &egui::Context::default());
    assert!(app.io_error.is_some());
    assert_eq!(
        app.recovery_indicator(app.active().unwrap()),
        crate::RecoveryIndicator::Pending
    );
}

#[test]
fn disk_check_results_cannot_apply_after_edit_save_or_detach() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.txt");
    std::fs::write(&source, "original").unwrap();
    let mut app = app(dir.path());
    app.open_paths(vec![source.clone()]);
    std::fs::write(&source, "external").unwrap();
    app.start_disk_checks(true);
    linkage::detach(app.active_mut().unwrap());
    drain(&mut app, &egui::Context::default());
    assert!(app.active().unwrap().disk.pending.is_none());
    assert_eq!(
        app.active().unwrap().draft_text.as_deref(),
        Some("original")
    );
    app.open_paths(vec![source.clone()]);
    std::fs::write(&source, "external two").unwrap();
    app.start_disk_checks(true);
    drain(&mut app, &egui::Context::default());
    assert_eq!(
        app.active().unwrap().disk.pending,
        Some(linkage::External::Changed)
    );
}

#[test]
fn background_snapshot_includes_unselected_open_tabs_and_preserves_invalid_record() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app(dir.path());
    app.new_document();
    app.active_mut().unwrap().draft_text = Some("unselected content".into());
    app.active_mut().unwrap().sync_draft();
    app.new_document();
    let path = dir.path().join("session.json");
    std::fs::write(&path, "invalid original").unwrap();
    app.start_background_recovery();
    drain(&mut app, &egui::Context::default());
    let restored = recovery::load(&path).unwrap().unwrap().restore();
    assert_eq!(restored.len(), 2);
    assert_eq!(
        restored[0].draft_text.as_deref(),
        Some("unselected content")
    );
    assert!(
        std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("session.invalid-")
                    && std::fs::read_to_string(entry.path())
                        .is_ok_and(|text| text == "invalid original")
            })
    );
}

#[test]
fn loading_progress_cancel_button_clears_pending_queue_without_touching_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app(dir.path());
    app.new_document();
    let id = app.active_id;
    let (sender, receiver) = mpsc::channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    app.background_io.opening = Some(Opening {
        path: dir.path().join("loading.txt"),
        cancel: cancelled.clone(),
        progress: Arc::new(AtomicU64::new(50)),
        total: Arc::new(AtomicU64::new(100)),
        receiver,
        active_at_start: id,
        phase: Arc::new(AtomicU8::new(0)),
        activate: true,
    });
    app.background_io
        .queue
        .push_back(dir.path().join("queued.txt"));
    app.background_io.queued_total = 2;
    let ctx = egui::Context::default();
    crate::setup_fonts(&ctx);
    for phase in [0, 1, 2] {
        app.background_io
            .opening
            .as_ref()
            .unwrap()
            .phase
            .store(phase, Ordering::Relaxed);
        let mut labels = Vec::new();
        for _ in 0..2 {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 600.0),
                    )),
                    ..Default::default()
                },
                |ctx| app.background_io_panel(ctx),
            );
            labels = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                    _ => None,
                })
                .collect();
        }
        let zh = app.lang == crate::Lang::ZhCn;
        let expected = match (phase, zh) {
            (0, true) => "读取中",
            (0, false) => "Reading",
            (1, true) => "正在解码",
            (1, false) => "Decoding",
            (_, true) => "正在建立索引",
            _ => "Indexing",
        };
        assert!(
            labels.iter().any(|label| label.contains(expected)),
            "{labels:?}"
        );
        assert!(labels.iter().any(|label| label.contains("1 / 2")));
        assert!(!cancelled.load(Ordering::Relaxed));
    }
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
            |ctx| app.background_io_panel(ctx),
        );
        if frame == 1 {
            position = output.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Text(text)
                    if text.galley.text()
                        == if app.lang == crate::Lang::ZhCn {
                            "取消全部"
                        } else {
                            "Cancel all"
                        } =>
                {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            });
        }
    }
    assert!(cancelled.load(Ordering::Relaxed));
    assert!(app.background_io.opening.is_none());
    assert!(app.background_io.queue.is_empty());
    assert_eq!(app.active_id, id);
    assert_eq!(app.files.len(), 1);
    drop(sender);
}

#[test]
fn background_restore_keeps_scratch_pin_state() {
    let dir = tempfile::tempdir().unwrap();
    let drafts = dir.path().join("drafts");
    let mut original = LiteTextApp {
        recovery_path_override: Some(dir.path().join("session.json")),
        scratch_dir: Some(drafts.clone()),
        ..Default::default()
    };
    original.new_document();
    original.active_mut().unwrap().draft_text = Some("pinned draft".into());
    original.active_mut().unwrap().sync_draft();
    original.scratch_flush(None, true);
    original.toggle_pin(original.active_id.unwrap());
    original.write_recovery().unwrap();

    let mut restored = LiteTextApp {
        recovery_path_override: Some(dir.path().join("session.json")),
        scratch_dir: Some(drafts.clone()),
        ..Default::default()
    };
    restored.start_restore(dir.path().join("session.json"), Some(drafts));
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while restored.restoring() {
        restored.poll_background_io(&ctx);
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(restored.files.len(), 1);
    assert!(
        restored.files[0].pinned,
        "the background restore path must re-apply the draft's pin flag"
    );
    assert!(restored.files[0].is_scratch_mirror());
}

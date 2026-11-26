use super::*;
use crate::{DARK, LIGHT, Lang, recovery};

fn draw(
    ctx: &egui::Context,
    app: &mut LiteTextApp,
    pal: &Palette,
    width: f32,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, 600.0),
            )),
            events,
            ..Default::default()
        },
        |ctx| app.top_bar(ctx, pal),
    );
    for shape in &output.shapes {
        if let egui::Shape::Text(text) = &shape.shape {
            let message = text.galley.text();
            assert!(
                !(message.starts_with("🔥 ") && message.contains(" ID ")),
                "tab strip emitted a GUI ID collision: {message}"
            );
        }
    }
    output
}

fn pointer(pos: egui::Pos2, button: egui::PointerButton, pressed: bool) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(pos),
        egui::Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: Default::default(),
        },
    ]
}

fn geometry(ctx: &egui::Context, name: &str) -> (egui::Rect, f32) {
    ctx.data(|data| data.get_temp(egui::Id::new(name))).unwrap()
}

#[test]
fn pin_and_unpin_menu_moves_only_target_preserves_selection_and_buffer() {
    for pal in [&DARK, &LIGHT] {
        for lang in [Lang::ZhCn, Lang::En] {
            let dir = tempfile::tempdir().unwrap();
            let mut app = LiteTextApp {
                vertical_tabs: false,
                lang,
                recovery_path_override: Some(dir.path().join("session.json")),
                ..Default::default()
            };
            app.new_document();
            let target = app.active_id.unwrap();
            app.active_mut().unwrap().draft_text = Some("keep this text".into());
            app.active_mut().unwrap().sync_draft();
            let revision = app.active().unwrap().content_revision;
            app.new_document();
            let selected = app.active_id;
            let ctx = egui::Context::default();
            crate::setup_fonts(&ctx);
            for pinned in [false, true] {
                for _ in 0..3 {
                    draw(&ctx, &mut app, pal, 800.0, vec![]);
                }
                let rect = ctx
                    .data(|data| {
                        data.get_temp::<egui::Rect>(egui::Id::new((
                            if pinned {
                                "pinned_tab_rect"
                            } else {
                                "normal_tab_rect"
                            },
                            target,
                        )))
                    })
                    .unwrap();
                for pressed in [true, false] {
                    draw(
                        &ctx,
                        &mut app,
                        pal,
                        800.0,
                        pointer(rect.center(), egui::PointerButton::Secondary, pressed),
                    );
                }
                let mut menu_output = draw(&ctx, &mut app, pal, 800.0, vec![]);
                for _ in 0..2 {
                    menu_output = draw(&ctx, &mut app, pal, 800.0, vec![]);
                }
                let labels: Vec<_> = menu_output
                    .shapes
                    .iter()
                    .filter_map(|s| match &s.shape {
                        egui::Shape::Text(text) => Some(text.galley.text()),
                        _ => None,
                    })
                    .collect();
                let t = tr(lang);
                for label in [t.close, t.close_others, t.close_left, t.close_right] {
                    assert_eq!(labels.contains(&label), !pinned, "{label}: {labels:?}");
                }
                let choice = ctx
                    .data(|data| {
                        data.get_temp::<egui::Rect>(egui::Id::new(("pin_tab_rect", target)))
                    })
                    .unwrap();
                for pressed in [true, false] {
                    draw(
                        &ctx,
                        &mut app,
                        pal,
                        800.0,
                        pointer(choice.center(), egui::PointerButton::Primary, pressed),
                    );
                }
                draw(&ctx, &mut app, pal, 800.0, vec![]);
                let file = app.files.iter().find(|file| file.id == target).unwrap();
                assert_eq!(file.pinned, !pinned);
                assert_eq!(file.draft_text.as_deref(), Some("keep this text"));
                assert_eq!(file.content_revision, revision);
                assert_eq!(app.active_id, selected);
                assert_eq!(
                    recovery::load(&dir.path().join("session.json"))
                        .unwrap()
                        .unwrap()
                        .restore()
                        .iter()
                        .find(|file| file.id == target)
                        .unwrap()
                        .pinned,
                    !pinned
                );
            }
            assert_eq!(app.files.last().unwrap().id, target);
        }
    }
}

#[test]
fn repeated_menu_unpin_uses_one_tab_partition_per_pass_including_last_pinned_tab() {
    for pal in [&DARK, &LIGHT] {
        for lang in [Lang::ZhCn, Lang::En] {
            let dir = tempfile::tempdir().unwrap();
            let ctx = egui::Context::default();
            crate::setup_fonts(&ctx);
            let mut app = LiteTextApp {
                vertical_tabs: false,
                lang,
                recovery_path_override: Some(dir.path().join("session.json")),
                ..Default::default()
            };
            for _ in 0..7 {
                app.new_document();
            }
            let ids: Vec<_> = app.files.iter().map(|f| f.id).collect();
            let active = app.active_id;
            for &id in &ids {
                app.toggle_pin(id);
            }
            for _ in 0..4 {
                draw(&ctx, &mut app, pal, 800.0, vec![]);
            }
            for index in [4, 3, 6, 5, 2, 1, 0] {
                let id = ids[index];
                app.reveal_tab = Some(id);
                for _ in 0..20 {
                    draw(&ctx, &mut app, pal, 800.0, vec![]);
                }
                let tab = ctx
                    .data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("pinned_tab_rect", id))))
                    .unwrap();
                let viewport = geometry(&ctx, "pinned_tabs_viewport").0;
                assert!(
                    viewport.contains(tab.center()),
                    "index={index}; tab={tab:?}; viewport={viewport:?}; offset={}",
                    geometry(&ctx, "pinned_tabs_viewport").1
                );
                for pressed in [true, false] {
                    draw(
                        &ctx,
                        &mut app,
                        pal,
                        800.0,
                        pointer(tab.center(), egui::PointerButton::Secondary, pressed),
                    );
                }
                for _ in 0..2 {
                    draw(&ctx, &mut app, pal, 800.0, vec![]);
                }
                let choice = ctx
                    .data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("pin_tab_rect", id))))
                    .unwrap();
                for pressed in [true, false] {
                    draw(
                        &ctx,
                        &mut app,
                        pal,
                        800.0,
                        pointer(choice.center(), egui::PointerButton::Primary, pressed),
                    );
                }
                for _ in 0..20 {
                    draw(&ctx, &mut app, pal, 800.0, vec![]);
                }
                assert!(!app.files.iter().find(|f| f.id == id).unwrap().pinned);
                assert_eq!(app.files.len(), 7);
                assert_eq!(app.active_id, active);
            }
            assert!(app.files.iter().all(|f| !f.pinned));
            assert!(
                recovery::load(&dir.path().join("session.json"))
                    .unwrap()
                    .unwrap()
                    .restore()
                    .iter()
                    .all(|f| !f.pinned)
            );
        }
    }
}

#[test]
fn pinned_region_is_compact_bounded_and_scrolls_independently_from_normal_tabs() {
    for width in [400.0, 800.0, 1440.0] {
        let ctx = egui::Context::default();
        let mut app = LiteTextApp { vertical_tabs: false, ..Default::default() };
        for index in 0..30 {
            app.new_document();
            if index < 12 {
                app.toggle_pin(app.active_id.unwrap());
            }
        }
        app.reveal_tab = None;
        for _ in 0..4 {
            draw(&ctx, &mut app, &DARK, width, vec![]);
        }
        let (pinned, _) = geometry(&ctx, "pinned_tabs_viewport");
        let (normal, _) = geometry(&ctx, "normal_tabs_viewport");
        let divider = ctx
            .data(|data| data.get_temp::<egui::Rect>(egui::Id::new("pinned_tabs_divider")))
            .unwrap();
        assert!(pinned.width() <= 240.0 && pinned.width() >= WIDTH);
        assert!(pinned.right() <= divider.left() && divider.right() < normal.left());
        assert!(normal.width() >= 40.0, "{width}: {normal:?}");
        let pinned_id = app.files[0].id;
        let rect = ctx
            .data(|data| data.get_temp::<egui::Rect>(egui::Id::new(("pinned_tab_rect", pinned_id))))
            .unwrap();
        assert_eq!(rect.width(), WIDTH);
        let scroll = |app: &mut LiteTextApp, pos| {
            draw(
                &ctx,
                app,
                &DARK,
                width,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, -100.0),
                        modifiers: Default::default(),
                    },
                ],
            );
            for _ in 0..20 {
                draw(&ctx, app, &DARK, width, vec![]);
            }
        };
        scroll(&mut app, pinned.center());
        let pinned_offset = geometry(&ctx, "pinned_tabs_viewport").1;
        assert!(pinned_offset > 0.0);
        assert_eq!(geometry(&ctx, "normal_tabs_viewport").1, 0.0);
        scroll(&mut app, normal.center());
        assert!(geometry(&ctx, "normal_tabs_viewport").1 > 0.0);
        assert!((geometry(&ctx, "pinned_tabs_viewport").1 - pinned_offset).abs() < 0.1);
    }
}

#[test]
fn singleton_pinned_region_has_no_extra_minimum_width_and_title_is_centered() {
    for width in [400.0, 800.0, 1440.0] {
        let ctx = egui::Context::default();
        let mut app = LiteTextApp { vertical_tabs: false, ..Default::default() };
        app.new_document();
        let id = app.active_id.unwrap();
        for _ in 0..3 {
            draw(&ctx, &mut app, &DARK, width, vec![]);
        }
        assert_eq!(geometry(&ctx, "normal_tabs_viewport").0.left(), 14.0);
        app.toggle_pin(id);
        app.new_document();
        for _ in 0..4 {
            draw(&ctx, &mut app, &DARK, width, vec![]);
        }
        let output = draw(&ctx, &mut app, &DARK, width, vec![]);
        let (viewport, offset) = geometry(&ctx, "pinned_tabs_viewport");
        let rect = ctx
            .data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("pinned_tab_rect", id))))
            .unwrap();
        assert_eq!(viewport.width(), WIDTH);
        assert_eq!(viewport.left(), 4.0);
        assert_eq!(rect.left(), viewport.left());
        assert_eq!(offset, 0.0);
        assert!((viewport.right() - rect.right()).abs() < 0.1);
        let title = output
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(text) if rect.contains(text.pos) => Some(text),
                _ => None,
            })
            .unwrap();
        assert!((title.pos.x + title.galley.size().x / 2.0 - rect.center().x).abs() < 1.0);
        assert!(!output.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::LineSegment { points, .. } if points.iter().all(|p| rect.shrink(2.0).contains(*p)))));
    }
}

#[test]
fn pin_state_survives_both_recovery_paths_and_old_records_default_to_unpinned() {
    let dir = tempfile::tempdir().unwrap();
    let snapshot = dir.path().join("session.json");
    let mut app = LiteTextApp {
        recovery_path_override: Some(snapshot.clone()),
        ..Default::default()
    };
    for _ in 0..3 {
        app.new_document();
    }
    let id = app.files[1].id;
    app.toggle_pin(id);
    let mut capture = recovery::Capture::new(&app.files, app.active_id);
    while !capture.step(&app.files, 1024) {}
    assert!(capture.finish().restore()[0].pinned);
    app.write_recovery().unwrap();
    assert_eq!(
        recovery::load(&snapshot).unwrap().unwrap().restore()[0].id,
        id
    );
    let mut old: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&snapshot).unwrap()).unwrap();
    for file in old["files"].as_array_mut().unwrap() {
        file.as_object_mut().unwrap().remove("pinned");
    }
    std::fs::write(&snapshot, serde_json::to_vec(&old).unwrap()).unwrap();
    assert!(
        recovery::load(&snapshot)
            .unwrap()
            .unwrap()
            .restore()
            .iter()
            .all(|file| !file.pinned)
    );
    app.toggle_pin(id);
    assert!(!app.files.last().unwrap().pinned);
    assert_eq!(app.files.last().unwrap().id, id);
}

#[test]
fn normal_reordering_keeps_pinned_order_and_close_requires_unpinning_first() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = LiteTextApp {
        recovery_path_override: Some(dir.path().join("session.json")),
        ..Default::default()
    };
    for _ in 0..5 {
        app.new_document();
    }
    let ids: Vec<_> = app.files.iter().map(|file| file.id).collect();
    app.toggle_pin(ids[2]);
    app.toggle_pin(ids[0]);
    app.reorder_normal_tabs(&[ids[4], ids[1], ids[3]]);
    assert_eq!(
        app.files.iter().map(|file| file.id).collect::<Vec<_>>(),
        [ids[2], ids[0], ids[4], ids[1], ids[3]]
    );
    app.active_id = Some(ids[2]);
    app.active_mut().unwrap().draft_text = Some("unsaved pinned".into());
    app.active_mut().unwrap().sync_draft();
    app.request_tab_action(TabAction::Close(ids[2]));
    assert!(app.pending_close.is_none());
    assert_eq!(app.files.len(), 5);
    app.close_tab(ids[2]);
    assert_eq!(app.files.len(), 5);
    app.toggle_pin(ids[2]);
    app.request_tab_action(TabAction::Close(ids[2]));
    assert!(app.pending_close.is_some());
    assert_eq!(app.files.len(), 5);
    app.pending_close = None;
    app.active_mut().unwrap().draft_text = Some(String::new());
    app.active_mut().unwrap().sync_draft();
    app.request_tab_action(TabAction::Close(ids[2]));
    assert_eq!(app.files.len(), 4);
    assert!(app.files[0].pinned);
    assert_eq!(app.files[0].id, ids[0]);
}

#[test]
fn pinned_tabs_ignore_double_click_shortcut_and_bulk_close_until_unpinned() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = LiteTextApp {
        vertical_tabs: false,
        recovery_path_override: Some(dir.path().join("session.json")),
        ..Default::default()
    };
    for _ in 0..4 {
        app.new_document();
    }
    let ids: Vec<_> = app.files.iter().map(|file| file.id).collect();
    app.toggle_pin(ids[0]);
    app.active_id = Some(ids[0]);
    let ctx = egui::Context::default();
    for _ in 0..3 {
        draw(&ctx, &mut app, &DARK, 800.0, vec![]);
    }
    let rect = ctx
        .data(|data| data.get_temp::<egui::Rect>(egui::Id::new(("pinned_tab_rect", ids[0]))))
        .unwrap();
    for _ in 0..2 {
        for pressed in [true, false] {
            draw(
                &ctx,
                &mut app,
                &DARK,
                800.0,
                pointer(rect.center(), egui::PointerButton::Primary, pressed),
            );
        }
    }
    assert_eq!(app.files.len(), 4);
    assert!(app.pending_close.is_none());
    assert_eq!(app.active_id, Some(ids[0]));
    // Keyboard close uses the same protected action path.
    let _ = ctx.run(
        egui::RawInput {
            modifiers: egui::Modifiers::COMMAND,
            events: vec![egui::Event::Key {
                key: egui::Key::W,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            }],
            ..Default::default()
        },
        |ctx| {
            app.handle_shortcuts(ctx);
        },
    );
    assert_eq!(app.files.len(), 4);
    assert_eq!(app.closing_ids(&TabAction::CloseLeft(ids[2])), vec![ids[1]]);
    app.request_tab_action(TabAction::CloseLeft(ids[2]));
    assert_eq!(
        app.files.iter().map(|file| file.id).collect::<Vec<_>>(),
        [ids[0], ids[2], ids[3]]
    );
    app.request_tab_action(TabAction::CloseOthers(ids[3]));
    assert_eq!(
        app.files.iter().map(|file| file.id).collect::<Vec<_>>(),
        [ids[0], ids[3]]
    );
    // After unpinning, the normal strip's double click closes the empty tab.
    app.toggle_pin(ids[0]);
    app.reveal_tab = None;
    // Begin a fresh click sequence, outside the earlier double/triple-click window.
    let _ = ctx.run(
        egui::RawInput {
            time: Some(10.0),
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            ..Default::default()
        },
        |ctx| app.top_bar(ctx, &DARK),
    );
    for _ in 0..3 {
        draw(&ctx, &mut app, &DARK, 800.0, vec![]);
    }
    let rect = ctx
        .data(|data| data.get_temp::<egui::Rect>(egui::Id::new(("normal_tab_rect", ids[0]))))
        .unwrap();
    for _ in 0..2 {
        for pressed in [true, false] {
            draw(
                &ctx,
                &mut app,
                &DARK,
                800.0,
                pointer(rect.center(), egui::PointerButton::Primary, pressed),
            );
        }
    }
    assert!(!app.files.iter().any(|file| file.id == ids[0]));
}

#[test]
fn pinned_state_survives_restart_for_real_files_and_scratch_drafts() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("notes.md");
    std::fs::write(&real, "# hi\n").unwrap();
    let snapshot = dir.path().join("recovery/session.json");
    let drafts = dir.path().join("drafts");
    let mut app = LiteTextApp {
        recovery_path_override: Some(snapshot.clone()),
        scratch_dir: Some(drafts.clone()),
        ..Default::default()
    };
    app.open_paths_now(vec![real.clone()]);
    let real_id = app.active_id.unwrap();
    app.toggle_pin(real_id);

    app.new_document();
    let draft_id = app.active_id.unwrap();
    app.active_mut().unwrap().draft_text = Some("pinned draft".into());
    app.active_mut().unwrap().sync_draft();
    app.scratch_flush(None, true);
    app.toggle_pin(draft_id);
    app.write_recovery().unwrap();
    drop(app); // A restart releases the previous window's mirror leases.

    let mut restored = LiteTextApp {
        recovery_path_override: Some(snapshot),
        scratch_dir: Some(drafts),
        ..Default::default()
    };
    restored.restore_session();
    assert_eq!(restored.files.len(), 2, "one real file and one draft");
    assert!(
        restored
            .files
            .iter()
            .find(|file| file.path == crate::linkage::resolved(&real))
            .unwrap()
            .pinned,
        "a pinned real file must reopen pinned"
    );
    assert!(
        restored
            .files
            .iter()
            .find(|file| file.is_scratch_mirror())
            .unwrap()
            .pinned,
        "a pinned untitled draft must reopen pinned"
    );
}

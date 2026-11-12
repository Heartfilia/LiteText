use super::*;

#[test]
fn workspace_zoom_accepts_plus_equals_is_bounded_persistent_and_configurable() {
    let ctx = egui::Context::default();
    let mut app = LiteTextApp::default();
    for key in [Key::Plus, Key::Equals] {
        for shift in [false, true] {
            let before = app.workspace_font_size;
            let mut modifiers = Modifiers::COMMAND;
            modifiers.shift = shift;
            let _ = ctx.run(
                input(vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                }]),
                |ctx| {
                    ctx.options_mut(|o| o.zoom_with_keyboard = false);
                    app.handle_shortcuts(ctx);
                },
            );
            assert_eq!(app.workspace_font_size, before + 1.0);
        }
    }
    for _ in 0..100 {
        app.zoom_workspace(1.0, &ctx);
    }
    assert_eq!(app.workspace_font_size, 32.0);
    for _ in 0..100 {
        app.zoom_workspace(-1.0, &ctx);
    }
    assert_eq!(app.workspace_font_size, 9.0);
    assert_eq!(ctx.zoom_factor(), 1.0);
    let restored: LiteTextApp =
        serde_json::from_str(&serde_json::to_string(&app).unwrap()).unwrap();
    assert_eq!(restored.workspace_font_size, 9.0);
    assert_eq!(
        serde_json::from_str::<LiteTextApp>("{}")
            .unwrap()
            .workspace_font_size,
        13.5
    );
    let custom = KeyboardShortcut::new(Modifiers::ALT, Key::F8);
    app.shortcut_keys.set(Action::ZoomIn, custom).unwrap();
    let _ = ctx.run(input(vec![key(Action::ZoomIn)]), |ctx| {
        app.handle_shortcuts(ctx);
    });
    assert_eq!(app.workspace_font_size, 9.0);
    let _ = ctx.run(
        input(vec![egui::Event::Key {
            key: Key::F8,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::ALT,
        }]),
        |ctx| {
            app.handle_shortcuts(ctx);
        },
    );
    assert_eq!(app.workspace_font_size, 10.0);
}

#[test]
fn bindings_highlight_clear_to_clickable_blank_and_can_be_reassigned() {
    for delete in [Key::Delete, Key::Backspace] {
        let ctx = egui::Context::default();
        ctx.style_mut(|style| style.animation_time = 0.0);
        let mut app = LiteTextApp {
            show_settings: true,
            shortcuts_page: true,
            ..Default::default()
        };
        let run = |app: &mut LiteTextApp, events| {
            ctx.run(input(events), |ctx| app.settings_window(ctx, &crate::DARK))
        };
        let click = |app: &mut LiteTextApp, rect: egui::Rect| {
            for pressed in [true, false] {
                run(
                    app,
                    vec![
                        egui::Event::PointerMoved(rect.center()),
                        egui::Event::PointerButton {
                            pos: rect.center(),
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: Modifiers::NONE,
                        },
                    ],
                );
            }
        };
        let binding_rect = || {
            ctx.data(|d| {
                d.get_temp::<egui::Rect>(egui::Id::new((
                    "shortcut_binding_rect",
                    Action::Save as usize,
                )))
            })
            .unwrap()
        };
        run(&mut app, vec![]);
        run(&mut app, vec![]);
        click(&mut app, binding_rect());
        assert_eq!(app.shortcut_recording, Some(Action::Save));
        let output = run(&mut app, vec![]);
        let rect = binding_rect();
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Rect(r) if r.rect == rect.expand(2.0) && r.stroke.color == crate::DARK.accent
        )));
        run(
            &mut app,
            vec![egui::Event::Key {
                key: delete,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
        );
        assert!(app.shortcut_keys.binding(Action::Save).is_none());
        assert!(app.shortcut_recording.is_none());
        let output = run(&mut app, vec![]);
        let blank = binding_rect();
        assert!(blank.width() >= 90.0);
        assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Text(text) if blank.contains(text.pos)
        )));
        let restored: LiteTextApp =
            serde_json::from_str(&serde_json::to_string(&app).unwrap()).unwrap();
        assert!(restored.shortcut_keys.binding(Action::Save).is_none());
        app.show_settings = false;
        let mut saved = false;
        let _ = ctx.run(input(vec![key(Action::Save)]), |ctx| {
            saved = app.handle_shortcuts(ctx);
        });
        assert!(!saved);
        app.show_settings = true;
        run(&mut app, vec![]);
        run(&mut app, vec![]);
        click(&mut app, binding_rect());
        assert_eq!(app.shortcut_recording, Some(Action::Save));
        run(&mut app, vec![key(Action::Open)]);
        assert!(app.shortcut_error.is_some());
        assert_eq!(app.shortcut_recording, Some(Action::Save));
        assert!(app.shortcut_keys.binding(Action::Save).is_none());
        let new_key = egui::Event::Key {
            key: Key::F8,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        run(&mut app, vec![new_key.clone()]);
        assert_eq!(
            app.shortcut_keys.binding(Action::Save),
            Some(KeyboardShortcut::new(Modifiers::NONE, Key::F8))
        );
        assert!(app.shortcut_recording.is_none() && app.shortcut_error.is_none());
        app.show_settings = false;
        let _ = ctx.run(input(vec![new_key]), |ctx| {
            saved = app.handle_shortcuts(ctx);
        });
        assert!(saved);
    }
}

#[test]
fn unbound_native_keys_stay_disabled_and_legacy_configuration_still_loads() {
    let mut map = Keymap::default();
    map.clear(Action::Copy);
    map.set(Action::Save, Action::Copy.binding()).unwrap(); // Cleared keys are available to other actions.
    let json = r#"{"overrides":{"Save":{"modifiers":{"alt":false,"ctrl":false,"shift":true,"mac_cmd":false,"command":true},"logical_key":"S"}}}"#;
    let legacy: Keymap = serde_json::from_str(json).unwrap();
    assert_eq!(
        legacy.binding(Action::Save),
        Some(KeyboardShortcut::new(
            Modifiers::COMMAND | Modifiers::SHIFT,
            Key::S
        ))
    );
    let ctx = egui::Context::default();
    let mut app = LiteTextApp::default();
    app.shortcut_keys.clear(Action::Copy);
    let _ = ctx.run(input(vec![key(Action::Copy)]), |ctx| {
        app.handle_shortcuts(ctx);
        assert!(!ctx.input(|i| i.events.contains(&egui::Event::Copy)));
    });
    app.shortcut_keys = Keymap::default();
    assert_eq!(
        app.shortcut_keys.binding(Action::Copy),
        Some(Action::Copy.binding())
    );
}

#[test]
fn keycaps_use_keyboard_symbols_but_keep_enter_and_delete_names() {
    for mac in [true, false] {
        for (key, legend) in [
            (Key::Comma, ","),
            (Key::Questionmark, "?"),
            (Key::Slash, "/"),
            (Key::Backslash, "\\"),
            (Key::Quote, "'"),
            (Key::Semicolon, ";"),
            (Key::OpenBracket, "["),
            (Key::CloseBracket, "]"),
            (Key::OpenCurlyBracket, "{"),
            (Key::CloseCurlyBracket, "}"),
            (Key::Backtick, "`"),
            (Key::Minus, "-"),
            (Key::Equals, "="),
            (Key::ArrowLeft, "←"),
            (Key::ArrowRight, "→"),
            (Key::ArrowUp, "↑"),
            (Key::ArrowDown, "↓"),
            (Key::Tab, "⇥"),
            (Key::Space, "␣"),
            (Key::Escape, "⎋"),
            (Key::Enter, "Enter"),
            (Key::Delete, "Delete"),
            (Key::F3, "F3"),
            (Key::A, "A"),
        ] {
            assert_eq!(key_legend(key, mac), legend);
        }
        let ctx = egui::Context::default();
        let output = ctx.run(input(vec![]), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                for action in [
                    Action::Settings,
                    Action::Comment,
                    Action::NextTab,
                    Action::Replace,
                ] {
                    shortcut_row(
                        ui,
                        action,
                        Some(action.binding_for(mac)),
                        false,
                        Lang::ZhCn,
                        &crate::DARK,
                    );
                }
            });
        });
        let labels: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(text) => Some(text.galley.text()),
                _ => None,
            })
            .collect();
        for symbol in [",", "/", "⇥", "Enter"] {
            assert!(labels.contains(&symbol));
        }
        for old in ["Comma", "Questionmark", "Tab"] {
            assert!(!labels.contains(&old));
        }
    }
    assert_eq!(key_legend(Key::Backspace, true), "Delete");
}

fn input(mut events: Vec<egui::Event>) -> egui::RawInput {
    let releases: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            egui::Event::Key {
                key,
                modifiers,
                pressed: true,
                ..
            } => Some(egui::Event::Key {
                key: *key,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers: *modifiers,
            }),
            _ => None,
        })
        .collect();
    events.extend(releases);
    egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1080.0, 720.0),
        )),
        events,
        ..Default::default()
    }
}

fn key(action: Action) -> egui::Event {
    let binding = action.binding();
    egui::Event::Key {
        key: binding.logical_key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: binding.modifiers,
    }
}

#[test]
fn default_keymap_is_unique_localized_and_uses_exact_platform_modifiers() {
    for (index, action) in ACTIONS.iter().enumerate() {
        assert!(
            ACTIONS[..index]
                .iter()
                .all(|other| other.binding() != action.binding())
        );
        for lang in Lang::ALL {
            assert!(!action.label(lang).is_empty());
        }
    }
    for modifiers in [
        Modifiers::CTRL | Modifiers::COMMAND,
        Modifiers::MAC_CMD | Modifiers::COMMAND,
    ] {
        let ctx = egui::Context::default();
        let mut event = key(Action::Save);
        if let egui::Event::Key { modifiers: m, .. } = &mut event {
            *m = modifiers;
        }
        let _ = ctx.run(input(vec![event]), |ctx| {
            assert!(take(ctx, Action::Save));
            assert!(!take(ctx, Action::Save));
        });
    }
    let ctx = egui::Context::default();
    let _ = ctx.run(
        input(vec![
            key(Action::PreviousMatch),
            key(Action::Count),
            key(Action::FocusReplace),
        ]),
        |ctx| {
            assert!(!take(ctx, Action::NextMatch));
            assert!(!take(ctx, Action::Copy));
            assert!(!take(ctx, Action::Find));
            assert!(take(ctx, Action::PreviousMatch));
            assert!(take(ctx, Action::Count));
            assert!(take(ctx, Action::FocusReplace));
        },
    );
    let held = || egui::RawInput {
        events: vec![key(Action::HexCase)],
        ..Default::default()
    };
    let _ = ctx.run(held(), |ctx| {
        assert!(take(ctx, Action::HexCase));
    });
    let _ = ctx.run(held(), |ctx| {
        assert!(!take(ctx, Action::HexCase));
        assert!(ctx.input(|i| i.events.is_empty()));
    });
}

#[test]
fn shortcuts_toggle_tools_per_tab_and_respect_view_and_dialogs() {
    let ctx = egui::Context::default();
    let mut app = LiteTextApp::default();
    app.new_document();
    let first = app.active_id;
    app.new_document();
    let second = app.active_id;
    let press = |app: &mut LiteTextApp, action| {
        let _ = ctx.run(input(vec![key(action)]), |ctx| {
            app.handle_shortcuts(ctx);
        });
    };
    app.active_mut().unwrap().editor_state.tools.wrap = true;
    press(&mut app, Action::PreviousTab);
    assert_eq!(app.active_id, first);
    assert!(!app.active().unwrap().editor_state.tools.wrap);
    press(&mut app, Action::NextTab);
    assert_eq!(app.active_id, second);
    app.active_mut().unwrap().view = ViewMode::Hex;
    assert!(app.active().unwrap().view == ViewMode::Hex);
    press(&mut app, Action::DimZeros);
    assert!(app.dim_zeros);
    app.active_mut().unwrap().view = ViewMode::Text;
    press(&mut app, Action::DimZeros);
    assert!(app.dim_zeros);
    press(&mut app, Action::Settings);
    assert!(app.show_settings);
    press(&mut app, Action::New);
    assert_eq!(app.files.len(), 2);
    assert!(app.active().unwrap().editor_state.tools.wrap);
    assert_eq!(app.active().unwrap().draft_text.as_deref(), Some(""));
    app.show_settings = false;
    crate::text_tools::replace_range(app.active_mut().unwrap(), &ctx, 0..0, "unsaved".to_owned());
    press(&mut app, Action::Close);
    assert!(app.pending_close.is_some());
    assert_eq!(app.files.len(), 2);
    press(&mut app, Action::New);
    assert_eq!(app.files.len(), 2);
}

#[test]
fn shortcut_search_is_on_demand_and_replacement_uses_current_result_in_both_editors() {
    for large in [false, true] {
        let ctx = egui::Context::default();
        let mut app = LiteTextApp::default();
        app.new_document();
        let file = app.active_mut().unwrap();
        let original = format!(
            "hit middle hit\n{}",
            "unchanged\n".repeat(if large { 30_000 } else { 0 })
        );
        file.draft_text = Some(original.clone());
        file.sync_draft();
        file.editor_state.tools.query = "hit".to_owned();
        file.editor_state.tools.replacement = "NEW".to_owned();
        let run = |app: &mut LiteTextApp, events| {
            ctx.run(input(events), |ctx| {
                app.handle_shortcuts(ctx);
                egui::CentralPanel::default().show(ctx, |ui| {
                    crate::draft_editor(
                        ui,
                        app.active_mut().unwrap(),
                        &crate::DARK,
                        false,
                        Lang::En,
                    );
                });
            })
        };
        run(&mut app, vec![key(Action::Find)]);
        run(&mut app, vec![]);
        let file = app.active_mut().unwrap();
        assert!(file.editor_state.tools.current.is_none());
        assert!(ctx.memory(|m| m.has_focus(egui::Id::new(("find_query", file.id)))));
        file.editor_state.pending_selection = Some(4..4);
        run(&mut app, vec![]);
        run(&mut app, vec![key(Action::NextMatch)]);
        assert_eq!(
            app.active().unwrap().editor_state.tools.current,
            Some(11..14)
        );
        run(&mut app, vec![key(Action::Replace)]);
        assert!(
            app.active()
                .unwrap()
                .draft_text
                .as_deref()
                .unwrap()
                .starts_with("hit middle NEW\n")
        );
        let after = app.active().unwrap().draft_text.clone();
        run(&mut app, vec![key(Action::Replace)]);
        assert_eq!(app.active().unwrap().draft_text, after);
        run(&mut app, vec![key(Action::Count)]);
        assert!(app.active().unwrap().editor_state.tools.current.is_none());
        run(&mut app, vec![key(Action::ReplaceAll)]);
        assert!(
            app.active()
                .unwrap()
                .draft_text
                .as_deref()
                .unwrap()
                .starts_with("NEW middle NEW\n")
        );
        run(&mut app, vec![key(Action::FocusReplace)]);
        let id = app.active_id.unwrap();
        assert!(ctx.memory(|m| m.has_focus(egui::Id::new(("find_replacement", id)))));
    }
}

#[test]
fn settings_shortcuts_entry_opens_secondary_page_and_back_preserves_settings() {
    for lang in Lang::ALL {
        let ctx = egui::Context::default();
        ctx.style_mut(|style| style.animation_time = 0.0);
        let mut app = LiteTextApp {
            lang,
            show_settings: true,
            ..Default::default()
        };
        let run = |app: &mut LiteTextApp, events| {
            ctx.run(input(events), |ctx| app.settings_window(ctx, &crate::DARK))
        };
        let click = |app: &mut LiteTextApp, rect: egui::Rect| {
            for pressed in [true, false] {
                run(
                    app,
                    vec![
                        egui::Event::PointerMoved(rect.center()),
                        egui::Event::PointerButton {
                            pos: rect.center(),
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: Modifiers::NONE,
                        },
                    ],
                );
            }
            run(app, vec![]);
        };
        run(&mut app, vec![]);
        run(&mut app, vec![]);
        let entry = ctx
            .data(|d| d.get_temp::<egui::Rect>(egui::Id::new("settings_shortcuts_rect")))
            .unwrap();
        click(&mut app, entry);
        assert!(app.shortcuts_page && app.show_settings);
        let output = run(&mut app, vec![]);
        assert!(output.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text() == tr(lang).shortcuts_desc)));
        let back = ctx
            .data(|d| d.get_temp::<egui::Rect>(egui::Id::new("shortcuts_back_rect")))
            .unwrap();
        click(&mut app, back);
        assert!(!app.shortcuts_page && app.show_settings);
        assert_eq!(app.lang, lang);
    }
}

#[test]
fn shortcuts_page_fits_all_groups_and_separates_labels_and_keycaps() {
    for lang in Lang::ALL {
        for size in [
            egui::vec2(1080.0, 720.0),
            egui::vec2(1440.0, 900.0),
            egui::vec2(640.0, 420.0),
        ] {
            let ctx = egui::Context::default();
            ctx.style_mut(|style| style.animation_time = 0.0);
            let mut app = LiteTextApp {
                lang,
                show_settings: true,
                shortcuts_page: true,
                ..Default::default()
            };
            for _ in 0..3 {
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                        ..Default::default()
                    },
                    |ctx| app.settings_window(ctx, &crate::DARK),
                );
            }
            let (viewport, content) = ctx
                .data(|d| {
                    d.get_temp::<(egui::Rect, egui::Vec2)>(egui::Id::new(
                        "shortcut_scroll_geometry",
                    ))
                })
                .unwrap();
            if size.x >= 1080.0 {
                assert!(
                    content.y <= viewport.height() + 1.0,
                    "lang={lang:?}, size={size:?}, content={content:?}, viewport={viewport:?}"
                );
                for group in 0..4usize {
                    let card = ctx
                        .data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("shortcut_card", group))))
                        .unwrap();
                    assert!(
                        viewport.expand(1.0).contains_rect(card),
                        "clipped group: {group}, {card:?}, viewport={viewport:?}"
                    );
                }
            } else {
                assert!(content.y > viewport.height());
            }
            for &action in ACTIONS {
                let (row, label, keys) = ctx
                    .data(|d| {
                        d.get_temp::<(egui::Rect, egui::Rect, f32)>(egui::Id::new((
                            "shortcut_row",
                            action as usize,
                        )))
                    })
                    .unwrap();
                assert!(
                    row.right() - keys - label.right() >= 13.0,
                    "label/key collision: {action:?}"
                );
                let mut previous = None;
                let mut count = 0;
                for index in 0..4usize {
                    if let Some(rect) = ctx.data(|d| {
                        d.get_temp::<egui::Rect>(egui::Id::new((
                            "shortcut_keycap",
                            action as usize,
                            index,
                        )))
                    }) {
                        if let Some(right) = previous {
                            assert!(rect.left() - right >= 5.0);
                        }
                        assert!(row.contains_rect(rect));
                        previous = Some(rect.right());
                        count += 1;
                    }
                }
                assert!(count > 0);
            }
        }
    }
}

#[test]
fn custom_bindings_record_conflicts_cancel_and_survive_serialization() {
    let ctx = egui::Context::default();
    let mut app = LiteTextApp {
        show_settings: true,
        shortcuts_page: true,
        ..Default::default()
    };
    let run = |app: &mut LiteTextApp, events| {
        let _ = ctx.run(input(events), |ctx| app.settings_window(ctx, &crate::DARK));
    };
    run(&mut app, vec![]);
    run(&mut app, vec![]);
    let cap = ctx
        .data(|d| {
            d.get_temp::<egui::Rect>(egui::Id::new((
                "shortcut_keycap",
                Action::Save as usize,
                0usize,
            )))
        })
        .unwrap();
    for pressed in [true, false] {
        run(
            &mut app,
            vec![
                egui::Event::PointerMoved(cap.center()),
                egui::Event::PointerButton {
                    pos: cap.center(),
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                },
            ],
        );
    }
    assert_eq!(app.shortcut_recording, Some(Action::Save));
    run(&mut app, vec![key(Action::Open)]);
    assert!(app.shortcut_error.is_some());
    assert_eq!(app.shortcut_recording, Some(Action::Save));
    let binding = KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::S);
    run(
        &mut app,
        vec![egui::Event::Key {
            key: binding.logical_key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: binding.modifiers,
        }],
    );
    assert!(app.shortcut_recording.is_none());
    assert_eq!(app.shortcut_keys.binding(Action::Save), Some(binding));
    let restored: LiteTextApp =
        serde_json::from_str(&serde_json::to_string(&app).unwrap()).unwrap();
    assert_eq!(restored.shortcut_keys.binding(Action::Save), Some(binding));
    assert!(restored.shortcut_recording.is_none());
    app.shortcut_recording = Some(Action::Open);
    run(
        &mut app,
        vec![egui::Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }],
    );
    assert!(app.shortcut_recording.is_none() && app.shortcuts_page && app.show_settings);
    assert!(app.shortcut_keys.set(Action::Open, binding).is_err());
    assert!(
        !ACTIONS
            .iter()
            .any(|a| a.label(Lang::En).contains("Highlighter"))
    );
}

#[test]
fn native_edit_bindings_can_move_without_old_keys_or_swapped_keys_firing() {
    let ctx = egui::Context::default();
    let mut app = LiteTextApp::default();
    app.shortcut_keys
        .set(
            Action::Copy,
            KeyboardShortcut::new(Modifiers::COMMAND, Key::J),
        )
        .unwrap();
    app.shortcut_keys
        .set(Action::SelectAll, Action::Copy.binding())
        .unwrap();
    let _ = ctx.run(input(vec![key(Action::Copy)]), |ctx| {
        app.handle_shortcuts(ctx);
        assert!(ctx.input(|i| i.events.iter().any(|e| matches!(
            e,
            egui::Event::Key {
                key: Key::A,
                pressed: true,
                ..
            }
        ))));
        assert!(!ctx.input(|i| i.events.contains(&egui::Event::Copy)));
    });
    let _ = ctx.run(
        input(vec![egui::Event::Key {
            key: Key::J,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        }]),
        |ctx| {
            app.handle_shortcuts(ctx);
            assert!(ctx.input(|i| i.events.contains(&egui::Event::Copy)));
        },
    );
    app.shortcut_keys
        .set(
            Action::Paste,
            KeyboardShortcut::new(Modifiers::COMMAND, Key::P),
        )
        .unwrap();
    let output = ctx.run(
        input(vec![egui::Event::Key {
            key: Key::P,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        }]),
        |ctx| {
            app.handle_shortcuts(ctx);
        },
    );
    assert!(
        output
            .viewport_output
            .values()
            .any(|v| v.commands.contains(&egui::ViewportCommand::RequestPaste))
    );
    let _ = ctx.run(
        input(vec![egui::Event::Paste("original\r\nbytes".to_owned())]),
        |ctx| {
            app.handle_shortcuts(ctx);
            assert!(ctx.input(|i| {
                i.events
                    .contains(&egui::Event::Paste("original\r\nbytes".to_owned()))
            }));
        },
    );
}

#[test]
fn platform_line_delete_and_slash_comment_bindings_are_explicit() {
    assert_eq!(
        Action::DeleteLine.binding_for(true),
        KeyboardShortcut::new(Modifiers::COMMAND, Key::Backspace)
    );
    assert_eq!(
        Action::DeleteLine.binding_for(false),
        KeyboardShortcut::new(Modifiers::COMMAND, Key::Y)
    );
    let ctx = egui::Context::default();
    for mac in [true, false] {
        assert_eq!(
            Action::Comment.binding_for(mac),
            KeyboardShortcut::new(Modifiers::COMMAND, Key::Slash)
        );
    }
    let _ = ctx.run(
        input(vec![egui::Event::Key {
            key: Key::Slash,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        }]),
        |ctx| assert!(take(ctx, Action::Comment)),
    );
    let _ = ctx.run(
        input(vec![egui::Event::Key {
            key: Key::Slash,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND | Modifiers::SHIFT,
        }]),
        |ctx| assert!(!take(ctx, Action::Comment)),
    );
    let mut map = Keymap::default();
    assert!(
        map.set(
            Action::Save,
            KeyboardShortcut::new(Modifiers::COMMAND, Key::Slash)
        )
        .is_err()
    );
}

#[test]
fn line_shortcuts_are_undoable_in_both_editors_and_do_not_edit_from_search_fields() {
    for large in [false, true] {
        let ctx = egui::Context::default();
        let mut app = LiteTextApp::default();
        app.new_document();
        let original = format!(
            "  first()\nsecond()\n{}",
            "unchanged\n".repeat(if large { 30_000 } else { 0 })
        );
        let file = app.active_mut().unwrap();
        file.draft_text = Some(original.clone());
        file.sync_draft();
        file.editor_state.pending_selection = Some(0.."  first()\nsecond()".chars().count());
        let run = |app: &mut LiteTextApp, events| {
            let _ = ctx.run(input(events), |ctx| {
                app.handle_shortcuts(ctx);
                egui::CentralPanel::default().show(ctx, |ui| {
                    crate::draft_editor(
                        ui,
                        app.active_mut().unwrap(),
                        &crate::DARK,
                        false,
                        Lang::En,
                    )
                });
            });
        };
        run(&mut app, vec![]);
        let id = egui::Id::new(if large {
            "large_editor"
        } else {
            "draft_editor"
        })
        .with(app.active_id.unwrap());
        ctx.memory_mut(|m| m.request_focus(id));
        run(&mut app, vec![]);
        run(&mut app, vec![key(Action::Comment)]);
        assert!(
            app.active()
                .unwrap()
                .draft_text
                .as_deref()
                .unwrap()
                .starts_with("  # first()\n# second()\n")
        );
        run(&mut app, vec![key(Action::Undo)]);
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some(original.as_str())
        );
        run(&mut app, vec![key(Action::Redo)]);
        assert!(
            app.active()
                .unwrap()
                .draft_text
                .as_deref()
                .unwrap()
                .starts_with("  # first()\n# second()\n")
        );
        run(&mut app, vec![key(Action::Undo)]);
        app.active_mut().unwrap().editor_state.pending_selection = Some(0..0);
        run(&mut app, vec![]);
        run(&mut app, vec![key(Action::DeleteLine)]);
        assert!(
            app.active()
                .unwrap()
                .draft_text
                .as_deref()
                .unwrap()
                .starts_with("second()\n")
        );
        run(&mut app, vec![key(Action::Undo)]);
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some(original.as_str())
        );
        run(&mut app, vec![key(Action::Find)]);
        run(&mut app, vec![]);
        run(
            &mut app,
            vec![key(Action::DeleteLine), key(Action::Comment)],
        );
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some(original.as_str())
        );
    }
}

#[test]
fn language_comment_shortcut_toggles_blocks_and_prefixes_and_undoes_in_both_editors() {
    for large in [false, true] {
        for (language, expected) in [
            ("CSS", "  /* first */\r\n\t/* second */\r\n"),
            ("HTML", "  <!-- first -->\r\n\t<!-- second -->\r\n"),
            ("SQL", "  -- first\r\n\t-- second\r\n"),
            ("Batch File", "  REM first\r\n\tREM second\r\n"),
            ("Clojure", "  ; first\r\n\t; second\r\n"),
        ] {
            let ctx = egui::Context::default();
            let mut app = LiteTextApp::default();
            app.new_document();
            let original = format!(
                "  first\r\n\tsecond\r\n{}",
                "padding\n".repeat(if large { 40_000 } else { 0 })
            );
            app.active_mut().unwrap().draft_text = Some(original.clone());
            app.active_mut().unwrap().sync_draft();
            app.active_mut().unwrap().language_override = Some(language.to_owned());
            app.active_mut().unwrap().editor_state.pending_selection =
                Some(0.."  first\r\n\tsecond".chars().count());
            let run = |app: &mut LiteTextApp, events| {
                let _ = ctx.run(input(events), |ctx| {
                    app.handle_shortcuts(ctx);
                    egui::CentralPanel::default().show(ctx, |ui| {
                        crate::draft_editor(
                            ui,
                            app.active_mut().unwrap(),
                            &crate::DARK,
                            false,
                            Lang::En,
                        )
                    });
                });
            };
            run(&mut app, vec![]);
            let id = egui::Id::new(if large {
                "large_editor"
            } else {
                "draft_editor"
            })
            .with(app.active_id.unwrap());
            ctx.memory_mut(|m| m.request_focus(id));
            run(&mut app, vec![]);
            run(&mut app, vec![key(Action::Comment)]);
            assert!(
                app.active()
                    .unwrap()
                    .draft_text
                    .as_ref()
                    .unwrap()
                    .starts_with(expected),
                "{large} {language}"
            );
            run(&mut app, vec![key(Action::Undo)]);
            assert_eq!(
                app.active().unwrap().draft_text.as_deref(),
                Some(original.as_str())
            );
            run(&mut app, vec![key(Action::Redo)]);
            assert!(
                app.active()
                    .unwrap()
                    .draft_text
                    .as_ref()
                    .unwrap()
                    .starts_with(expected)
            );
            run(&mut app, vec![key(Action::Comment)]);
            assert_eq!(
                app.active().unwrap().draft_text.as_deref(),
                Some(original.as_str())
            );
        }
    }
}

#[test]
fn custom_selection_and_copy_apply_to_entire_small_and_large_documents() {
    for large in [false, true] {
        let ctx = egui::Context::default();
        let mut app = LiteTextApp::default();
        app.new_document();
        let original = format!(
            "中🙂\n{}",
            "unchanged\n".repeat(if large { 30_000 } else { 0 })
        );
        let file = app.active_mut().unwrap();
        file.draft_text = Some(original.clone());
        file.sync_draft();
        app.shortcut_keys
            .set(
                Action::SelectAll,
                KeyboardShortcut::new(Modifiers::NONE, Key::F6),
            )
            .unwrap();
        app.shortcut_keys
            .set(
                Action::Copy,
                KeyboardShortcut::new(Modifiers::NONE, Key::F7),
            )
            .unwrap();
        let run = |app: &mut LiteTextApp, events| {
            ctx.run(input(events), |ctx| {
                app.handle_shortcuts(ctx);
                egui::CentralPanel::default().show(ctx, |ui| {
                    crate::draft_editor(
                        ui,
                        app.active_mut().unwrap(),
                        &crate::DARK,
                        false,
                        Lang::En,
                    )
                });
            })
        };
        run(&mut app, vec![]);
        let id = egui::Id::new(if large {
            "large_editor"
        } else {
            "draft_editor"
        })
        .with(app.active_id.unwrap());
        ctx.memory_mut(|m| m.request_focus(id));
        run(&mut app, vec![]);
        let press = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        run(&mut app, vec![press(Key::F6)]);
        let output = run(&mut app, vec![press(Key::F7)]);
        assert!(
            output.platform_output.commands.iter().any(
                |cmd| matches!(cmd, egui::OutputCommand::CopyText(value) if value == &original)
            )
        );
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some(original.as_str())
        );
    }
}

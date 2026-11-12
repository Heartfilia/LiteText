//! Undoable document whitespace tools and the persisted Tab input preference.
use crate::{Lang, LoadedFile, Palette};
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::ops::Range;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum TabMode {
    #[default]
    Default,
    Spaces2,
    Spaces4,
}

impl TabMode {
    pub(crate) fn width(self) -> usize {
        if self == Self::Spaces2 { 2 } else { 4 }
    }

    fn label(self, zh: bool) -> &'static str {
        match (self, zh) {
            (Self::Default, true) => "默认制表符",
            (Self::Default, false) => "Default tab",
            (Self::Spaces2, true) => "2空格",
            (Self::Spaces2, false) => "2 spaces",
            (Self::Spaces4, true) => "4空格",
            (Self::Spaces4, false) => "4 spaces",
        }
    }

    /// Only remap physical Tab input to the focused document. Paste, search
    /// fields and modified shortcuts stay untouched. Default inserts a literal
    /// Tab; transforming the event also prevents widget focus navigation.
    pub(crate) fn remap(self, ctx: &egui::Context, editor_id: egui::Id) {
        if !ctx.memory(|m| m.has_focus(editor_id)) {
            return;
        }
        ctx.input_mut(|i| {
            for event in &mut i.events {
                if matches!(event, egui::Event::Key { key: egui::Key::Tab, pressed: true, modifiers, .. } if modifiers.is_none()) {
                    *event = egui::Event::Text(if self == Self::Default { "\t".to_owned() } else { " ".repeat(self.width()) });
                }
            }
        });
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    TrimStart,
    TrimEnd,
    TrimBoth,
    TabsToSpaces,
    LeadingTabsToSpaces,
    SpacesToTabs,
    LeadingSpacesToTabs,
}

pub(crate) const ACTIONS: [Action; 7] = [
    Action::TrimStart,
    Action::TrimEnd,
    Action::TrimBoth,
    Action::TabsToSpaces,
    Action::LeadingTabsToSpaces,
    Action::SpacesToTabs,
    Action::LeadingSpacesToTabs,
];

impl Action {
    pub(crate) fn label(self, zh: bool) -> &'static str {
        match (self, zh) {
            (Self::TrimStart, true) => "去掉行首空白",
            (Self::TrimEnd, true) => "去掉行尾空白",
            (Self::TrimBoth, true) => "去掉首尾空白",
            (Self::TabsToSpaces, true) => "TAB转空格",
            (Self::LeadingTabsToSpaces, true) => "行首TAB转空格",
            (Self::SpacesToTabs, true) => "空格转TAB",
            (Self::LeadingSpacesToTabs, true) => "行首空格转TAB",
            (Self::TrimStart, false) => "Trim leading whitespace",
            (Self::TrimEnd, false) => "Trim trailing whitespace",
            (Self::TrimBoth, false) => "Trim both ends",
            (Self::TabsToSpaces, false) => "Tabs to spaces",
            (Self::LeadingTabsToSpaces, false) => "Leading tabs to spaces",
            (Self::SpacesToTabs, false) => "Spaces to tabs",
            (Self::LeadingSpacesToTabs, false) => "Leading spaces to tabs",
        }
    }
}

fn newline(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

fn edits(text: &str, action: Action, width: usize) -> Vec<(Range<usize>, String)> {
    let mut edits = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive(newline) {
        let content = line.trim_end_matches(newline);
        let leading = content.len() - content.trim_start_matches(char::is_whitespace).len();
        if matches!(
            action,
            Action::TrimStart | Action::TrimEnd | Action::TrimBoth
        ) {
            let start = if action != Action::TrimEnd {
                leading
            } else {
                0
            };
            let end = if action != Action::TrimStart {
                content
                    .trim_end_matches(char::is_whitespace)
                    .len()
                    .max(start)
            } else {
                content.len()
            };
            if start > 0 {
                edits.push((offset..offset + start, String::new()));
            }
            if end < content.len() {
                edits.push((offset + end..offset + content.len(), String::new()));
            }
        } else {
            let limit = if matches!(
                action,
                Action::LeadingTabsToSpaces | Action::LeadingSpacesToTabs
            ) {
                leading
            } else {
                content.len()
            };
            let bytes = content.as_bytes();
            let mut at = 0;
            while at < limit {
                if matches!(action, Action::TabsToSpaces | Action::LeadingTabsToSpaces)
                    && bytes[at] == b'\t'
                {
                    let start = at;
                    while at < limit && bytes[at] == b'\t' {
                        at += 1;
                    }
                    edits.push((
                        offset + start..offset + at,
                        " ".repeat((at - start) * width),
                    ));
                } else if matches!(action, Action::SpacesToTabs | Action::LeadingSpacesToTabs)
                    && bytes[at] == b' '
                {
                    let start = at;
                    while at < limit && bytes[at] == b' ' {
                        at += 1;
                    }
                    let tabs = (at - start) / width;
                    if tabs > 0 {
                        edits.push((
                            offset + start..offset + start + tabs * width,
                            "\t".repeat(tabs),
                        ));
                    }
                } else {
                    at += 1;
                }
            }
        }
        offset += line.len();
    }
    edits
}

pub(crate) fn apply(file: &mut LoadedFile, ctx: &egui::Context, action: Action, mode: TabMode) {
    if file.view != crate::ViewMode::Text || file.is_binary {
        return;
    }
    let Some(text) = file.draft_text.as_deref() else {
        return;
    };
    let selected = crate::line_commands::selected_bytes(file, text);
    let changes = edits(text, action, mode.width());
    crate::line_commands::commit_edits(file, ctx, changes, Some(selected));
    ctx.request_repaint();
}

pub(crate) fn picker(
    ui: &mut egui::Ui,
    mode: TabMode,
    lang: Lang,
    pal: &Palette,
    enabled: bool,
) -> Option<TabMode> {
    let zh = lang == Lang::ZhCn;
    let id = egui::Id::new("status_tab_mode");
    let galley = ui.painter().layout_no_wrap(
        format!("TAB: {}  ▴", mode.label(zh)),
        egui::FontId::proportional(12.0),
        pal.text_faint,
    );
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(galley.size().x + 8.0, 14.0),
        egui::Sense::click(),
    );
    let response = response.on_hover_text(if zh {
        "TAB 输入方式：默认输入制表符，或输入 2/4 个空格。默认转换宽度为 4。"
    } else {
        "Tab input: a literal tab, 2 spaces or 4 spaces. Default conversion width: 4."
    });
    ui.ctx().data_mut(|d| d.insert_temp(id.with("rect"), rect));
    ui.painter()
        .galley(rect.center() - galley.size() * 0.5, galley, pal.text_faint);
    if !enabled && ui.memory(|m| m.is_popup_open(id)) {
        ui.memory_mut(|m| m.close_popup());
    }
    if enabled && response.clicked() {
        ui.memory_mut(|m| m.toggle_popup(id));
    }
    let mut picked = None;
    if enabled && ui.memory(|m| m.is_popup_open(id)) {
        let popup = egui::Area::new(id.with("popup"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::LEFT_BOTTOM)
            .fixed_pos(rect.left_top() - egui::vec2(0.0, 8.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style())
                    .fill(pal.bg_panel)
                    .show(ui, |ui| {
                        let font = egui::TextStyle::Button.resolve(ui.style());
                        let width = [TabMode::Default, TabMode::Spaces2, TabMode::Spaces4]
                            .into_iter()
                            .map(|choice| {
                                ui.painter()
                                    .layout_no_wrap(
                                        choice.label(zh).to_owned(),
                                        font.clone(),
                                        pal.text_faint,
                                    )
                                    .size()
                                    .x
                            })
                            .fold(0.0_f32, f32::max)
                            + 2.0 * ui.spacing().button_padding.x;
                        ui.set_width(width);
                        for choice in [TabMode::Default, TabMode::Spaces2, TabMode::Spaces4] {
                            let row = ui.selectable_label(mode == choice, choice.label(zh));
                            ui.ctx().data_mut(|d| {
                                d.insert_temp(id.with(choice.label(false)), row.rect)
                            });
                            if row.clicked() {
                                picked = Some(choice);
                            }
                        }
                    });
            });
        ui.ctx()
            .data_mut(|d| d.insert_temp(id.with("popup_rect"), popup.response.rect));
        if picked.is_some()
            || ui.input(|i| {
                i.key_pressed(egui::Key::Escape)
                    || i.pointer.any_click()
                        && i.pointer
                            .interact_pos()
                            .is_some_and(|p| !rect.contains(p) && !popup.response.rect.contains(p))
            })
        {
            ui.memory_mut(|m| m.close_popup());
        }
    }
    picked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1080.0, 720.0),
            )),
            events,
            ..Default::default()
        }
    }

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            modifiers,
            physical_key: None,
            pressed: true,
            repeat: false,
        }
    }

    fn draw(ctx: &egui::Context, app: &mut crate::LiteTextApp, events: Vec<egui::Event>) {
        let _ = ctx.run(raw(events), |ctx| {
            app.handle_shortcuts(ctx);
            egui::CentralPanel::default().show(ctx, |ui| {
                crate::draft_editor(ui, app.active_mut().unwrap(), &crate::DARK, false, Lang::En)
            });
        });
    }

    #[test]
    fn tab_input_replaces_selection_and_undoes_in_both_editors_without_changing_paste() {
        for large in [false, true] {
            for mode in [TabMode::Default, TabMode::Spaces2, TabMode::Spaces4] {
                let ctx = egui::Context::default();
                let mut app = crate::LiteTextApp {
                    tab_mode: mode,
                    ..Default::default()
                };
                app.new_document();
                let original = format!(
                    "中🙂 tail\n{}",
                    "padding\n".repeat(if large { 40_000 } else { 0 })
                );
                app.active_mut().unwrap().draft_text = Some(original.clone());
                app.active_mut().unwrap().sync_draft();
                app.active_mut().unwrap().editor_state.pending_selection = Some(0..2);
                draw(&ctx, &mut app, vec![]);
                ctx.memory_mut(|m| {
                    m.request_focus(
                        egui::Id::new(if large {
                            "large_editor"
                        } else {
                            "draft_editor"
                        })
                        .with(app.active_id.unwrap()),
                    )
                });
                draw(&ctx, &mut app, vec![]);
                draw(
                    &ctx,
                    &mut app,
                    vec![key(egui::Key::Tab, Default::default())],
                );
                let replacement = if mode == TabMode::Default {
                    "\t".to_owned()
                } else {
                    " ".repeat(mode.width())
                };
                assert_eq!(
                    app.active().unwrap().draft_text.as_ref().unwrap(),
                    &format!("{replacement}{}", &original["中🙂".len()..]),
                    "{large} {mode:?}"
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
                draw(&ctx, &mut app, vec![egui::Event::Paste("\t".to_owned())]);
                assert!(
                    app.active()
                        .unwrap()
                        .draft_text
                        .as_ref()
                        .unwrap()
                        .contains('\t')
                );
                let before = app.active().unwrap().draft_text.clone();
                ctx.memory_mut(|m| {
                    m.request_focus(egui::Id::new("find_query").with(app.active_id.unwrap()))
                });
                let _ = ctx.run(raw(vec![key(egui::Key::Tab, Default::default())]), |ctx| {
                    app.handle_shortcuts(ctx);
                    assert!(ctx.input(|i| i.events.iter().any(|e| matches!(
                        e,
                        egui::Event::Key {
                            key: egui::Key::Tab,
                            ..
                        }
                    ))));
                });
                assert_eq!(app.active().unwrap().draft_text, before);
            }
        }
    }

    #[test]
    fn document_whitespace_tools_are_single_undoable_and_never_write_originals() {
        for large in [false, true] {
            for action in ACTIONS {
                let ctx = egui::Context::default();
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("sample.txt");
                let original =
                    "     \t中🙂    value\t \u{a0}\r\n".repeat(if large { 15_000 } else { 2 });
                std::fs::write(&path, &original).unwrap();
                let mut app = crate::LiteTextApp::default();
                app.open_paths(vec![path.clone()]);
                draw(&ctx, &mut app, vec![]);
                ctx.memory_mut(|m| {
                    m.request_focus(
                        egui::Id::new(if large {
                            "large_editor"
                        } else {
                            "draft_editor"
                        })
                        .with(app.active_id.unwrap()),
                    )
                });
                apply(app.active_mut().unwrap(), &ctx, action, TabMode::Default);
                let expected = transformed(&original, action, TabMode::Default);
                assert_ne!(expected, original);
                assert_eq!(
                    app.active().unwrap().draft_text.as_deref(),
                    Some(expected.as_str())
                );
                draw(&ctx, &mut app, vec![]);
                draw(
                    &ctx,
                    &mut app,
                    vec![key(egui::Key::Z, egui::Modifiers::COMMAND)],
                );
                assert_eq!(
                    app.active().unwrap().draft_text.as_deref(),
                    Some(original.as_str()),
                    "{large} {action:?}"
                );
                assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
            }
        }
    }

    #[test]
    fn status_picker_and_whitespace_submenu_accept_real_clicks_in_both_languages() {
        for lang in [Lang::ZhCn, Lang::En] {
            let ctx = egui::Context::default();
            crate::setup_fonts(&ctx);
            let mut app = crate::LiteTextApp {
                lang,
                ..Default::default()
            };
            app.new_document();
            app.active_mut().unwrap().draft_text = Some("  中\t \r\n".to_owned());
            let run_status = |app: &mut crate::LiteTextApp, events| {
                ctx.run(raw(events), |ctx| app.status_bar(ctx, &crate::DARK))
            };
            let button = |rect: egui::Rect, pressed| {
                vec![
                    egui::Event::PointerMoved(rect.center()),
                    egui::Event::PointerButton {
                        pos: rect.center(),
                        pressed,
                        button: egui::PointerButton::Primary,
                        modifiers: Default::default(),
                    },
                ]
            };
            run_status(&mut app, vec![]);
            let id = egui::Id::new("status_tab_mode");
            let rect = ctx
                .data(|d| d.get_temp::<egui::Rect>(id.with("rect")))
                .unwrap();
            for pressed in [true, false] {
                run_status(&mut app, button(rect, pressed));
            }
            run_status(&mut app, vec![]);
            run_status(&mut app, vec![]);
            let popup = ctx
                .data(|d| d.get_temp::<egui::Rect>(id.with("popup_rect")))
                .unwrap();
            let longest = [TabMode::Default, TabMode::Spaces2, TabMode::Spaces4]
                .into_iter()
                .map(|mode| {
                    ctx.data(|d| d.get_temp::<egui::Rect>(id.with(mode.label(false))))
                        .unwrap()
                        .width()
                })
                .fold(0.0_f32, f32::max);
            assert!(
                popup.width() <= longest + 20.0,
                "excess popup width: {popup:?}, row={longest}"
            );
            assert_eq!(TabMode::Default.label(true), "默认制表符");
            let choice = ctx
                .data(|d| d.get_temp::<egui::Rect>(id.with("2 spaces")))
                .unwrap();
            assert!(choice.bottom() < rect.top());
            for pressed in [true, false] {
                run_status(&mut app, button(choice, pressed));
            }
            assert_eq!(app.tab_mode, TabMode::Spaces2);
            let anchor =
                egui::Rect::from_min_size(egui::pos2(1000.0, 20.0), egui::vec2(28.0, 24.0));
            let run_menu = |app: &mut crate::LiteTextApp, events| {
                ctx.run(raw(events), |ctx| app.app_menu(ctx, anchor, &crate::DARK))
            };
            app.show_menu = true;
            run_menu(&mut app, vec![]);
            let output = run_menu(&mut app, vec![]);
            let find = |output: &egui::FullOutput, needle: &str| {
                output
                    .shapes
                    .iter()
                    .find_map(|s| match &s.shape {
                        egui::Shape::Text(t) if t.galley.text() == needle => {
                            Some(egui::Rect::from_min_size(t.pos, t.galley.size()))
                        }
                        _ => None,
                    })
                    .unwrap()
            };
            let entry = find(
                &output,
                if lang == Lang::ZhCn {
                    "‹  空白符处理"
                } else {
                    "‹  Whitespace"
                },
            );
            let plugins = find(&output, &format!("‹  {}", crate::tr(lang).plugins));
            assert!(entry.top() > plugins.bottom());
            for _ in 0..3 {
                run_menu(&mut app, vec![egui::Event::PointerMoved(entry.center())]);
            }
            assert!(app.menu_whitespace_open && !app.menu_plugins_open);
            let output = run_menu(&mut app, vec![]);
            for action in ACTIONS {
                find(&output, action.label(lang == Lang::ZhCn));
            }
            let trim = find(&output, Action::TrimBoth.label(lang == Lang::ZhCn));
            for pressed in [true, false] {
                run_menu(&mut app, button(trim, pressed));
            }
            assert!(!app.show_menu);
            assert_eq!(app.active().unwrap().draft_text.as_deref(), Some("中\r\n"));
        }
    }

    fn transformed(text: &str, action: Action, mode: TabMode) -> String {
        let mut result = text.to_owned();
        for (range, value) in edits(text, action, mode.width()).into_iter().rev() {
            result.replace_range(range, &value);
        }
        result
    }

    #[test]
    fn whitespace_operations_preserve_newlines_unicode_and_conversion_remainders() {
        let input = "\t \u{a0}\u{3000}中🙂 \t\u{b}\u{c}\r\n  \t\rnext  \n\u{2028} \u{2029}\u{85}";
        assert_eq!(
            transformed(input, Action::TrimBoth, TabMode::Default),
            "中🙂\r\n\rnext\n\u{2028}\u{2029}\u{85}"
        );
        assert_eq!(
            transformed(" \t a \t\r\n \n", Action::TrimStart, TabMode::Default),
            "a \t\r\n\n"
        );
        assert_eq!(
            transformed(" \t a \t\r\n \n", Action::TrimEnd, TabMode::Default),
            " \t a\r\n\n"
        );
        for mode in [TabMode::Default, TabMode::Spaces2, TabMode::Spaces4] {
            let w = mode.width();
            assert_eq!(
                transformed(" \t中\t\r\n", Action::TabsToSpaces, mode),
                format!(" {}中{}\r\n", " ".repeat(w), " ".repeat(w))
            );
            assert_eq!(
                transformed(" \t中\t\n", Action::LeadingTabsToSpaces, mode),
                format!(" {}中\t\n", " ".repeat(w))
            );
            let spaces = " ".repeat(w + 1);
            assert_eq!(
                transformed(&format!("{spaces}中{spaces}\n"), Action::SpacesToTabs, mode),
                "\t 中\t \n"
            );
            assert_eq!(
                transformed(
                    &format!("{spaces}中{spaces}\n"),
                    Action::LeadingSpacesToTabs,
                    mode
                ),
                format!("\t 中{spaces}\n")
            );
        }
        for action in ACTIONS {
            assert_eq!(transformed("", action, TabMode::Default), "");
        }
    }

    #[test]
    fn tab_preference_persists_and_only_changes_unmodified_document_tab() {
        let ctx = egui::Context::default();
        let editor = egui::Id::new("editor");
        ctx.memory_mut(|m| m.request_focus(editor));
        for mode in [TabMode::Default, TabMode::Spaces2, TabMode::Spaces4] {
            let app = crate::LiteTextApp {
                tab_mode: mode,
                ..Default::default()
            };
            let restored: crate::LiteTextApp =
                serde_json::from_str(&serde_json::to_string(&app).unwrap()).unwrap();
            assert_eq!(restored.tab_mode, mode);
            let key = |modifiers| egui::Event::Key {
                key: egui::Key::Tab,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            };
            let events = vec![
                key(Default::default()),
                key(egui::Modifiers::SHIFT),
                egui::Event::Paste("\t".to_owned()),
            ];
            let _ = ctx.run(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ctx| {
                    mode.remap(ctx, editor);
                    let events = ctx.input(|i| i.events.clone());
                    if mode == TabMode::Default {
                        assert_eq!(events[0], egui::Event::Text("\t".to_owned()));
                    } else {
                        assert_eq!(events[0], egui::Event::Text(" ".repeat(mode.width())));
                    }
                    assert!(matches!(events[1], egui::Event::Key { .. }));
                    assert_eq!(events[2], egui::Event::Paste("\t".to_owned()));
                },
            );
            ctx.memory_mut(|m| m.request_focus(editor));
        }
    }
}

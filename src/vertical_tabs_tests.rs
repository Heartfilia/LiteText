use super::*;
use crate::{LiteTextApp, DARK, LIGHT};

fn draw(ctx: &egui::Context, app: &mut LiteTextApp, size: egui::Vec2, time: f64, events: Vec<egui::Event>, pal: &Palette) {
    let _ = ctx.run(egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
        time: Some(time), events, ..Default::default()
    }, |ctx| {
        app.top_bar(ctx, pal);
        app.vertical_tab_panel(ctx, pal);
        egui::CentralPanel::default().show(ctx, |ui| {
            ctx.data_mut(|d| d.insert_temp(egui::Id::new("test_editor_rect"), ui.max_rect()));
        });
    });
}

fn click(ctx: &egui::Context, app: &mut LiteTextApp, pos: egui::Pos2, time: f64, button: egui::PointerButton) {
    for (index, pressed) in [true, false].into_iter().enumerate() {
        draw(ctx, app, egui::vec2(1080.0, 720.0), time + index as f64 * 0.02, vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton { pos, button, pressed, modifiers: egui::Modifiers::default() },
        ], &DARK);
    }
}

#[test]
fn vertical_toolbar_moves_to_left_and_all_submenus_open_right() {
    for vertical in [true, false] {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = LiteTextApp { vertical_tabs: vertical, ..Default::default() };
        app.new_document();
        for frame in 0..4 { draw(&ctx, &mut app, egui::vec2(1080.0,720.0), frame as f64, vec![], &DARK); }
        let menu = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new("toolbar_menu_rect"))).unwrap();
        let id = app.active_id.unwrap();
        let tools: Vec<_> = [3, 2, 1, 0].into_iter().map(|action|
            ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("text_tool_rect", id, action)))).unwrap()
        ).collect();
        assert!(tools.windows(2).all(|pair| pair[0].right() <= pair[1].left()), "wrap/whitespace/pen/find must keep their left-to-right order");
        if vertical { assert!(menu.right() < tools[0].left()); }
        else { assert!(tools[3].right() < menu.left()); }
        if vertical {
            assert!(menu.left() < 30.0, "{menu:?}");
            let blank = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new("tab_blank_new_rect"))).unwrap();
            assert!(blank.left() > menu.right());
            let panel = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new("vertical_tabs_panel_rect"))).unwrap();
            assert!(panel.width() <= 160.0, "{panel:?}");
        } else { assert!(menu.right() > 1000.0, "{menu:?}"); }
        for (index, name) in ["recent", "plugins", "whitespace", "conversion"].into_iter().enumerate() {
            let base = index as f64 * 30.0;
            app.show_menu = true;
            for frame in 0..3 { draw(&ctx, &mut app, egui::vec2(1080.0,720.0), base + 10.0 + frame as f64, vec![], &DARK); }
            let row = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(format!("app_{name}_row_rect")))).unwrap();
            for frame in 0..3 { draw(&ctx, &mut app, egui::vec2(1080.0,720.0), base + 20.0 + frame as f64, vec![egui::Event::PointerMoved(row.center())], &DARK); }
            let parent = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new("app_menu_rect"))).unwrap();
            let sub = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(format!("app_{name}_submenu_rect")))).unwrap();
            if vertical { assert!(sub.left() >= parent.right(), "{name}: {sub:?} / {parent:?}"); }
            else { assert!(sub.right() <= parent.left(), "{name}: {sub:?} / {parent:?}"); }
            let bridge = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(format!("app_{name}_bridge_rect")))).unwrap();
            let x = if vertical { (parent.right() + sub.left()) / 2.0 } else { (sub.right() + parent.left()) / 2.0 };
            let point = egui::pos2(x, row.center().y);
            assert!(bridge.contains(point));
            draw(&ctx, &mut app, egui::vec2(1080.0,720.0), base + 24.0, vec![egui::Event::PointerMoved(point)], &DARK);
            assert!(ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(format!("app_{name}_submenu_rect")))).is_some());
            app.show_menu = false;
            draw(&ctx, &mut app, egui::vec2(1080.0,720.0), base + 25.0, vec![], &DARK);
        }
    }
}

#[test]
fn vertical_groups_share_width_and_leave_gutter_to_the_right() {
    for pal in [&DARK, &LIGHT] {
        for size in [egui::vec2(640.0, 420.0), egui::vec2(1080.0, 720.0)] {
            let ctx = egui::Context::default();
            crate::setup_fonts(&ctx);
            let mut app = LiteTextApp { vertical_tabs: true, ..Default::default() };
            app.new_document(); let pinned = app.active_id.unwrap(); app.toggle_pin(pinned);
            app.new_document(); let normal = app.active_id.unwrap();
            for frame in 0..4 { draw(&ctx, &mut app, size, frame as f64, vec![], pal); }
            let row = |id| ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("vertical_tab_rect", id)))).unwrap();
            let p = row(pinned); let n = row(normal);
            let editor = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new("test_editor_rect"))).unwrap();
            assert!((p.width() - n.width()).abs() < 0.1);
            assert!(p.bottom() < n.top());
            assert!(n.right() < editor.left());
            assert!(p.top() >= 38.0);
            assert!(editor.width() > 250.0);
        }
    }
}

#[test]
fn vertical_rows_switch_close_and_preserve_pinned_tabs() {
    let ctx = egui::Context::default();
    let mut app = LiteTextApp { vertical_tabs: true, ..Default::default() };
    app.new_document(); let pinned = app.active_id.unwrap(); app.toggle_pin(pinned);
    app.new_document(); let normal = app.active_id.unwrap();
    for frame in 0..3 { draw(&ctx, &mut app, egui::vec2(1080.0,720.0), frame as f64, vec![], &DARK); }
    let p = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("vertical_tab_rect", pinned)))).unwrap();
    click(&ctx, &mut app, p.center(), 4.0, egui::PointerButton::Primary);
    assert_eq!(app.active_id, Some(pinned));
    click(&ctx, &mut app, p.center(), 5.0, egui::PointerButton::Middle);
    assert!(app.files.iter().any(|f| f.id == pinned));
    assert!(ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("vertical_tab_close_rect", normal)))).is_none());
    let row = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("vertical_tab_rect", normal)))).unwrap();
    click(&ctx, &mut app, row.center(), 6.0, egui::PointerButton::Primary);
    click(&ctx, &mut app, row.center(), 6.15, egui::PointerButton::Primary);
    assert!(!app.files.iter().any(|f| f.id == normal));
    assert_eq!(app.active_id, Some(pinned));
}

#[test]
fn new_active_tab_is_revealed_in_a_long_vertical_list() {
    let ctx = egui::Context::default();
    let mut app = LiteTextApp { vertical_tabs: true, ..Default::default() };
    for _ in 0..40 { app.new_document(); }
    let last = app.active_id.unwrap();
    for frame in 0..5 { draw(&ctx, &mut app, egui::vec2(640.0,420.0), frame as f64, vec![], &DARK); }
    app.active_id = app.files.first().map(|f| f.id);
    for frame in 5..10 { draw(&ctx, &mut app, egui::vec2(640.0,420.0), frame as f64, vec![], &DARK); }
    app.active_id = Some(last);
    for frame in 10..15 { draw(&ctx, &mut app, egui::vec2(640.0,420.0), frame as f64, vec![], &DARK); }
    let row = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("vertical_tab_rect", last)))).unwrap();
    assert!(row.top() >= 38.0 && row.bottom() <= 420.0, "{row:?}");
}

#[test]
fn settings_dropdown_really_selects_tab_layout() {
    let ctx = egui::Context::default();
    let mut app = LiteTextApp { show_settings: true, ..Default::default() };
    crate::setup_fonts(&ctx);
    let render = |app: &mut LiteTextApp, time, events| {
        let _ = ctx.run(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1080.0,720.0))),
            time: Some(time), events, ..Default::default()
        }, |ctx| app.settings_window(ctx, &DARK));
    };
    for frame in 0..4 { render(&mut app, frame as f64, vec![]); }
    let id = egui::Id::new("settings_tab_layout");
    let rect = ctx.data(|d| d.get_temp::<egui::Rect>(id.with("button_rect"))).unwrap();
    for (index, pressed) in [true, false].into_iter().enumerate() {
        render(&mut app, 5.0 + index as f64 * 0.02, vec![
            egui::Event::PointerMoved(rect.center()),
            egui::Event::PointerButton { pos: rect.center(), button: egui::PointerButton::Primary, pressed, modifiers: Default::default() },
        ]);
    }
    assert!(!app.vertical_tabs, "opening the dropdown must not change the layout");
    for frame in 0..3 { render(&mut app, 6.0 + frame as f64 * 0.1, vec![]); }
    let option = ctx.data(|d| d.get_temp::<egui::Rect>(id.with(("option_rect", "垂直")))).unwrap();
    for (index, pressed) in [true, false].into_iter().enumerate() {
        render(&mut app, 7.0 + index as f64 * 0.02, vec![
            egui::Event::PointerMoved(option.center()),
            egui::Event::PointerButton { pos: option.center(), button: egui::PointerButton::Primary, pressed, modifiers: Default::default() },
        ]);
    }
    assert!(app.vertical_tabs);
}

#[test]
fn vertical_drag_reorders_normal_tabs_without_moving_pinned_group() {
    let ctx = egui::Context::default();
    let mut app = LiteTextApp { vertical_tabs: true, ..Default::default() };
    app.new_document(); let pinned = app.active_id.unwrap(); app.toggle_pin(pinned);
    app.new_document(); let first = app.active_id.unwrap();
    app.new_document(); let second = app.active_id.unwrap();
    for frame in 0..3 { draw(&ctx, &mut app, egui::vec2(1080.0,720.0), frame as f64, vec![], &DARK); }
    let row = |id| ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("vertical_tab_rect", id)))).unwrap();
    let from = row(first).center();
    let to = row(second).center() + egui::vec2(0.0, 8.0);
    draw(&ctx, &mut app, egui::vec2(1080.0,720.0), 4.0, vec![
        egui::Event::PointerMoved(from),
        egui::Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() },
    ], &DARK);
    draw(&ctx, &mut app, egui::vec2(1080.0,720.0), 4.1, vec![egui::Event::PointerMoved(to)], &DARK);
    draw(&ctx, &mut app, egui::vec2(1080.0,720.0), 4.2, vec![
        egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() },
    ], &DARK);
    assert_eq!(app.files.iter().map(|f| f.id).collect::<Vec<_>>(), vec![pinned, second, first]);
    assert!(app.tab_drag_id.is_none());
}

#[test]
fn vertical_mode_keeps_top_double_click_creation_and_switches_without_losing_tabs() {
    let ctx = egui::Context::default();
    let mut app = LiteTextApp { vertical_tabs: true, ..Default::default() };
    app.new_document();
    let original = app.active_id.unwrap();
    app.active_mut().unwrap().draft_text = Some("keep me".into());
    for frame in 0..3 { draw(&ctx, &mut app, egui::vec2(1080.0,720.0), frame as f64, vec![], &DARK); }
    let blank = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new("tab_blank_new_rect"))).unwrap();
    click(&ctx, &mut app, blank.center(), 4.0, egui::PointerButton::Primary);
    click(&ctx, &mut app, blank.center(), 4.15, egui::PointerButton::Primary);
    assert_eq!(app.files.len(), 2);
    app.vertical_tabs = false;
    draw(&ctx, &mut app, egui::vec2(1080.0,720.0), 5.0, vec![], &DARK);
    assert_eq!(app.files.iter().find(|f| f.id == original).unwrap().draft_text.as_deref(), Some("keep me"));
    let editor = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new("test_editor_rect"))).unwrap();
    assert!(editor.left() < 20.0);
}

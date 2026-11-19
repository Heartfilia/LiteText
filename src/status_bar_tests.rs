//! Status-bar menus must be easy to hit and not silently disabled by unrelated work.
use crate::{DARK, Lang, LiteTextApp};
use eframe::egui;

fn draw(ctx: &egui::Context, app: &mut LiteTextApp, events: Vec<egui::Event>) -> egui::FullOutput {
    ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 600.0),
            )),
            events,
            ..Default::default()
        },
        |ctx| app.status_bar(ctx, &DARK),
    )
}

fn click(ctx: &egui::Context, app: &mut LiteTextApp, pos: egui::Pos2) {
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
    for _ in 0..3 {
        draw(ctx, app, vec![]);
    }
}

fn rect(ctx: &egui::Context, id: egui::Id) -> egui::Rect {
    ctx.data(|data| data.get_temp::<egui::Rect>(id.with("rect")))
        .expect("picker must publish its painted rect")
}

#[test]
fn status_menus_accept_clicks_above_the_painted_text_strip() {
    let cases = [
        "workspace_language",
        "status_tab_mode",
        "status_line_ending",
        "status_encoding",
    ];
    for name in cases {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = LiteTextApp {
            lang: Lang::ZhCn,
            ..Default::default()
        };
        app.new_document();
        let file_id = app.active_id.unwrap();
        for _ in 0..3 {
            draw(&ctx, &mut app, vec![]);
        }
        let id = if name == "workspace_language" {
            egui::Id::new((name, file_id))
        } else {
            egui::Id::new(name)
        };
        let painted = rect(&ctx, id);
        assert_eq!(painted.height(), 14.0);
        // 9 px above the text centre is outside the old 14 px-wide hit rect
        // (half height 7) but inside the expanded one (half height 12).
        let target = painted.center() - egui::vec2(0.0, 9.0);
        assert!(!painted.contains(target));
        click(&ctx, &mut app, target);
        assert!(
            ctx.memory(|memory| memory.is_popup_open(id)),
            "{name}: click just above the text should open the menu"
        );
        // Escape still closes it; the padded hit area does not trap the menu.
        draw(
            &ctx,
            &mut app,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        for _ in 0..2 {
            draw(&ctx, &mut app, vec![]);
        }
        assert!(
            !ctx.memory(|memory| memory.is_popup_open(id)),
            "{name}: click far above must not hit the picker"
        );
    }
}

#[test]
fn background_search_and_value_popup_do_not_disable_status_menus() {
    let ctx = egui::Context::default();
    crate::setup_fonts(&ctx);
    let mut app = LiteTextApp {
        lang: Lang::ZhCn,
        ..Default::default()
    };
    app.new_document();
    app.inspector_popup = Some("private value".into());
    assert!(app.shortcuts_blocked());
    assert!(!app.status_controls_blocked());
    for _ in 0..3 {
        draw(&ctx, &mut app, vec![]);
    }
    let file_id = app.active_id.unwrap();
    let id = egui::Id::new(("workspace_language", file_id));
    let painted = rect(&ctx, id);
    click(&ctx, &mut app, painted.center() - egui::vec2(0.0, 9.0));
    assert!(
        ctx.memory(|memory| memory.is_popup_open(id)),
        "value popup must not swallow the first language-menu click"
    );

    // Real dialogs and in-flight file operations still block the menus.
    app.show_settings = true;
    assert!(app.status_controls_blocked());
    app.show_settings = false;
    app.pending_window_close = true;
    assert!(app.status_controls_blocked());
    app.pending_window_close = false;
    app.pending_batch = None;
}

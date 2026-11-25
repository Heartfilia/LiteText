//! Exercise the real toolbar -> plugin menu path, not just converter APIs.
use crate::*;

#[test]
fn opening_plugin_menu_keeps_selection_and_formats_only_selected_json() {
    for large in [false, true] {
        let ctx = egui::Context::default(); setup_fonts(&ctx);
        let mut app = LiteTextApp::default(); app.new_document();
        let prefix = "before\n";
        let input = "{\"中文\":1}";
        let suffix = if large { "\nafter\n".repeat(40_000) } else { "\nafter".into() };
        let original = format!("{prefix}{input}{suffix}");
        app.active_mut().unwrap().draft_text = Some(original.clone());
        let mut frame = 0usize;
        let mut draw = |app: &mut LiteTextApp, events: Vec<egui::Event>| {
            frame += 1;
            ctx.run(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1080.0,720.0))),
                time: Some(frame as f64 * 0.05), events, ..Default::default()
            }, |ctx| {
                app.top_bar(ctx, &DARK);
                egui::CentralPanel::default().show(ctx, |ui| {
                    draft_editor(ui, app.active_mut().unwrap(), &DARK, false, Lang::ZhCn);
                });
            })
        };
        for _ in 0..4 { draw(&mut app, vec![]); }
        let cursor = egui::text::CCursorRange::two(
            egui::text::CCursor::new(prefix.chars().count()),
            egui::text::CCursor::new(prefix.chars().count() + input.chars().count()));
        app.active_mut().unwrap().editor_state.pending_selection = Some(cursor.primary.index..cursor.secondary.index);
        app.active_mut().unwrap().editor_state.cursor = Some(cursor);
        app.active_mut().unwrap().editor_state.focus_requested = true;
        for _ in 0..3 { draw(&mut app, vec![]); }
        let menu = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new("toolbar_menu_rect"))).unwrap();
        for pressed in [true, false] {
            draw(&mut app, vec![egui::Event::PointerMoved(menu.center()), egui::Event::PointerButton {
                pos: menu.center(), button: egui::PointerButton::Primary, pressed, modifiers: Default::default(),
            }]);
        }
        assert!(app.show_menu);
        for _ in 0..3 { draw(&mut app, vec![]); }
        let plugins = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new("app_plugins_row_rect"))).unwrap();
        let mut output = draw(&mut app, vec![egui::Event::PointerMoved(plugins.center())]);
        for _ in 0..3 { output = draw(&mut app, vec![egui::Event::PointerMoved(plugins.center())]); }
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Text(text) if text.galley.text().contains(input)
                && text.galley.rows.iter().any(|row| row.visuals.mesh.vertices.iter()
                    .any(|vertex| vertex.color == ctx.style().visuals.selection.bg_fill)))),
            "selection must remain visibly highlighted while menus have focus; large={large}");
        let selected = crate::line_commands::selected_bytes(app.active().unwrap(), &original);
        assert_eq!(&original[selected], input, "menu must retain document selection; large={large}");
        let action = output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == "Format JSON" => Some(text.pos + text.galley.size() * 0.5), _ => None,
        }).unwrap();
        for _ in 0..3 { draw(&mut app, vec![egui::Event::PointerMoved(action)]); }
        for pressed in [true, false] { draw(&mut app, vec![egui::Event::PointerMoved(action), egui::Event::PointerButton {
            pos: action, button: egui::PointerButton::Primary, pressed, modifiers: Default::default(),
        }]); }
        assert!(!app.show_menu);
        assert!(app.plugin_error.is_none(), "{:?}", app.plugin_error);
        assert_eq!(app.active().unwrap().draft_text.as_deref(), Some(format!("{prefix}{}{suffix}", crate::json_plugin::format_json(input).unwrap()).as_str()));
    }
}

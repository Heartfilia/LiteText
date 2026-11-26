use crate::*;

#[test]
fn optional_close_buttons_work_in_both_layouts_and_protect_pinned_tabs() {
    for vertical in [false, true] {
        for mode in [TabCloseMode::DoubleClick, TabCloseMode::MiddleClick, TabCloseMode::DoubleOrMiddle,
            TabCloseMode::Button, TabCloseMode::ButtonOrDouble, TabCloseMode::ButtonOrMiddle] {
            let ctx = egui::Context::default(); setup_fonts(&ctx);
            let mut app = LiteTextApp { vertical_tabs: vertical, tab_close_mode: mode, ..Default::default() };
            app.new_document(); let pinned = app.active_id.unwrap(); app.toggle_pin(pinned);
            app.new_document(); let normal = app.active_id.unwrap();
            let mut frame = 0;
            let mut draw = |app: &mut LiteTextApp, events| {
                frame += 1;
                let _ = ctx.run(egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1080.0,720.0))),
                    time: Some(frame as f64 * 0.05), events, ..Default::default()
                }, |ctx| { app.top_bar(ctx, &DARK); app.vertical_tab_panel(ctx, &DARK); });
            };
            for _ in 0..3 { draw(&mut app, vec![]); }
            let button = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("tab_close_button_rect", normal))));
            assert_eq!(button.is_some(), mode.allows_button());
            assert!(ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("tab_close_button_rect", pinned)))).is_none());
            if let Some(rect) = button {
                let tab = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new((
                    if vertical { "vertical_tab_rect" } else { "normal_tab_rect" }, normal)))).unwrap();
                assert!((rect.center().y - tab.center().y).abs() < 0.01);
                for pressed in [true, false] { draw(&mut app, vec![egui::Event::PointerMoved(rect.center()), egui::Event::PointerButton {
                    pos: rect.center(), button: egui::PointerButton::Primary, pressed, modifiers: Default::default(),
                }]); }
                assert!(!app.files.iter().any(|f| f.id == normal), "{mode:?} vertical={vertical}");
                assert!(app.files.iter().any(|f| f.id == pinned));
            }
            let serialized = serde_json::to_string(&mode).unwrap();
            assert_eq!(serde_json::from_str::<TabCloseMode>(&serialized).unwrap(), mode);
        }
    }
}

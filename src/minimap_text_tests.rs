use crate::*;

#[test]
fn text_minimap_navigates_both_editors_without_touching_content_and_skips_markdown_views() {
    for large in [false, true] {
        let ctx = egui::Context::default(); setup_fonts(&ctx);
        let original = format!("{}\n{}", "wide content ".repeat(150),
            (0..if large { 50_000 } else { 400 }).map(|i| format!("  line {i} content\n")).collect::<String>());
        let mut app = LiteTextApp::default(); app.new_document();
        app.active_mut().unwrap().draft_text = Some(original.clone()); app.active_mut().unwrap().sync_draft();
        let id = app.active_id.unwrap(); let map = egui::Id::new(("text_minimap", id));
        let mut frame = 0;
        let mut draw = |app: &mut LiteTextApp, events| {
            frame += 1;
            ctx.run(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0,600.0))),
                time: Some(frame as f64 * 0.05), events, ..Default::default()
            }, |ctx| { egui::CentralPanel::default().show(ctx, |ui| { draft_editor(ui, app.active_mut().unwrap(), &DARK, false, Lang::En); }); })
        };
        for _ in 0..4 { draw(&mut app, vec![]); }
        let rect = ctx.data(|d| d.get_temp::<egui::Rect>(map.with("minimap_rect"))).unwrap();
        assert!(rect.width() <= crate::minimap::WIDTH + 1.0);
        assert_eq!(ctx.data(|d| d.get_temp::<f32>(map.with("minimap_visibility"))), Some(0.0));
        let scroll_geometry = ctx.data(|d| d.get_temp::<(egui::Id, egui::Vec2, egui::Rect)>(egui::Id::new(("editor_scroll_geometry", id)))).unwrap();
        let content_geometry = ctx.data(|d| d.get_temp::<(egui::Rect, egui::Vec2)>(egui::Id::new(("editor_content_geometry", id)))).unwrap();
        assert!(content_geometry.1.x > scroll_geometry.2.width(), "exercise a real horizontal scroll range, large={large}");
        for _ in 0..8 { draw(&mut app, vec![egui::Event::PointerMoved(rect.center())]); }
        assert_eq!(ctx.data(|d| d.get_temp::<f32>(map.with("minimap_visibility"))), Some(1.0));
        assert_eq!(ctx.data(|d| d.get_temp::<egui::Rect>(map.with("minimap_rect"))), Some(rect));
        assert_eq!(ctx.data(|d| d.get_temp::<(egui::Id, egui::Vec2, egui::Rect)>(egui::Id::new(("editor_scroll_geometry", id)))).unwrap(), scroll_geometry);
        assert_eq!(ctx.data(|d| d.get_temp::<(egui::Rect, egui::Vec2)>(egui::Id::new(("editor_content_geometry", id)))).unwrap(), content_geometry,
            "gutter/content dimensions (and horizontal scroll range) must not change on hover, large={large}");
        let initial_range = ctx.data(|d| d.get_temp::<std::ops::Range<usize>>(map.with("minimap_sample_range"))).unwrap();
        let initial_thumb = ctx.data(|d| d.get_temp::<egui::Rect>(map.with("minimap_thumb"))).unwrap();
        assert!(initial_thumb.height() >= rect.height() / 10.0 - 0.1);
        if large { assert!(initial_range.len() < 1000, "local range, not all 50k rows: {initial_range:?}"); }
        let pos = egui::pos2(rect.center().x, rect.bottom() - 10.0);
        for pressed in [true, false] { draw(&mut app, vec![egui::Event::PointerMoved(pos), egui::Event::PointerButton {
            pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default(),
        }]); }
        for _ in 0..3 { draw(&mut app, vec![]); }
        let (_, offset, viewport) = ctx.data(|d| d.get_temp::<(egui::Id,egui::Vec2,egui::Rect)>(egui::Id::new(("editor_scroll_geometry", id)))).unwrap();
        assert!(offset.y > 1000.0, "large={large}: {offset:?}");
        let range = ctx.data(|d| d.get_temp::<std::ops::Range<usize>>(map.with("minimap_sample_range"))).unwrap();
        assert!(range.start > initial_range.start, "preview window must move with document: {initial_range:?} -> {range:?}");
        assert!(range.len() <= initial_range.len() + 2);
        let thumb = ctx.data(|d| d.get_temp::<egui::Rect>(map.with("minimap_thumb"))).unwrap();
        assert!((thumb.height() - initial_thumb.height()).abs() < 0.1);
        assert!(rect.bottom() <= viewport.bottom() + 0.1);
        assert_eq!(app.active().unwrap().draft_text.as_deref(), Some(original.as_str()));
        let before_leave_scroll = ctx.data(|d| d.get_temp::<(egui::Id, egui::Vec2, egui::Rect)>(egui::Id::new(("editor_scroll_geometry", id)))).unwrap();
        draw(&mut app, vec![egui::Event::PointerGone]);
        for _ in 0..8 { draw(&mut app, vec![]); }
        assert_eq!(ctx.data(|d| d.get_temp::<f32>(map.with("minimap_visibility"))), Some(0.0));
        assert_eq!(ctx.data(|d| d.get_temp::<(egui::Id, egui::Vec2, egui::Rect)>(egui::Id::new(("editor_scroll_geometry", id)))).unwrap(), before_leave_scroll);
        assert_eq!(ctx.data(|d| d.get_temp::<(egui::Rect, egui::Vec2)>(egui::Id::new(("editor_content_geometry", id)))).unwrap(), content_geometry);
    }
    for mode in [markdown::Mode::Live, markdown::Mode::Split] {
        let ctx = egui::Context::default(); setup_fonts(&ctx);
        let mut app = LiteTextApp::default(); app.new_document();
        let file = app.active_mut().unwrap(); file.draft_text = Some("# Header\n\nbody".into());
        file.language_override = Some("Markdown".into()); file.markdown_mode = mode; file.sync_draft(); let id = file.id;
        let _ = ctx.run(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0,700.0))), ..Default::default() }, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| draft_editor(ui, app.active_mut().unwrap(), &DARK, false, Lang::En));
        });
        assert!(ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("text_minimap",id)).with("minimap_rect"))).is_none());
    }
}

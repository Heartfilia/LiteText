use super::*;

const FILE_ID: u64 = 719;

fn draw(ctx: &egui::Context, text: &mut String, state: &mut EditorState, split: bool) -> f32 {
    let mut row = 0.0;
    let _ = ctx.run(egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
        ..Default::default()
    }, |ctx| {
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| {
            row = row_height(ui, &state.font());
            if split {
                show_synced(ui, text, state, FILE_ID, &crate::DARK, false,
                    crate::i18n::tr(crate::i18n::Lang::En), &[], &[], None);
            } else {
                show_impl(ui, text, state, FILE_ID, &crate::DARK, false,
                    crate::i18n::tr(crate::i18n::Lang::En), None, None);
            }
        });
    });
    row
}

fn geometry(ctx: &egui::Context) -> (egui::Id, egui::Vec2, egui::Rect, egui::Vec2) {
    let (id, offset, viewport) = ctx.data(|d| d.get_temp::<(egui::Id, egui::Vec2, egui::Rect)>(
        egui::Id::new(("editor_scroll_geometry", FILE_ID)))).unwrap();
    let (_, content) = ctx.data(|d| d.get_temp::<(egui::Rect, egui::Vec2)>(
        egui::Id::new(("editor_content_geometry", FILE_ID)))).unwrap();
    (id, offset, viewport, content)
}

#[test]
fn scroll_tail_short_empty_and_split_documents_do_not_gain_overflow() {
    for (split, large) in [(false, false), (false, true), (true, false)] {
        for source in [String::new(), "hello".to_owned(), "one\ntwo\nthree".to_owned(),
            std::iter::repeat_n("hello", 12).collect::<Vec<_>>().join("\n")] {
            let ctx = egui::Context::default();
            let mut text = source.clone();
            let mut state = EditorState::default();
            state.large.active = large;
            for _ in 0..5 { draw(&ctx, &mut text, &mut state, split); }
            let (_, offset, viewport, content) = geometry(&ctx);
            assert!(content.y <= viewport.height() + 0.5, "{source:?} split={split} large={large}: {content:?} {viewport:?}");
            assert!((viewport.right() - 800.0).abs() < 0.5, "no vertical bar for short content: {viewport:?}");
            assert_eq!(offset.y, 0.0);
            assert_eq!(text, source);
        }
    }
}

#[test]
fn scroll_tail_uses_real_rows_and_reaches_half_viewport_without_editing() {
    for (large, lines) in [(false, 22), (false, 100), (true, 22), (true, 100)] {
        let ctx = egui::Context::default();
        ctx.style_mut(|s| s.animation_time = 0.0);
        let source = std::iter::repeat_n("hello", lines).collect::<Vec<_>>().join("\n");
        let mut text = source.clone();
        let mut state = EditorState::default();
        state.large.active = large;
        let mut row = 0.0;
        for _ in 0..5 { row = draw(&ctx, &mut text, &mut state, false); }
        let (id, _, viewport, content) = geometry(&ctx);
        let expected = lines as f32 * row + if large { 24.0 } else { 12.0 } + viewport.height() * 0.5;
        assert!((content.y - expected).abs() < 1.0, "large={large} lines={lines}: {content:?} expected={expected} row={row}");
        assert!(content.y > viewport.height());
        let mut scroll = egui::scroll_area::State::default();
        scroll.offset.y = 1e7;
        scroll.store(&ctx, id);
        for _ in 0..5 { draw(&ctx, &mut text, &mut state, false); }
        let (_, offset, viewport, content) = geometry(&ctx);
        assert!((offset.y - (content.y - viewport.height())).abs() < 1.0);
        let last_line_bottom = 12.0 + lines as f32 * row - offset.y;
        let expected_bottom = viewport.height() * 0.5 - if large { 12.0 } else { 0.0 };
        assert!((last_line_bottom - expected_bottom).abs() < 1.0);
        assert_eq!(text, source);
        assert_eq!(state.large.active, large);
    }
}

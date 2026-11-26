//! Integration tests for the HEX overview rather than the shared geometry alone.
use super::*;

fn opts() -> HexViewOpts {
    HexViewOpts { font_size: 14.0, upper: true, dim_zero: true,
        show_ascii: true, show_invisible: false }
}

fn frame(
    ctx: &egui::Context,
    file: &LoadedFile,
    selection: &mut Option<(usize, usize)>,
    anchor: &mut Option<usize>,
    events: Vec<egui::Event>,
) -> (egui::FullOutput, egui::Id) {
    let mut id = egui::Id::NULL;
    let output = ctx.run(egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 500.0))),
        time: Some(ctx.input(|input| input.time) + 0.05),
        events,
        ..Default::default()
    }, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            id = ui.make_persistent_id(("hex_minimap", file.id));
            hex_view(ui, file, &DARK, selection, anchor, &opts());
        });
    });
    (output, id)
}

fn thumb(output: &egui::FullOutput) -> egui::Rect {
    output.shapes.iter().find_map(|shape| match &shape.shape {
        // Area fade-in scales opacity during warm-up frames; geometry is stable.
        egui::Shape::Rect(rect) if (rect.rect.width() - minimap::WIDTH).abs() < 0.01
            && rect.stroke.width == 1.0 && rect.rect.height() <= 80.0 && rect.rect.height() >= 20.0 => Some(rect.rect),
        _ => None,
    }).expect("HEX minimap viewport thumb")
}

fn pointer(pos: egui::Pos2, pressed: bool) -> Vec<egui::Event> {
    vec![egui::Event::PointerMoved(pos), egui::Event::PointerButton {
        pos, button: egui::PointerButton::Primary, pressed,
        modifiers: egui::Modifiers::NONE,
    }]
}

#[test]
fn hex_minimap_excludes_header_and_scrollbar_and_navigates_without_editing() {
    let mut app = LiteTextApp::default();
    app.new_document();
    let file = app.active_mut().unwrap();
    file.bytes = vec![0x42; 160_000];
    let original = file.bytes.clone();
    let ctx = egui::Context::default();
    let mut selection = Some((17, 23));
    let mut anchor = None;
    for _ in 0..8 { frame(&ctx, file, &mut selection, &mut anchor, vec![]); }
    let (idle, id) = frame(&ctx, file, &mut selection, &mut anchor, vec![]);
    assert_eq!(ctx.data(|data| data.get_temp::<f32>(id.with("minimap_visibility"))), Some(0.0));
    assert!(!idle.shapes.iter().any(|shape| matches!(shape.shape, egui::Shape::LineSegment { stroke, .. } if stroke.color.a() == 24)));
    let rect = ctx.data(|data| data.get_temp::<egui::Rect>(id.with("minimap_rect"))).unwrap();
    for _ in 0..8 {
        frame(&ctx, file, &mut selection, &mut anchor, vec![egui::Event::PointerMoved(rect.center())]);
    }
    assert_eq!(ctx.data(|data| data.get_temp::<f32>(id.with("minimap_visibility"))), Some(1.0));
    let (output, _) = frame(&ctx, file, &mut selection, &mut anchor, vec![]);
    assert_eq!(ctx.data(|data| data.get_temp::<egui::Rect>(id.with("minimap_rect"))), Some(rect));
    let initial = thumb(&output);
    assert_eq!(initial.width(), minimap::WIDTH);
    assert!(initial.top() > 25.0, "frozen Address header is outside minimap");
    assert!(initial.right() < 980.0, "right margin and scrollbar remain exposed");
    let target = egui::pos2(initial.center().x, 400.0);
    frame(&ctx, file, &mut selection, &mut anchor, pointer(target, true));
    frame(&ctx, file, &mut selection, &mut anchor, pointer(target, false));
    let (output, _) = frame(&ctx, file, &mut selection, &mut anchor, vec![]);
    let moved = thumb(&output);
    assert!(moved.top() > initial.top() + 100.0, "stored scroll state moves HEX viewport");
    frame(&ctx, file, &mut selection, &mut anchor, pointer(moved.center(), true));
    let dragged_to = egui::pos2(moved.center().x, 100.0);
    frame(&ctx, file, &mut selection, &mut anchor, vec![egui::Event::PointerMoved(dragged_to)]);
    frame(&ctx, file, &mut selection, &mut anchor, pointer(dragged_to, false));
    let (output, _) = frame(&ctx, file, &mut selection, &mut anchor, vec![]);
    assert!(thumb(&output).top() < moved.top() - 100.0, "dragging thumb navigates upward");
    assert_eq!(selection, Some((17, 23)), "overlay must not select underlying bytes");
    assert_eq!(anchor, None);
    assert_eq!(file.bytes, original);
}

#[test]
fn hex_minimap_is_not_created_for_markdown_live_or_split() {
    let mut app = LiteTextApp::default();
    app.new_document();
    let file = app.active_mut().unwrap();
    file.path = PathBuf::from("notes.md");
    file.bytes = vec![b'x'; 16_000];
    assert!(markdown::is_markdown(file));
    for mode in [markdown::Mode::Live, markdown::Mode::Split, markdown::Mode::Source] {
        file.markdown_mode = mode;
        let ctx = egui::Context::default();
        let (_, id) = frame(&ctx, file, &mut None, &mut None, vec![]);
        assert_eq!(ctx.data(|data| data.get_temp::<minimap::Preview>(id.with("cache")).is_some()),
            mode == markdown::Mode::Source);
    }
}

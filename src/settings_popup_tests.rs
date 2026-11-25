//! Popups over text must be opaque from their first visible frame.
use crate::{DARK, LIGHT, Lang, LiteTextApp, Palette};
use eframe::egui;

#[test]
fn long_dropdown_stays_on_screen_and_scrolls_to_last_option() {
    let ctx = egui::Context::default();
    let id = egui::Id::new("long-theme-list-test");
    let render = |events| ctx.run(egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0,420.0))), events,
        ..Default::default()
    }, |ctx| { egui::CentralPanel::default().show(ctx, |ui| {
        crate::dropdown(ui, &DARK, id, "Theme 0".into(), (0..25).map(|i| (format!("Theme {i}"), i)).collect(), 230.0);
    }); });
    render(vec![]); ctx.memory_mut(|m| m.open_popup(id));
    for _ in 0..4 { render(vec![]); }
    let viewport = ctx.data(|d| d.get_temp::<egui::Rect>(id.with("options_viewport"))).unwrap();
    assert!(viewport.top() >= 0.0 && viewport.bottom() <= 420.0);
    assert!((290.0..=301.0).contains(&viewport.height()), "long selector must show multiple rows: {viewport:?}");
    render(vec![egui::Event::PointerMoved(viewport.center()), egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, -2000.0), modifiers: Default::default(),
    }]);
    for _ in 0..30 { render(vec![]); }
    let last = ctx.data(|d| d.get_temp::<egui::Rect>(id.with(("option_rect", "Theme 24")))).unwrap();
    assert!(viewport.intersects(last), "last option must be reachable: {last:?} / {viewport:?}");
}

#[test]
fn general_and_terminal_selectors_share_width_and_centered_values() {
    for pal in [&DARK, &LIGHT] {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = LiteTextApp { show_settings: true, ..Default::default() };
        for _ in 0..3 { draw(&ctx, &mut app, pal, vec![]); }
        let output = draw(&ctx, &mut app, pal, vec![]);
        for (id, label) in [
            ("settings_lang", app.lang.native_name()),
            ("settings_tab_layout", "水平"),
            ("settings_terminal", "系统默认"),
        ] {
            let rect = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(id).with("button_rect"))).unwrap();
            assert_eq!(rect.width(), crate::SETTINGS_SELECT_WIDTH);
            let text_center = output.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == label =>
                    Some(text.pos.x + text.galley.size().x * 0.5),
                _ => None,
            }).unwrap();
            assert!((text_center - rect.center().x).abs() < 0.1, "{id}: {text_center} / {rect:?}");
        }
    }
}

fn draw(
    ctx: &egui::Context,
    app: &mut LiteTextApp,
    pal: &Palette,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 800.0),
            )),
            events,
            ..Default::default()
        },
        |ctx| app.settings_window(ctx, pal),
    )
}

fn click(
    ctx: &egui::Context,
    app: &mut LiteTextApp,
    pal: &Palette,
    pos: egui::Pos2,
) -> egui::FullOutput {
    let mut last = draw(ctx, app, pal, vec![]);
    for pressed in [true, false] {
        last = draw(
            ctx,
            app,
            pal,
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
    last
}

/// Find the language selector's shared-width, 26-point painted background.
fn chip_rect(output: &egui::FullOutput) -> egui::Rect {
    output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Rect(rect)
                if (rect.rect.width() - crate::SETTINGS_SELECT_WIDTH).abs() < 1.0
                    && (rect.rect.height() - 26.0).abs() < 1.0 =>
            {
                Some(rect.rect)
            }
            _ => None,
        })
        .expect("language chip background must be painted")
}

/// Topmost painted rectangle containing `pos`; later shapes paint over earlier ones.
fn fill_at(output: &egui::FullOutput, pos: egui::Pos2, min_height: f32) -> egui::Color32 {
    output
        .shapes
        .iter()
        .rev()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Rect(rect)
                if rect.rect.height() >= min_height && rect.rect.contains(pos) =>
            {
                Some(rect.fill)
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("no background painted at {pos:?}"))
}

fn maybe_option_text(output: &egui::FullOutput, label: &str) -> Option<egui::Pos2> {
    output.shapes.iter().find_map(|shape| match &shape.shape {
        egui::Shape::Text(text) if text.galley.text() == label => Some(text.pos),
        _ => None,
    })
}

fn option_text(output: &egui::FullOutput, label: &str) -> egui::Pos2 {
    output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == label => Some(text.pos),
            _ => None,
        })
        .unwrap_or_else(|| panic!("popup option {label:?} was not painted"))
}

#[test]
fn language_popup_and_selected_row_are_opaque_from_the_first_painted_frame() {
    for pal in [&DARK, &LIGHT] {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = LiteTextApp {
            lang: Lang::ZhCn,
            show_settings: true,
            terminal_installed: Some(vec![]),
            ..Default::default()
        };
        for _ in 0..4 {
            draw(&ctx, &mut app, pal, vec![]);
        }
        let rect = chip_rect(&draw(&ctx, &mut app, pal, vec![]));
        assert_eq!(
            fill_at(&draw(&ctx, &mut app, pal, vec![]), rect.center(), 20.0).a(),
            255
        );
        click(&ctx, &mut app, pal, rect.center());
        // While the list is open the chip keeps an opaque tint, and the list
        // frame/selected row must be opaque as soon as they are first painted
        // instead of fading over the settings text behind them.
        let mut checked_popup = false;
        for _ in 0..6 {
            let output = draw(&ctx, &mut app, pal, vec![]);
            assert_eq!(
                fill_at(&output, rect.center(), 20.0).a(),
                255,
                "open language chip must stay opaque"
            );
            if let Some(pos) = output.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "English" => Some(text.pos),
                _ => None,
            }) {
                assert_eq!(
                    fill_at(&output, pos, 40.0).a(),
                    255,
                    "language popup frame must not be translucent"
                );
                assert_eq!(
                    fill_at(&output, pos, 20.0).a(),
                    255,
                    "selected option row must not be translucent"
                );
                checked_popup = true;
                break;
            }
        }
        assert!(checked_popup, "language popup never painted");
        let english = option_text(&draw(&ctx, &mut app, pal, vec![]), "English");
        click(&ctx, &mut app, pal, english);
        for _ in 0..3 {
            draw(&ctx, &mut app, pal, vec![]);
        }
        assert_eq!(app.lang, Lang::En);
        let output = draw(&ctx, &mut app, pal, vec![]);
        assert!(
            !ctx.memory(|m| m.is_popup_open(egui::Id::new("settings_lang"))),
            "selecting a language must close the list"
        );
        assert_eq!(
            fill_at(&output, rect.center(), 20.0).a(),
            255,
            "closed language chip must be opaque"
        );
    }
}

#[test]
fn font_page_lists_system_fonts_and_applies_the_choice() {
    let ctx = egui::Context::default();
    crate::setup_fonts(&ctx);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("HarmonyOS_Sans_Regular.ttf");
    std::fs::write(
        &path,
        include_bytes!("../assets/fonts/HarmonyOS_Sans_Regular.ttf"),
    )
    .unwrap();
    let mut app = LiteTextApp {
        lang: Lang::ZhCn,
        show_settings: true,
        ..Default::default()
    };
    app.font_scan.seed(vec![
        crate::fonts::SystemFont {
            family: "HarmonyOS Sans".into(),
            path: path.clone(),
            index: 0,
            chinese: true,
        },
        // With a Chinese UI this Latin-only row must not be listed.
        crate::fonts::SystemFont {
            family: "Latin Only".into(),
            path: path.clone(),
            index: 0,
            chinese: false,
        },
    ]);
    let mut output = draw(&ctx, &mut app, &DARK, vec![]);
    for _ in 0..3 {
        output = draw(&ctx, &mut app, &DARK, vec![]);
    }
    let entry = option_text(&output, "工作区字体    ›");
    let _ = click(&ctx, &mut app, &DARK, entry);
    assert!(app.font_page && !app.shortcuts_page && !app.workspace_theme_page);
    let output = draw(&ctx, &mut app, &DARK, vec![]);
    assert!(output.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Text(t)
        if t.galley.text() == "内置默认字体")));
    // A Chinese UI hides faces that cannot set Chinese text.
    let latin_row = ctx.data(|d| {
        d.get_temp::<egui::Rect>(egui::Id::new((
            "font_row",
            "Latin Only".to_owned(),
        )))
    });
    assert!(latin_row.is_none(), "non-Chinese fonts must be filtered out");
    // Selecting the row stores the choice and applies it live.
    let row = ctx
        .data(|d| {
            d.get_temp::<egui::Rect>(egui::Id::new((
                "font_row",
                "HarmonyOS Sans".to_owned(),
            )))
        })
        .expect("the scanned family must be listed");
    click(&ctx, &mut app, &DARK, row.center());
    let choice = app.workspace_font.clone().expect("choice stored");
    assert_eq!(choice.family, "HarmonyOS Sans");
    assert_eq!(choice.path, path);
    // The current choice is painted in its own font: the preview image is a
    // mesh with a non-atlas texture inside the row.
    let output = draw(&ctx, &mut app, &DARK, vec![]);
    let row = ctx
        .data(|d| {
            d.get_temp::<egui::Rect>(egui::Id::new((
                "font_row",
                "HarmonyOS Sans".to_owned(),
            )))
        })
        .unwrap();
    let painted = output.shapes.iter().any(|s| match &s.shape {
        egui::Shape::Mesh(mesh) => {
            mesh.texture_id != egui::TextureId::default()
                && mesh
                    .vertices
                    .iter()
                    .any(|vertex| row.contains(vertex.pos))
        }
        _ => false,
    });
    assert!(
        painted,
        "the family name must be painted with its own font as a preview image"
    );
    // The size row edits the same field the zoom shortcuts adjust.
    let value = ctx
        .data(|d| d.get_temp::<egui::Rect>(egui::Id::new("font_size_value_rect")))
        .expect("the size control must be visible");
    let plus = ctx
        .data(|d| d.get_temp::<egui::Rect>(egui::Id::new("font_size_plus_rect")))
        .unwrap();
    let minus = ctx
        .data(|d| d.get_temp::<egui::Rect>(egui::Id::new("font_size_minus_rect")))
        .unwrap();
    assert!(value.width() > 0.0);
    assert_eq!(app.workspace_font_size, 14.0);
    click(&ctx, &mut app, &DARK, plus.center());
    assert_eq!(app.workspace_font_size, 15.0);
    click(&ctx, &mut app, &DARK, minus.center());
    click(&ctx, &mut app, &DARK, minus.center());
    assert_eq!(app.workspace_font_size, 13.0);
    // The number shown here follows the field, so shortcut changes appear too.
    app.workspace_font_size = 20.0;
    let output = draw(&ctx, &mut app, &DARK, vec![]);
    assert!(output.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Text(t)
        if t.galley.text() == "20")));
    // Clamping matches the shortcut bounds.
    app.workspace_font_size = 10.0;
    let _ = draw(&ctx, &mut app, &DARK, vec![]);
    click(&ctx, &mut app, &DARK, minus.center());
    assert_eq!(app.workspace_font_size, 10.0);
    let reset = ctx
        .data(|d| d.get_temp::<egui::Rect>(egui::Id::new("font_reset_rect")))
        .unwrap();
    click(&ctx, &mut app, &DARK, reset.center());
    assert!(app.workspace_font.is_none());
    // Outside Chinese the full list is available again.
    app.lang = Lang::En;
    let _ = draw(&ctx, &mut app, &DARK, vec![]);
    let latin_row = ctx.data(|d| {
        d.get_temp::<egui::Rect>(egui::Id::new((
            "font_row",
            "Latin Only".to_owned(),
        )))
    });
    assert!(
        latin_row.is_some(),
        "an English UI must list every family"
    );
}

#[test]
fn shortcuts_page_offers_tab_close_modes_below_the_back_button() {
    for pal in [&DARK, &LIGHT] {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = LiteTextApp {
            lang: Lang::ZhCn,
            show_settings: true,
            shortcuts_page: true,
            ..Default::default()
        };
        assert_eq!(app.tab_close_mode, crate::TabCloseMode::DoubleClick);
        let mut output = None;
        for _ in 0..4 {
            output = Some(draw(&ctx, &mut app, pal, vec![]));
        }
        let output = output.unwrap();
        let back = option_text(&output, "‹ 返回系统设置");
        let label = option_text(&output, "关闭标签方式");
        assert!(
            label.y > back.y,
            "the tab-close row belongs below the back button"
        );
        assert_eq!(option_text(&output, "双击关闭").y, label.y);
        let chip = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect)
                    if (rect.rect.width() - 190.0).abs() < 1.0
                        && (rect.rect.height() - 26.0).abs() < 1.0 =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .expect("tab-close chip must be painted");
        click(&ctx, &mut app, pal, chip.center());
        let mut opened = None;
        for _ in 0..4 {
            let output = draw(&ctx, &mut app, pal, vec![]);
            if let Some(pos) = maybe_option_text(&output, "中键关闭") {
                opened = Some(pos);
                break;
            }
        }
        let option = opened.expect("the tab-close list must offer middle-click");
        click(&ctx, &mut app, pal, option + egui::vec2(4.0, 4.0));
        for _ in 0..3 {
            draw(&ctx, &mut app, pal, vec![]);
        }
        assert_eq!(app.tab_close_mode, crate::TabCloseMode::MiddleClick);
        let output = draw(&ctx, &mut app, pal, vec![]);
        assert!(!ctx.memory(|m| m.is_popup_open(egui::Id::new("settings_tab_close_mode"))));
        option_text(&output, "中键关闭");
    }
}

#[test]
fn scratch_open_folder_button_opens_the_configured_directory() {
    let ctx = egui::Context::default();
    crate::setup_fonts(&ctx);
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("drafts");
    let mut app = LiteTextApp {
        lang: Lang::ZhCn,
        show_settings: true,
        terminal_installed: Some(vec![]),
        scratch_dir: Some(root.clone()),
        ..Default::default()
    };
    for _ in 0..4 {
        let _ = draw(&ctx, &mut app, &DARK, vec![]);
    }
    let view = ctx
        .data(|d| d.get_temp::<egui::Rect>(egui::Id::new("settings_home_scroll_rect")))
        .expect("settings scroll area");
    let mut open_rect = None;
    // The scratch card sits below the fold; scroll a little at a time so the
    // frame that first paints it is also the frame that records its rect.
    for _ in 0..80 {
        let _ = draw(
            &ctx,
            &mut app,
            &DARK,
            vec![
                egui::Event::PointerMoved(view.center()),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -100.0),
                    modifiers: Default::default(),
                },
            ],
        );
        open_rect = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new("scratch_open_folder_rect")));
        if open_rect.is_some() {
            break;
        }
    }
    open_rect.expect("the scratch folder buttons must become reachable");
    // Wheel scrolling keeps settling for a few frames; wait for the layout to
    // stop before turning the recorded rect into a click target.
    for _ in 0..4 {
        let _ = draw(&ctx, &mut app, &DARK, vec![]);
    }
    let open_rect = ctx
        .data(|d| d.get_temp::<egui::Rect>(egui::Id::new("scratch_open_folder_rect")))
        .expect("the scratch folder buttons stay painted after scrolling");
    assert!(open_rect.is_positive(), "the button needs a real hit area: {open_rect:?}");
    assert!(!root.exists(), "the configured folder is created on demand");
    click(&ctx, &mut app, &DARK, open_rect.center());
    assert!(
        root.exists(),
        "the open-folder button must create and reveal the configured folder"
    );
    assert!(app.io_error.is_none(), "{:?}", app.io_error);
}

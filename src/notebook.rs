//! Display-only rules for numbered workspaces. Reserve a paint slot before the
//! editor so rules stay behind glyphs, carets, selections and marker backgrounds.
use crate::workspace_theme::{Render, Texture};
use eframe::egui;

pub(crate) fn rule_color(background: egui::Color32, text: egui::Color32) -> egui::Color32 {
    let mix = |bg: u8, fg: u8| (bg as f32 + (fg as f32 - bg as f32) * 0.08).round() as u8;
    egui::Color32::from_rgb(mix(background.r(), text.r()), mix(background.g(), text.g()), mix(background.b(), text.b()))
}

pub(crate) fn slot(painter: &egui::Painter, render: Option<&Render>) -> Option<egui::layers::ShapeIdx> {
    render.filter(|render| render.texture == Texture::Notebook)
        .map(|_| painter.add(egui::Shape::Noop))
}

pub(crate) fn paint_rows(
    painter: &egui::Painter, slot: Option<egui::layers::ShapeIdx>, render: Option<&Render>,
    galley: &egui::Galley, pos: egui::Pos2, viewport: egui::Rect,
) {
    let (Some(slot), Some(render)) = (slot, render) else { return; };
    let ppp = painter.pixels_per_point();
    let stroke = egui::Stroke::new(1.0_f32 / ppp, rule_color(render.colors.background, render.colors.text));
    let mut shapes = Vec::new();
    for row in &galley.rows {
        // Folded rows have no height; don't stack rules on the fold header.
        if row.rect.height() < 1.0 { continue; }
        let y = painter.round_to_pixel_center(pos.y + row.rect.bottom() - 0.5 / ppp);
        if y >= viewport.top() && y < viewport.bottom() {
            shapes.push(egui::Shape::line_segment([
                egui::pos2(viewport.left(), y), egui::pos2(viewport.right(), y),
            ], stroke));
        }
    }
    painter.set(slot, egui::Shape::Vec(shapes));
}

pub(crate) fn paint_hex_row(painter: &egui::Painter, render: Option<&Render>, rect: egui::Rect) {
    let Some(render) = render.filter(|r| r.texture == Texture::Notebook) else { return; };
    let ppp = painter.pixels_per_point();
    let y = painter.round_to_pixel_center(rect.bottom() - 0.5 / ppp);
    painter.hline(rect.x_range(), y, egui::Stroke::new(1.0_f32 / ppp,
        rule_color(render.colors.background, render.colors.text)));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notebook_rules_follow_visible_rows_and_are_painted_behind_text() {
        for notebook in [false, true] {
            let ctx = egui::Context::default();
            let theme = crate::workspace_theme::WorkspaceTheme {
                texture: if notebook { Texture::Notebook } else { Texture::Default },
                ..Default::default()
            };
            let mut app = crate::LiteTextApp::default();
            app.new_document();
            let file = app.active_mut().unwrap();
            file.draft_text = Some("under_score.\n\nlast.".into()); file.sync_draft();
            crate::workspace_theme::configure(file, &theme, true);
            let expected = rule_color(theme.colors(true).background, theme.colors(true).text);
            for _ in 0..3 {
                let output = ctx.run(egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
                    ..Default::default()
                }, |ctx| { egui::CentralPanel::default().show(ctx, |ui| {
                    crate::draft_editor(ui, app.active_mut().unwrap(), &crate::DARK, false, crate::Lang::En);
                }); });
                let lines: Vec<_> = output.shapes.iter().enumerate().filter_map(|(index, clipped)| {
                    if let egui::Shape::Vec(shapes) = &clipped.shape {
                        Some((index, shapes.iter().filter_map(|shape| match shape {
                            egui::Shape::LineSegment { points, stroke } if stroke.color == expected => Some(*points),
                            _ => None,
                        }).collect::<Vec<_>>()))
                    } else { None }
                }).filter(|(_, lines)| !lines.is_empty()).collect();
                if notebook {
                    assert_eq!(lines.len(), 1);
                    assert_eq!(lines[0].1.len(), 3, "blank lines also get a rule");
                    assert!(lines[0].1.windows(2).all(|rows| rows[0][0].y < rows[1][0].y));
                    let text_index = output.shapes.iter().position(|shape| matches!(&shape.shape,
                        egui::Shape::Text(text) if text.galley.text().contains("under_score."))).unwrap();
                    assert!(lines[0].0 < text_index, "rules stay behind punctuation and selection");
                } else { assert!(lines.is_empty()); }
            }
        }
    }

    #[test]
    fn live_markdown_has_no_rules_and_large_editor_only_paints_viewport_rows() {
        for live in [false, true] {
            let ctx = egui::Context::default();
            crate::setup_fonts(&ctx);
            let theme = crate::workspace_theme::WorkspaceTheme { texture: Texture::Notebook, ..Default::default() };
            let mut app = crate::LiteTextApp::default(); app.new_document();
            let file = app.active_mut().unwrap();
            file.draft_text = Some(if live { "# Heading\n\nbody _ .".into() } else { "row _ .\n".repeat(150_000) });
            file.sync_draft();
            if live { file.language_override = Some("Markdown".into()); file.markdown_mode = crate::markdown::Mode::Live; }
            crate::workspace_theme::configure(file, &theme, true);
            let expected = rule_color(theme.colors(true).background, theme.colors(true).text);
            for _ in 0..3 {
                let output = ctx.run(egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
                    ..Default::default()
                }, |ctx| { egui::CentralPanel::default().show(ctx, |ui| {
                    crate::draft_editor(ui, app.active_mut().unwrap(), &crate::DARK, false, crate::Lang::En);
                }); });
                let count = output.shapes.iter().filter_map(|shape| match &shape.shape {
                    egui::Shape::Vec(shapes) => Some(shapes.iter().filter(|shape| matches!(shape,
                        egui::Shape::LineSegment { stroke, .. } if stroke.color == expected)).count()),
                    _ => None,
                }).sum::<usize>();
                if live { assert_eq!(count, 0); }
                else { assert!(count > 10 && count < 100, "only visible rules: {count}"); }
            }
        }
    }

    #[test]
    fn rules_are_much_fainter_than_text_in_light_and_dark_palettes() {
        for (bg, text) in [(egui::Color32::from_gray(30), egui::Color32::from_gray(212)),
            (egui::Color32::WHITE, egui::Color32::BLACK)] {
            let rule = rule_color(bg, text);
            assert!(rule.r().abs_diff(bg.r()) <= 21);
            assert!(rule.r().abs_diff(text.r()) > 150);
        }
    }
}

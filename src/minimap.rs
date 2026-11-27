//! A transparent, bounded-cost document overview. The caller owns the overlay rectangle
//! (exclude horizontal/vertical scrollbars) and invalidates the preview after edits.
use crate::Palette;
use eframe::egui::{self, Color32, Id, Pos2, Rect, Sense, Stroke, Vec2};

pub const WIDTH: f32 = 84.0;
const MAX_ROWS: usize = 140;
const MINI_FONT_SIZE: f32 = 4.0;
const MINI_ROW_HEIGHT: f32 = 5.0;

/// Cheap metadata for one source line; no document or line string is retained.
/// `width` is the non-indentation content width, in character columns.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MiniLine {
    pub indent: usize,
    pub width: usize,
    /// Bounded actual characters for a readable miniature (never a whole line).
    pub text: String,
    /// One foreground per cached character; absent for unhighlighted samples.
    pub colors: Vec<Color32>,
}
impl MiniLine {
    pub fn from_chars(chars: impl Iterator<Item = char>) -> Self {
        let mut indent = 0; let mut text = String::new();
        for ch in chars.take(160) {
            if text.is_empty() && ch.is_whitespace() { indent += 1; }
            else if text.chars().count() < 64 { text.push(if ch.is_control() { ' ' } else { ch }); }
            else { break; }
        }
        Self { indent, width: text.chars().count(), text, colors: Vec::new() }
    }
    pub fn from_glyphs(row: &egui::epaint::text::Row, job: &egui::text::LayoutJob, fallback: Color32) -> Self {
        let mut sample = Self::default();
        for glyph in row.glyphs.iter().take(160) {
            if sample.text.is_empty() && glyph.chr.is_whitespace() { sample.indent += 1; continue; }
            if sample.colors.len() == 64 { break; }
            let color = job.sections.get(glyph.section_index as usize).map_or(fallback, |s| s.format.color);
            sample.text.push(if color == Color32::TRANSPARENT { '▪' } else if glyph.chr.is_control() { ' ' } else { glyph.chr });
            sample.colors.push(if color == Color32::TRANSPARENT { fallback } else { color });
        }
        sample.width = sample.colors.len(); sample
    }
}

#[derive(Clone, Default)]
pub struct Preview {
    cache: Vec<MiniLine>,
    cache_key: Option<(Id, usize, usize, usize)>,
    grab_offset: Option<f32>,
}

impl Preview {
    /// Call after source text changes (including same-line-count edits).
    pub fn invalidate(&mut self) {
        self.cache_key = None;
    }

    /// Paint a non-modal overlay and return an absolute requested scroll offset.
    /// Samples at most 140 visual rows from a moving, at-most-ten-screen local
    /// window on a cache miss, zero on a cache hit. `sample` receives absolute
    /// visual-row indices in the whole document, not offsets within that window.
    /// Use a different `id` per document and call `invalidate` after content edits.
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        id: Id,
        rect: Rect,
        line_count: usize,
        scroll_y: f32,
        content_height: f32,
        viewport_height: f32,
        pal: &Palette,
        mut sample: impl FnMut(usize) -> MiniLine,
    ) -> Option<f32> {
        let mut rect = rect
            .intersect(ui.clip_rect())
            .intersect(ui.ctx().screen_rect());
        // A short document is a miniature, not a stretched full-height track.
        // TextEdit may fill its viewport even for a single row, so use the
        // actual row count when there is no vertical overflow.
        if content_height <= viewport_height + 1.0 {
            let height = (line_count.max(1) as f32 * MINI_ROW_HEIGHT + 8.0).min(rect.height());
            rect.max.y = rect.top() + height;
        }
        if !rect.is_finite() || rect.width() <= 0.0 || rect.height() <= 0.0 {
            self.grab_offset = None;
            return None;
        }
        let sample_geometry = Geometry::new(rect, scroll_y, content_height, viewport_height);
        let span_rows = if finite_nonnegative(content_height) > 0.0 {
            ((line_count as f64 * sample_geometry.span_height as f64
                / finite_nonnegative(content_height) as f64).ceil() as usize).min(line_count)
        } else {
            line_count
        };
        let span_travel = finite_nonnegative(content_height) - sample_geometry.span_height;
        let start = if span_travel > 0.0 {
            ((line_count.saturating_sub(span_rows) as f64
                * (sample_geometry.span_start / span_travel) as f64).floor() as usize)
                .min(line_count.saturating_sub(span_rows))
        } else {
            0
        };
        let end = start + span_rows;
        ui.ctx().data_mut(|data| data.insert_temp(id.with("minimap_sample_range"), start..end));
        let persistent = ui.ctx().data(|d| d.get_temp::<bool>(id.with("persistent"))).unwrap_or(false);
        let pointer_near = persistent || ui.input(|input| input.pointer.hover_pos().is_some_and(|pos|
            rect.expand2(Vec2::new(18.0, 0.0)).contains(pos)));
        let dragging = self.grab_offset.is_some() && ui.input(|input| input.pointer.primary_down());
        let progress = ui.ctx().animate_bool_with_time(id.with("minimap_reveal"), pointer_near || dragging, 0.18);
        ui.ctx().data_mut(|data| data.insert_temp(id.with("minimap_visibility"), progress));
        let clip = rect;
        let rect = rect.translate(Vec2::new(rect.width() * (1.0 - progress), 0.0));
        let geometry = Geometry::new(rect, scroll_y, content_height, viewport_height);
        ui.ctx().data_mut(|data| {
            data.insert_temp(id.with("minimap_rect"), clip);
            data.insert_temp(id.with("minimap_thumb"), geometry.thumb);
        });
        if progress <= 0.001 { self.grab_offset = None; return None; }
        let rows = span_rows.min(MAX_ROWS).min((rect.height() / MINI_ROW_HEIGHT).floor().max(1.0) as usize);
        let key = (id, start, end, rows);
        if self.cache_key != Some(key) {
            self.cache.clear();
            for row in 0..rows {
                let index = if rows <= 1 {
                    start
                } else {
                    start + ((row as u128 * span_rows.saturating_sub(1) as u128) / (rows - 1) as u128)
                        as usize
                };
                self.cache.push(sample(index));
            }
            self.cache_key = Some(key);
        }
        let mut target = None;
        // A separate layer wins pointer hit testing over a TextEdit painted earlier,
        // without a modal layer, keyboard focus, or interception outside `rect`.
        egui::Area::new(id.with("minimap_overlay"))
            .order(egui::Order::Foreground)
            .fixed_pos(rect.min)
            .default_size(rect.size())
            .constrain(false)
            .movable(false)
            .sense(Sense::hover())
            .show(ui.ctx(), |overlay| {
                overlay.set_clip_rect(clip);
                let (area, response) = overlay
                    .allocate_exact_size(rect.size(), Sense::click_and_drag() - Sense::FOCUSABLE);
                let active = response.hovered() || response.is_pointer_button_down_on();
                if !overlay.input(|i| i.pointer.primary_down()) {
                    self.grab_offset = None;
                }
                if response.is_pointer_button_down_on()
                    && line_count > 0
                    && geometry.max_scroll > 0.0
                    && let Some(pointer) = response.interact_pointer_pos()
                {
                        let offset = *self.grab_offset.get_or_insert_with(|| {
                            // Use press origin so crossing the drag threshold does not
                            // shift the grabbed point within the viewport box.
                            let origin = overlay
                                .input(|i| i.pointer.press_origin())
                                .unwrap_or(pointer);
                            if geometry.thumb.contains(origin) {
                                origin.y - geometry.thumb.top()
                            } else {
                                geometry.thumb.height() * 0.5
                            }
                        });
                        target = Some(geometry.scroll_at(pointer.y, offset));
                }
                let painter = overlay.painter().with_clip_rect(area.intersect(clip));
                // Light canvases need stronger translucent ink; keep dark previews subdued.
                // Apply the same opacity to syntax colors without changing their hue.
                let text_opacity = if pal.dark { 24.0 } else { 100.0 };
                let ink = alpha(pal.text_main, (text_opacity * progress) as u8);
                let column = 2.2_f32;
                let row_height = rect.height() / self.cache.len().max(1) as f32;
                for (index, line) in self.cache.iter().enumerate() {
                    if line.width == 0 {
                        continue;
                    }
                    let x = rect.left() + 4.0 + (line.indent.min(28) as f32 * column);
                    let right = (x + line.width.min(64) as f32 * column).min(rect.right() - 3.0);
                    let y = rect.top() + (index as f32 + 0.5) * row_height;
                    if !line.text.is_empty() {
                        let mut job = egui::text::LayoutJob::default();
                        job.wrap.max_width = f32::INFINITY;
                        for (index, ch) in line.text.chars().enumerate() {
                            let color = line.colors.get(index).copied().unwrap_or(pal.text_main);
                            job.append(&ch.to_string(), 0.0, egui::TextFormat {
                                font_id: egui::FontId::monospace(MINI_FONT_SIZE),
                                color: alpha(color, (text_opacity * progress) as u8), ..Default::default()
                            });
                        }
                        let galley = painter.layout_job(job);
                        painter.galley(Pos2::new(x, y - MINI_FONT_SIZE * 0.5), galley, ink);
                    } else if right > x {
                        painter.line_segment(
                            [Pos2::new(x, y), Pos2::new(right, y)],
                            Stroke::new(row_height.min(1.3), ink),
                        );
                    }
                }
                if line_count > 0 {
                    painter.rect_filled(
                        geometry.thumb,
                        2.0,
                        alpha(pal.text_faint, ((if !pal.dark { 20.0 } else if active { 12.0 } else { 7.0 }) * progress) as u8),
                    );
                    painter.rect_stroke(
                        geometry.thumb,
                        2.0,
                        Stroke::new(1.0_f32, alpha(pal.text_faint, ((if pal.dark { 38.0 } else { 80.0 }) * progress) as u8)),
                        egui::StrokeKind::Inside,
                    );
                }
            });
        target
    }
}

fn alpha(color: Color32, opacity: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), opacity)
}

#[derive(Clone, Copy)]
struct Geometry {
    track: Rect,
    thumb: Rect,
    max_scroll: f32,
    span_height: f32,
    span_start: f32,
}

impl Geometry {
    fn new(track: Rect, scroll: f32, content: f32, viewport: f32) -> Self {
        let content = finite_nonnegative(content);
        let viewport = finite_nonnegative(viewport);
        let max_scroll = (content - viewport).max(0.0);
        let span_height = content.min(viewport * 10.0);
        let ratio = if span_height > 0.0 {
            (viewport / span_height).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let height = (track.height() * ratio).max(8.0).min(track.height());
        let progress = if max_scroll > 0.0 {
            finite_nonnegative(scroll).min(max_scroll) / max_scroll
        } else {
            0.0
        };
        let top = track.top() + progress * (track.height() - height);
        Self {
            track,
            thumb: Rect::from_min_size(
                Pos2::new(track.left(), top),
                Vec2::new(track.width(), height),
            ),
            max_scroll,
            span_height,
            // This keeps the current viewport at the same relative position in
            // the moving local sample as its thumb on the global track.
            span_start: progress * (content - span_height).max(0.0),
        }
    }

    fn scroll_at(self, pointer_y: f32, grab_offset: f32) -> f32 {
        let travel = self.track.height() - self.thumb.height();
        if travel <= 0.0 {
            return 0.0;
        }
        ((pointer_y - grab_offset - self.track.top()) / travel).clamp(0.0, 1.0) * self.max_scroll
    }
}

fn finite_nonnegative(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> Rect {
        Rect::from_min_size(Pos2::new(200.0, 20.0), Vec2::new(WIDTH, 200.0))
    }

    #[test]
    fn miniature_reuses_syntax_foregrounds_without_revealing_concealed_text() {
        let ctx = egui::Context::default();
        let _ = ctx.run(Default::default(), |ctx| { egui::CentralPanel::default().show(ctx, |ui| {
            let mut job = egui::text::LayoutJob::default();
            job.append("let ", 0.0, egui::TextFormat::simple(egui::FontId::monospace(14.0), egui::Color32::RED));
            job.append("secret", 0.0, egui::TextFormat::simple(egui::FontId::monospace(14.0), Color32::TRANSPARENT));
            let galley = ui.fonts(|fonts| fonts.layout_job(job));
            let sample = MiniLine::from_glyphs(&galley.rows[0], &galley.job, Color32::WHITE);
            assert_eq!(sample.text, "let ▪▪▪▪▪▪");
            assert_eq!(sample.colors[0], Color32::RED);
            assert_eq!(sample.colors[4], Color32::WHITE);
        }); });
    }

    fn assert_theme_text_shapes(pal: &Palette, text_alpha: u8, outline_alpha: u8, fill_alpha: u8) {
        let ctx = egui::Context::default();
        let id = Id::new("theme-minimap");
        ctx.data_mut(|data| data.insert_temp(id.with("persistent"), true));
        let mut preview = Preview::default();
        let syntax = [Color32::from_rgb(190, 60, 90), Color32::from_rgb(40, 100, 190)];
        let mut draw = |hover: bool| ctx.run(egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(500.0, 400.0))),
            time: Some(ctx.input(|input| input.time) + 0.05),
            events: vec![egui::Event::PointerMoved(if hover { rect().center() } else { Pos2::ZERO })],
            ..Default::default()
        }, |ctx| { egui::CentralPanel::default().show(ctx, |ui| {
            let mut job = egui::text::LayoutJob::default();
            for (ch, color) in ["l", "e"].into_iter().zip(syntax) {
                job.append(ch, 0.0, egui::TextFormat::simple(egui::FontId::monospace(14.0), color));
            }
            let galley = ui.fonts(|fonts| fonts.layout_job(job));
            let highlighted = MiniLine::from_glyphs(&galley.rows[0], &galley.job, pal.text_main);
            preview.show(ui, id, rect(), 1000, 400.0, 1000.0, 200.0, pal, |index| {
                if index % 2 == 0 { highlighted.clone() } else { MiniLine::from_chars("plain".chars()) }
            });
        }); });
        for _ in 0..8 { let _ = draw(false); }
        for hover in [false, true] {
            // Fully revealed persistent previews must not brighten abruptly on hover.
            let output = draw(hover);
            let texts: Vec<_> = output.shapes.iter().filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text),
                _ => None,
            }).collect();
            assert!(!texts.is_empty(), "exercise actual miniature TextShapes");
            let mut syntax_count = 0;
            let mut plain_count = 0;
            let mut vertex_count = 0;
            let mut opaque_vertices = 0;
            for text in texts {
                assert_eq!(text.fallback_color, alpha(pal.text_main, text_alpha));
                let expected: Vec<_> = match text.galley.text() {
                    "le" => { syntax_count += 1; syntax.map(|color| alpha(color, text_alpha)).to_vec() }
                    "plain" => { plain_count += 1; vec![alpha(pal.text_main, text_alpha)] }
                    other => panic!("unexpected miniature text: {other}"),
                };
                assert_eq!(text.galley.job.sections.iter().map(|section| section.format.color).collect::<Vec<_>>(),
                    if expected.len() == 1 { vec![expected[0]; 5] } else { expected.clone() });
                for vertex in text.galley.rows.iter().flat_map(|row| &row.visuals.mesh.vertices) {
                    assert!(expected.contains(&vertex.color), "real glyph mesh must retain tinted syntax colors");
                    vertex_count += 1;
                    opaque_vertices += usize::from(vertex.color.a() == 255);
                }
            }
            assert!(syntax_count > 0 && plain_count > 0);
            assert!(vertex_count > 0, "measure rendered glyphs, not only layout metadata");
            let opaque_ratio = opaque_vertices as f32 / vertex_count as f32;
            assert_eq!(opaque_ratio, 0.0, "minimap glyphs must remain translucent");
            let thumb_rect = Geometry::new(rect(), 400.0, 1000.0, 200.0).thumb;
            let thumb_shapes: Vec<_> = output.shapes.iter().filter_map(|shape| match &shape.shape {
                egui::Shape::Rect(shape) if shape.rect == thumb_rect => Some(shape),
                _ => None,
            }).collect();
            let expected_fill = if pal.dark && hover { 12 } else { fill_alpha };
            assert!(thumb_shapes.iter().any(|shape| shape.fill == alpha(pal.text_faint, expected_fill)));
            assert!(thumb_shapes.iter().any(|shape| shape.stroke.color == alpha(pal.text_faint, outline_alpha)));
            assert!(thumb_shapes.iter().all(|shape| shape.fill.a() < 255 && shape.stroke.color.a() < 255));
        }
    }

    #[test]
    fn light_text_shapes_preserve_syntax_hues_with_visible_translucent_ink_and_thumb() {
        assert_theme_text_shapes(&crate::LIGHT, 100, 80, 20);
    }

    #[test]
    fn dark_text_shapes_keep_existing_low_opacity_and_translucent_thumb() {
        // Hover only changes the existing dark fill from 7 to 12.
        assert_theme_text_shapes(&crate::DARK, 24, 38, 7);
    }

    #[test]
    fn miniature_keeps_bounded_real_characters_and_indentation() {
        let sample = MiniLine::from_chars("    let 中文 = example();".chars());
        assert_eq!(sample.indent, 4);
        assert_eq!(sample.text, "let 中文 = example();");
        assert_eq!(MiniLine::from_chars("x".repeat(10000).chars()).text.len(), 64);
    }

    #[test]
    fn single_screen_content_keeps_compact_preview_height() {
        let ctx = egui::Context::default();
        let mut preview = Preview::default(); let id = Id::new("short-minimap");
        let _ = ctx.run(egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0,600.0))), ..Default::default()
        }, |ctx| { egui::CentralPanel::default().show(ctx, |ui| {
            preview.show(ui, id, Rect::from_min_size(Pos2::new(700.0,10.0), Vec2::new(WIDTH,580.0)),
                4, 0.0, 580.0, 580.0, &crate::DARK, |_| MiniLine { indent: 0, width: 12, text: String::new(), colors: Vec::new() });
        }); });
        let rect = ctx.data(|d| d.get_temp::<Rect>(id.with("minimap_rect"))).unwrap();
        assert_eq!(rect.height(), 28.0);
        assert_eq!(rect.top(), 10.0);
    }

    #[test]
    fn thumb_proportion_clamping_and_empty() {
        let g = Geometry::new(rect(), 400.0, 1000.0, 200.0);
        assert_eq!(g.thumb.height(), 40.0);
        assert_eq!(g.thumb.top(), 100.0);
        assert_eq!(g.scroll_at(-100.0, 20.0), 0.0);
        assert_eq!(g.scroll_at(1000.0, 20.0), 800.0);
        for content in [0.0, 100.0, f32::NAN] {
            let g = Geometry::new(rect(), f32::INFINITY, content, 200.0);
            assert_eq!(g.thumb, rect());
            assert_eq!(g.scroll_at(100.0, 0.0), 0.0);
        }
        assert_eq!(Geometry::new(rect(), 0.0, 1e9, 200.0).thumb.height(), 20.0);
    }

    fn frame(
        ctx: &egui::Context,
        preview: &mut Preview,
        scroll: f32,
        events: Vec<egui::Event>,
        count: usize,
        samples: &mut usize,
    ) -> Option<f32> {
        frame_output(ctx, preview, scroll, events, count, samples).0
    }

    fn frame_output(
        ctx: &egui::Context,
        preview: &mut Preview,
        scroll: f32,
        events: Vec<egui::Event>,
        count: usize,
        samples: &mut usize,
    ) -> (Option<f32>, egui::FullOutput) {
        let mut result = None;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(500.0, 400.0))),
                time: Some(ctx.input(|input| input.time) + 0.05),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    result = preview.show(
                        ui,
                        Id::new("test"),
                        rect(),
                        count,
                        scroll,
                        1000.0,
                        200.0,
                        &crate::DARK,
                        |_| {
                            *samples += 1;
                            MiniLine {
                                indent: 4,
                                width: 40,
                                text: String::new(),
                                colors: Vec::new(),
                            }
                        },
                    );
                });
            },
        );
        (result, output)
    }

    fn reveal(ctx: &egui::Context, preview: &mut Preview, count: usize, samples: &mut usize) {
        for _ in 0..8 {
            frame(ctx, preview, 400.0, vec![egui::Event::PointerMoved(rect().center())], count, samples);
        }
        assert_eq!(visibility(ctx), 1.0);
    }

    fn visibility(ctx: &egui::Context) -> f32 {
        ctx.data(|data| data.get_temp(Id::new("test").with("minimap_visibility"))).unwrap()
    }

    fn button(pos: Pos2, pressed: bool) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            },
        ]
    }

    #[test]
    fn render_click_drag_preserves_grab_and_cache_is_bounded() {
        let ctx = egui::Context::default();
        let mut preview = Preview::default();
        let mut samples = 0;
        for _ in 0..3 {
            frame(&ctx, &mut preview, 400.0, vec![], 1_000_000, &mut samples);
        }
        assert_eq!(samples, 0, "idle frames must not sample the document");
        assert!(preview.cache.is_empty());
        assert!(preview.cache_key.is_none());
        reveal(&ctx, &mut preview, 1_000_000, &mut samples);
        assert_eq!(samples, (rect().height() / MINI_ROW_HEIGHT) as usize);
        // Thumb is y=100..140. Grab seven points below its top.
        let pos = Pos2::new(240.0, 107.0);
        let first = frame(
            &ctx,
            &mut preview,
            400.0,
            button(pos, true),
            1_000_000,
            &mut samples,
        )
        .unwrap();
        assert!((first - 400.0).abs() < 0.01);
        let moved = frame(
            &ctx,
            &mut preview,
            400.0,
            vec![egui::Event::PointerMoved(pos + Vec2::new(0.0, 20.0))],
            1_000_000,
            &mut samples,
        )
        .unwrap();
        assert!((moved - 500.0).abs() < 0.01);
        frame(
            &ctx,
            &mut preview,
            moved,
            button(pos, false),
            1_000_000,
            &mut samples,
        );
        let target = frame(
            &ctx,
            &mut preview,
            0.0,
            button(Pos2::new(240.0, 180.0), true),
            1_000_000,
            &mut samples,
        )
        .unwrap();
        assert!(
            (target - 700.0).abs() < 0.01,
            "outside click centers viewport"
        );
        assert_eq!(samples, (rect().height() / MINI_ROW_HEIGHT) as usize);
        preview.invalidate();
        frame(&ctx, &mut preview, 0.0, vec![], 1_000_000, &mut samples);
        assert_eq!(samples, (rect().height() / MINI_ROW_HEIGHT) as usize * 2);
    }

    #[test]
    fn million_rows_use_ten_screen_window_and_global_drag() {
        let ctx = egui::Context::default();
        let id = Id::new("million-row-minimap");
        let mut preview = Preview::default();
        let mut sampled = Vec::new();
        let count = 1_000_000;
        let content = count as f32 * 20.0;
        let viewport = 200.0;
        let max_scroll = content - viewport;
        let mut run = |scroll, events| {
            let mut target = None;
            let _ = ctx.run(egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(500.0, 400.0))),
                time: Some(ctx.input(|input| input.time) + 0.05),
                events,
                ..Default::default()
            }, |ctx| { egui::CentralPanel::default().show(ctx, |ui| {
                target = preview.show(ui, id, rect(), count, scroll, content, viewport,
                    &crate::DARK, |index| {
                        sampled.push(index);
                        MiniLine::default()
                    });
            }); });
            let range = ctx.data(|data| data.get_temp::<std::ops::Range<usize>>(id.with("minimap_sample_range"))).unwrap();
            let thumb = ctx.data(|data| data.get_temp::<Rect>(id.with("minimap_thumb"))).unwrap();
            (target, range, thumb)
        };
        let (_, first, thumb) = run(0.0, vec![]);
        assert_eq!(first, 0..100);
        assert_eq!(thumb.height(), 20.0);
        let (_, middle, middle_thumb) = run(max_scroll * 0.5, vec![]);
        assert!(middle.start > first.start);
        assert_eq!(middle.len(), first.len());
        assert_eq!(middle_thumb.height(), thumb.height());
        let (_, end, end_thumb) = run(max_scroll, vec![]);
        assert_eq!(end.end, count);
        assert_eq!(end.len(), 10 * (viewport / 20.0) as usize);
        assert_eq!(end_thumb.bottom(), rect().bottom());
        // Real egui pointer capture: retain the grab offset and map the whole
        // track, including positions beyond its boundaries, to global scroll.
        for _ in 0..8 {
            run(0.0, vec![egui::Event::PointerMoved(rect().center())]);
        }
        let pos = Pos2::new(240.0, rect().top() + 7.0);
        assert_eq!(run(0.0, button(pos, true)).0, Some(0.0));
        let dragged = run(0.0, vec![egui::Event::PointerMoved(Pos2::new(pos.x, 117.0))]).0.unwrap();
        let (_, moving, moving_thumb) = run(dragged, vec![]);
        assert!(moving.start > first.start && moving.start < end.start);
        assert_eq!(moving.len(), first.len());
        assert_eq!(moving_thumb.height(), thumb.height());
        assert!((moving_thumb.top() - 110.0).abs() < 0.1);
        assert_eq!(run(dragged, vec![egui::Event::PointerMoved(Pos2::new(pos.x, 500.0))]).0, Some(max_scroll));
        let (_, bottom, _) = run(max_scroll, vec![]);
        assert_eq!(bottom.end, count);
        assert_eq!(run(max_scroll, vec![egui::Event::PointerMoved(Pos2::new(pos.x, 0.0))]).0, Some(0.0));
        assert_eq!(sampled.last(), Some(&(count - 1)));
        assert!(sampled.iter().all(|&index| index < count));
        assert_eq!(Geometry::new(rect(), 0.0, 1e12, viewport).thumb.height(), thumb.height());
    }

    #[test]
    fn minimap_is_hidden_idle_reveals_and_leaves_without_accent_or_opaque_background() {
        let ctx = egui::Context::default();
        let mut preview = Preview::default();
        let mut samples = 0;
        let (_, idle) = frame_output(&ctx, &mut preview, 400.0, vec![], 1000, &mut samples);
        assert_eq!(visibility(&ctx), 0.0);
        assert_eq!(samples, 0, "hidden preview must not warm its cache");
        assert!(!idle.shapes.iter().any(|shape| matches!(shape.shape, egui::Shape::LineSegment { .. })));
        let track = ctx.data(|data| data.get_temp::<Rect>(Id::new("test").with("minimap_rect"))).unwrap();
        frame(&ctx, &mut preview, 400.0, vec![egui::Event::PointerMoved(rect().center())], 1000, &mut samples);
        frame(&ctx, &mut preview, 400.0, vec![], 1000, &mut samples);
        assert!(visibility(&ctx) > 0.0 && visibility(&ctx) < 1.0);
        assert_eq!(samples, (rect().height() / MINI_ROW_HEIGHT) as usize,
            "sampling starts during hover reveal, not only once fully visible");
        reveal(&ctx, &mut preview, 1000, &mut samples);
        let (_, visible) = frame_output(&ctx, &mut preview, 400.0, vec![], 1000, &mut samples);
        assert_eq!(ctx.data(|data| data.get_temp::<Rect>(Id::new("test").with("minimap_rect"))).unwrap(), track);
        let strokes: Vec<_> = visible.shapes.iter().filter_map(|shape| match &shape.shape {
            egui::Shape::LineSegment { stroke, .. } => Some(stroke),
            _ => None,
        }).collect();
        assert_eq!(strokes.len(), (rect().height() / MINI_ROW_HEIGHT) as usize);
        assert!(strokes.iter().all(|stroke| stroke.color == alpha(crate::DARK.text_main, 24)));
        let thumb = visible.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Rect(shape) if shape.rect == Geometry::new(rect(), 400.0, 1000.0, 200.0).thumb
                && shape.stroke.width > 0.0 => Some(shape),
            _ => None,
        }).expect("viewport outline");
        assert_eq!(thumb.stroke.color, alpha(crate::DARK.text_faint, 38));
        assert!(visible.shapes.iter().filter_map(|shape| match &shape.shape {
            egui::Shape::Rect(shape) if shape.rect.intersects(rect()) => Some(shape),
            _ => None,
        }).all(|shape| shape.fill.a() <= 12 || !rect().contains_rect(shape.rect)));
        frame(&ctx, &mut preview, 400.0, vec![egui::Event::PointerGone], 1000, &mut samples);
        frame(&ctx, &mut preview, 400.0, vec![], 1000, &mut samples);
        assert!(visibility(&ctx) > 0.0 && visibility(&ctx) < 1.0);
        for _ in 0..8 { frame(&ctx, &mut preview, 400.0, vec![], 1000, &mut samples); }
        let (_, hidden) = frame_output(&ctx, &mut preview, 400.0, vec![], 1000, &mut samples);
        assert_eq!(visibility(&ctx), 0.0);
        assert!(!hidden.shapes.iter().any(|shape| matches!(shape.shape, egui::Shape::LineSegment { .. })));
        assert_eq!(samples, (rect().height() / MINI_ROW_HEIGHT) as usize);
    }

    #[test]
    fn hidden_invalidation_defers_sampling_until_reveal_and_stops_after_leaving() {
        let ctx = egui::Context::default();
        let mut preview = Preview::default();
        let mut samples = 0;
        let rows = (rect().height() / MINI_ROW_HEIGHT) as usize;

        // Invalidations and changing document sizes must stay cheap while idle.
        for count in [1000, 2000, 3000, 4000] {
            preview.invalidate();
            for _ in 0..3 {
                frame(&ctx, &mut preview, 400.0, vec![], count, &mut samples);
                assert_eq!(visibility(&ctx), 0.0);
                assert_eq!(samples, 0);
                assert!(preview.cache.is_empty());
                assert!(preview.cache_key.is_none());
            }
        }
        reveal(&ctx, &mut preview, 4000, &mut samples);
        assert_eq!(samples, rows, "first reveal fills the cache exactly once");

        frame(&ctx, &mut preview, 400.0, vec![egui::Event::PointerGone], 4000, &mut samples);
        for _ in 0..8 {
            frame(&ctx, &mut preview, 400.0, vec![], 4000, &mut samples);
        }
        assert_eq!(visibility(&ctx), 0.0);
        assert_eq!(samples, rows, "leaving must reuse the existing cache");

        // A stale populated cache must also defer its refresh until revealed.
        preview.invalidate();
        for _ in 0..8 {
            frame(&ctx, &mut preview, 400.0, vec![], 4000, &mut samples);
            assert_eq!(visibility(&ctx), 0.0);
            assert_eq!(samples, rows);
            assert!(preview.cache_key.is_none());
        }
        reveal(&ctx, &mut preview, 4000, &mut samples);
        assert_eq!(samples, rows * 2, "reveal refreshes invalidated data only once");
        for _ in 0..4 {
            frame(&ctx, &mut preview, 400.0, vec![], 4000, &mut samples);
        }
        assert_eq!(samples, rows * 2, "visible cache hits must not resample");

        frame(&ctx, &mut preview, 400.0, vec![egui::Event::PointerGone], 4000, &mut samples);
        for _ in 0..8 {
            frame(&ctx, &mut preview, 400.0, vec![], 4000, &mut samples);
        }
        assert_eq!(visibility(&ctx), 0.0);
        for count in [5000, 6000, 7000] {
            preview.invalidate();
            frame(&ctx, &mut preview, 0.0, vec![], count, &mut samples);
            assert_eq!(samples, rows * 2, "hidden edits after leaving must not sample");
        }
    }

    #[test]
    fn render_empty_and_outside_overlay_do_not_intercept() {
        let ctx = egui::Context::default();
        let mut preview = Preview::default();
        let mut samples = 0;
        for _ in 0..3 {
            assert_eq!(
                frame(&ctx, &mut preview, 0.0, vec![], 0, &mut samples),
                None
            );
        }
        assert_eq!(samples, 0);
        reveal(&ctx, &mut preview, 0, &mut samples);
        assert_eq!(
            frame(
                &ctx,
                &mut preview,
                0.0,
                button(rect().center(), true),
                0,
                &mut samples
            ),
            None
        );
        frame(
            &ctx,
            &mut preview,
            0.0,
            button(rect().center(), false),
            0,
            &mut samples,
        );
        assert_eq!(
            frame(
                &ctx,
                &mut preview,
                0.0,
                button(Pos2::new(240.0, 230.0), true),
                0,
                &mut samples
            ),
            None
        );
    }
}

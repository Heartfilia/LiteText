//! Theme-aware optional close control, shared by horizontal and vertical tabs.
use crate::Palette;
use eframe::egui;
#[cfg(test)]
#[path = "tab_close_button_tests.rs"]
mod tests;

pub(crate) fn show(ui: &mut egui::Ui, tab: egui::Rect, id: u64, pal: &Palette) -> egui::Response {
    let rect = egui::Rect::from_center_size(
        egui::pos2(tab.right() - 13.0, tab.center().y), egui::vec2(20.0, 20.0));
    let response = ui.interact(rect, egui::Id::new(("tab_close_button", id)), egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let painter = ui.painter_at(rect.intersect(ui.clip_rect()));
    let color = if response.hovered() { pal.accent } else { pal.text_faint };
    if response.hovered() { painter.rect_filled(rect.shrink(1.0), 5.0, pal.accent.gamma_multiply(0.12)); }
    let center = rect.center();
    let stroke = egui::Stroke::new(1.4_f32, color);
    painter.line_segment([center - egui::vec2(3.0,3.0), center + egui::vec2(3.0,3.0)], stroke);
    painter.line_segment([center + egui::vec2(-3.0,3.0), center + egui::vec2(3.0,-3.0)], stroke);
    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new(("tab_close_button_rect", id)), rect));
    response
}

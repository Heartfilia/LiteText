//! Optional IDE-style tab list. Uses the same tab actions and document order as
//! the horizontal strip; changing layout never replaces or recreates documents.
use crate::{Lang, LiteTextApp, Palette, TabAction};
use eframe::egui;

#[cfg(test)]
#[path = "vertical_tabs_tests.rs"]
mod tests;

impl LiteTextApp {
    pub(crate) fn vertical_tab_panel(&mut self, ctx: &egui::Context, pal: &Palette) {
        if !self.vertical_tabs {
            return;
        }
        // New/opened documents and keyboard tab navigation must reveal the
        // active row, without pulling the user back while manually scrolling.
        let previous = ctx.data(|d| d.get_temp::<Option<u64>>(egui::Id::new("vertical_tabs_active")));
        if previous != Some(self.active_id) { self.reveal_tab = self.active_id; }
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("vertical_tabs_active"), self.active_id));
        let mut action = None;
        let mut reorder = None;
        let blocked = self.shortcuts_blocked();
        egui::SidePanel::left("vertical_tabs_panel")
            .default_width(160.0)
            .width_range(160.0..=320.0)
            .resizable(true)
            .frame(egui::Frame::new().fill(pal.bg_panel).inner_margin(egui::Margin::same(8)))
            .show(ctx, |ui| {
                ctx.data_mut(|d| d.insert_temp(egui::Id::new("vertical_tabs_panel_rect"), ui.max_rect()));
                if blocked { ui.disable(); }
                let pinned: Vec<_> = self.files.iter().filter(|f| f.pinned).map(|f| f.id).collect();
                let normal: Vec<_> = self.files.iter().filter(|f| !f.pinned).map(|f| f.id).collect();
                let width = ui.available_width();
                // The pinned group stays above the independently scrolling file
                // list. Bound it on small windows so neither group disappears.
                if !pinned.is_empty() {
                    ui.label(egui::RichText::new(if self.lang == Lang::ZhCn { "固定" } else { "Pinned" })
                        .small().strong().color(pal.text_faint));
                    let height = (ui.available_height() * 0.4).max(34.0);
                    egui::ScrollArea::vertical().id_salt("vertical_pinned_tabs")
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                        .max_height(height).auto_shrink([false, true]).show(ui, |ui| {
                            for id in pinned.iter().copied() {
                                if let Some(picked) = self.vertical_tab_row(ui, pal, id, true, width) { action = Some(picked); }
                            }
                        });
                    ui.add_space(4.0);
                    ui.separator();
                }
                ui.label(egui::RichText::new(if self.lang == Lang::ZhCn { "标签页" } else { "Tabs" })
                    .small().strong().color(pal.text_faint));
                egui::ScrollArea::vertical().id_salt("vertical_normal_tabs")
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .auto_shrink([false, false]).show(ui, |ui| {
                        for id in normal.iter().copied() {
                            if let Some(picked) = self.vertical_tab_row(ui, pal, id, false, width) { action = Some(picked); }
                            // Drop before/after the hovered row, constrained to
                            // normal tabs; fixed tabs stay in their own group.
                            let rect = ui.ctx().data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("vertical_tab_rect", id)))).unwrap();
                            if let Some(drag) = self.tab_drag_id.filter(|drag| normal.contains(drag))
                                && let Some(pos) = ui.input(|i| i.pointer.interact_pos())
                                && rect.contains(pos) && ui.clip_rect().contains(pos) && drag != id
                            {
                                let below = pos.y >= rect.center().y;
                                let y = if below { rect.bottom() } else { rect.top() };
                                ui.painter().hline(rect.x_range(), y, egui::Stroke::new(2.0_f32, pal.accent));
                                if ui.input(|i| i.pointer.any_released()) {
                                    let mut order: Vec<_> = normal.iter().copied().filter(|item| *item != drag).collect();
                                    if let Some(at) = order.iter().position(|item| *item == id) {
                                        order.insert(at + usize::from(below), drag);
                                        reorder = Some(order);
                                    }
                                }
                            }
                        }
                        if normal.is_empty() {
                            ui.label(egui::RichText::new(if self.lang == Lang::ZhCn { "暂无其他标签页" } else { "No other tabs" }).small().color(pal.text_faint));
                        }
                    });
                // Explicit divider separates the list from the editor's own
                // line-number gutter (including the empty-document screen).
                let rect = ui.max_rect();
                ui.painter().vline(rect.right() + 7.0, rect.y_range(),
                    egui::Stroke::new(1.0_f32, pal.text_faint.gamma_multiply(0.35)));
            });
        if ctx.input(|i| i.pointer.any_released()) { self.tab_drag_id = None; }
        if let Some(order) = reorder { self.reorder_normal_tabs(&order); }
        if let Some(action) = action { self.request_tab_action(action); ctx.request_repaint(); }
    }

    fn vertical_tab_row(&mut self, ui: &mut egui::Ui, pal: &Palette, id: u64, pinned: bool, width: f32) -> Option<TabAction> {
        let file = self.files.iter().find(|f| f.id == id)?;
        let active = self.active_id == Some(id);
        let dirty = file.has_unsaved_changes();
        let title = self.tab_title(id);
        let path = if !file.path.as_os_str().is_empty() { file.path.display().to_string() }
            else if let Some(path) = &file.scratch_path { path.display().to_string() }
            else { title.clone() };
        let (_, rect) = ui.allocate_space(egui::vec2(width, 30.0));
        let response = ui.interact(rect, egui::Id::new(("vertical_tab", id)),
            if pinned { egui::Sense::click() } else { egui::Sense::click_and_drag() });
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new(("vertical_tab_rect", id)), rect));
        let response = response.on_hover_text(path);
        let painter = ui.painter_at(rect.intersect(ui.clip_rect()));
        painter.rect_filled(rect, 5.0, if active { pal.bg_window } else if response.hovered() { pal.bg_elevated } else { pal.bg_panel });
        if dirty { painter.rect_filled(rect, 5.0, pal.danger.gamma_multiply(if active { 0.12 } else { 0.06 })); }
        if active { painter.rect_filled(egui::Rect::from_min_size(rect.min, egui::vec2(3.0, rect.height())), 2.0, pal.accent); }
        let title_rect = egui::Rect::from_min_max(rect.min + egui::vec2(6.0, 0.0), rect.max - egui::vec2(6.0, 0.0));
        crate::paint_tab_title(&painter, title_rect, &title, if active { pal.text_main } else { pal.text_faint });
        let mut action = None;
        if !pinned {
            if (self.tab_close_mode.allows_double() && response.double_clicked())
                || (self.tab_close_mode.allows_middle() && response.clicked_by(egui::PointerButton::Middle)) {
                action = Some(TabAction::Close(id));
            }
            if response.drag_started() { self.tab_drag_id = Some(id); }
        }
        if response.clicked() && action.is_none() && !active {
            self.active_id = Some(id);
            self.selection = None;
            self.drag_anchor = None;
            self.inspector_popup = None;
        }
        response.context_menu(|menu| {
            if let Some(picked) = self.tab_context_menu(menu, id) { action = Some(picked); }
        });
        if self.reveal_tab == Some(id) { ui.scroll_to_rect(rect, None); self.reveal_tab = None; }
        action
    }
}

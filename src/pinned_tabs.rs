//! Compact pinned tabs have their own bounded horizontal viewport.
use crate::{LiteTextApp, Palette, TabAction, i18n::tr};
use eframe::egui;

pub(crate) const WIDTH: f32 = 48.0;
const GAP: f32 = 6.0;

pub(crate) fn area_width(count: usize, available: f32) -> f32 {
    if count == 0 {
        return 0.0;
    }
    (count as f32 * (WIDTH + GAP) - GAP).min((available * 0.32).clamp(WIDTH, 240.0))
}

#[cfg(test)]
#[path = "pinned_tabs_tests.rs"]
mod tests;

impl LiteTextApp {
    pub(crate) fn reorder_normal_tabs(&mut self, order: &[u64]) {
        let (mut pinned, normal): (Vec<_>, Vec<_>) =
            self.files.drain(..).partition(|file| file.pinned);
        let mut normal: std::collections::HashMap<_, _> =
            normal.into_iter().map(|file| (file.id, file)).collect();
        pinned.extend(order.iter().filter_map(|id| normal.remove(id)));
        self.files = pinned;
    }
    pub(crate) fn toggle_pin(&mut self, id: u64) {
        let Some(index) = self.files.iter().position(|file| file.id == id) else {
            return;
        };
        let mut file = self.files.remove(index);
        file.pinned = !file.pinned;
        self.diagnostic_event(
            "INFO",
            "tabs",
            &format!(
                "{} tab id={id}; active_id={:?}; tabs={}",
                if file.pinned { "Pinned" } else { "Unpinned" },
                self.active_id,
                self.files.len() + 1
            ),
        );
        if file.pinned {
            let end = self.files.iter().take_while(|file| file.pinned).count();
            self.files.insert(end, file);
        } else {
            self.files.push(file);
        }
        self.tab_drag_id = None;
        self.reveal_tab = Some(id);
    }

    pub(crate) fn tab_context_menu(&self, menu: &mut egui::Ui, id: u64) -> Option<TabAction> {
        let file = self.files.iter().find(|file| file.id == id)?;
        let t = tr(self.lang);
        let zh = self.lang == crate::Lang::ZhCn;
        let mut action = None;
        let pin = menu.button(match (file.pinned, zh) {
            (true, true) => "取消固定",
            (true, false) => "Unpin tab",
            (false, true) => "固定",
            (false, false) => "Pin tab",
        });
        menu.ctx()
            .data_mut(|data| data.insert_temp(egui::Id::new(("pin_tab_rect", id)), pin.rect));
        if pin.clicked() {
            action = Some(TabAction::Pin(id));
        }
        menu.separator();
        let has_path = !file.path.as_os_str().is_empty();
        if menu
            .add_enabled(has_path, egui::Button::new(t.open_folder))
            .clicked()
        {
            action = Some(TabAction::OpenFolder(id));
        }
        let reload = menu.add_enabled(has_path, egui::Button::new(if zh { "从磁盘重新加载" } else { "Reload from disk" }))
            .on_hover_text(if zh { "重新读取原文件，成功后重置标记与撤销历史；失败或取消保留工作区。" } else { "Reread disk; reset marks and undo history only on success. Cancel/failure retains the workspace." });
        menu.ctx()
            .data_mut(|data| data.insert_temp(egui::Id::new(("reload_tab_rect", id)), reload.rect));
        if reload.clicked() {
            action = Some(TabAction::Reload(id));
        }
        if !file.pinned {
            menu.separator();
            for (label, value) in [
                (t.close, TabAction::Close(id)),
                (t.close_others, TabAction::CloseOthers(id)),
                (t.close_left, TabAction::CloseLeft(id)),
                (t.close_right, TabAction::CloseRight(id)),
            ] {
                if menu.button(label).clicked() {
                    action = Some(value);
                }
            }
        }
        if action.is_some() {
            menu.close_menu();
        }
        action
    }

    pub(crate) fn pinned_tab_strip(
        &mut self,
        ui: &mut egui::Ui,
        pal: &Palette,
        width: f32,
    ) -> Option<TabAction> {
        let mut pending_action = None;
        let ids: Vec<_> = self
            .files
            .iter()
            .filter(|file| file.pinned)
            .map(|file| file.id)
            .collect();
        ui.scope(|ui| {
            ui.style_mut().always_scroll_the_only_direction = true;
            let output = egui::ScrollArea::horizontal()
                .id_salt("pinned_tabs")
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .auto_shrink([false, true])
                .min_scrolled_width(0.0)
                .max_width(width)
                .show(ui, |ui| {
                    let origin = ui.cursor().min;
                    for (index, id) in ids.iter().copied().enumerate() {
                        let active = self.active_id == Some(id);
                        let Some(file) = self.files.iter().find(|file| file.id == id) else {
                            continue;
                        };
                        let title = self.tab_title(id);
                        let tooltip = if file.path.as_os_str().is_empty() {
                            title.clone()
                        } else {
                            format!("{title}\n{}", file.path.display())
                        };
                        let rect = egui::Rect::from_min_size(
                            origin + egui::vec2(index as f32 * (WIDTH + GAP), 6.0),
                            egui::vec2(WIDTH, if active { 32.0 } else { 26.0 }),
                        );
                        ui.ctx().data_mut(|data| {
                            data.insert_temp(egui::Id::new(("pinned_tab_rect", id)), rect)
                        });
                        let response = ui
                            .interact(
                                rect,
                                egui::Id::new("file_tab").with(id),
                                egui::Sense::click(),
                            )
                            .on_hover_text(tooltip);
                        let painter = ui.painter_at(rect);
                        painter.rect_filled(
                            rect,
                            egui::CornerRadius {
                                nw: 6,
                                ne: 6,
                                sw: if active { 0 } else { 6 },
                                se: if active { 0 } else { 6 },
                            },
                            if active {
                                pal.bg_window
                            } else if response.hovered() {
                                pal.bg_inset
                            } else {
                                pal.bg_elevated
                            },
                        );
                        if file.has_unsaved_changes() {
                            painter.rect_filled(
                                rect.shrink(1.0),
                                6.0,
                                pal.danger.gamma_multiply(0.12),
                            );
                        }
                        let color = if active {
                            pal.text_main
                        } else {
                            pal.text_faint
                        };
                        let label: String =
                            if file.path.as_os_str().is_empty() && file.source_path.is_none() {
                                file.name
                                    .rsplit(' ')
                                    .next()
                                    .unwrap_or(&file.name)
                                    .chars()
                                    .take(2)
                                    .collect()
                            } else {
                                file.name.chars().take(2).collect()
                            };
                        painter.text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            label,
                            egui::FontId::proportional(12.0),
                            color,
                        );
                        if response.clicked() && !active {
                            self.active_id = Some(id);
                            self.selection = None;
                            self.drag_anchor = None;
                            self.inspector_popup = None;
                        }
                        let mut action = None;
                        response.context_menu(|menu| {
                            action = self.tab_context_menu(menu, id);
                        });
                        if self.reveal_tab == Some(id) {
                            ui.scroll_to_rect(rect, None);
                            self.reveal_tab = None;
                        }
                        if let Some(action) = action {
                            pending_action = Some(action);
                        }
                    }
                    ui.allocate_space(egui::vec2(
                        (ids.len() as f32 * (WIDTH + GAP) - GAP).max(0.0),
                        38.0,
                    ));
                });
            ui.ctx().data_mut(|data| {
                data.insert_temp(
                    egui::Id::new("pinned_tabs_viewport"),
                    (output.inner_rect, output.state.offset.x),
                )
            });
        });
        let (rect, _) = ui.allocate_exact_size(egui::vec2(1.0, 20.0), egui::Sense::hover());
        ui.painter().line_segment(
            [rect.center_top(), rect.center_bottom()],
            egui::Stroke::new(1.0_f32, pal.text_faint.gamma_multiply(0.25)),
        );
        ui.ctx()
            .data_mut(|data| data.insert_temp(egui::Id::new("pinned_tabs_divider"), rect));
        pending_action
    }
}

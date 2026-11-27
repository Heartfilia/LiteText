//! Workspace context-menu plugin entries. Execute at the app boundary using
//! the same validated, undoable operations as the main menu.
use eframe::egui;
#[derive(Clone, Copy)]
pub(crate) enum Action {
    Json, Web(crate::web_tools::Action), Curl(crate::curl_plugin::Target),
    Conversion(crate::text_conversion::Action), Whitespace(crate::whitespace::Action), Compare,
}
pub(crate) fn submenu(ui: &mut egui::Ui, file: u64, enabled: bool, zh: bool) -> Option<Action> {
    let mut picked = None;
    let response = ui.add_enabled_ui(enabled, |ui| ui.menu_button(if zh { "插件" } else { "Plugins" }, |ui| {
        if ui.button("Format JSON").clicked() { picked = Some(Action::Json); }
        for tool in [crate::web_tools::Action::Html, crate::web_tools::Action::Header, crate::web_tools::Action::Cookie] {
            if ui.button(tool.label()).clicked() { picked = Some(Action::Web(tool)); }
        }
        ui.separator();
        ui.menu_button(if zh { "curl解析" } else { "Parse cURL" }, |ui| {
            for target in crate::curl_plugin::TARGETS { if ui.button(target.name()).clicked() { picked = Some(Action::Curl(target)); } }
        });
        ui.menu_button(if zh { "文本转换" } else { "Text conversion" }, |ui| {
            for action in crate::text_conversion::ACTIONS { if ui.button(action.label(zh)).clicked() { picked = Some(Action::Conversion(action)); } }
        });
        ui.menu_button(if zh { "空白符处理" } else { "Whitespace" }, |ui| {
            for action in crate::whitespace::ACTIONS { if ui.button(action.label(zh)).clicked() { picked = Some(Action::Whitespace(action)); } }
        });
        if ui.button(if zh { "文本对比…" } else { "Compare text…" }).clicked() { picked = Some(Action::Compare); }
        if picked.is_some() { ui.close_menu(); }
    })).inner;
    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new(("workspace_plugins_rect", file)), response.response.rect));
    picked
}
impl crate::LiteTextApp {
    pub(crate) fn execute_context_plugin(&mut self, ctx: &egui::Context) {
        let Some((id, action)) = self.active_mut().and_then(|file| file.editor_state.context_plugin.take().map(|action| (file.id, action))) else { return; };
        if self.active_id != Some(id) || self.shortcuts_blocked() { return; }
        match action {
            Action::Json => self.format_active_json(ctx),
            Action::Web(tool) => self.apply_web_tool(ctx, tool),
            Action::Curl(target) => self.convert_active_curl(ctx, target),
            Action::Conversion(action) => self.convert_active_text(ctx, action),
            Action::Whitespace(action) => { let mode = self.tab_mode; if let Some(file) = self.active_mut() { crate::whitespace::apply(file, ctx, action, mode); } },
            Action::Compare => self.text_diff.open = true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deferred_plugin_reuses_selection_converter_without_touching_outside() {
        let ctx = egui::Context::default();
        let mut app = crate::LiteTextApp::default(); app.new_document();
        let file = app.active_mut().unwrap();
        file.draft_text = Some("前{\"a\":1}后".into()); file.sync_draft();
        file.editor_state.cursor = Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(1), egui::text::CCursor::new(8)));
        file.editor_state.context_plugin = Some(Action::Json);
        app.execute_context_plugin(&ctx);
        assert_eq!(app.active().unwrap().draft_text.as_deref(), Some("前{\n  \"a\": 1\n}后"));
        assert!(app.active().unwrap().editor_state.context_plugin.is_none());
    }
    #[test]
    fn editor_context_menu_lists_plugins_after_time_and_excludes_split() {
        for split in [false, true] {
            let ctx = egui::Context::default(); crate::setup_fonts(&ctx);
            let mut app = crate::LiteTextApp::default(); app.new_document();
            let file = app.active_mut().unwrap(); file.draft_text = Some("content".into()); file.sync_draft();
            if split { file.language_override = Some("Markdown".into()); file.markdown_mode = crate::markdown::Mode::Split; }
            let id = file.id;
            let draw = |app: &mut crate::LiteTextApp, events| ctx.run(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0,700.0))),
                events, ..Default::default()
            }, |ctx| { egui::CentralPanel::default().show(ctx, |ui| crate::draft_editor(ui, app.active_mut().unwrap(), &crate::DARK, false, crate::Lang::ZhCn)); });
            for _ in 0..3 { draw(&mut app, vec![]); }
            let pos = egui::pos2(150.0,100.0);
            for pressed in [true,false] { draw(&mut app, vec![egui::Event::PointerMoved(pos), egui::Event::PointerButton {
                pos, button: egui::PointerButton::Secondary, pressed, modifiers: Default::default(),
            }]); }
            for _ in 0..3 { draw(&mut app, vec![]); }
            let output = draw(&mut app, vec![]);
            let plugin = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("workspace_plugins_rect",id))));
            if split { assert!(plugin.is_none()); }
            else {
                let plugin = plugin.unwrap();
                let time = output.shapes.iter().find_map(|shape| match &shape.shape {
                    egui::Shape::Text(t) if t.galley.text() == crate::tr(crate::Lang::ZhCn).insert_time => Some(t.pos.y), _ => None,
                }).unwrap();
                assert!(plugin.top() > time);
            }
        }
    }
}

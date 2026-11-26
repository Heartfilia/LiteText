//! A draft title is not a filesystem association. Renaming changes only the
//! display/save suggestion; the owned cache path remains stable.
use crate::{Lang, LiteTextApp, Palette};
use eframe::egui;

pub(crate) struct Pending {
    id: u64,
    name: String,
    error: Option<String>,
}

fn validate(name: &str) -> Result<&str, &'static str> {
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." || name.chars().count() > 120
        || name.chars().any(|c| c.is_control() || "/\\:*?\"<>|".contains(c))
        || name.ends_with('.')
    { return Err("请输入有效名称（1–120 字，不含路径分隔符） / Enter a valid name (1–120 characters, no path separators)"); }
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].into_iter().any(|prefix| stem.strip_prefix(prefix)
            .is_some_and(|suffix| suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9')))
    { return Err("名称是系统保留名 / This name is reserved by the operating system"); }
    Ok(name)
}

impl LiteTextApp {
    pub(crate) fn request_rename_draft(&mut self, id: u64) {
        if self.shortcuts_blocked() { return; }
        if let Some(file) = self.files.iter().find(|f| f.id == id && f.is_untitled()) {
            self.rename_draft = Some(Pending { id, name: file.name.clone(), error: None });
        }
    }
    pub(crate) fn rename_draft_to(&mut self, id: u64, name: &str) -> Result<(), String> {
        let name = validate(name).map_err(str::to_owned)?.to_owned();
        if self.files.iter().any(|f| f.id != id && f.is_untitled() && f.name == name) {
            return Err("已有同名草稿 / A draft with that name is already open".into());
        }
        let Some(file) = self.files.iter_mut().find(|f| f.id == id && f.is_untitled()) else {
            return Err("只能重命名未保存草稿 / Only unsaved drafts can be renamed".into());
        };
        file.name = name;
        Ok(())
    }
    pub(crate) fn rename_draft_window(&mut self, ctx: &egui::Context, pal: &Palette) {
        let Some(mut pending) = self.rename_draft.take() else { return; };
        let zh = self.lang == Lang::ZhCn;
        let mut open = true; let mut confirm = false; let mut cancel = false;
        egui::Window::new(if zh { "重命名草稿" } else { "Rename draft" })
            .id(egui::Id::new("rename_draft_window")).collapsible(false).resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0,0.0]).open(&mut open).show(ctx, |ui| {
                ui.memory_mut(|m| m.set_modal_layer(ui.layer_id()));
                ui.label(if zh { "只修改标签名称；保存时仍会选择真实文件位置。" } else { "Only the draft name changes. Saving still asks for a file location." });
                let input = ui.add(egui::TextEdit::singleline(&mut pending.name).desired_width(300.0));
                ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("rename_draft_input_rect"), input.rect));
                if let Some(error) = &pending.error { ui.label(egui::RichText::new(error).color(pal.danger).small()); }
                ui.horizontal(|ui| {
                    if ui.button(if zh { "取消" } else { "Cancel" }).clicked() { cancel = true; }
                    let response = ui.button(if zh { "确认" } else { "Rename" });
                    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("rename_draft_confirm_rect"), response.rect));
                    confirm = response.clicked();
                });
            });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) { cancel = true; }
        if !open || cancel { return; }
        if confirm {
            match self.rename_draft_to(pending.id, &pending.name) {
                Ok(()) => { self.backup_or_report(); ctx.request_repaint(); return; }
                Err(error) => pending.error = Some(error),
            }
        }
        self.rename_draft = Some(pending);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unit_test_recovery_never_falls_back_to_real_user_storage() {
        let app = LiteTextApp::default();
        assert!(app.recovery_path().is_none());
    }

    #[test]
    fn context_menu_exposes_rename_and_confirmation_keeps_document_unsaved() {
        let ctx = egui::Context::default(); crate::setup_fonts(&ctx);
        let temp = tempfile::tempdir().unwrap();
        let mut app = LiteTextApp { recovery_path_override: Some(temp.path().join("session.json")), ..Default::default() };
        app.new_document(); let id = app.active_id.unwrap();
        let _ = ctx.run(Default::default(), |ctx| { egui::CentralPanel::default().show(ctx, |ui| {
            app.tab_context_menu(ui, id);
        }); });
        assert!(ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("rename_tab_rect", id)))).is_some());
        app.request_rename_draft(id); app.rename_draft.as_mut().unwrap().name = "draft-title".into();
        for _ in 0..3 { let _ = ctx.run(Default::default(), |ctx| app.rename_draft_window(ctx, &crate::DARK)); }
        let rect = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new("rename_draft_confirm_rect"))).unwrap();
        for pressed in [true, false] { let _ = ctx.run(egui::RawInput {
            events: vec![egui::Event::PointerMoved(rect.center()), egui::Event::PointerButton {
                pos: rect.center(), button: egui::PointerButton::Primary, pressed, modifiers: Default::default(),
            }], ..Default::default()
        }, |ctx| app.rename_draft_window(ctx, &crate::DARK)); }
        assert!(app.rename_draft.is_none());
        assert_eq!(app.active().unwrap().name, "draft-title");
        assert!(app.active().unwrap().path.as_os_str().is_empty());
    }

    #[test]
    fn renamed_mirror_restores_name_and_stays_unsaved() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = LiteTextApp { scratch_dir: Some(dir.path().to_path_buf()), ..Default::default() };
        app.new_document(); let id = app.active_id.unwrap();
        app.active_mut().unwrap().draft_text = Some("cached content".into());
        app.active_mut().unwrap().sync_draft(); app.scratch_flush(None, true);
        let mirror = app.active().unwrap().scratch_path.clone().unwrap();
        app.rename_draft_to(id, "未命名 99").unwrap();
        app.active_mut().unwrap().draft_text = Some("cached content".into());
        app.active_mut().unwrap().sync_draft(); app.scratch_flush(None, true);
        assert_eq!(app.active().unwrap().name, "未命名 99");
        app.rename_draft_to(id, "工作笔记.md").unwrap(); app.flush_draft_meta();
        assert_eq!(app.active().unwrap().scratch_path.as_ref(), Some(&mirror));
        drop(app);
        let mut restored = LiteTextApp { scratch_dir: Some(dir.path().to_path_buf()), ..Default::default() };
        restored.open_scratch_files();
        assert_eq!(restored.files.len(), 1);
        assert_eq!(restored.files[0].name, "工作笔记.md");
        assert!(restored.files[0].path.as_os_str().is_empty());
        assert_eq!(restored.files[0].draft_text.as_deref(), Some("cached content"));
    }

    #[test]
    fn rename_changes_title_not_association_or_contents() {
        let mut app = LiteTextApp::default(); app.new_document();
        let id = app.active_id.unwrap(); app.active_mut().unwrap().draft_text = Some("body".into());
        app.rename_draft_to(id, "接口笔记.md").unwrap();
        let file = app.active().unwrap();
        assert!(file.path.as_os_str().is_empty()); assert!(file.is_untitled());
        assert_eq!(file.draft_text.as_deref(), Some("body")); assert_eq!(file.name, "接口笔记.md");
        assert!(app.rename_draft_to(id, "../escape").is_err());
        assert!(app.rename_draft_to(id, "CON.txt").is_err());
        assert_eq!(app.active().unwrap().name, "接口笔记.md");
        app.new_document(); let other = app.active_id.unwrap();
        assert!(app.rename_draft_to(other, "接口笔记.md").is_err());
    }
}

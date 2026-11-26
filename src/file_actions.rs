//! Save As reuses the checked atomic-save transaction; opening failures are batched.
use crate::{LiteTextApp, i18n::tr, linkage};
use eframe::egui;
use std::path::PathBuf;

pub(crate) struct OpenError {
    pub path: PathBuf,
    pub reason: String,
}

pub(crate) struct SaveFailure {
    id: u64,
    target: PathBuf,
    kind: std::io::ErrorKind,
    message: String,
}

impl SaveFailure {
    pub(crate) fn document_id(&self) -> u64 {
        self.id
    }
}

impl LiteTextApp {
    pub(crate) fn report_save_failure(&mut self, id: u64, target: PathBuf, error: std::io::Error) {
        self.diagnostic_event("ERROR", "save", &format!("Save failed: tab_id={id}; kind={:?}; os_code={:?}; target omitted", error.kind(), error.raw_os_error()));
        let message = format!(
            "{}: {}\n{error}",
            tr(self.lang).save_failed,
            target.display()
        );
        self.io_error = Some(message.clone());
        self.save_failure = Some(SaveFailure {
            id,
            target,
            kind: error.kind(),
            message,
        });
    }

    pub(crate) fn resolve_save_failure(&mut self, choice: u8) {
        let Some(failure) = self.save_failure.take() else {
            return;
        };
        self.io_error = None;
        if !self.files.iter().any(|file| file.id == failure.id) {
            return;
        }
        match choice {
            1 => {
                self.request_save_to(failure.id, failure.target);
            }
            2 => {
                if let Some(target) = self.choose_save_target(failure.id) {
                    self.request_save_to(failure.id, target);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn save_failure_dialog(&mut self, ctx: &egui::Context) {
        let Some(failure) = &self.save_failure else {
            return;
        };
        let zh = self.lang == crate::Lang::ZhCn;
        let mut choice = 0;
        egui::Window::new(tr(self.lang).save_failed).id(egui::Id::new("save_failure")).collapsible(false).resizable(true).default_width(460.0).show(ctx, |ui| {
            ui.memory_mut(|memory| memory.set_modal_layer(ui.layer_id()));
            ui.add(egui::Label::new(&failure.message).wrap().selectable(true));
            ui.add(egui::Label::new(if zh { "保存未完成。工作区和未保存状态已保留；不会自动提权、改权限或换编码。" } else { "Save did not complete. Your working copy and unsaved state are retained. No elevation, permission changes or automatic encoding changes." }).wrap());
            if failure.kind == std::io::ErrorKind::InvalidData {
                ui.add(egui::Label::new(if zh { "请取消此提示后，从状态栏选择可表示全部字符的保存编码；未知／有损文本请先用正确编码重新打开。" } else { "Cancel this prompt and select a save encoding that represents all characters. Reopen unknown/lossy input with its correct encoding first." }).wrap());
            }
            ui.horizontal(|ui| {
                if failure.kind != std::io::ErrorKind::InvalidData && ui.button(if zh { "重试" } else { "Retry" }).clicked() { choice = 1; }
                if ui.button(tr(self.lang).save_as).clicked() { choice = 2; }
                if ui.button(tr(self.lang).cancel).clicked() { choice = 3; }
            });
        });
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            choice = 3;
        }
        if choice != 0 {
            self.resolve_save_failure(choice);
        }
    }
    fn save_target_defaults(&self, id: u64) -> Option<(Option<PathBuf>, String)> {
        let file = self.files.iter().find(|file| file.id == id)?;
        let source = if file.path.as_os_str().is_empty() {
            file.source_path.as_ref()
        } else {
            Some(&file.path)
        };
        let directory = source.and_then(|path| path.parent()).map(PathBuf::from);
        let name = source
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| {
                if std::path::Path::new(&file.name).extension().is_some() { file.name.clone() }
                else { format!("{}.{}", file.name, if crate::markdown::is_markdown(file) { "md" } else { "txt" }) }
            });
        Some((directory, name))
    }

    pub(crate) fn choose_save_target(&mut self, id: u64) -> Option<PathBuf> {
        let (directory, name) = self.save_target_defaults(id)?;
        #[cfg(test)]
        if let Some(target) = self.save_as_target_override.take() {
            return target;
        }
        let mut dialog = rfd::FileDialog::new()
            .set_title(tr(self.lang).save_as)
            .set_file_name(name);
        if let Some(directory) = directory {
            dialog = dialog.set_directory(directory);
        }
        dialog.save_file()
    }

    pub(crate) fn save_as(&mut self) -> bool {
        if self.shortcuts_blocked() {
            return false;
        }
        let Some(id) = self.active_id else {
            return false;
        };
        let Some(target) = self.choose_save_target(id) else {
            return false;
        };
        self.request_save_to(id, target)
    }

    pub(crate) fn request_save_to(&mut self, id: u64, target: PathBuf) -> bool {
        let Some(file) = self.files.iter().find(|file| file.id == id) else {
            return false;
        };
        let resolved = linkage::resolved(&target);
        if self.files.iter().any(|other| {
            other.id != id
                && !other.path.as_os_str().is_empty()
                && linkage::resolved(&other.path) == resolved
        }) {
            self.report_save_failure(
                id,
                target,
                std::io::Error::new(std::io::ErrorKind::AlreadyExists, tr(self.lang).target_open),
            );
            return false;
        }
        let same = !file.path.as_os_str().is_empty() && linkage::resolved(&file.path) == resolved;
        if !same {
            match std::fs::read(&target) {
                Ok(expected) => {
                    self.pending_save = Some(linkage::PendingSave {
                        id,
                        target,
                        expected,
                    });
                    return false;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    self.report_save_failure(id, target, error);
                    return false;
                }
            }
        }
        self.commit_save(id, &target, None)
    }

    pub(crate) fn open_error_dialog(&mut self, ctx: &egui::Context) {
        if self.open_errors.is_empty() || self.opening_files() {
            return;
        }
        let t = tr(self.lang);
        let mut open = true;
        let mut done = false;
        let mut retry = false;
        egui::Window::new(t.open_failed)
            .id(egui::Id::new("open_errors"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(440.0)
            .max_width((ctx.screen_rect().width() - 32.0).max(100.0))
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(t.open_failed_desc);
                ui.label(if self.lang == crate::Lang::ZhCn { "读取期间变化的结果已丢弃。可重试读取最新版本，或关闭此提示保留当前工作区。" } else { "Results changed during loading are discarded. Retry to read the latest version, or dismiss to keep the current workspace." });
                egui::ScrollArea::vertical()
                    .max_height((ctx.screen_rect().height() * 0.5).max(80.0))
                    .show(ui, |ui| {
                        for error in &self.open_errors {
                            ui.separator();
                            ui.add(
                                egui::Label::new(error.path.to_string_lossy())
                                    .wrap()
                                    .selectable(true),
                            );
                            ui.add(egui::Label::new(&error.reason).wrap().selectable(true));
                        }
                    });
                ui.horizontal(|ui| {
                    retry = ui.button(if self.lang == crate::Lang::ZhCn { "重试失败项" } else { "Retry failed" }).clicked();
                    done = ui.button(t.done).clicked();
                });
            });
        if retry {
            self.retry_open_errors();
            ctx.request_repaint();
        } else if !open || done {
            self.open_errors.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DARK, LIGHT, Lang, ViewMode, recovery, text_tools};

    fn app(dir: &std::path::Path) -> LiteTextApp {
        LiteTextApp {
            recovery_path_override: Some(dir.join("session.json")),
            ..Default::default()
        }
    }

    #[test]
    fn renamed_draft_save_dialog_uses_title_and_keeps_save_as_semantics() {
        let mut app = LiteTextApp::default(); app.new_document();
        let id = app.active_id.unwrap();
        app.rename_draft_to(id, "接口记录.md").unwrap();
        assert_eq!(app.save_target_defaults(id).unwrap().1, "接口记录.md");
        assert!(app.active().unwrap().path.as_os_str().is_empty());
        app.rename_draft_to(id, "接口记录").unwrap();
        assert_eq!(app.save_target_defaults(id).unwrap().1, "接口记录.txt");
        app.save_as_target_override = Some(None);
        assert!(!app.save_file(id));
        assert!(app.active().unwrap().path.as_os_str().is_empty());
    }

    #[test]
    fn save_failure_alternative_target_keeps_original_and_targets_failed_document() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.txt");
        std::fs::write(&source, "original").unwrap();
        let mut app = app(dir.path());
        app.open_paths(vec![source.clone()]);
        let id = app.active_id.unwrap();
        app.active_mut().unwrap().draft_text = Some("edited".into());
        app.active_mut().unwrap().sync_draft();
        app.report_save_failure(
            id,
            source.clone(),
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, "Permission denied"),
        );
        assert!(app.shortcuts_blocked());
        assert!(app.active().unwrap().is_modified());
        app.new_document();
        let selected = app.active_id;
        let target = dir.path().join("copy.txt");
        app.save_as_target_override = Some(Some(target.clone()));
        app.resolve_save_failure(2);
        assert_eq!(app.active_id, selected);
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "original");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "edited");
        assert_eq!(app.files[0].path, target.canonicalize().unwrap());
        assert!(app.save_failure.is_none());
    }

    #[test]
    fn failed_save_retry_rechecks_overwrite_and_cancel_preserves_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.txt");
        let target = dir.path().join("target.txt");
        std::fs::write(&source, "original").unwrap();
        std::fs::write(&target, "external").unwrap();
        let mut app = app(dir.path());
        app.open_paths(vec![source.clone()]);
        let id = app.active_id.unwrap();
        app.active_mut().unwrap().draft_text = Some("edited".into());
        app.active_mut().unwrap().sync_draft();
        app.report_save_failure(
            id,
            target.clone(),
            std::io::Error::other("No space left on device"),
        );
        app.resolve_save_failure(1);
        assert!(app.pending_save.is_some());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "external");
        app.pending_save = None;
        app.report_save_failure(id, source.clone(), std::io::Error::other("write failed"));
        app.resolve_save_failure(3);
        assert!(app.save_failure.is_none());
        assert!(app.io_error.is_none());
        assert!(app.active().unwrap().is_modified());
        assert_eq!(app.active().unwrap().draft_text.as_deref(), Some("edited"));
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "original");
    }

    #[test]
    fn failed_save_blocks_close_prompts_and_native_exit_until_resolved() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.txt");
        std::fs::write(&source, "original").unwrap();
        let mut app = app(dir.path());
        app.open_paths(vec![source.clone()]);
        let id = app.active_id.unwrap();
        app.active_mut().unwrap().draft_text = Some("edited".into());
        app.active_mut().unwrap().sync_draft();
        app.request_tab_action(crate::TabAction::Close(id));
        app.pending_window_close = true;
        app.report_save_failure(id, source.clone(), std::io::Error::other("write failed"));
        let ctx = egui::Context::default();
        let output = ctx.run(Default::default(), |ctx| {
            app.file_dialogs(ctx);
            app.request_window_close(ctx);
        });
        assert!(output.viewport_output.values().any(|viewport| {
            viewport
                .commands
                .contains(&egui::ViewportCommand::CancelClose)
        }));
        assert!(app.pending_close.is_some() && app.pending_window_close);
        assert!(app.save_failure.is_some());
        assert_eq!(app.files.len(), 1);
        assert_eq!(std::fs::read_to_string(source).unwrap(), "original");
        app.resolve_save_failure(3);
        assert!(app.save_failure.is_none());
        assert!(app.pending_close.is_some());
        assert!(app.active().unwrap().has_unsaved_changes());
    }

    #[test]
    fn save_as_reassociates_active_document_preserves_source_encoding_and_history() {
        for large in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let source = dir.path().join("source.txt");
            let target = dir.path().join("另存为.txt");
            let original = "中文\r\n".repeat(if large { 40_000 } else { 1 });
            let encoding = recovery::Encoding::Utf16Le;
            let bytes = encoding.encode(&original).unwrap();
            std::fs::write(&source, &bytes).unwrap();
            let mut app = app(dir.path());
            app.open_paths(vec![source.clone()]);
            let id = app.active_id.unwrap();
            let ctx = egui::Context::default();
            let cursor = egui::text::CCursorRange::one(egui::text::CCursor::new(1));
            app.active_mut().unwrap().editor_state.cursor = Some(cursor);
            text_tools::replace_range(app.active_mut().unwrap(), &ctx, 0.."中".len(), "新".into());
            let edited = app.active().unwrap().draft_text.clone().unwrap();
            let revision = app.active().unwrap().editor_state.large.revision;
            app.new_document();
            let other = app.active_id;
            app.active_id = Some(id);
            app.save_as_target_override = Some(Some(target.clone()));
            assert!(app.save_as());
            let file = app.active().unwrap();
            assert_eq!(file.id, id);
            assert_eq!(file.path, target.canonicalize().unwrap());
            assert_eq!(file.name, "另存为.txt");
            assert_eq!(file.encoding, encoding);
            assert!(!file.is_modified());
            assert_eq!(file.editor_state.large.revision, revision);
            assert_eq!(file.editor_state.cursor, Some(cursor));
            assert_eq!(std::fs::read(&source).unwrap(), bytes);
            assert_eq!(
                std::fs::read(&target).unwrap(),
                encoding.encode(&edited).unwrap()
            );
            assert!(
                app.files
                    .iter()
                    .any(|file| Some(file.id) == other && file.path.as_os_str().is_empty())
            );
            let restored = recovery::load(&dir.path().join("session.json"))
                .unwrap()
                .unwrap()
                .restore();
            assert_eq!(restored[0].path, target.canonicalize().unwrap());
            assert_eq!(restored[0].draft_text.as_deref(), Some(edited.as_str()));
            assert_eq!(app.recent_files[0], target.canonicalize().unwrap());
            for frame in 0..3 {
                let editor_id = egui::Id::new(if large {
                    "large_editor"
                } else {
                    "draft_editor"
                })
                .with(id);
                ctx.memory_mut(|memory| memory.request_focus(editor_id));
                let events = if frame == 2 {
                    vec![egui::Event::Key {
                        key: egui::Key::Z,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::COMMAND,
                    }]
                } else {
                    vec![]
                };
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(800.0, 600.0),
                        )),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        app.handle_shortcuts(ctx);
                        egui::CentralPanel::default().show(ctx, |ui| {
                            crate::draft_editor(
                                ui,
                                app.active_mut().unwrap(),
                                &DARK,
                                false,
                                Lang::En,
                            )
                        });
                    },
                );
            }
            assert_eq!(
                app.active().unwrap().draft_text.as_deref(),
                Some(original.as_str())
            );
            assert!(app.active().unwrap().is_modified());
            assert_eq!(
                std::fs::read(&target).unwrap(),
                encoding.encode(&edited).unwrap()
            );
        }
    }

    #[test]
    fn save_as_scratch_detached_hex_and_binary_documents() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(dir.path());
        app.new_document();
        let id = app.active_id.unwrap();
        assert!(app.save_target_defaults(id).unwrap().1.ends_with(".txt"));
        app.save_as_target_override = Some(Some(dir.path().join("empty.txt")));
        assert!(app.save_as());
        assert!(!app.active().unwrap().is_modified());
        let source = dir.path().join("source.log");
        std::fs::write(&source, "a\r\nb").unwrap();
        app.open_paths(vec![source.clone()]);
        let id = app.active_id.unwrap();
        linkage::detach(app.active_mut().unwrap());
        assert_eq!(
            app.save_target_defaults(id),
            Some((
                Some(dir.path().canonicalize().unwrap()),
                "source.log".into()
            ))
        );
        app.active_mut().unwrap().view = ViewMode::Hex;
        assert!(app.request_save_to(id, dir.path().join("copy.log")));
        assert!(app.active().unwrap().view == ViewMode::Hex);
        assert!(app.active().unwrap().source_path.is_none());
        let source = dir.path().join("source.bin");
        let bytes = [0, 1, 2, 255];
        std::fs::write(&source, bytes).unwrap();
        app.open_paths(vec![source.clone()]);
        let id = app.active_id.unwrap();
        assert!(app.request_save_to(id, dir.path().join("copy.bin")));
        assert_eq!(std::fs::read(dir.path().join("copy.bin")).unwrap(), bytes);
        assert_eq!(std::fs::read(source).unwrap(), bytes);
        assert!(app.active().unwrap().is_binary);
    }

    #[test]
    fn save_as_cancel_failure_duplicate_and_lossy_keep_working_copy_and_originals() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.txt");
        let other = dir.path().join("other.txt");
        std::fs::write(&source, "source").unwrap();
        std::fs::write(&other, "other").unwrap();
        let mut app = app(dir.path());
        app.open_paths(vec![other.clone(), source.clone()]);
        let id = app.active_id.unwrap();
        let original_path = app.active().unwrap().path.clone();
        app.active_mut().unwrap().draft_text = Some("edited".into());
        app.active_mut().unwrap().sync_draft();
        app.save_as_target_override = Some(None);
        assert!(!app.save_as());
        assert!(!app.request_save_to(id, other.clone()));
        assert!(
            app.io_error
                .as_ref()
                .unwrap()
                .contains(tr(app.lang).target_open)
        );
        let invalid = dir.path().join("source.txt/child.txt");
        assert!(!app.request_save_to(id, invalid));
        app.active_mut().unwrap().encoding = recovery::Encoding::Lossy;
        let lossy = dir.path().join("lossy.txt");
        assert!(!app.request_save_to(id, lossy.clone()));
        assert!(!lossy.exists());
        let file = app.active().unwrap();
        assert_eq!(file.path, original_path);
        assert_eq!(file.draft_text.as_deref(), Some("edited"));
        assert!(file.is_modified());
        assert_eq!(std::fs::read_to_string(source).unwrap(), "source");
        assert_eq!(std::fs::read_to_string(other).unwrap(), "other");
    }

    #[test]
    fn save_as_overwrite_checks_approved_target_and_same_path_external_changes() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.txt");
        let target = dir.path().join("target.txt");
        std::fs::write(&source, "source").unwrap();
        std::fs::write(&target, "target").unwrap();
        let mut app = app(dir.path());
        app.open_paths(vec![source.clone()]);
        let id = app.active_id.unwrap();
        assert!(!app.request_save_to(id, target.clone()));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "target");
        app.pending_save.take(); // Cancel overwrite.
        assert_eq!(app.active().unwrap().path, source.canonicalize().unwrap());
        assert!(!app.request_save_to(id, target.clone()));
        let pending = app.pending_save.take().unwrap();
        std::fs::write(&target, "external change").unwrap();
        assert!(!app.commit_save(id, &target, Some(&pending.expected)));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "external change");
        assert!(!app.request_save_to(id, target.clone()));
        let pending = app.pending_save.take().unwrap();
        assert!(app.commit_save(id, &target, Some(&pending.expected)));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "source");
        assert!(app.request_save_to(id, target.clone()));
        std::fs::write(&target, "changed again").unwrap();
        assert!(!app.request_save_to(id, target.clone()));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "changed again");
        assert_eq!(app.active().unwrap().draft_text.as_deref(), Some("source"));
    }

    #[test]
    fn save_as_readonly_target_refuses_overwrite_and_same_path_save_stays_safe() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.txt");
        let target = dir.path().join("readonly.txt");
        std::fs::write(&source, "source").unwrap();
        std::fs::write(&target, "target").unwrap();
        let mut permissions = std::fs::metadata(&target).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&target, permissions).unwrap();
        let mut app = app(dir.path());
        app.open_paths(vec![source.clone()]);
        let id = app.active_id.unwrap();
        assert!(!app.request_save_to(id, target.clone()));
        let pending = app.pending_save.take().unwrap();
        assert!(!app.commit_save(id, &target, Some(&pending.expected)));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "target");
        assert_eq!(app.active().unwrap().path, source.canonicalize().unwrap());
        app.active_mut().unwrap().draft_text = Some("edit".into());
        app.active_mut().unwrap().sync_draft();
        assert!(app.request_save_to(id, source.clone()));
        assert_eq!(std::fs::read_to_string(source).unwrap(), "edit");
        // Restore original permissions so the temporary directory can be removed on Windows.
        let permissions = std::fs::metadata(dir.path().join("source.txt"))
            .unwrap()
            .permissions();
        std::fs::set_permissions(target, permissions).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn save_as_checks_symlink_aliases_and_preserves_target_links() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.txt");
        let target = dir.path().join("target.txt");
        let alias = dir.path().join("alias.txt");
        std::fs::write(&source, "source").unwrap();
        std::fs::write(&target, "target").unwrap();
        std::os::unix::fs::symlink(&target, &alias).unwrap();
        let mut app = app(dir.path());
        app.open_paths(vec![target.clone(), source.clone()]);
        let id = app.active_id.unwrap();
        assert!(!app.request_save_to(id, alias.clone()));
        assert!(app.pending_save.is_none());
        app.files.remove(0);
        assert!(!app.request_save_to(id, alias.clone()));
        let pending = app.pending_save.take().unwrap();
        assert!(app.commit_save(id, &alias, Some(&pending.expected)));
        assert!(
            std::fs::symlink_metadata(alias)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "source");
        assert_eq!(app.active().unwrap().path, target.canonicalize().unwrap());
    }

    #[test]
    fn open_failures_are_batched_keep_successful_tabs_and_show_paths_and_reasons() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("good.txt");
        let missing = dir.path().join("missing.txt");
        std::fs::write(&good, "good").unwrap();
        for lang in [Lang::ZhCn, Lang::En] {
            let mut app = app(dir.path());
            app.lang = lang;
            app.new_document();
            app.open_paths(vec![
                missing.clone(),
                good.clone(),
                dir.path().to_path_buf(),
            ]);
            assert_eq!(app.files.len(), 2);
            assert_eq!(app.open_errors.len(), 2);
            assert_eq!(app.active().unwrap().draft_text.as_deref(), Some("good"));
            assert_eq!(app.recent_files, [good.canonicalize().unwrap()]);
            assert!(app.shortcuts_blocked());
            let ctx = egui::Context::default();
            crate::setup_fonts(&ctx);
            for _ in 0..2 {
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(640.0, 480.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| app.file_dialogs(ctx),
                );
                let text: Vec<_> = output
                    .shapes
                    .iter()
                    .filter_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) => Some(text.galley.text()),
                        _ => None,
                    })
                    .collect();
                if !text.is_empty() {
                    assert!(text.contains(&tr(lang).open_failed));
                    assert!(text.contains(&missing.to_str().unwrap()));
                    assert!(text.contains(&app.open_errors[0].reason.as_str()));
                }
            }
            let mut done_pos = None;
            for frame in 0..3 {
                let events = if frame > 0 {
                    let pos = done_pos.unwrap();
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed: frame == 1,
                            modifiers: Default::default(),
                        },
                    ]
                } else {
                    vec![]
                };
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(640.0, 480.0),
                        )),
                        events,
                        ..Default::default()
                    },
                    |ctx| app.file_dialogs(ctx),
                );
                if frame == 0 {
                    done_pos = output.shapes.iter().find_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) if text.galley.text() == tr(lang).done => {
                            Some(text.pos + text.galley.size() * 0.5)
                        }
                        _ => None,
                    });
                }
            }
            assert!(app.open_errors.is_empty());
            assert!(!app.shortcuts_blocked());
            assert_eq!(app.files.len(), 2);
        }
    }

    #[test]
    fn save_as_menu_between_open_and_new_window_clicks_or_disables() {
        for lang in [Lang::ZhCn, Lang::En] {
            for palette in [&DARK, &LIGHT] {
                for has_document in [false, true] {
                    let dir = tempfile::tempdir().unwrap();
                    let target = dir.path().join("menu.txt");
                    let mut app = app(dir.path());
                    app.lang = lang;
                    app.show_menu = true;
                    if has_document {
                        app.new_document();
                    }
                    app.save_as_target_override = Some(Some(target.clone()));
                    let ctx = egui::Context::default();
                    crate::setup_fonts(&ctx);
                    let anchor =
                        egui::Rect::from_min_size(egui::pos2(590.0, 7.0), egui::vec2(28.0, 24.0));
                    let mut position = None;
                    for frame in 0..4 {
                        let events = if frame >= 2 {
                            let pos = position.unwrap();
                            vec![
                                egui::Event::PointerMoved(pos),
                                egui::Event::PointerButton {
                                    pos,
                                    button: egui::PointerButton::Primary,
                                    pressed: frame == 2,
                                    modifiers: Default::default(),
                                },
                            ]
                        } else {
                            vec![]
                        };
                        let output = ctx.run(
                            egui::RawInput {
                                screen_rect: Some(egui::Rect::from_min_size(
                                    egui::Pos2::ZERO,
                                    egui::vec2(640.0, 720.0),
                                )),
                                events,
                                ..Default::default()
                            },
                            |ctx| app.app_menu(ctx, anchor, palette),
                        );
                        if frame == 1 {
                            let texts: Vec<_> = output
                                .shapes
                                .iter()
                                .filter_map(|s| match &s.shape {
                                    egui::Shape::Text(t) => Some(t),
                                    _ => None,
                                })
                                .collect();
                            let save = texts
                                .iter()
                                .find(|t| t.galley.text() == tr(lang).save_as)
                                .unwrap();
                            let open = texts
                                .iter()
                                .find(|t| t.galley.text() == tr(lang).menu_open)
                                .unwrap();
                            let new = texts
                                .iter()
                                .find(|t| {
                                    t.galley.text()
                                        == if lang == Lang::ZhCn {
                                            "新窗口"
                                        } else {
                                            "New window"
                                        }
                                })
                                .unwrap();
                            assert!(open.pos.y < save.pos.y && save.pos.y < new.pos.y);
                            position = Some(save.pos + egui::vec2(8.0, 8.0));
                        }
                    }
                    assert_eq!(target.exists(), has_document);
                    assert_eq!(app.show_menu, !has_document);
                }
            }
        }
    }
}

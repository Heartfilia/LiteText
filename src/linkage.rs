//! Disk association: an empty writable path means a detached/untitled working copy.
use crate::{LoadedFile, editor, recovery};
use std::{
    io,
    path::{Path, PathBuf},
    time::SystemTime,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum External {
    Changed,
    Missing,
    Unreadable(String),
}

#[derive(Clone, Default)]
pub(crate) struct DiskState {
    pub checked: bool,
    pub stamp: Option<(u64, SystemTime)>,
    pub pending: Option<External>,
}

pub(crate) fn stamp(path: &Path) -> io::Result<(u64, SystemTime)> {
    let metadata = std::fs::metadata(path)?;
    Ok((metadata.len(), metadata.modified()?))
}

pub(crate) fn resolved(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| {
        let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        match (absolute.parent(), absolute.file_name()) {
            (Some(parent), Some(name)) => std::fs::canonicalize(parent)
                .map(|p| p.join(name))
                .unwrap_or(absolute),
            _ => absolute,
        }
    })
}

pub(crate) fn check(file: &mut LoadedFile, force: bool) {
    if file.path.as_os_str().is_empty() || file.disk.pending.is_some() {
        return;
    }
    let result = stamp(&file.path);
    if !force
        && file.disk.checked
        && result.as_ref().ok() == file.disk.stamp.as_ref()
        && result.is_ok()
    {
        return;
    }
    file.disk.checked = true;
    file.disk.stamp = result.ok();
    file.disk.pending = match std::fs::read(&file.path) {
        Ok(bytes) if bytes != file.saved_bytes => Some(External::Changed),
        Ok(_) => None,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Some(External::Missing),
        Err(error) => Some(External::Unreadable(error.to_string())),
    };
}

pub(crate) fn detach(file: &mut LoadedFile) {
    if file.path.as_os_str().is_empty() {
        return;
    }
    file.source_path = Some(std::mem::take(&mut file.path));
    file.disk = DiskState::default();
    file.content_revision += 1;
}

pub(crate) fn reload(file: &mut LoadedFile) -> io::Result<()> {
    // Read and decode completely before replacing any working state.
    let bytes = std::fs::read(&file.path)?;
    let binary = crate::detect_binary(&bytes);
    let (encoding, text) = if binary {
        (recovery::Encoding::default(), String::new())
    } else {
        file.saved_encoding.decode_as(&bytes).map_or_else(
            || recovery::Encoding::decode(&bytes),
            |t| (file.saved_encoding, t),
        )
    };
    if !binary && encoding == recovery::Encoding::Lossy {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Latest disk content cannot be decoded without loss; working copy retained",
        ));
    }
    let tools = file.editor_state.tools.clone();
    file.is_binary = binary;
    file.encoding = encoding;
    file.saved_encoding = encoding;
    file.character_count = text.chars().count();
    file.lines = if !binary && text.len() < crate::large_editor::THRESHOLD {
        text.split('\n').map(str::to_owned).collect()
    } else {
        Vec::new()
    };
    file.draft_text = (!binary).then_some(text);
    file.bytes = bytes.clone();
    file.saved_bytes = bytes;
    file.bytes_dirty = false;
    file.saved_large_revision = None;
    file.editor_state = editor::EditorState::default();
    file.editor_state.tools = tools;
    file.editor_state.tools.marks.clear();
    file.editor_state.tools.current = None;
    if binary {
        file.view = crate::ViewMode::Hex;
    }
    file.disk = DiskState::default();
    file.source_path = None;
    file.content_revision += 1;
    Ok(())
}

pub(crate) struct PendingSave {
    pub id: u64,
    pub target: PathBuf,
    pub expected: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detached_binary_copy_is_recoverable_without_source() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("binary.bin");
        let snapshot = dir.path().join("session.json");
        let original = [0, 1, 2, 255];
        std::fs::write(&path, original).unwrap();
        let mut app = crate::LiteTextApp::default();
        app.open_paths(vec![path.clone()]);
        detach(app.active_mut().unwrap());
        std::fs::remove_file(&path).unwrap();
        recovery::store_buffers(&snapshot, &app.files, app.active_id, false).unwrap();
        let restored = recovery::load(&snapshot).unwrap().unwrap().restore();
        assert_eq!(restored[0].bytes, original);
        assert!(restored[0].path.as_os_str().is_empty());
        assert!(restored[0].source_path.is_some());
    }

    #[test]
    fn detach_survives_restart_ignores_source_and_requires_confirmed_reassociation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("original.txt");
        let snapshot = dir.path().join("session.json");
        std::fs::write(&path, b"original\r\n").unwrap();
        let mut app = crate::LiteTextApp {
            recovery_path_override: Some(snapshot.clone()),
            ..Default::default()
        };
        app.open_paths(vec![path.clone()]);
        let file = app.active_mut().unwrap();
        file.draft_text = Some("workspace edits\r\n".to_owned());
        file.sync_draft();
        std::fs::write(&path, b"external").unwrap();
        check(file, true);
        assert_eq!(file.disk.pending, Some(External::Changed));
        detach(file);
        assert!(file.path.as_os_str().is_empty());
        assert!(file.source_path.is_some());
        app.write_recovery().unwrap();
        let mut restored = recovery::load(&snapshot).unwrap().unwrap().restore();
        let file = &mut restored[0];
        assert!(file.path.as_os_str().is_empty());
        assert_eq!(file.draft_text.as_deref(), Some("workspace edits\r\n"));
        assert!(file.editor_state.changes.is_marked(0));
        check(file, true);
        assert!(file.disk.pending.is_none());
        assert!(recovery::save_document(file, &path).is_err());
        let expected = std::fs::read(&path).unwrap();
        std::fs::write(&path, b"changed after dialog").unwrap();
        assert!(recovery::save_document_checked(file, &path, Some(&expected)).is_err());
        let expected = std::fs::read(&path).unwrap();
        recovery::save_document_checked(file, &path, Some(&expected)).unwrap();
        assert!(!file.path.as_os_str().is_empty());
        assert!(file.source_path.is_none());
        assert!(!file.is_modified());
        assert_eq!(std::fs::read(&path).unwrap(), b"workspace edits\r\n");
    }

    #[test]
    fn reload_reads_latest_version_preserves_buffer_on_failure_and_clears_history_on_success() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.txt");
        std::fs::write(&path, b"original").unwrap();
        let mut app = crate::LiteTextApp::default();
        app.open_paths(vec![path.clone()]);
        let file = app.active_mut().unwrap();
        file.draft_text = Some("workspace".to_owned());
        file.sync_draft();
        std::fs::write(&path, b"version B").unwrap();
        check(file, true);
        std::fs::write(&path, b"version C").unwrap();
        reload(file).unwrap();
        assert_eq!(file.draft_text.as_deref(), Some("version C"));
        assert!(!file.is_modified());
        assert!(file.disk.pending.is_none());
        std::fs::remove_file(&path).unwrap();
        check(file, true);
        assert_eq!(file.disk.pending, Some(External::Missing));
        assert!(reload(file).is_err());
        assert!(recovery::save_document(file, &path).is_err());
        assert!(!path.exists());
        assert_eq!(file.draft_text.as_deref(), Some("version C"));
        std::fs::write(&path, [b'a', 0xff, b'b']).unwrap();
        assert!(reload(file).is_err());
        assert_eq!(file.draft_text.as_deref(), Some("version C"));
    }

    #[test]
    fn clean_working_copy_is_kept_on_detach_and_reopening_source_is_a_separate_tab() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.txt");
        std::fs::write(&path, b"original").unwrap();
        let mut app = crate::LiteTextApp::default();
        app.open_paths(vec![path.clone()]);
        detach(app.active_mut().unwrap());
        app.open_paths(vec![path.clone()]);
        assert_eq!(app.files.len(), 2);
        assert!(app.files[0].is_modified());
        assert!(app.files[0].path.as_os_str().is_empty());
        assert_eq!(app.files[0].draft_text.as_deref(), Some("original"));
        assert!(!app.files[1].is_modified());
    }

    #[test]
    fn modified_line_numbers_clear_on_undo_and_save_for_small_and_large_text() {
        for large in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("lines.txt");
            let original = format!(
                "first\r\n中文🙂\r\n{}last",
                "unchanged\r\n".repeat(if large { 30_000 } else { 2 })
            );
            std::fs::write(&path, original.as_bytes()).unwrap();
            let mut app = crate::LiteTextApp::default();
            app.open_paths(vec![path.clone()]);
            let ctx = eframe::egui::Context::default();
            let file = app.active_mut().unwrap();
            crate::text_tools::replace_range(file, &ctx, 7..13, "改动".to_owned());
            let render = |file: &mut LoadedFile, events, focus| {
                ctx.run(
                    eframe::egui::RawInput {
                        screen_rect: Some(eframe::egui::Rect::from_min_size(
                            eframe::egui::Pos2::ZERO,
                            eframe::egui::vec2(800.0, 600.0),
                        )),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        eframe::egui::CentralPanel::default().show(ctx, |ui| {
                            crate::draft_editor(ui, file, &crate::DARK, focus, crate::Lang::En)
                        });
                    },
                )
            };
            render(file, vec![], true);
            let output = render(file, vec![], false);
            assert!(file.editor_state.changes.is_marked(1));
            assert!(!file.editor_state.changes.is_marked(0));
            assert!(!file.editor_state.changes.is_marked(2));
            assert!(output.shapes.iter().any(|s| matches!(&s.shape, eframe::egui::Shape::Text(t) if t.galley.text() == "2" && t.fallback_color == crate::DARK.accent)));
            render(
                file,
                vec![eframe::egui::Event::Key {
                    key: eframe::egui::Key::Z,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: eframe::egui::Modifiers::COMMAND,
                }],
                false,
            );
            render(file, vec![], false);
            assert_eq!(file.draft_text.as_deref(), Some(original.as_str()));
            assert!(file.editor_state.changes.marked.iter().all(|m| !m));
            crate::text_tools::replace_range(file, &ctx, 7..13, "修改".to_owned());
            render(file, vec![], false);
            recovery::save_document(file, &path).unwrap();
            render(file, vec![], false);
            assert!(!file.is_modified());
            assert!(file.editor_state.changes.marked.iter().all(|m| !m));
        }
    }

    #[test]
    fn checking_unchanged_content_does_not_prompt_and_archives_preserve_clean_old_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.txt");
        let archive = dir.path().join("before-reload.json");
        std::fs::write(&path, b"old version").unwrap();
        let mut app = crate::LiteTextApp::default();
        app.open_paths(vec![path.clone()]);
        check(app.active_mut().unwrap(), true);
        assert!(app.active().unwrap().disk.pending.is_none());
        std::fs::write(&path, b"old version").unwrap();
        check(app.active_mut().unwrap(), true);
        assert!(app.active().unwrap().disk.pending.is_none());
        std::fs::write(&path, b"new version").unwrap();
        check(app.active_mut().unwrap(), true);
        recovery::archive_buffers(&archive, &app.files, app.active_id).unwrap();
        reload(app.active_mut().unwrap()).unwrap();
        let restored = recovery::load(&archive).unwrap().unwrap().restore();
        assert_eq!(restored[0].draft_text.as_deref(), Some("old version"));
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some("new version")
        );
    }

    #[test]
    fn external_dialog_detach_button_keeps_text_and_removes_repeat_prompts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.txt");
        std::fs::write(&path, b"original").unwrap();
        let mut app = crate::LiteTextApp {
            recovery_path_override: Some(dir.path().join("session.json")),
            ..Default::default()
        };
        app.open_paths(vec![path.clone()]);
        std::fs::write(&path, b"external").unwrap();
        check(app.active_mut().unwrap(), true);
        let ctx = eframe::egui::Context::default();
        let mut click = None;
        for frame in 0..5 {
            let events = if frame == 2 || frame == 3 {
                let pos = click.unwrap();
                vec![
                    eframe::egui::Event::PointerMoved(pos),
                    eframe::egui::Event::PointerButton {
                        pos,
                        button: eframe::egui::PointerButton::Primary,
                        pressed: frame == 2,
                        modifiers: Default::default(),
                    },
                ]
            } else {
                vec![]
            };
            let output = ctx.run(
                eframe::egui::RawInput {
                    events,
                    screen_rect: Some(eframe::egui::Rect::from_min_size(
                        eframe::egui::Pos2::ZERO,
                        eframe::egui::vec2(1080.0, 720.0),
                    )),
                    ..Default::default()
                },
                |ctx| app.file_dialogs(ctx),
            );
            for shape in output.shapes {
                if let eframe::egui::Shape::Text(text) = shape.shape
                    && text.galley.text() == crate::tr(app.lang).detach_copy
                {
                    click = Some(text.pos + text.galley.size() * 0.5);
                }
            }
        }
        assert!(app.active().unwrap().path.as_os_str().is_empty());
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some("original")
        );
        check(app.active_mut().unwrap(), true);
        assert!(app.active().unwrap().disk.pending.is_none());
    }
}

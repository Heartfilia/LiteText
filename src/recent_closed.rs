//! Bounded, session-only undo for explicit tab closes. No document text is
//! written to preferences; restoring always allocates a fresh widget identity.
use crate::{LoadedFile, LiteTextApp};
use std::collections::VecDeque;

const MAX_TABS: usize = 20;
const MAX_BYTES: usize = 32 * 1024 * 1024;

#[derive(Default)]
pub(crate) struct State {
    entries: VecDeque<(LoadedFile, usize)>,
    bytes: usize,
}

fn memory_cost(file: &LoadedFile) -> usize {
    // Include the entire undo history retained by the clone conservatively by
    // refusing large-editor snapshots. Ordinary text history is reset below.
    file.draft_text.as_ref().map_or(0, String::len)
        .saturating_add(file.bytes.len()).saturating_add(file.saved_bytes.len())
}

impl LiteTextApp {
    pub(crate) fn remember_closed(&mut self, file: &LoadedFile) {
        if file.editor_state.large.active { return; }
        let cost = memory_cost(file);
        if cost > MAX_BYTES { return; }
        // Construct the snapshot without cloning undo histories, line caches
        // or image state merely to discard them immediately afterwards.
        let mut editor = crate::editor::EditorState {
            cursor: file.editor_state.cursor,
            cursor_line: file.editor_state.cursor_line,
            font_size: file.editor_state.font_size,
            line_ending: file.editor_state.line_ending,
            ..Default::default()
        };
        editor.tools.wrap = file.editor_state.tools.wrap;
        editor.tools.whitespace = file.editor_state.tools.whitespace;
        let snapshot = LoadedFile {
            pinned: false, id: file.id, path: file.path.clone(), name: file.name.clone(),
            bytes: file.bytes.clone(), is_binary: file.is_binary, lines: Vec::new(),
            view: file.view, language_override: file.language_override.clone(),
            draft_text: file.draft_text.clone(), editor_state: editor,
            encoding: file.encoding, saved_bytes: file.saved_bytes.clone(),
            character_count: file.character_count, saved_large_revision: None,
            bytes_dirty: file.bytes_dirty, content_revision: file.content_revision,
            saved_encoding: file.saved_encoding, source_path: file.source_path.clone(),
            disk: file.disk.clone(), line_ending: file.line_ending, markdown_mode: file.markdown_mode,
            markdown: Default::default(), scratch_path: None, saved_generation: file.saved_generation,
        };
        self.recent_closed.entries.push_back((snapshot, cost));
        self.recent_closed.bytes = self.recent_closed.bytes.saturating_add(cost);
        while self.recent_closed.entries.len() > MAX_TABS || self.recent_closed.bytes > MAX_BYTES {
            if let Some((_, cost)) = self.recent_closed.entries.pop_front() { self.recent_closed.bytes -= cost; }
        }
    }

    pub(crate) fn can_reopen_closed(&self) -> bool { !self.recent_closed.entries.is_empty() }

    pub(crate) fn reopen_closed_tab(&mut self) {
        let Some((mut file, cost)) = self.recent_closed.entries.pop_back() else { return; };
        self.recent_closed.bytes -= cost;
        // Never replace an already open working copy of the same real file.
        // Preserve the recovered text as an independent temporary copy instead.
        if !file.path.as_os_str().is_empty() && self.files.iter().any(|open|
            !open.path.as_os_str().is_empty() && crate::linkage::resolved(&open.path) == crate::linkage::resolved(&file.path)) {
            file.source_path = Some(file.path.clone());
            file.path.clear();
        }
        if file.is_untitled() { file.name = format!("{} {}", crate::tr(self.lang).untitled, self.next_untitled_number()); }
        file.id = self.next_file_id; self.next_file_id += 1;
        file.pinned = false;
        let id = file.id;
        if let Some(cursor) = file.editor_state.cursor {
            file.editor_state.pending_selection = Some(
                cursor.secondary.index.min(cursor.primary.index)..cursor.secondary.index.max(cursor.primary.index));
        }
        if file.draft_text.is_some() { file.sync_draft(); }
        self.files.push(file);
        self.active_id = Some(id); self.focus_editor = Some(id); self.reveal_tab = Some(id);
        self.selection = None; self.drag_anchor = None; self.inspector_popup = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reopen_preserves_working_copy_and_uses_new_identity() {
        let mut app = LiteTextApp::default(); app.new_document();
        let id = app.active_id.unwrap();
        app.active_mut().unwrap().draft_text = Some("unsaved 中文".into());
        app.active_mut().unwrap().language_override = Some("JSON".into());
        app.close_tab(id);
        assert!(app.can_reopen_closed()); app.reopen_closed_tab();
        assert_ne!(app.active_id, Some(id));
        assert_eq!(app.active().unwrap().draft_text.as_deref(), Some("unsaved 中文"));
        assert_eq!(app.active().unwrap().language_override.as_deref(), Some("JSON"));
        assert!(!app.can_reopen_closed());
    }
    #[test]
    fn history_is_bounded_and_newest_first() {
        let mut app = LiteTextApp::default();
        for index in 0..25 { app.new_document(); app.active_mut().unwrap().draft_text = Some(index.to_string()); app.close_tab(app.active_id.unwrap()); }
        assert_eq!(app.recent_closed.entries.len(), 20);
        app.reopen_closed_tab(); assert_eq!(app.active().unwrap().draft_text.as_deref(), Some("24"));
    }
}

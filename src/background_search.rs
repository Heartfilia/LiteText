//! Search scans, regex expansion, output assembly and initial index run in workers.
use super::*;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

type Handoff = Arc<Mutex<Option<PendingSearch>>>;
type Key = (u64, u64, String, String, bool, bool);
pub(crate) struct PendingSearch {
    id: u64,
    key: Key,
    cancel: Arc<AtomicBool>,
    receiver: mpsc::Receiver<Result<Output, String>>,
}

enum Output {
    Found(Option<Range<usize>>, Option<Range<usize>>),
    Count(usize),
    Replace(
        Vec<(Range<usize>, usize)>,
        Box<crate::large_editor::PreparedReplacement>,
        /// Byte and character range of the next hit after a single replace;
        /// `None` for replace-all.
        Option<(Range<usize>, Range<usize>)>,
    ),
    Empty,
}

fn key(file: &crate::LoadedFile) -> Key {
    let tools = &file.editor_state.tools;
    (
        file.content_revision,
        file.editor_state.large.revision,
        tools.query.clone(),
        tools.replacement.clone(),
        tools.regex,
        tools.ignore_case,
    )
}

pub(crate) fn start(file: &mut crate::LoadedFile, ctx: &egui::Context, action: usize) {
    if ctx.data(|data| {
        data.get_temp::<Handoff>(egui::Id::new("search_handoff"))
            .is_some()
    }) {
        return;
    }
    file.editor_state
        .tools
        .invalidate_search_if_changed((file.content_revision, file.editor_state.large.revision));
    let _timing = crate::timing::phase("search.snapshot");
    let source = file.draft_text.clone().unwrap();
    let tools = &file.editor_state.tools;
    let mut worker = Tools {
        query: tools.query.clone(),
        replacement: tools.replacement.clone(),
        regex: tools.regex,
        ignore_case: tools.ignore_case,
        ..Default::default()
    };
    let selected = crate::line_commands::selected_bytes(file, &source);
    let current = tools.current.clone().filter(|range| *range == selected);
    let from = if let Some(range) = &current {
        if action == 1 { range.start } else { range.end }
    } else {
        file.editor_state.cursor.map_or(0, |cursor| {
            if file.editor_state.large.active {
                file.editor_state
                    .large
                    .byte_for_char(&source, cursor.primary.index)
            } else {
                byte_at(&source, cursor.primary.index)
            }
        })
    };
    let encoding = file.encoding;
    let cancel = Arc::new(AtomicBool::new(false));
    let cancelled = cancel.clone();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _timing = crate::timing::phase("search.scan_expand_prepare");
        let result = (|| {
            if !worker.prepare_matcher((0, 0)) {
                return worker.search_error.clone().map_or(Ok(Output::Empty), Err);
            }
            let matcher = worker.matcher.as_ref().unwrap();
            if action == 5 {
                let mut count = 0;
                for _ in matcher.find_iter(&source) {
                    if cancelled.load(Ordering::Relaxed) {
                        return Ok(Output::Empty);
                    }
                    count += 1;
                }
                return Ok(Output::Count(count));
            }
            if action == 4 || action == 3 {
                let mut edits = Vec::new();
                if action == 3 {
                    if let Some(range) = current
                        && matcher
                            .find_at(&source, range.start)
                            .is_some_and(|found| found.range() == range)
                    {
                        let value = worker.replacement_for(&source, &range);
                        edits.push((range, value));
                    }
                } else {
                    for captures in matcher.captures_iter(&source) {
                        if cancelled.load(Ordering::Relaxed) {
                            return Ok(Output::Empty);
                        }
                        let range = captures.get(0).unwrap().range();
                        let value = if worker.regex {
                            let mut value = String::new();
                            captures.expand(&worker.replacement, &mut value);
                            value
                        } else {
                            worker.replacement.clone()
                        };
                        edits.push((range, value));
                    }
                }
                let Some(first) = edits.first() else {
                    return Ok(Output::Empty);
                };
                let advance = if action == 3 {
                    worker.find_one_after_edit(&source, &first.0, &first.1)
                } else {
                    None
                };
                let range = first.0.start..edits.last().unwrap().0.end;
                let mut inserted = String::new();
                let mut at = range.start;
                for (edit, value) in &edits {
                    if cancelled.load(Ordering::Relaxed) {
                        return Ok(Output::Empty);
                    }
                    inserted.push_str(&source[at..edit.start]);
                    inserted.push_str(value);
                    at = edit.end;
                }
                inserted.push_str(&source[at..range.end]);
                let cursor = egui::text::CCursorRange::one(egui::text::CCursor::new(
                    source[..range.start].chars().count()
                        + if action == 3 {
                            inserted.chars().count()
                        } else {
                            0
                        },
                ));
                let prepared = crate::large_editor::prepare_replacement(
                    &source, range, inserted, cursor, encoding,
                );
                return Ok(Output::Replace(
                    edits
                        .into_iter()
                        .map(|(range, value)| (range, value.len()))
                        .collect(),
                    Box::new(prepared),
                    advance,
                ));
            }
            let found = worker.find_one(
                &source,
                from,
                action == 1,
                current.as_ref().is_some_and(|range| range.is_empty()),
            );
            let characters = found.as_ref().map(|range| {
                let start = source[..range.start].chars().count();
                start..start + source[range.clone()].chars().count()
            });
            Ok(Output::Found(found, characters))
        })();
        let _ = sender.send(result);
    });
    ctx.data_mut(|data| {
        data.insert_temp(
            egui::Id::new("search_handoff"),
            Arc::new(Mutex::new(Some(PendingSearch {
                id: file.id,
                key: key(file),
                cancel,
                receiver,
            }))),
        )
    });
    ctx.request_repaint();
}

impl Drop for PendingSearch {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl crate::LiteTextApp {
    pub(crate) fn poll_search(&mut self, ctx: &egui::Context) {
        if let Some(handoff) =
            ctx.data_mut(|data| data.remove_temp::<Handoff>(egui::Id::new("search_handoff")))
        {
            self.pending_search = handoff.lock().unwrap().take();
        }
        let Some(job) = self.pending_search.take() else {
            return;
        };
        if crate::new_window::finished() {
            return;
        }
        match job.receiver.try_recv() {
            Ok(output) => {
                if let Some(file) = self
                    .files
                    .iter_mut()
                    .find(|file| file.id == job.id && key(file) == job.key)
                {
                    match output {
                        Ok(Output::Found(found, characters)) => {
                            file.editor_state.tools.not_found = found.is_none();
                            file.editor_state.tools.current = found;
                            if let Some(characters) = characters {
                                file.editor_state.selection = None;
                                file.editor_state.folds.expand_all();
                                file.editor_state.pending_selection = Some(characters);
                            }
                        }
                        Ok(Output::Count(count)) => {
                            file.editor_state.tools.count = true;
                            file.editor_state.tools.count_result = Some(count);
                            file.editor_state.tools.search_error = None;
                        }
                        Ok(Output::Replace(edits, prepared, advance)) => {
                            if !file.editor_state.large.active {
                                file.activate_large(ctx);
                            }
                            for (range, length) in edits.into_iter().rev() {
                                file.editor_state.tools.apply_edit(range, length);
                            }
                            file.editor_state
                                .large
                                .commit_prepared(file.draft_text.as_mut().unwrap(), *prepared);
                            file.character_count = file.editor_state.large.characters;
                            file.bytes.clear();
                            file.lines.clear();
                            file.bytes_dirty = true;
                            file.editor_state.selection = None;
                            file.editor_state.cursor = file.editor_state.large.tool_cursor();
                            file.editor_state.pending_selection = file
                                .editor_state
                                .cursor
                                .map(|cursor| cursor.secondary.index..cursor.primary.index);
                            // A single replace lands on the next hit so
                            // repeated replaces need no extra "next".
                            if let Some((found, characters)) = advance {
                                file.editor_state.tools.current = Some(found);
                                file.editor_state.tools.not_found = false;
                                file.editor_state.pending_selection = Some(characters);
                                // The edit bumped the revision; re-anchor so
                                // the advanced match is not dropped as stale.
                                let revision =
                                    (file.content_revision, file.editor_state.large.revision);
                                file.editor_state.tools.reanchor(revision);
                            }
                            if self.active_id == Some(file.id) {
                                self.focus_editor = Some(file.id);
                                file.editor_state.focus_requested = true;
                                ctx.memory_mut(|memory| {
                                    memory
                                        .request_focus(egui::Id::new("large_editor").with(file.id))
                                });
                            }
                        }
                        Ok(Output::Empty) => file.editor_state.tools.not_found = true,
                        Err(error) => file.editor_state.tools.search_error = Some(error),
                    }
                }
            }
            Err(mpsc::TryRecvError::Empty) => self.pending_search = Some(job),
            Err(mpsc::TryRecvError::Disconnected) => {
                self.io_error = Some("Search worker stopped; working copy retained".into())
            }
        }
        if self.pending_search.is_some() {
            let zh = self.lang == crate::Lang::ZhCn;
            let mut cancel = false;
            egui::Window::new(if zh {
                "正在搜索／处理文本"
            } else {
                "Searching / processing text"
            })
            .id(egui::Id::new("search_progress"))
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.spinner();
                cancel = ui.button(crate::tr(self.lang).cancel).clicked();
            });
            if cancel {
                self.pending_search = None;
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(30));
        }
    }
}

#[cfg(test)]
pub(crate) fn drain(app: &mut crate::LiteTextApp, ctx: &egui::Context) {
    if app.pending_search.is_none()
        && !ctx.data(|data| {
            data.get_temp::<Handoff>(egui::Id::new("search_handoff"))
                .is_some()
        })
    {
        return;
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let _ = ctx.run(Default::default(), |ctx| app.poll_search(ctx));
        if app.pending_search.is_none() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "search worker timeout"
        );
        std::thread::yield_now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> crate::LiteTextApp {
        let mut app = crate::LiteTextApp::default();
        app.new_document();
        app.active_mut().unwrap().draft_text = Some("中文 hit42\r\n".repeat(40_000));
        app.active_mut().unwrap().sync_draft();
        let tools = &mut app.active_mut().unwrap().editor_state.tools;
        tools.query = "hit(\\d+)".into();
        tools.regex = true;
        tools.replacement = "value$1".into();
        app
    }

    #[test]
    fn background_regex_count_and_replace_are_atomic_and_reject_stale_query() {
        let ctx = egui::Context::default();
        let mut app = app();
        start(app.active_mut().unwrap(), &ctx, 5);
        drain(&mut app, &ctx);
        assert_eq!(
            app.active().unwrap().editor_state.tools.count_result,
            Some(40_000)
        );
        let source = app.active().unwrap().draft_text.clone();
        start(app.active_mut().unwrap(), &ctx, 4);
        app.active_mut().unwrap().editor_state.tools.query = "different".into();
        drain(&mut app, &ctx);
        assert_eq!(app.active().unwrap().draft_text, source);
        app.active_mut().unwrap().editor_state.tools.query = "hit(\\d+)".into();
        start(app.active_mut().unwrap(), &ctx, 4);
        assert_eq!(app.active().unwrap().draft_text, source);
        drain(&mut app, &ctx);
        let file = app.active().unwrap();
        assert_eq!(
            file.draft_text.as_deref(),
            Some("中文 value42\r\n".repeat(40_000).as_str())
        );
        assert_eq!(file.editor_state.large.newlines.crlf, 40_000);
        assert_eq!(
            file.editor_state.large.characters,
            file.draft_text.as_ref().unwrap().chars().count()
        );
        assert!(file.editor_state.large.active);
    }

    #[test]
    fn cancelling_search_and_closed_documents_never_publish() {
        let ctx = egui::Context::default();
        let mut app = app();
        let source = app.active().unwrap().draft_text.clone();
        start(app.active_mut().unwrap(), &ctx, 4);
        let handoff = ctx
            .data_mut(|data| data.remove_temp::<Handoff>(egui::Id::new("search_handoff")))
            .unwrap();
        drop(handoff.lock().unwrap().take());
        drain(&mut app, &ctx);
        assert_eq!(app.active().unwrap().draft_text, source);
        start(app.active_mut().unwrap(), &ctx, 4);
        app.close_tab(app.active_id.unwrap());
        drain(&mut app, &ctx);
        assert!(app.files.is_empty());
    }
}

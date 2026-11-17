//! Reload is a cancellable read transaction; only a current, stable result may publish.
use crate::{Lang, LiteTextApp, LoadedFile, background_io, linkage, recovery};
use eframe::egui;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
        mpsc,
    },
};

pub(crate) struct Task {
    id: u64,
    path: PathBuf,
    revision: (u64, u64, u64),
    confirm: bool,
    error: Option<String>,
    receiver: Option<mpsc::Receiver<std::io::Result<LoadedFile>>>,
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
    phase: Arc<AtomicU8>,
}

impl Drop for Task {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn revision(file: &LoadedFile) -> (u64, u64, u64) {
    (
        file.content_revision,
        file.editor_state.large.revision,
        file.saved_generation,
    )
}

impl LiteTextApp {
    pub(crate) fn request_reload(&mut self, id: u64, confirmed: bool) {
        if self.reload_task.is_some()
            || self.encoding_task.is_some()
            || self.pending_search.is_some()
            || self.pending_batch.is_some()
            || self.restoring()
        {
            return;
        }
        let Some(file) = self
            .files
            .iter()
            .find(|file| file.id == id && !file.path.as_os_str().is_empty())
        else {
            return;
        };
        self.reload_task = Some(Task {
            id,
            path: file.path.clone(),
            revision: revision(file),
            confirm: file.has_unsaved_changes() && !confirmed,
            error: None,
            receiver: None,
            cancel: Arc::new(AtomicBool::new(false)),
            progress: Arc::new(AtomicU64::new(0)),
            total: Arc::new(AtomicU64::new(0)),
            phase: Arc::new(AtomicU8::new(0)),
        });
        if self.reload_task.as_ref().is_some_and(|task| !task.confirm) {
            self.begin_reload();
        }
    }

    pub(crate) fn begin_reload(&mut self) {
        let Some(task) = &mut self.reload_task else {
            return;
        };
        if task.receiver.is_some() {
            return;
        }
        let Some(file) = self.files.iter().find(|file| {
            file.id == task.id && file.path == task.path && revision(file) == task.revision
        }) else {
            self.reload_task = None;
            return;
        };
        let encoding = (!file.is_binary).then_some(file.saved_encoding);
        let path = task.path.clone();
        let (cancel, progress, total, phase) = (
            task.cancel.clone(),
            task.progress.clone(),
            task.total.clone(),
            task.phase.clone(),
        );
        let (sender, receiver) = mpsc::channel();
        task.phase.store(0, Ordering::Relaxed);
        task.progress.store(0, Ordering::Relaxed);
        task.total.store(0, Ordering::Relaxed);
        std::thread::spawn(move || {
            let result = if encoding == Some(recovery::Encoding::Lossy) {
                Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Unknown/lossy encoding: use Reopen with encoding to choose the correct encoding; working copy retained",
                ))
            } else {
                background_io::load_with_progress(
                    &path, &cancel, &progress, &total, &phase, encoding,
                )
            };
            let _ = sender.send(result);
        });
        task.confirm = false;
        task.error = None;
        task.receiver = Some(receiver);
    }

    pub(crate) fn reload_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut task) = self.reload_task.take() else {
            return;
        };
        if crate::new_window::finished() {
            return;
        }
        if let Some(receiver) = &task.receiver {
            match receiver.try_recv() {
                Ok(result) => {
                    if !self.files.iter().any(|file| {
                        file.id == task.id
                            && file.path == task.path
                            && revision(file) == task.revision
                    }) {
                        return;
                    }
                    match result.and_then(|file| {
                        linkage::validate_loaded(&file)?;
                        Ok(file)
                    }) {
                        Ok(mut loaded) => {
                            let file = self
                                .files
                                .iter_mut()
                                .find(|file| file.id == task.id)
                                .unwrap();
                            let line = file.editor_state.cursor_line;
                            loaded.id = file.id;
                            loaded.pinned = file.pinned;
                            loaded.content_revision = file.content_revision + 1;
                            loaded.saved_generation = file.saved_generation + 1;
                            loaded.editor_state.tools.wrap = file.editor_state.tools.wrap;
                            loaded.editor_state.tools.whitespace =
                                file.editor_state.tools.whitespace;
                            loaded.language_override = file.language_override.clone();
                            if !loaded.is_binary {
                                loaded.view = file.view;
                            }
                            if let Some(text) = &loaded.draft_text {
                                let line = line.min(loaded.line_count().saturating_sub(1));
                                let start = if loaded.editor_state.large.active {
                                    loaded
                                        .editor_state
                                        .large
                                        .line_start(text, line)
                                        .unwrap_or(0)
                                } else {
                                    text.split_inclusive('\n')
                                        .take(line)
                                        .map(|line| line.chars().count())
                                        .sum()
                                };
                                loaded.editor_state.pending_selection = Some(start..start);
                                loaded.editor_state.cursor = Some(egui::text::CCursorRange::one(
                                    egui::text::CCursor::new(start),
                                ));
                            }
                            *file = loaded;
                            for name in ["draft_editor", "large_editor"] {
                                egui::text_edit::TextEditState::default()
                                    .store(ctx, egui::Id::new(name).with(file.id));
                            }
                            if self.active_id == Some(file.id) {
                                self.selection = None;
                                self.drag_anchor = None;
                                self.inspector_popup = None;
                                self.focus_editor = Some(file.id);
                            }
                            self.backup_or_report();
                            ctx.request_repaint();
                            return;
                        }
                        Err(error) => {
                            self.diagnostic_event("ERROR", "reload", &format!("Reload failed: tab_id={}; kind={:?}; os_code={:?}", task.id, error.kind(), error.raw_os_error()));
                            task.error =
                                Some(format!("{}: {error}", crate::tr(self.lang).reload_failed));
                            task.receiver = None;
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.diagnostic_event("ERROR", "reload", "Reload worker disconnected; working copy retained");
                    task.error = Some(format!(
                        "{}: Reload worker stopped; working copy retained",
                        crate::tr(self.lang).reload_failed
                    ));
                    task.receiver = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let zh = self.lang == Lang::ZhCn;
        let mut retry = false;
        let mut cancel = false;
        egui::Window::new(if zh {
            "从磁盘重新加载"
        } else {
            "Reload from disk"
        })
        .id(egui::Id::new("reload_file"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.memory_mut(|memory| memory.set_modal_layer(ui.layer_id()));
            ui.set_max_width(460.0);
            ui.add(egui::Label::new(task.path.to_string_lossy()).wrap());
            if let Some(error) = &task.error {
                ui.add(egui::Label::new(error).wrap().selectable(true));
            } else if task.confirm {
                ui.add(
                    egui::Label::new(if zh {
                        "重新加载会丢弃当前未保存改动、标记和撤销历史，是否继续？"
                    } else {
                        "Reload discards unsaved changes, marks and undo history. Continue?"
                    })
                    .wrap(),
                );
            } else {
                let phase = task.phase.load(Ordering::Relaxed);
                if phase == 0 {
                    let total = task.total.load(Ordering::Relaxed);
                    let done = task.progress.load(Ordering::Relaxed);
                    ui.add(
                        egui::ProgressBar::new(if total == 0 {
                            0.0
                        } else {
                            done as f32 / total as f32
                        })
                        .text(format!(
                            "{} / {}",
                            crate::format_size(done),
                            crate::format_size(total)
                        )),
                    );
                } else {
                    ui.spinner();
                    ui.label(if phase == 1 {
                        if zh { "正在解码" } else { "Decoding" }
                    } else if zh {
                        "正在建立索引"
                    } else {
                        "Indexing"
                    });
                }
            }
            ui.horizontal(|ui| {
                if task.receiver.is_none() {
                    retry = ui
                        .button(if task.error.is_some() {
                            if zh { "重新读取" } else { "Retry" }
                        } else if zh {
                            "确认重新加载"
                        } else {
                            "Reload"
                        })
                        .clicked();
                }
                cancel = ui.button(crate::tr(self.lang).cancel).clicked();
            });
        });
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            cancel = true;
        }
        if !cancel {
            let waiting = task.receiver.is_some();
            self.reload_task = Some(task);
            if retry {
                self.begin_reload();
            }
            if waiting || retry {
                ctx.request_repaint_after(std::time::Duration::from_millis(30));
            }
        }
    }
}

#[cfg(test)]
#[path = "reload_tests.rs"]
mod tests;

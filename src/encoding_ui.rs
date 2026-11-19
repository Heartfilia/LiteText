//! Explicit, transactional encoding actions. Disk bytes change only on manual save.
use crate::{Lang, LiteTextApp, LoadedFile, Palette, recovery::Encoding};
use eframe::egui;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc,
};

pub(crate) struct Task {
    id: u64,
    path: std::path::PathBuf,
    revision: (u64, u64, u64),
    encoding: Encoding,
    reopen: bool,
    cancel: Arc<AtomicBool>,
    receiver: Option<mpsc::Receiver<std::io::Result<ResultData>>>,
    error: Option<String>,
}

#[cfg(test)]
#[path = "encoding_ui_tests.rs"]
mod tests;

enum ResultData {
    Reopened(Box<LoadedFile>),
    Converted(Vec<u8>, (usize, usize)),
}

impl Drop for Task {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub(crate) fn picker(
    ui: &mut egui::Ui,
    file: &LoadedFile,
    lang: Lang,
    pal: &Palette,
    enabled: bool,
) -> Option<(bool, Encoding)> {
    let zh = lang == Lang::ZhCn;
    let name = if file.encoding == Encoding::Lossy {
        crate::tr(lang).encoding_unknown
    } else {
        file.encoding.name()
    };
    let galley = ui.painter().layout_no_wrap(
        name.into(),
        egui::FontId::proportional(12.0),
        pal.text_faint,
    );
    let (rect, _) = ui.allocate_exact_size(egui::vec2(galley.size().x, 14.0), egui::Sense::hover());
    let id = egui::Id::new("status_encoding");
    // Extend the clickable area to the full bar height; the painted 14 px text
    // strip alone leaves dead space above and below. Outside-click checks use
    // the same rect so a padded click does not open and instantly close it.
    let hit = rect.expand2(egui::vec2(3.0, 5.0)).intersect(ui.clip_rect());
    let response = ui
        .interact(hit, id, egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let open = enabled && ui.memory(|memory| memory.is_popup_open(id));
    if (response.hovered() || open) && enabled {
        ui.painter()
            .rect_filled(rect.expand2(egui::vec2(2.0, 3.0)), 4.0, pal.bg_elevated);
    }
    ui.painter()
        .galley(rect.center() - galley.size() * 0.5, galley, pal.text_faint);
    let response = response.on_hover_text(if zh {
        "编码：指定编码重新打开，或转换下次保存编码（不自动保存）"
    } else {
        "Encoding: reopen disk bytes or convert the next save; never auto-saved"
    });
    ui.ctx()
        .data_mut(|data| data.insert_temp(id.with("rect"), rect));
    if !enabled {
        if ui.memory(|memory| memory.is_popup_open(id)) {
            ui.memory_mut(|memory| memory.close_popup());
        }
        return None;
    }
    if response.clicked() {
        ui.memory_mut(|memory| memory.toggle_popup(id));
    }
    let mut picked = None;
    if ui.memory(|memory| memory.is_popup_open(id)) {
        let popup = egui::Area::new(id.with("popup")).order(egui::Order::Foreground).pivot(egui::Align2::LEFT_BOTTOM).fixed_pos(rect.left_top() - egui::vec2(0.0, 8.0))
            // Opaque from the first visible frame; a fading menu overlaps the text behind it.
            .fade_in(false)
            .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).fill(pal.bg_panel).show(ui, |ui| {
                ui.horizontal(|ui| {
                    for reopen in [true, false] {
                        ui.vertical(|ui| {
                            ui.label(if reopen { if zh { "用指定编码重新打开" } else { "Reopen with encoding" } } else if zh { "转换保存编码" } else { "Convert save encoding" });
                            for encoding in Encoding::CHOICES {
                                let allowed = if reopen { !file.path.as_os_str().is_empty() } else { file.encoding != Encoding::Lossy && encoding != file.encoding };
                                let row = ui.add_enabled(allowed, egui::Button::new(encoding.name()).selected(!reopen && encoding == file.encoding));
                                ui.ctx().data_mut(|data| data.insert_temp(id.with((reopen, encoding.name())), row.rect));
                                if row.clicked() { picked = Some((reopen, encoding)); }
                            }
                        });
                    }
                });
                ui.label(egui::RichText::new(if zh { "BOM 变体要求匹配的 BOM。未知／有损文本请先重新打开修正乱码。" } else { "BOM variants require a matching BOM. Reopen unknown/lossy input before conversion." }).small());
            });
        });
        if picked.is_some()
            || ui.input(|input| {
                input.key_pressed(egui::Key::Escape)
                    || input.pointer.any_click()
                        && input.pointer.interact_pos().is_some_and(|pos| {
                            !hit.contains(pos) && !popup.response.rect.contains(pos)
                        })
            })
        {
            ui.memory_mut(|memory| memory.close_popup());
        }
    }
    picked
}

impl LiteTextApp {
    pub(crate) fn request_encoding(&mut self, reopen: bool, encoding: Encoding) {
        let Some(file) = self.active() else {
            return;
        };
        if self.status_controls_blocked()
            || file.draft_text.is_none()
            || encoding == Encoding::Lossy
            || reopen && file.path.as_os_str().is_empty()
        {
            return;
        }
        self.encoding_task = Some(Task {
            id: file.id,
            path: file.path.clone(),
            revision: (
                file.content_revision,
                file.editor_state.large.revision,
                file.saved_generation,
            ),
            encoding,
            reopen,
            cancel: Arc::new(AtomicBool::new(false)),
            receiver: None,
            error: None,
        });
    }

    pub(crate) fn begin_encoding(&mut self) {
        let Some(task) = &mut self.encoding_task else {
            return;
        };
        if task.receiver.is_some() {
            return;
        }
        let Some(file) = self.files.iter().find(|file| {
            file.id == task.id
                && file.path == task.path
                && (
                    file.content_revision,
                    file.editor_state.large.revision,
                    file.saved_generation,
                ) == task.revision
        }) else {
            self.encoding_task = None;
            return;
        };
        let text = (!task.reopen).then(|| file.draft_text.clone().unwrap());
        let source_encoding = file.encoding;
        let (path, reopen, encoding, cancel) = (
            task.path.clone(),
            task.reopen,
            task.encoding,
            task.cancel.clone(),
        );
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let _timing = crate::timing::phase("encoding.read_validate_prepare");
            let result = if reopen {
                crate::background_io::load_with_encoding(
                    &path,
                    &cancel,
                    &AtomicU64::new(0),
                    &AtomicU64::new(0),
                    Some(encoding),
                )
                .map(|file| ResultData::Reopened(Box::new(file)))
            } else if source_encoding == Encoding::Lossy {
                Err(std::io::Error::other(
                    "Unknown/lossy input must be reopened before conversion",
                ))
            } else {
                let text = text.unwrap();
                encoding.encode(&text).map(|bytes| {
                    let metrics = if encoding.is_legacy() {
                        (bytes.len(), 0)
                    } else {
                        (0, 0)
                    };
                    ResultData::Converted(bytes, metrics)
                })
            };
            let _ = sender.send(result);
        });
        task.receiver = Some(receiver);
        task.error = None;
    }

    pub(crate) fn encoding_dialog(&mut self, ctx: &egui::Context) {
        let Some(task) = self.encoding_task.take() else {
            return;
        };
        if crate::new_window::finished() {
            return;
        }
        if let Some(receiver) = &task.receiver {
            match receiver.try_recv() {
                Ok(result) => {
                    let result = result.and_then(|result| {
                        if let ResultData::Reopened(file) = &result {
                            crate::linkage::validate_loaded(file)?;
                        }
                        Ok(result)
                    });
                    if result
                        .as_ref()
                        .is_err_and(|error| error.kind() == std::io::ErrorKind::WouldBlock)
                    {
                        let mut task = task;
                        task.error = Some(result.err().unwrap().to_string());
                        task.receiver = None;
                        self.encoding_task = Some(task);
                        ctx.request_repaint();
                        return;
                    }
                    if let Some(file) = self.files.iter_mut().find(|file| {
                        file.id == task.id
                            && file.path == task.path
                            && (
                                file.content_revision,
                                file.editor_state.large.revision,
                                file.saved_generation,
                            ) == task.revision
                    }) {
                        match result {
                            Ok(ResultData::Reopened(mut loaded)) => {
                                loaded.id = file.id;
                                loaded.pinned = file.pinned;
                                // Reopening discards the working copy and its
                                // edit mirror.
                                if let Some(mirror) = file.scratch_path.take() {
                                    let _ = crate::scratch::remove(&mirror);
                                }
                                loaded.content_revision = file.content_revision + 1;
                                loaded.saved_generation = file.saved_generation + 1;
                                loaded.editor_state.tools.wrap = file.editor_state.tools.wrap;
                                loaded.editor_state.tools.whitespace =
                                    file.editor_state.tools.whitespace;
                                loaded.language_override = file.language_override.clone();
                                *file = *loaded;
                                for name in ["draft_editor", "large_editor"] {
                                    egui::text_edit::TextEditState::default()
                                        .store(ctx, egui::Id::new(name).with(file.id));
                                }
                                if self.active_id == Some(file.id) {
                                    self.focus_editor = Some(file.id);
                                }
                            }
                            Ok(ResultData::Converted(bytes, metrics)) => {
                                file.encoding = task.encoding;
                                file.bytes = bytes;
                                file.bytes_dirty = false;
                                file.editor_state
                                    .large
                                    .install_encoding_metrics(task.encoding, metrics);
                                file.content_revision += 1;
                            }
                            Err(error) => self.io_error = Some(error.to_string()),
                        }
                    }
                    self.last_recovery = None;
                    ctx.request_repaint();
                    return;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.io_error = Some("Encoding worker stopped; working copy retained".into());
                    return;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let waiting = task.receiver.is_some();
        let zh = self.lang == Lang::ZhCn;
        let mut confirm = false;
        let mut cancel = false;
        egui::Window::new(if zh { "文本编码" } else { "Text encoding" }).id(egui::Id::new("encoding_action")).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            ui.memory_mut(|memory| memory.set_modal_layer(ui.layer_id()));
            ui.label(task.encoding.name());
            if let Some(error) = &task.error { ui.add(egui::Label::new(error).wrap().selectable(true)); }
            if waiting { ui.spinner(); }
            else if task.reopen {
                ui.label(if zh { "从磁盘重新读取并解码，将丢弃当前未保存改动、选区、标记和撤销历史。原文件不会改写；失败保留工作区。" } else { "Reread disk bytes. This discards unsaved edits, selection, marks and undo history. Disk is never rewritten; failure retains the workspace." });
            } else {
                ui.label(if zh { "保留当前文字和撤销历史，改变下次保存编码。无法表示的字符会报错，不替换为 ?。只在手动保存时修改原文件。" } else { "Keep text and undo history; change the next save encoding. Unrepresentable characters are rejected, never replaced with ?. Disk changes only on manual save." });
            }
            ui.horizontal(|ui| {
                if !waiting {
                    let response = ui.button(if task.error.is_some() { if zh { "重新读取" } else { "Retry" } } else if zh { "确认" } else { "Confirm" });
                    ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("encoding_confirm_rect"), response.rect));
                    confirm = response.clicked();
                }
                let response = ui.button(crate::tr(self.lang).cancel);
                ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("encoding_cancel_rect"), response.rect));
                cancel = response.clicked();
            });
        });
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            cancel = true;
        }
        if !cancel {
            self.encoding_task = Some(task);
            if confirm {
                self.begin_encoding();
            }
            if waiting || confirm {
                ctx.request_repaint_after(std::time::Duration::from_millis(30));
            }
        }
    }
}

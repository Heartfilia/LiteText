//! Per-document newline detection, explicit undoable conversion and Enter input.
use crate::{Lang, LoadedFile, Palette};
use eframe::egui;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum LineEnding {
    #[default]
    Lf,
    Crlf,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Counts {
    pub lf: usize,
    pub crlf: usize,
}

impl Counts {
    pub(crate) fn of(text: &str) -> Self {
        Self::from_bytes(text.bytes(), None)
    }

    pub(crate) fn from_bytes(
        bytes: impl IntoIterator<Item = u8>,
        mut previous: Option<u8>,
    ) -> Self {
        let mut counts = Self::default();
        for byte in bytes {
            if byte == b'\n' {
                if previous == Some(b'\r') {
                    counts.crlf += 1;
                } else {
                    counts.lf += 1;
                }
            }
            previous = Some(byte);
        }
        counts
    }

    pub(crate) fn preferred(self, fallback: LineEnding) -> LineEnding {
        if self.lf == 0 && self.crlf == 0 {
            fallback
        } else if self.crlf > self.lf {
            LineEnding::Crlf
        } else {
            LineEnding::Lf
        }
    }

    pub(crate) fn label(self, fallback: LineEnding, zh: bool) -> &'static str {
        if self.lf > 0 && self.crlf > 0 {
            if zh { "混合" } else { "Mixed" }
        } else {
            self.preferred(fallback).label()
        }
    }
}

impl LineEnding {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Lf => "LF",
            Self::Crlf => "CRLF",
        }
    }

    pub(crate) fn remap_enter(
        self,
        ctx: &egui::Context,
        id: egui::Id,
        composing: &mut bool,
        text: &str,
        cursor: Option<egui::text::CCursorRange>,
    ) {
        if !ctx.memory(|m| m.has_focus(id)) {
            *composing = false;
            return;
        }
        ctx.input_mut(|i| {
            // IME confirmation and its Enter can arrive in the same frame.
            let ime_frame = i
                .events
                .iter()
                .any(|event| matches!(event, egui::Event::Ime(_)));
            if ime_frame || *composing {
                // IME commits its own text. Do not also insert a document line
                // break from the physical key used to confirm the candidate.
                i.events.retain(|event| {
                    !matches!(
                        event,
                        egui::Event::Key {
                            key: egui::Key::Enter,
                            pressed: true,
                            ..
                        }
                    )
                });
            }
            for event in &mut i.events {
                match event {
                    egui::Event::Ime(egui::ImeEvent::Enabled | egui::ImeEvent::Preedit(_)) => {
                        *composing = true
                    }
                    egui::Event::Ime(egui::ImeEvent::Disabled | egui::ImeEvent::Commit(_)) => {
                        *composing = false
                    }
                    egui::Event::Key {
                        key: egui::Key::Enter,
                        pressed: true,
                        modifiers,
                        ..
                    } if modifiers.is_none() && !*composing && !ime_frame => {
                        let cursor = cursor.unwrap_or_default();
                        let indent = crate::indentation::inherited(
                            text,
                            cursor.primary.index.min(cursor.secondary.index),
                        );
                        // Paste is an editor insertion event, not a clipboard read.
                        *event = egui::Event::Paste(format!(
                            "{}{indent}",
                            if self == Self::Crlf { "\r\n" } else { "\n" }
                        ));
                    }
                    _ => {}
                }
            }
        });
    }
}

pub(crate) fn convert(file: &mut LoadedFile, ctx: &egui::Context, ending: LineEnding) {
    if file.is_binary || file.view != crate::ViewMode::Text {
        return;
    }
    let Some(text) = file.draft_text.as_deref() else {
        return;
    };
    let selected = crate::line_commands::selected_bytes(file, text);
    let edits = text
        .bytes()
        .enumerate()
        .filter_map(|(at, byte)| {
            if byte != b'\n' {
                return None;
            }
            let crlf = at > 0 && text.as_bytes()[at - 1] == b'\r';
            match (ending, crlf) {
                (LineEnding::Lf, true) => Some((at - 1..at, String::new())),
                (LineEnding::Crlf, false) => Some((at..at, "\r".to_owned())),
                _ => None,
            }
        })
        .collect();
    crate::line_commands::commit_edits(file, ctx, edits, Some(selected));
    type Handoff = std::sync::Arc<std::sync::Mutex<Option<crate::line_commands::PendingBatch>>>;
    let pending = ctx.data(|data| data.get_temp::<Handoff>(egui::Id::new("prepared_line_batch")));
    if let Some(pending) = pending {
        if let Some(job) = pending.lock().unwrap().as_mut() {
            job.line_ending = Some(ending);
        }
    } else {
        file.line_ending = ending;
        file.content_revision += 1; // Also persist the preference of an empty document.
    }
    ctx.request_repaint();
}

pub(crate) fn picker(
    ui: &mut egui::Ui,
    file: &LoadedFile,
    lang: Lang,
    pal: &Palette,
    enabled: bool,
) -> Option<LineEnding> {
    if file.is_binary || file.view != crate::ViewMode::Text {
        return None;
    }
    let zh = lang == Lang::ZhCn;
    let counts = file.newline_counts();
    let current = counts.preferred(file.line_ending);
    let id = egui::Id::new("status_line_ending");
    let text = format!("{}  ▴", counts.label(file.line_ending, zh));
    let galley =
        ui.painter()
            .layout_no_wrap(text, egui::FontId::proportional(12.0), pal.text_faint);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(galley.size().x + 8.0, 14.0),
        egui::Sense::hover(),
    );
    // Extend the clickable area to the full bar height; the painted 14 px text
    // strip alone leaves dead space above and below. Outside-click checks use
    // the same rect so a padded click does not open and instantly close it.
    let hit = rect.expand2(egui::vec2(3.0, 5.0)).intersect(ui.clip_rect());
    let response = ui
        .interact(hit, id, egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            enabled,
            if zh { "换行格式" } else { "Line endings" },
        )
    });
    let response = response.on_hover_text(if zh {
        format!("LF: {}，CRLF: {}。回车使用 {}；选择格式将转换当前全文，可撤销，不自动保存。", counts.lf, counts.crlf, current.label())
    } else {
        format!("LF: {}, CRLF: {}. Enter uses {}. Choose to convert this document; undoable, never auto-saved.", counts.lf, counts.crlf, current.label())
    });
    let open = enabled && ui.memory(|m| m.is_popup_open(id));
    if (response.hovered() || open) && enabled {
        ui.painter()
            .rect_filled(rect.expand2(egui::vec2(2.0, 3.0)), 4.0, pal.bg_elevated);
    }
    ui.painter()
        .galley(rect.center() - galley.size() * 0.5, galley, pal.text_faint);
    ui.ctx().data_mut(|d| d.insert_temp(id.with("rect"), rect));
    if !enabled && ui.memory(|m| m.is_popup_open(id)) {
        ui.memory_mut(|m| m.close_popup());
    }
    if enabled && response.clicked() {
        ui.memory_mut(|m| m.toggle_popup(id));
    }
    let mut picked = None;
    if enabled && ui.memory(|m| m.is_popup_open(id)) {
        let popup = egui::Area::new(id.with("popup"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::LEFT_BOTTOM)
            .fixed_pos(rect.left_top() - egui::vec2(0.0, 8.0))
            // Opaque from the first visible frame; a fading menu overlaps the text behind it.
            .fade_in(false)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style())
                    .fill(pal.bg_panel)
                    .show(ui, |ui| {
                        for choice in [LineEnding::Lf, LineEnding::Crlf] {
                            let row = ui.add_sized(
                                [64.0, 26.0],
                                egui::Button::new(choice.label()).selected(
                                    current == choice && !(counts.lf > 0 && counts.crlf > 0),
                                ),
                            );
                            ui.ctx()
                                .data_mut(|d| d.insert_temp(id.with(choice.label()), row.rect));
                            if row.clicked() {
                                picked = Some(choice);
                            }
                        }
                    });
            });
        if picked.is_some()
            || ui.input(|i| {
                i.key_pressed(egui::Key::Escape)
                    || i.pointer.any_click()
                        && i.pointer
                            .interact_pos()
                            .is_some_and(|p| !hit.contains(p) && !popup.response.rect.contains(p))
            })
        {
            ui.memory_mut(|m| m.close_popup());
        }
    }
    picked
}

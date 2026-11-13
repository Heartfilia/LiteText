//! One source of truth for the default keymap and its settings reference.
use crate::{Lang, LiteTextApp, Palette, TabAction, ViewMode, tr};
use eframe::egui::{self, Key, KeyboardShortcut, Modifiers};

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub(crate) enum Action {
    New,
    Open,
    Save,
    Close,
    NextTab,
    PreviousTab,
    Settings,
    ZoomIn,
    ZoomOut,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    DeleteLine,
    Comment,
    Find,
    FocusReplace,
    NextMatch,
    PreviousMatch,
    Replace,
    ReplaceAll,
    Count,
    HexCase,
    DimZeros,
    Ascii,
    Invisible,
}

pub(crate) const ACTIONS: &[Action] = &[
    Action::New,
    Action::Open,
    Action::Save,
    Action::Close,
    Action::NextTab,
    Action::PreviousTab,
    Action::Settings,
    Action::ZoomIn,
    Action::ZoomOut,
    Action::Undo,
    Action::Redo,
    Action::Cut,
    Action::Copy,
    Action::Paste,
    Action::SelectAll,
    Action::DeleteLine,
    Action::Comment,
    Action::Find,
    Action::FocusReplace,
    Action::NextMatch,
    Action::PreviousMatch,
    Action::Replace,
    Action::ReplaceAll,
    Action::Count,
    Action::HexCase,
    Action::DimZeros,
    Action::Ascii,
    Action::Invisible,
];

impl Action {
    pub(crate) fn binding(self) -> KeyboardShortcut {
        self.binding_for(cfg!(target_os = "macos"))
    }

    pub(crate) fn binding_for(self, mac: bool) -> KeyboardShortcut {
        use Action::*;
        let command = Modifiers::COMMAND;
        let shift = command | Modifiers::SHIFT;
        let tool = command | Modifiers::ALT;
        let (modifiers, key) = match self {
            New => (command, Key::N),
            Open => (command, Key::O),
            Save => (command, Key::S),
            Close => (command, Key::W),
            Settings => (command, Key::Comma),
            ZoomIn => (command, Key::Plus),
            ZoomOut => (command, Key::Minus),
            NextTab => (Modifiers::CTRL, Key::Tab),
            PreviousTab => (Modifiers::CTRL | Modifiers::SHIFT, Key::Tab),
            Undo => (command, Key::Z),
            Redo => (shift, Key::Z),
            Cut => (command, Key::X),
            Copy => (command, Key::C),
            Paste => (command, Key::V),
            SelectAll => (command, Key::A),
            DeleteLine => (command, if mac { Key::Backspace } else { Key::Y }),
            Comment => (command, Key::Slash),
            Find => (command, Key::F),
            FocusReplace => (tool, Key::F),
            NextMatch => (Modifiers::NONE, Key::F3),
            PreviousMatch => (Modifiers::SHIFT, Key::F3),
            Replace => (tool, Key::Enter),
            ReplaceAll => (tool | Modifiers::SHIFT, Key::Enter),
            // C/X/V are intercepted as clipboard events by egui-winit even with
            // extra modifiers. Command also prevents Option+Z inserting text on macOS.
            Count => (tool, Key::K),
            HexCase => (tool, Key::U),
            DimZeros => (tool, Key::Num0),
            Ascii => (tool, Key::A),
            Invisible => (tool, Key::I),
        };
        KeyboardShortcut::new(modifiers, key)
    }

    fn group(self) -> usize {
        use Action::*;
        match self {
            New | Open | Save | Close | NextTab | PreviousTab | Settings | ZoomIn | ZoomOut => 0,
            Undo | Redo | Cut | Copy | Paste | SelectAll | DeleteLine | Comment => 1,
            Find | FocusReplace | NextMatch | PreviousMatch | Replace | ReplaceAll | Count => 2,
            HexCase | DimZeros | Ascii | Invisible => 3,
        }
    }

    pub(crate) fn label(self, lang: Lang) -> String {
        use Action::*;
        let t = tr(lang);
        let zh = lang == Lang::ZhCn;
        let label = match self {
            New => t.new_file,
            Open => t.open_file,
            Save => t.save,
            Close => t.close,
            Settings => t.settings,
            ZoomIn => {
                if zh {
                    "放大工作区"
                } else {
                    "Zoom in workspace"
                }
            }
            ZoomOut => {
                if zh {
                    "缩小工作区"
                } else {
                    "Zoom out workspace"
                }
            }
            Find => t.find_replace,
            Replace => t.replace,
            ReplaceAll => t.replace_all,
            Count => t.count_matches,
            HexCase => t.hex_case,
            DimZeros => t.dim_zeros,
            Ascii => t.toggle_ascii,
            Invisible => t.show_invisible,
            NextTab => {
                if zh {
                    "下一个标签页"
                } else {
                    "Next tab"
                }
            }
            PreviousTab => {
                if zh {
                    "上一个标签页"
                } else {
                    "Previous tab"
                }
            }
            Undo => {
                if zh {
                    "撤销"
                } else {
                    "Undo"
                }
            }
            Redo => {
                if zh {
                    "重做"
                } else {
                    "Redo"
                }
            }
            Cut => {
                if zh {
                    "剪切"
                } else {
                    "Cut"
                }
            }
            Copy => t.copy,
            Paste => {
                if zh {
                    "粘贴"
                } else {
                    "Paste"
                }
            }
            SelectAll => {
                if zh {
                    "全选"
                } else {
                    "Select all"
                }
            }
            DeleteLine => {
                if zh {
                    "整行删除"
                } else {
                    "Delete line"
                }
            }
            Comment => {
                if zh {
                    "切换注释"
                } else {
                    "Toggle comment"
                }
            }
            FocusReplace => {
                if zh {
                    "聚焦替换输入框"
                } else {
                    "Focus replacement"
                }
            }
            NextMatch => {
                if zh {
                    "下一个匹配"
                } else {
                    "Next match"
                }
            }
            PreviousMatch => {
                if zh {
                    "上一个匹配"
                } else {
                    "Previous match"
                }
            }
        };
        label.to_owned()
    }

    fn native_edit(self) -> bool {
        matches!(
            self,
            Self::Undo | Self::Redo | Self::Cut | Self::Copy | Self::Paste | Self::SelectAll
        )
    }
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct Keymap {
    // Missing = default, null = unbound; old serialized bindings remain compatible.
    overrides: std::collections::BTreeMap<Action, Option<KeyboardShortcut>>,
}

impl Keymap {
    pub(crate) fn binding(&self, action: Action) -> Option<KeyboardShortcut> {
        self.overrides
            .get(&action)
            .copied()
            .unwrap_or_else(|| Some(action.binding()))
    }

    fn set(&mut self, action: Action, binding: KeyboardShortcut) -> Result<(), Action> {
        if let Some(other) = ACTIONS.iter().copied().find(|other| {
            *other != action
                && self
                    .binding(*other)
                    .is_some_and(|current| equivalent(current, binding))
        }) {
            return Err(other);
        }
        if equivalent(binding, action.binding()) {
            self.overrides.remove(&action);
        } else {
            self.overrides.insert(action, Some(binding));
        }
        Ok(())
    }

    fn clear(&mut self, action: Action) {
        self.overrides.insert(action, None);
    }
}

// Logical '+' / Shift+'=' and '?' / Shift+'/' vary with keyboard layout.
fn normalized(mut binding: KeyboardShortcut) -> KeyboardShortcut {
    if matches!(binding.logical_key, Key::Equals | Key::Plus) {
        binding.logical_key = Key::Plus;
        binding.modifiers.shift = false;
    }
    if binding.logical_key == Key::Slash && binding.modifiers.shift {
        binding.logical_key = Key::Questionmark;
    }
    if binding.logical_key == Key::Questionmark {
        binding.modifiers.shift = false;
    }
    binding
}

fn equivalent(a: KeyboardShortcut, b: KeyboardShortcut) -> bool {
    let a = normalized(a);
    let b = normalized(b);
    a.logical_key == b.logical_key && a.modifiers.matches_exact(b.modifiers)
}

fn event_matches(
    event: &egui::Event,
    binding: impl Into<Option<KeyboardShortcut>>,
    current: Modifiers,
) -> bool {
    let Some(binding) = binding.into() else {
        return false;
    };
    let candidate = match event {
        egui::Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } => KeyboardShortcut::new(*modifiers, *key),
        egui::Event::Copy => KeyboardShortcut::new(current, Key::C),
        egui::Event::Cut => KeyboardShortcut::new(current, Key::X),
        egui::Event::Paste(_) => KeyboardShortcut::new(current, Key::V),
        _ => return false,
    };
    equivalent(candidate, binding)
}

/// Exact modifiers avoid Shift+F3 also firing F3, or tool chords firing Save/Copy.
#[cfg(test)]
pub(crate) fn take(ctx: &egui::Context, action: Action) -> bool {
    take_binding(ctx, action.binding())
}

pub(crate) fn take_binding(
    ctx: &egui::Context,
    binding: impl Into<Option<KeyboardShortcut>>,
) -> bool {
    let Some(binding) = binding.into() else {
        return false;
    };
    let mut found = false;
    ctx.input_mut(|input| {
        let current_modifiers = input.modifiers;
        input.events.retain(|event| {
            let matches = event_matches(event, binding, current_modifiers);
            found |= matches && !matches!(event, egui::Event::Key { repeat: true, .. });
            !matches
        });
        if found && !binding.modifiers.command && !binding.modifiers.ctrl {
            input
                .events
                .retain(|event| !matches!(event, egui::Event::Text(_)));
        }
    });
    found
}

impl LiteTextApp {
    fn zoom_workspace(&mut self, delta: f32, ctx: &egui::Context) {
        self.workspace_font_size = (self.workspace_font_size + delta).clamp(10.0, 32.0);
        ctx.request_repaint();
    }

    pub(crate) fn capture_shortcut(&mut self, ctx: &egui::Context) {
        let Some(action) = self.shortcut_recording else {
            return;
        };
        let events = ctx.input_mut(|i| {
            let mut events = Vec::new();
            i.events.retain(|event| {
                if matches!(
                    event,
                    egui::Event::Key { .. }
                        | egui::Event::Text(_)
                        | egui::Event::Paste(_)
                        | egui::Event::Copy
                        | egui::Event::Cut
                ) {
                    events.push(event.clone());
                    false
                } else {
                    true
                }
            });
            events
        });
        for event in events {
            if let egui::Event::Key {
                key,
                modifiers,
                pressed: true,
                repeat: false,
                ..
            } = event
            {
                if key == Key::Escape {
                    self.shortcut_recording = None;
                    self.shortcut_error = None;
                    break;
                }
                if matches!(key, Key::Delete | Key::Backspace)
                    && modifiers.matches_exact(Modifiers::NONE)
                {
                    self.shortcut_keys.clear(action);
                    self.shortcut_recording = None;
                    self.shortcut_error = None;
                    break;
                }
                let mut modifiers = modifiers;
                if modifiers.command {
                    modifiers.ctrl = false;
                    modifiers.mac_cmd = false;
                }
                match self
                    .shortcut_keys
                    .set(action, normalized(KeyboardShortcut::new(modifiers, key)))
                {
                    Ok(()) => {
                        self.shortcut_recording = None;
                        self.shortcut_error = None;
                    }
                    Err(other) => {
                        self.shortcut_error = Some(format!(
                            "{}: {}",
                            if self.lang == Lang::ZhCn {
                                "按键已用于"
                            } else {
                                "Already assigned to"
                            },
                            other.label(self.lang)
                        ))
                    }
                }
                break;
            }
            // Clipboard keys are translated before reaching us on desktop.
            let key = match event {
                egui::Event::Copy => Some(Key::C),
                egui::Event::Cut => Some(Key::X),
                egui::Event::Paste(_) => Some(Key::V),
                _ => None,
            };
            if let Some(key) = key {
                let mut modifiers = ctx.input(|i| i.modifiers);
                if modifiers.command {
                    modifiers.ctrl = false;
                    modifiers.mac_cmd = false;
                }
                match self
                    .shortcut_keys
                    .set(action, KeyboardShortcut::new(modifiers, key))
                {
                    Ok(()) => {
                        self.shortcut_recording = None;
                        self.shortcut_error = None;
                    }
                    Err(other) => {
                        self.shortcut_error = Some(format!(
                            "{}: {}",
                            if self.lang == Lang::ZhCn {
                                "按键已用于"
                            } else {
                                "Already assigned to"
                            },
                            other.label(self.lang)
                        ))
                    }
                }
                break;
            }
        }
    }

    pub(crate) fn shortcuts_blocked(&self) -> bool {
        self.show_settings
            || self.plugin_error.is_some()
            || self.pending_close.is_some()
            || self.pending_window_close
            || self.pending_save.is_some()
            || self.files.iter().any(|file| file.disk.pending.is_some())
            || self.inspector_popup.is_some()
    }

    /// Returns a deferred save flag so input in this frame is included in the save.
    pub(crate) fn handle_shortcuts(&mut self, ctx: &egui::Context) -> bool {
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("app_keymap_managed"), true));
        if self.shortcuts_blocked() {
            return false;
        }
        let mut save = false;
        for &action in ACTIONS {
            // These remain owned by the focused TextEdit, including settings inputs.
            if action.native_edit() {
                continue;
            }
            let text = self
                .active()
                .is_some_and(|f| f.view == ViewMode::Text && f.draft_text.is_some());
            let hex = self.active().is_some_and(|f| f.view == ViewMode::Hex);
            if (action.group() == 2 || matches!(action, Action::DeleteLine | Action::Comment))
                && !text
            {
                continue;
            }
            if action.group() == 3 && !hex {
                continue;
            }
            if !take_binding(ctx, self.shortcut_keys.binding(action)) {
                continue;
            }
            use Action::*;
            match action {
                ZoomIn => self.zoom_workspace(1.0, ctx),
                ZoomOut => self.zoom_workspace(-1.0, ctx),
                New => self.new_document(),
                Open => self.open_dialog(),
                Save => save = true,
                Close => {
                    if let Some(id) = self.active_id {
                        self.request_tab_action(TabAction::Close(id));
                    }
                }
                Settings => {
                    self.show_settings = true;
                    self.shortcuts_page = false;
                    self.workspace_theme_page = false;
                }
                NextTab | PreviousTab => {
                    if let Some(pos) = self.files.iter().position(|f| Some(f.id) == self.active_id)
                    {
                        let next =
                            (pos + if action == NextTab {
                                1
                            } else {
                                self.files.len() - 1
                            }) % self.files.len();
                        self.active_id = Some(self.files[next].id);
                        self.focus_editor = self.active_id;
                        self.selection = None;
                        self.drag_anchor = None;
                    }
                }
                DeleteLine | Comment => {
                    if let Some(file) = self.active_mut() {
                        let id = egui::Id::new(if file.editor_state.large.active {
                            "large_editor"
                        } else {
                            "draft_editor"
                        })
                        .with(file.id);
                        if ctx.memory(|m| m.has_focus(id)) || file.editor_state.selection.is_some()
                        {
                            crate::line_commands::apply(file, ctx, action == Comment);
                        }
                    }
                }
                HexCase => self.hex_upper = !self.hex_upper,
                DimZeros => self.dim_zeros = !self.dim_zeros,
                Ascii => self.show_ascii = !self.show_ascii,
                Invisible => self.show_invisible = !self.show_invisible,
                _ => {
                    if let Some(file) = self.active_mut() {
                        match action {
                            Find | FocusReplace => {
                                file.editor_state.tools.find_open = true;
                                file.editor_state.tools.focus_find = Some(action == FocusReplace);
                            }
                            NextMatch | PreviousMatch | Replace | ReplaceAll | Count => {
                                file.editor_state.tools.find_open = true;
                                file.editor_state.tools.pending_action = Some(match action {
                                    PreviousMatch => 1,
                                    NextMatch => 2,
                                    Replace => 3,
                                    ReplaceAll => 4,
                                    _ => 5,
                                });
                            }
                            _ => {}
                        }
                    }
                }
            }
            ctx.request_repaint();
            if self.shortcuts_blocked() {
                break;
            }
        }
        if !self.shortcuts_blocked() {
            self.remap_edit_keys(ctx);
            if let Some(file) = self
                .active()
                .filter(|f| f.view == ViewMode::Text && !f.is_binary)
            {
                let id = egui::Id::new(if file.editor_state.large.active {
                    "large_editor"
                } else {
                    "draft_editor"
                })
                .with(file.id);
                self.tab_mode.remap(ctx, id);
            }
        }
        save
    }

    fn remap_edit_keys(&self, ctx: &egui::Context) {
        let (events, modifiers) = ctx.input_mut(|i| (std::mem::take(&mut i.events), i.modifiers));
        let paste_id = egui::Id::new("shortcut_paste_requested");
        let now = ctx.input(|i| i.time);
        let waiting = ctx
            .data(|d| d.get_temp::<f64>(paste_id))
            .is_some_and(|deadline| now <= deadline);
        let native = [
            Action::Undo,
            Action::Redo,
            Action::Cut,
            Action::Copy,
            Action::Paste,
            Action::SelectAll,
        ];
        let mut output = Vec::new();
        let mut consumed_typing = false;
        for event in events {
            // A requested paste returns asynchronously, potentially after key release.
            if matches!(event, egui::Event::Paste(_))
                && (waiting || !modifiers.command && !modifiers.ctrl)
            {
                ctx.data_mut(|d| d.remove::<f64>(paste_id));
                output.push(event);
                continue;
            }
            if let Some(action) = native.iter().copied().find(|action| {
                event_matches(&event, self.shortcut_keys.binding(*action), modifiers)
            }) {
                let binding = self.shortcut_keys.binding(action).expect("matched binding");
                consumed_typing |= !binding.modifiers.command && !binding.modifiers.ctrl;
                match action {
                    Action::Copy => output.push(egui::Event::Copy),
                    Action::Cut => output.push(egui::Event::Cut),
                    Action::Paste => {
                        if matches!(event, egui::Event::Paste(_)) {
                            output.push(event);
                        } else {
                            ctx.data_mut(|d| d.insert_temp(paste_id, now + 5.0));
                            ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                        }
                    }
                    _ => {
                        let b = action.binding();
                        output.push(egui::Event::Key {
                            key: b.logical_key,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers: b.modifiers,
                        });
                    }
                }
            } else if !native
                .iter()
                .any(|action| event_matches(&event, action.binding(), modifiers))
                && !event_matches(
                    &event,
                    KeyboardShortcut::new(Modifiers::COMMAND, Key::Y),
                    modifiers,
                )
            {
                output.push(event);
            }
        }
        if consumed_typing {
            output.retain(|event| !matches!(event, egui::Event::Text(_)));
        }
        ctx.input_mut(|i| i.events = output);
    }

    pub(crate) fn shortcuts_ui(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let zh = self.lang == Lang::ZhCn;
        let back = ui.button(if zh {
            "‹ 返回系统设置"
        } else {
            "‹ Back to settings"
        });
        ui.ctx()
            .data_mut(|d| d.insert_temp(egui::Id::new("shortcuts_back_rect"), back.rect));
        if back.clicked() {
            self.shortcuts_page = false;
            self.shortcut_recording = None;
            self.shortcut_error = None;
        }
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(tr(self.lang).shortcuts_desc)
                .size(13.0)
                .color(pal.text_faint),
        );
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui
                .button(if zh {
                    "恢复全部默认"
                } else {
                    "Reset all defaults"
                })
                .clicked()
            {
                self.shortcut_keys = Keymap::default();
                self.shortcut_recording = None;
                self.shortcut_error = None;
            }
            if let Some(action) = self.shortcut_recording {
                ui.label(
                    egui::RichText::new(format!(
                        "{} · {}",
                        action.label(self.lang),
                        if zh {
                            "输入新快捷键；Delete 清空，Esc 取消"
                        } else {
                            "Press new keys; Delete clears, Esc cancels"
                        }
                    ))
                    .color(pal.accent),
                );
            }
        });
        if let Some(error) = &self.shortcut_error {
            ui.label(egui::RichText::new(error).color(pal.danger));
        }
        let groups = if zh {
            ["文件与标签页", "基础编辑", "查找替换"]
        } else {
            ["Files & tabs", "Editing", "Find & replace"]
        };
        let columns = if ui.available_width() >= 940.0 {
            3
        } else if ui.available_width() >= 680.0 {
            2
        } else {
            1
        };
        let scroll = egui::ScrollArea::vertical()
            .id_salt("shortcuts_scroll")
            .auto_shrink([false, true])
            .max_height((ui.ctx().screen_rect().height() - 210.0).max(140.0))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(16.0, 12.0);
                for start in (0..groups.len()).step_by(columns) {
                    ui.columns(columns, |uis| {
                        for (column, ui) in uis.iter_mut().enumerate() {
                            let group = start + column;
                            if group >= groups.len() {
                                continue;
                            }
                            let card = egui::Frame::new()
                                .fill(pal.bg_window)
                                .stroke(egui::Stroke::new(1.0_f32, pal.bg_elevated))
                                .corner_radius(8.0)
                                .inner_margin(egui::Margin::same(12))
                                .show(ui, |ui| {
                                    ui.set_min_width(ui.available_width());
                                    ui.spacing_mut().item_spacing.y = 4.0;
                                    ui.label(
                                        egui::RichText::new(groups[group])
                                            .size(14.0)
                                            .strong()
                                            .color(pal.accent),
                                    );
                                    ui.add_space(6.0);
                                    for &action in ACTIONS.iter().filter(|a| a.group() == group) {
                                        let binding = self.shortcut_keys.binding(action);
                                        if shortcut_row(
                                            ui,
                                            action,
                                            binding,
                                            self.shortcut_recording == Some(action),
                                            self.lang,
                                            pal,
                                        ) {
                                            self.shortcut_recording = Some(action);
                                            self.shortcut_error = None;
                                        }
                                    }
                                });
                            ui.ctx().data_mut(|d| {
                                d.insert_temp(
                                    egui::Id::new(("shortcut_card", group)),
                                    card.response.rect,
                                )
                            });
                        }
                    });
                }
            });
        ui.ctx().data_mut(|d| {
            d.insert_temp(
                egui::Id::new("shortcut_scroll_geometry"),
                (scroll.inner_rect, scroll.content_size),
            );
        });
    }
}

/// Keyboard legends, not internal English enum names. Keep Enter/Delete readable.
fn key_legend(key: Key, mac: bool) -> &'static str {
    match key {
        Key::ArrowLeft => "←",
        Key::ArrowRight => "→",
        Key::ArrowUp => "↑",
        Key::ArrowDown => "↓",
        Key::Tab => "⇥",
        Key::Escape => "⎋",
        Key::Backspace => {
            if mac {
                "Delete"
            } else {
                "⌫"
            }
        }
        Key::Space => "␣",
        Key::Home => "↖",
        Key::End => "↘",
        Key::PageUp => "⇞",
        Key::PageDown => "⇟",
        Key::Quote => "'",
        Key::Minus => "-",
        _ => key.symbol_or_name(),
    }
}

fn shortcut_row(
    ui: &mut egui::Ui,
    action: Action,
    shortcut: Option<KeyboardShortcut>,
    selected: bool,
    lang: Lang,
    pal: &Palette,
) -> bool {
    let mac = ui.ctx().os() == egui::os::OperatingSystem::Mac;
    let mut keys = Vec::new();
    if let Some(shortcut) = shortcut {
        if shortcut.modifiers.command {
            keys.push(if mac { "⌘" } else { "Ctrl" });
        } else if shortcut.modifiers.ctrl {
            keys.push("Ctrl");
        }
        if shortcut.modifiers.alt {
            keys.push(if mac { "⌥" } else { "Alt" });
        }
        if shortcut.modifiers.shift {
            keys.push("⇧");
        }
        keys.push(key_legend(shortcut.logical_key, mac));
    }
    let font = egui::FontId::proportional(13.0);
    let widths: Vec<_> = keys
        .iter()
        .map(|key| {
            (ui.painter()
                .layout_no_wrap((*key).to_owned(), font.clone(), pal.text_main)
                .size()
                .x
                + 12.0)
                .max(24.0)
        })
        .collect();
    let key_width = if keys.is_empty() {
        96.0
    } else {
        widths.iter().sum::<f32>() + keys.len().saturating_sub(1) as f32 * 6.0
    };
    let label_width = (ui.available_width() - key_width - 14.0).max(40.0);
    let label = ui
        .painter()
        .layout(action.label(lang), font.clone(), pal.text_main, label_width);
    let height = label.size().y.max(26.0);
    let (row, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    let label_rect = egui::Rect::from_min_size(
        row.left_center() - egui::vec2(0.0, label.size().y / 2.0),
        label.size(),
    );
    ui.painter().galley(label_rect.min, label, pal.text_main);
    let mut x = row.right() - key_width;
    let button = ui
        .interact(
            egui::Rect::from_min_max(egui::pos2(x, row.top()), row.max),
            ui.id().with(action),
            egui::Sense::click(),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(if lang == Lang::ZhCn {
            "点击修改快捷键"
        } else {
            "Click to change binding"
        });
    button.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            ui.is_enabled(),
            action.label(lang),
        )
    });
    let selected = selected || button.clicked();
    let binding_rect = egui::Rect::from_min_max(egui::pos2(x, row.top()), row.max);
    if selected || button.hovered() || keys.is_empty() {
        ui.painter().rect_filled(
            binding_rect.expand(2.0),
            5.0,
            if selected {
                pal.accent.gamma_multiply(0.18)
            } else {
                pal.bg_inset
            },
        );
        ui.painter().rect_stroke(
            binding_rect.expand(2.0),
            5.0,
            egui::Stroke::new(
                if selected { 1.5_f32 } else { 1.0_f32 },
                if selected {
                    pal.accent
                } else {
                    pal.text_faint.gamma_multiply(0.25)
                },
            ),
            egui::StrokeKind::Inside,
        );
    }
    for (index, (key, width)) in keys.iter().zip(widths).enumerate() {
        let rect = egui::Rect::from_center_size(
            egui::pos2(x + width / 2.0, row.center().y),
            egui::vec2(width, 24.0),
        );
        ui.painter().rect_filled(
            rect,
            4.0,
            if selected {
                pal.accent.gamma_multiply(0.18)
            } else {
                pal.bg_inset
            },
        );
        ui.painter().rect_stroke(
            rect,
            4.0,
            egui::Stroke::new(1.0_f32, pal.text_faint.gamma_multiply(0.25)),
            egui::StrokeKind::Inside,
        );
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            key,
            font.clone(),
            if selected { pal.accent } else { pal.text_main },
        );
        ui.ctx().data_mut(|d| {
            d.insert_temp(
                egui::Id::new(("shortcut_keycap", action as usize, index)),
                rect,
            )
        });
        x += width + 6.0;
    }
    ui.ctx().data_mut(|d| {
        d.insert_temp(
            egui::Id::new(("shortcut_binding_rect", action as usize)),
            binding_rect,
        );
        d.insert_temp(
            egui::Id::new(("shortcut_row", action as usize)),
            (row, label_rect, key_width),
        )
    });
    button.clicked()
}

#[cfg(test)]
#[path = "shortcuts_tests.rs"]
mod tests;

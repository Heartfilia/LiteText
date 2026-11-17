//! Logical-line indentation, separate from pasted whitespace and search inputs.
use crate::{LoadedFile, ViewMode, line_commands, whitespace::TabMode};
use eframe::egui;

pub(crate) fn handle_tab(file: &mut LoadedFile, ctx: &egui::Context, mode: TabMode) {
    if file.is_binary || file.view != ViewMode::Text {
        return;
    }
    let id = egui::Id::new(if file.editor_state.large.active {
        "large_editor"
    } else {
        "draft_editor"
    })
    .with(file.id);
    if !ctx.memory(|memory| memory.has_focus(id)) || file.editor_state.composing {
        return;
    }
    if ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Ime(_)))) {
        return;
    }
    let Some(text) = file.draft_text.as_deref() else {
        return;
    };
    let selected = line_commands::selected_bytes(file, text);
    let multiline = text[selected.clone()].contains('\n');
    let mut commands = Vec::new();
    ctx.input_mut(|input| {
        input.events.retain(|event| {
            if let egui::Event::Key {
                key: egui::Key::Tab,
                pressed: true,
                modifiers,
                ..
            } = event
                && !modifiers.alt
                && !modifiers.ctrl
                && !modifiers.command
                && (modifiers.shift || multiline)
            {
                commands.push(modifiers.shift);
                return false;
            }
            true
        })
    });
    for unindent in commands {
        let text = file.draft_text.as_deref().unwrap();
        let selected = line_commands::selected_bytes(file, text);
        let span = line_commands::line_span(text, selected.clone());
        let unit = if mode == TabMode::Default {
            "\t".to_owned()
        } else {
            " ".repeat(mode.width())
        };
        let mut edits = Vec::new();
        let mut offset = span.start;
        for line in text[span.clone()].split_inclusive('\n') {
            if unindent {
                let removed = if line.starts_with('\t') {
                    1
                } else {
                    line.bytes()
                        .take(mode.width())
                        .take_while(|&b| b == b' ')
                        .count()
                };
                if removed > 0 {
                    edits.push((offset..offset + removed, String::new()));
                }
            } else {
                edits.push((offset..offset, unit.clone()));
            }
            offset += line.len();
        }
        if span.is_empty() && !unindent {
            edits.push((span, unit));
        }
        line_commands::commit_edits(file, ctx, edits, Some(selected));
        ctx.request_repaint();
    }
}

/// Inherit only whitespace before the replacement start, not text after the caret.
pub(crate) fn inherited(text: &str, character: usize) -> &str {
    let byte = crate::text_tools::byte_at(text, character);
    let start = text[..byte].rfind('\n').map_or(0, |at| at + 1);
    let prefix = &text[start..byte];
    &prefix[..prefix.len() - prefix.trim_start_matches([' ', '\t']).len()]
}

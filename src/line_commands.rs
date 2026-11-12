//! Undoable logical-line commands, independent of the viewport and language UI.
use crate::{LoadedFile, text_tools};
use eframe::egui;
use std::ops::Range;

/// JSON and non-code documents use an editor convention where no native
/// comment syntax exists; this does not make annotated JSON valid JSON.
struct CommentStyle {
    prefix: &'static str,
    suffix: &'static str,
}

impl CommentStyle {
    fn for_file(file: &LoadedFile) -> Self {
        let language = crate::workspace_theme::effective_language(file);
        Self::for_language(&language)
    }

    fn for_language(language: &str) -> Self {
        let (prefix, suffix) = match language {
            "Rust"
            | "JavaScript"
            | "TypeScript"
            | "TSX"
            | "JSX"
            | "C"
            | "C++"
            | "C#"
            | "Java"
            | "Go"
            | "Swift"
            | "Kotlin"
            | "ActionScript"
            | "D"
            | "Graphviz (DOT)"
            | "Groovy"
            | "JavaScript (Rails)"
            | "Objective-C"
            | "Objective-C++"
            | "PHP"
            | "PHP Source"
            | "Scala"
            | "JSON"
            | "Regular Expressions (Javascript)" => ("//", ""),
            "SQL" | "SQL (Rails)" | "Lua" | "Haskell" | "Literate Haskell" | "AppleScript" => {
                ("--", "")
            }
            "HTML" | "XML" | "Vue" | "Svelte" | "Markdown" | "MultiMarkdown" | "Textile"
            | "NAnt Build File" => ("<!--", "-->"),
            "CSS" | "JavaDoc" => ("/*", "*/"),
            "OCaml" | "OCamllex" | "OCamlyacc" | "camlp4" => ("(*", "*)"),
            "Pascal" => ("{", "}"),
            "Clojure" | "Lisp" => (";", ""),
            "Erlang"
            | "MATLAB"
            | "TeX"
            | "LaTeX"
            | "BibTeX"
            | "Rd (R Documentation)"
            | "LaTeX Log" => ("%", ""),
            "Batch File" => ("REM", ""),
            "ASP" => ("'", ""),
            "HTML (ASP)" => ("<!--", "-->"),
            "Java Server Page (JSP)" => ("<%--", "--%>"),
            "HTML (Rails)" => ("<%#", "%>"),
            "HTML (Erlang)" | "HTML (Tcl)" => ("<!--", "-->"),
            "Ruby Haml" => ("-#", ""),
            "reStructuredText" => ("..", ""),
            "Regular Expression" => ("(?#", ")"),
            // Explicitly cover the remaining built-in languages and output
            // formats. Unknown custom languages retain plain-text convention.
            "text"
            | "Python"
            | "Bourne Again Shell (bash)"
            | "Shell-Unix-Generic"
            | "commands-builtin-shell-bash"
            | "Cargo Build Results"
            | "Diff"
            | "Java Properties"
            | "Make Output"
            | "Makefile"
            | "Perl"
            | "R"
            | "R Console"
            | "Regular Expressions (Python)"
            | "Ruby"
            | "Ruby on Rails"
            | "Tcl"
            | "YAML" => ("#", ""),
            _ => ("#", ""),
        };
        Self { prefix, suffix }
    }
}

fn line_span(text: &str, selection: Range<usize>) -> Range<usize> {
    let start = text[..selection.start].rfind('\n').map_or(0, |i| i + 1);
    let last = if selection.end > selection.start
        && text.as_bytes().get(selection.end - 1) == Some(&b'\n')
    {
        selection.end - 1
    } else {
        selection.end
    };
    let end = text[last..].find('\n').map_or(text.len(), |i| last + i + 1);
    start..end
}

pub(crate) fn selected_bytes(file: &LoadedFile, text: &str) -> Range<usize> {
    if let Some((first, last)) = file.editor_state.selection {
        let start = if first == 0 {
            0
        } else {
            text.match_indices('\n')
                .nth(first - 1)
                .map_or(text.len(), |(i, _)| i + 1)
        };
        let end = text
            .match_indices('\n')
            .nth(last)
            .map_or(text.len(), |(i, _)| i + 1);
        start..end
    } else {
        let cursor = file.editor_state.cursor.unwrap_or_default();
        text_tools::byte_at(text, cursor.primary.index.min(cursor.secondary.index))
            ..text_tools::byte_at(text, cursor.primary.index.max(cursor.secondary.index))
    }
}

fn comment_edits(
    text: &str,
    span: Range<usize>,
    style: CommentStyle,
) -> Vec<(Range<usize>, String)> {
    let CommentStyle { prefix, suffix } = style;
    let opening = format!("{prefix} ");
    let closing = if suffix.is_empty() {
        String::new()
    } else {
        format!(" {suffix}")
    };
    if span.is_empty() {
        return vec![(span, format!("{opening}{closing}"))];
    }
    let span_start = span.start;
    let only_line = !text[span.clone()]
        .trim_end_matches(['\r', '\n'])
        .contains('\n');
    let mut lines = Vec::new();
    let mut offset = span.start;
    for line in text[span].split_inclusive('\n') {
        let content = line.trim_end_matches(['\r', '\n']);
        let indent = content.len() - content.trim_start_matches([' ', '\t']).len();
        if !content[indent..].is_empty() {
            lines.push((offset + indent, &content[indent..]));
        }
        offset += line.len();
    }
    if lines.is_empty() && only_line {
        let line = text[span_start..]
            .split('\n')
            .next()
            .unwrap_or("")
            .trim_end_matches('\r');
        let start = span_start + line.len();
        return vec![(start..start, format!("{opening}{closing}"))];
    }
    // Also unwrap an existing block spanning several lines, not only the
    // per-line wrappers generated by this command.
    if !suffix.is_empty() && lines.len() > 1 {
        let (first_at, first) = lines[0];
        let (last_at, last) = lines[lines.len() - 1];
        if first.starts_with(prefix)
            && !first.trim_end().ends_with(suffix)
            && last.trim_end_matches([' ', '\t']).ends_with(suffix)
        {
            let end = last_at + last.trim_end_matches([' ', '\t']).len();
            let open_end =
                first_at + prefix.len() + usize::from(first[prefix.len()..].starts_with(' '));
            let mut close_start = end - suffix.len();
            if text.as_bytes().get(close_start.wrapping_sub(1)) == Some(&b' ') {
                close_start -= 1;
            }
            return vec![
                (first_at..open_end, String::new()),
                (close_start..end, String::new()),
            ];
        }
    }
    let enclosed = |content: &str| {
        let starts = if prefix == "REM" {
            content
                .get(..3)
                .is_some_and(|s| s.eq_ignore_ascii_case("REM"))
                && content[3..].starts_with([' ', '\t'])
        } else {
            content.starts_with(prefix)
        };
        starts
            && (suffix.is_empty()
                || content
                    .trim_end_matches([' ', '\t'])
                    .strip_prefix(prefix)
                    .is_some_and(|s| s.ends_with(suffix)))
    };
    let uncomment = !lines.is_empty() && lines.iter().all(|(_, text)| enclosed(text));
    let mut edits = Vec::new();
    for (start, content) in lines {
        if uncomment {
            let mut opening_end = start + prefix.len();
            if content[prefix.len()..].starts_with(' ') {
                opening_end += 1;
            }
            if !suffix.is_empty() {
                let end = start + content.trim_end_matches([' ', '\t']).len();
                let mut closing_start = end - suffix.len();
                if closing_start > opening_end && text.as_bytes()[closing_start - 1] == b' ' {
                    closing_start -= 1;
                }
                // Empty wrappers must not produce overlapping deletion ranges.
                opening_end = opening_end.min(closing_start);
                edits.push((start..opening_end, String::new()));
                edits.push((closing_start..end, String::new()));
            } else {
                edits.push((start..opening_end, String::new()));
            }
        } else {
            edits.push((start..start, opening.clone()));
            if !closing.is_empty() {
                edits.push((
                    start + content.len()..start + content.len(),
                    closing.clone(),
                ));
            }
        }
    }
    edits
}

pub(crate) fn apply(file: &mut LoadedFile, ctx: &egui::Context, comment: bool) {
    let text = file.draft_text.as_deref().expect("editable text");
    let selected = selected_bytes(file, text);
    let span = line_span(text, selected.clone());
    let edits = if comment {
        comment_edits(text, span.clone(), CommentStyle::for_file(file))
    } else {
        let mut deletion = span;
        // Last unterminated line also removes its preceding terminator.
        if deletion.end == text.len()
            && deletion.start > 0
            && !text[deletion.clone()].ends_with('\n')
        {
            deletion.start -= 1;
            if deletion.start > 0 && text.as_bytes()[deletion.start - 1] == b'\r' {
                deletion.start -= 1;
            }
        }
        vec![(deletion, String::new())]
    };
    commit_edits(file, ctx, edits, comment.then_some(selected));
}

/// One undo entry while retaining marker ranges between individual edits.
pub(crate) fn commit_edits(
    file: &mut LoadedFile,
    ctx: &egui::Context,
    edits: Vec<(Range<usize>, String)>,
    selected: Option<Range<usize>>,
) {
    let text = file.draft_text.as_deref().expect("editable text");
    if edits.is_empty() {
        return;
    }
    if edits
        .iter()
        .all(|(range, inserted)| range.is_empty() && inserted.is_empty())
    {
        return;
    }
    let range = edits.first().unwrap().0.start..edits.last().unwrap().0.end;
    let mut inserted = String::new();
    let mut at = range.start;
    for (edit, value) in &edits {
        inserted.push_str(&text[at..edit.start]);
        inserted.push_str(value);
        at = edit.end;
    }
    inserted.push_str(&text[at..range.end]);
    let map = |position: usize| {
        let mut delta = 0isize;
        for (edit, value) in &edits {
            if position < edit.start {
                break;
            }
            if position <= edit.end {
                return edit.start.saturating_add_signed(delta) + value.len();
            }
            delta += value.len() as isize - edit.len() as isize;
        }
        position.saturating_add_signed(delta)
    };
    let selection = if let Some(selected) = selected {
        map(selected.start)..map(selected.end)
    } else {
        range.start..range.start
    };
    let count_before = text[..range.start].chars().count();
    let as_char = |position: usize| {
        if position < range.start {
            text[..position].chars().count()
        } else if position <= range.start + inserted.len() {
            count_before + inserted[..position - range.start].chars().count()
        } else {
            count_before
                + inserted.chars().count()
                + text[range.end..range.end + position - range.start - inserted.len()]
                    .chars()
                    .count()
        }
    };
    let start = as_char(selection.start);
    let end = as_char(selection.end);
    let cursor = egui::text::CCursorRange::two(
        egui::text::CCursor::new(start),
        egui::text::CCursor::new(end),
    );
    // Preserve marks between individual line prefixes, not just outside the batch.
    for (edit, value) in edits.iter().rev() {
        file.editor_state
            .tools
            .apply_edit(edit.clone(), value.len());
    }
    let marks = std::mem::take(&mut file.editor_state.tools.marks);
    text_tools::replace_range_with_cursor(file, ctx, range, inserted, Some(cursor));
    file.editor_state.tools.marks = marks;
    file.editor_state.pending_selection = Some(start..end);
    file.editor_state.cursor = Some(cursor);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_with(app: &mut crate::LiteTextApp, text: &str, start: usize, end: usize) {
        app.new_document();
        let file = app.active_mut().unwrap();
        file.draft_text = Some(text.to_owned());
        file.sync_draft();
        file.editor_state.cursor = Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(start),
            egui::text::CCursor::new(end),
        ));
    }

    #[test]
    fn every_selectable_language_comments_and_uncomments_without_losing_bytes() {
        let ctx = egui::Context::default();
        for language in crate::workspace_theme::language_options() {
            for original in ["  中文🙂 value\r\n\tsecond\r\n\r\ntail", "", "  "] {
                let mut app = crate::LiteTextApp::default();
                file_with(&mut app, original, 0, original.chars().count());
                app.active_mut().unwrap().language_override = Some(language.clone());
                apply(app.active_mut().unwrap(), &ctx, true);
                let commented = app.active().unwrap().draft_text.as_ref().unwrap().clone();
                assert_ne!(commented, original, "{language} {original:?}");
                apply(app.active_mut().unwrap(), &ctx, true);
                assert_eq!(
                    app.active().unwrap().draft_text.as_deref(),
                    Some(original),
                    "{language} {commented:?}"
                );
            }
        }
        for (language, prefix, suffix) in [
            ("HTML", "<!--", "-->"),
            ("CSS", "/*", "*/"),
            ("OCaml", "(*", "*)"),
            ("Clojure", ";", ""),
            ("Erlang", "%", ""),
            ("Batch File", "REM", ""),
            ("Python", "#", ""),
            ("JSON", "//", ""),
        ] {
            let style = CommentStyle::for_language(language);
            assert_eq!((style.prefix, style.suffix), (prefix, suffix), "{language}");
        }
        let mut app = crate::LiteTextApp::default();
        let block = "  /* first\r\nsecond */\r\n";
        file_with(&mut app, block, 0, block.chars().count());
        app.active_mut().unwrap().language_override = Some("CSS".to_owned());
        apply(app.active_mut().unwrap(), &ctx, true);
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some("  first\r\nsecond\r\n")
        );
    }

    #[test]
    fn deletes_complete_logical_lines_with_crlf_unicode_and_eof() {
        for (text, start, end, expected) in [
            ("中a\r\nb\r\nc", 1, 1, "b\r\nc"),
            ("a\nb\nc", 2, 2, "a\nc"),
            ("a\r\nb", 3, 3, "a"),
            ("a\n", 2, 2, "a"),
            ("a\nb\nc", 0, 2, "b\nc"),
            ("abc", 0, 3, ""),
            ("", 0, 0, ""),
        ] {
            let mut app = crate::LiteTextApp::default();
            file_with(&mut app, text, start, end);
            apply(app.active_mut().unwrap(), &egui::Context::default(), false);
            assert_eq!(
                app.active().unwrap().draft_text.as_deref(),
                Some(expected),
                "{text:?}"
            );
        }
    }

    #[test]
    fn comments_preserve_indentation_newlines_and_toggle_selected_lines() {
        let ctx = egui::Context::default();
        let mut app = crate::LiteTextApp::default();
        let original = "  中文()\r\n\tprint(1)\r\n\r\ntail";
        file_with(
            &mut app,
            original,
            0,
            "  中文()\r\n\tprint(1)\r\n".chars().count(),
        );
        apply(app.active_mut().unwrap(), &ctx, true);
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some("  # 中文()\r\n\t# print(1)\r\n\r\ntail")
        );
        apply(app.active_mut().unwrap(), &ctx, true);
        assert_eq!(app.active().unwrap().draft_text.as_deref(), Some(original));
        file_with(&mut app, "  #x\n  # y", 0, 11);
        apply(app.active_mut().unwrap(), &ctx, true);
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some("  x\n  y")
        );
        file_with(&mut app, "", 0, 0);
        apply(app.active_mut().unwrap(), &ctx, true);
        assert_eq!(app.active().unwrap().draft_text.as_deref(), Some("# "));
    }

    #[test]
    fn comment_style_is_separate_from_keybindings_and_marks_between_prefixes_survive() {
        let mut app = crate::LiteTextApp::default();
        file_with(&mut app, "one\ntwo\n", 0, 8);
        let file = app.active_mut().unwrap();
        file.path = "sample.rs".into();
        file.editor_state.tools.marks.push(crate::text_tools::Mark {
            range: 4..7,
            color: 2,
        });
        apply(file, &egui::Context::default(), true);
        let file = app.active().unwrap();
        assert_eq!(file.draft_text.as_deref(), Some("// one\n// two\n"));
        assert_eq!(file.editor_state.tools.marks[0].range, 10..13);
        for (language, prefix) in [
            ("Python", "#"),
            ("SQL", "--"),
            ("TypeScript", "//"),
            ("text", "#"),
        ] {
            app.active_mut().unwrap().language_override = Some(language.to_owned());
            assert_eq!(CommentStyle::for_file(app.active().unwrap()).prefix, prefix);
        }
    }
}

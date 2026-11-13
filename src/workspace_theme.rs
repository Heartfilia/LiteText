//! Global app/editor palettes and TextMate-style language highlighting.
//! Dark+/Light+ inspired, not VS Code semantic-token/LSP integration.
use crate::{Lang, LiteTextApp, Palette};
use eframe::egui::{self, Color32};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    ops::Range,
    path::Path,
    sync::{Arc, Mutex, OnceLock},
};
use syntect::{
    easy::ScopeRangeIterator,
    parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Preset {
    #[default]
    Auto,
    DarkPlus,
    LightPlus,
    Darcula,
    IntelliJLight,
    Monokai,
    Mariana,
    OneDark,
    Nord,
}

impl Preset {
    const ALL: [Self; 9] = [
        Self::Auto,
        Self::DarkPlus,
        Self::LightPlus,
        Self::Darcula,
        Self::IntelliJLight,
        Self::Monokai,
        Self::Mariana,
        Self::OneDark,
        Self::Nord,
    ];
    fn label(self, zh: bool) -> &'static str {
        match self {
            Self::Auto => {
                if zh {
                    "跟随系统"
                } else {
                    "Follow system"
                }
            }
            Self::DarkPlus => "Dark+",
            Self::LightPlus => "Light+",
            Self::Darcula => "Darcula · JetBrains",
            Self::IntelliJLight => "IntelliJ Light · JetBrains",
            Self::Monokai => "Monokai · Sublime",
            Self::Mariana => "Mariana · Sublime",
            Self::OneDark => "One Dark",
            Self::Nord => "Nord",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Colors {
    pub text: Color32,
    pub background: Color32,
    pub bracket: Color32,
    pub selection: Color32,
    pub selected_text: Color32,
    pub search: Color32,
    pub cursor: Color32,
    pub gutter: Color32,
    pub keyword: Color32,
    pub string: Color32,
    pub comment: Color32,
    pub number: Color32,
    pub function: Color32,
    pub type_name: Color32,
    pub property: Color32,
}

impl Colors {
    fn preset(dark: bool) -> Self {
        let rgb = |hex: u32| Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8);
        if dark {
            Self {
                text: rgb(0xd4d4d4),
                background: rgb(0x1e1e1e),
                bracket: rgb(0xffd700),
                selection: rgb(0x264f78),
                selected_text: rgb(0xffffff),
                search: rgb(0x613315),
                cursor: rgb(0xaeafad),
                gutter: rgb(0x858585),
                keyword: rgb(0x569cd6),
                string: rgb(0xce9178),
                comment: rgb(0x6a9955),
                number: rgb(0xb5cea8),
                function: rgb(0xdcdcaa),
                type_name: rgb(0x4ec9b0),
                property: rgb(0x9cdcfe),
            }
        } else {
            Self {
                text: rgb(0x000000),
                background: rgb(0xffffff),
                bracket: rgb(0x0431fa),
                selection: rgb(0xadd6ff),
                selected_text: rgb(0x000000),
                search: rgb(0xffe6a0),
                cursor: rgb(0x000000),
                gutter: rgb(0x6e7681),
                keyword: rgb(0x0000ff),
                string: rgb(0xa31515),
                comment: rgb(0x008000),
                number: rgb(0x098658),
                function: rgb(0x795e26),
                type_name: rgb(0x267f99),
                property: rgb(0x001080),
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct WorkspaceTheme {
    pub preset: Preset,
    pub syntax: bool,
    pub custom: Option<Colors>,
}

impl Default for WorkspaceTheme {
    fn default() -> Self {
        Self {
            preset: Preset::Auto,
            syntax: true,
            custom: None,
        }
    }
}

impl WorkspaceTheme {
    pub(crate) fn colors(&self, dark: bool) -> Colors {
        self.custom.unwrap_or_else(|| self.preset.colors(dark))
    }

    pub(crate) fn palette(&self, system: Option<egui::Theme>) -> Palette {
        let colors = self.colors(system.unwrap_or(egui::Theme::Dark) == egui::Theme::Dark);
        let mix = |a: Color32, b: Color32, t: f32| {
            let blend = |x: u8, y: u8| (x as f32 * (1.0 - t) + y as f32 * t).round() as u8;
            Color32::from_rgb(
                blend(a.r(), b.r()),
                blend(a.g(), b.g()),
                blend(a.b(), b.b()),
            )
        };
        let bg = Color32::from_rgb(
            colors.background.r(),
            colors.background.g(),
            colors.background.b(),
        );
        let dark =
            (bg.r() as f32 * 0.2126 + bg.g() as f32 * 0.7152 + bg.b() as f32 * 0.0722) < 140.0;
        let base = if dark { crate::DARK } else { crate::LIGHT };
        let distance = |a: Color32, b: Color32| {
            a.r().abs_diff(b.r()) as u32
                + a.g().abs_diff(b.g()) as u32
                + a.b().abs_diff(b.b()) as u32
        };
        // Keep app labels readable if custom editor text is near its background.
        let text = if distance(colors.text, bg) < 150 {
            base.text_main
        } else {
            colors.text
        };
        let accent = if distance(colors.keyword, bg) < 150 {
            base.accent
        } else {
            colors.keyword
        };
        Palette {
            dark,
            bg_window: bg,
            bg_panel: mix(bg, text, 0.035),
            bg_elevated: mix(bg, text, 0.10),
            bg_inset: mix(bg, if dark { Color32::BLACK } else { Color32::WHITE }, 0.15),
            text_main: text,
            text_faint: mix(bg, text, 0.65),
            accent,
            accent_hover: mix(accent, text, 0.20),
            ..base
        }
    }
}

impl Preset {
    /// Inspired editor palettes, not imported vendor themes or semantic tokens.
    fn colors(self, dark: bool) -> Colors {
        let values = match self {
            Self::Auto => return Colors::preset(dark),
            Self::DarkPlus => return Colors::preset(true),
            Self::LightPlus => return Colors::preset(false),
            Self::Darcula => [
                0xa9b7c6, 0x2b2b2b, 0xe8bf6a, 0x214283, 0xffffff, 0x51462e, 0xbbbbbb, 0x888888,
                0xcc7832, 0x6aab73, 0x8b919c, 0x6897bb, 0xffc66d, 0xa9b7c6, 0xc6a1c8,
            ],
            Self::IntelliJLight => [
                0x202020, 0xffffff, 0x7040a0, 0xb3d4fc, 0x202020, 0xffe4a3, 0x202020, 0x727272,
                0x0033b3, 0x067d17, 0x747474, 0x1750eb, 0x7a3e00, 0x00627a, 0x871094,
            ],
            Self::Monokai => [
                0xf8f8f2, 0x272822, 0xf8f8f2, 0x49483e, 0xffffff, 0x65542b, 0xf8f8f0, 0x90908a,
                0xf92672, 0xe6db74, 0x90908a, 0xae81ff, 0xa6e22e, 0x66d9ef, 0xfd971f,
            ],
            Self::Mariana => [
                0xd8dee9, 0x303841, 0xfac761, 0x46586b, 0xffffff, 0x665638, 0xffcc66, 0x8996a8,
                0xc695c6, 0x9acd7a, 0x91a0b5, 0xf9ae58, 0x79b8ef, 0xfac761, 0x5ec4c4,
            ],
            Self::OneDark => [
                0xabb2bf, 0x282c34, 0xe5c07b, 0x3e4b63, 0xffffff, 0x635333, 0x61afef, 0x8892a3,
                0xc678dd, 0x98c379, 0x8892a3, 0xd19a66, 0x61afef, 0xe5c07b, 0xe06c75,
            ],
            Self::Nord => [
                0xd8dee9, 0x2e3440, 0xebcb8b, 0x434f65, 0xeceff4, 0x61583e, 0xd8dee9, 0x8c9bb0,
                0x81a1c1, 0xa3be8c, 0x8c9bb0, 0xb48ead, 0x88c0d0, 0x8fbcbb, 0xebcb8b,
            ],
        };
        let c = values.map(|hex| Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8));
        Colors {
            text: c[0],
            background: c[1],
            bracket: c[2],
            selection: c[3],
            selected_text: c[4],
            search: c[5],
            cursor: c[6],
            gutter: c[7],
            keyword: c[8],
            string: c[9],
            comment: c[10],
            number: c[11],
            function: c[12],
            type_name: c[13],
            property: c[14],
        }
    }
}

fn syntaxes() -> &'static SyntaxSet {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAXES.get_or_init(SyntaxSet::load_defaults_newlines)
}

pub(crate) fn language(path: &Path, json: bool) -> String {
    if path
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("log"))
    {
        return "Log".into();
    }
    if json {
        return "JSON".to_owned();
    }
    let set = syntaxes();
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let syntax = match extension.as_str() {
        "ts" | "tsx" | "jsx" => set.find_syntax_by_extension("js"),
        "vue" | "svelte" => set.find_syntax_by_extension("html"),
        "jsonl" | "jsonc" => set.find_syntax_by_extension("json"),
        _ => set.find_syntax_by_extension(&extension),
    }
    .or_else(|| set.find_syntax_by_extension(name));
    syntax
        .unwrap_or_else(|| set.find_syntax_plain_text())
        .name
        .clone()
}

#[derive(Clone)]
struct Checkpoint {
    parser: ParseState,
    scopes: ScopeStack,
}

#[derive(Default)]
pub(crate) struct SyntaxCache {
    key: Option<(String, u64, u64)>,
    checkpoints: BTreeMap<usize, Checkpoint>,
    last: Option<(usize, String, Arc<Vec<Token>>)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Text,
    Keyword,
    String,
    Comment,
    Number,
    Function,
    Type,
    Property,
    Bracket,
    LogError,
    LogWarning,
    LogInfo,
}

impl Kind {
    fn color(self, c: Colors) -> Color32 {
        match self {
            Self::Text => c.text,
            Self::Keyword => c.keyword,
            Self::String => c.string,
            Self::Comment => c.comment,
            Self::Number => c.number,
            Self::Function => c.function,
            Self::Type => c.type_name,
            Self::Property => c.property,
            Self::Bracket => c.bracket,
            Self::LogError => {
                if c.background.r() as u16 + c.background.g() as u16 + (c.background.b() as u16)
                    < 420
                {
                    Color32::from_rgb(255, 112, 112)
                } else {
                    Color32::from_rgb(184, 28, 38)
                }
            }
            Self::LogWarning => {
                if c.background.r() as u16 + c.background.g() as u16 + (c.background.b() as u16)
                    < 420
                {
                    Color32::from_rgb(230, 190, 95)
                } else {
                    Color32::from_rgb(143, 91, 0)
                }
            }
            Self::LogInfo => c.type_name,
        }
    }
}

#[derive(Clone, Debug)]
struct Token {
    range: Range<usize>,
    kind: Kind,
}

fn push_token(tokens: &mut Vec<Token>, range: Range<usize>, kind: Kind) {
    if range.is_empty() {
        return;
    }
    if let Some(last) = tokens.last_mut()
        && last.kind == kind
        && last.range.end == range.start
    {
        last.range.end = range.end;
    } else {
        tokens.push(Token { range, kind });
    }
}

/// JSON strings cannot span physical lines, so a line-aligned viewport needs
/// no prefix parser state (including JSONL with one document per line). This
/// lexer only colors text: it never validates, normalizes or rewrites data.
fn json_tokens(text: &str) -> Vec<Token> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let start = at;
        let kind = match bytes[at] {
            b'"' => {
                at += 1;
                while at < bytes.len() && !matches!(bytes[at], b'"' | b'\n' | b'\r') {
                    if bytes[at] == b'\\' {
                        at += 1;
                        if at == bytes.len() || matches!(bytes[at], b'\n' | b'\r') {
                            break;
                        }
                    }
                    at += 1;
                }
                let closed = bytes.get(at) == Some(&b'"');
                if closed {
                    at += 1;
                }
                let mut next = at;
                while bytes.get(next).is_some_and(u8::is_ascii_whitespace) {
                    next += 1;
                }
                if closed && bytes.get(next) == Some(&b':') {
                    Kind::Property
                } else {
                    Kind::String
                }
            }
            b'{' | b'}' | b'[' | b']' => {
                at += 1;
                Kind::Bracket
            }
            b'-' | b'0'..=b'9' => {
                at += 1;
                while bytes.get(at).is_some_and(|b| {
                    b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-')
                }) {
                    at += 1;
                }
                Kind::Number
            }
            b't' | b'f' | b'n' => {
                while bytes.get(at).is_some_and(u8::is_ascii_alphabetic) {
                    at += 1;
                }
                if matches!(&text[start..at], "true" | "false" | "null") {
                    Kind::Keyword
                } else {
                    Kind::Text
                }
            }
            _ => {
                // Move over whole Unicode scalar values on the fallback path.
                at += text[at..].chars().next().unwrap().len_utf8();
                Kind::Text
            }
        };
        push_token(&mut tokens, start..at, kind);
    }
    tokens
}

/// Stateless viewport-only log lexer: no scan of a multi-gigabyte prefix.
fn log_tokens(text: &str) -> Vec<Token> {
    static PATTERN: OnceLock<regex::Regex> = OnceLock::new();
    let pattern = PATTERN.get_or_init(|| regex::Regex::new(r#"(?i)\b(?:fatal|critical|error|err|warn|warning|info|notice|debug|trace)\b|\b\d{4}[-/]\d{2}[-/]\d{2}(?:[T ]\d{2}:\d{2}:\d{2}(?:[.,]\d+)?(?:Z|[+-]\d{2}:?\d{2})?)?|\b\d{2}:\d{2}:\d{2}(?:[.,]\d+)?|"(?:[^"\\\r\n]|\\.)*"|'[^'\r\n]*'|\b\d+(?:\.\d+)?\b"#).expect("log token regex"));
    let mut tokens = Vec::new();
    let mut at = 0;
    for found in pattern.find_iter(text) {
        push_token(&mut tokens, at..found.start(), Kind::Text);
        let word = found.as_str().to_ascii_uppercase();
        let kind = match word.as_str() {
            "FATAL" | "CRITICAL" | "ERROR" | "ERR" => Kind::LogError,
            "WARN" | "WARNING" => Kind::LogWarning,
            "INFO" | "NOTICE" => Kind::LogInfo,
            "DEBUG" | "TRACE" => Kind::Comment,
            _ if word.starts_with(['\'', '"']) => Kind::String,
            _ => Kind::Number,
        };
        push_token(&mut tokens, found.range(), kind);
        at = found.end();
    }
    push_token(&mut tokens, at..text.len(), Kind::Text);
    tokens
}

fn classify(scopes: &ScopeStack) -> Kind {
    let names: Vec<_> = scopes.as_slice().iter().map(|s| s.build_string()).collect();
    // Outer string/comment context must protect punctuation inside it.
    if names.iter().any(|s| s.starts_with("comment")) {
        return Kind::Comment;
    }
    if names.iter().any(|s| {
        s.contains("key.json") || s.contains("attribute-name") || s.contains("mapping.key")
    }) {
        return Kind::Property;
    }
    if names.iter().any(|s| s.starts_with("string")) {
        return Kind::String;
    }
    for name in names.iter().rev() {
        if name.starts_with("constant.numeric") {
            return Kind::Number;
        }
        if name.starts_with("keyword")
            || name.starts_with("storage")
            || name.starts_with("constant.language")
        {
            return Kind::Keyword;
        }
        if name.contains("entity.name.function") || name.contains("support.function") {
            return Kind::Function;
        }
        if name.contains("entity.name.type")
            || name.contains("entity.name.class")
            || name.starts_with("support.type")
            || name.starts_with("support.class")
        {
            return Kind::Type;
        }
        if name.starts_with("variable") || name.starts_with("entity.name.tag") {
            return Kind::Property;
        }
    }
    Kind::Text
}

fn parse_line(
    state: &mut Checkpoint,
    line: &str,
    syntax: &SyntaxSet,
    output: &mut Vec<Token>,
    offset: usize,
) {
    let Ok(ops) = state.parser.parse_line(line, syntax) else {
        output.push(Token {
            range: offset..offset + line.len(),
            kind: Kind::Text,
        });
        return;
    };
    for (range, op) in ScopeRangeIterator::new(&ops, line) {
        if state.scopes.apply(op).is_err() {
            continue;
        }
        if range.is_empty() {
            continue;
        }
        let kind = classify(&state.scopes);
        let mut start = range.start;
        if !matches!(kind, Kind::String | Kind::Comment | Kind::Property) {
            for (at, ch) in line[range.clone()].char_indices() {
                if matches!(ch, '(' | ')' | '[' | ']' | '{' | '}') {
                    let at = range.start + at;
                    if at > start {
                        output.push(Token {
                            range: offset + start..offset + at,
                            kind,
                        });
                    }
                    output.push(Token {
                        range: offset + at..offset + at + 1,
                        kind: Kind::Bracket,
                    });
                    start = at + 1;
                }
            }
        }
        if start < range.end {
            output.push(Token {
                range: offset + start..offset + range.end,
                kind,
            });
        }
    }
}

#[derive(Clone)]
pub(crate) struct Render {
    pub colors: Colors,
    pub language: String,
    pub syntax: bool,
    fast_json: bool,
    cache: Arc<Mutex<SyntaxCache>>,
}

impl Render {
    pub(crate) fn prepare(
        theme: &WorkspaceTheme,
        dark: bool,
        language: String,
        revision: (u64, u64),
        cache: Arc<Mutex<SyntaxCache>>,
    ) -> Self {
        let key = (language.clone(), revision.0, revision.1);
        {
            let mut c = cache.lock().unwrap();
            if c.key.as_ref() != Some(&key) {
                *c = SyntaxCache {
                    key: Some(key),
                    ..Default::default()
                };
            }
        }
        Self {
            colors: theme.colors(dark),
            fast_json: language == "JSON",
            language,
            syntax: theme.syntax,
            cache,
        }
    }

    /// Parse the viewport with the real prefix state, so multiline strings and
    /// comments retain their color when scrolling. Cache parser checkpoints.
    pub(crate) fn prepare_prefix(&self, full: &str, offset: usize) -> bool {
        if !self.syntax
            || self.fast_json
            || matches!(self.language.as_str(), "Plain Text" | "text" | "Log")
        {
            return true;
        }
        let mut cache = self.cache.lock().unwrap();
        if cache.checkpoints.contains_key(&offset) {
            return true;
        }
        let syntax = syntax_for(&self.language);
        let (mut position, mut state) = cache
            .checkpoints
            .range(..=offset)
            .next_back()
            .map(|(&at, state)| (at, state.clone()))
            .unwrap_or((
                0,
                Checkpoint {
                    parser: ParseState::new(syntax),
                    scopes: ScopeStack::new(),
                },
            ));
        let started = std::time::Instant::now();
        for (line_number, line) in full[position..offset].split_inclusive('\n').enumerate() {
            // No token output is needed for offscreen prefix lines.
            if line.len() <= 16 * 1024 && let Ok(ops) = state.parser.parse_line(line, syntaxes()) {
                for (_, op) in ops {
                    let _ = state.scopes.apply(&op);
                }
            }
            position += line.len();
            if line_number % 128 == 127 {
                cache.checkpoints.insert(position, state.clone());
            }
            if (started.elapsed() >= std::time::Duration::from_millis(6) || line_number >= 511) && position < offset {
                cache.checkpoints.insert(position, state);
                return false;
            }
        }
        cache.checkpoints.insert(offset, state);
        true
    }

    fn tokens(&self, text: &str, offset: usize) -> Arc<Vec<Token>> {
        let mut cache = self.cache.lock().unwrap();
        if let Some((at, last, tokens)) = &cache.last
            && *at == offset
            && last == text
        {
            return tokens.clone();
        }
        if self.fast_json || self.language == "Log" {
            let tokens = Arc::new(if self.language == "Log" {
                log_tokens(text)
            } else {
                json_tokens(text)
            });
            cache.last = Some((offset, text.to_owned(), tokens.clone()));
            return tokens;
        }
        let mut state = cache
            .checkpoints
            .get(&offset)
            .cloned()
            .unwrap_or_else(|| Checkpoint {
                parser: ParseState::new(syntax_for(&self.language)),
                scopes: ScopeStack::new(),
            });
        let mut tokens = Vec::new();
        let mut position = 0;
        for line in text.split_inclusive('\n') {
            if line.len() > 16 * 1024 {
                push_token(&mut tokens, position..position + line.len(), Kind::Text);
            } else { parse_line(&mut state, line, syntaxes(), &mut tokens, position); }
            position += line.len();
        }
        let tokens = Arc::new(tokens);
        cache.last = Some((offset, text.to_owned(), tokens.clone()));
        tokens
    }

    pub(crate) fn layout(
        &self,
        text: &str,
        font: egui::FontId,
        width: f32,
        offset: usize,
    ) -> egui::text::LayoutJob {
        let mut job =
            egui::text::LayoutJob::simple(text.to_owned(), font.clone(), self.colors.text, width);
        if !self.syntax || matches!(self.language.as_str(), "Plain Text" | "text") {
            return job;
        }
        let tokens = self.tokens(text, offset);
        if !tokens.is_empty() {
            job.sections = tokens
                .iter()
                .map(|token| egui::text::LayoutSection {
                    leading_space: 0.0,
                    byte_range: token.range.clone(),
                    format: egui::TextFormat {
                        font_id: font.clone(),
                        color: token.kind.color(self.colors),
                        ..Default::default()
                    },
                })
                .collect();
        }
        job
    }

    pub(crate) fn apply_style(&self, ui: &mut egui::Ui) {
        let v = ui.visuals_mut();
        v.selection.bg_fill = self.colors.selection;
        v.selection.stroke.color = self.colors.selected_text;
        v.text_cursor.stroke.color = self.colors.cursor;
    }

    /// egui 0.31 paints a selection background but does not recolor selected
    /// glyphs. Update only this TextEdit's painted glyph quads, not its cache.
    pub(crate) fn paint_selected_text(
        &self,
        ui: &egui::Ui,
        output: &egui::text_edit::TextEditOutput,
    ) {
        if !output.response.has_focus() {
            return;
        }
        let Some(cursor) = output.state.cursor.char_range() else {
            return;
        };
        let selection = cursor.primary.index.min(cursor.secondary.index)
            ..cursor.primary.index.max(cursor.secondary.index);
        if selection.is_empty() {
            return;
        }
        ui.ctx().graphics_mut(|graphics| {
            let list = graphics.entry(ui.layer_id());
            let indices: Vec<_> = list
                .all_entries()
                .enumerate()
                .filter_map(|(index, shape)| match &shape.shape {
                    egui::Shape::Text(text)
                        if text.pos == output.galley_pos
                            && text.galley.text() == output.galley.text() =>
                    {
                        Some(index)
                    }
                    _ => None,
                })
                .collect();
            for index in indices {
                list.mutate_shape(egui::layers::ShapeIdx(index), |shape| {
                    let egui::Shape::Text(text) = &mut shape.shape else {
                        return;
                    };
                    let galley = Arc::make_mut(&mut text.galley);
                    let mut offset = 0;
                    for row in &mut galley.rows {
                        let start = selection.start.saturating_sub(offset).min(row.glyphs.len());
                        let end = selection.end.saturating_sub(offset).min(row.glyphs.len());
                        if start < end {
                            let left = row.x_offset(start);
                            let right = row.x_offset(end);
                            let range = row.visuals.glyph_vertex_range.clone();
                            for quad in row.visuals.mesh.vertices[range].as_chunks_mut::<4>().0 {
                                let center = quad.iter().map(|v| v.pos.x).sum::<f32>() / 4.0;
                                if left <= center && center < right {
                                    for vertex in quad {
                                        vertex.color = self.colors.selected_text;
                                    }
                                }
                            }
                        }
                        offset += row.char_count_including_newline();
                    }
                });
            }
        });
    }
}

fn syntax_for(name: &str) -> &'static SyntaxReference {
    let name = match name {
        "text" => "Plain Text",
        "TypeScript" | "TSX" | "JSX" => "JavaScript",
        "Vue" | "Svelte" => "HTML",
        _ => name,
    };
    syntaxes()
        .find_syntax_by_name(name)
        .unwrap_or_else(|| syntaxes().find_syntax_plain_text())
}

pub(crate) fn configure(file: &mut crate::LoadedFile, theme: &WorkspaceTheme, dark: bool) {
    let language = effective_language(file);
    let mut render = Render::prepare(
        theme,
        dark,
        language,
        (file.content_revision, file.editor_state.large.revision),
        file.editor_state.syntax_cache.clone(),
    );
    // JSONC permits multiline block comments, unlike JSON / JSONL. Keep its
    // stateful grammar instead of treating every viewport as independent.
    let path = file.source_path.as_deref().unwrap_or(&file.path);
    if path
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonc"))
    {
        render.fast_json = false;
    }
    file.editor_state.tools.workspace = Some(render);
}

pub(crate) fn effective_language(file: &crate::LoadedFile) -> String {
    file.language_override.clone().unwrap_or_else(|| {
        let path = file.source_path.as_deref().unwrap_or(&file.path);
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        match ext.as_str() {
            "ts" => "TypeScript".to_owned(),
            "tsx" => "TSX".to_owned(),
            "jsx" => "JSX".to_owned(),
            "vue" => "Vue".to_owned(),
            "svelte" => "Svelte".to_owned(),
            _ => {
                let detected = language(path, false);
                if detected == "Plain Text" {
                    "text".to_owned()
                } else {
                    detected
                }
            }
        }
    })
}

pub(crate) fn language_options() -> Vec<String> {
    let mut names: Vec<_> = syntaxes()
        .syntaxes()
        .iter()
        .map(|s| s.name.clone())
        .filter(|s| s != "Plain Text")
        .chain(["TypeScript", "TSX", "JSX", "Vue", "Svelte", "Log"].map(str::to_owned))
        .collect();
    names.sort_unstable();
    names.dedup();
    let common = [
        "text",
        "JSON",
        "Log",
        "Python",
        "JavaScript",
        "TypeScript",
        "TSX",
        "Rust",
        "C",
        "C++",
        "Java",
        "Go",
        "HTML",
        "CSS",
        "SQL",
    ];
    let mut options: Vec<_> = common.into_iter().map(str::to_owned).collect();
    names.retain(|name| !options.contains(name));
    options.extend(names);
    options
}

/// Compact status control; popup grows upwards without changing bar height.
pub(crate) fn language_picker(
    ui: &mut egui::Ui,
    file: &crate::LoadedFile,
    lang: Lang,
    pal: &Palette,
    enabled: bool,
) -> Option<Option<String>> {
    let selected = effective_language(file);
    let label = format!("{selected}  ▴");
    let font = egui::FontId::proportional(12.0);
    let galley = ui.painter().layout_no_wrap(label, font, pal.text_faint);
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(galley.size().x + 8.0, 14.0),
        egui::Sense::click(),
    );
    let id = egui::Id::new(("workspace_language", file.id));
    ui.ctx().data_mut(|d| d.insert_temp(id.with("rect"), rect));
    if !enabled && ui.memory(|m| m.is_popup_open(id)) {
        ui.memory_mut(|m| m.close_popup());
    }
    let mut open = enabled && ui.memory(|m| m.is_popup_open(id));
    if response.hovered() && enabled {
        ui.painter()
            .rect_filled(rect.expand2(egui::vec2(2.0, 3.0)), 4.0, pal.bg_elevated);
    }
    ui.painter()
        .galley(rect.center() - galley.size() * 0.5, galley, pal.text_faint);
    let response = response.on_hover_text(if lang == Lang::ZhCn {
        "工作区语言：按文件名自动识别，也可手动切换"
    } else {
        "Workspace language: detect from filename or choose manually"
    });
    if enabled && response.clicked() {
        open = !open;
        ui.memory_mut(|m| {
            if open {
                m.open_popup(id)
            } else {
                m.close_popup()
            }
        });
    }
    if !open {
        return None;
    }
    let mut picked = None;
    let popup = egui::Area::new(id.with("popup"))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::LEFT_BOTTOM)
        .fixed_pos(rect.left_top() - egui::vec2(0.0, 8.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style())
                .fill(pal.bg_panel)
                .show(ui, |ui| {
                    ui.set_width(220.0);
                    ui.label(
                        egui::RichText::new(if lang == Lang::ZhCn {
                            "工作区语言"
                        } else {
                            "Workspace language"
                        })
                        .strong(),
                    );
                    let auto = ui.selectable_label(
                        file.language_override.is_none(),
                        if lang == Lang::ZhCn {
                            "自动识别（文件名）"
                        } else {
                            "Auto detect (filename)"
                        },
                    );
                    ui.ctx()
                        .data_mut(|d| d.insert_temp(id.with("auto"), auto.rect));
                    if auto.clicked() {
                        picked = Some(None);
                    }
                    ui.separator();
                    // This is a menu: moving from the status button to an item must
                    // not become an inertial drag that scrolls on the next opening.
                    egui::ScrollArea::vertical()
                        .id_salt(id.with("scroll"))
                        .drag_to_scroll(false)
                        .max_height((ui.ctx().screen_rect().height() - 160.0).clamp(100.0, 340.0))
                        .show(ui, |ui| {
                            for name in language_options() {
                                let row = ui.selectable_label(selected == name, &name);
                                ui.ctx()
                                    .data_mut(|d| d.insert_temp(id.with(&name), row.rect));
                                if row.clicked() {
                                    picked = Some(Some(name));
                                }
                            }
                        });
                });
        });
    if picked.is_some()
        || ui.input(|i| {
            i.key_pressed(egui::Key::Escape)
                || i.pointer.any_click()
                    && i.pointer
                        .interact_pos()
                        .is_some_and(|p| !rect.contains(p) && !popup.response.rect.contains(p))
        })
    {
        ui.memory_mut(|m| m.close_popup());
    }
    picked
}

fn color_card(ui: &mut egui::Ui, pal: &Palette, title: &str, entries: &mut [(&str, &mut Color32)]) {
    egui::Frame::new()
        .fill(pal.bg_elevated.gamma_multiply(0.45))
        .corner_radius(8.0)
        .inner_margin(12.0)
        .show(ui, |ui| {
            ui.set_min_width((ui.available_width() - 1.0).max(0.0));
            ui.label(egui::RichText::new(title).strong().size(14.0));
            ui.add_space(5.0);
            for (label, color) in entries {
                ui.horizontal(|ui| {
                    ui.set_min_height(25.0);
                    ui.label(*label);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.color_edit_button_srgba(color);
                    });
                });
            }
        });
}

fn theme_preview(
    ui: &mut egui::Ui,
    theme: &WorkspaceTheme,
    colors: Colors,
    pal: &Palette,
    zh: bool,
) {
    ui.label(
        egui::RichText::new(if zh { "实时预览" } else { "Live preview" })
            .strong()
            .size(14.0),
    );
    ui.label(
        egui::RichText::new("Python · JSON")
            .small()
            .color(pal.text_faint),
    );
    for (language, sample) in [
        (
            "Python",
            "# LiteText\ndef greet(name):\n    count = 42\n    return \"Hello\"\n",
        ),
        (
            "JSON",
            "{\n  \"name\": \"LiteText\",\n  \"enabled\": true,\n  \"count\": 42\n}",
        ),
    ] {
        let preview = WorkspaceTheme {
            custom: Some(colors),
            ..theme.clone()
        };
        let render = Render::prepare(
            &preview,
            pal.dark,
            language.to_owned(),
            (0, 0),
            Default::default(),
        );
        let job = render.layout(
            sample,
            egui::FontId::monospace(12.5),
            (ui.available_width() - 20.0).max(20.0),
            0,
        );
        egui::Frame::new()
            .fill(colors.background)
            .corner_radius(6.0)
            .inner_margin(10.0)
            .show(ui, |ui| {
                ui.label(job);
            });
        ui.add_space(8.0);
    }
    ui.label(
        egui::RichText::new(if zh {
            "界面背景与文字同步跟随此配色\n颜色修改不影响文件内容"
        } else {
            "App backgrounds and labels follow this palette\nColors never modify file content"
        })
        .small()
        .color(pal.text_faint),
    );
}

impl LiteTextApp {
    pub(crate) fn migrate_legacy_theme(&mut self) {
        if let Some(legacy) = self.legacy_theme.take()
            && self.workspace_theme.preset == Preset::Auto
            && self.workspace_theme.custom.is_none()
        {
            self.workspace_theme.preset = match legacy {
                crate::Theme::Dark => Preset::DarkPlus,
                crate::Theme::Light => Preset::LightPlus,
                crate::Theme::System => Preset::Auto,
            };
        }
    }

    pub(crate) fn workspace_theme_ui(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let zh = self.lang == Lang::ZhCn;
        if ui
            .button(if zh {
                "‹ 返回系统设置"
            } else {
                "‹ Back to settings"
            })
            .clicked()
        {
            self.workspace_theme_page = false;
        }
        ui.heading(if zh { "全局主题" } else { "Global theme" });
        ui.label(
            egui::RichText::new(if zh {
                "统一软件界面与编辑器配色 · 修改即时生效"
            } else {
                "Unified app and editor colors · Changes apply immediately"
            })
            .color(pal.text_faint),
        );
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(if zh { "内置风格" } else { "Preset" });
            if let Some(preset) = crate::dropdown(
                ui,
                pal,
                egui::Id::new("workspace_theme_preset"),
                self.workspace_theme.preset.label(zh).to_owned(),
                Preset::ALL
                    .into_iter()
                    .map(|p| (p.label(zh).to_owned(), p))
                    .collect(),
                230.0,
            ) {
                self.workspace_theme.preset = preset;
                self.workspace_theme.custom = None;
            }
            if self.workspace_theme.custom.is_some() {
                ui.label(
                    egui::RichText::new(if zh { "自定义" } else { "Custom" }).color(pal.accent),
                );
            }
            if ui
                .button(if zh { "恢复预设" } else { "Reset colors" })
                .clicked()
            {
                self.workspace_theme.custom = None;
            }
        });
        ui.add_space(6.0);
        ui.checkbox(
            &mut self.workspace_theme.syntax,
            if zh {
                "编程语言语法配色（语言在左下角切换）"
            } else {
                "Syntax highlighting (choose language in the status bar)"
            },
        );
        ui.add_space(10.0);
        let mut colors = self.workspace_theme.colors(pal.dark);
        let before = colors;
        let wide = ui.available_width() >= 860.0;
        let scroll = egui::ScrollArea::vertical()
            .id_salt("workspace_theme_scroll")
            .max_height((ui.ctx().screen_rect().height() - 315.0).max(80.0));
        let output = scroll.show(ui, |ui| {
            let columns = if wide { 3 } else { 2 };
            ui.columns(columns, |uis| {
                color_card(
                    &mut uis[0],
                    pal,
                    if zh {
                        "正文与结构"
                    } else {
                        "Text & structure"
                    },
                    &mut [
                        (if zh { "正文" } else { "Text" }, &mut colors.text),
                        (
                            if zh { "背景" } else { "Background" },
                            &mut colors.background,
                        ),
                        (if zh { "括号" } else { "Brackets" }, &mut colors.bracket),
                        (if zh { "行号" } else { "Line numbers" }, &mut colors.gutter),
                        (if zh { "光标" } else { "Cursor" }, &mut colors.cursor),
                    ],
                );
                uis[0].add_space(10.0);
                color_card(
                    &mut uis[0],
                    pal,
                    if zh {
                        "选中与搜索"
                    } else {
                        "Selection & search"
                    },
                    &mut [
                        (
                            if zh { "选区背景" } else { "Selection" },
                            &mut colors.selection,
                        ),
                        (
                            if zh { "选中文字" } else { "Selected text" },
                            &mut colors.selected_text,
                        ),
                        (
                            if zh { "搜索命中" } else { "Search result" },
                            &mut colors.search,
                        ),
                    ],
                );
                color_card(
                    &mut uis[1],
                    pal,
                    if zh {
                        "语言语法"
                    } else {
                        "Language syntax"
                    },
                    &mut [
                        (if zh { "关键字" } else { "Keywords" }, &mut colors.keyword),
                        (if zh { "字符串" } else { "Strings" }, &mut colors.string),
                        (if zh { "注释" } else { "Comments" }, &mut colors.comment),
                        (if zh { "数字" } else { "Numbers" }, &mut colors.number),
                        (if zh { "函数" } else { "Functions" }, &mut colors.function),
                        (
                            if zh {
                                "类型 / 类"
                            } else {
                                "Types / classes"
                            },
                            &mut colors.type_name,
                        ),
                        (
                            if zh {
                                "属性 / 变量"
                            } else {
                                "Properties / variables"
                            },
                            &mut colors.property,
                        ),
                    ],
                );
                if wide {
                    theme_preview(&mut uis[2], &self.workspace_theme, colors, pal, zh);
                }
            });
            if !wide {
                ui.add_space(10.0);
                theme_preview(ui, &self.workspace_theme, colors, pal, zh);
            }
        });
        ui.ctx().data_mut(|d| {
            d.insert_temp(
                egui::Id::new("workspace_theme_scroll_geometry"),
                (output.content_size, output.inner_rect),
            )
        });
        if colors != before {
            self.workspace_theme.custom = Some(colors);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_highlighting_is_theme_aware_utf8_safe_and_viewport_only() {
        assert_eq!(language(Path::new("task.LOG"), false), "Log");
        assert!(language_options().contains(&"Log".to_owned()));
        let text = "2026-11-13 12:34:56.123 INFO 中文\r\n[WARNING] retry 3\nERROR failed \"a🙂\"\nDEBUG details\n";
        let tokens = log_tokens(text);
        let mut at = 0;
        for token in &tokens {
            assert_eq!(at, token.range.start);
            assert!(
                text.is_char_boundary(token.range.start) && text.is_char_boundary(token.range.end)
            );
            at = token.range.end;
        }
        assert_eq!(at, text.len());
        assert!(tokens.iter().any(|t| t.kind == Kind::LogError));
        assert!(tokens.iter().any(|t| t.kind == Kind::LogWarning));
        for dark in [false, true] {
            let cache = Arc::new(Mutex::new(SyntaxCache::default()));
            let render = Render::prepare(
                &WorkspaceTheme::default(),
                dark,
                "Log".into(),
                (1, 1),
                cache.clone(),
            );
            let full = format!("{}{}", "prefix\n".repeat(10000), text);
            let offset = full.len() - text.len();
            render.prepare_prefix(&full, offset);
            assert!(cache.lock().unwrap().checkpoints.is_empty());
            let job = render.layout(text, egui::FontId::monospace(14.0), f32::INFINITY, offset);
            assert_eq!(job.text, text);
            assert!(
                job.sections
                    .iter()
                    .any(|s| s.format.color == Kind::LogError.color(render.colors))
            );
            assert_ne!(
                Kind::LogError.color(render.colors),
                Kind::LogWarning.color(render.colors)
            );
            assert_ne!(Kind::LogError.color(render.colors), render.colors.text);
            let _ = render.layout(text, egui::FontId::monospace(14.0), f32::INFINITY, offset);
            assert!(cache.lock().unwrap().checkpoints.is_empty());
        }
    }

    #[test]
    fn global_palettes_drive_app_and_editor_and_follow_system_without_legacy_dependency() {
        let ctx = egui::Context::default();
        for preset in Preset::ALL {
            for system in [egui::Theme::Dark, egui::Theme::Light] {
                let theme = WorkspaceTheme {
                    preset,
                    ..Default::default()
                };
                let colors = theme.colors(system == egui::Theme::Dark);
                let pal = theme.palette(Some(system));
                assert_eq!(pal.bg_window, colors.background);
                assert_ne!(pal.bg_panel, pal.text_main);
                assert_ne!(pal.bg_elevated, pal.text_main);
                crate::apply_style(&ctx, &pal);
                assert_eq!(ctx.style().visuals.window_fill, pal.bg_window);
                assert_eq!(ctx.style().visuals.override_text_color, Some(pal.text_main));
                let mut app = LiteTextApp {
                    workspace_theme: theme,
                    ..Default::default()
                };
                app.new_document();
                let theme = app.workspace_theme.clone();
                configure(app.active_mut().unwrap(), &theme, pal.dark);
                assert_eq!(
                    app.active()
                        .unwrap()
                        .editor_state
                        .tools
                        .workspace
                        .as_ref()
                        .unwrap()
                        .colors,
                    colors
                );
                let output = ctx.run(raw(vec![], egui::vec2(1080.0, 720.0)), |ctx| {
                    app.top_bar(ctx, &pal);
                    app.status_bar(ctx, &pal);
                });
                assert!(
                    output.shapes.iter().any(
                        |s| matches!(&s.shape, egui::Shape::Rect(r) if r.fill == pal.bg_panel)
                    )
                );
            }
        }
        let mut theme = WorkspaceTheme::default();
        let mut custom = theme.colors(true);
        custom.background = Color32::from_rgb(32, 40, 52);
        custom.text = custom.background;
        theme.custom = Some(custom);
        assert_eq!(theme.palette(None).bg_window, custom.background);
        assert_ne!(theme.palette(None).text_main, custom.background);
    }

    #[test]
    fn old_theme_migrates_once_and_settings_expose_only_global_theme() {
        for (old, preset) in [
            ("Dark", Preset::DarkPlus),
            ("Light", Preset::LightPlus),
            ("System", Preset::Auto),
        ] {
            let mut app: LiteTextApp =
                serde_json::from_str(&format!(r#"{{"theme":"{old}"}}"#)).unwrap();
            app.migrate_legacy_theme();
            assert_eq!(app.workspace_theme.preset, preset);
            assert!(app.legacy_theme.is_none());
            let json = serde_json::to_value(&app).unwrap();
            assert!(json.get("theme").is_none());
        }
        let mut app: LiteTextApp =
            serde_json::from_str(r#"{"theme":"Light","workspace_theme":{"preset":"Monokai"}}"#)
                .unwrap();
        app.migrate_legacy_theme();
        assert_eq!(app.workspace_theme.preset, Preset::Monokai);
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        app.show_settings = true;
        let pal = app.workspace_theme.palette(None);
        crate::apply_style(&ctx, &pal);
        let mut output = None;
        for _ in 0..3 {
            output = Some(ctx.run(raw(vec![], egui::vec2(1080.0, 720.0)), |ctx| {
                app.settings_window(ctx, &pal)
            }));
        }
        let labels: Vec<_> = output
            .as_ref()
            .unwrap()
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some(t.galley.text()),
                _ => None,
            })
            .collect();
        assert!(labels.contains(&"全局主题    ›"));
        for obsolete in ["主题", "黑夜", "白天", "工作区主题    ›"] {
            assert!(!labels.contains(&obsolete));
        }
    }

    #[test]
    fn builtin_palettes_are_distinct_persist_and_restore_after_customization() {
        let mut palettes = Vec::new();
        for preset in Preset::ALL {
            let mut theme = WorkspaceTheme {
                preset,
                ..Default::default()
            };
            let colors = theme.colors(true);
            if preset != Preset::Auto {
                assert_eq!(theme.colors(false), colors);
                assert!(!palettes.contains(&colors), "duplicate {preset:?}");
                palettes.push(colors);
            }
            assert_ne!(colors.text, colors.background);
            assert_ne!(colors.selected_text, colors.selection);
            theme.custom = Some(Colors {
                text: Color32::RED,
                ..colors
            });
            let mut restored: WorkspaceTheme =
                serde_json::from_str(&serde_json::to_string(&theme).unwrap()).unwrap();
            assert_eq!(restored.preset, preset);
            assert_eq!(restored.colors(true).text, Color32::RED);
            restored.custom = None;
            assert_eq!(restored.colors(true), colors);
        }
    }

    #[test]
    fn settings_buttons_and_theme_controls_follow_dark_light_switches() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let mut app = LiteTextApp {
            show_settings: true,
            ..Default::default()
        };
        for pal in [&crate::LIGHT, &crate::DARK, &crate::LIGHT, &crate::DARK] {
            crate::apply_style(&ctx, pal);
            let visuals = ctx.style().visuals.clone();
            assert_eq!(visuals.dark_mode, pal.dark);
            assert_eq!(visuals.widgets.inactive.weak_bg_fill, pal.bg_elevated);
            assert_eq!(visuals.widgets.hovered.weak_bg_fill, pal.bg_elevated);
            assert_eq!(
                visuals.widgets.noninteractive.fg_stroke.color,
                pal.text_main
            );
            for page in [false, true] {
                app.workspace_theme_page = page;
                let mut output = None;
                for _ in 0..3 {
                    output = Some(ctx.run(raw(vec![], egui::vec2(1080.0, 720.0)), |ctx| {
                        app.settings_window(ctx, pal)
                    }));
                }
                let labels: Vec<_> = output
                    .as_ref()
                    .unwrap()
                    .shapes
                    .iter()
                    .filter_map(|s| match &s.shape {
                        egui::Shape::Text(text) => Some(text),
                        _ => None,
                    })
                    .collect();
                let expected = if page {
                    vec!["‹ 返回系统设置", "恢复预设", "正文与结构", "语言语法"]
                } else {
                    vec!["全局主题    ›", "快捷键    ›"]
                };
                for label in expected {
                    let text = labels
                        .iter()
                        .find(|text| text.galley.text() == label)
                        .unwrap_or_else(|| panic!("missing {label}"));
                    let actual = text.fallback_color.to_array();
                    let expected = pal
                        .text_main
                        .gamma_multiply(text.fallback_color.a() as f32 / 255.0)
                        .to_array();
                    for channel in 0..3 {
                        assert!(
                            actual[channel].abs_diff(expected[channel]) <= 2,
                            "{label}: {actual:?} vs {expected:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn json_fast_path_preserves_utf8_escapes_and_line_independent_viewports() {
        let render = Render::prepare(
            &WorkspaceTheme::default(),
            true,
            "JSON".to_owned(),
            (1, 0),
            Default::default(),
        );
        let sample = concat!(
            r#"{"中🙂\"key": "[not a bracket] \\" ,"n":-1.2e+30,"yes":true,"no":false,"nil":null}"#,
            "\r\n"
        );
        let job = render.layout(sample, egui::FontId::monospace(13.5), f32::INFINITY, 0);
        assert_eq!(job.text, sample);
        assert_eq!(color_at(&job, "中"), render.colors.property);
        assert_eq!(color_at(&job, "[not"), render.colors.string);
        assert_eq!(color_at(&job, "-1.2"), render.colors.number);
        for literal in ["true", "false", "null"] {
            assert_eq!(color_at(&job, literal), render.colors.keyword);
        }
        for text in [
            sample,
            "{\"unfinished\n{\"next\":42}\n",
            "🙂\\\"",
            "{\"escaped\":\"中\\🙂\"}",
        ] {
            let tokens = json_tokens(text);
            let mut end = 0;
            for token in tokens {
                assert_eq!(token.range.start, end);
                assert!(
                    text.is_char_boundary(token.range.start)
                        && text.is_char_boundary(token.range.end)
                );
                end = token.range.end;
            }
            assert_eq!(end, text.len());
        }
        // Deep scrolling and a new revision never parse the offscreen prefix.
        let full = format!("{}{}", sample.repeat(20_000), sample);
        let offset = full.len() - sample.len();
        render.prepare_prefix(&full, offset);
        let tail = render.layout(
            &full[offset..],
            egui::FontId::monospace(13.5),
            f32::INFINITY,
            offset,
        );
        assert_eq!(tail.sections, job.sections);
        assert!(render.cache.lock().unwrap().checkpoints.is_empty());
        let updated = Render::prepare(
            &WorkspaceTheme::default(),
            true,
            "JSON".to_owned(),
            (2, 0),
            render.cache.clone(),
        );
        updated.prepare_prefix(&full, offset);
        assert!(updated.cache.lock().unwrap().checkpoints.is_empty());
        let mut app = LiteTextApp::default();
        app.new_document();
        app.active_mut().unwrap().path = Path::new("comments.JSONC").to_owned();
        configure(app.active_mut().unwrap(), &WorkspaceTheme::default(), true);
        assert!(
            !app.active()
                .unwrap()
                .editor_state
                .tools
                .workspace
                .as_ref()
                .unwrap()
                .fast_json
        );
    }

    fn render(path: &str, text: &str) -> egui::text::LayoutJob {
        Render::prepare(
            &WorkspaceTheme::default(),
            true,
            language(Path::new(path), false),
            (1, 0),
            Default::default(),
        )
        .layout(text, egui::FontId::monospace(13.5), f32::INFINITY, 0)
    }

    fn color_at(job: &egui::text::LayoutJob, needle: &str) -> Color32 {
        let at = job.text.find(needle).unwrap();
        job.sections
            .iter()
            .find(|section| section.byte_range.contains(&at))
            .unwrap()
            .format
            .color
    }

    #[test]
    fn language_grammars_and_dark_plus_tokens_color_code_without_changing_bytes() {
        for (path, sample, keyword, string, comment) in [
            (
                "a.py",
                "def greet(name):\n    # 中文\n    return \"hello\"\n",
                "def",
                "hello",
                "#",
            ),
            (
                "a.rs",
                "fn main() {\n // 中文\n let x = \"hello\";\n}\n",
                "fn",
                "hello",
                "//",
            ),
            (
                "a.js",
                "function greet() {\n // 中文\n return \"hello\";\n}\n",
                "function",
                "hello",
                "//",
            ),
            (
                "a.java",
                "class Test {\n // 中文\n String s = \"hello\";\n}\n",
                "class",
                "hello",
                "//",
            ),
            (
                "a.go",
                "package main\n // 中文\n var s = \"hello\"\n",
                "package",
                "hello",
                "//",
            ),
            (
                "a.cpp",
                "int main() {\n // 中文\n return \"hello\";\n}\n",
                "return",
                "hello",
                "//",
            ),
        ] {
            assert_ne!(language(Path::new(path), false), "Plain Text", "{path}");
            let job = render(path, sample);
            let colors = Colors::preset(true);
            assert_eq!(job.text, sample);
            assert_eq!(color_at(&job, keyword), colors.keyword, "keyword {path}");
            assert_eq!(color_at(&job, string), colors.string, "string {path}");
            assert_eq!(color_at(&job, comment), colors.comment, "comment {path}");
            let mut position = 0;
            for section in &job.sections {
                assert_eq!(section.byte_range.start, position);
                assert!(sample.is_char_boundary(section.byte_range.end));
                position = section.byte_range.end;
            }
            assert_eq!(position, sample.len());
        }
        for extension in [
            "json", "html", "css", "sql", "sh", "rb", "yaml", "xml", "md",
        ] {
            assert_ne!(
                language(Path::new(&format!("a.{extension}")), false),
                "Plain Text",
                "{extension}"
            );
        }
        assert_eq!(language(Path::new("a.ts"), false), "JavaScript");
        assert_eq!(language(Path::new("a.unknown"), false), "Plain Text");
        assert_eq!(language(Path::new(""), true), "JSON");
        let job = render("a.json", "{\"key\": [42, true, \"(hello)\"]}");
        let colors = Colors::preset(true);
        assert_eq!(color_at(&job, "{"), colors.bracket);
        assert_eq!(color_at(&job, "key"), colors.property);
        assert_eq!(color_at(&job, "42"), colors.number);
        assert_eq!(color_at(&job, "true"), colors.keyword);
        assert_eq!(color_at(&job, "(hello)"), colors.string);
    }

    #[test]
    fn multiline_syntax_uses_prefix_checkpoints_and_rebuilds_on_edit() {
        let cache: Arc<Mutex<SyntaxCache>> = Default::default();
        let theme = WorkspaceTheme::default();
        let render = Render::prepare(&theme, true, "Python".to_owned(), (1, 0), cache.clone());
        let full = format!(
            "{}value = \"\"\"\ninside (42)\n\"\"\"\nprint(42)\n",
            "# prefix\n".repeat(300)
        );
        let offset = full.find("inside").unwrap();
        while !render.prepare_prefix(&full, offset) {}
        let job = render.layout(
            &full[offset..],
            egui::FontId::monospace(13.5),
            f32::INFINITY,
            offset,
        );
        assert_eq!(color_at(&job, "inside"), render.colors.string);
        assert_eq!(color_at(&job, "(42)"), render.colors.string);
        assert_eq!(color_at(&job, "print"), render.colors.function);
        assert!(cache.lock().unwrap().checkpoints.len() >= 3);
        let again = render.tokens(&full[offset..], offset);
        assert!(Arc::ptr_eq(&again, &render.tokens(&full[offset..], offset)));
        let updated = Render::prepare(&theme, true, "Python".to_owned(), (2, 0), cache.clone());
        assert!(cache.lock().unwrap().checkpoints.is_empty());
        let job = updated.layout(
            "value = 42\n",
            egui::FontId::monospace(13.5),
            f32::INFINITY,
            0,
        );
        assert_eq!(color_at(&job, "42"), updated.colors.number);
    }

    #[test]
    fn prefix_preparation_yields_and_long_lines_use_plain_tokens() {
        let cache: Arc<Mutex<SyntaxCache>> = Default::default();
        let render = Render::prepare(&WorkspaceTheme::default(), true, "Python".into(), (1, 0), cache.clone());
        let full = "# prefix\n".repeat(2000);
        assert!(!render.prepare_prefix(&full, full.len()));
        let first = *cache.lock().unwrap().checkpoints.keys().next_back().unwrap();
        assert!(first > 0 && first <= "# prefix\n".len() * 512);
        let mut frames = 1;
        while !render.prepare_prefix(&full, full.len()) { frames += 1; assert!(frames < 2000); }
        let long = "x".repeat(20 * 1024);
        let tokens = render.tokens(&long, 0);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].range, 0..long.len());
        assert!(matches!(tokens[0].kind, Kind::Text));
    }

    #[test]
    fn palettes_persist_reset_follow_theme_and_merge_markers_and_search() {
        let mut app = LiteTextApp::default();
        assert_ne!(
            app.workspace_theme.colors(true),
            app.workspace_theme.colors(false)
        );
        app.workspace_theme.preset = Preset::LightPlus;
        assert_eq!(
            app.workspace_theme.colors(true),
            app.workspace_theme.colors(false)
        );
        let mut colors = app.workspace_theme.colors(true);
        colors.bracket = Color32::from_rgb(17, 31, 47);
        colors.text = Color32::from_rgb(23, 41, 59);
        app.workspace_theme.custom = Some(colors);
        let restored: LiteTextApp =
            serde_json::from_str(&serde_json::to_string(&app).unwrap()).unwrap();
        assert_eq!(restored.workspace_theme.colors(false), colors);
        let legacy: LiteTextApp = serde_json::from_str("{}").unwrap();
        assert_eq!(legacy.workspace_theme.colors(true), Colors::preset(true));
        app.workspace_theme.custom = None;
        assert_eq!(app.workspace_theme.colors(true), Colors::preset(false));
        let text = "{\"key\": 42}";
        let mut tools = crate::text_tools::Tools::default();
        tools.workspace = Some(Render::prepare(
            &WorkspaceTheme {
                custom: Some(colors),
                ..Default::default()
            },
            true,
            "JSON".to_owned(),
            (0, 0),
            Default::default(),
        ));
        tools.find_open = true;
        tools.current = Some(8..10);
        let job = crate::text_tools::layout(
            text,
            egui::FontId::monospace(13.5),
            &crate::DARK,
            f32::INFINITY,
            &tools,
        );
        assert_eq!(color_at(&job, "{"), colors.bracket);
        let number = job
            .sections
            .iter()
            .find(|s| s.byte_range.contains(&8))
            .unwrap();
        assert_eq!(number.format.background, colors.search);
        assert_eq!(number.format.color, colors.number);
        tools.marks.push(crate::text_tools::Mark {
            range: 8..10,
            color: 0,
        });
        let job = crate::text_tools::layout(
            text,
            egui::FontId::monospace(13.5),
            &crate::DARK,
            f32::INFINITY,
            &tools,
        );
        let number = job
            .sections
            .iter()
            .find(|s| s.byte_range.contains(&8))
            .unwrap();
        assert_eq!(
            number.format.background,
            crate::text_tools::pen_color(0).gamma_multiply(0.30)
        );
        assert_eq!(number.format.color, colors.number);
        tools.workspace.as_mut().unwrap().syntax = false;
        let job = crate::text_tools::layout(
            text,
            egui::FontId::monospace(13.5),
            &crate::DARK,
            f32::INFINITY,
            &tools,
        );
        assert_eq!(color_at(&job, "{"), colors.text);
    }

    fn raw(events: Vec<egui::Event>, size: egui::Vec2) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            events,
            ..Default::default()
        }
    }

    #[test]
    fn status_language_picker_updates_only_current_tab_and_colors_then_returns_to_auto() {
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.py");
        let original = "def greet():\n    return 42\n";
        std::fs::write(&path, original).unwrap();
        let mut app = LiteTextApp::default();
        app.new_document();
        let scratch_id = app.active_id.unwrap();
        assert_eq!(effective_language(app.active().unwrap()), "text");
        app.open_paths(vec![path.clone()]);
        let id = app.active_id.unwrap();
        assert_eq!(effective_language(app.active().unwrap()), "Python");
        let run = |app: &mut LiteTextApp, events| {
            ctx.run(raw(events, egui::vec2(1080.0, 720.0)), |ctx| {
                app.status_bar(ctx, &crate::DARK)
            })
        };
        let click = |app: &mut LiteTextApp, rect: egui::Rect| {
            for pressed in [true, false] {
                run(
                    app,
                    vec![
                        egui::Event::PointerMoved(rect.center()),
                        egui::Event::PointerButton {
                            pos: rect.center(),
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: Default::default(),
                        },
                    ],
                );
            }
        };
        let popup_id = egui::Id::new(("workspace_language", id));
        let get_rect = |suffix: &str| {
            ctx.data(|d| d.get_temp::<egui::Rect>(popup_id.with(suffix)))
                .unwrap()
        };
        run(&mut app, vec![]);
        run(&mut app, vec![]);
        let rect = get_rect("rect");
        assert!(rect.left() < 20.0);
        click(&mut app, rect);
        run(&mut app, vec![]);
        run(&mut app, vec![]);
        let text_row = get_rect("text");
        assert!(text_row.bottom() < rect.top());
        click(&mut app, text_row);
        assert_eq!(
            app.active().unwrap().language_override.as_deref(),
            Some("text")
        );
        assert_eq!(
            app.active()
                .unwrap()
                .editor_state
                .tools
                .workspace
                .as_ref()
                .unwrap()
                .language,
            "text"
        );
        assert_eq!(
            effective_language(app.files.iter().find(|f| f.id == scratch_id).unwrap()),
            "text"
        );
        assert!(!app.active().unwrap().has_unsaved_changes());
        run(&mut app, vec![]);
        click(&mut app, get_rect("rect"));
        run(&mut app, vec![]);
        run(&mut app, vec![]);
        assert!(ctx.memory(|m| m.is_popup_open(popup_id)));
        click(&mut app, get_rect("JSON"));
        assert_eq!(effective_language(app.active().unwrap()), "JSON");
        let render = app
            .active()
            .unwrap()
            .editor_state
            .tools
            .workspace
            .as_ref()
            .unwrap();
        let job = render.layout(
            "{\"a\":42}",
            egui::FontId::monospace(13.5),
            f32::INFINITY,
            0,
        );
        assert_eq!(color_at(&job, "{"), render.colors.bracket);
        run(&mut app, vec![]);
        click(&mut app, get_rect("rect"));
        run(&mut app, vec![]);
        run(&mut app, vec![]);
        click(&mut app, get_rect("auto"));
        assert_eq!(effective_language(app.active().unwrap()), "Python");
        assert!(app.active().unwrap().language_override.is_none());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        // Clicking an already open control closes it instead of redrawing it.
        click(&mut app, get_rect("rect"));
        assert!(ctx.memory(|m| m.is_popup_open(popup_id)));
        click(&mut app, get_rect("rect"));
        assert!(!ctx.memory(|m| m.is_popup_open(popup_id)));
        click(&mut app, get_rect("rect"));
        assert!(ctx.memory(|m| m.is_popup_open(popup_id)));
        // Disable switching while settings are modal.
        app.show_settings = true;
        run(&mut app, vec![]);
        click(&mut app, get_rect("rect"));
        assert!(!ctx.memory(|m| m.is_popup_open(popup_id)));
    }

    #[test]
    fn language_overrides_restore_and_json_formatting_changes_language_only_on_success() {
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.py");
        let snapshot = dir.path().join("session.json");
        std::fs::write(&path, "{\"a\":1}").unwrap();
        let mut app = LiteTextApp {
            recovery_path_override: Some(snapshot.clone()),
            ..Default::default()
        };
        app.open_paths(vec![path.clone()]);
        app.write_recovery().unwrap();
        app.active_mut().unwrap().language_override = Some("Rust".to_owned());
        app.write_recovery().unwrap();
        let restored = crate::recovery::load(&snapshot).unwrap().unwrap().restore();
        assert_eq!(effective_language(&restored[0]), "Rust");
        assert!(!restored[0].has_unsaved_changes());
        app.format_active_json(&ctx);
        assert_eq!(effective_language(app.active().unwrap()), "JSON");
        configure(app.active_mut().unwrap(), &WorkspaceTheme::default(), true);
        assert_eq!(
            app.active()
                .unwrap()
                .editor_state
                .tools
                .workspace
                .as_ref()
                .unwrap()
                .language,
            "JSON"
        );
        app.write_recovery().unwrap();
        let restored = crate::recovery::load(&snapshot).unwrap().unwrap().restore();
        assert_eq!(effective_language(&restored[0]), "JSON");
        app.active_mut().unwrap().language_override = Some("Python".to_owned());
        let length = app.active().unwrap().draft_text.as_ref().unwrap().len();
        crate::text_tools::replace_range(
            app.active_mut().unwrap(),
            &ctx,
            0..length,
            "not json".to_owned(),
        );
        app.format_active_json(&ctx);
        assert!(app.plugin_error.is_some());
        assert_eq!(effective_language(app.active().unwrap()), "Python");
        // An explicit text choice wins over the fact JSON folding was enabled.
        app.active_mut().unwrap().language_override = Some("text".to_owned());
        configure(app.active_mut().unwrap(), &WorkspaceTheme::default(), true);
        assert_eq!(
            app.active()
                .unwrap()
                .editor_state
                .tools
                .workspace
                .as_ref()
                .unwrap()
                .language,
            "text"
        );
        let render = app
            .active()
            .unwrap()
            .editor_state
            .tools
            .workspace
            .as_ref()
            .unwrap();
        render.prepare_prefix("# prefix\nvalue = 42\n", 9);
        let plain = render.layout(
            "{\"a\":42}",
            egui::FontId::monospace(13.5),
            f32::INFINITY,
            0,
        );
        assert_eq!(plain.sections.len(), 1);
        assert_eq!(color_at(&plain, "{"), render.colors.text);
        let cache = render.cache.lock().unwrap();
        assert!(cache.checkpoints.is_empty() && cache.last.is_none());
        drop(cache);
        app.active_mut().unwrap().language_override = Some("SQL".to_owned());
        let persisted: crate::LoadedFile =
            serde_json::from_str(&serde_json::to_string(app.active().unwrap()).unwrap()).unwrap();
        assert_eq!(effective_language(&persisted), "SQL");
        app.active_mut().unwrap().language_override = None;
        assert_eq!(effective_language(app.active().unwrap()), "Python");
        for (name, language, grammar) in [
            ("a.ts", "TypeScript", "JavaScript"),
            ("a.tsx", "TSX", "JavaScript"),
            ("a.vue", "Vue", "HTML"),
            ("a.unknown", "text", "Plain Text"),
        ] {
            app.active_mut().unwrap().path = Path::new(name).to_owned();
            assert_eq!(effective_language(app.active().unwrap()), language);
            assert_eq!(syntax_for(language).name, grammar);
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"a\":1}");
        let options = language_options();
        for name in [
            "text",
            "JSON",
            "Python",
            "Rust",
            "JavaScript",
            "TypeScript",
            "C++",
            "Java",
            "Go",
            "SQL",
        ] {
            assert!(options.iter().any(|option| option == name), "{name}");
        }
    }

    #[test]
    fn workspace_settings_entry_opens_below_theme_and_fits_small_windows() {
        for lang in [Lang::ZhCn, Lang::En] {
            let ctx = egui::Context::default();
            crate::setup_fonts(&ctx);
            let mut app = LiteTextApp {
                lang,
                show_settings: true,
                ..Default::default()
            };
            for _ in 0..2 {
                let _ = ctx.run(raw(vec![], egui::vec2(1080.0, 720.0)), |ctx| {
                    app.settings_window(ctx, &crate::DARK)
                });
            }
            let rect = ctx
                .data(|d| d.get_temp::<egui::Rect>(egui::Id::new("settings_workspace_theme_rect")))
                .unwrap();
            let shortcuts = ctx
                .data(|d| d.get_temp::<egui::Rect>(egui::Id::new("settings_shortcuts_rect")))
                .unwrap();
            assert!(rect.bottom() < shortcuts.top());
            for pressed in [true, false] {
                let _ = ctx.run(
                    raw(
                        vec![
                            egui::Event::PointerMoved(rect.center()),
                            egui::Event::PointerButton {
                                pos: rect.center(),
                                button: egui::PointerButton::Primary,
                                pressed,
                                modifiers: Default::default(),
                            },
                        ],
                        egui::vec2(1080.0, 720.0),
                    ),
                    |ctx| app.settings_window(ctx, &crate::DARK),
                );
            }
            assert!(app.workspace_theme_page && !app.shortcuts_page);
            for size in [egui::vec2(640.0, 420.0), egui::vec2(1080.0, 720.0)] {
                for frame in 0..6 {
                    let output = ctx.run(raw(vec![], size), |ctx| {
                        app.settings_window(ctx, &crate::DARK)
                    });
                    let done = output.shapes.iter().find_map(|s| match &s.shape {
                        egui::Shape::Text(text) if text.galley.text() == crate::tr(lang).done => {
                            Some(text.pos)
                        }
                        _ => None,
                    });
                    assert!(done.is_none(), "Immediate settings have no Done footer");
                    if size.x >= 1080.0 && frame == 5 {
                        let (content, viewport) = ctx
                            .data(|d| {
                                d.get_temp::<(egui::Vec2, egui::Rect)>(egui::Id::new(
                                    "workspace_theme_scroll_geometry",
                                ))
                            })
                            .unwrap();
                        assert!(
                            content.y <= viewport.height() + 1.0,
                            "single screen {lang:?}: content={content:?}, viewport={viewport:?}"
                        );
                        assert!(
                            content.x <= viewport.width() + 1.0,
                            "horizontal overflow {lang:?}: {content:?} {viewport:?}"
                        );
                    }
                }
            }
            let _ = ctx.run(
                raw(
                    vec![egui::Event::Key {
                        key: egui::Key::Escape,
                        pressed: true,
                        repeat: false,
                        physical_key: None,
                        modifiers: Default::default(),
                    }],
                    egui::vec2(1080.0, 720.0),
                ),
                |ctx| app.settings_window(ctx, &crate::DARK),
            );
            assert!(!app.workspace_theme_page && app.show_settings);
        }
    }

    #[test]
    fn both_editors_use_workspace_colors_selection_and_preserve_files() {
        for large in [false, true] {
            let ctx = egui::Context::default();
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("sample.py");
            let original = format!(
                "def greet(name):\n    return \"中🙂\"\n{}",
                "# padding\n".repeat(if large { 30_000 } else { 0 })
            );
            std::fs::write(&path, &original).unwrap();
            let mut app = LiteTextApp::default();
            app.open_paths(vec![path.clone()]);
            let mut colors = Colors::preset(false);
            colors.selected_text = Color32::from_rgb(31, 47, 79);
            colors.selection = Color32::from_rgb(170, 190, 210);
            app.workspace_theme.custom = Some(colors);
            configure(
                app.active_mut().unwrap(),
                &WorkspaceTheme {
                    custom: Some(colors),
                    ..Default::default()
                },
                true,
            );
            let id = app.active_id.unwrap();
            let editor_id = egui::Id::new(if large {
                "large_editor"
            } else {
                "draft_editor"
            })
            .with(id);
            app.active_mut().unwrap().editor_state.pending_selection = Some(0..3);
            let run = |app: &mut LiteTextApp| {
                ctx.run(raw(vec![], egui::vec2(1080.0, 720.0)), |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        crate::draft_editor(
                            ui,
                            app.active_mut().unwrap(),
                            &crate::DARK,
                            true,
                            Lang::En,
                        )
                    });
                })
            };
            run(&mut app);
            ctx.memory_mut(|m| m.request_focus(editor_id));
            let output = run(&mut app);
            assert!(
                output.shapes.iter().any(
                    |s| matches!(&s.shape, egui::Shape::Rect(r) if r.fill == colors.background)
                )
            );
            let painted = output
                .shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::Shape::Text(text) if text.galley.text().starts_with("def greet") => {
                        Some(text.galley.clone())
                    }
                    _ => None,
                })
                .unwrap();
            assert!(
                painted.rows[0]
                    .visuals
                    .mesh
                    .vertices
                    .iter()
                    .any(|v| v.color == colors.selected_text)
            );
            assert!(
                painted.rows[0]
                    .visuals
                    .mesh
                    .vertices
                    .iter()
                    .any(|v| v.color == colors.selection)
            );
            assert_eq!(
                app.active().unwrap().draft_text.as_deref(),
                Some(original.as_str())
            );
            assert!(!app.active().unwrap().has_unsaved_changes());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        }
    }
}

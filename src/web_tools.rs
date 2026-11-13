//! Offline web text tools. Parse completely before committing any editor edit.
use serde_json::{Map, Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Html,
    Header,
    Cookie,
}

impl Action {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Html => "Format HTML",
            Self::Header => "Format Header",
            Self::Cookie => "Format Cookie",
        }
    }
}

pub(crate) fn pretty(value: &Value) -> Result<String, String> {
    serde_json::to_string_pretty(value).map_err(|e| e.to_string())
}

// Repeated keys are arrays, never silently overwritten (Set-Cookie in particular).
pub(crate) fn insert(map: &mut Map<String, Value>, name: String, value: String) {
    match map.get_mut(&name) {
        Some(Value::Array(values)) => values.push(Value::String(value)),
        Some(previous) => *previous = json!([previous.clone(), value]),
        None => {
            map.insert(name, Value::String(value));
        }
    }
}

fn token(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

fn object(input: &str) -> Result<Value, String> {
    let value: Value = serde_json::from_str(input).map_err(|e| e.to_string())?;
    if !value.is_object() {
        return Err("Expected a JSON object".into());
    }
    Ok(value)
}

pub(crate) fn headers(input: &str) -> Result<String, String> {
    let input = input.trim();
    if input.starts_with('{') {
        return pretty(&object(input)?);
    }
    let mut map = Map::new();
    for (index, line) in input.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if index == 0
            && (line.starts_with("HTTP/")
                || line
                    .split_whitespace()
                    .last()
                    .is_some_and(|s| s.starts_with("HTTP/")))
        {
            continue;
        }
        // HTTP/2 pseudo headers have a leading colon.
        let start = usize::from(line.starts_with(':'));
        let (key, value) = line[start..]
            .split_once(':')
            .ok_or_else(|| format!("Header line {}: expected name: value", index + 1))?;
        if !token(key.trim()) {
            return Err(format!("Invalid header name at line {}", index + 1));
        }
        let name = format!("{}{}", if start == 1 { ":" } else { "" }, key.trim());
        let name = map
            .keys()
            .find(|k| k.eq_ignore_ascii_case(&name))
            .cloned()
            .unwrap_or(name);
        insert(&mut map, name, value.trim().to_owned());
    }
    if map.is_empty() {
        return Err("No headers found".into());
    }
    pretty(&Value::Object(map))
}

/// Split cookie pairs without breaking quoted semicolons or '=' in values.
fn cookie_pairs(input: &str, map: &mut Map<String, Value>, first_only: bool) -> Result<(), String> {
    let mut start = 0;
    let mut quote = false;
    let mut escaped = false;
    let mut parts = Vec::new();
    for (at, ch) in input.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if quote && ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '"' {
            quote = !quote;
        }
        if ch == ';' && !quote {
            parts.push(&input[start..at]);
            start = at + 1;
        }
    }
    if quote {
        return Err("Unclosed cookie quote".into());
    }
    parts.push(&input[start..]);
    for part in parts {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (key, value) = part.split_once('=').ok_or("Expected cookie name=value")?;
        if !token(key.trim()) {
            return Err("Invalid cookie name".into());
        }
        insert(map, key.trim().to_owned(), value.trim().to_owned());
        if first_only {
            break;
        }
    }
    Ok(())
}

pub(crate) fn cookies(input: &str) -> Result<String, String> {
    let input = input.trim_matches([' ', '\r', '\n']);
    if input.starts_with('{') {
        return pretty(&object(input)?);
    }
    let mut map = Map::new();
    if input.starts_with('[') {
        let values: Vec<Value> = serde_json::from_str(input).map_err(|e| e.to_string())?;
        for entry in values {
            let name = entry
                .get("name")
                .and_then(Value::as_str)
                .ok_or("Cookie entry needs a string name")?;
            let value = entry
                .get("value")
                .and_then(Value::as_str)
                .ok_or("Cookie entry needs a string value")?;
            if !token(name) {
                return Err("Invalid cookie name".into());
            }
            insert(&mut map, name.into(), value.into());
        }
    } else {
        for line in input.lines() {
            let line = line.trim_matches([' ', '\r', '\n']);
            // Netscape cookie jar exports, including #HttpOnly_ domain records.
            if line.is_empty() || (line.starts_with('#') && !line.starts_with("#HttpOnly_")) {
                continue;
            }
            if line.contains('\t') {
                let fields: Vec<_> = line.split('\t').collect();
                if fields.len() != 7 || !token(fields[5]) {
                    return Err("Invalid Netscape cookie record".into());
                }
                insert(&mut map, fields[5].into(), fields[6].into());
            } else if line
                .get(..11)
                .is_some_and(|s| s.eq_ignore_ascii_case("set-cookie:"))
            {
                // Response attributes are metadata, not additional cookies.
                cookie_pairs(line[11..].trim(), &mut map, true)?;
            } else {
                let line = if line
                    .get(..7)
                    .is_some_and(|s| s.eq_ignore_ascii_case("cookie:"))
                {
                    &line[7..]
                } else {
                    line
                };
                cookie_pairs(line, &mut map, false)?;
            }
        }
    }
    if map.is_empty() {
        return Err("No cookies found".into());
    }
    pretty(&Value::Object(map))
}

pub(crate) fn parse_url(input: &str) -> Result<String, String> {
    let input = input.trim();
    if input.chars().any(char::is_whitespace) {
        return Err("Expected one absolute URL without spaces".into());
    }
    let mut parsed = url::Url::parse(input).map_err(|e| format!("Invalid URL: {e}"))?;
    if parsed.host_str().is_none()
        || !matches!(parsed.scheme(), "http" | "https" | "ftp" | "ws" | "wss")
    {
        return Err("Expected an absolute network URL".into());
    }
    for part in [parsed.path(), parsed.fragment().unwrap_or("")] {
        crate::text_conversion::convert(part, crate::text_conversion::Action::UrlDecode)?;
    }
    let mut params = Map::new();
    // Validate percent encoding and UTF-8 rather than replacing malformed data.
    if let Some(query) = parsed.query() {
        for pair in query.split('&').filter(|s| !s.is_empty()) {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let decode = |s: &str| {
                crate::text_conversion::convert(
                    &s.replace('+', " "),
                    crate::text_conversion::Action::UrlDecode,
                )
            };
            insert(&mut params, decode(key)?, decode(value)?);
        }
    }
    let fragment = parsed.fragment().unwrap_or("").to_owned();
    parsed.set_query(None);
    parsed.set_fragment(None);
    pretty(
        &json!({ "url": parsed.as_str(), "scheme": parsed.scheme(), "host": parsed.host_str(), "port": parsed.port_or_known_default(), "path": parsed.path(), "params": params, "fragment": fragment }),
    )
}

pub(crate) fn convert(input: &str, action: Action) -> Result<String, String> {
    match action {
        Action::Header => headers(input),
        Action::Cookie => cookies(input),
        Action::Html => {
            if input.len() > 32 * 1024 * 1024 {
                return Err("HTML formatting limit is 32 MiB".into());
            }
            // Bound recursive parser depth for pasted/untrusted markup.
            if input.bytes().filter(|b| *b == b'<').count() > 100_000 {
                return Err("HTML formatting limit is 100000 tags".into());
            }
            let (masked, attributes) = mask_braced_attributes(input)?;
            check_html_depth(&masked)?;
            if !input.contains('<') {
                return Err("No HTML markup found".into());
            }
            let mut options = markup_fmt::config::FormatOptions::default();
            options.layout.indent_width = 2;
            let mut result =
                markup_fmt::format_text(&masked, markup_fmt::Language::Html, &options, |code, _| {
                    Ok(code.into())
                })
                .map_err(|e| e.to_string())?;
            if input.contains("\r\n") {
                result = result.replace("\r\n", "\n").replace('\n', "\r\n");
            }
            if let Some((first, _)) = attributes.first() {
                let prefix = first.trim_end_matches(|c: char| c.is_ascii_digit());
                let pattern = regex::Regex::new(&format!(r#"["']?{}\d+["']?"#, regex::escape(prefix))).map_err(|e| e.to_string())?;
                let values: std::collections::HashMap<_,_> = attributes.iter().map(|(k,v)| (k.as_str(), v.as_str())).collect();
                let mut restored = 0;
                result = pattern.replace_all(&result, |captures: &regex::Captures<'_>| {
                    let marker = captures[0].trim_matches(['\'', '"']);
                    if let Some(original) = values.get(marker) { restored += 1; (*original).to_owned() }
                    else { captures[0].to_owned() }
                }).into_owned();
                if restored != attributes.len() { return Err("Unable to preserve a braced attribute; original unchanged".into()); }
            }
            Ok(result)
        }
    }
}

/// Real-world HTML often contains unquoted JSON/JSX-like attribute values.
/// Shield only balanced `={...}` in opening tags, then restore exact bytes.
/// Comments, text, script/style bodies and already quoted attributes are opaque.
fn mask_braced_attributes(input: &str) -> Result<(String, Vec<(String, String)>), String> {
    let bytes = input.as_bytes();
    let mut prefix = "litetext_braced_attribute_".to_owned();
    while input.contains(&prefix) { prefix.push('x'); }
    let mut replacements = Vec::new();
    let mut ranges = Vec::new();
    let mut at = 0;
    let mut raw: Option<String> = None;
    while at < bytes.len() {
        if let Some(name) = raw.take() {
            let close = format!("</{name}");
            let Some(next) = bytes[at..].windows(close.len()).position(|w| w.eq_ignore_ascii_case(close.as_bytes())) else { break; };
            at += next;
        }
        if bytes[at] != b'<' { at += 1; continue; }
        if input[at..].starts_with("<!--") {
            at = input[at + 4..].find("-->").map_or(input.len(), |n| at + 4 + n + 3);
            continue;
        }
        let name_start = at + 1;
        if !bytes.get(name_start).is_some_and(u8::is_ascii_alphabetic) { at += 1; continue; }
        let mut name_end = name_start;
        while bytes.get(name_end).is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b':' | b'-')) { name_end += 1; }
        let name = input[name_start..name_end].to_ascii_lowercase();
        at = name_end;
        while at < bytes.len() && bytes[at] != b'>' {
            if matches!(bytes[at], b'\'' | b'"') {
                let quote = bytes[at]; at += 1;
                while at < bytes.len() && bytes[at] != quote { at += 1; }
                at = (at + 1).min(bytes.len());
                continue;
            }
            if bytes[at] == b'=' {
                at += 1;
                while bytes.get(at).is_some_and(u8::is_ascii_whitespace) { at += 1; }
                if bytes.get(at) != Some(&b'{') { continue; }
                let start = at;
                let mut depth = 0usize;
                let mut quote = None;
                let mut escaped = false;
                while at < bytes.len() {
                    let b = bytes[at];
                    at += 1;
                    if escaped { escaped = false; continue; }
                    if let Some(q) = quote {
                        if b == b'\\' { escaped = true; } else if b == q { quote = None; }
                    } else if matches!(b, b'\'' | b'"' | b'`') { quote = Some(b); }
                    else if b == b'{' { depth += 1; }
                    else if b == b'}' { depth -= 1; if depth == 0 { break; } }
                }
                if depth != 0 || quote.is_some() { return Err("Unbalanced braced HTML attribute".into()); }
                let marker = format!("{prefix}{}", replacements.len());
                ranges.push((start..at, format!("\"{marker}\"")));
                replacements.push((marker, input[start..at].to_owned()));
            } else { at += 1; }
        }
        at = (at + 1).min(bytes.len());
        if matches!(name.as_str(), "script" | "style" | "textarea" | "pre") { raw = Some(name); }
    }
    let mut output = String::with_capacity(input.len());
    let mut last = 0;
    for (range, marker) in ranges {
        output.push_str(&input[last..range.start]);
        output.push_str(&marker);
        last = range.end;
    }
    output.push_str(&input[last..]);
    Ok((output, replacements))
}

fn check_html_depth(input: &str) -> Result<(), String> {
    static TAG: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let tag = TAG.get_or_init(|| {
        regex::Regex::new(
            r#"(?is)<!--.*?-->|<(/?)([a-z][a-z0-9:-]*)\b(?:[^>"']|"[^"]*"|'[^']*')*>"#,
        )
        .expect("HTML guard regex")
    });
    let mut depth = 0usize;
    for captures in tag.captures_iter(input) {
        let Some(name) = captures.get(2) else {
            continue;
        };
        if &captures[1] == "/" {
            depth = depth.saturating_sub(1);
        } else if !captures[0].ends_with("/>")
            && !matches!(
                name.as_str().to_ascii_lowercase().as_str(),
                "area"
                    | "base"
                    | "br"
                    | "col"
                    | "embed"
                    | "hr"
                    | "img"
                    | "input"
                    | "link"
                    | "meta"
                    | "param"
                    | "source"
                    | "track"
                    | "wbr"
            )
        {
            depth += 1;
            if depth > 128 {
                return Err("HTML formatting nesting limit is 128 levels".into());
            }
        }
    }
    Ok(())
}

impl crate::LiteTextApp {
    pub(crate) fn apply_web_tool(&mut self, ctx: &eframe::egui::Context, action: Action) {
        if self.shortcuts_blocked() {
            return;
        }
        let Some(file) = self.active() else {
            return;
        };
        if file.is_binary || file.view != crate::ViewMode::Text {
            return;
        }
        let Some(text) = file.draft_text.as_deref() else {
            return;
        };
        match convert(text, action) {
            Ok(result) => self.replace_tool_result(
                ctx,
                result,
                if action == Action::Html {
                    "HTML"
                } else {
                    "JSON"
                },
            ),
            Err(error) => {
                self.plugin_error = Some(format!(
                    "{}: {error}\n原内容未更改 / Original content unchanged",
                    action.label()
                ))
            }
        }
    }

    pub(crate) fn replace_tool_result(
        &mut self,
        ctx: &eframe::egui::Context,
        result: String,
        language: &str,
    ) {
        let Some(file) = self.active_mut() else {
            return;
        };
        let Some(text) = file.draft_text.as_deref() else {
            return;
        };
        if text != result {
            let len = text.len();
            crate::line_commands::commit_edits(file, ctx, vec![(0..len, result)], None);
        }
        file.language_override = Some(language.into());
        file.editor_state.folds.enabled = language == "JSON";
        file.editor_state.folds.invalidate();
        self.focus_editor = self.active_id;
        self.plugin_error = None;
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn value(result: Result<String, String>) -> Value {
        serde_json::from_str(&result.unwrap()).unwrap()
    }
    #[test]
    fn header_colons_pseudo_headers_duplicates_and_invalid_lines() {
        let v = value(headers(
            "GET / HTTP/1.1\r\nHost: example.com\r\n:method: GET\nX-Url: https://a:b/\nx-url: second\nEmpty:",
        ));
        assert_eq!(v["X-Url"], json!(["https://a:b/", "second"]));
        assert_eq!(v[":method"], "GET");
        assert_eq!(v["Empty"], "");
        for bad in ["", "broken", "bad name: x", "Good: yes\nbad"] {
            assert!(headers(bad).is_err());
        }
    }
    #[test]
    fn cookie_formats_preserve_values_and_duplicates() {
        assert_eq!(
            value(cookies("Set-Cookie: a=\"x;y\"; Path=/"))["a"],
            "\"x;y\""
        );
        assert_eq!(value(cookies("a.com\tTRUE\t/\tFALSE\t0\ta\t"))["a"], "");
        assert_eq!(
            value(cookies("Cookie: a=x=y; b=中文; a=2; empty=")),
            json!({"a":["x=y","2"],"b":"中文","empty":""})
        );
        assert_eq!(
            value(cookies(
                r#"[{"name":"a","value":"1","domain":"a"},{"name":"a","value":"2"}]"#
            )),
            json!({"a":["1","2"]})
        );
        assert_eq!(
            value(cookies(
                "Set-Cookie: sid=abc; Path=/; HttpOnly\nSet-Cookie: b=2; SameSite=Lax"
            )),
            json!({"sid":"abc","b":"2"})
        );
        assert_eq!(
            value(cookies(
                "# Netscape\n#HttpOnly_.a\tTRUE\t/\tTRUE\t0\ta\tx=y"
            )),
            json!({"a":"x=y"})
        );
        assert_eq!(value(cookies("a=\"x;y\"; b=2"))["a"], "\"x;y\"");
        for bad in ["oops", "a=1; broken", "[{\"name\":\"a\"}]", "a=\"oops"] {
            assert!(cookies(bad).is_err());
        }
    }
    #[test]
    fn url_parts_query_duplicates_plus_and_bad_encoding() {
        let v = value(parse_url(
            "https://example.com:8443/a?q=%E4%B8%AD&q=a+b&empty=&flag#part",
        ));
        assert_eq!(v["url"], "https://example.com:8443/a");
        assert_eq!(v["params"]["q"], json!(["中", "a b"]));
        assert_eq!(v["params"]["flag"], "");
        assert_eq!(v["fragment"], "part");
        for bad in [
            "",
            "relative/path",
            "javascript:alert(1)",
            "https://a/?x=%FF",
            "https://a/?x=%GG",
            "https://a/ x",
        ] {
            assert!(parse_url(bad).is_err());
        }
    }
    #[test]
    fn html_formats_markup_without_executing_or_rewriting_raw_content() {
        let input = "<!doctype html><html><body><div><p>中文 &amp; text</p><p>second</p></div><pre>  a\n b</pre><script>const x = '<tag>'; fetch('/no');</script></body></html>";
        let out = convert(input, Action::Html).unwrap();
        assert!(out.contains("\n"));
        assert!(out.contains("  a\n b"));
        assert!(out.contains("const x = '<tag>'; fetch('/no');"));
        assert_eq!(convert(&out, Action::Html).unwrap(), out);
        assert!(convert("<div title=\"oops>", Action::Html).is_err());
        assert!(convert(&"<div>".repeat(130), Action::Html).is_err());
    }

    #[test]
    fn html_accepts_unquoted_braced_attributes_and_switches_workspace_language() {
        let attr = r#"data-cost={"renderCost":"0.4","dataCost":1}"#;
        let source = format!(r#"<div {attr} data-extra={{"nested":{{"a":"中文 > b"}}}}><p>text</p></div>"#);
        let output = convert(&source, Action::Html).unwrap();
        assert!(output.contains(attr), "{output}");
        assert!(output.contains(r#"data-extra={"nested":{"a":"中文 > b"}}"#));
        assert_eq!(convert(&output, Action::Html).unwrap(), output);
        let opaque = r#"<script>const a = 'data-cost={"a":1}';</script><!-- data-cost={bad --><pre>data-cost={"a":1}</pre>"#;
        let (masked, attrs) = mask_braced_attributes(opaque).unwrap();
        assert!(attrs.is_empty());
        assert_eq!(masked, opaque);
        let multiline = "<div data-cost={\"a\":1,\n\"b\":2}>\r\ntext</div>\r\n";
        assert!(convert(multiline, Action::Html).unwrap().contains("{\"a\":1,\n\"b\":2}"), "attribute bytes must survive document newline conversion");
        let ctx = eframe::egui::Context::default();
        for (action, input, language) in [(Action::Html, source.as_str(), "HTML"), (Action::Header, "A: 1", "JSON"), (Action::Cookie, "a=1", "JSON")] {
            let mut app = crate::LiteTextApp::default();
            app.new_document();
            app.active_mut().unwrap().language_override = Some("Python".into());
            app.active_mut().unwrap().draft_text = Some(input.into());
            app.apply_web_tool(&ctx, action);
            assert!(app.plugin_error.is_none(), "{:?}", app.plugin_error);
            assert_eq!(crate::workspace_theme::effective_language(app.active().unwrap()), language);
            let output = ctx.run(eframe::egui::RawInput {
                screen_rect: Some(eframe::egui::Rect::from_min_size(eframe::egui::Pos2::ZERO, eframe::egui::vec2(1080.0, 720.0))),
                ..Default::default()
            }, |ctx| app.status_bar(ctx, &crate::DARK));
            assert!(output.shapes.iter().any(|s| matches!(&s.shape,
                eframe::egui::Shape::Text(text) if text.galley.text() == format!("{language}  ▴")
            )), "status bar must show {language}");
        }
        let many = format!("<html><body>{}</body></html>", "<div><span>x</span></div>".repeat(2200));
        assert!(convert(&many, Action::Html).is_ok());
    }

    #[test]
    fn all_tools_are_active_only_one_undo_and_never_save_originals() {
        use eframe::egui;
        for large in [false, true] {
            // Exercise each formatter plus every cURL target on both editor paths.
            let operations: Vec<_> = [Action::Html, Action::Header, Action::Cookie]
                .into_iter()
                .map(Ok)
                .chain(crate::curl_plugin::TARGETS.into_iter().map(Err))
                .collect();
            for operation in operations {
                let padding = "x".repeat(if large { 300_000 } else { 8 });
                let original = match operation {
                    Ok(Action::Html) => {
                        format!("<html><body><div>{padding}</div><p>中文</p></body></html>")
                    }
                    Ok(Action::Header) => format!("X-Test: {padding}\nHost: example.com"),
                    Ok(Action::Cookie) => format!("a={padding}; b=中文"),
                    Err(_) => format!(
                        "curl 'https://example.com/' -H 'X-Test: {padding}' --data-raw '中文'"
                    ),
                };
                let ctx = egui::Context::default();
                let temp = tempfile::tempdir().unwrap();
                let path = temp.path().join("input.txt");
                std::fs::write(&path, &original).unwrap();
                let mut app = crate::LiteTextApp::default();
                app.new_document();
                let other = app.active_id;
                app.active_mut().unwrap().draft_text = Some("other workspace".into());
                app.open_paths(vec![path.clone()]);
                let draw = |app: &mut crate::LiteTextApp, events| {
                    let _ = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(800.0, 600.0),
                            )),
                            events,
                            ..Default::default()
                        },
                        |ctx| {
                            egui::CentralPanel::default().show(ctx, |ui| {
                                crate::draft_editor(
                                    ui,
                                    app.active_mut().unwrap(),
                                    &crate::DARK,
                                    true,
                                    crate::Lang::En,
                                );
                            });
                        },
                    );
                };
                draw(&mut app, vec![]);
                draw(&mut app, vec![]);
                match operation {
                    Ok(action) => app.apply_web_tool(&ctx, action),
                    Err(target) => app.convert_active_curl(&ctx, target),
                }
                assert!(
                    app.plugin_error.is_none(),
                    "{operation:?}: {:?}",
                    app.plugin_error
                );
                assert_ne!(
                    app.active().unwrap().draft_text.as_deref(),
                    Some(original.as_str())
                );
                draw(&mut app, vec![]);
                draw(
                    &mut app,
                    vec![egui::Event::Key {
                        key: egui::Key::Z,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::COMMAND,
                    }],
                );
                assert_eq!(
                    app.active().unwrap().draft_text.as_deref(),
                    Some(original.as_str()),
                    "{operation:?} large={large}"
                );
                assert_eq!(std::fs::read_to_string(path).unwrap(), original);
                assert_eq!(
                    app.files
                        .iter()
                        .find(|f| Some(f.id) == other)
                        .unwrap()
                        .draft_text
                        .as_deref(),
                    Some("other workspace")
                );
            }
        }
    }

    #[test]
    fn failures_and_blocked_views_do_not_change_text_or_language() {
        let ctx = eframe::egui::Context::default();
        for action in [Action::Html, Action::Header, Action::Cookie] {
            let mut app = crate::LiteTextApp::default();
            app.new_document();
            app.active_mut().unwrap().draft_text = Some("invalid".into());
            app.apply_web_tool(&ctx, action);
            assert!(app.plugin_error.is_some());
            assert_eq!(app.active().unwrap().draft_text.as_deref(), Some("invalid"));
            assert!(app.active().unwrap().language_override.is_none());
            app.plugin_error = None;
            app.active_mut().unwrap().view = crate::ViewMode::Hex;
            app.apply_web_tool(&ctx, action);
            assert!(app.plugin_error.is_none());
        }
        let mut app = crate::LiteTextApp::default();
        app.new_document();
        app.active_mut().unwrap().draft_text = Some("curl https://a/ --unsupported".into());
        app.convert_active_curl(&ctx, crate::curl_plugin::Target::Python);
        assert!(app.plugin_error.is_some());
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some("curl https://a/ --unsupported")
        );
        assert!(app.active().unwrap().language_override.is_none());
    }

    #[test]
    fn plugin_entries_and_floating_curl_language_menu_are_clickable() {
        use eframe::egui;
        for lang in [crate::Lang::ZhCn, crate::Lang::En] {
            for entry in ["Format HTML", "Format Header", "Format Cookie", "curl"] {
                let ctx = egui::Context::default();
                crate::setup_fonts(&ctx);
                let mut app = crate::LiteTextApp {
                    lang,
                    show_menu: true,
                    ..Default::default()
                };
                app.new_document();
                app.active_mut().unwrap().draft_text = Some(
                    match entry {
                        "Format HTML" => "<div><p>text</p></div>",
                        "Format Header" => "Host: example.com",
                        "Format Cookie" => "a=1; b=2",
                        _ => "curl https://example.com/",
                    }
                    .into(),
                );
                let anchor =
                    egui::Rect::from_min_size(egui::pos2(1000.0, 20.0), egui::vec2(28.0, 24.0));
                let draw = |app: &mut crate::LiteTextApp, events| {
                    ctx.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(1080.0, 720.0),
                            )),
                            events,
                            ..Default::default()
                        },
                        |ctx| {
                            app.app_menu(ctx, anchor, &crate::DARK);
                            app.curl_target_menu(ctx, &crate::DARK);
                        },
                    )
                };
                let find = |output: &egui::FullOutput, label: &str| {
                    output
                        .shapes
                        .iter()
                        .find_map(|s| match &s.shape {
                            egui::Shape::Text(t) if t.galley.text() == label => {
                                Some(egui::Rect::from_min_size(t.pos, t.galley.size()))
                            }
                            _ => None,
                        })
                        .unwrap_or_else(|| panic!("missing {label}"))
                };
                draw(&mut app, vec![]);
                let out = draw(&mut app, vec![]);
                let row = find(&out, &format!("‹  {}", crate::tr(lang).plugins));
                for _ in 0..3 {
                    draw(&mut app, vec![egui::Event::PointerMoved(row.center())]);
                }
                let out = draw(&mut app, vec![]);
                for name in [
                    "Format JSON",
                    "Format HTML",
                    "Format Header",
                    "Format Cookie",
                ] {
                    find(&out, name);
                }
                let label = if entry == "curl" {
                    if lang == crate::Lang::ZhCn {
                        "curl解析  ›"
                    } else {
                        "Parse cURL  ›"
                    }
                } else {
                    entry
                };
                let row = find(&out, label);
                for pressed in [true, false] {
                    draw(
                        &mut app,
                        vec![
                            egui::Event::PointerMoved(row.center()),
                            egui::Event::PointerButton {
                                pos: row.center(),
                                button: egui::PointerButton::Primary,
                                pressed,
                                modifiers: Default::default(),
                            },
                        ],
                    );
                }
                assert!(!app.show_menu);
                if entry == "curl" {
                    assert!(app.show_curl_menu);
                    for _ in 0..2 {
                        draw(&mut app, vec![]);
                    }
                    let out = draw(&mut app, vec![]);
                    let labels: Vec<_> = crate::curl_plugin::TARGETS
                        .iter()
                        .map(|t| {
                            format!(
                                "{} {}",
                                if lang == crate::Lang::ZhCn {
                                    "转成"
                                } else {
                                    "Convert to"
                                },
                                t.name()
                            )
                        })
                        .collect();
                    for label in &labels {
                        find(&out, label);
                    }
                    let row = find(&out, &labels[0]);
                    assert!(row.top() >= 72.0);
                    for pressed in [true, false] {
                        draw(
                            &mut app,
                            vec![
                                egui::Event::PointerMoved(row.center()),
                                egui::Event::PointerButton {
                                    pos: row.center(),
                                    button: egui::PointerButton::Primary,
                                    pressed,
                                    modifiers: Default::default(),
                                },
                            ],
                        );
                    }
                    assert!(!app.show_curl_menu);
                    assert!(
                        app.active()
                            .unwrap()
                            .draft_text
                            .as_deref()
                            .unwrap()
                            .starts_with("import requests")
                    );
                }
                assert!(app.plugin_error.is_none());
                assert!(app.active().unwrap().language_override.is_some());
            }
        }
    }
}

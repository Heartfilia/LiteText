//! Native, offline cURL subset. Never invokes a shell, reads @files or sends HTTP.
//! Unknown flags fail closed instead of silently changing request semantics.
use crate::{Lang, LiteTextApp, Palette, ViewMode};
use base64::Engine;
use eframe::egui;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    Python,
    JavaScript,
    Node,
    Go,
    Rust,
    Php,
    Http,
}
pub(crate) const TARGETS: [Target; 7] = [
    Target::Python,
    Target::JavaScript,
    Target::Node,
    Target::Go,
    Target::Rust,
    Target::Php,
    Target::Http,
];
impl Target {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Python => "Python (requests)",
            Self::JavaScript => "JavaScript (fetch)",
            Self::Node => "Node.js (fetch)",
            Self::Go => "Go (net/http)",
            Self::Rust => "Rust (reqwest)",
            Self::Php => "PHP (cURL)",
            Self::Http => "HTTP",
        }
    }
    fn language(self) -> &'static str {
        match self {
            Self::Python => "Python",
            Self::JavaScript | Self::Node => "JavaScript",
            Self::Go => "Go",
            Self::Rust => "Rust",
            Self::Php => "PHP",
            Self::Http => "text",
        }
    }
}

#[derive(Debug)]
struct Request {
    url: url::Url,
    method: String,
    headers: Vec<(String, String)>,
    body: Option<String>,
    follow: bool,
}

fn header(input: &str) -> Result<(String, String), String> {
    let (key, value) = input.split_once(':').ok_or("Expected header name: value")?;
    if key.is_empty()
        || !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
        || value.contains(['\r', '\n'])
    {
        return Err("Invalid HTTP header".into());
    }
    Ok((key.into(), value.trim().into()))
}

fn words(input: &str) -> Result<Vec<String>, String> {
    if input.len() > 1024 * 1024 {
        return Err("cURL conversion limit is 1 MiB".into());
    }
    let mut quote = None;
    let mut normalized = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\0' {
            return Err("NUL is not valid in shell arguments".into());
        }
        if ch == '\\' && quote != Some('\'') {
            if let Some(next) = chars.next() {
                if next == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                } else if next != '\n' {
                    normalized.push(ch);
                    normalized.push(next);
                }
            } else {
                normalized.push(ch);
            }
            continue;
        }
        if quote == Some(ch) {
            quote = None;
            normalized.push(ch);
            continue;
        }
        if quote.is_none() && matches!(ch, '\'' | '"') {
            quote = Some(ch);
            normalized.push(ch);
            continue;
        }
        if quote != Some('\'') && matches!(ch, '$' | '`') {
            return Err("Shell expansions, variables and command substitution are not supported; paste literal values".into());
        }
        if quote.is_none() && matches!(ch, '|' | ';' | '&' | '<' | '>' | '^') {
            return Err(
                "Pipelines, redirects and shell operators are not supported; quote the URL/data"
                    .into(),
            );
        }
        if ch != '\r' || quote.is_some() {
            normalized.push(ch);
        }
    }
    shlex::split(&normalized).ok_or_else(|| "Unclosed shell quote or escape".into())
}

fn parse(input: &str) -> Result<Request, String> {
    let args = words(input)?;
    if !args.first().is_some_and(|s| s == "curl" || s == "curl.exe") {
        return Err("Expected one curl command (Bash/POSIX quoting)".into());
    }
    let mut url = None;
    let mut method = None;
    let mut head = false;
    let mut get = false;
    let mut follow = false;
    let mut headers = Vec::new();
    let mut data: Vec<String> = Vec::new();
    let mut raw_body = false;
    let mut at = 1;
    let mut positional = false;
    while at < args.len() {
        let arg = &args[at];
        at += 1;
        if arg == "--" {
            positional = true;
            continue;
        }
        if positional || !arg.starts_with('-') {
            if url.replace(arg.clone()).is_some() {
                return Err("Only one URL/request is supported".into());
            }
            continue;
        }
        let (flag, attached) = if arg.starts_with("--") {
            arg.split_once('=')
                .map_or((arg.as_str(), None), |(k, v)| (k, Some(v.to_owned())))
        } else if arg.len() > 2
            && matches!(
                arg.as_bytes()[1],
                b'X' | b'H' | b'd' | b'b' | b'u' | b'A' | b'e'
            )
        {
            (&arg[..2], Some(arg[2..].to_owned()))
        } else {
            (arg.as_str(), None)
        };
        let takes_value = matches!(
            flag,
            "-X" | "--request"
                | "-H"
                | "--header"
                | "-d"
                | "--data"
                | "--data-raw"
                | "--data-binary"
                | "--data-urlencode"
                | "--json"
                | "--url"
                | "-b"
                | "--cookie"
                | "-u"
                | "--user"
                | "-A"
                | "--user-agent"
                | "-e"
                | "--referer"
        );
        let value = if takes_value {
            let value = attached
                .or_else(|| {
                    let v = args.get(at).cloned();
                    at += 1;
                    v
                })
                .ok_or_else(|| format!("Missing value for {flag}"))?;
            if value.starts_with('@') && flag != "--data-raw" {
                return Err("@file/stdin input is not supported; paste the literal content".into());
            }
            value
        } else {
            if attached.is_some() {
                return Err(format!("Unexpected value for {flag}"));
            }
            String::new()
        };
        match flag {
            "-X" | "--request" => method = Some(value),
            "--url" => {
                if url.replace(value).is_some() {
                    return Err("Only one URL is supported".into());
                }
            }
            "-H" | "--header" => {
                let pair = header(&value)?;
                if pair.1.is_empty() {
                    return Err("cURL header suppression (-H 'Name:') is not supported".into());
                }
                headers.push(pair);
            }
            "-b" | "--cookie" => {
                if !value.contains('=') {
                    return Err("Cookie files are not supported".into());
                }
                headers.push(header(&format!("Cookie: {value}"))?);
            }
            "-u" | "--user" => {
                if !value.contains(':') {
                    return Err(
                        "Use explicit user:password; interactive passwords are unsupported".into(),
                    );
                }
                headers.push((
                    "Authorization".into(),
                    format!(
                        "Basic {}",
                        base64::engine::general_purpose::STANDARD.encode(value)
                    ),
                ));
            }
            "-A" | "--user-agent" => headers.push(header(&format!("User-Agent: {value}"))?),
            "-e" | "--referer" => {
                if value.ends_with(";auto") {
                    return Err("Automatic referer is not supported".into());
                }
                headers.push(header(&format!("Referer: {value}"))?);
            }
            "-d" | "--data" | "--data-raw" | "--data-binary" | "--data-urlencode" | "--json" => {
                if flag == "--json" {
                    if !headers
                        .iter()
                        .any(|(k, _)| k.eq_ignore_ascii_case("content-type"))
                    {
                        headers.push(("Content-Type".into(), "application/json".into()));
                    }
                    if !headers
                        .iter()
                        .any(|(k, _)| k.eq_ignore_ascii_case("accept"))
                    {
                        headers.push(("Accept".into(), "application/json".into()));
                    }
                }
                if matches!(flag, "--data-binary" | "--json") {
                    if !data.is_empty() {
                        return Err(
                            "Mixed/repeated binary or JSON body options are not supported".into(),
                        );
                    }
                    raw_body = true;
                } else if raw_body {
                    return Err("Mixed body option semantics are not supported".into());
                }
                let value = if flag == "--data-urlencode" {
                    let encode = |s: &str| {
                        url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>()
                    };
                    if value.contains('@') && !value.contains('=') {
                        return Err("data-urlencode file input is unsupported".into());
                    }
                    match value.split_once('=') {
                        Some(("", v)) => encode(v),
                        Some((k, v)) => format!("{k}={}", encode(v)),
                        None => encode(&value),
                    }
                } else {
                    value
                };
                data.push(value);
            }
            "-I" | "--head" => head = true,
            "-G" | "--get" => get = true,
            "-L" | "--location" => follow = true,
            // Output-only flags do not change the HTTP request.
            "-s" | "--silent" | "-S" | "--show-error" | "-sS" | "-v" | "--verbose" => {}
            _ => {
                return Err(format!(
                    "Unsupported cURL option: {flag}. Original text was not changed."
                ));
            }
        }
    }
    let source = url.ok_or("Missing URL")?;
    let source = if source.contains("://") {
        source
    } else {
        format!("http://{source}")
    };
    let mut url = url::Url::parse(&source).map_err(|e| e.to_string())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("Only HTTP(S) URLs are supported".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("URL credentials are unsupported; use -u user:password".into());
    }
    if source.contains(['{', '}', '[', ']']) && url.host_str().is_none_or(|h| !h.starts_with('[')) {
        return Err("URL globbing is unsupported".into());
    }
    let mut body = (!data.is_empty()).then(|| data.join("&"));
    let body_len = if get {
        0
    } else {
        body.as_ref().map_or(0, String::len)
    };
    if headers.iter().any(|(k, v)| {
        k.eq_ignore_ascii_case("content-length") && v.parse::<usize>().ok() != Some(body_len)
    }) {
        return Err("Content-Length does not match UTF-8 body length".into());
    }
    if get && let Some(body) = body.take() {
        let query = match url.query() {
            Some(q) if !q.is_empty() => format!("{q}&{body}"),
            _ => body,
        };
        url.set_query(Some(&query));
    }
    if head && body.is_some() {
        return Err("HEAD with request data is unsupported".into());
    }
    if headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("transfer-encoding"))
    {
        return Err("Explicit transfer framing is not supported".into());
    }
    if body.is_some()
        && !headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("content-type"))
    {
        headers.push((
            "Content-Type".into(),
            "application/x-www-form-urlencoded".into(),
        ));
    }
    let method = method.unwrap_or_else(|| {
        if head {
            "HEAD"
        } else if body.is_some() {
            "POST"
        } else {
            "GET"
        }
        .into()
    });
    if method.is_empty() || !method.bytes().all(|b| b.is_ascii_alphabetic() || b == b'-') {
        return Err("Invalid HTTP method".into());
    }
    url.set_fragment(None);
    Ok(Request {
        url,
        method,
        headers,
        body,
        follow,
    })
}

fn quoted(value: &str) -> String {
    serde_json::to_string(value).expect("string serialization")
}
fn python_string(value: &str) -> String {
    // JSON double-quoted strings are valid Python except lowercase booleans,
    // which cannot occur here, and JSON's \uXXXX works in Python strings.
    quoted(value)
}
fn php_string(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

pub(crate) fn convert(input: &str, target: Target) -> Result<String, String> {
    let r = parse(input)?;
    if matches!(target, Target::JavaScript | Target::Node)
        && r.body.is_some()
        && matches!(r.method.to_ascii_uppercase().as_str(), "GET" | "HEAD")
    {
        return Err("fetch cannot send a GET/HEAD body; choose another target".into());
    }
    if matches!(target, Target::Python | Target::JavaScript | Target::Node) {
        let mut seen = std::collections::HashSet::new();
        if r.headers
            .iter()
            .any(|(k, _)| !seen.insert(k.to_ascii_lowercase()))
        {
            return Err("This target cannot faithfully represent repeated headers; choose Go, Rust, PHP or HTTP".into());
        }
    }
    let u = quoted(r.url.as_str());
    let m = quoted(&r.method);
    let mut out = String::new();
    use std::fmt::Write;
    match target {
        Target::Python => {
            out.push_str("import requests\n\nheaders = {\n");
            for (k, v) in &r.headers {
                writeln!(out, "    {}: {},", python_string(k), python_string(v)).unwrap();
            }
            out.push_str("}\n\nresponse = requests.request(\n");
            writeln!(out, "    {m},\n    {u},\n    headers=headers,").unwrap();
            if let Some(body) = &r.body {
                writeln!(out, "    data={}.encode(\"utf-8\"),", python_string(body)).unwrap();
            }
            writeln!(
                out,
                "    allow_redirects={},\n    timeout=30,\n)\nprint(response.text)",
                if r.follow { "True" } else { "False" }
            )
            .unwrap();
        }
        Target::JavaScript | Target::Node => {
            out.push_str(if target == Target::Node {
                "// Node.js 18+ (built-in fetch)\n"
            } else {
                "// Browser fetch: CORS and forbidden-header rules apply.\n"
            });
            writeln!(
                out,
                "const response = await fetch({u}, {{\n  method: {m},\n  headers: {{"
            )
            .unwrap();
            for (k, v) in &r.headers {
                writeln!(out, "    {}: {},", quoted(k), quoted(v)).unwrap();
            }
            out.push_str("  },\n");
            if let Some(body) = &r.body {
                writeln!(out, "  body: {},", quoted(body)).unwrap();
            }
            writeln!(
                out,
                "  redirect: \"{}\",\n}});\nconsole.log(await response.text());",
                if r.follow { "follow" } else { "manual" }
            )
            .unwrap();
        }
        Target::Go => {
            out.push_str("package main\n\nimport (\n  \"fmt\"\n  \"io\"\n  \"net/http\"\n  \"strings\"\n  \"time\"\n)\n\nfunc main() {\n");
            writeln!(out, "  req, err := http.NewRequest({m}, {u}, strings.NewReader({}))\n  if err != nil {{ panic(err) }}", quoted(r.body.as_deref().unwrap_or(""))).unwrap();
            for (k, v) in &r.headers {
                if k.eq_ignore_ascii_case("host") {
                    writeln!(out, "  req.Host = {}", quoted(v)).unwrap();
                } else if k.eq_ignore_ascii_case("content-length") {
                    writeln!(
                        out,
                        "  req.ContentLength = {}",
                        v.parse::<u64>().map_err(|_| "Invalid Content-Length")?
                    )
                    .unwrap();
                } else {
                    writeln!(out, "  req.Header.Add({}, {})", quoted(k), quoted(v)).unwrap();
                }
            }
            out.push_str("  client := &http.Client{Timeout: 30 * time.Second");
            if !r.follow {
                out.push_str(", CheckRedirect: func(req *http.Request, via []*http.Request) error { return http.ErrUseLastResponse }");
            }
            out.push_str("}\n  resp, err := client.Do(req)\n  if err != nil { panic(err) }\n  defer resp.Body.Close()\n  body, err := io.ReadAll(resp.Body)\n  if err != nil { panic(err) }\n  fmt.Println(string(body))\n}\n");
        }
        Target::Rust => {
            // JSON \u escapes are not Rust escapes. Debug string formatting emits
            // Rust literals, including escaped control characters and Unicode.
            out.push_str("// Cargo.toml: reqwest = { version = \"0.12\", features = [\"blocking\"] }\nfn main() -> Result<(), Box<dyn std::error::Error>> {\n");
            writeln!(out, "    let client = reqwest::blocking::Client::builder()\n        .redirect(reqwest::redirect::Policy::{}())\n        .timeout(std::time::Duration::from_secs(30))\n        .build()?;\n    let mut headers = reqwest::header::HeaderMap::new();", if r.follow { "default" } else { "none" }).unwrap();
            for (k, v) in &r.headers {
                writeln!(out, "    headers.append(reqwest::header::HeaderName::from_bytes({:?}.as_bytes())?, reqwest::header::HeaderValue::from_str({v:?})?);", k).unwrap();
            }
            writeln!(out, "    let response = client.request(reqwest::Method::from_bytes({:?}.as_bytes())?, {:?})\n        .headers(headers)", r.method, r.url.as_str()).unwrap();
            if let Some(body) = &r.body {
                writeln!(out, "        .body({body:?})").unwrap();
            }
            out.push_str(
                "        .send()?;\n    println!(\"{}\", response.text()?);\n    Ok(())\n}\n",
            );
        }
        Target::Php => {
            writeln!(out, "<?php\n$ch = curl_init({});\ncurl_setopt_array($ch, [\n    CURLOPT_CUSTOMREQUEST => {},\n    CURLOPT_RETURNTRANSFER => true,\n    CURLOPT_FOLLOWLOCATION => {},\n    CURLOPT_TIMEOUT => 30,\n    CURLOPT_HTTPHEADER => [", php_string(r.url.as_str()), php_string(&r.method), r.follow).unwrap();
            for (k, v) in &r.headers {
                writeln!(out, "        {},", php_string(&format!("{k}: {v}"))).unwrap();
            }
            out.push_str("    ],\n");
            if r.method == "HEAD" {
                out.push_str("    CURLOPT_NOBODY => true,\n");
            }
            if let Some(body) = &r.body {
                writeln!(out, "    CURLOPT_POSTFIELDS => {},", php_string(body)).unwrap();
            }
            out.push_str("]);\n$response = curl_exec($ch);\nif ($response === false) { throw new RuntimeException(curl_error($ch)); }\necho $response;\ncurl_close($ch);\n");
        }
        Target::Http => {
            let path = format!(
                "{}{}",
                r.url.path(),
                r.url.query().map_or(String::new(), |q| format!("?{q}"))
            );
            write!(out, "{} {} HTTP/1.1\r\n", r.method, path).unwrap();
            if !r
                .headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("host"))
            {
                writeln!(
                    out,
                    "Host: {}{}\r",
                    r.url.host_str().unwrap(),
                    r.url.port().map_or(String::new(), |p| format!(":{p}"))
                )
                .unwrap();
            }
            for (k, v) in &r.headers {
                write!(out, "{k}: {v}\r\n").unwrap();
            }
            if let Some(body) = &r.body
                && !r
                    .headers
                    .iter()
                    .any(|(k, _)| k.eq_ignore_ascii_case("content-length"))
            {
                write!(out, "Content-Length: {}\r\n", body.len()).unwrap();
            }
            out.push_str("\r\n");
            if let Some(body) = &r.body {
                out.push_str(body);
            }
        }
    }
    Ok(out)
}

impl LiteTextApp {
    pub(crate) fn convert_active_curl(&mut self, ctx: &egui::Context, target: Target) {
        if self.shortcuts_blocked() {
            return;
        }
        let Some(file) = self.active() else {
            return;
        };
        if file.is_binary || file.view != ViewMode::Text {
            return;
        }
        let Some(text) = file.draft_text.as_deref() else {
            return;
        };
        match convert(text, target) {
            Ok(out) => self.replace_tool_result(ctx, out, target.language()),
            Err(error) => self.plugin_error = Some(format!("cURL → {}: {error}", target.name())),
        }
    }

    pub(crate) fn curl_target_menu(&mut self, ctx: &egui::Context, pal: &Palette) {
        if !self.show_curl_menu {
            return;
        }
        if self.shortcuts_blocked() {
            self.show_curl_menu = false;
            return;
        }
        let zh = self.lang == Lang::ZhCn;
        let mut open = true;
        let mut picked = None;
        egui::Window::new(if zh { "curl解析" } else { "Parse cURL" })
            .id(egui::Id::new("curl_target_menu"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 72.0))
            .fixed_size(egui::vec2((ctx.screen_rect().width() - 64.0).clamp(280.0, 500.0),
                (ctx.screen_rect().height() - 150.0).clamp(140.0, 430.0)))
            .frame(egui::Frame::new().fill(pal.bg_panel).corner_radius(10.0)
                .stroke(egui::Stroke::new(1.0_f32, pal.bg_elevated)).inner_margin(egui::Margin::same(16)))
            .show(ctx, |ui| {
                crate::settings_control_style(ui, pal);
                ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);
                let scroll = egui::ScrollArea::vertical().max_height(ui.available_height()).auto_shrink([false, false]).show(ui, |ui| {
                ui.label(egui::RichText::new(if zh { "将 cURL 转换为代码" } else { "Turn cURL into code" })
                    .size(18.0).strong().color(pal.text_main));
                ui.add(egui::Label::new(egui::RichText::new(if zh {
                    "选择目标语言，将替换当前工作区内容，可撤销。"
                } else {
                    "Choose a target to replace the active workspace. Undo is supported."
                }).size(12.0).color(pal.text_faint)).wrap());
                ui.add_space(2.0);
                let enabled = self.active().is_some_and(|f| {
                    !f.is_binary && f.view == ViewMode::Text && f.draft_text.is_some()
                });
                ui.add_enabled_ui(enabled, |ui| {
                    let columns = if ui.available_width() >= 400.0 { 2 } else { 1 };
                    for targets in TARGETS.chunks(columns) {
                        ui.columns(columns, |uis| {
                            for (target, ui) in targets.iter().zip(uis) {
                                let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 62.0), egui::Sense::click());
                                response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), target.name()));
                                let hovered = response.hovered() && ui.is_enabled();
                                let stroke = if hovered { pal.accent } else { pal.text_faint.gamma_multiply(0.25) };
                                ui.painter().rect(rect, 8.0, if hovered { pal.bg_elevated } else { pal.bg_window },
                                    egui::Stroke::new(1.0_f32, stroke), egui::StrokeKind::Inside);
                                let label = format!("{} {}", if zh { "转成" } else { "Convert to" }, target.name());
                                ui.painter().text(rect.left_top() + egui::vec2(12.0, 18.0), egui::Align2::LEFT_CENTER,
                                    label, egui::FontId::proportional(13.0), if hovered { pal.accent } else { pal.text_main });
                                let detail = match target {
                                    Target::Python => "requests · Python 3",
                                    Target::JavaScript => "Fetch API · Browser",
                                    Target::Node => "Native fetch · Node.js 18+",
                                    Target::Go => "Standard library · net/http",
                                    Target::Rust => "Blocking client · reqwest",
                                    Target::Php => "cURL extension · PHP",
                                    Target::Http => "Raw request · HTTP/1.1",
                                };
                                ui.painter().text(rect.left_top() + egui::vec2(12.0, 43.0), egui::Align2::LEFT_CENTER,
                                    detail, egui::FontId::proportional(11.0), pal.text_faint);
                                ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new(("curl_target", target.name())), rect));
                                if response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() { picked = Some(*target); }
                            }
                        });
                    }
                });
                ui.add_space(2.0);
                egui::Frame::new().fill(pal.bg_inset).corner_radius(7.0).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                ui.add(egui::Label::new(egui::RichText::new(if zh {
                    "离线转换 · 不执行命令，不发送请求\n暂不支持文件、multipart 和 shell 表达式。"
                } else {
                    "Offline · No commands or network requests\nFiles, multipart and shell expressions are not supported."
                }).size(11.0).color(pal.text_faint)).wrap());
                });
                if !enabled {
                    ui.label(egui::RichText::new(if zh { "请先打开一个文本工作区。" } else { "Open a text workspace first." }).color(pal.accent));
                }
                });
                ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("curl_scroll_rect"), scroll.inner_rect));
            });
        if !open || ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.show_curl_menu = false;
        }
        if let Some(target) = picked {
            self.show_curl_menu = false;
            self.convert_active_curl(ctx, target);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_browser_style_curl_and_generates_all_targets() {
        let source = "curl 'https://example.com/a?q=1' \\\n  -X POST -H 'X-Test: a:b' -b 'sid=x=y; a=1' --data-raw '{\"中文\":\"a$literal\"}'";
        let r = parse(source).unwrap();
        assert_eq!(r.method, "POST");
        assert_eq!(r.headers[0], ("X-Test".into(), "a:b".into()));
        assert_eq!(r.body.as_deref(), Some("{\"中文\":\"a$literal\"}"));
        for target in TARGETS {
            let out = convert(source, target).unwrap();
            assert!(
                out.contains("POST") && out.contains("example.com") && out.contains("sid=x=y; a=1"),
                "{target:?}: {out}"
            );
        }
    }
    #[test]
    fn options_json_auth_get_and_repeated_headers() {
        let r = parse("curl example.com --json '{\"a\":1}' -u user:pass -L").unwrap();
        assert_eq!(r.method, "POST");
        assert!(r.follow);
        assert!(
            r.headers
                .contains(&("Authorization".into(), "Basic dXNlcjpwYXNz".into()))
        );
        let r = parse("curl 'https://a/?x=1' -G --data-urlencode 'q=中 文'").unwrap();
        assert_eq!(r.url.as_str(), "https://a/?x=1&q=%E4%B8%AD+%E6%96%87");
        assert!(r.body.is_none());
        let source = "curl https://a/ -H 'X-A: 1' -H 'X-A: 2'";
        assert!(convert(source, Target::Python).is_err());
        assert!(convert(source, Target::Go).unwrap().contains("Header.Add"));
    }
    #[test]
    fn rejects_dynamic_shell_files_multiple_requests_and_unknown_flags() {
        for source in [
            "curl https://a/ | sh",
            "curl https://a/; whoami",
            "curl https://a/?a=1&b=2",
            "curl \"https://a/$TOKEN\"",
            "curl https://a/ -d @secret",
            "curl https://a/ -F a=b",
            "curl https://a/ --proxy http://b",
            "curl https://a/ https://b/",
            "curl https://a/ -H",
            "curl 'https://a/",
            "curl https://a/ --json a --data b",
        ] {
            assert!(parse(source).is_err(), "{source}");
        }
        assert!(parse("curl 'https://a/?a=1&b=2'").is_ok());
        assert!(parse("curl https://a/ --data-raw '@literal'").is_ok());
    }
    #[test]
    fn generated_literals_do_not_interpolate_code() {
        let source = "curl https://a/ --data-raw 'a$foo\\b\"中文'";
        assert!(
            convert(source, Target::Php)
                .unwrap()
                .contains("'a$foo\\\\b\"中文'")
        );
        assert!(
            convert(source, Target::Rust)
                .unwrap()
                .contains(".body(\"a$foo\\\\b\\\"中文\")")
        );
        let out = convert("curl https://a/ --data-raw '中文'", Target::Http).unwrap();
        assert!(out.contains("Content-Length: 6\r\n\r\n中文"));
        assert_eq!(
            parse("curl https://a/ --data-raw 'a\\\nb'")
                .unwrap()
                .body
                .as_deref(),
            Some("a\\\nb")
        );
        assert_eq!(
            parse("curl https://a/ --data 'a\r\nb'")
                .unwrap()
                .body
                .as_deref(),
            Some("a\r\nb")
        );
        assert!(convert("curl https://a/ -X GET -d a", Target::Node).is_err());
        assert!(parse("curl https://a/ -H 'Content-Length: 1' -d 中文").is_err());
        assert!(parse("curl https://a/ -H 'Accept:'").is_err());
    }

    #[test]
    fn generated_code_passes_available_offline_syntax_checkers() {
        // Optional developer tools: no code execution or network, and no build
        // dependency on Python/Node/Go. CI may only have some of these installed.
        let temp = tempfile::tempdir().unwrap();
        let source =
            "curl https://example.com/ -X POST -H 'X-Value: a:b' --data-raw '中文 $x \\\n\"text\"'";
        for (target, filename, program, flags) in [
            (
                Target::Python,
                "request.py",
                "python3",
                vec![
                    "-c",
                    "import ast,sys; ast.parse(open(sys.argv[1], encoding='utf-8').read())",
                ],
            ),
            (Target::Node, "request.mjs", "node", vec!["--check"]),
            (Target::Go, "request.go", "gofmt", vec!["-e"]),
        ] {
            let file = temp.path().join(filename);
            std::fs::write(&file, convert(source, target).unwrap()).unwrap();
            let output = std::process::Command::new(program)
                .args(flags)
                .arg(file)
                .output();
            match output {
                Ok(output) => assert!(
                    output.status.success(),
                    "{target:?}: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => panic!("{program}: {e}"),
            }
        }
    }

    #[test]
    fn target_cards_fit_themes_languages_and_small_windows() {
        for pal in [&crate::DARK, &crate::LIGHT] {
            for lang in [Lang::ZhCn, Lang::En] {
                for size in [egui::vec2(640.0, 420.0), egui::vec2(1080.0, 720.0), egui::vec2(400.0, 500.0)] {
                    let ctx = egui::Context::default();
                    crate::setup_fonts(&ctx);
                    crate::apply_style(&ctx, pal);
                    let mut app = LiteTextApp { lang, show_curl_menu: true, ..Default::default() };
                    app.new_document();
                    app.active_mut().unwrap().draft_text = Some("curl https://example.com/".into());
                    let raw = || egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)), ..Default::default() };
                    let mut output = None;
                    for _ in 0..5 { output = Some(ctx.run(raw(), |ctx| app.curl_target_menu(ctx, pal))); }
                    let view = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new("curl_scroll_rect"))).unwrap();
                    assert!(view.left() >= 0.0 && view.right() <= size.x);
                    assert!(view.top() >= 0.0 && view.bottom() <= size.y);
                    for target in TARGETS {
                        let card = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("curl_target", target.name())))).unwrap();
                        assert!(card.left() >= view.left() - 1.0 && card.right() <= view.right() + 1.0);
                        let label = format!("{} {}", if lang == Lang::ZhCn { "转成" } else { "Convert to" }, target.name());
                        // Fully visible labels must fit their own cards.
                        if view.contains_rect(card) {
                            let text = output.as_ref().unwrap().shapes.iter().find_map(|s| match &s.shape {
                                egui::Shape::Text(t) if t.galley.text() == label => Some(egui::Rect::from_min_size(t.pos, t.galley.size())),
                                _ => None,
                            }).unwrap();
                            assert!(card.contains_rect(text), "{label}: {text:?} outside {card:?}");
                        }
                    }
                    for _ in 0..12 {
                        let mut input = raw();
                        input.events = vec![egui::Event::PointerMoved(view.center()), egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, -1000.0), modifiers: Default::default(),
                        }];
                        let _ = ctx.run(input, |ctx| app.curl_target_menu(ctx, pal));
                    }
                    let http = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("curl_target", Target::Http.name())))).unwrap();
                    assert!(view.contains_rect(http), "last target must be reachable: {http:?}, {view:?}");
                }
            }
        }
    }

    #[test]
    fn floating_menu_converts_the_current_tab_and_escape_does_not_edit() {
        let ctx = egui::Context::default();
        let mut app = LiteTextApp::default();
        app.new_document();
        let first = app.active_id;
        app.active_mut().unwrap().draft_text = Some("curl https://first.example/".into());
        app.show_curl_menu = true;
        app.new_document();
        app.active_mut().unwrap().draft_text = Some("curl https://second.example/".into());
        app.show_curl_menu = false;
        app.convert_active_curl(&ctx, Target::Python);
        assert!(
            app.active()
                .unwrap()
                .draft_text
                .as_deref()
                .unwrap()
                .contains("second.example")
        );
        assert_eq!(
            app.files
                .iter()
                .find(|f| Some(f.id) == first)
                .unwrap()
                .draft_text
                .as_deref(),
            Some("curl https://first.example/")
        );
        let before = app.active().unwrap().draft_text.clone();
        app.show_curl_menu = true;
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                }],
                ..Default::default()
            },
            |ctx| app.curl_target_menu(ctx, &crate::DARK),
        );
        assert!(!app.show_curl_menu);
        assert_eq!(app.active().unwrap().draft_text, before);
    }
}

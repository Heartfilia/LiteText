//! Whole-document UTF-8 conversions: active workspace only, one undo entry.
use base64::{Engine, engine::general_purpose::STANDARD};
use sha2::Digest;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Md5,
    Base64Encode,
    Base64Decode,
    UrlEncode,
    UrlDecode,
}
pub(crate) const ACTIONS: [Action; 5] = [
    Action::Md5,
    Action::Base64Encode,
    Action::Base64Decode,
    Action::UrlEncode,
    Action::UrlDecode,
];

impl Action {
    pub(crate) fn label(self, zh: bool) -> &'static str {
        match (self, zh) {
            (Self::Md5, true) => "文本转MD5",
            (Self::Base64Encode, true) => "文本转Base64",
            (Self::Base64Decode, true) => "Base64转文本",
            (Self::Md5, false) => "Text to MD5",
            (Self::Base64Encode, false) => "Text to Base64",
            (Self::Base64Decode, false) => "Base64 to text",
            (Self::UrlEncode, _) => "urlencode",
            (Self::UrlDecode, _) => "urldecode",
        }
    }
}

fn convert(text: &str, action: Action) -> Result<String, String> {
    match action {
        Action::Md5 => Ok(format!("{:x}", md5::Md5::digest(text.as_bytes()))),
        Action::Base64Encode => Ok(STANDARD.encode(text.as_bytes())),
        Action::Base64Decode => {
            let compact: Vec<_> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
            let bytes = STANDARD
                .decode(compact)
                .map_err(|e| format!("Invalid Base64: {e}"))?;
            String::from_utf8(bytes).map_err(|e| format!("Decoded data is not UTF-8 text: {e}"))
        }
        Action::UrlEncode => {
            // RFC 3986 component encoding, like encodeURIComponent: spaces are
            // %20, literal '+' is %2B. No form-style '+' / space ambiguity.
            let mut result = String::new();
            use std::fmt::Write;
            for b in text.bytes() {
                if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
                    result.push(b as char);
                } else {
                    write!(&mut result, "%{b:02X}").expect("writing to String");
                }
            }
            Ok(result)
        }
        Action::UrlDecode => {
            let mut result = Vec::with_capacity(text.len());
            let mut input = text.bytes();
            while let Some(b) = input.next() {
                if b == b'%' {
                    let hex = |b: u8| (b as char).to_digit(16).map(|n| n as u8);
                    let a = input
                        .next()
                        .and_then(hex)
                        .ok_or("Invalid URL percent escape")?;
                    let b = input
                        .next()
                        .and_then(hex)
                        .ok_or("Invalid URL percent escape")?;
                    result.push(a * 16 + b);
                } else {
                    result.push(b);
                }
            }
            String::from_utf8(result).map_err(|e| format!("Decoded data is not UTF-8 text: {e}"))
        }
    }
}

impl crate::LiteTextApp {
    pub(crate) fn convert_active_text(&mut self, ctx: &eframe::egui::Context, action: Action) {
        if self.shortcuts_blocked() {
            return;
        }
        let Some(file) = self.active_mut() else {
            return;
        };
        if file.is_binary || file.view != crate::ViewMode::Text {
            return;
        }
        let Some(text) = file.draft_text.as_deref() else {
            return;
        };
        match convert(text, action) {
            Ok(result) if result != text => {
                let len = text.len();
                crate::line_commands::commit_edits(file, ctx, vec![(0..len, result)], None);
                ctx.request_repaint();
            }
            Ok(_) => {}
            Err(error) => {
                self.plugin_error = Some(format!(
                    "{}: {error}",
                    action.label(self.lang == crate::Lang::ZhCn)
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions_are_active_only_undoable_and_never_write_the_original() {
        for large in [false, true] {
            for action in ACTIONS {
                let ctx = eframe::egui::Context::default();
                let temp = tempfile::tempdir().unwrap();
                let path = temp.path().join("original.txt");
                let source = "中文🙂 + /?=\r\n".repeat(if large { 20000 } else { 1 });
                let original = match action {
                    Action::Base64Decode => convert(&source, Action::Base64Encode).unwrap(),
                    Action::UrlDecode => convert(&source, Action::UrlEncode).unwrap(),
                    _ => source,
                };
                std::fs::write(&path, &original).unwrap();
                let mut app = crate::LiteTextApp::default();
                app.new_document();
                let other = app.active_id;
                app.active_mut().unwrap().draft_text = Some("keep other workspace".into());
                app.open_paths(vec![path.clone()]);
                let draw = |app: &mut crate::LiteTextApp, events| {
                    let _ = ctx.run(
                        eframe::egui::RawInput {
                            screen_rect: Some(eframe::egui::Rect::from_min_size(
                                eframe::egui::Pos2::ZERO,
                                eframe::egui::vec2(800.0, 600.0),
                            )),
                            events,
                            ..Default::default()
                        },
                        |ctx| {
                            eframe::egui::CentralPanel::default().show(ctx, |ui| {
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
                app.convert_active_text(&ctx, action);
                assert_eq!(
                    app.active().unwrap().draft_text.as_deref(),
                    Some(convert(&original, action).unwrap().as_str())
                );
                draw(&mut app, vec![]);
                draw(
                    &mut app,
                    vec![eframe::egui::Event::Key {
                        key: eframe::egui::Key::Z,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: eframe::egui::Modifiers::COMMAND,
                    }],
                );
                assert_eq!(
                    app.active().unwrap().draft_text.as_deref(),
                    Some(original.as_str())
                );
                assert_eq!(std::fs::read_to_string(path).unwrap(), original);
                assert_eq!(
                    app.files
                        .iter()
                        .find(|f| Some(f.id) == other)
                        .unwrap()
                        .draft_text
                        .as_deref(),
                    Some("keep other workspace")
                );
            }
        }
    }

    #[test]
    fn decoding_errors_and_hex_leave_current_text_unchanged() {
        let ctx = eframe::egui::Context::default();
        let mut app = crate::LiteTextApp::default();
        app.new_document();
        app.active_mut().unwrap().draft_text = Some("%GG".into());
        app.convert_active_text(&ctx, Action::UrlDecode);
        assert!(app.plugin_error.is_some());
        assert_eq!(app.active().unwrap().draft_text.as_deref(), Some("%GG"));
        app.plugin_error = None;
        app.active_mut().unwrap().view = crate::ViewMode::Hex;
        app.convert_active_text(&ctx, Action::Md5);
        assert_eq!(app.active().unwrap().draft_text.as_deref(), Some("%GG"));
    }

    #[test]
    fn menu_conversion_is_below_whitespace_and_click_applies_to_active_text() {
        use eframe::egui;
        for lang in [crate::Lang::ZhCn, crate::Lang::En] {
            let ctx = egui::Context::default();
            crate::setup_fonts(&ctx);
            let mut app = crate::LiteTextApp {
                lang,
                show_menu: true,
                ..Default::default()
            };
            app.new_document();
            app.active_mut().unwrap().draft_text = Some("abc".into());
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
                    |ctx| app.app_menu(ctx, anchor, &crate::DARK),
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
                    .unwrap()
            };
            draw(&mut app, vec![]);
            let output = draw(&mut app, vec![]);
            let entry = find(
                &output,
                if lang == crate::Lang::ZhCn {
                    "‹  文本转换"
                } else {
                    "‹  Text conversion"
                },
            );
            let whitespace = find(
                &output,
                if lang == crate::Lang::ZhCn {
                    "‹  空白符处理"
                } else {
                    "‹  Whitespace"
                },
            );
            assert!(entry.top() > whitespace.bottom());
            for _ in 0..3 {
                draw(&mut app, vec![egui::Event::PointerMoved(entry.center())]);
            }
            assert!(app.menu_conversion_open && !app.menu_whitespace_open);
            let output = draw(&mut app, vec![]);
            for action in ACTIONS {
                find(&output, action.label(lang == crate::Lang::ZhCn));
            }
            let md5 = find(&output, Action::Md5.label(lang == crate::Lang::ZhCn));
            for pressed in [true, false] {
                draw(
                    &mut app,
                    vec![
                        egui::Event::PointerMoved(md5.center()),
                        egui::Event::PointerButton {
                            pos: md5.center(),
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: Default::default(),
                        },
                    ],
                );
            }
            assert!(!app.show_menu);
            assert_eq!(
                app.active().unwrap().draft_text.as_deref(),
                Some("900150983cd24fb0d6963f7d28e17f72")
            );
        }
    }
    #[test]
    fn vectors_unicode_roundtrips_and_invalid_input() {
        assert_eq!(
            convert("", Action::Md5).unwrap(),
            "d41d8cd98f00b204e9800998ecf8427e"
        );
        assert_eq!(
            convert("abc", Action::Md5).unwrap(),
            "900150983cd24fb0d6963f7d28e17f72"
        );
        assert_eq!(convert("abc", Action::Base64Encode).unwrap(), "YWJj");
        let original = "中文🙂 + /?=%\r\n";
        for (encode, decode) in [
            (Action::Base64Encode, Action::Base64Decode),
            (Action::UrlEncode, Action::UrlDecode),
        ] {
            assert_eq!(
                convert(&convert(original, encode).unwrap(), decode).unwrap(),
                original
            );
        }
        assert_eq!(convert("a+b%20c", Action::UrlDecode).unwrap(), "a+b c");
        for bad in ["%", "%A", "%GG", "%FF"] {
            assert!(convert(bad, Action::UrlDecode).is_err());
        }
        for bad in ["*", "YWJ", "/w=="] {
            assert!(convert(bad, Action::Base64Decode).is_err());
        }
    }
}

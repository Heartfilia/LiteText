//! Local terminal discovery and argument-safe launching. No shell command field.
use crate::{Lang, LiteTextApp, LoadedFile, Palette};
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Settings {
    pub custom: bool,
    pub program: Option<PathBuf>,
    pub custom_path: String,
}

#[derive(Clone, Debug)]
pub(crate) struct Installed {
    name: String,
    path: PathBuf,
}

fn home() -> std::io::Result<PathBuf> {
    #[cfg(windows)]
    let value = std::env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let value = std::env::var_os("HOME");
    value
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .ok_or_else(|| std::io::Error::other("User home directory unavailable"))
}

pub(crate) fn directory(file: &LoadedFile) -> std::io::Result<PathBuf> {
    // Detached copies and scratch buffers are unsaved, even with a source path.
    if file.path.as_os_str().is_empty() {
        return home();
    }
    let path = std::path::absolute(&file.path)?;
    path.parent()
        .filter(|p| p.is_dir())
        .map(Path::to_owned)
        .ok_or_else(|| std::io::Error::other("The file's parent directory no longer exists"))
}

fn in_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|p| p.join(name))
        .find(|p| executable(p))
}

fn executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

pub(crate) fn discover() -> Vec<Installed> {
    let mut found = Vec::new();
    #[cfg(target_os = "macos")]
    {
        let mut roots = vec![
            PathBuf::from("/Applications"),
            PathBuf::from("/System/Applications/Utilities"),
            PathBuf::from("/Applications/Utilities"),
        ];
        if let Ok(home) = home() {
            roots.push(home.join("Applications"));
        }
        for (name, bundle) in [
            ("Terminal", "Terminal.app"),
            ("iTerm2", "iTerm.app"),
            ("Ghostty", "Ghostty.app"),
            ("WezTerm", "WezTerm.app"),
            ("kitty", "kitty.app"),
            ("Alacritty", "Alacritty.app"),
            ("Warp", "Warp.app"),
            ("Hyper", "Hyper.app"),
        ] {
            if let Some(path) = roots.iter().map(|r| r.join(bundle)).find(|p| p.is_dir()) {
                found.push(Installed {
                    name: name.to_owned(),
                    path,
                });
            }
        }
    }
    #[cfg(windows)]
    let programs = [
        ("Windows Terminal", "wt.exe"),
        ("PowerShell 7", "pwsh.exe"),
        ("Windows PowerShell", "powershell.exe"),
        ("Command Prompt", "cmd.exe"),
        ("WezTerm", "wezterm.exe"),
        ("Alacritty", "alacritty.exe"),
    ];
    #[cfg(not(any(windows, target_os = "macos")))]
    let programs = [
        ("System terminal", "x-terminal-emulator"),
        ("GNOME Terminal", "gnome-terminal"),
        ("Konsole", "konsole"),
        ("Xfce Terminal", "xfce4-terminal"),
        ("kitty", "kitty"),
        ("Alacritty", "alacritty"),
        ("WezTerm", "wezterm"),
        ("Ghostty", "ghostty"),
        ("xterm", "xterm"),
    ];
    #[cfg(not(target_os = "macos"))]
    for (name, command) in programs {
        if let Some(path) = in_path(command) {
            found.push(Installed {
                name: name.to_owned(),
                path,
            });
        }
    }
    found
}

#[derive(Debug)]
struct Launch {
    program: PathBuf,
    args: Vec<OsString>,
    directory: PathBuf,
}

fn plan(settings: &Settings, directory: PathBuf) -> std::io::Result<Launch> {
    let program = if settings.custom {
        let path = settings.custom_path.trim();
        if path.is_empty() {
            return Err(std::io::Error::other(
                "Choose a terminal application or executable first",
            ));
        }
        let path = if let Some(rest) = path.strip_prefix("~/") {
            home()?.join(rest)
        } else {
            PathBuf::from(path)
        };
        if path.components().count() == 1 {
            in_path(path.to_str().unwrap_or_default()).unwrap_or(path)
        } else {
            path
        }
    } else if let Some(program) = &settings.program {
        program.clone()
    } else {
        default_program()?
    };
    let mut args: Vec<OsString> = Vec::new();
    let name = program
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    #[cfg(target_os = "macos")]
    if program.extension().is_some_and(|s| s == "app") {
        if !program.is_dir() {
            return Err(std::io::Error::other(
                "Terminal application no longer exists",
            ));
        }
        if name == "terminal" || name == "iterm" {
            // Paths are passed as data, quoted by AppleScript, never inserted
            // into source. macOS may request Automation permission on first use.
            let script = if name == "terminal" {
                "on run argv\ntell application \"Terminal\"\nactivate\ndo script (\"cd -- \" & quoted form of item 1 of argv)\nend tell\nend run"
            } else {
                "on run argv\ntell application \"iTerm\"\nactivate\nset w to (create window with default profile)\ntell current session of w to write text (\"cd -- \" & quoted form of item 1 of argv)\nend tell\nend run"
            };
            return Ok(Launch {
                program: "/usr/bin/osascript".into(),
                args: vec!["-e".into(), script.into(), directory.as_os_str().to_owned()],
                directory,
            });
        }
        args.extend([OsString::from("-a"), program.as_os_str().to_owned()]);
        let options = terminal_args(&name, &directory);
        if options.is_empty() {
            args.push(directory.as_os_str().to_owned());
        } else {
            args.push("--args".into());
            args.extend(options);
        }
        return Ok(Launch {
            program: "/usr/bin/open".into(),
            args,
            directory,
        });
    }
    if !executable(&program) {
        return Err(std::io::Error::other(
            "Terminal executable not found or not executable",
        ));
    }
    args.extend(terminal_args(&name, &directory));
    Ok(Launch {
        program,
        args,
        directory,
    })
}

fn terminal_args(name: &str, directory: &Path) -> Vec<OsString> {
    let flag: &[&str] = match name {
        "wt" => &["-d"],
        "wezterm" | "wezterm-gui" => &["start", "--cwd"],
        "kitty" => &["--directory"],
        "alacritty" | "ghostty" | "gnome-terminal" | "xfce4-terminal" => &["--working-directory"],
        "konsole" => &["--workdir"],
        "pwsh" | "powershell" => return vec!["-NoExit".into()],
        "cmd" => return vec!["/K".into()],
        _ => &[], // Unknown executables inherit cwd; no guessed shell arguments.
    };
    let mut args: Vec<_> = flag.iter().map(OsString::from).collect();
    if !args.is_empty() {
        args.push(directory.as_os_str().to_owned());
    }
    args
}

fn default_program() -> std::io::Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        Ok(PathBuf::from("/System/Applications/Utilities/Terminal.app"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        discover()
            .into_iter()
            .next()
            .map(|item| item.path)
            .ok_or_else(|| {
                std::io::Error::other(
                    "No terminal detected; select a custom executable in settings",
                )
            })
    }
}

fn launch(plan: Launch) -> std::io::Result<()> {
    let mut command = Command::new(plan.program);
    command.args(plan.args).current_dir(plan.directory);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x00000010);
    }
    let mut child = command.spawn()?;
    #[cfg(target_os = "macos")]
    {
        let status = child.wait()?;
        if !status.success() {
            return Err(std::io::Error::other(format!(
                "Terminal launcher failed ({status}); check macOS Automation permission"
            )));
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
    Ok(())
}

impl LiteTextApp {
    pub(crate) fn open_terminal(&mut self, ctx: &egui::Context) {
        if self.shortcuts_blocked() || self.terminal_pending.is_some() {
            return;
        }
        let Some(file) = self.active() else {
            return;
        };
        let plan = directory(file).and_then(|directory| plan(&self.terminal, directory));
        match plan {
            Err(error) => self.io_error = Some(format!("Open terminal: {error}")),
            Ok(plan) => {
                let (tx, rx) = std::sync::mpsc::channel();
                self.terminal_pending = Some(rx);
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(launch(plan).map_err(|e| e.to_string()));
                    ctx.request_repaint();
                });
            }
        }
    }

    pub(crate) fn poll_terminal(&mut self) {
        if let Some(result) = self
            .terminal_pending
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        {
            self.terminal_pending = None;
            if let Err(error) = result {
                self.io_error = Some(format!("Open terminal: {error}"));
            }
        }
    }

    pub(crate) fn terminal_settings_ui(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let zh = self.lang == Lang::ZhCn;
        let installed = self.terminal_installed.get_or_insert_with(discover);
        let label = if self.terminal.custom {
            if zh { "自定义" } else { "Custom" }.to_owned()
        } else if let Some(path) = &self.terminal.program {
            installed.iter().find(|t| &t.path == path).map_or_else(
                || {
                    path.file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                },
                |t| t.name.clone(),
            )
        } else {
            if zh { "系统默认" } else { "System default" }.to_owned()
        };
        ui.horizontal(|ui| {
            ui.label(if zh {
                "默认终端"
            } else {
                "Default terminal"
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut options = vec![(
                    if zh { "系统默认" } else { "System default" }.to_owned(),
                    (false, None),
                )];
                options.extend(
                    installed
                        .iter()
                        .map(|t| (t.name.clone(), (false, Some(t.path.clone())))),
                );
                options.push((
                    if zh { "自定义…" } else { "Custom…" }.to_owned(),
                    (true, None),
                ));
                if let Some((custom, program)) = crate::dropdown(
                    ui,
                    pal,
                    egui::Id::new("settings_terminal"),
                    label,
                    options,
                    190.0,
                ) {
                    self.terminal.custom = custom;
                    self.terminal.program = program;
                }
            });
        });
        if self.terminal.custom {
            ui.add(
                egui::TextEdit::singleline(&mut self.terminal.custom_path)
                    .hint_text(if zh {
                        "终端程序路径（不是命令行）"
                    } else {
                        "Executable path (not a shell command)"
                    })
                    .desired_width(ui.available_width()),
            );
            ui.horizontal(|ui| {
                if ui
                    .button(if zh {
                        "选择程序…"
                    } else {
                        "Choose app…"
                    })
                    .clicked()
                    && let Some(path) = rfd::FileDialog::new().pick_file()
                {
                    self.terminal.custom_path = path.to_string_lossy().into_owned();
                }
                if ui.button(if zh { "重新检测" } else { "Refresh" }).clicked() {
                    self.terminal_installed = None;
                }
            });
        } else if ui
            .small_button(if zh {
                "重新检测本机终端"
            } else {
                "Refresh installed terminals"
            })
            .clicked()
        {
            self.terminal_installed = None;
        }
        ui.label(
            egui::RichText::new(if zh {
                "已保存文件：所在目录；未保存文件：用户主目录。"
            } else {
                "Saved files: parent folder. Unsaved: home folder."
            })
            .small()
            .color(pal.text_faint),
        );
    }
}

pub(crate) fn button(ui: &mut egui::Ui, pal: &Palette, lang: Lang) -> egui::Response {
    let response = crate::toolbar_toggle(
        ui,
        false,
        if lang == Lang::ZhCn {
            "打开终端"
        } else {
            "Open terminal"
        },
        pal,
        |p, rect, color| {
            let bounds = egui::Rect::from_center_size(rect.center(), egui::vec2(18.0, 14.0));
            let stroke = egui::Stroke::new(1.3, color);
            p.rect_stroke(bounds, 3.0, stroke, egui::StrokeKind::Inside);
            let c = bounds.center();
            p.line_segment(
                [c + egui::vec2(-5.0, -3.0), c + egui::vec2(-2.0, 0.0)],
                stroke,
            );
            p.line_segment(
                [c + egui::vec2(-2.0, 0.0), c + egui::vec2(-5.0, 3.0)],
                stroke,
            );
            p.line_segment([c + egui::vec2(0.0, 3.0), c + egui::vec2(5.0, 3.0)], stroke);
        },
    );
    ui.ctx()
        .data_mut(|d| d.insert_temp(egui::Id::new("terminal_button_rect"), response.rect));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_directory_uses_saved_parent_or_home_and_settings_persist() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file ' 中文 ; $(touch nope).txt");
        std::fs::write(&path, "original").unwrap();
        let mut app = LiteTextApp::default();
        app.new_document();
        assert_eq!(directory(app.active().unwrap()).unwrap(), home().unwrap());
        app.open_paths(vec![path.clone()]);
        assert_eq!(
            directory(app.active().unwrap()).unwrap(),
            dir.path().canonicalize().unwrap()
        );
        app.active_mut().unwrap().draft_text = Some("unsaved edits".to_owned());
        assert_eq!(
            directory(app.active().unwrap()).unwrap(),
            dir.path().canonicalize().unwrap()
        );
        let file = app.active_mut().unwrap();
        file.source_path = Some(std::mem::take(&mut file.path));
        assert_eq!(directory(app.active().unwrap()).unwrap(), home().unwrap());
        app.terminal = Settings {
            custom: true,
            custom_path: "/some path/My Terminal".to_owned(),
            program: None,
        };
        let restored: LiteTextApp =
            serde_json::from_str(&serde_json::to_string(&app).unwrap()).unwrap();
        assert_eq!(restored.terminal, app.terminal);
        let old: LiteTextApp = serde_json::from_str("{}").unwrap();
        assert_eq!(old.terminal, Settings::default());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "original");
    }

    #[test]
    fn launch_plans_pass_directories_as_data_and_reject_invalid_custom_programs() {
        let directory = PathBuf::from("/path with spaces/' ; $(touch nope) 中文");
        for name in [
            "wt",
            "wezterm",
            "kitty",
            "alacritty",
            "ghostty",
            "gnome-terminal",
            "konsole",
        ] {
            let args = terminal_args(name, &directory);
            assert_eq!(args.last().unwrap(), directory.as_os_str());
        }
        assert!(
            plan(
                &Settings {
                    custom: true,
                    ..Default::default()
                },
                directory.clone()
            )
            .is_err()
        );
        assert!(
            plan(
                &Settings {
                    custom: true,
                    custom_path: "/missing/terminal --execute dangerous".to_owned(),
                    ..Default::default()
                },
                directory.clone()
            )
            .is_err()
        );
        #[cfg(target_os = "macos")]
        {
            let plan = plan(&Settings::default(), directory.clone()).unwrap();
            assert_eq!(plan.program, Path::new("/usr/bin/osascript"));
            assert_eq!(plan.args.last().unwrap(), directory.as_os_str());
            assert!(!plan.args[1].to_string_lossy().contains("touch nope"));
        }
    }

    #[test]
    fn terminal_icon_is_document_only_between_find_and_view_and_settings_are_narrow() {
        for lang in [Lang::ZhCn, Lang::En] {
            let ctx = egui::Context::default();
            crate::setup_fonts(&ctx);
            let mut app = LiteTextApp {
                lang,
                ..Default::default()
            };
            let raw = || egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1080.0, 720.0),
                )),
                ..Default::default()
            };
            let _ = ctx.run(raw(), |ctx| app.top_bar(ctx, &crate::DARK));
            assert!(
                ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new("terminal_button_rect")))
                    .is_none()
            );
            app.new_document();
            for _ in 0..3 {
                let _ = ctx.run(raw(), |ctx| app.top_bar(ctx, &crate::DARK));
            }
            let terminal = ctx
                .data(|d| d.get_temp::<egui::Rect>(egui::Id::new("terminal_button_rect")))
                .unwrap();
            let find = ctx
                .data(|d| {
                    d.get_temp::<egui::Rect>(egui::Id::new((
                        "text_tool_rect",
                        app.active_id.unwrap(),
                        0,
                    )))
                })
                .unwrap();
            assert!(find.right() < terminal.left());
            app.show_settings = true;
            let mut output = None;
            for _ in 0..3 {
                output = Some(ctx.run(raw(), |ctx| app.settings_window(ctx, &crate::DARK)));
            }
            let text = output
                .as_ref()
                .unwrap()
                .shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::Shape::Text(t)
                        if t.galley.text()
                            == if lang == Lang::ZhCn {
                                "默认终端"
                            } else {
                                "Default terminal"
                            } =>
                    {
                        Some(t)
                    }
                    _ => None,
                });
            assert!(text.is_some());
            let entry = ctx
                .data(|d| d.get_temp::<egui::Rect>(egui::Id::new("settings_workspace_theme_rect")))
                .unwrap();
            assert!(entry.width() <= 461.0);
            app.terminal.custom = true;
            for _ in 0..3 {
                let _ = ctx.run(raw(), |ctx| app.settings_window(ctx, &crate::DARK));
            }
            assert!(app.terminal_pending.is_none()); // tests never launch a real terminal
        }
    }
}

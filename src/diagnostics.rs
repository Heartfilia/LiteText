//! Opt-in bounded in-memory diagnostics. Never writes documents or log files.
use crate::{Lang, LiteTextApp, Palette};
use eframe::egui;
use std::{
    collections::VecDeque,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

const MAX_ENTRIES: usize = 2000;
const MAX_BYTES: usize = 512 * 1024;
const MAX_MESSAGE: usize = 2048;

#[derive(Default)]
pub(crate) struct Settings {
    pub(crate) unlocked: bool,
    pub(crate) enabled: bool,
}

#[derive(Default)]
struct Buffer {
    lines: VecDeque<String>,
    bytes: usize,
    dropped: usize,
}

impl Buffer {
    fn push(&mut self, level: &str, source: &str, message: &str) {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let safe = sanitize(message);
        let line = format!(
            "{}.{:03} [{level}] {source}: {safe}\n",
            stamp.as_secs(),
            stamp.subsec_millis()
        );
        self.bytes += line.len();
        self.lines.push_back(line);
        while self.lines.len() > MAX_ENTRIES || self.bytes > MAX_BYTES {
            if let Some(line) = self.lines.pop_front() {
                self.bytes -= line.len();
                self.dropped += 1;
            }
        }
    }

    fn text(&self) -> String {
        let mut text = format!(
            "LiteText diagnostics | version={} | os={}/{} | build={} | timestamps=Unix seconds\n",
            crate::updater::VERSION,
            std::env::consts::OS,
            std::env::consts::ARCH,
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
        );
        if self.dropped > 0 {
            text.push_str(&format!(
                "[WARN] {} older entries discarded (bounded buffer)\n",
                self.dropped
            ));
        }
        for line in &self.lines {
            text.push_str(line);
        }
        text
    }
}

/// Only known GUI/rendering dependencies are captured: never HTTP payloads,
/// proxy credentials, document text, terminal commands or arbitrary log targets.
fn framework_target(target: &str) -> bool {
    [
        "egui",
        "egui_winit",
        "egui_glow",
        "epaint",
        "eframe",
        "winit",
        "glutin",
        "glow",
    ]
    .iter()
    .any(|prefix| target == *prefix || target.starts_with(&format!("{prefix}::")))
}

fn sanitize(message: &str) -> String {
    static PRIVATE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let private = PRIVATE.get_or_init(|| regex::Regex::new(r#"(?i)(?:https?://|file://|[a-z]:[\\/]|/|\\\\)[^\r\n]*|(?:token|password|authorization|apikey|api_key)\s*[:=][^\r\n]*"#).unwrap());
    let message = private.replace_all(message, "[redacted]");
    let mut text: String = message
        .chars()
        .take(MAX_MESSAGE)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if message.chars().count() > MAX_MESSAGE {
        text.push_str("… [truncated]");
    }
    text
}

struct RuntimeLogger {
    enabled: AtomicBool,
    buffer: Mutex<Buffer>,
}

static LOGGER: RuntimeLogger = RuntimeLogger {
    enabled: AtomicBool::new(false),
    buffer: Mutex::new(Buffer {
        lines: VecDeque::new(),
        bytes: 0,
        dropped: 0,
    }),
};

impl log::Log for RuntimeLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        self.enabled.load(Ordering::Relaxed)
            && (metadata.target() == "litetext::fold"
                || metadata.level() <= log::Level::Warn && framework_target(metadata.target()))
    }
    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata())
            && let Ok(mut buffer) = self.buffer.lock()
        {
            buffer.push(
                record.level().as_str(),
                record.target(),
                &record.args().to_string(),
            );
        }
    }
    fn flush(&self) {}
}

pub(crate) fn install_logger() {
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
    // Preserve the normal panic handler; record only source location, never a
    // potentially document-derived panic payload. Process termination cannot
    // guarantee that an in-memory panel remains available.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if LOGGER.enabled.load(Ordering::Relaxed)
            && let Ok(mut buffer) = LOGGER.buffer.try_lock()
        {
            let location = info
                .location()
                .map(|l| format!("{}:{}", l.file(), l.line()))
                .unwrap_or_default();
            buffer.push(
                "ERROR",
                "panic",
                &format!("Panic at {location}; payload omitted"),
            );
        }
        previous(info);
    }));
}

#[derive(Default)]
pub(crate) struct State {
    pub(crate) page: bool,
    clicks: u8,
    last_click: Option<f64>,
    buffer: Buffer,
    last_io: Option<String>,
    last_plugin: Option<String>,
    last_update: Option<String>,
    last_open_errors: usize,
    last_frame_warning: Option<Instant>,
    last_gui_warnings: Vec<String>,
}

impl State {
    pub(crate) fn reset_clicks(&mut self) {
        self.clicks = 0;
        self.last_click = None;
    }
}

impl LiteTextApp {
    pub(crate) fn initialize_diagnostics(&mut self) {
        // Always start hidden and off, including when restoring older settings.
        self.diagnostics_settings = Settings::default();
        self.diagnostics = State::default();
        self.configure_diagnostic_capture();
        #[cfg(not(test))]
        if let Ok(mut buffer) = LOGGER.buffer.lock() {
            *buffer = Buffer::default();
        }
    }

    fn configure_diagnostic_capture(&self) {
        #[cfg(not(test))]
        LOGGER
            .enabled
            .store(self.diagnostics_settings.enabled, Ordering::Relaxed);
    }

    pub(crate) fn diagnostic_event(&mut self, level: &str, source: &str, message: &str) {
        if self.diagnostics_settings.enabled {
            self.diagnostics.buffer.push(level, source, message);
        }
    }

    fn diagnostic_snapshot(&mut self) {
        if self.diagnostics_settings.enabled {
            let pinned = self.files.iter().filter(|f| f.pinned).count();
            self.diagnostic_event(
                "INFO",
                "state",
                &format!(
                    "tabs={}; pinned={pinned}; active_id={:?}; font_size={}; ui_language={:?}",
                    self.files.len(),
                    self.active_id,
                    self.workspace_font_size,
                    self.lang
                ),
            );
        }
    }

    fn set_diagnostic_recording(&mut self, enabled: bool) {
        if self.diagnostics_settings.enabled == enabled {
            return;
        }
        if !enabled {
            self.observe_diagnostics();
            self.diagnostics.buffer.push(
                "INFO",
                "session",
                "Recording disabled; no more entries will be collected",
            );
        }
        self.diagnostics_settings.enabled = enabled && self.diagnostics_settings.unlocked;
        self.configure_diagnostic_capture();
        if self.diagnostics_settings.enabled {
            self.diagnostic_event(
                "INFO",
                "session",
                "Recording enabled (current run only; hidden and off on next launch)",
            );
            self.diagnostic_snapshot();
            self.observe_diagnostics();
        }
    }

    pub(crate) fn diagnostics_version_click(&mut self, now: f64) {
        if self.diagnostics_settings.unlocked {
            return;
        }
        if self
            .diagnostics
            .last_click
            .is_none_or(|last| now - last > 1.5 || now < last)
        {
            self.diagnostics.clicks = 0;
        }
        self.diagnostics.last_click = Some(now);
        self.diagnostics.clicks += 1;
        if self.diagnostics.clicks == 5 {
            self.diagnostics_settings.unlocked = true;
            self.diagnostics.reset_clicks();
        }
    }

    pub(crate) fn observe_diagnostics(&mut self) {
        if !self.diagnostics_settings.enabled {
            return;
        }
        if let Ok(mut runtime) = LOGGER.buffer.lock() {
            while let Some(line) = runtime.lines.pop_front() {
                self.diagnostics.buffer.bytes += line.len();
                self.diagnostics.buffer.lines.push_back(line);
            }
            self.diagnostics.buffer.dropped += runtime.dropped;
            *runtime = Buffer::default();
        }
        // Capture before dismissible dialogs can clear them; raw UI messages
        // may contain filenames, source fragments or secrets and are not copied.
        if self.io_error != self.diagnostics.last_io {
            if self.io_error.is_some() {
                self.diagnostic_event(
                    "ERROR",
                    "io",
                    "IO error displayed; raw text omitted for privacy",
                );
            }
            self.diagnostics.last_io = self.io_error.clone();
        }
        if self.plugin_error != self.diagnostics.last_plugin {
            if self.plugin_error.is_some() {
                self.diagnostic_event(
                    "ERROR",
                    "plugin",
                    "Text/plugin operation failed; input and raw error omitted",
                );
            }
            self.diagnostics.last_plugin = self.plugin_error.clone();
        }
        if self.open_errors.len() > self.diagnostics.last_open_errors {
            self.diagnostic_event(
                "ERROR",
                "open",
                &format!(
                    "File-open failures displayed: {} (paths omitted)",
                    self.open_errors.len()
                ),
            );
        }
        self.diagnostics.last_open_errors = self.open_errors.len();
        if self.diagnostics.last_update.as_deref() != Some(self.updater.diagnostic_status()) {
            if !self.updater.diagnostic_status().is_empty() {
                self.diagnostic_event(
                    "INFO",
                    "updater",
                    "Update status changed (network details omitted)",
                );
            }
            self.diagnostics.last_update = Some(self.updater.diagnostic_status().to_owned());
        }
        // Enforce the same bounds after merging worker/framework messages.
        while self.diagnostics.buffer.lines.len() > MAX_ENTRIES
            || self.diagnostics.buffer.bytes > MAX_BYTES
        {
            if let Some(line) = self.diagnostics.buffer.lines.pop_front() {
                self.diagnostics.buffer.bytes -= line.len();
                self.diagnostics.buffer.dropped += 1;
            }
        }
    }

    pub(crate) fn diagnostic_frame(&mut self, elapsed: std::time::Duration) {
        if self.diagnostics_settings.enabled
            && elapsed.as_millis() >= 100
            && self
                .diagnostics
                .last_frame_warning
                .is_none_or(|t| t.elapsed().as_secs() >= 2)
        {
            self.diagnostic_event(
                "WARN",
                "ui",
                &format!(
                    "Slow UI frame: {:.1} ms; tabs={}; active_id={:?}",
                    elapsed.as_secs_f64() * 1000.0,
                    self.files.len(),
                    self.active_id
                ),
            );
            self.diagnostics.last_frame_warning = Some(Instant::now());
        }
    }

    /// egui paints ID-collision errors directly instead of using log::warn.
    /// Inspect its dedicated debug layer only, never document shapes.
    pub(crate) fn capture_gui_diagnostics(&mut self, ctx: &egui::Context) {
        if !self.diagnostics_settings.enabled {
            return;
        }
        let layer = ctx.debug_painter().layer_id();
        let mut warnings = ctx.graphics(|graphics| {
            graphics.get(layer).map(|list| list.all_entries().filter_map(|s| {
                if let egui::Shape::Text(text) = &s.shape {
                    let value = text.galley.text();
                    if value.starts_with("🔥 ") && value.contains(" ID ")
                        && ["First use of ", "Second use of ", "Double use of "].iter().any(|prefix| value.contains(prefix)) {
                        let id = value.rsplit(" ID ").next().unwrap_or("unknown");
                        let id: String = id.chars().take(16).filter(|c| c.is_ascii_hexdigit()).collect();
                        return Some(format!("Widget ID collision: id={id}; a red GUI diagnostic was painted"));
                    }
                }
                None
            }).collect::<Vec<_>>()).unwrap_or_default()
        });
        warnings.sort();
        warnings.dedup();
        for warning in &warnings {
            if !self.diagnostics.last_gui_warnings.contains(warning) {
                self.diagnostic_event("WARN", "egui", warning);
            }
        }
        self.diagnostics.last_gui_warnings = warnings;
    }

    pub(crate) fn diagnostics_ui(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let zh = self.lang == Lang::ZhCn;
        let back = ui.button(if zh {
            "‹ 返回设置"
        } else {
            "‹ Back to settings"
        });
        ui.ctx()
            .data_mut(|d| d.insert_temp(egui::Id::new("diagnostics_back_rect"), back.rect));
        if back.clicked() {
            self.diagnostics.page = false;
        }
        ui.heading(if zh {
            "诊断日志"
        } else {
            "Diagnostic logs"
        });
        let mut enabled = self.diagnostics_settings.enabled;
        let toggle = ui.checkbox(
            &mut enabled,
            if zh {
                "启用诊断日志记录"
            } else {
                "Enable diagnostic recording"
            },
        );
        ui.ctx()
            .data_mut(|d| d.insert_temp(egui::Id::new("diagnostics_toggle_rect"), toggle.rect));
        if toggle.changed() {
            self.set_diagnostic_recording(enabled);
        }
        ui.label(egui::RichText::new(if zh {
            "仅本次运行有效。重新启动后入口隐藏、记录关闭，需连续点击版本号 5 次重新解锁并手动开启记录。日志仅保留在本次运行的内存中（最多 2000 条／512 KiB），退出即清除。记录应用错误、GUI 警告及关键操作，不记录正文、文件路径或网络请求。依赖库消息会脱敏，分享前请检查。"
        } else {
            "Current run only. Each launch hides the entry and turns recording off; click the version five times to unlock again, then enable recording manually. Logs stay in memory only (up to 2000 entries / 512 KiB) and are lost on exit. Records app errors, GUI warnings and key operations, not documents, paths or network requests. Review redacted dependency messages before sharing."
        }).small().color(pal.text_faint));
        self.observe_diagnostics();
        ui.horizontal(|ui| {
            let copy = ui.button(if zh {
                "复制全部日志"
            } else {
                "Copy all logs"
            });
            ui.ctx()
                .data_mut(|d| d.insert_temp(egui::Id::new("diagnostics_copy_rect"), copy.rect));
            if copy.clicked() {
                ui.ctx().copy_text(self.diagnostics.buffer.text());
            }
            let clear = ui.button(if zh { "清空日志" } else { "Clear logs" });
            ui.ctx()
                .data_mut(|d| d.insert_temp(egui::Id::new("diagnostics_clear_rect"), clear.rect));
            if clear.clicked() {
                self.diagnostics.buffer = Buffer::default();
                #[cfg(not(test))]
                if let Ok(mut buffer) = LOGGER.buffer.lock() {
                    *buffer = Buffer::default();
                }
            }
            ui.label(format!(
                "{} / {MAX_ENTRIES}",
                self.diagnostics.buffer.lines.len()
            ));
        });
        let text = self.diagnostics.buffer.text();
        egui::ScrollArea::both()
            .id_salt("diagnostics_log_scroll")
            .auto_shrink([false, false])
            .max_height(ui.available_height().max(80.0))
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut text.as_str())
                        .font(egui::TextStyle::Monospace)
                        .desired_width(f32::INFINITY)
                        .desired_rows(12),
                );
            });
    }
}

#[cfg(test)]
#[path = "diagnostics_tests.rs"]
mod tests;

//! Installation-independent user preferences; never stores document content.
use crate::{LiteTextApp, recovery};
use serde::{Deserialize, Serialize};
use std::{io, path::{Path, PathBuf}, time::{Duration, Instant}};

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Preferences {
    lang: crate::Lang,
    workspace_theme: crate::workspace_theme::WorkspaceTheme,
    workspace_font_size: f32,
    tab_mode: crate::whitespace::TabMode,
    terminal: crate::terminal::Settings,
    updates: crate::updater::Settings,
    shortcut_keys: crate::shortcuts::Keymap,
    hex_case_upper: bool,
    dim_zeros: bool,
    show_ascii: bool,
    show_invisible: bool,
    inspector_width: f32,
}

impl Default for Preferences {
    fn default() -> Self { Self::capture(&LiteTextApp::default()) }
}

impl Preferences {
    fn capture(app: &LiteTextApp) -> Self {
        Self {
            lang: app.lang, workspace_theme: app.workspace_theme.clone(),
            workspace_font_size: app.workspace_font_size, tab_mode: app.tab_mode,
            terminal: app.terminal.clone(), updates: app.updates.clone(),
            shortcut_keys: app.shortcut_keys.clone(), hex_case_upper: app.hex_upper,
            dim_zeros: app.dim_zeros, show_ascii: app.show_ascii,
            show_invisible: app.show_invisible, inspector_width: app.inspector_width,
        }
    }

    fn apply(self, app: &mut LiteTextApp) {
        app.lang = self.lang;
        app.workspace_theme = self.workspace_theme;
        app.workspace_font_size = self.workspace_font_size;
        app.tab_mode = self.tab_mode;
        app.terminal = self.terminal;
        app.updates = self.updates;
        app.shortcut_keys = self.shortcut_keys;
        app.hex_upper = self.hex_case_upper;
        app.dim_zeros = self.dim_zeros;
        app.show_ascii = self.show_ascii;
        app.show_invisible = self.show_invisible;
        app.inspector_width = self.inspector_width;
    }
}

#[derive(Default)]
pub(crate) struct State {
    observed: Option<Vec<u8>>,
    pending_since: Option<Instant>,
    initialize_only: bool,
    #[cfg(test)]
    pub(crate) path_override: Option<PathBuf>,
}

fn load(path: &Path, app: &mut LiteTextApp) -> io::Result<bool> {
    match std::fs::read(path) {
        Ok(bytes) => {
            serde_json::from_slice::<Preferences>(&bytes).map_err(io::Error::other)?.apply(app);
            Ok(false)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error),
    }
}

impl LiteTextApp {
    fn preferences_path(&self) -> Option<PathBuf> {
        #[cfg(test)]
        { self.preferences.path_override.clone() }
        #[cfg(not(test))]
        { eframe::storage_dir("LiteText").map(|p| p.join("preferences.json")) }
    }

    pub(crate) fn load_preferences(&mut self) -> io::Result<()> {
        let result = self.preferences_path().map(|path| load(&path, self)).transpose();
        // Even a failed load establishes a baseline. Do not replace unreadable
        // or corrupt data just because the first GUI frame is being rendered.
        self.preferences.observed = Some(serde_json::to_vec(&Preferences::capture(self)).map_err(io::Error::other)?);
        self.preferences.initialize_only = result?.unwrap_or(false);
        self.preferences.pending_since = self.preferences.initialize_only.then(Instant::now);
        Ok(())
    }

    pub(crate) fn preferences_repaint_after(&self) -> Option<Duration> {
        self.preferences.pending_since.map(|since| Duration::from_millis(500).saturating_sub(since.elapsed()))
    }

    /// Debounce typing, but flush when closing settings, exiting or installing.
    /// An unchanged live window must not overwrite another window's preferences.
    pub(crate) fn save_preferences(&mut self, force: bool) -> io::Result<()> {
        let Some(path) = self.preferences_path() else { return Ok(()); };
        let bytes = serde_json::to_vec(&Preferences::capture(self)).map_err(io::Error::other)?;
        if self.preferences.observed.as_ref() != Some(&bytes) {
            self.preferences.observed = Some(bytes.clone());
            self.preferences.pending_since = Some(Instant::now());
            self.preferences.initialize_only = false;
        }
        if let Some(since) = self.preferences.pending_since
            && (force || since.elapsed() >= Duration::from_millis(500))
        {
            // Back off all failures, including lock/read/backup failures.
            self.preferences.pending_since = Some(Instant::now());
            std::fs::create_dir_all(path.parent().ok_or_else(|| io::Error::other("Missing preferences directory"))?)?;
            let lock = std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true)
                .open(path.with_extension("lock"))?;
            fs2::FileExt::lock_exclusive(&lock)?;
            let previous = match std::fs::read(&path) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => return Err(error),
            };
            // A different window may already have completed first-run migration.
            if self.preferences.initialize_only && previous.is_some() {
                self.preferences.pending_since = None;
                self.preferences.initialize_only = false;
                return Ok(());
            }
            // Preserve invalid previous settings before replacing them on an
            // explicit edit; an unreadable file is never overwritten.
            if let Some(previous) = previous
                && serde_json::from_slice::<Preferences>(&previous).is_err() {
                let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
                std::fs::write(path.with_extension(format!("invalid-{stamp}.json")), previous)?;
            }
            recovery::atomic_write(&path, &bytes)?;
            self.preferences.pending_since = None;
            self.preferences.initialize_only = false;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferences_survive_reinstall_and_unchanged_windows_do_not_overwrite() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("data/preferences.json");
        let app = || {
            let mut app = LiteTextApp::default();
            app.preferences.path_override = Some(path.clone());
            app
        };
        let mut first = app();
        first.load_preferences().unwrap();
        let mut other = app();
        other.load_preferences().unwrap();
        first.lang = crate::Lang::En;
        first.terminal.show_toolbar_button = true;
        first.terminal.custom = true;
        first.terminal.custom_path = "/Applications/Otty.app".into();
        first.workspace_font_size = 18.0;
        first.workspace_theme.preset = crate::workspace_theme::Preset::Nord;
        first.updates = serde_json::from_value(serde_json::json!({
            "proxy_enabled": true, "proxy_url": "http://127.0.0.1:7890"
        })).unwrap();
        first.tab_mode = crate::whitespace::TabMode::Spaces4;
        first.new_document();
        first.active_mut().unwrap().draft_text = Some("private document not a setting".into());
        first.save_preferences(true).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("private document"));
        other.save_preferences(true).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let mut reinstalled = app(); // no window app.ron / installation files
        reinstalled.load_preferences().unwrap();
        assert_eq!(reinstalled.lang, crate::Lang::En);
        assert_eq!(reinstalled.terminal, first.terminal);
        assert_eq!(reinstalled.workspace_font_size, 18.0);
        assert_eq!(reinstalled.workspace_theme.preset, crate::workspace_theme::Preset::Nord);
        assert_eq!(serde_json::to_value(Preferences::capture(&reinstalled)).unwrap(), serde_json::to_value(Preferences::capture(&first)).unwrap());
        assert!(reinstalled.files.is_empty());
    }

    #[test]
    fn legacy_preferences_and_write_failures_keep_current_choices() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("preferences.json");
        std::fs::write(&path, br#"{"lang":"en","terminal":{"show_toolbar_button":true},"updates":{"proxy_enabled":true,"proxy_url":"http://127.0.0.1:7890"}}"#).unwrap();
        let mut app = LiteTextApp::default();
        app.preferences.path_override = Some(path.clone());
        app.load_preferences().unwrap();
        assert_eq!(app.lang, crate::Lang::En);
        assert!(app.terminal.show_toolbar_button);
        app.save_preferences(true).unwrap();
        let restored: Preferences = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(serde_json::to_value(restored.updates).unwrap()["proxy_url"], "http://127.0.0.1:7890");
        let blocker = temp.path().join("blocked");
        std::fs::write(&blocker, "not a directory").unwrap();
        app.preferences.path_override = Some(blocker.join("preferences.json"));
        app.terminal.show_toolbar_button = false;
        assert!(app.save_preferences(true).is_err());
        assert!(!app.terminal.show_toolbar_button);
        assert!(app.preferences.pending_since.is_some());
    }

    #[test]
    fn missing_preferences_migrate_legacy_choices_and_debounce_edits() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("preferences.json");
        let mut app = LiteTextApp::default();
        app.preferences.path_override = Some(path.clone());
        app.lang = crate::Lang::En; // choices already restored from app.ron
        app.load_preferences().unwrap();
        app.save_preferences(false).unwrap();
        assert!(!path.exists());
        app.save_preferences(true).unwrap();
        let original = std::fs::read(&path).unwrap();
        assert_eq!(serde_json::from_slice::<Preferences>(&original).unwrap().lang, crate::Lang::En);
        app.workspace_font_size = 22.0; // status bar / shortcut, outside settings
        app.save_preferences(false).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert!(app.preferences_repaint_after().is_some());
        app.preferences.pending_since = Some(Instant::now() - Duration::from_secs(1));
        app.save_preferences(false).unwrap();
        assert_eq!(serde_json::from_slice::<Preferences>(&std::fs::read(&path).unwrap()).unwrap().workspace_font_size, 22.0);
        assert!(app.preferences_repaint_after().is_none());
    }

    #[test]
    fn corrupt_or_unreadable_preferences_are_not_silently_replaced() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("preferences.json");
        let invalid = b"{incomplete preferences";
        std::fs::write(&path, invalid).unwrap();
        let mut app = LiteTextApp::default();
        app.preferences.path_override = Some(path.clone());
        assert!(app.load_preferences().is_err());
        app.save_preferences(true).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), invalid);
        app.lang = crate::Lang::En;
        app.save_preferences(true).unwrap();
        let backup = std::fs::read_dir(temp.path()).unwrap().filter_map(Result::ok)
            .find(|e| e.file_name().to_string_lossy().contains("invalid-")).unwrap();
        assert_eq!(std::fs::read(backup.path()).unwrap(), invalid);
        assert_eq!(serde_json::from_slice::<Preferences>(&std::fs::read(&path).unwrap()).unwrap().lang, crate::Lang::En);

        let unreadable = temp.path().join("directory.json");
        std::fs::create_dir(&unreadable).unwrap();
        app.preferences.path_override = Some(unreadable.clone());
        assert!(app.load_preferences().is_err());
        app.save_preferences(true).unwrap(); // no edits, no replacement
        app.workspace_font_size = 20.0;
        assert!(app.save_preferences(true).is_err());
        assert!(unreadable.is_dir());
        assert!(app.preferences_repaint_after().unwrap() > Duration::from_millis(100));
    }
}

//! Additional processes have independent settings/recovery storage. Never clear
//! the original window's session to implement an empty new window.
use std::{path::PathBuf, sync::OnceLock};

static WINDOW: OnceLock<String> = OnceLock::new();

pub(crate) fn initialize() {
    let args: Vec<_> = std::env::args_os().collect();
    if let Some(index) = args.iter().position(|a| a == "--litetext-window")
        && let Some(id) = args.get(index + 1).and_then(|s| s.to_str())
        && valid_id(id)
    {
        let _ = WINDOW.set(id.to_owned());
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

fn directory_for(id: &str) -> Option<PathBuf> {
    eframe::storage_dir("LiteText").map(|p| p.join("windows").join(id))
}

pub(crate) fn directory() -> Option<PathBuf> {
    directory_for(WINDOW.get()?)
}

pub(crate) fn lock_current() -> std::io::Result<Option<std::fs::File>> {
    use fs2::FileExt;
    let Some(directory) = directory() else {
        return Ok(None);
    };
    std::fs::create_dir_all(&directory)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join("window.lock"))?;
    file.try_lock_exclusive()?;
    Ok(Some(file))
}

pub(crate) fn restore_closed_windows() {
    use fs2::FileExt;
    static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if DONE.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    if WINDOW.get().is_some() {
        return;
    }
    let Some(root) = eframe::storage_dir("LiteText").map(|p| p.join("windows")) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(id) = path
            .file_name()
            .and_then(|s| s.to_str())
            .filter(|id| valid_id(id))
        else {
            continue;
        };
        let Ok(Some(session)) = crate::recovery::load(&path.join("recovery/session.json")) else {
            continue;
        };
        if session.is_empty() {
            continue;
        }
        let Ok(lock) = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path.join("window.lock"))
        else {
            continue;
        };
        if lock.try_lock_exclusive().is_err() {
            continue;
        }
        let Ok(exe) = std::env::current_exe() else {
            continue;
        };
        let mut command = std::process::Command::new(exe);
        command
            .arg("--litetext-window")
            .arg(id)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        crate::windows::hide_console(&mut command);
        drop(lock);
        let _ = command.spawn();
    }
}

pub(crate) fn append_arguments(command: &mut std::process::Command) {
    if let Some(id) = WINDOW.get() {
        command.arg("--litetext-window").arg(id);
    }
}

pub(crate) fn seed() -> Option<crate::LiteTextApp> {
    serde_json::from_slice(&std::fs::read(directory()?.join("initial-settings.json")).ok()?).ok()
}

impl crate::LiteTextApp {
    pub(crate) fn open_new_window(&mut self) {
        #[cfg(test)]
        {
            self.new_window_requested = true;
        }
        #[cfg(not(test))]
        if let Err(error) = self.spawn_new_window() {
            self.io_error = Some(format!("Unable to open new window: {error}"));
        }
    }

    #[cfg(not(test))]
    fn spawn_new_window(&self) -> Result<(), Box<dyn std::error::Error>> {
        let root = eframe::storage_dir("LiteText")
            .ok_or("Application storage unavailable")?
            .join("windows");
        std::fs::create_dir_all(&root)?;
        let directory = tempfile::Builder::new()
            .prefix("window-")
            .tempdir_in(root)?;
        let id = directory
            .path()
            .file_name()
            .ok_or("Invalid window directory")?;
        std::fs::write(
            directory.path().join("initial-settings.json"),
            serde_json::to_vec(self)?,
        )?;
        let mut command = std::process::Command::new(std::env::current_exe()?);
        command
            .arg("--litetext-window")
            .arg(id)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        crate::windows::hide_console(&mut command);
        command.spawn()?;
        let _ = directory.keep();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn window_ids_cannot_escape_independent_storage() {
        for id in ["../x", "a/b", "a\\b", "", ".", "a b"] {
            assert!(!valid_id(id));
        }
        assert!(valid_id("window-aB09_x"));
        assert_ne!(directory_for("window-a"), directory_for("window-b"));
    }

    #[test]
    fn new_window_settings_seed_has_no_workspaces_and_recovery_is_independent() {
        let temp = tempfile::tempdir().unwrap();
        let original_path = temp.path().join("original.jsonl");
        std::fs::write(&original_path, "original").unwrap();
        let mut original = crate::LiteTextApp {
            recovery_path_override: Some(temp.path().join("first/session.json")),
            ..Default::default()
        };
        original.open_paths(vec![original_path.clone()]);
        original.active_mut().unwrap().draft_text = Some("first unsaved".into());
        original.active_mut().unwrap().sync_draft();
        original.workspace_font_size = 20.0;
        assert!(original.backup_or_report());
        let mut next: crate::LiteTextApp =
            serde_json::from_slice(&serde_json::to_vec(&original).unwrap()).unwrap();
        assert!(next.files.is_empty());
        assert_eq!(next.workspace_font_size, 20.0);
        next.recovery_path_override = Some(temp.path().join("second/session.json"));
        next.restore_session();
        assert!(next.files.is_empty());
        next.new_document();
        next.active_mut().unwrap().draft_text = Some("second unsaved".into());
        next.active_mut().unwrap().sync_draft();
        assert!(next.backup_or_report());
        let first = crate::recovery::load(&original.recovery_path_override.unwrap())
            .unwrap()
            .unwrap()
            .restore();
        let second = crate::recovery::load(&next.recovery_path_override.unwrap())
            .unwrap()
            .unwrap()
            .restore();
        assert_eq!(first[0].draft_text.as_deref(), Some("first unsaved"));
        assert_eq!(second[0].draft_text.as_deref(), Some("second unsaved"));
        assert_eq!(std::fs::read_to_string(original_path).unwrap(), "original");
    }
}

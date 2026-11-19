//! Independent live buffers, serialized window membership and last-window restore.
//! Normal closes retire private snapshots; crash/update handoff keeps recovery.
use std::{path::PathBuf, sync::OnceLock};

static WINDOW: OnceLock<String> = OnceLock::new();
static RUNTIME: OnceLock<std::sync::Mutex<Runtime>> = OnceLock::new();

struct Runtime {
    root: PathBuf,
    id: String,
    lock: Option<std::fs::File>,
    restore: Option<PathBuf>,
    finished: bool,
}

fn registry_lock(root: &std::path::Path) -> std::io::Result<std::fs::File> {
    use fs2::FileExt;
    std::fs::create_dir_all(root)?;
    let file = std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(root.join("windows.lock"))?;
    file.lock_exclusive()?;
    Ok(file)
}

fn live_windows(root: &std::path::Path) -> std::io::Result<Vec<String>> {
    use fs2::FileExt;
    let mut live = Vec::new();
    let windows = root.join("windows");
    std::fs::create_dir_all(&windows)?;
    for entry in std::fs::read_dir(windows)? {
        let entry = entry?;
        let Some(id) = entry.file_name().to_str().filter(|id| valid_id(id)).map(str::to_owned) else { continue; };
        if !entry.file_type()?.is_dir() { continue; }
        let file = std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(entry.path().join("window.lock"))?;
        match file.try_lock_exclusive() {
            Ok(()) => {},
            Err(error) if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() => live.push(id),
            Err(error) => return Err(error),
        }
    }
    Ok(live)
}

/// Retire only recognized snapshots of non-running windows. Keep settings,
/// unknown files and malformed recovery data untouched; never follow symlinks.
fn retire_inactive_snapshots(root: &std::path::Path, keep: &[String]) -> std::io::Result<()> {
    for entry in std::fs::read_dir(root.join("windows"))? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(id) = name.to_str().filter(|id| valid_id(id)) else { continue; };
        if keep.iter().any(|keep| keep == id) || !entry.file_type()?.is_dir() { continue; }
        let recovery = entry.path().join("recovery");
        if std::fs::symlink_metadata(&recovery).is_ok_and(|m| m.file_type().is_symlink()) { continue; }
        let path = recovery.join("session.json");
        if std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_file())
            && crate::recovery::load(&path).is_ok_and(|s| s.is_some_and(|s| s.has_files())) {
            crate::recovery::store_buffers(&path, &[], None, false)?;
        }
        crate::recovery::prune_reload_archives(&path)?;
    }
    Ok(())
}

/// Remove window directories whose snapshots were just retired. Durable
/// settings live in `preferences.json`, `window.json` and `last-settings.json`,
/// so a completed window directory holds nothing that must survive. Anything
/// unrecognized (drafts, archives, damaged snapshots, symlinks) keeps the
/// whole directory.
fn prune_window_directories(root: &std::path::Path, keep: &[String]) -> std::io::Result<()> {
    const ALLOWED: [&str; 4] = ["app.ron", "initial-settings.json", "window.lock", "recovery"];
    for entry in std::fs::read_dir(root.join("windows"))? {
        let entry = entry?;
        let Some(id) = entry.file_name().to_str().filter(|id| valid_id(id)).map(str::to_owned) else { continue; };
        if keep.contains(&id) || !entry.file_type()?.is_dir() { continue; }
        let mut removable = true;
        for child in std::fs::read_dir(entry.path())? {
            let child = child?;
            let name = child.file_name();
            let Some(name) = name.to_str() else { removable = false; break; };
            let metadata = std::fs::symlink_metadata(child.path())?;
            if metadata.file_type().is_symlink() {
                removable = false;
                break;
            }
            if name == "recovery" && metadata.is_dir() {
                for snapshot in std::fs::read_dir(child.path())? {
                    let snapshot = snapshot?;
                    if snapshot.file_name() != "session.json" {
                        removable = false;
                        break;
                    }
                    if !std::fs::symlink_metadata(snapshot.path())?.is_file() {
                        removable = false;
                        break;
                    }
                    // A malformed or still-populated snapshot is never deleted.
                    if !crate::recovery::load(&snapshot.path())
                        .is_ok_and(|session| session.is_none_or(|session| !session.has_files()))
                    {
                        removable = false;
                        break;
                    }
                }
                if !removable { break; }
            } else if !ALLOWED.contains(&name) || !metadata.is_file() {
                removable = false;
                break;
            }
        }
        if removable { std::fs::remove_dir_all(entry.path())?; }
    }
    Ok(())
}

impl Runtime {
    fn start(root: PathBuf, requested: Option<&str>) -> std::io::Result<Self> {
        use fs2::FileExt;
        let _registry = registry_lock(&root)?;
        let live = live_windows(&root)?;
        let id = match requested {
            Some(id) => id.to_owned(),
            None => {
                let dir = tempfile::Builder::new().prefix("window-").tempdir_in(root.join("windows"))?;
                let id = dir.path().file_name().unwrap().to_string_lossy().into_owned();
                let _ = dir.keep();
                id
            }
        };
        let directory = root.join("windows").join(&id);
        std::fs::create_dir_all(&directory)?;
        // Preserve preferences from pre-registry installations on first launch.
        // Copy only settings, never revive all historical window snapshots.
        if requested.is_none() && !root.join("last-settings.json").exists() && root.join("app.ron").is_file() && !directory.join("app.ron").exists() {
            std::fs::copy(root.join("app.ron"), directory.join("app.ron"))?;
        }
        let lock = std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(directory.join("window.lock"))?;
        lock.try_lock_exclusive()?;
        let own_session = directory.join("recovery/session.json");
        let published = root.join("recovery/session.json");
        let restore = if requested.is_some() {
            // A menu/update window owns exactly its private snapshot.
            own_session.is_file().then(|| own_session.clone())
        } else if live.is_empty() {
            // A cold launch merges every snapshot left by the previous run
            // instead of trusting only the last window to close. A normal
            // close keeps its private snapshot, so tabs from a window that was
            // not the last one are still restored. The newest source supplies
            // the active tab and the tab order.
            let mut sources = Vec::new();
            if published.is_file() {
                sources.push(published.clone());
            }
            let mut windows: Vec<PathBuf> = std::fs::read_dir(root.join("windows"))?
                .filter_map(Result::ok)
                .map(|entry| entry.path().join("recovery/session.json"))
                .filter(|path| path.is_file() && path != &own_session)
                .collect();
            windows.sort_by_key(|path| {
                std::cmp::Reverse(std::fs::metadata(path).and_then(|m| m.modified()).ok())
            });
            sources.extend(windows);
            let merged = crate::recovery::merge(sources.into_iter().filter_map(|path| {
                crate::recovery::load(&path)
                    .ok()
                    .flatten()
                    .filter(|session| session.has_files())
            }));
            match merged {
                Some(session) => {
                    std::fs::create_dir_all(own_session.parent().unwrap())?;
                    crate::recovery::write_session(&own_session, &session)?;
                    Some(own_session.clone())
                }
                None => None,
            }
        } else {
            // Another window is live; never steal or delete its tabs.
            None
        };
        // A successful private claim becomes the only startup copy. Otherwise
        // a closed tab could survive in an older shared/crashed snapshot.
        let claimed = restore.as_ref().is_some_and(|path|
            crate::recovery::load(path).is_ok_and(|session| session.is_some_and(|s| s.has_files())));
        if live.is_empty() {
            if claimed {
                retire_inactive_snapshots(&root, std::slice::from_ref(&id))?;
            }
            // Tidy-up must never prevent the application from starting.
            let _ = prune_window_directories(&root, std::slice::from_ref(&id));
        }
        // Only a claimed snapshot may be removed. A second instance launched
        // while another window is live must keep the last published session so
        // the next cold start can still restore it.
        if claimed
            && std::fs::symlink_metadata(&published).is_ok_and(|m| m.is_file())
            && crate::recovery::load(&published).is_ok_and(|s| s.is_some()) {
            std::fs::remove_file(&published)?;
        }
        crate::recovery::prune_reload_archives(&published)?;
        Ok(Self { root, id, lock: Some(lock), restore, finished: false })
    }

    fn finish(&mut self, files: &[crate::LoadedFile], active: Option<u64>) -> std::io::Result<()> {
        if self.finished { return Ok(()); }
        let _registry = registry_lock(&self.root)?;
        let live = live_windows(&self.root)?;
        let others = live.iter().any(|id| id != &self.id);
        // Publish the final snapshot before releasing membership. Concurrent
        // closes are serialized, so exactly the last window wins.
        if !others {
            crate::recovery::store_buffers(&self.root.join("recovery/session.json"), files, active, true)?;
        }
        // Keep this window's own snapshot. The next cold launch merges every
        // surviving snapshot, so closing a window before the last one no
        // longer drops its tabs. The merged copy is written before these are
        // retired, so a crash cannot lose them.
        crate::recovery::store_buffers(&self.root.join("windows").join(&self.id).join("recovery/session.json"), files, active, true)?;
        self.finished = true;
        self.lock.take();
        Ok(())
    }
}

pub(crate) fn start_runtime() -> std::io::Result<()> {
    let root = eframe::storage_dir("LiteText").ok_or_else(|| std::io::Error::other("Application storage unavailable"))?;
    let runtime = Runtime::start(root, WINDOW.get().map(String::as_str))?;
    let _ = RUNTIME.set(std::sync::Mutex::new(runtime));
    Ok(())
}

pub(crate) fn restore_path() -> Option<PathBuf> {
    if let Some(runtime) = RUNTIME.get() { return runtime.lock().unwrap().restore.clone(); }
    crate::recovery::session_path()
}

pub(crate) fn other_windows_running() -> std::io::Result<bool> {
    let Some(runtime) = RUNTIME.get() else { return Ok(false); };
    let runtime = runtime.lock().unwrap();
    let _registry = registry_lock(&runtime.root)?;
    Ok(live_windows(&runtime.root)?.into_iter().any(|id| id != runtime.id))
}

pub(crate) fn finish(files: &[crate::LoadedFile], active: Option<u64>) -> std::io::Result<()> {
    if let Some(runtime) = RUNTIME.get() { runtime.lock().unwrap().finish(files, active)?; }
    Ok(())
}

pub(crate) fn finished() -> bool {
    RUNTIME.get().is_some_and(|r| r.lock().unwrap().finished)
}

/// Stable per-window namespace for scratch files that belong to this window.
/// Falls back to the process id before the runtime starts (and in unit tests).
pub(crate) fn window_key() -> Option<String> {
    if let Some(runtime) = RUNTIME.get() {
        return Some(runtime.lock().unwrap().id.clone());
    }
    WINDOW.get().cloned()
}

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
    if let Some(runtime) = RUNTIME.get() {
        let runtime = runtime.lock().unwrap();
        return Some(runtime.root.join("windows").join(&runtime.id));
    }
    directory_for(WINDOW.get()?)
}

pub(crate) fn append_arguments(command: &mut std::process::Command) {
    if let Some(runtime) = RUNTIME.get() {
        command.arg("--litetext-window").arg(&runtime.lock().unwrap().id);
        return;
    }
    if let Some(id) = WINDOW.get() {
        command.arg("--litetext-window").arg(id);
    }
}

pub(crate) fn seed() -> Option<crate::LiteTextApp> {
    let directory = directory()?;
    let path = directory.join("initial-settings.json");
    let fallback = eframe::storage_dir("LiteText")?.join("last-settings.json");
    serde_json::from_slice(&std::fs::read(if path.exists() { path } else { fallback }).ok()?).ok()
}

impl crate::LiteTextApp {
    fn dirty_local_ids(&self) -> Vec<u64> {
        self.files.iter().filter(|f| !f.path.as_os_str().is_empty() && f.has_unsaved_changes()).map(|f| f.id).collect()
    }

    pub(crate) fn needs_window_confirmation(&self, others: bool) -> bool {
        others && !self.dirty_local_ids().is_empty()
    }

    pub(crate) fn request_window_close(&mut self, ctx: &eframe::egui::Context) {
        self.finish_restore();
        use eframe::egui::ViewportCommand;
        if self.save_failure.is_some() || self.reload_task.is_some() {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            return;
        }
        if self.update_restarting {
            if !self.backup_or_report() { ctx.send_viewport_cmd(ViewportCommand::CancelClose); }
            return;
        }
        let others = match other_windows_running() {
            Ok(others) => others,
            Err(error) => {
                self.io_error = Some(format!("Unable to check running windows: {error}"));
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                return;
            }
        };
        if self.pending_window_close || (!self.window_close_ready && self.needs_window_confirmation(others)) {
            self.pending_window_close = true;
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            return;
        }
        if !self.backup_or_report() {
            self.window_close_ready = false;
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            return;
        }
        if let Err(error) = finish(&self.files, self.active_id) {
            self.window_close_ready = false;
            self.io_error = Some(format!("Unable to save final window session: {error}"));
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
        }
    }

    pub(crate) fn window_close_dialog(&mut self, ctx: &eframe::egui::Context) {
        if !self.pending_window_close || self.pending_save.is_some() || self.save_failure.is_some() || self.reload_task.is_some() || self.files.iter().any(|f| f.disk.pending.is_some()) { return; }
        let t = crate::tr(self.lang);
        let mut choice = 0;
        eframe::egui::Window::new(t.save).id(eframe::egui::Id::new("unsaved_window_close"))
            .collapsible(false).resizable(false).anchor(eframe::egui::Align2::CENTER_CENTER, [0.0,0.0])
            .show(ctx, |ui| {
                ui.label(t.close_unsaved);
                for id in self.dirty_local_ids() {
                    if let Some(file) = self.files.iter().find(|f| f.id == id) { ui.label(file.path.to_string_lossy()); }
                }
                ui.horizontal(|ui| {
                    if ui.button(t.save_and_close).clicked() { choice = 1; }
                    if ui.button(t.discard).clicked() { choice = 2; }
                    if ui.button(t.cancel).clicked() { choice = 3; }
                });
            });
        self.resolve_window_close(ctx, choice);
    }

    pub(crate) fn resolve_window_close(&mut self, ctx: &eframe::egui::Context, choice: u8) {
        if choice == 0 { return; }
        if choice == 3 { self.pending_window_close = false; self.window_close_ready = false; return; }
        let ids = self.dirty_local_ids();
        if choice == 1 && !ids.iter().all(|&id| self.save_file(id)) { return; }
        if choice == 2 {
            // Explicit discard must stay discarded even if another window exits
            // while this confirmation is open and this becomes the last window.
            let mut removed = Vec::new();
            self.files.retain(|f| {
                let discard = ids.contains(&f.id);
                if discard {
                    removed.push(f.clone());
                }
                !discard
            });
            for file in removed {
                self.retire_file(file);
            }
            if !self.files.iter().any(|f| Some(f.id) == self.active_id) { self.active_id = self.files.first().map(|f| f.id); }
        }
        self.pending_window_close = false;
        self.window_close_ready = true;
        ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Close);
    }

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
    fn startup_consumes_old_snapshots_without_touching_running_windows() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let mut app = crate::LiteTextApp::default();
        app.new_document();
        app.active_mut().unwrap().draft_text = Some("recover once".into());
        app.active_mut().unwrap().sync_draft();
        let published = root.join("recovery/session.json");
        let stale = root.join("windows/old/recovery/session.json");
        crate::recovery::store_buffers(&published, &app.files, app.active_id, true).unwrap();
        crate::recovery::store_buffers(&stale, &app.files, app.active_id, true).unwrap();
        let mut first = Runtime::start(root.to_owned(), None).unwrap();
        assert!(!published.exists());
        // The retired window directory is removed instead of left behind.
        assert!(!root.join("windows/old").exists());
        let own = first.restore.as_ref().unwrap().clone();
        assert!(crate::recovery::load(&own).unwrap().unwrap().has_files());
        let mut second = Runtime::start(root.to_owned(), Some("second")).unwrap();
        assert!(crate::recovery::load(&own).unwrap().unwrap().has_files());
        let second_session = root.join("windows/second/recovery/session.json");
        crate::recovery::store_buffers(&second_session, &app.files, app.active_id, true).unwrap();
        first.finish(&[], None).unwrap();
        // A non-last close no longer clears another window's snapshot.
        assert!(crate::recovery::load(&second_session).unwrap().unwrap().has_files());
        second.finish(&app.files, app.active_id).unwrap();
        let cold = Runtime::start(root.to_owned(), None).unwrap();
        let restored = crate::recovery::load(cold.restore.as_ref().unwrap()).unwrap().unwrap();
        assert!(restored.has_files());
        assert_eq!(restored.restore().len(), 1);
    }

    #[test]
    fn retiring_snapshots_preserves_unknown_files_and_symlink_targets() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let recovery = root.join("windows/old/recovery");
        std::fs::create_dir_all(&recovery).unwrap();
        let session = recovery.join("session.json");
        std::fs::write(&session, "unrecognized recovery data").unwrap();
        let notes = recovery.join("notes.txt");
        std::fs::write(&notes, "user notes").unwrap();
        retire_inactive_snapshots(root, &[]).unwrap();
        assert_eq!(std::fs::read_to_string(&session).unwrap(), "unrecognized recovery data");
        assert_eq!(std::fs::read_to_string(notes).unwrap(), "user notes");
        #[cfg(unix)]
        {
            let target = root.join("outside.json");
            crate::recovery::store_buffers(&target, &[], None, false).unwrap();
            let bytes = std::fs::read(&target).unwrap();
            let linked = root.join("windows/linked/recovery");
            std::fs::create_dir_all(&linked).unwrap();
            std::os::unix::fs::symlink(&target, linked.join("session.json")).unwrap();
            std::os::unix::fs::symlink(&target, recovery.join("before-reload-1-2.json")).unwrap();
            retire_inactive_snapshots(root, &[]).unwrap();
            assert_eq!(std::fs::read(target).unwrap(), bytes);
            assert!(recovery.join("before-reload-1-2.json").exists());
        }
    }

    #[test]
    fn simultaneous_closes_publish_one_complete_last_session() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_owned();
        let a = Runtime::start(root.clone(), Some("a")).unwrap();
        let b = Runtime::start(root.clone(), Some("b")).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        std::thread::scope(|scope| {
            for (mut runtime, content) in [(a, "A"), (b, "B")] {
                let barrier = barrier.clone();
                scope.spawn(move || {
                    let mut app = crate::LiteTextApp::default();
                    app.new_document();
                    app.active_mut().unwrap().draft_text = Some(content.into());
                    app.active_mut().unwrap().sync_draft();
                    barrier.wait();
                    runtime.finish(&app.files, app.active_id).unwrap();
                });
            }
        });
        assert!(live_windows(&root).unwrap().is_empty());
        let files = crate::recovery::load(&root.join("recovery/session.json")).unwrap().unwrap().restore();
        assert_eq!(files.len(), 1);
        assert!(matches!(files[0].draft_text.as_deref(), Some("A" | "B")));
    }

    #[test]
    fn failed_final_snapshot_keeps_membership_and_can_retry() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_owned();
        let mut runtime = Runtime::start(root.clone(), None).unwrap();
        std::fs::write(root.join("recovery"), "blocked").unwrap();
        assert!(runtime.finish(&[], None).is_err());
        assert!(!runtime.finished);
        assert_eq!(live_windows(&root).unwrap(), vec![runtime.id.clone()]);
        std::fs::remove_file(root.join("recovery")).unwrap();
        runtime.finish(&[], None).unwrap();
        assert!(runtime.finished);
        assert!(live_windows(&root).unwrap().is_empty());
    }

    #[test]
    fn failed_backup_cancels_close_without_discarding_buffers() {
        let temp = tempfile::tempdir().unwrap();
        let blocked = temp.path().join("blocked");
        std::fs::write(&blocked, "not a directory").unwrap();
        let mut app = crate::LiteTextApp {
            recovery_path_override: Some(blocked.join("session.json")),
            window_close_ready: true,
            ..Default::default()
        };
        app.new_document();
        app.active_mut().unwrap().draft_text = Some("keep this".into());
        let ctx = eframe::egui::Context::default();
        let output = ctx.run(Default::default(), |ctx| app.request_window_close(ctx));
        assert!(output.viewport_output[&eframe::egui::ViewportId::ROOT].commands.contains(&eframe::egui::ViewportCommand::CancelClose));
        assert!(app.io_error.is_some());
        assert!(!app.window_close_ready);
        assert_eq!(app.active().unwrap().draft_text.as_deref(), Some("keep this"));
    }

    #[test]
    fn a_crashed_snapshot_is_merged_with_the_clean_close() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_owned();
        let mut app = crate::LiteTextApp::default();
        app.new_document();
        app.active_mut().unwrap().draft_text = Some("previous session".into());
        let published = root.join("recovery/session.json");
        crate::recovery::store_buffers(&published, &app.files, app.active_id, true).unwrap();
        let mut cold = Runtime::start(root.clone(), None).unwrap();
        let claimed = cold.restore.as_ref().unwrap();
        assert_ne!(claimed, &published);
        assert!(crate::recovery::load(claimed).unwrap().unwrap().has_files());
        // A second window crashes during this run, before the clean close.
        let stale = root.join("windows/stale/recovery/session.json");
        crate::recovery::store_buffers(&stale, &app.files, app.active_id, true).unwrap();
        cold.finish(&[], None).unwrap();
        let next = Runtime::start(root, None).unwrap();
        let files = crate::recovery::load(next.restore.as_ref().unwrap()).unwrap().unwrap().restore();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].draft_text.as_deref(), Some("previous session"));
    }

    #[test]
    fn clean_close_and_crash_snapshots_merge_without_losing_tabs() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_owned();
        let published = root.join("recovery/session.json");
        let mut clean = crate::LiteTextApp::default();
        clean.new_document();
        clean.active_mut().unwrap().draft_text = Some("clean tab".into());
        crate::recovery::store_buffers(&published, &clean.files, clean.active_id, true).unwrap();
        let mut crashed = crate::LiteTextApp::default();
        crashed.new_document();
        crashed.active_mut().unwrap().draft_text = Some("newer unsaved crash content".into());
        crashed.active_mut().unwrap().sync_draft();
        let crashed_session = root.join("windows/crashed/recovery/session.json");
        crate::recovery::store_buffers(&crashed_session, &crashed.files, crashed.active_id, true).unwrap();
        let next = Runtime::start(root, None).unwrap();
        let files = crate::recovery::load(next.restore.as_ref().unwrap()).unwrap().unwrap().restore();
        assert_eq!(files.len(), 2);
        // The published snapshot is the newest source, so its tab comes first.
        assert_eq!(files[0].draft_text.as_deref(), Some("clean tab"));
        assert!(files.iter().any(|file| file.draft_text.as_deref() == Some("newer unsaved crash content")));
    }

    #[test]
    fn tabs_from_every_closed_window_are_merged() {
        for first_is_a in [true, false] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().to_owned();
            let mut a = Runtime::start(root.clone(), None).unwrap();
            let mut b = Runtime::start(root.clone(), Some("window-b")).unwrap();
            assert_eq!(live_windows(&root).unwrap().len(), 2);
            assert!(b.restore.is_none());
            let mut app_a = crate::LiteTextApp::default();
            app_a.new_document();
            app_a.active_mut().unwrap().draft_text = Some("temporary A".into());
            app_a.active_mut().unwrap().sync_draft();
            let mut app_b = crate::LiteTextApp::default();
            app_b.new_document();
            app_b.active_mut().unwrap().draft_text = Some("temporary B".into());
            app_b.active_mut().unwrap().sync_draft();
            if first_is_a {
                a.finish(&app_a.files, app_a.active_id).unwrap();
                b.finish(&app_b.files, app_b.active_id).unwrap();
            } else {
                b.finish(&app_b.files, app_b.active_id).unwrap();
                a.finish(&app_a.files, app_a.active_id).unwrap();
            }
            assert!(live_windows(&root).unwrap().is_empty());
            // Every normal close keeps its own snapshot until the next launch.
            for (runtime, app) in [(&a, &app_a), (&b, &app_b)] {
                let session = crate::recovery::load(&root.join("windows").join(&runtime.id).join("recovery/session.json")).unwrap().unwrap();
                assert_eq!(session.restore().len(), app.files.len());
            }
            let cold = Runtime::start(root.clone(), None).unwrap();
            let files = crate::recovery::load(cold.restore.as_ref().unwrap()).unwrap().unwrap().restore();
            assert_eq!(files.len(), 2);
            let first = if first_is_a { "temporary B" } else { "temporary A" };
            assert_eq!(files[0].draft_text.as_deref(), Some(first));
            assert!(files.iter().any(|file| file.draft_text.as_deref() == Some("temporary A")));
            assert!(files.iter().any(|file| file.draft_text.as_deref() == Some("temporary B")));
            let extra = Runtime::start(root, None).unwrap();
            assert!(extra.restore.is_none(), "a second launch while running must not restore the previous session");
        }
    }

    #[test]
    fn pruning_window_directories_keeps_unknown_content() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let empty = r#"{"version":2,"active_id":null,"files":[]}"#;
        std::fs::create_dir_all(root.join("windows/keepme/recovery")).unwrap();
        std::fs::write(root.join("windows/keepme/recovery/session.json"), empty).unwrap();
        std::fs::write(root.join("windows/keepme/notes.txt"), "user data").unwrap();
        std::fs::create_dir_all(root.join("windows/plain/recovery")).unwrap();
        std::fs::write(root.join("windows/plain/recovery/session.json"), empty).unwrap();
        std::fs::write(root.join("windows/plain/app.ron"), "settings").unwrap();
        let mut app = crate::LiteTextApp::default();
        app.new_document();
        app.active_mut().unwrap().draft_text = Some("still open".into());
        std::fs::create_dir_all(root.join("windows/crashed/recovery")).unwrap();
        crate::recovery::store_buffers(&root.join("windows/crashed/recovery/session.json"), &app.files, app.active_id, true).unwrap();
        std::fs::create_dir_all(root.join("windows/malformed/recovery")).unwrap();
        std::fs::write(root.join("windows/malformed/recovery/session.json"), "not json").unwrap();
        prune_window_directories(root, &[]).unwrap();
        assert!(root.join("windows/keepme").exists(), "unknown files keep the directory");
        assert!(!root.join("windows/plain").exists());
        assert!(root.join("windows/crashed").exists(), "a populated snapshot is never deleted");
        assert!(root.join("windows/malformed").exists(), "a damaged snapshot is preserved");
    }

    #[test]
    fn window_confirmation_only_for_modified_linked_files_and_choices_are_safe() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("local.txt");
        std::fs::write(&path, "original").unwrap();
        let ctx = eframe::egui::Context::default();
        let mut app = crate::LiteTextApp { recovery_path_override: Some(temp.path().join("session.json")), ..Default::default() };
        app.new_document();
        app.active_mut().unwrap().draft_text = Some("temporary must not prompt".into());
        app.active_mut().unwrap().sync_draft();
        assert!(!app.needs_window_confirmation(true));
        app.open_paths(vec![path.clone()]);
        assert!(!app.needs_window_confirmation(true));
        app.active_mut().unwrap().draft_text = Some("modified".into());
        app.active_mut().unwrap().sync_draft();
        assert!(app.needs_window_confirmation(true));
        assert!(!app.needs_window_confirmation(false));
        app.pending_window_close = true;
        app.resolve_window_close(&ctx, 3);
        assert!(!app.pending_window_close && !app.window_close_ready);
        assert_eq!(app.active().unwrap().draft_text.as_deref(), Some("modified"));
        app.pending_window_close = true;
        app.resolve_window_close(&ctx, 1);
        assert!(app.window_close_ready);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "modified");
        app.window_close_ready = false;
        app.active_mut().unwrap().draft_text = Some("discard this".into());
        app.active_mut().unwrap().sync_draft();
        app.pending_window_close = true;
        app.resolve_window_close(&ctx, 2);
        assert!(app.window_close_ready);
        assert_eq!(std::fs::read_to_string(path).unwrap(), "modified");
        assert_eq!(app.files.len(), 1);
        assert_eq!(app.files[0].draft_text.as_deref(), Some("temporary must not prompt"));
    }

    #[test]
    fn updater_restart_preserves_own_session_and_crash_restores_one_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let mut app = crate::LiteTextApp::default();
        app.new_document();
        app.active_mut().unwrap().draft_text = Some("unsaved after crash".into());
        app.active_mut().unwrap().sync_draft();
        let runtime = Runtime::start(temp.path().to_owned(), Some("window-restart")).unwrap();
        let snapshot = temp.path().join("windows/window-restart/recovery/session.json");
        crate::recovery::store_buffers(&snapshot, &app.files, app.active_id, true).unwrap();
        drop(runtime); // crash/update handoff, not a normal close
        let restarted = Runtime::start(temp.path().to_owned(), Some("window-restart")).unwrap();
        assert_eq!(restarted.restore, Some(snapshot.clone()));
        drop(restarted);
        let cold = Runtime::start(temp.path().to_owned(), None).unwrap();
        assert_ne!(cold.restore.as_ref(), Some(&snapshot));
        let restored = crate::recovery::load(cold.restore.as_ref().unwrap()).unwrap().unwrap().restore();
        assert_eq!(restored[0].draft_text.as_deref(), Some("unsaved after crash"));
        assert!(restore_path().is_some()); // production entry is also used by test builds
    }
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

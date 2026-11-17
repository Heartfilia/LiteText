//! Cancellable file reads and version-checked, staged periodic recovery.
use crate::{LiteTextApp, LoadedFile, RecoveryKey, ViewMode, editor, i18n::tr, linkage, recovery};
use eframe::egui;
use std::{
    collections::VecDeque,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
        mpsc,
    },
};

type RestoredSession = io::Result<(Vec<LoadedFile>, Option<u64>)>;

#[derive(Default)]
pub(crate) struct State {
    queue: VecDeque<PathBuf>,
    opening: Option<Opening>,
    pub(crate) recovery: Option<Recovering>,
    capturing: Option<Capturing>,
    disk: Option<mpsc::Receiver<Vec<DiskResult>>>,
    baselines: std::collections::HashMap<u64, (u64, Arc<Vec<u8>>)>,
    restoring: Option<mpsc::Receiver<RestoredSession>>,
    queued_total: usize,
    completed: usize,
    focus_owner: Option<u64>,
    allow_focus: bool,
}

struct DiskResult {
    id: u64,
    path: PathBuf,
    revision: (u64, u64),
    stamp: Option<(u64, std::time::SystemTime)>,
    external: Option<linkage::External>,
}

struct Opening {
    path: PathBuf,
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
    receiver: mpsc::Receiver<io::Result<LoadedFile>>,
    active_at_start: Option<u64>,
    phase: Arc<AtomicU8>,
    activate: bool,
}

pub(crate) struct Recovering {
    path: PathBuf,
    key: RecoveryKey,
    stamp: Option<(u64, std::time::SystemTime)>,
    receiver: mpsc::Receiver<io::Result<tempfile::NamedTempFile>>,
}

struct Capturing {
    path: PathBuf,
    key: RecoveryKey,
    stamp: Option<(u64, std::time::SystemTime)>,
    capture: recovery::Capture,
}

fn stamp(path: &Path) -> Option<(u64, std::time::SystemTime)> {
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.len(), metadata.modified().ok()?))
}

pub(crate) fn load_with_encoding(
    path: &Path,
    cancel: &AtomicBool,
    progress: &AtomicU64,
    total: &AtomicU64,
    explicit: Option<recovery::Encoding>,
) -> io::Result<LoadedFile> {
    load_with_progress(path, cancel, progress, total, &AtomicU8::new(0), explicit)
}

pub(crate) fn load_with_progress(
    path: &Path,
    cancel: &AtomicBool,
    progress: &AtomicU64,
    total: &AtomicU64,
    phase: &AtomicU8,
    explicit: Option<recovery::Encoding>,
) -> io::Result<LoadedFile> {
    load_inner(path, cancel, progress, total, phase, explicit, || {})
}

fn load_inner(
    path: &Path,
    cancel: &AtomicBool,
    progress: &AtomicU64,
    total: &AtomicU64,
    phase: &AtomicU8,
    explicit: Option<recovery::Encoding>,
    after_read: impl FnOnce(),
) -> io::Result<LoadedFile> {
    let _timing = crate::timing::phase("open.read_decode_index");
    if !std::fs::metadata(path)?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Not a regular file",
        ));
    }
    let mut source = std::fs::File::open(path)?;
    let metadata = source.metadata()?;
    let before = linkage::ReadStamp::file(&source)?;
    if linkage::ReadStamp::path(path)? != before {
        return Err(linkage::changed_during_load());
    }
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Not a regular file",
        ));
    }
    total.store(metadata.len(), Ordering::Relaxed);
    let mut bytes = Vec::new();
    let mut chunk = vec![0; 256 * 1024];
    let read_timing = crate::timing::phase("open.read");
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Loading cancelled",
            ));
        }
        let count = source.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..count]);
        progress.store(bytes.len() as u64, Ordering::Relaxed);
    }
    drop(read_timing);
    after_read();
    if linkage::ReadStamp::file(&source)? != before
        || linkage::ReadStamp::path(path).ok().as_ref() != Some(&before)
        || bytes.len() as u64 != metadata.len()
    {
        return Err(linkage::changed_during_load());
    }
    phase.store(1, Ordering::Relaxed);
    let decode_timing = crate::timing::phase("open.decode");
    let binary = explicit.is_none() && crate::detect_binary(&bytes);
    let (encoding, text) = if let Some(encoding) = explicit {
        (encoding, encoding.decode_explicit(&bytes)?)
    } else if binary {
        (recovery::Encoding::default(), String::new())
    } else {
        recovery::Encoding::decode(&bytes)
    };
    drop(decode_timing);
    phase.store(2, Ordering::Relaxed);
    let _index_timing = crate::timing::phase("open.index_baseline");
    let mut file = LoadedFile {
        id: 0,
        path: linkage::resolved(path),
        name: path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        saved_bytes: bytes.clone(),
        bytes,
        is_binary: binary,
        lines: if !binary && text.len() < crate::large_editor::THRESHOLD {
            text.split('\n').map(str::to_owned).collect()
        } else {
            Vec::new()
        },
        view: if binary {
            ViewMode::Hex
        } else {
            ViewMode::Text
        },
        language_override: None,
        character_count: text.chars().count(),
        line_ending: crate::line_endings::Counts::of(&text).preferred(Default::default()),
        draft_text: (!binary).then_some(text),
        editor_state: editor::EditorState::default(),
        encoding,
        saved_encoding: encoding,
        saved_large_revision: None,
        bytes_dirty: false,
        content_revision: 0,
        source_path: None,
        saved_generation: 0,
        disk: linkage::DiskState {
            checked: false,
            stamp: stamp(path),
            pending: None,
            loaded: Some(before),
        },
    };
    // Build expensive logical-line indices off the UI thread, with no native history to migrate.
    if let Some(text) = file
        .draft_text
        .as_deref()
        .filter(|text| crate::large_editor::needs_windowing(text))
    {
        file.editor_state.large.prepare_loaded(text);
        file.editor_state.large.set_encoding(text, encoding);
        file.saved_large_revision = Some(0);
    }
    file.prepare_changes();
    linkage::validate_loaded(&file)?;
    if cancel.load(Ordering::Relaxed) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Loading cancelled",
        ));
    }
    Ok(file)
}

impl LiteTextApp {
    pub(crate) fn restoring(&self) -> bool {
        self.background_io.restoring.is_some()
    }

    pub(crate) fn start_restore(&mut self, path: PathBuf) {
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let _timing = crate::timing::phase("restore.load_decode_index");
            let result = recovery::load(&path).map(|session| {
                let Some(session) = session else {
                    return (Vec::new(), None);
                };
                let active = session.active_id;
                let mut files = session.restore();
                for file in &mut files {
                    let modified = file.is_modified();
                    if let Some(text) = &file.draft_text
                        && crate::large_editor::needs_windowing(text)
                    {
                        file.editor_state.large.prepare_loaded(text);
                        file.editor_state.large.set_encoding(text, file.encoding);
                        file.saved_large_revision = (!modified).then_some(0);
                    }
                    file.prepare_changes();
                }
                (files, active)
            });
            let _ = sender.send(result);
        });
        self.background_io.restoring = Some(receiver);
    }

    fn apply_restored(&mut self, result: RestoredSession) {
        match result {
            Ok((files, active)) => {
                self.files = files;
                self.active_id = active
                    .filter(|id| self.files.iter().any(|file| file.id == *id))
                    .or_else(|| self.files.first().map(|file| file.id));
                self.next_file_id = self.files.iter().map(|file| file.id).max().unwrap_or(0) + 1;
                self.remember_recovered_versions();
                self.last_recovery = Some(std::time::Instant::now());
            }
            Err(error) => {
                self.io_error = Some(format!("{}: {error}", tr(self.lang).recovery_failed))
            }
        }
    }

    /// Closing/saving during startup must never publish an empty placeholder session.
    pub(crate) fn finish_restore(&mut self) {
        if let Some(receiver) = self.background_io.restoring.take() {
            let result = receiver
                .recv()
                .unwrap_or_else(|_| Err(io::Error::other("Session restore worker stopped")));
            self.apply_restored(result);
        }
    }
    pub(crate) fn prune_io_caches(&mut self) {
        self.background_io
            .baselines
            .retain(|id, _| self.files.iter().any(|file| file.id == *id));
    }
    pub(crate) fn cancel_open(&mut self) {
        if let Some(job) = self.background_io.opening.take() {
            job.cancel.store(true, Ordering::Relaxed);
        }
        self.background_io.queue.clear();
    }
    pub(crate) fn skip_open(&mut self) {
        if let Some(job) = self.background_io.opening.take() {
            job.cancel.store(true, Ordering::Relaxed);
            self.background_io.completed += 1;
        }
    }
    pub(crate) fn opening_files(&self) -> bool {
        self.background_io.opening.is_some() || !self.background_io.queue.is_empty()
    }
    pub(crate) fn queue_open(&mut self, paths: Vec<PathBuf>) {
        self.queue_open_inner(paths, true);
    }
    pub(crate) fn retry_open_errors(&mut self) {
        let paths = std::mem::take(&mut self.open_errors)
            .into_iter()
            .map(|error| error.path)
            .collect();
        self.queue_open_inner(paths, false);
    }
    fn queue_open_inner(&mut self, paths: Vec<PathBuf>, activate_existing: bool) {
        if !self.opening_files() {
            self.background_io.completed = 0;
            self.background_io.queued_total = 0;
            self.background_io.focus_owner = self.active_id;
            self.background_io.allow_focus = activate_existing;
        }
        for path in paths {
            let path = linkage::resolved(&path);
            if let Some(file) = self.files.iter().find(|file| {
                !file.path.as_os_str().is_empty() && linkage::resolved(&file.path) == path
            }) {
                if activate_existing {
                    self.active_id = Some(file.id);
                }
            } else if !self.background_io.queue.contains(&path)
                && !self
                    .background_io
                    .opening
                    .as_ref()
                    .is_some_and(|job| job.path == path)
            {
                self.background_io.queue.push_back(path);
                self.background_io.queued_total += 1;
            }
        }
    }

    fn recovery_version_key(&self) -> RecoveryKey {
        (
            self.active_id,
            self.files
                .iter()
                .map(|file| {
                    (
                        file.id,
                        file.path.clone(),
                        file.name.clone(),
                        file.view,
                        file.content_revision,
                        file.editor_state.large.revision,
                        file.language_override.clone(),
                    )
                })
                .collect(),
        )
    }

    pub(crate) fn start_background_recovery(&mut self) {
        if self.restoring()
            || self.background_io.capturing.is_some()
            || self.background_io.recovery.is_some()
            || crate::new_window::finished()
        {
            return;
        }
        self.last_recovery = Some(std::time::Instant::now());
        let Some(path) = self.recovery_path() else {
            self.io_error = Some(tr(self.lang).recovery_failed.to_owned());
            return;
        };
        let key = self.recovery_version_key();
        let stamp = stamp(&path);
        if self.recovery_key.as_ref() == Some(&key) && self.recovery_stamp == stamp {
            return;
        }
        self.background_io.capturing = Some(Capturing {
            path,
            key,
            stamp,
            capture: recovery::Capture::new(&self.files, self.active_id),
        });
    }

    pub(crate) fn invalidate_recovery_worker(&mut self) {
        self.background_io.capturing = None;
        self.background_io.recovery = None;
    }

    pub(crate) fn poll_background_io(&mut self, ctx: &egui::Context) {
        if let Some(receiver) = self.background_io.restoring.take() {
            match receiver.try_recv() {
                Ok(result) => self.apply_restored(result),
                Err(mpsc::TryRecvError::Empty) => {
                    self.background_io.restoring = Some(receiver);
                    ctx.request_repaint_after(std::time::Duration::from_millis(30));
                    return;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.io_error = Some("Session restore worker stopped".into())
                }
            }
        }
        self.prune_io_caches();
        if crate::new_window::finished() {
            self.cancel_open();
            self.invalidate_recovery_worker();
            return;
        }
        if let Some(mut pending) = self.background_io.capturing.take() {
            if pending.key != self.recovery_version_key() || pending.stamp != stamp(&pending.path) {
                self.last_recovery = None;
            } else if pending.capture.step(&self.files, 1024 * 1024) {
                let session = pending.capture.finish();
                let (sender, receiver) = mpsc::channel();
                let target = pending.path.clone();
                std::thread::spawn(move || {
                    let _ = sender.send(recovery::stage(&target, &session));
                });
                self.background_io.recovery = Some(Recovering {
                    path: pending.path,
                    key: pending.key,
                    stamp: pending.stamp,
                    receiver,
                });
            } else {
                self.background_io.capturing = Some(pending);
            }
        }
        if let Some(receiver) = self.background_io.disk.take() {
            match receiver.try_recv() {
                Ok(results) => {
                    for result in results {
                        if let Some(file) = self.files.iter_mut().find(|file| {
                            file.id == result.id
                                && file.path == result.path
                                && (file.content_revision, file.editor_state.large.revision)
                                    == result.revision
                                && file.disk.pending.is_none()
                        }) {
                            file.disk.checked = true;
                            file.disk.stamp = result.stamp;
                            file.disk.pending = result.external;
                        }
                    }
                }
                Err(mpsc::TryRecvError::Empty) => self.background_io.disk = Some(receiver),
                Err(mpsc::TryRecvError::Disconnected) => {}
            }
        }
        if let Some(job) = self.background_io.recovery.take() {
            match job.receiver.try_recv() {
                Ok(result)
                    if job.key == self.recovery_version_key()
                        && job.stamp == stamp(&job.path)
                        && !crate::new_window::finished() =>
                {
                    match result.and_then(|temp| {
                        recovery::publish(
                            &job.path,
                            temp,
                            self.recovery_key.is_none() || self.recovery_stamp != job.stamp,
                        )
                    }) {
                        Ok(()) => {
                            self.recovery_key = Some(job.key);
                            self.recovery_stamp = stamp(&job.path);
                            self.remember_recovered_versions();
                        }
                        Err(error) => {
                            self.recovery_versions.clear();
                            self.io_error =
                                Some(format!("{}: {error}", tr(self.lang).recovery_failed));
                        }
                    }
                }
                Ok(_) => {
                    self.last_recovery = None;
                }
                Err(mpsc::TryRecvError::Empty) => self.background_io.recovery = Some(job),
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.last_recovery = None;
                }
            }
        }
        if let Some(job) = self.background_io.opening.take() {
            match job.receiver.try_recv() {
                Ok(result) if !job.cancel.load(Ordering::Relaxed) => {
                    self.background_io.completed += 1;
                    let result = result.and_then(|file| {
                        linkage::validate_loaded(&file)?;
                        Ok(file)
                    });
                    match result {
                        Ok(mut file) => {
                            // Recheck identity: another operation may have opened/saved this target meanwhile.
                            if !self.files.iter().any(|other| other.path == file.path) {
                                file.id = self.next_file_id;
                                self.next_file_id += 1;
                                if job.activate && self.active_id == job.active_at_start {
                                    self.active_id = Some(file.id);
                                    self.background_io.focus_owner = Some(file.id);
                                    self.selection = None;
                                    self.drag_anchor = None;
                                    self.inspector_popup = None;
                                    self.focus_editor = Some(file.id);
                                }
                                self.recent_files.retain(|path| path != &file.path);
                                self.recent_files.insert(0, file.path.clone());
                                self.recent_files.truncate(10);
                                self.files.push(file);
                            }
                        }
                        Err(error) => self.open_errors.push(crate::file_actions::OpenError {
                            path: job.path,
                            reason: error.to_string(),
                        }),
                    }
                }
                Ok(_) => {}
                Err(mpsc::TryRecvError::Empty) => self.background_io.opening = Some(job),
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.background_io.completed += 1;
                    self.open_errors.push(crate::file_actions::OpenError {
                        path: job.path,
                        reason: "File loading worker stopped".into(),
                    })
                }
            }
        }
        if self.background_io.opening.is_none()
            && let Some(path) = self.background_io.queue.pop_front()
        {
            let cancel = Arc::new(AtomicBool::new(false));
            let progress = Arc::new(AtomicU64::new(0));
            let total = Arc::new(AtomicU64::new(0));
            let phase = Arc::new(AtomicU8::new(0));
            let (sender, receiver) = mpsc::channel();
            let (target, cancelled, read, size) = (
                path.clone(),
                cancel.clone(),
                progress.clone(),
                total.clone(),
            );
            let stage = phase.clone();
            std::thread::spawn(move || {
                let _ = sender.send(load_with_progress(
                    &target, &cancelled, &read, &size, &stage, None,
                ));
            });
            self.background_io.opening = Some(Opening {
                path,
                cancel,
                progress,
                total,
                receiver,
                active_at_start: self.active_id,
                phase,
                activate: self.background_io.allow_focus
                    && self.active_id == self.background_io.focus_owner,
            });
        }
        if self.background_io.opening.is_some()
            || self.background_io.capturing.is_some()
            || self.background_io.recovery.is_some()
            || self.background_io.disk.is_some()
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(30));
        }
    }

    pub(crate) fn start_disk_checks(&mut self, force: bool) {
        if self.background_io.disk.is_some() {
            return;
        }
        let mut files = Vec::new();
        for file in self
            .files
            .iter()
            .filter(|file| !file.path.as_os_str().is_empty() && file.disk.pending.is_none())
        {
            let baseline = self
                .background_io
                .baselines
                .entry(file.id)
                .or_insert_with(|| (file.saved_generation, Arc::new(file.saved_bytes.clone())));
            if baseline.0 != file.saved_generation {
                *baseline = (file.saved_generation, Arc::new(file.saved_bytes.clone()));
            }
            files.push((
                file.id,
                file.path.clone(),
                (file.content_revision, file.editor_state.large.revision),
                file.disk.clone(),
                baseline.1.clone(),
            ));
        }
        if files.is_empty() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let results = files
                .into_iter()
                .map(|(id, path, revision, previous, baseline)| {
                    let stamp = stamp(&path);
                    let external = if !force
                        && previous.checked
                        && stamp.is_some()
                        && stamp == previous.stamp
                    {
                        None
                    } else {
                        match std::fs::read(&path) {
                            Ok(bytes) if bytes.as_slice() != baseline.as_slice() => {
                                Some(linkage::External::Changed)
                            }
                            Ok(_) => None,
                            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                                Some(linkage::External::Missing)
                            }
                            Err(error) => Some(linkage::External::Unreadable(error.to_string())),
                        }
                    };
                    DiskResult {
                        id,
                        path,
                        revision,
                        stamp,
                        external,
                    }
                })
                .collect();
            let _ = sender.send(results);
        });
        self.background_io.disk = Some(receiver);
    }

    pub(crate) fn background_io_panel(&mut self, ctx: &egui::Context) {
        if self.restoring() {
            egui::Window::new(if self.lang == crate::Lang::ZhCn {
                "正在恢复工作区"
            } else {
                "Restoring workspace"
            })
            .id(egui::Id::new("restore_progress"))
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.spinner();
            });
        }
        if let Some(job) = &self.background_io.opening {
            let zh = self.lang == crate::Lang::ZhCn;
            let mut cancel = false;
            let mut skip = false;
            egui::Window::new(if zh {
                "正在打开文件"
            } else {
                "Opening file"
            })
            .id(egui::Id::new("loading_file"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::RIGHT_BOTTOM, [-16.0, -40.0])
            .show(ctx, |ui| {
                ui.set_max_width(300.0);
                ui.add(egui::Label::new(job.path.to_string_lossy()).wrap());
                ui.label(if zh {
                    format!(
                        "第 {} / {} 个 · 还有 {} 个等待",
                        self.background_io.completed + 1,
                        self.background_io.queued_total,
                        self.background_io.queue.len()
                    )
                } else {
                    format!(
                        "File {} / {} · {} queued",
                        self.background_io.completed + 1,
                        self.background_io.queued_total,
                        self.background_io.queue.len()
                    )
                });
                let total = job.total.load(Ordering::Relaxed);
                let done = job.progress.load(Ordering::Relaxed);
                if job.phase.load(Ordering::Relaxed) == 0 {
                    ui.add(
                        egui::ProgressBar::new(if total == 0 {
                            0.0
                        } else {
                            done as f32 / total as f32
                        })
                        .text(format!(
                            "{}: {} / {}",
                            if zh { "读取中" } else { "Reading" },
                            crate::format_size(done),
                            crate::format_size(total)
                        )),
                    );
                } else {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(match (job.phase.load(Ordering::Relaxed), zh) {
                            (1, true) => "正在解码",
                            (1, false) => "Decoding",
                            (_, true) => "正在建立索引",
                            _ => "Indexing",
                        });
                    });
                }
                ui.horizontal(|ui| {
                    skip = ui
                        .button(if zh {
                            "跳过当前文件"
                        } else {
                            "Skip current"
                        })
                        .clicked();
                    cancel = ui
                        .button(if zh { "取消全部" } else { "Cancel all" })
                        .clicked();
                });
            });
            if cancel {
                self.cancel_open();
            } else if skip {
                self.skip_open();
                ctx.request_repaint();
            }
        }
    }
}

#[cfg(test)]
#[path = "background_io_tests.rs"]
mod tests;

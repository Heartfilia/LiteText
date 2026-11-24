//! On-disk mirrors for untitled drafts and unsaved edits of real files.
//!
//! Untitled documents are written to a plain-text file in a user-configurable
//! directory so their content survives a crash and can be found with any file
//! manager. Unsaved real-file edits are copied below `edits/<window>/`; the
//! original is only replaced by an explicit Save.
//!
//! A mirror is removed when its tab is explicitly closed, saved, or returned
//! to the saved content. Normal application exit deliberately keeps every
//! mirror so the drafts are reopened on the next launch.

use crate::{LoadedFile, LiteTextApp, ViewMode, i18n::tr, recovery};
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// Do not mirror very large untitled buffers; recovery snapshots still cover
/// them and reading a multi-gigabyte draft on startup would block the window.
const MAX_SCAN_BYTES: u64 = 64 * 1024 * 1024;
/// Unsaved edits to real files are mirrored too, but above this size the
/// inline session snapshot is smaller than another full copy on disk.
const MAX_EDIT_MIRROR_BYTES: usize = 8 * 1024 * 1024;
/// Coalesce keystrokes so mirroring does not write on every character.
const DEBOUNCE: Duration = Duration::from_millis(600);

#[derive(Default)]
pub(crate) struct State {
    last_flush: Option<Instant>,
    /// Content versions already present in each mirror.
    written: HashMap<u64, (u64, u64)>,
    /// Last `.litetext-drafts.json` bytes written, to avoid rewriting it every
    /// frame when no metadata changed.
    meta_written: Option<Vec<u8>>,
    last_error: Option<String>,
}

/// One file found in the scratch directory.
pub(crate) struct Draft {
    pub path: PathBuf,
    pub name: String,
    pub text: String,
    pub language_override: Option<String>,
    pub pinned: bool,
    pub markdown_mode: Option<crate::markdown::Mode>,
}

/// Hidden metadata beside the drafts. The session still carries the same
/// fields, but a lost/overwritten session must not reset a draft's pin,
/// language or Markdown view.
const DRAFT_META_FILE: &str = ".litetext-drafts.json";

#[derive(Clone, Default, Serialize, Deserialize)]
struct DraftMeta {
    #[serde(default)]
    pinned: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    markdown_mode: Option<crate::markdown::Mode>,
}

fn read_draft_meta(root: &Path) -> HashMap<String, DraftMeta> {
    std::fs::read(root.join(DRAFT_META_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Persistent default: an app-data subdirectory rather than an OS temporary
/// directory, so a normal exit or reboot never removes drafts.
pub(crate) fn default_root() -> Option<PathBuf> {
    #[cfg(not(test))]
    {
        eframe::storage_dir("LiteText").map(|root| root.join("drafts"))
    }
    #[cfg(test)]
    {
        None
    }
}

pub(crate) fn scan(root: Option<&Path>) -> Vec<Draft> {
    let Some(root) = root else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(root) else { return Vec::new() };
    let meta = read_draft_meta(root);
    let mut drafts = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(file_name) = name.to_str() else { continue };
        // Atomic writes stage dot-prefixed temporary files in this directory.
        if file_name.starts_with('.') || file_name.ends_with(".tmp") { continue; }
        // App metadata may live beside the drafts if the user picks the
        // app-data root itself; never open it as a draft.
        if matches!(
            file_name,
            "session.json" | "preferences.json" | "last-settings.json" | "window.json" | "windows.lock"
        ) || file_name.starts_with("before-reload-")
        {
            continue;
        }
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) { continue; }
        let Ok(metadata) = entry.metadata() else { continue };
        if metadata.len() == 0 || metadata.len() > MAX_SCAN_BYTES { continue; }
        let path = entry.path();
        let Ok(bytes) = std::fs::read(&path) else { continue };
        if crate::detect_binary(&bytes) { continue; }
        let (_, text) = recovery::Encoding::decode(&bytes);
        let entry = meta.get(file_name);
        drafts.push(Draft {
            name: display_name(file_name),
            language_override: entry
                .and_then(|meta| meta.language.clone())
                .or_else(|| markdown_override(&path)),
            pinned: entry.is_some_and(|meta| meta.pinned),
            markdown_mode: entry.and_then(|meta| meta.markdown_mode),
            path,
            text,
        });
    }
    drafts.sort_by(|a, b| a.path.cmp(&b.path));
    drafts
}

fn display_name(file_name: &str) -> String {
    file_name
        .strip_suffix(".txt")
        .unwrap_or(file_name)
        .to_owned()
}

fn markdown_override(path: &Path) -> Option<String> {
    const EXTENSIONS: &[&str] = &["md", "mdown", "markdown", "markdn"];
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            EXTENSIONS
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
        })
        .then(|| "Markdown".to_owned())
}

/// File-system-safe name, keeping CJK titles readable for manual recovery.
fn sanitize(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for (length, character) in name.chars().enumerate() {
        if length >= 80 { break; }
        if matches!(character, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
            || character.is_control()
        {
            out.push('_');
        } else {
            out.push(character);
        }
    }
    let trimmed = out.trim().trim_end_matches('.').trim();
    if trimmed.is_empty() { "untitled".to_owned() } else { trimmed.to_owned() }
}

fn mirror_file_name(name: &str, markdown: bool) -> String {
    let base = sanitize(name);
    let path = Path::new(&base);
    let extension = path.extension().and_then(|extension| extension.to_str());
    let managed = extension.is_some_and(|extension| {
        extension.eq_ignore_ascii_case("txt") || extension.eq_ignore_ascii_case("md")
    });
    // Files the user placed in the directory keep their own extension; `.txt`
    // and `.md` are managed so a Markdown override can rename the mirror.
    if extension.is_some() && !managed {
        return base;
    }
    let stem = if managed {
        path.file_stem().and_then(|stem| stem.to_str()).unwrap_or(&base)
    } else {
        &base
    };
    if markdown {
        format!("{stem}.md")
    } else {
        format!("{stem}.txt")
    }
}

/// Recognize both locales and legacy collision suffixes such as `1 (2)`.
/// Such suffixes still reserve the original untitled number.
pub(crate) fn untitled_number(name: &str) -> Option<u64> {
    let stem = draft_stem(name);
    [crate::tr(crate::Lang::ZhCn).untitled, crate::tr(crate::Lang::En).untitled]
        .into_iter().find_map(|prefix| {
            let rest = stem.strip_prefix(&format!("{prefix} "))?;
            let number = rest.split_once(" (").map_or(rest, |(number, suffix)| {
                if suffix.strip_suffix(')').is_some_and(|n| n.parse::<u64>().is_ok()) { number } else { rest }
            });
            number.parse::<u64>().ok().filter(|number| *number > 0)
        })
}

/// Pick a path that does not overwrite another window's draft.
fn allocate(root: &Path, name: &str, markdown: bool) -> PathBuf {
    let file_name = mirror_file_name(name, markdown);
    let candidate = root.join(&file_name);
    if !candidate.exists() { return candidate; }
    let path = Path::new(&file_name);
    let stem = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or("untitled");
    let extension = path.extension().and_then(|extension| extension.to_str()).unwrap_or("txt");
    for index in 2..10_000 {
        let candidate = root.join(format!("{stem} ({index}).{extension}"));
        if !candidate.exists() { return candidate; }
    }
    root.join(format!("{stem}-{}.{extension}", std::process::id()))
}

/// Unsaved edits of real files live in a window-scoped subdirectory. It is
/// never scanned as drafts; only the session record points at these mirrors.
fn edits_directory(root: &Path) -> PathBuf {
    let namespace = crate::new_window::window_key()
        .unwrap_or_else(|| format!("process-{}", std::process::id()));
    root.join("edits").join(namespace)
}

/// Move mirrors left by windows that are gone into the flat `edits/` directory
/// (keeping the newest copy per mirror name) and remove the now-empty
/// per-window directories. Referenced mirrors stay where they are until the
/// next flush moves them with their tab.
fn maintain_edits(root: &Path, referenced: &std::collections::HashSet<PathBuf>) -> io::Result<()> {
    let edits = root.join("edits");
    let Ok(entries) = std::fs::read_dir(&edits) else { return Ok(()) };
    for entry in entries.flatten() {
        let directory = entry.path();
        let Ok(kind) = entry.file_type() else { continue };
        if !kind.is_dir() {
            continue;
        }
        let mut retain = false;
        for child in std::fs::read_dir(&directory)?.flatten() {
            let path = child.path();
            if !child.file_type().is_ok_and(|kind| kind.is_file()) {
                retain = true;
                continue;
            }
            if referenced.contains(&crate::linkage::resolved(&path)) {
                retain = true;
                continue;
            }
            let Some(name) = path.file_name() else { continue };
            let target = edits.join(name);
            match std::fs::metadata(&target) {
                Ok(existing) if existing.is_file() => {
                    let current = existing.modified().ok();
                    let candidate = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
                    if candidate > current {
                        std::fs::rename(&path, &target)?;
                    } else {
                        remove(&path)?;
                    }
                }
                _ => std::fs::rename(&path, &target)?,
            }
        }
        if !retain {
            let _ = std::fs::remove_dir(&directory);
        }
    }
    Ok(())
}

/// A mirror that moved to a new window namespace leaves its old directory
/// empty; remove it so each launch does not accumulate directories.
fn remove_empty_edit_dirs(root: &Path) {
    let edits = root.join("edits");
    let Ok(entries) = std::fs::read_dir(&edits) else { return };
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        if std::fs::read_dir(entry.path()).is_ok_and(|mut children| children.next().is_none()) {
            let _ = std::fs::remove_dir(entry.path());
        }
    }
}

fn path_hash(path: &Path) -> u64 {
    // FNV-1a keeps the mirror name stable across restarts without pulling in
    // a hashing dependency.
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Readable, collision-resistant name for a real file's edit mirror.
fn edit_mirror_name(name: &str, path: &Path) -> String {
    let base = sanitize(name);
    let base_path = Path::new(&base);
    let stem = base_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("untitled");
    let extension = base_path
        .extension()
        .and_then(|extension| extension.to_str())
        .filter(|extension| {
            !extension.is_empty()
                && extension.len() <= 12
                && extension.chars().all(|c| c.is_ascii_alphanumeric())
        })
        .unwrap_or("txt");
    format!("{stem}-{:016x}.{extension}", path_hash(path))
}

/// Keep a mirror's managed extension in sync with the Markdown override so a
/// draft restored from disk still opens in the Markdown view. Files the user
/// put in the directory keep their own extension.
fn extension_matches(path: &Path, markdown: bool) -> bool {
    let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
        return true;
    };
    match extension.to_ascii_lowercase().as_str() {
        "md" => markdown,
        "txt" => !markdown,
        _ => true,
    }
}

pub(crate) fn remove(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

impl LoadedFile {
    /// Untitled scratch documents have no user path and no detached source.
    pub(crate) fn is_untitled(&self) -> bool {
        self.path.as_os_str().is_empty() && self.source_path.is_none()
    }

    /// Mirrored untitled drafts are authoritative on disk, so session recovery
    /// must not duplicate them. Used by tests to distinguish draft mirrors from
    /// real-file edit mirrors.
    #[cfg(test)]
    pub(crate) fn is_scratch_mirror(&self) -> bool {
        self.is_untitled() && self.scratch_path.is_some()
    }
}

/// Tab titles for drafts restored from the directory keep a `.txt`/`.md`
/// suffix on some paths; comparing stems matches the same tab either way.
fn draft_stem(name: &str) -> &str {
    let path = Path::new(name);
    let managed = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("txt") || extension.eq_ignore_ascii_case("md")
        });
    if managed {
        path.file_stem().and_then(|stem| stem.to_str()).unwrap_or(name)
    } else {
        name
    }
}

fn wants_markdown(file: &LoadedFile) -> bool {
    file.language_override
        .as_deref()
        .is_some_and(|language| language.to_ascii_lowercase().contains("markdown"))
}

impl LiteTextApp {
    pub(crate) fn next_untitled_number(&self) -> u64 {
        let mut used: std::collections::HashSet<u64> = self.files.iter()
            .filter(|file| file.is_untitled())
            .filter_map(|file| untitled_number(&file.name)).collect();
        // A cached draft may belong to another window or still be restoring in
        // the background. Its name must be reserved even if not in this window.
        if let Some(root) = self.scratch_root()
            && let Ok(entries) = std::fs::read_dir(root)
        {
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|kind| kind.is_file())
                    && let Some(number) = entry.file_name().to_str().and_then(untitled_number)
                { used.insert(number); }
            }
        }
        (1..).find(|number| !used.contains(number)).unwrap()
    }

    pub(crate) fn scratch_root(&self) -> Option<PathBuf> {
        self.scratch_dir
            .clone()
            .filter(|directory| !directory.as_os_str().is_empty())
            .or_else(default_root)
    }

    fn scratch_needs_write(&self, file: &LoadedFile, root: &Path) -> bool {
        let revision = (file.content_revision, file.editor_state.large.revision);
        if file.is_untitled() {
            let Some(text) = file.draft_text.as_deref() else { return false };
            if text.is_empty() { return file.scratch_path.is_some(); }
            return match &file.scratch_path {
                None => true,
                Some(path) => {
                    path.parent() != Some(root)
                        || !extension_matches(path, wants_markdown(file))
                        || self.scratch.written.get(&file.id) != Some(&revision)
                }
            };
        }
        // Detached buffers keep using the inline session fallback; retire any
        // mirror they inherited before the original file was detached.
        if file.path.as_os_str().is_empty() { return file.scratch_path.is_some(); }
        let wanted = file.draft_text.is_some()
            && file.encoding != recovery::Encoding::Lossy
            && file.has_unsaved_changes()
            && file.byte_len() <= MAX_EDIT_MIRROR_BYTES;
        if !wanted { return file.scratch_path.is_some(); }
        match &file.scratch_path {
            None => true,
            Some(path) => {
                path.parent() != Some(edits_directory(root).as_path())
                    || self.scratch.written.get(&file.id) != Some(&revision)
            }
        }
    }

    /// Whether a current mirror still has a pending rewrite. Recovery must wait
    /// for the debounce so a snapshot never points at stale mirror content. A
    /// first-time mirror write has no path yet and can safely fall back to an
    /// inline snapshot instead of blocking recovery.
    pub(crate) fn scratch_stale(&self) -> bool {
        let Some(root) = self.scratch_root() else { return false };
        self.files.iter().any(|file| {
            file.scratch_path.is_some() && self.scratch_needs_write(file, &root)
        })
    }

    /// Persist per-draft pin/language/Markdown state beside the drafts. This is
    /// the fallback when the session snapshot is missing or was written by an
    /// older build; a present session still overrides it.
    pub(crate) fn flush_draft_meta(&mut self) {
        let Some(root) = self.scratch_root() else { return };
        let mut meta: std::collections::BTreeMap<String, DraftMeta> = Default::default();
        for file in &self.files {
            if !file.is_untitled() {
                continue;
            }
            let Some(path) = &file.scratch_path else { continue };
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            meta.insert(
                name.to_owned(),
                DraftMeta {
                    pinned: file.pinned,
                    language: file.language_override.clone(),
                    markdown_mode: Some(file.markdown_mode),
                },
            );
        }
        let Ok(bytes) = serde_json::to_vec(&meta) else { return };
        if self.scratch.meta_written.as_deref() == Some(bytes.as_slice()) {
            return;
        }
        match recovery::atomic_write(&root.join(DRAFT_META_FILE), &bytes) {
            Ok(()) => self.scratch.meta_written = Some(bytes),
            Err(error) => self.report_scratch_error(&error),
        }
    }

    /// Startup housekeeping for the scratch directory: persist draft metadata
    /// and flatten edit mirrors left in old per-window directories.
    pub(crate) fn scratch_maintenance(&mut self) {
        self.flush_draft_meta();
        let Some(root) = self.scratch_root() else { return };
        let referenced: std::collections::HashSet<PathBuf> = self
            .files
            .iter()
            .filter_map(|file| file.scratch_path.as_deref())
            .map(crate::linkage::resolved)
            .collect();
        if let Err(error) = maintain_edits(&root, &referenced) {
            self.report_scratch_error(&error);
        }
    }

    /// Write changed untitled buffers and unsaved real-file edits. A `None`
    /// context is used by exit and synchronous recovery paths; otherwise the
    /// caller gets woken after the debounce interval so the last keystroke is
    /// not delayed to the next unrelated repaint.
    pub(crate) fn scratch_flush(&mut self, ctx: Option<&egui::Context>, force: bool) {
        self.flush_draft_meta();
        let Some(root) = self.scratch_root() else { return };
        if !self.files.iter().any(|file| self.scratch_needs_write(file, &root)) { return; }
        if !force
            && let Some(last) = self.scratch.last_flush
        {
            let elapsed = last.elapsed();
            if elapsed < DEBOUNCE {
                if let Some(ctx) = ctx { ctx.request_repaint_after(DEBOUNCE - elapsed); }
                return;
            }
        }
        self.scratch.last_flush = Some(Instant::now());
        let mut error = None;
        for index in 0..self.files.len() {
            let (id, name, source, text, revision, existing, markdown, untitled, encoding, wanted) = {
                let file = &self.files[index];
                if !self.scratch_needs_write(file, &root) { continue; }
                let text = file.draft_text.clone().unwrap_or_default();
                let untitled = file.is_untitled();
                let wanted = if untitled {
                    !text.is_empty()
                } else {
                    !file.path.as_os_str().is_empty()
                        && file.draft_text.is_some()
                        && file.encoding != recovery::Encoding::Lossy
                        && file.has_unsaved_changes()
                        && file.byte_len() <= MAX_EDIT_MIRROR_BYTES
                };
                (
                    file.id,
                    file.name.clone(),
                    file.path.clone(),
                    text,
                    (file.content_revision, file.editor_state.large.revision),
                    file.scratch_path.clone(),
                    wants_markdown(file),
                    untitled,
                    file.encoding,
                    wanted,
                )
            };
            if !wanted {
                if let Some(path) = existing.as_deref()
                    && let Err(failure) = remove(path)
                {
                    error = Some(failure);
                }
                self.files[index].scratch_path = None;
                self.scratch.written.remove(&id);
                continue;
            }
            let (path, bytes) = if untitled {
                let path = match existing {
                    Some(path)
                        if path.parent() == Some(root.as_path())
                            && extension_matches(&path, markdown) =>
                    {
                        path
                    }
                    _ => {
                        // A different window may have written this number
                        // since this tab was created. Renumber before mirroring
                        // instead of quietly creating `未命名 1 (2).txt`.
                        let target = root.join(mirror_file_name(&name, markdown));
                        if untitled_number(&name).is_some() && target.exists() {
                            let number = self.next_untitled_number();
                            let prefix = if name.starts_with(crate::tr(crate::Lang::En).untitled) {
                                crate::tr(crate::Lang::En).untitled
                            } else { crate::tr(crate::Lang::ZhCn).untitled };
                            let renamed = format!("{prefix} {number}");
                            allocate(&root, &renamed, markdown)
                        } else { allocate(&root, &name, markdown) }
                    },
                };
                (path, text.into_bytes())
            } else {
                // Preserve the document encoding so Save writes the same bytes.
                let Ok(bytes) = encoding.encode(&text) else {
                    // An unrepresentable legacy encoding falls back to the
                    // inline session copy instead of a lossy mirror.
                    if let Some(path) = existing.as_deref() {
                        let _ = remove(path);
                    }
                    self.files[index].scratch_path = None;
                    self.scratch.written.remove(&id);
                    continue;
                };
                if bytes.len() > MAX_EDIT_MIRROR_BYTES {
                    if let Some(path) = existing.as_deref() {
                        let _ = remove(path);
                    }
                    self.files[index].scratch_path = None;
                    self.scratch.written.remove(&id);
                    continue;
                }
                let directory = edits_directory(&root);
                let path = match existing {
                    Some(path) if path.parent() == Some(directory.as_path()) => path,
                    _ => directory.join(edit_mirror_name(&name, &source)),
                };
                (path, bytes)
            };
            let previous = self.files[index].scratch_path.clone();
            match recovery::atomic_write(&path, &bytes) {
                Ok(()) => {
                    if untitled && untitled_number(&name).is_some()
                        && let Some(file_name) = path.file_name().and_then(|name| name.to_str())
                        && draft_stem(file_name) != draft_stem(&name)
                    {
                        // Publish a changed title only after its mirror exists.
                        self.files[index].name = draft_stem(file_name).to_owned();
                    }
                    self.files[index].scratch_path = Some(path.clone());
                    self.scratch.written.insert(id, revision);
                    // Moving a mirror to a new configured directory.
                    if let Some(previous) = previous
                        && previous != path
                    {
                        let _ = remove(&previous);
                    }
                }
                Err(failure) => {
                    error = Some(failure);
                    // Drop the now-stale mirror so recovery falls back to an
                    // inline copy instead of restoring older content.
                    if let Some(previous) = previous.as_deref() {
                        let _ = remove(previous);
                    }
                    self.files[index].scratch_path = None;
                    self.scratch.written.remove(&id);
                }
            }
        }
        match error {
            Some(failure) => {
                let message = format!("{}: {failure}", tr(self.lang).scratch_write_failed);
                if self.scratch.last_error.as_deref() != Some(message.as_str()) {
                    self.scratch.last_error = Some(message.clone());
                    self.io_error = Some(message);
                }
            }
            None => self.scratch.last_error = None,
        }
        // Drafts whose mirror was just allocated now have a file name to key
        // the metadata sidecar by.
        self.flush_draft_meta();
        remove_empty_edit_dirs(&root);
    }

    /// Add every text file currently in the scratch directory as an untitled
    /// tab, skipping anything already open by path.
    pub(crate) fn open_scratch_files(&mut self) {
        let drafts = scan(self.scratch_root().as_deref());
        self.add_scratch_drafts(drafts);
    }

    pub(crate) fn add_scratch_drafts(&mut self, drafts: Vec<Draft>) {
        for draft in drafts {
            let draft_path = crate::linkage::resolved(&draft.path);
            if self.files.iter().any(|file| {
                file.scratch_path.as_deref().is_some_and(|scratch| {
                    crate::linkage::resolved(scratch) == draft_path
                }) || (!file.path.as_os_str().is_empty()
                    && crate::linkage::resolved(&file.path) == draft_path)
            }) {
                continue;
            }
            // A session snapshot written before this feature existed (or one
            // that raced with a mirror write) may still describe the same
            // untitled tab. The mirror is refreshed more often, so adopt it
            // instead of opening a duplicate tab. Compare stems because a
            // draft that switched to Markdown carries a `.md` file extension.
            if let Some(index) = self.files.iter().position(|file| {
                file.is_untitled()
                    && file.scratch_path.is_none()
                    && file
                        .draft_text
                        .as_deref()
                        .is_some_and(|text| !text.is_empty())
                    && draft_stem(&file.name) == draft_stem(&draft.name)
            }) {
                let file = &mut self.files[index];
                if file.draft_text.as_deref() != Some(draft.text.as_str()) {
                    file.draft_text = Some(draft.text);
                    file.sync_draft();
                }
                if file.language_override.is_none() {
                    file.language_override = draft.language_override;
                }
                file.pinned |= draft.pinned;
                if let Some(mode) = draft.markdown_mode
                    && crate::markdown::is_markdown(file)
                {
                    file.markdown_mode = mode;
                }
                file.scratch_path = Some(draft.path);
                self.scratch.written.insert(
                    file.id,
                    (file.content_revision, file.editor_state.large.revision),
                );
                continue;
            }
            let id = self.next_file_id;
            self.next_file_id += 1;
            let line_ending = crate::line_endings::Counts::of(&draft.text)
                .preferred(crate::line_endings::LineEnding::default());
            let character_count = draft.text.chars().count();
            let mut file = LoadedFile {
                pinned: draft.pinned,
                id,
                path: PathBuf::new(),
                name: draft.name,
                bytes: Vec::new(),
                is_binary: false,
                lines: Vec::new(),
                view: ViewMode::Text,
                language_override: draft.language_override,
                draft_text: Some(draft.text),
                editor_state: crate::editor::EditorState::default(),
                encoding: recovery::Encoding::default(),
                saved_bytes: Vec::new(),
                character_count,
                saved_large_revision: None,
                bytes_dirty: false,
                content_revision: 0,
                saved_encoding: recovery::Encoding::default(),
                source_path: None,
                disk: crate::linkage::DiskState::default(),
                line_ending,
                markdown_mode: draft.markdown_mode.unwrap_or(self.markdown_mode),
                markdown: crate::markdown::State::default(),
                saved_generation: 0,
                scratch_path: Some(draft.path),
            };
            file.sync_draft();
            self.scratch.written.insert(id, (file.content_revision, file.editor_state.large.revision));
            self.files.push(file);
        }
    }

    /// Apply the metadata stored alongside a session. Version 1 kept untitled
/// drafts outside the file records; version 2 stores them as records, so this
/// mainly fills in state for older snapshots. A draft whose mirror is missing
/// is reopened as a blank tab instead of being lost.
pub(crate) fn restore_scratch_meta(&mut self, meta: &[recovery::ScratchMeta]) {
    for entry in meta {
        let stem = draft_stem(&entry.file_name);
        let found = self
            .files
            .iter()
            .position(|file| file.is_untitled() && draft_stem(&file.name) == stem);
        if let Some(index) = found {
            let file = &mut self.files[index];
            file.pinned = entry.pinned;
            if let Some(language) = &entry.language {
                file.language_override = Some(language.clone());
            }
            if let Some(mode) = entry.markdown_mode
                && crate::markdown::is_markdown(file)
            {
                file.markdown_mode = mode;
            }
            continue;
        }
        // The mirror may have been removed after an older snapshot or never
        // written. Preserve the tab itself with an empty buffer.
        let id = self.next_file_id;
        self.next_file_id += 1;
        let language_override = entry
            .language
            .clone()
            .or_else(|| markdown_override(Path::new(&entry.file_name)));
        let mut file = LoadedFile {
            pinned: entry.pinned,
            id,
            path: PathBuf::new(),
            name: stem.to_owned(),
            bytes: Vec::new(),
            is_binary: false,
            lines: Vec::new(),
            view: ViewMode::Text,
            language_override,
            draft_text: Some(String::new()),
            editor_state: crate::editor::EditorState::default(),
            encoding: recovery::Encoding::default(),
            saved_bytes: Vec::new(),
            character_count: 0,
            saved_large_revision: None,
            bytes_dirty: false,
            content_revision: 0,
            saved_encoding: recovery::Encoding::default(),
            source_path: None,
            disk: crate::linkage::DiskState::default(),
            line_ending: crate::line_endings::LineEnding::default(),
            markdown_mode: crate::markdown::Mode::Source,
            markdown: crate::markdown::State::default(),
            saved_generation: 0,
            scratch_path: None,
        };
        file.sync_draft();
        if let Some(mode) = entry.markdown_mode
            && crate::markdown::is_markdown(&file)
        {
            file.markdown_mode = mode;
        }
        self.files.push(file);
    }
    // Pinned tabs belong to the leading region. Drafts come back from the
    // directory in path order, so move them into the order the session stored
    // and insert them after any pinned real files.
    let mut pinned_drafts = Vec::new();
    let mut rest = Vec::with_capacity(self.files.len());
    for file in std::mem::take(&mut self.files) {
        if file.pinned && file.is_untitled() {
            pinned_drafts.push(file);
        } else {
            rest.push(file);
        }
    }
    pinned_drafts.sort_by_key(|file| {
        meta.iter()
            .position(|entry| draft_stem(&entry.file_name) == draft_stem(&file.name))
            .unwrap_or(usize::MAX)
    });
    let insert_at = rest
        .iter()
        .rposition(|file| file.pinned)
        .map_or(0, |index| index + 1);
    rest.splice(insert_at..insert_at, pinned_drafts);
    self.files = rest;
}

/// Remove the mirror belonging to a tab that is being closed on purpose.
    pub(crate) fn retire_file(&mut self, file: LoadedFile) {
        if let Some(path) = &file.scratch_path
            && let Err(error) = remove(path)
        {
            self.report_scratch_error(&error);
        }
        self.scratch.written.remove(&file.id);
    }

    /// A successful Save As turns the draft into a real file; its mirror is no
    /// longer needed. Saving to the mirror path itself keeps that file.
    pub(crate) fn retire_mirror_after_save(&mut self, id: u64, mirror: Option<PathBuf>, target: &Path) {
        if let Some(path) = mirror {
            if crate::linkage::resolved(&path) != crate::linkage::resolved(target)
                && let Err(error) = remove(&path)
            {
                self.report_scratch_error(&error);
            }
            if let Some(file) = self.files.iter_mut().find(|file| file.id == id) {
                file.scratch_path = None;
            }
            self.scratch.written.remove(&id);
        }
    }

    fn report_scratch_error(&mut self, error: &io::Error) {
        let message = format!("{}: {error}", tr(self.lang).scratch_write_failed);
        if self.scratch.last_error.as_deref() != Some(message.as_str()) {
            self.scratch.last_error = Some(message.clone());
            self.io_error = Some(message);
        }
    }

    /// Human-readable directory shown in settings, including the default.
    pub(crate) fn scratch_display_root(&self) -> Option<PathBuf> {
        self.scratch_root()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app(root: &Path) -> LiteTextApp {
        LiteTextApp {
            scratch_dir: Some(root.to_path_buf()),
            recovery_path_override: Some(root.join("recovery/session.json")),
            ..Default::default()
        }
    }

    fn only_file(root: &Path) -> Option<PathBuf> {
        std::fs::read_dir(root)
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .find(|path| path.file_name().is_some_and(|name| name != "session.json" && name != DRAFT_META_FILE))
    }

    #[test]
    fn new_numbers_reserve_cached_and_legacy_collision_names() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["未命名 1.txt", "未命名 1 (2).txt", "Untitled 2.md", "未命名 3.txt"] {
            std::fs::write(dir.path().join(name), "preserve").unwrap();
        }
        let mut app = test_app(dir.path());
        app.new_document();
        assert_eq!(app.active().unwrap().name, "未命名 4");
        assert_eq!(untitled_number("未命名 1 (2).txt"), Some(1));
        assert_eq!(untitled_number("Untitled 5.md"), Some(5));
        assert_eq!(untitled_number("notes 1.txt"), None);
    }

    #[test]
    fn late_cache_collision_renumbers_tab_without_overwriting_either_draft() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = test_app(dir.path());
        app.new_document();
        std::fs::write(dir.path().join("未命名 1.txt"), "another window").unwrap();
        app.active_mut().unwrap().draft_text = Some("new draft".into());
        app.active_mut().unwrap().sync_draft();
        app.scratch_flush(None, true);
        assert_eq!(app.active().unwrap().name, "未命名 2");
        assert_eq!(std::fs::read_to_string(dir.path().join("未命名 1.txt")).unwrap(), "another window");
        assert_eq!(std::fs::read_to_string(dir.path().join("未命名 2.txt")).unwrap(), "new draft");
        assert!(!dir.path().join("未命名 1 (2).txt").exists());
    }

    #[test]
    fn untitled_draft_is_mirrored_updated_and_removed_only_when_closed() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = test_app(dir.path());
        app.new_document();
        app.active_mut().unwrap().draft_text = Some("first draft".into());
        app.active_mut().unwrap().sync_draft();
        app.scratch_flush(None, true);

        let mirror = only_file(dir.path()).expect("mirror created");
        assert_eq!(std::fs::read_to_string(&mirror).unwrap(), "first draft");

        app.active_mut().unwrap().draft_text = Some("second draft".into());
        app.active_mut().unwrap().sync_draft();
        app.scratch_flush(None, true);
        assert_eq!(std::fs::read_to_string(&mirror).unwrap(), "second draft");

        // Normal shutdown keeps the mirror for the next launch.
        app.write_recovery().unwrap();
        assert!(mirror.exists());

        let id = app.active_id.unwrap();
        app.close_tab(id);
        assert!(!mirror.exists(), "explicit close removes the mirror");
    }

    #[test]
    fn clearing_an_untitled_draft_removes_its_mirror() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = test_app(dir.path());
        app.new_document();
        app.active_mut().unwrap().draft_text = Some("content".into());
        app.active_mut().unwrap().sync_draft();
        app.scratch_flush(None, true);
        assert!(only_file(dir.path()).is_some());

        app.active_mut().unwrap().draft_text = Some(String::new());
        app.active_mut().unwrap().sync_draft();
        app.scratch_flush(None, true);
        assert!(only_file(dir.path()).is_none());
        assert!(app.active().unwrap().scratch_path.is_none());
    }

    #[test]
    fn saving_to_a_real_file_removes_the_mirror() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = test_app(dir.path());
        app.new_document();
        app.active_mut().unwrap().draft_text = Some("save me".into());
        app.active_mut().unwrap().sync_draft();
        app.scratch_flush(None, true);
        let mirror = only_file(dir.path()).expect("mirror created");
        let id = app.active_id.unwrap();
        let target = dir.path().join("saved.txt");

        assert!(app.request_save_to(id, target.clone()));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "save me");
        assert!(!mirror.exists(), "Save As retires the untitled mirror");
        assert!(app.files[0].scratch_path.is_none());
        assert!(!app.files[0].is_untitled());
    }

    #[test]
    fn startup_opens_every_text_file_as_an_untitled_tab() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("未命名 3.txt"), "draft body").unwrap();
        std::fs::write(dir.path().join("notes.md"), "# Heading").unwrap();
        std::fs::write(dir.path().join("ignored.bin"), [0u8, 159, 146, 150]).unwrap();
        std::fs::write(dir.path().join(".hidden"), "hidden").unwrap();

        let mut app = test_app(dir.path());
        app.open_scratch_files();
        assert_eq!(app.files.len(), 2);
        let draft = app.files.iter().find(|file| file.name == "未命名 3").unwrap();
        assert!(draft.path.as_os_str().is_empty());
        assert!(draft.source_path.is_none());
        assert_eq!(draft.draft_text.as_deref(), Some("draft body"));
        assert!(draft.scratch_path.is_some());
        let notes = app.files.iter().find(|file| file.name == "notes.md").unwrap();
        assert_eq!(notes.language_override.as_deref(), Some("Markdown"));
        assert!(crate::markdown::is_markdown(notes));
    }

    #[test]
    fn session_recovery_does_not_duplicate_mirrored_drafts() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = test_app(dir.path());
        app.new_document();
        app.active_mut().unwrap().draft_text = Some("persisted draft".into());
        app.active_mut().unwrap().sync_draft();
        app.scratch_flush(None, true);
        app.write_recovery().unwrap();

        let session = recovery::load(&dir.path().join("recovery/session.json")).unwrap().unwrap();
        assert_eq!(
            session.record_count_for_testing(),
            1,
            "the tab is stored once, as metadata plus a mirror path"
        );
        let json = std::fs::read_to_string(dir.path().join("recovery/session.json")).unwrap();
        assert!(
            !json.contains("persisted draft"),
            "the mirrored body must not be duplicated in the session"
        );
        assert!(json.contains("\"mirror\""), "the session must point at the mirror");
        assert_eq!(session.scratch_active(), None);

        let mut restored = test_app(dir.path());
        restored.restore_session();
        assert_eq!(restored.files.len(), 1);
        assert_eq!(restored.files[0].draft_text.as_deref(), Some("persisted draft"));
        assert!(restored.files[0].is_scratch_mirror());
        assert_eq!(restored.active_id, Some(restored.files[0].id));
    }

    #[test]
    fn session_untitled_tab_adopts_its_mirror_instead_of_duplicating() {
        let dir = tempfile::tempdir().unwrap();
        let mut first = LiteTextApp {
            recovery_path_override: Some(dir.path().join("recovery/session.json")),
            ..Default::default()
        };
        first.new_document();
        first.active_mut().unwrap().draft_text = Some("stale session text".into());
        first.active_mut().unwrap().sync_draft();
        first.write_recovery().unwrap();
        // The mirror is newer than the session snapshot.
        std::fs::write(dir.path().join("未命名 1.txt"), "newer mirror text").unwrap();

        let mut restored = test_app(dir.path());
        restored.restore_session();
        assert_eq!(restored.files.len(), 1, "no duplicate tab for the same draft");
        assert_eq!(
            restored.files[0].draft_text.as_deref(),
            Some("newer mirror text")
        );
        assert!(restored.files[0].is_scratch_mirror());
    }

    #[test]
    fn scratch_pin_names_restore_the_recorded_pinned_order() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = test_app(dir.path());
        app.new_document();
        let first = app.active_id.unwrap();
        app.active_mut().unwrap().draft_text = Some("first".into());
        app.active_mut().unwrap().sync_draft();
        app.new_document();
        let second = app.active_id.unwrap();
        app.active_mut().unwrap().draft_text = Some("second".into());
        app.active_mut().unwrap().sync_draft();
        app.toggle_pin(second);
        app.toggle_pin(first);
        app.write_recovery().unwrap();

        let session = recovery::load(&dir.path().join("recovery/session.json"))
            .unwrap()
            .unwrap();
        assert_eq!(session.record_count_for_testing(), 2);
        let json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(dir.path().join("recovery/session.json")).unwrap(),
        )
        .unwrap();
        let names: Vec<&str> = json["files"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|record| record["pinned"] == true)
            .map(|record| record["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["未命名 2", "未命名 1"], "pin order follows the tab strip");

        let mut restored = test_app(dir.path());
        restored.restore_session();
        let pinned: Vec<&str> = restored
            .files
            .iter()
            .filter(|file| file.pinned)
            .map(|file| file.name.as_str())
            .collect();
        assert_eq!(pinned, ["未命名 2", "未命名 1"]);
    }

    #[test]
    fn scratch_markdown_mode_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = test_app(dir.path());
        app.new_document();
        {
            let file = app.active_mut().unwrap();
            file.draft_text = Some("# draft\n\ntext\n".into());
            file.language_override = Some("Markdown".into());
            file.markdown_mode = crate::markdown::Mode::Split;
            file.sync_draft();
        }
        app.scratch_flush(None, true);
        app.write_recovery().unwrap();

        let mut restored = test_app(dir.path());
        restored.restore_session();
        assert_eq!(restored.files.len(), 1);
        assert_eq!(
            restored.files[0].markdown_mode,
            crate::markdown::Mode::Split,
            "a mirrored draft keeps its own Markdown view"
        );
    }

    #[test]
    fn scratch_language_override_survives_restart_and_can_be_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = test_app(dir.path());
        app.new_document();
        app.active_mut().unwrap().draft_text = Some("{\"a\": 1}".into());
        app.active_mut().unwrap().sync_draft();
        app.active_mut().unwrap().language_override = Some("JSON".into());
        app.scratch_flush(None, true);
        app.write_recovery().unwrap();

        let session = recovery::load(&dir.path().join("recovery/session.json"))
            .unwrap()
            .unwrap();
        assert_eq!(session.record_count_for_testing(), 1);
        let json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(dir.path().join("recovery/session.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            json["files"][0]["language_override"].as_str(),
            Some("JSON"),
            "the manual language choice travels with the tab record"
        );
        assert!(
            json["files"][0]["mirror"]
                .as_str()
                .is_some_and(|path| path.ends_with(".txt")),
            "a language override must not rename the mirror"
        );

        let mut restored = test_app(dir.path());
        restored.restore_session();
        assert_eq!(restored.files.len(), 1);
        assert_eq!(
            restored.files[0].language_override.as_deref(),
            Some("JSON"),
            "a restored draft keeps its manual language"
        );
        assert_eq!(
            crate::workspace_theme::effective_language(&restored.files[0]),
            "JSON"
        );

        // Clearing the override (Auto) must not resurrect the stored choice.
        restored.active_mut().unwrap().language_override = None;
        restored.write_recovery().unwrap();
        let session = recovery::load(&dir.path().join("recovery/session.json"))
            .unwrap()
            .unwrap();
        assert!(
            session.record_count_for_testing() == 1,
            "the cleared draft is still represented by its record"
        );
        let json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(dir.path().join("recovery/session.json")).unwrap(),
        )
        .unwrap();
        assert!(
            json["files"]
                .as_array()
                .unwrap()
                .iter()
                .all(|record| record["language_override"].is_null()),
            "an Auto draft must not keep writing a language"
        );
        let mut again = test_app(dir.path());
        again.restore_session();
        assert_eq!(again.files[0].language_override, None);
        assert_ne!(
            crate::workspace_theme::effective_language(&again.files[0]),
            "JSON"
        );
    }

    #[test]
    fn explicit_non_markdown_choice_is_not_replaced_by_the_mirror_extension() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = test_app(dir.path());
        app.new_document();
        app.active_mut().unwrap().draft_text = Some("# heading".into());
        app.active_mut().unwrap().sync_draft();
        app.active_mut().unwrap().language_override = Some("Markdown".into());
        app.scratch_flush(None, true);
        let mirror = app.active().unwrap().scratch_path.clone().unwrap();
        assert_eq!(mirror.extension().unwrap(), "md");

        // The picker can still choose a non-Markdown language afterwards. The
        // mirror is renamed to `.txt`, but the explicit choice must come back
        // instead of being replaced by extension-derived language detection.
        app.active_mut().unwrap().language_override = Some("Rust".into());
        app.write_recovery().unwrap();
        let mut restored = test_app(dir.path());
        restored.restore_session();
        assert_eq!(restored.files[0].language_override.as_deref(), Some("Rust"));
        assert_eq!(
            restored.files[0].scratch_path.as_ref().unwrap().extension().unwrap(),
            "txt",
            "the mirror extension follows Markdown-ness, not the chosen language"
        );
        assert!(!mirror.exists(), "the old .md mirror is replaced");
    }

    #[test]
    fn session_untitled_markdown_tab_adopts_its_md_mirror() {
        let dir = tempfile::tempdir().unwrap();
        let mut first = LiteTextApp {
            recovery_path_override: Some(dir.path().join("recovery/session.json")),
            ..Default::default()
        };
        first.new_document();
        let file = first.active_mut().unwrap();
        file.draft_text = Some("older session text".into());
        file.language_override = Some("Markdown".into());
        file.sync_draft();
        // No mirror yet: the session snapshot still stores this tab.
        first.write_recovery().unwrap();
        assert!(
            recovery::load(&dir.path().join("recovery/session.json"))
                .unwrap()
                .unwrap()
                .has_files()
        );
        std::fs::write(dir.path().join("未命名 1.md"), "# newer mirror text").unwrap();

        let mut restored = test_app(dir.path());
        restored.restore_session();
        assert_eq!(restored.files.len(), 1, "no duplicate tab for the same draft");
        assert_eq!(
            restored.files[0].draft_text.as_deref(),
            Some("# newer mirror text")
        );
        assert!(crate::markdown::is_markdown(&restored.files[0]));
        assert_eq!(
            restored.files[0]
                .scratch_path
                .as_ref()
                .and_then(|path| path.file_name())
                .map(|name| name.to_string_lossy())
                .as_deref(),
            Some("未命名 1.md")
        );
    }

    #[test]
    fn markdown_override_renames_the_mirror_and_back() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = test_app(dir.path());
        app.new_document();
        app.active_mut().unwrap().draft_text = Some("# Heading\n".into());
        app.active_mut().unwrap().sync_draft();
        app.scratch_flush(None, true);

        let text_mirror = app.active().unwrap().scratch_path.clone().unwrap();
        assert_eq!(text_mirror.extension().unwrap(), "txt");
        assert_eq!(text_mirror.file_stem().unwrap(), "未命名 1");

        app.active_mut().unwrap().language_override = Some("Markdown".into());
        app.scratch_flush(None, true);
        let markdown_mirror = app.active().unwrap().scratch_path.clone().unwrap();
        assert_eq!(markdown_mirror.extension().unwrap(), "md");
        assert_eq!(markdown_mirror.file_stem(), text_mirror.file_stem());
        assert!(!text_mirror.exists(), "the old .txt mirror is not left behind");
        assert_eq!(std::fs::read_to_string(&markdown_mirror).unwrap(), "# Heading\n");

        // A restored draft reads the extension back as the Markdown override.
        let mut restored = test_app(dir.path());
        restored.open_scratch_files();
        assert_eq!(restored.files.len(), 1);
        assert!(crate::markdown::is_markdown(&restored.files[0]));

        app.active_mut().unwrap().language_override = None;
        app.scratch_flush(None, true);
        let back = app.active().unwrap().scratch_path.clone().unwrap();
        assert_eq!(back.extension().unwrap(), "txt");
        assert!(!markdown_mirror.exists(), "the .md mirror is replaced");
        assert_eq!(std::fs::read_to_string(&back).unwrap(), "# Heading\n");
    }

    #[test]
    fn changing_the_directory_moves_existing_mirrors() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let mut app = test_app(first.path());
        app.new_document();
        app.active_mut().unwrap().draft_text = Some("relocate me".into());
        app.active_mut().unwrap().sync_draft();
        app.scratch_flush(None, true);
        let old = app.active().unwrap().scratch_path.clone().unwrap();
        assert!(old.exists());

        app.scratch_dir = Some(second.path().to_path_buf());
        app.scratch_flush(None, true);
        let new = app.active().unwrap().scratch_path.clone().unwrap();
        assert_eq!(new.parent(), Some(second.path()));
        assert_eq!(std::fs::read_to_string(&new).unwrap(), "relocate me");
        assert!(!old.exists(), "the old mirror is not left behind");
    }

    fn walk_edits(root: &Path) -> Option<PathBuf> {
        let entries = std::fs::read_dir(root).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(found) = walk_edits(&path) {
                    return Some(found);
                }
            } else if path.is_file() {
                return Some(path);
            }
        }
        None
    }

    #[test]
    fn unsaved_real_file_edits_live_in_the_edits_mirror_and_not_the_session() {
        let dir = tempfile::tempdir().unwrap();
        let docs = dir.path().join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        let real = docs.join("notes.txt");
        std::fs::write(&real, "original\n").unwrap();
        let mut app = test_app(dir.path());
        app.open_paths(vec![real.clone()]);
        app.active_mut().unwrap().draft_text = Some("edited\n".into());
        app.active_mut().unwrap().sync_draft();
        app.scratch_flush(None, true);

        assert_eq!(
            std::fs::read_to_string(&real).unwrap(),
            "original\n",
            "mirroring must never touch the original file"
        );
        let mirror = app.active().unwrap().scratch_path.clone().unwrap();
        assert!(
            mirror.starts_with(dir.path().join("edits")),
            "real-file mirrors live in the window-scoped edits directory"
        );
        assert_eq!(std::fs::read_to_string(&mirror).unwrap(), "edited\n");

        app.write_recovery().unwrap();
        let snapshot =
            std::fs::read_to_string(dir.path().join("recovery/session.json")).unwrap();
        assert!(
            !snapshot.contains("edited"),
            "the session records a path, not the unsaved body"
        );
        let json: serde_json::Value = serde_json::from_str(&snapshot).unwrap();
        assert!(
            json["files"][0]["text"].is_null() && json["files"][0]["baseline"].is_null(),
            "a current mirror also replaces the inline body and baseline"
        );
        assert!(json["files"][0]["mirror"].as_str().is_some());
        assert_eq!(
            std::fs::read_to_string(&real).unwrap(),
            "original\n",
            "writing the session does not save the document"
        );

        let mut restored = test_app(dir.path());
        restored.restore_session();
        assert_eq!(restored.files.len(), 1);
        let file = restored.active().unwrap();
        assert_eq!(file.draft_text.as_deref(), Some("edited\n"));
        assert!(file.is_modified());
        assert_eq!(file.path, std::fs::canonicalize(&real).unwrap());
        assert!(file.scratch_path.as_ref().is_some_and(|path| path == &mirror));
    }

    #[test]
    fn save_replaces_the_original_and_retires_the_real_file_mirror() {
        let dir = tempfile::tempdir().unwrap();
        let docs = dir.path().join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        let real = docs.join("notes.txt");
        std::fs::write(&real, "original\n").unwrap();
        let mut app = test_app(dir.path());
        app.open_paths(vec![real.clone()]);
        app.active_mut().unwrap().draft_text = Some("edited\n".into());
        app.active_mut().unwrap().sync_draft();
        app.scratch_flush(None, true);
        let id = app.active_id.unwrap();
        let mirror = app.active().unwrap().scratch_path.clone().unwrap();

        assert!(app.request_save_to(id, real.clone()));
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "edited\n");
        assert!(!mirror.exists(), "Save removes the edit mirror");
        assert!(app.files[0].scratch_path.is_none());
        app.scratch_flush(None, true);
        assert!(
            app.files[0].scratch_path.is_none(),
            "a clean real file must not recreate a mirror"
        );
    }

    #[test]
    fn returning_to_the_saved_text_removes_the_edit_mirror() {
        let dir = tempfile::tempdir().unwrap();
        let docs = dir.path().join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        let real = docs.join("notes.txt");
        std::fs::write(&real, "original\n").unwrap();
        let mut app = test_app(dir.path());
        app.open_paths(vec![real.clone()]);
        app.active_mut().unwrap().draft_text = Some("edited\n".into());
        app.active_mut().unwrap().sync_draft();
        app.scratch_flush(None, true);
        let mirror = app.active().unwrap().scratch_path.clone().unwrap();
        assert!(mirror.exists());

        app.active_mut().unwrap().draft_text = Some("original\n".into());
        app.active_mut().unwrap().sync_draft();
        app.scratch_flush(None, true);
        assert!(!mirror.exists(), "undoing all edits retires the mirror");
        assert!(app.files[0].scratch_path.is_none());
        assert!(!app.files[0].is_modified());
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "original\n");
    }

    #[test]
    fn closing_a_dirty_real_file_removes_its_edit_mirror() {
        let dir = tempfile::tempdir().unwrap();
        let docs = dir.path().join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        let real = docs.join("notes.txt");
        std::fs::write(&real, "original\n").unwrap();
        let mut app = test_app(dir.path());
        app.open_paths(vec![real.clone()]);
        app.active_mut().unwrap().draft_text = Some("edited\n".into());
        app.active_mut().unwrap().sync_draft();
        app.scratch_flush(None, true);
        let id = app.active_id.unwrap();
        let mirror = app.active().unwrap().scratch_path.clone().unwrap();
        assert!(walk_edits(&dir.path().join("edits")).is_some());

        app.close_tab(id);
        assert!(!mirror.exists(), "closing the tab discards its edit mirror");
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "original\n");
    }

    #[test]
    fn version_one_missing_mirror_still_reopens_a_blank_pinned_tab() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("recovery")).unwrap();
        std::fs::write(
            dir.path().join("recovery/session.json"),
            serde_json::to_vec(&serde_json::json!({
                "version": 1,
                "active_id": null,
                "scratch_active": "未命名 1.txt",
                "scratch_meta": [
                    { "file_name": "未命名 1.txt", "pinned": true },
                    {
                        "file_name": "笔记.md",
                        "pinned": true,
                        "markdown_mode": "Split"
                    }
                ],
                "scratch_pinned": [],
                "files": []
            }))
            .unwrap(),
        )
        .unwrap();
        let mut app = test_app(dir.path());
        app.restore_session();
        assert_eq!(app.files.len(), 2, "missing mirrors must not drop tabs");
        assert_eq!(app.files[0].name, "未命名 1");
        assert!(app.files[0].pinned);
        assert_eq!(app.files[0].draft_text.as_deref(), Some(""));
        assert!(app.files[0].scratch_path.is_none());
        assert_eq!(app.files[1].name, "笔记");
        assert!(app.files[1].pinned);
        assert!(crate::markdown::is_markdown(&app.files[1]));
        assert_eq!(app.files[1].markdown_mode, crate::markdown::Mode::Split);
        assert_eq!(app.active_id, Some(app.files[0].id));
    }

    #[test]
    fn unavailable_edit_mirror_falls_back_to_an_inline_session() {
        let dir = tempfile::tempdir().unwrap();
        let docs = dir.path().join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        let real = docs.join("notes.txt");
        std::fs::write(&real, "original\n").unwrap();
        // A regular file where the edits directory should be makes every
        // mirror write fail.
        std::fs::write(dir.path().join("edits"), "not a directory").unwrap();
        let mut app = test_app(dir.path());
        app.open_paths(vec![real.clone()]);
        app.active_mut().unwrap().draft_text = Some("edited\n".into());
        app.active_mut().unwrap().sync_draft();
        app.scratch_flush(None, true);
        assert!(
            app.files[0].scratch_path.is_none(),
            "a failed mirror write must not leave a stale path"
        );
        app.write_recovery().unwrap();
        let snapshot =
            std::fs::read_to_string(dir.path().join("recovery/session.json")).unwrap();
        assert!(
            snapshot.contains("edited"),
            "the session must retain the text instead of losing it"
        );

        let mut restored = test_app(dir.path());
        restored.restore_session();
        assert_eq!(restored.files[0].draft_text.as_deref(), Some("edited\n"));
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "original\n");
    }

    #[test]
    fn edit_mirror_preserves_the_document_encoding() {
        let dir = tempfile::tempdir().unwrap();
        let docs = dir.path().join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        let real = docs.join("utf16.txt");
        let original = recovery::Encoding::Utf16Le.encode("original\r\n").unwrap();
        std::fs::write(&real, &original).unwrap();
        let mut app = test_app(dir.path());
        app.open_paths(vec![real.clone()]);
        assert_eq!(
            app.active().unwrap().encoding,
            recovery::Encoding::Utf16Le
        );
        app.active_mut().unwrap().draft_text = Some("edited\r\n".into());
        app.active_mut().unwrap().sync_draft();
        app.scratch_flush(None, true);

        let mirror = app.active().unwrap().scratch_path.clone().unwrap();
        assert_eq!(
            std::fs::read(&mirror).unwrap(),
            recovery::Encoding::Utf16Le.encode("edited\r\n").unwrap(),
            "the mirror must not silently transcode the document"
        );
        app.write_recovery().unwrap();
        let mut restored = test_app(dir.path());
        restored.restore_session();
        assert_eq!(restored.files[0].draft_text.as_deref(), Some("edited\r\n"));
        assert_eq!(
            restored.files[0].encoding,
            recovery::Encoding::Utf16Le
        );
        assert_eq!(std::fs::read(&real).unwrap(), original);
    }

    #[test]
    fn detaching_a_real_file_retires_its_mirror_and_keeps_inline_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let docs = dir.path().join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        let real = docs.join("notes.txt");
        std::fs::write(&real, "original\n").unwrap();
        let mut app = test_app(dir.path());
        app.open_paths(vec![real.clone()]);
        app.active_mut().unwrap().draft_text = Some("edited\n".into());
        app.active_mut().unwrap().sync_draft();
        app.scratch_flush(None, true);
        let mirror = app.active().unwrap().scratch_path.clone().unwrap();
        assert!(mirror.exists());

        crate::linkage::detach(app.active_mut().unwrap());
        app.scratch_flush(None, true);
        assert!(!mirror.exists(), "a detached buffer uses the inline session");
        assert!(app.files[0].scratch_path.is_none());
        app.write_recovery().unwrap();

        let mut restored = test_app(dir.path());
        restored.restore_session();
        assert_eq!(restored.files.len(), 1);
        assert_eq!(restored.files[0].draft_text.as_deref(), Some("edited\n"));
        assert!(restored.files[0].path.as_os_str().is_empty());
        assert!(restored.files[0].source_path.is_some());
    }

    #[test]
    fn draft_metadata_survives_a_lost_session() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = test_app(dir.path());
        app.new_document();
        let file = app.active_mut().unwrap();
        file.draft_text = Some("keep my settings".into());
        file.sync_draft();
        file.pinned = true;
        file.language_override = Some("JSON".to_owned());
        app.scratch_flush(None, true);

        // No session is read here: only the hidden sidecar and the mirror exist.
        let drafts = scan(Some(dir.path()));
        assert_eq!(drafts.len(), 1);
        assert!(drafts[0].pinned, "a pinned draft stays pinned on rescan");
        assert_eq!(drafts[0].language_override.as_deref(), Some("JSON"));
    }

    #[test]
    fn orphan_edit_directories_are_flattened_and_referenced_mirrors_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("edits/window-old");
        let live = dir.path().join("edits/window-live");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&live).unwrap();
        std::fs::write(old.join("note-aaaaaaaaaaaaaaaa.txt"), "abandoned").unwrap();
        std::fs::write(live.join("note-bbbbbbbbbbbbbbbb.txt"), "in use").unwrap();
        let mut referenced = std::collections::HashSet::new();
        referenced.insert(crate::linkage::resolved(&live.join("note-bbbbbbbbbbbbbbbb.txt")));
        maintain_edits(dir.path(), &referenced).unwrap();

        assert!(!old.exists(), "the retired window directory is gone");
        assert!(live.exists(), "a referenced mirror stays with its live tab");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("edits/note-aaaaaaaaaaaaaaaa.txt")).unwrap(),
            "abandoned",
            "an unreferenced mirror is preserved in the flat directory"
        );
    }

    #[test]
    fn startup_without_a_session_still_opens_sidecar_pinned_drafts() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = test_app(dir.path());
        app.new_document();
        app.active_mut().unwrap().draft_text = Some("draft text".into());
        app.active_mut().unwrap().sync_draft();
        app.active_mut().unwrap().pinned = true;
        app.active_mut().unwrap().language_override = Some("Python".to_owned());
        app.scratch_flush(None, true);

        let mut reopened = test_app(dir.path());
        reopened.restore_session();
        assert_eq!(reopened.files.len(), 1);
        assert!(reopened.files[0].pinned);
        assert_eq!(reopened.files[0].language_override.as_deref(), Some("Python"));
        assert_eq!(reopened.files[0].draft_text.as_deref(), Some("draft text"));
    }
}
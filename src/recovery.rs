use crate::{LoadedFile, ViewMode, detect_binary, editor};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, PartialEq, Eq, Clone, Copy, Default, Serialize, Deserialize)]
pub(crate) enum Encoding {
    #[default]
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    Gbk,
    Gb18030,
    Big5,
    ShiftJis,
    Windows1252,
    /// Invalid UTF-8 must not silently replace original bytes on manual save.
    Lossy,
}

impl Encoding {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Utf8 => "UTF-8",
            Self::Utf8Bom => "UTF-8 BOM",
            Self::Utf16Le => "UTF-16 LE BOM",
            Self::Utf16Be => "UTF-16 BE BOM",
            Self::Gbk => "GBK",
            Self::Gb18030 => "GB18030",
            Self::Big5 => "Big5",
            Self::ShiftJis => "Shift_JIS",
            Self::Windows1252 => "Windows-1252",
            Self::Lossy => "Unknown / Lossy",
        }
    }

    fn legacy(self) -> Option<&'static encoding_rs::Encoding> {
        match self {
            Self::Gbk => Some(encoding_rs::GBK),
            Self::Gb18030 => Some(encoding_rs::GB18030),
            Self::Big5 => Some(encoding_rs::BIG5),
            Self::ShiftJis => Some(encoding_rs::SHIFT_JIS),
            Self::Windows1252 => Some(encoding_rs::WINDOWS_1252),
            _ => None,
        }
    }

    /// Count encoded output and unrepresentable scalars without replacing any saved content.
    /// Numeric references produced by encoding_rs are only counted for invalid preview metadata;
    /// encode() always rejects such output on conversion/save.
    pub(crate) fn legacy_metrics(self, text: &str) -> (usize, usize) {
        let Some(encoding) = self.legacy() else {
            return (0, 0);
        };
        let (bytes, _, errors) = encoding.encode(text);
        let invalid = if errors {
            text.chars()
                .filter(|c| {
                    let mut buffer = [0; 4];
                    encoding.encode(c.encode_utf8(&mut buffer)).2
                })
                .count()
        } else {
            0
        };
        (bytes.len(), invalid)
    }

    pub(crate) fn is_legacy(self) -> bool {
        self.legacy().is_some()
    }

    /// A stored encoding is authoritative for clean session restoration. Do not guess again.
    pub(crate) fn decode_as(self, bytes: &[u8]) -> Option<String> {
        if let Some(encoding) = self.legacy() {
            let (text, errors) = encoding.decode_without_bom_handling(bytes);
            if !errors && self.encode(&text).is_ok_and(|encoded| encoded == bytes) {
                return Some(text.into_owned());
            }
        } else {
            let (encoding, text) = Self::decode(bytes);
            if encoding == self {
                return Some(text);
            }
        }
        None
    }

    pub(crate) fn decode(bytes: &[u8]) -> (Self, String) {
        match bytes {
            [0xFF, 0xFE, rest @ ..] if rest.len() % 2 == 0 => Self::decode_utf16(bytes, rest, true),
            [0xFE, 0xFF, rest @ ..] if rest.len() % 2 == 0 => {
                Self::decode_utf16(bytes, rest, false)
            }
            [0xEF, 0xBB, 0xBF, rest @ ..] => match std::str::from_utf8(rest) {
                Ok(text) => (Self::Utf8Bom, text.to_owned()),
                Err(_) => (Self::Lossy, String::from_utf8_lossy(bytes).into_owned()),
            },
            _ => match std::str::from_utf8(bytes) {
                Ok(text) => (Self::Utf8, text.to_owned()),
                Err(_) => (Self::Lossy, String::from_utf8_lossy(bytes).into_owned()),
            },
        }
    }

    fn decode_utf16(bytes: &[u8], rest: &[u8], le: bool) -> (Self, String) {
        let encoding = if le { Self::Utf16Le } else { Self::Utf16Be };
        let text = crate::decode_utf16(rest, le);
        if encoding.encode(&text).is_ok_and(|encoded| encoded == bytes) {
            (encoding, text)
        } else {
            (Self::Lossy, text)
        }
    }

    pub(crate) fn encode(self, text: &str) -> io::Result<Vec<u8>> {
        if let Some(encoding) = self.legacy() {
            let (bytes, _, errors) = encoding.encode(text);
            if errors {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "{} cannot represent all characters; conversion was refused",
                        self.name()
                    ),
                ));
            }
            return Ok(bytes.into_owned());
        }
        Ok(match self {
            Self::Utf8 => text.as_bytes().to_vec(),
            Self::Utf8Bom => [b"\xEF\xBB\xBF".as_slice(), text.as_bytes()].concat(),
            Self::Utf16Le | Self::Utf16Be => {
                let le = matches!(self, Self::Utf16Le);
                let mut bytes = if le {
                    vec![0xFF, 0xFE]
                } else {
                    vec![0xFE, 0xFF]
                };
                for unit in text.encode_utf16() {
                    bytes.extend_from_slice(&if le {
                        unit.to_le_bytes()
                    } else {
                        unit.to_be_bytes()
                    });
                }
                bytes
            }
            Self::Lossy => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Unsupported or invalid text encoding; refusing a lossy overwrite",
                ));
            }
            Self::Gbk | Self::Gb18030 | Self::Big5 | Self::ShiftJis | Self::Windows1252 => {
                unreachable!("handled above")
            }
        })
    }
}

#[derive(Serialize, Deserialize)]
struct Record {
    id: u64,
    path: PathBuf,
    #[serde(default)]
    source_path: Option<PathBuf>,
    name: String,
    view: ViewMode,
    #[serde(default)]
    language_override: Option<String>,
    encoding: Encoding,
    #[serde(default)]
    saved_encoding: Option<Encoding>,
    /// Only modified / untitled buffers need a recovery copy of their content.
    text: Option<String>,
    baseline: Option<Vec<u8>>,
    #[serde(default)]
    raw: Option<Vec<u8>>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Session {
    version: u32,
    pub(crate) active_id: Option<u64>,
    files: Vec<Record>,
}

impl Session {
    pub(crate) fn is_empty(&self) -> bool { self.files.is_empty() }
    #[cfg(test)]
    pub(crate) fn capture(files: &[LoadedFile], active_id: Option<u64>) -> Self {
        Self {
            version: 1,
            active_id,
            files: files
                .iter()
                .map(|f| Record {
                    id: f.id,
                    path: f.path.clone(),
                    source_path: f.source_path.clone(),
                    name: f.name.clone(),
                    view: f.view,
                    language_override: f.language_override.clone(),
                    encoding: f.encoding,
                    saved_encoding: Some(f.saved_encoding),
                    text: f.is_modified().then(|| f.draft_text.clone()).flatten(),
                    baseline: f.is_modified().then(|| f.saved_bytes.clone()),
                    raw: (f.is_binary && f.path.as_os_str().is_empty()).then(|| f.bytes.clone()),
                })
                .collect(),
        }
    }

    pub(crate) fn restore(&self) -> Vec<LoadedFile> {
        self.files
            .iter()
            .filter_map(|record| {
                if let Some(bytes) = &record.raw {
                    return Some(LoadedFile {
                        id: record.id,
                        path: record.path.clone(),
                        name: record.name.clone(),
                        source_path: record.source_path.clone(),
                        bytes: bytes.clone(),
                        saved_bytes: bytes.clone(),
                        is_binary: true,
                        lines: Vec::new(),
                        view: ViewMode::Hex,
                        language_override: record.language_override.clone(),
                        draft_text: None,
                        editor_state: editor::EditorState::default(),
                        encoding: record.encoding,
                        saved_encoding: record.encoding,
                        character_count: 0,
                        saved_large_revision: None,
                        bytes_dirty: false,
                        content_revision: 0,
                        disk: crate::linkage::DiskState::default(),
                    });
                }
                if let Some(text) = &record.text {
                    let mut file = LoadedFile {
                        id: record.id,
                        path: record.path.clone(),
                        name: record.name.clone(),
                        bytes: Vec::new(),
                        is_binary: false,
                        lines: Vec::new(),
                        view: record.view,
                        language_override: record.language_override.clone(),
                        draft_text: Some(text.clone()),
                        editor_state: editor::EditorState::default(),
                        encoding: record.encoding,
                        saved_bytes: record.baseline.clone().unwrap_or_default(),
                        character_count: 0,
                        saved_large_revision: None,
                        bytes_dirty: false,
                        content_revision: 0,
                        saved_encoding: record.saved_encoding.unwrap_or(record.encoding),
                        source_path: record.source_path.clone(),
                        disk: crate::linkage::DiskState::default(),
                    };
                    file.sync_draft();
                    Some(file)
                } else {
                    let bytes = std::fs::read(&record.path).ok()?;
                    let is_binary = detect_binary(&bytes);
                    let (encoding, text) = if is_binary {
                        (Encoding::default(), String::new())
                    } else {
                        record.encoding.decode_as(&bytes).map_or_else(
                            || Encoding::decode(&bytes),
                            |text| (record.encoding, text),
                        )
                    };
                    let character_count = text.chars().count();
                    Some(LoadedFile {
                        id: record.id,
                        path: record.path.clone(),
                        name: record.name.clone(),
                        saved_bytes: bytes.clone(),
                        bytes,
                        is_binary,
                        lines: if is_binary || text.len() >= crate::large_editor::THRESHOLD {
                            Vec::new()
                        } else {
                            text.split('\n').map(str::to_owned).collect()
                        },
                        view: if is_binary {
                            ViewMode::Hex
                        } else {
                            record.view
                        },
                        draft_text: (!is_binary).then_some(text),
                        language_override: record.language_override.clone(),
                        editor_state: editor::EditorState::default(),
                        encoding,
                        character_count,
                        saved_large_revision: None,
                        bytes_dirty: false,
                        content_revision: 0,
                        saved_encoding: encoding,
                        source_path: record.source_path.clone(),
                        disk: crate::linkage::DiskState::default(),
                    })
                }
            })
            .collect()
    }
}

pub(crate) fn session_path() -> Option<PathBuf> {
    if let Some(directory) = crate::new_window::directory() { return Some(directory.join("recovery/session.json")); }
    eframe::storage_dir("LiteText").map(|p| p.join("recovery").join("session.json"))
}

/// Same-directory temporary write + sync + atomic replacement. Failed writes retain the old file.
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    atomic_write_with(path, |writer| writer.write_all(bytes))
}

fn atomic_write_with(
    path: &Path,
    write: impl FnOnce(&mut dyn Write) -> io::Result<()>,
) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    if let Ok(metadata) = std::fs::metadata(path) {
        if metadata.permissions().readonly() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "File is read-only",
            ));
        }
        temp.as_file().set_permissions(metadata.permissions())?;
    }
    {
        let mut writer = io::BufWriter::new(temp.as_file_mut());
        write(&mut writer)?;
        writer.flush()?;
    }
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

fn preserve_invalid(path: &Path) -> io::Result<()> {
    // Preserve a malformed/unknown previous snapshot instead of silently destroying it.
    if path.exists() && load(path).is_err() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::fs::copy(path, path.with_extension(format!("invalid-{stamp}.json")))?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn store(path: &Path, session: &Session) -> io::Result<()> {
    preserve_invalid(path)?;
    atomic_write_with(path, |writer| {
        serde_json::to_writer(writer, session).map_err(io::Error::other)
    })
}

/// Borrow buffers and stream directly into the atomic temporary file: no full snapshot clones
/// and no extra JSON-sized allocation. Version 1 remains readable by earlier builds.
pub(crate) fn store_buffers(
    path: &Path,
    files: &[LoadedFile],
    active_id: Option<u64>,
    validate: bool,
) -> io::Result<()> {
    store_buffers_inner(path, files, active_id, validate, false)
}

pub(crate) fn archive_buffers(
    path: &Path,
    files: &[LoadedFile],
    active_id: Option<u64>,
) -> io::Result<()> {
    store_buffers_inner(path, files, active_id, false, true)
}

fn store_buffers_inner(
    path: &Path,
    files: &[LoadedFile],
    active_id: Option<u64>,
    validate: bool,
    archive: bool,
) -> io::Result<()> {
    #[derive(Serialize)]
    struct BorrowedRecord<'a> {
        id: u64,
        path: &'a Path,
        source_path: &'a Option<PathBuf>,
        name: &'a str,
        view: ViewMode,
        language_override: &'a Option<String>,
        encoding: Encoding,
        saved_encoding: Encoding,
        text: Option<&'a str>,
        baseline: Option<&'a [u8]>,
        raw: Option<&'a [u8]>,
    }
    #[derive(Serialize)]
    struct BorrowedSession<'a> {
        version: u32,
        active_id: Option<u64>,
        files: Vec<BorrowedRecord<'a>>,
    }
    if validate {
        preserve_invalid(path)?;
    }
    let snapshot = BorrowedSession {
        version: 1,
        active_id,
        files: files
            .iter()
            .map(|f| {
                let dirty = f.is_modified() || archive;
                BorrowedRecord {
                    id: f.id,
                    path: &f.path,
                    source_path: &f.source_path,
                    name: &f.name,
                    view: f.view,
                    language_override: &f.language_override,
                    encoding: f.encoding,
                    saved_encoding: f.saved_encoding,
                    text: if dirty { f.draft_text.as_deref() } else { None },
                    baseline: dirty.then_some(f.saved_bytes.as_slice()),
                    raw: (f.is_binary && (f.path.as_os_str().is_empty() || archive))
                        .then_some(f.bytes.as_slice()),
                }
            })
            .collect(),
    };
    atomic_write_with(path, |writer| {
        serde_json::to_writer(writer, &snapshot).map_err(io::Error::other)
    })
}

pub(crate) fn load(path: &Path) -> io::Result<Option<Session>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let session: Session = serde_json::from_slice(&bytes)?;
    if session.version != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Unsupported recovery version",
        ));
    }
    Ok(Some(session))
}

#[cfg(test)]
pub(crate) fn save_document(file: &mut LoadedFile, target: &Path) -> io::Result<()> {
    save_document_checked(file, target, None)
}

/// expected_target is the exact version approved by the overwrite confirmation.
pub(crate) fn save_document_checked(
    file: &mut LoadedFile,
    target: &Path,
    expected_target: Option<&[u8]>,
) -> io::Result<()> {
    let text = file
        .draft_text
        .as_ref()
        .ok_or_else(|| io::Error::other("Binary editing is not supported"))?;
    // A recovered buffer must not overwrite disk changes made by another application.
    let same_target = !file.path.as_os_str().is_empty()
        && crate::linkage::resolved(&file.path) == crate::linkage::resolved(target);
    if same_target {
        match std::fs::read(target) {
            Ok(bytes) if bytes != file.saved_bytes => {
                return Err(io::Error::other(
                    "File changed on disk; original file was not overwritten",
                ));
            }
            Err(error) => return Err(error),
            _ => {}
        }
    } else {
        match std::fs::read(target) {
            Ok(bytes) if expected_target != Some(bytes.as_slice()) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "Target exists; explicit overwrite confirmation required",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if expected_target.is_some() {
                    return Err(io::Error::other("Target changed after confirmation"));
                }
            }
            Err(error) => return Err(error),
        }
    }
    let bytes = file.encoding.encode(text)?;
    // Preserve symbolic links by replacing their target rather than the link itself.
    let destination = std::fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf());
    // Recheck immediately before replacement; atomic write prevents partial output, not
    // simultaneous writes by uncooperative applications during this final small window.
    if same_target {
        if std::fs::read(&destination)? != file.saved_bytes {
            return Err(io::Error::other("File changed while saving"));
        }
    } else if let Some(expected) = expected_target {
        if std::fs::read(&destination)? != expected {
            return Err(io::Error::other("Target changed while saving"));
        }
    } else if destination.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Target appeared while saving",
        ));
    }
    atomic_write(&destination, &bytes)?;
    file.path = destination;
    file.name = file
        .path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    file.bytes = bytes.clone();
    file.saved_bytes = bytes;
    file.bytes_dirty = false;
    file.saved_large_revision = Some(file.editor_state.large.revision);
    file.saved_encoding = file.encoding;
    file.content_revision += 1;
    file.source_path = None;
    file.disk = crate::linkage::DiskState::default();
    file.editor_state.changes = crate::changes::Changes::default();
    file.prepare_changes();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LiteTextApp;

    #[test]
    fn common_encodings_round_trip_and_refuse_unrepresentable_characters() {
        for (encoding, text) in [
            (Encoding::Utf8, "中文🙂\r\n"),
            (Encoding::Utf8Bom, "中文🙂\r\n"),
            (Encoding::Utf16Le, "中文🙂\r\n"),
            (Encoding::Utf16Be, "中文🙂\r\n"),
            (Encoding::Gbk, "简体中文\r\n"),
            (Encoding::Gb18030, "中文🙂\r\n"),
            (Encoding::Big5, "繁體中文\r\n"),
            (Encoding::ShiftJis, "日本語テスト\r\n"),
            (Encoding::Windows1252, "café €\r\n"),
        ] {
            let bytes = encoding.encode(text).unwrap();
            assert_eq!(
                encoding.decode_as(&bytes).as_deref(),
                Some(text),
                "{encoding:?}"
            );
        }
        for encoding in [
            Encoding::Gbk,
            Encoding::Big5,
            Encoding::ShiftJis,
            Encoding::Windows1252,
        ] {
            assert!(encoding.encode("中文🙂").is_err(), "{encoding:?}");
        }
    }

    #[test]
    fn encoding_only_changes_recover_and_save_without_touching_original_until_manual_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("document.txt");
        let snapshot = dir.path().join("session.json");
        let original = "简体中文\r\nhello\r\n";
        std::fs::write(&path, original.as_bytes()).unwrap();
        let mut app = LiteTextApp {
            recovery_path_override: Some(snapshot.clone()),
            ..Default::default()
        };
        app.open_paths(vec![path.clone()]);
        app.active_mut()
            .unwrap()
            .change_encoding(Encoding::Gbk)
            .unwrap();
        assert!(app.active().unwrap().is_modified());
        assert_eq!(app.active().unwrap().draft_text.as_deref(), Some(original));
        app.write_recovery().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original.as_bytes());
        let mut restored = load(&snapshot).unwrap().unwrap().restore();
        assert_eq!(restored[0].encoding, Encoding::Gbk);
        assert_eq!(restored[0].saved_encoding, Encoding::Utf8);
        assert!(restored[0].is_modified());
        save_document(&mut restored[0], &path).unwrap();
        assert!(!restored[0].is_modified());
        assert_eq!(
            std::fs::read(&path).unwrap(),
            Encoding::Gbk.encode(original).unwrap()
        );
        store_buffers(&snapshot, &restored, Some(restored[0].id), false).unwrap();
        let clean = load(&snapshot).unwrap().unwrap().restore();
        assert_eq!(clean[0].encoding, Encoding::Gbk);
        assert_eq!(clean[0].draft_text.as_deref(), Some(original));
        assert!(!clean[0].is_modified());
        let before = restored[0].bytes.clone();
        assert!(restored[0].change_encoding(Encoding::Windows1252).is_err());
        assert_eq!(restored[0].encoding, Encoding::Gbk);
        assert_eq!(restored[0].bytes, before);
        assert_eq!(restored[0].draft_text.as_deref(), Some(original));
        let mut invalid = LiteTextApp::default();
        let invalid_path = dir.path().join("invalid.txt");
        std::fs::write(&invalid_path, [b'a', 0xFF, b'b']).unwrap();
        invalid.open_paths(vec![invalid_path.clone()]);
        assert_eq!(invalid.active().unwrap().encoding, Encoding::Lossy);
        assert!(
            invalid
                .active_mut()
                .unwrap()
                .change_encoding(Encoding::Utf8)
                .is_err()
        );
        assert_eq!(std::fs::read(invalid_path).unwrap(), [b'a', 0xFF, b'b']);
    }

    fn edit(app: &mut LiteTextApp, text: &str) {
        let file = app.active_mut().unwrap();
        file.draft_text = Some(text.to_owned());
        file.sync_draft();
    }

    #[test]
    fn large_edits_lazy_hex_revision_save_undo_and_streamed_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.txt");
        let snapshot = dir.path().join("session.json");
        let original = "中文🙂 abc\r\n".repeat(30_000);
        let bytes = Encoding::Utf16Le.encode(&original).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let mut app = LiteTextApp {
            recovery_path_override: Some(snapshot.clone()),
            ..Default::default()
        };
        app.open_paths(vec![path.clone()]);
        let ctx = eframe::egui::Context::default();
        let render = |app: &mut LiteTextApp, events, modifiers, focus| {
            let _ = ctx.run(
                eframe::egui::RawInput {
                    events,
                    modifiers,
                    screen_rect: Some(eframe::egui::Rect::from_min_size(
                        eframe::egui::Pos2::ZERO,
                        eframe::egui::vec2(800.0, 600.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    eframe::egui::CentralPanel::default().show(ctx, |ui| {
                        crate::draft_editor(
                            ui,
                            app.active_mut().unwrap(),
                            &crate::DARK,
                            focus,
                            crate::Lang::En,
                        );
                    });
                },
            );
        };
        render(&mut app, vec![], Default::default(), true);
        assert!(!app.active().unwrap().is_modified());
        app.active_mut()
            .unwrap()
            .change_encoding(Encoding::Utf8Bom)
            .unwrap();
        assert!(app.active().unwrap().is_modified());
        assert_eq!(app.active().unwrap().byte_len(), original.len() + 3);
        app.active_mut()
            .unwrap()
            .change_encoding(Encoding::Utf16Le)
            .unwrap();
        assert!(!app.active().unwrap().is_modified());
        render(
            &mut app,
            vec![eframe::egui::Event::Text("新增🙂\n".to_owned())],
            Default::default(),
            false,
        );
        let file = app.active_mut().unwrap();
        assert!(file.is_modified());
        assert!(file.bytes.is_empty());
        assert!(file.lines.is_empty());
        assert!(file.bytes_dirty);
        let expected = file.draft_text.clone().unwrap();
        assert_eq!(file.character_count, expected.chars().count());
        assert_eq!(
            file.byte_len(),
            Encoding::Utf16Le.encode(&expected).unwrap().len()
        );
        file.ensure_bytes();
        assert_eq!(file.bytes, Encoding::Utf16Le.encode(&expected).unwrap());
        app.write_recovery().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let first = std::fs::metadata(&snapshot).unwrap().modified().unwrap();
        app.write_recovery().unwrap();
        assert_eq!(
            std::fs::metadata(&snapshot).unwrap().modified().unwrap(),
            first
        );
        std::fs::remove_file(&snapshot).unwrap();
        app.write_recovery().unwrap();
        assert!(
            snapshot.exists(),
            "deleted backup must be recreated even without edits"
        );
        std::fs::write(&snapshot, b"corrupted externally").unwrap();
        app.write_recovery().unwrap();
        assert!(std::fs::read_dir(dir.path()).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("invalid-")
        }));
        let restored = load(&snapshot).unwrap().unwrap().restore();
        assert_eq!(restored[0].draft_text.as_deref(), Some(expected.as_str()));
        assert!(restored[0].is_modified());
        save_document(app.active_mut().unwrap(), &path).unwrap();
        assert!(!app.active().unwrap().is_modified());
        let key = |shift| eframe::egui::Event::Key {
            key: eframe::egui::Key::Z,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: if shift {
                eframe::egui::Modifiers::COMMAND | eframe::egui::Modifiers::SHIFT
            } else {
                eframe::egui::Modifiers::COMMAND
            },
        };
        render(
            &mut app,
            vec![key(false)],
            eframe::egui::Modifiers::COMMAND,
            false,
        );
        assert!(app.active().unwrap().is_modified());
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some(original.as_str())
        );
        render(
            &mut app,
            vec![key(true)],
            eframe::egui::Modifiers::COMMAND | eframe::egui::Modifiers::SHIFT,
            false,
        );
        assert!(!app.active().unwrap().is_modified());
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some(expected.as_str())
        );
        std::fs::write(&path, b"external change").unwrap();
        assert!(save_document(app.active_mut().unwrap(), &path).is_err());
    }

    #[test]
    fn recovery_preserves_untitled_buffers_order_active_tab_and_original_files() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("original.txt");
        let snapshot = dir.path().join("recovery/session.json");
        std::fs::write(&original, b"on disk\r\n").unwrap();
        let mut app = LiteTextApp::default();
        app.open_paths(vec![original.clone()]);
        edit(&mut app, "edited\r\n中文");
        app.new_document();
        edit(&mut app, "untitled\n🙂\n");
        app.new_document(); // Empty untitled files should also survive a restart.
        store(&snapshot, &Session::capture(&app.files, app.active_id)).unwrap();
        assert_eq!(std::fs::read(&original).unwrap(), b"on disk\r\n");
        let session = load(&snapshot).unwrap().unwrap();
        assert_eq!(session.active_id, app.active_id);
        let files = session.restore();
        assert_eq!(files.len(), 3);
        assert_eq!(
            files.iter().map(|f| f.id).collect::<Vec<_>>(),
            app.files.iter().map(|f| f.id).collect::<Vec<_>>()
        );
        assert_eq!(files[0].draft_text.as_deref(), Some("edited\r\n中文"));
        assert_eq!(files[1].draft_text.as_deref(), Some("untitled\n🙂\n"));
        assert_eq!(files[2].draft_text.as_deref(), Some(""));
        assert!(files.iter().all(LoadedFile::is_modified));
        assert_eq!(files[0].saved_bytes, b"on disk\r\n");
    }

    #[test]
    fn manual_save_commits_only_selected_document_and_clears_modified_state() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("saved.txt");
        let mut app = LiteTextApp::default();
        app.new_document();
        edit(&mut app, "first\n");
        save_document(app.active_mut().unwrap(), &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"first\n");
        assert!(!app.active().unwrap().is_modified());
        assert_eq!(app.active().unwrap().name, "saved.txt");
        edit(&mut app, "second\n");
        let snapshot = dir.path().join("session.json");
        store(&snapshot, &Session::capture(&app.files, app.active_id)).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"first\n");
        save_document(app.active_mut().unwrap(), &target).unwrap();
        store(&snapshot, &Session::capture(&app.files, app.active_id)).unwrap();
        let session = load(&snapshot).unwrap().unwrap();
        assert!(session.files[0].text.is_none());
        assert_eq!(session.restore()[0].draft_text.as_deref(), Some("second\n"));
    }

    #[test]
    fn manual_save_preserves_bom_utf16_newlines_and_trailing_newline() {
        let dir = tempfile::tempdir().unwrap();
        for encoding in [
            Encoding::Utf8,
            Encoding::Utf8Bom,
            Encoding::Utf16Le,
            Encoding::Utf16Be,
        ] {
            let target = dir.path().join("encoded.txt");
            let bytes = encoding.encode("中文\r\nhello\r\n").unwrap();
            assert!(!detect_binary(&bytes));
            std::fs::write(&target, bytes).unwrap();
            let mut app = LiteTextApp::default();
            app.open_paths(vec![target.clone()]);
            edit(&mut app, "中文🙂\r\nupdated\r\n");
            save_document(app.active_mut().unwrap(), &target).unwrap();
            assert_eq!(
                std::fs::read(&target).unwrap(),
                encoding.encode("中文🙂\r\nupdated\r\n").unwrap()
            );
        }
    }

    #[test]
    fn recovered_edits_survive_deleted_original_and_external_changes_are_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("original.txt");
        std::fs::write(&target, "original").unwrap();
        let mut app = LiteTextApp::default();
        app.open_paths(vec![target.clone()]);
        edit(&mut app, "my edits");
        let session = Session::capture(&app.files, app.active_id);
        std::fs::write(&target, "external edits").unwrap();
        let mut files = session.restore();
        assert_eq!(files[0].draft_text.as_deref(), Some("my edits"));
        assert!(save_document(&mut files[0], &target).is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "external edits");
        std::fs::remove_file(&target).unwrap();
        let mut files = session.restore();
        assert_eq!(files[0].draft_text.as_deref(), Some("my edits"));
        assert!(save_document(&mut files[0], &target).is_err());
        assert!(
            !target.exists(),
            "deleted originals must not be silently recreated"
        );
        crate::linkage::detach(&mut files[0]);
        save_document(&mut files[0], &target).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "my edits");
    }

    #[test]
    fn clean_files_are_reread_and_binary_files_remain_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let text = dir.path().join("text.txt");
        let binary = dir.path().join("binary.bin");
        std::fs::write(&text, "before").unwrap();
        std::fs::write(&binary, [0, 1, 2, 3]).unwrap();
        let mut app = LiteTextApp::default();
        app.open_paths(vec![text.clone(), binary]);
        let session = Session::capture(&app.files, app.active_id);
        std::fs::write(&text, "after").unwrap();
        let files = session.restore();
        assert_eq!(files[0].draft_text.as_deref(), Some("after"));
        assert!(files[1].draft_text.is_none());
        assert!(files[1].is_binary);
        std::fs::remove_file(text).unwrap();
        assert_eq!(session.restore().len(), 1);
    }

    #[test]
    fn invalid_encoding_cannot_silently_overwrite_original_and_save_failure_keeps_buffer() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("invalid.txt");
        for bytes in [vec![0x80, 0x81], vec![0xFF, 0xFE, 0x00, 0xD8]] {
            std::fs::write(&target, &bytes).unwrap();
            let mut app = LiteTextApp::default();
            app.open_paths(vec![target.clone()]);
            edit(&mut app, "edited");
            assert!(save_document(app.active_mut().unwrap(), &target).is_err());
            assert_eq!(std::fs::read(&target).unwrap(), bytes);
            assert_eq!(app.active().unwrap().draft_text.as_deref(), Some("edited"));
        }
        let mut app = LiteTextApp::default();
        app.new_document();
        edit(&mut app, "still here");
        assert!(save_document(app.active_mut().unwrap(), dir.path()).is_err());
        assert!(app.active().unwrap().path.as_os_str().is_empty());
        assert_eq!(
            app.active().unwrap().draft_text.as_deref(),
            Some("still here")
        );
    }

    #[test]
    fn snapshot_replacement_is_complete_and_corrupt_old_records_are_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("session.json");
        std::fs::write(&target, "malformed recovery").unwrap();
        assert!(load(&target).is_err());
        let mut app = LiteTextApp::default();
        app.new_document();
        store(&target, &Session::capture(&app.files, app.active_id)).unwrap();
        let archived = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .find(|e| e.file_name().to_string_lossy().contains("invalid-"))
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(archived.path()).unwrap(),
            "malformed recovery"
        );
        app.files.clear();
        store(&target, &Session::capture(&app.files, None)).unwrap();
        assert!(load(&target).unwrap().unwrap().restore().is_empty());
    }

    #[test]
    fn exit_hook_saves_latest_input_and_startup_restores_order_and_active_tab() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        let mut app = LiteTextApp {
            recovery_path_override: Some(path.clone()),
            ..Default::default()
        };
        app.new_document();
        edit(&mut app, "last keystroke before exit");
        app.new_document();
        let first = app.files[0].id;
        app.active_id = Some(first);
        assert_eq!(
            eframe::App::auto_save_interval(&app),
            std::time::Duration::from_secs(5)
        );
        eframe::App::on_exit(&mut app, None);
        let mut restarted = LiteTextApp {
            recovery_path_override: Some(path),
            ..Default::default()
        };
        restarted.restore_session();
        assert_eq!(restarted.files.len(), 2);
        assert_eq!(restarted.active_id, Some(first));
        assert_eq!(
            restarted.active().unwrap().draft_text.as_deref(),
            Some("last keystroke before exit")
        );
        assert_eq!(restarted.next_file_id, app.files[1].id + 1);
    }

    #[test]
    fn empty_scratch_tabs_close_silently_even_after_edits_and_large_buffer_clear() {
        for large in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let snapshot = dir.path().join("session.json");
            let mut app = LiteTextApp {
                recovery_path_override: Some(snapshot.clone()),
                ..Default::default()
            };
            app.new_document();
            let blank = app.active_id.unwrap();
            app.request_tab_action(crate::TabAction::Close(blank));
            assert!(app.files.is_empty() && app.pending_close.is_none());
            app.new_document();
            let ctx = eframe::egui::Context::default();
            let original = "content\n".repeat(if large { 40_000 } else { 1 });
            crate::text_tools::replace_range(
                app.active_mut().unwrap(),
                &ctx,
                0..0,
                original.clone(),
            );
            assert!(app.active().unwrap().has_unsaved_changes());
            crate::text_tools::replace_range(
                app.active_mut().unwrap(),
                &ctx,
                0..original.len(),
                String::new(),
            );
            let file = app.active().unwrap();
            assert_eq!(file.editor_state.large.active, large);
            assert!(!file.has_unsaved_changes());
            app.request_tab_action(crate::TabAction::Close(file.id));
            assert!(app.files.is_empty() && app.pending_close.is_none());
            assert!(load(&snapshot).unwrap().unwrap().restore().is_empty());
        }
    }

    #[test]
    fn saved_tabs_close_silently_but_new_unsaved_edits_and_cleared_saved_files_prompt() {
        for large in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("saved.txt");
            let mut app = LiteTextApp {
                recovery_path_override: Some(dir.path().join("session.json")),
                ..Default::default()
            };
            app.new_document();
            let ctx = eframe::egui::Context::default();
            let original = "content\n".repeat(if large { 40_000 } else { 1 });
            crate::text_tools::replace_range(
                app.active_mut().unwrap(),
                &ctx,
                0..0,
                original.clone(),
            );
            let id = app.active_id.unwrap();
            assert!(app.commit_save(id, &path, None));
            assert!(!app.active().unwrap().has_unsaved_changes());
            app.request_tab_action(crate::TabAction::Close(id));
            assert!(app.files.is_empty() && app.pending_close.is_none());
            app.open_paths(vec![path.clone()]);
            let id = app.active_id.unwrap();
            crate::text_tools::replace_range(
                app.active_mut().unwrap(),
                &ctx,
                0..original.len(),
                String::new(),
            );
            app.request_tab_action(crate::TabAction::Close(id));
            assert!(app.pending_close.is_some());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
            app.pending_close = None;
            assert!(app.save_file(id));
            app.request_tab_action(crate::TabAction::Close(id));
            assert!(app.files.is_empty() && app.pending_close.is_none());
        }
    }

    #[test]
    fn batch_close_only_prompts_for_nonempty_unsaved_work() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = LiteTextApp {
            recovery_path_override: Some(dir.path().join("session.json")),
            ..Default::default()
        };
        app.new_document();
        let keep = app.active_id.unwrap();
        app.new_document();
        app.new_document();
        app.request_tab_action(crate::TabAction::CloseOthers(keep));
        assert!(app.pending_close.is_none());
        assert_eq!(app.files.len(), 1);
        app.new_document();
        edit(&mut app, "unsaved content");
        app.new_document();
        app.request_tab_action(crate::TabAction::CloseOthers(keep));
        assert!(app.pending_close.is_some());
        let ids = app.closing_ids(app.pending_close.as_ref().unwrap());
        assert_eq!(
            app.files
                .iter()
                .filter(|f| ids.contains(&f.id) && f.has_unsaved_changes())
                .count(),
            1
        );
    }

    #[test]
    fn close_actions_prompt_before_discarding_modified_buffers_and_update_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        let mut app = LiteTextApp {
            recovery_path_override: Some(path.clone()),
            ..Default::default()
        };
        app.new_document();
        edit(&mut app, "keep unless confirmed");
        let id = app.active_id.unwrap();
        app.request_tab_action(crate::TabAction::Close(id));
        assert_eq!(app.files.len(), 1);
        assert!(app.pending_close.is_some());
        app.backup_or_report();
        assert_eq!(load(&path).unwrap().unwrap().restore().len(), 1);
        let action = app.pending_close.take().unwrap();
        app.apply_tab_action(action);
        assert!(load(&path).unwrap().unwrap().restore().is_empty());
    }

    #[test]
    fn framework_auto_save_only_writes_recovery_while_manual_save_commits_disk() {
        #[derive(Default)]
        struct Storage(std::collections::HashMap<String, String>);
        impl eframe::Storage for Storage {
            fn get_string(&self, key: &str) -> Option<String> {
                self.0.get(key).cloned()
            }
            fn set_string(&mut self, key: &str, value: String) {
                self.0.insert(key.to_owned(), value);
            }
            fn flush(&mut self) {}
        }
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("original.txt");
        let snapshot = dir.path().join("session.json");
        std::fs::write(&original, "original").unwrap();
        let mut app = LiteTextApp {
            recovery_path_override: Some(snapshot.clone()),
            ..Default::default()
        };
        app.open_paths(vec![original.clone()]);
        edit(&mut app, "pending edits");
        eframe::App::save(&mut app, &mut Storage::default());
        assert_eq!(std::fs::read_to_string(&original).unwrap(), "original");
        assert_eq!(
            load(&snapshot).unwrap().unwrap().restore()[0]
                .draft_text
                .as_deref(),
            Some("pending edits")
        );
        assert!(app.save_file(app.active_id.unwrap()));
        assert_eq!(std::fs::read_to_string(&original).unwrap(), "pending edits");
        assert!(!app.active().unwrap().is_modified());
    }
}

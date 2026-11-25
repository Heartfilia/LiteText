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
    pub(crate) const CHOICES: [Self; 9] = [Self::Utf8, Self::Utf8Bom, Self::Utf16Le, Self::Utf16Be, Self::Gbk, Self::Gb18030, Self::Big5, Self::ShiftJis, Self::Windows1252];

    /// Explicit reopening never substitutes invalid input or guesses another encoding.
    /// BOM variants require their BOM so a clean buffer round-trips exactly.
    pub(crate) fn decode_explicit(self, bytes: &[u8]) -> io::Result<String> {
        let text = if self == Self::Utf8 {
            if bytes.starts_with(b"\xEF\xBB\xBF") { None }
            else { std::str::from_utf8(bytes).ok().map(str::to_owned) }
        } else { self.decode_as(bytes) };
        text.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, format!("Input is not valid, lossless {} (BOM variants require the matching BOM)", self.name())))
    }
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
    #[serde(default)]
    pinned: bool,
    id: u64,
    path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_path: Option<PathBuf>,
    name: String,
    view: ViewMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    language_override: Option<String>,
    encoding: Encoding,
    #[serde(default)]
    saved_encoding: Option<Encoding>,
    /// Only modified / untitled buffers need a recovery copy of their content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(default, with = "base64_option", skip_serializing_if = "Option::is_none")]
    baseline: Option<Vec<u8>>,
    #[serde(default, with = "base64_option", skip_serializing_if = "Option::is_none")]
    raw: Option<Vec<u8>>,
    #[serde(default)]
    line_ending: crate::line_endings::LineEnding,
    /// Per-tab Markdown view (source / live / split). Older snapshots omit it
    /// and fall back to the source view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    markdown_mode: Option<crate::markdown::Mode>,
    /// Copy of the unsaved text in the scratch directory. When present it is
    /// authoritative and the session stores no inline body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mirror: Option<PathBuf>,
}

/// Binary buffers are stored as base64 strings instead of JSON number arrays.
/// A modified document's baseline can be hundreds of kilobytes; as numbers it
/// expanded several times and made session restore spend most of its time in
/// `serde_json`. The deserializer still accepts the old array form so existing
/// recovery files keep working.
mod base64_option {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde::{
        Deserializer, Serializer,
        de::{self, SeqAccess, Visitor},
    };
    use std::fmt;

    pub(super) fn serialize<S>(value: &Option<Vec<u8>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(bytes) => serializer.serialize_some(&STANDARD.encode(bytes)),
            None => serializer.serialize_none(),
        }
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<Option<Vec<u8>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct OptVisitor;
        impl<'de> Visitor<'de> for OptVisitor {
            type Value = Option<Vec<u8>>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("base64 bytes, a byte array, or null")
            }

            fn visit_none<E>(self) -> Result<Self::Value, E> {
                Ok(None)
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(None)
            }

            fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
            where
                D: Deserializer<'de>,
            {
                deserializer.deserialize_any(BytesVisitor).map(Some)
            }
        }

        struct BytesVisitor;
        impl<'de> Visitor<'de> for BytesVisitor {
            type Value = Vec<u8>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("base64 text or a byte array")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                STANDARD.decode(value).map_err(de::Error::custom)
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut bytes = Vec::with_capacity(seq.size_hint().unwrap_or(0));
                while let Some(byte) = seq.next_element::<u8>()? {
                    bytes.push(byte);
                }
                Ok(bytes)
            }
        }

        deserializer.deserialize_option(OptVisitor)
    }
}

/// The streaming writer borrows its buffers instead of cloning them, so it
/// needs the same compact representation as `base64_option`.
mod base64_borrowed_option {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde::Serializer;

    pub(super) fn serialize<S>(value: &Option<&[u8]>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(bytes) => serializer.serialize_some(&STANDARD.encode(bytes)),
            None => serializer.serialize_none(),
        }
    }
}

/// Version-1 session metadata for one untitled draft whose content lives in a
/// scratch mirror. Version 2 stores every tab as a record instead; this type is
/// kept so older snapshots still restore pinned/language/Markdown state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct ScratchMeta {
    pub(crate) file_name: String,
    #[serde(default)]
    pub(crate) pinned: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) language: Option<String>,
    /// Per-tab Markdown view for a mirrored draft.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) markdown_mode: Option<crate::markdown::Mode>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Session {
    version: u32,
    pub(crate) active_id: Option<u64>,
    /// File name of the active untitled draft when its content lives in a
    /// scratch mirror rather than in this session.
    #[serde(default)]
    scratch_active: Option<String>,
    /// Pin flag and manual language choice for mirrored drafts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    scratch_meta: Vec<ScratchMeta>,
    /// Pin-only list written by an earlier build; merged when reading.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    scratch_pinned: Vec<String>,
    files: Vec<Record>,
}

/// Copy at most a bounded slice of live buffers each frame. The caller checks
/// the complete document/version key before every step and before publication.
pub(crate) struct Capture {
    session: Session,
    /// One index per record. Every tab has a record; mirrored tabs keep their
    /// body in the scratch directory and only stream the small metadata here.
    included: Vec<usize>,
    record: usize,
    field: usize,
}

fn included_indices(files: &[LoadedFile]) -> Vec<usize> {
    (0..files.len()).collect()
}

impl Capture {
    pub(crate) fn new(files: &[LoadedFile], active_id: Option<u64>) -> Self {
        let included = included_indices(files);
        let records: Vec<Record> = included.iter().map(|&index| {
            let file = &files[index];
            let modified = file.is_modified();
            // A scratch mirror owns the text. Storing it in the session too
            // would double the snapshot size for every unsaved edit.
            let mirror = file.scratch_path.clone();
            let inline = mirror.is_none();
            Record {
                pinned: file.pinned,
                id: file.id, path: file.path.clone(), source_path: file.source_path.clone(),
                name: file.name.clone(), view: file.view, language_override: file.language_override.clone(),
                encoding: file.encoding, saved_encoding: Some(file.saved_encoding),
                text: (inline
                    && (modified || (file.pinned && file.is_untitled()))
                    && file.draft_text.is_some())
                .then(String::new),
                baseline: (inline && modified).then(Vec::new),
                raw: (file.is_binary && file.path.as_os_str().is_empty()).then(Vec::new),
                line_ending: file.line_ending,
                markdown_mode: Some(file.markdown_mode),
                mirror,
            }
        }).collect();
        Self {
            record: 0, field: 0, included,
            session: Session {
                version: 2,
                active_id,
                scratch_active: None,
                scratch_meta: Vec::new(),
                scratch_pinned: Vec::new(),
                files: records,
            },
        }
    }

    pub(crate) fn step(&mut self, files: &[LoadedFile], mut budget: usize) -> bool {
        let _timing = crate::timing::phase("recovery.snapshot_slice");
        while self.record < self.session.files.len() && budget > 0 {
            let record = &mut self.session.files[self.record];
            let file = &files[self.included[self.record]];
            let copied = match self.field {
                0 => if let Some(target) = &mut record.text {
                    let source = file.draft_text.as_deref().unwrap();
                    if target.is_empty() { target.reserve_exact(source.len()); }
                    let start = target.len();
                    let mut end = (start + budget).min(source.len());
                    while !source.is_char_boundary(end) { end -= 1; }
                    if end == start && start < source.len() { return false; }
                    target.push_str(&source[start..end]);
                    if end == source.len() { self.field += 1; }
                    end - start
                } else { self.field += 1; 0 },
                1 | 2 => {
                    let (target, source) = if self.field == 1 { (&mut record.baseline, &file.saved_bytes) } else { (&mut record.raw, &file.bytes) };
                    if let Some(target) = target {
                        if target.is_empty() { target.reserve_exact(source.len()); }
                        let start = target.len();
                        let end = (start + budget).min(source.len());
                        target.extend_from_slice(&source[start..end]);
                        if end == source.len() { self.field += 1; }
                        end - start
                    } else { self.field += 1; 0 }
                }
                _ => { self.record += 1; self.field = 0; 0 }
            };
            budget -= copied;
        }
        self.record == self.session.files.len()
    }

    pub(crate) fn finish(self) -> Session { self.session }
}

impl Session {
    pub(crate) fn has_files(&self) -> bool {
        !self.files.is_empty()
            || self.scratch_active.is_some()
            || !self.scratch_meta.is_empty()
            || !self.scratch_pinned.is_empty()
    }

    pub(crate) fn scratch_active(&self) -> Option<&str> {
        self.scratch_active.as_deref()
    }

    /// Metadata for mirrored drafts, including the pin-only list written by an
    /// earlier build.
    pub(crate) fn scratch_meta(&self) -> Vec<ScratchMeta> {
        let mut meta = self.scratch_meta.clone();
        for file_name in &self.scratch_pinned {
            if !meta.iter().any(|entry| &entry.file_name == file_name) {
                meta.push(ScratchMeta {
                    file_name: file_name.clone(),
                    pinned: true,
                    language: None,
                    markdown_mode: None,
                });
            }
        }
        meta
    }

    #[cfg(test)]
    pub(crate) fn record_count_for_testing(&self) -> usize {
        self.files.len()
    }

    #[cfg(test)]
    pub(crate) fn capture(files: &[LoadedFile], active_id: Option<u64>) -> Self {
        let _timing = crate::timing::phase("recovery.snapshot");
        let included = included_indices(files);
        Self {
            version: 2,
            active_id,
            scratch_active: None,
            scratch_meta: Vec::new(),
            scratch_pinned: Vec::new(),
            files: included
                .iter()
                .map(|&index| {
                    let f = &files[index];
                    let mirror = f.scratch_path.clone();
                    let inline = mirror.is_none();
                    Record {
                        pinned: f.pinned,
                        id: f.id,
                        path: f.path.clone(),
                        source_path: f.source_path.clone(),
                        name: f.name.clone(),
                        view: f.view,
                        language_override: f.language_override.clone(),
                        encoding: f.encoding,
                        saved_encoding: Some(f.saved_encoding),
                        text: (inline && f.is_modified())
                            .then(|| f.draft_text.clone())
                            .flatten(),
                        baseline: (inline && f.is_modified())
                            .then(|| f.saved_bytes.clone()),
                        raw: (f.is_binary && f.path.as_os_str().is_empty())
                            .then(|| f.bytes.clone()),
                        line_ending: f.line_ending,
                        markdown_mode: Some(f.markdown_mode),
                        mirror,
                    }
                })
                .collect(),
        }
    }

    /// Claim before reading; occupied mirror-only records belong to their live
    /// owner. Inline records may recover independently without that mirror.
    pub(crate) fn restore_claimed(mut self) -> io::Result<(Vec<LoadedFile>, std::collections::HashMap<PathBuf, std::fs::File>)> {
        let mut leases = std::collections::HashMap::new();
        let mut failure = None;
        self.files.retain_mut(|record| {
            let Some(path) = record.mirror.as_ref() else { return true; };
            let key = crate::linkage::resolved(path);
            if leases.contains_key(&key) { return true; }
            match crate::scratch::claim(path) {
                Ok(lease) => { leases.insert(key, lease); true }
                Err(error) if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() => {
                    if record.text.is_some() { record.mirror = None; true } else { false }
                }
                Err(error) => {
                    // A lease failure must not discard unrelated session tabs.
                    // Recover readable content independently and never retain
                    // a write association to the unclaimed mirror.
                    if record.text.is_none() {
                        match std::fs::read(path) {
                            Ok(bytes) => record.text = Some(if record.path.as_os_str().is_empty() {
                                Encoding::decode(&bytes).1
                            } else {
                                record.encoding.decode_as(&bytes).unwrap_or_else(|| Encoding::decode(&bytes).1)
                            }),
                            Err(_) => { failure = Some(error); return true; }
                        }
                    }
                    record.mirror = None;
                    true
                },
            }
        });
        if let Some(error) = failure { return Err(error); }
        Ok((self.restore(), leases))
    }

    pub(crate) fn restore(&self) -> Vec<LoadedFile> {
        self.files
            .iter()
            .filter_map(|record| {
                if let Some(bytes) = &record.raw {
                    return Some(LoadedFile {
                        pinned: record.pinned,
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
                        line_ending: record.line_ending,
                        markdown_mode: record.markdown_mode.unwrap_or_default(),
                        markdown: crate::markdown::State::default(),
                        scratch_path: record.mirror.clone(),
                        saved_generation: 0,
                    });
                }
                // The scratch mirror is authoritative for unsaved text. Read
                // it with the document encoding so Save writes identical bytes.
                let mirrored = record.mirror.as_deref().and_then(|path| {
                    let bytes = std::fs::read(path).ok()?;
                    Some(if record.path.as_os_str().is_empty() {
                        Encoding::decode(&bytes).1
                    } else {
                        record
                            .encoding
                            .decode_as(&bytes)
                            .unwrap_or_else(|| Encoding::decode(&bytes).1)
                    })
                });
                let text = record.text.clone().or(mirrored);
                if let Some(text) = text {
                    let saved_bytes = record.baseline.clone().unwrap_or_else(|| {
                        if record.path.as_os_str().is_empty() {
                            Vec::new()
                        } else {
                            // The original file still holds the saved bytes; a
                            // missing file leaves them empty so Save can recreate it.
                            std::fs::read(&record.path).unwrap_or_default()
                        }
                    });
                    let mut file = LoadedFile {
                        pinned: record.pinned,
                        id: record.id,
                        path: record.path.clone(),
                        name: record.name.clone(),
                        bytes: Vec::new(),
                        is_binary: false,
                        lines: Vec::new(),
                        view: record.view,
                        language_override: record.language_override.clone(),
                        draft_text: Some(text),
                        editor_state: editor::EditorState::default(),
                        encoding: record.encoding,
                        saved_bytes,
                        character_count: 0,
                        saved_large_revision: None,
                        bytes_dirty: false,
                        content_revision: 0,
                        saved_encoding: record.saved_encoding.unwrap_or(record.encoding),
                        source_path: record.source_path.clone(),
                        disk: crate::linkage::DiskState::default(),
                        line_ending: record.line_ending,
                        markdown_mode: record.markdown_mode.unwrap_or_default(),
                        markdown: crate::markdown::State::default(),
                        scratch_path: record.mirror.clone(),
                        saved_generation: 0,
                    };
                    file.sync_draft();
                    return Some(file);
                }
                if record.path.as_os_str().is_empty() {
                    // A missing mirror must not silently drop an untitled tab.
                    let mut file = LoadedFile {
                        pinned: record.pinned,
                        id: record.id,
                        path: PathBuf::new(),
                        name: record.name.clone(),
                        bytes: Vec::new(),
                        is_binary: false,
                        lines: Vec::new(),
                        view: record.view,
                        language_override: record.language_override.clone(),
                        draft_text: Some(String::new()),
                        editor_state: editor::EditorState::default(),
                        encoding: record.encoding,
                        saved_bytes: Vec::new(),
                        character_count: 0,
                        saved_large_revision: None,
                        bytes_dirty: false,
                        content_revision: 0,
                        saved_encoding: record.saved_encoding.unwrap_or(record.encoding),
                        source_path: record.source_path.clone(),
                        disk: crate::linkage::DiskState::default(),
                        line_ending: record.line_ending,
                        markdown_mode: record.markdown_mode.unwrap_or_default(),
                        markdown: crate::markdown::State::default(),
                        scratch_path: None,
                        saved_generation: 0,
                    };
                    file.sync_draft();
                    return Some(file);
                }
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
                    pinned: record.pinned,
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
                    line_ending: record.line_ending,
                    markdown_mode: record.markdown_mode.unwrap_or_default(),
                    markdown: crate::markdown::State::default(),
                    scratch_path: None,
                    saved_generation: 0,
                })
            })
            .collect()
    }
}

/// Identity of a tab across snapshots. Real files are matched by their
/// resolved path; mirrored drafts by their mirror. An inline draft (no mirror)
/// is matched by name plus a content hash, so the identical copies written by
/// two snapshots collapse while two differently edited drafts of the same name
/// do not.
fn tab_key(record: &Record) -> (PathBuf, PathBuf) {
    if !record.path.as_os_str().is_empty() {
        return (crate::linkage::resolved(&record.path), PathBuf::new());
    }
    if let Some(mirror) = &record.mirror {
        return (PathBuf::new(), mirror.clone());
    }
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let text = record.text.as_deref().unwrap_or_default();
    for byte in record.name.bytes().chain([0]).chain(text.bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (PathBuf::from(format!("#{hash:016x}")), PathBuf::from(&record.name))
}

/// Combine the snapshots of every window from the previous run into a single
/// tab list. A normal close keeps each window's own snapshot, so the last
/// window is no longer the only one that can be restored. Earlier sources win
/// duplicates and supply the active tab; ids are reassigned so records from
/// different windows cannot collide.
pub(crate) fn merge(sources: impl IntoIterator<Item = Session>) -> Option<Session> {
    let mut merged = Session {
        version: 2,
        active_id: None,
        scratch_active: None,
        scratch_meta: Vec::new(),
        scratch_pinned: Vec::new(),
        files: Vec::new(),
    };
    let mut keys: std::collections::HashSet<(PathBuf, PathBuf)> = std::collections::HashSet::new();
    let mut active_key = None;
    let mut next_id = 1;
    for session in sources {
        let meta = session.scratch_meta();
        let Session { active_id, scratch_active, files, .. } = session;
        let source_active = active_id
            .and_then(|id| files.iter().find(|file| file.id == id))
            .map(tab_key);
        for mut record in files {
            let key = tab_key(&record);
            if !keys.insert(key.clone()) {
                continue;
            }
            if active_key.is_none() && source_active.as_ref() == Some(&key) {
                active_key = Some(key);
            }
            record.id = next_id;
            next_id += 1;
            merged.files.push(record);
        }
        if merged.scratch_active.is_none() {
            merged.scratch_active = scratch_active;
        }
        for entry in meta {
            if !merged
                .scratch_meta
                .iter()
                .any(|known| known.file_name == entry.file_name)
            {
                merged.scratch_meta.push(entry);
            }
        }
    }
    if merged.files.is_empty() && merged.scratch_meta.is_empty() {
        return None;
    }
    merged.active_id = active_key
        .and_then(|key| {
            merged
                .files
                .iter()
                .find(|file| tab_key(file) == key)
                .map(|file| file.id)
        })
        .or_else(|| merged.files.first().map(|file| file.id));
    Some(merged)
}

/// Write an already merged snapshot without borrowing live buffers.
pub(crate) fn write_session(path: &Path, session: &Session) -> io::Result<()> {
    atomic_write_with(path, |writer| {
        serde_json::to_writer(writer, session).map_err(io::Error::other)
    })
}

/// A worker may serialize/fsync, but only the UI may publish a current snapshot.
pub(crate) fn stage(path: &Path, session: &Session) -> io::Result<tempfile::NamedTempFile> {
    let _timing = crate::timing::phase("recovery.serialize_fsync");
    let parent = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    {
        let mut writer = io::BufWriter::new(temp.as_file_mut());
        serde_json::to_writer(&mut writer, session).map_err(io::Error::other)?;
        writer.flush()?;
    }
    temp.as_file().sync_all()?;
    Ok(temp)
}

pub(crate) fn publish(path: &Path, temp: tempfile::NamedTempFile, validate: bool) -> io::Result<()> {
    if validate { preserve_invalid(path)?; }
    if let Ok(metadata) = std::fs::metadata(path) {
        if metadata.permissions().readonly() { return Err(io::Error::new(io::ErrorKind::PermissionDenied, "Recovery file is read-only")); }
        temp.as_file().set_permissions(metadata.permissions())?;
    }
    temp.persist(path).map_err(|error| error.error)?;
    prune_reload_archives(path)
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
/// and no extra JSON-sized allocation. Version 2 stores every tab as a record
/// and points at scratch mirrors instead of inlining unsaved bodies; version 1
/// snapshots remain readable.
pub(crate) fn store_buffers(
    path: &Path,
    files: &[LoadedFile],
    active_id: Option<u64>,
    validate: bool,
) -> io::Result<()> {
    store_buffers_inner(path, files, active_id, validate, false)
}

#[cfg(test)]
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
        pinned: bool,
        id: u64,
        path: &'a Path,
        #[serde(skip_serializing_if = "Option::is_none")]
        source_path: &'a Option<PathBuf>,
        name: &'a str,
        view: ViewMode,
        #[serde(skip_serializing_if = "Option::is_none")]
        language_override: &'a Option<String>,
        encoding: Encoding,
        saved_encoding: Encoding,
        #[serde(skip_serializing_if = "Option::is_none")]
        text: Option<&'a str>,
        #[serde(with = "base64_borrowed_option", skip_serializing_if = "Option::is_none")]
        baseline: Option<&'a [u8]>,
        #[serde(with = "base64_borrowed_option", skip_serializing_if = "Option::is_none")]
        raw: Option<&'a [u8]>,
        line_ending: crate::line_endings::LineEnding,
        #[serde(skip_serializing_if = "Option::is_none")]
        markdown_mode: Option<crate::markdown::Mode>,
        #[serde(skip_serializing_if = "Option::is_none")]
        mirror: Option<&'a Path>,
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
        version: 2,
        active_id,
        files: files
            .iter()
            .map(|f| {
                let dirty = f.is_modified() || archive;
                // Archives deliberately keep a full copy: a mirror may be
                // deleted or rewritten after the archive is written.
                let mirror = (!archive).then_some(f.scratch_path.as_deref()).flatten();
                let inline = mirror.is_none();
                BorrowedRecord {
                    pinned: f.pinned,
                    id: f.id,
                    path: &f.path,
                    source_path: &f.source_path,
                    name: &f.name,
                    view: f.view,
                    language_override: &f.language_override,
                    encoding: f.encoding,
                    saved_encoding: f.saved_encoding,
                    text: if inline && (dirty || (f.pinned && f.is_untitled())) {
                        f.draft_text.as_deref()
                    } else {
                        None
                    },
                    baseline: (inline && dirty).then_some(f.saved_bytes.as_slice()),
                    raw: (f.is_binary && (f.path.as_os_str().is_empty() || archive))
                        .then_some(f.bytes.as_slice()),
                    line_ending: f.line_ending,
                    markdown_mode: Some(f.markdown_mode),
                    mirror,
                }
            })
            .collect(),
    };
    atomic_write_with(path, |writer| {
        serde_json::to_writer(writer, &snapshot).map_err(io::Error::other)
    })?;
    prune_reload_archives(path)
}

/// Only the app's old, well-formed reload backup names in this snapshot's
/// directory are eligible. Never recurse or touch arbitrary user files.
pub(crate) fn prune_reload_archives(session: &Path) -> io::Result<()> {
    let Some(parent) = session.parent() else { return Ok(()); };
    if std::fs::symlink_metadata(parent).is_ok_and(|m| m.file_type().is_symlink()) { return Ok(()); }
    let entries = match std::fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        if entry.path() == session { continue; }
        let name = entry.file_name();
        let Some((id, stamp)) = name.to_str().and_then(|s| s.strip_prefix("before-reload-"))
            .and_then(|s| s.strip_suffix(".json")).and_then(|s| s.split_once('-')) else { continue; };
        if id.parse::<u64>().is_err() || stamp.parse::<u128>().is_err() || !entry.file_type()?.is_file() { continue; }
        if load(&entry.path()).is_ok_and(|session| session.is_some()) {
            std::fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

pub(crate) fn load(path: &Path) -> io::Result<Option<Session>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let session: Session = serde_json::from_slice(&bytes)?;
    if !matches!(session.version, 1 | 2) {
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
    let bytes = match file.draft_text.as_deref() {
        Some(text) => file.encoding.encode(text)?,
        None if file.is_binary => file.bytes.clone(),
        None => return Err(io::Error::other("Document content is unavailable")),
    };
    // Preserve symbolic links by replacing their target rather than the link itself.
    let destination = crate::linkage::resolved(target);
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
    file.saved_generation += 1;
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
    fn unreadable_lease_sidecar_does_not_discard_other_session_records() {
        let dir = tempfile::tempdir().unwrap();
        let mirror = dir.path().join("draft.txt");
        std::fs::write(&mirror, "mirror body").unwrap();
        std::fs::create_dir(dir.path().join(".draft.txt.litetext-lock")).unwrap();
        let session: Session = serde_json::from_value(serde_json::json!({
            "version": 2, "active": 1, "files": [
                { "id": 1, "path": "", "name": "first", "view": "Text", "encoding": "Utf8", "mirror": mirror },
                { "id": 2, "path": "", "name": "second", "view": "Text", "encoding": "Utf8", "text": "inline body" }
            ]
        })).unwrap();
        let (files, _) = session.restore_claimed().unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].draft_text.as_deref(), Some("mirror body"));
        assert!(files[0].scratch_path.is_none());
        assert_eq!(files[1].draft_text.as_deref(), Some("inline body"));
    }

    #[test]
    fn partial_atomic_write_failure_preserves_original_and_removes_temporary_output() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("original.txt");
        std::fs::write(&path, "original bytes").unwrap();
        let error = atomic_write_with(&path, |writer| {
            writer.write_all(b"partial replacement")?;
            Err(io::Error::other("simulated disk full / write failure"))
        }).unwrap_err();
        assert!(error.to_string().contains("write failure"));
        assert_eq!(std::fs::read(&path).unwrap(), b"original bytes");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn closing_tabs_replaces_snapshot_with_only_open_tabs() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("source.txt");
        std::fs::write(&path, "original").unwrap();
        let snapshot = temp.path().join("recovery/session.json");
        let mut app = LiteTextApp { recovery_path_override: Some(snapshot.clone()), ..Default::default() };
        app.open_paths(vec![path.clone()]);
        let linked = app.active_id.unwrap();
        app.new_document();
        let closed = app.active_id.unwrap();
        app.active_mut().unwrap().draft_text = Some("closed private draft".into());
        app.active_mut().unwrap().sync_draft();
        app.new_document();
        app.active_mut().unwrap().draft_text = Some("keep open but unselected".into());
        app.active_mut().unwrap().sync_draft();
        let kept = app.active_id.unwrap();
        app.active_id = Some(linked);
        assert!(app.backup_or_report());
        assert!(std::fs::read_to_string(&snapshot).unwrap().contains("closed private draft"));
        app.apply_tab_action(crate::TabAction::Close(closed));
        let bytes = std::fs::read_to_string(&snapshot).unwrap();
        assert!(!bytes.contains("closed private draft"));
        let restored = load(&snapshot).unwrap().unwrap().restore();
        assert_eq!(restored.len(), 2);
        assert!(restored.iter().any(|f| f.id == kept && f.draft_text.as_deref() == Some("keep open but unselected")));
        app.apply_tab_action(crate::TabAction::Close(kept));
        app.apply_tab_action(crate::TabAction::Close(linked));
        assert!(!load(&snapshot).unwrap().unwrap().has_files());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "original");
    }

    #[test]
    fn cleanup_removes_only_recognized_reload_archives_after_successful_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let session = temp.path().join("session.json");
        let archive = temp.path().join("before-reload-12-1700000000000000.json");
        let mut app = LiteTextApp::default();
        app.new_document();
        app.active_mut().unwrap().draft_text = Some("obsolete reload copy".into());
        archive_buffers(&archive, &app.files, app.active_id).unwrap();
        let unknown = temp.path().join("before-reload-custom.json");
        std::fs::copy(&archive, &unknown).unwrap();
        let malformed = temp.path().join("before-reload-1-2.json");
        std::fs::write(&malformed, "unknown user data").unwrap();
        store_buffers(&session, &[], None, true).unwrap();
        assert!(!archive.exists());
        assert!(unknown.exists());
        assert_eq!(std::fs::read_to_string(malformed).unwrap(), "unknown user data");
    }

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
    fn real_files_restore_with_pins_and_per_tab_markdown_modes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let notes = root.join("notes.md");
        let plain = root.join("plain.txt");
        std::fs::write(&notes, "# Notes\n\nbody\n").unwrap();
        std::fs::write(&plain, "plain\n").unwrap();
        let snapshot = dir.path().join("session.json");
        let mut app = LiteTextApp {
            recovery_path_override: Some(snapshot.clone()),
            scratch_dir: Some(root.join("drafts")),
            ..Default::default()
        };
        app.open_paths(vec![notes.clone(), plain.clone()]);
        let notes_id = app
            .files
            .iter()
            .find(|file| file.path == notes)
            .unwrap()
            .id;
        app.files
            .iter_mut()
            .find(|file| file.id == notes_id)
            .unwrap()
            .markdown_mode = crate::markdown::Mode::Live;
        app.toggle_pin(notes_id);
        app.active_id = Some(notes_id);
        app.write_recovery().unwrap();

        let mut restored = LiteTextApp {
            recovery_path_override: Some(snapshot),
            scratch_dir: Some(root.join("drafts")),
            ..Default::default()
        };
        restored.restore_session();
        assert_eq!(restored.files.len(), 2, "both real files must come back");
        let notes_file = restored
            .files
            .iter()
            .find(|file| file.path == notes)
            .expect("the pinned Markdown file must be restored");
        assert_eq!(notes_file.draft_text.as_deref(), Some("# Notes\n\nbody\n"));
        assert!(notes_file.pinned, "pin state belongs to the tab");
        assert_eq!(
            notes_file.markdown_mode,
            crate::markdown::Mode::Live,
            "each tab keeps its own Markdown view"
        );
        assert_eq!(restored.active_id, Some(notes_id));
        assert!(
            restored.files.iter().any(|file| file.path == plain),
            "unrelated real files are not dropped"
        );
    }

    #[test]
    fn pinned_blank_untitled_tabs_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let snapshot = dir.path().join("session.json");
        let mut app = LiteTextApp {
            recovery_path_override: Some(snapshot.clone()),
            scratch_dir: Some(dir.path().join("drafts")),
            ..Default::default()
        };
        app.new_document();
        let blank = app.active_id.unwrap();
        app.toggle_pin(blank);
        app.write_recovery().unwrap();

        let mut restored = LiteTextApp {
            recovery_path_override: Some(snapshot),
            scratch_dir: Some(dir.path().join("drafts")),
            ..Default::default()
        };
        restored.restore_session();
        assert_eq!(restored.files.len(), 1);
        assert!(restored.files[0].pinned);
        assert!(restored.files[0].is_untitled());
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



    #[test]
    fn recovery_snapshot_writes_base64_baseline() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.txt");
        let original = "a".repeat(700);
        std::fs::write(&source, &original).unwrap();
        let snapshot = dir.path().join("session.json");
        let mut app = LiteTextApp {
            recovery_path_override: Some(snapshot.clone()),
            ..Default::default()
        };
        app.open_paths(vec![source]);
        app.active_mut().unwrap().draft_text = Some("edited text".into());
        app.active_mut().unwrap().sync_draft();
        app.write_recovery().unwrap();

        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&snapshot).unwrap()).unwrap();
        let baseline = value["files"][0]["baseline"]
            .as_str()
            .expect("the streaming writer must store a base64 string");
        use base64::Engine;
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(baseline)
                .unwrap(),
            original.as_bytes()
        );
        let restored = load(&snapshot).unwrap().unwrap().restore();
        assert_eq!(restored[0].draft_text.as_deref(), Some("edited text"));
    }

    #[test]
    fn merged_snapshots_keep_tabs_from_every_window_without_id_clashes() {
        let first: Session = serde_json::from_value(serde_json::json!({
            "version": 2,
            "active_id": 2,
            "files": [
                { "id": 1, "path": "/tmp/a.txt", "name": "a.txt", "view": "Text", "encoding": "Utf8" },
                { "id": 2, "path": "", "name": "未命名 1", "view": "Text", "encoding": "Utf8",
                  "mirror": "/tmp/scratch/未命名 1.txt" }
            ]
        }))
        .unwrap();
        let second: Session = serde_json::from_value(serde_json::json!({
            "version": 2,
            "active_id": 5,
            "files": [
                { "id": 4, "path": "/tmp/a.txt", "name": "a.txt", "view": "Text", "encoding": "Utf8" },
                { "id": 5, "path": "/tmp/b.txt", "name": "b.txt", "view": "Text", "encoding": "Utf8" }
            ]
        }))
        .unwrap();
        let merged = merge([first, second]).expect("merged session");
        assert_eq!(merged.files.len(), 3, "the duplicate file is kept once");
        let ids: Vec<u64> = merged.files.iter().map(|file| file.id).collect();
        assert_eq!(ids, vec![1, 2, 3], "ids are unique across windows");
        assert_eq!(merged.files[0].path, PathBuf::from("/tmp/a.txt"));
        assert_eq!(merged.files[1].path, PathBuf::from(""));
        assert_eq!(merged.files[2].path, PathBuf::from("/tmp/b.txt"));
        assert_eq!(merged.active_id, Some(2), "the newest source's active tab wins");
    }

    #[test]
    fn merge_keeps_differently_edited_inline_drafts_of_the_same_name() {
        let first: Session = serde_json::from_value(serde_json::json!({
            "version": 2,
            "active_id": 1,
            "files": [{ "id": 1, "path": "", "name": "未命名 1", "view": "Text", "encoding": "Utf8", "text": "A" }]
        }))
        .unwrap();
        let second: Session = serde_json::from_value(serde_json::json!({
            "version": 2,
            "active_id": 9,
            "files": [{ "id": 9, "path": "", "name": "未命名 1", "view": "Text", "encoding": "Utf8", "text": "B" }]
        }))
        .unwrap();
        let merged = merge([first, second]).expect("merged session");
        assert_eq!(merged.files.len(), 2, "same title but different content is not one tab");
    }

    #[test]
    fn legacy_scratch_pinned_entries_merge_into_scratch_meta() {
        let session: Session = serde_json::from_value(serde_json::json!({
            "version": 1,
            "active_id": null,
            "scratch_active": null,
            "scratch_meta": [
                { "file_name": "未命名 1.txt", "pinned": true, "language": "JSON" }
            ],
            "scratch_pinned": ["未命名 1.txt", "未命名 2.md"],
            "files": []
        }))
        .unwrap();
        let meta = session.scratch_meta();
        assert_eq!(meta.len(), 2, "a legacy pin must not duplicate an existing entry");
        assert_eq!(meta[0].file_name, "未命名 1.txt");
        assert_eq!(meta[0].language.as_deref(), Some("JSON"));
        assert_eq!(meta[1].file_name, "未命名 2.md");
        assert!(meta[1].pinned);
        assert!(meta[1].language.is_none());
        assert!(session.has_files(), "a drafts-only session must stay restorable");
    }

    #[test]
    fn version_one_inline_records_still_restore_with_their_baseline() {
        use base64::Engine;
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("original.txt");
        std::fs::write(&original, "original\n").unwrap();
        let session: Session = serde_json::from_value(serde_json::json!({
            "version": 1,
            "active_id": 7,
            "scratch_active": null,
            "scratch_meta": [],
            "scratch_pinned": [],
            "files": [{
                "pinned": false,
                "id": 7,
                "path": original,
                "source_path": null,
                "name": "original.txt",
                "view": "Text",
                "language_override": null,
                "encoding": "Utf8",
                "saved_encoding": "Utf8",
                "text": "edited\n",
                "baseline": base64::engine::general_purpose::STANDARD.encode(b"original\n"),
                "raw": null,
                "line_ending": "Lf",
                "markdown_mode": "Live"
            }]
        }))
        .unwrap();
        assert_eq!(session.version, 1);
        let mut files = session.restore();
        assert_eq!(files[0].draft_text.as_deref(), Some("edited\n"));
        assert_eq!(files[0].saved_bytes, b"original\n");
        assert!(files[0].is_modified());
        assert_eq!(files[0].markdown_mode, crate::markdown::Mode::Live);
        assert_eq!(std::fs::read_to_string(&original).unwrap(), "original\n");
        // The old inline baseline still guards external changes on Save.
        std::fs::write(&original, "external\n").unwrap();
        assert!(save_document(&mut files[0], &original).is_err());
        assert_eq!(std::fs::read_to_string(&original).unwrap(), "external\n");
    }

    #[test]
    fn session_stores_binary_buffers_as_base64_and_accepts_legacy_arrays() {
        let baseline: Vec<u8> = (0..=255).collect();
        let session = Session {
            version: 1,
            active_id: Some(1),
            scratch_active: None,
            scratch_meta: Vec::new(),
            scratch_pinned: Vec::new(),
            files: vec![Record {
                pinned: false,
                id: 1,
                path: PathBuf::from("/tmp/a.bin"),
                source_path: None,
                name: "a.bin".to_owned(),
                view: ViewMode::Hex,
                language_override: None,
                encoding: Encoding::Utf8,
                saved_encoding: Some(Encoding::Utf8),
                text: None,
                baseline: Some(baseline.clone()),
                raw: None,
                line_ending: crate::line_endings::LineEnding::Lf,
                markdown_mode: None,
                mirror: None,
            }],
        };
        let bytes = serde_json::to_vec(&session).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(
            value["files"][0]["baseline"].is_string(),
            "binary baseline must not expand into a JSON number array"
        );
        let back: Session = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back.files[0].baseline.as_deref(), Some(baseline.as_slice()));

        // Sessions written before the base64 change used a numeric array.
        let mut legacy = value;
        legacy["files"][0]["baseline"] = serde_json::Value::Array(
            baseline.iter().map(|byte| (*byte).into()).collect(),
        );
        let legacy_bytes = serde_json::to_vec(&legacy).unwrap();
        let legacy_back: Session = serde_json::from_slice(&legacy_bytes).unwrap();
        assert_eq!(
            legacy_back.files[0].baseline.as_deref(),
            Some(baseline.as_slice()),
            "legacy numeric-array sessions must keep loading"
        );
    }

}

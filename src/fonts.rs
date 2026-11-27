//! Workspace font selection: discover the fonts installed on the system and
//! layer a user-chosen one over the built-in editor font.
//!
//! Discovery only reads the small `name` tables of font files (via seeks), so
//! even multi-megabyte CJK collections are scanned cheaply. The full file is
//! read once, when the font is actually applied.

use eframe::egui;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

/// A user's persisted font choice. The family is what the list shows; the
/// path and face index are what egui loads. Persisting the path (instead of
/// re-resolving the family on every start) keeps startup scans off the
/// critical path; a missing file simply falls back to the built-in stack.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Choice {
    pub family: String,
    pub path: PathBuf,
    #[serde(default)]
    pub index: u32,
}

/// One selectable entry of the font list.
#[derive(Clone, Debug)]
pub(crate) struct SystemFont {
    pub family: String,
    pub path: PathBuf,
    pub index: u32,
    /// The face maps the common simplified-Chinese sample characters. Used to
    /// hide fonts that cannot render Chinese when the UI language is Chinese.
    pub chinese: bool,
}

/// Background scan state: the list arrives once, off the UI thread. The Arc
/// lets the settings page hold the list while mutating app state.
#[derive(Default)]
pub(crate) struct Scan {
    receiver: Option<std::sync::mpsc::Receiver<Vec<SystemFont>>>,
    fonts: Option<std::sync::Arc<Vec<SystemFont>>>,
}

impl Scan {
    /// Kick off a scan unless a list is already available or one is running.
    pub(crate) fn start(&mut self) {
        if self.fonts.is_some() || self.receiver.is_some() {
            return;
        }
        let (sender, receiver) = std::sync::mpsc::channel();
        self.receiver = Some(receiver);
        std::thread::spawn(move || {
            let fonts = scan(&font_dirs());
            let _ = sender.send(fonts);
        });
    }

    /// Test-only: install a ready-made list without touching the filesystem.
    #[cfg(test)]
    pub(crate) fn seed(&mut self, fonts: Vec<SystemFont>) {
        self.receiver = None;
        self.fonts = Some(std::sync::Arc::new(fonts));
    }

    /// Take a finished scan, if any.
    pub(crate) fn poll(&mut self) {
        if self.fonts.is_some() {
            return;
        }
        match self.receiver.as_ref().map(|receiver| receiver.try_recv()) {
            Some(Ok(fonts)) => {
                self.fonts = Some(std::sync::Arc::new(fonts));
                self.receiver = None;
            }
            // A dead worker must not spin the spinner forever.
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                self.fonts = Some(std::sync::Arc::new(Vec::new()));
                self.receiver = None;
            }
            _ => {}
        }
    }

    /// The scanned list, `None` while still pending.
    pub(crate) fn list(&self) -> Option<std::sync::Arc<Vec<SystemFont>>> {
        self.fonts.clone()
    }
}

fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    #[cfg(target_os = "macos")]
    let home = std::env::var_os("HOME").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    {
        dirs.push(PathBuf::from("/System/Library/Fonts"));
        // Optional/downloaded system faces live here, including PingFang —
        // the default Chinese font on current macOS releases.
        dirs.push(PathBuf::from("/System/Library/AssetsV2"));
        dirs.push(PathBuf::from("/Library/Fonts"));
        if let Some(home) = &home {
            dirs.push(home.join("Library/Fonts"));
        }
    }
    #[cfg(target_os = "windows")]
    {
        dirs.push(PathBuf::from(r"C:\Windows\Fonts"));
        if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
            dirs.push(local.join(r"Microsoft\Windows\Fonts"));
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let home = std::env::var_os("HOME").map(PathBuf::from);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        dirs.push(PathBuf::from("/usr/share/fonts"));
        dirs.push(PathBuf::from("/usr/local/share/fonts"));
        if let Some(home) = &home {
            dirs.push(home.join(".fonts"));
            dirs.push(home.join(".local/share/fonts"));
        }
    }
    dirs.retain(|dir| dir.is_dir());
    dirs
}

/// Scan `dirs` (recursively, bounded) for fonts with a readable family name.
pub(crate) fn scan(dirs: &[PathBuf]) -> Vec<SystemFont> {
    // The face offset is kept alongside the row so Chinese coverage is only
    // probed for the family winner, not for every face of a collection.
    let mut found: BTreeMap<String, (SystemFont, u64)> = BTreeMap::new();
    let mut budget = 4_000usize;
    for dir in dirs {
        walk(dir, 0, &mut |path| {
            if budget == 0 {
                return;
            }
            budget -= 1;
            for (family, index, offset, regular) in families_in_file(path) {
                // ".SF NS" & friends are private system faces: real fonts, but
                // never something a user picks by hand.
                if family.starts_with('.') {
                    continue;
                }
                // One row per family: prefer a Regular face, then the first
                // readable one. Later files never override an earlier Regular.
                let replace = found
                    .get(&family)
                    .is_none_or(|(existing, _)| {
                        regular && !path_is_regular(&existing.path)
                    });
                if replace {
                    found.insert(
                        family.clone(),
                        (
                            SystemFont {
                                family,
                                path: path.to_owned(),
                                index,
                                chinese: false,
                            },
                            offset,
                        ),
                    );
                }
            }
        });
    }
    found
        .into_values()
        .map(|(mut font, offset)| {
            font.chinese = chinese_at(&font.path, offset);
            font
        })
        .collect()
}

fn walk(dir: &Path, depth: usize, visit: &mut impl FnMut(&Path)) {
    const MAX_DEPTH: usize = 4;
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        let path = entry.path();
        if kind.is_dir() {
            dirs.push(path);
        } else {
            files.push(path);
        }
    }
    files.sort();
    dirs.sort();
    for path in files {
        let is_font = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                matches!(
                    extension.to_ascii_lowercase().as_str(),
                    "ttf" | "otf" | "ttc" | "otc"
                )
            });
        if is_font {
            visit(&path);
        }
    }
    for child in dirs {
        walk(&child, depth + 1, visit);
    }
}

/// Read `len` bytes at `offset`.
fn read_at(path: &Path, offset: u64, len: usize) -> Option<Vec<u8>> {
    use std::io::{Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut buffer = vec![0u8; len];
    file.read_exact(&mut buffer).ok()?;
    Some(buffer)
}

fn u16be(bytes: &[u8], at: usize) -> Option<u16> {
    bytes.get(at..at + 2).map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
}

fn u32be(bytes: &[u8], at: usize) -> Option<u32> {
    bytes.get(at..at + 4).map(|quad| u32::from_be_bytes([quad[0], quad[1], quad[2], quad[3]]))
}

/// Families (plus face index, face offset and a Regular heuristic) inside one
/// file. Collection files (`.ttc`/`.otc`) contribute every face; only the
/// first 32 are considered, which covers the system collections in practice.
fn families_in_file(path: &Path) -> Vec<(String, u32, u64, bool)> {
    let Some(header) = read_at(path, 0, 12) else {
        return Vec::new();
    };
    // Collection files (`.ttc`/`.otc`) hold several faces; only the first 32
    // are considered, which covers the system collections in practice.
    let mut offsets: Vec<u64> = Vec::new();
    if header.first() == Some(&b't')
        && header.get(1) == Some(&b't')
        && header.get(2) == Some(&b'c')
        && header.get(3) == Some(&b'f')
    {
        if let Some(count) = u32be(&header, 8) {
            let count = count.min(32) as usize;
            if let Some(table) = read_at(path, 12, count * 4) {
                for index in 0..count {
                    if let Some(offset) = u32be(&table, index * 4) {
                        offsets.push(offset as u64);
                    }
                }
            }
        }
    } else {
        offsets.push(0);
    }
    offsets
        .into_iter()
        .enumerate()
        .filter_map(|(index, offset)| {
            family_at(path, offset)
                .map(|family| (family, index as u32, offset, path_is_regular(path)))
        })
        .collect()
}

fn path_is_regular(path: &Path) -> bool {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| {
            let lower = stem.to_ascii_lowercase();
            lower.ends_with("regular") || lower.ends_with("-regular") || lower.ends_with("-r")
        })
}

/// The absolute file range of `tag` in the face starting at `offset`.
fn table_range(path: &Path, offset: u64, tag: &[u8; 4]) -> Option<(u64, u32)> {
    let header = read_at(path, offset, 12)?;
    let version = u32be(&header, 0)?;
    // 0x00010000: TrueType, 'OTTO': OpenType with CFF, 'true': legacy.
    if version != 0x0001_0000 && version != 0x4F54_544F && version != 0x7472_7565 {
        return None;
    }
    let tables = u16be(&header, 4)? as usize;
    let records = read_at(path, offset + 12, tables.checked_mul(16)?)?;
    for table in 0..tables {
        let at = table * 16;
        if &records[at..at + 4] != tag {
            continue;
        }
        // Face table offsets in a collection are absolute file positions,
        // never relative to the face header.
        return Some((u32be(&records, at + 8)? as u64, u32be(&records, at + 12)?));
    }
    None
}

/// The preferred family name of the face starting at `offset`.
fn family_at(path: &Path, offset: u64) -> Option<String> {
    let (start, length) = table_range(path, offset, b"name")?;
    let table = read_at(path, start, length.min(128 * 1024) as usize)?;
    family_from_name_table(&table)
}

/// Characters a font must map to count as Chinese-capable. Simplified-only
/// forms screen out Japanese and Korean faces, whose common ideographs would
/// otherwise pass: they cover 中文 but not 输 or 汉.
pub(crate) const CHINESE_SAMPLE: [char; 4] = ['中', '文', '输', '汉'];

/// Whether the face starting at `offset` maps every sample character.
fn chinese_at(path: &Path, offset: u64) -> bool {
    let Some((start, length)) = table_range(path, offset, b"cmap") else {
        return false;
    };
    // No real cmap comes anywhere near this; bounds the read for corrupt files.
    let Some(table) = read_at(path, start, length.min(1024 * 1024) as usize) else {
        return false;
    };
    CHINESE_SAMPLE
        .iter()
        .all(|&character| cmap_covers(&table, character))
}

/// Does any Unicode subtable of `cmap` map `character` to a non-zero glyph?
fn cmap_covers(table: &[u8], character: char) -> bool {
    let Some(count) = u16be(table, 2).map(usize::from) else {
        return false;
    };
    for record in 0..count {
        let at = 4 + record * 8;
        let (Some(platform), Some(encoding), Some(offset)) =
            (u16be(table, at), u16be(table, at + 2), u32be(table, at + 4))
        else {
            continue;
        };
        // Platform 0 is Unicode by definition; platform 3 needs its Unicode
        // encodings (BMP or full repertoire). Symbol and legacy Mac tables
        // never carry Chinese.
        let unicode = platform == 0 || (platform == 3 && matches!(encoding, 1 | 10));
        if !unicode {
            continue;
        }
        let Some(subtable) = table.get(offset as usize..) else {
            continue;
        };
        if subtable_covers(subtable, character) {
            return true;
        }
    }
    false
}

fn subtable_covers(subtable: &[u8], character: char) -> bool {
    let code = character as u32;
    match u16be(subtable, 0) {
        Some(0) => code < 256 && subtable.get(6 + code as usize).is_some_and(|&id| id != 0),
        Some(4) if code <= 0xFFFF => format4_covers(subtable, code as u16),
        Some(6) => format6_covers(subtable, code),
        Some(12) => format12_covers(subtable, code),
        _ => false,
    }
}

/// Format 4 is the classic BMP segment mapping.
fn format4_covers(subtable: &[u8], code: u16) -> bool {
    let Some(segments) = u16be(subtable, 6).map(|x2| usize::from(x2) / 2) else {
        return false;
    };
    let end_codes = 14;
    let start_codes = end_codes + segments * 2 + 2;
    let deltas = start_codes + segments * 2;
    let range_offsets = deltas + segments * 2;
    for segment in 0..segments {
        let (Some(end), Some(start), Some(delta), Some(range)) = (
            u16be(subtable, end_codes + segment * 2),
            u16be(subtable, start_codes + segment * 2),
            u16be(subtable, deltas + segment * 2),
            u16be(subtable, range_offsets + segment * 2),
        ) else {
            return false;
        };
        if code > end {
            continue;
        }
        if code < start {
            return false;
        }
        let glyph = if range == 0 {
            code.wrapping_add(delta)
        } else {
            // The range offset is relative to the idRangeOffset entry itself.
            let at = range_offsets + segment * 2 + usize::from(range) + (code - start) as usize * 2;
            match u16be(subtable, at) {
                Some(0) => 0,
                Some(glyph) => glyph.wrapping_add(delta),
                None => return false,
            }
        };
        return glyph != 0;
    }
    false
}

/// Format 6 is a trimmed contiguous range.
fn format6_covers(subtable: &[u8], code: u32) -> bool {
    let (Some(first), Some(count)) = (u16be(subtable, 6), u16be(subtable, 8)) else {
        return false;
    };
    let (first, count) = (u32::from(first), u32::from(count));
    if code < first || code >= first + count {
        return false;
    }
    let at = 10 + (code - first) as usize * 2;
    u16be(subtable, at).is_some_and(|glyph| glyph != 0)
}

/// Format 12 is the full-repertoire group mapping; groups are sorted.
fn format12_covers(subtable: &[u8], code: u32) -> bool {
    let Some(groups) = u32be(subtable, 12).map(|groups| groups as usize) else {
        return false;
    };
    let (mut low, mut high) = (0usize, groups);
    while low < high {
        let middle = (low + high) / 2;
        let at = 16 + middle * 12;
        let (Some(start), Some(end), Some(glyph)) = (
            u32be(subtable, at),
            u32be(subtable, at + 4),
            u32be(subtable, at + 8),
        ) else {
            return false;
        };
        if code < start {
            high = middle;
        } else if code > end {
            low = middle + 1;
        } else {
            return glyph.wrapping_add(code - start) != 0;
        }
    }
    false
}

/// nameId 1 (family). macOS system collections carry localized family names
/// (French "Gras", Japanese "ヒラギノ…") and occasionally odd encodings, so the
/// preference order matters: Windows/English first, then Unicode platform,
/// then any Windows language, then classic Mac ASCII. Platform-3 symbol
/// encoding (0) is single-byte, not UTF-16 — decoding it as UTF-16 produces
/// the mojibake the list used to show.
fn family_from_name_table(table: &[u8]) -> Option<String> {
    let count = u16be(table, 2)? as usize;
    let strings = u16be(table, 4)? as usize;
    let mut windows_english: Option<String> = None;
    let mut unicode: Option<String> = None;
    let mut windows_any: Option<String> = None;
    let mut macintosh: Option<String> = None;
    for record in 0..count {
        let at = 6 + record * 12;
        let (Some(platform), Some(encoding), Some(language), Some(name_id), Some(length), Some(offset)) = (
            u16be(table, at),
            u16be(table, at + 2),
            u16be(table, at + 4),
            u16be(table, at + 6),
            u16be(table, at + 8),
            u16be(table, at + 10),
        ) else {
            continue;
        };
        if name_id != 1 || length == 0 {
            continue;
        }
        let from = strings + offset as usize;
        let Some(raw) = table.get(from..from + length as usize) else {
            continue;
        };
        let decoded = match (platform, encoding) {
            (3, 1) | (3, 10) | (0, _) => decode_utf16be(raw),
            // Symbol encoding and classic Mac strings are single-byte.
            (3, 0) | (1, 0) => Some(String::from_utf8_lossy(raw).into_owned()),
            _ => None,
        };
        let Some(decoded) = decoded else { continue };
        let decoded = decoded.trim().to_owned();
        if !is_clean_family(&decoded) {
            continue;
        }
        match platform {
            3 if (language == 0x409 || language == 0) && windows_english.is_none() => {
                windows_english = Some(decoded);
            }
            0 if unicode.is_none() => unicode = Some(decoded),
            3 if windows_any.is_none() => windows_any = Some(decoded),
            1 if macintosh.is_none() => macintosh = Some(decoded),
            _ => {}
        }
    }
    windows_english.or(unicode).or(windows_any).or(macintosh)
}

/// Reject control characters, replacement runes and absurd lengths; these
/// point at a mis-decoded or non-family string.
fn is_clean_family(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| !c.is_control() && c != '\u{FFFD}')
}

fn decode_utf16be(bytes: &[u8]) -> Option<String> {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_be_bytes(*pair))
        .collect();
    String::from_utf16(&units).ok()
}

/// Family names rendered with their own font: rasterized once through
/// ab_glyph, kept as tiny white-alpha textures and tinted at paint time, so
/// the preview works in both themes. Fonts that cannot rasterize their own
/// name fall back to the regular label.
#[derive(Default)]
pub(crate) struct Previews {
    cache: std::collections::HashMap<String, Option<egui::TextureHandle>>,
    files: Vec<(PathBuf, std::sync::Arc<Vec<u8>>)>,
}

impl Previews {
    /// Drop cached textures (e.g. when the page closes). File bytes are cheap
    /// to re-read; textures would otherwise linger for every row ever shown.
    pub(crate) fn clear(&mut self) {
        self.cache.clear();
        self.files.clear();
    }

    /// A few open font files at a time; big CJK collections are tens of MB.
    fn file_bytes(&mut self, path: &Path) -> Option<std::sync::Arc<Vec<u8>>> {
        if let Some((_, bytes)) = self.files.iter().find(|(cached, _)| cached == path) {
            return Some(bytes.clone());
        }
        let bytes = std::sync::Arc::new(std::fs::read(path).ok()?);
        if self.files.len() >= 4 {
            self.files.clear();
        }
        self.files.push((path.to_owned(), bytes.clone()));
        Some(bytes)
    }

    /// Rasterize a sample only when the selected face contains every glyph.
    /// Callers may request independent groups so one missing script does not
    /// hide the other groups in the same preview row.
    pub(crate) fn texture(
        &mut self,
        ctx: &egui::Context,
        text: &str,
        path: &Path,
        index: u32,
    ) -> Option<egui::TextureHandle> {
        let key = format!("{path:?}\u{0}{index}\u{0}{text}");
        if let Some(cached) = self.cache.get(&key) {
            return cached.clone();
        }
        let rendered = self
            .file_bytes(path)
            .and_then(|bytes| rasterize(text, &bytes, index));
        let handle = rendered.map(|image| {
            ctx.load_texture(
                format!("font-preview-{key}"),
                image,
                egui::TextureOptions::default(),
            )
        });
        self.cache.insert(key, handle.clone());
        handle
    }
}

/// Rasterize `text` with one font face as a white-on-transparent image.
/// `None` when any glyph is missing — the caller shows a plain label instead.
fn rasterize(text: &str, bytes: &[u8], index: u32) -> Option<egui::ColorImage> {
    use ab_glyph::{Font, FontRef, Glyph, Point, PxScale, ScaleFont};
    let font = FontRef::try_from_slice_and_index(bytes, index).ok()?;
    // 2x oversampling keeps the preview crisp on retina displays.
    const PX: f32 = 30.0;
    let scaled = font.as_scaled(PX);
    let mut positions = Vec::new();
    let mut pen = 0.0f32;
    let mut previous: Option<ab_glyph::GlyphId> = None;
    for character in text.chars() {
        let id = font.glyph_id(character);
        if id.0 == 0 {
            return None;
        }
        if let Some(previous) = previous {
            pen += scaled.kern(previous, id);
        }
        positions.push((id, pen));
        pen += scaled.h_advance(id);
        previous = Some(id);
    }
    if pen <= 0.0 {
        return None;
    }
    let width = pen.ceil() as u32 + 1;
    let ascent = scaled.ascent();
    let height = (scaled.ascent() - scaled.descent()).ceil() as u32 + 1;
    if width == 0 || height == 0 || width > 4096 {
        return None;
    }
    let mut coverage = vec![0u8; (width * height) as usize];
    for (id, pen) in positions {
        let glyph = Glyph {
            id,
            scale: PxScale::from(PX),
            position: Point { x: pen, y: ascent },
        };
        // Spaces and other blank glyphs have no outline; they still advance.
        // `draw` reports pixel coordinates relative to the glyph's own
        // bounding box, so shift them by the box origin first.
        if let Some(outlined) = font.outline_glyph(glyph) {
            let bounds = outlined.px_bounds();
            let (origin_x, origin_y) = (bounds.min.x.floor() as i64, bounds.min.y.floor() as i64);
            outlined.draw(|x, y, value| {
                let pixel_x = origin_x + x as i64;
                let pixel_y = origin_y + y as i64;
                if pixel_x < 0
                    || pixel_y < 0
                    || pixel_x >= width as i64
                    || pixel_y >= height as i64
                {
                    return;
                }
                let index = pixel_y as usize * width as usize + pixel_x as usize;
                if let Some(slot) = coverage.get_mut(index) {
                    *slot = (*slot).max((value * 255.0).min(255.0) as u8);
                }
            });
        }
    }
    // Trim fully transparent columns and rows for a tight texture.
    let (left, right, top, bottom) = trim_bounds(&coverage, width as usize, height as usize)?;
    let trimmed_width = right - left;
    let trimmed_height = bottom - top;
    let pixels = (top..bottom)
        .flat_map(|y| (left..right).map(move |x| (x, y)))
        .map(|(x, y)| {
            egui::Color32::from_white_alpha(coverage[y * width as usize + x])
        })
        .collect();
    Some(egui::ColorImage {
        size: [trimmed_width, trimmed_height],
        pixels,
    })
}

/// Inclusive pixel bounds of the drawn glyphs, or `None` when empty.
fn trim_bounds(coverage: &[u8], width: usize, height: usize) -> Option<(usize, usize, usize, usize)> {
    let has = |x: usize, y: usize| coverage[y * width + x] != 0;
    let left = (0..width).find(|&x| (0..height).any(|y| has(x, y)))?;
    let right = (0..width).rev().find(|&x| (0..height).any(|y| has(x, y)))? + 1;
    let top = (0..height).find(|&y| (0..width).any(|x| has(x, y)))?;
    let bottom = (0..height).rev().find(|&y| (0..width).any(|x| has(x, y)))? + 1;
    Some((left, right, top, bottom))
}

/// Layer the chosen font over the built-in stack and install it. Invalid or
/// missing files fall back to the default definitions.
pub(crate) fn apply(ctx: &egui::Context, choice: Option<&Choice>) {
    let mut definitions = crate::build_font_definitions();
    if let Some(choice) = choice
        && let Ok(bytes) = std::fs::read(&choice.path)
        && let Ok(font) = ab_glyph::FontVec::try_from_vec_and_index(bytes.clone(), choice.index)
        // A face with no glyph for a plain letter would render tofu; reject
        // it rather than silently shipping a broken stack.
        && ab_glyph::Font::glyph_id(&font, 'A').0 != 0
    {
        let mut data = egui::FontData::from_owned(bytes);
        data.index = choice.index;
        definitions
            .font_data
            .insert("workspace".to_owned(), std::sync::Arc::new(data));
        if let Some(family) = definitions
            .families
            .get_mut(&egui::FontFamily::Monospace)
        {
            // The document font leads; missing glyphs (CJK, symbols) still
            // resolve through the built-in fallback chain behind it.
            family.insert(0, "workspace".to_owned());
        }
    }
    ctx.set_fonts(definitions);
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMBEDDED: &[u8] = include_bytes!("../assets/fonts/HarmonyOS_Sans_Regular.ttf");

    fn temp_font() -> PathBuf {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.keep().join("HarmonyOS_Sans_Regular.ttf");
        std::fs::write(&path, EMBEDDED).unwrap();
        path
    }

    #[test]
    fn system_font_names_scan_cleanly() {
        // Guards the collection (`.ttc`) handling and the name preference
        // order; silently skips platforms or CI images without fonts.
        let dirs = font_dirs();
        if dirs.is_empty() {
            return;
        }
        let fonts = scan(&dirs);
        if fonts.is_empty() {
            // A headless CI image may ship no fonts at all; nothing to check.
            return;
        }
        for font in &fonts {
            assert!(
                is_clean_family(&font.family),
                "mojibake or hidden family name: {:?} ({})",
                font.family,
                font.path.display()
            );
            assert!(
                !font.family.starts_with('.'),
                "private system fonts are filtered: {:?}",
                font.family
            );
        }
        // The counts and specific families below only hold on desktop
        // systems with a full font set; Linux CI images are minimal.
        #[cfg(target_os = "macos")]
        {
            assert!(fonts.len() > 50, "the system must yield fonts");
            // Localized family names exist in these collections; the English
            // record must win over the localized one.
            assert!(
                fonts.iter().any(|font| font.family.starts_with("Hiragino")),
                "expected the English Hiragino names, got: {:?}",
                fonts.iter().map(|f| f.family.as_str()).take(5).collect::<Vec<_>>()
            );
        }
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            assert!(fonts.len() > 20, "a desktop system must yield fonts");
            // Coverage detection separates CJK faces from Latin-only ones,
            // which is what the Chinese UI uses to trim the list.
            assert!(
                fonts.iter().any(|font| font.chinese),
                "expected Chinese-capable system fonts"
            );
            assert!(
                fonts.iter().any(|font| !font.chinese),
                "Latin-only system fonts must stay distinguishable"
            );
        }
    }

    #[test]
    fn the_name_table_parser_reads_the_embedded_font_family() {
        let path = temp_font();
        assert_eq!(
            families_in_file(&path),
            vec![("HarmonyOS Sans".to_owned(), 0, 0, true)]
        );
    }

    #[test]
    fn scan_collects_one_regular_row_per_family() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a-regular.ttf"), EMBEDDED).unwrap();
        std::fs::write(dir.path().join("b-bold.ttf"), EMBEDDED).unwrap();
        let sub = dir.path().join("nested");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("c-regular.ttf"), EMBEDDED).unwrap();
        // Not a font: skipped without failing the scan.
        std::fs::write(dir.path().join("notes.txt"), b"not a font").unwrap();
        let fonts = scan(&[dir.path().to_owned()]);
        // One row per family: the first Regular face wins, later files never
        // override it and non-Regular faces are skipped in its presence.
        assert_eq!(fonts.len(), 1);
        assert_eq!(fonts[0].family, "HarmonyOS Sans");
        assert!(fonts[0]
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == "a-regular.ttf"));
        // Every row points at a file that can actually be loaded.
        for font in &fonts {
            let bytes = std::fs::read(&font.path).unwrap();
            assert!(ab_glyph::FontVec::try_from_vec_and_index(bytes, font.index).is_ok());
        }
        // The embedded asset is a Latin-only subset, so it must not be
        // mistaken for a Chinese-capable font.
        assert!(
            !fonts[0].chinese,
            "the Latin-only embedded subset has no Chinese glyphs"
        );
    }

    #[test]
    fn scan_survives_unreadable_and_garbage_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("garbage.ttf"), b"nope").unwrap();
        let fonts = scan(&[dir.path().to_owned()]);
        assert!(fonts.is_empty());
    }

    #[test]
    fn previews_render_names_and_fall_back_when_glyphs_are_missing() {
        let mut previews = Previews::default();
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let path = temp_font();
        let handle = previews
            .texture(&ctx, "HarmonyOS Sans", &path, 0)
            .expect("the embedded font can render its own name");
        let [width, height] = handle.size();
        assert!(width > 20 && height > 10, "preview must have real extent");
        // A reserved codepoint no font covers: no texture, the row falls back
        // to a plain label instead of tofu.
        assert!(previews.texture(&ctx, "\u{378}", &path, 0).is_none());
        // The second call is served from the cache.
        assert!(previews.texture(&ctx, "HarmonyOS Sans", &path, 0).is_some());
        // The row sample renders in the same font; it is wider than the name
        // alone, which is what the settings row draws beside it.
        let sample = previews
            .texture(&ctx, "[Aa 012]", &path, 0)
            .expect("the embedded font renders Latin samples");
        assert!(sample.size()[0] > 20);
        // Chinese samples need a Chinese-capable face.
        assert!(previews.texture(&ctx, "中文", &path, 0).is_none());
        previews.clear();
        assert!(previews.cache.is_empty());
    }

    #[test]
    fn applying_a_choice_leads_the_monospace_stack_and_falls_back_cleanly() {
        let ctx = egui::Context::default();
        crate::setup_fonts(&ctx);
        let measure = |ctx: &egui::Context| {
            let mut width = 0.0;
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                width = ctx.fonts(|fonts| {
                    fonts
                        .layout_no_wrap("iiii".to_owned(), egui::FontId::monospace(14.0), egui::Color32::WHITE)
                        .size()
                        .x
                });
            });
            width
        };
        let default = measure(&ctx);
        let path = temp_font();
        apply(
            &ctx,
            Some(&Choice {
                family: "HarmonyOS Sans".into(),
                path: path.clone(),
                index: 0,
            }),
        );
        let custom = measure(&ctx);
        assert!(
            (custom - default).abs() > 0.5,
            "the custom font must actually lead the monospace stack"
        );
        // A missing file falls back to the default stack without panicking.
        apply(
            &ctx,
            Some(&Choice {
                family: "Gone".into(),
                path: path.with_extension("missing"),
                index: 0,
            }),
        );
        let fallback = measure(&ctx);
        assert!((fallback - default).abs() < 0.01);
    }
}

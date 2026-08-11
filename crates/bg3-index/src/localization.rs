use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::package::{checked_slice, read_package_entry, read_u16, read_u32};

const LOCA_SIGNATURE: &[u8; 4] = b"LOCA";
const LOCA_HEADER_SIZE: usize = 12;
const LOCA_ENTRY_SIZE: usize = 70;
const MAX_HANDLE_LENGTH: usize = 64;
const CATALOG_RECORD_SIZE: usize = 15;
const MAX_LOCALIZATION_SIZE: usize = 256 * 1024 * 1024;
const MAX_LOCALIZATION_ENTRIES: usize = 2_000_000;

/// One borrowed localization value from a loose or packed language catalog.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LocalizedText<'a> {
    pub version: u16,
    pub text: &'a str,
}

/// Read-only base-game localization values with no navigable source ranges.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct LocalizationCatalog {
    language: String,
    records: Vec<u8>,
    handles: String,
    texts: String,
}

impl LocalizationCatalog {
    /// Creates an empty catalog for one configured language.
    pub fn new(language: impl Into<String>) -> Self {
        Self {
            language: language.into(),
            records: Vec::new(),
            handles: String::new(),
            texts: String::new(),
        }
    }

    /// Builds a compact sorted catalog and lets the last duplicate handle win.
    pub fn from_entries(
        language: impl Into<String>,
        entries: impl IntoIterator<Item = (String, u16, String)>,
    ) -> Result<Self, Error> {
        let mut values = Vec::new();
        for (handle, version, text) in entries {
            if !valid_handle(&handle) {
                return Err(Error::Localization(format!(
                    "localization handle `{handle}` is not valid"
                )));
            }
            values.push((handle, version, text));
        }
        // Stable sorting keeps declaration order within a duplicate group. The
        // loop can then retain the last declaration without another lookup map.
        values.sort_by(|left, right| left.0.cmp(&right.0));

        let mut catalog = Self::new(language);
        catalog.records.reserve(
            values
                .len()
                .checked_mul(CATALOG_RECORD_SIZE)
                .ok_or_else(|| Error::Localization("catalog record size overflowed".into()))?,
        );
        let mut values = values.into_iter().peekable();
        while let Some((mut handle, mut version, mut text)) = values.next() {
            while values
                .peek()
                .is_some_and(|(next_handle, _, _)| next_handle == &handle)
            {
                (handle, version, text) = values.next().expect("peeked duplicate exists");
            }
            let handle_start = u32::try_from(catalog.handles.len()).map_err(|_| {
                Error::Localization("catalog handle offset exceeds four bytes".into())
            })?;
            let handle_length = u8::try_from(handle.len())
                .map_err(|_| Error::Localization("one catalog handle exceeds one byte".into()))?;
            let text_start = u32::try_from(catalog.texts.len()).map_err(|_| {
                Error::Localization("catalog text offset exceeds four bytes".into())
            })?;
            let text_length = u32::try_from(text.len()).map_err(|_| {
                Error::Localization("one catalog text value exceeds four bytes".into())
            })?;
            catalog.handles.push_str(&handle);
            catalog.texts.push_str(&text);
            catalog
                .records
                .extend_from_slice(&handle_start.to_le_bytes());
            catalog.records.push(handle_length);
            catalog.records.extend_from_slice(&version.to_le_bytes());
            catalog.records.extend_from_slice(&text_start.to_le_bytes());
            catalog
                .records
                .extend_from_slice(&text_length.to_le_bytes());
        }
        Ok(catalog)
    }

    /// Returns the configured language label.
    pub fn language(&self) -> &str {
        &self.language
    }

    /// Returns the localized value for one exact handle.
    pub fn get(&self, handle: &str) -> Option<LocalizedText<'_>> {
        if !valid_handle(handle) {
            return None;
        }
        let mut left = 0;
        let mut right = self.len();
        while left < right {
            let middle = left + (right - left) / 2;
            match self.record_handle(middle)?.cmp(handle) {
                std::cmp::Ordering::Less => left = middle + 1,
                std::cmp::Ordering::Greater => right = middle,
                std::cmp::Ordering::Equal => {
                    let record = self.record(middle)?;
                    let version = u16::from_le_bytes(record[5..7].try_into().ok()?);
                    let start =
                        usize::try_from(u32::from_le_bytes(record[7..11].try_into().ok()?)).ok()?;
                    let length =
                        usize::try_from(u32::from_le_bytes(record[11..15].try_into().ok()?))
                            .ok()?;
                    return Some(LocalizedText {
                        version,
                        text: self.texts.get(start..start.checked_add(length)?)?,
                    });
                }
            }
        }
        None
    }

    /// Returns the number of indexed handles.
    pub fn len(&self) -> usize {
        self.records.len() / CATALOG_RECORD_SIZE
    }

    /// Tests whether the catalog contains no handles.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Returns one complete fixed-width record from a cached byte arena.
    fn record(&self, index: usize) -> Option<&[u8]> {
        let start = index.checked_mul(CATALOG_RECORD_SIZE)?;
        self.records
            .get(start..start.checked_add(CATALOG_RECORD_SIZE)?)
    }

    /// Returns the binary handle key from one complete catalog record.
    fn record_handle(&self, index: usize) -> Option<&str> {
        let record = self.record(index)?;
        let start = usize::try_from(u32::from_le_bytes(record[..4].try_into().ok()?)).ok()?;
        let length = usize::from(record[4]);
        self.handles.get(start..start.checked_add(length)?)
    }
}

/// Reads the configured base language package when the package exists.
pub fn read_base_localization_package(
    game_data: &Path,
    language: &str,
) -> Result<Option<LocalizationCatalog>, Error> {
    let path = game_data
        .join("Localization")
        .join(format!("{language}.pak"));
    if !path.is_file() {
        return Ok(None);
    }
    read_localization_package(&path, language).map(Some)
}

/// Reads one canonical LOCA entry from a constrained BG3 v18 language package.
pub fn read_localization_package(
    path: &Path,
    language: &str,
) -> Result<LocalizationCatalog, Error> {
    let expected = canonical_entry_name(language);
    let loca = read_package_entry(path, &expected, MAX_LOCALIZATION_SIZE)?;
    parse_loca(&loca, language)
}

/// Decodes one bounded LOCA payload into a handle lookup table.
fn parse_loca(bytes: &[u8], language: &str) -> Result<LocalizationCatalog, Error> {
    if bytes.len() < LOCA_HEADER_SIZE {
        return Err(Error::Localization("LOCA header is truncated".into()));
    }
    if checked_slice(bytes, 0, 4, "LOCA signature")? != LOCA_SIGNATURE {
        return Err(Error::Localization("LOCA signature is not valid".into()));
    }
    let count = usize::try_from(read_u32(bytes, 4, "LOCA entry count")?)
        .map_err(|_| Error::Localization("LOCA entry count does not fit in memory".into()))?;
    if count > MAX_LOCALIZATION_ENTRIES {
        return Err(Error::Localization(format!(
            "LOCA entry count exceeds the {MAX_LOCALIZATION_ENTRIES} entry limit"
        )));
    }
    let table_end = LOCA_HEADER_SIZE
        .checked_add(
            count
                .checked_mul(LOCA_ENTRY_SIZE)
                .ok_or_else(|| Error::Localization("LOCA table size overflowed".into()))?,
        )
        .ok_or_else(|| Error::Localization("LOCA table bounds overflowed".into()))?;
    checked_slice(bytes, 0, table_end, "LOCA entry table")?;
    let texts_offset = usize::try_from(read_u32(bytes, 8, "LOCA text offset")?)
        .map_err(|_| Error::Localization("LOCA text offset does not fit in memory".into()))?;
    if texts_offset < table_end || texts_offset > bytes.len() {
        return Err(Error::Localization(format!(
            "LOCA text offset {texts_offset} is outside the valid range {table_end}..={}",
            bytes.len()
        )));
    }

    let mut metadata = Vec::with_capacity(count);
    for index in 0..count {
        let start = LOCA_HEADER_SIZE + index * LOCA_ENTRY_SIZE;
        let entry = checked_slice(bytes, start, LOCA_ENTRY_SIZE, "LOCA entry")?;
        let key_end = entry[..64].iter().position(|byte| *byte == 0).unwrap_or(64);
        let key = std::str::from_utf8(&entry[..key_end])
            .map_err(|error| Error::Localization(format!("LOCA handle is not UTF-8: {error}")))?
            .to_owned();
        let version = read_u16(entry, 64, "LOCA version")?;
        let length = usize::try_from(read_u32(entry, 66, "LOCA text length")?)
            .map_err(|_| Error::Localization("LOCA text length does not fit in memory".into()))?;
        if length == 0 {
            return Err(Error::Localization(format!(
                "LOCA handle `{key}` has a zero text length"
            )));
        }
        metadata.push((key, version, length));
    }

    let mut entries = Vec::with_capacity(count);
    let mut cursor = texts_offset;
    for (key, version, length) in metadata {
        let stored = checked_slice(bytes, cursor, length, "LOCA text")?;
        if stored.last() != Some(&0) {
            return Err(Error::Localization(format!(
                "LOCA text for `{key}` has no null terminator"
            )));
        }
        let text = std::str::from_utf8(&stored[..stored.len() - 1])
            .map_err(|error| {
                Error::Localization(format!("LOCA text for `{key}` is not UTF-8: {error}"))
            })?
            .to_owned();
        entries.push((key, version, text));
        cursor = cursor
            .checked_add(length)
            .ok_or_else(|| Error::Localization("LOCA text bounds overflowed".into()))?;
    }
    LocalizationCatalog::from_entries(language, entries)
}

/// Accepts canonical ASCII handles without assuming their generated suffix form.
pub(crate) fn valid_handle(handle: &str) -> bool {
    let source = handle.as_bytes();
    !source.is_empty()
        && source.len() <= MAX_HANDLE_LENGTH
        && source[0] == b'h'
        && source.is_ascii()
}

/// Returns the canonical language entry path inside a language package.
fn canonical_entry_name(language: &str) -> String {
    format!(
        "Localization/{language}/{}.loca",
        language.to_ascii_lowercase()
    )
}

/// Returns the package path for cache keys and file watches.
pub fn base_localization_package_path(game_data: &Path, language: &str) -> PathBuf {
    game_data
        .join("Localization")
        .join(format!("{language}.pak"))
}

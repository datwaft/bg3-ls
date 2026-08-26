//! Detection of the build identifier shipped with a BG3 installation.
//!
//! The build identifier is deliberately read from an installation rather than
//! inferred from a patch name.  On macOS it is stored in the application
//! bundle's `Info.plist`; on Windows (including Proton installations) it is
//! stored in the executable's PE version resource.

use std::fs;
use std::path::{Path, PathBuf};

use quick_xml::Reader;
use quick_xml::events::Event;
use thiserror::Error;

/// The installation artifact from which a build identifier was obtained.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GameBuildVersionSource {
    /// The macOS application bundle metadata file.
    MacOsInfoPlist(PathBuf),
    /// A Windows executable containing a PE version resource.
    WindowsExecutable(PathBuf),
}

/// A build identifier read from a BG3 installation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GameBuildVersion {
    /// The Larian build identifier, for example `4.1.1.7398727`.
    pub version: String,
    /// The file that supplied [`Self::version`].
    pub source: GameBuildVersionSource,
}

/// Errors produced while locating or reading an installation build version.
#[derive(Debug, Error)]
pub enum GameBuildVersionError {
    #[error("could not find BG3 build metadata below {root}")]
    NotFound { root: PathBuf },
    #[error("invalid BG3 build metadata in {path}: {reason}")]
    Invalid { path: PathBuf, reason: String },
    #[error("conflicting BG3 build versions: {first} and {second}")]
    Conflicting { first: String, second: String },
    #[error("could not read BG3 build metadata {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Reads the build identifier from a BG3 installation root.
///
/// The function checks the macOS bundle metadata and the known Windows
/// executable locations.  If more than one artifact is present, all detected
/// identifiers must agree.  This catches an accidentally mixed installation
/// before its metadata is used to select a generated catalog.
pub fn detect_game_build_version(
    game_root: &Path,
) -> Result<GameBuildVersion, GameBuildVersionError> {
    let mut versions = Vec::new();

    for path in macos_info_plist_candidates(game_root) {
        if path.is_file() {
            let version = read_info_plist_version(&path)?;
            versions.push(GameBuildVersion {
                version,
                source: GameBuildVersionSource::MacOsInfoPlist(path),
            });
        }
    }

    for path in windows_executable_candidates(game_root) {
        if path.is_file() {
            let version = read_pe_version(&path)?;
            versions.push(GameBuildVersion {
                version,
                source: GameBuildVersionSource::WindowsExecutable(path),
            });
        }
    }

    let Some(first) = versions.first().cloned() else {
        return Err(GameBuildVersionError::NotFound {
            root: game_root.to_path_buf(),
        });
    };
    if let Some(conflict) = versions
        .iter()
        .find(|candidate| candidate.version != first.version)
    {
        return Err(GameBuildVersionError::Conflicting {
            first: first.version,
            second: conflict.version.clone(),
        });
    }
    Ok(first)
}

fn macos_info_plist_candidates(game_root: &Path) -> Vec<PathBuf> {
    let mut candidates = vec![game_root.join("Contents/Info.plist")];
    if game_root.file_name().is_some_and(|name| name == "Contents") {
        candidates.push(game_root.join("Info.plist"));
    }
    candidates
}

fn windows_executable_candidates(game_root: &Path) -> Vec<PathBuf> {
    ["bin/bg3.exe", "bin/bg3_dx11.exe", "bg3.exe"]
        .into_iter()
        .map(|relative| game_root.join(relative))
        .collect()
}

fn read_info_plist_version(path: &Path) -> Result<String, GameBuildVersionError> {
    let bytes = fs::read(path).map_err(|source| GameBuildVersionError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let (version, short_version) =
        parse_info_plist_versions(&bytes).map_err(|reason| GameBuildVersionError::Invalid {
            path: path.to_path_buf(),
            reason,
        })?;
    if let Some(short_version) = short_version
        && short_version != version
    {
        return Err(GameBuildVersionError::Invalid {
            path: path.to_path_buf(),
            reason: format!(
                "CFBundleShortVersionString ({short_version}) and CFBundleVersion ({version}) disagree"
            ),
        });
    }
    Ok(version)
}

fn parse_info_plist_versions(bytes: &[u8]) -> Result<(String, Option<String>), String> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut current_key = None;
    let mut bundle_version = None;
    let mut short_version = None;

    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(|error| error.to_string())?
        {
            Event::Start(event) if event.name().as_ref() == b"key" => {
                let key = reader
                    .read_text(event.name())
                    .map_err(|error| error.to_string())?
                    .xml10_content()
                    .map_err(|error| error.to_string())?;
                current_key = Some(key.into_owned());
            }
            Event::Start(event) if event.name().as_ref() == b"string" => {
                let value = reader
                    .read_text(event.name())
                    .map_err(|error| error.to_string())?
                    .xml10_content()
                    .map_err(|error| error.to_string())?;
                match current_key.take().as_deref() {
                    Some("CFBundleVersion") => bundle_version = Some(value.into_owned()),
                    Some("CFBundleShortVersionString") => {
                        short_version = Some(value.into_owned());
                    }
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }

    let bundle_version = bundle_version
        .filter(|version| !version.trim().is_empty())
        .ok_or_else(|| "missing or empty CFBundleVersion string".to_string())?;
    Ok((bundle_version, short_version))
}

fn read_pe_version(path: &Path) -> Result<String, GameBuildVersionError> {
    let bytes = fs::read(path).map_err(|source| GameBuildVersionError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    parse_pe_version(&bytes).map_err(|reason| GameBuildVersionError::Invalid {
        path: path.to_path_buf(),
        reason,
    })
}

fn parse_pe_version(bytes: &[u8]) -> Result<String, String> {
    let resource = pe_version_resource(bytes)?;
    let mut values = VersionValues::default();
    scan_version_block(resource, 0, resource.len(), &mut values)?;

    match (values.product_version, values.file_version) {
        (Some(product), Some(file)) if product != file => Err(format!(
            "ProductVersion ({product}) and FileVersion ({file}) disagree"
        )),
        (Some(product), _) | (None, Some(product)) if !product.trim().is_empty() => Ok(product),
        _ => Err("version resource has no ProductVersion or FileVersion".into()),
    }
}

#[derive(Default)]
struct VersionValues {
    product_version: Option<String>,
    file_version: Option<String>,
}

fn scan_version_block(
    bytes: &[u8],
    start: usize,
    limit: usize,
    values: &mut VersionValues,
) -> Result<(), String> {
    if start.checked_add(6).is_none_or(|end| end > limit) {
        return Err("truncated version resource block".into());
    }
    let length = usize::from(read_u16(bytes, start)?);
    let value_length = usize::from(read_u16(bytes, start + 2)?);
    let value_type = read_u16(bytes, start + 4)?;
    let end = start
        .checked_add(length)
        .filter(|end| *end <= limit)
        .ok_or_else(|| "version resource block exceeds its parent".to_string())?;

    let mut cursor = start + 6;
    let key = read_utf16_z(bytes, &mut cursor, end)?;
    cursor = align_four(cursor).ok_or_else(|| "invalid version resource alignment".to_string())?;
    let value_bytes = if value_type == 1 {
        value_length
            .checked_mul(2)
            .ok_or_else(|| "version resource value is too large".to_string())?
    } else {
        value_length
    };
    let value_end = cursor
        .checked_add(value_bytes)
        .filter(|value_end| *value_end <= end)
        .ok_or_else(|| "truncated version resource value".to_string())?;

    if (key == "ProductVersion" || key == "FileVersion") && value_type == 1 {
        let value = decode_utf16(&bytes[cursor..value_end])
            .trim_end_matches('\0')
            .trim()
            .to_owned();
        if key == "ProductVersion" {
            values.product_version = Some(value);
        } else {
            values.file_version = Some(value);
        }
    }

    cursor = align_four(value_end).unwrap_or(value_end);
    while cursor + 6 <= end {
        let child_length = usize::from(read_u16(bytes, cursor)?);
        if child_length == 0 {
            break;
        }
        scan_version_block(bytes, cursor, end, values)?;
        cursor = cursor
            .checked_add(child_length)
            .ok_or_else(|| "version resource child offset overflow".to_string())?;
        cursor =
            align_four(cursor).ok_or_else(|| "invalid version resource alignment".to_string())?;
    }
    Ok(())
}

fn read_utf16_z(bytes: &[u8], cursor: &mut usize, limit: usize) -> Result<String, String> {
    let mut units = Vec::new();
    loop {
        if cursor.checked_add(2).is_none_or(|end| end > limit) {
            return Err("unterminated version resource key".into());
        }
        let unit = u16::from_le_bytes([bytes[*cursor], bytes[*cursor + 1]]);
        *cursor += 2;
        if unit == 0 {
            return String::from_utf16(&units).map_err(|_| "invalid UTF-16 version key".into());
        }
        units.push(unit);
    }
}

fn decode_utf16(bytes: &[u8]) -> String {
    let units = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]));
    String::from_utf16_lossy(&units.collect::<Vec<_>>())
}

fn pe_version_resource(bytes: &[u8]) -> Result<&[u8], String> {
    if bytes.get(0..2) != Some(b"MZ") {
        return Err("missing DOS executable header".into());
    }
    let pe_offset = usize::try_from(read_u32(bytes, 0x3c)?).map_err(|_| "invalid PE offset")?;
    if bytes.get(pe_offset..pe_offset + 4) != Some(b"PE\0\0") {
        return Err("missing PE signature".into());
    }
    let section_count = usize::from(read_u16(bytes, pe_offset + 6)?);
    let optional_size = usize::from(read_u16(bytes, pe_offset + 20)?);
    let optional_start = pe_offset + 24;
    let magic = read_u16(bytes, optional_start)?;
    if magic != 0x10b && magic != 0x20b {
        return Err("unsupported PE optional header".into());
    }
    let resource_rva = read_u32(bytes, optional_start + 96 + 16)?;
    let resource_size = usize::try_from(read_u32(bytes, optional_start + 96 + 20)?)
        .map_err(|_| "invalid PE resource size")?;
    if resource_rva == 0 || resource_size == 0 {
        return Err("PE has no resource directory".into());
    }

    let section_start = optional_start + optional_size;
    let resource_offset = rva_to_file_offset(bytes, section_start, section_count, resource_rva)?;
    let resource_end = resource_offset
        .checked_add(resource_size)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| "PE resource directory exceeds the file".to_string())?;
    let resource = &bytes[resource_offset..resource_end];
    let data = find_resource_data(
        resource,
        0,
        0,
        resource_rva,
        bytes,
        section_start,
        section_count,
    )?;
    Ok(data)
}

fn find_resource_data<'a>(
    resource: &'a [u8],
    directory: usize,
    depth: usize,
    resource_rva: u32,
    file: &'a [u8],
    section_start: usize,
    section_count: usize,
) -> Result<&'a [u8], String> {
    if depth == 4 {
        let data_rva = read_u32(resource, directory)?;
        let size = usize::try_from(read_u32(resource, directory + 4)?)
            .map_err(|_| "invalid version resource data size")?;
        let offset = rva_to_file_offset(file, section_start, section_count, data_rva)?;
        return offset
            .checked_add(size)
            .filter(|end| *end <= file.len())
            .map(|end| &file[offset..end])
            .ok_or_else(|| "version resource data exceeds the file".into());
    }
    let entries = directory_entries(resource, directory)?;
    for (id, child) in entries {
        if depth == 0 && id != Some(16) {
            continue;
        }
        if child & 0x8000_0000 == 0 && depth != 2 && depth != 3 {
            continue;
        }
        let child_directory = usize::try_from(child & 0x7fff_ffff)
            .map_err(|_| "invalid version resource directory offset")?;
        return find_resource_data(
            resource,
            child_directory,
            depth + 1,
            resource_rva,
            file,
            section_start,
            section_count,
        );
    }
    let _ = resource_rva;
    Err("version resource has no data entry".into())
}

fn directory_entries(resource: &[u8], directory: usize) -> Result<Vec<(Option<u32>, u32)>, String> {
    let named = usize::from(read_u16(resource, directory + 12)?);
    let ids = usize::from(read_u16(resource, directory + 14)?);
    let count = named
        .checked_add(ids)
        .ok_or_else(|| "invalid version resource entry count".to_string())?;
    let mut entries = Vec::with_capacity(count);
    for index in 0..count {
        let offset = directory
            .checked_add(16 + index * 8)
            .ok_or_else(|| "version resource directory offset overflow".to_string())?;
        let name = read_u32(resource, offset)?;
        let child = read_u32(resource, offset + 4)?;
        let id = if index < named {
            None
        } else {
            Some(name & 0x7fff_ffff)
        };
        entries.push((id, child));
    }
    Ok(entries)
}

fn rva_to_file_offset(
    bytes: &[u8],
    section_start: usize,
    section_count: usize,
    rva: u32,
) -> Result<usize, String> {
    for index in 0..section_count {
        let section = section_start
            .checked_add(index * 40)
            .ok_or_else(|| "PE section table offset overflow".to_string())?;
        let virtual_size = read_u32(bytes, section + 8)?;
        let virtual_address = read_u32(bytes, section + 12)?;
        let raw_size = read_u32(bytes, section + 16)?;
        let raw_offset = usize::try_from(read_u32(bytes, section + 20)?)
            .map_err(|_| "invalid PE section file offset")?;
        let span = virtual_size.max(raw_size);
        if rva >= virtual_address && rva - virtual_address < span {
            return raw_offset
                .checked_add(usize::try_from(rva - virtual_address).unwrap_or(usize::MAX))
                .filter(|offset| *offset < bytes.len())
                .ok_or_else(|| "PE section offset exceeds the file".into());
        }
    }
    Err("RVA is not contained in a PE section".into())
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, String> {
    bytes
        .get(offset..offset + 2)
        .and_then(|bytes| bytes.try_into().ok())
        .map(u16::from_le_bytes)
        .ok_or_else(|| "truncated PE/version resource data".into())
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, String> {
    bytes
        .get(offset..offset + 4)
        .and_then(|bytes| bytes.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or_else(|| "truncated PE/version resource data".into())
}

fn align_four(value: usize) -> Option<usize> {
    value.checked_add(3).map(|value| value & !3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn detects_macos_bundle_version() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("Contents")).unwrap();
        fs::write(
            root.path().join("Contents/Info.plist"),
            br#"<?xml version="1.0"?><plist><dict><key>CFBundleShortVersionString</key><string>4.1.1.7398727</string><key>CFBundleVersion</key><string>4.1.1.7398727</string></dict></plist>"#,
        )
        .unwrap();

        let detected = detect_game_build_version(root.path()).unwrap();
        assert_eq!(detected.version, "4.1.1.7398727");
        assert!(matches!(
            detected.source,
            GameBuildVersionSource::MacOsInfoPlist(_)
        ));
    }

    #[test]
    fn rejects_disagreeing_macos_version_keys() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("Contents")).unwrap();
        fs::write(
            root.path().join("Contents/Info.plist"),
            br#"<plist><dict><key>CFBundleShortVersionString</key><string>4.1.1</string><key>CFBundleVersion</key><string>4.1.1.7398727</string></dict></plist>"#,
        )
        .unwrap();

        assert!(matches!(
            detect_game_build_version(root.path()),
            Err(GameBuildVersionError::Invalid { .. })
        ));
    }

    #[test]
    fn detects_product_version_from_synthetic_pe_resource() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("bin")).unwrap();
        fs::write(
            root.path().join("bin/bg3.exe"),
            synthetic_pe_version("4.1.1.7398727", "4.1.1.7398727"),
        )
        .unwrap();

        let detected = detect_game_build_version(root.path()).unwrap();
        assert_eq!(detected.version, "4.1.1.7398727");
        assert!(matches!(
            detected.source,
            GameBuildVersionSource::WindowsExecutable(_)
        ));
    }

    #[test]
    fn rejects_disagreeing_version_sources() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("Contents")).unwrap();
        fs::create_dir(root.path().join("bin")).unwrap();
        fs::write(
            root.path().join("Contents/Info.plist"),
            br#"<plist><dict><key>CFBundleVersion</key><string>4.1.1.1</string></dict></plist>"#,
        )
        .unwrap();
        fs::write(
            root.path().join("bin/bg3.exe"),
            synthetic_pe_version("4.1.1.2", "4.1.1.2"),
        )
        .unwrap();

        assert!(matches!(
            detect_game_build_version(root.path()),
            Err(GameBuildVersionError::Conflicting { .. })
        ));
    }

    fn synthetic_pe_version(product: &str, file: &str) -> Vec<u8> {
        let version = version_block(
            "VS_VERSION_INFO",
            &[],
            &[version_block(
                "StringFileInfo",
                &[],
                &[version_block(
                    "040904b0",
                    &[],
                    &[
                        version_block("ProductVersion", &utf16(product), &[]),
                        version_block("FileVersion", &utf16(file), &[]),
                    ],
                )],
            )],
        );
        let raw_offset = 0x200usize;
        let resource_data_offset = 0x100usize;
        let resource_rva = 0x1000u32;
        let data_rva = resource_rva + u32::try_from(resource_data_offset).unwrap();
        let mut bytes = vec![0; raw_offset + 0x400];
        bytes[0..2].copy_from_slice(b"MZ");
        bytes[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
        put_u16(&mut bytes, 0x86, 1);
        put_u16(&mut bytes, 0x94, 0xe0);
        let optional = 0x98;
        put_u16(&mut bytes, optional, 0x10b);
        put_u32(&mut bytes, optional + 96 + 16, resource_rva);
        put_u32(&mut bytes, optional + 96 + 20, 0x400);
        let section = optional + 0xe0;
        bytes[section..section + 8].copy_from_slice(b".rsrc\0\0\0");
        put_u32(&mut bytes, section + 8, 0x400);
        put_u32(&mut bytes, section + 12, resource_rva);
        put_u32(&mut bytes, section + 16, 0x400);
        put_u32(&mut bytes, section + 20, u32::try_from(raw_offset).unwrap());
        let resource = raw_offset;
        put_u16(&mut bytes, resource + 14, 1);
        put_u32(&mut bytes, resource + 16, 16);
        put_u32(&mut bytes, resource + 20, 0x8000_0020);
        for directory in [0x20, 0x40] {
            put_u16(&mut bytes, resource + directory + 14, 1);
            put_u32(&mut bytes, resource + directory + 16, 1);
            put_u32(
                &mut bytes,
                resource + directory + 20,
                (0x8000_0000 | (directory + 0x20)) as u32,
            );
        }
        put_u16(&mut bytes, resource + 0x60 + 14, 1);
        put_u32(&mut bytes, resource + 0x60 + 16, 1033);
        put_u32(&mut bytes, resource + 0x60 + 20, 0x80);
        put_u32(&mut bytes, resource + 0x80, data_rva);
        put_u32(
            &mut bytes,
            resource + 0x80 + 4,
            u32::try_from(version.len()).unwrap(),
        );
        bytes[resource + resource_data_offset..resource + resource_data_offset + version.len()]
            .copy_from_slice(&version);
        bytes
    }

    fn version_block(key: &str, value: &[u8], children: &[Vec<u8>]) -> Vec<u8> {
        let mut block = vec![0, 0, 0, 0, 1, 0];
        for unit in key.encode_utf16() {
            block.extend_from_slice(&unit.to_le_bytes());
        }
        block.extend_from_slice(&0u16.to_le_bytes());
        while block.len() % 4 != 0 {
            block.push(0);
        }
        block.extend_from_slice(value);
        while block.len() % 4 != 0 {
            block.push(0);
        }
        for child in children {
            block.extend_from_slice(child);
        }
        let length = u16::try_from(block.len()).unwrap();
        let value_length = u16::try_from(value.len() / 2).unwrap();
        block[0..2].copy_from_slice(&length.to_le_bytes());
        block[2..4].copy_from_slice(&value_length.to_le_bytes());
        block
    }

    fn utf16(value: &str) -> Vec<u8> {
        value
            .encode_utf16()
            .chain(std::iter::once(0))
            .flat_map(|unit| unit.to_le_bytes())
            .collect()
    }

    fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
}

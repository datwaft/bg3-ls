use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::Error;

const PACKAGE_SIGNATURE: &[u8; 4] = b"LSPK";
const PACKAGE_VERSION: u32 = 18;
const PACKAGE_HEADER_SIZE: usize = 40;
const PACKAGE_ENTRY_SIZE: usize = 272;
const MAX_FILE_LIST_SIZE: usize = 64 * 1024 * 1024;
const MAX_FILE_COUNT: usize = MAX_FILE_LIST_SIZE / PACKAGE_ENTRY_SIZE;
const SOLID_PACKAGE_FLAG: u8 = 0x04;

/// The bounded metadata read from an LSPK package header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PackageHeader {
    /// The package format version.
    pub version: u32,
    /// Package flags from the v18 header.
    pub flags: u8,
    /// The package load priority from the v18 header.
    pub priority: u8,
    /// The package checksum stored in the v18 header.
    pub checksum: [u8; 16],
    /// Number of archive parts declared by the package.
    pub parts: u16,
    /// Byte offset of the compressed file list.
    pub file_list_offset: u64,
    /// Size of the compressed file list, including its eight-byte prefix.
    pub file_list_size: usize,
}

/// The archive layout observed before the reader validates its file list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PackageLayout {
    Supported,
    UnsupportedVersion,
    Solid,
    Multipart,
}

/// Bounded package metadata and its supported-reader classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PackageInspection {
    header: PackageHeader,
    layout: PackageLayout,
}

impl PackageInspection {
    pub(crate) fn header(self) -> PackageHeader {
        self.header
    }

    pub(crate) fn layout(self) -> PackageLayout {
        self.layout
    }
}

/// Reads the package checksum used for inexpensive cache invalidation.
pub(crate) fn package_fingerprint(path: &Path) -> Result<[u8; 16], Error> {
    let mut package = File::open(path)?;
    let package_size = package.metadata()?.len();
    Ok(read_header(&mut package, package_size)?.checksum)
}

/// One entry from an LSPK v18 file list.
///
/// The entry stores a package-relative name and byte ranges. It does not
/// represent a filesystem path. Duplicate names remain separate entries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageEntry {
    name: String,
    offset: u64,
    archive_part: u8,
    compression: u8,
    size_on_disk: usize,
    uncompressed_size: usize,
}

impl PackageEntry {
    /// Returns the package-relative entry name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the archive part containing this entry.
    pub fn archive_part(&self) -> u8 {
        self.archive_part
    }

    /// Returns the LSPK compression method (zero for stored data, two for
    /// LZ4 block data).
    pub fn compression(&self) -> u8 {
        self.compression
    }

    /// Returns the stored byte count.
    pub fn size_on_disk(&self) -> usize {
        self.size_on_disk
    }

    /// Returns the decoded byte count declared by the package.
    pub fn uncompressed_size(&self) -> usize {
        self.uncompressed_size
    }
}

/// A read-only, bounded view of one supported LSPK v18 package.
///
/// Opening a package reads only its header and compressed file list. Entry
/// contents are read on demand and are never extracted to the filesystem.
#[derive(Clone, Debug)]
pub struct PackageReader {
    path: PathBuf,
    header: PackageHeader,
    entries: Vec<PackageEntry>,
    package_size: u64,
}

impl PackageReader {
    /// Opens and validates one single-part, non-solid LSPK v18 package.
    pub fn open(path: &Path) -> Result<Self, Error> {
        let mut package = File::open(path)?;
        let package_size = package.metadata()?.len();
        let header = read_header(&mut package, package_size)?;
        let file_list = read_file_list(&mut package, &header, package_size)?;
        let entries = parse_entries(
            &file_list,
            package_size,
            header.file_list_offset,
            header.file_list_size,
        )?;
        Ok(Self {
            path: path.to_owned(),
            header,
            entries,
            package_size,
        })
    }

    /// Returns the package path used to open this reader.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the validated package header.
    pub fn header(&self) -> PackageHeader {
        self.header
    }

    /// Returns all entries in their file-list order, including duplicates.
    pub fn entries(&self) -> &[PackageEntry] {
        &self.entries
    }

    /// Returns exact Thoth source entries owned by `module`.
    ///
    /// The module name is treated as one path component. The returned entries
    /// retain package-relative names and can be read with [`Self::read_entry`].
    pub fn thoth_entries(&self, module: &str) -> Result<Vec<&PackageEntry>, Error> {
        validate_module_name(module)?;
        let prefix = format!("Mods/{module}/Scripts/thoth/");
        Ok(self
            .entries
            .iter()
            .filter(|entry| {
                entry.name.starts_with(&prefix)
                    && entry.name.ends_with(".khn")
                    && entry.name.len() > prefix.len() + ".khn".len()
            })
            .collect())
    }

    /// Returns exact Osiris goal entries from one module in the package.
    pub fn osiris_goal_entries(&self, module: &str) -> Result<Vec<&PackageEntry>, Error> {
        validate_module_name(module)?;
        let prefix = format!("Mods/{module}/Story/RawFiles/Goals/");
        Ok(self
            .entries
            .iter()
            .filter(|entry| {
                let Some(file) = entry.name.strip_prefix(&prefix) else {
                    return false;
                };
                file.ends_with(".txt")
                    && file.len() > ".txt".len()
                    && !file.contains('/')
                    && !file.contains('\\')
                    && !file.chars().any(char::is_control)
            })
            .collect())
    }

    /// Returns exact Thoth source entries from every module in the package.
    ///
    /// The returned entries retain package-relative names and remain in file
    /// list order. This does not change the module-specific filtering or
    /// precedence rules used by [`Self::thoth_entries`].
    pub fn all_thoth_entries(&self) -> Vec<&PackageEntry> {
        self.entries
            .iter()
            .filter(|entry| {
                let Some(module_entries) = entry.name.strip_prefix("Mods/") else {
                    return false;
                };
                let Some((module, source_path)) = module_entries.split_once("/Scripts/thoth/")
                else {
                    return false;
                };
                !module.is_empty()
                    && module != "."
                    && module != ".."
                    && !module.contains('/')
                    && !module.contains('\\')
                    && !module.chars().any(char::is_control)
                    && source_path.ends_with(".khn")
                    && !source_path.contains('\\')
                    && !source_path.chars().any(char::is_control)
                    && source_path
                        .split('/')
                        .all(|part| !part.is_empty() && part != "." && part != "..")
            })
            .collect()
    }

    /// Reads and optionally decompresses one entry with a strict allocation
    /// limit. No unrelated entry data is read.
    pub fn read_entry(
        &self,
        entry: &PackageEntry,
        max_entry_size: usize,
    ) -> Result<Vec<u8>, Error> {
        read_entry_from_file(
            &self.path,
            self.package_size,
            &self.header,
            entry,
            max_entry_size,
        )
    }
}

/// Reads bounded header metadata and classifies the archive layout.
pub(crate) fn inspect_package(path: &Path) -> Result<PackageInspection, Error> {
    let mut package = File::open(path)?;
    let package_size = package.metadata()?.len();
    let header = read_package_header(&mut package, package_size)?;
    let layout = package_layout(header);
    Ok(PackageInspection { header, layout })
}

/// Reads one exact entry without loading or decompressing unrelated package data.
pub(crate) fn read_package_entry(
    path: &Path,
    expected_name: &str,
    max_entry_size: usize,
) -> Result<Vec<u8>, Error> {
    let mut package = File::open(path)?;
    let package_size = package.metadata()?.len();
    let header = read_header(&mut package, package_size)?;
    let file_list = read_file_list(&mut package, &header, package_size)?;
    let entry = find_entry(
        &file_list,
        package_size,
        header.file_list_offset,
        header.file_list_size,
        expected_name,
    )?
    .ok_or_else(|| Error::Package(format!("package has no entry `{expected_name}`")))?;
    read_entry_from_file(path, package_size, &header, &entry, max_entry_size)
}

fn read_entry_from_file(
    path: &Path,
    package_size: u64,
    header: &PackageHeader,
    entry: &PackageEntry,
    max_entry_size: usize,
) -> Result<Vec<u8>, Error> {
    let mut package = File::open(path)?;
    if package.metadata()?.len() != package_size {
        return Err(Error::Package(
            "package changed while it was indexed".into(),
        ));
    }
    validate_entry_range(
        entry,
        package_size,
        header.file_list_offset,
        header.file_list_size,
    )?;
    if entry.size_on_disk > max_entry_size || entry.uncompressed_size > max_entry_size {
        return Err(Error::Package(format!(
            "package entry `{}` exceeds the {max_entry_size} byte limit",
            entry.name
        )));
    }
    package.seek(SeekFrom::Start(entry.offset))?;
    let mut stored = vec![0_u8; entry.size_on_disk];
    package.read_exact(&mut stored)?;
    match entry.compression {
        0 => Ok(stored),
        2 => {
            if entry.uncompressed_size == 0 {
                if !stored.is_empty() {
                    return Err(Error::Package(format!(
                        "LZ4 entry `{}` has stored bytes but no uncompressed size",
                        entry.name
                    )));
                }
                return Ok(Vec::new());
            }
            let decoded =
                lz4_flex::block::decompress(&stored, entry.uncompressed_size).map_err(|error| {
                    Error::Package(format!(
                        "cannot decompress LZ4 entry `{}`: {error}",
                        entry.name
                    ))
                })?;
            if decoded.len() != entry.uncompressed_size {
                return Err(Error::Package(format!(
                    "LZ4 entry `{}` decoded to {} bytes instead of {}",
                    entry.name,
                    decoded.len(),
                    entry.uncompressed_size
                )));
            }
            Ok(decoded)
        }
        method => Err(Error::Package(format!(
            "package entry `{}` uses unsupported compression method {method}",
            entry.name
        ))),
    }
}

fn read_header(package: &mut File, package_size: u64) -> Result<PackageHeader, Error> {
    let header = read_package_header(package, package_size)?;
    match package_layout(header) {
        PackageLayout::Supported => Ok(header),
        PackageLayout::UnsupportedVersion => Err(Error::Package(format!(
            "package version {} is unsupported; expected version {PACKAGE_VERSION}",
            header.version
        ))),
        PackageLayout::Solid => Err(Error::Package("solid packages are unsupported".into())),
        PackageLayout::Multipart => Err(Error::Package(format!(
            "multipart packages are unsupported: found {} parts",
            header.parts
        ))),
    }
}

fn package_layout(header: PackageHeader) -> PackageLayout {
    if header.version != PACKAGE_VERSION {
        PackageLayout::UnsupportedVersion
    } else if header.flags & SOLID_PACKAGE_FLAG != 0 {
        PackageLayout::Solid
    } else if header.parts != 1 {
        PackageLayout::Multipart
    } else {
        PackageLayout::Supported
    }
}

fn read_package_header(package: &mut File, package_size: u64) -> Result<PackageHeader, Error> {
    if package_size < PACKAGE_HEADER_SIZE as u64 {
        return Err(Error::Package("package header is truncated".into()));
    }
    let mut header = [0_u8; PACKAGE_HEADER_SIZE];
    package.read_exact(&mut header)?;
    if &header[..4] != PACKAGE_SIGNATURE {
        return Err(Error::Package("package signature is not `LSPK`".into()));
    }
    let version = read_u32(&header, 4, "package version")?;
    let flags = header[20];
    let parts = read_u16(&header, 38, "package part count")?;
    let file_list_offset = read_u64(&header, 8, "file-list offset")?;
    let file_list_size = usize::try_from(read_u32(&header, 16, "file-list size")?)
        .map_err(|_| Error::Package("file-list size does not fit in memory".into()))?;
    if file_list_size > MAX_FILE_LIST_SIZE {
        return Err(Error::Package(format!(
            "file list exceeds the {MAX_FILE_LIST_SIZE} byte limit"
        )));
    }
    let file_list_end =
        file_list_offset
            .checked_add(u64::try_from(file_list_size).map_err(|_| {
                Error::Package("file-list size does not fit in package bounds".into())
            })?)
            .ok_or_else(|| Error::Package("file-list bounds overflowed".into()))?;
    if file_list_offset < PACKAGE_HEADER_SIZE as u64 || file_list_end > package_size {
        return Err(Error::Package(
            "file list is outside the file bounds".into(),
        ));
    }
    Ok(PackageHeader {
        version,
        flags,
        priority: header[21],
        checksum: header[22..38]
            .try_into()
            .expect("the package checksum has sixteen bytes"),
        parts,
        file_list_offset,
        file_list_size,
    })
}

fn read_file_list(
    package: &mut File,
    header: &PackageHeader,
    package_size: u64,
) -> Result<Vec<u8>, Error> {
    if header.file_list_size < 8 {
        return Err(Error::Package("file list is truncated".into()));
    }
    let file_list_end =
        header
            .file_list_offset
            .checked_add(u64::try_from(header.file_list_size).map_err(|_| {
                Error::Package("file-list size does not fit in package bounds".into())
            })?)
            .ok_or_else(|| Error::Package("file-list bounds overflowed".into()))?;
    if file_list_end > package_size {
        return Err(Error::Package(
            "file list is outside the file bounds".into(),
        ));
    }
    package.seek(SeekFrom::Start(header.file_list_offset))?;
    let mut file_list = vec![0_u8; header.file_list_size];
    package.read_exact(&mut file_list)?;
    Ok(file_list)
}

fn decode_file_list(file_list: &[u8]) -> Result<(usize, Vec<u8>), Error> {
    let count = usize::try_from(read_u32(file_list, 0, "file count")?)
        .map_err(|_| Error::Package("file count does not fit in memory".into()))?;
    if count > MAX_FILE_COUNT {
        return Err(Error::Package(format!(
            "file count exceeds the {MAX_FILE_COUNT} entry limit"
        )));
    }
    let compressed_size = usize::try_from(read_u32(file_list, 4, "compressed file-list size")?)
        .map_err(|_| Error::Package("compressed file-list size does not fit in memory".into()))?;
    let compressed = checked_slice(file_list, 8, compressed_size, "compressed file list")?;
    if 8usize
        .checked_add(compressed_size)
        .is_none_or(|end| end != file_list.len())
    {
        return Err(Error::Package(
            "compressed file list has trailing bytes".into(),
        ));
    }
    let decoded_size = count
        .checked_mul(PACKAGE_ENTRY_SIZE)
        .ok_or_else(|| Error::Package("decoded file-list size overflowed".into()))?;
    if decoded_size > MAX_FILE_LIST_SIZE {
        return Err(Error::Package(format!(
            "decoded file list exceeds the {MAX_FILE_LIST_SIZE} byte limit"
        )));
    }
    let decoded = if decoded_size == 0 {
        if !compressed.is_empty() {
            return Err(Error::Package(
                "empty file list has compressed entry data".into(),
            ));
        }
        Vec::new()
    } else {
        lz4_flex::block::decompress(compressed, decoded_size).map_err(|error| {
            Error::Package(format!("cannot decompress the package file list: {error}"))
        })?
    };
    if decoded.len() != decoded_size {
        return Err(Error::Package(format!(
            "decoded file list has {} bytes instead of {decoded_size}",
            decoded.len()
        )));
    }
    Ok((count, decoded))
}

fn parse_entries(
    file_list: &[u8],
    package_size: u64,
    file_list_offset: u64,
    file_list_size: usize,
) -> Result<Vec<PackageEntry>, Error> {
    let (count, decoded) = decode_file_list(file_list)?;

    let mut entries = Vec::with_capacity(count);
    for index in 0..count {
        let start = index * PACKAGE_ENTRY_SIZE;
        let raw = checked_slice(&decoded, start, PACKAGE_ENTRY_SIZE, "package entry")?;
        let entry = parse_entry(raw, index)?;
        validate_entry_range(&entry, package_size, file_list_offset, file_list_size)?;
        entries.push(entry);
    }
    Ok(entries)
}

fn find_entry(
    file_list: &[u8],
    package_size: u64,
    file_list_offset: u64,
    file_list_size: usize,
    expected_name: &str,
) -> Result<Option<PackageEntry>, Error> {
    let (count, decoded) = decode_file_list(file_list)?;
    for index in 0..count {
        let start = index * PACKAGE_ENTRY_SIZE;
        let raw = checked_slice(&decoded, start, PACKAGE_ENTRY_SIZE, "package entry")?;
        let name_end = raw[..256].iter().position(|byte| *byte == 0).unwrap_or(256);
        let Ok(name) = std::str::from_utf8(&raw[..name_end]) else {
            continue;
        };
        if !name.eq_ignore_ascii_case(expected_name) {
            continue;
        }
        let entry = parse_entry(raw, index)?;
        validate_entry_range(&entry, package_size, file_list_offset, file_list_size)?;
        return Ok(Some(entry));
    }
    Ok(None)
}

fn parse_entry(raw: &[u8], index: usize) -> Result<PackageEntry, Error> {
    let name_end = raw[..256].iter().position(|byte| *byte == 0).unwrap_or(256);
    if raw[name_end..256].iter().any(|byte| *byte != 0) {
        return Err(Error::Package(format!(
            "package entry {index} has non-zero bytes after its name"
        )));
    }
    let name = std::str::from_utf8(&raw[..name_end])
        .map_err(|error| Error::Package(format!("package entry name is not UTF-8: {error}")))?;
    validate_entry_name(name)?;
    let offset_low = u64::from(read_u32(raw, 256, "entry offset")?);
    let offset_high = u64::from(read_u16(raw, 260, "entry offset")?);
    let offset = offset_low | (offset_high << 32);
    let archive_part = raw[262];
    if archive_part != 0 {
        return Err(Error::Package(format!(
            "entry `{name}` uses unsupported archive part {archive_part}"
        )));
    }
    let size_on_disk = usize::try_from(read_u32(raw, 264, "entry disk size")?)
        .map_err(|_| Error::Package("entry disk size does not fit in memory".into()))?;
    let uncompressed_size = usize::try_from(read_u32(raw, 268, "entry uncompressed size")?)
        .map_err(|_| Error::Package("entry size does not fit in memory".into()))?;
    Ok(PackageEntry {
        name: name.to_owned(),
        offset,
        archive_part,
        compression: raw[263] & 0x0f,
        size_on_disk,
        uncompressed_size,
    })
}

fn validate_entry_range(
    entry: &PackageEntry,
    package_size: u64,
    file_list_offset: u64,
    file_list_size: usize,
) -> Result<(), Error> {
    let end = entry
        .offset
        .checked_add(u64::try_from(entry.size_on_disk).map_err(|_| {
            Error::Package(format!(
                "entry `{}` size does not fit in package bounds",
                entry.name
            ))
        })?)
        .ok_or_else(|| Error::Package(format!("entry `{}` bounds overflowed", entry.name)))?;
    if end > package_size {
        return Err(Error::Package(format!(
            "entry `{}` is outside the file bounds",
            entry.name
        )));
    }
    if entry.size_on_disk != 0 {
        let file_list_end = file_list_offset
            .checked_add(u64::try_from(file_list_size).map_err(|_| {
                Error::Package("file-list size does not fit in package bounds".into())
            })?)
            .ok_or_else(|| Error::Package("file-list bounds overflowed".into()))?;
        if entry.offset < PACKAGE_HEADER_SIZE as u64
            || (entry.offset < file_list_end && file_list_offset < end)
        {
            return Err(Error::Package(format!(
                "entry `{}` overlaps package metadata",
                entry.name
            )));
        }
    }
    Ok(())
}

fn validate_module_name(module: &str) -> Result<(), Error> {
    if module.is_empty()
        || module == "."
        || module == ".."
        || module
            .chars()
            .any(|character| character == '/' || character == '\\' || character.is_control())
    {
        return Err(Error::Package(format!(
            "module name `{module}` is not one safe path component"
        )));
    }
    Ok(())
}

fn validate_entry_name(name: &str) -> Result<(), Error> {
    if name.is_empty()
        || name.starts_with('/')
        || name.contains('\\')
        || name.chars().any(char::is_control)
        || name
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(Error::Package(format!(
            "package entry name `{name}` is not a safe relative path"
        )));
    }
    Ok(())
}

pub(crate) fn checked_slice<'a>(
    bytes: &'a [u8],
    offset: usize,
    length: usize,
    label: &str,
) -> Result<&'a [u8], Error> {
    let end = offset
        .checked_add(length)
        .ok_or_else(|| Error::Package(format!("{label} bounds overflowed")))?;
    bytes
        .get(offset..end)
        .ok_or_else(|| Error::Package(format!("{label} is outside the file bounds")))
}

pub(crate) fn read_u16(bytes: &[u8], offset: usize, label: &str) -> Result<u16, Error> {
    let value: [u8; 2] = checked_slice(bytes, offset, 2, label)?
        .try_into()
        .expect("the checked slice has two bytes");
    Ok(u16::from_le_bytes(value))
}

pub(crate) fn read_u32(bytes: &[u8], offset: usize, label: &str) -> Result<u32, Error> {
    let value: [u8; 4] = checked_slice(bytes, offset, 4, label)?
        .try_into()
        .expect("the checked slice has four bytes");
    Ok(u32::from_le_bytes(value))
}

fn read_u64(bytes: &[u8], offset: usize, label: &str) -> Result<u64, Error> {
    let value: [u8; 8] = checked_slice(bytes, offset, 8, label)?
        .try_into()
        .expect("the checked slice has eight bytes");
    Ok(u64::from_le_bytes(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn synthetic_package(entries: &[(&str, &[u8], u8)], priority: u8) -> Vec<u8> {
        let mut stored_entries = Vec::new();
        let mut raw_entries = Vec::new();
        let mut offset = PACKAGE_HEADER_SIZE;
        for (name, contents, compression) in entries {
            let stored = match compression {
                0 => contents.to_vec(),
                2 => lz4_flex::block::compress(contents),
                _ => contents.to_vec(),
            };
            let mut entry = vec![0_u8; PACKAGE_ENTRY_SIZE];
            entry[..name.len()].copy_from_slice(name.as_bytes());
            entry[256..260].copy_from_slice(&u32::try_from(offset).unwrap().to_le_bytes());
            entry[263] = *compression;
            entry[264..268].copy_from_slice(&u32::try_from(stored.len()).unwrap().to_le_bytes());
            entry[268..272].copy_from_slice(
                &u32::try_from(if *compression == 0 { 0 } else { contents.len() })
                    .unwrap()
                    .to_le_bytes(),
            );
            offset += stored.len();
            stored_entries.push(stored);
            raw_entries.extend_from_slice(&entry);
        }
        let compressed_list = lz4_flex::block::compress(&raw_entries);
        let mut file_list = Vec::with_capacity(8 + compressed_list.len());
        file_list.extend_from_slice(&u32::try_from(entries.len()).unwrap().to_le_bytes());
        file_list.extend_from_slice(&u32::try_from(compressed_list.len()).unwrap().to_le_bytes());
        file_list.extend_from_slice(&compressed_list);
        let file_list_offset = offset;
        let mut package = Vec::with_capacity(file_list_offset + file_list.len());
        package.extend_from_slice(b"LSPK");
        package.extend_from_slice(&18_u32.to_le_bytes());
        package.extend_from_slice(&u64::try_from(file_list_offset).unwrap().to_le_bytes());
        package.extend_from_slice(&u32::try_from(file_list.len()).unwrap().to_le_bytes());
        package.push(0);
        package.push(priority);
        package.extend_from_slice(&[0_u8; 16]);
        package.extend_from_slice(&1_u16.to_le_bytes());
        for stored in stored_entries {
            package.extend_from_slice(&stored);
        }
        package.extend_from_slice(&file_list);
        package
    }

    fn rewrite_file_list(package: &[u8], decoded: &[u8]) -> Vec<u8> {
        let file_list_offset =
            usize::try_from(u64::from_le_bytes(package[8..16].try_into().unwrap())).unwrap();
        let count = &package[file_list_offset..file_list_offset + 4];
        let compressed = lz4_flex::block::compress(decoded);
        let file_list_size = 8 + compressed.len();
        let mut rewritten = package[..file_list_offset].to_vec();
        rewritten.extend_from_slice(count);
        rewritten.extend_from_slice(&u32::try_from(compressed.len()).unwrap().to_le_bytes());
        rewritten.extend_from_slice(&compressed);
        rewritten[16..20].copy_from_slice(&u32::try_from(file_list_size).unwrap().to_le_bytes());
        rewritten
    }

    #[test]
    fn lists_thoth_entries_and_reads_both_supported_compressions() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("base.pak");
        fs::write(
            &path,
            synthetic_package(
                &[
                    (
                        "Mods/Configured/Scripts/thoth/helpers/First.khn",
                        b"first",
                        0,
                    ),
                    ("Mods/Other/Scripts/thoth/Other.khn", b"other", 0),
                    (
                        "Mods/Configured/Scripts/thoth/helpers/Second.khn",
                        b"second",
                        2,
                    ),
                    (
                        "Mods/Configured/Scripts/thoth/helpers/First.khn",
                        b"duplicate",
                        0,
                    ),
                ],
                7,
            ),
        )
        .unwrap();
        let reader = PackageReader::open(&path).unwrap();
        assert_eq!(reader.header().priority, 7);
        let entries = reader.thoth_entries("Configured").unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(reader.read_entry(entries[0], 32).unwrap(), b"first");
        assert_eq!(reader.read_entry(entries[1], 32).unwrap(), b"second");
        assert_eq!(reader.read_entry(entries[2], 32).unwrap(), b"duplicate");
        assert_eq!(reader.thoth_entries("Other").unwrap().len(), 1);
        assert!(reader.thoth_entries("Configured/Other").is_err());

        let all_entries = reader.all_thoth_entries();
        assert_eq!(all_entries.len(), 4);
        assert_eq!(
            all_entries
                .iter()
                .map(|entry| entry.name())
                .collect::<Vec<_>>(),
            vec![
                "Mods/Configured/Scripts/thoth/helpers/First.khn",
                "Mods/Other/Scripts/thoth/Other.khn",
                "Mods/Configured/Scripts/thoth/helpers/Second.khn",
                "Mods/Configured/Scripts/thoth/helpers/First.khn",
            ]
        );
    }

    #[test]
    fn osiris_goal_entries_require_one_direct_txt_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("goals.pak");
        fs::write(
            &path,
            synthetic_package(
                &[
                    (
                        "Mods/Configured/Story/RawFiles/Goals/Valid.txt",
                        b"valid",
                        0,
                    ),
                    (
                        "Mods/Configured/Story/RawFiles/Goals/nested/Invalid.txt",
                        b"nested",
                        0,
                    ),
                    (
                        "Mods/Configured/Story/RawFiles/Goals/.txt",
                        b"missing name",
                        0,
                    ),
                    (
                        "Mods/Configured/Story/RawFiles/Goals/NoExtension",
                        b"plain",
                        0,
                    ),
                ],
                0,
            ),
        )
        .unwrap();

        let reader = PackageReader::open(&path).unwrap();
        assert_eq!(
            reader
                .osiris_goal_entries("Configured")
                .unwrap()
                .iter()
                .map(|entry| entry.name())
                .collect::<Vec<_>>(),
            vec!["Mods/Configured/Story/RawFiles/Goals/Valid.txt"]
        );
    }

    #[test]
    fn inspects_package_layouts_before_opening_file_lists() {
        let directory = tempfile::tempdir().unwrap();
        let package = synthetic_package(&[], 0);
        let cases = [
            ("supported.pak", package.clone(), PackageLayout::Supported),
            (
                "older.pak",
                {
                    let mut package = package.clone();
                    package[4..8].copy_from_slice(&17_u32.to_le_bytes());
                    package
                },
                PackageLayout::UnsupportedVersion,
            ),
            (
                "solid.pak",
                {
                    let mut package = package.clone();
                    package[20] = SOLID_PACKAGE_FLAG;
                    package
                },
                PackageLayout::Solid,
            ),
            (
                "multipart.pak",
                {
                    let mut package = package;
                    package[38..40].copy_from_slice(&2_u16.to_le_bytes());
                    package
                },
                PackageLayout::Multipart,
            ),
        ];

        for (name, package, expected_layout) in cases {
            let path = directory.path().join(name);
            fs::write(&path, package).unwrap();
            assert_eq!(inspect_package(&path).unwrap().layout(), expected_layout);
        }
    }

    #[test]
    fn all_thoth_entries_rejects_near_matches() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("near-matches.pak");
        fs::write(
            &path,
            synthetic_package(
                &[
                    ("Mods/One/Scripts/thoth/Valid.khn", b"valid", 0),
                    ("Mods/Two/Scripts/thoth", b"directory", 0),
                    ("Mods/Three/Scripts/thoth/NoExtension", b"plain", 0),
                    ("Mods/Four/Scripts/other/NotThoth.khn", b"other", 0),
                    ("Mods/Five/Other/Scripts/thoth/NotAModule.khn", b"nested", 0),
                    ("Public/Scripts/thoth/Outside.khn", b"outside", 0),
                ],
                0,
            ),
        )
        .unwrap();

        let reader = PackageReader::open(&path).unwrap();
        assert_eq!(
            reader
                .all_thoth_entries()
                .iter()
                .map(|entry| entry.name())
                .collect::<Vec<_>>(),
            vec!["Mods/One/Scripts/thoth/Valid.khn"]
        );
    }

    #[test]
    fn rejects_out_of_bounds_entry_before_reading_contents() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("bad.pak");
        let mut package =
            synthetic_package(&[("Mods/Configured/Scripts/thoth/Bad.khn", b"bad", 0)], 0);
        let file_list_offset = u64::from_le_bytes(package[8..16].try_into().unwrap()) as usize;
        let compressed_size = u32::from_le_bytes(
            package[file_list_offset + 4..file_list_offset + 8]
                .try_into()
                .unwrap(),
        ) as usize;
        let decoded = lz4_flex::block::decompress(
            &package[file_list_offset + 8..file_list_offset + 8 + compressed_size],
            PACKAGE_ENTRY_SIZE,
        )
        .unwrap();
        let mut bad_entry = decoded;
        bad_entry[264..268].copy_from_slice(&u32::MAX.to_le_bytes());
        let compressed = lz4_flex::block::compress(&bad_entry);
        assert_eq!(compressed.len(), compressed_size);
        package[file_list_offset + 8..file_list_offset + 8 + compressed_size]
            .copy_from_slice(&compressed);
        fs::write(&path, package).unwrap();
        assert!(PackageReader::open(&path).is_err());
    }

    #[test]
    fn exact_entry_reads_ignore_malformed_unrelated_entries() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("unrelated.pak");
        let package = synthetic_package(
            &[
                ("Localization/English/english.loca", b"target", 0),
                ("Not/Needed.bin", b"unrelated", 0),
            ],
            0,
        );
        let file_list_offset =
            usize::try_from(u64::from_le_bytes(package[8..16].try_into().unwrap())).unwrap();
        let compressed_size = usize::try_from(u32::from_le_bytes(
            package[file_list_offset + 4..file_list_offset + 8]
                .try_into()
                .unwrap(),
        ))
        .unwrap();
        let mut decoded = lz4_flex::block::decompress(
            &package[file_list_offset + 8..file_list_offset + 8 + compressed_size],
            PACKAGE_ENTRY_SIZE * 2,
        )
        .unwrap();
        decoded[PACKAGE_ENTRY_SIZE] = 0xff;
        fs::write(&path, rewrite_file_list(&package, &decoded)).unwrap();

        assert_eq!(
            read_package_entry(&path, "Localization/English/english.loca", 64,).unwrap(),
            b"target"
        );
        assert!(PackageReader::open(&path).is_err());
    }

    #[test]
    fn rejects_nonempty_lz4_entry_with_zero_decoded_size() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("bad-lz4.pak");
        let package = synthetic_package(
            &[("Mods/Configured/Scripts/thoth/Bad.khn", b"stored", 2)],
            0,
        );
        let file_list_offset =
            usize::try_from(u64::from_le_bytes(package[8..16].try_into().unwrap())).unwrap();
        let compressed_size = usize::try_from(u32::from_le_bytes(
            package[file_list_offset + 4..file_list_offset + 8]
                .try_into()
                .unwrap(),
        ))
        .unwrap();
        let mut decoded = lz4_flex::block::decompress(
            &package[file_list_offset + 8..file_list_offset + 8 + compressed_size],
            PACKAGE_ENTRY_SIZE,
        )
        .unwrap();
        decoded[268..272].copy_from_slice(&0_u32.to_le_bytes());
        fs::write(&path, rewrite_file_list(&package, &decoded)).unwrap();
        let reader = PackageReader::open(&path).unwrap();
        let entry = reader.thoth_entries("Configured").unwrap().pop().unwrap();
        assert!(reader.read_entry(entry, 64).is_err());
    }

    #[test]
    fn rejects_entry_payload_overlap_with_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("overlap.pak");
        let package = synthetic_package(&[("Mods/Configured/Scripts/thoth/Bad.khn", b"bad", 0)], 0);
        let file_list_offset =
            usize::try_from(u64::from_le_bytes(package[8..16].try_into().unwrap())).unwrap();
        let compressed_size = usize::try_from(u32::from_le_bytes(
            package[file_list_offset + 4..file_list_offset + 8]
                .try_into()
                .unwrap(),
        ))
        .unwrap();
        let mut decoded = lz4_flex::block::decompress(
            &package[file_list_offset + 8..file_list_offset + 8 + compressed_size],
            PACKAGE_ENTRY_SIZE,
        )
        .unwrap();
        decoded[256..260].copy_from_slice(&0_u32.to_le_bytes());
        fs::write(&path, rewrite_file_list(&package, &decoded)).unwrap();
        assert!(PackageReader::open(&path).is_err());
    }
}

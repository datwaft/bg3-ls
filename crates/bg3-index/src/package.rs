use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::Error;

const PACKAGE_SIGNATURE: &[u8; 4] = b"LSPK";
const PACKAGE_VERSION: u32 = 18;
const PACKAGE_HEADER_SIZE: usize = 40;
const PACKAGE_ENTRY_SIZE: usize = 272;
const MAX_FILE_LIST_SIZE: usize = 64 * 1024 * 1024;
const MAX_FILE_COUNT: usize = 100_000;
const SOLID_PACKAGE_FLAG: u8 = 0x04;

/// Reads one exact entry without loading or decompressing unrelated package data.
pub(crate) fn read_package_entry(
    path: &Path,
    expected_name: &str,
    max_entry_size: usize,
) -> Result<Vec<u8>, Error> {
    let mut package = File::open(path)?;
    let mut header = [0_u8; PACKAGE_HEADER_SIZE];
    package.read_exact(&mut header)?;
    if &header[..4] != PACKAGE_SIGNATURE {
        return Err(Error::Package("package signature is not `LSPK`".into()));
    }
    let version = read_u32(&header, 4, "package version")?;
    if version != PACKAGE_VERSION {
        return Err(Error::Package(format!(
            "package version {version} is unsupported; expected version {PACKAGE_VERSION}"
        )));
    }
    if header[20] & SOLID_PACKAGE_FLAG != 0 {
        return Err(Error::Package("solid packages are unsupported".into()));
    }
    let parts = read_u16(&header, 38, "package part count")?;
    if parts != 1 {
        return Err(Error::Package(format!(
            "multipart packages are unsupported: found {parts} parts"
        )));
    }

    let file_list_offset = read_u64(&header, 8, "file-list offset")?;
    let file_list_size = usize::try_from(read_u32(&header, 16, "file-list size")?)
        .map_err(|_| Error::Package("file-list size does not fit in memory".into()))?;
    if file_list_size > MAX_FILE_LIST_SIZE {
        return Err(Error::Package(format!(
            "file list exceeds the {MAX_FILE_LIST_SIZE} byte limit"
        )));
    }
    package.seek(SeekFrom::Start(file_list_offset))?;
    let mut file_list = vec![0_u8; file_list_size];
    package.read_exact(&mut file_list)?;
    let entry = find_entry(&file_list, expected_name)?
        .ok_or_else(|| Error::Package(format!("package has no entry `{expected_name}`")))?;
    entry.read(&mut package, max_entry_size)
}

#[derive(Clone, Debug)]
struct PackageEntry {
    name: String,
    offset: u64,
    flags: u8,
    size_on_disk: usize,
    uncompressed_size: usize,
}

impl PackageEntry {
    fn read(&self, package: &mut File, max_entry_size: usize) -> Result<Vec<u8>, Error> {
        if self.size_on_disk > max_entry_size || self.uncompressed_size > max_entry_size {
            return Err(Error::Package(format!(
                "package entry `{}` exceeds the {max_entry_size} byte limit",
                self.name
            )));
        }
        package.seek(SeekFrom::Start(self.offset))?;
        let mut stored = vec![0_u8; self.size_on_disk];
        package.read_exact(&mut stored)?;
        match self.flags & 0x0f {
            0 => Ok(stored),
            2 => {
                if self.uncompressed_size == 0 {
                    return Err(Error::Package(format!(
                        "LZ4 entry `{}` has no uncompressed size",
                        self.name
                    )));
                }
                let decoded = lz4_flex::block::decompress(&stored, self.uncompressed_size)
                    .map_err(|error| {
                        Error::Package(format!(
                            "cannot decompress LZ4 entry `{}`: {error}",
                            self.name
                        ))
                    })?;
                if decoded.len() != self.uncompressed_size {
                    return Err(Error::Package(format!(
                        "LZ4 entry `{}` decoded to {} bytes instead of {}",
                        self.name,
                        decoded.len(),
                        self.uncompressed_size
                    )));
                }
                Ok(decoded)
            }
            method => Err(Error::Package(format!(
                "package entry `{}` uses unsupported compression method {method}",
                self.name
            ))),
        }
    }
}

fn find_entry(file_list: &[u8], expected_name: &str) -> Result<Option<PackageEntry>, Error> {
    if file_list.len() < 8 {
        return Err(Error::Package("file list is truncated".into()));
    }
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
    let decoded_size = count
        .checked_mul(PACKAGE_ENTRY_SIZE)
        .ok_or_else(|| Error::Package("decoded file-list size overflowed".into()))?;
    if decoded_size > MAX_FILE_LIST_SIZE {
        return Err(Error::Package(format!(
            "decoded file list exceeds the {MAX_FILE_LIST_SIZE} byte limit"
        )));
    }
    let decoded = lz4_flex::block::decompress(compressed, decoded_size).map_err(|error| {
        Error::Package(format!("cannot decompress the package file list: {error}"))
    })?;
    if decoded.len() != decoded_size {
        return Err(Error::Package(format!(
            "decoded file list has {} bytes instead of {decoded_size}",
            decoded.len()
        )));
    }

    for index in 0..count {
        let start = index * PACKAGE_ENTRY_SIZE;
        let raw = checked_slice(&decoded, start, PACKAGE_ENTRY_SIZE, "package entry")?;
        let name_end = raw[..256].iter().position(|byte| *byte == 0).unwrap_or(256);
        let name = std::str::from_utf8(&raw[..name_end])
            .map_err(|error| Error::Package(format!("package entry name is not UTF-8: {error}")))?;
        if !name.eq_ignore_ascii_case(expected_name) {
            continue;
        }
        let offset_low = u64::from(read_u32(raw, 256, "entry offset")?);
        let offset_high = u64::from(read_u16(raw, 260, "entry offset")?);
        let archive_part = raw[262];
        if archive_part != 0 {
            return Err(Error::Package(format!(
                "entry `{name}` uses unsupported archive part {archive_part}"
            )));
        }
        return Ok(Some(PackageEntry {
            name: name.to_owned(),
            offset: offset_low | (offset_high << 32),
            flags: raw[263],
            size_on_disk: usize::try_from(read_u32(raw, 264, "entry disk size")?)
                .map_err(|_| Error::Package("entry disk size does not fit in memory".into()))?,
            uncompressed_size: usize::try_from(read_u32(raw, 268, "entry uncompressed size")?)
                .map_err(|_| Error::Package("entry size does not fit in memory".into()))?,
        }));
    }
    Ok(None)
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

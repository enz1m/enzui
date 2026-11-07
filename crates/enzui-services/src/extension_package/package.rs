//! Package admission: a strict reader for the classic ZIP subset a package
//! may use.
//!
//! General ZIP readers index entries by name, so a second entry for the
//! same name silently replaces the first, and they accept ZIP64, split
//! archives, encrypted or overlapping entries. Packages are a reviewed
//! artifact, so this reader walks the central directory itself and refuses
//! anything ambiguous: ZIP64, more than one disk, a directory count that
//! disagrees with the entries present, encryption, methods other than
//! stored and deflate, links and special files, duplicate or colliding
//! names, local headers that disagree with the directory, overlapping
//! entry data, a CRC mismatch, and sizes beyond [`PackageLimits`].

use super::manifest::{Host, plain_relative, reserved};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;

/// Bounds on one package.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PackageLimits {
    /// The package file's size.
    pub max_package_bytes: u64,
    /// Files in the package, folders excluded.
    pub max_files: usize,
    /// All files' uncompressed bytes together.
    pub max_extracted_bytes: u64,
    /// One path's length in bytes.
    pub max_name_bytes: usize,
}

impl Default for PackageLimits {
    fn default() -> Self {
        Self {
            max_package_bytes: 64 << 20,
            max_files: 128,
            max_extracted_bytes: 64 << 20,
            max_name_bytes: 240,
        }
    }
}

/// Reads the package at `path` into its files, by package path.
///
/// # Errors
///
/// The file is not a regular file, is too large, or is not an
/// unambiguous classic ZIP within `limits`; the message is shown to the
/// user.
pub fn read(
    path: &Path,
    host: &Host,
    limits: &PackageLimits,
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let bytes = super::read_regular(path, limits.max_package_bytes)?;
    parse(&bytes, host, limits)
}

const EOCD: &[u8; 4] = b"PK\x05\x06";
const ZIP64_LOCATOR: &[u8; 4] = b"PK\x06\x07";
const CENTRAL: &[u8; 4] = b"PK\x01\x02";
const LOCAL: &[u8; 4] = b"PK\x03\x04";

fn u16_at(bytes: &[u8], at: usize) -> Result<u16, String> {
    bytes
        .get(at..at + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or_else(truncated)
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32, String> {
    bytes
        .get(at..at + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(truncated)
}

fn truncated() -> String {
    "The extension package is truncated".into()
}

/// One central-directory record, checked.
struct Record {
    name: String,
    directory: bool,
    method: u16,
    crc: u32,
    compressed: u64,
    size: u64,
    local: u64,
}

pub(super) fn parse(
    bytes: &[u8],
    host: &Host,
    limits: &PackageLimits,
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let end = find_end(bytes)?;
    if end >= 20 && bytes.get(end - 20..end - 16) == Some(ZIP64_LOCATOR) {
        return Err("ZIP64 extension packages are not supported".into());
    }
    let (disk, cd_disk) = (u16_at(bytes, end + 4)?, u16_at(bytes, end + 6)?);
    let (here, total) = (u16_at(bytes, end + 8)?, u16_at(bytes, end + 10)?);
    let cd_size = u64::from(u32_at(bytes, end + 12)?);
    let cd_offset = u64::from(u32_at(bytes, end + 16)?);
    if disk != 0 || cd_disk != 0 || here != total {
        return Err("Extension packages must be a single-disk ZIP".into());
    }
    if total == u16::MAX || cd_size == u64::from(u32::MAX) || cd_offset == u64::from(u32::MAX) {
        return Err("ZIP64 extension packages are not supported".into());
    }
    if cd_offset + cd_size != end as u64 {
        return Err("The extension package directory is inconsistent".into());
    }
    let mut records = Vec::with_capacity(usize::from(total));
    let mut at = usize::try_from(cd_offset).map_err(|_| truncated())?;
    for _ in 0..total {
        let (record, next) = central(bytes, at, limits)?;
        records.push(record);
        at = next;
    }
    if at != end {
        // Entries the stated count does not cover: a reader trusting the
        // count and one walking the directory would see different packages.
        return Err("The extension package directory is inconsistent".into());
    }

    let mut names = BTreeSet::new();
    let mut ranges: Vec<(u64, u64)> = Vec::new();
    let mut files = BTreeMap::new();
    let mut total_bytes = 0u64;
    for record in &records {
        check_name(&record.name, record.directory, host, limits)?;
        let key = record.name.trim_end_matches('/').to_owned();
        if !names.insert(key.clone()) {
            return Err(format!(
                "The extension package contains {key:?} more than once"
            ));
        }
        if record.directory {
            continue;
        }
        if files.len() == limits.max_files {
            return Err("Too many files in the extension package".into());
        }
        let data = local_data(bytes, record, cd_offset)?;
        ranges.push(data);
        total_bytes = total_bytes.saturating_add(record.size);
        if total_bytes > limits.max_extracted_bytes {
            return Err("The extension package exceeds the extracted size limit".into());
        }
        let raw = usize::try_from(data.0)
            .ok()
            .zip(usize::try_from(data.1).ok())
            .and_then(|(start, end)| bytes.get(start..end))
            .ok_or("The extension package has an entry outside the archive")?;
        files.insert(key, inflate(record, raw)?);
    }
    // A name used as a folder by another entry is a collision, not a nest.
    for name in &names {
        let mut parent = Path::new(name).parent();
        while let Some(dir) = parent.filter(|dir| !dir.as_os_str().is_empty()) {
            if files.contains_key(dir.to_str().unwrap_or_default()) {
                return Err(format!(
                    "The extension package uses {:?} as both a file and a folder",
                    dir.display()
                ));
            }
            parent = dir.parent();
        }
    }
    ranges.sort_unstable();
    if ranges.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Err("Extension package entries overlap".into());
    }
    Ok(files)
}

/// The end-of-central-directory record: the last one whose comment runs
/// exactly to the end of the file.
fn find_end(bytes: &[u8]) -> Result<usize, String> {
    let earliest = bytes.len().saturating_sub(22 + usize::from(u16::MAX));
    (earliest..=bytes.len().saturating_sub(22))
        .rev()
        .find(|&at| {
            bytes.get(at..at + 4) == Some(EOCD)
                && u16_at(bytes, at + 20)
                    .is_ok_and(|comment| at + 22 + usize::from(comment) == bytes.len())
        })
        .ok_or_else(|| "Not a valid extension package (ZIP)".to_owned())
}

fn central(bytes: &[u8], at: usize, limits: &PackageLimits) -> Result<(Record, usize), String> {
    if bytes.get(at..at + 4) != Some(CENTRAL) {
        return Err("The extension package directory is damaged".into());
    }
    let made_by = u16_at(bytes, at + 4)?;
    let flags = u16_at(bytes, at + 8)?;
    let method = u16_at(bytes, at + 10)?;
    let crc = u32_at(bytes, at + 16)?;
    let compressed = u32_at(bytes, at + 20)?;
    let size = u32_at(bytes, at + 24)?;
    let name_len = usize::from(u16_at(bytes, at + 28)?);
    let extra_len = usize::from(u16_at(bytes, at + 30)?);
    let comment_len = usize::from(u16_at(bytes, at + 32)?);
    let disk = u16_at(bytes, at + 34)?;
    let external = u32_at(bytes, at + 38)?;
    let local = u32_at(bytes, at + 42)?;
    if flags & 0x41 != 0 {
        return Err("Encrypted extension packages are not supported".into());
    }
    if method != 0 && method != 8 {
        return Err("The extension package uses an unsupported compression method".into());
    }
    if disk != 0 || [compressed, size, local].contains(&u32::MAX) {
        return Err("ZIP64 extension packages are not supported".into());
    }
    if name_len > limits.max_name_bytes {
        return Err("A path in the extension package is too long".into());
    }
    let name = bytes
        .get(at + 46..at + 46 + name_len)
        .ok_or_else(truncated)?;
    let name = std::str::from_utf8(name)
        .map_err(|_| "Extension package paths must be UTF-8".to_owned())?
        .to_owned();
    // Unix-made entries carry their file type; anything but a regular file
    // or folder (a symlink, a device) is refused.
    if made_by >> 8 == 3 {
        let kind = (external >> 16) & 0o170_000;
        if kind != 0 && kind != 0o100_000 && kind != 0o040_000 {
            return Err("Links and special files are not allowed in extension packages".into());
        }
    }
    let directory = name.ends_with('/');
    if directory && size != 0 {
        return Err("The extension package directory is damaged".into());
    }
    Ok((
        Record {
            name,
            directory,
            method,
            crc,
            compressed: u64::from(compressed),
            size: u64::from(size),
            local: u64::from(local),
        },
        at + 46 + name_len + extra_len + comment_len,
    ))
}

fn check_name(
    name: &str,
    directory: bool,
    host: &Host,
    limits: &PackageLimits,
) -> Result<(), String> {
    let trimmed = if directory {
        name.strip_suffix('/').unwrap_or(name)
    } else {
        name
    };
    if trimmed.is_empty()
        || trimmed.len() > limits.max_name_bytes
        || trimmed.contains('\\')
        || trimmed.chars().any(char::is_control)
        || trimmed
            .split('/')
            .any(|part| part.is_empty() || part.starts_with('.'))
        || !plain_relative(Path::new(trimmed))
        || reserved(trimmed, host)
    {
        return Err(format!(
            "Unsafe or reserved path in the extension package: {trimmed:?}"
        ));
    }
    Ok(())
}

/// The byte range of an entry's data, checked against its local header.
fn local_data(bytes: &[u8], record: &Record, cd_offset: u64) -> Result<(u64, u64), String> {
    let at = usize::try_from(record.local).map_err(|_| truncated())?;
    if bytes.get(at..at + 4) != Some(LOCAL) {
        return Err("The extension package is damaged".into());
    }
    let flags = u16_at(bytes, at + 6)?;
    let method = u16_at(bytes, at + 8)?;
    let name_len = usize::from(u16_at(bytes, at + 26)?);
    let extra_len = usize::from(u16_at(bytes, at + 28)?);
    let name = bytes
        .get(at + 30..at + 30 + name_len)
        .ok_or_else(truncated)?;
    if name != record.name.as_bytes() || method != record.method || flags & 0x41 != 0 {
        return Err("The extension package's entries disagree with its directory".into());
    }
    let start = (at + 30 + name_len + extra_len) as u64;
    let end = start + record.compressed;
    if end > cd_offset {
        return Err(truncated());
    }
    Ok((start, end))
}

fn inflate(record: &Record, raw: &[u8]) -> Result<Vec<u8>, String> {
    let size = usize::try_from(record.size).map_err(|_| truncated())?;
    let data = if record.method == 0 {
        if record.compressed != record.size {
            return Err("The extension package is damaged".into());
        }
        raw.to_vec()
    } else {
        let mut data = Vec::with_capacity(size.min(1 << 20));
        flate2::read::DeflateDecoder::new(raw)
            .take(record.size + 1)
            .read_to_end(&mut data)
            .map_err(|_| "The extension package is damaged".to_owned())?;
        data
    };
    if data.len() != size || crc32fast::hash(&data) != record.crc {
        return Err(format!(
            "{:?} in the extension package is damaged",
            record.name
        ));
    }
    Ok(data)
}

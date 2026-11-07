//! The Git index (`.git/index`), versions 2, 3 and 4: the tracked paths and
//! the stat data recorded when each was last staged. Extension blocks, trailing
//! checksums, path containment, and padding bytes are not validated.

use super::ReadError;
use std::path::Path;

const MAX_INDEX: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Entry {
    pub(super) path: Vec<u8>,
    pub(super) mode: u32,
    pub(super) size: u32,
    pub(super) mtime: (u32, u32),
    pub(super) id: Vec<u8>,
    pub(super) stage: u8,
    pub(super) skip_worktree: bool,
    pub(super) intent_to_add: bool,
}

impl Entry {
    pub(super) fn is_gitlink(&self) -> bool {
        self.mode & 0o170000 == 0o160000
    }

    pub(super) fn is_symlink(&self) -> bool {
        self.mode & 0o170000 == 0o120000
    }

    pub(super) fn is_executable(&self) -> bool {
        self.mode & 0o111 != 0
    }
}

pub(super) fn read(index: &Path, id_len: usize) -> Result<Option<Vec<Entry>>, ReadError> {
    let bytes = match super::read_regular(index, MAX_INDEX, false) {
        Ok(bytes) => bytes,
        Err(super::Refusal::Missing) => return Ok(None),
        Err(super::Refusal::Refused) => return Err(ReadError::Io("the index".into())),
    };
    parse(&bytes, id_len).map(Some)
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], ReadError> {
        let end = self.at.checked_add(n).ok_or(ReadError::Format("index"))?;
        let slice = self
            .bytes
            .get(self.at..end)
            .ok_or(ReadError::Format("index"))?;
        self.at = end;
        Ok(slice)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], ReadError> {
        self.take(N)?
            .try_into()
            .map_err(|_| ReadError::Format("index"))
    }

    fn u32(&mut self) -> Result<u32, ReadError> {
        self.array().map(u32::from_be_bytes)
    }

    fn u16(&mut self) -> Result<u16, ReadError> {
        self.array().map(u16::from_be_bytes)
    }

    // Index v4 encodes prefix stripping with an offset varint.
    fn varint(&mut self) -> Result<usize, ReadError> {
        let mut byte = self.take(1)?[0];
        let mut value = usize::from(byte & 0x7f);
        while byte & 0x80 != 0 {
            byte = self.take(1)?[0];
            value = value
                .checked_add(1)
                .and_then(|v| v.checked_mul(128))
                .and_then(|v| v.checked_add(usize::from(byte & 0x7f)))
                .ok_or(ReadError::Format("index path length"))?;
        }
        Ok(value)
    }
}

pub(super) fn parse(bytes: &[u8], id_len: usize) -> Result<Vec<Entry>, ReadError> {
    let mut cursor = Cursor { bytes, at: 0 };
    if cursor.take(4)? != b"DIRC" {
        return Err(ReadError::Format("index signature"));
    }
    let version = cursor.u32()?;
    if !(2..=4).contains(&version) {
        return Err(ReadError::Format("index version"));
    }
    let count = cursor.u32()? as usize;
    // Every entry takes at least 62 bytes; a count the file cannot hold is
    // corruption, not a reason to allocate.
    if count > bytes.len() / 62 {
        return Err(ReadError::Format("index entry count"));
    }
    let mut entries = Vec::with_capacity(count);
    let mut previous: Vec<u8> = Vec::new();
    for _ in 0..count {
        let start = cursor.at;
        let _ctime = (cursor.u32()?, cursor.u32()?);
        let mtime = (cursor.u32()?, cursor.u32()?);
        let _dev = cursor.u32()?;
        let _ino = cursor.u32()?;
        let mode = cursor.u32()?;
        let _uid = cursor.u32()?;
        let _gid = cursor.u32()?;
        let size = cursor.u32()?;
        let id = cursor.take(id_len)?.to_vec();
        let flags = cursor.u16()?;
        let stage = ((flags >> 12) & 0b11) as u8;
        let extended = flags & 0x4000 != 0;
        let (mut skip_worktree, mut intent_to_add) = (false, false);
        if extended {
            if version < 3 {
                return Err(ReadError::Format("index extended flags"));
            }
            let more = cursor.u16()?;
            skip_worktree = more & 0x4000 != 0;
            intent_to_add = more & 0x2000 != 0;
        }
        let path = if version == 4 {
            let strip = cursor.varint()?;
            if strip > previous.len() {
                return Err(ReadError::Format("index path prefix"));
            }
            let mut path = previous[..previous.len() - strip].to_vec();
            let suffix_end = bytes[cursor.at..]
                .iter()
                .position(|byte| *byte == 0)
                .ok_or(ReadError::Format("index path"))?;
            path.extend_from_slice(cursor.take(suffix_end)?);
            cursor.take(1)?;
            path
        } else {
            let name_len = usize::from(flags & 0x0fff);
            let path = if name_len < 0x0fff {
                cursor.take(name_len)?.to_vec()
            } else {
                // A name too long for the flags: NUL-terminated instead.
                let end = bytes[cursor.at..]
                    .iter()
                    .position(|byte| *byte == 0)
                    .ok_or(ReadError::Format("index path"))?;
                cursor.take(end)?.to_vec()
            };
            // NUL padding to a multiple of eight bytes from the entry start.
            let used = cursor.at - start;
            let padded = (used + 8) & !7;
            cursor.take(padded - used)?;
            path
        };
        previous.clone_from(&path);
        entries.push(Entry {
            path,
            mode,
            size,
            mtime,
            id,
            stage,
            skip_worktree,
            intent_to_add,
        });
    }
    Ok(entries)
}

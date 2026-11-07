//! Split-archive naming: which files make up one archive, in order.

use super::{ArchiveError, ArchiveFormat};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// How one archive's parts are named.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Scheme {
    /// One file.
    Single(Vec<u8>),
    /// `name.7z.001`, `name.7z.002`, ...: `prefix` ends with the dot.
    Numbered { prefix: Vec<u8>, width: usize },
    /// `name.part1.rar`, `name.part2.rar`, ...: `stem` ends with `part`.
    RarParts {
        stem: Vec<u8>,
        width: usize,
        extension: Vec<u8>,
    },
    /// `name.rar`, `name.r00`, `name.r01`, ...: `stem` is `name`.
    RarOld { stem: Vec<u8> },
}

/// The parts of one archive and the name its format is detected from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct VolumeSet {
    /// The archive's name without any part number: `name.7z`, `name.rar`.
    pub base_name: Vec<u8>,
    scheme: Scheme,
}

/// Reads the part scheme from one part's file name.
pub(super) fn parse(name: &[u8]) -> VolumeSet {
    let lower = name.to_ascii_lowercase();
    // `name.part<N>.rar`
    if lower.ends_with(b".rar")
        && let Some(at) = find_last(&lower[..lower.len() - 4], b".part")
    {
        let digits = &name[at + 5..name.len() - 4];
        if !digits.is_empty() && digits.iter().all(u8::is_ascii_digit) {
            let mut base_name = name[..at].to_vec();
            base_name.extend_from_slice(&name[name.len() - 4..]);
            return VolumeSet {
                base_name,
                scheme: Scheme::RarParts {
                    stem: name[..at + 5].to_vec(),
                    width: digits.len(),
                    extension: name[name.len() - 4..].to_vec(),
                },
            };
        }
    }
    // `name.r<NN>`
    if lower.len() > 4
        && lower[lower.len() - 4..lower.len() - 2] == *b".r"
        && lower[lower.len() - 2..].iter().all(u8::is_ascii_digit)
    {
        let stem = name[..name.len() - 4].to_vec();
        let mut base_name = stem.clone();
        base_name.extend_from_slice(b".rar");
        return VolumeSet {
            base_name,
            scheme: Scheme::RarOld { stem },
        };
    }
    // `name.<format>.<NNN>`
    if let Some(dot) = name.iter().rposition(|&byte| byte == b'.') {
        let digits = &name[dot + 1..];
        if digits.len() >= 3
            && digits.iter().all(u8::is_ascii_digit)
            && ArchiveFormat::from_suffix(&name[..dot]).is_some()
        {
            return VolumeSet {
                base_name: name[..dot].to_vec(),
                scheme: Scheme::Numbered {
                    prefix: name[..=dot].to_vec(),
                    width: digits.len(),
                },
            };
        }
    }
    VolumeSet {
        base_name: name.to_vec(),
        scheme: Scheme::Single(name.to_vec()),
    }
}

fn find_last(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .rposition(|window| window == needle)
}

impl VolumeSet {
    /// Whether the archive spans several files.
    fn is_split(&self) -> bool {
        !matches!(self.scheme, Scheme::Single(_))
    }

    /// The name of part `index`, counting from zero.
    fn part(&self, dir: &Path, index: usize) -> PathBuf {
        let name = match &self.scheme {
            Scheme::Single(name) => name.clone(),
            Scheme::Numbered { prefix, width } => {
                let mut name = prefix.clone();
                name.extend_from_slice(format!("{:0width$}", index + 1).as_bytes());
                name
            }
            Scheme::RarParts {
                stem,
                width,
                extension,
            } => {
                let mut name = stem.clone();
                name.extend_from_slice(format!("{:0width$}", index + 1).as_bytes());
                name.extend_from_slice(extension);
                name
            }
            Scheme::RarOld { stem } => {
                let mut name = stem.clone();
                if index == 0 {
                    name.extend_from_slice(b".rar");
                } else {
                    name.extend_from_slice(format!(".r{:02}", index - 1).as_bytes());
                }
                name
            }
        };
        dir.join(OsStr::from_bytes(&name))
    }

    /// Every present part in order, from the first. A lone `name.rar` with
    /// a `name.r00` beside it becomes an old-style RAR set.
    pub(super) fn resolve(&mut self, dir: &Path) -> Result<Vec<PathBuf>, ArchiveError> {
        if let Scheme::Single(name) = &self.scheme
            && name.len() > 4
            && name[name.len() - 4..].eq_ignore_ascii_case(b".rar")
        {
            let stem = name[..name.len() - 4].to_vec();
            let mut first = stem.clone();
            first.extend_from_slice(b".r00");
            if dir.join(OsStr::from_bytes(&first)).is_file() {
                self.scheme = Scheme::RarOld { stem };
            }
        }
        let first = self.part(dir, 0);
        if !first.exists() {
            return Err(if self.is_split() {
                ArchiveError::MissingVolume(first)
            } else {
                ArchiveError::Io(std::sync::Arc::new(std::io::Error::from(
                    std::io::ErrorKind::NotFound,
                )))
            });
        }
        let mut parts = vec![first];
        if self.is_split() {
            while parts.len() < 10_000 {
                let next = self.part(dir, parts.len());
                if !next.exists() {
                    break;
                }
                parts.push(next);
            }
        }
        Ok(parts)
    }

    /// Whether the parts are byte splits of one file, read as one stream,
    /// rather than RAR volumes with headers of their own.
    pub(super) fn is_joined(&self) -> bool {
        matches!(self.scheme, Scheme::Numbered { .. })
    }

    /// Whether any part after the absent `missing` exists: a gap.
    pub(super) fn has_part_after(&self, dir: &Path, missing: &Path) -> bool {
        let Some(index) = (0..10_000).find(|&index| self.part(dir, index) == missing) else {
            return false;
        };
        (index + 1..index + 64).any(|index| self.part(dir, index).exists())
    }

    /// The first absent part of a split set, or `None` for a single file.
    pub(super) fn next_missing(&self, dir: &Path) -> Option<PathBuf> {
        if !self.is_split() {
            return None;
        }
        (0..10_000)
            .map(|index| self.part(dir, index))
            .find(|part| !part.exists())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(set: &VolumeSet, count: usize) -> Vec<String> {
        (0..count)
            .map(|index| {
                set.part(Path::new(""), index)
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    #[test]
    fn part_names_follow_each_scheme_from_any_part() {
        let numbered = parse(b"Backup.7z.003");
        assert_eq!(numbered.base_name, b"Backup.7z");
        assert_eq!(names(&numbered, 2), ["Backup.7z.001", "Backup.7z.002"]);

        let parts = parse(b"Movie.Part07.RAR");
        assert_eq!(parts.base_name, b"Movie.RAR");
        assert_eq!(names(&parts, 2), ["Movie.Part01.RAR", "Movie.Part02.RAR"]);

        let old = parse(b"docs.r04");
        assert_eq!(old.base_name, b"docs.rar");
        assert_eq!(names(&old, 3), ["docs.rar", "docs.r00", "docs.r01"]);

        // Not part numbers: a version suffix, a name with digits, a lone rar.
        for name in [&b"notes.txt.001"[..], b"release.2024", b"photos.rar"] {
            assert_eq!(parse(name).scheme, Scheme::Single(name.to_vec()));
        }
    }
}

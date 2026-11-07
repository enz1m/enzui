//! The callers the installed OS declares, read from root-owned descriptors
//! in [`CALLERS_DIRECTORY`] (lifecycle caller descriptor, schema 1).
//!
//! Each `<bus_name>.json` names one service that may stop applications: its
//! executable, bus name, user unit and, optionally, the system witness that
//! vouches for it inside a user namespace. The directory is read again on
//! every caller check, so installing or removing a package takes effect on
//! the next call. A missing directory, too many entries or no valid
//! descriptor admits nobody; an invalid descriptor admits nobody and leaves
//! the others alone.

use super::admission::{CallerPolicy, OwnerView, WitnessCall};
use serde::Deserialize;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path};

/// Where the OS installs lifecycle caller descriptors.
pub const CALLERS_DIRECTORY: &str = "/usr/share/enzui/lifecycle-callers";

/// More `.json` entries than this is a tampered directory: nobody is admitted.
const MAX_ENTRIES: usize = 32;
/// The largest descriptor, in bytes.
const MAX_BYTES: u64 = 4096;
const MAX_NAME: usize = 255;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Descriptor {
    schema: u64,
    executable: String,
    bus_name: String,
    unit: String,
    witness: Option<Witness>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Witness {
    bus: String,
    path: String,
    interface: String,
    method: String,
    schema: u64,
}

/// The valid descriptors in `directory`, in file-name byte order, or `None`
/// when the directory itself admits nobody.
pub(crate) fn installed(view: OwnerView, directory: &Path) -> Option<Vec<CallerPolicy>> {
    if !protected_directory(view, directory) {
        return None;
    }
    let mut names: Vec<std::ffi::OsString> = std::fs::read_dir(directory)
        .ok()?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<Result<_, _>>()
        .ok()?;
    names.retain(|name| name.as_encoded_bytes().ends_with(b".json"));
    if names.len() > MAX_ENTRIES {
        return None;
    }
    names.sort_by(|left, right| left.as_encoded_bytes().cmp(right.as_encoded_bytes()));
    Some(
        names
            .iter()
            .filter_map(|name| {
                let bytes = read_descriptor(view, &directory.join(name))?;
                parse(name.to_str()?, &bytes)
            })
            .collect(),
    )
}

/// A real directory, as is every ancestor, each protected-owned and not
/// group or world writable.
fn protected_directory(view: OwnerView, directory: &Path) -> bool {
    directory.is_absolute()
        && directory.ancestors().all(|ancestor| {
            std::fs::symlink_metadata(ancestor)
                .is_ok_and(|meta| meta.is_dir() && view.protects(&meta))
        })
}

/// The descriptor's bytes, checked on the open descriptor: a protected-owned
/// regular file with one link and 1 to [`MAX_BYTES`] bytes.
fn read_descriptor(view: OwnerView, path: &Path) -> Option<Vec<u8>> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file()
        || !view.protects(&meta)
        || meta.nlink() != 1
        || !(1..=MAX_BYTES).contains(&meta.len())
    {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes).ok()?;
    (!bytes.is_empty() && bytes.len() as u64 <= MAX_BYTES).then_some(bytes)
}

/// One descriptor named `file_name`, when it is valid schema 1.
pub(crate) fn parse(file_name: &str, bytes: &[u8]) -> Option<CallerPolicy> {
    let descriptor: Descriptor = serde_json::from_slice(bytes).ok()?;
    let witness = match descriptor.witness {
        None => None,
        Some(witness) => {
            let valid = witness.schema == 1
                && bus_name(&witness.bus)
                && object_path(&witness.path)
                && bus_name(&witness.interface)
                && !witness.interface.contains('-')
                && member(&witness.method);
            if !valid {
                return None;
            }
            Some(WitnessCall {
                bus: witness.bus,
                path: witness.path,
                interface: witness.interface,
                method: witness.method,
            })
        }
    };
    let valid = descriptor.schema == 1
        && executable(&descriptor.executable)
        && bus_name(&descriptor.bus_name)
        && file_name.strip_suffix(".json") == Some(descriptor.bus_name.as_str())
        && unit(&descriptor.unit);
    valid.then(|| CallerPolicy {
        executable: descriptor.executable.into(),
        bus_name: descriptor.bus_name,
        unit: descriptor.unit,
        witness,
    })
}

fn executable(path: &str) -> bool {
    path.len() <= 4096
        && path.starts_with('/')
        && !path.ends_with('/')
        && !path.contains("//")
        && Path::new(path)
            .components()
            .skip(1)
            .all(|part| matches!(part, Component::Normal(_)))
        && !path
            .split('/')
            .skip(1)
            .any(|part| part == "." || part == "..")
}

/// A well-known D-Bus name: two or more non-empty `[A-Za-z0-9_-]` elements,
/// none starting with a digit.
fn bus_name(name: &str) -> bool {
    name.len() <= MAX_NAME
        && !name.starts_with(':')
        && name.split('.').count() >= 2
        && name.split('.').all(|element| {
            element
                .bytes()
                .next()
                .is_some_and(|first| !first.is_ascii_digit())
                && element
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        })
}

fn object_path(path: &str) -> bool {
    path == "/"
        || path.strip_prefix('/').is_some_and(|rest| {
            rest.split('/').all(|element| {
                !element.is_empty()
                    && element
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            })
        })
}

fn member(name: &str) -> bool {
    name.len() <= MAX_NAME
        && name
            .bytes()
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn unit(name: &str) -> bool {
    name.len() <= MAX_NAME
        && name.ends_with(".service")
        && !name.ends_with("@.service")
        && name.len() > ".service".len()
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'_' | b'.' | b'@' | b'-')
        })
}

#[cfg(test)]
#[path = "callers_tests.rs"]
mod tests;

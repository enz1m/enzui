#![allow(
    clippy::cast_possible_truncation,
    reason = "test fixtures write known small lengths"
)]

use super::*;
use std::collections::BTreeMap;
use std::io::Write;

const HOST: Host = Host {
    api: 1..=1,
    permissions: &["clipboard"],
    builtins: &["echo"],
    reserved_prefixes: &["org.enzui."],
    reserved_names: &["index.json"],
};

/// One crafted ZIP entry.
struct Entry {
    name: Vec<u8>,
    data: Vec<u8>,
    deflate: bool,
    unix_mode: u32,
    flags: u16,
    method: Option<u16>,
    crc: Option<u32>,
}

fn file(name: &str, data: &[u8]) -> Entry {
    Entry {
        name: name.as_bytes().to_vec(),
        data: data.to_vec(),
        deflate: true,
        unix_mode: 0o100_644,
        flags: 0,
        method: None,
        crc: None,
    }
}

/// Builds a ZIP; `tweak` edits the end-of-directory record and the
/// directory before they are written.
fn zip(entries: &[Entry], tweak: impl FnOnce(&mut Vec<u8>, &mut Vec<u8>)) -> Vec<u8> {
    let mut out = Vec::new();
    let mut directory = Vec::new();
    for entry in entries {
        let stored = if entry.deflate {
            let mut encoder =
                flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(&entry.data).unwrap();
            encoder.finish().unwrap()
        } else {
            entry.data.clone()
        };
        let method = entry.method.unwrap_or(if entry.deflate { 8 } else { 0 });
        let crc = entry.crc.unwrap_or_else(|| crc32fast::hash(&entry.data));
        let offset = out.len() as u32;
        let header = |signature: &[u8], central: bool| {
            let mut h = signature.to_vec();
            if central {
                h.extend_from_slice(&(3u16 << 8 | 20).to_le_bytes());
            }
            h.extend_from_slice(&20u16.to_le_bytes());
            h.extend_from_slice(&entry.flags.to_le_bytes());
            h.extend_from_slice(&method.to_le_bytes());
            h.extend_from_slice(&[0; 4]);
            h.extend_from_slice(&crc.to_le_bytes());
            h.extend_from_slice(&(stored.len() as u32).to_le_bytes());
            h.extend_from_slice(&(entry.data.len() as u32).to_le_bytes());
            h.extend_from_slice(&(entry.name.len() as u16).to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes());
            if central {
                h.extend_from_slice(&[0; 6]);
                h.extend_from_slice(&(entry.unix_mode << 16).to_le_bytes());
                h.extend_from_slice(&offset.to_le_bytes());
            }
            h.extend_from_slice(&entry.name);
            h
        };
        out.extend(header(b"PK\x03\x04", false));
        out.extend(&stored);
        directory.extend(header(b"PK\x01\x02", true));
    }
    let mut end = b"PK\x05\x06".to_vec();
    end.extend_from_slice(&[0; 4]);
    end.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    end.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    end.extend_from_slice(&(directory.len() as u32).to_le_bytes());
    end.extend_from_slice(&(out.len() as u32).to_le_bytes());
    end.extend_from_slice(&0u16.to_le_bytes());
    tweak(&mut end, &mut directory);
    out.extend(directory);
    out.extend(end);
    out
}

fn admit(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, String> {
    package::parse(bytes, &HOST, &PackageLimits::default())
}

#[test]
fn ambiguous_or_hostile_packages_are_refused() {
    let base = || file("manifest.json", b"{}");
    let mut link = file("link", b"/etc/passwd");
    link.unix_mode = 0o120_777;
    let mut encrypted = file("secret", b"x");
    encrypted.flags = 1;
    let mut unknown_method = file("odd", b"x");
    unknown_method.method = Some(12);
    let mut bad_crc = file("bad", b"payload");
    bad_crc.crc = Some(1);
    let cases: Vec<(&str, Vec<u8>)> = vec![
        (
            "duplicate",
            zip(&[base(), file("a", b"1"), file("a", b"2")], |_, _| {}),
        ),
        (
            "file and folder",
            zip(&[base(), file("a", b"1"), file("a/b", b"2")], |_, _| {}),
        ),
        ("symlink", zip(&[base(), link], |_, _| {})),
        ("encrypted", zip(&[base(), encrypted], |_, _| {})),
        ("method", zip(&[base(), unknown_method], |_, _| {})),
        ("crc", zip(&[base(), bad_crc], |_, _| {})),
        ("parent", zip(&[base(), file("../x", b"1")], |_, _| {})),
        ("absolute", zip(&[base(), file("/x", b"1")], |_, _| {})),
        ("backslash", zip(&[base(), file("a\\b", b"1")], |_, _| {})),
        (
            "hidden",
            zip(&[base(), file(".git/config", b"1")], |_, _| {}),
        ),
        (
            "state",
            zip(&[base(), file("state.json", b"{}")], |_, _| {}),
        ),
        (
            "host reserved",
            zip(&[base(), file("index.json", b"{}")], |_, _| {}),
        ),
        (
            "count disagrees",
            zip(&[base(), file("a", b"1")], |end, _| {
                end[8..10].copy_from_slice(&1u16.to_le_bytes());
                end[10..12].copy_from_slice(&1u16.to_le_bytes());
            }),
        ),
        (
            "multi-disk",
            zip(&[base()], |end, _| {
                end[4..6].copy_from_slice(&1u16.to_le_bytes());
            }),
        ),
        (
            "zip64",
            zip(&[base()], |end, directory| {
                directory.extend_from_slice(b"PK\x06\x07");
                directory.extend_from_slice(&[0; 16]);
                let size = directory.len() as u32;
                end[12..16].copy_from_slice(&size.to_le_bytes());
            }),
        ),
        ("not a zip", b"hello".to_vec()),
    ];
    for (case, bytes) in cases {
        assert!(admit(&bytes).is_err(), "{case} was admitted");
    }
}

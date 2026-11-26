//! Packs bundled SVGs and generates the index and enzUI icon metadata.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::missing_assert_message,
    reason = "a build script fails the build by panicking"
)]

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let fallback_path = manifest_dir.join("assets/fallback.svg");
    let icon_manifest_path = manifest_dir.join("icon-manifest.toml");

    println!("cargo:rerun-if-changed={}", icon_manifest_path.display());
    println!("cargo:rerun-if-changed={}", fallback_path.display());

    let manifest = fs::read_to_string(&icon_manifest_path).unwrap();
    let lucide = required_assets(&manifest, "icons");
    let enzui = required_assets(&manifest, "enzui");

    let mut packed = Vec::new();
    let mut index = Vec::with_capacity(lucide.len() + enzui.len() + 1);
    append_asset("fallback", &fallback_path, &mut packed, &mut index);
    for (directory, entries) in [("lucide", &lucide), ("enzui", &enzui)] {
        for &(name, file) in entries {
            let path = manifest_dir.join("assets").join(directory).join(file);
            println!("cargo:rerun-if-changed={}", path.display());
            append_asset(name, &path, &mut packed, &mut index);
        }
    }
    index.sort_by_key(|entry| entry.0.clone());

    fs::write(out_dir.join("icons.pack"), packed).unwrap();

    let mut generated = String::from(
        "static ICON_BYTES: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/icons.pack\"));\n\
         static ICON_INDEX: &[(&str, usize, usize)] = &[\n",
    );
    for (name, start, end) in index {
        let _ = writeln!(generated, "    ({name:?}, {start}, {end}),");
    }
    generated.push_str(
        "];\n\
         pub(super) fn icon_data(name: &str) -> Option<&'static [u8]> {\n\
             let index = ICON_INDEX.binary_search_by_key(&name, |entry| entry.0).ok()?;\n\
             let (_, start, end) = ICON_INDEX[index];\n\
             Some(&ICON_BYTES[start..end])\n\
         }\n",
    );
    generated.push_str("pub(super) const ENZUI_ICONS: &[super::BundledIcon] = &[\n");
    for (name, file) in enzui {
        let path = format!("assets/enzui/{file}");
        let _ = writeln!(
            generated,
            "    super::BundledIcon {{ name: {name:?}, asset_path: {path:?}, upstream_name: \"\", license: \"MIT\" }},",
        );
    }
    generated.push_str("];\n");
    fs::write(out_dir.join("embedded_icons.rs"), generated).unwrap();
}

fn required_assets<'a>(manifest: &'a str, section: &str) -> Vec<(&'a str, &'a str)> {
    let header = format!("[{section}]");
    let mut lines = manifest.lines().map(str::trim);
    assert!(
        lines.any(|line| line == header),
        "missing [{section}] in icon-manifest.toml"
    );
    lines
        .take_while(|line| !line.starts_with('['))
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let (name, file) = line
                .split_once('=')
                .unwrap_or_else(|| panic!("invalid icon manifest entry: {line}"));
            (name.trim().trim_matches('"'), file.trim().trim_matches('"'))
        })
        .collect()
}

fn append_asset(
    name: &str,
    path: &Path,
    packed: &mut Vec<u8>,
    index: &mut Vec<(String, usize, usize)>,
) {
    let start = packed.len();
    packed.extend(fs::read(path).unwrap());
    index.push((name.to_owned(), start, packed.len()));
}

//! Packs the bundled Lucide SVGs into one blob and generates the name table
//! that indexes it.

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
    let lucide_dir = manifest_dir.join("assets/lucide");
    let fallback_path = manifest_dir.join("assets/fallback.svg");
    let icon_manifest_path = manifest_dir.join("icon-manifest.toml");

    println!("cargo:rerun-if-changed={}", icon_manifest_path.display());
    println!("cargo:rerun-if-changed={}", fallback_path.display());

    let manifest = fs::read_to_string(&icon_manifest_path).unwrap();
    let assets = required_assets(&manifest, &lucide_dir);

    let mut packed = Vec::new();
    let mut index = Vec::with_capacity(assets.len() + 1);
    append_asset("fallback", &fallback_path, &mut packed, &mut index);
    for path in assets {
        println!("cargo:rerun-if-changed={}", path.display());
        let name = path.file_stem().unwrap().to_str().unwrap();
        append_asset(name, &path, &mut packed, &mut index);
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
    fs::write(out_dir.join("embedded_icons.rs"), generated).unwrap();
}

fn required_assets(manifest: &str, lucide_dir: &Path) -> Vec<PathBuf> {
    let icons = manifest
        .split_once("[icons]")
        .and_then(|(_, rest)| rest.split_once("[aliases]"))
        .map(|(icons, _)| icons)
        .expect("icon-manifest.toml must contain [icons] followed by [aliases]");

    icons
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let (_, file) = line
                .split_once('=')
                .unwrap_or_else(|| panic!("invalid icon manifest entry: {line}"));
            lucide_dir.join(file.trim().trim_matches('"'))
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

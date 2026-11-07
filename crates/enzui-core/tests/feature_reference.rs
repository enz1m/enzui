//! `docs/features.md` lists every feature of every enzUI crate, under the
//! crate's own heading, and nothing that does not exist.

use std::collections::BTreeSet;

#[test]
fn every_crate_feature_is_documented() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let page = std::fs::read_to_string(root.join("docs/features.md")).expect("docs/features.md");
    let mut missing = Vec::new();
    let mut stale = Vec::new();
    for entry in std::fs::read_dir(root.join("crates"))
        .expect("crates")
        .flatten()
    {
        let Ok(manifest) = std::fs::read_to_string(entry.path().join("Cargo.toml")) else {
            continue;
        };
        let name = manifest
            .lines()
            .find_map(|line| line.strip_prefix("name = \""))
            .and_then(|rest| rest.strip_suffix('"'))
            .expect("package name")
            .to_owned();
        let mut declared = BTreeSet::new();
        let mut in_features = false;
        for line in manifest.lines() {
            if line.starts_with('[') {
                in_features = line == "[features]";
                continue;
            }
            if in_features
                && let Some((feature, _)) = line.split_once('=')
                && !line.starts_with(' ')
                && !line.starts_with('#')
            {
                let feature = feature.trim();
                if feature != "default" {
                    declared.insert(feature.to_owned());
                }
            }
        }
        // The crate's section runs from its heading to the next one.
        let heading = format!("## {name}\n");
        let section = page.split_once(&heading).map(|(_, rest)| {
            rest.split_once("\n## ")
                .map_or(rest, |(section, _)| section)
        });
        let documented: BTreeSet<String> = section
            .map(|section| {
                section
                    .lines()
                    .filter_map(|line| line.strip_prefix("| `"))
                    .filter_map(|rest| rest.split_once('`').map(|(feature, _)| feature.to_owned()))
                    .collect()
            })
            .unwrap_or_default();
        for feature in declared.difference(&documented) {
            missing.push(format!("{name}/{feature}"));
        }
        for feature in documented.difference(&declared) {
            stale.push(format!("{name}/{feature}"));
        }
    }
    assert!(
        missing.is_empty() && stale.is_empty(),
        "docs/features.md is out of date.\nundocumented: {missing:?}\nno longer exist: {stale:?}"
    );
}

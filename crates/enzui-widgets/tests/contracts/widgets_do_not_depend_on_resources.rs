//! `enzui-widgets` must not depend on `enzui-resources`.

use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

const FORBIDDEN: &str = "enzui-resources";

/// The directory holding all workspace crates.
fn crates_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR is `<root>/crates/enzui-widgets`.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("widgets manifest dir has a parent")
        .to_path_buf()
}

fn manifest_of(crate_name: &str) -> String {
    // Verified for all 10 workspace crates: directory name == package name.
    let path = crates_dir().join(crate_name).join("Cargo.toml");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// One declared dependency: which table it came from, and the crate name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Dep {
    table: String,
    name: String,
}

/// Every dependency declared in `manifest`, from every dependency table.
fn declared_dependencies(manifest: &str) -> Vec<Dep> {
    let mut out = Vec::new();
    let mut table: Option<String> = None;

    for line in manifest.lines() {
        let trimmed = line.trim();

        if let Some(header) = trimmed.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
            // Matches `dependencies`, `dev-dependencies`, `build-dependencies`
            // and `target.'cfg(...)'.dependencies` alike.
            table = header
                .rsplit('.')
                .next()
                .filter(|last| last.ends_with("dependencies"))
                .map(|_| header.to_owned());
            continue;
        }

        let Some(current) = table.as_deref() else {
            continue;
        };

        let code = trimmed.split('#').next().unwrap_or("").trim();
        if code.is_empty() {
            continue;
        }
        let Some((lhs, _)) = code.split_once('=') else {
            // A continuation line of a multi-line value, e.g. a features list.
            continue;
        };
        let name = lhs.trim().split('.').next().unwrap_or("").trim();
        if name.is_empty() || name.starts_with('"') {
            continue;
        }
        out.push(Dep {
            table: current.to_owned(),
            name: name.to_owned(),
        });
    }

    out
}

/// Names only, for the crate's own manifest.
fn dependency_names(crate_name: &str) -> BTreeSet<String> {
    declared_dependencies(&manifest_of(crate_name))
        .into_iter()
        .map(|dep| dep.name)
        .collect()
}

/// The direct edge: widgets must not name resources in any dependency table.
#[test]
fn widgets_does_not_declare_a_dependency_on_resources() {
    let deps = declared_dependencies(&manifest_of("enzui-widgets"));

    if let Some(found) = deps.iter().find(|dep| dep.name == FORBIDDEN) {
        panic!(
            "`enzui-widgets` declares `{FORBIDDEN}` in [{}].\n\n\
             The dependency arrow must point from the application DOWN into \
             resources, never from a widget outward into the transport. \
             `{FORBIDDEN}` owns libcurl, SSRF address screening, the HTTP cache \
             and a worker pool; making it a dependency of the widget library \
             puts a network stack underneath every button.\n\n\
             If a widget needs to display a remote image, it renders the pure \
             state view `enzui_core::async_image::EnzAsyncImage`, and the \
             APPLICATION owns the `ImageService` that fills it in. That split is \
             what makes the fetch cancellable, bounded and generation-checked; \
             a widget calling the service from `view` cannot do any of \
             those things.",
            found.table
        );
    }
}

/// Walk first-party crates reachable from `root`, returning the dependency path
/// to `FORBIDDEN` if it is reachable.
fn find_forbidden_path(
    root: &str,
    read_deps: &dyn Fn(&str) -> BTreeSet<String>,
) -> Result<BTreeSet<String>, String> {
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::new();
    queue.push_back((root.to_owned(), vec![root.to_owned()]));
    seen.insert(root.to_owned());

    while let Some((crate_name, path)) = queue.pop_front() {
        for dep in read_deps(&crate_name) {
            if dep == FORBIDDEN {
                let mut full = path;
                full.push(FORBIDDEN.to_owned());
                return Err(full.join(" -> "));
            }
            // Follow first-party crates only; see the module docs on scope.
            if dep.starts_with("enzui-") && seen.insert(dep.clone()) {
                let mut next = path.clone();
                next.push(dep.clone());
                queue.push_back((dep, next));
            }
        }
    }

    Ok(seen)
}

/// The laundered edge: no first-party crate that widgets depends on may pull
/// resources in either.
#[test]
fn no_first_party_crate_beneath_widgets_pulls_in_resources() {
    let visited = match find_forbidden_path("enzui-widgets", &|c| dependency_names(c)) {
        Ok(visited) => visited,
        Err(path) => panic!(
            "`{FORBIDDEN}` is reachable from `enzui-widgets` via {path}.\n\n\
             The widget library must not reach the network stack, directly or \
             through a first-party crate. If the shared type belongs lower down, \
             move the TYPE into `enzui-core` (as `async_image`, \
             `url_policy` and `decode_limits` already are) — do not move the \
             service up."
        ),
    };

    assert!(
        visited.len() > 1,
        "the transitive walk visited only {} crate(s), so it checked nothing \
         beyond the direct dependencies it claims to extend. Visited: {visited:?}",
        visited.len()
    );
    for expected in ["enzui-core", "enzui-icons", "enzui-theme"] {
        assert!(
            visited.contains(expected),
            "the walk did not reach `{expected}`, which widgets genuinely \
             depends on, so its coverage claim is false. Visited: {visited:?}"
        );
    }
}

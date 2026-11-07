//! The core-capability keyword index's coverage floor.

#![allow(
    missing_docs,
    unreachable_pub,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::panic,
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    clippy::print_stdout,
    clippy::string_slice,
    clippy::missing_assert_message,
    reason = "a test crate reports by panicking and printing, and none of it is public API"
)]

use std::collections::BTreeSet;
use std::path::PathBuf;

/// Root of `enzui-core/`.
fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Root of the repository checkout.
fn repo_root() -> PathBuf {
    crate_root()
        .parent()
        .and_then(|crates| crates.parent())
        .expect("enzui-core lives at <repo>/crates/enzui-core")
        .to_path_buf()
}

/// The capability index page, relative to the repository root.
const INDEX_PAGE_PATH: &str = "crates/enzui-core/src/capability_index.md";

/// The forge copy of the index, generated from [`INDEX_PAGE_PATH`].
const GENERATED_PAGE_PATH: &str = "docs/core-capabilities.md";

/// The heading the keyword table lives under.
const INDEX_SECTION_MARKER: &str = "## Finding a core capability by keyword";

/// The capability index source text (the crate documentation).
fn index_page() -> String {
    let path = repo_root().join(INDEX_PAGE_PATH);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The public module-shaped API set declared at the top of `lib.rs`.
fn declared_pub_mods(lib_rs: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for raw in lib_rs.lines() {
        let line = raw.trim();
        if let Some(name) = line
            .strip_prefix("pub mod ")
            .and_then(|rest| rest.strip_suffix(';'))
        {
            out.insert(name.trim().to_owned());
            continue;
        }
        if let Some(path) = line
            .strip_prefix("pub use ")
            .and_then(|rest| rest.strip_suffix(';'))
            .filter(|path| {
                !path.contains('{')
                    && ["enzui_security::", "enzui_test_support::"]
                        .iter()
                        .any(|prefix| path.starts_with(prefix))
            })
            && let Some(name) = path.rsplit("::").next()
        {
            out.insert(name.trim().to_owned());
        }
    }
    out
}

/// The lib.rs text.
fn lib_rs() -> String {
    let path = crate_root().join("src/lib.rs");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Parse the aliases table under [`INDEX_SECTION_MARKER`] on the capability
/// index page and return `(alias keywords, target modules)` per row.
fn parse_alias_table(page: &str) -> Vec<(Vec<String>, Vec<String>)> {
    parse_alias_rows(page)
        .into_iter()
        .map(|row| (row.aliases, row.core))
        .collect()
}

/// One row of the keyword table, with its primary targets split by where
/// their destination points.
#[allow(dead_code, reason = "fixture fields are written but never read")]
#[derive(Debug)]
struct AliasRow {
    aliases: Vec<String>,
    /// Candidate core namespaces: a bare link or code span, or an intra-doc
    /// destination. Not yet checked against `lib.rs`.
    core: Vec<String>,
    /// Primary targets whose explicit destination is a sibling crate's
    /// rustdoc page. Their labels never count as core modules.
    external: Vec<SiblingDocLink>,
    /// Primary targets that are Markdown pages beside this index, such as
    /// `theme-file.md` for a capability documented on its own page.
    docs: Vec<String>,
    /// Sibling destinations that did not parse; always a violation.
    malformed: Vec<String>,
}

/// The namespace an intra-doc destination such as `mod@foo`,
/// `crate::foo::Bar` or `foo::Bar` names, or `None` for a destination that
/// is not an intra-doc path (a URL or an anchor).
fn intra_doc_namespace(dest: &str) -> Option<String> {
    let dest = dest.trim();
    if dest.contains("://") || dest.starts_with('#') || dest.contains('/') {
        return None;
    }
    let path = dest.split_once('@').map_or(dest, |(_, path)| path);
    let name = path
        .trim_start_matches("crate::")
        .trim_start_matches("enzui_core::")
        .split("::")
        .next()
        .unwrap_or("")
        .trim();
    is_doc_ident(name).then(|| name.to_owned())
}

fn parse_alias_rows(page: &str) -> Vec<AliasRow> {
    assert!(
        page.contains(INDEX_SECTION_MARKER),
        "the section `{INDEX_SECTION_MARKER}` is missing from {INDEX_PAGE_PATH}. \
         That is the header the gate parses; without it there is no index to check.",
    );

    let mut rows = Vec::new();
    let mut inside = false;
    for (line_idx, line) in page.lines().enumerate() {
        let l = line.trim().trim_start_matches("//!").trim();
        // The absence table is a separate section with its own parser; a row
        // that lands under the wrong heading must not be read as an alias.
        if l.starts_with("## ") {
            if inside {
                break;
            }
            inside = l == INDEX_SECTION_MARKER;
            continue;
        }
        if !inside {
            continue;
        }
        if !l.starts_with('|') || !l.ends_with('|') {
            continue;
        }
        // Skip the header row and the `|---|---|` separator.
        if l.contains("you searched for")
            || l.chars()
                .all(|c| c == '|' || c == '-' || c == ':' || c == ' ')
        {
            continue;
        }
        let cells: Vec<&str> = l.trim_matches('|').split('|').collect();
        if cells.len() < 2 {
            continue;
        }

        // Cell 1: alias keywords in backticks.
        let mut aliases = Vec::new();
        let mut chars = cells[0].char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            if c == '`'
                && let Some(end) = cells[0][i + 1..].find('`')
            {
                aliases.push(cells[0][i + 1..i + 1 + end].trim().to_owned());
                // advance past the closing tick
                for _ in 0..end + 1 {
                    chars.next();
                }
            }
        }

        let joined = cells[1..].join("|");
        let rest = joined
            .split_once('—')
            .map_or(joined.as_str(), |(head, _)| head);
        let mut targets = Vec::new();
        let mut external = Vec::new();
        let mut malformed = Vec::new();
        let mut docs = Vec::new();
        let mut i = 0;
        let bytes = rest.as_bytes();
        while i < bytes.len() {
            let span = if bytes[i] == b'[' {
                rest[i + 1..]
                    .find(']')
                    .map(|c| (c, &rest[i + 1..i + 1 + c]))
            } else if bytes[i] == b'`' {
                rest[i + 1..]
                    .find('`')
                    .map(|c| (c, &rest[i + 1..i + 1 + c]))
            } else {
                None
            };
            if let Some((close, inner)) = span {
                let after = i + close + 2;
                if bytes[i] == b'['
                    && rest[after..].starts_with('(')
                    && let Some(end) = rest[after + 1..].find(')')
                {
                    let dest = &rest[after + 1..after + 1 + end];
                    if dest.ends_with(".md") && !dest.contains("://") {
                        docs.push(dest.to_owned());
                    } else if dest.starts_with("../") {
                        match parse_sibling_href(dest, line_idx + 1) {
                            Ok(link) => external.push(link),
                            Err(error) => malformed.push(error),
                        }
                    } else if let Some(name) = intra_doc_namespace(dest) {
                        targets.push(name);
                    }
                    i = after + end + 2;
                    continue;
                }
                let name = inner
                    .trim()
                    .trim_matches('`')
                    .trim_start_matches("crate::")
                    .trim_start_matches("enzui_core::")
                    .split("::")
                    .next()
                    .unwrap_or("")
                    .trim();
                if !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit())
                {
                    targets.push(name.to_owned());
                }
                i += close + 2;
                continue;
            }
            i += 1;
        }

        // A row with keywords but no primary target is kept, so the coverage
        // gate reports it instead of the row silently leaving the population.
        if !aliases.is_empty() {
            rows.push(AliasRow {
                aliases,
                core: targets,
                external,
                docs,
                malformed,
            });
        }
    }
    rows
}

/// Normalise a keyword for "is this alias the module's own name?" — same
/// shape as the widgets test.
fn normalise(k: &str) -> String {
    k.trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c == ' ' || c == '-' { '_' } else { c })
        .collect()
}

/// Every row that points at a module must carry at least one keyword
/// which, after normalisation, is not that module's own name. This is the
/// same clause `every_tier1_module_has_a_non_self_keyword` enforces on the
/// widgets side, and it is what turns the table from a phone book (which
/// only helps someone who already knows the module name) into an index
/// (which reaches the module from the intent term).
#[test]
fn every_indexed_row_has_a_non_self_keyword() {
    let declared = declared_pub_mods(&lib_rs());
    let rows = parse_alias_table(&index_page());
    assert!(!rows.is_empty(), "alias table parsed as empty");

    for (keywords, targets) in &rows {
        for t in targets {
            if !declared.contains(t) {
                continue;
            }
            let has_non_self = keywords.iter().any(|k| normalise(k) != *t);
            assert!(
                has_non_self,
                "row targeting `{t}` has no keyword other than its own \
                 module name. Keywords: {keywords:?}. An alias equal to the \
                 module name teaches the searcher nothing they did not \
                 already have — they had to know the name to type it. Add \
                 an intent term (e.g. `hotkey` for `shortcut`, `range \
                 select` for `list_selection`).",
            );
        }
    }
}

#[allow(dead_code, reason = "fixture fields are written but never read")]
#[derive(Debug, Clone)]
struct SiblingDocLink {
    href: String,
    crate_us: String,
    module: Option<String>,
    /// The rustdoc page file, e.g. `fn.read.html` or `index.html`.
    page: String,
    /// An item anchor such as `method.file_transfer`.
    fragment: Option<String>,
    line: usize,
}

fn is_doc_ident(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn is_rustdoc_page(page: &str) -> bool {
    if page == "index.html" {
        return true;
    }
    let Some(stem) = page.strip_suffix(".html") else {
        return false;
    };
    let Some((kind, item)) = stem.split_once('.') else {
        return false;
    };
    is_doc_ident(kind)
        && !item.is_empty()
        && item.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn parse_sibling_href(href: &str, line: usize) -> Result<SiblingDocLink, String> {
    let rest = href
        .strip_prefix("../")
        .expect("caller only passes sibling hrefs");
    let (rest, fragment) = match rest.split_once('#') {
        Some((path, fragment)) => (path, Some(fragment)),
        None => (rest, None),
    };
    if let Some(fragment) = fragment
        && !fragment.split_once('.').is_some_and(|(kind, item)| {
            is_doc_ident(kind)
                && !item.is_empty()
                && item.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
    {
        return Err(format!(
            "{INDEX_PAGE_PATH}:{line}: malformed rustdoc anchor `#{fragment}` in `{href}`; \
             expected `#<kind>.<item>`"
        ));
    }
    let parts: Vec<&str> = rest.split('/').collect();
    let valid_shape = matches!(parts.as_slice(), [_, _] | [_, _, _]);
    if !valid_shape
        || !parts[0].starts_with("enzui_")
        || !is_doc_ident(parts[0])
        || !is_rustdoc_page(parts[parts.len() - 1])
    {
        return Err(format!(
            "{INDEX_PAGE_PATH}:{line}: malformed sibling rustdoc target `{href}`; \
             expected `../enzui_<crate>/<page>.html` or \
             `../enzui_<crate>/<module>/<page>.html`"
        ));
    }

    let module = if parts.len() == 3 {
        if !is_doc_ident(parts[1]) {
            return Err(format!(
                "{INDEX_PAGE_PATH}:{line}: malformed module segment `{}` in \
                 sibling rustdoc target `{href}`",
                parts[1]
            ));
        }
        Some(parts[1].to_owned())
    } else {
        None
    };

    Ok(SiblingDocLink {
        href: href.to_owned(),
        crate_us: parts[0].to_owned(),
        module,
        page: parts[parts.len() - 1].to_owned(),
        fragment: fragment.map(str::to_owned),
        line,
    })
}

/// `docs/core-capabilities.md` is the index with its rustdoc links rewritten
/// to source files, so it reads on a forge. Regenerate it after editing the
/// source page:
///
/// ```sh
/// ENZUI_WRITE_CAPABILITY_DOC=1 cargo wt --test capability_index docs_core_capabilities
/// ```
#[test]
fn docs_core_capabilities_md_is_not_stale() {
    let repo = repo_root();
    let rewritten = enzui_test_support::doc_links::rewrite_rustdoc_links(
        &index_page(),
        &repo,
        "enzui-core",
        "docs",
    );
    assert!(
        rewritten.unresolved.is_empty(),
        "{INDEX_PAGE_PATH} links to items that no source file defines: {:?}",
        rewritten.unresolved
    );
    let expected = format!(
        "<!-- Generated from {INDEX_PAGE_PATH}; edit that file and run\n     \
         ENZUI_WRITE_CAPABILITY_DOC=1 cargo wt --test capability_index docs_core_capabilities -->\n\n{}",
        rewritten.markdown
    );
    let path = repo.join(GENERATED_PAGE_PATH);
    if std::env::var_os("ENZUI_WRITE_CAPABILITY_DOC").is_some() {
        std::fs::write(&path, &expected)
            .unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
        return;
    }
    let actual = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        actual == expected,
        "{GENERATED_PAGE_PATH} is stale; regenerate it with \
         ENZUI_WRITE_CAPABILITY_DOC=1 cargo wt --test capability_index docs_core_capabilities"
    );
}

/// Every link on every documentation page resolves on a forge: relative
/// targets exist, and no rustdoc-only link form (intra-doc paths, rustdoc
/// HTML paths, bare code-span shortcuts) is left for a reader to click.
#[test]
fn every_documentation_page_link_resolves() {
    let repo = repo_root();
    let mut pages = vec!["README.md".to_owned(), "vendor/README.md".to_owned()];
    for (dir, file) in [("docs", None), ("crates", Some("README.md"))] {
        let mut entries: Vec<_> = std::fs::read_dir(repo.join(dir))
            .unwrap_or_else(|e| panic!("read {dir}: {e}"))
            .flatten()
            .map(|e| e.path())
            .collect();
        entries.sort();
        for path in entries {
            let page = match file {
                None if path.extension().is_some_and(|e| e == "md") => path,
                Some(name) if path.join(name).is_file() => path.join(name),
                _ => continue,
            };
            pages.push(
                page.strip_prefix(&repo)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }
    let mut dead = Vec::new();
    let mut examined = 0;
    for page in &pages {
        let (found, count) = enzui_test_support::doc_links::check_links(&repo, page);
        dead.extend(found);
        examined += count;
    }
    assert!(
        pages.len() > 30 && examined > 1000,
        "only {} pages and {examined} links were examined; the page walk or the link scanner \
         stopped matching",
        pages.len()
    );
    assert!(dead.is_empty(), "dead documentation links: {dead:#?}");
}

//! Turn rustdoc links in Markdown into links a forge can follow.
//!
//! Crate documentation and the pages under `docs/` share text. Rustdoc
//! resolves intra-doc links (`` [`Store`] ``, `[text](persist::Store)`) and
//! relative HTML paths (`../enzui_services/oauth/index.html`), but a forge
//! renders the same Markdown literally: bare `` [`Store`] `` shows brackets,
//! and the other two forms are dead links. [`rewrite_rustdoc_links`] replaces
//! each of them with a link to the source file that defines the item, and
//! [`check_links`] reports any link in a page that would still be dead.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

/// One link that [`rewrite_rustdoc_links`] could not resolve to a source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unresolved {
    /// The link text, as written.
    pub text: String,
    /// The rustdoc target the text pointed at.
    pub target: String,
}

/// The result of rewriting one page.
#[derive(Debug, Clone, Default)]
pub struct Rewritten {
    /// The page with every resolvable rustdoc link replaced.
    pub markdown: String,
    /// Links whose target is outside this repository's crates; their text is
    /// kept as plain text.
    pub external: Vec<Unresolved>,
    /// Links that name a repository crate but match no source file. A
    /// generator should fail on these: the page names something that does
    /// not exist.
    pub unresolved: Vec<Unresolved>,
}

/// Rewrites the rustdoc links in `markdown` into links to source files.
///
/// `repo` is the repository root, `default_crate` the crate directory name
/// (`enzui-core`) that `crate::` and unqualified paths belong to, and
/// `page_dir` the repository-relative directory the page is written to, so
/// emitted links are relative to it. Fenced code blocks and code spans
/// outside link text are copied unchanged.
pub fn rewrite_rustdoc_links(
    markdown: &str,
    repo: &Path,
    default_crate: &str,
    page_dir: &str,
) -> Rewritten {
    let resolver = Resolver::new(repo, default_crate);
    let prefix = up_prefix(page_dir);
    let mut out = Rewritten::default();
    let mut fenced = false;
    for line in markdown.split_inclusive('\n') {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            out.markdown.push_str(line);
            continue;
        }
        if fenced {
            out.markdown.push_str(line);
            continue;
        }
        rewrite_line(line, &resolver, &prefix, &mut out);
    }
    out
}

/// A link in a page that does not resolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeadLink {
    /// The page, relative to the repository root.
    pub page: String,
    /// The link target as written.
    pub target: String,
    /// Why the link is dead.
    pub reason: &'static str,
}

/// Every link in `page` (repository-relative) that a forge cannot follow.
///
/// Relative targets must exist on disk. Rustdoc-only forms are reported too:
/// a target with `::`, a target ending in `.html` that is not a URL, and a
/// bare code-span shortcut such as `` [`Store`] `` with no definition.
/// Returns the dead links and the number of links examined.
pub fn check_links(repo: &Path, page: &str) -> (Vec<DeadLink>, usize) {
    let text = fs::read_to_string(repo.join(page)).unwrap_or_else(|e| panic!("read {page}: {e}"));
    let dir = Path::new(page).parent().unwrap_or_else(|| Path::new(""));
    let mut dead = Vec::new();
    let mut examined = 0usize;
    let mut fenced = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        for link in scan_links(line) {
            examined += 1;
            let reason = match &link.kind {
                LinkKind::Inline(target) => classify_target(repo, dir, target),
                LinkKind::Shortcut => {
                    if has_definition(&text, &link.text) {
                        None
                    } else {
                        Some("rustdoc shortcut link; a forge shows it as plain brackets")
                    }
                }
                LinkKind::Reference => None,
            };
            if let Some(reason) = reason {
                let target = match link.kind {
                    LinkKind::Inline(target) => target,
                    LinkKind::Shortcut | LinkKind::Reference => link.text.clone(),
                };
                dead.push(DeadLink {
                    page: page.to_owned(),
                    target,
                    reason,
                });
            }
        }
    }
    (dead, examined)
}

fn classify_target(repo: &Path, dir: &Path, target: &str) -> Option<&'static str> {
    if target.contains("://") || target.starts_with("mailto:") || target.starts_with('#') {
        return None;
    }
    let path = target.split('#').next().unwrap_or(target);
    if path.contains("::") {
        return Some("rustdoc intra-doc path");
    }
    if path.ends_with(".html") {
        return Some("rustdoc HTML path");
    }
    if normalize(&repo.join(dir).join(path)).exists() {
        None
    } else if path.contains(['/', '.']) {
        Some("relative target does not exist")
    } else {
        Some("rustdoc intra-doc path")
    }
}

fn has_definition(text: &str, label: &str) -> bool {
    let wanted = format!("[{}]:", label.to_lowercase());
    text.lines()
        .any(|line| line.trim_start().to_lowercase().starts_with(&wanted))
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

fn up_prefix(page_dir: &str) -> String {
    page_dir
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|_| "../")
        .collect()
}

#[derive(Debug)]
enum LinkKind {
    Inline(String),
    Reference,
    Shortcut,
}

#[derive(Debug)]
struct Link {
    /// Byte range of the whole link in the line.
    start: usize,
    end: usize,
    text: String,
    kind: LinkKind,
}

/// Finds the links in one line, skipping code spans outside link text.
fn scan_links(line: &str) -> Vec<Link> {
    let bytes = line.as_bytes();
    let mut links = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'`' => i = skip_code_span(bytes, i),
            b'[' => {
                let Some(close) = find_bracket_close(bytes, i) else {
                    i += 1;
                    continue;
                };
                let text = line[i + 1..close].to_owned();
                let after = close + 1;
                if bytes.get(after) == Some(&b'(') {
                    if let Some(paren) = find_paren_close(bytes, after) {
                        let target = line[after + 1..paren].trim().to_owned();
                        links.push(Link {
                            start: i,
                            end: paren + 1,
                            text,
                            kind: LinkKind::Inline(target),
                        });
                        i = paren + 1;
                        continue;
                    }
                } else if bytes.get(after) == Some(&b'[') {
                    if let Some(end) = line[after..].find(']') {
                        links.push(Link {
                            start: i,
                            end: after + end + 1,
                            text,
                            kind: LinkKind::Reference,
                        });
                        i = after + end + 1;
                        continue;
                    }
                } else if bytes.get(after) != Some(&b':') && is_code_path(&text) {
                    links.push(Link {
                        start: i,
                        end: after,
                        text,
                        kind: LinkKind::Shortcut,
                    });
                    i = after;
                    continue;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    links
}

fn skip_code_span(bytes: &[u8], start: usize) -> usize {
    let run = bytes[start..].iter().take_while(|b| **b == b'`').count();
    let mut i = start + run;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let close = bytes[i..].iter().take_while(|b| **b == b'`').count();
            if close == run {
                return i + close;
            }
            i += close;
        } else {
            i += 1;
        }
    }
    // An unmatched backtick run is literal text.
    start + run
}

fn find_bracket_close(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 1,
            b'`' => {
                i = skip_code_span(bytes, i);
                continue;
            }
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn find_paren_close(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (i, b) in bytes.iter().enumerate().skip(open) {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            b' ' if depth == 1 => return None,
            _ => {}
        }
    }
    None
}

/// Whether link text is a single code span naming a Rust path, the only form
/// rustdoc turns into a shortcut link that a forge cannot.
fn is_code_path(text: &str) -> bool {
    let Some(inner) = text.strip_prefix('`').and_then(|t| t.strip_suffix('`')) else {
        return false;
    };
    !inner.is_empty()
        && inner.chars().any(char::is_alphabetic)
        && inner
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | ':' | '@' | '!' | '(' | ')'))
}

fn rewrite_line(line: &str, resolver: &Resolver, prefix: &str, out: &mut Rewritten) {
    let mut cursor = 0;
    for link in scan_links(line) {
        let target = match &link.kind {
            LinkKind::Inline(target) => {
                if is_rustdoc_target(target) {
                    target.clone()
                } else {
                    continue;
                }
            }
            LinkKind::Shortcut => link.text.trim_matches('`').to_owned(),
            LinkKind::Reference => continue,
        };
        out.markdown.push_str(&line[cursor..link.start]);
        match resolver.resolve(&target) {
            Resolution::Source(path) => {
                let _ = write!(out.markdown, "[{}]({prefix}{path})", link.text);
            }
            Resolution::External => {
                out.markdown.push_str(&link.text);
                out.external.push(Unresolved {
                    text: link.text.clone(),
                    target,
                });
            }
            Resolution::Missing => {
                out.markdown.push_str(&link.text);
                out.unresolved.push(Unresolved {
                    text: link.text.clone(),
                    target,
                });
            }
        }
        cursor = link.end;
    }
    out.markdown.push_str(&line[cursor..]);
}

fn is_rustdoc_target(target: &str) -> bool {
    if target.contains("://") || target.starts_with('#') || target.starts_with("mailto:") {
        return false;
    }
    let path = target.split('#').next().unwrap_or(target);
    path.contains("::") || path.ends_with(".html") || !path.contains(['/', '.'])
}

enum Resolution {
    Source(String),
    External,
    Missing,
}

struct Resolver<'a> {
    repo: &'a Path,
    default_crate: &'a str,
}

impl<'a> Resolver<'a> {
    fn new(repo: &'a Path, default_crate: &'a str) -> Self {
        Self {
            repo,
            default_crate,
        }
    }

    fn resolve(&self, target: &str) -> Resolution {
        let Some(segments) = target_segments(target) else {
            return Resolution::External;
        };
        self.resolve_segments(segments, 0)
    }

    fn resolve_segments(&self, segments: Vec<String>, depth: usize) -> Resolution {
        let mut segments = segments.as_slice();
        let krate = match segments.first().map(String::as_str) {
            Some("crate" | "self" | "super") => {
                segments = &segments[1..];
                self.default_crate.to_owned()
            }
            Some(first) if first.starts_with("enzui_") => {
                let dir = first.replace('_', "-");
                if !self.repo.join("crates").join(&dir).is_dir() {
                    return Resolution::Missing;
                }
                segments = &segments[1..];
                dir
            }
            Some(first) if is_external_root(first) => return Resolution::External,
            _ => self.default_crate.to_owned(),
        };
        let src = format!("crates/{krate}/src");
        let mut dir = src.clone();
        let mut file = format!("{src}/lib.rs");
        let mut consumed = 0;
        for seg in segments {
            let flat = format!("{dir}/{seg}.rs");
            let nested = format!("{dir}/{seg}/mod.rs");
            if self.repo.join(&flat).is_file() {
                file = flat;
            } else if self.repo.join(&nested).is_file() {
                file = nested;
            } else {
                break;
            }
            dir = format!("{dir}/{seg}");
            consumed += 1;
        }
        if consumed == 0 {
            let Some(name) = segments.first() else {
                return Resolution::Source(file);
            };
            if let Some(found) = self.find_definition(&src, name) {
                return Resolution::Source(found);
            }
            // A crate-root re-export (`pub use other::module;`) names an item
            // that another crate defines.
            if let Some(origin) = self.root_reexport(&krate, name) {
                let mut rerouted = origin;
                rerouted.extend(segments[1..].iter().cloned());
                if rerouted.first().is_some_and(|f| f.starts_with("enzui_")) && depth < 4 {
                    return self.resolve_segments(rerouted, depth + 1);
                }
                return Resolution::External;
            }
            if segments.len() == 1 && is_prelude_name(name) {
                return Resolution::External;
            }
            return Resolution::Missing;
        }
        Resolution::Source(file)
    }

    /// The full path a `pub use` at `krate`'s root gives `name`.
    fn root_reexport(&self, krate: &str, name: &str) -> Option<Vec<String>> {
        let lib = fs::read_to_string(self.repo.join(format!("crates/{krate}/src/lib.rs"))).ok()?;
        // Join each `pub use ...;` statement onto one line.
        let mut statements = Vec::new();
        let mut current: Option<String> = None;
        for line in lib.lines() {
            let line = line.trim();
            if let Some(acc) = current.as_mut() {
                acc.push(' ');
                acc.push_str(line);
            } else if let Some(rest) = line.strip_prefix("pub use ") {
                current = Some(rest.to_owned());
            }
            if current.as_ref().is_some_and(|acc| acc.ends_with(';')) {
                statements.extend(current.take());
            }
        }
        for statement in statements {
            let statement = statement.trim_end_matches(';').trim();
            let (base, items): (&str, Vec<&str>) = match statement.split_once('{') {
                Some((base, group)) => (
                    base.trim_end_matches("::"),
                    group
                        .trim_end_matches('}')
                        .split(',')
                        .map(str::trim)
                        .collect(),
                ),
                None => match statement.rsplit_once("::") {
                    Some((base, item)) => (base, vec![item]),
                    None => continue,
                },
            };
            for item in items {
                let (path, alias) = match item.split_once(" as ") {
                    Some((path, alias)) => (path.trim(), alias.trim()),
                    None => (item, item.rsplit("::").next().unwrap_or(item)),
                };
                if alias == name {
                    let mut full: Vec<String> = base.split("::").map(str::to_owned).collect();
                    full.extend(path.split("::").map(str::to_owned));
                    return Some(full);
                }
            }
        }
        None
    }

    /// The file under `src` that defines a public item called `name`.
    fn find_definition(&self, src: &str, name: &str) -> Option<String> {
        let mut files = Vec::new();
        collect_rs(&self.repo.join(src), &mut files);
        files.sort();
        let kinds = [
            "struct ",
            "enum ",
            "fn ",
            "const fn ",
            "async fn ",
            "unsafe fn ",
            "trait ",
            "type ",
            "const ",
            "static ",
            "mod ",
            "union ",
        ];
        for path in &files {
            let Ok(text) = fs::read_to_string(path) else {
                continue;
            };
            for line in text.lines() {
                let line = line.trim_start();
                let rest = line
                    .strip_prefix("pub ")
                    .or_else(|| line.strip_prefix("pub(crate) "))
                    .or_else(|| line.strip_prefix("macro_rules! "));
                let Some(rest) = rest else { continue };
                let hit = if line.starts_with("macro_rules! ") {
                    rest.strip_prefix(name)
                } else {
                    kinds
                        .iter()
                        .find_map(|k| rest.strip_prefix(k).and_then(|r| r.strip_prefix(name)))
                };
                if hit.is_some_and(|tail| {
                    !tail.starts_with(|c: char| c.is_alphanumeric() || c == '_')
                }) {
                    let rel = path.strip_prefix(self.repo).unwrap_or(path);
                    return Some(rel.to_string_lossy().into_owned());
                }
            }
        }
        None
    }
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Standard-library names rustdoc links to without a path.
fn is_prelude_name(name: &str) -> bool {
    matches!(
        name,
        "Drop"
            | "Clone"
            | "Copy"
            | "Default"
            | "Debug"
            | "Display"
            | "Send"
            | "Sync"
            | "Option"
            | "Result"
            | "Vec"
            | "String"
            | "Box"
            | "Arc"
            | "Rc"
            | "Fn"
            | "FnMut"
            | "FnOnce"
            | "Iterator"
            | "From"
            | "Into"
            | "PartialEq"
            | "Eq"
            | "Hash"
            | "Ord"
            | "PartialOrd"
            | "AsRef"
            | "Deref"
            | "Path"
            | "PathBuf"
            | "Duration"
            | "Instant"
            | "Mutex"
    )
}

fn is_external_root(first: &str) -> bool {
    matches!(
        first,
        "std" | "core" | "alloc" | "iced" | "winit" | "wgpu" | "zbus" | "tokio"
    ) || first.starts_with("iced_")
        || first.starts_with("accesskit")
}

/// The path segments a rustdoc target names, or `None` for a target this
/// resolver does not understand.
fn target_segments(target: &str) -> Option<Vec<String>> {
    let target = target.split('#').next().unwrap_or(target);
    if let Some(html) = target.strip_suffix(".html") {
        // `../crate/module/struct.Name.html` or `../crate/module/index.html`.
        let mut parts: Vec<String> = html
            .split('/')
            .filter(|p| !p.is_empty() && *p != ".." && *p != ".")
            .map(str::to_owned)
            .collect();
        let last = parts.pop()?;
        if last != "index" {
            let name = last.split_once('.').map_or(last.as_str(), |(_, n)| n);
            parts.push(name.to_owned());
        }
        return Some(parts);
    }
    let target = target.split_once('@').map_or(target, |(_, t)| t);
    let target = target.trim_end_matches("()").trim_end_matches('!');
    let segments: Vec<String> = target.split("::").map(str::to_owned).collect();
    if segments
        .iter()
        .any(|s| s.is_empty() || !s.chars().all(|c| c.is_alphanumeric() || c == '_'))
    {
        return None;
    }
    Some(segments)
}

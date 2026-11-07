//! Byte-based ignore patterns from repository files and the default global file.
//! Supports component globs, directory patterns, anchoring, negation, and nested
//! precedence; does not implement every Git configuration or wildmatch feature.

use std::path::Path;

const MAX_IGNORE_FILE: u64 = 1024 * 1024;

#[derive(Debug, Clone)]
struct Pattern {
    glob: Vec<u8>,
    negated: bool,
    directory_only: bool,
    anchored: bool,
}

#[derive(Debug, Clone, Default)]
pub(super) struct RuleSet {
    base: Vec<u8>,
    patterns: Vec<Pattern>,
}

impl RuleSet {
    pub(super) fn parse(base: &[u8], text: &[u8]) -> Self {
        let patterns = text
            .split(|byte| *byte == b'\n')
            .filter_map(parse_line)
            .collect();
        Self {
            base: base.to_vec(),
            patterns,
        }
    }

    pub(super) fn from_file(base: &[u8], file: &Path) -> Option<Self> {
        let text = super::read_regular(file, MAX_IGNORE_FILE, false).ok()?;
        Some(Self::parse(base, &text))
    }

    // User-managed excludes may be symlinked, but must resolve to a bounded regular file.
    pub(super) fn from_user_file(base: &[u8], file: &Path) -> Option<Self> {
        let text = super::read_regular(file, MAX_IGNORE_FILE, true).ok()?;
        Some(Self::parse(base, &text))
    }

    fn verdict(&self, path: &[u8], is_dir: bool) -> Option<bool> {
        let below = path.strip_prefix(self.base.as_slice())?;
        let name = below.rsplit(|byte| *byte == b'/').next().unwrap_or(below);
        self.patterns.iter().rev().find_map(|pattern| {
            if pattern.directory_only && !is_dir {
                return None;
            }
            let subject = if pattern.anchored { below } else { name };
            glob(&pattern.glob, subject).then_some(!pattern.negated)
        })
    }
}

fn parse_line(line: &[u8]) -> Option<Pattern> {
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    if line.is_empty() || line[0] == b'#' {
        return None;
    }
    // Trailing spaces go unless escaped with a backslash.
    let mut end = line.len();
    while end > 0 && line[end - 1] == b' ' && !(end > 1 && line[end - 2] == b'\\') {
        end -= 1;
    }
    let mut line = &line[..end];
    if line.is_empty() {
        return None;
    }
    let negated = line[0] == b'!';
    // `!` negates; `\!` and `\#` are those characters, escaped.
    if negated || line.starts_with(b"\\!") || line.starts_with(b"\\#") {
        line = &line[1..];
    }
    let directory_only = line.ends_with(b"/");
    if directory_only {
        line = &line[..line.len() - 1];
    }
    if line.is_empty() {
        return None;
    }
    let anchored = line.contains(&b'/');
    let line = line.strip_prefix(b"/").unwrap_or(line);
    Some(Pattern {
        glob: line.to_vec(),
        negated,
        directory_only,
        anchored,
    })
}

pub(super) fn glob(pattern: &[u8], text: &[u8]) -> bool {
    match pattern.first() {
        None => text.is_empty(),
        Some(b'*') if pattern.get(1) == Some(&b'*') => {
            // `**/` matches zero or more whole directories; a trailing `**`
            // matches everything below.
            let rest = &pattern[2..];
            let rest = rest.strip_prefix(b"/").unwrap_or(rest);
            if rest.is_empty() {
                return true;
            }
            (0..=text.len()).any(|i| (i == 0 || text[i - 1] == b'/') && glob(rest, &text[i..]))
        }
        Some(b'*') => {
            let rest = &pattern[1..];
            (0..=text.len())
                .take_while(|i| *i == 0 || text[i - 1] != b'/')
                .any(|i| glob(rest, &text[i..]))
        }
        Some(b'?') => text.first().is_some_and(|c| *c != b'/') && glob(&pattern[1..], &text[1..]),
        Some(b'[') => match (class(&pattern[1..], text.first().copied()), text.first()) {
            (Some((true, used)), Some(_)) => glob(&pattern[1 + used..], &text[1..]),
            _ => false,
        },
        Some(b'\\') if pattern.len() > 1 => {
            text.first() == Some(&pattern[1]) && glob(&pattern[2..], &text[1..])
        }
        Some(c) => text.first() == Some(c) && glob(&pattern[1..], &text[1..]),
    }
}

fn class(pattern: &[u8], byte: Option<u8>) -> Option<(bool, usize)> {
    let byte = byte?;
    if byte == b'/' {
        return Some((false, pattern.iter().position(|c| *c == b']')? + 1));
    }
    let (negated, mut i) = match pattern.first() {
        Some(b'!' | b'^') => (true, 1),
        _ => (false, 0),
    };
    let mut matched = false;
    let mut first = true;
    while i < pattern.len() {
        let c = pattern[i];
        if c == b']' && !first {
            return Some((matched != negated, i + 1));
        }
        first = false;
        if pattern.get(i + 1) == Some(&b'-') && pattern.get(i + 2).is_some_and(|end| *end != b']') {
            let end = pattern[i + 2];
            if (c..=end).contains(&byte) {
                matched = true;
            }
            i += 3;
        } else {
            if c == byte {
                matched = true;
            }
            i += 1;
        }
    }
    None
}

#[derive(Debug, Clone, Default)]
pub(super) struct Stack {
    sets: Vec<RuleSet>,
}

impl Stack {
    pub(super) fn new(base_sets: Vec<RuleSet>) -> Self {
        Self { sets: base_sets }
    }

    pub(super) fn with(&self, set: Option<RuleSet>) -> Self {
        let mut sets = self.sets.clone();
        sets.extend(set);
        Self { sets }
    }

    pub(super) fn is_ignored(&self, path: &[u8], is_dir: bool) -> bool {
        self.sets
            .iter()
            .rev()
            .find_map(|set| set.verdict(path, is_dir))
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_match_components_recursive_paths_classes_and_escapes() {
        assert!(glob(b"*.log", b"debug.log"));
        assert!(!glob(b"*.log", b"logs/debug.log"), "* stays in a component");
        assert!(glob(b"**/build", b"a/b/build"));
        assert!(glob(b"**/build", b"build"));
        assert!(glob(b"docs/**", b"docs/a/b.md"));
        assert!(glob(b"a/**/z", b"a/z"));
        assert!(glob(b"a/**/z", b"a/b/c/z"));
        assert!(glob(b"file[0-9].txt", b"file7.txt"));
        assert!(!glob(b"file[!0-9].txt", b"file7.txt"));
        assert!(glob(b"\\#notes", b"#notes"));
        assert!(glob(b"?.rs", b"a.rs") && !glob(b"?.rs", b"ab.rs"));
    }
}

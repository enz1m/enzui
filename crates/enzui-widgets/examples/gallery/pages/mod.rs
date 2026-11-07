//! One module per gallery page.

mod buttons;
mod charts;
mod containers;
mod content;
mod data;
mod dialogs;
mod feedback;
mod images;
mod input;
mod layouts;
mod materials;
mod motion;
mod navigation;
mod pickers;
mod selection;

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::Path;

    /// Every `pub fn` name in the framework crates the specimens show.
    fn public_functions() -> HashSet<String> {
        fn walk(dir: &Path, names: &mut HashSet<String>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, names);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let source = std::fs::read_to_string(&path).unwrap_or_default();
                    for (at, _) in source.match_indices("pub fn ") {
                        let name: String = source[at + 7..]
                            .chars()
                            .take_while(|c| c.is_alphanumeric() || *c == '_')
                            .collect();
                        let _ = names.insert(name);
                    }
                }
            }
        }
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut names = HashSet::new();
        for krate in ["enzui-widgets", "enzui-core", "enzui-theme"] {
            walk(&crates.join(krate).join("src"), &mut names);
        }
        names
    }

    /// The string literal `text` starts with, after whitespace.
    fn first_literal(text: &str) -> Option<&str> {
        let text = text.trim_start().strip_prefix('"')?;
        Some(&text[..text.find('"')?])
    }

    /// Where the first argument of a call ends, when the call has a second:
    /// the first comma outside nested brackets before the call closes.
    fn top_level_comma(arguments: &str) -> Option<usize> {
        let mut depth = 0usize;
        for (at, c) in arguments.char_indices() {
            match c {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' if depth == 0 => return None,
                ')' | ']' | '}' => depth -= 1,
                ',' if depth == 0 => return Some(at),
                _ => {}
            }
        }
        None
    }

    /// The method and function names the pages put in their code: the first
    /// literal of `.set(`, the literal after the condition of `.set_if(` and
    /// `.variant(`, and the function of `code(path, function)`.
    fn shown_names(source: &str) -> Vec<String> {
        let mut names = Vec::new();
        for (pattern, skip_condition) in [(".set(", false), (".set_if(", true), (".variant(", true)]
        {
            for (at, _) in source.match_indices(pattern) {
                let mut rest = &source[at + pattern.len()..];
                if skip_condition {
                    match top_level_comma(rest) {
                        Some(comma) => rest = &rest[comma + 1..],
                        None => continue,
                    }
                }
                names.extend(first_literal(rest).map(str::to_owned));
            }
        }
        for (at, _) in source.match_indices("&code(") {
            let rest = &source[at..];
            let after_path = rest.find(',').map_or(rest, |comma| &rest[comma + 1..]);
            names.extend(first_literal(after_path).map(str::to_owned));
        }
        names
    }

    #[test]
    fn every_method_the_code_shows_exists() {
        let functions = public_functions();
        assert!(
            functions.len() > 1000,
            "found {} functions",
            functions.len()
        );
        let pages = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/gallery/pages");
        let mut checked = 0;
        let mut missing = Vec::new();
        for entry in std::fs::read_dir(pages).unwrap().flatten() {
            if entry.file_name() == "mod.rs" {
                continue;
            }
            let source = std::fs::read_to_string(entry.path()).unwrap();
            for name in shown_names(&source) {
                checked += 1;
                if !functions.contains(&name) {
                    missing.push(format!("{}: {name}", entry.file_name().to_string_lossy()));
                }
            }
        }
        assert!(
            checked > 150,
            "only {checked} names found: the scan is broken"
        );
        assert!(
            missing.is_empty(),
            "code shows methods that do not exist: {missing:#?}"
        );
    }
}

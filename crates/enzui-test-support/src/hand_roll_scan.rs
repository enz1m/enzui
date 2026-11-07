//! Shared source-tree completeness check for the per-app anti-hand-rolling
//! guards.
//!
//! An app's `hand_roll_guard` reddens when a view source file reintroduces a
//! raw `iced::widget::button(...)` call, a raw `Color::from_rgb*(...)` literal
//! or a bordered `container::Style { .. }` hand-roll. Each guard scans a
//! hand-maintained list of `include_str!` paths, and a view file missing from
//! that list would never be scanned. A hand-maintained covered set is not a
//! gate: dropping a line disables the check.
//!
//! # Derive the covered set, don't list it
//!
//! This helper adds a completeness check the guard runs alongside its per-file
//! scans, keeping `include_str!` (compile-time embedded, so independent of the
//! working directory):
//!
//! 1. Walk the app's `src/` tree, rooted at the guard crate's
//!    `CARGO_MANIFEST_DIR`.
//! 2. Classify every `.rs` file as *view-ish* or not: a file is view-ish if its
//!    production half (the source with every inline `#[cfg(test)]` module
//!    excised and comments stripped) contains `Element<`, the signature of a
//!    function that builds a widget tree. Pure-logic modules are not required
//!    to be scanned.
//! 3. Redden if any view-ish file is missing from the guard's scanned set.
//!
//! A new view file is covered the day it is created.
//!
//! # Floors: a walker that matches nothing looks like a clean pass
//!
//! [`CoverageReport`] carries the number of `.rs` files walked and of view-ish
//! files found, and the caller asserts both are above a floor
//! ([`CoverageReport::assert_floor`]). A walker rooted in the wrong place would
//! otherwise report "no unscanned view files" exactly like a complete guard.
//!
//! The same holds for depth. Listing a file is worthless if the production
//! projection keeps only part of it, so:
//!
//! * [`inline_test_module_spans`] recognises what a `cfg(test)` attribute is
//!   attached to. Only the `mod` keyword ends production code; a test-only
//!   field, variant, method, `impl`, `type` or `use` is skipped over. It
//!   tolerates a trailing `//` comment on the attribute line and intervening
//!   attributes and doc comments, and reads a compound gate
//!   (`#[cfg(all(test, target_os = "linux"))]`) through the shared
//!   [`leading_cfg_attribute`] predicate.
//! * [`production_code`] excises those spans rather than truncating at the
//!   first one, so production code between or below test modules is scanned.
//! * [`assert_scan_depth_floor`] reddens when the scanned fraction of any file
//!   falls below a floor, so a listed file is a scanned file.

use std::path::{Path, PathBuf};

/// Strip `//` line comments and `/* ... */` block comments from `src`.
///
/// This mirrors the `strip_comments` helper embedded in every per-app guard so
/// that the view-ish classification here matches the guards' own notion of
/// "production code": an `Element<` mention inside a `//!` doc comment or a
/// `// was: Element<..>` audit-trail line must not make a pure-domain module
/// look view-ish.
///
/// Block-comment handling is naive (no awareness of string literals), which is
/// the same trade-off the per-app guards make; none of the scanned sources
/// carry `/*` or `*/` inside string literals in their production halves.
///
/// # UTF-8 correctness
///
/// Output is copied in source slices, never byte by byte as `char`s, so
/// multi-byte UTF-8 sequences survive exactly.
pub fn strip_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let b = src.as_bytes();
    let mut i = 0;
    let mut run_start = 0usize;
    while i < b.len() {
        if i + 1 < b.len() && b[i] == b'/' && b[i + 1] == b'*' {
            if i > run_start {
                out.push_str(&src[run_start..i]);
            }
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(b.len());
            run_start = i;
            continue;
        }
        if i + 1 < b.len() && b[i] == b'/' && b[i + 1] == b'/' {
            if i > run_start {
                out.push_str(&src[run_start..i]);
            }
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            run_start = i;
            continue;
        }
        i += 1;
    }
    if run_start < b.len() {
        out.push_str(&src[run_start..]);
    }
    out
}

/// Blank every comment and string/character literal in one pass, preserving
/// byte offsets and newlines so diagnostics stay aligned with the source.
///
/// This exists because the two jobs cannot be composed in either
/// order. [`strip_comments`] is string-unaware, so running it first mangles
/// a guard fixture such as `"/* legacy: Color::from_rgb(0.5) */"` into
/// unbalanced quotes, and a later real literal then leaks its needle into the
/// scan (observed as a false positive in a real guard file).
/// Running a string stripper first breaks on `// he said "hi`. A single lexer
/// that knows both states has neither failure.
///
/// Needles inside string literals are not call sites: every app's
/// `hand_roll_guard.rs` quotes the very spellings it bans in fixtures and
/// assertion messages, and a tree scan that reads them reports the guard's own
/// failure text as a violation.
///
/// Handles `//`, nested `/* .. */`, `".."` with `\` escapes, raw strings
/// `r".."` / `r#".."#` / `br#".."#` with any hash count, and character
/// literals. A lifetime (`&'a str`) is copied verbatim, never treated as an
/// unterminated literal.
pub fn strip_comments_and_string_literals(src: &str) -> String {
    mask_rust_non_code(src)
}

/// Blank Rust comments while preserving every literal byte-for-byte.
///
/// Unlike [`strip_comments`], this uses the canonical literal-aware lexer, so
/// `//`, `/*`, braces, or quotes inside a normal/raw/byte/character literal are
/// data and cannot start or end a comment. Newlines and byte offsets survive.
pub fn strip_comments_preserving_literals(src: &str) -> String {
    let mut out = src.as_bytes().to_vec();
    for (start, end) in rust_non_code_spans(src) {
        if !src[start..].starts_with("//") && !src[start..].starts_with("/*") {
            continue;
        }
        for byte in &mut out[start..end] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    }
    String::from_utf8(out).expect("blanking Rust comments with ASCII spaces preserves UTF-8")
}

/// Project Rust source to code only while preserving byte offsets and line
/// breaks. Comments and every string/character literal form are replaced with
/// spaces; attributes, lifetimes and all other code remain byte-for-byte.
///
/// This is the shared lexical front-end for source-policy gates. It handles
/// line comments, nested block comments, normal/raw/byte/C strings, escaped
/// characters, byte characters and UTF-8 character literals. Keeping one
/// implementation prevents two policy censuses from disagreeing about whether
/// the same source byte is code.
pub fn mask_rust_non_code(src: &str) -> String {
    let bytes = src.as_bytes();
    let mut out = bytes.to_vec();
    for (start, end) in rust_non_code_spans(src) {
        for byte in &mut out[start..end] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    }
    String::from_utf8(out).expect("masking Rust source with ASCII spaces preserves UTF-8")
}

/// A per-byte companion to [`mask_rust_non_code`]. `true` means the byte is
/// Rust code; `false` means it belongs to a comment or literal. The extra final
/// element permits callers to index a match at EOF without a separate case.
pub fn rust_code_mask(src: &str) -> Vec<bool> {
    let mut mask = vec![true; src.len() + 1];
    for (start, end) in rust_non_code_spans(src) {
        mask[start..end].fill(false);
        // A newline remains a code boundary in the textual projection.
        for (offset, byte) in src.as_bytes()[start..end].iter().enumerate() {
            if *byte == b'\n' {
                mask[start + offset] = true;
            }
        }
    }
    mask
}

// ---------------------------------------------------------------------------
// `cfg(test)` attribute recognition
// ---------------------------------------------------------------------------

/// Truth of a `cfg` predicate evaluated with `test` off and every other
/// leaf unknown.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CfgValue {
    /// Holds even without `cfg(test)` — the item is production code.
    TrueWithoutTest,
    /// Cannot hold without `cfg(test)` — the item is test-only.
    FalseWithoutTest,
    /// Configuration-dependent, but not on `test`.
    Unknown,
}

/// One node of a `cfg(..)` predicate.
#[derive(Debug, Eq, PartialEq)]
enum CfgPredicate {
    /// The bare `test` word as a whole atom — never a substring. `contest`,
    /// `attest`, `test_support` and `feature = "contest"` all parse as
    /// [`CfgPredicate::Opaque`] and can never mark an item test-only.
    Test,
    /// Any other leaf: a bare identifier, or `ident = <literal>`.
    Opaque,
    All(Vec<CfgPredicate>),
    Any(Vec<CfgPredicate>),
    Not(Box<CfgPredicate>),
}

impl CfgPredicate {
    /// Evaluate with `test` off and every other leaf unknown.
    fn without_test(&self) -> CfgValue {
        match self {
            CfgPredicate::Test => CfgValue::FalseWithoutTest,
            CfgPredicate::Opaque => CfgValue::Unknown,
            CfgPredicate::All(parts) => {
                if parts
                    .iter()
                    .any(|part| part.without_test() == CfgValue::FalseWithoutTest)
                {
                    CfgValue::FalseWithoutTest
                } else if parts
                    .iter()
                    .any(|part| part.without_test() == CfgValue::Unknown)
                {
                    CfgValue::Unknown
                } else {
                    CfgValue::TrueWithoutTest
                }
            }
            CfgPredicate::Any(parts) => {
                if parts
                    .iter()
                    .any(|part| part.without_test() == CfgValue::TrueWithoutTest)
                {
                    CfgValue::TrueWithoutTest
                } else if parts
                    .iter()
                    .any(|part| part.without_test() == CfgValue::Unknown)
                {
                    CfgValue::Unknown
                } else {
                    CfgValue::FalseWithoutTest
                }
            }
            CfgPredicate::Not(inner) => match inner.without_test() {
                CfgValue::FalseWithoutTest => CfgValue::TrueWithoutTest,
                CfgValue::TrueWithoutTest => CfgValue::FalseWithoutTest,
                CfgValue::Unknown => CfgValue::Unknown,
            },
        }
    }

    /// Whether this predicate contains `test` as a whole atom.
    fn contains_test(&self) -> bool {
        match self {
            CfgPredicate::Test => true,
            CfgPredicate::Opaque => false,
            CfgPredicate::All(parts) | CfgPredicate::Any(parts) => {
                parts.iter().any(CfgPredicate::contains_test)
            }
            CfgPredicate::Not(inner) => inner.contains_test(),
        }
    }

    /// Whether an item carrying this predicate is absent from every non-test
    /// build *because the predicate actually contains `test`*. Requiring both
    /// conditions prevents always-false predicates such as `any()` and
    /// `not(all())` from masquerading as test gates. An `Unknown` keeps the
    /// item in production, which is the safe direction (a hand-roll stays
    /// visible to the sweeps).
    fn is_test_only(&self) -> bool {
        self.contains_test() && self.without_test() == CfgValue::FalseWithoutTest
    }
}

/// Offset of the `)` matching the `(` at `open`, or `None`.
///
/// The scan stops at a `{`, `}` or `;`, none of which can appear inside a
/// `cfg` predicate once literals are masked. That bound is what keeps an
/// unterminated attribute from swallowing the rest of the file (and from
/// pairing with some unrelated `)` far below it).
fn matching_cfg_paren(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0_i32;
    let mut at = open;
    while at < bytes.len() {
        match bytes[at] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(at);
                }
            }
            b'{' | b'}' | b';' => return None,
            _ => {}
        }
        at += 1;
    }
    None
}

/// Parse a `cfg(..)` predicate list from `code`, which must already be the
/// canonical length-preserving lexical projection ([`mask_rust_non_code`]), so
/// a `(`, `,` or the word `test` inside a string or a comment cannot be read
/// as syntax.
///
/// Returns `None` for anything this parser does not recognise; an unrecognised
/// predicate keeps its item in production (fail loud, not fail open).
fn parse_cfg_list(code: &str) -> Option<Vec<CfgPredicate>> {
    let bytes = code.as_bytes();
    let mut at = 0usize;
    let mut out = Vec::new();
    loop {
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        if at >= bytes.len() {
            return Some(out);
        }
        // A Rust cfg key is one identifier, never a numeric literal or path.
        // Keep unsupported identifier forms in production rather than
        // accepting malformed `123` / `foo::bar` predicates as opaque leaves.
        let start = at;
        if !bytes
            .get(at)
            .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
        {
            return None;
        }
        at += 1;
        while at < bytes.len() && (bytes[at].is_ascii_alphanumeric() || bytes[at] == b'_') {
            at += 1;
        }
        let ident = &code[start..at];
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        let predicate = match bytes.get(at) {
            Some(b'(') => {
                let open = at;
                let close = matching_cfg_paren(bytes, open)?;
                let inner = parse_cfg_list(&code[open + 1..close])?;
                at = close + 1;
                match ident {
                    "all" => CfgPredicate::All(inner),
                    "any" => CfgPredicate::Any(inner),
                    "not" => {
                        let mut inner = inner;
                        if inner.len() != 1 {
                            return None;
                        }
                        CfgPredicate::Not(Box::new(inner.remove(0)))
                    }
                    _ => return None,
                }
            }
            // `ident = <literal>`: the literal is blanked in the projection,
            // so its text cannot matter. `feature = "test"` is opaque.
            Some(b'=') => {
                at += 1;
                while at < bytes.len() && bytes[at] != b',' {
                    if bytes[at] == b'(' || bytes[at] == b')' {
                        return None;
                    }
                    at += 1;
                }
                CfgPredicate::Opaque
            }
            _ => {
                if ident == "test" {
                    CfgPredicate::Test
                } else {
                    CfgPredicate::Opaque
                }
            }
        };
        out.push(predicate);
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        match bytes.get(at) {
            None => return Some(out),
            Some(b',') => at += 1,
            Some(_) => return None,
        }
    }
}

/// An outer `#[cfg(..)]` attribute recognised at the start of a slice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CfgAttribute {
    /// Byte length of the attribute text, `#[` through its closing `]`. The
    /// attribute may span several lines.
    pub len: usize,
    /// True when the predicate contains a whole `test` atom and cannot hold
    /// with that atom disabled, so it is specifically a test gate.
    pub gates_on_test: bool,
}

/// Parse an outer attribute at the very start of `code`.
///
/// `code` must already be [`mask_rust_non_code`]d: comments and every literal
/// are blanked while byte offsets survive, so a `)`, `]` or the word `test`
/// written inside a string cannot be read as attribute syntax.
///
/// This is the one shared answer to "does this attribute make its item
/// test-only?", used by both the inline-test-module scanner and the
/// `cfg(test)` item projection. It recognises `#[cfg(test)]`,
/// `#[cfg(all(test, unix))]`, `#[cfg(all(target_os = "linux", any(test)))]`
/// and `#[cfg(not(not(test)))]`; it deliberately does not recognise
/// `#[cfg(any(test, unix))]` (that item also exists in a `unix` non-test
/// build), `#[cfg(feature = "contest")]`, `#[cfg_attr(test, ..)]`, or any
/// predicate shape it cannot parse. Every unrecognised shape reports
/// `gates_on_test: false`, keeping the item in production — the safe
/// direction, since a false positive is a visible hand-roll finding while a
/// false negative hides code from every sweep.
///
/// Returns `None` when `code` does not begin with a complete `#[cfg(..)]`
/// attribute. Nothing here indexes past the end of `code` or panics: a
/// truncated or unterminated attribute is simply not recognised, whatever its
/// length.
pub fn leading_cfg_attribute(code: &str) -> Option<CfgAttribute> {
    let bytes = code.as_bytes();
    if !code.starts_with("#[") {
        return None;
    }
    let mut at = 2usize;
    while at < bytes.len() && bytes[at].is_ascii_whitespace() {
        at += 1;
    }
    if !code[at..].starts_with("cfg") {
        return None;
    }
    at += "cfg".len();
    // `cfg` must be a whole token: `cfg_attr(test, ..)` conditionally *adds*
    // an attribute, it does not gate the item out of a non-test build.
    match bytes.get(at) {
        Some(b'(') => {}
        Some(byte) if byte.is_ascii_whitespace() => {
            while at < bytes.len() && bytes[at].is_ascii_whitespace() {
                at += 1;
            }
            if bytes.get(at) != Some(&b'(') {
                return None;
            }
        }
        _ => return None,
    }
    let open = at;
    let close = matching_cfg_paren(bytes, open)?;
    let mut end = close + 1;
    while end < bytes.len() && bytes[end].is_ascii_whitespace() {
        end += 1;
    }
    if bytes.get(end) != Some(&b']') {
        return None;
    }
    let gates_on_test = parse_cfg_list(&code[open + 1..close])
        .is_some_and(|predicate| predicate.len() == 1 && predicate[0].is_test_only());
    Some(CfgAttribute {
        len: end + 1,
        gates_on_test,
    })
}

/// True when `attribute` — one whole outer attribute, `#[` through `]`, as
/// carried by [`RustAttribute`] and [`FileModuleDeclaration`] — gates its item
/// on `cfg(test)`.
///
/// The shared predicate behind [`leading_cfg_attribute`]; see it for the exact
/// shapes accepted and refused.
pub fn attribute_gates_on_cfg_test(attribute: &str) -> bool {
    let code = mask_rust_non_code(attribute.trim());
    leading_cfg_attribute(&code)
        .is_some_and(|parsed| parsed.gates_on_test && code[parsed.len..].trim().is_empty())
}

/// Byte spans occupied by items directly gated on `cfg(test)`.
///
/// The gating attribute is recognised by [`leading_cfg_attribute`], so a
/// compound `#[cfg(all(test, target_os = "linux"))]` item is excised exactly
/// like a plain `#[cfg(test)]` one, while `#[cfg(any(test, unix))]` and
/// `#[cfg(feature = "contest")]` items stay in production.
///
/// Boundaries are found in the canonical length-preserving lexical projection,
/// so braces, semicolons, commas, and comment markers inside any Rust literal
/// or comment cannot terminate an item. The spans index the original source.
/// A complete gate with no complete item yields no span and leaves the
/// remaining source visible rather than panicking or guessing a boundary.
fn cfg_test_item_spans(src: &str) -> Vec<(usize, usize)> {
    let code = mask_rust_non_code(src);
    let bytes = code.as_bytes();
    let mut spans = Vec::new();
    let mut cursor = 0;
    while let Some(relative) = code[cursor..].find("#[") {
        let cfg = cursor + relative;
        let Some(attribute) = leading_cfg_attribute(&code[cfg..]) else {
            cursor = cfg + "#[".len();
            continue;
        };
        let mut at = cfg + attribute.len;
        if !attribute.gates_on_test {
            cursor = at;
            continue;
        }
        let (mut parens, mut brackets, mut angles) = (0_i32, 0_i32, 0_i32);
        let mut end = None;
        while at < bytes.len() {
            match bytes[at] {
                b'(' => parens += 1,
                b')' => parens -= 1,
                b'[' => brackets += 1,
                b']' => brackets -= 1,
                b'<' => angles += 1,
                b'>' if angles > 0 => angles -= 1,
                b';' | b',' if parens == 0 && brackets == 0 && angles == 0 => {
                    end = Some(at + 1);
                    break;
                }
                b'{' if parens == 0 && brackets == 0 && angles == 0 => {
                    let mut depth = 1_i32;
                    at += 1;
                    while at < bytes.len() && depth != 0 {
                        match bytes[at] {
                            b'{' => depth += 1,
                            b'}' => depth -= 1,
                            _ => {}
                        }
                        at += 1;
                    }
                    if depth == 0 {
                        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
                            at += 1;
                        }
                        if matches!(bytes.get(at), Some(b';' | b',')) {
                            at += 1;
                        }
                        end = Some(at);
                    }
                    break;
                }
                _ => {}
            }
            at += 1;
        }
        let Some(end) = end else {
            // A complete cfg attribute with no item, or with an incomplete
            // item, is malformed source. Retain it and everything following
            // it rather than panicking or guessing a span that may hide code.
            break;
        };
        spans.push((cfg, end));
        cursor = end;
    }
    spans
}

fn remove_spans(src: &str, spans: &[(usize, usize)]) -> String {
    let mut out = String::with_capacity(src.len());
    let mut cursor = 0;
    for &(start, end) in spans {
        out.push_str(&src[cursor..start]);
        cursor = end;
    }
    out.push_str(&src[cursor..]);
    out
}

/// Source with every directly `#[cfg(test)]`-gated item removed while all
/// production comments and string, byte-string, character, and raw literals
/// are retained byte-for-byte.
///
/// Use this when a policy needle intentionally includes literal data (for
/// example an environment-variable name). Callers that need call syntax only
/// should use [`production_rust_code`], which additionally masks non-code.
pub fn remove_cfg_test_items_preserving_literals(src: &str) -> String {
    remove_spans(src, &cfg_test_item_spans(src))
}

/// Code-only projection for policy censuses: mask non-code and remove every
/// item directly gated by `#[cfg(test)]`, while retaining production items
/// before, between and after them.
///
/// This deliberately handles braced, semicolon-terminated, and comma-ended
/// items. Out-of-line test modules are classified separately by
/// [`derive_cfg_test_module_files`].
pub fn production_rust_code(src: &str) -> String {
    let code = mask_rust_non_code(src);
    remove_spans(&code, &cfg_test_item_spans(src))
}

fn rust_non_code_spans(src: &str) -> Vec<(usize, usize)> {
    let bytes = src.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"//") {
            let start = i;
            i = bytes[i..]
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(bytes.len(), |offset| i + offset);
            spans.push((start, i));
            continue;
        }
        if bytes[i..].starts_with(b"/*") {
            let start = i;
            i += 2;
            let mut depth = 1usize;
            while i < bytes.len() && depth > 0 {
                if bytes[i..].starts_with(b"/*") {
                    depth += 1;
                    i += 2;
                } else if bytes[i..].starts_with(b"*/") {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            spans.push((start, i));
            continue;
        }

        // A literal prefix only starts where an identifier cannot continue;
        // otherwise the `b`, `c` or `r` in an identifier could eat code.
        let fresh = i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
        if fresh {
            let mut quote = i;
            if matches!(bytes[quote], b'b' | b'c') {
                quote += 1;
            }
            let raw = bytes.get(quote) == Some(&b'r');
            if raw {
                quote += 1;
            }
            let hash_start = quote;
            while bytes.get(quote) == Some(&b'#') {
                quote += 1;
            }
            let hashes = quote - hash_start;
            if bytes.get(quote) == Some(&b'"') && (raw || hashes == 0) {
                let start = i;
                i = quote + 1;
                if raw {
                    while i < bytes.len() {
                        if bytes[i] == b'"'
                            && bytes.len() - (i + 1) >= hashes
                            && bytes[i + 1..i + 1 + hashes]
                                .iter()
                                .all(|byte| *byte == b'#')
                        {
                            i += 1 + hashes;
                            break;
                        }
                        i += 1;
                    }
                } else {
                    while i < bytes.len() {
                        if bytes[i] == b'\\' {
                            i = (i + 2).min(bytes.len());
                        } else if bytes[i] == b'"' {
                            i += 1;
                            break;
                        } else {
                            i += 1;
                        }
                    }
                }
                spans.push((start, i));
                continue;
            }
        }

        // Character / byte-character literal versus a lifetime. A plain
        // character has one UTF-8 scalar before its close; an escape is always
        // a literal and is consumed through its closing quote.
        let char_quote = if bytes[i] == b'\'' {
            Some(i)
        } else if fresh && bytes[i] == b'b' && bytes.get(i + 1) == Some(&b'\'') {
            Some(i + 1)
        } else {
            None
        };
        if let Some(quote) = char_quote {
            let after = quote + 1;
            let literal = if bytes.get(after) == Some(&b'\\') {
                true
            } else if after < bytes.len() {
                let scalar_len = src[after..].chars().next().map_or(0, char::len_utf8);
                bytes.get(after + scalar_len) == Some(&b'\'')
            } else {
                false
            };
            if literal {
                let start = i;
                i = after;
                while i < bytes.len() {
                    if bytes[i] == b'\\' {
                        i = (i + 2).min(bytes.len());
                    } else if bytes[i] == b'\'' {
                        i += 1;
                        break;
                    } else {
                        i += 1;
                    }
                }
                spans.push((start, i));
                continue;
            }
        }

        i += src[i..].chars().next().map_or(1, char::len_utf8);
    }
    spans
}

/// `(b)r#*"` at `at` → `(hash count, byte index just past the opening quote)`.
pub fn raw_string_start(b: &[u8], at: usize) -> Option<(usize, usize)> {
    let mut j = at;
    if b[j] == b'b' {
        j += 1;
    }
    if j >= b.len() || b[j] != b'r' {
        return None;
    }
    let hash_start = j + 1;
    let mut hashes = 0usize;
    while hash_start + hashes < b.len() && b[hash_start + hashes] == b'#' {
        hashes += 1;
    }
    if hash_start + hashes < b.len() && b[hash_start + hashes] == b'"' {
        Some((hashes, hash_start + hashes + 1))
    } else {
        None
    }
}

/// Byte index of the closing quote of a character literal opening at `at`, or
/// `None` when `at` starts a lifetime.
pub fn char_literal_end(b: &[u8], at: usize) -> Option<usize> {
    let mut j = at + 1;
    if j >= b.len() {
        return None;
    }
    if b[j] == b'\\' {
        j += 1;
        while j < b.len() && j <= at + 12 {
            if b[j] == b'\'' {
                return Some(j);
            }
            j += 1;
        }
        return None;
    }
    let mut k = j + 1;
    while k < b.len() && (b[k] & 0xC0) == 0x80 {
        k += 1;
    }
    if k < b.len() && b[k] == b'\'' {
        Some(k)
    } else {
        None
    }
}

/// One out-of-line Rust module declaration derived from source.
///
/// `owner` and `root` are `/`-separated paths relative to the scanned source
/// root. `root` omits the `.rs` suffix so callers can resolve either Rust module
/// file shape (`root.rs` or `root/mod.rs`). Attributes are retained in source
/// order, which lets policy gates distinguish an inherited test-only child from
/// a platform-gated child without maintaining their own module parser.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct FileModuleDeclaration {
    /// Declaring source file, relative to the scanned source root.
    pub owner: String,
    /// Rust module identifier from the declaration.
    pub name: String,
    /// Resolved module path without a `.rs` or `/mod.rs` suffix.
    pub root: String,
    /// Outer attributes attached directly to the declaration.
    pub attributes: Vec<String>,
    /// Whether the declaration omits every `pub` visibility qualifier.
    pub is_private: bool,
}

fn module_child_dir(rel: &str) -> String {
    let parent = rel
        .rsplit_once('/')
        .map_or_else(String::new, |(dir, _)| dir.to_owned());
    let file = rel.rsplit('/').next().unwrap_or(rel);
    if matches!(file, "main.rs" | "lib.rs" | "mod.rs") {
        parent
    } else {
        join_mod(&parent, file.strip_suffix(".rs").unwrap_or(file))
    }
}

/// Parse the literal value of a live `#[path = "..."]` attribute.
///
/// The caller invokes this only on a line whose masked projection contains an
/// outer attribute, so a lookalike in a comment or literal cannot arrive
/// here. Rust module paths in this workspace use ordinary string literals;
/// unsupported escape syntax fails closed instead of guessing another file.
fn path_attribute_value(raw: &str) -> Option<String> {
    let marker = raw.find("#[path")?;
    let attribute = &raw[marker..];
    let equals = attribute.find('=')?;
    let value = attribute[equals + 1..].trim_start();
    let value = value.strip_prefix('"')?;
    let mut escaped = false;
    for (index, ch) in value.char_indices() {
        if escaped {
            panic!("escaped #[path] literals are unsupported: {raw:?}");
        }
        match ch {
            '\\' => escaped = true,
            '"' => return Some(value[..index].to_string()),
            _ => {}
        }
    }
    panic!("unterminated #[path] literal: {raw:?}")
}

/// Resolve and lexically normalize a `#[path]` value relative to its declaring
/// file. The result has neither `.rs` nor `/mod.rs`; a leading `..` is retained
/// for an intentional module outside the scanned source root and matches no
/// file in that root's ownership inventory.
fn normalized_relative_source_path(owner: &str, value: &str) -> String {
    use std::path::Component;

    let owner_dir = owner.rsplit_once('/').map_or("", |(dir, _)| dir);
    let mut parts: Vec<String> = owner_dir
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect();
    for component in Path::new(value).components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => parts.push(
                part.to_str()
                    .unwrap_or_else(|| panic!("non-UTF-8 #[path] from {owner}: {value:?}"))
                    .to_owned(),
            ),
            Component::ParentDir => match parts.last().map(String::as_str) {
                Some("..") | None => parts.push("..".to_owned()),
                Some(_) => {
                    parts.pop();
                }
            },
            Component::RootDir | Component::Prefix(_) => {
                panic!("absolute #[path] from {owner} is unsupported: {value:?}")
            }
        }
    }
    let joined = parts.join("/");
    assert!(
        !joined.is_empty(),
        "source path from {owner} resolves to no file"
    );
    joined
}

fn normalized_path_module_root(owner: &str, value: &str) -> String {
    let joined = normalized_relative_source_path(owner, value);
    joined
        .strip_suffix("/mod.rs")
        .or_else(|| joined.strip_suffix(".rs"))
        .unwrap_or(&joined)
        .to_owned()
}

/// Derive every out-of-line `mod name;` edge below a Rust source root.
///
/// Comments and literals are masked by the canonical lexer before parsing;
/// declarations are therefore code, not documentation examples or fixtures.
/// Live `#[path = "..."]` attributes are resolved from their preserved literal
/// and lexically normalized (including `.` and `..`) before `root` is emitted.
/// This deliberately returns both private and visible declarations: consumers
/// enforcing a private suite must fail on visibility drift rather than silently
/// dropping the now-public edge from their population.
pub fn derive_file_module_declarations(src_root: &Path) -> Vec<FileModuleDeclaration> {
    let sources: Vec<(String, String)> = walk_rs_files(src_root)
        .into_iter()
        .map(|owner| {
            let source = read_source_or_panic(&src_root.join(&owner));
            (owner, source)
        })
        .collect();
    derive_file_module_declarations_from_sources(&sources)
}

fn derive_file_module_declarations_from_sources(
    sources: &[(String, String)],
) -> Vec<FileModuleDeclaration> {
    parse_file_module_declarations(sources)
        .into_iter()
        .map(|parsed| parsed.declaration)
        .collect()
}

struct ParsedFileModuleDeclaration {
    declaration: FileModuleDeclaration,
    site_test_only: bool,
}

fn parse_file_module_declarations(
    sources: &[(String, String)],
) -> Vec<ParsedFileModuleDeclaration> {
    let mut declarations = Vec::new();
    for (owner, source) in sources {
        let code = mask_rust_non_code(source);
        let raw_lines: Vec<&str> = source.lines().collect();
        let module_dir = module_child_dir(owner);
        let test_spans = cfg_test_item_spans(source);
        let mut next_line = 0usize;
        let line_offsets: Vec<usize> = code
            .lines()
            .map(|line| {
                let start = next_line;
                next_line += line.len() + 1;
                start
            })
            .collect();
        let mut attributes = Vec::new();
        let mut unfinished_attribute: Option<String> = None;
        let mut unfinished_path_attribute: Option<String> = None;
        let mut redirected_root: Option<String> = None;

        for (line_index, line) in code.lines().enumerate() {
            let mut item = line.trim();
            if let Some(mut raw_attribute) = unfinished_path_attribute.take() {
                raw_attribute.push('\n');
                raw_attribute.push_str(raw_lines[line_index]);
                if item.contains(']') {
                    let value = path_attribute_value(&raw_attribute)
                        .expect("a #[path] attribute must contain one literal");
                    redirected_root = Some(normalized_path_module_root(owner, &value));
                } else {
                    unfinished_path_attribute = Some(raw_attribute);
                }
            } else if let Some(at) = item.find("#[path") {
                if item[at..].contains(']') {
                    let value = path_attribute_value(raw_lines[line_index])
                        .expect("a #[path] attribute must contain one literal");
                    redirected_root = Some(normalized_path_module_root(owner, &value));
                } else {
                    unfinished_path_attribute = Some(raw_lines[line_index].to_owned());
                }
            }
            if let Some(mut attribute) = unfinished_attribute.take() {
                if let Some(close) = item.find(']') {
                    attribute.push(' ');
                    attribute.push_str(&item[..=close]);
                    attributes.push(attribute);
                    item = item[close + 1..].trim();
                } else {
                    attribute.push(' ');
                    attribute.push_str(item);
                    unfinished_attribute = Some(attribute);
                    continue;
                }
            }
            while item.starts_with("#[") {
                if let Some(close) = item.find(']') {
                    attributes.push(item[..=close].trim().to_owned());
                    item = item[close + 1..].trim();
                } else {
                    unfinished_attribute = Some(item.to_owned());
                    item = "";
                    break;
                }
            }
            if item.is_empty() {
                continue;
            }
            if let Some(name) = declared_mod_name(item) {
                let stripped = strip_visibility(item);
                let declaration_at = line_offsets[line_index] + line.len() - item.len();
                declarations.push(ParsedFileModuleDeclaration {
                    declaration: FileModuleDeclaration {
                        owner: owner.clone(),
                        root: redirected_root
                            .take()
                            .unwrap_or_else(|| join_mod(&module_dir, &name)),
                        name,
                        attributes: std::mem::take(&mut attributes),
                        is_private: stripped == item,
                    },
                    site_test_only: test_spans
                        .iter()
                        .any(|(start, end)| declaration_at >= *start && declaration_at < *end),
                });
            } else {
                attributes.clear();
                redirected_root = None;
            }
        }
        assert!(
            unfinished_path_attribute.is_none(),
            "unterminated #[path] attribute in {owner}"
        );
    }
    declarations.sort_by(|left, right| left.declaration.cmp(&right.declaration));
    declarations
}

#[derive(Default)]
struct ModuleReachability {
    production: std::collections::BTreeSet<String>,
    test: std::collections::BTreeSet<String>,
}

fn declaration_targets(
    declaration: &FileModuleDeclaration,
    all: &std::collections::BTreeSet<String>,
) -> Vec<String> {
    [
        format!("{}.rs", declaration.root),
        format!("{}/mod.rs", declaration.root),
    ]
    .into_iter()
    .filter(|candidate| all.contains(candidate))
    .collect()
}

/// Every literal `include!("...")` edge in `source`, with the ownership of the
/// site. An include inside any directly `cfg(test)`-gated item is a test edge,
/// including a directly attributed `include!` and an include nested in an
/// inline test module.
fn literal_include_targets(owner: &str, source: &str) -> Vec<(String, bool)> {
    let code = mask_rust_non_code(source);
    let test_spans = cfg_test_item_spans(source);
    let bytes = code.as_bytes();
    let raw = source.as_bytes();
    let mut targets = Vec::new();
    let mut cursor = 0usize;
    while let Some(offset) = code[cursor..].find("include") {
        let start = cursor + offset;
        let end = start + "include".len();
        cursor = end;
        let boundary = |byte: Option<&u8>| {
            byte.is_none_or(|byte| !byte.is_ascii_alphanumeric() && *byte != b'_')
        };
        if !boundary(bytes.get(start.wrapping_sub(1))) || !boundary(bytes.get(end)) {
            continue;
        }
        let mut at = end;
        while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
            at += 1;
        }
        if bytes.get(at) != Some(&b'!') {
            continue;
        }
        at += 1;
        while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
            at += 1;
        }
        if bytes.get(at) != Some(&b'(') {
            continue;
        }
        at += 1;
        // Stop at the literal's own opening quote. Masking blanks a string's
        // bytes, quotes included, so a whitespace skip driven by the masked
        // projection alone walks straight through the argument and finds no
        // include edge at all — a silent, always-empty extractor. Comments
        // between `(` and the literal are masked to spaces and skipped here.
        while bytes.get(at).is_some_and(u8::is_ascii_whitespace) && raw.get(at) != Some(&b'"') {
            at += 1;
        }
        if raw.get(at) != Some(&b'"') {
            continue;
        }
        let value_start = at + 1;
        let Some(value_len) = raw[value_start..].iter().position(|byte| *byte == b'"') else {
            panic!("unterminated include! literal in {owner}");
        };
        let value = &source[value_start..value_start + value_len];
        assert!(
            !value.contains('\\'),
            "escaped include! literals are unsupported in {owner}: {value:?}"
        );
        let test_only = test_spans
            .iter()
            .any(|(span_start, span_end)| start >= *span_start && start < *span_end);
        targets.push((normalized_relative_source_path(owner, value), test_only));
    }
    targets
}

/// Derive production and test reachability from actual out-of-line module
/// edges. Literal `include!("...")` source edges are followed as well, since
/// split test suites use them beneath a test-owned module. Files with no
/// incoming source edge are conservative production
/// roots: this preserves `include!` fragments and otherwise-unattached source
/// instead of guessing ownership from a basename or directory name.
fn derive_module_reachability(src_root: &Path) -> ModuleReachability {
    let sources: Vec<(String, String)> = walk_rs_files(src_root)
        .into_iter()
        .map(|owner| {
            let source = read_source_or_panic(&src_root.join(&owner));
            (owner, source)
        })
        .collect();
    derive_module_reachability_from_sources(&sources)
}

fn is_cargo_source_root(path: &str) -> bool {
    path == "lib.rs"
        || path == "main.rs"
        || path
            .strip_prefix("bin/")
            .is_some_and(|tail| tail.ends_with(".rs") && !tail.contains('/'))
}

fn derive_module_reachability_from_sources(sources: &[(String, String)]) -> ModuleReachability {
    let declarations = parse_file_module_declarations(sources);
    let all: std::collections::BTreeSet<String> =
        sources.iter().map(|(owner, _)| owner.clone()).collect();
    let mut edges: Vec<(String, Vec<String>, bool)> = declarations
        .iter()
        .map(|parsed| {
            let declaration = &parsed.declaration;
            let test_only = parsed.site_test_only
                || declaration
                    .attributes
                    .iter()
                    .any(|attribute| attribute_gates_on_cfg_test(attribute));
            (
                declaration.owner.clone(),
                declaration_targets(declaration, &all),
                test_only,
            )
        })
        .collect();
    for (owner, source) in sources {
        for test_only in [false, true] {
            let targets = literal_include_targets(owner, source)
                .into_iter()
                .filter(|(target, site_test_only)| {
                    *site_test_only == test_only && all.contains(target)
                })
                .map(|(target, _)| target)
                .collect::<Vec<_>>();
            if !targets.is_empty() {
                edges.push((owner.clone(), targets, test_only));
            }
        }
    }
    let incoming: std::collections::BTreeSet<String> = edges
        .iter()
        .flat_map(|(_, targets, _)| targets.iter().cloned())
        .collect();

    let cargo_roots = all
        .iter()
        .filter(|path| is_cargo_source_root(path))
        .cloned();
    let mut reachability = ModuleReachability {
        // Files with no incoming edge are retained conservatively. Cargo entry
        // points are production roots even if a malformed or synthetic graph
        // also points to one through a test-only edge.
        production: all
            .difference(&incoming)
            .cloned()
            .chain(cargo_roots)
            .collect(),
        test: edges
            .iter()
            .filter(|(_, _, test_only)| *test_only)
            .flat_map(|(_, targets, _)| targets.iter().cloned())
            .collect(),
    };

    loop {
        let before = (reachability.production.len(), reachability.test.len());
        for (owner, targets, test_only) in &edges {
            if reachability.test.contains(owner) {
                reachability.test.extend(targets.iter().cloned());
            }
            if !test_only && reachability.production.contains(owner) {
                reachability.production.extend(targets.iter().cloned());
            }
        }
        if before == (reachability.production.len(), reachability.test.len()) {
            break;
        }
    }
    reachability
}

/// Every `.rs` file under `src_root` reachable through a test-only module edge
/// and through no production module path. Paths are relative to `src_root` and
/// `/`-separated.
///
/// Reachability follows actual conventional, `#[path]`, and literal
/// `include!` source edges; it never assigns ownership to every physical file
/// below a module directory.
/// A file reached by both test and production edges is production, even when it
/// lives below a test-owned module's directory. Unattached files are retained
/// conservatively as production (including `include!` fragments and
/// production files with names such as `tests.rs`).
///
/// The gating attribute is read with [`attribute_gates_on_cfg_test`], the same
/// predicate the inline-module scanner and the item projection use, so
/// `#[cfg(all(test, unix))] mod tests;` claims its file exactly like
/// `#[cfg(test)] mod tests;` while `#[cfg(any(test, feature = "x"))]` and
/// `#[cfg(feature = "contest")]` declare production modules and keep theirs in
/// the swept population.
pub fn derive_cfg_test_module_files(src_root: &Path) -> std::collections::BTreeSet<String> {
    let reachability = derive_module_reachability(src_root);
    test_owned_files(reachability)
}

/// [`derive_cfg_test_module_files`] over in-memory `(relative path, source)`
/// pairs instead of a directory.
#[must_use]
pub fn derive_cfg_test_module_files_from_sources(
    sources: &[(String, String)],
) -> std::collections::BTreeSet<String> {
    test_owned_files(derive_module_reachability_from_sources(sources))
}

fn test_owned_files(reachability: ModuleReachability) -> std::collections::BTreeSet<String> {
    reachability
        .test
        .difference(&reachability.production)
        .cloned()
        .collect()
}

/// Every Rust source file that is not reachable only through a `#[cfg(test)]`
/// module edge. Classification is structural: a production `include!` fragment
/// named `tests.rs` remains included, while a test-only module is excluded no
/// matter what its file is called.
pub fn production_rs_files(src_root: &Path) -> Vec<String> {
    let test_owned = derive_cfg_test_module_files(src_root);
    walk_rs_files(src_root)
        .into_iter()
        .filter(|rel| !test_owned.contains(rel))
        .collect()
}

/// Every Rust source file reachable from a crate root (`main.rs` /
/// `lib.rs`) through a production module edge, in ascending sorted order.
/// Paths are relative to `src_root`, `/`-separated.
///
/// Unlike [`production_rs_files`], this drops orphan files that no
/// production `mod` / `#[path]` / `include!` edge reaches: a `.rs` file
/// present on disk but not attached to the module tree is not shipped, and
/// must not be counted as production population by a source-policy gate
/// that would otherwise inflate call-site or file-count floors with
/// abandoned scratch. Test-only files are also excluded (they are
/// reachable only through `#[cfg(test)]` edges).
pub fn production_reachable_rs_files(src_root: &Path) -> Vec<String> {
    let sources: Vec<(String, String)> = walk_rs_files(src_root)
        .into_iter()
        .map(|owner| {
            let source = read_source_or_panic(&src_root.join(&owner));
            (owner, source)
        })
        .collect();
    production_reachable_from_sources(&sources)
}

/// Sibling of [`production_reachable_rs_files`] taking `(owner, source)`
/// pairs directly, without touching the filesystem. Enables the unit
/// test that verifies strict reachability against synthetic sources.
pub fn production_reachable_from_sources(sources: &[(String, String)]) -> Vec<String> {
    use std::collections::BTreeSet;
    let all: BTreeSet<String> = sources.iter().map(|(owner, _)| owner.clone()).collect();
    let declarations = parse_file_module_declarations(sources);
    let mut edges: Vec<(String, Vec<String>)> = declarations
        .iter()
        .filter(|parsed| {
            !(parsed.site_test_only
                || parsed
                    .declaration
                    .attributes
                    .iter()
                    .any(|attribute| attribute_gates_on_cfg_test(attribute)))
        })
        .map(|parsed| {
            (
                parsed.declaration.owner.clone(),
                declaration_targets(&parsed.declaration, &all),
            )
        })
        .collect();
    for (owner, source) in sources {
        let targets: Vec<String> = literal_include_targets(owner, source)
            .into_iter()
            .filter(|(target, site_test_only)| !*site_test_only && all.contains(target))
            .map(|(target, _)| target)
            .collect();
        if !targets.is_empty() {
            edges.push((owner.clone(), targets));
        }
    }
    let mut reached: BTreeSet<String> = all
        .iter()
        .filter(|path| is_cargo_source_root(path))
        .cloned()
        .collect();
    loop {
        let before = reached.len();
        for (owner, targets) in &edges {
            if reached.contains(owner) {
                reached.extend(targets.iter().cloned());
            }
        }
        if reached.len() == before {
            break;
        }
    }
    let mut out: Vec<String> = reached.into_iter().collect();
    out.sort();
    out
}

/// Absolute production Rust paths below a source root that may itself be a
/// symlink. Root canonicalization is fail-closed and happens before module
/// resolution, keeping symlink and `..` normalization out of policy adapters.
pub fn production_rs_paths_follow_root(src_root: &Path) -> Vec<PathBuf> {
    let canonical = src_root.canonicalize().unwrap_or_else(|error| {
        panic!(
            "canonicalize production source root {}: {error}",
            src_root.display()
        )
    });
    production_rs_files(&canonical)
        .into_iter()
        .map(|relative| canonical.join(relative))
        .collect()
}

// ---------------------------------------------------------------------------
// Rust attribute derivation
// ---------------------------------------------------------------------------

/// One outer attribute derived from masked source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RustAttribute {
    /// The attribute text including `#[` and `]`.
    pub text: String,
    /// 1-based line number of the attribute's opening `#[`.
    pub line: usize,
}

/// Compact an attribute by collapsing interior whitespace.
fn compact_attribute(attribute: &str) -> String {
    let mut out = String::with_capacity(attribute.len());
    let mut prev_ws = false;
    for ch in attribute.chars() {
        if ch.is_whitespace() {
            if !prev_ws && !out.is_empty() {
                out.push(' ');
            }
            prev_ws = true;
        } else {
            out.push(ch);
            prev_ws = false;
        }
    }
    out.trim_end().to_owned()
}

/// Scan contiguous outer attributes starting at `lines[start]`.
///
/// Returns the collected attributes and the index of the first non-attribute
/// line following the run.
fn scan_attribute_run(lines: &[&str], start: usize) -> (Vec<RustAttribute>, usize) {
    let mut attributes = Vec::new();
    let mut i = start;
    while i < lines.len() {
        let trimmed = lines[i].trim();
        if !trimmed.starts_with("#[") {
            break;
        }
        let open_line = i;
        if trimmed.contains(']') {
            attributes.push(RustAttribute {
                text: compact_attribute(trimmed),
                line: open_line + 1,
            });
            i += 1;
        } else {
            // Multi-line attribute: accumulate until `]`.
            let mut raw = trimmed.to_owned();
            i += 1;
            while i < lines.len() {
                let part = lines[i].trim();
                raw.push(' ');
                raw.push_str(part);
                i += 1;
                if part.contains(']') {
                    break;
                }
            }
            attributes.push(RustAttribute {
                text: compact_attribute(&raw),
                line: open_line + 1,
            });
        }
    }
    (attributes, i)
}

/// Derive every outer attribute in `source`, ignoring comments and literals.
pub fn derive_rust_attributes(source: &str) -> Vec<RustAttribute> {
    let masked = mask_rust_non_code(source);
    let lines: Vec<&str> = masked.lines().collect();
    let mut attributes = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim();
        if trimmed.starts_with("#[") {
            let (run, next) = scan_attribute_run(&lines, i);
            attributes.extend(run);
            i = next;
        } else {
            i += 1;
        }
    }
    attributes
}

// Later items on the same line do not change the declaration's module ownership.
fn declared_mod_name(line: &str) -> Option<String> {
    let line = strip_visibility(line.trim()).trim();
    let rest = line.strip_prefix("mod ")?;
    let name = rest.split_once(';')?.0.trim();
    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    Some(name.to_owned())
}

fn join_mod(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

/// Strip a leading Rust visibility qualifier (`pub`, `pub(crate)`,
/// `pub(super)`, `pub(in path)`) from an item's source text.
///
/// Load-bearing for [`inline_test_module_spans`]: real code writes
/// `#[cfg(test)] pub mod tests { .. }` (for example a test module that exposes
/// a `backend_construction_is_forced_to_fail()` switch the production code
/// reads). A rule that only matched a bare `mod ` would treat those as
/// *production* code and scan the whole test module for banned needles — the
/// mirror-image false positive of the truncation bug.
fn strip_visibility(item: &str) -> &str {
    let item = item.trim_start();
    let Some(rest) = item.strip_prefix("pub") else {
        return item;
    };
    // `pub` must be a whole token: `public_thing` is not a visibility.
    match rest.chars().next() {
        Some('(') => {
            // `pub(crate)`, `pub(super)`, `pub(in a::b)` — skip to the `)`.
            match rest.find(')') {
                Some(close) => rest[close + 1..].trim_start(),
                None => item,
            }
        }
        Some(c) if c.is_whitespace() => rest.trim_start(),
        _ => item,
    }
}

/// The item text a `cfg(test)` attribute is attached to, plus the index of
/// the line that text starts on.
///
/// `attr_end_line` is the line the attribute's closing `]` sits on (an
/// attribute may wrap across lines) and `rest` is what follows it on that
/// line, in the masked projection.
///
/// This is the whole point of the attachment fix: the old parser peeked one
/// non-blank line and asked only "does it end in `;`?". Three shapes defeat
/// that and all three are present in this workspace:
///
/// * a trailing comment on the attribute line itself —
///   `#[cfg(test)] // TEST-ONLY; ..` (a common spelling in real
///   code); in the masked projection such a comment is
///   blank, so it can never be mistaken for the item,
/// * an intervening attribute — `#[cfg(test)]` then
///   (seen in real code), or an intervening doc comment,
/// * an item that simply is not a module: a struct field, an enum
///   variant, a method, an `impl` block, a `type` alias, a `use`.
///
/// Returns `None` if the attribute is the last thing in the file.
fn cfg_test_item<'a>(
    lines: &[&'a str],
    attr_end_line: usize,
    rest: &'a str,
) -> Option<(usize, &'a str)> {
    let rest = rest.trim();
    // `#[cfg(test)] mod tests {` — the item is on the attribute's own line.
    // A trailing `//` comment is blanked by the mask; fall through to the scan.
    if !rest.is_empty() && !rest.starts_with("//") {
        return Some((attr_end_line, rest));
    }
    let mut j = attr_end_line + 1;
    while j < lines.len() {
        let s = lines[j].trim();
        // Blank lines, doc comments, ordinary comments and *further attributes*
        // all sit between an attribute and the item it decorates.
        if s.is_empty() || s.starts_with("//") || s.starts_with("#[") || s.starts_with("#![") {
            j += 1;
            continue;
        }
        return Some((j, s));
    }
    None
}

/// True if `item` is an inline test module — `mod NAME {` — as opposed to
/// a module *reference* (`mod NAME;`) or any other item.
fn is_inline_test_module(item: &str) -> bool {
    let item = strip_visibility(item);
    let Some(rest) = item.strip_prefix("mod") else {
        return false;
    };
    // `mod` must be a whole token — `modifier_state: u8,` is a struct field.
    if !rest.starts_with(|c: char| c.is_whitespace()) {
        return false;
    }
    // A module *reference* ends in `;` and has no body to excise.
    !rest.trim_end().ends_with(';')
}

/// Byte spans `[start, end)` of every inline `#[cfg(test)] mod NAME { .. }` in
/// `src`, from the `#[cfg(test)]` attribute line through one past the module's
/// closing `}`. Spans are returned in source order and never overlap (a nested
/// `#[cfg(test)] mod` inside an outer one is swallowed by the outer span).
///
/// # Why not a one-line peek
///
/// A rule such as "the first `#[cfg(test)]` whose next non-blank line does not
/// end in `;`" ends the scan at a test-only struct field, enum variant,
/// method, `impl` block, `type` alias or `use`, hiding every line below it.
/// A single test-only field near the top of a large `main.rs` would leave the
/// hand-roll sweeps reading a few percent of the file.
///
/// # Why not `syn`
///
/// A `syn` parse would *know* what an attribute decorates instead of guessing,
/// but it costs more than it buys:
///
/// * `syn` with the `full` feature is a non-optional new dependency of
///   `enzui-core` (this is a `pub fn`, not a dev-only helper), so every
///   consumer — all apps and the widgets crate — would link
///   `syn` + `proc-macro2` + `quote` + `unicode-ident` for a source sweep.
/// * `syn::parse_file` is all-or-nothing: a file using a nightly or
///   not-yet-supported syntax fails to parse and the guard would have to
///   choose between "scan nothing" (a check that cannot fail) and "fall back
///   to the string sweep", which means keeping this code anyway.
/// * The guards downstream of this are themselves `contains("..")` string
///   sweeps over the projection. A precise AST boundary feeding an imprecise
///   needle buys much less than it costs.
///
/// What makes the string version defensible is that the ambiguity is
/// bounded and named: the only construct that ends production code is the
/// `mod` keyword, every other item is skipped over rather than guessed at, and
/// each construct has its own fixture in this module's tests.
///
/// # Which attributes count
///
/// The gate is [`leading_cfg_attribute`], the shared `cfg(test)` predicate:
/// `#[cfg(test)]` and any compound that cannot hold without `test`
/// (`#[cfg(all(test, target_os = "linux"))]`, `#[cfg(all(unix, any(test)))]`)
/// end production code. `#[cfg(any(test, unix))]`, `#[cfg(feature =
/// "contest")]` and `#[cfg_attr(test, ..)]` do not: those items exist in a
/// non-test build. Files in this workspace gate an inline test *module*
/// compoundly (`enzui-core`'s `process_transport.rs`, `enzui-security`'s
/// `secret.rs` and `random.rs`); unrecognised, their test bodies would be
/// projected as production by every consumer of this scanner.
/// Further files gate individual *items* the same way (`enzui-resources`'s
/// `http_cache.rs`, `enzui-core`'s
/// `credential_store.rs`, `enzui-widgets`'s `lib.rs`); those are excised by
/// the `cfg(test)` item projection ([`production_rust_code`]), not by this
/// scanner.
///
/// # Known limits
///
/// Boundaries are found in [`mask_rust_non_code`], so a brace, a `#[cfg(`
/// spelling or a `mod` keyword written inside a string, a character literal or
/// a (nested) block comment is data and can neither open nor close a span. An
/// attribute the predicate parser does not understand keeps its item in
/// production, which is the safe direction: a false positive is a visible
/// hand-roll finding, a false negative is code no sweep ever reads.
pub fn inline_test_module_spans(src: &str) -> Vec<(usize, usize)> {
    // Byte offsets survive masking, so spans computed here index `src`.
    let code = mask_rust_non_code(src);
    let lines: Vec<&str> = code.lines().collect();
    // Byte offset of the start of each line, plus one past the end.
    let mut offsets = Vec::with_capacity(lines.len() + 1);
    let mut acc = 0usize;
    for l in &lines {
        offsets.push(acc);
        acc += l.len() + 1;
    }
    offsets.push(src.len());
    let line_start = |i: usize| offsets[i].min(src.len());
    let line_end = |i: usize| (offsets[i] + lines[i].len()).min(src.len());

    let mut spans = Vec::new();
    let mut i = 0usize;
    while i < lines.len() {
        let trimmed = lines[i].trim_start();
        if !trimmed.starts_with("#[") {
            i += 1;
            continue;
        }
        let attribute_at = line_start(i) + (lines[i].len() - trimmed.len());
        let Some(attribute) = leading_cfg_attribute(&code[attribute_at..]) else {
            i += 1;
            continue;
        };
        if !attribute.gates_on_test {
            i += 1;
            continue;
        }
        // A wrapped `#[cfg(all(\n    test,\n    unix\n))]` ends on a later line
        // than it starts; the item it decorates follows the closing `]`.
        let attribute_end = attribute_at + attribute.len;
        let end_line = i + code[attribute_at..attribute_end].matches('\n').count();
        let rest = &code[attribute_end..line_end(end_line).max(attribute_end)];
        let Some((item_line, item)) = cfg_test_item(&lines, end_line, rest) else {
            break;
        };
        if !is_inline_test_module(item) {
            i += 1;
            continue;
        }
        // Brace-match from the item line to the module's closing `}`.
        // Brace-match to the module's closing `}`. `end` is the byte offset
        // ONE PAST that brace — not one past its line, so a trailing `//`
        // comment or a second item on the same line stays in production.
        let mut depth = 0i32;
        let mut opened = false;
        let mut k = item_line;
        let mut end = None;
        let mut end_line = item_line;
        'outer: while k < lines.len() {
            for (col, ch) in lines[k].char_indices() {
                match ch {
                    '{' => {
                        depth += 1;
                        opened = true;
                    }
                    '}' => {
                        depth -= 1;
                        if opened && depth <= 0 {
                            end = Some((line_start(k) + col + ch.len_utf8()).min(src.len()));
                            end_line = k;
                            break 'outer;
                        }
                    }
                    _ => {}
                }
            }
            k += 1;
        }
        match end {
            Some(e) => {
                spans.push((line_start(i), e));
                i = end_line + 1;
            }
            // Unbalanced: the module runs to EOF. Excise the rest.
            None => {
                spans.push((line_start(i), src.len()));
                break;
            }
        }
    }
    spans
}

/// Everything in `src` before the first *inline test module* (`#[cfg(test)]
/// mod X { .. }`).
///
/// See [`inline_test_module_spans`] for the boundary rule and why it is not a
/// `syn` parse. In short: only the `mod` keyword ends production code. A
/// `#[cfg(test)]` on a struct field, an enum variant, a method, an `impl`
/// block, a `type` alias or a `use` is skipped over; a `#[cfg(test)] mod X;`
/// module *reference* is skipped because it has no inline body.
///
/// **Prefer [`production_code`]** for new callers. This function truncates at
/// the *first* test module, so production code written between two test
/// modules, or below the last one, is discarded with them. It is kept because
/// it returns a borrowed `&str` and several callers depend on that.
pub fn production_only(src: &str) -> &str {
    match inline_test_module_spans(src).first() {
        Some(&(start, _)) => &src[..start],
        None => src,
    }
}

/// `src` with every inline `#[cfg(test)] mod X { .. }` excised, keeping
/// production code that sits between or below test modules.
///
/// This is the projection the hand-roll guards should scan.
/// [`production_only`] truncates at the first test module, which drops code
/// between two test modules and code below the last one.
///
/// Excision closes both, and the excised region is exactly the union of the
/// test-module spans, so nothing inside a `#[cfg(test)] mod` is ever scanned.
pub fn production_code(src: &str) -> String {
    let spans = inline_test_module_spans(src);
    if spans.is_empty() {
        return src.to_owned();
    }
    let mut out = String::with_capacity(src.len());
    let mut cursor = 0usize;
    for (start, end) in spans {
        if start > cursor {
            out.push_str(&src[cursor..start]);
        }
        cursor = cursor.max(end);
    }
    if cursor < src.len() {
        out.push_str(&src[cursor..]);
    }
    out
}

/// `needle` occurs in `haystack` bounded by non-identifier characters.
///
/// Empty needles never match. Boundaries exclude ASCII letters, digits, and
/// underscores; this is a lexical check, not Unicode identifier parsing.
pub fn contains_ident_word(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let bytes = haystack.as_bytes();
    let mut from = 0usize;
    while let Some(offset) = haystack.get(from..).and_then(|rest| rest.find(needle)) {
        let start = from + offset;
        let end = start + needle.len();
        let before_ok = start == 0 || !is_ident_byte(bytes[start - 1]);
        let after_ok = end == bytes.len() || !is_ident_byte(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
    }
    false
}

/// A byte that may appear inside a Rust identifier.
fn is_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Item names declared `pub` at any indentation in `src`, over the shared
/// [`production_code`] + [`strip_comments`] projection, so a name that only
/// appears in a doc comment or an inline `#[cfg(test)]` module is not
/// mistaken for public API.
///
/// Covers `pub struct|enum|const|fn|type|trait|static`. Deliberately
/// syntactic: this is the same lexical projection every other gate in the
/// workspace reads, not a second parser with its own idea of the language.
pub fn declared_pub_items(src: &str) -> std::collections::BTreeSet<String> {
    const PREFIXES: [&str; 7] = [
        "pub struct ",
        "pub enum ",
        "pub const ",
        "pub fn ",
        "pub type ",
        "pub trait ",
        "pub static ",
    ];
    let code = production_code(&strip_comments(src));
    let mut out = std::collections::BTreeSet::new();
    for raw in code.lines() {
        let line = raw.trim();
        for prefix in PREFIXES {
            let Some(rest) = line.strip_prefix(prefix) else {
                continue;
            };
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                out.insert(name);
            }
        }
    }
    out
}

/// `pub mod <name>;` declarations in `src` (file modules only, not inline
/// `pub mod x { .. }`), over the same production projection.
pub fn declared_pub_mods(src: &str) -> std::collections::BTreeSet<String> {
    let code = production_code(&strip_comments(src));
    code.lines()
        .filter_map(|raw| {
            raw.trim()
                .strip_prefix("pub mod ")
                .and_then(|rest| rest.strip_suffix(';'))
                .map(|name| name.trim().to_owned())
                .filter(|name| !name.is_empty())
        })
        .collect()
}

/// Variant names of `pub enum <name>` declared in `src`, or `None` when the
/// enum is not declared there.
///
/// Returning `Option` rather than an empty set is load-bearing for callers
/// that gate on the enum's existence: an empty set and "no such enum" report
/// the same clean verdict otherwise.
pub fn declared_enum_variants(src: &str, name: &str) -> Option<std::collections::BTreeSet<String>> {
    let code = production_code(&strip_comments(src));
    let header = format!("pub enum {name} {{");
    let start = code.find(&header)? + header.len();
    let body_end = code[start..].find('}')? + start;
    let mut out = std::collections::BTreeSet::new();
    for raw in code[start..body_end].lines() {
        let line = raw.trim().trim_end_matches(',');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let variant: String = line
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !variant.is_empty() {
            out.insert(variant);
        }
    }
    Some(out)
}

/// Byte offsets **one past the closing `}`** of every inline
/// `#[cfg(test)]` module, derived from [`inline_test_module_spans`].
///
/// It backs the
/// `no_banned_needle_hides_below_the_last_inline_test_module` net,
/// which is a belt-and-braces second opinion on the tail of each
/// scanned file.
pub fn inline_test_module_ends(src: &str) -> Vec<usize> {
    inline_test_module_spans(src)
        .into_iter()
        .map(|(_, end)| end)
        .collect()
}

/// Count bordered `container::Style` struct literals whose body
/// includes a `border:` field assignment.
///
/// A `container::Style` that sets only `background` (with
/// `..Default::default()` for the rest) is not counted: it is a plain
/// background fill, not the bordered-surface defect class.
///
/// Promoted from the ten identical per-app copies.
pub fn count_bordered_container_styles(code: &str) -> usize {
    let needle: String = ["container", "::Style {"].concat();
    let bytes = code.as_bytes();
    let mut count = 0;
    let mut start = 0;
    while let Some(rel) = code[start..].find(&needle) {
        let abs = start + rel;
        let brace = abs + needle.len() - 1;
        let mut depth = 1i32;
        let mut k = brace + 1;
        while k < bytes.len() && depth > 0 {
            match bytes[k] {
                b'{' => depth += 1,
                b'}' => depth -= 1,
                _ => {}
            }
            k += 1;
        }
        let body_end = if depth == 0 { k - 1 } else { bytes.len() };
        let body = &code[brace + 1..body_end];
        if body.contains("border") {
            count += 1;
        }
        start = abs + needle.len();
    }
    count
}

/// True if `code` contains at least one bordered `container::Style` hand-roll.
///
/// Promoted from the ten identical per-app copies.
pub fn has_bordered_container_style(code: &str) -> bool {
    count_bordered_container_styles(code) > 0
}

/// Extract the body of the function whose signature matches `signature`.
///
/// Returns everything from the opening `{` through the matching closing `}`.
/// This is a simple brace-matching projection, shared here so callers do not
/// keep their own copies.
pub fn fn_body(src: &str, signature: &str) -> String {
    let start = src
        .find(signature)
        .unwrap_or_else(|| panic!("fn_body: no function starting {signature:?} in this file"));
    let open = start
        + src[start..]
            .find('{')
            .expect("a function signature is followed by a body");
    let mut depth = 0usize;
    for (i, c) in src[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return src[open..=open + i].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("fn_body: unbalanced braces after {signature:?}");
}

/// True if the production half of `src` (comments stripped) contains the
/// view marker `Element<`.
///
/// `Element<` is the discriminator rather than `iced::widget` because the
/// latter also matches non-chrome type aliases such as
/// `iced::widget::image::Handle` and `iced::widget::text_editor::Content`,
/// which pure-data modules legitimately import. `Element<` appears only in the
/// return type / type alias of a function that actually builds a widget tree —
/// i.e. exactly the files the anti-hand-roll guard must cover.
pub fn is_view_ish(src: &str) -> bool {
    let marker: String = ["Element", "<"].concat();
    strip_comments(&production_code(src)).contains(&marker)
}

// Keep the established hand-roll-scan API while the filesystem implementation
// lives in a small module that source-gate integration targets can include
// without pulling this module's parser and test tree into their crates.
pub use crate::strict_fs::{
    optional_dir_exists, optional_file_exists, optional_metadata, read_dir_sorted,
    read_source_or_panic, required_metadata, walk_files_with_ext, walk_files_with_ext_follow_root,
    walk_rs_files, walk_rs_files_follow_root,
};

/// How much of one file survives the production projection.
///
/// The completeness gate answers *which files are scanned* and
/// never *how much of each file survives the strip*. A guard reporting
/// "main.rs is covered" while 98% of main.rs is discarded is the same failure
/// one level down.
#[derive(Debug, Clone)]
pub struct ScanDepth {
    /// The file, relative to the scan root, `/`-separated.
    pub file: String,
    /// Non-blank lines in the file as written.
    pub total_lines: usize,
    /// Non-blank lines the production projection ([`production_code`]) keeps —
    /// i.e. the lines the hand-roll needles actually run over.
    pub scanned_lines: usize,
}

impl ScanDepth {
    /// Fraction of the file's non-blank lines the sweep sees, in `0.0..=1.0`.
    /// An empty file reports `1.0` (nothing to miss).
    pub fn ratio(&self) -> f64 {
        if self.total_lines == 0 {
            1.0
        } else {
            self.scanned_lines as f64 / self.total_lines as f64
        }
    }
}

/// The outcome of walking a source tree and comparing the view-ish files
/// found against the guard's declared scanned set.
#[derive(Debug, Clone)]
pub struct CoverageReport {
    /// Total number of `.rs` files walked under the scan root (before any
    /// exclusion). Used to floor the walk so a mis-rooted walker that matches
    /// nothing cannot masquerade as a clean pass.
    pub total_rs_files: usize,
    /// Every production-owned, view-ish `.rs` file found (relative to the
    /// scan root, `/`-separated), excluding caller-selected files such as the
    /// guard itself.
    pub view_ish: Vec<String>,
    /// The view-ish files that are not present in the declared scanned set.
    /// A non-empty vec means the guard has drifted out of coverage.
    pub unscanned_view_ish: Vec<String>,
}

/// Walk `manifest_dir/src` (the app crate's source tree) and build a
/// [`CoverageReport`] comparing the view-ish files found against `scanned`.
///
/// * `manifest_dir` must be the calling guard crate's `env!("CARGO_MANIFEST_DIR")`
///   — a compile-time constant, so this is robust to whatever CWD the test
///   harness runs under.
/// * `scanned` is the set of paths the guard already `include_str!`s, expressed
///   relative to the scan root with `/` separators (e.g. `"view/shell.rs"`).
///   Extra entries in `scanned` that do not exist on disk are ignored here (the
///   `include_str!` itself is the compile-time check that they exist).
/// * `exclude` is a predicate over the relative path returning `true` for files
///   that must never be required (the guard file itself, legacy test filename
///   conventions, etc.). It is absolute: a caller that excludes a path keeps it
///   out of the population whatever the module graph says. Files reached only
///   through a `#[cfg(test)]` module or `include!` edge are derived
///   structurally and excluded without relying on this predicate.
///
/// Files whose production half does not build an `Element<` are not view-ish
/// and are silently allowed to be unscanned. [`CoverageReport::total_rs_files`]
/// still counts physically present test-owned files, so ownership filtering
/// cannot weaken the walk floor.
pub fn coverage_report<F>(manifest_dir: &str, scanned: &[&str], exclude: F) -> CoverageReport
where
    F: Fn(&str) -> bool,
{
    let src_root = Path::new(manifest_dir).join("src");
    let all = walk_rs_files(&src_root);
    let total_rs_files = all.len();
    let test_owned = derive_cfg_test_module_files(&src_root);

    let scanned_set: std::collections::BTreeSet<&str> = scanned.iter().copied().collect();

    let mut view_ish = Vec::new();
    let mut unscanned_view_ish = Vec::new();
    for rel in &all {
        if test_owned.contains(rel) || exclude(rel) {
            continue;
        }
        let full = src_root.join(rel);
        // Fail closed — an unreadable view file must not read as
        // "not view-ish" and silently drop out of the completeness population.
        let src = read_source_or_panic(&full);
        if is_view_ish(&src) {
            view_ish.push(rel.clone());
            if !scanned_set.contains(rel.as_str()) {
                unscanned_view_ish.push(rel.clone());
            }
        }
    }

    CoverageReport {
        total_rs_files,
        view_ish,
        unscanned_view_ish,
    }
}

impl CoverageReport {
    /// Assert the walk actually reached the source tree. Panics (reddening the
    /// test) if fewer than `min_total` `.rs` files were walked or fewer than
    /// `min_view_ish` view-ish files were classified.
    ///
    /// This is the floor the module docs describe: a walker that resolves to
    /// the wrong root matches zero files and would otherwise report an empty
    /// `unscanned_view_ish` — indistinguishable from a genuinely complete
    /// guard. Pin both counts a comfortable margin below the real numbers so
    /// deleting real files or breaking the walker reddens, but adding one file
    /// does not need the floor bumped.
    pub fn assert_floor(&self, app: &str, min_total: usize, min_view_ish: usize) {
        assert!(
            self.total_rs_files >= min_total,
            "completeness floor for {app}: walked only {} `.rs` file(s) under \
             src/, but expected at least {min_total}. The tree walker almost \
             certainly resolved to the wrong root (a mis-rooted walk matches \
             nothing and looks identical to a clean pass). Check the \
             CARGO_MANIFEST_DIR passed to coverage_report.",
            self.total_rs_files
        );
        assert!(
            self.view_ish.len() >= min_view_ish,
            "completeness floor for {app}: classified only {} view-ish \
             file(s), but expected at least {min_view_ish}. Either the walker is \
             mis-rooted or the `Element<` view-ish discriminator stopped \
             matching — either way the completeness check is dark. Found: {:?}",
            self.view_ish.len(),
            self.view_ish
        );
    }

    /// Assert every view-ish file is in the guard's scanned set. Panics
    /// (reddening the test) with the drifted file list if any view-ish file is
    /// unscanned.
    pub fn assert_complete(&self, app: &str) {
        assert!(
            self.unscanned_view_ish.is_empty(),
            "coverage drift in {app}: {} view source file(s) build an \
             `Element<` in production but are NOT in the guard's scanned_files() \
             list, so the anti-hand-roll sweep never scans them. A raw \
             `iced::widget::button(`, `Color::from_rgb(` or bordered \
             `container::Style {{ }}` could land in any of these green. Add each \
             to scanned_files(): {:?}",
            self.unscanned_view_ish.len(),
            self.unscanned_view_ish
        );
    }
}

/// Measure how much of each `(name, contents)` source the production
/// projection actually keeps.
///
/// Pass the guard's own `scanned_files()` slice. Counts are of non-blank
/// lines so reformatting does not move the number.
pub fn scan_depths(sources: &[(&str, &str)]) -> Vec<ScanDepth> {
    let nonblank = |s: &str| s.lines().filter(|l| !l.trim().is_empty()).count();
    sources
        .iter()
        .map(|(name, src)| ScanDepth {
            file: (*name).to_owned(),
            total_lines: nonblank(src),
            scanned_lines: nonblank(&production_code(src)),
        })
        .collect()
}

/// Assert no scanned file has less than `min_ratio` of its non-blank lines
/// surviving the production projection.
///
/// # The hole this closes
///
/// `CoverageReport::assert_complete` answers *"is every view file in the
/// list?"* and never *"how much of each listed file does the sweep read?"*.
/// Both questions have to be answered or the gate is green while dark: a
/// correctly listed `main.rs` whose projection keeps a few percent of its lines
/// hides every hand-roll below the cut.
///
/// This is a ratchet in the safe direction: it reddens when the *scanned*
/// fraction falls, which can only happen if the projection starts discarding
/// production code. It does not redden when a
/// maintainer adds tests (that raises `total_lines`, but a big test module is
/// excised as a unit and legitimately lowers the ratio, which is why the floor
/// is a low bound and not a pinned number).
///
/// Pick `min_ratio` a comfortable margin below the app's measured minimum;
/// 0.20 catches truncation (which yields a few percent) without
/// firing on a test-heavy but correctly-projected file.
pub fn assert_scan_depth_floor(app: &str, sources: &[(&str, &str)], min_ratio: f64) {
    for d in scan_depths(sources) {
        assert!(
            d.ratio() >= min_ratio,
            "SCAN DEPTH floor in {app}: the production projection keeps \
             only {}/{} non-blank lines ({:.1}%) of `{}`, below the {:.1}% \
             floor. The hand-roll sweep therefore reads a fraction of a file \
             the completeness gate reports as `covered` — a guard that is \
             green because it is blind. A `#[cfg(test)]` on a struct field, \
             enum variant, method or impl block must not end the scan. Check \
             `hand_roll_scan::inline_test_module_spans` still recognises only \
             `mod` as a boundary, and that this file has not grown a test \
             module whose gating attribute the shared `cfg(test)` predicate \
             now reads as production (which would scan it) or a production \
             item this file's parser reads as test-only (which would hide \
             it).",
            d.scanned_lines,
            d.total_lines,
            d.ratio() * 100.0,
            d.file,
            min_ratio * 100.0,
        );
    }
}

fn has_test_shaped_basename(rel: &str) -> bool {
    rel.ends_with("_tests.rs") || rel.ends_with("/tests.rs") || rel == "tests.rs"
}

/// A ready-made exclusion predicate: excludes the guard file itself, any
/// `*_tests.rs` integration/unit test file, and any `tests.rs` submodule.
///
/// `guard_rel` is the guard's own path relative to the scan root (e.g.
/// `"hand_roll_guard.rs"` or `"ui/hand_roll_guard.rs"`).
///
/// [`coverage_report`] additionally derives test ownership from real module
/// edges, so a guard that passes this predicate keeps its existing basename
/// exclusions *and* gains structural ones. Callers that want a production
/// census with no basename convention at all should pass their own predicate
/// (typically `|rel| rel == guard_rel`) — the exclusion is absolute either
/// way.
pub fn default_exclude(rel: &str, guard_rel: &str) -> bool {
    rel == guard_rel || has_test_shaped_basename(rel)
}

//! Syntax classification shared by read-only code and editable text.
//!
//! [`tokenize`] classifies a complete source string. [`SyntaxHighlighter`]
//! carries the same lexer state between editor lines and keeps checkpoints for
//! exact revisits. Classification emits semantic [`TokenClass`] values; themes
//! resolve their colours at draw time.
//!
//! # The partition invariant
//!
//! [`tokenize`] returns tokens that partition the input exactly: they are
//! contiguous, non-overlapping, cover byte 0 to `source.len()`, and every byte
//! belongs to exactly one token. Concatenating `&source[t.start..t.end]` over
//! the returned tokens therefore reproduces `source` byte-for-byte. This holds
//! for malformed input too (unterminated strings/comments). A tokenizer that
//! drops or duplicates a byte silently corrupts displayed code, so this is the
//! load-bearing property and is asserted for every fixture.
//!
//! # Languages
//!
//! This is a deliberately small hand-written lexer, not a grammar engine.
//! Unsupported extensions and labels resolve to [`Language::Plain`]; supported
//! languages do not imply support for every construct in those languages:
//!
//! * [`Language::Rust`] — line `//` and nestable block `/* /* */ */`
//!   comments, `"..."` strings with `\` escapes, `r#"..."#` raw strings,
//!   `'a'` char literals and lifetimes, numeric literals, keywords.
//! * [`Language::C`] — line `//` and non-nestable block `/* */` comments,
//!   `"..."` and `'.'` literals with escapes, numbers, keywords.
//! * [`Language::Python`] — `#` line comments, `'...'`/`"..."` and triple-quoted
//!   `"""..."""` strings with escapes, numbers, keywords.
//! * [`Language::Json`] — `"..."` strings with escapes, numbers, and the three
//!   JSON literals `true` / `false` / `null` as keywords. No comments (JSON has
//!   none). Punctuation `{ } [ ] : ,`.
//! * [`Language::Shell`] — `#` line comments, `"..."` and `'...'` strings,
//!   numbers, and POSIX-shell keywords (`if`, `then`, `fi`, `for`, `while`, …).
//! * [`Language::Markdown`] — a dedicated lightweight tokenizer (Markdown is
//!   not a C-family grammar): `<!-- -->` HTML comments, fenced ```` ``` ```` and
//!   inline `` `code` `` spans as strings, and leading `#` ATX heading markers as
//!   keywords. Everything else is plain prose.
//! * [`Language::Plain`] — the unknown/`"text"` fallback:
//!   the whole input is returned as one [`TokenClass::Plain`] run (or no
//!   run at all for empty input). Nothing is classified.

#![allow(clippy::string_slice, reason = "the tokenizer advances by whole chars")]

/// A semantic token class — not a colour. `code_block.rs` maps each class
/// to a `TextTone`, re-resolved against the live theme at draw time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenClass {
    /// A language keyword (`fn`, `if`, `return`, `def`, …).
    Keyword,
    /// A string or char literal, including its delimiters.
    String,
    /// A line or block comment, including its delimiters.
    Comment,
    /// A numeric literal.
    Number,
    /// Punctuation / operators (`{`, `;`, `+`, `=>`, …).
    Punctuation,
    /// An identifier that is not a keyword.
    Identifier,
    /// Unclassified text: whitespace, and everything in [`Language::Plain`].
    Plain,
}

/// One classified run over the source, as a half-open byte range
/// `[start, end)`. `end > start` always (empty tokens are never emitted).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    /// Inclusive start byte offset into the source.
    pub start: usize,
    /// Exclusive end byte offset into the source.
    pub end: usize,
    /// The semantic class of this run.
    pub class: TokenClass,
}

/// A supported source language, plus the plain/unknown fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Language {
    /// Rust: nestable block comments, raw strings, lifetimes.
    Rust,
    /// C / C-like: non-nestable block comments, char literals.
    C,
    /// Python: `#` comments, triple-quoted strings.
    Python,
    /// JSON: `"..."` strings, numbers, `true`/`false`/`null` literals. No comments.
    Json,
    /// Shell (`sh`/`bash`/`zsh`): `#` comments, `"..."` and `'...'` strings,
    /// numbers, POSIX-shell keywords.
    Shell,
    /// Markdown: `<!-- -->` comments, fenced/inline code spans as strings, ATX
    /// heading markers as keywords. Dedicated tokenizer, not a C-family lexer.
    Markdown,
    /// No highlighting; the whole input is one [`TokenClass::Plain`] run. This
    /// is the default and the value for any unrecognised language string.
    #[default]
    Plain,
}

impl Language {
    /// Parse a language label (as passed to `EnzCodeBlock::language`) into a
    /// [`Language`]. Case-insensitive. Any unrecognised label — including
    /// `"text"` — maps to [`Language::Plain`], so
    /// unknown languages stay flat rather than being mis-highlighted.
    #[must_use]
    pub fn from_label(label: &str) -> Self {
        match label.trim().to_ascii_lowercase().as_str() {
            "rust" | "rs" => Language::Rust,
            "c" | "h" | "cpp" | "c++" | "cc" | "hpp" | "cxx" => Language::C,
            "python" | "py" => Language::Python,
            "json" => Language::Json,
            "md" | "markdown" => Language::Markdown,
            "sh" | "bash" | "zsh" | "shell" => Language::Shell,
            _ => Language::Plain,
        }
    }

    /// The language's stable identifier: the inverse of
    /// [`from_label`](Self::from_label), one of [`OVERRIDE_LABELS`](Self::OVERRIDE_LABELS).
    /// Show [`display_name`](Self::display_name) to people.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Language::Rust => "Rust",
            Language::C => "C",
            Language::Python => "Python",
            Language::Json => "JSON",
            Language::Shell => "Shell",
            Language::Markdown => "Markdown",
            Language::Plain => "Plain Text",
        }
    }

    /// The name to show for this language, in the current language.
    /// Proper names such as "Rust" stay as they are; "Plain Text" is translated.
    #[must_use]
    pub fn display_name(self) -> String {
        match self {
            Language::Plain => enzui_core::localization::text("enzui-syntax-plain-text"),
            other => other.label().to_owned(),
        }
    }

    /// The name to show for an [`OVERRIDE_LABELS`](Self::OVERRIDE_LABELS)
    /// entry, including the translated "Auto".
    #[must_use]
    pub fn override_display_name(label: &str) -> String {
        if label == "Auto" {
            enzui_core::localization::text("enzui-syntax-auto")
        } else {
            Self::from_label(label).display_name()
        }
    }

    /// How much of a document [`Language::detect`] reads.
    ///
    /// 64 KiB is far past every signal the heuristics look for (the largest is
    /// a fenced code block near the top of a Markdown file) and small enough
    /// that the sniff is free beside a frame.
    pub const SNIFF_BYTES: usize = 64 * 1024;

    /// The prefix [`Language::detect`] inspects, cut on a UTF-8 boundary.
    fn sniff_window(content: &str) -> &str {
        if content.len() <= Self::SNIFF_BYTES {
            return content;
        }
        let mut end = Self::SNIFF_BYTES;
        while end > 0 && !content.is_char_boundary(end) {
            end -= 1;
        }
        &content[..end]
    }

    /// Detect the language from buffer content (not a label or extension),
    /// for buffers with no useful extension — `Untitled` scratch buffers,
    /// shebang scripts, `Makefile`, and the like.
    ///
    /// Framework home for content-based detection rather than one
    /// per app; the heuristic is not app-specific.
    ///
    /// # It is allowed to decline
    ///
    /// When nothing in the content is a confident marker, this returns
    /// [`Language::Plain`] rather than guessing. A caller that wants a
    /// *label*-driven choice uses
    /// [`from_label`](Self::from_label); a caller with a file path should prefer
    /// the extension first and only fall back to this on an unhelpful one.
    ///
    /// # Markers
    ///
    /// * Shebang `#!…`: `python` → [`Python`](Language::Python);
    ///   `sh`/`bash`/`zsh` → [`Shell`](Language::Shell).
    /// * JSON: a body that (trimmed) starts with `{` or `[` and contains a
    ///   `":` key–value colon — a leading brace alone is too weak (a Rust block
    ///   or a shell `{ … }` starts with one), so the `":` guard is what makes it
    ///   decline on non-JSON braces.
    /// * Markdown: an ATX heading (`# `, `## `, …) at the start of a *line*,
    ///   or a fenced code block whose fence begins a line. These are checked
    ///   before Rust/C/Python because they are document-level structure: a
    ///   Markdown document may embed a fenced ```rust example, and that example
    ///   must not steal the whole document to Rust.
    /// * Rust: `fn main(`, `pub fn `, `use std::`, or a `let … =`/`fn ` shape.
    /// * Python: leading/`\n`-anchored `def `, `import `, `from `.
    /// * C: `#include <` or `int main(`.
    /// * Shell (content, no shebang): a `$VAR`/`${VAR}`/`$(…)` expansion, or a
    ///   paired control-flow shape (`if…then…fi`, `case…esac`, `do…done`).
    ///   Checked last so it only claims text nothing else did, and gated on
    ///   shell *syntax* so plain prose still declines.
    ///
    /// Order matters: the shebang wins first, then structured JSON, then
    /// Markdown document structure, then the language-marker families, then a
    /// content-based Shell fallback. Extension-based resolution (when a path is
    /// available) is a caller concern and not duplicated here.
    /// # Cost
    ///
    /// This is a sniff, not a parse: it inspects at most
    /// [`Language::SNIFF_BYTES`] of `content`. Every signal
    /// above is a property of the top of a document -- a shebang, an opening
    /// brace, a heading, an import -- so reading further buys nothing, while a
    /// text editor calls this once per frame on a document that can be tens of
    /// megabytes. Scanning the whole buffer cost a measured ~5 ms per megabyte
    /// per frame there, which is the difference between an editable large file
    /// and a frozen one.
    #[must_use]
    pub fn detect(content: &str) -> Self {
        let head = Self::sniff_window(content).trim_start();
        if head.is_empty() {
            return Language::Plain;
        }

        // Shebang scripts win outright.
        if let Some(first_line) = head.lines().next()
            && let Some(rest) = first_line.strip_prefix("#!")
        {
            let l = rest.to_ascii_lowercase();
            if l.contains("python") {
                return Language::Python;
            }
            // Match a shell interpreter word, not merely the substring "sh"
            // (which would fire on e.g. `/usr/bin/flash`). Look for a path
            // component or word ending in sh / named bash / zsh.
            if l.contains("bash")
                || l.contains("zsh")
                || l.split(['/', ' ', '\t'])
                    .any(|w| w == "sh" || w.ends_with("sh"))
            {
                return Language::Shell;
            }
        }

        // Structured JSON: a brace/bracket opener and a `":` key colon. The
        // colon guard is what lets a non-JSON `{ … }` decline.
        if (head.starts_with('{') || head.starts_with('['))
            && (head.contains("\":") || head.contains("\" :"))
        {
            return Language::Json;
        }

        // Markdown first: headings and fences are document-level structure that
        // outrank language markers embedded inside them. Both signals are
        // line-anchored, so `#!` shebangs, `#include`, and mid-line `` `x` ``
        // mentions never read as document structure.
        if starts_a_line_with(head, "# ")
            || starts_a_line_with(head, "## ")
            || starts_a_line_with(head, "### ")
            || starts_a_line_with(head, "#### ")
            || starts_a_line_with(head, "##### ")
            || starts_a_line_with(head, "###### ")
            || has_fence_line(head)
        {
            return Language::Markdown;
        }

        // Rust markers. Checked after Markdown so a fenced ```rust example in a
        // Markdown document does not win here on a bare `contains`.
        if head.contains("fn main(") || head.contains("pub fn ") || head.contains("use std::") {
            return Language::Rust;
        }

        // Python markers.
        if head.starts_with("def ")
            || head.starts_with("import ")
            || head.starts_with("from ")
            || head.contains("\ndef ")
            || head.contains("\nimport ")
        {
            return Language::Python;
        }

        // C markers.
        if head.contains("#include <") || head.contains("int main(") {
            return Language::C;
        }

        // Content-based Shell detection, checked last: only shell control-flow
        // and expansion shapes (see `looks_like_shell`) claim text here, so
        // plain prose still declines to `Plain`.
        if looks_like_shell(head) {
            return Language::Shell;
        }

        Language::Plain
    }

    /// Resolve the tokenizer language from a file extension.
    ///
    /// This is the framework home for the extension→language table every
    /// editor-ish app was otherwise re-implementing. Extensions the tokenizer
    /// cannot colour (TOML, YAML, HTML and others)
    /// map to [`Language::Plain`]: the file still renders, just flat. The
    /// *display* label (see [`display_label`](Self::display_label)) is
    /// independent and stays specific so the UI is honest about the file type.
    ///
    /// Returns `None` when there is no path or no extension, so a caller can
    /// fall back to [`detect`](Self::detect) on buffer content.
    #[must_use]
    pub fn from_extension(path: Option<&std::path::Path>) -> Option<Self> {
        let ext = path?.extension()?.to_str()?;
        Some(match ext.to_ascii_lowercase().as_str() {
            "rs" => Language::Rust,
            "c" | "h" | "cpp" | "cc" | "cxx" | "hpp" | "hxx" => Language::C,
            "py" | "pyw" => Language::Python,
            "json" => Language::Json,
            "md" | "markdown" => Language::Markdown,
            "sh" | "bash" | "zsh" => Language::Shell,
            // Extensions we recognise for labelling but do not tokenize.
            "toml" | "yaml" | "yml" | "txt" | "html" | "htm" | "css" | "js" | "ts" => {
                Language::Plain
            }
            _ => return None,
        })
    }

    /// A human display label for a file, by extension.
    ///
    /// Independent of whether the framework can *tokenize* the language — a
    /// `.json` file shows `"JSON"` even where highlighting falls back to flat,
    /// and `.toml`/`.yaml`/`.html`/… get a specific label though they are not
    /// tokenized. Returns `None` for an unrecognised or absent extension.
    #[must_use]
    pub fn display_label(path: Option<&std::path::Path>) -> Option<&'static str> {
        let ext = path?.extension()?.to_str()?;
        Some(match ext.to_ascii_lowercase().as_str() {
            "rs" => "Rust",
            "json" => "JSON",
            "toml" => "TOML",
            "yaml" | "yml" => "YAML",
            "md" | "markdown" => "Markdown",
            "sh" | "bash" | "zsh" => "Shell",
            "c" | "h" => "C",
            "cpp" | "cc" | "cxx" | "hpp" | "hxx" => "C++",
            "py" | "pyw" => "Python",
            "txt" => "Plain Text",
            "html" | "htm" => "HTML",
            "css" => "CSS",
            "js" => "JavaScript",
            "ts" => "TypeScript",
            _ => return None,
        })
    }

    /// Resolve the tokenizer language for a buffer: a manual
    /// `override_label` wins outright, else the file extension, else a
    /// content [`detect`](Self::detect), else [`Language::Plain`].
    ///
    /// This is the single policy an editor needs, folding
    /// [`from_extension`](Self::from_extension), [`detect`](Self::detect) and
    /// [`from_label`](Self::from_label) into the extension-then-content-then-plain
    /// order. `override_label` is the user's status-bar override; `"Auto"` (or
    /// `None`) means "no override".
    ///
    /// A recognised extension is *authoritative* — a `.rs` file whose body
    /// looks like C still resolves to Rust, and a `.txt` (or `.toml`, `.html`
    /// …) file stays [`Language::Plain`] whatever its prose resembles, so the
    /// colours never contradict the file type the editor shows. Content
    /// detection runs only for an absent or unrecognised extension
    /// (`Untitled`, `Makefile`, a shebang script).
    #[must_use]
    pub fn resolve(
        path: Option<&std::path::Path>,
        content: &str,
        override_label: Option<&str>,
    ) -> Self {
        if let Some(label) = override_label
            && !label.eq_ignore_ascii_case("auto")
        {
            return Language::from_label(label);
        }
        Language::from_extension(path).unwrap_or_else(|| Language::detect(content))
    }

    /// The identifiers offered in an editor's manual language-override menu;
    /// show each through [`override_display_name`](Self::override_display_name).
    /// `"Auto"` first (meaning "resolve automatically"), then one
    /// label per language the framework can tokenize. `Language::resolve`
    /// understands each of these via [`from_label`](Self::from_label).
    pub const OVERRIDE_LABELS: &'static [&'static str] = &[
        "Auto",
        "Rust",
        "C",
        "Python",
        "JSON",
        "Shell",
        "Markdown",
        "Plain Text",
    ];
}

// ---------------------------------------------------------------------------
// Content-detection helpers (used by `Language::detect`)
// ---------------------------------------------------------------------------

/// Whether any line of `head` begins (after trimming leading spaces) with
/// `marker`. Used for ATX headings so a `#` mid-line is not a heading.
fn starts_a_line_with(head: &str, marker: &str) -> bool {
    head.lines()
        .any(|line| line.trim_start().starts_with(marker))
}

/// Genuine fence *lines* only (first non-space content is ```): bare
/// `contains` fires on mid-sentence literals and shell heredocs.
fn has_fence_line(head: &str) -> bool {
    head.lines()
        .any(|line| line.trim_start().starts_with("```"))
}

/// Content-based shell heuristic. Returns true only on a shell
/// *control-flow or expansion shape*, never on tokens ordinary prose contains,
/// so plain text still declines to `Plain`.
fn looks_like_shell(head: &str) -> bool {
    // `$VAR` / `${VAR}` / `$(...)` expansion: a `$` immediately followed by an
    // identifier char, `{`, or `(`. Prose almost never writes `$word`.
    if let Some(idx) = head.find('$')
        && head[idx + 1..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '{' || c == '(')
    {
        return true;
    }

    // Paired control-flow keywords in shell's own layout: the opener ends a
    // line (`for f in *; do`, `if [ -x f ]; then`) and the closer starts one.
    // Prose uses "do", "done", "then" and "if" freely, but never like this.
    // A keyword straight after `;` counts the same, for one-line loops.
    let opens = |word| ends_a_line_with(head, word) || after_separator(head, word);
    let closes = |word| starts_a_line_with_word(head, word) || after_separator(head, word);
    (opens("then") && closes("fi"))
        || (starts_a_line_with_word(head, "case") && closes("esac"))
        || (opens("do") && closes("done"))
}

/// Whether the whole word `word` follows a `;` (`…; do echo; done`).
fn after_separator(head: &str, word: &str) -> bool {
    head.match_indices(';').any(|(i, _)| {
        head[i + 1..]
            .trim_start_matches([' ', '\t'])
            .strip_prefix(word)
            .is_some_and(|after| after.is_empty() || after.starts_with([' ', '\t', ';', '\n']))
    })
}

/// Whether some line ends with the whole word `word` (`…; do`).
fn ends_a_line_with(head: &str, word: &str) -> bool {
    head.lines().any(|line| {
        let line = line.trim_end();
        line.strip_suffix(word)
            .is_some_and(|before| before.is_empty() || before.ends_with([' ', '\t', ';']))
    })
}

/// Whether some line starts with the whole word `word` (`done`, `fi;`).
fn starts_a_line_with_word(head: &str, word: &str) -> bool {
    head.lines().any(|line| {
        line.trim_start()
            .strip_prefix(word)
            .is_some_and(|after| after.is_empty() || after.starts_with([' ', '\t', ';', ')']))
    })
}

/// Tokenize `source` under `language`, returning a list of [`Token`]s that
/// partition the input exactly (see the module docs).
///
/// * Empty input yields no tokens (an empty `Vec`), regardless of
///   language — there is nothing to classify. This is pinned by a test.
/// * [`Language::Plain`] yields exactly one [`TokenClass::Plain`] token
///   spanning the whole input (for non-empty input).
///
/// This never panics and never loops forever, including on unterminated
/// strings or block comments running to EOF.
#[must_use]
pub fn tokenize(source: &str, language: Language) -> Vec<Token> {
    if source.is_empty() {
        return Vec::new();
    }
    match language {
        Language::Plain => vec![Token {
            start: 0,
            end: source.len(),
            class: TokenClass::Plain,
        }],
        Language::Rust => Lexer::new(source, LangRules::rust()).run(),
        Language::C => Lexer::new(source, LangRules::c()).run(),
        Language::Python => Lexer::new(source, LangRules::python()).run(),
        Language::Json => Lexer::new(source, LangRules::json()).run(),
        Language::Shell => Lexer::new(source, LangRules::shell()).run(),
        Language::Markdown => tokenize_markdown(source),
    }
}

// ---------------------------------------------------------------------------
// Per-language rules
// ---------------------------------------------------------------------------

/// The knobs that differ between the C-family lexers. Everything else (number
/// scanning, identifier scanning, keyword lookup, the partition machinery) is
/// shared.
struct LangRules {
    /// Recognised keywords, sorted-agnostic (looked up by linear scan; the
    /// sets are tiny).
    keywords: &'static [&'static str],
    /// Whether `/* */` block comments nest (Rust: yes; C: no).
    nested_block_comments: bool,
    /// Whether `//` starts a line comment (Rust, C: yes; Python: no).
    slash_line_comment: bool,
    /// Whether `/* */` block comments exist (Rust, C: yes; Python: no).
    slash_block_comment: bool,
    /// Whether `#` starts a line comment (Python: yes).
    hash_line_comment: bool,
    /// Whether `r"..."` / `r#"..."#` raw strings exist (Rust: yes).
    raw_strings: bool,
    /// Whether triple-quoted strings exist (Python: yes).
    triple_strings: bool,
    /// Whether `'x'` is a char literal (Rust, C: yes). When false, `'` is
    /// treated as an ordinary string quote (Python).
    char_literals: bool,
}

impl LangRules {
    fn rust() -> Self {
        Self {
            keywords: &[
                "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else",
                "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match",
                "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct",
                "super", "trait", "true", "type", "unsafe", "use", "where", "while",
            ],
            nested_block_comments: true,
            slash_line_comment: true,
            slash_block_comment: true,
            hash_line_comment: false,
            raw_strings: true,
            triple_strings: false,
            char_literals: true,
        }
    }

    fn c() -> Self {
        Self {
            keywords: &[
                "auto", "break", "case", "char", "const", "continue", "default", "do", "double",
                "else", "enum", "extern", "float", "for", "goto", "if", "inline", "int", "long",
                "register", "restrict", "return", "short", "signed", "sizeof", "static", "struct",
                "switch", "typedef", "union", "unsigned", "void", "volatile", "while",
            ],
            nested_block_comments: false,
            slash_line_comment: true,
            slash_block_comment: true,
            hash_line_comment: false,
            raw_strings: false,
            triple_strings: false,
            char_literals: true,
        }
    }

    fn python() -> Self {
        Self {
            keywords: &[
                "and", "as", "assert", "async", "await", "break", "class", "continue", "def",
                "del", "elif", "else", "except", "False", "finally", "for", "from", "global", "if",
                "import", "in", "is", "lambda", "None", "nonlocal", "not", "or", "pass", "raise",
                "return", "True", "try", "while", "with", "yield",
            ],
            nested_block_comments: false,
            slash_line_comment: false,
            slash_block_comment: false,
            hash_line_comment: true,
            raw_strings: false,
            triple_strings: true,
            char_literals: false,
        }
    }

    fn json() -> Self {
        Self {
            keywords: &["true", "false", "null"],
            nested_block_comments: false,
            slash_line_comment: false,
            slash_block_comment: false,
            hash_line_comment: false,
            raw_strings: false,
            triple_strings: false,
            // JSON has no char literals; a `'` is not a JSON string delimiter,
            // so with char_literals=false the shared lexer would treat `'` as a
            // string quote. JSON never contains a bare `'`, and if malformed
            // input does, running it to EOF as a string is a harmless, partition-
            // preserving classification (never a panic/hang).
            char_literals: false,
        }
    }

    fn shell() -> Self {
        Self {
            keywords: &[
                "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case",
                "esac", "in", "function", "select", "return", "break", "continue", "local",
                "export", "readonly", "declare", "unset", "echo", "exit", "shift", "source",
                "alias", "eval", "trap", "set",
            ],
            nested_block_comments: false,
            slash_line_comment: false,
            slash_block_comment: false,
            hash_line_comment: true,
            raw_strings: false,
            triple_strings: false,
            // In shell, `'...'` is a non-escaping single-quoted string.
            char_literals: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Markdown tokenizer (dedicated — not a C-family lexer)
// ---------------------------------------------------------------------------

/// Tokenize Markdown into the honest subset of classes its grammar has:
///
/// * [`TokenClass::Comment`] — `<!-- … -->` HTML comments (unterminated runs to
///   EOF).
/// * [`TokenClass::String`] — fenced code blocks ```` ```…``` ```` and inline
///   `` `code` `` spans (an unterminated fence/span runs to EOF).
/// * [`TokenClass::Keyword`] — a leading ATX heading marker (`#`, `##`, … up to
///   six) at the start of a line, including its trailing space.
///
/// Everything else is [`TokenClass::Plain`] prose. Like the C-family lexer this
/// partitions the input exactly, emits nothing for empty input, and never
/// panics or loops — including on unterminated fences, spans, or comments.
fn tokenize_markdown(source: &str) -> Vec<Token> {
    let src = source.as_bytes();
    let mut out: Vec<Token> = Vec::new();
    let mut pos = 0usize;
    let mut plain_start: Option<usize> = None;
    let mut at_line_start = true;

    // Flush pending plain runs.
    macro_rules! flush_plain {
        ($upto:expr) => {
            if let Some(s) = plain_start.take() {
                if $upto > s {
                    out.push(Token {
                        start: s,
                        end: $upto,
                        class: TokenClass::Plain,
                    });
                }
            }
        };
    }

    while pos < src.len() {
        let b = src[pos];

        // HTML comment `<!-- ... -->`.
        if b == b'<'
            && src.get(pos + 1) == Some(&b'!')
            && src.get(pos + 2) == Some(&b'-')
            && src.get(pos + 3) == Some(&b'-')
        {
            let start = pos;
            pos += 4;
            while pos < src.len() {
                if src[pos] == b'-'
                    && src.get(pos + 1) == Some(&b'-')
                    && src.get(pos + 2) == Some(&b'>')
                {
                    pos += 3;
                    break;
                }
                pos += 1;
            }
            flush_plain!(start);
            out.push(Token {
                start,
                end: pos,
                class: TokenClass::Comment,
            });
            at_line_start = false;
            continue;
        }

        // ATX heading marker at line start: 1–6 `#` then a space.
        if at_line_start && b == b'#' {
            let mut i = pos;
            let mut hashes = 0;
            while i < src.len() && src[i] == b'#' && hashes < 6 {
                i += 1;
                hashes += 1;
            }
            if i < src.len() && src[i] == b' ' {
                let start = pos;
                let end = i + 1; // include the space after the marker
                flush_plain!(start);
                out.push(Token {
                    start,
                    end,
                    class: TokenClass::Keyword,
                });
                pos = end;
                at_line_start = false;
                continue;
            }
        }

        // Fenced code block at any position: >=3 backticks, then up to the
        // matching closing fence (or EOF).
        if b == b'`' && src.get(pos + 1) == Some(&b'`') && src.get(pos + 2) == Some(&b'`') {
            let start = pos;
            // count the opening run
            let mut fence = 0;
            while pos < src.len() && src[pos] == b'`' {
                fence += 1;
                pos += 1;
            }
            // scan to a line that is a closing fence of >= `fence` backticks.
            loop {
                while pos < src.len() && src[pos] != b'\n' {
                    pos += 1;
                }
                if pos >= src.len() {
                    break; // unterminated fence -> to EOF
                }
                pos += 1; // consume the newline
                // count backticks starting this line (leading spaces allowed)
                let mut j = pos;
                while j < src.len() && (src[j] == b' ') {
                    j += 1;
                }
                let mut close = 0;
                while j < src.len() && src[j] == b'`' {
                    close += 1;
                    j += 1;
                }
                if close >= fence {
                    pos = j;
                    break;
                }
            }
            flush_plain!(start);
            out.push(Token {
                start,
                end: pos,
                class: TokenClass::String,
            });
            at_line_start = false;
            continue;
        }

        // Inline code span: next backtick on the same line, else EOL/EOF.
        if b == b'`' {
            let start = pos;
            pos += 1;
            while pos < src.len() && src[pos] != b'`' && src[pos] != b'\n' {
                pos += 1;
            }
            if pos < src.len() && src[pos] == b'`' {
                pos += 1; // consume closing backtick
            }
            flush_plain!(start);
            out.push(Token {
                start,
                end: pos,
                class: TokenClass::String,
            });
            at_line_start = false;
            continue;
        }

        // Plain byte; track line starts.
        at_line_start = b == b'\n';
        if plain_start.is_none() {
            plain_start = Some(pos);
        }
        pos += 1;
    }

    flush_plain!(src.len());
    out
}

// ---------------------------------------------------------------------------
// Lexer
// ---------------------------------------------------------------------------

/// A byte-offset lexer over the source. Works on bytes; all delimiters,
/// keywords, digits and identifier characters are ASCII, and multi-byte UTF-8
/// only ever appears *inside* strings, comments, or identifiers where it is
/// scanned opaquely — so byte offsets always land on char boundaries.
struct Lexer<'a> {
    src: &'a [u8],
    text: &'a str,
    pos: usize,
    rules: LangRules,
    out: Vec<Token>,
    /// Start of the current run of `Plain` bytes not yet flushed.
    plain_start: Option<usize>,
}

impl<'a> Lexer<'a> {
    fn new(text: &'a str, rules: LangRules) -> Self {
        Self {
            src: text.as_bytes(),
            text,
            pos: 0,
            rules,
            out: Vec::new(),
            plain_start: None,
        }
    }

    /// Mark `[start, end)` classified, flushing preceding plain runs first.
    fn emit(&mut self, start: usize, end: usize, class: TokenClass) {
        debug_assert!(end > start, "empty token");
        self.flush_plain(start);
        self.out.push(Token { start, end, class });
    }

    /// Flush the pending plain run up to (but not including) `upto`.
    fn flush_plain(&mut self, upto: usize) {
        if let Some(start) = self.plain_start.take()
            && upto > start
        {
            self.out.push(Token {
                start,
                end: upto,
                class: TokenClass::Plain,
            });
        }
    }

    /// Extend the pending plain run over the current byte, then advance.
    fn take_plain(&mut self) {
        if self.plain_start.is_none() {
            self.plain_start = Some(self.pos);
        }
        self.pos += 1;
    }

    fn peek(&self, off: usize) -> Option<u8> {
        self.src.get(self.pos + off).copied()
    }

    fn run(mut self) -> Vec<Token> {
        while self.pos < self.src.len() {
            let b = self.src[self.pos];

            // Line comments.
            if self.rules.hash_line_comment && b == b'#' {
                self.scan_line_comment();
                continue;
            }
            if self.rules.slash_line_comment && b == b'/' && self.peek(1) == Some(b'/') {
                self.scan_line_comment();
                continue;
            }
            // Block comments.
            if self.rules.slash_block_comment && b == b'/' && self.peek(1) == Some(b'*') {
                self.scan_block_comment();
                continue;
            }
            // Raw strings (Rust): r"..." / r#"..."# / r##"..."## …
            if self.rules.raw_strings && b == b'r' && self.at_raw_string_prefix() {
                self.scan_raw_string();
                continue;
            }
            // Triple-quoted strings before ordinary ones.
            if self.rules.triple_strings && (b == b'"' || b == b'\'') && self.at_triple(b) {
                self.scan_triple_string(b);
                continue;
            }
            // Char literal vs string quote.
            if b == b'\'' {
                if self.rules.char_literals {
                    // A lifetime in Rust looks like `'a` with no closing quote.
                    if self.rules.raw_strings && self.at_lifetime() {
                        self.scan_identifier_like(); // treats `'a` label below
                        continue;
                    }
                    self.scan_char_literal();
                    continue;
                }
                // Python `'` quotes strings.
                self.scan_string(b'\'');
                continue;
            }
            if b == b'"' {
                self.scan_string(b'"');
                continue;
            }
            // Numbers: a digit, or `.` immediately followed by a digit.
            if b.is_ascii_digit() {
                self.scan_number();
                continue;
            }
            // Identifiers / keywords.
            if is_ident_start(b) {
                self.scan_identifier();
                continue;
            }
            // Punctuation: any ASCII graphic that is not whitespace and not
            // an identifier/number/string/comment starter handled above.
            if is_punct(b) {
                let start = self.pos;
                self.pos += 1;
                self.emit(start, self.pos, TokenClass::Punctuation);
                continue;
            }
            // Whitespace and anything else → plain.
            self.take_plain();
        }
        // Flush trailing plain runs.
        let end = self.src.len();
        self.flush_plain(end);
        self.out
    }

    // --- scanners ---------------------------------------------------------

    fn scan_line_comment(&mut self) {
        let start = self.pos;
        // To EOL; the newline stays plain (own run, partition-harmless).
        while self.pos < self.src.len() && self.src[self.pos] != b'\n' {
            self.pos += 1;
        }
        self.emit(start, self.pos, TokenClass::Comment);
    }

    fn scan_block_comment(&mut self) {
        let start = self.pos;
        self.pos += 2; // consume `/*`
        let mut depth = 1usize;
        while self.pos < self.src.len() {
            if self.src[self.pos] == b'/' && self.peek(1) == Some(b'*') {
                self.pos += 2;
                if self.rules.nested_block_comments {
                    depth += 1;
                }
                continue;
            }
            if self.src[self.pos] == b'*' && self.peek(1) == Some(b'/') {
                self.pos += 2;
                depth -= 1;
                if depth == 0 {
                    break;
                }
                continue;
            }
            self.pos += 1;
        }
        // Unterminated: `pos` reached EOF with depth > 0 — the whole tail is
        // the comment. Terminates, does not hang.
        self.emit(start, self.pos, TokenClass::Comment);
    }

    /// Scan a `"..."` or `'...'` string with `\`-escapes. Unterminated strings
    /// run to EOF (they do not stop at the newline — a string spanning a
    /// line is still a string).
    fn scan_string(&mut self, quote: u8) {
        let start = self.pos;
        self.pos += 1; // opening quote
        while self.pos < self.src.len() {
            let c = self.src[self.pos];
            if c == b'\\' {
                self.pos += 2;
                continue;
            }
            self.pos += 1;
            if c == quote {
                break;
            }
        }
        // Clamp in case an escape at EOF advanced past the end.
        if self.pos > self.src.len() {
            self.pos = self.src.len();
        }
        self.emit(start, self.pos, TokenClass::String);
    }

    /// Rust/C char literal `'a'`, `'\n'`, `'\''`. Unterminated (a lone `'`)
    /// runs to EOF like a string.
    fn scan_char_literal(&mut self) {
        self.scan_string(b'\'');
    }

    /// Whether the current `r` begins a raw-string prefix: `r"` or `r#`… `"`.
    fn at_raw_string_prefix(&self) -> bool {
        let mut i = self.pos + 1;
        while self.src.get(i) == Some(&b'#') {
            i += 1;
        }
        self.src.get(i) == Some(&b'"')
    }

    /// Scan a Rust raw string `r"..."`, `r#"..."#`, `r##"..."##`, …. The
    /// closing delimiter is `"` followed by exactly as many `#` as opened it.
    /// No escapes. Unterminated runs to EOF.
    fn scan_raw_string(&mut self) {
        let start = self.pos;
        self.pos += 1; // `r`
        let mut hashes = 0usize;
        while self.peek(0) == Some(b'#') {
            hashes += 1;
            self.pos += 1;
        }
        self.pos += 1; // opening `"`
        while self.pos < self.src.len() {
            if self.src[self.pos] == b'"' {
                let mut i = self.pos + 1;
                let mut count = 0;
                while count < hashes && self.src.get(i) == Some(&b'#') {
                    i += 1;
                    count += 1;
                }
                if count == hashes {
                    self.pos = i;
                    break;
                }
            }
            self.pos += 1;
        }
        self.emit(start, self.pos, TokenClass::String);
    }

    /// Whether the current quote begins a triple-quoted string.
    fn at_triple(&self, b: u8) -> bool {
        self.peek(1) == Some(b) && self.peek(2) == Some(b)
    }

    /// Scan a Python triple-quoted string `"""..."""` / `'''...'''` with
    /// `\`-escapes. Unterminated runs to EOF.
    fn scan_triple_string(&mut self, quote: u8) {
        let start = self.pos;
        self.pos += 3; // opening triple
        while self.pos < self.src.len() {
            if self.src[self.pos] == b'\\' {
                self.pos += 2;
                continue;
            }
            if self.src[self.pos] == quote
                && self.peek(1) == Some(quote)
                && self.peek(2) == Some(quote)
            {
                self.pos += 3;
                break;
            }
            self.pos += 1;
        }
        if self.pos > self.src.len() {
            self.pos = self.src.len();
        }
        self.emit(start, self.pos, TokenClass::String);
    }

    /// Whether the current `'` begins a Rust lifetime (`'a`, `'static`) rather
    /// than a char literal. Heuristic: `'` followed by an identifier-start byte
    /// that is not immediately closed by a `'` within a plausible char
    /// literal (i.e. not `'a'`).
    fn at_lifetime(&self) -> bool {
        match self.peek(1) {
            Some(c) if is_ident_start(c) => {}
            _ => return false,
        }
        // `'a'` is a char literal, not a lifetime: peek for a closing quote
        // right after a single ident char.
        if self.peek(2) == Some(b'\'') {
            return false;
        }
        true
    }

    /// Scan a lifetime label `'ident` as an identifier-class run beginning at
    /// the quote. (Lifetimes are not keywords; classing them as identifiers is
    /// honest and keeps the partition exact.)
    fn scan_identifier_like(&mut self) {
        let start = self.pos;
        self.pos += 1; // the `'`
        while self.pos < self.src.len() && is_ident_continue(self.src[self.pos]) {
            self.pos += 1;
        }
        self.emit(start, self.pos, TokenClass::Identifier);
    }

    /// Scan a numeric literal. Accepts a leading digit run, an optional
    /// fractional part (`.` + digits), hex/bin/oct prefixes, digit separators
    /// (`_`), and a trailing exponent. A `.` is only consumed as part of the
    /// number when followed by a digit, so `x.y` keeps the `.` as punctuation.
    fn scan_number(&mut self) {
        let start = self.pos;
        if self.src[self.pos] == b'0'
            && let Some(p) = self.peek(1)
            && matches!(p, b'x' | b'X' | b'b' | b'B' | b'o' | b'O')
        {
            self.pos += 2;
        }
        while self.pos < self.src.len() {
            let c = self.src[self.pos];
            if c.is_ascii_alphanumeric() || c == b'_' {
                self.pos += 1;
            } else if c == b'.' && self.peek(1).is_some_and(|n| n.is_ascii_digit()) {
                self.pos += 1; // decimal point followed by a digit
            } else {
                break;
            }
        }
        self.emit(start, self.pos, TokenClass::Number);
    }

    /// Scan an identifier (or keyword). The class is [`TokenClass::Keyword`]
    /// if the exact text is in the language's keyword set, else
    /// [`TokenClass::Identifier`].
    fn scan_identifier(&mut self) {
        let start = self.pos;
        self.pos += 1;
        while self.pos < self.src.len() && is_ident_continue(self.src[self.pos]) {
            self.pos += 1;
        }
        let word = &self.text[start..self.pos];
        let class = if self.rules.keywords.contains(&word) {
            TokenClass::Keyword
        } else {
            TokenClass::Identifier
        };
        self.emit(start, self.pos, class);
    }
}

// ---------------------------------------------------------------------------
// Byte classification helpers
// ---------------------------------------------------------------------------

fn is_ident_start(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphabetic() || b >= 0x80
}

fn is_ident_continue(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphanumeric() || b >= 0x80
}

/// ASCII punctuation / operator bytes. Excludes `_`, quotes, and the digits
/// (handled elsewhere), and excludes whitespace.
fn is_punct(b: u8) -> bool {
    matches!(
        b,
        b'!' | b'%'
            | b'&'
            | b'('
            | b')'
            | b'*'
            | b'+'
            | b','
            | b'-'
            | b'.'
            | b'/'
            | b':'
            | b';'
            | b'<'
            | b'='
            | b'>'
            | b'?'
            | b'@'
            | b'['
            | b'\\'
            | b']'
            | b'^'
            | b'{'
            | b'|'
            | b'}'
            | b'~'
            | b'#'
            | b'$'
            | b'`'
    )
}

// ---------------------------------------------------------------------------
// text_editor bridge: a `text::Highlighter` over `tokenize`
// ---------------------------------------------------------------------------
//
// Read-only (`EnzCodeBlock`) and editable (`EnzTextArea`, via the
// line-at-a-time `text::Highlighter` interface) widgets share one tokenizer
// plus the centralised [`tone_for_class`] mapping; only the transport differs
// (tone → `rich_text` vs theme-resolved `Color` in [`format_for_class`]).

use iced::advanced::text::highlighter::{self as text_highlighter};
use iced::advanced::text::parser;
use std::collections::BTreeMap;
use std::ops::Range;

/// Map a [`TokenClass`] to the semantic [`crate::enz_text::TextTone`] it paints,
/// or `None` for classes inheriting the ambient body ink.
///
/// Single decision point shared by the read-only and editable paths.
/// Conservative: only strongly distinguishable classes get tones, so ordinary
/// code is not a rainbow.
#[must_use]
pub fn tone_for_class(class: TokenClass) -> Option<crate::enz_text::TextTone> {
    use crate::enz_text::TextTone;
    match class {
        TokenClass::Keyword => Some(TextTone::Accent),
        TokenClass::String => Some(TextTone::Success),
        TokenClass::Comment => Some(TextTone::Muted),
        TokenClass::Number => Some(TextTone::Warning),
        TokenClass::Punctuation | TokenClass::Identifier | TokenClass::Plain => None,
    }
}

/// Concrete highlight [`Style`](text_highlighter::Style) for a [`TokenClass`]
/// against a live theme.
///
/// Editor-side analogue of the code block's tone resolution: the class's tone
/// resolves through the theme palette (dark/light/custom each paint their own
/// hue). `None` tones keep the editor's ambient value ink; `font: None` keeps
/// the configured monospace font — highlighting is colour, not font.
#[must_use]
pub fn format_for_class(class: &TokenClass, theme: &iced::Theme) -> text_highlighter::Style {
    let color = tone_for_class(*class).map(|tone| {
        let tokens =
            crate::enz_text::TextTokens::from_palette(&crate::theme_lookup::palette(theme));
        tokens.color_for(tone)
    });
    text_highlighter::Style {
        color,
        font: None,
        ..text_highlighter::Style::default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
enum LexicalState {
    #[default]
    Code,
    BlockComment(usize),
    Quoted(u8),
    RustRaw(usize),
    PythonTriple(u8),
    MarkdownFence(usize),
    MarkdownComment,
}

/// Maximum lexical checkpoints retained by an editable highlighter.
pub const EDITOR_CHECKPOINT_CAPACITY: usize = 4_096;

#[derive(Debug, Clone)]
struct LineCheckpoint {
    start: LexicalState,
}

/// Deterministic work counters for an editable syntax highlighter.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyntaxWork {
    /// Physical lines classified since construction or the last reset.
    pub lines: usize,
    /// Direct seeks satisfied from a valid lexical checkpoint.
    pub checkpoint_seeks: usize,
    /// Direct seeks refused because no valid checkpoint existed.
    pub replay_seeks: usize,
    /// Old checkpoints discarded to keep highlighter memory bounded.
    pub checkpoint_evictions: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct StatefulSyntaxEngine {
    language: Language,
    current_line: usize,
    state: LexicalState,
    checkpoints: BTreeMap<usize, LineCheckpoint>,
    valid_through: usize,
    work: SyntaxWork,
    /// One-bit gate: the last invalidation was `note_edits`, so re-keyed
    /// checkpoints below the edited region can be trusted for reconvergence.
    /// Cleared by any non-`note_edits` invalidation (`change_line`, `rewind`)
    /// so the plain path never trusts a checkpoint it did not re-key.
    reuse_pending: bool,
}

impl StatefulSyntaxEngine {
    pub(crate) fn new(language: Language) -> Self {
        Self {
            language,
            current_line: 0,
            state: LexicalState::Code,
            checkpoints: BTreeMap::new(),
            valid_through: 0,
            work: SyntaxWork::default(),
            reuse_pending: false,
        }
    }

    pub(crate) fn language(&self) -> Language {
        self.language
    }

    pub(crate) fn update_language(&mut self, language: Language) {
        if self.language != language {
            *self = Self::new(language);
        } else {
            self.rewind();
        }
    }

    pub(crate) fn rewind(&mut self) {
        self.current_line = 0;
        self.state = LexicalState::Code;
        self.reuse_pending = false;
    }

    pub(crate) fn change_line(&mut self, line: usize) {
        self.reuse_pending = false;
        if self.checkpoints.is_empty() {
            self.current_line = 0;
            self.state = LexicalState::Code;
            return;
        }

        let requested = self.current_line.min(line);
        let (restart, restart_state) = if requested == self.current_line {
            (requested, self.state.clone())
        } else if let Some((checkpoint_line, checkpoint)) =
            self.checkpoints.range(..=requested).next_back()
        {
            (*checkpoint_line, checkpoint.start.clone())
        } else {
            (0, LexicalState::Code)
        };

        self.current_line = restart;
        self.state = restart_state;
        self.valid_through = self.valid_through.min(restart);
    }

    pub(crate) fn highlight_line(&mut self, line: &str) -> Vec<Token> {
        let index = self.current_line;
        let start = self.state.clone();

        // Reuse: replay reconverging onto a `note_edits`-re-keyed checkpoint
        // revalidates the contiguous run in one step, stopping at the first
        // gap so lower edited regions are never crossed.
        if self.reuse_pending
            && index >= self.valid_through
            && self
                .checkpoints
                .get(&index)
                .is_some_and(|existing| existing.start == start)
        {
            let mut frontier = index;
            while self.checkpoints.contains_key(&(frontier + 1)) {
                frontier += 1;
            }
            if frontier + 1 > self.valid_through {
                self.valid_through = frontier + 1;
            }
            self.reuse_pending = false;
        }

        let (tokens, end) = tokenize_line_from_state(line, self.language, &start);
        self.checkpoints.insert(index, LineCheckpoint { start });

        while self.checkpoints.len() > EDITOR_CHECKPOINT_CAPACITY {
            let first = *self
                .checkpoints
                .keys()
                .next()
                .expect("an over-budget checkpoint map is non-empty");
            let last = *self
                .checkpoints
                .keys()
                .next_back()
                .expect("an over-budget checkpoint map is non-empty");
            let evicted = if index.abs_diff(first) > index.abs_diff(last) {
                first
            } else {
                last
            };
            self.checkpoints.remove(&evicted);
            self.work.checkpoint_evictions += 1;
        }

        self.state = end;
        self.current_line += 1;
        self.valid_through = self.valid_through.max(self.current_line);
        self.work.lines += 1;
        tokens
    }

    pub(crate) fn current_line(&self) -> usize {
        self.current_line
    }

    /// Applies the exact line-level edits since the last classification so the
    /// unedited suffix keeps its checkpoints across insertions, deletions and
    /// disjoint replacements.
    ///
    /// Edits apply in order, each in the document the previous one left, so
    /// disjoint replacements re-key correctly. Edited lines lose checkpoints;
    /// unedited lines below only shift. Validity pulls back to the topmost
    /// edited line and re-extends only on reconvergence (see
    /// [`highlight_line`](Self::highlight_line)).
    pub(crate) fn note_edits(&mut self, edits: &[parser::LineEdit]) {
        if edits.is_empty() {
            return;
        }

        let shift = |value: usize, at: usize, removed: usize, delta: isize| -> usize {
            let removed_end = at + removed;
            if value >= removed_end {
                (value as isize + delta) as usize
            } else if value >= at {
                at
            } else {
                value
            }
        };

        let mut dirty_from = usize::MAX;
        for edit in edits {
            let at = edit.at;
            let removed_end = at + edit.removed;
            let delta = edit.inserted as isize - edit.removed as isize;

            let mut rekeyed = BTreeMap::new();
            for (&key, checkpoint) in &self.checkpoints {
                if key < at {
                    rekeyed.insert(key, checkpoint.clone());
                } else if key >= removed_end {
                    rekeyed.insert((key as isize + delta) as usize, checkpoint.clone());
                }
                // Keys in `[at, removed_end)` are the edited lines; drop them.
            }
            self.checkpoints = rekeyed;

            dirty_from = if dirty_from == usize::MAX {
                at
            } else {
                shift(dirty_from, at, edit.removed, delta).min(at)
            };
            self.current_line = shift(self.current_line, at, edit.removed, delta);
            self.valid_through = shift(self.valid_through, at, edit.removed, delta);
        }

        self.valid_through = self.valid_through.min(dirty_from);
        self.reuse_pending = true;
        self.restart_at(dirty_from);
    }

    fn restart_at(&mut self, line: usize) {
        if let Some((&checkpoint_line, checkpoint)) = self.checkpoints.range(..=line).next_back() {
            self.current_line = checkpoint_line;
            self.state = checkpoint.start.clone();
        } else {
            self.current_line = 0;
            self.state = LexicalState::Code;
        }
    }

    pub(crate) fn seek_line(&mut self, line: usize) -> bool {
        // Only the current cursor owns revalidated state at the frontier;
        // stored checkpoints there can still predate the edit.
        if line >= self.valid_through && line != self.current_line {
            self.work.replay_seeks += 1;
            return false;
        }
        if line == self.current_line {
            self.work.checkpoint_seeks += 1;
            return true;
        }
        if line == 0 {
            self.current_line = 0;
            self.state = LexicalState::Code;
            self.work.checkpoint_seeks += 1;
            return true;
        }
        let Some(checkpoint) = self.checkpoints.get(&line) else {
            self.work.replay_seeks += 1;
            return false;
        };

        self.current_line = line;
        self.state = checkpoint.start.clone();
        self.work.checkpoint_seeks += 1;
        true
    }

    pub(crate) fn work(&self) -> SyntaxWork {
        self.work
    }

    pub(crate) fn resident_checkpoints(&self) -> usize {
        self.checkpoints.len()
    }

    pub(crate) fn reset_work(&mut self) {
        self.work = SyntaxWork::default();
    }
}

fn tokenize_line_from_state(
    line: &str,
    language: Language,
    state: &LexicalState,
) -> (Vec<Token>, LexicalState) {
    let prefix = state_prefix(state);
    let source;
    let input = if prefix.is_empty() {
        line
    } else {
        source = format!("{prefix}{line}");
        source.as_str()
    };

    let all = tokenize(input, language);
    let next = ending_state(input, language, &all);
    let prefix_len = prefix.len();
    let tokens = all
        .into_iter()
        .filter_map(|token| {
            let start = token.start.max(prefix_len) - prefix_len;
            let end = token.end.saturating_sub(prefix_len).min(line.len());
            (start < end).then_some(Token {
                start,
                end,
                class: token.class,
            })
        })
        .collect();

    (tokens, next)
}

fn state_prefix(state: &LexicalState) -> String {
    match state {
        LexicalState::Code => String::new(),
        LexicalState::BlockComment(depth) => "/*".repeat(*depth),
        LexicalState::Quoted(quote) => char::from(*quote).to_string(),
        LexicalState::RustRaw(hashes) => format!("r{}\"", "#".repeat(*hashes)),
        LexicalState::PythonTriple(quote) => char::from(*quote).to_string().repeat(3),
        LexicalState::MarkdownFence(ticks) => format!("{}\n", "`".repeat(*ticks)),
        LexicalState::MarkdownComment => "<!--".to_owned(),
    }
}

fn ending_state(source: &str, language: Language, tokens: &[Token]) -> LexicalState {
    let Some(last) = tokens.last().filter(|token| token.end == source.len()) else {
        return LexicalState::Code;
    };
    let text = &source[last.start..last.end];

    match (language, last.class) {
        (Language::Rust, TokenClass::Comment) if text.starts_with("/*") => {
            let depth = block_comment_depth(text, true);
            if depth > 0 {
                LexicalState::BlockComment(depth)
            } else {
                Default::default()
            }
        }
        (Language::C, TokenClass::Comment) if text.starts_with("/*") => {
            if !text.contains("*/") {
                LexicalState::BlockComment(1)
            } else {
                Default::default()
            }
        }
        (Language::Rust, TokenClass::String) if rust_raw_hashes(text).is_some() => {
            let hashes = rust_raw_hashes(text).unwrap_or_default();
            if !raw_string_closed(text, hashes) {
                LexicalState::RustRaw(hashes)
            } else {
                Default::default()
            }
        }
        (Language::Python, TokenClass::String)
            if text.starts_with("\"\"\"") || text.starts_with("'''") =>
        {
            let quote = text.as_bytes()[0];
            if !triple_string_closed(text, quote) {
                LexicalState::PythonTriple(quote)
            } else {
                Default::default()
            }
        }
        (Language::Markdown, TokenClass::Comment) if text.starts_with("<!--") => {
            if !text.ends_with("-->") {
                LexicalState::MarkdownComment
            } else {
                Default::default()
            }
        }
        (Language::Markdown, TokenClass::String) => {
            let ticks = text.bytes().take_while(|byte| *byte == b'`').count();
            if ticks >= 3 && !markdown_fence_closed(text, ticks) {
                LexicalState::MarkdownFence(ticks)
            } else {
                Default::default()
            }
        }
        (
            Language::Rust | Language::C | Language::Python | Language::Json | Language::Shell,
            TokenClass::String,
        ) if matches!(text.as_bytes().first(), Some(b'\'' | b'\"')) => {
            let quote = text.as_bytes()[0];
            if !quoted_string_closed(text, quote) {
                LexicalState::Quoted(quote)
            } else {
                Default::default()
            }
        }
        _ => LexicalState::Code,
    }
}

fn block_comment_depth(text: &str, nested: bool) -> usize {
    let bytes = text.as_bytes();
    let mut position = 0;
    let mut depth = 0usize;
    while position + 1 < bytes.len() {
        match (bytes[position], bytes[position + 1]) {
            (b'/', b'*') if nested || depth == 0 => {
                depth += 1;
                position += 2;
            }
            (b'*', b'/') if depth > 0 => {
                depth -= 1;
                position += 2;
            }
            _ => position += 1,
        }
    }
    depth
}

fn rust_raw_hashes(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.first() != Some(&b'r') {
        return None;
    }
    let hashes = bytes[1..].iter().take_while(|byte| **byte == b'#').count();
    (bytes.get(hashes + 1) == Some(&b'\"')).then_some(hashes)
}

fn raw_string_closed(text: &str, hashes: usize) -> bool {
    let opener = hashes + 2;
    let closer = format!("\"{}", "#".repeat(hashes));
    text.get(opener..)
        .is_some_and(|body| body.contains(&closer))
}

fn quoted_string_closed(text: &str, quote: u8) -> bool {
    let bytes = text.as_bytes();
    let mut position = 1;
    while position < bytes.len() {
        if bytes[position] == b'\\' {
            position += 2;
        } else if bytes[position] == quote {
            return true;
        } else {
            position += 1;
        }
    }
    false
}

fn triple_string_closed(text: &str, quote: u8) -> bool {
    let bytes = text.as_bytes();
    let mut position = 3;
    while position + 2 < bytes.len() {
        if bytes[position] == b'\\' {
            position += 2;
        } else if bytes[position..].starts_with(&[quote, quote, quote]) {
            return true;
        } else {
            position += 1;
        }
    }
    false
}

fn markdown_fence_closed(text: &str, ticks: usize) -> bool {
    text.split_once('\n').is_some_and(|(_, body)| {
        body.lines().any(|line| {
            line.trim_start()
                .bytes()
                .take_while(|byte| *byte == b'`')
                .count()
                >= ticks
        })
    })
}

/// Stateful editable-text syntax classifier with reusable line checkpoints.
///
/// Multi-line constructs match [`tokenize`]. A direct seek succeeds only when
/// that line has a valid checkpoint; otherwise it refuses the seek so iced
/// replays source lines instead of guessing a reset-to-code state. Changes
/// invalidate the following suffix, which becomes seekable only as its lines
/// are replayed; equal line text never proves positional identity after edits.
#[derive(Debug, Clone)]
pub struct SyntaxHighlighter {
    engine: StatefulSyntaxEngine,
}

impl SyntaxHighlighter {
    /// Returns deterministic lexical-work counters.
    #[must_use]
    pub fn work(&self) -> SyntaxWork {
        self.engine.work()
    }

    /// Returns the number of retained checkpoints.
    #[must_use]
    pub fn resident_checkpoints(&self) -> usize {
        self.engine.resident_checkpoints()
    }

    /// Resets the lexical-work counters without changing checkpoints.
    pub fn reset_work(&mut self) {
        self.engine.reset_work();
    }
}

impl parser::Parser for SyntaxHighlighter {
    type Settings = Language;
    type Output = TokenClass;
    type Iterator<'a> = std::iter::FilterMap<
        std::vec::IntoIter<Token>,
        fn(Token) -> Option<(Range<usize>, TokenClass)>,
    >;

    fn new(settings: &Self::Settings) -> Self {
        Self {
            engine: StatefulSyntaxEngine::new(*settings),
        }
    }

    fn update(&mut self, new_settings: &Self::Settings) {
        self.engine.update_language(*new_settings);
    }

    fn change_line(&mut self, line: usize) {
        self.engine.change_line(line);
    }

    fn parse_line(&mut self, line: &str) -> Self::Iterator<'_> {
        fn painted(token: Token) -> Option<(Range<usize>, TokenClass)> {
            tone_for_class(token.class).map(|_| (token.start..token.end, token.class))
        }
        self.engine
            .highlight_line(line)
            .into_iter()
            .filter_map(painted as fn(Token) -> Option<(Range<usize>, TokenClass)>)
    }

    fn current_line(&self) -> usize {
        self.engine.current_line()
    }

    fn seek_line(&mut self, line: usize) -> bool {
        self.engine.seek_line(line)
    }

    fn note_edits(&mut self, edits: &[parser::LineEdit]) {
        self.engine.note_edits(edits);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

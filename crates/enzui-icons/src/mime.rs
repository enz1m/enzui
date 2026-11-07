//! Pure MIME resolution: shared-mime-info glob precedence, typed certainty,
//! and the ordered generic-icon fallback chain.
//!
//! I/O-free by design: a MIME type is resolved from a filename against an
//! in-memory table of glob rules (the parsed equivalent of freedesktop's
//! `globs2`), and icon-name candidates are derived from a MIME type. Nothing
//! here reads a file, sniffs content or touches a disk.
//!
//! # Precedence (shared-mime-info)
//!
//! When several glob rules match a name, the winner is chosen by, in order:
//!
//! 1. Literal filename rules beat every wildcard rule (`Makefile` wins over
//!    `*file`).
//! 2. Higher weight wins (default weight is 50, max 100).
//! 3. Longer pattern wins (`*.tar.gz` beats `*.gz`).
//! 4. Case-sensitive rules beat case-insensitive ones.
//!
//! Matching is case-insensitive unless a rule sets [`GlobRule::case_sensitive`],
//! as the spec requires: `IMAGE.GIF` still matches `*.gif`, while `main.C` can
//! be distinguished from `main.c`.
//!
//! # Example
//! ```
//! use enzui_icons::mime::{GlobRule, GlobTable, Certainty};
//!
//! let table = GlobTable::new(vec![
//!     GlobRule::literal("text/x-makefile", "Makefile"),
//!     GlobRule::glob("application/octet-stream", "*file"),
//! ]);
//!
//! let m = table.resolve("Makefile").unwrap();
//! assert_eq!(m.mime, "text/x-makefile");
//! assert_eq!(m.certainty, Certainty::FilenameLiteral);
//! ```

use std::collections::HashSet;
use std::hash::Hash;

/// Where a MIME determination came from, and how much to trust it.
///
/// MIME is advisory here: a filename can lie and content sniffing can be
/// fooled, so a caller must be able to tell a literal filename match from a
/// suffix guess from a pure fallback.
///
/// Variants are declared from most to least certain, so the derived [`Ord`]
/// orders strongest first. Use [`Certainty::rank`] when a higher number must
/// mean stronger evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Certainty {
    /// The file's exact name matched a literal rule (`Makefile`, `.bashrc`).
    /// The strongest filename evidence — no wildcard was involved.
    FilenameLiteral,
    /// The file's name matched a glob rule (`*.rs`, `*.tar.gz`). Strong, but a
    /// renamed file will mislead it.
    FilenameGlob,
    /// The determination came from inspecting file content (magic bytes). This
    /// module never produces it; a later content-sniff phase reports through the
    /// same type.
    Content,
    /// Nothing matched; this is the generic catch-all (`application/octet-stream`).
    Fallback,
}

impl Certainty {
    /// Numeric strength, higher = more certain. Useful when a caller wants
    /// to keep the most trustworthy of two determinations.
    pub fn rank(self) -> u8 {
        match self {
            Certainty::FilenameLiteral => 3,
            Certainty::FilenameGlob => 2,
            Certainty::Content => 1,
            Certainty::Fallback => 0,
        }
    }

    /// Whether this determination came from the filename (literal or glob), as
    /// opposed to content or fallback. Filename evidence is advisory: a file
    /// can be renamed.
    pub fn is_from_filename(self) -> bool {
        matches!(self, Certainty::FilenameLiteral | Certainty::FilenameGlob)
    }
}

/// A resolved MIME determination: the type string plus how it was reached.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MimeMatch {
    /// The MIME type, e.g. `text/x-rust`.
    pub mime: String,
    /// How certain this determination is, and where it came from.
    pub certainty: Certainty,
}

/// The full result of resolving a filename against the glob table, preserving
/// ambiguity that a single [`MimeMatch`] would collapse.
///
/// One extension can be claimed by two MIME types at equal precedence: `*.m` is
/// both `text/x-objcsrc` and `text/x-matlab`. [`GlobTable::resolve`] must pick
/// one by a lexicographic tiebreak, which is arbitrary with respect to the
/// file's content, so this type reports the primary plus every
/// equal-top-precedence alternative and lets a caller sniff only when
/// [`MimeResolution::is_ambiguous`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MimeResolution {
    /// The deterministic winner: the MIME match a filename-only resolver would
    /// return. Used verbatim when [`Self::is_ambiguous`] is `false`, and as the
    /// documented fallback when an ambiguity-driven content sniff is
    /// inconclusive.
    pub primary: MimeMatch,
    /// Every distinct MIME type that ties with `primary` at the top
    /// precedence, in deterministic order, including `primary.mime`. When
    /// this has length 1 the resolution is unambiguous. When it has length ≥ 2
    /// the filename cannot decide and content should — see [`Self::is_ambiguous`].
    pub alternatives: Vec<String>,
}

impl MimeResolution {
    /// Whether the filename is genuinely ambiguous: two or more distinct MIME
    /// types tie for the top precedence, so a caller with a content sniffer
    /// should consult it.
    ///
    /// Sniffing on this condition is neither "always", which opens every file,
    /// nor "only on fallback", which leaves an ambiguous glob hit decided by an
    /// arbitrary tiebreak.
    pub fn is_ambiguous(&self) -> bool {
        self.alternatives.len() > 1
    }

    /// Whether `mime` is one of the equal-top-precedence alternatives.
    ///
    /// A content sniff may override the tiebreak only with a type the filename
    /// already put on the table; anything else is inconclusive and the primary
    /// stands.
    pub fn is_alternative(&self, mime: &str) -> bool {
        self.alternatives.iter().any(|a| a == mime)
    }
}

/// The classification of a glob pattern, which decides its precedence class
/// independently of weight. Literal beats simple-suffix beats complex.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PatternKind {
    /// A complex wildcard: contains `*`, `?` or `[` beyond a leading `*.`,
    /// e.g. `*file`, `xyz*.log`, `*.[ch]`.
    ComplexGlob,
    /// A simple suffix pattern: `*.` followed by literal characters with no
    /// further `*?[`, e.g. `*.tar.gz`, `*.rs`.
    SimpleSuffix,
    /// A literal filename with no wildcard characters, e.g. `Makefile`.
    Literal,
}

fn classify(pattern: &str) -> PatternKind {
    if let Some(rest) = pattern.strip_prefix("*.") {
        if !rest.contains(['*', '?', '[']) && !rest.is_empty() {
            return PatternKind::SimpleSuffix;
        }
        return PatternKind::ComplexGlob;
    }
    if pattern.contains(['*', '?', '[']) {
        PatternKind::ComplexGlob
    } else {
        PatternKind::Literal
    }
}

/// A single glob rule: a pattern that maps a filename to a MIME type, with the
/// weight and case-sensitivity flags from shared-mime-info's `globs2`.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct GlobRule {
    /// The MIME type this rule assigns.
    pub mime: String,
    /// The fnmatch-style pattern (`Makefile`, `*.rs`, `*.tar.gz`, `*.[ch]`).
    pub pattern: String,
    /// Glob weight; default 50, max 100. Higher weight wins ties.
    pub weight: u8,
    /// Whether the pattern must match with case preserved. When `false`
    /// (the spec default) matching is case-insensitive.
    pub case_sensitive: bool,
    /// Cached classification of `pattern`.
    kind: PatternKind,
}

impl GlobRule {
    /// A rule from a pattern and MIME type with the default weight (50) and
    /// case-insensitive matching. The pattern is classified automatically.
    pub fn glob(mime: impl Into<String>, pattern: impl Into<String>) -> Self {
        let pattern = pattern.into();
        let kind = classify(&pattern);
        Self {
            mime: mime.into(),
            pattern,
            weight: 50,
            case_sensitive: false,
            kind,
        }
    }

    /// A literal-filename rule (`Makefile`), default weight, case-insensitive.
    ///
    /// The pattern is still classified, so passing a wildcard here does not
    /// produce a mislabelled literal.
    pub fn literal(mime: impl Into<String>, pattern: impl Into<String>) -> Self {
        Self::glob(mime, pattern)
    }

    /// Builder: set the glob weight (clamped to the spec max of 100).
    pub fn with_weight(mut self, weight: u8) -> Self {
        self.weight = weight.min(100);
        self
    }

    /// Builder: mark the rule case-sensitive.
    pub fn case_sensitive_rule(mut self) -> Self {
        self.case_sensitive = true;
        self
    }

    /// Whether this rule's pattern matches `filename`, honouring the rule's
    /// case-sensitivity. Only the basename is considered — a rule never
    /// matches a directory component.
    fn matches(&self, filename: &str) -> bool {
        let name = basename(filename);
        match self.kind {
            PatternKind::Literal => {
                if self.case_sensitive {
                    name == self.pattern
                } else {
                    name.eq_ignore_ascii_case(&self.pattern)
                }
            }
            PatternKind::SimpleSuffix => {
                let suffix = self.pattern.strip_prefix('*').unwrap_or(&self.pattern); // includes the leading '.'
                if self.case_sensitive {
                    name.len() > suffix.len() && name.ends_with(suffix)
                } else {
                    ends_with_ignore_case(name, suffix)
                }
            }
            PatternKind::ComplexGlob => fnmatch(&self.pattern, name, self.case_sensitive),
        }
    }
}

fn basename(path: &str) -> &str {
    match path.rsplit_once('/') {
        Some((_, name)) => name,
        None => path,
    }
}

fn ends_with_ignore_case(name: &str, suffix: &str) -> bool {
    // Compare bytes, not string slices: `name.len() - suffix.len()` is a byte
    // offset that need not fall on a char boundary, so slicing the `&str` there
    // panics on a name like "01 - Café.ogg" tested against ".flac". A suffix
    // glob is always ASCII, so a non-ASCII tail byte simply fails to compare.
    let name = name.as_bytes();
    let suffix = suffix.as_bytes();
    if name.len() <= suffix.len() {
        // Strictly longer: a file named exactly ".rs" has nothing before the
        // dot and must not match `*.rs`.
        return false;
    }
    let tail = &name[name.len() - suffix.len()..];
    tail.eq_ignore_ascii_case(suffix)
}

/// A minimal fnmatch supporting `*`, `?` and `[...]` character classes, which
/// is all shared-mime-info glob patterns use. No `{}` brace expansion (not part
/// of the spec's fnmatch usage). Case-insensitive unless `cs`.
fn fnmatch(pattern: &str, name: &str, cs: bool) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = if cs {
        name.chars().collect()
    } else {
        name.chars().flat_map(char::to_lowercase).collect()
    };
    let pat: Vec<char> = if cs {
        p
    } else {
        pattern.chars().flat_map(char::to_lowercase).collect()
    };
    fnmatch_inner(&pat, &n)
}

fn fnmatch_inner(p: &[char], n: &[char]) -> bool {
    let mut pi = 0;
    let mut ni = 0;
    // Backtracking positions for the most recent '*'.
    let mut star_p: Option<usize> = None;
    let mut star_n = 0;

    while ni < n.len() {
        if pi < p.len() {
            match p[pi] {
                '*' => {
                    star_p = Some(pi);
                    star_n = ni;
                    pi += 1;
                    continue;
                }
                '?' => {
                    pi += 1;
                    ni += 1;
                    continue;
                }
                '[' => {
                    if let Some((matched, next_pi)) = match_class(p, pi, n[ni]) {
                        if matched {
                            pi = next_pi;
                            ni += 1;
                            continue;
                        }
                    } else {
                        // Malformed class: treat '[' literally.
                        if p[pi] == n[ni] {
                            pi += 1;
                            ni += 1;
                            continue;
                        }
                    }
                }
                c => {
                    if c == n[ni] {
                        pi += 1;
                        ni += 1;
                        continue;
                    }
                }
            }
        }
        // Mismatch (or ran out of pattern): backtrack to last '*' if any.
        if let Some(sp) = star_p {
            pi = sp + 1;
            star_n += 1;
            ni = star_n;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Match a `[...]` class starting at `p[start]` (which is `[`). Returns
/// `Some((matched, index_after_class))` or `None` if the class is malformed
/// (no closing `]`).
fn match_class(p: &[char], start: usize, ch: char) -> Option<(bool, usize)> {
    let mut i = start + 1;
    let mut negate = false;
    if i < p.len() && (p[i] == '!' || p[i] == '^') {
        negate = true;
        i += 1;
    }
    let mut matched = false;
    let class_start = i;
    while i < p.len() {
        if p[i] == ']' && i > class_start {
            let result = matched ^ negate;
            return Some((result, i + 1));
        }
        // Range a-z (but not if '-' is first or last).
        if i + 2 < p.len() && p[i + 1] == '-' && p[i + 2] != ']' {
            let lo = p[i];
            let hi = p[i + 2];
            if lo <= ch && ch <= hi {
                matched = true;
            }
            i += 3;
        } else {
            if p[i] == ch {
                matched = true;
            }
            i += 1;
        }
    }
    None // no closing ']'
}

/// An in-memory table of glob rules, the parsed equivalent of `globs2`.
#[derive(Clone, Debug, Default)]
pub struct GlobTable {
    rules: Vec<GlobRule>,
}

impl GlobTable {
    /// Build a table from a set of rules. Order does not matter — precedence
    /// is decided entirely by the shared-mime-info comparison, not by input
    /// order, so a rule appearing first in the vec cannot beat a
    /// higher-precedence rule appearing later.
    pub fn new(rules: Vec<GlobRule>) -> Self {
        Self { rules }
    }

    /// Resolve `filename` to a MIME type, or `None` if no rule matches.
    ///
    /// Among all matching rules the winner is chosen by the full
    /// shared-mime-info precedence: literal, then simple-suffix, then
    /// complex-glob; then higher weight, longer pattern, and case-sensitive.
    /// The spec leaves further ties unspecified; they resolve to the
    /// lexicographically smaller pattern so results are stable.
    pub fn resolve(&self, filename: &str) -> Option<MimeMatch> {
        self.resolve_with(filename, &NoResolutionObserver)
    }

    /// Primary-only lookup. Unlike ambiguous resolution, this never allocates
    /// or deduplicates an alternatives list that its caller would discard.
    fn resolve_with<O: ResolutionObserver>(
        &self,
        filename: &str,
        observer: &O,
    ) -> Option<MimeMatch> {
        let mut best: Option<&GlobRule> = None;
        for rule in &self.rules {
            observer.rule_visited();
            if !rule.matches(filename) {
                continue;
            }
            let replace = match best {
                None => true,
                Some(current) => precedence(rule)
                    .cmp(&precedence(current))
                    .then_with(|| current.mime.cmp(&rule.mime))
                    .is_gt(),
            };
            if replace {
                best = Some(rule);
            }
        }
        best.map(mime_match)
    }

    /// Resolve `filename`, preserving ambiguity rather than collapsing it to a
    /// single winner.
    ///
    /// `primary` is chosen by the full shared-mime-info precedence, exactly as
    /// [`resolve`](Self::resolve) chooses it. `alternatives` holds the distinct
    /// MIME types whose precedence key equals the primary's, so a
    /// lower-precedence match is not an alternative: it is beaten by the
    /// filename rather than tied with it.
    ///
    /// Returns `None` when nothing matches. When it returns `Some`,
    /// `alternatives` always contains at least `primary.mime`.
    pub fn resolve_ambiguous(&self, filename: &str) -> Option<MimeResolution> {
        self.resolve_ambiguous_with(filename, &NoResolutionObserver)
    }

    /// Single-pass ambiguous lookup. The vector preserves database order while
    /// the randomized set makes duplicate membership constant-time on average.
    fn resolve_ambiguous_with<O: ResolutionObserver>(
        &self,
        filename: &str,
        observer: &O,
    ) -> Option<MimeResolution> {
        let mut best: Option<&GlobRule> = None;
        let mut alternatives = Vec::new();
        let mut seen: HashSet<ObservedMime<'_, '_, O>> = HashSet::with_capacity(self.rules.len());

        for rule in &self.rules {
            observer.rule_visited();
            if !rule.matches(filename) {
                continue;
            }

            let ordering = best.map_or(std::cmp::Ordering::Greater, |current| {
                precedence(rule).cmp(&precedence(current))
            });
            if ordering.is_gt() {
                best = Some(rule);
                alternatives.clear();
                seen.clear();
                if seen.insert(ObservedMime::new(&rule.mime, observer)) {
                    alternatives.push(rule.mime.clone());
                }
            } else if ordering.is_eq() {
                if best.is_none_or(|best| rule.mime < best.mime) {
                    best = Some(rule);
                }
                if seen.insert(ObservedMime::new(&rule.mime, observer)) {
                    alternatives.push(rule.mime.clone());
                }
            }
        }

        let best = best?;
        Some(MimeResolution {
            primary: mime_match(best),
            alternatives,
        })
    }

    /// Resolve `filename`, falling back to `application/octet-stream` with
    /// [`Certainty::Fallback`] when nothing matches. This is the call callers
    /// use when they always need *some* answer.
    pub fn resolve_or_fallback(&self, filename: &str) -> MimeMatch {
        self.resolve(filename).unwrap_or_else(|| MimeMatch {
            mime: "application/octet-stream".to_owned(),
            certainty: Certainty::Fallback,
        })
    }

    /// Like [`resolve_ambiguous`](Self::resolve_ambiguous) but never `None`:
    /// when nothing matches it returns the `application/octet-stream`
    /// [`Certainty::Fallback`] as a single-alternative, unambiguous resolution.
    pub fn resolve_ambiguous_or_fallback(&self, filename: &str) -> MimeResolution {
        self.resolve_ambiguous(filename).unwrap_or_else(|| {
            let mime = "application/octet-stream".to_owned();
            MimeResolution {
                primary: MimeMatch {
                    mime: mime.clone(),
                    certainty: Certainty::Fallback,
                },
                alternatives: vec![mime],
            }
        })
    }
}

fn mime_match(rule: &GlobRule) -> MimeMatch {
    let certainty = match rule.kind {
        PatternKind::Literal => Certainty::FilenameLiteral,
        PatternKind::SimpleSuffix | PatternKind::ComplexGlob => Certainty::FilenameGlob,
    };
    MimeMatch {
        mime: rule.mime.clone(),
        certainty,
    }
}

trait ResolutionObserver {
    fn rule_visited(&self) {}
    fn hash_probe(&self) {}
    fn equality_probe(&self) {}
}

struct NoResolutionObserver;
impl ResolutionObserver for NoResolutionObserver {}

// A set key that reports work where the membership implementation performs it:
// one hash probe per real `HashSet` insert, or one comparison per element under
// a `Vec::contains` sabotage. The key holds no counters; the observer decides
// which of the two reports counts as membership work.
struct ObservedMime<'mime, 'observer, O: ResolutionObserver> {
    mime: &'mime str,
    observer: &'observer O,
}

impl<'mime, 'observer, O: ResolutionObserver> ObservedMime<'mime, 'observer, O> {
    fn new(mime: &'mime str, observer: &'observer O) -> Self {
        Self { mime, observer }
    }
}

impl<O: ResolutionObserver> std::hash::Hash for ObservedMime<'_, '_, O> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.observer.hash_probe();
        self.mime.hash(state);
    }
}

impl<O: ResolutionObserver> PartialEq for ObservedMime<'_, '_, O> {
    fn eq(&self, other: &Self) -> bool {
        self.observer.equality_probe();
        self.mime == other.mime
    }
}

impl<O: ResolutionObserver> Eq for ObservedMime<'_, '_, O> {}

/// The comparable precedence key for a rule. `max_by` picks the largest, so
/// higher tuple = higher precedence.
///
/// The field order encodes the shared-mime-info precedence exactly:
///
/// 1. `is_literal` — literal filename rules beat every wildcard rule
///    unconditionally ("Literal patterns must be matched before all others"),
///    regardless of weight. This is why it sorts above `weight`.
/// 2. `weight` — the primary sort among wildcard rules ("lines are ordered by
///    glob weight"); default 50, max 100.
/// 3. `is_simple` — among equal-weight wildcards, simple `*.ext` suffix
///    patterns are matched before other wildcarded patterns.
/// 4. `pattern_len` — same weight and class: the longer pattern wins
///    (`*.tar.gz` over `*.gz`).
/// 5. `case_sensitive` — a case-sensitive rule beats a case-insensitive one.
/// 6. `pattern` (inverted) — final deterministic tiebreak so tests are stable.
fn precedence(r: &GlobRule) -> PrecedenceKey<'_> {
    PrecedenceKey {
        is_literal: r.kind == PatternKind::Literal,
        weight: r.weight,
        is_simple: r.kind == PatternKind::SimpleSuffix,
        pattern_len: r.pattern.len(),
        case_sensitive: r.case_sensitive,
        pattern: &r.pattern,
    }
}

/// Ordered precedence key. Larger compares as higher precedence for every field
/// except `pattern`, whose comparison is inverted so the lexicographically
/// smaller pattern wins the final tiebreak.
struct PrecedenceKey<'a> {
    is_literal: bool,
    weight: u8,
    is_simple: bool,
    pattern_len: usize,
    case_sensitive: bool,
    pattern: &'a str,
}

impl PartialEq for PrecedenceKey<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}
impl Eq for PrecedenceKey<'_> {}
impl PartialOrd for PrecedenceKey<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for PrecedenceKey<'_> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.is_literal
            .cmp(&other.is_literal)
            .then(self.weight.cmp(&other.weight))
            .then(self.is_simple.cmp(&other.is_simple))
            .then(self.pattern_len.cmp(&other.pattern_len))
            .then(self.case_sensitive.cmp(&other.case_sensitive))
            // Final deterministic tiebreak: smaller pattern wins, so invert.
            .then_with(|| other.pattern.cmp(self.pattern))
    }
}

/// The ordered icon-name candidates for a MIME type, following the freedesktop
/// generic-icon fallback chain and guaranteed to end at a name
/// [`crate::bundled_icon`] resolves.
///
/// For `text/x-python` the chain is `text-x-python`, `text-x-generic`,
/// `text-plain`, then the bundled Lucide category icon. That trailing name is
/// what stops a chain dead-ending in a blank slot: even a wholly unknown type
/// resolves to `"file"`.
///
/// `generic` may supply the shared-mime-info `generic-icon` name to insert
/// after the specific name; `None` uses only the derived media-generic.
pub fn icon_candidates(mime: &str, generic: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    let mut push = |s: String| {
        if !out.contains(&s) {
            out.push(s);
        }
    };

    let (media, sub) = split_mime(mime);

    if !media.is_empty() {
        if sub.is_empty() {
            push(media.to_owned());
        } else {
            push(format!("{media}-{sub}"));
        }
    }

    if let Some(g) = generic
        && !g.is_empty()
    {
        push(g.to_owned());
    }

    if !media.is_empty() {
        push(format!("{media}-x-generic"));
    }

    // All `text/*` are subclasses of `text/plain`.
    if media == "text" {
        push("text-plain".to_owned());
    }

    // Terminal: always a name `bundled_icon()` resolves, defaulting to "file".
    push(bundled_terminal(media).to_owned());

    out
}

/// Resolve a MIME type to the first candidate name this crate's
/// [`crate::bundled_icon`] can actually draw.
///
/// The early names in [`icon_candidates`] (`image-png`, `text-x-python`) are
/// host icon-theme names and are absent from the bundled Lucide catalog, so a
/// consumer without host theme assets cannot use `candidates.first()`. This
/// walks the chain and returns the first bundled name, which always exists
/// because every chain terminates at one; the final `"file"` is a guard for a
/// chain that somehow does not.
///
/// Use this for the bundled drawing path. For a themed lookup against on-disk
/// host assets, use the specific names from [`icon_candidates`] directly.
pub fn bundled_icon_candidate(mime: &str, generic: Option<&str>) -> String {
    icon_candidates(mime, generic)
        .into_iter()
        .find(|c| crate::bundled_icon(c).is_some())
        .unwrap_or_else(|| "file".to_owned())
}

/// The bundled Lucide icon that terminates a candidate chain for a media type.
/// Every returned name is bundled, so a chain cannot dead-end in a blank slot.
fn bundled_terminal(media: &str) -> &'static str {
    match media {
        "image" => "image",
        "audio" => "music",
        "video" => "video",
        "inode" => "folder",
        // text, application, and everything else fall to the generic file glyph.
        _ => "file",
    }
}

fn split_mime(mime: &str) -> (&str, &str) {
    match mime.split_once('/') {
        Some((m, s)) => (m, s),
        None => (mime, ""),
    }
}

/// The outcome of parsing a freedesktop `globs2` document: the accepted rules
/// plus a count of lines rejected as malformed.
///
/// A `globs2` file comes from the user's machine and will contain lines this
/// parser does not understand. One bad line never discards the rest of the
/// database, and the reject count is reported rather than swallowed so a health
/// check can weigh it against [`ParsedGlobs::rules`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParsedGlobs {
    /// The rules that parsed successfully, in file order (order does not affect
    /// precedence — see [`GlobTable::new`]).
    pub rules: Vec<GlobRule>,
    /// The MIME types this file marks with a `__NOGLOBS__` sentinel: every glob
    /// for these types from lower-priority sources (later in the XDG search
    /// order) must be discarded. Empty on a file with no sentinels.
    ///
    /// The sentinel is a directive, not a pattern: emitting it as a literal
    /// rule would turn a request to suppress inherited rules into an extra one.
    pub suppressions: Vec<String>,
    /// How many non-blank, non-comment lines were rejected as malformed; `0` on
    /// a clean file. A count rather than the offending text.
    pub rejected: usize,
}

/// The `globs2` pattern that is not a pattern: a sentinel meaning "discard all
/// globs for this MIME type inherited from lower-priority directories". Written
/// by `update-mime-database` from a `<glob-deleteall>` element. See
/// [`ParsedGlobs::suppressions`].
const NOGLOBS_SENTINEL: &str = "__NOGLOBS__";

/// Parse a freedesktop shared-mime-info `globs2` document into rules.
///
/// A pure function: `&str` in, rules out. It performs no I/O, reads no
/// environment and touches no path; discovery and the XDG search order live in
/// [`xdg`]. To resolve a filename, wrap the result in a [`GlobTable`].
///
/// # Format
///
/// One rule per line, `weight:mimetype:glob[:flags[:…]]`, following the field
/// grammar of the reference parser (`xdgmimeglob.c`
/// `_xdg_mime_glob_read_from_file`):
///
/// - weight: text up to the first colon, required to be a base-10 integer here.
/// - mimetype: text up to the second colon.
/// - glob: text up to the first colon of field three, or the rest of the line.
///   A glob therefore cannot contain a colon.
/// - flags: a comma-separated list between the third and fourth colons. Only
///   `cs` (case-sensitive) is acted on, matched as a whole token; unknown flags
///   are ignored rather than absorbed into the glob.
/// - anything after the fourth colon is ignored.
///
/// Lines beginning with `#` and blank lines are ignored. A leading UTF-8 BOM and
/// `\r\n` line endings are tolerated. The line is not otherwise trimmed:
/// trailing spaces and tabs are significant pattern bytes.
///
/// # The `__NOGLOBS__` sentinel
///
/// A glob of exactly `__NOGLOBS__` is a directive meaning "discard every glob
/// for this MIME type inherited from lower-priority directories". Such a line
/// produces no [`GlobRule`]; its MIME type is recorded in
/// [`ParsedGlobs::suppressions`] for the merge layer.
///
/// # Malformed lines are skipped, not fatal
///
/// A line is rejected, counted in [`ParsedGlobs::rejected`] and never fatal,
/// when it has fewer than three fields, an empty MIME type, an empty glob, or a
/// weight that is not a base-10 integer. A weight above the spec maximum of 100
/// is clamped like [`GlobRule::with_weight`]; a weight of `0` is honoured as the
/// lowest valid priority rather than treated as missing.
pub fn parse_globs2(text: &str) -> ParsedGlobs {
    let mut rules = Vec::new();
    let mut suppressions = Vec::new();
    let mut rejected = 0usize;

    for (idx, raw) in text.lines().enumerate() {
        let line = normalized_globs2_line(idx, raw);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        match parse_globs2_line(line) {
            LineOutcome::Rule {
                mime,
                glob,
                weight,
                case_sensitive,
            } => {
                let mut rule = GlobRule::glob(mime, glob);
                rule.weight = weight;
                rule.case_sensitive = case_sensitive;
                rules.push(rule);
            }
            LineOutcome::Suppress(mime) => suppressions.push(mime.to_owned()),
            LineOutcome::Reject => rejected += 1,
        }
    }

    ParsedGlobs {
        rules,
        suppressions,
        rejected,
    }
}

fn normalized_globs2_line(index: usize, raw: &str) -> &str {
    let line = if index == 0 {
        raw.strip_prefix('\u{feff}').unwrap_or(raw)
    } else {
        raw
    };
    line.strip_suffix('\r').unwrap_or(line)
}

/// The result of parsing a single `globs2` line, borrowing fields so a loader
/// can validate a whole candidate before retaining any strings from it.
enum LineOutcome<'a> {
    /// A well-formed glob rule.
    Rule {
        mime: &'a str,
        glob: &'a str,
        weight: u8,
        case_sensitive: bool,
    },
    /// A `__NOGLOBS__` sentinel for the given MIME type — suppress inherited
    /// globs for it, emit no rule.
    Suppress(&'a str),
    /// The line is malformed and is counted as a reject.
    Reject,
}

/// Parse one non-comment `globs2` line into a rule, a suppression, or a reject.
///
/// Field splitting mirrors the reference `xdgmimeglob.c`: the glob field ends at
/// its first colon and everything after is the comma-separated flags field, so a
/// glob cannot contain a colon.
fn parse_globs2_line(line: &str) -> LineOutcome<'_> {
    let Some((weight_str, after_weight)) = line.split_once(':') else {
        return LineOutcome::Reject; // fewer than 2 fields
    };
    let Some((mime, field3)) = after_weight.split_once(':') else {
        return LineOutcome::Reject; // fewer than 3 fields
    };

    if mime.is_empty() {
        return LineOutcome::Reject;
    }

    // The weight must be a base-10 integer: a non-numeric or negative weight
    // rejects the line instead of silently becoming the default. `u32` lets an
    // over-range value parse and clamp like `with_weight`, while a sign fails.
    let Ok(weight) = weight_str.parse::<u32>() else {
        return LineOutcome::Reject;
    };
    let weight = weight.min(100) as u8;

    let (glob, flags) = match field3.split_once(':') {
        Some((g, rest)) => {
            // Flags run to the next colon; anything past that is a further
            // extension field we ignore.
            let flags = rest.split_once(':').map_or(rest, |(f, _)| f);
            (g, flags)
        }
        None => (field3, ""),
    };

    if glob.is_empty() {
        return LineOutcome::Reject;
    }

    // `__NOGLOBS__` is a directive, not a pattern.
    if glob == NOGLOBS_SENTINEL {
        return LineOutcome::Suppress(mime);
    }

    LineOutcome::Rule {
        mime,
        glob,
        weight,
        case_sensitive: flags_contain_cs(flags),
    }
}

/// Whether the comma-separated `flags` field contains `cs` (case-sensitive) as a
/// whole token, as the reference parser requires: `cs`, `cs,foo` and `foo,cs`
/// set it, while `cset`, `foocs` and `xcs` do not. Unknown flags are ignored.
fn flags_contain_cs(flags: &str) -> bool {
    flags.split(',').any(|tok| tok == "cs")
}

/// One shared structural budget for the attacker-influenced MIME database.
/// The loader injects a smaller copy in tests, while production uses
/// [`PRODUCTION_MIME_BUDGET`] for discovery, reading, parsing, and merging.
#[derive(Clone, Copy, Debug)]
pub(super) struct MimeDatabaseBudget {
    pub(super) max_candidates: usize,
    pub(super) max_aggregate_bytes: u64,
    pub(super) max_file_bytes: u64,
    pub(super) max_records: usize,
    pub(super) max_rules: usize,
    pub(super) max_suppressions: usize,
    pub(super) max_field_bytes: usize,
}

pub(super) const PRODUCTION_MIME_BUDGET: MimeDatabaseBudget = MimeDatabaseBudget {
    max_candidates: 64,
    max_aggregate_bytes: 32 * 1024 * 1024,
    max_file_bytes: 16 * 1024 * 1024,
    max_records: 262_144,
    max_rules: 65_536,
    max_suppressions: 16_384,
    max_field_bytes: 4_096,
};

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Globs2Structure {
    pub(super) records: usize,
    pub(super) rules: usize,
    pub(super) suppressions: usize,
}

/// Validate an entire candidate without allocating its retained MIME/pattern
/// strings. `Err` reports the bounded prefix inspected before the whole
/// candidate was rejected, so aggregate work remains accounted across files.
pub(super) fn inspect_globs2_structure(
    text: &str,
    remaining: MimeDatabaseBudget,
) -> Result<Globs2Structure, Globs2Structure> {
    let mut structure = Globs2Structure::default();
    for (index, raw) in text.lines().enumerate() {
        structure.records = structure.records.saturating_add(1);
        if structure.records > remaining.max_records {
            return Err(structure);
        }

        let line = normalized_globs2_line(index, raw);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((mime, glob)) = globs2_fields(line)
            && (mime.len() > remaining.max_field_bytes || glob.len() > remaining.max_field_bytes)
        {
            return Err(structure);
        }

        match parse_globs2_line(line) {
            LineOutcome::Rule { .. } => {
                structure.rules = structure.rules.saturating_add(1);
                if structure.rules > remaining.max_rules {
                    return Err(structure);
                }
            }
            LineOutcome::Suppress(_) => {
                structure.suppressions = structure.suppressions.saturating_add(1);
                if structure.suppressions > remaining.max_suppressions {
                    return Err(structure);
                }
            }
            LineOutcome::Reject => {}
        }
    }
    Ok(structure)
}

fn globs2_fields(line: &str) -> Option<(&str, &str)> {
    let (_, after_weight) = line.split_once(':')?;
    let (mime, field3) = after_weight.split_once(':')?;
    let glob = field3.split_once(':').map_or(field3, |(glob, _)| glob);
    Some((mime, glob))
}

/// XDG discovery for on-disk `globs2` databases: the disk-facing layer that
/// finds the freedesktop `shared-mime-info` files in search order and feeds
/// them to [`parse_globs2`]. Kept in its own submodule so this parent module
/// stays I/O-free; only [`xdg`] touches the filesystem and environment.
pub mod xdg;

#[cfg(test)]
mod tests;

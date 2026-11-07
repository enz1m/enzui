//! Parser for the freedesktop `magic` database, as installed at
//! `/usr/share/mime/magic` on a typical Linux system.
//!
//! # Wire format
//!
//! The database is binary (value bytes carry arbitrary bytes including newlines
//! and NULs), laid out as:
//!
//! ```text
//! MIME-Magic\0\n                 (12-byte header, exactly)
//! [priority:mimetype]\n           (section header; opens a MIME group)
//! [indent]>offset=LENGTH<bytes>[&mask][~word-size][+range-length]\n
//! ...
//! ```
//!
//! where `LENGTH` is a two-byte big-endian unsigned integer counting the
//! following literal bytes of `<bytes>`. `&mask` (same length as `<bytes>`),
//! `~word-size` and `+range-length` are optional suffixes. `[indent]` is a
//! decimal nesting depth; `0` or the empty string means depth 0.
//!
//! # Scope of this parser
//!
//! - Depth 0 only. The format expresses and-trees via nested rules (`1>...`,
//!   `2>...`), and this parser skips indented continuation rules and returns
//!   only the top-level rule of each section. A rule that fires in the real
//!   database only as an and-child does not fire here, but no rule fires more
//!   loosely than intended.
//! - `~word-size` is parsed and stored in [`MagicRule::word_size`], but no
//!   endian swap is performed.
//! - `&mask` is parsed and stored in [`MagicRule::mask`]; the matcher applies it.
//! - `+range-length` is parsed and stored in [`MagicRule::range`].
//! - Priorities are parsed into [`MagicRule::priority`] and the returned list is
//!   sorted by descending priority, so `match_magic`'s first-match discipline
//!   over sorted output means the highest priority wins.
//!
//! # Malformed input
//!
//! Every recoverable defect skips the rule, keeps parsing, and increments
//! [`MagicParseResult::rejected`](crate::sniff::magic_parse::MagicParseResult::rejected):
//!
//! - a truncated line, `LENGTH` past end of input, or a truncated mask;
//! - a section header with a missing, non-numeric or out-of-range priority;
//! - a mask whose length differs from the value length (a spec violation);
//! - offset arithmetic that would overflow `usize`;
//! - a rule that appears before any section header, with no MIME to bind to.
//!
//! An indented continuation rule (depth > 0) is skipped silently and is not
//! counted as rejected: not implemented is different from malformed.
//!
//! The one defect that rejects the whole input is a missing or malformed magic
//! header. A file whose first 12 bytes are not `b"MIME-Magic\0\n"` is not a
//! magic file, and half a parse of arbitrary bytes would be worse than an empty
//! result.

use super::MagicRule;

/// Result of parsing a magic database.
#[derive(Debug, Clone)]
pub struct MagicParseResult {
    /// Parsed rules, sorted by descending [`MagicRule::priority`]. `match_magic`
    /// scans in slice order, so priority is honoured without the matcher
    /// needing to sort.
    pub rules: Vec<MagicRule>,
    /// Number of rule- or section-scoped errors skipped during parsing.
    ///
    /// Silently-skipped indented continuation rules (depth > 0) are not
    /// counted here — those are a known coverage gap, not malformed input.
    pub rejected: usize,
    /// Number of top-level (depth-0) rules that were dropped because they
    /// had required indented continuation children this flat parser cannot
    /// evaluate. Keeping such a parent alone would match too loosely and
    /// mislabel files, so the whole subtree is discarded. This is a coverage
    /// gap (and-trees are not implemented), not malformed input, so it is
    /// tracked separately from [`MagicParseResult::rejected`].
    pub dropped_parents: usize,
}

/// The magic file header, exactly 12 bytes. A file that does not start with
/// this is rejected as a whole: it is not a magic file.
pub const MAGIC_HEADER: &[u8] = b"MIME-Magic\x00\n";

/// Parse a freedesktop magic database from raw bytes.
///
/// Signature is `&[u8]` on purpose — the value bytes in the database are
/// binary and can include `\n` and `\x00`. A `&str` API would force lossy
/// UTF-8 conversion and silently corrupt those rules.
///
/// # Errors
///
/// Returns `Err(MagicParseError::BadHeader)` if the input does not begin with
/// [`MAGIC_HEADER`]. Any other malformed input inside a valid file yields a
/// successful [`MagicParseResult`] with the offending rules skipped and
/// counted in [`MagicParseResult::rejected`].
pub fn parse_magic(input: &[u8]) -> Result<MagicParseResult, MagicParseError> {
    if !input.starts_with(MAGIC_HEADER) {
        return Err(MagicParseError::BadHeader);
    }
    let mut cursor = MAGIC_HEADER.len();
    let mut rules: Vec<MagicRule> = Vec::new();
    let mut rejected: usize = 0;
    let mut current_section: Option<Section> = None;

    // A depth-0 rule is not committed immediately. In the freedesktop format
    // `<a><b/><c/></a>` means "a and (b or c)": a top-level rule with indented
    // continuation children matches only when the parent and at least one child
    // match. This parser flattens to depth 0 and does not evaluate children, so
    // keeping such a parent alone would match too loosely and mislabel files.
    // The pending depth-0 rule is therefore buffered and committed only once the
    // next rule proves not to be a deeper continuation; otherwise it is dropped
    // and counted as a coverage gap.
    let mut pending: Option<MagicRule> = None;
    let mut dropped_parents: usize = 0;

    while cursor < input.len() {
        let first = input[cursor];

        if first == b'[' {
            // A new section ends any pending subtree: commit the buffered
            // depth-0 rule (it had no children).
            if let Some(r) = pending.take() {
                rules.push(r);
            }
            // Section header. Read to next newline (headers are ASCII, no
            // binary payload, so scanning for \n is safe here).
            let Some(line_end) = find_byte(input, cursor, b'\n') else {
                // Truncated header line at EOF.
                rejected += 1;
                break;
            };
            let line = &input[cursor..line_end];
            cursor = line_end + 1;
            match parse_section_header(line) {
                Some(s) => current_section = Some(s),
                None => {
                    rejected += 1;
                    // Also drop the current section so orphaned rules under
                    // a broken header don't get bound to the previous MIME.
                    current_section = None;
                }
            }
            continue;
        }

        // A rule line carries a length-prefixed binary payload, so the parser
        // must honour LENGTH rather than scan for a newline.
        match parse_rule_line(input, cursor) {
            RuleParse::Ok { rule, next, depth } => {
                cursor = next;
                if depth > 0 {
                    // A nested continuation rule means the buffered depth-0
                    // parent has required children this parser cannot evaluate,
                    // so drop the parent rather than match it loosely.
                    if pending.take().is_some() {
                        dropped_parents += 1;
                    }
                    continue;
                }
                // depth == 0: a new top-level rule. Commit any previous
                // pending depth-0 rule (it had no children), then buffer this
                // one.
                if let Some(r) = pending.take() {
                    rules.push(r);
                }
                match &current_section {
                    Some(sec) => {
                        pending = Some(MagicRule {
                            mime: sec.mime.clone(),
                            offset: rule.offset,
                            bytes: rule.bytes,
                            mask: rule.mask,
                            range: rule.range,
                            priority: sec.priority,
                            word_size: rule.word_size,
                        });
                    }
                    None => {
                        // Rule with no owning section: cannot be bound to a
                        // MIME. Reject.
                        rejected += 1;
                    }
                }
            }
            RuleParse::Skip { next } => {
                cursor = next;
                rejected += 1;
            }
            RuleParse::Fatal => {
                // Cursor cannot advance safely (e.g. length runs past EOF
                // with no line terminator to skip past). Stop parsing and
                // count one reject.
                rejected += 1;
                break;
            }
        }
    }
    // Commit any final pending depth-0 rule (reached EOF with no children).
    if let Some(r) = pending.take() {
        rules.push(r);
    }

    // Sort by descending priority so `match_magic`'s "first match wins" gives
    // "highest priority wins".
    rules.sort_by_key(|r| std::cmp::Reverse(r.priority));
    Ok(MagicParseResult {
        rules,
        rejected,
        dropped_parents,
    })
}

/// Failures that reject the whole input, not just one rule.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum MagicParseError {
    /// Input did not start with `MIME-Magic\0\n`. Not a magic file.
    BadHeader,
}

// ---------------------------------------------------------------------------
// Internals

/// Section header binding all following top-level rules until the next `[`.
struct Section {
    priority: u16,
    mime: String,
}

/// Parse a `[priority:mimetype]` header line (the `[` and `]` included).
/// Returns `None` on any structural / range / encoding defect.
fn parse_section_header(line: &[u8]) -> Option<Section> {
    let inner = line.strip_prefix(b"[")?.strip_suffix(b"]")?;
    let colon = inner.iter().position(|&b| b == b':')?;
    let (pri, mime) = inner.split_at(colon);
    let mime = &mime[1..]; // skip the ':'

    let pri_str = std::str::from_utf8(pri).ok()?;
    let priority: u32 = pri_str.parse().ok()?;
    // The spec defines priorities in 0..=100. Anything outside is malformed.
    if priority > 100 {
        return None;
    }
    let mime_str = std::str::from_utf8(mime).ok()?;
    if mime_str.is_empty() {
        return None;
    }
    Some(Section {
        priority: u16::try_from(priority).ok()?,
        mime: mime_str.to_owned(),
    })
}

/// A parsed rule stripped of its section-level metadata (mime / priority).
struct ParsedRule {
    offset: usize,
    bytes: Vec<u8>,
    mask: Option<Vec<u8>>,
    range: usize,
    word_size: u16,
}

enum RuleParse {
    Ok {
        rule: ParsedRule,
        next: usize,
        depth: u32,
    },
    /// Skip this rule and continue from `next`. `next` is placed just past a
    /// newline so parsing can resume cleanly.
    Skip { next: usize },
    /// Cannot recover: the length field advertises more bytes than remain and
    /// no newline follows to anchor recovery on.
    Fatal,
}

/// Parse one rule line starting at `input[start]`. See the module docs for
/// the format.
fn parse_rule_line(input: &[u8], start: usize) -> RuleParse {
    // 1. Optional decimal depth prefix, then '>'.
    let mut i = start;
    let depth_start = i;
    while i < input.len() && input[i].is_ascii_digit() {
        i += 1;
    }
    let depth: u32 = if i == depth_start {
        0
    } else {
        // A depth prefix has to fit in u32; anything wilder is nonsense.
        match std::str::from_utf8(&input[depth_start..i])
            .ok()
            .and_then(|s| s.parse().ok())
        {
            Some(d) => d,
            None => return skip_to_next_line(input, i),
        }
    };
    if i >= input.len() || input[i] != b'>' {
        return skip_to_next_line(input, i);
    }
    i += 1;

    // 2. Decimal offset, then '='.
    let off_start = i;
    while i < input.len() && input[i].is_ascii_digit() {
        i += 1;
    }
    if off_start == i {
        return skip_to_next_line(input, i);
    }
    let offset: usize = match std::str::from_utf8(&input[off_start..i])
        .ok()
        .and_then(|s| s.parse().ok())
    {
        Some(o) => o,
        None => return skip_to_next_line(input, i),
    };
    if i >= input.len() || input[i] != b'=' {
        return skip_to_next_line(input, i);
    }
    i += 1;

    // 3. Two-byte big-endian LENGTH.
    if i + 2 > input.len() {
        // Not enough bytes for the length field itself. There's no
        // newline-anchor to resume from either; treat as fatal.
        return RuleParse::Fatal;
    }
    let length = ((input[i] as usize) << 8) | (input[i + 1] as usize);
    i += 2;

    // 4. LENGTH bytes of value. An advertised length beyond the remaining input
    // is fatal, because the rule's end is then unknown.
    if i.checked_add(length).is_none_or(|e| e > input.len()) {
        return RuleParse::Fatal;
    }
    let value = input[i..i + length].to_vec();
    i += length;

    // 5. Optional '&mask' — same length as value.
    let mut mask: Option<Vec<u8>> = None;
    if i < input.len() && input[i] == b'&' {
        i += 1;
        // Spec: mask length equals value length.
        if i.checked_add(length).is_none_or(|e| e > input.len()) {
            // Not enough bytes for the mask. Skip to next line-anchor.
            return skip_to_next_line(input, i);
        }
        mask = Some(input[i..i + length].to_vec());
        i += length;
    }

    // 6. Optional '~word-size' — decimal, spec default 1, retained on the rule
    // (see `MagicRule::word_size`) rather than discarded.
    let mut word_size: u16 = 1;
    if i < input.len() && input[i] == b'~' {
        i += 1;
        let ws_start = i;
        while i < input.len() && input[i].is_ascii_digit() {
            i += 1;
        }
        if ws_start == i {
            return skip_to_next_line(input, i);
        }
        word_size = match std::str::from_utf8(&input[ws_start..i])
            .ok()
            .and_then(|s| s.parse().ok())
        {
            Some(w) => w,
            None => return skip_to_next_line(input, i),
        };
    }

    // 7. Optional '+range-length': per spec the number of candidate positions,
    // defaulting to 1 when absent. `matches_head` examines `range` positions
    // starting at `offset`, half-open.
    let mut range: usize = 1;
    if i < input.len() && input[i] == b'+' {
        i += 1;
        let r_start = i;
        while i < input.len() && input[i].is_ascii_digit() {
            i += 1;
        }
        if r_start == i {
            return skip_to_next_line(input, i);
        }
        range = match std::str::from_utf8(&input[r_start..i])
            .ok()
            .and_then(|s| s.parse().ok())
        {
            Some(r) => r,
            None => return skip_to_next_line(input, i),
        };
    }

    // 8. Terminating newline.
    if i >= input.len() {
        // File ends with no trailing newline after a full rule. That's not
        // ambiguous, since LENGTH has bounded the value — accept it.
    } else if input[i] == b'\n' {
        i += 1;
    } else {
        // Unexpected trailing junk before the newline. Try to recover by
        // seeking to the next newline; the rule as parsed so far is
        // untrustworthy.
        return skip_to_next_line(input, i);
    }

    // 9. Offset-overflow guard: `offset + bytes.len()` and `offset + range` must
    // not wrap. `matches_head` guards too, but rejecting here keeps impossible
    // rules out of the database and counts them.
    //
    // `i` has already advanced past this rule's terminating newline, so it
    // points at the start of the next record: recovering with
    // `skip_to_next_line(input, i)` would swallow that record. Return
    // `Skip { next: i }` instead.
    if offset.checked_add(value.len()).is_none()
        || offset.checked_add(range.saturating_sub(1)).is_none()
    {
        return RuleParse::Skip { next: i };
    }

    RuleParse::Ok {
        rule: ParsedRule {
            offset,
            bytes: value,
            mask,
            range,
            word_size,
        },
        next: i,
        depth,
    }
}

/// Try to find the next newline after `pos` and return `Skip { next = pos_of_\n + 1 }`.
/// If there is no more newline in the input, this rule is unrecoverable.
fn skip_to_next_line(input: &[u8], pos: usize) -> RuleParse {
    match find_byte(input, pos, b'\n') {
        Some(e) => RuleParse::Skip { next: e + 1 },
        None => RuleParse::Fatal,
    }
}

/// First position of `b` in `input[start..]`, or `None`.
fn find_byte(input: &[u8], start: usize, b: u8) -> Option<usize> {
    input[start..]
        .iter()
        .position(|&x| x == b)
        .map(|p| p + start)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_header_rejects_whole_file() {
        // Not a magic file at all: no header. Reject the whole input, not a
        // "successful parse with 0 rules".
        let input = b"not a magic file\n[90:x/y]\n>0=\x00\x03ABC\n";
        assert!(matches!(
            parse_magic(input),
            Err(MagicParseError::BadHeader)
        ));
    }
}

//! XDG discovery for freedesktop's `shared-mime-info` `globs2` database.
//!
//! [`globs2_candidates_os`](crate::mime::xdg::globs2_candidates_os) resolves captured roots without I/O.
//! [`Globs2Env`](crate::mime::xdg::Globs2Env) snapshots OS-string environment values;
//! [`discover_globs2`](crate::mime::xdg::discover_globs2) loads the candidates.
//! Absolute non-UTF-8 Unix paths are preserved.
//!
//! Search starts at `$XDG_DATA_HOME`, falling back to `$HOME/.local/share`, then
//! follows `$XDG_DATA_DIRS` in order. Empty or unset `XDG_DATA_DIRS` uses
//! `/usr/local/share:/usr/share`; empty entries within a nonempty list are skipped.
//! Relative XDG roots are ignored. A missing, empty or relative `HOME` omits the
//! user fallback, while an explicit absolute `XDG_DATA_HOME` remains usable.
//!
//! [`load_globs2_from`](crate::mime::xdg::load_globs2_from) skips missing or unreadable files and merges their
//! rules with search-order precedence. See its documentation for duplicate-rule
//! and `__NOGLOBS__` handling; MIME weights still distinguish competing types.

use super::{
    GlobRule, MimeDatabaseBudget, PRODUCTION_MIME_BUDGET, ParsedGlobs, inspect_globs2_structure,
    parse_globs2,
};
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::hash::{BuildHasher, Hash, RandomState};
use std::io::Read;
use std::path::{Path, PathBuf};

/// The default `XDG_DATA_HOME`, relative to the home directory: `.local/share`.
const DEFAULT_DATA_HOME_TAIL: &str = ".local/share";

/// The default `XDG_DATA_DIRS` when the variable is unset or empty (the
/// spec treats both identically).
const DEFAULT_DATA_DIRS: &str = "/usr/local/share:/usr/share";

/// The relative path from a data directory to its `globs2` file.
const GLOBS2_TAIL: &str = "mime/globs2";

/// Layer 1 (pure): compute the ordered list of `globs2` candidate paths from
/// the XDG environment values, without any I/O.
///
/// `data_home` and `data_dirs` are the raw values of `$XDG_DATA_HOME` and
/// `$XDG_DATA_DIRS` as [`std::env::var`] returns them: `None` when unset,
/// `Some("")` when set to the empty string. `home` expands the
/// `XDG_DATA_HOME` default; pass `None` when `$HOME` is unset, in which case the
/// user database is omitted.
///
/// # Order
///
/// 1. `$XDG_DATA_HOME/mime/globs2`, or `<home>/.local/share/mime/globs2` when
///    `XDG_DATA_HOME` is unset, empty or relative. Omitted entirely when the
///    default would apply and `home` is `None`.
/// 2. `<dir>/mime/globs2` for each absolute, non-empty entry of
///    `$XDG_DATA_DIRS`, in order, defaulting to
///    `/usr/local/share:/usr/share` when the variable is unset or empty. Empty
///    entries are skipped and relative entries dropped while the rest of the
///    list survives.
///
/// Earlier entries appear earlier in the returned vector, which is the order in
/// which [`load_globs2_from`] applies precedence.
///
/// Paths are not deduplicated: two search entries resolving to the same file
/// appear twice, and the merge makes the second occurrence a no-op. The
/// production MIME budget caps the result at 64 candidates so an
/// attacker-controlled `XDG_DATA_DIRS` cannot create an unbounded path list;
/// within that bound the search order is preserved exactly.
///
/// ```
/// use enzui_icons::mime::xdg::globs2_candidates;
/// use std::path::PathBuf;
///
/// let got = globs2_candidates(None, None, Some("/home/u"));
/// assert_eq!(got, vec![
///     PathBuf::from("/home/u/.local/share/mime/globs2"),
///     PathBuf::from("/usr/local/share/mime/globs2"),
///     PathBuf::from("/usr/share/mime/globs2"),
/// ]);
/// ```
pub fn globs2_candidates(
    data_home: Option<&str>,
    data_dirs: Option<&str>,
    home: Option<&str>,
) -> Vec<PathBuf> {
    globs2_candidates_os(
        data_home.map(OsStr::new),
        data_dirs.map(OsStr::new),
        home.map(OsStr::new),
    )
}

/// Layer 1 (pure), byte-faithful: [`globs2_candidates`] over raw [`OsStr`]
/// values, which is what [`std::env::var_os`] yields and what the boot path
/// uses.
///
/// Same rules as [`globs2_candidates`], which wraps this. The difference is
/// that a non-UTF-8 value survives instead of collapsing into "unset".
pub fn globs2_candidates_os(
    data_home: Option<&OsStr>,
    data_dirs: Option<&OsStr>,
    home: Option<&OsStr>,
) -> Vec<PathBuf> {
    globs2_candidates_os_with_budget(data_home, data_dirs, home, PRODUCTION_MIME_BUDGET)
}

pub(super) fn globs2_candidates_os_with_budget(
    data_home: Option<&OsStr>,
    data_dirs: Option<&OsStr>,
    home: Option<&OsStr>,
    budget: MimeDatabaseBudget,
) -> Vec<PathBuf> {
    let mut out = Vec::new();

    // XDG_DATA_HOME is used only when non-empty and absolute; unset, empty or
    // relative all fall back to the `$HOME/.local/share` default.
    match data_home {
        Some(h)
            if !h.is_empty() && Path::new(h).is_absolute() && out.len() < budget.max_candidates =>
        {
            out.push(join_globs2(Path::new(h)));
        }
        _ => {
            if let Some(home) = home.filter(|h| Path::new(h).is_absolute())
                && out.len() < budget.max_candidates
            {
                let base = Path::new(home).join(DEFAULT_DATA_HOME_TAIL);
                out.push(join_globs2(&base));
            }
        }
    }

    // The spec makes an empty XDG_DATA_DIRS identical to unset: both take the
    // `/usr/local/share:/usr/share` default.
    let dirs: &OsStr = match data_dirs {
        Some(d) if !d.is_empty() => d,
        _ => OsStr::new(DEFAULT_DATA_DIRS),
    };
    // `split_paths` is the platform's own `PATH` splitter and works on bytes, so
    // a non-UTF-8 entry splits like any other.
    for entry in std::env::split_paths(dirs) {
        if out.len() == budget.max_candidates {
            break;
        }
        // Empty entries are skipped, never treated as ".". This covers leading
        // (":/a"), trailing ("/a:"), and doubled ("/a::/b") colons.
        if entry.as_os_str().is_empty() {
            continue;
        }
        // Relative entries are invalid per the spec and dropped individually,
        // never taking the rest of the list with them. A CWD-relative entry
        // would otherwise resolve against the current directory.
        if !entry.is_absolute() {
            continue;
        }
        out.push(join_globs2(&entry));
    }

    out
}

fn join_globs2(base: &Path) -> PathBuf {
    base.join(GLOBS2_TAIL)
}

/// Layer 2 (thin I/O): read each existing candidate `globs2` file, parse it, and
/// merge the rules with search-order precedence, earlier files winning.
///
/// Missing, unreadable and non-UTF-8 files are skipped and the scan continues;
/// none is an error. Most machines have `/usr/share/mime/globs2` and no
/// `~/.local/share/mime/globs2`.
///
/// # Merge semantics
///
/// [`super::GlobTable`] applies shared-mime-info precedence irrespective of
/// input order, so concatenating the files would let a high-weight rule from a
/// low-priority directory beat a deliberate user override of the same rule.
/// Rules are therefore deduplicated by `(mime, pattern, case_sensitive)`,
/// keeping the first occurrence in search order, which makes search order
/// dominant over weight for the same rule while leaving cross-pattern
/// precedence untouched.
///
/// The key includes the MIME type because two types can legitimately claim the
/// same pattern and the lookup returns all matching types for weight ordering to
/// choose between; keying on the pattern alone would discard a real alternative.
/// Only a byte-identical triple counts as a redefinition.
///
/// # `__NOGLOBS__` suppression
///
/// A sentinel (see [`ParsedGlobs::suppressions`]) for a MIME type in an earlier,
/// higher-priority file discards every glob for that type from later files, so a
/// user can remove an inherited mapping rather than only add to it. Suppression
/// accumulates down the search order; rules for that type in the same file as,
/// or earlier than, the sentinel are kept, since the sentinel only affects what
/// is inherited from below.
///
/// Returns the merged [`ParsedGlobs`] plus the number of files found and read,
/// so a caller can tell a working discovery from one that silently found
/// nothing. The returned `suppressions` list is the union of every file's
/// sentinels, for diagnostics; it has already been applied to `rules`.
///
/// Discovery and loading share one production budget: at most 64 candidate
/// files, 32 MiB total input (16 MiB per file), 262,144 physical records, 65,536
/// accepted rules, 16,384 suppressions, and 4,096 bytes per MIME or pattern
/// field. A candidate that would cross a remaining aggregate limit is rejected
/// whole before its strings are retained, while the bounded work spent
/// inspecting it still counts toward the aggregate.
pub fn load_globs2_from(candidates: &[PathBuf]) -> (ParsedGlobs, usize) {
    load_globs2_from_with_budget(candidates, PRODUCTION_MIME_BUDGET)
}

pub(super) fn load_globs2_from_with_budget(
    candidates: &[PathBuf],
    budget: MimeDatabaseBudget,
) -> (ParsedGlobs, usize) {
    load_globs2_from_with_budget_observer(candidates, budget, &mut NoLoadObserver)
}

fn load_globs2_from_with_budget_observer<O: LoadObserver>(
    candidates: &[PathBuf],
    budget: MimeDatabaseBudget,
    observer: &mut O,
) -> (ParsedGlobs, usize) {
    let mut merged: Vec<GlobRule> = Vec::new();
    let rule_hash_state = RandomState::new();
    let mut seen: HashMap<u64, Vec<usize>> = HashMap::new();
    let suppression_hash_state = RandomState::new();
    let mut suppressed: HashMap<u64, Vec<usize>> = HashMap::new();
    let mut all_suppressions: Vec<String> = Vec::new();
    let mut rejected = 0usize;
    let mut files_read = 0usize;
    let mut bytes_used = 0u64;
    let mut records_used = 0usize;
    let mut rules_used = 0usize;
    let mut suppressions_used = 0usize;

    for path in candidates.iter().take(budget.max_candidates) {
        let remaining_bytes = budget.max_aggregate_bytes.saturating_sub(bytes_used);
        if remaining_bytes == 0 {
            break;
        }
        let read_cap = budget.max_file_bytes.min(remaining_bytes);
        let Some(read) = read_globs2_file(path, read_cap) else {
            continue;
        };
        observer.candidate_attempted(read.bytes_attempted);
        bytes_used = bytes_used
            .saturating_add(read.bytes_attempted)
            .min(budget.max_aggregate_bytes);
        let Some(text) = read.text else {
            continue;
        };
        files_read += 1;

        let remaining = MimeDatabaseBudget {
            max_candidates: budget.max_candidates,
            max_aggregate_bytes: budget.max_aggregate_bytes.saturating_sub(bytes_used),
            max_file_bytes: read_cap,
            max_records: budget.max_records.saturating_sub(records_used),
            max_rules: budget.max_rules.saturating_sub(rules_used),
            max_suppressions: budget.max_suppressions.saturating_sub(suppressions_used),
            max_field_bytes: budget.max_field_bytes,
        };
        let structure = match inspect_globs2_structure(&text, remaining) {
            Ok(structure) => structure,
            Err(inspected) => {
                // Rejected candidates retain no strings, but the records and
                // valid entries already examined still consume aggregate work.
                records_used = records_used
                    .saturating_add(inspected.records)
                    .min(budget.max_records);
                rules_used = rules_used
                    .saturating_add(inspected.rules)
                    .min(budget.max_rules);
                suppressions_used = suppressions_used
                    .saturating_add(inspected.suppressions)
                    .min(budget.max_suppressions);
                continue;
            }
        };

        // Only now allocate retained MIME and pattern strings. A candidate that
        // exceeds any structural limit contributes no retained prefix.
        let parsed = parse_globs2(&text);
        records_used += structure.records;
        rules_used += structure.rules;
        suppressions_used += structure.suppressions;
        rejected = rejected.saturating_add(parsed.rejected);

        for rule in parsed.rules {
            if string_index_contains(
                &suppressed,
                &all_suppressions,
                &rule.mime,
                &suppression_hash_state,
            ) {
                continue;
            }
            let hash = rule_fingerprint(&rule, &rule_hash_state);
            let duplicate = seen.get(&hash).is_some_and(|indices| {
                indices
                    .iter()
                    .any(|&index| same_rule(&merged[index], &rule))
            });
            if !duplicate {
                let index = merged.len();
                merged.push(rule);
                seen.entry(hash).or_default().push(index);
            }
        }

        // Apply this file's sentinels after its own rules.
        for mime in parsed.suppressions {
            let hash = hash_value(&mime, &suppression_hash_state);
            let duplicate = suppressed.get(&hash).is_some_and(|indices| {
                indices.iter().any(|&index| all_suppressions[index] == mime)
            });
            if !duplicate {
                let index = all_suppressions.len();
                all_suppressions.push(mime);
                suppressed.entry(hash).or_default().push(index);
            }
        }
    }

    (
        ParsedGlobs {
            rules: merged,
            suppressions: all_suppressions,
            rejected,
        },
        files_read,
    )
}

fn same_rule(left: &GlobRule, right: &GlobRule) -> bool {
    left.mime == right.mime
        && left.pattern == right.pattern
        && left.case_sensitive == right.case_sensitive
}

fn rule_fingerprint(rule: &GlobRule, state: &RandomState) -> u64 {
    hash_value(&(&rule.mime, &rule.pattern, rule.case_sensitive), state)
}

fn hash_value<T: Hash + ?Sized>(value: &T, state: &RandomState) -> u64 {
    state.hash_one(value)
}

fn string_index_contains(
    index: &HashMap<u64, Vec<usize>>,
    strings: &[String],
    value: &str,
    state: &RandomState,
) -> bool {
    let hash = hash_value(value, state);
    index
        .get(&hash)
        .is_some_and(|indices| indices.iter().any(|&position| strings[position] == value))
}

trait LoadObserver {
    fn candidate_attempted(&mut self, _bytes: u64) {}
}

struct NoLoadObserver;
impl LoadObserver for NoLoadObserver {}

struct Globs2Read {
    text: Option<String>,
    bytes_attempted: u64,
}

impl Globs2Read {
    fn rejected(bytes_attempted: u64) -> Self {
        Self {
            text: None,
            bytes_attempted,
        }
    }
}

/// Read one candidate `globs2` file safely. `None` means no regular file was
/// opened. Once a regular candidate is attempted, the result reports its byte
/// work even when size, decoding or I/O rejects the candidate atomically.
///
/// # Safety against symlink TOCTOU
///
/// An `exists()` check followed by a read opens the path twice, and a user who
/// controls a path component can swap the target between the two, redirecting
/// the read. This performs a single [`std::fs::File::open`] and inspects the
/// already-open descriptor with [`std::fs::File::metadata`], which resolves the
/// descriptor rather than the path again, so there is no second resolution to
/// race. On Unix the open adds `O_NOFOLLOW`, refusing a symlink at the final
/// component outright, and `O_NONBLOCK`, so a writer-less FIFO opens without
/// waiting and is then refused by the descriptor check. Only a regular file
/// within the size cap is read.
fn read_globs2_file(path: &Path, max_bytes: u64) -> Option<Globs2Read> {
    // One open, then inspect the fd: there is no separate check to race.
    let file = open_no_follow(path)?;

    // This resolves the descriptor already held, not the path again.
    let meta = file.metadata().ok()?;
    if !meta.is_file() {
        // A directory, FIFO, device, etc. at this path is not a globs2 file.
        return None;
    }
    Some(read_globs2_bounded(file, meta.len(), max_bytes))
}

/// Read through one byte beyond `max_bytes`, rejecting rather than parsing a
/// prefix when the already-open file grows after its metadata was sampled.
///
/// Reading through only `max_bytes` cannot distinguish an exact-cap input from
/// one that grew after the metadata sample.
fn read_globs2_bounded(reader: impl Read, declared_len: u64, max_bytes: u64) -> Globs2Read {
    if declared_len > max_bytes {
        // Charge the full bounded attempt so a sequence of oversized files
        // cannot bypass the aggregate byte budget without issuing reads.
        return Globs2Read::rejected(max_bytes);
    }

    let Some(capacity) = usize::try_from(declared_len)
        .ok()
        .and_then(|len| len.checked_add(1))
    else {
        return Globs2Read::rejected(max_bytes);
    };
    let mut buf = Vec::with_capacity(capacity);
    let result = reader
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut buf);
    let observed = u64::try_from(buf.len()).unwrap_or_else(|_| max_bytes.saturating_add(1));
    if result.is_err() {
        // `read_to_end` may have filled part of the buffer before the error.
        // The declared bytes were nevertheless attempted, so neither part can
        // disappear from aggregate accounting.
        return Globs2Read::rejected(observed.max(declared_len));
    }
    if observed > max_bytes {
        return Globs2Read::rejected(observed);
    }

    // Non-UTF-8 content is not parseable, but all bytes were still read and
    // therefore consume the same aggregate allowance as valid input.
    Globs2Read {
        text: String::from_utf8(buf).ok(),
        bytes_attempted: observed,
    }
}

#[cfg(test)]
mod bounded_read_tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn a_file_that_grows_after_stat_is_rejected_not_parsed_at_the_cap() {
        let complete_prefix = b"50:text/x-truncated:*.truncated\n";
        let cap = complete_prefix.len() as u64;

        let exact = read_globs2_bounded(Cursor::new(complete_prefix), cap, cap);
        let exact_text = exact.text.expect("an exactly-at-cap database is valid");
        let parsed = parse_globs2(&exact_text);
        assert_eq!(parsed.rules.len(), 1, "positive control must parse a rule");
        assert_eq!(parsed.rules[0].mime, "text/x-truncated");
        assert_eq!(exact.bytes_attempted, cap);

        let mut grew = complete_prefix.to_vec();
        grew.push(b'#');
        let rejected = read_globs2_bounded(Cursor::new(grew), cap, cap);
        assert!(
            rejected.text.is_none(),
            "cap+1 bytes after an at-cap metadata snapshot must skip the whole \
             file, not accept the valid cap-byte prefix"
        );
        assert_eq!(
            rejected.bytes_attempted,
            cap + 1,
            "the diagnostic byte read must remain charged"
        );
    }
}

/// Open `path` for reading without following a terminal symlink, on platforms
/// that support it.
#[cfg(unix)]
fn open_no_follow(path: &Path) -> Option<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    // O_NOFOLLOW makes open() fail with ELOOP when the final component is a
    // symlink, which is the exploitable one: the earlier components are fixed
    // system directories. O_NONBLOCK keeps a read-only open of a writer-less
    // FIFO from waiting; the regular-file check then rejects it.
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(o_nofollow() | o_nonblock())
        .open(path)
        .ok()
}

/// `O_NOFOLLOW`, spelled out rather than pulled from a dependency.
///
/// This crate has no `libc` in its graph, and the value is fixed by each
/// platform's ABI, so a literal is stable here. On an unrecognised Unix it falls
/// back to `0`, leaving the post-open `is_file` check as the remaining defence.
#[cfg(unix)]
const fn o_nofollow() -> i32 {
    #[cfg(target_os = "linux")]
    {
        0o400_000 // 0x20000
    }
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "netbsd",
        target_os = "openbsd"
    ))]
    {
        0x0100
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "netbsd",
        target_os = "openbsd"
    )))]
    {
        0
    }
}

/// `O_NONBLOCK`, spelled out rather than pulled from a dependency. Linux uses
/// the value from its `asm-generic/fcntl.h`; the BSD family and Darwin use
/// `0x0004`.
#[cfg(unix)]
const fn o_nonblock() -> i32 {
    #[cfg(target_os = "linux")]
    {
        0o4000
    }
    #[cfg(not(target_os = "linux"))]
    {
        0x0004
    }
}

/// Non-Unix fallback: a single `File::open` (still one resolution, so the
/// fd-metadata check below is not raced), without `O_NOFOLLOW`.
#[cfg(not(unix))]
fn open_no_follow(path: &Path) -> Option<std::fs::File> {
    std::fs::File::open(path).ok()
}

/// The three XDG values `globs2` discovery needs, taken from the process
/// environment in one snapshot.
///
/// One snapshot rather than three reads, so no consumer can mix values from two
/// observations of a mutating environment. The fields are [`OsString`], so a
/// non-UTF-8 value is carried through instead of being downgraded to "unset".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Globs2Env {
    /// Raw `$XDG_DATA_HOME`; `None` when unset.
    pub data_home: Option<OsString>,
    /// Raw `$XDG_DATA_DIRS`; `None` when unset.
    pub data_dirs: Option<OsString>,
    /// Raw `$HOME`; `None` when unset.
    pub home: Option<OsString>,
}

impl Globs2Env {
    /// The variables read, in the order [`snapshot`](Self::snapshot) reads them.
    pub const KEYS: [&'static str; 3] = ["XDG_DATA_HOME", "XDG_DATA_DIRS", "HOME"];

    /// Snapshot the real process environment. Production calls this once at
    /// boot; tests should use [`snapshot`](Self::snapshot) with a fixture
    /// lookup instead of mutating the process-global environment (which is
    /// `unsafe` in edition 2024 and races every other test in the binary).
    #[must_use]
    pub fn from_env() -> Self {
        Self::snapshot(|key: &str| std::env::var_os(key))
    }

    /// Snapshot from an injected lookup with [`std::env::var_os`]'s signature.
    ///
    /// The lookup returns [`OsString`], so it is not *possible* for this layer
    /// to lose a non-UTF-8 value: there is no `String` to convert into.
    pub fn snapshot(mut var: impl FnMut(&str) -> Option<OsString>) -> Self {
        let [data_home, data_dirs, home] = Self::KEYS;
        Self {
            data_home: var(data_home),
            data_dirs: var(data_dirs),
            home: var(home),
        }
    }

    /// The ordered `globs2` candidate paths for this snapshot (layer 1).
    #[must_use]
    pub fn candidates(&self) -> Vec<PathBuf> {
        globs2_candidates_os(
            self.data_home.as_deref(),
            self.data_dirs.as_deref(),
            self.home.as_deref(),
        )
    }
}

/// Convenience entry point: read the real XDG environment, the only impure
/// step, compute the candidate paths and load them.
///
/// Never fails: a machine with no `globs2` files yields an empty
/// [`ParsedGlobs`] and a `files_read` of `0`. Returns the merged database and
/// the number of files read, so a health check can notice a suspiciously empty
/// result.
pub fn discover_globs2() -> (ParsedGlobs, usize) {
    load_globs2_from(&Globs2Env::from_env().candidates())
}

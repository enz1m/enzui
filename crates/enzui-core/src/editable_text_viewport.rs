//! Viewport-bounded materialization for [`crate::chunked_text`] documents.
//!
//! The cache retains visible lines and reusable overscan under independent line and byte limits. [`EditableTextViewportCache::invalidate_from`] preserves unaffected earlier entries after an edit; changing revision without it clears the cache.

use crate::chunked_text::TextSnapshot;
use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

/// Default visible-line overscan on each side.
pub const DEFAULT_OVERSCAN_LINES: usize = 16;
/// Default maximum number of retained materialized logical lines.
pub const DEFAULT_CACHE_LINES: usize = 96;
/// Default maximum bytes retained in materialized logical lines.
pub const DEFAULT_CACHE_BYTES: usize = 2 * 1024 * 1024;
/// Default per-line byte limit used for `MaterializeError::LineTooLarge`.
pub const DEFAULT_MAX_LINE_BYTES: usize = 1024 * 1024;

/// Independent limits for editable viewport materialization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditableTextViewportPolicy {
    /// Extra logical lines retained above and below the visible range.
    pub overscan_lines: usize,
    /// Maximum number of cached logical lines.
    pub max_cached_lines: usize,
    /// Maximum UTF-8 bytes retained by cached logical lines.
    pub max_cached_bytes: usize,
    /// Maximum bytes accepted for one logical line.
    pub max_line_bytes: usize,
}

impl Default for EditableTextViewportPolicy {
    fn default() -> Self {
        Self {
            overscan_lines: DEFAULT_OVERSCAN_LINES,
            max_cached_lines: DEFAULT_CACHE_LINES,
            max_cached_bytes: DEFAULT_CACHE_BYTES,
            max_line_bytes: DEFAULT_MAX_LINE_BYTES,
        }
    }
}

/// Why a requested editable window could not be represented truthfully.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaterializeError {
    /// Visible plus overscan lines exceed the configured line budget.
    LineBudget {
        /// Number of visible-plus-overscan lines requested from the cache.
        requested: usize,
        /// Configured maximum cached lines.
        budget: usize,
    },
    /// One complete logical line exceeds the paragraph byte limit.
    LineTooLarge {
        /// Zero-based offending line in the complete document.
        line: usize,
        /// Byte length of that logical line, excluding LF.
        bytes: usize,
        /// Configured per-line byte limit.
        limit: usize,
    },
    /// The complete requested window exceeds the byte budget.
    ByteBudget {
        /// Total bytes the requested window would materialize.
        requested: usize,
        /// Configured maximum cached bytes.
        budget: usize,
    },
}

/// One materialized logical line. The LF separator is not included.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaterializedLine {
    /// Zero-based line in the complete document.
    pub line: usize,
    /// UTF-8 byte range in the complete document, excluding LF.
    pub bytes: Range<usize>,
    /// Shared contiguous text used by the editor/renderer.
    pub text: Arc<str>,
}

/// One viewport result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditableTextFrame {
    /// Document line range represented by [`lines`](Self::lines).
    pub materialized: Range<usize>,
    /// Subrange of `lines` that is actually visible (overscan excluded).
    pub visible: Range<usize>,
    /// Materialized lines in document order.
    pub lines: Vec<MaterializedLine>,
    /// Bytes in `lines` (LF separators excluded).
    pub materialized_bytes: usize,
}

impl EditableTextFrame {
    /// Builds editor text for this frame with synthesized separators, without flattening text outside it.
    #[must_use]
    pub fn editor_text(&self) -> String {
        let separators = self.lines.len().saturating_sub(1);
        let mut out = String::with_capacity(self.materialized_bytes.saturating_add(separators));
        for (index, line) in self.lines.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            out.push_str(&line.text);
        }
        out
    }
}

/// Deterministic lifetime counters for the cache.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EditableTextViewportCounters {
    /// Lines in requests that passed the line-count check, including cache hits.
    pub requested_lines: u64,
    /// Lines copied out of the chunked store.
    pub materialized_lines: u64,
    /// UTF-8 bytes copied out of the chunked store.
    pub materialized_bytes: u64,
    /// Requested lines served from retained entries.
    pub cache_hits: u64,
    /// Entries removed by budget eviction, invalidation, or cache clearing.
    pub evicted_lines: u64,
    /// Entries invalidated because an edit could change their text/line number.
    pub invalidated_lines: u64,
    /// Current retained line count.
    pub cached_lines: usize,
    /// Current retained UTF-8 byte count.
    pub cached_bytes: usize,
    /// Largest line count retained at once.
    pub peak_cached_lines: usize,
    /// Largest UTF-8 byte count retained at once.
    pub peak_cached_bytes: usize,
}

#[derive(Clone, Debug)]
struct Entry {
    bytes: Range<usize>,
    text: Arc<str>,
    last_used: u64,
}

/// Byte- and line-budgeted LRU over a [`TextSnapshot`].
#[derive(Debug)]
pub struct EditableTextViewportCache {
    policy: EditableTextViewportPolicy,
    revision: Option<u64>,
    clock: u64,
    entries: BTreeMap<usize, Entry>,
    counters: EditableTextViewportCounters,
}

impl EditableTextViewportCache {
    /// Creates an empty cache. Zero limits are literal rather than unbounded; requests that exceed them return typed budget errors.
    #[must_use]
    pub fn new(policy: EditableTextViewportPolicy) -> Self {
        Self {
            policy,
            revision: None,
            clock: 0,
            entries: BTreeMap::new(),
            counters: EditableTextViewportCounters::default(),
        }
    }

    /// Active policy.
    #[must_use]
    pub fn policy(&self) -> EditableTextViewportPolicy {
        self.policy
    }

    /// Counters, including current and peak residency.
    #[must_use]
    pub fn counters(&self) -> EditableTextViewportCounters {
        self.counters
    }

    /// Remove every materialized line.
    pub fn clear(&mut self) {
        let removed = self.entries.len();
        self.entries.clear();
        self.counters.cached_lines = 0;
        self.counters.cached_bytes = 0;
        self.counters.evicted_lines = self.counters.evicted_lines.saturating_add(removed as u64);
        self.revision = None;
    }

    /// Invalidates the edited line and renumberable tail while retaining earlier entries.
    ///
    /// Passing `new_revision` to the next [`materialize`](Self::materialize) call preserves that retained prefix; another revision clears the cache.
    pub fn invalidate_from(&mut self, new_revision: u64, first_affected_line: usize) {
        let doomed: Vec<_> = self
            .entries
            .range(first_affected_line..)
            .map(|(line, _)| *line)
            .collect();
        let mut removed_bytes = 0usize;
        for line in &doomed {
            if let Some(entry) = self.entries.remove(line) {
                removed_bytes = removed_bytes.saturating_add(entry.text.len());
            }
        }
        self.counters.cached_lines = self.entries.len();
        self.counters.cached_bytes = self.counters.cached_bytes.saturating_sub(removed_bytes);
        self.counters.evicted_lines = self
            .counters
            .evicted_lines
            .saturating_add(doomed.len() as u64);
        self.counters.invalidated_lines = self
            .counters
            .invalidated_lines
            .saturating_add(doomed.len() as u64);
        self.revision = Some(new_revision);
    }

    /// Materializes the visible range plus bounded overscan.
    /// A revision change without prior invalidation clears the cache to prevent stale text.
    ///
    /// # Errors
    ///
    /// A [`MaterializeError`] when the range exceeds the line or byte budget, or one line is too
    /// large.
    #[allow(
        clippy::missing_panics_doc,
        reason = "panics only on a broken invariant: line boundaries come from the same snapshot"
    )]
    pub fn materialize(
        &mut self,
        document: &TextSnapshot,
        revision: u64,
        first_visible: usize,
        visible_lines: usize,
    ) -> Result<EditableTextFrame, MaterializeError> {
        if self.revision != Some(revision) {
            self.clear_for_revision(revision);
        }

        let line_count = document.line_count();
        let visible_start = first_visible.min(line_count.saturating_sub(1));
        let visible_end = visible_start.saturating_add(visible_lines).min(line_count);
        let start = visible_start.saturating_sub(self.policy.overscan_lines);
        let end = visible_end
            .saturating_add(self.policy.overscan_lines)
            .min(line_count);
        let requested = end.saturating_sub(start);
        if requested > self.policy.max_cached_lines {
            return Err(MaterializeError::LineBudget {
                requested,
                budget: self.policy.max_cached_lines,
            });
        }

        self.counters.requested_lines = self
            .counters
            .requested_lines
            .saturating_add(requested as u64);
        self.clock = self.clock.wrapping_add(1);
        let tick = self.clock;

        // Validate every requested range before inserting the new frame so a refusal cannot leave a partial frame resident.
        let mut requested_bytes = 0usize;
        let mut requested_ranges = Vec::with_capacity(requested);
        for line in start..end {
            let begin = document.line_start(line).unwrap_or(document.len());
            let finish = document.line_end(line).unwrap_or(begin);
            let bytes = finish.saturating_sub(begin);
            if bytes > self.policy.max_line_bytes {
                return Err(MaterializeError::LineTooLarge {
                    line,
                    bytes,
                    limit: self.policy.max_line_bytes,
                });
            }
            requested_bytes = requested_bytes.saturating_add(bytes);
            requested_ranges.push((line, begin..finish));
        }
        if requested_bytes > self.policy.max_cached_bytes {
            return Err(MaterializeError::ByteBudget {
                requested: requested_bytes,
                budget: self.policy.max_cached_bytes,
            });
        }

        for (line, range) in requested_ranges {
            if let Some(entry) = self.entries.get_mut(&line) {
                entry.last_used = tick;
                self.counters.cache_hits = self.counters.cache_hits.saturating_add(1);
                continue;
            }
            let bytes = range.len();
            let text: Arc<str> = document
                .slice_to_string(range.clone())
                .expect("line boundaries come from this snapshot")
                .into();
            self.counters.materialized_lines = self.counters.materialized_lines.saturating_add(1);
            self.counters.materialized_bytes = self
                .counters
                .materialized_bytes
                .saturating_add(bytes as u64);
            self.counters.cached_bytes = self.counters.cached_bytes.saturating_add(bytes);
            self.entries.insert(
                line,
                Entry {
                    bytes: range,
                    text,
                    last_used: tick,
                },
            );
        }

        // Evict least-recently-used entries outside this frame; requested lines are not eviction candidates.
        while self.entries.len() > self.policy.max_cached_lines
            || self.counters.cached_bytes > self.policy.max_cached_bytes
        {
            let victim = self
                .entries
                .iter()
                .filter(|(line, _)| **line < start || **line >= end)
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(line, _)| *line);
            let Some(victim) = victim else {
                return Err(MaterializeError::ByteBudget {
                    requested: requested_bytes,
                    budget: self.policy.max_cached_bytes,
                });
            };
            self.remove_evicted(victim);
        }

        let mut lines = Vec::with_capacity(requested);
        for line in start..end {
            let entry = self.entries.get(&line).expect("requested line retained");
            lines.push(MaterializedLine {
                line,
                bytes: entry.bytes.clone(),
                text: Arc::clone(&entry.text),
            });
        }
        self.refresh_residency_peaks();

        Ok(EditableTextFrame {
            materialized: start..end,
            visible: visible_start.saturating_sub(start)..visible_end.saturating_sub(start),
            lines,
            materialized_bytes: requested_bytes,
        })
    }

    fn clear_for_revision(&mut self, revision: u64) {
        let removed = self.entries.len();
        self.entries.clear();
        self.counters.cached_lines = 0;
        self.counters.cached_bytes = 0;
        self.counters.evicted_lines = self.counters.evicted_lines.saturating_add(removed as u64);
        self.counters.invalidated_lines = self
            .counters
            .invalidated_lines
            .saturating_add(removed as u64);
        self.revision = Some(revision);
    }

    fn remove_evicted(&mut self, line: usize) {
        if let Some(entry) = self.entries.remove(&line) {
            self.counters.cached_bytes =
                self.counters.cached_bytes.saturating_sub(entry.text.len());
            self.counters.evicted_lines = self.counters.evicted_lines.saturating_add(1);
        }
        self.counters.cached_lines = self.entries.len();
    }

    fn refresh_residency_peaks(&mut self) {
        self.counters.cached_lines = self.entries.len();
        self.counters.peak_cached_lines = self.counters.peak_cached_lines.max(self.entries.len());
        self.counters.peak_cached_bytes = self
            .counters
            .peak_cached_bytes
            .max(self.counters.cached_bytes);
    }
}

impl Default for EditableTextViewportCache {
    fn default() -> Self {
        Self::new(EditableTextViewportPolicy::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunked_text::TextStore;

    #[test]
    fn an_edit_invalidates_affected_lines_but_reuses_earlier_summaries() {
        let mut store = TextStore::from_string("zero\none\ntwo\nthree\n".to_owned());
        let mut cache = EditableTextViewportCache::new(EditableTextViewportPolicy {
            overscan_lines: 0,
            max_cached_lines: 8,
            max_cached_bytes: 1024,
            max_line_bytes: 128,
        });
        let first = cache.materialize(store.current(), 10, 0, 4).unwrap();
        assert_eq!(first.lines[2].text.as_ref(), "two");
        let before = cache.counters();

        let start = store.current().line_start(2).unwrap();
        let end = store.current().line_end(2).unwrap();
        store.replace(start..end, "changed".to_owned()).unwrap();
        cache.invalidate_from(11, 2);
        let second = cache.materialize(store.current(), 11, 0, 4).unwrap();

        assert_eq!(second.lines[0].text.as_ref(), "zero");
        assert_eq!(second.lines[2].text.as_ref(), "changed");
        let after = cache.counters();
        assert_eq!(
            after.cache_hits - before.cache_hits,
            2,
            "unaffected prefix reused"
        );
        assert_eq!(after.materialized_lines - before.materialized_lines, 2);
        assert_eq!(after.invalidated_lines - before.invalidated_lines, 2);
    }
}

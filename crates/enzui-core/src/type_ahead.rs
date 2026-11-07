//! Type-to-jump over a list's visible rows (the Nautilus "type-ahead").
//!
//! A [`TypeAhead`] is a timed prefix buffer. Each typed character extends the
//! prefix and moves to the first visible row, searching from the focused row
//! and wrapping, whose label starts with it. Typing one character repeatedly
//! (`"s"`, `"ss"`, `"sss"`) instead cycles through the rows that start with
//! that character, which is how a user steps between `src`, `scripts` and
//! `static`. A pause longer than the timeout starts a fresh prefix.
//!
//! Matching folds case, compatibility forms and accents: typing `e` finds
//! `École`, `ﬁ` matches `fi`. That is the useful subset of locale-aware
//! matching without a collation database; ordering stays the caller's (the
//! rows are already sorted the way the view shows them).
//!
//! This is pure policy: no widget, no clock of its own. The caller passes the
//! instant, the visible row labels in display order and the focused row, and
//! decides what to do with the result: select and reveal the row, and show
//! [`TypeAhead::buffer`] as feedback, including when nothing matched.

use std::time::{Duration, Instant};

use unicode_normalization::UnicodeNormalization;

/// How long a pause may last before the next character starts a new prefix.
/// GTK and Nautilus use the same order of magnitude.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(1000);

/// The longest prefix kept; further characters are ignored.
pub const MAX_BUFFER_CHARS: usize = 64;

/// What one keystroke resolved to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeAheadResult {
    /// Move to this row (index into the labels passed in).
    Jump(usize),
    /// No visible row starts with the current prefix; keep the selection and
    /// show the prefix as unmatched.
    NoMatch,
    /// The character was not accepted (a control character, or the buffer is
    /// full); nothing changed.
    Ignored,
}

/// A timed type-ahead buffer.
#[derive(Clone, Debug)]
pub struct TypeAhead {
    buffer: String,
    last: Option<Instant>,
    timeout: Duration,
    matched: bool,
}

impl Default for TypeAhead {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeAhead {
    /// A buffer with [`DEFAULT_TIMEOUT`].
    #[must_use]
    pub fn new() -> Self {
        Self::with_timeout(DEFAULT_TIMEOUT)
    }

    /// A buffer that restarts after `timeout` without input.
    #[must_use]
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            buffer: String::new(),
            last: None,
            timeout,
            matched: false,
        }
    }

    /// The prefix typed so far: empty when idle, and still holding a lapsed
    /// prefix until the next character or [`TypeAhead::clear`].
    #[must_use]
    pub fn buffer(&self) -> &str {
        &self.buffer
    }

    /// Whether the last accepted character landed on a row, for feedback that
    /// tells "jumped" from "nothing starts with this".
    #[must_use]
    pub fn matched(&self) -> bool {
        !self.buffer.is_empty() && self.matched
    }

    /// Whether a prefix is live at `now`, i.e. the next character extends it.
    #[must_use]
    pub fn is_active(&self, now: Instant) -> bool {
        !self.buffer.is_empty()
            && self
                .last
                .is_some_and(|last| now.saturating_duration_since(last) <= self.timeout)
    }

    /// When the live prefix lapses, for scheduling the feedback's removal;
    /// `None` without a prefix, or when the timeout reaches past what an
    /// [`Instant`] can hold (it never lapses).
    #[must_use]
    pub fn expires_at(&self) -> Option<Instant> {
        use crate::monotonic_ms::{Policy, checked_instant_deadline};
        (!self.buffer.is_empty())
            .then_some(self.last?)
            .and_then(|last| checked_instant_deadline(last, self.timeout, Policy::Clamp))
    }

    /// Forget the prefix (Escape, a click, a navigation elsewhere).
    pub fn clear(&mut self) {
        self.buffer.clear();
        self.last = None;
        self.matched = false;
    }

    /// Feed one typed character.
    ///
    /// `labels` are the visible rows in display order; `current` is the
    /// focused row, where the search starts so that extending a prefix keeps
    /// a row that still matches.
    pub fn push<S: AsRef<str>>(
        &mut self,
        now: Instant,
        ch: char,
        labels: &[S],
        current: Option<usize>,
    ) -> TypeAheadResult {
        if ch.is_control() {
            return TypeAheadResult::Ignored;
        }
        if !self.is_active(now) {
            self.buffer.clear();
        }
        if self.buffer.chars().count() >= MAX_BUFFER_CHARS {
            self.last = Some(now);
            return TypeAheadResult::Ignored;
        }
        self.buffer.push(ch);
        self.last = Some(now);
        self.matched = false;
        if labels.is_empty() {
            return TypeAheadResult::NoMatch;
        }

        let prefix = fold(&self.buffer);
        let current = current.filter(|&index| index < labels.len());
        // One character repeated steps to the next row starting with it.
        let mut chars = prefix.chars();
        let first = chars.next();
        let repeated = self.buffer.chars().count() > 1 && chars.all(|c| Some(c) == first);
        let (needle, start) = if repeated {
            let single: String = first.into_iter().collect();
            (single, current.map_or(0, |index| index + 1))
        } else {
            (prefix, current.unwrap_or(0))
        };
        let found = (0..labels.len())
            .map(|offset| (start + offset) % labels.len())
            .find(|&index| fold(labels[index].as_ref()).starts_with(&needle));
        self.matched = found.is_some();
        match found {
            Some(index) => TypeAheadResult::Jump(index),
            None => TypeAheadResult::NoMatch,
        }
    }
}

/// Comparison text: [`crate::palette::normalize`], then compatibility-decomposed
/// with accents removed, so a user need not type them to jump.
///
/// For matching only; never an identity or a path.
#[must_use]
pub fn fold(text: &str) -> String {
    crate::palette::normalize(text)
        .nfkd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .collect()
}

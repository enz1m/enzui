//! Keyed page selection, deterministic fallback, boundary policies, and swipe-to-page resolution.

use crate::metrics::FinitePositive;
use crate::semantics::{Role, Semantics};

/// Default horizontal swipe distance threshold, in logical pixels.
pub const DEFAULT_SWIPE_THRESHOLD: f32 = 48.0;

/// Stable page identity that survives reordering and insertion.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PageId(pub String);

impl PageId {
    /// Build a page id from anything string-like.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The underlying key.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for PageId {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<String> for PageId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl std::fmt::Display for PageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// How a paging request should behave at the first/last page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Wrap {
    /// Stops at either end; requests beyond a boundary produce no selection change.
    #[default]
    Clamp,
    /// Past the last page comes the first, and vice versa.
    Wrap,
}

impl Wrap {
    /// Whether this policy wraps around the ends.
    #[must_use]
    pub fn wraps(self) -> bool {
        matches!(self, Wrap::Wrap)
    }
}

/// Describes a page request independently of keyboard, pointer, chrome, or swipe input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageRequest {
    /// Step one page toward the end.
    Next,
    /// Step one page toward the start.
    Previous,
    /// Jump to the first page.
    First,
    /// Jump to the last page.
    Last,
    /// Select a specific page by identity.
    Id(PageId),
}

impl PageRequest {
    /// Maps signed horizontal travel to a page request without applying a threshold.
    ///
    /// Negative travel advances; zero or non-finite travel returns `None`.
    #[must_use]
    pub fn for_swipe(distance: f32) -> Option<Self> {
        if !distance.is_finite() || distance == 0.0 {
            return None;
        }
        if distance < 0.0 {
            Some(PageRequest::Next)
        } else {
            Some(PageRequest::Previous)
        }
    }
}

/// Describes whether selection was honored or which fallback was applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// The requested id was present.
    Exact,
    /// No selection was requested; the first page was used.
    DefaultedToFirst,
    /// The requested id was absent, so the first page was selected.
    StaleDefaultedToFirst,
    /// There are no pages at all.
    Empty,
}

impl Resolution {
    /// Whether the caller's requested selection was honoured exactly.
    #[must_use]
    pub fn is_exact(self) -> bool {
        matches!(self, Resolution::Exact)
    }

    /// Whether the caller's stored selection should be rewritten, because what
    /// is displayed is not what was asked for.
    #[must_use]
    pub fn needs_repair(self) -> bool {
        matches!(
            self,
            Resolution::DefaultedToFirst | Resolution::StaleDefaultedToFirst
        )
    }
}

/// The outcome of resolving a selection against the current page list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageCursor {
    active: Option<usize>,
    resolution: Resolution,
    len: usize,
}

impl PageCursor {
    /// Resolves `selected` against the current id order. Duplicate ids use their first occurrence.
    #[must_use]
    pub fn resolve(ids: &[PageId], selected: Option<&PageId>) -> Self {
        let len = ids.len();
        if len == 0 {
            return Self {
                active: None,
                resolution: Resolution::Empty,
                len,
            };
        }
        match selected {
            None => Self {
                active: Some(0),
                resolution: Resolution::DefaultedToFirst,
                len,
            },
            Some(id) => match ids.iter().position(|candidate| candidate == id) {
                Some(index) => Self {
                    active: Some(index),
                    resolution: Resolution::Exact,
                    len,
                },
                None => Self {
                    active: Some(0),
                    resolution: Resolution::StaleDefaultedToFirst,
                    len,
                },
            },
        }
    }

    /// Index of the page to display, if any.
    #[must_use]
    pub fn active(self) -> Option<usize> {
        self.active
    }

    /// How the selection was resolved.
    #[must_use]
    pub fn resolution(self) -> Resolution {
        self.resolution
    }

    /// Number of pages considered.
    #[must_use]
    pub fn len(self) -> usize {
        self.len
    }

    /// Whether there are no pages.
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.len == 0
    }

    /// Returns whether the active page is first; empty lists return `true`.
    #[must_use]
    pub fn at_start(self) -> bool {
        self.active.is_none_or(|i| i == 0)
    }

    /// Returns whether the active page is last; empty lists return `true`.
    #[must_use]
    pub fn at_end(self) -> bool {
        match self.active {
            None => true,
            Some(i) => i + 1 >= self.len,
        }
    }

    /// Whether a [`PageRequest::Previous`] can currently do anything.
    #[must_use]
    pub fn can_go_previous(self, wrap: Wrap) -> bool {
        self.len > 1 && (wrap.wraps() || !self.at_start())
    }

    /// Whether a [`PageRequest::Next`] can currently do anything.
    #[must_use]
    pub fn can_go_next(self, wrap: Wrap) -> bool {
        self.len > 1 && (wrap.wraps() || !self.at_end())
    }
}

/// Applies a request and returns the selected id only when the selection changes.
///
/// A resolved stale id still produces the fallback id so callers can repair stored selection.
#[must_use]
pub fn apply(
    ids: &[PageId],
    selected: Option<&PageId>,
    request: &PageRequest,
    wrap: Wrap,
) -> Option<PageId> {
    let cursor = PageCursor::resolve(ids, selected);
    let current = cursor.active()?;
    let len = ids.len();

    let target = match request {
        PageRequest::First => 0,
        PageRequest::Last => len - 1,
        PageRequest::Id(id) => ids.iter().position(|candidate| candidate == id)?,
        PageRequest::Next => match current + 1 {
            t if t < len => t,
            _ if wrap.wraps() => 0,
            _ => return None,
        },
        PageRequest::Previous => match current.checked_sub(1) {
            Some(t) => t,
            None if wrap.wraps() => len - 1,
            None => return None,
        },
    };

    // Publish a fallback id even at the same index so the caller can repair stale selection.
    if target == current && cursor.resolution().is_exact() {
        return None;
    }
    Some(ids[target].clone())
}

/// Direction a page transition should animate in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageDirection {
    /// Moving toward the end of the list.
    Forward,
    /// Moving toward the start of the list.
    Backward,
    /// No movement or no meaningful direction because an endpoint is absent.
    None,
}

/// Returns transition direction between selections. Wrapping steps follow request direction rather than index-delta sign.
#[must_use]
pub fn direction(
    ids: &[PageId],
    from: Option<&PageId>,
    to: Option<&PageId>,
    wrap: Wrap,
) -> PageDirection {
    let (Some(from), Some(to)) = (from, to) else {
        return PageDirection::None;
    };
    let (Some(a), Some(b)) = (
        ids.iter().position(|c| c == from),
        ids.iter().position(|c| c == to),
    ) else {
        return PageDirection::None;
    };
    if a == b {
        return PageDirection::None;
    }
    if wrap.wraps() && ids.len() > 1 {
        // A wrapping single step keeps the request direction even though its index jump has the opposite sign.
        if a == ids.len() - 1 && b == 0 {
            return PageDirection::Forward;
        }
        if a == 0 && b == ids.len() - 1 {
            return PageDirection::Backward;
        }
    }
    if b > a {
        PageDirection::Forward
    } else {
        PageDirection::Backward
    }
}

/// Applies a distance threshold and maps a committed horizontal swipe to a page request.
///
/// Negative distance advances; travel shorter than `threshold` returns `None`.
#[must_use]
pub fn commit_swipe(distance: f32, threshold: FinitePositive) -> Option<PageRequest> {
    if !distance.is_finite() || distance.abs() < threshold.get() {
        return None;
    }
    PageRequest::for_swipe(distance)
}

/// Returns duplicate ids in first-duplicate order; page resolution still uses the first occurrence.
#[must_use]
pub fn duplicate_ids(ids: &[PageId]) -> Vec<PageId> {
    let mut seen = std::collections::HashSet::with_capacity(ids.len());
    let mut reported = std::collections::HashSet::new();
    let mut out = Vec::new();
    for id in ids {
        if !seen.insert(id) && reported.insert(id) {
            out.push(id.clone());
        }
    }
    out
}

/// Accessibility semantics for a page indicator, including selected state and one-based position within the set.
pub fn indicator_semantics(index: usize, count: usize, selected: bool) -> Semantics {
    Semantics::new(Role::Tab)
        .label(
            crate::localized!("enzui-paging-page-of", "page" => index + 1, "total" => count.max(1)),
        )
        .selected(selected)
}

/// Semantics for the carousel container itself.
pub fn carousel_semantics(label: Option<&str>, cursor: PageCursor) -> Semantics {
    let semantics = Semantics::new(Role::Group).label_opt(label);
    match cursor.active() {
        Some(active) => semantics.description(crate::localized!("enzui-paging-page-of", "page" => active + 1, "total" => cursor.len())),
        None => semantics.description(crate::localization::text("enzui-paging-no-pages")),
    }
}

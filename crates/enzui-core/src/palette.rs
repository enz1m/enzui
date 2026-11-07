//! Deterministic command-palette matching and ordering over [`Action`] snapshots.
//!
//! Queries use Unicode lowercase plus NFC, require every whitespace-separated token, and rank prefix before word before substring while preserving snapshot order for ties.

use crate::action::Action;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Match rank from best to worst; derived ordering follows variant order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MatchQuality {
    /// The normalized label starts with the normalized query.
    Prefix,
    /// A later word in the normalized label starts with the normalized query.
    Word,
    /// Lowest-ranked match, including token-only matches without a contiguous whole-query match.
    Substring,
}

/// A candidate that matched, with the information needed to order it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// The stable id of the matched action.
    pub id: &'static str,
    /// How well it matched.
    pub quality: MatchQuality,
    /// Original snapshot index, used for tie order and action lookup.
    pub index: usize,
}

/// The matching form of `text`: [`crate::text_fold::fold`], which drops
/// case, accents and compatibility forms, so `cafe` finds `Café`.
#[must_use]
pub fn normalize(text: &str) -> String {
    crate::text_fold::fold(text)
}

/// Normalizes non-empty Unicode-whitespace-separated query tokens.
#[must_use]
pub fn tokens(query: &str) -> Vec<String> {
    query.split_whitespace().map(normalize).collect()
}

fn starts_word(haystack: &str, token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    let mut previous: Option<char> = None;
    for (offset, ch) in haystack.char_indices() {
        let boundary = previous.is_none_or(|p| !p.is_alphanumeric());
        if boundary
            && haystack
                .get(offset..)
                .is_some_and(|rest| rest.starts_with(token))
        {
            return true;
        }
        previous = Some(ch);
    }
    false
}

/// Ranks a normalized whole query against normalized text; token-only matches fall back to [`MatchQuality::Substring`].
#[must_use]
pub fn quality(haystack: &str, query: &str) -> Option<MatchQuality> {
    if query.is_empty() {
        return Some(MatchQuality::Prefix);
    }
    if haystack.starts_with(query) {
        return Some(MatchQuality::Prefix);
    }
    if starts_word(haystack, query) {
        return Some(MatchQuality::Word);
    }
    if haystack.contains(query) {
        return Some(MatchQuality::Substring);
    }
    None
}

/// Whether a candidate's text matches every token of a query.
#[must_use]
pub fn matches_tokens(haystack: &str, tokens: &[String]) -> bool {
    tokens.iter().all(|token| haystack.contains(token.as_str()))
}

/// Filters and ranks actions for `query`, retaining disabled actions.
///
/// Empty or whitespace-only queries return snapshot order.
#[must_use]
pub fn search<Message>(actions: &[Action<Message>], query: &str) -> Vec<Match> {
    let tokens = tokens(query);
    let whole = normalize(query.trim());

    // Normalize each full label because Unicode string lowercasing is context-sensitive and may differ from character-wise folding.
    let mut matched: Vec<Match> = actions
        .iter()
        .enumerate()
        .filter_map(|(index, action)| {
            let haystack = normalize(action.label());
            if !matches_tokens(&haystack, &tokens) {
                return None;
            }
            Some(Match {
                id: action.id(),
                quality: quality(&haystack, &whole).unwrap_or(MatchQuality::Substring),
                index,
            })
        })
        .collect();

    // Stable sorting preserves snapshot order among equal-quality matches.
    matched.sort_by_key(|a| a.quality);
    matched
}

/// Stores action ids and normalized labels for repeated searches without borrowing the actions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Catalogue {
    entries: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    id: &'static str,
    haystack: String,
}

impl Catalogue {
    /// Prepares actions for repeated searching.
    ///
    /// # Panics
    /// Panics when action ids are duplicated.
    #[must_use]
    pub fn new<Message>(actions: &[Action<Message>]) -> Self {
        match Self::try_new(actions) {
            Ok(catalogue) => catalogue,
            #[allow(
                clippy::panic,
                reason = "the panicking constructor; `try_new` reports instead"
            )]
            Err(duplicates) => panic!(
                "command palette catalogue has duplicate action id(s) {duplicates:?}: two \
                 actions claiming one id cannot be told apart by a dispatching host, so the \
                 later one's message is permanently unreachable. Give each action a \
                 unique id, or use `Catalogue::try_new` to handle this without panicking."
            ),
        }
    }

    /// Prepares actions for repeated searching, rejecting duplicate ids.
    ///
    /// Each duplicated id is reported once in first-duplicate order; labels may repeat.
    ///
    /// # Errors
    /// Returns all duplicated ids.
    pub fn try_new<Message>(actions: &[Action<Message>]) -> Result<Self, Vec<&'static str>> {
        let mut seen = std::collections::HashSet::with_capacity(actions.len());
        let mut duplicates: Vec<&'static str> = Vec::new();
        for action in actions {
            if !seen.insert(action.id()) && !duplicates.contains(&action.id()) {
                duplicates.push(action.id());
            }
        }
        if !duplicates.is_empty() {
            return Err(duplicates);
        }

        Ok(Self {
            entries: actions
                .iter()
                .map(|action| Entry {
                    id: action.id(),
                    haystack: normalize(action.label()),
                })
                .collect(),
        })
    }

    /// How many commands are prepared.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the catalogue is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Searches prepared entries into `into`, preserving its allocation and matching [`search`].
    pub fn search_into(&self, query: &str, into: &mut Vec<Match>) {
        into.clear();
        let tokens = tokens(query);
        let whole = normalize(query.trim());

        into.extend(
            self.entries
                .iter()
                .enumerate()
                .filter_map(|(index, entry)| {
                    if !matches_tokens(&entry.haystack, &tokens) {
                        return None;
                    }
                    Some(Match {
                        id: entry.id,
                        quality: quality(&entry.haystack, &whole)
                            .unwrap_or(MatchQuality::Substring),
                        index,
                    })
                }),
        );

        into.sort_by_key(|a| a.quality);
    }

    /// Search the prepared catalogue into a fresh `Vec`.
    #[must_use]
    pub fn search(&self, query: &str) -> Vec<Match> {
        let mut out = Vec::new();
        self.search_into(query, &mut out);
        out
    }
}

/// Describes the bounded slice of search results a list should realize.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    /// Index of the first realized result.
    pub start: usize,
    /// Number of results to realize. May be less than the capacity at the end
    /// of the list.
    pub len: usize,
}

impl Window {
    /// One past the last realized index.
    #[must_use]
    pub fn end(&self) -> usize {
        self.start + self.len
    }

    /// Whether `index` falls inside the window.
    #[must_use]
    pub fn contains(&self, index: usize) -> bool {
        index >= self.start && index < self.end()
    }
}

/// Places at most `capacity` rows so `focus` is visible, scrolling only as needed.
///
/// Zero capacity yields an empty window.
#[must_use]
pub fn window(total: usize, capacity: usize, focus: Option<usize>) -> Window {
    if total == 0 || capacity == 0 {
        return Window { start: 0, len: 0 };
    }
    let len = capacity.min(total);
    let Some(focus) = focus else {
        return Window { start: 0, len };
    };
    let focus = focus.min(total - 1);
    let start = if focus < len { 0 } else { (focus + 1) - len };
    Window { start, len }
}

/// Places a bounded window using a remembered start.
///
/// The start is clamped after list shrinkage and changes only when focus leaves the window; no focus preserves the clamped start. Zero capacity yields an empty window.
#[must_use]
pub fn window_from(
    total: usize,
    capacity: usize,
    focus: Option<usize>,
    previous_start: usize,
) -> Window {
    if total == 0 || capacity == 0 {
        return Window { start: 0, len: 0 };
    }
    let len = capacity.min(total);
    let max_start = total - len;
    let start = previous_start.min(max_start);

    let Some(focus) = focus else {
        return Window { start, len };
    };
    let focus = focus.min(total - 1);

    let start = if focus < start {
        focus
    } else if focus >= start + len {
        (focus + 1) - len
    } else {
        start
    };
    Window { start, len }
}

/// Stores a palette query, stable-id highlight, generation, and presentation caches.
#[derive(Debug, Clone, Default)]
pub struct PaletteState {
    query: String,
    // Store the highlight by stable action id so refiltering cannot retarget it by position.
    highlighted: Option<&'static str>,
    // Advance with wrapping on every effective query change so activations from an older rendered query can be rejected.
    generation: u64,
    // Retained display position for `window_from`; excluded from model equality.
    window_start: Cell<usize>,
    // Reusable view-search buffer, lent to `realize` callbacks as a shared snapshot; its contents and capacity are excluded from model equality.
    scratch: RefCell<Rc<Vec<Match>>>,
}

impl PartialEq for PaletteState {
    fn eq(&self, other: &Self) -> bool {
        self.query == other.query
            && self.highlighted == other.highlighted
            && self.generation == other.generation
    }
}

impl Eq for PaletteState {}

impl PaletteState {
    /// A fresh palette: empty query, nothing highlighted.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The current query.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Returns the generation advanced with wrapping by each effective query change for activation revalidation.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Returns the stored highlighted action id, if any.
    #[must_use]
    pub fn highlighted(&self) -> Option<&'static str> {
        self.highlighted
    }

    /// Returns the remembered realized-window start; [`window_from`] safely clamps stale values.
    #[must_use]
    pub fn window_start(&self) -> usize {
        self.window_start.get()
    }

    /// Searches into reusable storage, computes and records the remembered window, and invokes `f` with both.
    ///
    /// `f` receives a snapshot of the results taken before it runs, so it may
    /// re-enter this state, including a recursive `realize`, without panicking.
    /// A search performed during `f` leaves that snapshot unchanged and takes
    /// effect for the next caller. Successive calls reuse one buffer and
    /// allocate nothing; a recursive call allocates one buffer for the inner
    /// search and leaves the innermost remembered window start in effect.
    pub fn realize<R>(
        &self,
        catalogue: &Catalogue,
        capacity: usize,
        f: impl FnOnce(&[Match], Window) -> R,
    ) -> R {
        let (results, window) = {
            let mut scratch = self.scratch.borrow_mut();
            {
                // Copy-on-write splits the buffer only while an outer call's snapshot is alive.
                let buffer = Rc::make_mut(&mut *scratch);
                catalogue.search_into(&self.query, buffer);
            }
            let focus = self
                .highlighted
                .and_then(|id| scratch.iter().position(|m| m.id == id));
            let window = window_from(scratch.len(), capacity, focus, self.window_start.get());
            self.window_start.set(window.start);
            (Rc::clone(&*scratch), window)
        };
        // `f` may re-enter this state, so no interior borrow is live across it.
        f(&results, window)
    }

    /// Sets the query and reconciles the highlight against results for that new query.
    ///
    /// Returns `false` when unchanged; `results` must correspond to `query`.
    pub fn set_query(&mut self, query: &str, results: &[Match]) -> bool {
        if self.query == query {
            return false;
        }
        self.query.clear();
        self.query.push_str(query);
        // Advance with wrapping before reconciliation so events from the outgoing query are already stale.
        self.generation = self.generation.wrapping_add(1);
        self.reconcile(results);
        true
    }

    /// Keeps a still-present highlighted id, otherwise highlights the first result.
    pub fn reconcile(&mut self, results: &[Match]) {
        let still_present = self
            .highlighted
            .is_some_and(|id| results.iter().any(|m| m.id == id));
        if !still_present {
            self.highlighted = results.first().map(|m| m.id);
        }
    }

    /// Highlights `id` only when it is present in `results`.
    pub fn highlight(&mut self, id: &'static str, results: &[Match]) -> bool {
        if results.iter().any(|m| m.id == id) {
            self.highlighted = Some(id);
            true
        } else {
            false
        }
    }

    /// Moves the highlight by `delta`, saturating rather than wrapping at list ends.
    pub fn move_highlight(&mut self, delta: isize, results: &[Match]) {
        if results.is_empty() {
            self.highlighted = None;
            return;
        }
        let current = self
            .highlighted
            .and_then(|id| results.iter().position(|m| m.id == id));
        let next = match current {
            Some(index) => index.saturating_add_signed(delta).min(results.len() - 1),
            None if delta >= 0 => 0,
            None => results.len() - 1,
        };
        self.highlighted = Some(results[next].id);
    }

    /// Highlight the first result.
    pub fn highlight_first(&mut self, results: &[Match]) {
        self.highlighted = results.first().map(|m| m.id);
    }

    /// Highlight the last result.
    pub fn highlight_last(&mut self, results: &[Match]) {
        self.highlighted = results.last().map(|m| m.id);
    }

    /// Returns the currently highlighted id only if it remains in `results`.
    #[must_use]
    pub fn activation(&self, results: &[Match]) -> Option<&'static str> {
        let id = self.highlighted?;
        results.iter().any(|m| m.id == id).then_some(id)
    }

    /// Returns whether an activation matches both the current query generation and current highlighted result.
    #[must_use]
    pub fn accepts_activation(&self, id: &'static str, generation: u64, results: &[Match]) -> bool {
        self.generation == generation && self.activation(results) == Some(id)
    }
}

/// Registration scope ordered from least to most specific; a more specific duplicate id wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ActionScope {
    /// Available in every window of the application.
    Application,
    /// Available throughout one window.
    Window,
    /// Available only while a particular surface/context is active.
    Context,
}

/// Registers an action with scope and current scope activity.
///
/// Inactive registrations are absent from snapshots; disabled active actions remain present.
#[derive(Debug, Clone)]
pub struct ScopedAction<Message> {
    action: Action<Message>,
    scope: ActionScope,
    active: bool,
}

impl<Message> ScopedAction<Message> {
    /// Register `action` in `scope`, active by default.
    pub fn new(scope: ActionScope, action: Action<Message>) -> Self {
        Self {
            action,
            scope,
            active: true,
        }
    }

    /// Sets whether this registration’s scope is active.
    #[must_use]
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// The registered action.
    pub const fn action(&self) -> &Action<Message> {
        &self.action
    }

    /// The scope this registration came from.
    pub const fn scope(&self) -> ActionScope {
        self.scope
    }

    /// Whether the scope is currently active.
    pub const fn is_active(&self) -> bool {
        self.active
    }
}

/// How a duplicate id was resolved when building an [`ActionSnapshot`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowedAction {
    /// The contested id.
    pub id: &'static str,
    /// The scope whose registration won.
    pub winner: ActionScope,
    /// The scope whose registration was shadowed.
    pub shadowed: ActionScope,
}

/// Contains the effective in-scope actions for one window.
///
/// Inactive registrations are skipped. For duplicate ids, a strictly more specific scope wins; within one scope, the first registration wins. Every collision is recorded.
#[derive(Debug, Clone)]
pub struct ActionSnapshot<Message> {
    effective: Vec<Action<Message>>,
    shadowed: Vec<ShadowedAction>,
}

impl<Message> Default for ActionSnapshot<Message> {
    fn default() -> Self {
        Self {
            effective: Vec::new(),
            shadowed: Vec::new(),
        }
    }
}

impl<Message: Clone> ActionSnapshot<Message> {
    /// Resolves registrations while preserving each id’s first active registration position.
    #[must_use]
    pub fn resolve(registrations: &[ScopedAction<Message>]) -> Self {
        let mut effective: Vec<Action<Message>> = Vec::new();
        let mut scopes: Vec<ActionScope> = Vec::new();
        let mut shadowed = Vec::new();
        let mut indices: std::collections::HashMap<&'static str, usize> =
            std::collections::HashMap::with_capacity(registrations.len());

        for registration in registrations {
            if !registration.active {
                continue;
            }
            let id = registration.action.id();
            match indices.get(id).copied() {
                None => {
                    indices.insert(id, effective.len());
                    effective.push(registration.action.clone());
                    scopes.push(registration.scope);
                }
                Some(at) => {
                    let incumbent = scopes[at];
                    if registration.scope > incumbent {
                        shadowed.push(ShadowedAction {
                            id,
                            winner: registration.scope,
                            shadowed: incumbent,
                        });
                        effective[at] = registration.action.clone();
                        scopes[at] = registration.scope;
                    } else {
                        shadowed.push(ShadowedAction {
                            id,
                            winner: incumbent,
                            shadowed: registration.scope,
                        });
                    }
                }
            }
        }

        Self {
            effective,
            shadowed,
        }
    }

    /// Returns unique effective actions ordered by each id’s first active registration position.
    pub fn actions(&self) -> &[Action<Message>] {
        &self.effective
    }

    /// How many actions are effective.
    #[must_use]
    pub fn len(&self) -> usize {
        self.effective.len()
    }

    /// Whether the snapshot is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.effective.is_empty()
    }

    /// Every duplicate-id resolution that occurred, in the order encountered.
    #[must_use]
    pub fn shadowed(&self) -> &[ShadowedAction] {
        &self.shadowed
    }

    /// Returns the unique effective in-scope action for `id`, if any.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Action<Message>> {
        self.effective.iter().find(|a| a.id() == id)
    }

    /// Prepare a [`Catalogue`] over the effective actions.
    #[must_use]
    pub fn catalogue(&self) -> Catalogue {
        Catalogue::new(&self.effective)
    }
}

//! Scroll-arrow affordance for a horizontally-scrolling strip.
//!
//! Wheel and drag are pointer gestures. A keyboard-only user, a user on a
//! trackpad without horizontal scroll, and a user of a pointer device with no
//! wheel all need a *clickable* way to reach the hidden end of a strip. These
//! are that control: a `‹` / `›` pair that scrolls by one tab.
//!
//! # Accessible names are mandatory, by construction
//!
//! `enz_toolbar_icon` falls back to the icon glyph name when no
//! `aria_label` is set (in the `toolbar_icon` module), so an unnamed chevron button
//! announces as *"chevron-left"* — the artwork's file name, not the action.
//! An unnamed chevron pair makes four controls announce only two glyph names
//! between them.
//!
//! That is the worst shape of a11y bug, because the node *is* published, does
//! carry `Role::Button`, and *is* keyboard reachable — every automated check
//! passes and the name is simply wrong.
//!
//! So [`ScrollArrows::new`] takes the two names as required arguments.
//! There is no builder default and no `Option` to leave unset: a caller cannot
//! construct this affordance without saying what the buttons do. Defaults are
//! provided as named constants ([`DEFAULT_PREV_LABEL`] /
//! [`DEFAULT_NEXT_LABEL`]) for callers who want the generic wording, but they
//! must be passed explicitly.
//!
//! # Disabled at the ends
//!
//! An arrow pointing at content that does not exist is a lie. Each button is
//! enabled only while its own side is actually clipped, which is the same
//! per-side signal the fades use
//! ([`ScrollFadeEdges`]).

use crate::scroll_fade_edge::ScrollFadeEdges;
use iced::Element;

/// Generic accessible name for the "scroll back" arrow.
///
/// A *default value*, not a default *behaviour*: it must still be passed
/// explicitly to [`ScrollArrows::new`]. A caller with domain wording ("Previous
/// month", "Earlier tabs") should pass that instead.
pub const DEFAULT_PREV_LABEL: &str = "Scroll tabs left";

/// Generic accessible name for the "scroll forward" arrow. See
/// [`DEFAULT_PREV_LABEL`].
pub const DEFAULT_NEXT_LABEL: &str = "Scroll tabs right";

/// Icon used for the "scroll back" arrow.
pub const PREV_ICON: &str = "chevron-left";
/// Icon used for the "scroll forward" arrow.
pub const NEXT_ICON: &str = "chevron-right";

/// A pair of scroll-arrow buttons for a horizontal strip.
///
/// Construct with [`ScrollArrows::new`], which requires both accessible names.
#[must_use]
pub struct ScrollArrows<Message> {
    prev_label: String,
    next_label: String,
    on_prev: Message,
    on_next: Message,
    edges: ScrollFadeEdges,
}

impl<Message: Clone> ScrollArrows<Message> {
    /// Build the arrow pair.
    ///
    /// `prev_label` / `next_label` are the accessible names and are
    /// required — see the module docs for why they are not optional. Passing an
    /// empty string is rejected at build time by
    /// [`Self::names_are_meaningful`], which the tests assert.
    pub fn new(
        prev_label: impl Into<String>,
        next_label: impl Into<String>,
        on_prev: Message,
        on_next: Message,
    ) -> Self {
        Self {
            prev_label: prev_label.into(),
            next_label: next_label.into(),
            on_prev,
            on_next,
            edges: ScrollFadeEdges::NONE,
        }
    }

    /// Which sides currently have hidden content; an arrow is enabled only for
    /// a side that does.
    ///
    /// Derive from live geometry with
    /// [`ScrollFadeEdges::from_geometry`], the same signal the fades use, so
    /// the arrow and the fade can never disagree about whether there is
    /// anything that way.
    pub fn edges(mut self, edges: ScrollFadeEdges) -> Self {
        self.edges = edges;
        self
    }

    /// Whether both names are non-empty after trimming.
    ///
    /// An empty accessible name is as bad as a glyph name — the control
    /// announces as an anonymous button. This is the check that makes the
    /// "names are mandatory" claim enforceable rather than decorative.
    pub fn names_are_meaningful(&self) -> bool {
        !self.prev_label.trim().is_empty() && !self.next_label.trim().is_empty()
    }

    /// The accessible name of the "scroll back" arrow.
    pub fn prev_label(&self) -> &str {
        &self.prev_label
    }

    /// The accessible name of the "scroll forward" arrow.
    pub fn next_label(&self) -> &str {
        &self.next_label
    }

    /// Whether the "scroll back" arrow is enabled (content hidden to the left).
    pub fn prev_enabled(&self) -> bool {
        self.edges.left
    }

    /// Whether the "scroll forward" arrow is enabled (content hidden right).
    pub fn next_enabled(&self) -> bool {
        self.edges.right
    }

    /// Build the "scroll back" button.
    pub fn prev_button<'a>(&self) -> Element<'a, Message>
    where
        Message: 'a,
    {
        crate::toolbar_icon::enz_toolbar_icon(PREV_ICON, self.on_prev.clone())
            // The name is the action, never the glyph.
            .aria_label(self.prev_label.clone())
            .enabled(self.prev_enabled())
            .into_element()
    }

    /// Build the "scroll forward" button.
    pub fn next_button<'a>(&self) -> Element<'a, Message>
    where
        Message: 'a,
    {
        crate::toolbar_icon::enz_toolbar_icon(NEXT_ICON, self.on_next.clone())
            .aria_label(self.next_label.clone())
            .enabled(self.next_enabled())
            .into_element()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

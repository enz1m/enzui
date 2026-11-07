//! The adaptive navigation shell state contract: the one place that
//! decides whether a sidebar is laid out beside the content or floated over it.
//!
//! # Why this is a separate, pure module
//!
//! This module is the contract that separates four things constantly
//! conflated in shell code:
//!
//! | concept | owner | lives for |
//! |---|---|---|
//! | user preference ([`SidebarPreference`]) | the application | the session |
//! | effective presentation ([`SidebarPresentation`]) | this module | one layout pass |
//! | overlay-open ([`NavSplitState::overlay_open`]) | the application | until dismissed |
//! | sidebar width ([`SidebarWidth`]) | the application, validated here | the session |
//!
//! Collapsing preference into presentation is the defect this exists to
//! prevent: *"Compact mode may force an overlay drawer but must not overwrite
//! the user's expanded-mode preference."* If a narrow window rewrote the
//! preference to `Hidden`, widening the window again would leave the sidebar
//! shut and the user's actual choice destroyed — an irreversible loss caused by
//! a transient geometry event.
//!
//! So [`resolve`] is a pure function of (preference, size class, geometry)
//! that returns the effective presentation without ever mutating the
//! preference. The application stores `SidebarPreference`; the view stores
//! nothing.
//!
//! # The collapse rule lives here and nowhere else
//!
//! Both the inline split view and the overlay drawer are presentations of the
//! *same* decision, so the decision is made once, in [`resolve`], and consumed
//! twice. A widget that re-derived "am I narrow?" from a local literal would be
//! a second breakpoint authority: two classifiers disagreeing at one width
//! produce a flicker at exactly the boundary.
//!
//! The rule is expressed against [`EnzSizeClass`], the transported breakpoint
//! catalog — not a raw pixel literal. Callers pass the class they resolved from
//! the live theme's scale.
//!
//! # The widget that consumes this: [`navigation_split_view`](crate::navigation_split_view)
//!
//! This module is the state half — the pure decision about how a sidebar
//! should present. The view half is
//! [`enz_navigation_split_view`](crate::navigation_split_view::enz_navigation_split_view),
//! which calls [`resolve_for_width`] on every layout pass, builds the
//! appropriate tree (inline split, overlay drawer, or content-only), and
//! handles the structural consequences (scrim, focus containment, accessible
//! landmarks).
//!
//! To render a sidebar + content layout, use
//! [`crate::navigation_split_view`], the widget composed in a `view()`
//! function. This module holds the collapse decision (when inline becomes
//! overlay) for that widget and its tests.
//!
//! # Content-minimum is a second, independent reason to collapse
//!
//! Size class alone is not sufficient. A 1400px window whose split view sits
//! inside a 380px pane must collapse, and a wide-but-short region with a huge
//! sidebar width must collapse too, because the *content* would be squeezed
//! below usability. [`resolve`] therefore also collapses when
//! `available - sidebar_width < content_minimum`, which is a constraint-local
//! decision: it reads the parent-assigned width, not the window's.

use enzui_core::metrics::OrderedRangeF32;
use enzui_theme::{BreakpointScale, EnzSizeClass};

/// The user's durable choice about the sidebar.
///
/// This is application state. Nothing in this module ever writes it; a
/// transient geometry change must not destroy a deliberate choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SidebarPreference {
    /// Let the shell decide from the available geometry. The default.
    #[default]
    Automatic,
    /// The user explicitly wants the sidebar visible.
    ///
    /// Honoured as an *inline* pane when there is room, and as an *overlay*
    /// drawer when there is not — a forced-shown sidebar in a 320px window
    /// would leave no content at all, so the presentation adapts even though
    /// the preference does not.
    Shown,
    /// The user explicitly dismissed the sidebar. It stays hidden at every
    /// width until they say otherwise.
    Hidden,
}

/// How the sidebar is actually presented for one layout pass.
///
/// Derived, never stored: this is the output of [`resolve`] and is recomputed
/// from the current geometry every pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarPresentation {
    /// Laid out beside the content, both sharing the available width.
    Inline,
    /// Floated above the content as a drawer, with a scrim.
    ///
    /// Whether the drawer is currently *open* is separate state
    /// ([`NavSplitState::overlay_open`]) — `Overlay` describes the mode, not
    /// the visibility.
    Overlay,
    /// Not present at all: absent from layout, events, focus and semantics.
    ///
    /// Absent, not hidden. The structural-visibility rule: clipping,
    /// opacity and zero-size `Space` do not remove a control from the
    /// operations or accessibility tree, so a hidden sidebar must not be built.
    Hidden,
}

impl SidebarPresentation {
    /// Does this presentation put the sidebar in the layout beside content?
    #[must_use]
    pub const fn is_inline(self) -> bool {
        matches!(self, Self::Inline)
    }

    /// Does this presentation float the sidebar over the content?
    #[must_use]
    pub const fn is_overlay(self) -> bool {
        matches!(self, Self::Overlay)
    }

    /// Is the sidebar absent from the tree entirely?
    #[must_use]
    pub const fn is_hidden(self) -> bool {
        matches!(self, Self::Hidden)
    }
}

/// A validated sidebar width in logical px.
///
/// The bounds are not decoration. A sidebar narrower than [`MIN_SIDEBAR_WIDTH`]
/// cannot show a readable label beside an icon, and one wider than
/// [`MAX_SIDEBAR_WIDTH`] starves the content pane on any realistic window. A
/// caller that computes a width from a drag gesture will produce out-of-range
/// and non-finite values, so the type refuses to hold one.
///
/// Construction clamps rather than rejecting a finite out-of-range value,
/// because a drag that runs past the end should stop at the end rather than
/// snap back to a default. Non-finite input has no defensible clamp target and
/// falls back to [`DEFAULT_SIDEBAR_WIDTH`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SidebarWidth {
    px: f32,
    adaptive: bool,
}

/// The narrowest sidebar that can still show an icon plus a label.
pub const MIN_SIDEBAR_WIDTH: f32 = 180.0;

/// The widest sidebar the shell will honour.
pub const MAX_SIDEBAR_WIDTH: f32 = 480.0;

/// The width used when none is supplied, and the fallback for non-finite input.
pub const DEFAULT_SIDEBAR_WIDTH: f32 = 260.0;

/// Minimum share of an inline split reserved for the content pane when the
/// framework default sidebar width is in use.
///
/// Reserving the content share, rather than assigning widths to named window
/// rungs, makes the policy constraint-local: a split nested in a 768 px region
/// receives the same answer as a 768 px window. The sidebar keeps its 260 px
/// default while that leaves at least this share, then yields continuously down
/// to [`MIN_SIDEBAR_WIDTH`]. Below that floor the sidebar does not get narrower;
/// the existing content-minimum/compact rules decide whether it becomes a
/// drawer.
pub const DEFAULT_CONTENT_SHARE: f32 = 0.775;

/// Resolve the width of an inline sidebar for its assigned region.
///
/// `adaptive` is for framework defaults. Explicit caller widths pass `false`
/// and are returned byte-for-byte. The floor is intentional: below 180 px an
/// icon plus label is no longer a useful sidebar, so adaptation stops rather
/// than manufacturing an unreadable rail.
#[must_use]
pub fn resolve_sidebar_width(width: f32, adaptive: bool, available: f32) -> f32 {
    if !adaptive || !available.is_finite() {
        return width;
    }

    let content_reservation = available.max(0.0) * DEFAULT_CONTENT_SHARE;
    (available - content_reservation).clamp(MIN_SIDEBAR_WIDTH, width)
}

/// The narrowest content pane worth rendering beside a sidebar.
///
/// Below this the split stops being a split: the content column is too narrow
/// to hold a line of text plus its padding, so the shell collapses the sidebar
/// to an overlay and gives the content the whole region. This is the
/// content-minimum constraint.
pub const DEFAULT_CONTENT_MINIMUM: f32 = 320.0;

impl SidebarWidth {
    /// The validated range, `[MIN, MAX]`.
    ///
    /// Built through [`OrderedRangeF32`], so the clamp cannot panic on a
    /// reversed pair and the bounds are proven finite once rather than at every
    /// call site.
    #[must_use]
    pub fn range() -> OrderedRangeF32 {
        // Both constants are finite and ascending, and the check runs at
        // compile time.
        const {
            OrderedRangeF32::new(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH)
                .expect("MIN_SIDEBAR_WIDTH <= MAX_SIDEBAR_WIDTH, both finite")
        }
    }

    /// Clamp a finite width into range; fall back to [`DEFAULT_SIDEBAR_WIDTH`]
    /// for a non-finite one.
    ///
    /// `0.0` is not treated as "unset". It is expressible intent meaning
    /// "as narrow as possible", so it clamps to [`MIN_SIDEBAR_WIDTH`] like any
    /// other too-small value rather than silently becoming the default — the
    /// recorded rule that a `0` cap must be honoured literally and not reversed
    /// into "unbounded".
    #[must_use]
    pub fn new(px: f32) -> Self {
        if px.is_finite() {
            Self {
                px: Self::range().clamp(px),
                // Calling `new` is an explicit pin, even when `px` happens to
                // equal the framework default.
                adaptive: false,
            }
        } else {
            Self {
                px: DEFAULT_SIDEBAR_WIDTH,
                adaptive: false,
            }
        }
    }

    /// The validated width in logical px. Always finite and in range.
    #[must_use]
    pub fn get(self) -> f32 {
        self.px
    }

    /// Resolve this width for an assigned region.
    ///
    /// A value built with [`SidebarWidth::new`] is pinned and never changes;
    /// only [`SidebarWidth::default`] participates in the adaptive policy.
    #[must_use]
    pub fn resolve_for_available(self, available: f32) -> f32 {
        resolve_sidebar_width(self.px, self.adaptive, available)
    }
}

impl Default for SidebarWidth {
    fn default() -> Self {
        Self {
            px: DEFAULT_SIDEBAR_WIDTH,
            adaptive: true,
        }
    }
}

/// The geometry [`resolve`] reads, all constraint-local.
///
/// `available` is the width the parent assigned this shell, not the
/// window's. That is the whole point of the constraint-local rule: a 360px
/// pane inside a 1400px window must resolve compact without a window
/// subscription, so a split view nested in a narrow pane collapses even though
/// the window is wide.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavSplitGeometry {
    /// Width assigned to the whole shell by its parent, in logical px.
    pub available: f32,
    /// The validated sidebar width.
    pub sidebar: SidebarWidth,
    /// The narrowest content pane worth rendering beside the sidebar.
    pub content_minimum: f32,
}

impl Default for NavSplitGeometry {
    fn default() -> Self {
        Self {
            available: 0.0,
            sidebar: SidebarWidth::default(),
            content_minimum: DEFAULT_CONTENT_MINIMUM,
        }
    }
}

impl NavSplitGeometry {
    /// Geometry for an assigned width, with the default sidebar and minimum.
    #[must_use]
    pub fn new(available: f32) -> Self {
        Self {
            available,
            ..Self::default()
        }
    }

    /// Builder: set the sidebar width.
    #[must_use]
    pub fn with_sidebar(mut self, sidebar: SidebarWidth) -> Self {
        self.sidebar = sidebar;
        self
    }

    /// Builder: set the content minimum.
    ///
    /// A non-finite or negative minimum is replaced by
    /// [`DEFAULT_CONTENT_MINIMUM`]; `0.0` is honoured literally and means "any
    /// content width is acceptable", which disables the content-minimum reason
    /// to collapse (the size-class reason still applies).
    #[must_use]
    pub fn with_content_minimum(mut self, minimum: f32) -> Self {
        self.content_minimum = if minimum.is_finite() && minimum >= 0.0 {
            minimum
        } else {
            DEFAULT_CONTENT_MINIMUM
        };
        self
    }

    /// Would an inline sidebar leave enough content beside it?
    ///
    /// The content-minimum half of the collapse rule, split out so it can be
    /// tested and reasoned about on its own.
    #[must_use]
    pub fn content_fits_beside_sidebar(&self) -> bool {
        if !self.available.is_finite() {
            // A non-finite assignment carries no information about how much
            // room there is. Assume the widest layout, matching the breakpoint
            // catalog's documented degenerate-width policy, so the shell does
            // not flash a collapsed sidebar during a pre-layout probe.
            return true;
        }
        self.available - self.sidebar.resolve_for_available(self.available) >= self.content_minimum
    }
}

/// The collapse rule. The single decision both presentations consume.
///
/// Returns the effective [`SidebarPresentation`] for a preference and a
/// geometry, and never mutates the preference.
///
/// # The rule, in order
///
/// 1. [`SidebarPreference::Hidden`] wins outright. An explicit dismissal is
///    honoured at every width; nothing about geometry can un-hide a sidebar the
///    user closed.
/// 2. Otherwise the shell asks whether an inline sidebar is *viable*, which
///    takes two independent conditions, both of which must hold:
///    * the region is not in a compact size class ([`EnzSizeClass::Compact`]),
///      as classified by the transported breakpoint catalog; and
///    * the content pane would still meet its minimum beside the sidebar
///      ([`NavSplitGeometry::content_fits_beside_sidebar`]).
/// 3. If inline is viable, [`SidebarPreference::Automatic`] and
///    [`SidebarPreference::Shown`] both resolve [`SidebarPresentation::Inline`].
/// 4. If inline is not viable, the two preferences diverge, and this is the
///    substance of the contract:
///    * `Shown` becomes [`SidebarPresentation::Overlay`] — the user asked for a
///      sidebar and gets one, as a drawer, because there is no room beside the
///      content. Their preference is untouched, so widening restores inline.
///    * `Automatic` also becomes `Overlay`, so the navigation remains
///      *reachable* (via a trigger) rather than disappearing. `Overlay`
///      describes the mode; whether the drawer is open is separate state.
///
/// # Why two independent collapse reasons and not one
///
/// Size class alone misses a wide-but-crowded region: a 1200px shell with a
/// 480px sidebar and a 900px content minimum has to collapse, and no width
/// threshold on the *shell* can express that, because the answer depends on the
/// sidebar width too. Content minimum alone misses the compact case, where even
/// a tiny sidebar is the wrong shape for a phone-width region. Both are
/// necessary; neither is sufficient.
#[must_use]
pub fn resolve(
    preference: SidebarPreference,
    class: EnzSizeClass,
    geometry: NavSplitGeometry,
) -> SidebarPresentation {
    if preference == SidebarPreference::Hidden {
        return SidebarPresentation::Hidden;
    }

    let inline_viable = class != EnzSizeClass::Compact && geometry.content_fits_beside_sidebar();

    if inline_viable {
        SidebarPresentation::Inline
    } else {
        SidebarPresentation::Overlay
    }
}

/// Classify an assigned width and resolve in one call, against a transported
/// breakpoint scale.
///
/// This is the entry point widgets use. Taking the [`BreakpointScale`] as a
/// parameter — rather than reaching for a global default — is what keeps this
/// module from becoming a second breakpoint authority: the scale comes from the
/// live runtime theme, so a custom theme or a live theme switch changes the
/// collapse point without this code knowing anything about pixels.
#[must_use]
pub fn resolve_for_width(
    preference: SidebarPreference,
    scale: BreakpointScale,
    geometry: NavSplitGeometry,
) -> SidebarPresentation {
    resolve(preference, scale.classify(geometry.available), geometry)
}

/// The application-owned durable state of a navigation shell.
///
/// Everything here survives layout; nothing here is derived from geometry. The
/// view holds none of it: durable state is application-owned and geometry is
/// view-derived.
///
/// The one method that changes the sidebar's visibility,
/// [`toggle_for_presentation`](Self::toggle_for_presentation), needs to know the *current* presentation
/// because "toggle the sidebar" means two different things in the two modes:
/// inline it flips the preference, overlay it flips the drawer. Routing both
/// through one method is what stops a caller flipping the preference when they
/// meant to open a drawer, which is the silent way a user's expanded-mode
/// choice gets destroyed by a narrow-window interaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NavSplitState {
    /// The user's durable choice.
    pub preference: SidebarPreference,
    /// Whether the overlay drawer is currently open.
    ///
    /// Meaningful only while the effective presentation is
    /// [`SidebarPresentation::Overlay`]; an inline sidebar is visible without
    /// being "open". Kept across a presentation change rather than reset, so a
    /// drawer that was open when the window widened does not leave a stale flag
    /// behind when it narrows again — [`effective_overlay_open`](Self::effective_overlay_open) gates it on
    /// the presentation instead.
    pub overlay_open: bool,
}

impl NavSplitState {
    /// A shell with the default preference and a closed drawer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct the initial state for a shell whose
    /// [`sidebar_first`](crate::app_shell::ShellTokens::sidebar_first) token
    /// may be set.
    ///
    /// When `sidebar_first` is `true` the drawer starts open
    /// (`overlay_open: true`), so the sidebar is the landing view at narrow
    /// widths. When `false` this returns the same default state as
    /// [`Self::new`].
    ///
    /// `overlay_open` keeps its standard meaning everywhere — toggle,
    /// dismiss, scrim, Escape — because the initial value is seeded here
    /// rather than inverted at every read site.
    #[must_use]
    pub fn for_sidebar_first(sidebar_first: bool) -> Self {
        Self {
            overlay_open: sidebar_first,
            ..Self::default()
        }
    }

    /// Is the drawer *actually* showing?
    ///
    /// Both conditions matter: the flag alone is stale in inline mode, and the
    /// presentation alone cannot tell an open drawer from a closed one. A
    /// caller that checked only the flag would build drawer content into an
    /// inline layout.
    #[must_use]
    pub fn effective_overlay_open(&self, presentation: SidebarPresentation) -> bool {
        presentation.is_overlay() && self.overlay_open
    }

    /// Handle a "toggle the sidebar" interaction for the current presentation.
    ///
    /// * Overlay — flips [`Self::overlay_open`] only. The preference is
    ///   deliberately not touched: this is the case the contract exists to
    ///   protect, because a compact window forcing a drawer must not rewrite
    ///   what the user chose for a wide one.
    /// * Inline — the sidebar is visible, so toggling means dismissing it:
    ///   the preference becomes [`SidebarPreference::Hidden`].
    /// * Hidden — the sidebar is dismissed, so toggling restores it as an
    ///   explicit [`SidebarPreference::Shown`] rather than `Automatic`. Going
    ///   back to `Automatic` would be wrong: the user just asked for it, and
    ///   `Automatic` in a compact region would immediately hide it again.
    pub fn toggle_for_presentation(&mut self, presentation: SidebarPresentation) {
        match presentation {
            SidebarPresentation::Overlay => self.overlay_open = !self.overlay_open,
            SidebarPresentation::Inline => self.preference = SidebarPreference::Hidden,
            SidebarPresentation::Hidden => self.preference = SidebarPreference::Shown,
        }
    }

    /// Dismiss the overlay drawer (scrim click, Escape, or a navigation).
    ///
    /// Only clears the drawer flag; the preference is untouched, so dismissing
    /// a drawer never destroys the user's expanded-mode choice.
    pub fn dismiss_overlay(&mut self) {
        self.overlay_open = false;
    }
}

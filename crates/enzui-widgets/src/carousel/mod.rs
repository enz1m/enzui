//! [`EnzCarousel`] — accessible keyed paging with indicators, previous/next
//! chrome, keyboard paging, and optional swipe. Not tab strips: arbitrary-
//! content paging; compact-position-readout indicators over labelled-navigation
//! surfaces. Named-page callers use
//! [`EnzViewSwitcher`](crate::view_switcher::EnzViewSwitcher).
//!
//! # Only the active page exists
//!
//! Page contents supplied to the builder ([`CarouselPage::new`]) are built
//! only for the resolved active page. Inactive pages never become
//! [`Element`]s: no widget tree, no state, no presence in the semantics tree.
//! This is structural, not enforced at draw time, so an API that builds
//! every page and hides the inactive ones cannot be expressed.
//!
//! # Selection is keyed
//!
//! [`PageId`] selections (never indexes): reorder-impervious shown pages. See
//! [`paging`](enzui_core::paging) resolution/fallback rules.
//!
//! # One paging path
//!
//! Indicator/chrome/arrow-key/`Home`/`End`/swipe inputs all reduce to
//! [`PageRequest`](enzui_core::paging::PageRequest)s
//! [`apply`](enzui_core::paging::apply)-resolved: boundary/wrap-agreement
//! across input methods; no-change-selection requests emit zero messages.

mod tokens;
mod view;
mod widget;

pub use tokens::{
    ARROW_GLYPH, BODY_PADDING, BODY_SPACING, CHROME_INSET, CHROME_SIZE, CarouselStageStyle,
    CarouselTokens, DEFAULT_ID_PREFIX, DOT_SIZE, INDICATOR_FADE_MS, INDICATOR_SPACING,
    INSIDE_ARROW_INSET, Indicator, LINE_ACTIVE_WIDTH, LINE_HEIGHT, LINE_WIDTH, MAX_VISIBLE_DOTS,
    SWIPE_ARBITRATION_DEADBAND, body_slot_height, chrome_clearance, chrome_clearance_for,
};

use enzui_core::paging::{PageId, Wrap};
use iced::{Element, Length};

/// One carousel page: a stable id plus a builder for its content.
///
/// The builder is `FnOnce` and is consumed only if this page is the resolved
/// active one, so building a page costs exactly as much as showing it.
pub struct CarouselPage<'a, Message> {
    id: PageId,
    build: Box<dyn FnOnce() -> Element<'a, Message> + 'a>,
}

impl<'a, Message> CarouselPage<'a, Message> {
    /// Declare a page with a stable id and a content builder.
    pub fn new<F>(id: impl Into<PageId>, build: F) -> Self
    where
        F: FnOnce() -> Element<'a, Message> + 'a,
    {
        Self {
            id: id.into(),
            build: Box::new(build),
        }
    }

    /// This page's stable identity.
    #[must_use]
    pub fn id(&self) -> &PageId {
        &self.id
    }
}

/// Where the previous and next arrows sit, against the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ArrowPlacement {
    /// Over the page, inset from its sides: a translucent dark circle with
    /// a light chevron that reads over any picture, hidden at a clamped
    /// end (default).
    #[default]
    Inside,
    /// Centred on the page's sides, half over it, in the stage's own
    /// colours.
    Edge,
    /// In a gutter beside the page, which narrows to make room, in the
    /// stage's own colours: for pages whose content the arrows must not
    /// cover.
    Outside,
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Paged content with indicators, optional chrome, keyboard paging, and
/// optional swipe. See the [module docs](self).
#[must_use]
pub struct EnzCarousel<'a, Message> {
    pages: Vec<CarouselPage<'a, Message>>,
    selected: Option<PageId>,
    on_select: Box<dyn Fn(PageId) -> Message + 'a>,
    indicator: Indicator,
    show_navigation: bool,
    arrow_placement: ArrowPlacement,
    chrome_inset: bool,
    swipe_enabled: bool,
    wrap: Wrap,
    enabled: bool,
    label: Option<String>,
    width: Length,
    height: Length,
    bordered: bool,
    tokens: Option<CarouselTokens>,
    stage_style: CarouselStageStyle,
    id_prefix: String,
}

/// Construct a carousel over keyed pages.
pub fn enz_carousel<'a, Message, F>(
    pages: Vec<CarouselPage<'a, Message>>,
    selected: Option<PageId>,
    on_select: F,
) -> EnzCarousel<'a, Message>
where
    Message: Clone + 'a,
    F: Fn(PageId) -> Message + 'a,
{
    EnzCarousel {
        pages,
        selected,
        on_select: Box::new(on_select),
        indicator: Indicator::default(),
        show_navigation: false,
        arrow_placement: ArrowPlacement::default(),
        chrome_inset: false,
        swipe_enabled: false,
        wrap: Wrap::default(),
        enabled: true,
        label: None,
        width: Length::Fill,
        height: Length::Fill,
        bordered: false,
        tokens: None,
        stage_style: CarouselStageStyle::default(),
        id_prefix: DEFAULT_ID_PREFIX.to_owned(),
    }
}

impl<'a, Message: Clone + 'a> EnzCarousel<'a, Message> {
    /// Show the one-pixel stage outline. Off by default; the stroke geometry
    /// remains reserved while transparent so this never changes layout.
    pub fn bordered(mut self, bordered: bool) -> Self {
        self.bordered = bordered;
        self
    }

    /// Indicator style (default [`Indicator::Lines`]).
    pub fn indicator(mut self, indicator: Indicator) -> Self {
        self.indicator = indicator;
        self
    }

    /// Show previous/next chrome overlaying the leading/trailing edges.
    ///
    /// Off by default, (`show-navigation: false`).
    pub fn show_navigation(mut self, show: bool) -> Self {
        self.show_navigation = show;
        self
    }

    /// Where the previous and next arrows sit; see [`ArrowPlacement`].
    pub fn arrow_placement(mut self, placement: ArrowPlacement) -> Self {
        self.arrow_placement = placement;
        self
    }

    /// Inset the page body so its content clears the prev/next chrome.
    ///
    /// The chrome is stacked over the body. That is right for a full-bleed
    /// gallery (arrows floating over a photograph) and wrong for interactive
    /// page content (arrows covering a card's own button). Off by default, so
    /// existing layouts paint as before. When on, the widget adds its own
    /// [`chrome_clearance`] to the leading and trailing body padding, so
    /// callers need not compute it from the chrome constants, which goes
    /// stale when the chrome is resized. The inset follows the
    /// chrome actually painted: with [`show_navigation`](Self::show_navigation)
    /// off or a single page there is no chrome and the stage keeps its full
    /// width. The centred indicator strip is unaffected.
    pub fn chrome_inset(mut self, inset: bool) -> Self {
        self.chrome_inset = inset;
        self
    }

    /// Enable horizontal swipe-to-page.
    ///
    /// Off by default, (`swipe-enabled: false`).
    pub fn swipe_enabled(mut self, enabled: bool) -> Self {
        self.swipe_enabled = enabled;
        self
    }

    /// Wrap past the ends instead of stopping (default [`Wrap::Clamp`]).
    pub fn wrap(mut self, wrap: Wrap) -> Self {
        self.wrap = wrap;
        self
    }

    /// Disable all paging input.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Accessible name for the carousel container.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Namespace for the stable per-indicator widget ids. Per-indicator
    /// `"{id_prefix}-{page id}"` publishing: AT activation routing to specific
    /// pages (never focused-whatever only). [`DEFAULT_ID_PREFIX`] defaults;
    /// distinct prefixes for same-screen carousels (collision-free per-dot ids
    /// — [`EnzPagination`](crate::pagination::EnzPagination)-identical disciplines).
    /// Namespace keys = own [`PageId`]s: reorder/rebuild-stable ids (page-
    /// following dots) + distinct per page. Pre-derived duplicate-`PageId`
    /// deduplication: zero shared live-dot ids even release (elided
    /// duplicate-id `debug_assert!`s).
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }

    /// Override the width (default [`Length::Fill`]).
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Override the height (default [`Length::Fill`]).
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    /// Pin tokens instead of resolving them from the runtime theme.
    pub fn tokens(mut self, tokens: CarouselTokens) -> Self {
        // Normalize at the public boundary so no non-finite geometry token can
        // reach renderer coordinates.
        self.tokens = Some(tokens.sanitized());
        self
    }

    /// Choose the stage fill and border; defaults to [`CarouselStageStyle::Filled`].
    ///
    /// [`CarouselStageStyle::Transparent`] reveals the host backdrop without
    /// changing indicator, chrome or page geometry. Border and radius settings
    /// have no visual effect while transparent and apply again when filled.
    pub fn stage_style(mut self, style: CarouselStageStyle) -> Self {
        self.stage_style = style;
        self
    }

    /// The current page order.
    #[must_use]
    pub fn ids(&self) -> Vec<PageId> {
        self.pages.iter().map(|p| p.id.clone()).collect()
    }

    /// Deduplicate pages by [`PageId`], keeping the first occurrence of each.
    ///
    /// Returns the number of pages dropped. Called by [`Self::into_element`]
    /// to prevent ambiguous selection and duplicate indicators in release
    /// builds where the `debug_assert!` is elided.
    fn dedup_pages(pages: &mut Vec<CarouselPage<'a, Message>>) -> usize {
        let before = pages.len();
        let mut seen = std::collections::HashSet::with_capacity(pages.len());
        pages.retain(|p| seen.insert(p.id.clone()));
        before - pages.len()
    }
}

//! Shell-wide geometry, responsive split policy, and activity placement.

use super::*;

/// The shell's resolution for one assigned width, in logical pixels.
///
/// Geometry-aware slot closures receive the same shell-wide value used to build
/// their subtree. It does not report each receiving slot's laid-out bounds.
/// `content_width` describes the content region before a secondary pane shares
/// it; floating overlap is reported separately by `content_inset`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShellGeometry {
    /// The sidebar's resolved presentation, independent of drawer open state.
    pub sidebar: SidebarPresentation,
    /// Resolved sidebar width in logical pixels.
    ///
    /// An adaptive default may narrow while inline; an overlay drawer retains the
    /// normalized requested width.
    pub sidebar_width: f32,
    /// Whether an inline sidebar has a separate sidebar-side titlebar pane.
    ///
    /// Requires `TitlebarSidebarContent` with `SidebarHeader::SplitBand`.
    pub split_titlebar: bool,
    /// Whether the inline sidebar starts alongside the top of the content region.
    ///
    /// True for `TitlebarSidebarContent` with `SidebarHeader::Inside`, or for
    /// `FloatingTitlebarSidebar` regardless of the header token.
    pub sidebar_full_height: bool,
    /// Whether the layout has a sidebar and allows collapse/reveal controls.
    ///
    /// This policy flag does not establish that a toggle callback was supplied.
    pub shows_toggle: bool,
    /// Floating chrome clearance as `[top, right, bottom, left]`, in logical pixels.
    ///
    /// Zero for non-floating layouts. A floating titlebar reserves the top edge;
    /// a floating sidebar reserves the left edge in LTR or right edge in RTL.
    /// The shell reports this clearance without applying it to content children.
    pub content_inset: [f32; 4],
    /// Primary pane corner radii as `[top-left, top-right, bottom-left, bottom-right]`.
    /// Values include normalized style overrides and resolved flush defaults.
    pub content_corner_radii: [f32; 4],
    /// Secondary pane corner radii in the same order as `content_corner_radii`.
    pub secondary_content_corner_radii: [f32; 4],
    /// Content-region width after outer insets and any inline sidebar, gutter,
    /// and seam, before a secondary pane shares it.
    ///
    /// Floating chrome overlap does not reduce this width; see `content_inset`.
    pub content_width: f32,
    /// Whether the split policy permits a secondary pane at this width.
    ///
    /// Evaluated even without a supplied secondary slot. A narrow beside split
    /// returns false under `NarrowSplit::Drop`, or true with a `Below` axis under
    /// `NarrowSplit::Restack`. A requested below split always returns true.
    pub secondary_content_fits: bool,
    /// Effective axis after narrow-split policy.
    ///
    /// A beside split that does not fit becomes `Below` under `Restack`. Under
    /// `Drop`, it retains `Beside` and `secondary_content_fits` is false.
    pub split_axis: SplitAxis,
}

impl ShellGeometry {
    /// Primary scrollbar clearance for floating chrome and rounded pane corners.
    /// Each edge clears both adjacent corner radii or the floating chrome,
    /// whichever is larger, so custom scrollbar widths remain inside the pane.
    /// Pass this to a scroll surface's `scrollbar_insets` in a geometry-aware
    /// content closure. It leaves the content viewport unchanged and follows
    /// shell token changes and responsive subtree geometry.
    #[must_use]
    pub fn scrollbar_safe_area(&self) -> iced::Padding {
        self.scrollbar_clearance(self.content_corner_radii)
    }

    /// Secondary scrollbar clearance for floating chrome and rounded pane corners.
    #[must_use]
    pub fn secondary_scrollbar_safe_area(&self) -> iced::Padding {
        self.scrollbar_clearance(self.secondary_content_corner_radii)
    }

    fn scrollbar_clearance(&self, corners: [f32; 4]) -> iced::Padding {
        let [top, right, bottom, left] = self.content_inset;
        let [tl, tr, bl, br] = corners;
        iced::Padding {
            top: top.max(tl).max(tr),
            right: right.max(tr).max(br),
            bottom: bottom.max(bl).max(br),
            left: left.max(tl).max(bl),
        }
    }

    /// Whether any reported floating-chrome inset is positive.
    #[must_use]
    pub fn has_floating_chrome(&self) -> bool {
        self.content_inset.iter().any(|v| *v > 0.0)
    }

    /// Whether toggle policy is enabled and the sidebar is not inline.
    ///
    /// This placement predicate does not establish that the chosen layout has a
    /// titlebar band or that a toggle callback was supplied. For centered titlebar
    /// content, prefer [`crate::title_bar::EnzTitleBar::center`].
    #[must_use]
    pub fn band_carries_toggle(&self) -> bool {
        self.shows_toggle && !self.sidebar.is_inline()
    }

    /// Whether a requested beside split resolved to a permitted below split.
    ///
    /// Pass the tokens used to resolve this geometry.
    #[must_use]
    pub fn restacked_split(&self, tokens: ShellTokens) -> bool {
        tokens.split_axis.is_horizontal()
            && !self.split_axis.is_horizontal()
            && self.secondary_content_fits
    }
}

/// Resolve sidebar presentation and split policy for an assigned logical width.
///
/// Normalizes numeric tokens and resolves adaptive sidebar width. This function
/// does not apply [`ShellTokens::with_flush_defaults`]; callers needing the
/// builder's flush arrangement must resolve those defaults first.
/// Drawer open state does not change its presentation or reserved inline width.
#[must_use]
pub fn resolve_shell_geometry(
    layout: ShellLayout,
    state: NavSplitState,
    tokens: ShellTokens,
    breakpoints: BreakpointScale,
    available: f32,
) -> ShellGeometry {
    // Public token fields can bypass setters, so normalize at the consumption boundary.
    let mut tokens = tokens.normalized();
    let drawer_width = tokens.sidebar_width;
    tokens.sidebar_width = crate::nav_split::resolve_sidebar_width(
        tokens.sidebar_width,
        tokens.sidebar_width_adaptive,
        available,
    );
    // A resolved width must not be treated as another adaptive input.
    tokens.sidebar_width_adaptive = false;
    if !layout.has_sidebar() {
        let (secondary_content_fits, split_axis) = resolve_split(
            layout,
            tokens,
            breakpoints,
            SidebarPresentation::Hidden,
            available,
        );
        return ShellGeometry {
            sidebar: SidebarPresentation::Hidden,
            sidebar_width: tokens.sidebar_width,
            split_titlebar: false,
            sidebar_full_height: false,
            shows_toggle: false,
            content_inset: floating_content_inset(layout, tokens, true),
            content_width: content_region_width(
                layout,
                tokens,
                SidebarPresentation::Hidden,
                available,
            ),
            content_corner_radii: tokens.content.radius_corners_or(tokens.pane_radius),
            secondary_content_corner_radii: tokens
                .secondary_content
                .radius_corners_or(tokens.pane_radius),
            secondary_content_fits,
            split_axis,
        };
    }
    if !tokens.sidebar_collapsible {
        let (secondary_content_fits, split_axis) = resolve_split(
            layout,
            tokens,
            breakpoints,
            SidebarPresentation::Inline,
            available,
        );
        return ShellGeometry {
            sidebar: SidebarPresentation::Inline,
            sidebar_width: tokens.sidebar_width,
            split_titlebar: splits_the_band(layout, tokens, SidebarPresentation::Inline),
            sidebar_full_height: sidebar_spans_full_height(
                layout,
                tokens,
                SidebarPresentation::Inline,
            ),
            shows_toggle: false,
            content_inset: floating_content_inset(layout, tokens, true),
            content_width: content_region_width(
                layout,
                tokens,
                SidebarPresentation::Inline,
                available,
            ),
            content_corner_radii: tokens.content.radius_corners_or(tokens.pane_radius),
            secondary_content_corner_radii: tokens
                .secondary_content
                .radius_corners_or(tokens.pane_radius),
            secondary_content_fits,
            split_axis,
        };
    }
    // An overlaid floating sidebar consumes no content-region width.
    let geometry = if layout == ShellLayout::FloatingSidebar {
        NavSplitGeometry::new(available)
            .with_sidebar(SidebarWidth::new(0.0))
            .with_content_minimum(tokens.content_minimum)
    } else {
        let chrome =
            tokens.effective_edge_inset() * 2.0 + tokens.effective_sidebar_content_gutter(layout);
        NavSplitGeometry::new((available - chrome).max(0.0))
            .with_sidebar(SidebarWidth::new(tokens.sidebar_width))
            .with_content_minimum(tokens.content_minimum)
    };

    let sidebar = resolve_for_width(state.preference, breakpoints, geometry);
    let (secondary_content_fits, split_axis) =
        resolve_split(layout, tokens, breakpoints, sidebar, available);

    ShellGeometry {
        sidebar,
        // Drawers retain the requested width instead of the inline content-share
        // limit; a hidden sidebar keeps the inline width it collapses from.
        sidebar_width: if sidebar == SidebarPresentation::Overlay {
            drawer_width
        } else {
            tokens.sidebar_width
        },
        split_titlebar: splits_the_band(layout, tokens, sidebar),
        sidebar_full_height: sidebar_spans_full_height(layout, tokens, sidebar),
        shows_toggle: true,
        content_inset: floating_content_inset(
            layout,
            tokens,
            sidebar == SidebarPresentation::Inline,
        ),
        content_width: content_region_width(layout, tokens, sidebar, available),
        content_corner_radii: tokens.content.radius_corners_or(tokens.pane_radius),
        secondary_content_corner_radii: tokens
            .secondary_content
            .radius_corners_or(tokens.pane_radius),
        secondary_content_fits,
        split_axis,
    }
}

pub(crate) fn splits_the_band(
    layout: ShellLayout,
    tokens: ShellTokens,
    sidebar: SidebarPresentation,
) -> bool {
    layout == ShellLayout::TitlebarSidebarContent
        && tokens.sidebar_header == SidebarHeader::SplitBand
        && sidebar == SidebarPresentation::Inline
}

pub(crate) fn sidebar_spans_full_height(
    layout: ShellLayout,
    tokens: ShellTokens,
    sidebar: SidebarPresentation,
) -> bool {
    if sidebar != SidebarPresentation::Inline {
        return false;
    }
    match layout {
        ShellLayout::TitlebarSidebarContent => tokens.sidebar_header.sidebar_is_full_height(),
        ShellLayout::FloatingTitlebarSidebar => true,
        _ => false,
    }
}

pub(crate) fn secondary_fits_beside(
    tokens: ShellTokens,
    breakpoints: BreakpointScale,
    content_region: f32,
) -> bool {
    let usable = (content_region - tokens.effective_pane_gutter()).max(0.0);
    let secondary_demand = tokens
        .secondary_width
        .unwrap_or_else(|| tokens.effective_secondary_minimum());
    // Secondary panes have no sidebar width bounds; compare their actual demand.
    breakpoints.classify(usable) != enzui_theme::EnzSizeClass::Compact
        && (!usable.is_finite() || usable - secondary_demand >= tokens.content_minimum)
}

pub(crate) fn content_region_width(
    layout: ShellLayout,
    tokens: ShellTokens,
    sidebar: SidebarPresentation,
    available: f32,
) -> f32 {
    let after_edges = (available - tokens.effective_edge_inset() * 2.0).max(0.0);
    let inline_sidebar = matches!(
        layout,
        ShellLayout::TitlebarSidebarContent | ShellLayout::FloatingTitlebarSidebar
    ) && sidebar == SidebarPresentation::Inline;
    if inline_sidebar {
        // The seam is a row sibling and consumes width independently of the gutter.
        let seam = if tokens.effective_seam().draws_rule() {
            tokens.effective_seam_thickness()
        } else {
            0.0
        };
        (after_edges
            - tokens.sidebar_width
            - tokens.effective_sidebar_content_gutter(layout)
            - seam)
            .max(0.0)
    } else {
        after_edges
    }
}

pub(crate) fn resolve_split(
    layout: ShellLayout,
    tokens: ShellTokens,
    breakpoints: BreakpointScale,
    sidebar: SidebarPresentation,
    available: f32,
) -> (bool, SplitAxis) {
    if !tokens.split_axis.is_horizontal() {
        return (true, tokens.split_axis);
    }
    let region = content_region_width(layout, tokens, sidebar, available);
    if secondary_fits_beside(tokens, breakpoints, region) {
        return (true, SplitAxis::Beside);
    }
    match tokens.narrow_split {
        NarrowSplit::Drop => (false, SplitAxis::Beside),
        NarrowSplit::Restack => (true, SplitAxis::Below),
    }
}

pub(crate) fn floating_content_inset(
    layout: ShellLayout,
    tokens: ShellTokens,
    chrome_present: bool,
) -> [f32; 4] {
    let rtl = tokens.is_rtl();
    match layout {
        ShellLayout::FloatingTitlebar | ShellLayout::FloatingTitlebarSidebar => [
            tokens.titlebar_height + tokens.effective_edge_inset() + tokens.effective_pane_gutter(),
            0.0,
            0.0,
            0.0,
        ],
        ShellLayout::FloatingSidebar if chrome_present => {
            let sidebar_extent = tokens.sidebar_width
                + tokens.effective_edge_inset()
                + tokens.effective_pane_gutter();
            if rtl {
                [0.0, sidebar_extent, 0.0, 0.0]
            } else {
                [0.0, 0.0, 0.0, sidebar_extent]
            }
        }
        _ => [0.0; 4],
    }
}

pub(crate) type Slot<'a, Message> = Box<dyn Fn() -> Element<'a, Message> + 'a>;

pub(crate) type GeometrySlotFn<'a, Message> =
    Box<dyn Fn(&ShellGeometry) -> Element<'a, Message> + 'a>;

pub(crate) enum ShellSlot<'a, Message> {
    Blind(Slot<'a, Message>),
    GeometryAware(GeometrySlotFn<'a, Message>),
}

impl<'a, Message> ShellSlot<'a, Message> {
    pub(crate) fn blind(build: impl Fn() -> Element<'a, Message> + 'a) -> Self {
        Self::Blind(Box::new(build))
    }

    pub(crate) fn geometry_aware(
        build: impl Fn(&ShellGeometry) -> Element<'a, Message> + 'a,
    ) -> Self {
        Self::GeometryAware(Box::new(build))
    }

    // Use the enclosing subtree's resolution; inline motion and drawer geometry can differ.
    pub(crate) fn build(&self, geometry: &ShellGeometry) -> Element<'a, Message> {
        match self {
            Self::Blind(build) => build(),
            Self::GeometryAware(build) => build(geometry),
        }
    }
}

/// The activity slot's resolved site, independent of whether it returns a surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityPlacement {
    /// No activity slot was supplied.
    ///
    /// A supplied slot returning `None` still resolves a site and retains an empty
    /// bottom wrapper to preserve the hosted pane's tree identity.
    None,
    /// Bottom of an inline sidebar or an open overlay drawer.
    SidebarBottom,
    /// Floating above the content's bottom leading corner when the sidebar is
    /// unavailable: left in LTR, right in RTL.
    ContentBottomLeft,
}

/// Maximum fraction of the inset content width available to floating activity.
///
/// The surface otherwise uses the sidebar width.
pub const FLOATING_ACTIVITY_CONTENT_FRACTION: f32 = 0.75;

pub(crate) enum ContentSlot<'a, Message> {
    Unsized(Slot<'a, Message>),
    WidthAware(Box<dyn Fn(f32) -> Element<'a, Message> + 'a>),
    GeometryAware(GeometrySlotFn<'a, Message>),
}

impl<'a, Message> ContentSlot<'a, Message> {
    pub(crate) fn build(&self, geometry: &ShellGeometry) -> Element<'a, Message> {
        match self {
            Self::Unsized(content) => content(),
            Self::WidthAware(content) => content(geometry.content_width),
            Self::GeometryAware(content) => content(geometry),
        }
    }
}

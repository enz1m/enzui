//! EnzPagination: numbered pagination with prev/next and ellipsis collapse.
//!
//! Shows page 1 and page N unconditionally, plus a "window" of
//! `sibling_count` pages on either side of the current page. The gap
//! between the first/last page and the visible window collapses to an
//! ellipsis when it spans more than one page.
//!
//! # Interaction model
//!
//! The view is built in O(visible cells): the two ends, the two ellipses,
//! and the bounded interior window; never a full `1..=N` sweep, so a
//! million-page pager renders the same handful of cells.
//!
//! The caller supplies a lazy message-factory closure mapping a 1-based
//! target page to a `Message` (`enz_button_with`). It is invoked once per
//! real activation, never at view-build time. Actionable cells (windowed
//! pages, Prev when not on page 1, Next when not on the last page, and the
//! current page, which re-selects as a no-op by construction) bind the
//! factory. Only truly inert cells (globally-disabled cells, and boundary
//! Prev/Next) are message-less and never invoke it. The current page is
//! selected, not disabled: it renders as an enabled, filled `Primary`
//! cell so a screen reader announces the selected state instead of a
//! silent disabled cell.
//!
//! # Compact
//!
//! The default measures the numbered strip against its allocated width. When
//! it cannot fit, navigation becomes icon-only before numbered pages are
//! reduced to the current page. If that also cannot fit, only the two
//! navigation controls remain.
//!
//! `.compact(true)` collapses to icon-only Prev/Next chrome (chevrons) with
//! no page cells, retaining the text labels as the accessible names.
//!
//! # Focus continuity
//!
//! The cell row is a [`KeyedRow`](crate::keyed_row::KeyedRow), not a plain
//! `Row`. Each cell is keyed by its stable page identity
//! (`derive_id(&id_prefix, &format!("page-{n}"))` for page cells, `"prev"`
//! / `"next"` / `"ellipsis-0"` / `"ellipsis-1"` for chrome). When the user
//! pages and the window shifts (e.g. page 5 → 6) `KeyedRow::diff` moves
//! retained child widget state (focus, pressed, hover) to follow the key,
//! not the positional slot. Without keyed reconciliation, focus would
//! transfer to whatever cell occupies the same index after the shift.
//!
//! # Accessibility
//!
//! The whole pager is wrapped in a labelled `SemanticsContainer` group
//! (`Role::Group`) so an AT sees a named region parenting the cell nodes.
//! The group node names the region ("Pagination" by default, overridable
//! via [`EnzPagination::group_label`]); it does not restate the page
//! numbers, which would double-announce every cell. List / ListItem /
//! navigation roles belong to the semantics layer and are not invented
//! here.
//!
//! Each page cell additionally carries a `set_position` ordinal so an AT
//! announces "page 5 of 12" instead of a bare "5, button". The ordinal
//! is the domain page number out of `page_count`, not the rendered-cell
//! index, which would shift as the window scrolls even though the page
//! did not. Prev/Next and ellipsis cells are not pages and carry no page
//! ordinal.

use crate::button::{ButtonIconPosition, ButtonKind, enz_button_with, enz_inert_button};
use crate::enz_text::{TextTone, enz_text};
use crate::keyed_row::keyed_row;
use crate::semantics_container::SemanticsContainer;
use enzui_core::identity::derive_id;
use enzui_core::semantics::{Role, Semantics};
use iced::{Alignment, Element, Length};

/// Default siblings shown either side of the current page.
pub const DEFAULT_SIBLING_COUNT: i32 = 1;

/// Icon name for the Previous control.
pub const PREV_ICON: &str = "navigation.back";
/// Icon name for the Next control.
pub const NEXT_ICON: &str = "navigation.forward";

// Pure window/ellipsis math, reused in tests.

/// Clamp a caller-supplied sibling count to a sane, overflow-safe range.
///
/// Rejects negatives (→ 0) and caps at a large-but-finite ceiling so the
/// downstream `current ± sibling` arithmetic can never wrap `i32`.
pub fn sanitize_sibling_count(count: i32) -> i32 {
    count.clamp(0, 1_000_000)
}

/// The inclusive interior window bounds (1-based) around the current page.
/// First and last page always show; the returned range covers only interior
/// pages (clamped to `[2, page_count - 1]`). Overflow-safe: uses saturating
/// arithmetic so extreme signed inputs cannot wrap.
pub fn window_bounds(current_page: i32, page_count: i32, sibling_count: i32) -> (i32, i32) {
    let sc = sanitize_sibling_count(sibling_count);
    let lo = current_page.saturating_sub(sc).max(2);
    let hi = current_page
        .saturating_add(sc)
        .min(page_count.saturating_sub(1));
    (lo, hi)
}

/// Is there a >1-page gap between page 1 and the window's leading edge?
pub fn show_leading_ellipsis(window_start: i32) -> bool {
    window_start > 2
}

/// Is there a >1-page gap between the window's trailing edge and page N?
///
/// Overflow-safe: `page_count - 1` is computed with saturating arithmetic so
/// the extreme public inputs `page_count == i32::MIN` (and `window_end ==
/// i32::MIN`) cannot wrap or panic in a debug build.
pub fn show_trailing_ellipsis(window_end: i32, page_count: i32) -> bool {
    window_end < page_count.saturating_sub(1)
}

/// One rendered cell in the pager sequence. Built in O(visible) so huge
/// page counts never materialise every page. Exposed for testing the
/// rendered sequence without constructing the iced tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageCell {
    /// The Previous control.
    Prev,
    /// A concrete page number (1-based).
    Page(i32),
    /// A collapsed-gap ellipsis.
    Ellipsis,
    /// The Next control.
    Next,
}

/// Compute the ordered cell sequence for the pager in O(visible) time:
/// two ends + two ellipses + bounded interior window, never `1..=N`.
pub fn cell_sequence(
    current_page: i32,
    page_count: i32,
    sibling_count: i32,
    compact: bool,
) -> Vec<PageCell> {
    let mut cells = Vec::new();
    cells.push(PageCell::Prev);

    if !compact && page_count >= 1 {
        let (win_start, win_end) = window_bounds(current_page, page_count, sibling_count);

        // Page 1 always shows.
        cells.push(PageCell::Page(1));

        if page_count >= 2 {
            if show_leading_ellipsis(win_start) {
                cells.push(PageCell::Ellipsis);
            }
            // Bounded interior window.
            if win_start <= win_end {
                for n in win_start..=win_end {
                    cells.push(PageCell::Page(n));
                }
            }
            if show_trailing_ellipsis(win_end, page_count) {
                cells.push(PageCell::Ellipsis);
            }
            // Page N always shows (distinct from page 1).
            cells.push(PageCell::Page(page_count));
        }
    }

    cells.push(PageCell::Next);
    cells
}

/// A themed 1-based pagination control.
#[must_use]
pub struct EnzPagination<'a, Message>
where
    Message: Clone + 'a,
{
    page_count: i32,
    current_page: i32,
    sibling_count: i32,
    compact: bool,
    enabled: bool,
    on_select: Box<dyn Fn(i32) -> Message + 'a>,
    previous_label: String,
    next_label: String,
    /// Accessible name published on the labelled group node that wraps the
    /// pager (default `"Pagination"`). Names the region; it never restates
    /// the page numbers, so it cannot double-announce a cell.
    group_label: String,
    /// Namespace for the stable page-keyed widget ids. Defaults to
    /// `"enz-pagination"`; set a distinct prefix when several paginations
    /// share a screen so their per-page ids do not collide. Page cells get
    /// `{prefix}-page-{n}`, Prev/Next get `{prefix}-prev` / `{prefix}-next`,
    /// the current cell gets `{prefix}-page-{n}` too, and the group node gets
    /// `{prefix}-group`, so focus and semantic-action targets stay bound to
    /// the logical page across page-count / sibling changes and two co-located
    /// pagers with distinct prefixes never collide.
    id_prefix: String,
    /// Explicit spacing-scale override for the layout-time cell
    /// `Row::spacing`. `Widget::layout` has no `&Theme` parameter, so the
    /// density that
    /// decides the inter-cell gap must be forwarded at build time. `None`
    /// resolves the stock ladder (`xs()` = 4 px). When `Some`, the gap
    /// resolves `xs()` against the forwarded ladder instead.
    spacing_scale: Option<enzui_theme::SpacingScale>,
}

impl<'a, Message> EnzPagination<'a, Message>
where
    Message: Clone + 'a,
{
    /// Set siblings either side of current (default `1`). Negative values
    /// are clamped to 0; extreme values are capped (overflow-safe).
    pub fn sibling_count(mut self, count: i32) -> Self {
        self.sibling_count = sanitize_sibling_count(count);
        self
    }

    /// Enable compact (icons-only Prev/Next) mode.
    pub fn compact(mut self, compact: bool) -> Self {
        self.compact = compact;
        self
    }

    /// Enable / disable the entire control.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Override the Previous button label / accessible name (default `"Previous"`).
    pub fn previous_label<S: Into<String>>(mut self, label: S) -> Self {
        self.previous_label = label.into();
        self
    }

    /// Override the Next button label / accessible name (default `"Next"`).
    pub fn next_label<S: Into<String>>(mut self, label: S) -> Self {
        self.next_label = label.into();
        self
    }

    /// Namespace for the stable page-keyed widget ids. Set a distinct
    /// prefix when several paginations share a screen so their per-cell ids
    /// and group id never collide.
    pub fn id_prefix<S: Into<String>>(mut self, prefix: S) -> Self {
        self.id_prefix = prefix.into();
        self
    }

    /// Override the accessible name of the labelled group node that wraps the
    /// pager (default `"Pagination"`). The group names the region; it does not
    /// restate page numbers, so overriding it cannot introduce a duplicate
    /// announcement.
    pub fn group_label<S: Into<String>>(mut self, label: S) -> Self {
        self.group_label = label.into();
        self
    }

    /// Forward a custom [`SpacingScale`](enzui_theme::SpacingScale) so the
    /// inter-cell gap (`Row::spacing`) resolves against it instead of the stock
    /// ladder.
    ///
    /// `Widget::layout` has no `&Theme` parameter — the cell gap is consumed in
    /// `Row::layout`, which iced never hands a theme — so a custom density must
    /// be threaded in explicitly at build time. Without this setter the pager
    /// uses the compile-time default (`xs()` = 4 px). Pass `theme.spacing` from
    /// the app's `EnzTheme` to make the gap follow a custom density.
    pub fn spacing_scale(mut self, spacing: enzui_theme::SpacingScale) -> Self {
        self.spacing_scale = Some(spacing);
        self
    }

    /// Convert into an iced [`Element`].
    pub fn into_element(self) -> Element<'a, Message> {
        let Self {
            page_count,
            current_page,
            sibling_count,
            compact,
            enabled,
            on_select,
            previous_label,
            next_label,
            group_label,
            id_prefix,
            spacing_scale,
        } = self;

        let on_select: std::rc::Rc<dyn Fn(i32) -> Message + 'a> = std::rc::Rc::from(on_select);

        let numbered = if compact {
            Vec::new()
        } else {
            cell_sequence(current_page, page_count, sibling_count, false)
        };
        let current = [PageCell::Prev, PageCell::Page(current_page), PageCell::Next];
        let navigation = [PageCell::Prev, PageCell::Next];
        let build = |icon_navigation: bool, sequence: &[PageCell]| {
            let prev_enabled = enabled && current_page > 1;
            let next_enabled = enabled && current_page < page_count;

            let spacing = spacing_scale.unwrap_or_default().xs();
            let mut ellipsis_index = 0u32;
            let mut keyed_children: Vec<(String, Element<'a, Message>)> = Vec::new();

            for cell in sequence.iter().copied() {
                match cell {
                    PageCell::Prev => {
                        let key = "prev".to_owned();
                        let icon_pos = if icon_navigation {
                            ButtonIconPosition::Only
                        } else {
                            ButtonIconPosition::Leading
                        };
                        if prev_enabled {
                            let on_select = std::rc::Rc::clone(&on_select);
                            let target = current_page.saturating_sub(1).max(1);
                            keyed_children.push((
                                key,
                                enz_button_with(previous_label.clone(), move || on_select(target))
                                    .kind(ButtonKind::Quiet)
                                    .icon(PREV_ICON)
                                    .icon_position(icon_pos)
                                    .aria_label(previous_label.clone())
                                    .id(derive_id(&id_prefix, "prev"))
                                    .into(),
                            ));
                        } else {
                            keyed_children.push((
                                key,
                                enz_inert_button(previous_label.clone(), ButtonKind::Quiet)
                                    .icon(PREV_ICON)
                                    .icon_position(icon_pos)
                                    .aria_label(previous_label.clone())
                                    .id(derive_id(&id_prefix, "prev"))
                                    .into(),
                            ));
                        }
                    }
                    PageCell::Next => {
                        let key = "next".to_owned();
                        let icon_pos = if icon_navigation {
                            ButtonIconPosition::Only
                        } else {
                            ButtonIconPosition::Trailing
                        };
                        if next_enabled {
                            let on_select = std::rc::Rc::clone(&on_select);
                            let target = (current_page + 1).min(page_count);
                            keyed_children.push((
                                key,
                                enz_button_with(next_label.clone(), move || on_select(target))
                                    .kind(ButtonKind::Quiet)
                                    .icon(NEXT_ICON)
                                    .icon_position(icon_pos)
                                    .aria_label(next_label.clone())
                                    .id(derive_id(&id_prefix, "next"))
                                    .into(),
                            ));
                        } else {
                            keyed_children.push((
                                key,
                                enz_inert_button(next_label.clone(), ButtonKind::Quiet)
                                    .icon(NEXT_ICON)
                                    .icon_position(icon_pos)
                                    .aria_label(next_label.clone())
                                    .id(derive_id(&id_prefix, "next"))
                                    .into(),
                            ));
                        }
                    }
                    PageCell::Ellipsis => {
                        let key = format!("ellipsis-{ellipsis_index}");
                        ellipsis_index += 1;
                        keyed_children.push((key, enz_text("…").tone(TextTone::Muted).into()));
                    }
                    PageCell::Page(n) => {
                        let key = format!("page-{n}");
                        let is_current = n == current_page;
                        // Ordinals describe document pages, not the shifting visible-cell index.
                        let page_position = enzui_core::semantics::SetPosition {
                            index: n.max(1) as usize,
                            count: page_count.max(1) as usize,
                        };
                        if !enabled {
                            keyed_children.push((
                                key,
                                enz_inert_button(n.to_string(), ButtonKind::Quiet)
                                    .aria_label(
                                        enzui_core::localized!("enzui-pagination-page", "page" => n),
                                    )
                                    .selected(is_current)
                                    .set_position(page_position)
                                    .id(derive_id(&id_prefix, &format!("page-{n}")))
                                    .into(),
                            ));
                        } else if is_current {
                            let on_select = std::rc::Rc::clone(&on_select);
                            keyed_children.push((
                                key,
                                enz_button_with(n.to_string(), move || on_select(n))
                                    .kind(ButtonKind::Primary)
                                    .aria_label(
                                        enzui_core::localized!("enzui-pagination-page", "page" => n),
                                    )
                                    .selected(true)
                                    .set_position(page_position)
                                    .id(derive_id(&id_prefix, &format!("page-{n}")))
                                    .into(),
                            ));
                        } else {
                            let on_select = std::rc::Rc::clone(&on_select);
                            keyed_children.push((
                                key,
                                enz_button_with(n.to_string(), move || on_select(n))
                                    .kind(ButtonKind::Quiet)
                                    .aria_label(
                                        enzui_core::localized!("enzui-pagination-page", "page" => n),
                                    )
                                    .selected(false)
                                    .set_position(page_position)
                                    .id(derive_id(&id_prefix, &format!("page-{n}")))
                                    .into(),
                            ));
                        }
                    }
                }
            }

            let laid_out = keyed_row(keyed_children)
                .spacing(spacing)
                .align_y(Alignment::Center)
                .width(Length::Shrink);

            // AccessKit resolves set size from the ancestor group.
            let group_semantics = Semantics::new(Role::Group)
                .label(group_label.clone())
                .disabled(!enabled)
                .set_position(enzui_core::semantics::SetPosition {
                    index: 1,
                    count: page_count.max(1) as usize,
                });

            SemanticsContainer::new(laid_out, group_semantics)
                .id(derive_id(&id_prefix, "group"))
                .into()
        };
        if compact {
            return build(true, &navigation);
        }
        let narrow = crate::responsive_switch::ResponsiveSwitch::new(
            build(true, &current),
            build(true, &navigation),
            build(true, &current),
            Length::Shrink,
            true,
            |width| width,
        )
        .into();
        let icon_navigation = crate::responsive_switch::ResponsiveSwitch::new(
            build(true, &numbered),
            narrow,
            build(true, &numbered),
            Length::Shrink,
            true,
            |width| width,
        )
        .into();
        crate::responsive_switch::ResponsiveSwitch::new(
            build(false, &numbered),
            icon_navigation,
            build(false, &numbered),
            Length::Shrink,
            true,
            |width| width,
        )
        .into()
    }
}

impl<'a, Message> From<EnzPagination<'a, Message>> for Element<'a, Message>
where
    Message: Clone + 'a,
{
    fn from(p: EnzPagination<'a, Message>) -> Self {
        p.into_element()
    }
}

/// Construct a new pagination control.
///
/// - `page_count` -- total number of pages (clamped to >= 1).
/// - `current_page` -- 1-based current page (clamped into range).
/// - `on_select` -- a lazy message factory mapping a 1-based target page
///   to a `Message`. It is stored and invoked once per real activation
///   (pointer / Enter / Space / routed AT action) of an actionable cell:
///   Prev when not on page 1, Next when not on the last page, each
///   windowed page, and the current page (re-selecting it, a no-op by
///   construction). It is never called at view-build time.
///   Globally-disabled cells (`.enabled(false)`) and inert boundary
///   Prev/Next construct no message and never invoke the factory.
pub fn enz_pagination<'a, Message, F>(
    page_count: i32,
    current_page: i32,
    on_select: F,
) -> EnzPagination<'a, Message>
where
    Message: Clone + 'a,
    F: Fn(i32) -> Message + 'a,
{
    let pc = page_count.max(1);
    EnzPagination {
        page_count: pc,
        current_page: current_page.clamp(1, pc),
        sibling_count: DEFAULT_SIBLING_COUNT,
        compact: false,
        enabled: true,
        on_select: Box::new(on_select),
        previous_label: enzui_core::localization::text("enzui-pagination-previous"),
        next_label: enzui_core::localization::text("enzui-pagination-next"),
        group_label: enzui_core::localization::text("enzui-pagination-pagination"),
        id_prefix: "enz-pagination".into(),
        spacing_scale: None,
    }
}

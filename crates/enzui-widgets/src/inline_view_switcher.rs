//! Choose a page with joined segmented tabs and roving keyboard focus.
//!
//! [`EnzInlineViewSwitcher`] and [`InlineSwitcherPage`] alias the shared
//! [`crate::view_switcher`] engine and page model. This constructor selects
//! [`SwitcherPolicy::INLINE_SEGMENTED`]: frameless joined cells, regular labels,
//! no width floors or compact icon fallback, and out-of-range selection left empty.
//! Override selection bounds, tokens or policy through the shared builder.
//!
//! The host stores selection and renders the selected page’s content. Keyboard
//! navigation follows focus, skips disabled pages and publishes selection through
//! the callback. The strip publishes tab-list semantics with selected/disabled tab
//! states; disabled selected tabs retain dimmed selection paint.
//!
//! Stable IDs can be used through [`EnzInlineViewSwitcher::selected_id`] and
//! [`EnzInlineViewSwitcher::on_select_id`]. Pages support icons and badge text.
//! Enable scrolling with [`EnzInlineViewSwitcher::scrollable`] when tabs overflow.
//! For a strip with no callback, use [`enz_inline_view_switcher_unwired`], which
//! renders and announces disabled controls.

use crate::view_switcher::{
    EnzViewSwitcher, Selector, SwitcherPage, SwitcherPolicy, switcher_with,
};

/// A segmented switcher page using the shared [`SwitcherPage`] model.
pub type InlineSwitcherPage = SwitcherPage;

/// A shared switcher builder configured for segmented presentation.
pub type EnzInlineViewSwitcher<'a, Message> = EnzViewSwitcher<'a, Message>;

/// Build joined tabs that publish a positional selection index.
///
/// Out-of-range selection leaves all tabs unselected by default. Override this
/// through [`EnzViewSwitcher::selection_bounds`] when clamping is desired.
pub fn enz_inline_view_switcher<'a, Message: Clone + 'a, F>(
    pages: Vec<InlineSwitcherPage>,
    selected_index: usize,
    on_select: F,
) -> EnzInlineViewSwitcher<'a, Message>
where
    F: Fn(usize) -> Message + 'a,
{
    switcher_with(
        pages,
        selected_index,
        Selector::Index(Box::new(on_select)),
        SwitcherPolicy::INLINE_SEGMENTED,
    )
}

/// Build a disabled segmented strip without a selection callback.
pub fn enz_inline_view_switcher_unwired<'a, Message: Clone + 'a>(
    pages: Vec<InlineSwitcherPage>,
    selected_index: usize,
) -> EnzInlineViewSwitcher<'a, Message> {
    switcher_with(
        pages,
        selected_index,
        Selector::None,
        SwitcherPolicy::INLINE_SEGMENTED,
    )
}

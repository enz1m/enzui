//! Owning tab-list implementation for [`super::EnzTabPill`].
//!
//! Public list types and constructors are explicitly re-exported by
//! [`crate::tab_pill`], preserving their established paths while keeping the
//! single-pill builder and shared token/indicator seams in the parent module.

mod seam;
mod widget_impls;

use super::{
    CellInk, IndicatorCells, IndicatorLayer, PillSpec, SelectionInkHandle, TabPillGeometry,
    TabPillTokens, build_pill, cell_hover_background, resolve_tokens_with, segmented_cell_bg,
    selection_ink_alpha, selection_ink_handle, strip_ink_alpha,
};
use enzui_core::collect::SemanticsNode;
use enzui_core::focus::{
    FocusState, Nav, PressFocus, navigation_from_key_in, press_focus_from_cursor,
    release_focus_if_inert,
};
use enzui_core::semantics::{Role, Semantics};
use iced::advanced::layout::Measure as _;
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::font::Weight;
use iced::{Alignment, Color, Element, Event, Length, Rectangle, Size, Vector};

/// Nominal readable tab width used by [`EnzTabList::scroll_step`].
pub(super) fn identifiability_floor(geo: TabPillTokens) -> f32 {
    // Mean glyph advance for a proportional UI sans at `label_size`.
    let ch = 0.5 * geo.label_size;
    // `…` is roughly one full em — wider than a mean advance.
    let ellipsis = geo.label_size;
    // Three glyphs is the smallest run that distinguishes typical tab labels
    // ("Ove…" vs "Det…" vs "His…").
    let label_budget = 3.0 * ch + ellipsis;
    let chrome = geo.icon_size + geo.icon_gap + 2.0 * geo.pill_padding_x;
    let floor = label_budget + chrome;
    // Defensive: tokens are normalised upstream, but a hand-built
    // `TabPillTokens` reaching a non-finite value here would poison layout.
    if floor.is_finite() && floor > 0.0 {
        floor
    } else {
        0.0
    }
}

/// Narrow target floor; labels also reserve a distinguishing prefix.
const NARROW_TAB_MIN_TARGET: f32 = 24.0;

fn narrow_cell_floor(
    item: &TabPillItem,
    geo: TabPillTokens,
    fill: bool,
    label_layout: TabLabelLayout,
    weight: Weight,
) -> f32 {
    let typography = crate::text_measure::MeasureTypography::new(crate::enz_text::TextSize::Body)
        .pin_size_px(geo.label_size)
        .weight(weight);
    let full_label = crate::text_measure::enz_text_measure_width(&item.label, typography);
    let label_budget = (2.5 * geo.label_size).min(full_label);
    let body = if item.icon.is_empty() {
        label_budget
    } else {
        match label_layout {
            TabLabelLayout::Inline if item.label.is_empty() => geo.icon_size,
            TabLabelLayout::Inline => geo.icon_size + geo.icon_gap + label_budget,
            TabLabelLayout::Stacked => geo.icon_size.max(label_budget),
        }
    };
    let padding = if fill { 0.0 } else { 2.0 * geo.pill_padding_x };
    (body
        + busy_width(item.busy, geo.icon_size, geo.icon_gap)
        + super::badge_width(&item.badge, geo.icon_gap)
        + padding)
        .max(NARROW_TAB_MIN_TARGET)
}

/// The room a busy tab's spinner takes after its label.
pub(crate) fn busy_width(busy: bool, icon_size: f32, icon_gap: f32) -> f32 {
    if busy { icon_gap + icon_size } else { 0.0 }
}

/// A cell's natural width: its full label with the same chrome
/// [`narrow_cell_floor`] adds. Measured with the floor's weight, so a
/// semibold selection never makes the estimate too small.
fn natural_cell_width(
    item: &TabPillItem,
    geo: TabPillTokens,
    fill: bool,
    label_layout: TabLabelLayout,
    weight: Weight,
) -> f32 {
    let typography = crate::text_measure::MeasureTypography::new(crate::enz_text::TextSize::Body)
        .pin_size_px(geo.label_size)
        .weight(weight);
    let label = crate::text_measure::enz_text_measure_width(&item.label, typography);
    let body = if item.icon.is_empty() {
        label
    } else {
        match label_layout {
            TabLabelLayout::Inline if item.label.is_empty() => geo.icon_size,
            TabLabelLayout::Inline => geo.icon_size + geo.icon_gap + label,
            TabLabelLayout::Stacked => geo.icon_size.max(label),
        }
    };
    let padding = if fill { 0.0 } else { 2.0 * geo.pill_padding_x };
    (body
        + busy_width(item.busy, geo.icon_size, geo.icon_gap)
        + super::badge_width(&item.badge, geo.icon_gap)
        + padding)
        .max(NARROW_TAB_MIN_TARGET)
}

// ---------------------------------------------------------------------------
// EnzTabList — the roving-focus tablist owner
// ---------------------------------------------------------------------------

/// One entry in an [`EnzTabList`].
#[derive(Debug, Clone)]
#[must_use]
pub struct TabPillItem {
    label: String,
    icon: String,
    enabled: bool,
    /// Stable logical key for this tab's routable identity.
    ///
    /// `None` falls back to the label in [`Self::identity_key`]. A caller
    /// with a better key than the label supplies it: a route key already
    /// stable across reorders, not moving when the title retranslates or
    /// gains a badge.
    key: Option<String>,
    /// The spoken accessible name, when it must differ from the visible
    /// [`Self::label`].
    ///
    /// `None` falls back to the label. Set for an icon-only tab: an empty
    /// label is also an empty accessible name — an anonymous tab AT cannot
    /// announce, and the identity invariant strips its `Click`.
    /// `.aria_label("Contents")` keeps the display icon-only while publishing
    /// the word and giving the tab a stable identity for its id.
    aria_label: Option<String>,
    /// Explicit hover-tooltip override.
    ///
    /// `None` derives the tooltip (see `resolved_tooltip`); `Some`
    /// forces this exact text regardless of whether the tab is icon-only or
    /// labelled. An empty string is "no tooltip".
    tooltip: Option<String>,
    tooltip_delay: Option<std::time::Duration>,
    /// The panel this tab shows, published as the tab's `controls`
    /// relation.
    panel: Option<iced::advanced::widget::Id>,
    /// A spinner after the label while the page works on something.
    busy: bool,
    /// A count after the label, drawn as a small badge.
    badge: String,
}

impl TabPillItem {
    /// A tab with a label.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            icon: String::new(),
            enabled: true,
            key: None,
            aria_label: None,
            tooltip: None,
            tooltip_delay: None,
            panel: None,
            busy: false,
            badge: String::new(),
        }
    }

    /// Show `badge` (a count, usually) after the label as a small badge,
    /// and announce it after the name. Empty shows none.
    pub fn badge(mut self, badge: impl Into<String>) -> Self {
        self.badge = badge.into();
        self
    }

    /// Name the panel this tab shows ([`super::enz_tab_panel`]), so a screen
    /// reader can move from the tab to its content.
    pub fn panel(mut self, id: iced::advanced::widget::Id) -> Self {
        self.panel = Some(id);
        self
    }

    /// Override the spoken accessible name, decoupling it from the visible
    /// [`label`](Self::new).
    ///
    /// The one way to build an announced icon-only tab: empty label +
    /// [`icon`](Self::icon) renders icon-only, the full word here publishes
    /// on the `Role::Tab` node and keeps the tab non-anonymous (its `Click`
    /// and derived id survive). Falls back to the tab's identity key when
    /// [`key`](Self::key) is unset; ignored for the name only when `None`.
    pub fn aria_label(mut self, name: impl Into<String>) -> Self {
        self.aria_label = Some(name.into());
        self
    }

    /// Override the app's hover delay for this control; zero starts revealing immediately.
    pub fn tooltip_delay(mut self, delay: std::time::Duration) -> Self {
        self.tooltip_delay = Some(delay);
        self
    }

    /// Override the hover tooltip text.
    ///
    /// By default the tooltip is *derived*: an icon-only tab with an
    /// accessible name shows it on hover, so a sighted user keeps parity
    /// with what AT speaks. A labelled tab derives *nothing* — its word is
    /// on screen, and a tooltip on every labelled tab is the avoided
    /// regression. The override wins both ways: more-than-the-word tips on
    /// labelled tabs, replacement names on icon-only ones. Empty override
    /// means "no tooltip" (also the labelled-tab suppressor).
    pub fn tooltip(mut self, tip: impl Into<String>) -> Self {
        self.tooltip = Some(tip.into());
        self
    }
    /// Add a leading icon.
    pub fn icon(mut self, name: impl Into<String>) -> Self {
        self.icon = name.into();
        self
    }
    /// Show a small spinner after the label, and report the tab busy to
    /// assistive technology, while its page works on something. The tab
    /// measures it, so the strip leaves room for it.
    pub fn busy(mut self, busy: bool) -> Self {
        self.busy = busy;
        self
    }

    /// Disable this tab (skipped by roving navigation, not clickable).
    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    /// Supply the stable logical key this tab's `Id`
    /// is derived from.
    ///
    /// Prefer over the label default whenever the caller owns a real route
    /// key. The label is presentation: it moves with copy, translation, and
    /// badge counts — a label key would silently re-identify a tab every
    /// badge tick, and an AT request against the previous frame would stop
    /// resolving.
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }

    /// The logical key this tab derives identity from: the explicit
    /// [`Self::key`], else the [`aria_label`](Self::aria_label), else the
    /// visible label.
    ///
    /// `aria_label` precedes the label so an icon-only `.aria_label` tab is
    /// *not* anonymous: stable id, kept `Click`. Empty only when all three
    /// are — a truly nameless tab stays anonymous, since a positional id
    /// would be worse than none.
    fn identity_key(&self) -> &str {
        self.key
            .as_deref()
            .or(self.aria_label.as_deref())
            .unwrap_or(&self.label)
    }

    /// The name a screen reader announces for this tab: the explicit
    /// [`aria_label`](Self::aria_label), else the visible label.
    ///
    /// Decoupled from the visible label so an icon-only tab (empty label,
    /// icon-only display) can still be announced by its full word.
    pub(super) fn announced_name(&self) -> String {
        let name = self.aria_label.as_deref().unwrap_or(&self.label);
        if self.badge.is_empty() {
            name.to_owned()
        } else {
            format!("{name} {}", self.badge)
        }
    }

    /// The hover-tooltip text this tab shows, or `""` for none.
    ///
    /// Precedence: explicit [`tooltip`](Self::tooltip) always wins;
    /// otherwise an icon-only tab derives its
    /// [`announced_name`](Self::announced_name); otherwise `""` — a labelled
    /// tab's word is already visible. The gate is on `label.is_empty()`, not
    /// on "has an aria_label": `announced_name` falls back to the visible
    /// label, so the latter gate would tip every labelled tab.
    /// A tab whose label can `collapse` tips its name too.
    pub(super) fn resolved_tooltip(&self, collapsed: bool) -> String {
        if let Some(tip) = self.tooltip.as_deref() {
            tip.to_owned()
        } else if self.label.is_empty() || collapsed {
            self.announced_name()
        } else {
            String::new()
        }
    }
}

impl From<&str> for TabPillItem {
    fn from(s: &str) -> Self {
        TabPillItem::new(s)
    }
}
impl From<String> for TabPillItem {
    fn from(s: String) -> Self {
        TabPillItem::new(s)
    }
}

/// How a tab's label weight tracks its selected state.
///
/// The two sources genuinely disagree (pill binds semibold-if-selected;
/// view-switcher segments inherit semibold-always), so this is policy, not
/// tidy-up: matching one silently changes the other's rendered label width,
/// exactly the drift the geometry tests catch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TabLabelWeight {
    /// Semibold when selected, regular otherwise (`EnzTabPill`, the default).
    #[default]
    SelectedSemibold,
    /// Semibold in every state (`EnzViewSwitcher`'s `EnzButton` segments).
    AlwaysSemibold,
    /// Regular in every state (`EnzInlineViewSwitcher`'s segments, whose labels
    /// were plain `text(..)` runs under `EnzButtonGroup`). Selecting a cell must
    /// not change its label's laid-out width, or the whole strip reflows on
    /// every selection change.
    AlwaysRegular,
}

impl TabLabelWeight {
    pub(super) fn font_weight(self, selected: bool) -> Weight {
        let rung = match self {
            TabLabelWeight::AlwaysSemibold => enzui_theme::WeightRung::Semibold,
            TabLabelWeight::AlwaysRegular => enzui_theme::WeightRung::Regular,
            TabLabelWeight::SelectedSemibold if selected => enzui_theme::WeightRung::Semibold,
            TabLabelWeight::SelectedSemibold => enzui_theme::WeightRung::Regular,
        };
        enzui_theme::text_defaults::weight(rung)
    }
}

/// What an out-of-range `selected` index means to an [`EnzTabList`].
///
/// Two callers genuinely disagree — policy, not bug: one parks overflow on
/// the last tab, the other must show no selection (its `active_id()` is
/// `None` there). A widget painting the last cell selected while the API
/// reports "nothing selected" would lie to one of the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TabSelectionBounds {
    /// An index past the end selects the last tab (default).
    #[default]
    Clamp,
    /// An index past the end selects nothing: no cell paints selected and
    /// no `Tab` node reports `selected: true`. Roving navigation still works —
    /// the first arrow key lands on the first (or last) enabled tab.
    Unselected,
}

/// Pill selection or opaque joined segments on a roving-focus tab strip.
///
/// Disabled pills use inert fill; disabled segments retain dimmed selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TabCellFill {
    /// Detached pills on a track; only the selected one is filled (default).
    #[default]
    Pill,
    /// One joined block: every cell opaque, seams between them, outer corners
    /// only.
    Segmented,
}

/// Hover geometry for pill cells without changing their hit rectangles or spacing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TabHoverShape {
    /// Zero-gap pills share a joined hover region; spaced pills remain rounded.
    #[default]
    Joined,
    /// Each unselected cell paints its own rounded hover/press pill.
    /// The wash stays within that cell and is withheld during selection motion.
    Detached,
}

/// How the cells of an [`EnzTabList`] claim horizontal space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TabListWidth {
    /// Each cell hugs its own content (default).
    #[default]
    Hug,
    /// Every cell claims an equal share of the strip width, and the strip
    /// itself fills its parent: the segmented "stretch" presentation.
    ///
    /// Must reach the painted pill, not an intermediate wrapper: wrappers
    /// stretched while buttons stay shrink-width would leave paint,
    /// hit-testing, focus and semantics all failing to fill their cell.
    Fill,
}

/// Placement of an icon relative to its visible label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum TabLabelLayout {
    /// Icon and label share one horizontal line.
    #[default]
    Inline,
    /// Icon is centred above the label (compact-width tab-bar treatment).
    Stacked,
}

/// The four values a caller provides to mount scroll arrows.
///
/// `None` = no arrows, `Some` = flanking arrows. Names are `String`, not
/// `Option<String>`: mandatory — an unnamed icon-only control is a defect
/// (see `ScrollArrows::new`).
struct ScrollArrowConfig<Message> {
    on_prev: Message,
    on_next: Message,
    prev_label: String,
    next_label: String,
}

/// Builder for a horizontal roving-focus tab strip.
///
/// Left/Right select adjacent enabled tabs; Home/End select the first/last.
/// Up/Down are left unhandled, even while the strip holds focus.
#[must_use]
pub struct EnzTabList<'a, Message> {
    items: Vec<TabPillItem>,
    selected: usize,
    /// Where a selection goes. `None` = nowhere: the strip has no reducer
    /// to reach, so it is not a live control at all — see
    /// [`EnzTabList::on_select_maybe`] and [`EnzTabList::live`].
    ///
    /// `Rc` rather than `Box` because every cell captures a clone of it.
    on_select: Option<std::rc::Rc<dyn Fn(usize) -> Message + 'a>>,
    pub(super) tokens: Option<TabPillTokens>,
    /// Geometry-only override; colours keep resolving from the runtime theme.
    geometry: Option<TabPillGeometry>,
    width: TabListWidth,
    label_weight: TabLabelWeight,
    label_layout: TabLabelLayout,
    collapse_labels: bool,
    compress_fill_cells: bool,
    /// Equal tracks only below this width (see `keyed_row`).
    equal_cells_below: Option<f32>,
    cell_min_width: Option<f32>,
    cell_fill: TabCellFill,
    hover_shape: TabHoverShape,
    /// What an out-of-range `selected` index means.
    selection_bounds: TabSelectionBounds,
    enabled: bool,
    id: Option<iced::advanced::widget::Id>,
    /// Namespace the per-cell derived tab ids live under. Defaults to
    /// [`DEFAULT_TAB_ID_PREFIX`].
    tab_id_prefix: String,
    /// Wrap the strip in a horizontal scroll view so an
    /// overflowing row can be reached instead of being clipped away.
    /// Off by default — see [`EnzTabList::scrollable`].
    scrollable: bool,
    /// Height the scroll strip is pinned to when [`Self::scrollable`] is on.
    /// `None` derives it from the pill height so the strip does not gain
    /// spurious vertical space.
    scroll_strip_height: Option<f32>,
    /// Id attached to the inner scrollable, so a caller can drive it with the
    /// stock `scroll_by` / `scroll_to` tasks.
    scroll_id: Option<iced::widget::Id>,
    /// Live scroll geometry, published by the strip's scrollable
    /// and read back at build time to decide which edge fades paint.
    ///
    /// Full geometry, not bare translation: "right edge still clips" needs
    /// viewport and content widths (remaining = content − viewport − offset;
    /// offset 200 ends one strip and halves another). `None` = caller did
    /// not opt into fades. See [`EnzTabList::fade_edges`].
    fade_cell: Option<crate::scroll_view::ScrollGeometryCell>,
    /// Fade width in px; `None` uses the module default.
    fade_width: Option<f32>,
    /// Arrow buttons flanking the strip. `None` means the caller
    /// did not opt in — no arrows are rendered and the strip is unchanged.
    ///
    /// The arrows read the *same* geometry cell the fades read
    /// (`ScrollFadeEdges::from_geometry`), so an arrow and a fade can never
    /// disagree about whether there is content that way.
    scroll_arrow_config: Option<ScrollArrowConfig<Message>>,
    /// Accessible name for the `TabList` group semantics node.
    ///
    /// A landmark-navigating screen-reader user hears this to identify the
    /// group *without* entering it; without it, two tab lists on one page
    /// are indistinguishable. The omission assert ships disarmed so
    /// un-migrated consumers keep working — armed per-process via env or
    /// per-thread, the staged-adoption pattern for opt-in asserts.
    pub(super) aria_label: Option<String>,
    /// Shared handle every cell's style closure reads to know how
    /// much of its own selected-fill to paint. `draw` republishes `0.0`
    /// while the indicator animates (moving thumb is the only selection ink)
    /// and `1.0` at rest. Created here so `build_row` and the widget hold
    /// the *same* `Rc`: the built-time capture is the frame-time write
    /// target.
    selection_ink: SelectionInkHandle,
}

/// Panic message for a missing `aria_label` on a `TabList` group.
///
/// Shared between the `debug_assert!` and the `#[should_panic]` test so the
/// two cannot drift apart.
pub const MISSING_TAB_LIST_LABEL: &str = "enz_tab_list: TabList group has no aria_label — a screen-reader user \
     cannot identify which group of tabs this is. \
     Set .aria_label(\"...\") on the tab list / view switcher.";

/// The environment variable that arms the missing-label assert.
///
/// Any non-empty value enables the process-wide debug assertion. Tests may use
/// [`with_tab_list_label_assert_armed`] for a thread-local scope.
pub const TAB_LIST_LABEL_ASSERT_ENV: &str = "ENZUI_ASSERT_TAB_LIST_LABEL";

thread_local! {
    /// Per-thread arming for [`with_tab_list_label_assert_armed`].
    static TAB_LIST_LABEL_ASSERT_ARMED: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

/// Restores the previous arming on drop, so nested scopes compose and a panic
/// (which is exactly what an armed assert produces) cannot leave the flag set.
struct TabListLabelAssertGuard(bool);

impl Drop for TabListLabelAssertGuard {
    fn drop(&mut self) {
        TAB_LIST_LABEL_ASSERT_ARMED.with(|c| c.set(self.0));
    }
}

/// Arm the missing-`aria_label` assert for the duration of `f`, on this
/// thread only.
///
/// # Why thread-scoped
///
/// Integration tests share one binary running concurrently per thread; a
/// process-wide arm would panic whichever unrelated test renders at that
/// instant. The tree builds synchronously on the calling thread, so the arm
/// covers exactly the render the test performs.
pub fn with_tab_list_label_assert_armed<R>(f: impl FnOnce() -> R) -> R {
    let previous = TAB_LIST_LABEL_ASSERT_ARMED.with(|c| c.replace(true));
    let _guard = TabListLabelAssertGuard(previous);
    f()
}

/// Is the missing-label assert armed right now?
///
/// Armed either by [`TAB_LIST_LABEL_ASSERT_ENV`] for the whole process, or
/// on this thread only by [`with_tab_list_label_assert_armed`].
#[must_use]
pub fn tab_list_label_assert_armed() -> bool {
    use std::sync::OnceLock;
    static FROM_ENV: OnceLock<bool> = OnceLock::new();
    let from_env = *FROM_ENV
        .get_or_init(|| std::env::var_os(TAB_LIST_LABEL_ASSERT_ENV).is_some_and(|v| !v.is_empty()));
    from_env || TAB_LIST_LABEL_ASSERT_ARMED.with(std::cell::Cell::get)
}

/// The namespace per-tab ids are derived under when no
/// [`EnzTabList::tab_id_prefix`] is supplied. The key is each tab's
/// [`TabPillItem::key`], falling back to its label.
pub const DEFAULT_TAB_ID_PREFIX: &str = "enz-tab";

/// Construct a roving-focus tab strip from labels/items, the selected index,
/// and a selection callback. Selection follows focus: each keyboard move
/// publishes `on_select(index)`.
pub fn enz_tab_list<'a, Message, I, T>(
    items: I,
    selected: usize,
    on_select: impl Fn(usize) -> Message + 'a,
) -> EnzTabList<'a, Message>
where
    Message: Clone + 'a,
    I: IntoIterator<Item = T>,
    T: Into<TabPillItem>,
{
    enz_tab_list_maybe(items, selected, Some(Box::new(on_select)))
}

/// Construct a roving-focus tab strip whose selection callback may be
/// absent — the `Option`-accepting door onto [`enz_tab_list`].
///
/// `None` renders and announces the strip disabled (see
/// [`EnzTabList::on_select_maybe`]): a normal-looking-but-inert strip is the
/// refused answer. Composing presets use this door so an unwired dialog
/// degrades instead of panicking.
pub fn enz_tab_list_maybe<'a, Message, I, T>(
    items: I,
    selected: usize,
    on_select: Option<Box<dyn Fn(usize) -> Message + 'a>>,
) -> EnzTabList<'a, Message>
where
    Message: Clone + 'a,
    I: IntoIterator<Item = T>,
    T: Into<TabPillItem>,
{
    EnzTabList {
        items: items.into_iter().map(Into::into).collect(),
        selected,
        on_select: on_select.map(std::rc::Rc::from),
        tokens: None,
        geometry: None,
        width: TabListWidth::default(),
        label_weight: TabLabelWeight::default(),
        label_layout: TabLabelLayout::default(),
        collapse_labels: false,
        compress_fill_cells: false,
        equal_cells_below: None,
        cell_min_width: None,
        cell_fill: TabCellFill::default(),
        hover_shape: TabHoverShape::default(),
        selection_bounds: TabSelectionBounds::default(),
        enabled: true,
        id: None,
        tab_id_prefix: DEFAULT_TAB_ID_PREFIX.to_owned(),
        // Opt-in. A strip that has always fitted must not
        // silently gain a scroll viewport — that would change hit-testing and
        // focus scrolling for every existing caller.
        scrollable: false,
        scroll_strip_height: None,
        scroll_id: None,
        fade_cell: None,
        fade_width: None,
        scroll_arrow_config: None,
        aria_label: None,
        selection_ink: selection_ink_handle(),
    }
}

impl<'a, Message: Clone + 'a> EnzTabList<'a, Message> {
    /// Override the colour + geometry tokens.
    pub fn tokens(mut self, t: TabPillTokens) -> Self {
        // Normalize at the public boundary so no non-finite geometry token can
        // reach renderer coordinates. `EnzTabList` stores its own
        // `tokens` (it does not delegate to `EnzTabPill`), so it must sanitize
        // independently.
        self.tokens = Some(t.sanitized());
        self
    }
    /// Override the pill geometry only, leaving every colour resolving from
    /// the runtime theme.
    ///
    /// The hook for a composite with its own geometry tokens:
    /// [`tokens`](Self::tokens) pins colours *and* geometry (an explicit
    /// theming opt-out), while restating `pill_height`/`pill_radius` alone
    /// must not pay that price.
    pub fn geometry(mut self, g: TabPillGeometry) -> Self {
        // Normalize at the public boundary, like `tokens()`.
        self.geometry = Some(g.sanitized());
        self
    }

    /// Wrap the strip in a horizontal scroll view, so a row of tabs wider than
    /// its container is reachable rather than clipped.
    ///
    /// ## Why this is opt-in
    ///
    /// A scroll viewport is not free: hit-testing changes, a clip boundary
    /// appears, focus movement scrolls. Enabling it everywhere would alter
    /// strips that always fitted. Overflow-capable callers (document tab
    /// bars, filter rails) opt in. Scrollbar hidden; wheel/drag scroll,
    /// matching the delegated preset.
    pub fn scrollable(mut self, on: bool) -> Self {
        self.scrollable = on;
        self
    }

    /// Paint a fade at whichever strip edge is currently clipping content.
    ///
    /// Pass a caller-owned [`ScrollGeometryCell`](crate::scroll_view::ScrollGeometryCell)
    /// kept across frames: the strip publishes live geometry into it per
    /// pass and reads it back next build for the fade decision.
    ///
    /// ## Why the caller owns the cell
    ///
    /// A cell built inside `into_element` reconstructs every build and reads
    /// `None` forever — fades never appearing no matter the scroll, with no
    /// single-build test able to tell. As a parameter its lifetime outlives
    /// a build.
    ///
    /// ## The one-frame lag
    ///
    /// The build-time offset is the previous pass's observation: the fade
    /// settles one frame after a gesture ends — invisible at interactive
    /// rates, and (unlike virtual lists) never a blank row. Needs
    /// [`scrollable(true)`](Self::scrollable): no viewport, no offset.
    pub fn fade_edges(mut self, cell: crate::scroll_view::ScrollGeometryCell) -> Self {
        self.fade_cell = Some(cell);
        self
    }

    /// Override the edge-fade width in px (default
    /// [`DEFAULT_FADE_WIDTH`](crate::scroll_fade_edge::DEFAULT_FADE_WIDTH)).
    ///
    /// Non-finite or negative values are ignored. `0.0` is expressible intent
    /// — fades disabled while leaving the rest of the wiring in place.
    pub fn fade_width(mut self, px: f32) -> Self {
        if px.is_finite() && px >= 0.0 {
            self.fade_width = Some(px);
        }
        self
    }

    /// Mount scroll-arrow buttons flanking the strip.
    ///
    /// Each arrow scrolls one tab-cell width ([`Self::scroll_step`]), enabled
    /// only while its own side hides content — reading the same per-side
    /// signal as the fades, so the two never disagree.
    ///
    /// ## Accessible names are required, deliberately
    ///
    /// Unnamed chevrons announce as artwork filenames (*"chevron-left"*),
    /// not actions. The builder carries `ScrollArrows::new`'s required names
    /// forward; generic wording is available via its `DEFAULT_*_LABEL`s.
    ///
    /// ## Requires a geometry cell
    ///
    /// Enabled state derives from the fade cell: without
    /// [`fade_edges`](Self::fade_edges) both arrows mount disabled (no
    /// geometry = "nothing clipped"). Only meaningful with
    /// [`scrollable(true)`](Self::scrollable).
    pub fn scroll_arrows(
        mut self,
        on_prev: Message,
        on_next: Message,
        prev_label: impl Into<String>,
        next_label: impl Into<String>,
    ) -> Self {
        self.scroll_arrow_config = Some(ScrollArrowConfig {
            on_prev,
            on_next,
            prev_label: prev_label.into(),
            next_label: next_label.into(),
        });
        self
    }

    /// Pin the scroll strip's height (only meaningful with
    /// [`scrollable(true)`](Self::scrollable)).
    ///
    /// `None` (the default) derives the height from the resolved pill height,
    /// so enabling scrolling does not add vertical space. Non-finite or
    /// negative values are ignored, keeping the derived height.
    pub fn scroll_strip_height(mut self, h: f32) -> Self {
        self.scroll_strip_height = (h.is_finite() && h >= 0.0).then_some(h);
        self
    }

    /// Attach an `Id` (`iced::widget::Id`) to the inner scrollable so the
    /// strip can be driven programmatically — `scroll_by` for the arrow
    /// affordance, or `scroll_to` to reveal a newly-selected tab.
    ///
    /// Only meaningful with [`scrollable(true)`](Self::scrollable).
    pub fn scroll_id(mut self, id: iced::widget::Id) -> Self {
        self.scroll_id = Some(id);
        self
    }

    /// The pixel distance one arrow-button press should scroll the strip: one
    /// tab cell plus its trailing gap.
    ///
    /// Exposed so the arrow affordance and any caller-built control agree
    /// without re-deriving: two independent derivations drift, invisibly
    /// until a tab lands half-scrolled.
    pub fn scroll_step(&self) -> f32 {
        let geo = self.geo_in_force();
        let step = identifiability_floor(geo) + geo.strip_gap;
        if step.is_finite() && step > 0.0 {
            step
        } else {
            0.0
        }
    }

    /// Set the horizontal width policy of the cells. Defaults to
    /// [`TabListWidth::Hug`].
    pub fn width_policy(mut self, width: TabListWidth) -> Self {
        self.width = width;
        self
    }

    /// Set how the label weight tracks the selected state. Defaults to
    /// [`TabLabelWeight::SelectedSemibold`] (this pill's own contract);
    /// `EnzViewSwitcher` selects [`TabLabelWeight::AlwaysSemibold`] to match the
    /// `EnzButton` segments it replaced.
    pub fn label_weight(mut self, weight: TabLabelWeight) -> Self {
        self.label_weight = weight;
        self
    }

    /// Choose live selected-indicator paint without pinning a colour token set.
    /// Defaults to filled accent; semantics and keyboard routing are unchanged.
    /// Alternative styles resolve selected colours from the live theme, overriding
    /// selected colours in pinned tokens; other token overrides remain in effect.
    pub fn selection_indicator(self, style: super::TabSelectionStyle) -> Self {
        self.selection_ink.style.set(style);
        self
    }

    /// Show a tab's label only while it is selected: the others show their
    /// icon alone, and a label slides open as its tab is chosen and closed
    /// as it is left. Tabs without an icon keep their label. Off by
    /// default.
    pub fn collapse_inactive_labels(mut self, collapse: bool) -> Self {
        self.collapse_labels = collapse;
        self
    }

    /// The item as its cell measures: icon-only while its label is collapsed.
    fn as_measured(&self, index: usize, item: &TabPillItem) -> TabPillItem {
        if self.collapse_labels && !item.icon.is_empty() && index != self.selected {
            TabPillItem {
                label: String::new(),
                ..item.clone()
            }
        } else {
            item.clone()
        }
    }

    /// Stack each icon over its label. Internal compact-rung hook for
    /// [`crate::view_switcher`].
    pub(crate) fn label_layout(mut self, layout: TabLabelLayout) -> Self {
        self.label_layout = layout;
        self
    }

    /// Ask for equal tracks only below `width`, the strip's own
    /// natural sum. Above it every cell keeps its own width and the strip hugs,
    /// so the room it does not need reaches its sibling; below it the equal
    /// tracks are back and no cell is squeezed to pay for a longer one.
    pub(crate) fn equal_cells_below(mut self, width: Option<f32>) -> Self {
        self.equal_cells_below = width;
        self
    }

    pub(crate) fn compress_fill_cells(mut self, on: bool) -> Self {
        self.compress_fill_cells = on;
        self
    }

    /// Set a caller minimum for content-sized cells.
    pub fn cell_min_width(mut self, min: f32) -> Self {
        self.cell_min_width = (min.is_finite() && min > 0.0).then_some(min);
        self
    }

    /// Choose how the cells paint their background. Defaults to
    /// [`TabCellFill::Pill`]; `EnzInlineViewSwitcher` selects
    /// [`TabCellFill::Segmented`] for the joined-block segmented control.
    pub fn cell_fill(mut self, fill: TabCellFill) -> Self {
        self.cell_fill = fill;
        self
    }

    /// Choose pill hover/press geometry; defaults to [`TabHoverShape::Joined`].
    /// Detached washes keep rounded ends at zero gap without changing layout,
    /// focus or selection. Segmented cells retain their segment geometry.
    pub fn hover_shape(mut self, shape: TabHoverShape) -> Self {
        self.hover_shape = shape;
        self
    }

    fn uses_joined_hover(&self, gap: f32) -> bool {
        self.cell_fill == TabCellFill::Pill
            && self.hover_shape == TabHoverShape::Joined
            && gap == 0.0
    }

    /// Enable / disable the whole strip.
    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    /// Replace the selection callback with one that may be absent.
    ///
    /// `None` withdraws activation entirely: no cell factory, no keyboard
    /// publication, [`Self::live`] is `false` — read by the render path
    /// exactly as `enabled(false)`.
    ///
    /// # Why absent means DISABLED, not merely inert
    ///
    /// A tab strip is the only on-screen navigation in its presentation: a
    /// full-strength selected pill swallowing every click is a control that
    /// lies (broken vs slow indistinguishable). Disabled makes the failure
    /// legible at a glance and to AT (`disabled` gates all action
    /// advertisement, so no `Click` is offered unserviceably). It also
    /// matches the sibling rail's existing "no activation = disabled" state,
    /// so both presentations of one builder agree. "Not *yet* usable" is
    /// instead [`Self::enabled`], which keeps the callback for re-arming.
    pub fn on_select_maybe(
        mut self,
        on_select: Option<Box<dyn Fn(usize) -> Message + 'a>>,
    ) -> Self {
        self.on_select = on_select.map(std::rc::Rc::from);
        self
    }

    /// Is this strip a live control?
    ///
    /// `false` when the caller disabled it *or* supplied no selection callback
    /// — the two are indistinguishable to a user, since neither can change the
    /// selection. Every render/semantics decision downstream reads this rather
    /// than `enabled` alone, so a callback-less strip cannot be styled live.
    #[must_use]
    pub fn live(&self) -> bool {
        self.enabled && self.on_select.is_some()
    }

    /// Set the accessible name for the `TabList` group.
    ///
    /// A screen-reader user navigating by landmark hears this label to
    /// identify the group without entering it. The assert that checks for
    /// omission can be armed per-process via [`TAB_LIST_LABEL_ASSERT_ENV`] or
    /// per-thread via [`with_tab_list_label_assert_armed`].
    pub fn aria_label(mut self, label: impl Into<String>) -> Self {
        self.aria_label = Some(label.into());
        self
    }

    /// Set an explicit focus id (for programmatic `focus(id)`).
    pub fn id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Namespace the per-tab derived ids.
    ///
    /// Distinct from [`Self::id`] (the strip's single focus stop): two strips
    /// offering same-keyed tabs would derive the same tab id; a distinct
    /// prefix per strip keeps every tab routable. Duplicates are rejected by
    /// the semantics collector in debug builds.
    pub fn tab_id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.tab_id_prefix = prefix.into();
        self
    }

    /// Choose what an out-of-range `selected` index means. Defaults
    /// to [`TabSelectionBounds::Clamp`].
    pub fn selection_bounds(mut self, bounds: TabSelectionBounds) -> Self {
        self.selection_bounds = bounds;
        self
    }

    /// Resolve the selected index under the [`TabSelectionBounds`] policy.
    ///
    /// Under `Unselected` an out-of-range index resolves to `items.len()` — a
    /// bounded sentinel that matches no cell. It is deliberately *not*
    /// `usize::MAX`: the focus ring indexes `selected * cell_stride` into the
    /// row's children, which would overflow in a debug build.
    pub(super) fn effective_selected(&self) -> usize {
        if self.items.is_empty() {
            return 0;
        }
        match self.selection_bounds {
            TabSelectionBounds::Clamp => self.selected.min(self.items.len() - 1),
            TabSelectionBounds::Unselected => self.selected.min(self.items.len()),
        }
    }

    /// The geometry actually in force: the geometry-only override wins, else the
    /// pinned tokens' geometry, else the preset (identical across presets).
    fn geo_in_force(&self) -> TabPillTokens {
        let mut t = self.tokens.unwrap_or_else(TabPillTokens::dark);
        if let Some(g) = self.geometry {
            // A handed-down geometry (a view switcher's) is already at the
            // desktop text size.
            g.apply(&mut t);
        } else {
            let ui = enzui_theme::text_scale::ui_px;
            t.label_size = ui(t.label_size);
            t.icon_size = ui(t.icon_size);
            t.pill_height = ui(t.pill_height);
        }
        t
    }

    /// Per-cell corner radii. In [`TabCellFill::Segmented`] the strip is one
    /// joined block, so only the first cell's left corners and the last cell's
    /// right corners round; interior corners are square.
    fn cell_radii(&self, i: usize, radius: f32) -> Option<iced::border::Radius> {
        if self.cell_fill != TabCellFill::Segmented {
            return None;
        }
        let last = self.items.len().saturating_sub(1);
        Some(iced::border::Radius {
            top_left: if i == 0 { radius } else { 0.0 },
            bottom_left: if i == 0 { radius } else { 0.0 },
            top_right: if i == last { radius } else { 0.0 },
            bottom_right: if i == last { radius } else { 0.0 },
        })
    }

    /// The widget id tab `index` publishes, to name it from its panel
    /// ([`super::enz_tab_panel`]). `None` for a tab with no label or key.
    #[must_use]
    pub fn tab_id(&self, index: usize) -> Option<iced::advanced::widget::Id> {
        self.tab_ids().get(index).cloned().flatten()
    }

    /// Per-tab AT ids, index-aligned with `self.items`.
    ///
    /// Cells are `embedded` and publish no `focused` of their own, so the
    /// `TabList` group must name the cursored cell as `active_descendant`
    /// or AT announces only the container. Single source: `build_row` ids
    /// the cells and `into_element` hands the same vector to the widget, so
    /// pointed-at and carried ids cannot drift. `None` for a keyless tab —
    /// naming it would dangle.
    fn tab_ids(&self) -> Vec<Option<iced::advanced::widget::Id>> {
        let mut keyed = enzui_core::identity::KeyedIds::new(self.tab_id_prefix.clone());
        self.items
            .iter()
            .map(|item| {
                let key = item.identity_key();
                (!key.is_empty()).then(|| keyed.id_for(key))
            })
            .collect()
    }

    fn build_row(&self) -> (Element<'a, Message>, Option<f32>) {
        let sel = self.effective_selected();
        let geo = self.geo_in_force();
        let fill = self.width == TabListWidth::Fill;
        let segmented = self.cell_fill == TabCellFill::Segmented;
        // Segmented cells are joined: they share a 1 px hairline seam instead of
        // a transparent gap, so the row spacing must be 0 and the seam is an
        // explicit child pushed between cells.
        let spacing = if segmented { 0.0 } else { geo.strip_gap };
        let cell_geo = self.geometry;
        let seam_h = geo.pill_height;
        let pinned = self.tokens;

        #[derive(Clone, Copy)]
        enum CellFloor {
            None,
            Clamped(f32),
            Overflowing(f32),
        }

        // Explicit scrolling predates the automatic narrow policy and has a
        // public geometry contract: Fill cells use the nominal 84.5px floor,
        // retain their overflowing MinWidth, and advance by the existing flex
        // geometry. Keep that path byte-for-byte equivalent to current main.
        // Only a non-scrollable strip gets the label-aware equal-track policy.
        let (cell_floors, automatic_content_floor) = if self.scrollable {
            let identifiability = if fill && !self.compress_fill_cells {
                identifiability_floor(geo)
            } else {
                0.0
            };
            let floor = match (fill, self.cell_min_width) {
                (true, None) if self.compress_fill_cells => CellFloor::None,
                (true, explicit) => {
                    CellFloor::Overflowing(identifiability.max(explicit.unwrap_or(0.0)))
                }
                (false, Some(explicit)) => CellFloor::Clamped(explicit),
                (false, None) => CellFloor::None,
            };
            (vec![floor; self.items.len()], None)
        } else {
            let floor_weight = match self.label_weight {
                TabLabelWeight::AlwaysRegular => Weight::Normal,
                TabLabelWeight::SelectedSemibold | TabLabelWeight::AlwaysSemibold => {
                    Weight::Semibold
                }
            };
            let mut floors: Vec<f32> = self
                .items
                .iter()
                .enumerate()
                .map(|(i, item)| {
                    narrow_cell_floor(
                        &self.as_measured(i, item),
                        geo,
                        fill,
                        self.label_layout,
                        floor_weight,
                    )
                })
                .collect();
            if !fill && let Some(caller_floor) = self.cell_min_width {
                for floor in &mut floors {
                    *floor = floor.max(caller_floor);
                }
            }
            // The strip starts scrolling while every cell can still hold the
            // widest cell's floor, so compressed tracks stay equal. Each
            // cell's own minimum stays its own floor, which never exceeds its
            // natural width: a short label keeps its natural cell above the
            // compression point instead of being widened to the widest floor.
            let common = floors.iter().copied().fold(0.0, f32::max);
            let seams = if segmented {
                self.items.len().saturating_sub(1) as f32
            } else {
                0.0
            };
            let cell_floor_sum = if self.collapse_labels {
                floors.iter().sum()
            } else {
                common * self.items.len() as f32
            };
            let content_floor =
                cell_floor_sum + spacing * self.items.len().saturating_sub(1) as f32 + seams;
            let natural_sum = self
                .items
                .iter()
                .enumerate()
                .map(|(i, item)| {
                    natural_cell_width(
                        &self.as_measured(i, item),
                        geo,
                        fill,
                        self.label_layout,
                        floor_weight,
                    )
                })
                .sum::<f32>()
                + spacing * self.items.len().saturating_sub(1) as f32
                + seams;
            (
                floors.into_iter().map(CellFloor::Clamped).collect(),
                Some((content_floor, natural_sum)),
            )
        };
        // Per-tab identity from each item's stable logical key via
        // `KeyedIds`, not bare `derive_id`: duplicate keys disambiguate by
        // *occurrence*, keeping every other tab's id put (index-derivation
        // would shift them all). One shared deriver — not a second inline
        // walk — so the cell's id and the group's `active_descendant` are
        // literally the same value.
        let derived_tab_ids = self.tab_ids();
        // A `KeyedRow`, not a `row![]`: stable `Id`s address tabs but do
        // nothing for reconciliation, which iced does positionally (retained
        // child state zipped by index, `Id` never consulted). Inserting or
        // reordering would hand slot k's focus/pressed/hover to whatever moved
        // into slot k — the focus ring visibly jumping tabs. Keys come from a
        // second `KeyedIds` in lockstep with the id deriver (both skip empty
        // keys), so reconciliation key and `Id` share the occurrence rule and
        // cannot drift apart.
        let mut row_keys = enzui_core::identity::KeyedIds::new("tab");
        let mut keyed_children: Vec<(String, Element<'a, Message>)> = Vec::new();
        // The key of the last cell pushed, so a seam is keyed by the cell it
        // follows and is therefore as stable as that neighbour.
        let mut previous_key = String::new();
        // A strip with no `on_select` has nowhere to send a selection,
        // so it is not a live control and every cell renders/announces disabled
        // -- see `on_select_maybe` for why disabled rather than silently inert.
        let live = self.live();
        for (i, item) in self.items.iter().enumerate() {
            let enabled = live && item.enabled;
            // Derived for every item, enabled or not, so a disabled tab in the
            // middle of the strip does not shift its neighbours' ids -- the
            // occurrence counter must see the same key sequence each frame.
            let key = item.identity_key();
            let tab_id = derived_tab_ids.get(i).cloned().flatten();
            // An anonymous tab (icon-only, no label and no explicit key) has
            // nothing stable to be named by, so its reconciliation key falls
            // back to its index. That is deliberate and is the honest answer:
            // such a tab has no logical identity, so its state resets when the
            // strip is mutated around it. `anon:` keeps fallback ids distinct
            // from caller-owned keys.
            let row_key = if key.is_empty() {
                format!("anon:{i}")
            } else {
                row_keys.key_for(key)
            };
            if segmented && i > 0 {
                let adjacent_selected = sel < self.items.len() && (i == sel || i == sel + 1);
                let seam_ink = std::rc::Rc::clone(&self.selection_ink);
                let sep = seam::Seam::new(seam_h, move |theme: &iced::Theme| {
                    let tokens = resolve_tokens_with(pinned, cell_geo, theme);
                    let alpha = strip_ink_alpha(&seam_ink);
                    let track =
                        crate::color::with_alpha(tokens.segment_bg, tokens.segment_bg.a * alpha);
                    let hairline = crate::color::with_alpha(
                        tokens.seam,
                        if adjacent_selected {
                            0.0
                        } else {
                            tokens.seam.a * alpha
                        },
                    );
                    (track, hairline)
                });
                // The seam is a child of the strip too, so it needs its own key:
                // an unkeyed gap in a keyed row still shifts everything after it.
                keyed_children.push((format!("seam:{previous_key}"), Element::new(sep)));
            }
            let joined_hover = self.uses_joined_hover(geo.strip_gap);
            let static_radius = geo.pill_radius;
            let cell_selected = i == sel;
            let wrapper_radius: iced::border::Radius = if joined_hover {
                let last = self.items.len().saturating_sub(1);
                iced::border::Radius {
                    top_left: if i == 0 { geo.pill_radius } else { 0.0 },
                    bottom_left: if i == 0 { geo.pill_radius } else { 0.0 },
                    top_right: if i == last { geo.pill_radius } else { 0.0 },
                    bottom_right: if i == last { geo.pill_radius } else { 0.0 },
                }
            } else {
                self.cell_radii(i, geo.pill_radius)
                    .unwrap_or_else(|| geo.pill_radius.into())
            };
            // Cells are non-clickable here: the owning list handles keyboard
            // and focus, so `clickable: false` withholds the per-cell focus
            // stop. Pointer activation is re-added below via `mouse_area`.
            let cell = build_pill::<Message>(
                &item.label,
                &item.icon,
                PillSpec {
                    badge: item.badge.clone(),
                    busy: item.busy,
                    selected: i == sel,
                    enabled,
                    pinned: self.tokens,
                    geo: cell_geo,
                    on_activated: None,
                    clickable: false,
                    weight: self.label_weight,
                    label_layout: self.label_layout,
                    collapse_label: self.collapse_labels && !item.icon.is_empty(),
                    // `Fill` must reach the *painted* pill, not a wrapper, so the
                    // fill, hover overlay and hit area all cover the whole cell.
                    fill,
                    cell_fill: self.cell_fill,
                    radii: if joined_hover && !enabled {
                        Some(wrapper_radius)
                    } else {
                        self.cell_radii(i, geo.pill_radius)
                    },
                    // The strip cell is non-clickable here (`clickable: false`),
                    // so `build_pill` renders no focus button and this id is
                    // unused; the list applies the per-tab derived id to its own
                    // `focus_button_maybe_with` below (`fb = fb.id(id)`).
                    id: None,
                    // Hand this cell's window onto the strip's ink down
                    // so its fill is suppressed and its label ink follows the
                    // thumb while the `IndicatorLayer` slides the indicator.
                    ink: Some(CellInk::new(std::rc::Rc::clone(&self.selection_ink))),
                },
            );
            let cell = match cell_floors.get(i).copied().unwrap_or(CellFloor::None) {
                CellFloor::None => cell,
                CellFloor::Clamped(floor) => crate::min_width::min_width(cell, floor).into(),
                CellFloor::Overflowing(floor) => crate::min_width::min_width(cell, floor)
                    .overflowing(true)
                    .into(),
            };
            // Publish the AT node from the cell itself: it carries the real
            // hit bounds and nests its text under one interactive ancestor.
            let semantics = Semantics::new(Role::Tab)
                // The *announced* name, which is the visible label
                // unless an `aria_label` overrides it — the only way an
                // icon-only tab (empty visible label) is still announced by
                // its full word rather than as an anonymous node.
                .label(item.announced_name())
                .selected(i == sel)
                .disabled(!enabled)
                .busy(item.busy)
                .set_position(enzui_core::semantics::SetPosition {
                    // 1-based, as AT announces it ("tab 2 of 4").
                    index: i + 1,
                    count: self.items.len(),
                });
            let semantics = match &item.panel {
                Some(panel) => semantics.controls(panel.clone()),
                None => semantics,
            };
            // Only an enabled tab advertises the default (Click) action — an AT
            // user must not be told a dead segment is activatable.
            let semantics = if enabled {
                semantics.default_action()
            } else {
                semantics
            };
            // Publish the leaf UNDER the tab's derived id, so an AT
            // request naming it resolves through `NodeIdMap`. An unkeyed
            // (icon-only, labelless) tab stays anonymous and the identity invariant
            // strips the `Click` it would otherwise advertise -- which is why
            // `semantics.default_action()` above is safe to leave unconditional
            // on identity.
            let leaf = crate::semantics_container::SemanticsContainer::leaf(cell, semantics);
            let cell: Element<'a, Message> = match &tab_id {
                Some(id) => leaf.id(id.clone()).into(),
                None => leaf.into(),
            };
            // Embedded activation keeps the AT carrier while the strip owns keyboard focus.
            let clickable: Element<'a, Message> = if enabled {
                // The factory exists only when a callback does. It is
                // `Some` on every pass through this branch (`enabled` implies
                // `live()` implies `on_select.is_some()`), but it is built from
                // the `Option` rather than unwrapped, so a callback-less strip
                // can never hand `focus_button` something that panics when run
                // -- which is exactly what `settings_dialog::id_noop` did.
                let on_select = self.on_select.clone();
                let activate: Option<Box<dyn Fn() -> Message + 'a>> =
                    on_select.map(|f| Box::new(move || f(i)) as Box<dyn Fn() -> Message + 'a>);
                let mut fb = crate::focus_button::focus_button_maybe_with(
                    cell,
                    // Lazy: the factory runs at activation time, not per tab per
                    // view pass -- preserving the property
                    // `mouse_area::on_press_with` gave this call site.
                    activate,
                )
                .embedded(true)
                // `FocusButton` defaults to `Shrink`: the wrapper would
                // collapse to the label and a `Fill` cell would paint and
                // hit-test only a label-sized island mid-share. Propagate the
                // cell's width policy through so `Fill` reaches the pill.
                .width(if fill { Length::Fill } else { Length::Shrink })
                // The pill paints its own fill/hover: the wrapper stays
                // transparent (no double-painted accent, no ring — the strip
                // draws the ring itself). Hover paints the runtime
                // `hover_overlay` token (draw-time resolved, theme-following)
                // on unselected cells only (selected carries Primary already).
                // Segmented mode inverts this: the pill goes transparent (an
                // opaque pill occluded the translucent wash) and the wrapper
                // paints the full opaque cell fill. Capture the ink handle so
                // the destination's opaque fill suppresses while the thumb is
                // in flight instead of stacking under it. The selection
                // indicator animates this style itself.
                .reduced_motion(true)
                .style_focus({
                    let ink = CellInk::new(std::rc::Rc::clone(&self.selection_ink));
                    move |theme: &iced::Theme, status| {
                        let t = super::resolve_selection_tokens(
                            resolve_tokens_with(pinned, cell_geo, theme),
                            ink.handle.style.get(),
                            theme,
                        );
                        // The indicator layer owns backgrounds during motion and material passes.
                        let ink_alpha = selection_ink_alpha(Some(&ink));
                        let background = if segmented {
                            let raw = segmented_cell_bg(
                                cell_selected,
                                /* enabled */ true,
                                status,
                                &t,
                            );
                            Some(iced::Background::Color(crate::color::with_alpha(
                                raw,
                                raw.a * ink_alpha,
                            )))
                        } else if joined_hover {
                            None
                        } else {
                            cell_hover_background(cell_selected, status, &t).map(|c| {
                                iced::Background::Color(crate::color::with_alpha(
                                    c,
                                    c.a * ink_alpha,
                                ))
                            })
                        };
                        iced::widget::button::Style {
                            background,
                            border: iced::Border {
                                color: Color::TRANSPARENT,
                                width: 0.0,
                                radius: super::retarget_radius(
                                    wrapper_radius,
                                    static_radius,
                                    t.pill_radius,
                                ),
                            },
                            ..iced::widget::button::Style::default()
                        }
                    }
                });
                if let Some(id) = tab_id.clone() {
                    fb = fb.id(id);
                }
                fb.into()
            } else {
                cell
            };
            // An icon-only tab shows its accessible name on hover (unless
            // overridden). The tooltip wraps the *outermost* cell element as
            // a pure pass-through — layout, `mouse_interaction`, `operate`
            // and `draw` delegate, so pill hit region, focus and the `Role::Tab`
            // leaf stay untouched; only a hover overlay adds. Empty tips
            // return the content unwrapped, so labelled tabs (resolving to
            // `""`) are never wrapped at all.
            let clickable = crate::tooltip::enz_tooltip_with_delay(
                clickable,
                // Every icon tab carries its tip, the selected one too, so a selection change
                // does not rebuild the cell and cut its label's slide short.
                item.resolved_tooltip(self.collapse_labels && !item.icon.is_empty()),
                iced::widget::tooltip::Position::Bottom,
                item.tooltip_delay
                    .unwrap_or_else(crate::tooltip::default_tooltip_delay),
            );
            keyed_children.push((row_key.clone(), clickable));
            previous_key = row_key;
        }
        let equal_below = match automatic_content_floor {
            None => self.equal_cells_below,
            Some(_) if self.collapse_labels || (fill && self.compress_fill_cells) => {
                self.equal_cells_below
            }
            Some((content_floor, natural_sum)) => {
                // Equal tracks from the natural sum down: between it and the
                // content floor a flex row would squeeze its last cell to a
                // sliver instead of sharing the shortfall.
                let narrow_equal_below = content_floor.max(natural_sum) + 0.5;
                Some(
                    self.equal_cells_below
                        .map_or(narrow_equal_below, |existing| {
                            existing.max(narrow_equal_below)
                        }),
                )
            }
        };
        let equal_cells = if self.scrollable {
            fill && self.compress_fill_cells || self.equal_cells_below.is_some()
        } else {
            fill && self.compress_fill_cells || equal_below.is_some()
        };
        let r = crate::keyed_row::keyed_row(keyed_children)
            .mirrored(
                enzui_core::placement::ambient_direction()
                    == enzui_core::placement::Direction::RightToLeft,
            )
            .spacing(spacing)
            .align_y(Alignment::Center)
            .equal_cells(equal_cells)
            .equal_cells_below(equal_below);
        let row = if fill {
            r.width(Length::Fill).into()
        } else {
            r.into()
        };
        (
            row,
            automatic_content_floor.map(|(content_floor, _)| content_floor),
        )
    }

    /// Build the list element.
    pub fn into_element(mut self) -> Element<'a, Message> {
        let (child, automatic_content_floor) = self.build_row();
        // The moving selection indicator wraps the ROW, before scroll /
        // fade / arrow wrappers: `layout.children()` must be the row's cells
        // (through a viewport the thumb would measure a wrapper rectangle),
        // and the thumb paints inside the scroll clip + translation instead
        // of over arrows and past the fade.
        let child: Element<'a, Message> = {
            let geo = self.geo_in_force();
            let cells = IndicatorCells {
                cell_stride: if self.cell_fill == TabCellFill::Segmented {
                    2
                } else {
                    1
                },
                cell_fill: self.cell_fill,
                joined_hover: self.uses_joined_hover(geo.strip_gap),
                // Indicator paint must use the cells' effective enabled state.
                enabled_flags: self
                    .items
                    .iter()
                    .map(|i| self.live() && i.enabled)
                    .collect(),
                radii: (0..self.items.len())
                    .map(|i| {
                        if self.uses_joined_hover(geo.strip_gap)
                            && !(self.live() && self.items[i].enabled)
                        {
                            iced::border::Radius {
                                top_left: if i == 0 { geo.pill_radius } else { 0.0 },
                                bottom_left: if i == 0 { geo.pill_radius } else { 0.0 },
                                top_right: if i + 1 == self.items.len() {
                                    geo.pill_radius
                                } else {
                                    0.0
                                },
                                bottom_right: if i + 1 == self.items.len() {
                                    geo.pill_radius
                                } else {
                                    0.0
                                },
                            }
                        } else {
                            self.cell_radii(i, geo.pill_radius)
                                .unwrap_or_else(|| geo.pill_radius.into())
                        }
                    })
                    .collect(),
                static_radius: geo.pill_radius,
            };
            Element::new(IndicatorLayer::new(
                child,
                self.effective_selected(),
                self.items.len(),
                cells,
                std::rc::Rc::clone(&self.selection_ink),
                self.tokens,
                self.geometry,
            ))
        };
        // Wrap the row in a horizontal scroll viewport when opted in, so
        // overflow is reachable not clipped — delegating to the crate's one
        // scroll implementation (hidden scrollbar, wheel + drag). The wrapper
        // sits *inside* `TabListWidget`: the list keeps roving, semantics and
        // the group node; only the painted row scrolls.
        let child = if self.scrollable {
            let geo = self.geo_in_force();
            // Derive the strip height from the pill so opting into scrolling
            // does not silently add vertical space. The preset's own 100px
            // default is meant for card rails, not a tab bar.
            let strip_h = self.scroll_strip_height.unwrap_or(geo.pill_height);
            let mut sv = crate::horizontal_scroll_view::enz_horizontal_scroll_view(child)
                .strip_height(strip_h);
            if let Some(id) = self.scroll_id.clone() {
                sv = sv.id(id);
            }
            let scrolled: Element<'a, Message> = sv.into();
            // Derive the per-side clipping signal once: the fades and arrows
            // both read it, so computing it once keeps them in agreement.
            let edges = self
                .fade_cell
                .as_ref()
                .and_then(super::super::scroll_view::ScrollGeometryCell::get)
                .map_or(crate::scroll_fade_edge::ScrollFadeEdges::NONE, |g| {
                    crate::scroll_fade_edge::ScrollFadeEdges::from_geometry(&g)
                });
            // Fades. Read the geometry the *previous* pass
            // stamped to decide which edges clip, then publish this pass's
            // geometry for the next build. Reading before wrapping is what
            // makes the fade track scrolling rather than freezing at "start".
            let strip: Element<'a, Message> = match &self.fade_cell {
                None => scrolled,
                Some(cell) => {
                    let published = crate::scroll_view::publish_geometry_of(scrolled, cell.clone());
                    let mut fade = crate::scroll_fade_edge::scroll_fade_edges(published, edges);
                    if let Some(w) = self.fade_width {
                        fade = fade.width(w);
                    }
                    fade.into()
                }
            };
            // Arrow buttons. Flank the (possibly faded) strip.
            // The arrows share the same `edges` signal so an arrow is enabled
            // on exactly the same sides the fade paints.
            //
            // Take the config out of self here rather than moving from `self`
            // directly — `self` is still borrowed below for `tab_ids` etc.
            match self.scroll_arrow_config.take() {
                None => strip,
                Some(cfg) => {
                    let arrows = crate::scroll_arrows::ScrollArrows::new(
                        cfg.prev_label,
                        cfg.next_label,
                        cfg.on_prev,
                        cfg.on_next,
                    )
                    .edges(edges);
                    iced::widget::row![arrows.prev_button(), strip, arrows.next_button(),]
                        .align_y(iced::Alignment::Center)
                        .into()
                }
            }
        } else {
            child
        };
        let tab_ids = self.tab_ids();
        let sel = self.effective_selected();
        let count = self.items.len();
        // `live()`, not `enabled` -- a strip with no callback must not
        // rove either, or the arrow keys would move a cursor whose move can
        // never be published.
        let live = self.live();
        let enabled_flags: Vec<bool> = self.items.iter().map(|i| live && i.enabled).collect();
        let pinned = self.tokens;
        // Segmented rows interleave a 1 px seam child between cells, so the
        // selected cell sits at `2 * selected` in the row's layout children. Get
        // this wrong and the focus ring lands on a seam or on the wrong cell.
        let cell_stride = if self.cell_fill == TabCellFill::Segmented {
            2
        } else {
            1
        };
        let list: Element<'a, Message> = Element::new(TabListWidget {
            child,
            selected: sel,
            count,
            enabled_flags,
            // The widget's own `enabled` is the live state, so the
            // group node, the focus registration, the press-focus sweep and
            // the keyboard arm all treat "no callback" exactly as they treat
            // "disabled by the caller" -- one predicate, no second path to
            // keep in step.
            enabled: live,
            on_select: self.on_select,
            pinned,
            geometry: self.geometry,
            cell_stride,
            id: self.id,
            // The same ids the cells carry, so the group can name
            // the cursored cell.
            tab_ids,
            aria_label: self.aria_label,
        });
        match automatic_content_floor {
            Some(content_floor) => Element::new(NarrowTabViewport::new(
                list,
                sel,
                cell_stride,
                content_floor,
            )),
            None => list,
        }
    }
}

impl<'a, Message: Clone + 'a> From<EnzTabList<'a, Message>> for Element<'a, Message> {
    fn from(t: EnzTabList<'a, Message>) -> Self {
        t.into_element()
    }
}

/// Pass-through normally; narrow rows clip and centre selection.
struct NarrowTabViewport<'a, Message> {
    child: Element<'a, Message>,
    selected: usize,
    cell_stride: usize,
    content_floor: f32,
}

#[derive(Default)]
struct NarrowTabViewportState(bool);

impl<'a, Message> NarrowTabViewport<'a, Message> {
    fn new(
        child: Element<'a, Message>,
        selected: usize,
        cell_stride: usize,
        content_floor: f32,
    ) -> Self {
        Self {
            child,
            selected,
            cell_stride,
            content_floor,
        }
    }

    fn child_layout(active: bool, layout: Layout, tree: &Tree) -> Layout {
        if active {
            layout
                .children(&tree.children)
                .next()
                .expect("narrow tab viewport has one strip child")
        } else {
            layout
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct TabListState {
    focus: FocusState,
    /// Per-window modality key: captured in `update`, read in
    /// `draw`, so a commit in another window does not hide this strip's ring.
    window_key: u64,
}

impl iced::advanced::widget::operation::Focusable for TabListState {
    fn is_focused(&self) -> bool {
        self.focus.is_focused()
    }
    fn focus(&mut self) {
        self.focus.focus();
    }
    fn unfocus(&mut self) {
        self.focus.unfocus();
    }
}

pub(super) struct TabListWidget<'a, Message> {
    pub(super) child: Element<'a, Message, iced::Theme, iced::Renderer>,
    pub(super) selected: usize,
    pub(super) count: usize,
    pub(super) enabled_flags: Vec<bool>,
    /// The live state, i.e. `EnzTabList::live()`: the caller enabled the
    /// strip and supplied somewhere for a selection to go. One
    /// predicate for both, so the focus registration, the press-focus sweep,
    /// the group node and the keyboard arm cannot disagree about whether this
    /// strip does anything.
    pub(super) enabled: bool,
    /// `None` when the caller supplied no selection callback. The
    /// keyboard arm re-checks it rather than unwrapping, so the strip's two
    /// publication points are independently safe.
    pub(super) on_select: Option<std::rc::Rc<dyn Fn(usize) -> Message + 'a>>,
    pub(super) pinned: Option<TabPillTokens>,
    pub(super) geometry: Option<TabPillGeometry>,
    /// Row-child stride per cell: 1 for a pill strip, 2 for a segmented strip
    /// (whose row interleaves seam children).
    pub(super) cell_stride: usize,
    pub(super) id: Option<iced::advanced::widget::Id>,
    /// Per-tab AT ids, index-aligned with the cells, supplied by
    /// `EnzTabList::tab_ids` so they are the *same* ids the cells carry.
    ///
    /// The group names the cursored cell through these.
    pub(super) tab_ids: Vec<Option<iced::advanced::widget::Id>>,
    /// Accessible name forwarded from `EnzTabList::aria_label`.
    pub(super) aria_label: Option<String>,
}

impl<'a, Message> TabListWidget<'a, Message> {
    /// Next enabled index in `dir` (+1/-1) from `from`, or `None` if none.
    fn step(&self, from: usize, dir: isize) -> Option<usize> {
        if self.count == 0 {
            return None;
        }
        let n = self.count as isize;
        let mut i = from as isize;
        for _ in 0..self.count {
            i += dir;
            if i < 0 {
                i = n - 1;
            } else if i >= n {
                i = 0;
            }
            if self.enabled_flags.get(i as usize).copied().unwrap_or(false) {
                return Some(i as usize);
            }
        }
        None
    }

    fn first_enabled(&self) -> Option<usize> {
        (0..self.count).find(|&i| self.enabled_flags.get(i).copied().unwrap_or(false))
    }
    fn last_enabled(&self) -> Option<usize> {
        (0..self.count)
            .rev()
            .find(|&i| self.enabled_flags.get(i).copied().unwrap_or(false))
    }

    /// Target index for a nav intent, or `None` if it doesn't move.
    ///
    /// With nothing selected (out-of-range index) there is no cursor to step
    /// from: forward enters at the first enabled tab, backward at the last —
    /// the same landing points Home/End use.
    pub(super) fn nav_target(&self, nav: Nav) -> Option<usize> {
        if self.selected >= self.count {
            return match nav {
                Nav::Increment => self.first_enabled(),
                Nav::Decrement => self.last_enabled(),
                Nav::Home => self.first_enabled(),
                Nav::End => self.last_enabled(),
                Nav::PageUp | Nav::PageDown => None,
            };
        }
        match nav {
            Nav::Increment => self.step(self.selected, 1),
            Nav::Decrement => self.step(self.selected, -1),
            Nav::Home => self.first_enabled(),
            Nav::End => self.last_enabled(),
            Nav::PageUp | Nav::PageDown => None,
        }
    }

    /// The tablist group semantics + its Tab-leaf children.
    /// The strip's roving cursor: the index the focus ring paints and every
    /// arrow key steps from.
    ///
    /// Selection follows focus, so the cursor *is* `selected` — while
    /// `selected` is on the strip. `Unselected` parks it out of range: no
    /// cursor at all, and `None` keeps "no cursor" distinct from "cursor at
    /// 0", so the group never names a cell the user is not on.
    fn cursor_index(&self) -> Option<usize> {
        (self.selected < self.count).then_some(self.selected)
    }

    pub(super) fn group_semantics(&self, focused: bool) -> Semantics {
        // A TabList without an accessible name is anonymous — a
        // screen-reader user navigating by landmark cannot tell which group
        // of tabs it is. The assert is gated behind
        // `tab_list_label_assert_armed()` so un-migrated consumers keep
        // working until they adopt `.aria_label("...")`, following the
        // crate's staged-adoption pattern for opt-in asserts.
        debug_assert!(
            !tab_list_label_assert_armed() || self.aria_label.is_some(),
            "{}",
            MISSING_TAB_LIST_LABEL,
        );
        let mut sem = Semantics::new(Role::TabList)
            .orientation(enzui_core::semantics::Orientation::Horizontal)
            .disabled(!self.enabled)
            .focused(focused);
        if let Some(ref label) = self.aria_label {
            sem = sem.label(label.clone());
        }
        sem
    }
}

/// The strip's cell layouts, below the single-child wrappers (a narrow
/// viewport, the row's own owner) that hold it in their trees.
fn cell_layouts(mut layout: Layout, mut tree: &Tree) -> iced::advanced::layout::Children<'_> {
    while let [only] = tree.children.as_slice() {
        layout = layout.child(&tree.children, 0);
        tree = only;
    }
    layout.children(&tree.children)
}

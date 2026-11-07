//! `EnzTabPill` + `EnzTabList` — tab-strip primitives.
//!
//! Pill-shaped tabs. A selected tab paints an accent fill (with
//! contrast-derived ink, ≥ 3:1 against the shell background per WCAG
//! SC 1.4.11) and is transparent with a subtle hover overlay otherwise.
//! Both the single [`enz_tab_pill`] pill and the owning [`enz_tab_list`]
//! container are exposed here.
//!
//! # Where this belongs
//!
//! [`crate::view_switcher`] and [`crate::inline_view_switcher`] compose a
//! row of these pills for their exclusive-selection UI — reach for the
//! switcher for a small set of exclusive views to swap between,
//! reach for the pill directly for a single selection cue.
//!
//! # Two layers
//!
//! * [`EnzTabPill`] — one stateless pill. Self-composed (icon + label in a
//!   styled `container`), so every colour resolves from the runtime
//!   `iced::Theme` at draw time, the label weight is regular when unselected
//!   and semibold when selected, and the icon and label share one foreground
//!   colour. Geometry: 28 px height, 9 px radius, 14 px horizontal padding,
//!   derived as `pill-height = button-height - 4` and
//!   `pill-radius = button-radius - 1`.
//! * [`EnzTabList`] — the WAI-ARIA tablist owner. A custom widget that owns
//!   the pill row as a single child, registers one focus stop, publishes a
//!   [`Role::TabList`] group with [`Role::Tab`] leaves, and drives roving
//!   keyboard navigation — Left/Right (and Up/Down) move the active tab,
//!   Home/End jump to first/last, and selection *follows focus* (each move
//!   publishes `on_select(index)`).
//!
//! A bare `row!` of [`enz_tab_pill`] calls is still fine for a purely
//! click-driven strip; reach for [`enz_tab_list`] for keyboard
//! roving + tablist semantics.

use enzui_core::semantics::{Role, Semantics};
use enzui_theme::Palette;
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::font::Weight;
use iced::widget::{Space, container, row};
use iced::{
    Alignment, Background, Border, Color, Element, Event, Length, Padding, Rectangle, Size, Vector,
};

// The colour + geometry token layer lives in a physically separate file to
// keep this module under its line budget; `include!` inlines it into the same
// module scope, so no items change visibility, name, or API.
include!("tab_pill/tokens.rs");

// The shared selection-indicator engine both switcher presets (and the
// lone `enz_tab_pill`) route through. `include!`-inlined to stay in the same
// module scope, so `SelectionInkHandle`, `IndicatorState`, `paint_selection_thumb`
// and `lerp_rect` are `pub(crate)` items of `tab_pill`.
include!("tab_pill/selection_indicator.rs");

mod list;

pub use list::DEFAULT_TAB_ID_PREFIX;
pub use list::EnzTabList;
pub use list::MISSING_TAB_LIST_LABEL;
pub use list::TAB_LIST_LABEL_ASSERT_ENV;
pub use list::TabCellFill;
pub use list::TabHoverShape;
pub(crate) use list::TabLabelLayout;
pub use list::TabLabelWeight;
pub use list::TabListWidth;
pub use list::TabPillItem;
pub use list::TabSelectionBounds;
pub(crate) use list::busy_width;
pub use list::enz_tab_list;
pub use list::enz_tab_list_maybe;
pub use list::tab_list_label_assert_armed;
pub use list::with_tab_list_label_assert_armed;

/// The content tab `tab` shows, published as a tab panel the tab names.
///
/// Give the tab [`TabPillItem::panel`] with the same `id`, and pass the tab's
/// id from [`EnzTabList::tab_id`]: the tab then `controls` the panel and
/// the panel is `labelled_by` the tab, so a screen reader announces the
/// panel by its tab's name and can move between them.
pub fn enz_tab_panel<'a, Message: 'a>(
    content: impl Into<iced::Element<'a, Message>>,
    id: iced::advanced::widget::Id,
    tab: Option<iced::advanced::widget::Id>,
) -> iced::Element<'a, Message> {
    let semantics = enzui_core::semantics::Semantics::new(enzui_core::semantics::Role::TabPanel);
    let semantics = match tab {
        Some(tab) => semantics.labelled_by(tab),
        None => semantics,
    };
    crate::semantics_container::SemanticsContainer::new(content, semantics)
        .id(id)
        .into()
}

// ---------------------------------------------------------------------------
// Single pill
// ---------------------------------------------------------------------------

/// The namespace a lone tab pill derives its id under when no
/// [`EnzTabPill::id`] or [`EnzTabPill::id_prefix`] is supplied.
pub const DEFAULT_ID_PREFIX: &str = "enz-tab-pill";

/// Tab-pill builder. Stateless: caller supplies label + `selected` + activation
/// message.
#[must_use]
pub struct EnzTabPill<'a, Message> {
    label: String,
    selected: bool,
    on_activated: Message,
    tokens: Option<TabPillTokens>,
    enabled: bool,
    icon: String,
    /// Explicit id override; wins over the key/label-derived default.
    id: Option<iced::advanced::widget::Id>,
    /// Stable logical key the derived id uses instead of the label. The label
    /// is *presentation* (it retranslates, it gains badges), so a caller that
    /// owns a route key should pass it here — mirrors [`TabPillItem::key`].
    key: Option<String>,
    /// Namespace for the derived default id. Defaults to [`DEFAULT_ID_PREFIX`].
    id_prefix: String,
    _phantom: std::marker::PhantomData<&'a ()>,
}

/// Construct a tab pill. `on_activated` is mandatory: the parameter is
/// taken by value at the type level, so a missing activation cannot be
/// expressed and no runtime panic can arise from omitting it.
pub fn enz_tab_pill<'a, Message: Clone + 'a>(
    label: impl Into<String>,
    selected: bool,
    on_activated: Message,
) -> EnzTabPill<'a, Message> {
    EnzTabPill {
        label: label.into(),
        selected,
        on_activated,
        tokens: None,
        enabled: true,
        icon: String::new(),
        id: None,
        key: None,
        id_prefix: DEFAULT_ID_PREFIX.to_owned(),
        _phantom: std::marker::PhantomData,
    }
}

impl<'a, Message: Clone + 'a> EnzTabPill<'a, Message> {
    /// Override the colour + geometry tokens. When pinned, colours no longer
    /// track the runtime theme (explicit opt-out).
    pub fn tokens(mut self, t: TabPillTokens) -> Self {
        // Normalize at the public boundary so no non-finite geometry token can
        // reach renderer coordinates.
        self.tokens = Some(t.sanitized());
        self
    }

    /// Enable / disable the pill.
    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    /// Optional leading icon. Empty string omits the
    /// slot. The icon shares the label's foreground colour in every state.
    pub fn icon(mut self, name: impl Into<String>) -> Self {
        self.icon = name.into();
        self
    }

    /// Assign a stable widget id, forwarded to the composed
    /// [`crate::focus_button::FocusButton`] so `focus(id)` and AccessKit action
    /// routing can target this pill. Overrides the key/label-derived
    /// default entirely.
    pub fn id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Supply the stable logical key this pill's derived id uses instead of
    /// the label. Prefer this whenever the caller owns a real route
    /// key: the label is presentation and re-identifies the pill when it is
    /// retranslated or gains a badge. Ignored when an explicit [`Self::id`] is
    /// set.
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }

    /// Namespace the derived default id. Two standalone pills sharing
    /// a label on one screen would otherwise derive the same id under
    /// [`DEFAULT_ID_PREFIX`] and collide; release keeps the first and drops
    /// the second. A distinct prefix per group keeps both routable. Ignored
    /// when an explicit [`Self::id`] is set.
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }

    /// Build the pill body (icon + label in a styled container). Colours are
    /// resolved from the runtime theme inside the style closures unless pinned.
    pub fn into_element(self) -> Element<'a, Message> {
        // Derive identity so release does not strip the lone pill's
        // Click. Key precedence: explicit `.id()` > `.key()` > label. The label
        // is a reasonable fallback but presentation; a caller with a route key
        // should pass `.key(...)`. An explicit id wins over both.
        let widget_id = self.id.unwrap_or_else(|| {
            let key = self.key.as_deref().unwrap_or(&self.label);
            enzui_core::identity::derive_id(&self.id_prefix, key)
        });
        build_pill(
            &self.label,
            &self.icon,
            PillSpec {
                busy: false,
                selected: self.selected,
                enabled: self.enabled,
                pinned: self.tokens,
                geo: None,
                on_activated: Some(self.on_activated),
                // A lone pill is its own click target.
                clickable: true,
                // A lone pill hugs its label; only a strip cell stretches.
                fill: false,
                weight: TabLabelWeight::default(),
                label_layout: TabLabelLayout::Inline,
                badge: String::new(),
                collapse_label: false,
                // A lone pill is always a pill; `Segmented` is a strip shape.
                cell_fill: TabCellFill::Pill,
                radii: None,
                id: Some(widget_id),
                // A lone pill has no moving indicator to coordinate
                // with, so no ink handle is needed — the style closure treats
                // `None` means paint the selected fill directly.
                ink: None,
            },
        )
    }
}

impl<'a, Message: Clone + 'a> From<EnzTabPill<'a, Message>> for Element<'a, Message> {
    fn from(p: EnzTabPill<'a, Message>) -> Self {
        p.into_element()
    }
}

/// Resolve pill tokens: pinned override, else derived from the runtime theme,
/// then apply any geometry-only override on top (so a composite can restate
/// its geometry without pinning — and killing — the runtime colours).
#[inline]
fn resolve_tokens_with(
    pinned: Option<TabPillTokens>,
    geo: Option<TabPillGeometry>,
    theme: &iced::Theme,
) -> TabPillTokens {
    let mut t = crate::theme_lookup::resolve_pinned_live(
        crate::theme_lookup::PIN_SITE_TAB_PILL,
        pinned,
        theme,
        TabPillTokens::from_palette,
    );
    match geo {
        Some(g) => g.apply(&mut t),
        // A pill is a button: unpinned, its corners follow the theme's
        // `shape.button` step, as the view switcher's do.
        None if pinned.is_none() => {
            t.pill_radius = crate::theme_lookup::shape_radius(theme, |shape| shape.button);
        }
        None => {}
    }
    t
}

/// Everything [`build_pill`] needs beyond the label + icon strings, grouped so
/// the call stays inside clippy's argument budget.
struct PillSpec<Message> {
    /// A spinner follows the label.
    busy: bool,
    selected: bool,
    enabled: bool,
    /// Pinned colour + geometry tokens (an explicit runtime-theming opt-out).
    pinned: Option<TabPillTokens>,
    /// Geometry-only override, applied over the resolved tokens.
    geo: Option<TabPillGeometry>,
    on_activated: Option<Message>,
    /// Whether the pill installs its own mouse handler + focus stop (a lone
    /// pill) or defers both to an owning [`EnzTabList`].
    clickable: bool,
    /// Claim the full available width rather than hugging the label. Set for
    /// [`TabListWidth::Fill`] cells so the *painted* pill — and therefore its
    /// fill, hover overlay and hit area — covers its whole equal-share cell.
    fill: bool,
    /// How the label weight tracks the selected state.
    weight: TabLabelWeight,
    /// Whether an icon sits beside or above its label.
    label_layout: TabLabelLayout,
    /// A count drawn as a small badge after the label; empty for none.
    badge: String,
    /// Show the label only while selected, sliding it open and closed; an
    /// unselected cell with an icon shows the icon alone.
    collapse_label: bool,
    /// How this cell fills. See [`TabCellFill`].
    cell_fill: TabCellFill,
    /// Per-corner radius override, replacing the uniform `pill_radius`. Used by
    /// [`TabCellFill::Segmented`] so only the strip's *outer* corners round.
    radii: Option<iced::border::Radius>,
    /// Stable widget id for the lone-pill focus button. `None` for a
    /// strip cell, whose identity the owning [`EnzTabList`] derives and applies
    /// itself. When `Some`, it is forwarded to the composed
    /// [`crate::focus_button::FocusButton`] so release does not strip the pill's
    /// `Action::Click` — an anonymous node is absent from `NodeIdMap` and has
    /// its whole action mask cleared at release (`enzui_core::collect::SemanticsCollector::to_tree_update_with_ids`).
    id: Option<iced::advanced::widget::Id>,
    /// This cell's window onto the strip's per-frame selection ink.
    /// `None` on a lone pill (no moving indicator to coordinate with). On a
    /// strip cell the owning [`IndicatorLayer`] republishes it every frame:
    /// while a transition is in flight the cell suppresses its own selected
    /// fill (only the moving thumb shows — no double indicator, no brightness
    /// overlap) and its label/icon ink follows the clipped pass being drawn,
    /// so glyph pixels over the thumb and glyph pixels over the track each
    /// carry the token their own background was chosen for.
    ink: Option<CellInk>,
}

/// Build a tab pill's label as a single-line, end-elided text run.
///
/// A raw `text(..)` runs at iced's default `Wrapping::Word`, so a label wider
/// than its cell wrapped to a second line and painted outside the
/// fixed-height pill — the exact failure
/// `button_group.rs`'s `group_label` and `document_tab.rs` already
/// guard against. `single_line_elided` sets `Wrapping::None` + `Ellipsis::End`,
/// converging the tab strip on the crate's one elision treatment (the one
/// `combo_box` uses) rather than `.clip(true)`, which mid-glyph-slices.
///
/// The ellipsis engages once a width bound exists. In a `Fill` cell that
/// bound is supplied by the *parent*, not by the label: `build_pill` centres a
/// `Shrink` icon+label row inside a `Length::Fill` + `align_x(Center)`
/// container, so the row is handed the cell's finite width as its layout max
/// and `Ellipsis::End` engages against it: a 100-char label in a 120 px
/// cell lays out to a single 18 px line, its box clamped to the cell.
/// Critically the label must stay `Shrink` and left-aligned: giving it
/// `.full_width()` + `.align(Center)` would make the label absorb *all* the
/// row's remaining width and centre its glyphs within that remainder, pinning
/// the `Shrink` icon hard against the left edge so the icon and label no longer
/// read as one centred unit. A Hug cell has no
/// parent bound and relies on `cell_min_width` + the pill's fixed height to
/// clip its horizontal overflow.
///
/// The `fill` flag does not change the label itself — the bound arrives from
/// the parent either way. It is retained so the seam and its layout tests stay
/// explicit about which mode is under test.
/// Factored out so the wrapping policy is a single testable seam: the layout
/// test measures this function's output directly.
fn tab_label(label: &str, size: f32, weight: Weight, _fill: bool) -> crate::enz_text::EnzText {
    crate::enz_text::enz_text(label.to_owned())
        .size_px(size)
        .weight(weight)
        .single_line_elided()
}

/// The wrapper background for a strip cell at a given interaction `status`.
///
/// The module doc promises a subtle hover overlay on an unselected ghost
/// pill. The per-cell wrapper is the only status-carrying layer (a plain
/// `container` style closure gets no status), so the overlay is resolved
/// here and returned as `Some(color)`, giving [`EnzViewSwitcher`] /
/// [`EnzInlineViewSwitcher`] the hover their `EnzButton` /
/// `EnzButtonGroup` origins carry:
///
/// * only when not selected — a selected cell already paints its Primary
///   fill and must not be tinted over it;
/// * only on `Hovered` / `Pressed` — `Active` / `Focused` / `Disabled` stay
///   transparent so the pill's own chrome (and the strip's focus ring) is what
///   shows.
///
/// Returned as a colour (not a `Background`) so the caller resolves it from
/// the runtime `TabPillTokens` every draw, keeping it on a live theme switch.
/// Pure, so the policy is a single testable seam.
fn cell_hover_background(
    selected: bool,
    status: crate::focus_button::FocusStatus,
    tokens: &TabPillTokens,
) -> Option<Color> {
    use crate::focus_button::FocusStatus;
    if !selected && matches!(status, FocusStatus::Hovered | FocusStatus::Pressed) {
        Some(tokens.hover_overlay)
    } else {
        None
    }
}

/// Composite `text` at `alpha` onto `base` in sRGB space, returning an
/// opaque colour. The same formula
/// [`enzui_theme::Palette::segment_selected_hover_bg`] uses; kept here
/// so an unselected segmented cell's hover fill (which the palette does not
/// name — the track colour is a widget concern) can be derived the same way
/// without exposing every widget's tokens back through the palette.
fn mix_srgb_over(base: Color, text: Color, alpha: f32) -> Color {
    let a = alpha.clamp(0.0, 1.0);
    let g = 1.0 - a;
    Color::from_rgb(
        base.r * g + text.r * a,
        base.g * g + text.g * a,
        base.b * g + text.b * a,
    )
}

/// The opaque background painted for one segmented cell, given its
/// `selected` state and the wrapper's live [`FocusStatus`]. This is the
/// single seam that decides a segmented cell's fill, so
/// `EnzButtonGroup::selectable(true)` and `EnzTabPill(Segmented)` cannot
/// silently drift again.
///
/// Precedence:
///
/// * disabled: `disabled_bg` (unselected) / dimmed `segment_selected_bg`
///   (selected).
/// * selected + Hovered / Pressed: derived step above the selected fill.
/// * selected + Active / Focused: resting `segment_selected_bg` (focus is
///   not hover; the strip draws the focus ring separately).
/// * unselected + Hovered / Pressed: derived step above the segment track.
/// * unselected + Active / Focused: resting `segment_bg`.
pub(crate) fn segmented_cell_bg(
    selected: bool,
    enabled: bool,
    status: crate::focus_button::FocusStatus,
    tokens: &TabPillTokens,
) -> Color {
    use crate::focus_button::FocusStatus;
    if !enabled {
        return if selected {
            crate::color::with_opacity(
                tokens.segment_selected_bg,
                tokens.segment_disabled_opacity(),
            )
        } else {
            tokens.disabled_bg
        };
    }
    match (selected, status) {
        (true, FocusStatus::Hovered) => tokens.segment_selected_hover_bg,
        (true, FocusStatus::Pressed) => tokens.segment_selected_pressed_bg,
        (true, _) => tokens.segment_selected_bg,
        (false, FocusStatus::Hovered) => tokens.segment_hover_bg,
        (false, FocusStatus::Pressed) => tokens.segment_pressed_bg,
        (false, _) => tokens.segment_bg,
    }
}

/// A tab badge's count size, capsule height and side padding.
const BADGE_TEXT: f32 = 11.0;
const BADGE_HEIGHT: f32 = 16.0;
const BADGE_PADDING_X: f32 = 5.0;

/// The room a tab's badge takes after its label, gap included.
pub(crate) fn badge_width(badge: &str, icon_gap: f32) -> f32 {
    if badge.is_empty() {
        return 0.0;
    }
    let typography = crate::text_measure::MeasureTypography::new(crate::enz_text::TextSize::Body)
        .pin_size_px(enzui_theme::text_scale::ui_px(BADGE_TEXT))
        .weight(iced::font::Weight::Semibold);
    icon_gap
        + (crate::text_measure::enz_text_measure_width(badge, typography) + 2.0 * BADGE_PADDING_X)
            .max(BADGE_HEIGHT)
}

fn build_pill<'a, Message: Clone + 'a>(
    label: &str,
    icon: &str,
    spec: PillSpec<Message>,
) -> Element<'a, Message> {
    let PillSpec {
        busy,
        selected,
        enabled,
        pinned,
        geo,
        on_activated,
        clickable,
        fill,
        weight: weight_policy,
        label_layout,
        badge,
        collapse_label,
        cell_fill,
        radii,
        id,
        ink,
    } = spec;
    let segmented = cell_fill == TabCellFill::Segmented;
    // The style closures below are `Fn` (not `FnOnce`) — the ink handle
    // must be pre-cloned and moved in, not re-cloned inside a closure body.
    let pill_ink = ink.clone();
    // Foreground resolves per clipped PASS, not per build, so the label
    // ink follows the fill actually under each glyph pixel. At rest it resolves
    // the exact colour `pill_fg_role(enabled, selected, segmented)` always gave.
    let fg = PillInkResolver {
        ink,
        selected,
        enabled,
        segmented,
        pinned,
        geo,
    };

    // Geometry is theme-independent; read it off the geometry override, else
    // the pinned tokens, else the dark preset (identical geometry across
    // presets).
    let geometry = {
        let mut t = pinned.unwrap_or_else(TabPillTokens::dark);
        if let Some(g) = geo {
            g.apply(&mut t);
        }
        t
    };

    // Leading and trailing follow the writing direction, as the strip does.
    let rtl =
        enzui_core::placement::ambient_direction() == enzui_core::placement::Direction::RightToLeft;

    // Label weight follows the caller's policy (see `TabLabelWeight`).
    let weight = weight_policy.font_weight(selected);
    let label_el = tab_label(label, geometry.label_size, weight, fill).color_resolved({
        let fg = fg.clone();
        move |theme: &iced::Theme| fg.color(theme)
    });

    // Body: icon + label, or label only. The icon tint is resolved from the
    // same pinned-or-runtime TabPillTokens foreground as the label, every
    // render — `fg.color(...)` — so selected/unselected/disabled and
    // custom/pinned pills never split icon and label colours (the old
    // `.tint_theme_tone` path resolved a *generic* palette tone instead).
    let label_el: Element<'a, Message> = label_el.into_element();
    let body: Element<'a, Message> = if icon.is_empty() {
        label_el
    } else if label.is_empty() {
        // An icon paired with an empty label renders icon-only, reserving no
        // label gap.
        crate::icon::enz_icon(icon)
            .size(geometry.icon_size)
            .tint_resolved({
                let fg = fg.clone();
                move |theme: &iced::Theme| fg.color(theme)
            })
            .into()
    } else {
        let icon_el = crate::icon::enz_icon(icon)
            .size(geometry.icon_size)
            .tint_resolved({
                let fg = fg.clone();
                move |theme: &iced::Theme| fg.color(theme)
            });
        match label_layout {
            // Collapsed: the label and its gap slide open as the cell is
            // selected and closed as it is left, so only the chosen cell
            // reads its name.
            // While the selection slides, the label shows only under the
            // moving indicator: it collapses with the indicator leaving it and
            // opens with the indicator arriving, never on the bare track.
            TabLabelLayout::Inline if collapse_label => row![
                icon_el,
                crate::reveal_width::reveal_width_unless(
                    row![Space::new().width(geometry.icon_gap), label_el]
                        .align_y(Alignment::Center),
                    selected,
                    {
                        let ink = pill_ink.clone();
                        move || {
                            ink.as_ref()
                                .is_some_and(|ink| ink.pass() == InkPass::OverTrack)
                        }
                    },
                )
            ]
            .mirrored(rtl)
            .align_y(Alignment::Center)
            .into(),
            TabLabelLayout::Inline => row![icon_el, label_el]
                .mirrored(rtl)
                .spacing(geometry.icon_gap)
                .align_y(Alignment::Center)
                .into(),
            TabLabelLayout::Stacked => iced::widget::column![icon_el, label_el]
                .spacing(geometry.icon_gap)
                .align_x(Alignment::Center)
                .into(),
        }
    };

    // A badge follows the label: the count in small type on a capsule tinted
    // with the label's own ink, so it reads as a badge on a selected and an
    // unselected tab alike.
    let body: Element<'a, Message> = if badge.is_empty() {
        body
    } else {
        let ink = {
            let fg = fg.clone();
            move |theme: &iced::Theme| fg.color(theme)
        };
        let capsule_ink = ink.clone();
        let count = iced::widget::text(badge)
            .size(enzui_theme::text_scale::ui_px(BADGE_TEXT))
            .font(enzui_theme::text_defaults::font(
                enzui_theme::FontRole::Ui,
                enzui_theme::WeightRung::Semibold,
            ))
            .style(move |theme: &iced::Theme| iced::widget::text::Style {
                color: Some(ink(theme)),
            });
        let capsule = container(count)
            .padding([0.0, BADGE_PADDING_X])
            .height(Length::Fixed(BADGE_HEIGHT))
            .center_y(Length::Fixed(BADGE_HEIGHT))
            .style(move |theme: &iced::Theme| iced::widget::container::Style {
                background: Some(crate::color::with_alpha(capsule_ink(theme), 0.16).into()),
                border: iced::Border::default().rounded(BADGE_HEIGHT / 2.0),
                ..Default::default()
            });
        row![body, capsule]
            .mirrored(rtl)
            .spacing(geometry.icon_gap)
            .align_y(Alignment::Center)
            .into()
    };

    // A busy page's spinner follows its label, inside the cell.
    let body: Element<'a, Message> = if busy {
        row![
            body,
            crate::spinner::enz_spinner()
                .size(geometry.icon_size)
                .into_element::<Message>()
        ]
        .mirrored(rtl)
        .spacing(geometry.icon_gap)
        .align_y(Alignment::Center)
        .into()
    } else {
        body
    };

    // Pill container: fill (Primary) when selected, transparent + hover overlay
    // otherwise. Colours resolve from the runtime theme.
    //
    // No horizontal content padding in Fill/Stretch mode.
    // In Fill mode every cell is an equal share of the strip and its `Fill`
    // label elides (`Ellipsis::End`) against that share's *content box*.
    // Reserving 14 px each side shrinks the content box by 28 px, so a label
    // that fits its equal-share cell (e.g. "Closed" at 37 px in a 53 px cell at
    // the 160 px rung) was nonetheless elided against the 25 px box and
    // truncated to "C…" despite fitting the cell. In Fill mode the
    // inter-cell `strip_gap` and the equal-division whitespace already provide
    // the visual breathing room the padding gave a Hug pill, so the inset is
    // redundant here and only forces premature elision. A Hug/Automatic pill
    // (`!fill`) keeps its full `pill_padding_x` — it shrink-wraps its label and
    // the padding is what insets the text from the Primary fill edge.
    let mut pill = container(body)
        // A cell is a paint boundary, not merely a layout suggestion. Text is
        // clipped here even if a future label/layout policy mismeasures it, so
        // one tab can never draw through its neighbour.
        .clip(true)
        .padding(Padding::from([
            0.0,
            if fill { 0.0 } else { geometry.pill_padding_x },
        ]))
        .height(Length::Fixed(geometry.pill_height))
        .align_y(Alignment::Center)
        .style(move |theme: &iced::Theme| {
            let t = resolve_selection_tokens(
                resolve_tokens_with(pinned, geo, theme),
                pill_ink
                    .as_ref()
                    .map_or_else(TabSelectionStyle::default, |ink| ink.handle.style.get()),
                theme,
            );
            let bg = if segmented {
                // An enabled segmented cell's opaque fill is painted
                // by the *outer wrapper* (`focus_button_maybe_with` in
                // `build_row`), which has the [`FocusStatus`] the pill
                // container's style closure cannot see. The pill container
                // therefore stays transparent so the wrapper's fill (rest
                // vs hover vs pressed) is what shows.
                //
                // Disabled precedence is unchanged: a disabled cell has no
                // wrapper (`build_row` only wraps enabled cells), so the pill
                // itself paints the inert / dimmed-selected fill.
                if enabled {
                    Some(Background::Color(Color::TRANSPARENT))
                } else if selected {
                    // A disabled cell has no wrapper, so this is the
                    // fill the moving thumb would otherwise stack on. Scale it
                    // by the ink so a transition onto a disabled page shows
                    // one indicator, not two.
                    Some(Background::Color(crate::color::with_opacity(
                        t.segment_selected_bg,
                        t.segment_disabled_opacity() * selection_ink_alpha(pill_ink.as_ref()),
                    )))
                } else {
                    // An opaque disabled fill drawn after the
                    // layer overpaints the moving thumb crossing this cell.
                    // While a transition runs the layer paints this cell's
                    // resting fill itself, under the thumb.
                    Some(Background::Color(crate::color::with_alpha(
                        t.disabled_bg,
                        t.disabled_bg.a * selection_ink_alpha(pill_ink.as_ref()),
                    )))
                }
            } else if !enabled {
                // Precedence: disabled > selected > transparent. A disabled pill
                // paints the inert `disabled_bg` in every state, matching the
                // `EnzButton` segments.
                //
                // Suppressed while a transition runs, for the
                // same reason as the segmented branch above — `disabled_bg` is
                // opaque and would overpaint the thumb sliding across it.
                Some(Background::Color(crate::color::with_alpha(
                    t.disabled_bg,
                    t.disabled_bg.a * selection_ink_alpha(pill_ink.as_ref()),
                )))
            } else if selected {
                // Multiply the selected fill alpha by the ink the
                // owning `IndicatorLayer` published for this frame. `None`
                // (lone pill / no strip) and the rest value (`1.0`) leave the
                // colour untouched; `0.0` suppresses the fill so the moving
                // thumb is the only selection ink on screen.
                Some(Background::Color(crate::color::with_alpha(
                    t.selected_bg,
                    t.selected_bg.a * selection_ink_alpha(pill_ink.as_ref()),
                )))
            } else {
                None
            };
            container::Style {
                background: bg,
                border: Border {
                    color: if selected && enabled && !segmented {
                        crate::color::with_alpha(
                            t.selected_edge,
                            t.selected_edge.a * selection_ink_alpha(pill_ink.as_ref()),
                        )
                    } else {
                        Color::TRANSPARENT
                    },
                    width: if selected && enabled && !segmented && t.selected_edge.a > 0.0 {
                        1.0
                    } else {
                        0.0
                    },
                    radius: radii.unwrap_or_else(|| t.pill_radius.into()),
                },
                ..container::Style::default()
            }
        });
    pill = pill.align_y(Alignment::Center).align_x(Alignment::Center);
    if fill {
        // The painted pill claims the whole cell, and its content centres in it
        // (the stretch mode centres the icon+label in the expanded segment).
        pill = pill.width(Length::Fill).align_x(Alignment::Center);
    }

    if clickable && enabled && on_activated.is_some() {
        // Lone pill: wrap in a focus button so it is Tab-focusable + Enter/Space
        // activatable, with Role::Tab semantics. The wrapper itself paints
        // *transparent* (the inner container already draws the pill fill), and
        // only contributes a focus-visible ring on Focused — otherwise the stock
        // button catalog would double-paint an accent background over the pill.
        let semantics = Semantics::new(Role::Tab)
            .label(label.to_owned())
            .selected(selected)
            .disabled(false);
        let ring_radius = geometry.pill_radius;
        let mut fb = crate::focus_button::focus_button_maybe(pill, on_activated)
            .semantics(semantics)
            .style_focus(move |theme, status| {
                use crate::focus_button::FocusStatus;
                let t = resolve_tokens_with(pinned, geo, theme);
                // Same draw-time foreground the pill body resolves; on
                // a lone pill there is no ink handle, so this is exactly the
                // build-time role colour.
                let text_color = fg.color(theme);
                let border = if matches!(status, FocusStatus::Focused) {
                    iced::Border {
                        color: t.focus_ring,
                        width: t.focus_ring_width,
                        radius: ring_radius.into(),
                    }
                } else {
                    iced::Border {
                        color: Color::TRANSPARENT,
                        width: 0.0,
                        radius: ring_radius.into(),
                    }
                };
                iced::widget::button::Style {
                    background: None,
                    text_color,
                    border,
                    ..iced::widget::button::Style::default()
                }
            });
        // Without an id the lone pill is anonymous and release strips its
        // Click; a focused screen-reader user could reach the tab and not
        // activate it. The id comes from the caller's stable key (or the label
        // fallback), never a position.
        if let Some(id) = id {
            fb = fb.id(id);
        }
        fb.into()
    } else {
        pill.into()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

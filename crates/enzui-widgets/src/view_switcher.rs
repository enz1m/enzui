//! Host-controlled page selection through rounded pills or joined segments.
//!
//! [`enz_view_switcher`] uses a bordered pill track; [`crate::inline_view_switcher`]
//! selects the frameless segmented preset of the same engine. The host retains
//! selection and renders page content. Stable page IDs support reordering through
//! [`EnzViewSwitcher::selected_id`] and [`EnzViewSwitcher::on_select_id`].
//!
//! The strip provides one keyboard focus stop with roving tab navigation.
//! Navigation skips disabled pages and publishes selection as focus moves.
//! Supply an accessible group name with [`EnzViewSwitcher::aria_label`] and
//! distinct [`EnzViewSwitcher::tab_id_prefix`] values for strips sharing page IDs.
//! Unwired strips render and announce disabled controls.
//!
//! Pill selection clamps out-of-range indices to the last page by default;
//! the inline preset leaves them unselected. [`EnzViewSwitcher::selection_bounds`]
//! overrides this, including the result of an unknown selected ID.
//!
//! Automatic pill strips use measured label widths with per-cell and strip
//! floors. At narrow assigned widths, iconless strips first reduce padding
//! and remove per-cell floors; any remaining strip floor is shared by the
//! cells and clamped to the parent. Strips with icons use equal cells with stacked
//! labels. A starved squeezed strip also switches to equal cells. Inline
//! segments have no width floors or compact fallback. Explicit
//! [`EnzViewSwitcher::scrollable`] disables responsive branch switching and
//! enables horizontal overflow scrolling. Stretch divides the available width.
//!
//! Utility dialogs can pair `size(Sm)` with `label_size(12.0)` and
//! `label_weight(TabLabelWeight::AlwaysRegular)`. Size tiers change control
//! height; label size and weight are independent. Use a Title3 dialog heading,
//! footnote explanation, and inline metrics beside their field label.
//! Labels centre their line box; uppercase ink commonly sits 1–2 px above
//! that centre, while descenders extend below it.
//!
//! Selection-indicator motion follows the theme and snaps under reduced motion;
//! resting indicators request no frames. Geometry overrides keep colours live.
//! [`TabSelectionStyle::TranslucentAccent`] is a tinted pill with no outline,
//! its label in the accent's own ink, kept readable on the wash;
//! [`TabSelectionStyle::NeutralMaterial`] filters the backdrop when supported.
//! Inside another material, the neutral indicator uses an opaque fallback and
//! theme text ink. Place translucent controls over backdrops appropriate
//! to the current light or dark theme. Reduced transparency and increased
//! contrast restore an opaque accent wash or neutral surface.
//! [`EnzViewSwitcher::policy`] replaces tokens and selection bounds, so apply
//! explicit geometry and selection overrides afterward.
//!
//! Use [`crate::button_group`] for action groups and [`crate::settings_dialog`]
//! for a complete preferences composition.

use crate::surface::{BorderStyle, SurfaceTone, enz_surface};
pub use crate::tab_pill::TabSelectionStyle;
use crate::tab_pill::{
    TabCellFill, TabHoverShape, TabLabelWeight, TabListWidth, TabPillGeometry, TabPillItem,
    TabSelectionBounds, enz_tab_list_maybe,
};
use crate::tokens::ButtonSizeTokens;
use iced::{Element, Length, Padding};

/// Switcher geometry in logical pixels, with concentric pill radii by default.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SwitcherTokens {
    /// Track-height token used by size helpers; rendered track height derives
    /// from cell height plus track padding.
    pub strip_height: f32,
    /// Per-cell height before desktop text scaling.
    pub tab_height: f32,
    /// Track corner radius.
    pub strip_radius: f32,
    /// Selected indicator corner radius.
    pub tab_radius: f32,
    /// Inner padding around the tab row (`spacing-1`).
    pub padding: f32,
    /// Per-tab horizontal content padding (`spacing-7`).
    pub tab_padding_x: f32,
    /// Tab label size (`body-size`).
    pub label_size: f32,
    /// Minimum tab width in Automatic mode (`tab-min-width`).
    pub tab_min_width: f32,
    /// Automatic-mode strip preferred-width floor, distributed into tab cells.
    pub auto_floor: f32,
    /// Focus outline width in logical pixels; `None` (the default) follows the
    /// theme's `stroke.focus_ring`. Zero hides the ring.
    pub focus_ring_width: Option<f32>,
    /// Focus outset beyond the selected cell; default 2 logical pixels.
    /// Zero keeps the ring inside the cell bounds. Larger outsets need enough
    /// track padding and parent clearance to avoid clipping.
    pub focus_ring_outset: f32,
}

impl Default for SwitcherTokens {
    fn default() -> Self {
        Self::PILL_STRIP
    }
}

/// Alias for [`SwitcherTokens`].
pub type ViewSwitcherTokens = SwitcherTokens;

impl SwitcherTokens {
    /// Default rounded-rectangle strip geometry.
    pub const ROUNDED_STRIP: Self = Self {
        strip_height: crate::tokens::FieldSizeTokens::MD.height,
        tab_height: crate::tokens::FieldSizeTokens::MD.height - 2.0 * 2.0,
        strip_radius: 10.0,
        tab_radius: 9.0,
        padding: 2.0,
        tab_padding_x: 14.0,
        label_size: 13.0,
        tab_min_width: 84.0,
        auto_floor: 180.0,
        focus_ring_width: None,
        focus_ring_outset: 2.0,
    };

    /// Default pill-shaped track with a two-pixel indicator inset.
    pub const PILL_STRIP: Self = Self {
        strip_radius: 16.0,
        tab_radius: 14.0,
        ..Self::ROUNDED_STRIP
    };

    /// Frameless segmented geometry: 32-pixel cells, no inset or width floors,
    /// 12-pixel labels and horizontal padding, and 8-pixel outer radii.
    pub const INLINE_SEGMENTED: Self = Self {
        strip_height: crate::button_group::DEFAULT_BUTTON_HEIGHT,
        tab_height: crate::button_group::DEFAULT_BUTTON_HEIGHT,
        strip_radius: 8.0,
        tab_radius: 8.0,
        padding: 0.0,
        tab_padding_x: 12.0,
        label_size: 12.0,
        tab_min_width: 0.0,
        auto_floor: 0.0,
        focus_ring_width: None,
        focus_ring_outset: 2.0,
    };

    /// Maximum horizontal padding per side on the squeezed rung, in logical pixels.
    pub const COMPRESSED_TAB_PADDING_X: f32 = 6.0;

    /// Sanitize the geometry, cap horizontal cell padding at 6 logical pixels,
    /// and remove the per-cell width floor. Other dimensions retain sanitized values.
    #[must_use]
    pub fn compressed(&self) -> Self {
        let t = self.sanitized();
        Self {
            tab_padding_x: t.tab_padding_x.min(Self::COMPRESSED_TAB_PADDING_X),
            tab_min_width: 0.0,
            ..t
        }
    }

    /// Normalize dimensions using [`Self::ROUNDED_STRIP`] fallbacks.
    /// Label size must be positive; other dimensions allow zero.
    #[must_use]
    pub fn sanitized(&self) -> Self {
        use enzui_core::metrics::{FiniteNonNeg, FinitePositive};
        let d = Self::ROUNDED_STRIP;
        Self {
            strip_height: FiniteNonNeg::new_or(self.strip_height, d.strip_height).get(),
            tab_height: FiniteNonNeg::new_or(self.tab_height, d.tab_height).get(),
            strip_radius: FiniteNonNeg::new_or(self.strip_radius, d.strip_radius).get(),
            tab_radius: FiniteNonNeg::new_or(self.tab_radius, d.tab_radius).get(),
            padding: FiniteNonNeg::new_or(self.padding, d.padding).get(),
            tab_padding_x: FiniteNonNeg::new_or(self.tab_padding_x, d.tab_padding_x).get(),
            label_size: FinitePositive::new_or(self.label_size, d.label_size).get(),
            tab_min_width: FiniteNonNeg::new_or(self.tab_min_width, d.tab_min_width).get(),
            auto_floor: FiniteNonNeg::new_or(self.auto_floor, d.auto_floor).get(),
            focus_ring_width: self.focus_ring_width.map(|w| {
                FiniteNonNeg::new_or(w, crate::focus_button::DEFAULT_FOCUS_RING_WIDTH).get()
            }),
            focus_ring_outset: FiniteNonNeg::new_or(self.focus_ring_outset, d.focus_ring_outset)
                .get(),
        }
    }

    /// Set strip height to the field-size tier and cell height to that height
    /// minus twice the current track padding, clamped at zero. Other fields remain
    /// unchanged; this helper does not sanitize them.
    #[must_use]
    pub fn for_size(&self, size: crate::text_field::TextFieldSize) -> Self {
        let height = size.tokens().height;
        Self {
            strip_height: height,
            tab_height: (height - 2.0 * self.padding).max(0.0),
            ..*self
        }
    }

    /// Measure a natural cell width with optional icon space, in logical pixels.
    ///
    /// Uses sanitized geometry, the current desktop text scale, semibold shaped
    /// text, horizontal padding, and the per-cell floor. This helper always uses
    /// semibold, even when a custom or inline policy renders regular labels.
    #[must_use]
    pub fn tab_width(&self, label: &str, has_icon: bool) -> f32 {
        let t = self.sanitized();
        let geo = t.tab_pill_geometry();
        let typo = crate::text_measure::MeasureTypography::new(crate::enz_text::TextSize::Body)
            .pin_size_px(geo.label_size)
            // Measurement must match the pill preset's semibold label weight.
            .weight(enzui_theme::text_defaults::weight(
                enzui_theme::WeightRung::Semibold,
            ));
        let text_w = crate::text_measure::enz_text_measure_width(label, typo);
        let icon_w = if has_icon {
            geo.icon_size + geo.icon_gap
        } else {
            0.0
        };
        (2.0 * geo.pill_padding_x + icon_w + text_w).max(t.tab_min_width)
    }

    /// Sum natural cell widths plus track padding, in logical pixels.
    ///
    /// `icons` applies to every label. Include badges in the supplied text; mixed
    /// icon strips can sum [`Self::tab_width`] individually. The whole-strip
    /// automatic width floor is excluded.
    #[must_use]
    pub fn strip_width(&self, labels: &[&str], icons: bool) -> f32 {
        let t = self.sanitized();
        let cells: f32 = labels.iter().map(|l| t.tab_width(l, icons)).sum();
        cells + 2.0 * t.padding
    }

    fn tab_button_tokens(&self) -> ButtonSizeTokens {
        ButtonSizeTokens {
            height: self.tab_height,
            padding_x: self.tab_padding_x,
            text_size: self.label_size,
            icon_size: 16.0,
            radius: self.tab_radius,
            gap: 8.0,
        }
    }

    /// Express per-tab geometry for [`crate::tab_pill::enz_tab_list`], preserving
    /// live colours. Joined switcher cells have no inter-pill gap. Height,
    /// label size and icon size follow the desktop text scale; radii, padding,
    /// icon gap and focus metrics remain unscaled. Raw token values are not
    /// sanitized here.
    #[must_use]
    pub fn tab_pill_geometry(&self) -> TabPillGeometry {
        let b = self.tab_button_tokens().at_text_scale();
        TabPillGeometry {
            focus_ring_width: self.focus_ring_width,
            focus_ring_outset: self.focus_ring_outset,
            pill_height: b.height,
            pill_radius: b.radius,
            pill_padding_x: b.padding_x,
            label_size: b.text_size,
            icon_size: b.icon_size,
            icon_gap: b.gap,
            strip_gap: 0.0,
        }
    }
}

/// A stable page identity, label, optional badge and icon, and enabled state.
/// Shared by pill and inline segmented presentations.
#[derive(Debug, Clone)]
#[must_use]
pub struct SwitcherPage {
    /// Stable identifier emitted by the ID-selection callback.
    pub id: String,
    /// Visible tab label, excluding badge text.
    pub title: String,
    /// Trailing badge appended with a space; empty means no badge.
    pub badge: String,
    /// Leading icon name resolved through `enzui-icons`; empty means text only.
    pub icon: String,
    /// Whether the page is selectable (default true). Disabled pages retain
    /// selection paint but are skipped by keyboard navigation and expose
    /// disabled tab semantics.
    pub enabled: bool,
    /// Whether the page is working on something: a small spinner follows
    /// its label and the tab is reported busy (default false).
    pub busy: bool,
}

impl SwitcherPage {
    /// Build a page with a stable id and title. No badge or icon by default,
    /// and enabled.
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            badge: String::new(),
            busy: false,
            icon: String::new(),
            enabled: true,
        }
    }

    /// Builder-style icon setter.
    pub fn with_icon(mut self, name: impl Into<String>) -> Self {
        self.icon = name.into();
        self
    }

    /// Builder-style badge setter. The badge is appended to the label with a
    /// space; the accessible name gets the same combined text.
    pub fn with_badge(mut self, badge: impl Into<String>) -> Self {
        self.badge = badge.into();
        self
    }

    /// Show a spinner inside the tab, after its label, while the page works
    /// on something (an install, a sync); the tab is reported busy to
    /// assistive technology. The switcher sizes the tab for it.
    pub fn with_busy(mut self, busy: bool) -> Self {
        self.busy = busy;
        self
    }

    /// Disable this page.
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }

    /// Set this page's enabled state explicitly (chainable).
    pub fn with_enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    /// Return the title without badge text.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.title
    }

    /// Return the rendered and accessible label: title plus a space and nonempty badge.
    #[must_use]
    pub fn display_label(&self) -> String {
        if self.badge.is_empty() {
            self.title.clone()
        } else {
            format!("{} {}", self.title, self.badge)
        }
    }
}

/// Presentation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SwitcherPresentation {
    /// Tabs hug their labels (default, macOS-segmented look).
    #[default]
    Automatic,
    /// Tabs share the parent width equally.
    Stretch,
}

/// Visual presentation preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitcherStyle {
    /// Individually-rounded pills floating on a rounded, bordered track. First
    /// class navigation chrome: a toolbar or titlebar page switcher.
    PillStrip,
    /// Joined flat segments flush against each other, outer corners only, no
    /// outer frame. Sits inline in content.
    InlineSegmented,
}

/// Presentation, geometry and selection defaults for a switcher.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SwitcherPolicy {
    /// Preset identity for inspection; other fields determine rendering.
    pub style: SwitcherStyle,
    /// Add a Surface-tone track with token padding, radius and a live border.
    /// Enabled in the pill preset; disabled inline.
    pub outer_surface: bool,
    /// Cell fill geometry: rounded pills or joined segments.
    pub cell_fill: TabCellFill,
    /// Label weight in every state: semibold for pills, regular inline.
    pub label_weight: TabLabelWeight,
    /// Enable narrow-width layouts: squeezed iconless strips or equal cells
    /// with stacked icons and labels. Explicit scrolling disables switching.
    pub compact_fallback: bool,
    /// Apply cell and automatic strip width floors on the wide rung.
    /// Stretch and scrolling omit the strip floor.
    pub width_floors: bool,
    /// Default out-of-range selection policy: Clamp for pills, Unselected inline.
    /// Override through [`EnzViewSwitcher::selection_bounds`].
    pub selection_bounds: TabSelectionBounds,
    /// Geometry tokens copied when the policy is applied.
    pub tokens: SwitcherTokens,
}

impl SwitcherPolicy {
    /// The pill-strip preset: [`enz_view_switcher`].
    pub const PILL_STRIP: Self = Self {
        style: SwitcherStyle::PillStrip,
        outer_surface: true,
        cell_fill: TabCellFill::Pill,
        label_weight: TabLabelWeight::AlwaysSemibold,
        compact_fallback: true,
        width_floors: true,
        selection_bounds: TabSelectionBounds::Clamp,
        tokens: SwitcherTokens::PILL_STRIP,
    };

    /// The inline segmented preset:
    /// [`enz_inline_view_switcher`](crate::inline_view_switcher::enz_inline_view_switcher).
    pub const INLINE_SEGMENTED: Self = Self {
        style: SwitcherStyle::InlineSegmented,
        outer_surface: false,
        cell_fill: TabCellFill::Segmented,
        label_weight: TabLabelWeight::AlwaysRegular,
        compact_fallback: false,
        width_floors: false,
        selection_bounds: TabSelectionBounds::Unselected,
        tokens: SwitcherTokens::INLINE_SEGMENTED,
    };

    /// The preset for a [`SwitcherStyle`].
    #[must_use]
    pub const fn for_style(style: SwitcherStyle) -> Self {
        match style {
            SwitcherStyle::PillStrip => Self::PILL_STRIP,
            SwitcherStyle::InlineSegmented => Self::INLINE_SEGMENTED,
        }
    }
}

impl Default for SwitcherPolicy {
    fn default() -> Self {
        Self::PILL_STRIP
    }
}

pub(crate) enum Selector<'a, Message> {
    Index(Box<dyn Fn(usize) -> Message + 'a>),
    Id(Box<dyn Fn(String) -> Message + 'a>),
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rung {
    Wide,
    Squeezed,
    Compact,
}

/// Shared page-switcher builder for pill and inline segmented presets.
#[must_use]
pub struct EnzViewSwitcher<'a, Message> {
    selection_indicator: TabSelectionStyle,
    material: Option<enzui_theme::MaterialStyle>,
    hover_shape: TabHoverShape,
    pages: Vec<SwitcherPage>,
    selected_index: usize,
    selector: Selector<'a, Message>,
    policy: SwitcherPolicy,
    presentation: SwitcherPresentation,
    enabled: bool,
    tokens: SwitcherTokens,
    tab_id_prefix: Option<String>,
    selection_bounds: TabSelectionBounds,
    aria_label: Option<String>,
    scrollable: bool,
    scroll_id: Option<iced::widget::Id>,
    collapse_labels: bool,
    icons_only: bool,
    tooltip_delay: Option<std::time::Duration>,
}

impl<'a, Message: Clone + 'a> EnzViewSwitcher<'a, Message> {
    /// Override the app's hover delay for icon-only and collapsed page hints.
    pub fn tooltip_delay(mut self, delay: std::time::Duration) -> Self {
        self.tooltip_delay = Some(delay);
        self
    }

    /// Show a page's label only while it is selected: the others show their
    /// icon alone (and their name on hover), and a label slides open as its
    /// page is chosen. Pages without an icon keep their label. Off by
    /// default.
    pub fn collapse_inactive_labels(mut self, collapse: bool) -> Self {
        self.collapse_labels = collapse;
        self
    }

    /// Hide visible labels while retaining page names, tooltips and stable tab identities.
    /// Pages without an icon retain their label. Defaults to false.
    pub fn icons_only(mut self, icons_only: bool) -> Self {
        self.icons_only = icons_only;
        self
    }

    /// Choose pill hover/press geometry without changing track spacing or hit areas.
    /// Defaults to [`TabHoverShape::Joined`]; segmented policies ignore this option.
    pub fn hover_shape(mut self, shape: TabHoverShape) -> Self {
        self.hover_shape = shape;
        self
    }

    /// Return the stored policy. Later token and selection-bound overrides
    /// are stored separately and are not reflected in this value.
    #[must_use]
    pub fn policy_in_force(&self) -> SwitcherPolicy {
        self.policy
    }

    /// Replace the presentation policy, geometry tokens and selection bounds.
    /// Apply explicit `.tokens(..)`, `.size(..)`, `.button_height(..)` and
    /// `.selection_bounds(..)` overrides afterward. Custom policy tokens are
    /// copied directly; [`Self::tokens`] performs explicit normalization.
    pub fn policy(mut self, policy: SwitcherPolicy) -> Self {
        self.policy = policy;
        self.tokens = policy.tokens;
        self.selection_bounds = policy.selection_bounds;
        self
    }

    /// Apply the preset for `style`, including its tokens and selection bounds.
    /// Other builder settings remain unchanged; see [`Self::policy`].
    pub fn style(self, style: SwitcherStyle) -> Self {
        self.policy(SwitcherPolicy::for_style(style))
    }

    /// Namespace derived tab IDs. Use distinct prefixes for strips sharing page
    /// IDs on the same screen to keep accessibility routing unambiguous.
    pub fn tab_id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.tab_id_prefix = Some(prefix.into());
        self
    }

    /// Set the presentation mode (e.g. natural width vs stretch-to-fill).
    /// Defaults to the [`SwitcherPresentation`] default.
    pub fn presentation(mut self, mode: SwitcherPresentation) -> Self {
        self.presentation = mode;
        self
    }

    /// Enable or disable the whole switcher. Defaults to `true`.
    ///
    /// Individual pages carry their own [`SwitcherPage::enabled`].
    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    /// Alias for [`Self::enabled`].
    pub fn group_enabled(self, enabled: bool) -> Self {
        self.enabled(enabled)
    }

    /// Choose equal parent-width cells when true, natural-width cells otherwise.
    pub fn stretch(self, s: bool) -> Self {
        self.presentation(if s {
            SwitcherPresentation::Stretch
        } else {
            SwitcherPresentation::Automatic
        })
    }

    /// Set the strip-height token in logical pixels and cell height to that value minus
    /// twice the track padding, clamped at zero. Nonpositive or nonfinite input
    /// uses [`crate::button_group::DEFAULT_BUTTON_HEIGHT`].
    pub fn button_height(mut self, h: f32) -> Self {
        let h = enzui_core::metrics::FinitePositive::new_or(
            h,
            crate::button_group::DEFAULT_BUTTON_HEIGHT,
        )
        .get();
        self.tokens = SwitcherTokens {
            strip_height: h,
            tab_height: (h - 2.0 * self.tokens.padding).max(0.0),
            ..self.tokens
        };
        self
    }

    /// Select the first page matching a stable ID. An unknown ID stores an
    /// out-of-range index: Clamp selects the last page, while Unselected selects
    /// none. Empty page lists have no selection.
    pub fn selected_id(mut self, id: &str) -> Self {
        self.selected_index = self
            .pages
            .iter()
            .position(|p| p.id == id)
            .unwrap_or(usize::MAX);
        self
    }

    /// Resolve a positional index emitted by the selection callback back to
    /// the page's stable `id`. `None` for an out-of-range index.
    #[must_use]
    pub fn id_at(&self, index: usize) -> Option<&str> {
        self.pages.get(index).map(|p| p.id.as_str())
    }

    /// Return the page ID selected under the current bounds policy.
    ///
    /// An empty list yields `None`; out-of-range selection clamps to the last
    /// page under Clamp or yields `None` under Unselected. Disabled pages can
    /// remain selected.
    #[must_use]
    pub fn active_id(&self) -> Option<&str> {
        self.effective_selected()
            .and_then(|i| self.pages.get(i))
            .map(|p| p.id.as_str())
    }

    fn effective_selected(&self) -> Option<usize> {
        if self.pages.is_empty() {
            return None;
        }
        match self.selection_bounds {
            TabSelectionBounds::Clamp => Some(self.selected_index.min(self.pages.len() - 1)),
            TabSelectionBounds::Unselected => {
                (self.selected_index < self.pages.len()).then_some(self.selected_index)
            }
        }
    }

    /// Replace geometry with sanitized tokens; the pill preset defaults to
    /// [`SwitcherTokens::PILL_STRIP`]. Colours remain resolved from the theme.
    pub fn tokens(mut self, tokens: SwitcherTokens) -> Self {
        self.tokens = tokens.sanitized();
        self
    }

    /// Retune current tokens to the field-size tier, preserving radii and widths.
    /// A later [`Self::tokens`] or [`Self::policy`] call replaces this geometry.
    pub fn size(mut self, size: crate::text_field::TextFieldSize) -> Self {
        self.tokens = self.tokens.for_size(size).sanitized();
        self
    }

    /// Set label size in logical pixels before desktop text scaling.
    /// Invalid values use 13 px. Cell height and icon size are unchanged;
    /// later token or policy overrides replace this setting.
    pub fn label_size(mut self, size: f32) -> Self {
        self.tokens.label_size = enzui_core::metrics::FinitePositive::new_or(size, 13.0).get();
        self
    }

    /// Choose regular, semibold, or selected-only semibold labels.
    /// The pill preset uses semibold in every state; a later policy replaces
    /// this setting. Pair regular labels at 12 px with `size(Sm)` for utility dialogs.
    pub fn label_weight(mut self, weight: TabLabelWeight) -> Self {
        self.policy.label_weight = weight;
        self
    }

    /// Set out-of-range selection behavior, including unknown selected IDs.
    /// Pills default to Clamp; inline segments default to Unselected.
    pub fn selection_bounds(mut self, bounds: TabSelectionBounds) -> Self {
        self.selection_bounds = bounds;
        self
    }

    /// Wrap the strip in a horizontal scroll viewport, so an overflowing strip
    /// is reachable by wheel or drag instead of clipped (default off, since a
    /// viewport adds a clip boundary and focus-driven scrolling). Settings
    /// dialogs turn it on. Pair with [`scroll_id`](Self::scroll_id) to drive
    /// or probe the viewport.
    pub fn scrollable(mut self, on: bool) -> Self {
        self.scrollable = on;
        self
    }

    /// Set the inner horizontal scroll viewport ID for scroll operations.
    /// Only used when [`Self::scrollable`] is enabled.
    pub fn scroll_id(mut self, id: iced::widget::Id) -> Self {
        self.scroll_id = Some(id);
        self
    }

    /// Set the accessible name for the TabList group.
    /// Omission can trigger a debug assertion during semantics collection when
    /// [`crate::tab_pill::TAB_LIST_LABEL_ASSERT_ENV`] or
    /// [`crate::tab_pill::with_tab_list_label_assert_armed`] enables the check.
    pub fn aria_label(mut self, label: impl Into<String>) -> Self {
        self.aria_label = Some(label.into());
        self
    }

    /// Switch to stable-id selection: the emitted message carries the selected
    /// [`SwitcherPage::id`] instead of its index, so it is reorder-safe.
    pub fn on_select_id<F>(mut self, on_select_id: F) -> Self
    where
        F: Fn(String) -> Message + 'a,
    {
        self.selector = Selector::Id(Box::new(on_select_id));
        self
    }

    /// Replace selection reporting with an optional stable-ID callback.
    /// `None` renders and announces disabled controls; a later callback restores
    /// reporting subject to the group and per-page enabled flags.
    pub fn on_select_id_maybe(
        mut self,
        on_select_id: Option<Box<dyn Fn(String) -> Message + 'a>>,
    ) -> Self {
        self.selector = match on_select_id {
            Some(f) => Selector::Id(f),
            None => Selector::None,
        };
        self
    }

    /// Choose theme-aware selected-indicator paint; filled accent is the default.
    /// Translucent accent exposes an enclosing glass surface. Neutral material
    /// filters its backdrop when supported and uses an opaque fallback when nested.
    pub fn selection_indicator(mut self, style: TabSelectionStyle) -> Self {
        self.selection_indicator = style;
        self
    }

    /// Put the switcher's track on liquid glass of `style` (see [`crate::glass`]).
    /// Controls inside default to glass.
    pub fn material(mut self, style: enzui_theme::MaterialStyle) -> Self {
        self.material = Some(style);
        self
    }

    /// Consume the builder into a switcher using the current policy and overrides.
    pub fn into_element(self) -> Element<'a, Message> {
        let mut policy = self.policy;
        if self.icons_only {
            policy.compact_fallback = false;
        }
        let material = self.material;
        let stretch = matches!(self.presentation, SwitcherPresentation::Stretch);
        let t = self.tokens;
        let live = self.enabled && !matches!(self.selector, Selector::None);

        // Collapsed labels make cells hug their icon: no per-cell floors.
        let floors = policy.width_floors && !self.collapse_labels && !self.icons_only;
        let track_cell_floor = if floors && !stretch && !self.scrollable && !self.pages.is_empty() {
            (t.auto_floor - 2.0 * t.padding).max(0.0) / self.pages.len() as f32
        } else {
            0.0
        };
        let cell_floor = if self.collapse_labels || self.icons_only {
            0.0
        } else {
            t.tab_min_width.max(track_cell_floor)
        };
        let required_width = if policy.compact_fallback {
            self.pages
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let collapsed = !p.icon.is_empty()
                        && (self.icons_only || (self.collapse_labels && i != self.selected_index));
                    let label = if collapsed {
                        String::new()
                    } else {
                        p.display_label()
                    };
                    (t.tab_width(&label, !p.icon.is_empty()) + {
                        let geo = t.tab_pill_geometry();
                        crate::tab_pill::busy_width(p.busy, geo.icon_size, geo.icon_gap)
                    })
                    .max(cell_floor)
                })
                .sum::<f32>()
                + 2.0 * t.padding
        } else {
            0.0
        };
        let has_icons = self.pages.iter().any(|p| !p.icon.is_empty());

        let items: Vec<TabPillItem> = self
            .pages
            .iter()
            .map(|p| {
                // The badge is drawn as one, after the title; it is still
                // announced after the name.
                let label = if self.icons_only && !p.icon.is_empty() {
                    String::new()
                } else {
                    p.title.clone()
                };
                let mut item = TabPillItem::new(label)
                    .aria_label(p.title.clone())
                    .badge(p.badge.clone())
                    .key(p.id.clone())
                    .enabled(p.enabled)
                    .busy(p.busy);
                if let Some(delay) = self.tooltip_delay {
                    item = item.tooltip_delay(delay);
                }
                if !p.icon.is_empty() {
                    item = item.icon(p.icon.clone());
                }
                item
            })
            .collect();
        let ids: Vec<String> = self.pages.iter().map(|p| p.id.clone()).collect();
        let on_select: Option<std::rc::Rc<dyn Fn(usize) -> Message + 'a>> = match self.selector {
            Selector::Index(f) => Some(std::rc::Rc::from(f)),
            Selector::Id(f) => Some(std::rc::Rc::new(move |i: usize| {
                f(ids.get(i).cloned().unwrap_or_default())
            })),
            Selector::None => None,
        };

        let callback = |source: &Option<std::rc::Rc<dyn Fn(usize) -> Message + 'a>>| {
            source.as_ref().map(|f| {
                let f = std::rc::Rc::clone(f);
                Box::new(move |i| f(i)) as Box<dyn Fn(usize) -> Message + 'a>
            })
        };

        let mut compact_geo = t.tab_pill_geometry();
        if has_icons {
            compact_geo.icon_gap = 2.0;
            compact_geo.pill_height =
                (compact_geo.icon_size + compact_geo.icon_gap + compact_geo.label_size * 1.2 + 4.0)
                    .ceil();
        }

        let squeezed_tokens = t.compressed();

        let squeezed_cells: f32 = self
            .pages
            .iter()
            .map(|p| {
                squeezed_tokens.tab_width(&p.display_label(), !p.icon.is_empty()) + {
                    let geo = squeezed_tokens.tab_pill_geometry();
                    crate::tab_pill::busy_width(p.busy, geo.icon_size, geo.icon_gap)
                }
            })
            .sum();
        let squeezed_gaps = squeezed_tokens.tab_pill_geometry().strip_gap
            * (self.pages.len().saturating_sub(1) as f32);
        let squeezed_row_width = squeezed_cells + squeezed_gaps;
        let squeezed_required = squeezed_cells + 2.0 * t.padding;
        let squeezes = squeezed_required + crate::bounds_probe::SPILL_EPSILON < required_width;

        let make_list = |rung: Rung,
                         squeezed_fill: bool,
                         on_select: Option<Box<dyn Fn(usize) -> Message + 'a>>|
         -> Element<'a, Message> {
            let compact = matches!(rung, Rung::Compact);
            let mut list = enz_tab_list_maybe(items.clone(), self.selected_index, on_select)
                .geometry(match rung {
                    Rung::Compact => compact_geo,
                    Rung::Squeezed => squeezed_tokens.tab_pill_geometry(),
                    Rung::Wide => t.tab_pill_geometry(),
                })
                .selection_indicator(self.selection_indicator)
                .cell_fill(policy.cell_fill)
                .hover_shape(self.hover_shape)
                .equal_cells_below(match rung {
                    // Below the natural sum, equal tracks preserve each page's hit area.
                    Rung::Squeezed => Some(squeezed_row_width),
                    _ => None,
                })
                .label_weight(policy.label_weight)
                .selection_bounds(self.selection_bounds)
                .collapse_inactive_labels(self.collapse_labels)
                .enabled(self.enabled);
            if let Some(prefix) = self.tab_id_prefix.clone() {
                list = list.tab_id_prefix(prefix);
            }
            if let Some(label) = self.aria_label.clone() {
                list = list.aria_label(label);
            }
            if compact {
                list = list
                    .width_policy(TabListWidth::Fill)
                    .label_layout(crate::tab_pill::TabLabelLayout::Stacked)
                    .compress_fill_cells(true);
            } else if squeezed_fill {
                list = list
                    .width_policy(TabListWidth::Fill)
                    .compress_fill_cells(true)
                    .equal_cells_below(None);
            } else if stretch {
                list = list.width_policy(TabListWidth::Fill);
            } else if floors && matches!(rung, Rung::Wide) {
                list = list.cell_min_width(cell_floor);
            }
            if self.scrollable {
                list = list.scrollable(true).scroll_strip_height(if compact {
                    compact_geo.pill_height
                } else {
                    t.tab_height
                });
                if let Some(id) = self.scroll_id.clone() {
                    list = list.scroll_id(id);
                }
            }
            list.into()
        };

        // Glass provides its own rim.
        let border = if live && material.is_none() {
            BorderStyle::Thin
        } else {
            BorderStyle::None
        };
        let make_strip = |row: Element<'a, Message>, width: Length, height: f32| {
            if !policy.outer_surface {
                return crate::glass::wrap(row, material, t.strip_radius);
            }
            let strip = enz_surface(
                iced::widget::container(row)
                    .width(width)
                    .height(Length::Fixed(height)),
            )
            .width(width)
            .tone(SurfaceTone::Surface)
            .border_style(border)
            .radius(t.strip_radius)
            .padding(Padding::from(t.padding));
            let strip: Element<'a, Message> = match material {
                Some(style) => strip
                    .material(style)
                    .material_ink(enzui_theme::MaterialInk::Strong)
                    .into_element(),
                None => strip.into_element(),
            };
            if policy.width_floors && !stretch && !self.scrollable && self.pages.is_empty() {
                // Empty tracks have no cells to receive the minimum width.
                crate::min_width::min_width(strip, t.auto_floor).into()
            } else {
                strip
            }
        };

        let wide_width = if stretch {
            Length::Fill
        } else {
            Length::Shrink
        };
        let wide = make_strip(
            make_list(Rung::Wide, false, callback(&on_select)),
            wide_width,
            t.tab_height,
        );
        if !policy.compact_fallback {
            return wide;
        }

        let compressing = squeezes && !has_icons;
        let narrow: Element<'a, Message> = if compressing {
            let floor_binds = track_cell_floor * self.pages.len() as f32 > squeezed_row_width;
            make_strip(
                make_list(Rung::Squeezed, floor_binds, callback(&on_select)),
                if floor_binds {
                    Length::Fixed(t.auto_floor)
                } else {
                    wide_width
                },
                t.tab_height,
            )
        } else {
            make_strip(
                make_list(Rung::Compact, false, callback(&on_select)),
                Length::Fill,
                compact_geo.pill_height,
            )
        };

        let probe: Element<'a, Message> = iced::widget::Space::new()
            .width(Length::Fixed(required_width))
            .height(Length::Shrink)
            .into();
        let responsive: Element<'a, Message> = crate::responsive_switch::ResponsiveSwitch::new(
            wide,
            narrow,
            probe,
            wide_width,
            // Explicit scrolling preserves the wide overflow layout.
            !items.is_empty() && !self.scrollable,
            // Band pressure must be read during layout; builders cannot see the sibling deficit.
            move |_| {
                if crate::title_bar::band_deficit() > 0.0 {
                    f32::INFINITY
                } else {
                    // Compensate ResponsiveSwitch's tolerance to keep the public threshold exact.
                    required_width + 0.01
                }
            },
        )
        .into();

        responsive
    }
}

impl<'a, Message: Clone + 'a> From<EnzViewSwitcher<'a, Message>> for Element<'a, Message> {
    fn from(s: EnzViewSwitcher<'a, Message>) -> Self {
        s.into_element()
    }
}

/// Construct a view switcher (index-based selection).
pub fn enz_view_switcher<'a, Message: Clone + 'a, F>(
    pages: Vec<SwitcherPage>,
    selected_index: usize,
    on_select: F,
) -> EnzViewSwitcher<'a, Message>
where
    F: Fn(usize) -> Message + 'a,
{
    switcher_with(
        pages,
        selected_index,
        Selector::Index(Box::new(on_select)),
        SwitcherPolicy::PILL_STRIP,
    )
}

/// Construct a disabled pill switcher without a selection callback.
/// Attach one later through [`EnzViewSwitcher::on_select_id`] or
/// [`EnzViewSwitcher::on_select_id_maybe`].
pub fn enz_view_switcher_unwired<'a, Message: Clone + 'a>(
    pages: Vec<SwitcherPage>,
    selected_index: usize,
) -> EnzViewSwitcher<'a, Message> {
    switcher_with(
        pages,
        selected_index,
        Selector::None,
        SwitcherPolicy::PILL_STRIP,
    )
}

pub(crate) fn switcher_with<'a, Message: Clone + 'a>(
    pages: Vec<SwitcherPage>,
    selected_index: usize,
    selector: Selector<'a, Message>,
    policy: SwitcherPolicy,
) -> EnzViewSwitcher<'a, Message> {
    EnzViewSwitcher {
        selection_indicator: TabSelectionStyle::default(),
        material: None,
        hover_shape: TabHoverShape::default(),
        pages,
        selected_index,
        selector,
        policy,
        presentation: SwitcherPresentation::default(),
        enabled: true,
        tokens: policy.tokens,
        tab_id_prefix: None,
        selection_bounds: policy.selection_bounds,
        aria_label: None,
        scrollable: false,
        scroll_id: None,
        collapse_labels: false,
        icons_only: false,
        tooltip_delay: None,
    }
}

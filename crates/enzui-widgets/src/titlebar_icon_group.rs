//! EnzTitlebarIconGroup: the container for a titlebar icon cluster — grouping
//! rhythm, centre alignment and a rounded clip mask.
//!
//! Wrap [`crate::toolbar_icon`] children in this, and wrap the bare row
//! returned by [`crate::titlebar_actions`] in it too. See `# Composing a
//! titlebar` in [`crate::title_bar`] for how the five titlebar widgets stack
//!
//!
//! Superset of `titlebar_group.rs` (which covers only the flat `ghost`
//! variant). Four variants:
//!
//! - `Ghost`     — transparent, no border, no padding. Same as
//!   `enz_titlebar_group`. Kept as the default here.
//! - `Pill`      — `accent-soft` fill, full-pill radius (`height / 2`).
//! - `Island`    — `surface-elevated` fill, `RadiusScale::lg` radius (10 px on
//!   the default scale, theme-responsive), hairline border.
//! - `Segmented` — `surface_hover` fill, full-pill radius, zero
//!   inter-child spacing (children draw their own dividers).
//!
//! # Elevation shadow
//!
//! The `Island` variant floats on the titlebar, so it casts a shadow at
//! [`TITLEBAR_ISLAND_ELEVATION`], resolved at draw time from the live theme
//! through [`crate::elevation::iced_shadow_for_theme`]. The other variants are
//! flat and paint no shadow.

use enzui_theme::{Palette, SpacingScale};
use iced::widget::{Row, container, row};
use iced::{Alignment, Color, Element, Length, Shadow};

/// Default group height in pixels.
///
/// The default group height: `max(button height - 4, field md height)` =
/// `max(28, 32)` = 32. The builder and the [`crate::titlebar_group`] alias
/// both use it.
pub const DEFAULT_GROUP_HEIGHT: f32 = 32.0;

/// The `Island` variant's elevation: level 2 (blur 8, y offset 2), below a
/// popover menu's 3, since a persistent titlebar cluster floats less than a
/// transient popover.
pub const TITLEBAR_ISLAND_ELEVATION: u8 = 2;

/// Visual variant of a titlebar icon cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TitlebarIconGroupVariant {
    /// Transparent, no border, no padding (the default).
    #[default]
    Ghost,
    /// `accent_soft` fill with a full-pill radius (`height / 2`).
    Pill,
    /// `surface_elevated` fill, medium radius, hairline border.
    Island,
    /// `surface_hover` fill, full-pill radius, zero inter-child spacing
    /// (children draw their own dividers).
    Segmented,
}

/// Builder for a variant-driven titlebar icon cluster. See
/// [`enz_titlebar_icon_group`].
#[must_use]
pub struct EnzTitlebarIconGroup<'a, Message> {
    children: Vec<Element<'a, Message>>,
    variant: TitlebarIconGroupVariant,
    /// Resolved group height in px. `pub(crate)` so the alias parity guard
    /// in `titlebar_group.rs` can read it without going through layout.
    pub(crate) group_height: f32,
    group_spacing: Option<f32>,
    padding_x: Option<f32>,
    radius: Option<f32>,
    background: Option<Color>,
    border_color: Option<Color>,
    border_width: Option<f32>,
}

/// Construct a titlebar icon group wrapping `children`. Defaults:
/// [`TitlebarIconGroupVariant::Ghost`], 32px height, all spacing/padding/
/// radius/colour derived from the variant unless overridden.
pub fn enz_titlebar_icon_group<'a, Message>(
    children: Vec<Element<'a, Message>>,
) -> EnzTitlebarIconGroup<'a, Message> {
    EnzTitlebarIconGroup {
        children,
        variant: TitlebarIconGroupVariant::Ghost,
        group_height: DEFAULT_GROUP_HEIGHT,
        group_spacing: None,
        padding_x: None,
        radius: None,
        background: None,
        border_color: None,
        border_width: None,
    }
}

impl<'a, Message: 'a> EnzTitlebarIconGroup<'a, Message> {
    /// Set the visual variant. Defaults to [`TitlebarIconGroupVariant::Ghost`].
    pub fn variant(mut self, v: TitlebarIconGroupVariant) -> Self {
        self.variant = v;
        self
    }
    /// Group height in px. Non-finite or non-positive input falls back to
    /// [`DEFAULT_GROUP_HEIGHT`].
    pub fn group_height(mut self, h: f32) -> Self {
        self.group_height =
            enzui_core::metrics::FinitePositive::new_or(h, DEFAULT_GROUP_HEIGHT).get();
        self
    }
    /// Override the gap between children in px. Unset by default (derived from
    /// the variant: 8 for most, 0 for `Segmented`).
    ///
    /// The gap feeds `Row::spacing`. `0` is expressible intent ("no
    /// gap") and is kept; a non-finite or negative override maps to unset
    /// so it falls back to the variant default rather than reaching layout as
    /// junk.
    pub fn group_spacing(mut self, s: f32) -> Self {
        self.group_spacing =
            enzui_core::metrics::FiniteNonNeg::new(s).map(enzui_core::metrics::FiniteNonNeg::get);
        self
    }
    /// Override the horizontal padding in px. Normalized to finite
    /// non-negative; non-finite input falls back to `0`.
    pub fn padding_x(mut self, p: f32) -> Self {
        self.padding_x = Some(enzui_core::metrics::FiniteNonNeg::new_or(p, 0.0).get());
        self
    }
    /// Override the corner radius in px. Normalized to finite non-negative
    /// so a non-finite radius never reaches the renderer.
    pub fn radius(mut self, r: f32) -> Self {
        self.radius = Some(enzui_core::metrics::FiniteNonNeg::new_or(r, 0.0).get());
        self
    }
    /// Override the fill. Unset by default (derived from the variant/theme).
    pub fn background(mut self, c: Color) -> Self {
        self.background = Some(c);
        self
    }
    /// Override the border colour. Unset by default (derived from the variant/theme).
    pub fn border_color(mut self, c: Color) -> Self {
        self.border_color = Some(c);
        self
    }
    /// Override the border width in px. Unset by default (derived from the variant).
    ///
    /// The width feeds `Border.width`. `0` is expressible ("no border")
    /// and is kept; a non-finite or negative override maps to unset so it
    /// falls back to the variant default rather than reaching the renderer.
    pub fn border_width(mut self, w: f32) -> Self {
        self.border_width =
            enzui_core::metrics::FiniteNonNeg::new(w).map(enzui_core::metrics::FiniteNonNeg::get);
        self
    }

    fn default_spacing(&self) -> f32 {
        // Anchored to `SpaceToken::Sm` (spacing-sm = 8 px on the default
        // scale). Resolved against the default `SpacingScale` because
        // Row spacing is a layout-time metric that predates the style
        // closure (no `&Theme` available here).
        match self.variant {
            TitlebarIconGroupVariant::Segmented => 0.0,
            _ => SpacingScale::default().sm(),
        }
    }
    fn default_padding_x(&self) -> f32 {
        // Same token as spacing: `SpaceToken::Sm` (8 px). Ghost gets no
        // padding; every other variant insets its children by one `sm` step.
        match self.variant {
            TitlebarIconGroupVariant::Ghost => 0.0,
            _ => SpacingScale::default().sm(),
        }
    }
    /// Compute the variant's default radius.
    ///
    /// `Pill` and `Segmented` use a full-pill radius (`height / 2`) that
    /// is geometry-driven and independent of the radius scale.
    ///
    /// `Island` anchors to `RadiusScale::lg` — the "cards / panels" tier (10 px,
    /// (8 px on the default scale). This is the same tier `EnzCard` uses
    /// and matches the floating-surface family: a persistent chrome
    /// cluster on the titlebar, not a transient popover (`EnzMenu`
    /// carries a larger radius from the `radius-lg = 10` legacy).
    /// Resolved from the runtime theme inside the style closure so a
    /// custom theme that rescales its radius ladder flows through.
    ///
    /// `Ghost` is unstyled → 0.
    fn default_radius_for_theme(
        variant: TitlebarIconGroupVariant,
        height: f32,
        theme: &iced::Theme,
    ) -> f32 {
        match variant {
            TitlebarIconGroupVariant::Pill | TitlebarIconGroupVariant::Segmented => height / 2.0,
            TitlebarIconGroupVariant::Island => crate::theme_lookup::radius(theme).lg,
            TitlebarIconGroupVariant::Ghost => 0.0,
        }
    }
    // Take the variant by value rather than `&self` so the style closure can
    // call these per render against the *runtime* palette — `self` is
    // moved into the element by then.
    fn default_background(variant: TitlebarIconGroupVariant, p: &Palette) -> Color {
        match variant {
            TitlebarIconGroupVariant::Pill => p.accent_soft,
            TitlebarIconGroupVariant::Island => p.surface_elevated,
            TitlebarIconGroupVariant::Segmented => p.surface_hover,
            TitlebarIconGroupVariant::Ghost => Color::TRANSPARENT,
        }
    }
    fn default_border_color(variant: TitlebarIconGroupVariant, p: &Palette) -> Color {
        match variant {
            TitlebarIconGroupVariant::Island => p.border,
            _ => Color::TRANSPARENT,
        }
    }
    fn default_border_width(&self) -> f32 {
        match self.variant {
            TitlebarIconGroupVariant::Island => 1.0,
            _ => 0.0,
        }
    }

    /// Consume the builder and produce the widget tree.
    pub fn into_element(self) -> Element<'a, Message> {
        let spacing = self.group_spacing.unwrap_or_else(|| self.default_spacing());
        let padding_x = self.padding_x.unwrap_or_else(|| self.default_padding_x());
        // Radius: if the caller pinned an explicit value, capture it; otherwise
        // resolve the variant default from the runtime theme inside the style
        // closure so a custom `RadiusScale` flows through.
        let radius_pin = self.radius;
        let variant = self.variant;
        let bg_pin = self.background;
        let border_pin = self.border_color;
        let border_width = self
            .border_width
            .unwrap_or_else(|| self.default_border_width());
        let height = self.group_height;

        let mut row: Row<'a, Message> = row![].spacing(spacing).align_y(Alignment::Center);
        for c in self.children {
            row = row.push(c);
        }

        // Mask the children to the group's rounded outline.
        //
        // `Container::clip(true)` (kept below) is rectangular, and this widget
        // is the crate's sharpest case for that mattering: `Pill` and
        // `Segmented` paint a full-pill radius (`height / 2` = 16 px at the
        // default 32 px height) while insetting children by `sm` = 8 px
        // horizontally and by nothing vertically. An 8 px inset cannot
        // clear a 16 px arc, and on the vertical axis a full-height child is
        // flush with the edge by construction.
        //
        // Measured without the clip, at scale 4 with a full-height child
        // (`tests/rounded_clip_widget_sweep.rs`):
        //   Pill (r=16):      212/3024 cutout px
        //   Segmented (r=16): 212/3024
        //   Island (r=8):     0/648    (8 px inset exactly clears an 8 px arc)
        //   Ghost (r=0):      0/0      (square)
        // Now 0 for all four.
        //
        // The radius is resolved from the runtime theme inside the style
        // closure (`Island` reads `theme_lookup::radius(theme).lg`), so
        // the mask must be too — a build-time pin would drift from the painted
        // outline under a custom `RadiusScale`. `radius_resolved` applies the
        // identical `pin.unwrap_or_else(default)` precedence the style closure
        // does, via the same `painted_radius` helper a test can assert.
        // Event dispatch has no `&Theme`, so seed RoundedClip with the default
        // painted radius rather than zero. Its draw pass then caches the exact
        // runtime-resolved radius for subsequent hit tests.
        let baseline_radius = radius_pin.unwrap_or_else(|| match variant {
            TitlebarIconGroupVariant::Pill | TitlebarIconGroupVariant::Segmented => height / 2.0,
            TitlebarIconGroupVariant::Island => enzui_theme::RadiusScale::default().lg,
            TitlebarIconGroupVariant::Ghost => 0.0,
        });
        let masked: Element<'a, Message> =
            crate::rounded_clip::rounded_clip(row, iced::border::Radius::new(baseline_radius))
                .radius_resolved(move |theme: &iced::Theme| {
                    iced::border::Radius::new(
                        radius_pin.unwrap_or_else(|| {
                            Self::default_radius_for_theme(variant, height, theme)
                        }),
                    )
                })
                .outset(iced::Padding::from([0.0, padding_x]))
                .into();

        container(masked)
            .height(Length::Fixed(height))
            .padding([0, enzui_core::metrics::to_u16_saturating(padding_x)])
            // Use align_y (not center_y): center_y(Fill) would reset the height
            // back to Fill and clobber the fixed group height, letting an
            // oversized child grow the group and defeating clip(true).
            .align_y(iced::alignment::Vertical::Center)
            .clip(true)
            .style(move |theme: &iced::Theme| {
                let p = crate::theme_lookup::palette(theme);
                let bg = bg_pin.unwrap_or_else(|| Self::default_background(variant, &p));
                let border_color =
                    border_pin.unwrap_or_else(|| Self::default_border_color(variant, &p));
                let radius = radius_pin
                    .unwrap_or_else(|| Self::default_radius_for_theme(variant, height, theme));
                iced::widget::container::Style {
                    background: Some(bg.into()),
                    border: iced::Border {
                        color: border_color,
                        width: border_width,
                        radius: radius.into(),
                    },
                    // Only `Island` carries an elevation drop-shadow (a
                    // floating chrome cluster on the titlebar surface). The
                    // other three variants are flat and paint
                    // `Shadow::default()` — no shadow at all, never a
                    // zero-blur shadow. `shadow_for` is the single resolver
                    // both the paint site and the accessor read, so a
                    // constant pinned here cannot escape the tests ; a constant pinned here cannot escape the tests.
                    shadow: shadow_for(theme, variant),
                    ..Default::default()
                }
            })
            .into()
    }
}

impl<'a, Message: 'a> From<EnzTitlebarIconGroup<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzTitlebarIconGroup<'a, Message>) -> Self {
        w.into_element()
    }
}

/// The shadow this widget paints for `variant` under `theme`.
///
/// Single source of truth for the elevation shadow — the render closure above
/// and [`resolved_shadow`] both call this. Two places computing the same thing
/// is exactly the trap that let the contract's shadow tests all pass while the paint
/// site was pinned to a constant. `Island` -> tier resolution via
/// [`crate::elevation::iced_shadow_for_theme`]; every other variant is flat
/// and returns [`Shadow::default`].
fn shadow_for(theme: &iced::Theme, variant: TitlebarIconGroupVariant) -> Shadow {
    match variant {
        TitlebarIconGroupVariant::Island => {
            crate::elevation::iced_shadow_for_theme(theme, TITLEBAR_ISLAND_ELEVATION)
        }
        _ => Shadow::default(),
    }
}

/// The [`Shadow`] this widget paints for `variant` under `theme`. Exposed so
/// a test can assert the *resolved* shadow rather than a stored field — and
/// so the assertion reads the same function the render closure does.
pub fn resolved_shadow(theme: &iced::Theme, variant: TitlebarIconGroupVariant) -> Shadow {
    shadow_for(theme, variant)
}

/// The default corner radius this widget paints for `variant` / `height`
/// under `theme`.
///
/// Reads the same `EnzTitlebarIconGroup::default_radius_for_theme` the
/// render closure calls, so a test asserting this value proves the paint
/// site. Anchored to `RadiusScale::lg` for `Island`.
pub fn resolved_radius(theme: &iced::Theme, variant: TitlebarIconGroupVariant, height: f32) -> f32 {
    EnzTitlebarIconGroup::<()>::default_radius_for_theme(variant, height, theme)
}

impl<'a, Message: 'a> EnzTitlebarIconGroup<'a, Message> {
    /// The corner radius this builder will actually paint under `theme`,
    /// including an explicit [`Self::radius`] pin.
    ///
    /// [`resolved_radius`] covers only the variant *default*, so a test built
    /// on it cannot tell a working `.radius(n)` override from one that is
    /// stored and then dropped on the floor — asserting `self.radius ==
    /// Some(n)` proves the setter, not the paint. This helper applies the same
    /// `pin.unwrap_or_else(default)` precedence the style closure applies, so
    /// asserting it exercises the override path end to end.
    pub fn painted_radius(&self, theme: &iced::Theme) -> f32 {
        self.radius.unwrap_or_else(|| {
            Self::default_radius_for_theme(self.variant, self.group_height, theme)
        })
    }
}

/// The default inter-child spacing for `variant`, resolved at layout time from
/// `SpacingScale::default().sm()` for non-Segmented variants.
pub fn resolved_spacing(variant: TitlebarIconGroupVariant) -> f32 {
    let dummy: EnzTitlebarIconGroup<'_, ()> = enz_titlebar_icon_group(vec![]).variant(variant);
    dummy.default_spacing()
}

/// The default horizontal padding for `variant`, resolved at layout time from
/// `SpacingScale::default().sm()` for non-Ghost variants.
pub fn resolved_padding_x(variant: TitlebarIconGroupVariant) -> f32 {
    let dummy: EnzTitlebarIconGroup<'_, ()> = enz_titlebar_icon_group(vec![]).variant(variant);
    dummy.default_padding_x()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

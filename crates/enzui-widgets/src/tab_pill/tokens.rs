// ---------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------

/// Colours, state opacity and geometry shared by tab pills and strips.
#[derive(Debug, Clone, Copy)]
pub struct TabPillTokens {
    /// Selected-pill background.
    pub selected_bg: Color,
    /// Selected-cell edge, providing the state cue on brighter titlebars.
    pub selected_edge: Color,
    /// Selected-pill label + icon colour.
    pub selected_fg: Color,
    /// Unselected-pill label + icon colour.
    pub unselected_fg: Color,
    /// Disabled-pill label + icon colour.
    pub disabled_fg: Color,
    /// Disabled-cell fill.
    pub disabled_bg: Color,
    /// Opacity of a selected disabled segment; defaults to `0.55`.
    /// Multiplies the selected fill and foreground alpha, leaving unselected
    /// disabled colours unchanged. Finite values are clamped to `[0, 1]`;
    /// non-finite values use the default.
    pub segment_disabled_alpha: f32,
    /// Unselected segmented-cell fill; dark defaults use the page background.
    pub segment_bg: Color,
    /// Selected accent fill, resolved for at least 3:1 contrast against the track.
    pub segment_selected_bg: Color,
    /// Selected hover fill; dark palettes deepen the base and light palettes darken it.
    pub segment_selected_hover_bg: Color,
    /// Selected pressed fill, with a stronger interaction step.
    pub segment_selected_pressed_bg: Color,
    /// Hovered fill of the enabled, unselected cell in
    /// [`TabCellFill::Segmented`] mode — the segment track's own hover step,
    /// composited opaquely so a segmented cell hover is actually painted
    /// (a wrapper wash drawn behind the pill's opaque fill would never be
    /// visible).
    pub segment_hover_bg: Color,
    /// Pressed fill of the enabled, unselected cell in
    /// [`TabCellFill::Segmented`] mode.
    pub segment_pressed_bg: Color,
    /// Label and icon ink paired with the resolved selected segment fill.
    pub segment_selected_fg: Color,
    /// Subtle divider between adjacent segmented cells.
    pub seam: Color,
    /// Hover-overlay background for the unselected pill.
    pub hover_overlay: Color,
    /// Focus-visible ring colour for single pills and strips.
    pub focus_ring: Color,
    /// Focus outline width in logical pixels; default 2. Zero hides the ring.
    pub focus_ring_width: f32,
    /// Composite-strip focus outset in logical pixels; default 2. Single pills
    /// use their own bounds. Zero keeps the strip ring inside the cell bounds.
    pub focus_ring_outset: f32,
    /// Pill height (28 px, `button-height - 4px`).
    pub pill_height: f32,
    /// Pill corner radius when pinned (9 px); unpinned pills follow the
    /// theme's `shape.button` step.
    pub pill_radius: f32,
    /// Horizontal padding inside a pill (14 px).
    pub pill_padding_x: f32,
    /// Label font size (13 px, `text-size-md`).
    pub label_size: f32,
    /// Leading-icon size (16 px).
    pub icon_size: f32,
    /// Gap between a leading icon and the label (8 px).
    pub icon_gap: f32,
    /// Inter-pill gap in a strip (4 px).
    pub strip_gap: f32,
}

impl TabPillTokens {
    /// Derive tab-pill tokens from a palette. Colour fields track the runtime
    /// theme when resolved via [`crate::theme_lookup::resolve`]; geometry is
    /// theme-independent.
    pub fn from_palette(p: &Palette) -> Self {
        let selected = p.filled_color(p.accent);
        let dark = p.is_dark();
        let track = if dark { p.background } else { p.surface_alt };
        let selected_step = if dark { p.background } else { p.text };
        Self {
            selected_bg: selected.color,
            selected_edge: if dark {
                Palette::brand_text_on(p.surface_alt, p.accent)
            } else {
                Color::TRANSPARENT
            },
            selected_fg: selected.text,
            unselected_fg: p.text,
            disabled_fg: p.disabled,
            disabled_bg: p.surface_alt,
            segment_disabled_alpha: 0.55,
            segment_bg: track,
            segment_selected_bg: p.segment_selected_accent_bg(),
            segment_selected_hover_bg: mix_srgb_over(
                p.segment_selected_accent_bg(),
                selected_step,
                0.07,
            ),
            segment_selected_pressed_bg: mix_srgb_over(
                p.segment_selected_accent_bg(),
                selected_step,
                0.12,
            ),
            segment_hover_bg: mix_srgb_over(track, p.text, 0.07),
            segment_pressed_bg: mix_srgb_over(track, p.text, 0.12),
            segment_selected_fg: p.segment_selected_accent_fg(),
            seam: p.border,
            hover_overlay: p.overlay_hover(),
            focus_ring: p.focus_ring,
            focus_ring_width: enzui_theme::text_defaults::focus_ring_width(),
            focus_ring_outset: 2.0,
            pill_height: 28.0,
            pill_radius: 9.0,
            pill_padding_x: 14.0,
            label_size: enzui_theme::text_scale::ui_px(13.0),
            icon_size: 16.0,
            icon_gap: 8.0,
            strip_gap: 4.0,
        }
    }
    /// Preset built from the built-in dark palette. Prefer
    /// [`TabPillTokens::from_palette`] with the live palette when one is in scope.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }
    /// Preset built from the built-in light palette. Prefer
    /// [`TabPillTokens::from_palette`] with the live palette when one is in scope.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }

    /// Normalize geometry and state opacity before layout or painting.
    /// Invalid dimensions use constructor defaults; zero lengths remain zero,
    /// except label size, which must be positive. Colours pass through.
    #[must_use]
    pub fn sanitized(&self) -> Self {
        use enzui_core::metrics::{FiniteNonNeg, FinitePositive};
        Self {
            segment_disabled_alpha: self.segment_disabled_opacity(),
            focus_ring_width: FiniteNonNeg::new_or(
                self.focus_ring_width,
                enzui_theme::text_defaults::focus_ring_width(),
            )
            .get(),
            focus_ring_outset: FiniteNonNeg::new_or(self.focus_ring_outset, 2.0).get(),
            pill_height: FiniteNonNeg::new_or(self.pill_height, 28.0).get(),
            pill_radius: FiniteNonNeg::new_or(self.pill_radius, 9.0).get(),
            pill_padding_x: FiniteNonNeg::new_or(self.pill_padding_x, 14.0).get(),
            // cosmic-text requires a positive line height even for an empty label.
            label_size: FinitePositive::new_or(self.label_size, 13.0).get(),
            icon_size: FiniteNonNeg::new_or(self.icon_size, 16.0).get(),
            icon_gap: FiniteNonNeg::new_or(self.icon_gap, 8.0).get(),
            strip_gap: FiniteNonNeg::new_or(self.strip_gap, 4.0).get(),
            ..*self
        }
    }

    fn segment_disabled_opacity(&self) -> f32 {
        if self.segment_disabled_alpha.is_finite() {
            self.segment_disabled_alpha.clamp(0.0, 1.0)
        } else {
            0.55
        }
    }
}

/// Geometry override for a tab strip, preserving live palette colours.
///
/// Values are logical pixels and take precedence over pinned token geometry.
/// Invalid values use [`Self::DEFAULT`]; zero is allowed except for label size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TabPillGeometry {
    /// Focus outline width; `None` (the default) keeps the tokens' width, the
    /// theme's `stroke.focus_ring`. Zero hides the ring.
    pub focus_ring_width: Option<f32>,
    /// Composite-strip focus outset; default 2. Zero keeps it inside the cell.
    pub focus_ring_outset: f32,
    /// Pill height.
    pub pill_height: f32,
    /// Pill corner radius.
    pub pill_radius: f32,
    /// Horizontal padding inside a pill.
    pub pill_padding_x: f32,
    /// Label font size.
    pub label_size: f32,
    /// Leading-icon size.
    pub icon_size: f32,
    /// Gap between a leading icon and the label.
    pub icon_gap: f32,
    /// Inter-pill gap in a strip.
    pub strip_gap: f32,
}

impl TabPillGeometry {
    /// The geometry of the default (`from_palette`) tokens.
    pub const DEFAULT: Self = Self {
        focus_ring_width: None,
        focus_ring_outset: 2.0,
        pill_height: 28.0,
        pill_radius: 9.0,
        pill_padding_x: 14.0,
        label_size: 13.0,
        icon_size: 16.0,
        icon_gap: 8.0,
        strip_gap: 4.0,
    };

    /// Normalize dimensions using [`Self::DEFAULT`] fallbacks.
    /// Label size must be positive; other dimensions allow zero.
    #[must_use]
    pub fn sanitized(&self) -> Self {
        use enzui_core::metrics::{FiniteNonNeg, FinitePositive};
        let d = Self::DEFAULT;
        Self {
            focus_ring_width: self.focus_ring_width.map(|w| {
                FiniteNonNeg::new_or(w, crate::focus_button::DEFAULT_FOCUS_RING_WIDTH).get()
            }),
            focus_ring_outset: FiniteNonNeg::new_or(self.focus_ring_outset, d.focus_ring_outset)
                .get(),
            pill_height: FiniteNonNeg::new_or(self.pill_height, d.pill_height).get(),
            pill_radius: FiniteNonNeg::new_or(self.pill_radius, d.pill_radius).get(),
            pill_padding_x: FiniteNonNeg::new_or(self.pill_padding_x, d.pill_padding_x).get(),
            // Font size, not a px length — see the sibling `sanitized()` above.
            label_size: FinitePositive::new_or(self.label_size, d.label_size).get(),
            icon_size: FiniteNonNeg::new_or(self.icon_size, d.icon_size).get(),
            icon_gap: FiniteNonNeg::new_or(self.icon_gap, d.icon_gap).get(),
            strip_gap: FiniteNonNeg::new_or(self.strip_gap, d.strip_gap).get(),
        }
    }

    fn apply(&self, t: &mut TabPillTokens) {
        if let Some(width) = self.focus_ring_width {
            t.focus_ring_width = width;
        }
        t.focus_ring_outset = self.focus_ring_outset;
        t.pill_height = self.pill_height;
        t.pill_radius = self.pill_radius;
        t.pill_padding_x = self.pill_padding_x;
        t.label_size = self.label_size;
        t.icon_size = self.icon_size;
        t.icon_gap = self.icon_gap;
        t.strip_gap = self.strip_gap;
    }
}

impl Default for TabPillGeometry {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Which foreground a pill uses, mapped to a shared [`crate::icon::IconTone`]
/// so the icon and label always resolve to the *same* runtime colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PillFg {
    Selected,
    /// The selected cell's label in [`TabCellFill::Segmented`] mode. Distinct
    /// from [`PillFg::Selected`] because a segmented selected cell fills with
    /// the mid-tone `segment_selected_bg` and its label must use the ink
    /// chosen for that fill (`segment_selected_fg`), not Pill mode's
    /// `selected_fg`.
    SelectedSegment,
    Unselected,
    Disabled,
    /// A disabled but selected segmented cell: it keeps the selected
    /// foreground, dimmed, rather than dropping to the inert disabled ink. It
    /// dims the segmented `segment_selected_fg`, not Pill mode's `selected_fg`.
    SelectedDisabled,
}

impl PillFg {
    fn color(self, t: &TabPillTokens) -> Color {
        match self {
            PillFg::Selected => t.selected_fg,
            PillFg::SelectedSegment => t.segment_selected_fg,
            PillFg::Unselected => t.unselected_fg,
            PillFg::Disabled => t.disabled_fg,
            PillFg::SelectedDisabled => {
                crate::color::with_opacity(t.segment_selected_fg, t.segment_disabled_opacity())
            }
        }
    }
}

/// Which foreground role a cell's label takes, from its `enabled` / `selected`
/// state and whether it is a segmented cell.
///
/// Factored out of `build_pill` so the routing is a single testable seam:
/// a segmented *selected* cell must route to [`PillFg::SelectedSegment`] (the
/// ink chosen for the mid-tone `control_border_strong` fill), not
/// [`PillFg::Selected`] (Pill mode's `text` on `button_primary_bg`). The token
/// being correct is not enough — the render path must actually read it.
fn pill_fg_role(enabled: bool, selected: bool, segmented: bool) -> PillFg {
    match (enabled, selected) {
        // Segmented: a disabled *selected* cell keeps the selected ink, dimmed
        // — it must not lose its selected indication.
        (false, true) if segmented => PillFg::SelectedDisabled,
        (false, _) => PillFg::Disabled,
        // A segmented selected cell's label uses the ink chosen for the
        // `control_border_strong` fill; a Pill selected label uses `text` on
        // `button_primary_bg`.
        (true, true) if segmented => PillFg::SelectedSegment,
        (true, true) => PillFg::Selected,
        (true, false) => PillFg::Unselected,
    }
}

impl crate::theme_lookup::FollowsThemeOpacity for TabPillTokens {
    fn with_theme_opacity(self, opacity: &enzui_theme::OpacityScale) -> Self {
        Self {
            segment_disabled_alpha: opacity.disabled_surface(),
            ..self
        }
    }
}

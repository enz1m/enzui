//! EnzToggle: a styled toggle/switch component for enzUI-ICED.
//!
//! Composes [`Switch`](crate::switch), the crate's focusable track+thumb
//! primitive, with enzUI's token-driven styling: 3 size variants, optional
//! description subtitle, palette-derived colours for dark/light themes, and
//! a configurable disabled alpha (default `0.62`). Does not wrap iced's built-in
//! `Toggler`, which forces `width = 2 * height` and offers no focus,
//! keyboard or accessibility (see [`crate::switch`]'s module docs).
//!
//! # Where this belongs
//!
//! [`crate::expander`] uses this widget as its expand/collapse control when
//! the caller asks for a switch (rather than a chevron) — the expander is
//! the container, the toggle is the affordance. Reach for `toggle` directly
//! for a stand-alone on/off in a settings row or dialog.
//!
//! # Motion and shadow
//!
//! `Switch` interpolates the thumb position over `motion-normal` via
//! `iced::animation::Animation<bool>`, honouring the theme's reduced-motion
//! policy — the same mechanism `crate::expander` and `crate::code_block`
//! use. The thumb also casts a 2 px blur / 1 px down drop-shadow reproduced
//! as palette-derived tokens on [`ToggleTokens`].
//!
//! # Behaviours not reproduced
//!
//! - Pressed-state thumb stretch (widening the thumb and shifting it left
//!   while pressed) needs a squash/stretch geometry model — an animated
//!   width, a corner radius that stops being `thumb / 2`, and a second
//!   `Animation` keyed on the press state. That is a geometry change,
//!   not a missing iced capability.
//! - Wrap mode for long labels needs the surrounding row to hand the label
//!   column a bounded width and the text to be told to wrap within it —
//!   a layout concern independent of the switch itself.

use crate::color::{darken, with_opacity};
pub use crate::switch::SwitchGeometry;
use crate::switch::{Switch, SwitchColors, SwitchState, switch};
use enzui_core::placement::Direction;
use enzui_theme::Palette;
use iced::advanced::layout::Measure as _;
use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Shell, Widget, mouse, overlay, renderer};
use iced::widget::{column, text};
use iced::{Alignment, Color, Element, Event, Length, Rectangle, Size};
use std::borrow::Cow;

/// Toggle size presets matching the enzUI 3-step scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToggleSize {
    /// 32×18 track, 14px thumb, body−1 label.
    Sm,
    /// 40×22 track, 18px thumb, body label (default).
    #[default]
    Md,
    /// 50×28 track, 24px thumb, body+2 label.
    Lg,
}

impl ToggleSize {
    /// Returns the exact switch track+thumb geometry for this size
    /// (sm 32×18, md 40×22, lg 50×28), replacing the stock Toggler's
    /// forced `width = 2 * height`.
    pub fn geometry(self) -> SwitchGeometry {
        match self {
            Self::Sm => SwitchGeometry::SM,
            Self::Md => SwitchGeometry::MD,
            Self::Lg => SwitchGeometry::LG,
        }
    }

    /// Track height in px .
    pub fn iced_size(self) -> f32 {
        self.geometry().track_height
    }

    /// Returns the label text size for this variant.
    pub fn label_size(self) -> f32 {
        match self {
            Self::Sm => 12.0,
            Self::Md => 13.0,
            Self::Lg => 15.0,
        }
    }

    /// Returns the description (subtitle) text size for this variant.
    pub fn description_size(self) -> f32 {
        match self {
            Self::Sm => 10.0,
            Self::Md => 11.0,
            Self::Lg => 13.0,
        }
    }
}

/// Color tokens for the toggle, derived from the palette.
#[derive(Debug, Clone, Copy)]
pub struct ToggleTokens {
    /// Track color when off.
    pub track_off: Color,
    /// Track color when off + hovered.
    pub track_off_hover: Color,
    /// Track color when on (accent).
    pub track_on: Color,
    /// Track color when on + hovered.
    pub track_on_hover: Color,
    /// Thumb color.
    pub thumb: Color,
    /// Thumb color when hovered. The default keeps the raised surface stable;
    /// custom tokens may give it a separate hover color.
    pub thumb_hover: Color,
    /// Label text color.
    pub text_color: Color,
    /// Description/subtitle text color (muted).
    pub description_color: Color,
    /// Disabled opacity in `[0, 1]`; defaults to `0.62` and multiplies colour alpha.
    /// Out-of-range values are clamped; non-finite values use the default.
    pub disabled_alpha: f32,
    /// Keyboard focus-ring width in logical pixels; defaults to `2.0`.
    /// Invalid values use the default. Zero hides the ring.
    pub focus_ring_width: f32,
    /// Focus-ring colour (keyboard focus-visible).
    pub focus_ring: Color,
    /// Thumb drop-shadow colour, driven by the `control-thumb-shadow`
    /// palette role.
    pub thumb_shadow: Color,
    /// Thumb drop-shadow blur radius, px .
    pub thumb_shadow_blur: f32,
    /// Thumb drop-shadow vertical offset, px .
    pub thumb_shadow_offset_y: f32,
}

/// The off track's text alpha in dark mode: about 1.6:1 against the page,
/// so the pill shows around the knob, as GNOME's and macOS's do.
const OFF_TRACK_DEPTH_DARK: f32 = 0.24;
/// The off track's text alpha in light mode.
const OFF_TRACK_DEPTH_LIGHT: f32 = 0.18;

fn switch_track(track: Color, thumb: Color) -> Color {
    enzui_theme::raise_to_floor(track, thumb, enzui_theme::ContrastFloor::NonText).color()
}

pub(crate) fn switch_colors(theme_tokens: ToggleTokens, st: SwitchState) -> SwitchColors {
    let alpha = crate::color::disabled_alpha(theme_tokens.disabled_alpha);
    let track = if st.disabled {
        if st.on {
            with_opacity(theme_tokens.track_on, alpha)
        } else {
            with_opacity(theme_tokens.track_off, alpha)
        }
    } else if st.on {
        if st.hovered {
            theme_tokens.track_on_hover
        } else {
            theme_tokens.track_on
        }
    } else if st.hovered {
        theme_tokens.track_off_hover
    } else {
        theme_tokens.track_off
    };
    let (thumb_rest, thumb_hover) = (theme_tokens.thumb, theme_tokens.thumb_hover);
    let thumb = if st.disabled {
        with_opacity(thumb_rest, alpha)
    } else if st.hovered {
        thumb_hover
    } else {
        thumb_rest
    };
    let thumb_shadow = if st.disabled {
        with_opacity(theme_tokens.thumb_shadow, alpha)
    } else {
        theme_tokens.thumb_shadow
    };
    SwitchColors {
        track,
        thumb,
        focus_ring: theme_tokens.focus_ring,
        focus_ring_width: enzui_core::metrics::FiniteNonNeg::new_or(
            theme_tokens.focus_ring_width,
            2.0,
        )
        .get(),
        thumb_shadow,
        thumb_shadow_blur: theme_tokens.thumb_shadow_blur,
        thumb_shadow_offset_y: theme_tokens.thumb_shadow_offset_y,
    }
}

impl ToggleTokens {
    /// Derive toggle tokens from a theme palette.
    pub fn from_palette(p: &Palette) -> Self {
        // The thumb is a light knob in both modes, like the slider's: a raised
        // surface on a light page, the text colour on a dark one. A dark knob
        // forced the dark-mode off track up to a bright grey to reach 3:1,
        // which read inverted. Track lightness is then adjusted, preserving
        // hue, until the knob has non-text contrast against it.
        // A knob is opaque wherever it sits, glass included.
        let thumb = if p.is_dark() {
            p.text
        } else {
            crate::color::with_alpha(p.surface_elevated, 1.0)
        };
        // The off track is a translucent groove; its contrast with the knob is
        // measured composited over the page. Hover steps on from it.
        // Dark glass keeps its lighter glass groove; on light glass that groove
        // (40 % white) took the white knob's colour and the off switch
        // vanished, so light glass grooves in the ink like a page does.
        let track_off = if p.is_on_glass() && p.is_dark() {
            p.meter_track()
        } else {
            let depth = if p.is_dark() {
                OFF_TRACK_DEPTH_DARK
            } else {
                OFF_TRACK_DEPTH_LIGHT
            };
            crate::color::with_alpha(p.text, depth)
        };
        Self {
            track_off,
            track_off_hover: crate::color::with_alpha(track_off, (track_off.a + 0.06).min(1.0)),
            track_on: switch_track(p.accent, thumb),
            track_on_hover: switch_track(darken(p.accent, 0.08), thumb),
            thumb,
            thumb_hover: thumb,
            text_color: p.text,
            description_color: p.text_muted,
            focus_ring: p.focus_ring,
            focus_ring_width: enzui_theme::text_defaults::focus_ring_width(),
            disabled_alpha: 0.62,
            // The thumb casts a small drop-shadow (2 px blur, 1 px down)
            // in the `control-thumb-shadow` role. Derived from the palette
            // through `crate::switch::thumb_shadow_color` rather than a
            // hardcoded `Color`, so a custom light/dark palette moves it.
            thumb_shadow: crate::switch::thumb_shadow_color(p),
            thumb_shadow_blur: crate::switch::THUMB_SHADOW_BLUR,
            thumb_shadow_offset_y: crate::switch::THUMB_SHADOW_OFFSET_Y,
        }
    }

    /// Convenience: tokens for dark theme.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }

    /// Convenience: tokens for light theme.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }
}

/// The namespace toggle ids are derived under when no [`EnzToggle::id`] or
/// [`EnzToggle::id_prefix`] is supplied.
///
/// The key is the toggle's accessible name (its visible label, or its
/// `aria_label` when there is no visible one), so the derived id is stable
/// across the surrounding list being filtered or reordered.
pub const DEFAULT_ID_PREFIX: &str = "enz-toggle";

/// Builder for an EnzToggle element.
#[must_use]
pub struct EnzToggle<'a, Message> {
    is_toggled: bool,
    label: Cow<'a, str>,
    on_toggle: Box<dyn Fn(bool) -> Message + 'a>,
    size: ToggleSize,
    description: Cow<'a, str>,
    description_spacing: f32,
    enabled: bool,
    // Read-only: focusable and announced, but the toggle is refused.
    // Distinct from `!enabled` (disabled/inert) exactly as
    // `crate::text_field::EnzTextField::read_only` is distinct from
    // `enabled(false)`: a read-only toggle keeps its callback, stays
    // keyboard-reachable and announces itself as read-only to AT, but
    // every activation path is refused so its value cannot change.
    // Forwarded to `crate::switch::Switch::read_only`.
    read_only: bool,
    tokens: Option<ToggleTokens>,
    // Accessibility-only name for the switch, published to assistive tech
    // without rendering a visible text column. Used when another element
    // (for example an `crate::expander::EnzExpander` header title) is the
    // visible label but the switch itself must still announce a name.
    // Distinct from `label`, which sets both the accessible name and the
    // visible text.
    aria_label: Option<String>,
    // Stable id for the underlying switch, for programmatic focus.
    id: Option<iced::advanced::widget::Id>,
    // Namespace for the label-derived default id. Defaults to
    // `DEFAULT_ID_PREFIX`.
    id_prefix: String,
    // Exact track/thumb geometry override. When set, wins over `size`'s
    // preset geometry (the label/description text sizes still follow
    // `size`). Used where a host needs a non-preset switch footprint —
    // for example `EnzPreferencesRow`'s 42×24/20 AdwSwitchRow toggle.
    geometry_override: Option<SwitchGeometry>,
    thumb_icon: Option<String>,
    direction: Option<Direction>,
    reduced_motion: Option<bool>,
}

/// Create an EnzToggle element.
///
/// # Arguments
///
/// * `is_toggled` — whether the toggle is currently on.
/// * `label` — primary text label.
/// * `on_toggle` — callback producing a message when toggled.
pub fn enz_toggle<'a, Message: 'a>(
    is_toggled: bool,
    label: impl Into<Cow<'a, str>>,
    on_toggle: impl Fn(bool) -> Message + 'a,
) -> EnzToggle<'a, Message> {
    EnzToggle {
        is_toggled,
        label: label.into(),
        on_toggle: Box::new(on_toggle),
        size: ToggleSize::default(),
        description: Cow::Borrowed(""),
        description_spacing: DESCRIPTION_SPACING,
        enabled: true,
        read_only: false,
        tokens: None,
        geometry_override: None,
        thumb_icon: None,
        direction: None,
        reduced_motion: None,
        aria_label: None,
        id: None,
        id_prefix: DEFAULT_ID_PREFIX.to_owned(),
    }
}

impl<'a, Message: 'a> EnzToggle<'a, Message> {
    /// Set the size variant.
    pub fn size(mut self, size: ToggleSize) -> Self {
        self.size = size;
        self
    }

    /// Override the exact track/thumb geometry, bypassing the `size`
    /// preset's geometry (label/description text sizes still follow
    /// `size`). For hosts that need a specific switch footprint — for
    /// example `EnzPreferencesRow`'s 42×24 track / 20 px thumb.
    pub fn geometry(mut self, geometry: SwitchGeometry) -> Self {
        self.geometry_override = Some(geometry);
        self
    }

    /// Paint a bundled symbolic icon inside the moving thumb.
    ///
    /// The icon is decorative and does not add an activation or accessibility
    /// node. The toggle's label remains its accessible name.
    pub fn thumb_icon(mut self, name: impl Into<String>) -> Self {
        self.thumb_icon = Some(name.into());
        self
    }

    /// Override the switch writing direction.
    ///
    /// By default the ambient locale is used. Right-to-left layout mirrors the
    /// off and on thumb endpoints while the icon remains centred in the thumb.
    pub fn direction(mut self, direction: Direction) -> Self {
        self.direction = Some(direction);
        self
    }

    /// Override reduced-motion policy for this toggle instance.
    ///
    /// The default follows the live theme. Passing `true` snaps the thumb and
    /// its icon to the new state without an intermediate frame.
    pub fn reduced_motion(mut self, reduced: bool) -> Self {
        self.reduced_motion = Some(reduced);
        self
    }

    /// Set the description (subtitle) text.
    pub fn description(mut self, desc: impl Into<Cow<'a, str>>) -> Self {
        self.description = desc.into();
        self
    }

    /// Space between the label and the description, in logical pixels;
    /// default 0, the two lines set close as one block.
    pub fn description_spacing(mut self, spacing: f32) -> Self {
        self.description_spacing = enzui_core::metrics::FiniteNonNeg::new_or(spacing, 0.0).get();
        self
    }

    /// Set enabled/disabled state.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Mark this toggle read-only: focusable and announced, edits refused.
    ///
    /// See the [`Self::read_only`](EnzToggle#structfield.read_only) field docs
    /// for the read-only-vs-disabled contract. `enabled(false)` still wins (an
    /// inert control is inert regardless of read-only), so set `enabled(true)`
    /// alongside this.
    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    /// Pin colours, focus width, disabled opacity and shadow geometry.
    /// Omitting this override derives colours from the live theme.
    pub fn tokens(mut self, tokens: ToggleTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Set an accessibility-only name for the switch, published to the AT
    /// tree without rendering any visible text column.
    ///
    /// Use this when the visible labelling lives on a sibling element (for
    /// example an [`crate::expander::EnzExpander`] header title that labels
    /// the section's enable gate) but the switch still needs a spoken
    /// name. Unlike `Self::label`, which sets the accessible name and
    /// renders visible text, `.aria_label` never creates the text column.
    /// If a visible `Self::label` is also present, the visible label wins
    /// the accessible name (a control should not advertise two conflicting
    /// names).
    pub fn aria_label(mut self, name: impl Into<String>) -> Self {
        self.aria_label = Some(name.into());
        self
    }

    /// Set a stable id on the underlying switch for programmatic focus.
    ///
    /// Wins over the name-derived default (see [`DEFAULT_ID_PREFIX`]). Set
    /// it when two toggles in one view share a label, which would otherwise
    /// derive one id twice.
    pub fn id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Namespace the name-derived default id.
    ///
    /// Two preference panes offering a "Notifications" toggle on one screen
    /// would derive the same id; a distinct prefix per pane keeps both
    /// individually routable. Ignored when [`Self::id`] is set.
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }

    // The accessible name the switch publishes: a visible `Self::label`
    // wins, else an `Self::aria_label`. Shared by the semantics wiring
    // and the id derivation so the two cannot disagree about what the
    // control is.
    fn a11y_name(&self) -> Option<String> {
        if !self.label.is_empty() {
            Some(self.label.to_string())
        } else {
            self.aria_label.clone()
        }
    }

    /// Build the element. If `.tokens()` was not called, chrome tokens
    /// are derived from the runtime `iced::Theme`.
    ///
    /// Composition: the switch and the label/description column are
    /// siblings in a single row, and clicking the text also toggles. The
    /// subtitle stays inside the toggle's event bounds rather than sitting
    /// below the track. The switch is the custom focusable [`Switch`]
    /// widget with exact geometry, Tab focus, Enter/Space activation and
    /// a focus ring.
    pub fn into_element(self) -> Element<'a, Message>
    where
        Message: Clone,
    {
        use std::rc::Rc;

        let sz = self.size;
        let geometry = self.geometry_override.unwrap_or_else(|| sz.geometry());
        let pinned_tokens = self.tokens;
        let enabled = self.enabled;
        let read_only = self.read_only;
        let is_on = self.is_toggled;
        // Resolved before `self.on_toggle` is moved below, so both the
        // accessible name and the id derived from it read the builder's
        // own fields rather than a partially-moved `self`.
        let a11y_name = self.a11y_name();
        let on_toggle: Rc<dyn Fn(bool) -> Message + 'a> = Rc::from(self.on_toggle);

        let toggle_cb = Rc::clone(&on_toggle);
        let mut sw: Switch<'a, Message> = switch(is_on, move |v| (toggle_cb)(v))
            .geometry(geometry)
            .colors(move |theme: &iced::Theme, st| {
                // Pinned tokens win; otherwise resolve dark/light from the
                // runtime iced::Theme so the switch re-themes live.
                let tokens = pinned_tokens.unwrap_or_else(|| {
                    crate::theme_lookup::resolve_live(theme, ToggleTokens::from_palette)
                });
                switch_colors(tokens, st)
            });
        // Accessible name precedence: a visible `label` sets both the visible
        // text (below) and the switch's accessible name. When there is no
        // visible label, an `.aria_label` supplies the accessible name without
        // rendering any text column — this is the path the EnzExpander gate
        // uses so a screen reader announces "Enable <title>" while the visible
        // title labels it.
        if let Some(name) = &a11y_name {
            sw = sw.label(name.clone());
        }
        if let Some(name) = &self.thumb_icon {
            sw = sw.thumb_icon(name);
        }
        if let Some(direction) = self.direction {
            sw = sw.direction(direction);
        }
        if let Some(reduced) = self.reduced_motion {
            sw = sw.reduced_motion(reduced);
        }
        // Derive the switch's identity from its accessible name when the
        // caller supplies none. An anonymous switch is absent from
        // `NodeIdMap`, so the collector strips the `Click` it advertises
        // and an AT user can read "Wi-Fi, switch, on" and never flip it.
        // The name is the toggle's logical key — what an AT user says to
        // reach it — so it is stable across the surrounding list being
        // filtered or reordered, unlike a position. A nameless toggle has
        // no logical key at all and stays anonymous by design: inventing
        // one would give it a positional identity, which is worse than
        // none.
        let switch_id = self.id.clone().or_else(|| {
            a11y_name
                .as_deref()
                .map(|name| enzui_core::identity::derive_id(&self.id_prefix, name))
        });
        if let Some(id) = switch_id {
            sw = sw.id(id);
        }
        // Convey the subtitle to assistive tech too: the description
        // sits visually below the label but is otherwise invisible to a screen
        // reader unless published as the switch node's a11y description.
        if !self.description.is_empty() {
            sw = sw.description(self.description.to_string());
        }
        // Dropping the callback when disabled is a load-bearing invariant,
        // not an optimisation: `EnzExpander`'s read-only gate (in the
        // `expander` module) is built with an `|_| unreachable!()` callback
        // and relies on this line to guarantee it is never called. Any
        // change that lets a disabled toggle emit must update that call
        // site first.
        if !enabled {
            sw = sw.on_toggle_maybe(None::<fn(bool) -> Message>);
        } else if read_only {
            // Read-only (but enabled): keep the callback so focus and semantics
            // behave as a live control, and forward `read_only` so the switch
            // refuses every activation and announces itself read-only rather
            // than disabled. `enabled(false)` above wins over this, so a
            // disabled read-only field is still fully inert.
            sw = sw.read_only(true);
        }
        let switch_el: Element<'a, Message> = sw.into();

        // Even a visually bare EnzToggle stays in the activation row: the row
        // supplies the framework hit-target floor without changing the painted
        // track. No text means no horizontal gap or extra width.
        let has_text = !self.label.is_empty() || !self.description.is_empty();

        // Label + description column (runtime-theme muted description).
        // With a description, both lines set a little tighter than body
        // text, so title and description read as one block.
        let line_height = if self.description.is_empty() {
            iced::widget::text::LineHeight::default()
        } else {
            iced::widget::text::LineHeight::Relative(DESCRIBED_LINE_HEIGHT)
        };
        let mut text_col = column![]
            .spacing(self.description_spacing)
            .align_x(Alignment::Start);
        if !self.label.is_empty() {
            let label_size = sz.label_size();
            let pinned = pinned_tokens;
            text_col = text_col.push(
                text(self.label.to_string())
                    .size(label_size)
                    .line_height(line_height)
                    .style(move |theme: &iced::Theme| iced::widget::text::Style {
                        color: Some({
                            let t = pinned.unwrap_or_else(|| {
                                crate::theme_lookup::resolve_live(theme, ToggleTokens::from_palette)
                            });
                            if enabled {
                                t.text_color
                            } else {
                                with_opacity(
                                    t.text_color,
                                    crate::color::disabled_alpha(t.disabled_alpha),
                                )
                            }
                        }),
                    }),
            );
        }
        if !self.description.is_empty() {
            let desc_size = sz.description_size();
            let pinned = pinned_tokens;
            text_col = text_col.push(
                text(self.description.to_string())
                    .size(desc_size)
                    .line_height(line_height)
                    .style(move |theme: &iced::Theme| iced::widget::text::Style {
                        color: Some({
                            let t = pinned.unwrap_or_else(|| {
                                crate::theme_lookup::resolve_live(theme, ToggleTokens::from_palette)
                            });
                            if enabled {
                                t.description_color
                            } else {
                                with_opacity(
                                    t.description_color,
                                    crate::color::disabled_alpha(t.disabled_alpha),
                                )
                            }
                        }),
                    }),
            );
        }

        // Compose switch + label/description as a single activation
        // target: switch, 8 px gap and label are one hit surface. A
        // `row![switch, mouse_area(text)]` leaves the gap belonging to
        // neither child, so a click landing between them (x=40..48 in the
        // MD row) toggles nothing at all; and the label's `mouse_area`
        // fires the toggle without focusing the switch, so the next
        // keyboard press operates on whatever had focus before. See
        // `tests/toggle_activation_hit_surface.rs`.
        //
        // `focus_button` does not fit here: the switch is the focusable
        // element (Role::Switch, keyboard Enter/Space, focus-visible ring
        // drawn by the switch itself), and wrapping the row in a
        // FocusButton would introduce a second focusable and a second
        // keyboard path. Instead this composite forwards pointer events
        // to the switch under widened bounds covering the whole row, so
        // the switch's own `press_focus` + toggle line (the one the click
        // path already uses) fires for any click within the row, focus
        // and all. Draw still sees the true 40×22 track bounds;
        // operations keep that narrow width while receiving the row's
        // 24 px target height.
        let text_el: Element<'a, Message> = text_col.into();
        ToggleActivationRow {
            switch: switch_el,
            text: text_el,
            switch_paint_size: Size::new(geometry.track_width, geometry.track_height),
            clip_paint: false,
            has_text,
            enabled,
        }
        .into()
    }
}

// Composite that lays out the switch and the label column side by side
// and makes the whole row a single pointer hit surface for the switch.
//
// See the composition comment in `EnzToggle::into_element` for why this
// exists rather than a `row![switch, mouse_area(text)]` (or a
// `focus_button` wrapper). The two properties this shape guarantees, both
// pinned by `tests/toggle_activation_hit_surface.rs`:
//
// * A click anywhere on the row — including the 8 px gap between the
//   switch and the label — publishes the toggle message and focuses the
//   switch.
// * A disabled row is entirely inert: nothing is emitted anywhere on it,
//   and no focus is taken.
//
// The switch's true (narrow) bounds are used for `draw`, while `operate`
// receives the same narrow width with the row's framework-floor height.
// This keeps the painted track at its exact preset geometry while making
// the published/focus target at least 24 px tall whenever the parent can
// provide it. `update` and `mouse_interaction` see the whole row
// activation surface. Vertical alignment is centre.
struct ToggleActivationRow<'a, Message> {
    switch: Element<'a, Message>,
    text: Element<'a, Message>,
    // Unconstrained track geometry. Draw compares this against the laid-
    // out node size to detect when fixed-size switch ink needs clipping
    // to the row allocation; normal geometry is not clipped, preserving
    // the focus-ring and shadow paint.
    switch_paint_size: Size,
    // Set during layout when either child geometry or the whole row
    // saturates a finite parent bound. Draw then clips exactly those
    // constrained cases.
    clip_paint: bool,
    has_text: bool,
    enabled: bool,
}

impl<Message> ToggleActivationRow<'_, Message> {
    // Gap between the switch and the label, in px.
    const GAP: f32 = 8.0;

    // Build an absolute `Layout` node for the switch using `bounds`.
    fn switch_node(bounds: Rectangle) -> layout::Node {
        layout::Node::new(bounds.size()).move_to(bounds.position())
    }

    // The switch's whole-row pointer target. It starts at the switch
    // (never to its left) and covers the gap, text and full allocated row
    // height.
    fn activation_bounds(row_bounds: Rectangle, switch_bounds: Rectangle) -> Rectangle {
        Rectangle {
            x: switch_bounds.x,
            y: row_bounds.y,
            width: row_bounds.width - (switch_bounds.x - row_bounds.x),
            height: row_bounds.height,
        }
    }

    // The narrow switch target published to operations and accessibility.
    // Its width remains the painted track's allocation, while its height
    // uses the row's hit-target floor and therefore matches a real
    // clickable area.
    fn operated_bounds(row_bounds: Rectangle, switch_bounds: Rectangle) -> Rectangle {
        Rectangle {
            x: switch_bounds.x,
            y: row_bounds.y,
            width: switch_bounds.width,
            height: row_bounds.height,
        }
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for ToggleActivationRow<'_, Message>
where
    Message: Clone,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::stateless()
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(&mut [&mut self.switch, &mut self.text]);
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: Length::Shrink,
            height: Length::Shrink,
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let hit_floor = enzui_theme::SizingTokens::default().min_hit_target;

        // Budget the gap before laying out the switch so the row never
        // exceeds its limits: at widths where the switch would otherwise
        // consume the allotment the row would be exactly 8 px too wide.
        // The gap yields first when the parent cannot provide both it and
        // the framework hit floor, keeping the switch visible instead of
        // shrinking it to zero at the tightest sweep rung.
        let gap = if self.has_text {
            Self::GAP.min((limits.max().width - hit_floor).max(0.0))
        } else {
            0.0
        };
        let switch_limits = limits.shrink(Size::new(gap, 0.0));
        let sw_node =
            self.switch
                .as_widget_mut()
                .measure(&mut tree.children[0], renderer, &switch_limits);
        let sw_size = sw_node.size();

        let text_limits = limits.shrink(Size::new(sw_size.width + gap, 0.0));
        let text_node =
            self.text
                .as_widget_mut()
                .measure(&mut tree.children[1], renderer, &text_limits);
        let text_size = text_node.size();

        let natural_height = sw_size.height.max(text_size.height);
        let total_height = limits.resolve_height(Length::Shrink, natural_height.max(hit_floor));
        let total_width = sw_size.width + gap + text_size.width;
        let max = limits.max();
        const SATURATION_EPSILON: f32 = 0.01;
        self.clip_paint = sw_size.width < self.switch_paint_size.width
            || sw_size.height < self.switch_paint_size.height
            || (max.width.is_finite() && total_width + SATURATION_EPSILON >= max.width)
            || (max.height.is_finite() && total_height + SATURATION_EPSILON >= max.height);

        // Vertically centre both children within the row.
        let sw_positioned =
            sw_node.move_to(iced::Point::new(0.0, (total_height - sw_size.height) / 2.0));
        let text_positioned = text_node.move_to(iced::Point::new(
            sw_size.width + gap,
            (total_height - text_size.height) / 2.0,
        ));

        layout::Node::with_children(
            Size::new(total_width, total_height),
            vec![sw_positioned, text_positioned],
        )
        .apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        // Operations keep the switch's narrow width but receive the row's
        // allocated height. The track still draws at its exact geometry, while
        // focus and accessibility expose a target that reaches the framework
        // floor whenever the parent can provide it.
        let row_bounds = layout.bounds();
        let mut children = layout.children(&tree.children);
        let sw_layout = children
            .next()
            .expect("the layout has sw and text children");
        let text_layout = children
            .next()
            .expect("the layout has sw and text children");
        let operated_node =
            Self::switch_node(Self::operated_bounds(row_bounds, sw_layout.bounds()));
        self.switch.as_widget_mut().operate(
            &mut tree.children[0],
            Layout::of(&operated_node),
            viewport,
            renderer,
            operation,
        );
        self.text.as_widget_mut().operate(
            &mut tree.children[1],
            text_layout,
            viewport,
            renderer,
            operation,
        );
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let row_bounds = layout.bounds();
        let mut children = layout.children(&tree.children);
        let sw_layout = children
            .next()
            .expect("the layout has sw and text children");
        let text_layout = children
            .next()
            .expect("the layout has sw and text children");

        // The label column has no interactive widgets today, but forwarding
        // is what lets it host one tomorrow (for example an inline hint)
        // without this composite swallowing its input.
        self.text.as_widget_mut().update(
            &mut tree.children[1],
            event,
            text_layout,
            cursor,
            renderer,
            shell,
            viewport,
        );

        if !self.enabled {
            self.switch.as_widget_mut().update(
                &mut tree.children[0],
                event,
                sw_layout,
                cursor,
                renderer,
                shell,
                viewport,
            );
            return;
        }

        // Enabled: feed the switch a widened layout so its own
        // `press_focus` and `cursor.is_over` toggle checks fire for clicks
        // anywhere on the row — gap and label included. The switch's
        // `update` publishes the toggle and focuses itself through the
        // exact same code path a click on its own track uses, so the AT /
        // semantics / focus behaviour of the label click cannot drift
        // from the switch click behaviour.
        let widened_node =
            Self::switch_node(Self::activation_bounds(row_bounds, sw_layout.bounds()));
        let widened_layout = Layout::of(&widened_node);
        self.switch.as_widget_mut().update(
            &mut tree.children[0],
            event,
            widened_layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        // Draw uses the true narrow layouts so the switch appears at its
        // laid-out size and position, the focus ring paints around the
        // track, and the text sits at its column position. The widened
        // bounds are an `update`-only extension for the hit surface.
        let mut children = layout.children(&tree.children);
        let sw_layout = children
            .next()
            .expect("the layout has sw and text children");
        let text_layout = children
            .next()
            .expect("the layout has sw and text children");
        let draw_children = |renderer: &mut iced::Renderer| {
            self.switch.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                sw_layout,
                cursor,
                viewport,
            );
            self.text.as_widget().draw(
                &tree.children[1],
                renderer,
                theme,
                style,
                text_layout,
                cursor,
                viewport,
            );
        };

        // `Switch` paints its fixed thumb geometry even when its layout
        // node is squeezed. Clip only that constrained case to the
        // activation row: narrow parents cannot receive thumb/shadow or
        // text ink over their neighbour, while an ordinary 40×22 MD
        // switch keeps its un-clipped focus ring and shadow.
        if self.clip_paint {
            use iced::advanced::Renderer as _;
            renderer.with_layer(layout.bounds(), draw_children);
        } else {
            draw_children(renderer);
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        // Extend the pointer-cursor feedback across the whole row so hovering
        // the label shows the click-me hand, matching the widened hit
        // surface. Disabled rows report the default cursor (like a bare
        // disabled Switch).
        let row_bounds = layout.bounds();
        if self.enabled && cursor.is_over(row_bounds) {
            return mouse::Interaction::Pointer;
        }
        let mut children = layout.children(&tree.children);
        let sw_layout = children
            .next()
            .expect("the layout has sw and text children");
        let text_layout = children
            .next()
            .expect("the layout has sw and text children");
        let sw = self.switch.as_widget().mouse_interaction(
            &tree.children[0],
            sw_layout,
            cursor,
            viewport,
            renderer,
        );
        let txt = self.text.as_widget().mouse_interaction(
            &tree.children[1],
            text_layout,
            cursor,
            viewport,
            renderer,
        );
        sw.max(txt)
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: iced::Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        let mut children = layout.children(&tree.children);
        let sw_layout = children
            .next()
            .expect("the layout has sw and text children");
        let text_layout = children
            .next()
            .expect("the layout has sw and text children");
        let mut trees = tree.children.iter_mut();
        let sw_tree = trees.next().expect("the tree has sw and text children");
        let text_tree = trees.next().expect("the tree has sw and text children");
        let mut overlays = self.switch.as_widget_mut().overlay(
            sw_tree,
            sw_layout,
            renderer,
            viewport,
            translation,
            window,
        );
        let text_overlay = self.text.as_widget_mut().overlay(
            text_tree,
            text_layout,
            renderer,
            viewport,
            translation,
            window,
        );
        overlays.extend(text_overlay);
        overlays
    }
}

impl<'a, Message: 'a> From<ToggleActivationRow<'a, Message>> for Element<'a, Message>
where
    Message: Clone,
{
    fn from(row: ToggleActivationRow<'a, Message>) -> Self {
        Element::new(row)
    }
}

impl<'a, Message: Clone + 'a> From<EnzToggle<'a, Message>> for Element<'a, Message> {
    fn from(toggle: EnzToggle<'a, Message>) -> Self {
        toggle.into_element()
    }
}

impl crate::theme_lookup::FollowsThemeOpacity for ToggleTokens {
    fn with_theme_opacity(self, opacity: &enzui_theme::OpacityScale) -> Self {
        Self {
            disabled_alpha: opacity.disabled(),
            ..self
        }
    }
}

/// Space between a label and its description by default.
const DESCRIPTION_SPACING: f32 = 0.0;
/// The line height of a label and its description set together.
const DESCRIBED_LINE_HEIGHT: f32 = 1.2;

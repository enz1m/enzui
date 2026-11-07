//! EnzDisclosureButton: expand/collapse toggle row.
//!
//! A tappable row with a chevron glyph (right when collapsed,
//! down when expanded) followed by a label. Emits a boolean on tap.
//!
//! Chevron: the widget renders the
//! chevron as a Lucide SVG `EnzIcon` (`navigation.forward`, i.e.
//! `chevron-right`) by default, in an independent 12×12 icon slot, and
//! turns it a quarter to point down as the section opens. When unpinned the
//! chevron tint follows the runtime theme (`Palette::text_muted`);
//! a pinned `DisclosureTokens` applies its explicit `icon_color`.
//! Callers who need the legacy Unicode `▶`/`▼` glyphs can opt out
//! with `.use_lucide_chevron(false)` and override them via
//! `.chevron_collapsed()` / `.chevron_expanded()`.

use crate::enz_text::enz_text;
use crate::focus_button::FocusStatus;
use enzui_theme::Palette;
use iced::widget::button::Style;
use iced::widget::{row, text};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding};

/// Color tokens for the disclosure button.
#[derive(Debug, Clone, Copy)]
pub struct DisclosureTokens {
    /// Label text color — `text-color = text`.
    pub text_color: Color,
    /// Chevron color — `icon-color = text-muted`.
    pub icon_color: Color,
    /// Hover background: the composited Overlay role `overlay_hover()`
    /// (a translucent `text` wash), not an opaque surface token. A
    /// disclosure is placed flush on its parent surface (a sidebar
    /// body is `surface_alt`), so an opaque `surface_hover`
    /// (byte-identical to `surface_alt` in both presets) would
    /// repaint the colour already under the row and give zero hover
    /// feedback. The overlay is a wash over whatever is behind,
    /// visible on any surface.
    pub hover_background: Color,
    /// Pressed background: the composited PressedOverlay role
    /// `overlay_pressed()`, one alpha step stronger than
    /// [`Self::hover_background`] so pressed reads heavier over the
    /// same surface. A pre-darkened opaque fill can move the wash
    /// the wrong way and leave pressed within ~1px of hover.
    pub pressed_background: Color,
    /// Keyboard focus-ring colour — `Palette::focus_ring` (dark `#0A84FF`,
    /// light `#006CDC`). The ring uses the semantic focus-ring token like every
    /// sibling widget, rather than a hardcoded `text_color`.
    pub focus_ring_color: Color,
    /// Keyboard focus-ring width, in px. Matches the crate convention
    /// (`pressable`, `list_row_surface`, `switch`, `toggle` all use a
    /// `focus_ring_width` token defaulting to `2.0`); using `1.5` would make
    /// this ring visibly thinner than its siblings.
    pub focus_ring_width: f32,
}

impl DisclosureTokens {
    /// Derive tokens from a palette.
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            text_color: p.text,
            icon_color: p.text_muted,
            // The row is flush on its parent surface, so hover and
            // pressed are composited Overlay washes over whatever
            // is behind, not opaque surface tokens (which alias
            // `surface_alt` and vanish on a sidebar).
            // `disclosure_style` paints these as `Background::Color`,
            // so their alpha composites over the real backdrop.
            hover_background: p.overlay_hover(),
            pressed_background: p.overlay_pressed(),
            focus_ring_color: p.focus_ring,
            focus_ring_width: enzui_theme::text_defaults::focus_ring_width(),
        }
    }
    /// Dark preset.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }
    /// Light preset.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }
}

/// Builder for an enzUI-styled disclosure button.
#[must_use]
pub struct EnzDisclosure<'a, Message> {
    label: String,
    expanded: bool,
    on_toggle: Box<dyn Fn(bool) -> Message + 'a>,
    text_size: f32,
    /// Row height. Default: `list-row-height` (34).
    row_height: f32,
    /// Row corner radius. Default: `radius-sm` (5).
    row_radius: f32,
    /// Gap between chevron and label. Default: `spacing-sm` (8).
    icon_gap: f32,
    /// Explicit caller-supplied widget id, used verbatim in place of the
    /// label-derived one. `None` means "derive from the label" (see
    /// [`enz_disclosure`] and `id_prefix`).
    id: Option<iced::advanced::widget::Id>,
    /// Namespace for the label-derived id. A disclosure is a single
    /// control, so its default id is `{id_prefix}-{label}`. Defaults to
    /// `"enz-disclosure"`; set a distinct prefix when two disclosures
    /// share a screen and a label (legal, and otherwise a silent
    /// collision — no widget can see its siblings).
    id_prefix: String,
    /// Chevron glyph shown when collapsed. Default: `▶` (Unicode). Only used
    /// when `use_lucide_chevron` is false.
    chevron_collapsed: String,
    /// Chevron glyph shown when expanded. Default: `▼` (Unicode). Only used
    /// when `use_lucide_chevron` is false.
    chevron_expanded: String,
    /// When true (the default), render the chevron as a Lucide SVG
    /// icon (`chevron-right`, turned down while open). Set
    /// false to fall back to the Unicode `chevron_collapsed` /
    /// `chevron_expanded` text glyphs.
    use_lucide_chevron: bool,
    /// Writing direction override; `None` follows the locale.
    direction: Option<enzui_core::placement::Direction>,
    /// Chevron icon slot size (px), independent of the body label size.
    /// Default: 12.
    icon_size: f32,
    tokens: Option<DisclosureTokens>,
    /// When `true`, thread `.audit_elision(true)`
    /// through to the label's `enz_text`, so the accessibility scan collector sees a
    /// `TextElision` finding on an over-long label rather than reporting
    /// clean. Default `false` — production apps pay nothing.
    audit_elision: bool,
}

/// Creates a new enzUI-styled disclosure button.
///
/// # Assistive-technology identity
///
/// So an assistive technology can activate the trigger (not merely
/// focus it), the disclosure needs a routable widget [`Id`]. When no
/// explicit [`id`] is set, one is derived from the label as
/// `{id_prefix}-{label}` (the same pattern [`crate::link::enz_link`]
/// uses for its url). An explicit [`id`] always wins over the
/// derived one.
///
/// Known limitation: a label-derived key is unstable if the label is
/// dynamic, and two disclosures with the same label collide. The
/// derivation is deliberately pure (label in, id out) so it stays
/// stable across relayouts; it is not disambiguated by position or
/// a counter, because either would make the id unstable across
/// relayouts — worse than a collision. Callers whose label changes
/// at runtime, or who place two disclosures with identical labels on
/// one screen, should set [`id`] explicitly (or a distinct
/// `id_prefix`).
///
/// [`Id`]: iced::advanced::widget::Id
/// [`id`]: EnzDisclosure::id
pub fn enz_disclosure<'a, Message: 'a>(
    label: impl Into<String>,
    expanded: bool,
    on_toggle: impl Fn(bool) -> Message + 'a,
) -> EnzDisclosure<'a, Message> {
    EnzDisclosure {
        label: label.into(),
        expanded,
        on_toggle: Box::new(on_toggle),
        // EnzDisclosureButton: body-size (13) label, list-row-height (34)
        // row.
        text_size: enzui_theme::text_scale::ui_px(13.0),
        row_height: enzui_theme::text_defaults::sizing().row_height,
        row_radius: 5.0,
        icon_gap: 8.0,
        chevron_collapsed: "▶".into(),
        chevron_expanded: "▼".into(),
        // Lucide chevron by default, in a fixed 12px slot.
        use_lucide_chevron: true,
        direction: None,
        icon_size: 12.0,
        tokens: None,
        id: None,
        id_prefix: "enz-disclosure".into(),
        audit_elision: false,
    }
}

impl<'a, Message: Clone + 'a> EnzDisclosure<'a, Message> {
    /// Sets the label text size.
    pub fn text_size(mut self, size: f32) -> Self {
        self.text_size = enzui_core::metrics::FinitePositive::new_or(size, 13.0).get();
        self
    }

    /// Sets the row height.
    pub fn row_height(mut self, h: f32) -> Self {
        self.row_height = enzui_core::metrics::FinitePositive::new_or(h, 34.0).get();
        self
    }

    /// Sets the row corner radius.
    pub fn row_radius(mut self, r: f32) -> Self {
        self.row_radius = enzui_core::metrics::FiniteNonNeg::new_or(r, 5.0).get();
        self
    }

    /// Sets the gap between chevron and label.
    pub fn icon_gap(mut self, g: f32) -> Self {
        self.icon_gap = enzui_core::metrics::FiniteNonNeg::new_or(g, 8.0).get();
        self
    }

    /// Sets the chevron glyph when collapsed (default `▶`).
    pub fn chevron_collapsed(mut self, s: impl Into<String>) -> Self {
        self.chevron_collapsed = s.into();
        self
    }

    /// Sets the chevron glyph when expanded (default `▼`).
    pub fn chevron_expanded(mut self, s: impl Into<String>) -> Self {
        self.chevron_expanded = s.into();
        self
    }

    /// Toggle the Lucide SVG chevron (`navigation.forward`, i.e.
    /// `chevron-right`, turned down while open), which is on by default.
    /// When enabled, the `.chevron_collapsed()` / `.chevron_expanded()`
    /// text overrides are ignored — the icon assets are used
    /// verbatim. Set false to render the legacy Unicode text glyphs
    /// instead.
    pub fn use_lucide_chevron(mut self, on: bool) -> Self {
        self.use_lucide_chevron = on;
        self
    }

    /// Override the writing direction. By default the disclosure follows the
    /// locale; right-to-left puts the chevron on the right, pointing left
    /// while collapsed.
    pub fn direction(mut self, direction: enzui_core::placement::Direction) -> Self {
        self.direction = Some(direction);
        self
    }

    /// Set the chevron icon slot size (px). The default 12×12 slot is
    /// independent of the body label size; overriding `text_size`
    /// must not resize the chevron. Non-finite / non-positive values are
    /// ignored.
    pub fn icon_size(mut self, px: f32) -> Self {
        if px.is_finite() && px > 0.0 {
            self.icon_size = px;
        }
        self
    }

    /// Pin color tokens explicitly.
    pub fn tokens(mut self, tokens: DisclosureTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Set a stable widget id so a host can move keyboard focus onto
    /// the disclosure trigger (`focus-disclosure()`) and so an
    /// assistive technology can route an activation to it.
    ///
    /// This overrides the label-derived default id. Use it when the
    /// label is dynamic or duplicated on-screen, so the id stays stable and
    /// unique. See [`enz_disclosure`] for the derivation rule.
    pub fn id(mut self, id: impl Into<iced::advanced::widget::Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Namespace for the auto-derived, label-keyed id. Set a distinct
    /// prefix when two disclosures carrying the same label share a screen — the
    /// label alone is then not unique and the derived ids would collide.
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }

    /// Opt this disclosure's label into the
    /// a11y elision audit. When `on` is `true`, the label's `enz_text` gets
    /// `.audit_elision(true)` and the accessibility scan collector observes an over-
    /// long label as a `TextElision` finding. Off by default — production
    /// apps pay nothing for the audit; a diagnostics harness opts in.
    pub fn audit_elision(mut self, on: bool) -> Self {
        self.audit_elision = on;
        self
    }

    /// Converts this builder into an iced `Element`.
    pub fn into_element(self) -> Element<'a, Message> {
        let text_size = self.text_size;
        let icon_size = self.icon_size;
        let row_height = self.row_height;
        let row_radius = self.row_radius;
        let icon_gap = self.icon_gap;
        let pinned = self.tokens;
        let expanded = self.expanded;
        let use_lucide = self.use_lucide_chevron;
        let direction = enzui_core::placement::resolve_direction(self.direction);
        let rtl = direction == enzui_core::placement::Direction::RightToLeft;
        let chevron_text_glyph = if expanded {
            self.chevron_expanded.clone()
        } else if rtl && self.chevron_collapsed == "▶" {
            // The default collapsed glyph points toward the end edge.
            "◀".to_owned()
        } else {
            self.chevron_collapsed.clone()
        };
        // Activation: toggle to the opposite of the current expanded
        // state. Build the message lazily (only on real activation)
        // via FocusButton's `on_press_with`, rather than eagerly
        // calling `on_toggle` during view construction. The closure
        // captures the caller's factory and fires on pointer,
        // Space/Enter, or the AccessKit default action.
        let on_toggle = self.on_toggle;
        let label_for_a11y = self.label.clone();
        // Derive a routable id from the label by default, so an
        // assistive technology can activate the trigger (FocusButton
        // only offers an activation carrier for a button that carries
        // an id — an anonymous one is focusable and clickable but
        // never answers a routed `Activate`). An explicit `.id()`
        // always wins. The label is the disclosure's logical key,
        // mirroring `enz_link`'s url-derived id.
        let id = self
            .id
            .unwrap_or_else(|| enzui_core::identity::derive_id(&self.id_prefix, &self.label));

        // Chevron is either a Lucide SVG (default) or a Unicode text glyph
        // (opt-out via .use_lucide_chevron(false)). The SVG uses the exact
        // 12px icon slot (independent of the label text size). Tint:
        // - unpinned → the icon's default runtime `ThemeMuted` (text_muted),
        //   so live/light/custom themes drive the chevron colour;
        // - pinned   → the pinned `DisclosureTokens::icon_color` verbatim,
        //   mirroring the build-time `icon-color` binding.
        let chevron_widget: iced::Element<'a, Message> = if use_lucide {
            // The collapsed glyph turns a quarter to point down as it opens.
            let icon = crate::icon::enz_icon_for_direction("navigation.forward", direction)
                .size(icon_size);
            let icon = match pinned {
                Some(tokens) => icon.tint(tokens.icon_color),
                None => icon, // default TintMode::ThemeMuted (runtime text_muted)
            };
            icon.quarter_turn_in(expanded, direction)
        } else {
            text(chevron_text_glyph)
                .size(text_size)
                .style(move |theme: &iced::Theme| {
                    let tokens = pinned.unwrap_or_else(|| {
                        crate::theme_lookup::resolve(theme, DisclosureTokens::from_palette)
                    });
                    iced::widget::text::Style {
                        color: Some(tokens.icon_color),
                    }
                })
                .into()
        };

        // Label: single-line, no-wrap, end-elided so a long label cannot
        // wrap or overflow the fixed-height row. `.full_width()` gives the
        // ellipsis a bound to elide against (it takes the row space left of the
        // chevron). Ink follows the pinned-or-runtime token like the chevron,
        // via `color_resolved` so a light/dark/custom theme swap re-tints it.
        let mut label_widget = enz_text(self.label)
            .size_px(text_size)
            .single_line_elided()
            .full_width()
            .color_resolved(move |theme: &iced::Theme| {
                pinned
                    .unwrap_or_else(|| {
                        crate::theme_lookup::resolve(theme, DisclosureTokens::from_palette)
                    })
                    .text_color
            });
        if self.audit_elision {
            // Name the finding with the disclosure's routable id, so the
            // report says which disclosure elided rather than [anonymous].
            label_widget = label_widget
                .audit_elision(true)
                .audit_elision_id(id.clone());
        }

        let content = row![chevron_widget, label_widget]
            .mirrored(rtl)
            .spacing(icon_gap)
            .align_y(Alignment::Center);

        // Semantics: a DisclosureTriangle carrying the label as its name and
        // the expandable/expanded state + default action.
        let mut semantics =
            enzui_core::semantics::Semantics::new(enzui_core::semantics::Role::Disclosure)
                .expanded(expanded)
                .default_action();
        if !label_for_a11y.is_empty() {
            semantics = semantics.label(label_for_a11y);
        }

        let btn = crate::focus_button::focus_button_maybe(content, None)
            .on_press_with(move || (on_toggle)(!expanded))
            .width(Length::Fill)
            // A full-width row keeps its chevron+label at the leading edge; the
            // Fill width must not imply centring.
            .content_align_x(Alignment::Start)
            .height(Length::Fixed(row_height))
            .padding(Padding {
                top: 0.0,
                right: icon_gap,
                bottom: 0.0,
                left: icon_gap,
            })
            // Correctly rectangular: the child is a chevron (16 px)
            // + label text, both centered vertically in a 34 px row and
            // inset by icon_gap (8 px) horizontally.  The row_radius (5 px)
            // arc depth is ~1.5 px, well within the 8 px horizontal inset
            // and the ~8 px vertical centering gap.  No caller slot exposes
            // a full-bleed child path.  Verified in
            // `rounded_clip_widget_sweep::cleared_disclosure_row_fill_is_
            // itself_rounded`.
            .clip(true)
            .semantics(semantics)
            .style_focus(move |theme: &iced::Theme, status: FocusStatus| {
                let tokens = pinned.unwrap_or_else(|| {
                    crate::theme_lookup::resolve(theme, DisclosureTokens::from_palette)
                });
                disclosure_style(&tokens, status, row_radius)
            })
            .id(id.clone())
            // This widget's activation carrier is its toggle, so it
            // answers a routed Expand/Collapse under its own id.
            // Declared explicitly rather than inferred by
            // `activation::expand` falling back to `activate` -- that
            // fallback would also "succeed" on a control with no
            // toggle at all (a tree leaf row), firing whatever its
            // activation happens to be.
            .expand_alias(id);
        btn.into()
    }
}

/// Resolve the `FocusButton` [`Style`] for a disclosure row from its (already
/// resolved) [`DisclosureTokens`] and interaction [`FocusStatus`].
///
/// Split out of the style closure so a test can assert the resolved
/// hover/pressed/focus output, not the token fields. A token-only
/// assertion cannot catch a pressed transform that collapses back
/// onto hover; this function is where the transform happens.
///
/// EnzPressableSurface "row": transparent base, hover / pressed overlays
/// (pressed is a real RGB-darkened step over hover), and a focus ring when
/// keyboard-focused.
pub(crate) fn disclosure_style(
    tokens: &DisclosureTokens,
    status: FocusStatus,
    row_radius: f32,
) -> Style {
    let bg = match status {
        // Pressed is the composited PressedOverlay wash, not a darkened
        // hover — `darken` on a translucent overlay moves RGB the wrong way on
        // the light preset and collapsed pressed onto hover.
        FocusStatus::Pressed => Some(Background::Color(tokens.pressed_background)),
        FocusStatus::Hovered => Some(Background::Color(tokens.hover_background)),
        _ => None,
    };
    let border = if matches!(status, FocusStatus::Focused) {
        Border {
            color: tokens.focus_ring_color,
            width: tokens.focus_ring_width,
            radius: row_radius.into(),
        }
    } else {
        Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: row_radius.into(),
        }
    };
    Style {
        background: bg,
        text_color: tokens.text_color,
        border,
        ..Style::default()
    }
}

impl<'a, Message: Clone + 'a> From<EnzDisclosure<'a, Message>> for Element<'a, Message> {
    fn from(d: EnzDisclosure<'a, Message>) -> Self {
        d.into_element()
    }
}

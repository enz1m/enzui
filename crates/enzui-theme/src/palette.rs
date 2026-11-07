//! Colour tokens: which colour a given slot should be painted.
//!
//! Every colour in an app comes from here. A hand-mixed literal is invisible to
//! the theme and quietly breaks contrast on a preset switch.
//!
//! Backgrounds form a depth stack: `background` > `surface` > `surface_alt` >
//! `surface_elevated`. Hover is either opaque replacement (`surface_hover`) or
//! composited wash (`overlay_hover()` / `overlay_pressed()`). Text steps down:
//! `text` > `text_muted` > `text_subtle` > `disabled`.
//!
//! # Example
//!
//! ```
//! use enzui_theme::Palette;
//!
//! let p = Palette::dark();
//! let hover = p.overlay_hover();
//! assert!(hover.a < 1.0);
//! assert_eq!((hover.r, hover.g, hover.b), (p.text.r, p.text.g, p.text.b));
//! ```

use iced::Color;

/// The WCAG contrast clause a backdrop-resolved weak ink must satisfy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContrastFloor {
    /// Normal-size text, WCAG 2.1 SC 1.4.3: 4.5:1.
    Text,
    /// Meaningful graphics and identifiable UI components, SC 1.4.11: 3:1.
    NonText,
    /// Large text, SC 1.4.3: 3:1 when its size/weight qualifies.
    LargeText,
}

impl ContrastFloor {
    /// The minimum WCAG contrast ratio required by this clause.
    pub const fn minimum_ratio(self) -> f32 {
        match self {
            Self::Text => 4.5,
            Self::NonText | Self::LargeText => 3.0,
        }
    }
}

/// Status tones with palette-level, backdrop-aware text foregrounds.
///
/// Widget tone enums may include additional roles that do not resolve through
/// [`Palette::tone_on`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SemanticTone {
    /// Success status text (`tone_success_fg`).
    Success,
    /// Warning status text (`tone_warning_fg`).
    Warning,
    /// Error / danger status text (`tone_error_fg`).
    Error,
}

/// The five surface roles a widget can paint behind interactive content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceRole {
    /// Permanent alternate surface — [`Palette::surface_alt`]. Never changes
    /// with interaction state.
    Static,
    /// Hovered element painted with the opaque [`Palette::surface_hover`].
    Hover,
    /// Element that stays selected/active after the pointer leaves —
    /// [`Palette::surface_selected`].
    Selected,
    /// Translucent wash composited over the background, resolved by
    /// [`Palette::overlay_hover`] (`text` at the hover-overlay alpha, 0.07). Do
    /// not substitute an opaque surface token and call it this role.
    Overlay,
    /// Stronger translucent wash for the pressed state of a composited overlay,
    /// resolved by [`Palette::overlay_pressed`] using `text` at alpha `0.12`
    /// (approximately `0x1f` after 8-bit quantization).
    PressedOverlay,
}

impl SurfaceRole {
    /// Every variant of [`SurfaceRole`], in a fixed order.
    pub const ALL: &'static [SurfaceRole] = &[
        SurfaceRole::Static,
        SurfaceRole::Hover,
        SurfaceRole::Selected,
        SurfaceRole::Overlay,
        SurfaceRole::PressedOverlay,
    ];

    /// Resolves every current role to `Some(Color)`, including the translucent
    /// overlay roles.
    pub fn resolve(self, p: &Palette) -> Option<Color> {
        match self {
            SurfaceRole::Static => Some(p.surface_alt),
            SurfaceRole::Hover => Some(p.surface_hover),
            SurfaceRole::Selected => Some(p.surface_selected),
            SurfaceRole::Overlay => Some(p.overlay_hover()),
            SurfaceRole::PressedOverlay => Some(p.overlay_pressed()),
        }
    }
}

/// Core colour palette: all semantic colours used across the design system.
#[derive(Debug, Clone)]
pub struct Palette {
    /// Page background behind all surfaces.
    pub background: Color,
    /// Card/panel surface.
    pub surface: Color,
    /// Alternate surface, such as a sidebar body, sunken well or track. The
    /// Static role in [`SurfaceRole`]: a permanent surface that does not change
    /// with hover, press or select. Do not use this for a hover fill, which is
    /// [`Palette::surface_hover`], even though the two are equal by default.
    pub surface_alt: Color,
    /// Elevated surface (popovers, dialogs).
    pub surface_elevated: Color,

    /// Primary content text.
    pub text: Color,
    /// Secondary/muted text.
    pub text_muted: Color,
    /// Tertiary/subtle text (placeholders).
    pub text_subtle: Color,

    /// Replacement hover fill, the Hover role in [`SurfaceRole`]. Use when
    /// hovering an element replaces its whole fill with an opaque surface
    /// (neutral secondary button, list, menu or model-picker row hover). This is
    /// equal to [`Palette::surface_alt`] in the built-in presets but is a
    /// distinct role: read this field for hovers so a theme that separates the
    /// two renders the intended one.
    pub surface_hover: Color,
    /// Persistent-selection fill, the Selected role in [`SurfaceRole`]. Use for
    /// elements that stay visibly selected or active after the pointer leaves
    /// (selected sidebar row, active toolbar-icon toggle, selected tab). This
    /// equals [`Palette::selected`] by default, but read this field rather than
    /// the raw `selected` alias for surface selection.
    pub surface_selected: Color,
    /// Media and image letterbox surface: the inert field around an image that
    /// does not fill its box. Near-black in both presets on purpose, because a
    /// light letterbox would alter the perceived colour of the photo.
    pub image_canvas: Color,
    /// Video-letterbox surface. Built-in palettes use pure black, distinct from
    /// their near-black [`Palette::image_canvas`]; custom palettes may override it.
    pub video_canvas: Color,

    /// Decorative hairline for separators and card outlines.
    pub border: Color,
    /// Emphasized decorative border: heavier than [`Palette::border`] but not
    /// an interaction affordance.
    pub border_strong: Color,
    /// State-conveying control boundary: the resting outline of an interactive
    /// component, and the unfilled part of a track.
    pub control_border: Color,
    /// The hover/emphasis step above [`Palette::control_border`] for the same
    /// interactive role. Built-in values clear 3:1 on their common surfaces and
    /// remain perceptibly distinct from the resting border.
    pub control_border_strong: Color,

    /// Resting body of an enabled text-entry field (the same family as
    /// [`Palette::field_border`]). White on a light page, as macOS, Fluent and
    /// the web draw inputs, so an enabled field never reads as the grey well
    /// ([`Palette::surface_alt`]) a disabled one sits in; a lifted fill in dark
    /// mode.
    pub field: Color,
    /// Resting border of a text-entry field: `enz_text_field`,
    /// `enz_text_area`, `enz_search_field`, `enz_secure_field`,
    /// `enz_date_field`, `enz_combo_box`, `enz_autocomplete_field`,
    /// `enz_input_otp`. Built-in palettes make it a shade of the field body
    /// (`surface_alt`, 12% darker): the body identifies the field, the edge
    /// only holds its shape.
    pub field_border: Color,
    /// Hover outline for text-entry fields. Built-in palettes move the field
    /// body a quarter of the way to `text`, so the edge answers the pointer.
    pub field_border_strong: Color,

    /// Primary accent for links and active controls.
    pub accent: Color,
    /// Accent on hover.
    pub accent_hover: Color,
    /// Accent when pressed.
    pub accent_pressed: Color,
    /// Soft accent background.
    pub accent_soft: Color,

    /// Danger role colour, usable as a fill; use [`Palette::on_danger`] for ink
    /// painted on it.
    pub danger: Color,
    /// Danger soft background.
    pub danger_soft: Color,
    /// Destructive action foreground.
    pub destructive: Color,
    /// Success role colour, usable as a fill; use [`Palette::on_success`] for
    /// ink painted on it.
    pub success: Color,
    /// Success soft background.
    pub success_soft: Color,
    /// Warning role colour, usable as a fill; use [`Palette::on_warning`] for
    /// ink painted on it.
    pub warning: Color,
    /// Warning soft background.
    pub warning_soft: Color,
    /// Filled rating mark, before backdrop contrast adjustment.
    pub rating: Color,
    /// Informational role colour, usable as a fill; use [`Palette::on_info`] for
    /// ink painted on it.
    pub info: Color,
    /// Info soft background.
    pub info_soft: Color,

    /// Success status-text foreground. Read this, not `success`, for text.
    pub tone_success_fg: Color,
    /// Warning status-text foreground. Read this, not `warning`, for text.
    pub tone_warning_fg: Color,
    /// Error/danger status-text foreground. Read this, not `danger`, for text.
    pub tone_error_fg: Color,

    /// Scrim/overlay with meaningful alpha.
    pub overlay: Color,
    /// Elevation drop-shadow colour, with alpha: the ink cast by the elevation
    /// ladder (`EnzCard`, `EnzSurface`, `EnzElevation`, modal surfaces, the
    /// switch thumb).
    pub shadow: Color,

    /// Focus indicator colour.
    pub focus_ring: Color,
    /// Disabled foreground.
    pub disabled: Color,
    /// Raw selected or highlighted background. Prefer
    /// [`Palette::surface_selected`] when the semantic role is a selected
    /// surface.
    pub selected: Color,
    /// Error role colour. Built-in palettes initialize it to the same value as
    /// `danger`, but custom palettes may set the fields independently.
    pub error: Color,

    /// Primary button background.
    pub button_primary_bg: Color,
    /// Button primary hover background.
    pub button_primary_hover_bg: Color,
    /// Button primary pressed background.
    pub button_primary_pressed_bg: Color,
}

impl Palette {
    /// The dark mode of the built-in enzUI family ([`crate::FAMILY`]).
    pub fn dark() -> Self {
        // Neutral greys with the light palette's hair of cool and no violet
        // cast, at the luminance of the earlier tinted values.
        Self {
            background: hex(0x1D, 0x1D, 0x1F),
            surface: hex(0x27, 0x27, 0x29),
            surface_alt: hex(0x30, 0x30, 0x32),
            surface_elevated: hex(0x38, 0x38, 0x3A),
            surface_hover: hex(0x30, 0x30, 0x32),
            surface_selected: hex(0x45, 0x45, 0x47),
            image_canvas: hex(0x16, 0x17, 0x1B),
            video_canvas: hex(0x00, 0x00, 0x00),

            text: hex(0xF1, 0xF1, 0xF3),
            text_muted: hex(0xB3, 0xB3, 0xB5),
            text_subtle: hex(0x84, 0x84, 0x86),

            border: hex(0x49, 0x49, 0x4B),
            border_strong: hex(0x5D, 0x5D, 0x5F),
            control_border: hex(0x88, 0x88, 0x8A),
            control_border_strong: hex(0xA8, 0xA8, 0xAA),

            // The field body is the elevated step, one above `surface_alt`: a
            // field on a sidebar (`surface_alt`) was the sidebar's own colour
            // and vanished. Its edge stays a darker shade, and on hover the
            // edge moves a quarter of the way to the text.
            field: hex(0x38, 0x38, 0x3A),
            field_border: hex(0x2A, 0x2A, 0x2C),
            field_border_strong: hex(0x60, 0x60, 0x62),

            accent: hex(0x0A, 0x84, 0xFF),
            accent_hover: hex(0x40, 0x9C, 0xFF),
            accent_pressed: hex(0x00, 0x6E, 0xDB),
            accent_soft: hex(0x18, 0x3A, 0x60),

            danger: hex(0xF8, 0x71, 0x71),
            danger_soft: hex(0x4A, 0x1F, 0x25),
            destructive: hex(0xF8, 0x71, 0x71),
            success: hex(0x30, 0xD1, 0x58),
            success_soft: hex(0x18, 0x46, 0x28),
            warning: hex(0xFF, 0xB3, 0x40),
            warning_soft: hex(0x4E, 0x3A, 0x18),
            rating: hex(0xF5, 0x9E, 0x0B),
            info: hex(0x64, 0xD2, 0xFF),
            info_soft: hex(0x1A, 0x3E, 0x4E),

            tone_success_fg: hex(0x4A, 0xDE, 0x80),
            tone_warning_fg: hex(0xFB, 0xBF, 0x24),
            tone_error_fg: hex(0xF8, 0x71, 0x71),

            overlay: Color::from_rgba(0.0, 0.0, 0.0, 0.51),
            shadow: Color::from_rgba8(0x00, 0x00, 0x00, 0x3a as f32 / 255.0),
            focus_ring: hex(0x0A, 0x84, 0xFF),
            disabled: hex(0x6C, 0x6C, 0x6E),
            selected: hex(0x45, 0x45, 0x47),
            error: hex(0xF8, 0x71, 0x71),

            // A raised tone above every surface rung, `surface_elevated`
            // included, so the default button reads by its fill alone, on a
            // dialog as on the page.
            button_primary_bg: hex(0x44, 0x44, 0x46),
            button_primary_hover_bg: hex(0x4D, 0x4D, 0x4F),
            button_primary_pressed_bg: hex(0x3A, 0x3A, 0x3C),
        }
    }

    /// The light mode of the built-in enzUI family ([`crate::FAMILY`]).
    pub fn light() -> Self {
        Self {
            // Neutral greys with no hue cast. A near-white page, as GNOME's
            // light style has it, with white cards and fields on it and grey
            // buttons that stand out from it by their fill alone.
            background: hex(0xF6, 0xF6, 0xF8),
            surface: hex(0xFF, 0xFF, 0xFF),
            surface_alt: hex(0xE5, 0xE5, 0xE9),
            // Popovers and dialogs lift off the page by their shadow; a hair
            // below white keeps an unshadowed raised tile apart from a white
            // surface.
            surface_elevated: hex(0xFC, 0xFC, 0xFD),
            surface_hover: hex(0xE4, 0xE4, 0xE8),
            surface_selected: hex(0xD9, 0xD9, 0xDE),
            image_canvas: hex(0x1A, 0x1B, 0x1F),
            video_canvas: hex(0x00, 0x00, 0x00),

            text: hex(0x1D, 0x1D, 0x1F),
            text_muted: hex(0x63, 0x63, 0x68),
            // Keeps 11 px caption text above 4.5:1 on background and surface.
            text_subtle: hex(0x6A, 0x6A, 0x70),

            border: hex(0xDE, 0xDE, 0xE3),
            border_strong: hex(0xC9, 0xC9, 0xCE),
            // Checkbox and radio rims hold 3:1 on their white box.
            control_border: hex(0x92, 0x92, 0x97),
            control_border_strong: hex(0x63, 0x63, 0x66),

            // A white field edged by a soft hairline, as macOS and the web
            // draw inputs; the white body on the grey page and the label
            // identify it. On hover the edge darkens toward the text.
            field: hex(0xFF, 0xFF, 0xFF),
            field_border: hex(0xD0, 0xD0, 0xD5),
            field_border_strong: hex(0xAE, 0xAE, 0xB4),

            accent: hex(0x00, 0x6C, 0xDC),
            // A light theme's hover and press darken, so the white label
            // keeps its contrast.
            accent_hover: hex(0x00, 0x62, 0xC8),
            accent_pressed: hex(0x00, 0x58, 0xB4),
            accent_soft: hex(0xE3, 0xEE, 0xFC),

            danger: hex(0xD7, 0x00, 0x15),
            danger_soft: hex(0xFF, 0xE5, 0xE7),
            destructive: hex(0xD7, 0x00, 0x15),
            success: hex(0x1E, 0x7E, 0x34),
            success_soft: hex(0xE3, 0xF6, 0xE8),
            warning: hex(0x9A, 0x5B, 0x00),
            warning_soft: hex(0xFF, 0xF3, 0xD6),
            rating: hex(0xF5, 0x9E, 0x0B),
            info: hex(0x00, 0x6E, 0xA8),
            info_soft: hex(0xE2, 0xF1, 0xFB),

            tone_success_fg: hex(0x1E, 0x7E, 0x34),
            tone_warning_fg: hex(0x9A, 0x5B, 0x00),
            tone_error_fg: hex(0xD7, 0x00, 0x15),

            overlay: Color::from_rgba(0.0, 0.0, 0.0, 0.35),
            shadow: Color::from_rgba(0.0, 0.0, 0.0, 0.13),
            focus_ring: hex(0x00, 0x6C, 0xDC),
            // Keeps disabled ink above 3:1 while below `text_subtle` in the emphasis ramp.
            disabled: hex(0x86, 0x86, 0x8B),
            selected: hex(0xD9, 0xD9, 0xDE),
            error: hex(0xD7, 0x00, 0x15),

            // A grey fill on the near-white page, with no edge: the fill
            // alone sets the button apart.
            button_primary_bg: hex(0xE8, 0xE8, 0xEC),
            button_primary_hover_bg: hex(0xDF, 0xDF, 0xE4),
            button_primary_pressed_bg: hex(0xD5, 0xD5, 0xDB),
        }
    }
}

impl Palette {
    /// This palette with `accent` as its accent family: the desktop's accent
    /// colour, for example. Hover, pressed, the soft background and the focus
    /// ring derive from it for this palette's mode; the accent's alpha is
    /// ignored.
    #[must_use]
    pub fn with_accent(mut self, accent: Color) -> Self {
        let accent = Color { a: 1.0, ..accent };
        let (hover, pressed, soft) = if self.is_dark() {
            (
                color_math::mix_srgb(accent, Color::WHITE, 0.2),
                color_math::mix_srgb(accent, Color::BLACK, 0.15),
                color_math::mix_srgb(self.background, accent, 0.3),
            )
        } else {
            (
                color_math::mix_srgb(accent, Color::BLACK, 0.08),
                color_math::mix_srgb(accent, Color::BLACK, 0.16),
                color_math::mix_srgb(Color::WHITE, accent, 0.15),
            )
        };
        self.accent = accent;
        self.accent_hover = hover;
        self.accent_pressed = pressed;
        self.accent_soft = soft;
        self.focus_ring = accent;
        self
    }

    /// This palette for a user who asked for more contrast (GNOME's High
    /// Contrast, the portal's `contrast` preference): black on white, or
    /// white on black, in this palette's mode.
    ///
    /// Surfaces become the page colour and are told apart by edges in the
    /// text colour, with filled controls a light grey step; secondary text is drawn in the text colour; the accent,
    /// state colours and focus ring are raised to 7:1 against the page
    /// (WCAG AAA); hover and selection stay visible as grey steps.
    #[must_use]
    pub fn high_contrast(self) -> Self {
        let dark = self.is_dark();
        let (page, ink) = if dark {
            (Color::BLACK, Color::WHITE)
        } else {
            (Color::WHITE, Color::BLACK)
        };
        let step = |amount: f32| color_math::mix_srgb(page, ink, amount);
        // Raise a colour to `target` against the page; where its hue cannot
        // get there in gamut, move it toward the ink until it does.
        let reach = |color: Color, target: f32| {
            let raised = raise_to_target(color, page, target);
            if raised.meets_floor() && wcag_contrast(raised.color(), page) >= target {
                return raised.color();
            }
            (1..=20)
                .map(|step| color_math::mix_srgb(color, ink, step as f32 / 20.0))
                .find(|mixed| wcag_contrast(*mixed, page) >= target)
                .unwrap_or(ink)
        };
        let raised = |color: Color| reach(color, 7.0);
        let accent = raised(self.accent);
        let mut palette = Self {
            background: page,
            surface: page,
            // Filled controls (fields, steppers) keep a visible body.
            surface_alt: step(0.12),
            surface_elevated: page,
            surface_hover: step(0.16),
            surface_selected: step(0.28),
            text: ink,
            text_muted: ink,
            text_subtle: ink,
            border: ink,
            border_strong: ink,
            control_border: ink,
            control_border_strong: ink,
            field: page,
            field_border: ink,
            field_border_strong: ink,
            danger: raised(self.danger),
            destructive: raised(self.destructive),
            success: raised(self.success),
            warning: raised(self.warning),
            rating: raised(self.rating),
            info: raised(self.info),
            tone_success_fg: raised(self.tone_success_fg),
            tone_warning_fg: raised(self.tone_warning_fg),
            tone_error_fg: raised(self.tone_error_fg),
            error: raised(self.error),
            // Disabled ink stays dimmer than text but readable.
            disabled: reach(self.disabled, 4.5),
            selected: step(0.28),
            button_primary_bg: page,
            button_primary_hover_bg: step(0.16),
            button_primary_pressed_bg: step(0.28),
            ..self
        }
        .with_accent(accent);
        // The accent family's soft tint still reads as a fill, not a page.
        palette.accent_soft = color_math::mix_srgb(page, accent, 0.25);
        palette
    }

    /// Resolves subtle ink against `backdrop`: keep `text_subtle` when it clears
    /// 4.5:1, otherwise promote to `text_muted` when it clears `floor`, then
    /// fall back to `text`.
    pub fn subtle_on(&self, backdrop: Color, floor: ContrastFloor) -> Color {
        const SUBTLE_BASELINE: f32 = 4.5;
        let solid = self.solid_backdrop(backdrop);
        if wcag_contrast(over(self.text_subtle, solid), solid) >= SUBTLE_BASELINE {
            self.text_subtle
        } else if wcag_contrast(over(self.text_muted, solid), solid) >= floor.minimum_ratio() {
            self.text_muted
        } else {
            self.text
        }
    }

    /// `backdrop` as the solid colour text on it actually sits on. Glass
    /// palettes' fills are white or black at a low alpha; judging one as if
    /// opaque read a faint white wash over dark glass as white, and pushed
    /// secondary ink dark (about 2:1 on the glass it was drawn on). A
    /// translucent fill is composited over this palette's background, or over
    /// the ground its ink is made for when that is translucent too.
    pub fn solid_backdrop(&self, backdrop: Color) -> Color {
        if backdrop.a >= 0.999 {
            return Color { a: 1.0, ..backdrop };
        }
        let ground = if self.background.a >= 0.999 {
            self.background
        } else if color_math::wcag_luminance(Color {
            a: 1.0,
            ..self.text
        }) > 0.4
        {
            hex(0x1C, 0x1C, 0x1E)
        } else {
            hex(0xF5, 0xF5, 0xF7)
        };
        over(backdrop, ground)
    }

    /// Resolves subtle ink for the 4.5:1 normal-text floor using the
    /// `text_subtle` → `text_muted` → `text` promotion ladder.
    pub fn text_subtle_on(&self, backdrop: Color) -> Color {
        self.subtle_on(backdrop, ContrastFloor::Text)
    }

    /// Keeps `text_muted` when it clears 4.5:1 on `backdrop`; otherwise moves
    /// it toward the readable end just far enough to reach 4.5:1.
    pub fn text_muted_on(&self, backdrop: Color) -> Color {
        // As muted as 4.5:1 allows: the muted ink moved toward the readable
        // end only as far as it must, so secondary text stays visibly
        // secondary rather than snapping to the full text colour.
        // Translucent (glass) muted ink stays translucent while it reads, so it
        // keeps the glass's vibrancy; both are judged as composited.
        let solid = self.solid_backdrop(backdrop);
        let shown = over(self.text_muted, solid);
        if self.text_muted.a < 1.0 && wcag_contrast(shown, solid) >= 4.5 {
            return self.text_muted;
        }
        lift_brand_to_text_contrast(shown, solid, 4.5)
    }

    /// Keeps `disabled` when it clears 3:1; otherwise targets 3.1:1 by mixing
    /// the first usable `text_subtle` or `text_muted` rung toward `backdrop`.
    pub fn disabled_on(&self, backdrop: Color) -> Color {
        const FLOOR: f32 = 3.0;
        // The 3.1 target leaves headroom for 8-bit quantization above the 3.0 floor.
        const TARGET: f32 = 3.1;
        if wcag_contrast(self.disabled, backdrop) >= FLOOR {
            self.disabled
        } else if wcag_contrast(self.text_subtle, backdrop) >= FLOOR {
            mix_to_contrast(self.text_subtle, backdrop, TARGET)
        } else {
            mix_to_contrast(self.text_muted, backdrop, TARGET)
        }
    }

    /// Keeps `preferred` only when its contrast is from 3:1 through half the
    /// enabled-text contrast; otherwise resolves through [`Self::disabled_on`].
    pub fn disabled_on_with_preferred(&self, preferred: Color, backdrop: Color) -> Color {
        const FLOOR: f32 = 3.0;
        const MAX_ENABLED_FRACTION: f32 = 0.5;
        let preferred_contrast = wcag_contrast(preferred, backdrop);
        let enabled_contrast = wcag_contrast(self.text, backdrop);
        if preferred_contrast >= FLOOR
            && preferred_contrast <= enabled_contrast * MAX_ENABLED_FRACTION
        {
            preferred
        } else {
            self.disabled_on(backdrop)
        }
    }

    /// Searches the input's fixed OkLab `a`/`b` path for a quantized colour at
    /// the 4.5:1 text floor, returning [`RaisedInk::Impossible`] when that path
    /// cannot reach it.
    pub fn tone_on(&self, tone: SemanticTone, backdrop: Color) -> RaisedInk {
        let raw = match tone {
            SemanticTone::Success => self.tone_success_fg,
            SemanticTone::Warning => self.tone_warning_fg,
            SemanticTone::Error => self.tone_error_fg,
        };
        raise_to_floor(raw, backdrop, ContrastFloor::Text)
    }

    /// Whether this palette is for content on glass ([`Self::on_glass`]):
    /// its surface roles are translucent there and opaque everywhere else.
    #[must_use]
    pub fn is_on_glass(&self) -> bool {
        self.surface.a < 1.0
    }

    /// Whether this is a high-contrast palette ([`Self::high_contrast`]):
    /// control edges in the text colour, and text at least 15:1 on the page.
    /// Widgets that edge a control with a shade of its fill use the edge
    /// colour instead.
    pub fn is_high_contrast(&self) -> bool {
        self.control_border == self.text && wcag_contrast(self.text, self.background) >= 15.0
    }

    /// This palette for content on a glass surface.
    ///
    /// Control fills, hover and selection washes and borders become white at
    /// a low alpha, so the glass shows through them and they read as lighter
    /// glass rather than opaque grey slabs. Text, accent and status roles are
    /// unchanged. Widgets built with pinned tokens are unaffected.
    #[must_use]
    pub fn on_glass(&self) -> Self {
        let white = |a: f32| Color {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a,
        };
        let tint = |c: Color| Color { a: c.a * 0.55, ..c };
        // (fill, raised fill, hover, pressed, selected, strong border).
        let (fill, raised, hover, pressed, selected, strong) = if self.is_dark() {
            (0.08, 0.16, 0.22, 0.28, 0.20, 0.22)
        } else {
            (0.30, 0.50, 0.65, 0.35, 0.60, 0.70)
        };
        // Glass separates by fill and by its own rim of light, not by
        // outlines: a hairline on glass reads as a sticker's edge. Resting
        // borders vanish; the strong ones stay for hover and focus.
        let none = white(0.0);
        Self {
            surface: white(fill),
            surface_alt: white(fill + 0.04),
            surface_elevated: white(raised),
            surface_hover: white(hover - 0.08),
            surface_selected: white(selected),
            border: none,
            border_strong: white(strong),
            control_border: none,
            control_border_strong: white(strong),
            field: white(fill + 0.04),
            field_border: none,
            field_border_strong: white(strong),
            button_primary_bg: white(raised),
            button_primary_hover_bg: white(hover),
            button_primary_pressed_bg: white(pressed),
            // Soft status tints keep their hue and let the glass through.
            accent_soft: tint(self.accent_soft),
            danger_soft: tint(self.danger_soft),
            success_soft: tint(self.success_soft),
            warning_soft: tint(self.warning_soft),
            rating: self.rating,
            info_soft: tint(self.info_soft),
            // Secondary ink is the primary ink, fainter, so it stays vibrant
            // over whatever the glass shows instead of a fixed grey.
            text_muted: self.text.scale_alpha(0.78),
            text_subtle: self.text.scale_alpha(0.62),
            ..self.clone()
        }
    }

    /// [`Self::on_glass`] for glass whose ink adapts to its backdrop:
    /// `light_ink` is `1.0` for light ink on darkened glass, `0.0` for dark
    /// ink on lightened glass, and blends in between during a crossfade.
    ///
    /// Both ends start from this palette, so its accents and tones stay; only
    /// the mode-bound roles (background and ink) come from the preset of the
    /// other mode where the end needs it.
    #[must_use]
    pub fn on_glass_ink(&self, light_ink: f32) -> Self {
        let t = if light_ink.is_finite() {
            light_ink.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let with_mode_of = |preset: Palette| Palette {
            background: preset.background,
            text: preset.text,
            text_muted: preset.text_muted,
            text_subtle: preset.text_subtle,
            tone_success_fg: preset.tone_success_fg,
            tone_warning_fg: preset.tone_warning_fg,
            tone_error_fg: preset.tone_error_fg,
            disabled: preset.disabled,
            ..self.clone()
        };
        let (light_ink_glass, dark_ink) = if self.is_dark() {
            (self.on_glass(), with_mode_of(Palette::light()).on_glass())
        } else {
            (with_mode_of(Palette::dark()).on_glass(), self.on_glass())
        };
        let mix = |a: Color, b: Color| Color {
            r: a.r + (b.r - a.r) * t,
            g: a.g + (b.g - a.g) * t,
            b: a.b + (b.b - a.b) * t,
            a: a.a + (b.a - a.a) * t,
        };
        Self {
            background: mix(dark_ink.background, light_ink_glass.background),
            surface: mix(dark_ink.surface, light_ink_glass.surface),
            surface_alt: mix(dark_ink.surface_alt, light_ink_glass.surface_alt),
            surface_elevated: mix(dark_ink.surface_elevated, light_ink_glass.surface_elevated),
            text: mix(dark_ink.text, light_ink_glass.text),
            text_muted: mix(dark_ink.text_muted, light_ink_glass.text_muted),
            text_subtle: mix(dark_ink.text_subtle, light_ink_glass.text_subtle),
            surface_hover: mix(dark_ink.surface_hover, light_ink_glass.surface_hover),
            surface_selected: mix(dark_ink.surface_selected, light_ink_glass.surface_selected),
            image_canvas: mix(dark_ink.image_canvas, light_ink_glass.image_canvas),
            video_canvas: mix(dark_ink.video_canvas, light_ink_glass.video_canvas),
            border: mix(dark_ink.border, light_ink_glass.border),
            border_strong: mix(dark_ink.border_strong, light_ink_glass.border_strong),
            control_border: mix(dark_ink.control_border, light_ink_glass.control_border),
            control_border_strong: mix(
                dark_ink.control_border_strong,
                light_ink_glass.control_border_strong,
            ),
            field: mix(dark_ink.field, light_ink_glass.field),
            field_border: mix(dark_ink.field_border, light_ink_glass.field_border),
            field_border_strong: mix(
                dark_ink.field_border_strong,
                light_ink_glass.field_border_strong,
            ),
            accent: mix(dark_ink.accent, light_ink_glass.accent),
            accent_hover: mix(dark_ink.accent_hover, light_ink_glass.accent_hover),
            accent_pressed: mix(dark_ink.accent_pressed, light_ink_glass.accent_pressed),
            accent_soft: mix(dark_ink.accent_soft, light_ink_glass.accent_soft),
            danger: mix(dark_ink.danger, light_ink_glass.danger),
            danger_soft: mix(dark_ink.danger_soft, light_ink_glass.danger_soft),
            destructive: mix(dark_ink.destructive, light_ink_glass.destructive),
            success: mix(dark_ink.success, light_ink_glass.success),
            success_soft: mix(dark_ink.success_soft, light_ink_glass.success_soft),
            warning: mix(dark_ink.warning, light_ink_glass.warning),
            warning_soft: mix(dark_ink.warning_soft, light_ink_glass.warning_soft),
            rating: mix(dark_ink.rating, light_ink_glass.rating),
            info: mix(dark_ink.info, light_ink_glass.info),
            info_soft: mix(dark_ink.info_soft, light_ink_glass.info_soft),
            tone_success_fg: mix(dark_ink.tone_success_fg, light_ink_glass.tone_success_fg),
            tone_warning_fg: mix(dark_ink.tone_warning_fg, light_ink_glass.tone_warning_fg),
            tone_error_fg: mix(dark_ink.tone_error_fg, light_ink_glass.tone_error_fg),
            overlay: mix(dark_ink.overlay, light_ink_glass.overlay),
            shadow: mix(dark_ink.shadow, light_ink_glass.shadow),
            focus_ring: mix(dark_ink.focus_ring, light_ink_glass.focus_ring),
            disabled: mix(dark_ink.disabled, light_ink_glass.disabled),
            selected: mix(dark_ink.selected, light_ink_glass.selected),
            error: mix(dark_ink.error, light_ink_glass.error),
            button_primary_bg: mix(
                dark_ink.button_primary_bg,
                light_ink_glass.button_primary_bg,
            ),
            button_primary_hover_bg: mix(
                dark_ink.button_primary_hover_bg,
                light_ink_glass.button_primary_hover_bg,
            ),
            button_primary_pressed_bg: mix(
                dark_ink.button_primary_pressed_bg,
                light_ink_glass.button_primary_pressed_bg,
            ),
        }
    }

    /// Classifies the palette as dark when a weighted sum of its gamma-encoded
    /// background channels is below `0.5`.
    ///
    /// This is a preset-independent heuristic, not WCAG relative luminance.
    pub fn is_dark(&self) -> bool {
        let c = self.background;
        let lum = 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b;
        lum < 0.5
    }

    /// Composited hover overlay wash, the resolution of
    /// [`SurfaceRole::Overlay`]: `text` at alpha 0.07, composited over whatever
    /// is behind rather than replacing it.
    pub fn overlay_hover(&self) -> Color {
        let t = self.text;
        Color::from_rgba(t.r, t.g, t.b, 0.07)
    }

    /// Composited pressed overlay wash, the resolution of
    /// [`SurfaceRole::PressedOverlay`]: `text` at alpha 0.12, one step stronger
    /// than [`Palette::overlay_hover`] so pressed reads heavier over the same
    /// background.
    pub fn overlay_pressed(&self) -> Color {
        let t = self.text;
        Color::from_rgba(t.r, t.g, t.b, 0.12)
    }

    /// The unfilled track of a slider, progress bar or gauge: a soft groove,
    /// the text at 18% alpha in light mode and 14% in dark, so it reads on
    /// the page and on any card or pane alike.
    ///
    /// What a meter must show is where the fill ends, so the fill is what has
    /// to stand apart. Against this track every stock fill (accent, success,
    /// warning, danger) clears 3:1 in both modes. The former `control_border`
    /// track cleared 3:1 against the page instead and left the fills at
    /// 1.03–1.98:1 against it, while painting a heavy grey bar.
    pub fn meter_track(&self) -> Color {
        if self.is_on_glass() {
            // A lighter groove of glass rather than an opaque bar.
            let alpha = if self.is_dark() { 0.18 } else { 0.40 };
            return Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: alpha,
            };
        }
        // Translucent text, so the groove reads on whatever it sits on: on
        // the page it composites to exactly the old opaque mix, and on a
        // lighter card or pane it stays a groove instead of vanishing.
        let depth = if self.is_dark() { 0.14 } else { 0.18 };
        Color {
            a: depth,
            ..self.text
        }
    }

    /// The meter track's outline: the track's own colour, so a default meter
    /// paints no separate line. A theme that wants a drawn edge sets the
    /// widget's `track_border` token.
    pub fn meter_track_border(&self) -> Color {
        self.meter_track()
    }

    /// The filled accent for selected segments (a toggled toolbar format,
    /// the chosen cell of a button group), as macOS toggled segments and the
    /// framework's filter chips draw selection; surface-ladder neutrals cannot
    /// provide the required 3:1 state cue in both presets.
    pub fn segment_selected_bg(&self) -> Color {
        self.filled_color(self.accent).color
    }

    /// Selected accent fill with 3:1 contrast against its segment track.
    ///
    /// Dark mode uses a deep control fill on the page-background track;
    /// light mode uses the accent on `surface_alt`.
    pub fn segment_selected_accent_bg(&self) -> Color {
        if self.is_dark() {
            lift_brand_to_text_contrast(self.filled_color(self.accent).color, self.background, 3.0)
        } else {
            lift_brand_to_text_contrast(self.accent, self.surface_alt, 3.0)
        }
    }

    /// The foreground for a selected accent segment, derived from the resolved
    /// [`Palette::segment_selected_accent_bg`] rather than the raw accent.
    pub fn segment_selected_accent_fg(&self) -> Color {
        legible_ink_on(self.segment_selected_accent_bg())
    }

    /// The selected fill mixed 7% toward `text`; this may lighten or darken the
    /// base and never substitutes the unselected hover fill.
    pub fn segment_selected_hover_bg(&self) -> Color {
        mix_srgb(self.segment_selected_bg(), self.text, 0.07)
    }

    /// The pressed fill of a selected segmented cell — one step stronger
    /// than [`Palette::segment_selected_hover_bg`].
    pub fn segment_selected_pressed_bg(&self) -> Color {
        mix_srgb(self.segment_selected_bg(), self.text, 0.12)
    }

    /// The seam between two adjacent selected segments: the selected fill a
    /// shade deeper, so the cells read as separate without a bright line
    /// cutting through the fill.
    pub fn segment_selected_seam(&self) -> Color {
        mix_srgb(self.segment_selected_bg(), Color::BLACK, 0.22)
    }

    /// Legible label and icon ink chosen for [`Palette::segment_selected_bg`].
    pub fn segment_selected_fg(&self) -> Color {
        self.filled_color(self.accent).text
    }

    /// Resolves an opaque control fill together with its label and icon ink.
    ///
    /// A fill that reads best with white text by APCA keeps white, and the
    /// fill is deepened (same hue, lower OkLab lightness) until white reaches 4.5:1,
    /// or 4.8:1 on a dark palette for headroom: an accent button stays blue
    /// with a white label instead of turning its label black. A fill too
    /// light for white (yellow, amber, a pale custom colour) keeps its colour
    /// and takes a near-black ink in its own hue. Raw accent and status
    /// colours remain available for links and indicators.
    #[must_use]
    pub fn filled_color(&self, fill: Color) -> iced::theme::palette::Pair {
        let fill = Color { a: 1.0, ..fill };
        if color_math::prefers_white(fill) {
            let target = if self.is_dark() { 4.8 } else { 4.5 };
            iced::theme::palette::Pair {
                color: color_math::deepen_for_white(fill, target),
                text: Color::WHITE,
            }
        } else {
            iced::theme::palette::Pair {
                color: fill,
                text: legible_ink_on(fill),
            }
        }
    }

    /// The label and icon ink for any coloured `fill`: white where it reads
    /// best, else a near-black in the fill's hue. See
    /// [`Self::filled_color`] for fills that carry text and may be deepened.
    pub fn ink_on(fill: Color) -> Color {
        legible_ink_on(fill)
    }

    /// `color` with at least `target` contrast against `surface`, moved in
    /// lightness (darker on a light surface, lighter on a dark one) with its
    /// hue kept: a gold that must hold 3:1 on a white page turns a deeper
    /// amber, not olive or grey.
    pub fn with_contrast_on(surface: Color, color: Color, target: f32) -> Color {
        color_math::shift_to_contrast(color, surface, target)
    }

    /// Returns the label ink for the raw `accent` (see [`Self::ink_on`]).
    pub fn on_accent(&self) -> Color {
        legible_ink_on(self.accent)
    }

    /// Returns the label ink for `danger` (see [`Self::ink_on`]).
    pub fn on_danger(&self) -> Color {
        legible_ink_on(self.danger)
    }

    /// Returns the label ink for `success` (see [`Self::ink_on`]).
    pub fn on_success(&self) -> Color {
        legible_ink_on(self.success)
    }

    /// Returns the label ink for `warning` (see [`Self::ink_on`]).
    pub fn on_warning(&self) -> Color {
        legible_ink_on(self.warning)
    }

    /// Returns the label ink for `info` (see [`Self::ink_on`]).
    pub fn on_info(&self) -> Color {
        legible_ink_on(self.info)
    }

    /// Returns the label ink for `control_border_strong` (see [`Self::ink_on`]).
    pub fn on_control_border_strong(&self) -> Color {
        legible_ink_on(self.control_border_strong)
    }

    /// Returns `brand` unchanged when it clears 4.5:1 on `surface`; otherwise
    /// mixes toward the higher-contrast black or white endpoint until it does,
    /// returning that endpoint if the target is unreachable.
    pub fn brand_text_on(surface: Color, brand: Color) -> Color {
        lift_brand_to_text_contrast(brand, surface, 4.5)
    }

    /// [`Self::brand_text_on`] with a caller-chosen contrast floor, for a
    /// surface only known approximately (glass shows what lies behind it, so
    /// ink judged against an estimate needs a margin above 4.5:1).
    pub fn brand_text_on_at(surface: Color, brand: Color, floor: f32) -> Color {
        lift_brand_to_text_contrast(brand, surface, floor.max(1.0))
    }

    /// Recovers an embedded enzUI palette byte-for-byte; stock iced themes are
    /// reconstructed from swatches, with unchannelled roles taken from the
    /// nearest dark or light preset.
    pub fn from_iced(theme: &iced::Theme) -> Self {
        // The extension payload preserves every enzUI palette role byte-for-byte.
        if let Some(palette) = crate::EnzTheme::palette_from_iced(theme) {
            return palette;
        }

        // Stock themes lack the payload, so unchannelled roles keep preset values.
        fallback_cache::resolve(*theme.palette(), Self::__from_iced_fallback_do_not_call)
    }
}

mod fallback_cache {
    use super::Palette;
    use std::cell::RefCell;

    thread_local! {
        pub(super) static CACHE:
            RefCell<Option<(iced::theme::palette::Palette, Palette)>>
                = const { RefCell::new(None) };
    }

    // One-slot cache: matching input is a hit; other input replaces the entry.
    pub(super) fn resolve(
        ip: iced::theme::palette::Palette,
        compute: impl FnOnce(iced::theme::palette::Palette) -> Palette,
    ) -> Palette {
        CACHE.with(|slot| {
            let mut slot = slot.borrow_mut();
            if let Some((cached_ip, cached_palette)) = slot.as_ref()
                && *cached_ip == ip
            {
                return cached_palette.clone();
            }
            let computed = compute(ip);
            *slot = Some((ip, computed.clone()));
            computed
        })
    }

    #[cfg(test)]
    pub(super) fn __clear_for_tests() {
        CACHE.with(|slot| *slot.borrow_mut() = None);
    }
}

impl Palette {
    // Takes the palette by value so cache misses can use this function as a callback.
    fn __from_iced_fallback_do_not_call(ip: iced::theme::palette::Palette) -> Palette {
        let ip = &ip;
        let mut p = if ip.is_dark {
            Self::dark()
        } else {
            Self::light()
        };

        let bg = ip.background;
        let primary = ip.primary;
        let success = ip.success;
        let warning = ip.warning;
        let danger = ip.danger;

        // Backgrounds and surfaces: the exact inverse of the tier assignment in
        // `EnzTheme::iced_theme`'s `enzui_extended_palette`, where each role lives
        // in a distinct tier, so a palette that went out through `iced_theme`
        // comes back byte-identical.
        p.background = bg.base.color;
        p.surface_elevated = bg.weakest.color;
        p.surface = bg.weaker.color;
        p.surface_alt = bg.weak.color;
        p.surface_hover = bg.neutral.color;
        p.surface_selected = bg.strong.color;
        // The transport has one selected-surface slot. It carries
        // `surface_selected`, so an independently customized `selected` value
        // collapses to `surface_selected` on fallback reconstruction.
        p.selected = bg.strong.color;

        // Text tiers. The forward mapping ferries muted, subtle and disabled
        // through `bg.weakest.text`, `bg.weaker.text` and `bg.weak.text` via
        // `carry`, bypassing `readable()`, so an enzUI-produced theme round-trips
        // these values byte for byte.
        p.text = bg.base.text;
        // The repair condition is "the ferried tiers are not a monotone emphasis
        // ramp", not "all three are byte-identical".
        let is_monotone_ramp = {
            let bg_c = bg.base.color;
            let d = |c: Color| srgb_distance(c, bg_c);
            let d_text = d(bg.base.text);
            let d_muted = d(bg.weakest.text);
            let d_subtle = d(bg.weaker.text);
            let d_disabled = d(bg.weak.text);
            d_text > d_muted && d_muted > d_subtle && d_subtle > d_disabled
        };
        if !is_monotone_ramp {
            // Re-derive the three tiers with fixed gamma-sRGB interpolation
            // fractions from `base.text` toward the background.
            p.text_muted = mix_srgb(bg.base.text, bg.base.color, 0.30);
            p.text_subtle = mix_srgb(bg.base.text, bg.base.color, 0.50);
            p.disabled = mix_srgb(bg.base.text, bg.base.color, 0.62);
        } else {
            p.text_muted = bg.weakest.text;
            p.text_subtle = bg.weaker.text;
            p.disabled = bg.weak.text;
        }

        p.border = bg.stronger.color;
        p.border_strong = bg.strongest.color;

        // Iced has no swatch slots for interactive borders; derive them from its text/background ramp.
        p.control_border = mix_to_contrast(bg.base.text, bg.base.color, 3.4);
        p.control_border_strong = mix_to_contrast(bg.base.text, bg.base.color, 5.0);

        // A field's edge is a shade of its `surface_alt` body, and on hover
        // that body moved toward the text, as in the built-in palettes.
        let body = p.surface_alt;
        p.field_border = Color::from_rgba(body.r * 0.88, body.g * 0.88, body.b * 0.88, body.a);

        // The transport has no button slots, so the Primary fill starts as the
        // built-in default. When that lands on this theme's `surface_alt` (the
        // disabled fill), enabled and disabled buttons would look alike, so
        // derive a raised fill from the theme's brightest surface instead: a step
        // toward the text on a dark theme, white on a light one.
        let l1 = |a: Color, b: Color| (a.r - b.r).abs() + (a.g - b.g).abs() + (a.b - b.b).abs();
        if l1(p.button_primary_bg, p.surface_alt) < 24.0 / 255.0 {
            // Brightest of the surface tiers: iced's own ramp does not always
            // put `surface_elevated` on top.
            let lum = |c: Color| 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b;
            let top = [
                p.surface_elevated,
                p.surface,
                p.surface_alt,
                p.surface_hover,
            ]
            .into_iter()
            .fold(
                p.surface_elevated,
                |a, c| if lum(c) > lum(a) { c } else { a },
            );
            let toward = |c: Color, t: Color, k: f32| {
                Color::from_rgba(
                    c.r + (t.r - c.r) * k,
                    c.g + (t.g - c.g) * k,
                    c.b + (t.b - c.b) * k,
                    c.a,
                )
            };
            let raised = if ip.is_dark {
                toward(top, p.text, 0.12)
            } else {
                Color::WHITE
            };
            p.button_primary_bg = raised;
            p.button_primary_hover_bg = toward(raised, p.text, 0.06);
            p.button_primary_pressed_bg =
                Color::from_rgba(raised.r * 0.92, raised.g * 0.92, raised.b * 0.92, raised.a);
        }
        p.field_border_strong = Color::from_rgba(
            body.r + (p.text.r - body.r) * 0.25,
            body.g + (p.text.g - body.g) * 0.25,
            body.b + (p.text.b - body.b) * 0.25,
            body.a,
        );

        // Accent from the primary swatch: the inverse of the forward mapping
        // (base from accent, weak from accent_soft, strong from accent_pressed).
        p.accent = primary.base.color;
        p.accent_pressed = primary.strong.color;
        p.accent_soft = primary.weak.color;
        p.focus_ring = focus_ring_with_contrast(p.accent, p.background, p.surface);
        p.accent_hover = if ip.is_dark {
            p.accent.mix(bg.base.text, 0.15)
        } else {
            p.accent.mix(bg.base.text, 0.10)
        };

        p.success = success.base.color;
        p.success_soft = success.weak.color;
        p.warning = warning.base.color;
        p.warning_soft = warning.weak.color;
        p.danger = danger.base.color;
        p.danger_soft = danger.weak.color;
        p.destructive = danger.base.color;
        p.error = danger.base.color;

        p.tone_success_fg = crate::unferried(success.strong.text).unwrap_or(success.base.color);
        p.tone_warning_fg = crate::unferried(warning.strong.text).unwrap_or(warning.base.color);
        p.tone_error_fg = crate::unferried(danger.strong.text).unwrap_or(danger.base.color);

        p
    }
}

mod color_math;
mod raise;

pub use raise::{RaisedInk, perceptual_distance, raise_to_floor, raise_to_target};

/// `top` drawn over the opaque `under`.
fn over(top: Color, under: Color) -> Color {
    if top.a >= 0.999 {
        return Color { a: 1.0, ..top };
    }
    Color {
        a: 1.0,
        ..mix_srgb(under, Color { a: 1.0, ..top }, top.a)
    }
}

#[cfg(test)]
pub(super) use color_math::quantise_srgb;
pub(super) use color_math::{
    focus_ring_with_contrast, hex, legible_ink_on, lift_brand_to_text_contrast, mix_srgb,
    mix_to_contrast, srgb_distance, wcag_contrast,
};
#[cfg(test)]
pub(super) use raise::{oklab_to_srgb, srgb_to_oklab};

#[cfg(test)]
mod raise_to_floor_gates;
#[cfg(test)]
mod tests;

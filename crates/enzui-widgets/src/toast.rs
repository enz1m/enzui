//! Notification surfaces with live-region semantics, optional icons and actions.
//!
//! Defaults are Info tone, medium 40px height floor, a 400px width cap, no
//! leading icon and a dismiss button. Small and large height floors are 32px
//! and 52px; descriptions add 18px to the floor. Content can grow vertically.
//! Nonempty descriptions select a card layout: the icon, title and dismiss
//! on the first line, the description below with the actions at its end.
//! Description-free toasts reflow by measured control width; very narrow
//! layouts omit the status icon. Controls preserve 24px hit targets and may
//! extend beyond the available content width. Text breaks words or glyphs.
//!
//! Unpinned title/action ink and chrome follow the live palette; tone colours
//! ink the status icon. Description ink mutes title ink with contrast headroom.
//! Pinned tokens override these roles, but the enabled dismiss icon retains its
//! shared toolbar styling. Default title/description sizes are UI-scaled 13/11px
//! at composition. Glass wrapping is available through [`EnzToast::material`].
//!
//! Warning and Error announce as Alert; other tones as Status. Hosts publishing
//! one shared region should set nested surfaces to Silent. Message-derived
//! namespaces collide for simultaneous equal messages and change on edits; pin
//! an id when the host owns a stable notification key. [`crate::toast_host`]
//! owns expiry and stacking; this surface does not dismiss itself on a timer.

use crate::focus_button::{FocusStatus, focus_button_maybe};
use enzui_core::semantics::{Role, Semantics};
use enzui_theme::Palette;
use iced::widget::button::Style as ButtonStyle;
use iced::widget::{column, container, row, text};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding};

/// Tone variants — `EnzToast.tone`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToastTone {
    /// Informational status; this is the default.
    #[default]
    Info,
    /// Positive outcome.
    Success,
    /// Needs attention.
    Warning,
    /// Failure.
    Error,
    /// Advisory notification without severity.
    Neutral,
}

impl ToastTone {
    /// Whether a toast of this tone shows its status icon unless told
    /// otherwise: Warning and Error do, so severity never rests on wording
    /// alone; plain notices stay text-only.
    #[must_use]
    pub const fn shows_icon_by_default(self) -> bool {
        matches!(self, ToastTone::Warning | ToastTone::Error)
    }

    /// Semantic Lucide alias for the leading status glyph.
    pub fn default_icon(self) -> &'static str {
        match self {
            ToastTone::Info => "status.info",
            ToastTone::Success => "status.success",
            ToastTone::Warning => "status.warning",
            ToastTone::Error => "status.error",
            ToastTone::Neutral => "info",
        }
    }

    /// Return Alert for Warning and Error, otherwise Status.
    /// These are live-region terms, without modal dialog semantics.
    #[must_use]
    pub fn announcement(self) -> enzui_core::live_region::Announcement {
        use enzui_core::live_region::Announcement;
        match self {
            ToastTone::Warning | ToastTone::Error => Announcement::Alert,
            ToastTone::Info | ToastTone::Success | ToastTone::Neutral => Announcement::Status,
        }
    }
}

/// Size variants — `EnzToast.size`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToastSize {
    /// Small — 32px base height.
    Sm,
    /// Medium (the default) — 40px base height.
    #[default]
    Md,
    /// Large — 52px base height.
    Lg,
}

impl ToastSize {
    /// Base height in logical pixels.
    pub fn base_height(self) -> f32 {
        match self {
            ToastSize::Sm => 32.0,
            ToastSize::Md => 40.0,
            ToastSize::Lg => 52.0,
        }
    }
}

/// Colour + geometry tokens for a toast.
#[derive(Debug, Clone, Copy)]
pub struct ToastTokens {
    /// Title and action ink, shared by every tone.
    pub text: Color,
    /// Info tone: foreground/ink colour.
    pub info_fg: Color,
    /// Info tone: surface fill.
    pub info_bg: Color,
    /// Info tone: border colour.
    pub info_border: Color,
    /// Description ink; palette derivation mutes primary ink only with contrast headroom.
    pub info_secondary_fg: Color,
    /// Success tone: foreground/ink colour.
    pub success_fg: Color,
    /// Success tone: surface fill.
    pub success_bg: Color,
    /// Success tone: border colour.
    pub success_border: Color,
    /// Success description ink; token overrides are used without contrast adjustment.
    pub success_secondary_fg: Color,
    /// Warning tone: foreground/ink colour.
    pub warning_fg: Color,
    /// Warning tone: surface fill.
    pub warning_bg: Color,
    /// Warning tone: border colour.
    pub warning_border: Color,
    /// Warning description ink; token overrides are used without contrast adjustment.
    pub warning_secondary_fg: Color,
    /// Error tone: foreground/ink colour.
    pub error_fg: Color,
    /// Error tone: surface fill.
    pub error_bg: Color,
    /// Error tone: border colour.
    pub error_border: Color,
    /// Error description ink; token overrides are used without contrast adjustment.
    pub error_secondary_fg: Color,
    /// Neutral tone: foreground/ink colour.
    pub neutral_fg: Color,
    /// Neutral tone: surface fill.
    pub neutral_bg: Color,
    /// Neutral tone: border colour.
    pub neutral_border: Color,
    /// Neutral description ink; token overrides are used without contrast adjustment.
    pub neutral_secondary_fg: Color,

    /// Maximum toast width in px (default 400).
    pub max_width: f32,
    /// Corner radius in px (default 10).
    pub radius: f32,
    /// Horizontal padding in px (default 16).
    pub padding_x: f32,
    /// Vertical padding in px (default 8).
    pub padding_y: f32,
    /// Gap between icon, text and dismiss control in px (default 8).
    pub gap: f32,
    /// Border width in px (default 1).
    pub border_width: f32,

    /// Title size in logical pixels; defaults to UI-scaled 13px.
    pub title_size: f32,
    /// Description size in logical pixels; defaults to UI-scaled 11px.
    pub description_size: f32,
    /// Leading status-icon size in px (default 16).
    pub icon_size: f32,
    /// Dismiss (×) glyph size in px (default 16, the banner's X).
    pub dismiss_glyph_size: f32,
    /// Elevation level (0-5) of the drop shadow on the theme's elevation
    /// ladder (default 2: a toast floats above the content).
    pub elevation: u8,

    /// Corner radius of the text action button in px when the tokens are
    /// pinned (default 6); unpinned it follows the theme's `shape.button`.
    pub action_radius: f32,
    /// Width of the text action button's focus outline in px (default 1.5).
    /// Zero hides it. The dismiss (×) control is the shared toolbar icon and
    /// takes its own tokens.
    pub control_focus_width: f32,
    /// Alpha of the title ink washed behind a hovered or pressed text action
    /// button, 0 to 1 (default 0.10).
    pub control_hover_alpha: f32,
}

impl ToastTokens {
    /// Resolve neutral chrome with tone-specific status-icon ink and default geometry.
    /// All fills use surface_elevated, edges its shared shade and title/actions text.
    /// Description ink mutes text toward the opaque surface only while meeting the
    /// 4.6 contrast target; without headroom it keeps text unchanged. Tone ink uses
    /// hand-tuned defaults when each channel differs from the selected stock role by
    /// less than 1/255; distinct custom roles pass through. Custom contrast is not guaranteed.
    pub fn from_palette(p: &Palette) -> Self {
        let dark = p.is_dark();
        let base = if dark {
            Palette::dark()
        } else {
            Palette::light()
        };
        let info_fg = tone_pick(
            p.info,
            base.info,
            if dark {
                rgb(0x38, 0xBD, 0xF8)
            } else {
                rgb(0x01, 0x71, 0xAB)
            },
        );
        let success_fg = tone_pick(
            p.tone_success_fg,
            base.tone_success_fg,
            if dark {
                rgb(0x4A, 0xDE, 0x80)
            } else {
                rgb(0x11, 0x7F, 0x39)
            },
        );
        let warning_fg = tone_pick(
            p.tone_warning_fg,
            base.tone_warning_fg,
            if dark {
                rgb(0xFB, 0xBF, 0x24)
            } else {
                rgb(0x91, 0x63, 0x02)
            },
        );
        let error_fg = tone_pick(
            p.tone_error_fg,
            base.tone_error_fg,
            if dark {
                rgb(0xF8, 0x71, 0x71)
            } else {
                rgb(0xC5, 0x22, 0x22)
            },
        );
        let neutral_fg = tone_pick(
            p.text_muted,
            base.text_muted,
            if dark {
                rgb(0xD4, 0xD0, 0xD8)
            } else {
                rgb(0x4A, 0x46, 0x4D)
            },
        );
        let text = p.text;
        let surface = p.surface_elevated;
        let border =
            crate::surface_edge::edge_color(surface, crate::surface_edge::DEFAULT_EDGE_DEPTH);
        let secondary = secondary_fg(text, surface);
        let (info_bg, success_bg, warning_bg, error_bg, neutral_bg) =
            (surface, surface, surface, surface, surface);
        let (info_border, success_border, warning_border, error_border, neutral_border) =
            (border, border, border, border, border);
        let (
            info_secondary_fg,
            success_secondary_fg,
            warning_secondary_fg,
            error_secondary_fg,
            neutral_secondary_fg,
        ) = (secondary, secondary, secondary, secondary, secondary);
        Self {
            text,
            info_fg,
            info_bg,
            info_border,
            info_secondary_fg,
            success_fg,
            success_bg,
            success_border,
            success_secondary_fg,
            warning_fg,
            warning_bg,
            warning_border,
            warning_secondary_fg,
            error_fg,
            error_bg,
            error_border,
            error_secondary_fg,
            neutral_fg,
            neutral_bg,
            neutral_border,
            neutral_secondary_fg,

            max_width: 400.0,
            radius: 10.0,
            padding_x: 16.0,
            padding_y: 8.0,
            gap: 8.0,
            border_width: 1.0,
            title_size: enzui_theme::text_scale::ui_px(13.0),
            description_size: enzui_theme::text_scale::ui_px(11.0),
            icon_size: 16.0,
            dismiss_glyph_size: 16.0,
            elevation: 2,

            action_radius: 6.0,
            control_focus_width: 1.5,
            control_hover_alpha: 0.10,
        }
    }

    /// Tokens for the built-in dark palette.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }
    /// Tokens for the built-in light palette.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }

    /// Normalize token dimensions against dark-preset defaults.
    /// Font sizes must be finite and positive; other dimensions allow zero. Elevation
    /// caps at 5, hover alpha clamps to 0..=1, and non-finite alpha uses the default.
    /// Colours pass through; default title and description fallbacks are UI-scaled.
    #[must_use]
    pub fn sanitized(&self) -> Self {
        use enzui_core::metrics::{FiniteNonNeg, FinitePositive};
        let d = Self::dark();
        Self {
            max_width: FiniteNonNeg::new_or(self.max_width, d.max_width).get(),
            radius: FiniteNonNeg::new_or(self.radius, d.radius).get(),
            padding_x: FiniteNonNeg::new_or(self.padding_x, d.padding_x).get(),
            padding_y: FiniteNonNeg::new_or(self.padding_y, d.padding_y).get(),
            gap: FiniteNonNeg::new_or(self.gap, d.gap).get(),
            border_width: FiniteNonNeg::new_or(self.border_width, d.border_width).get(),
            title_size: FinitePositive::new_or(self.title_size, d.title_size).get(),
            description_size: FinitePositive::new_or(self.description_size, d.description_size)
                .get(),
            icon_size: FiniteNonNeg::new_or(self.icon_size, d.icon_size).get(),
            dismiss_glyph_size: FiniteNonNeg::new_or(self.dismiss_glyph_size, d.dismiss_glyph_size)
                .get(),
            elevation: self.elevation.min(5),
            action_radius: FiniteNonNeg::new_or(self.action_radius, d.action_radius).get(),
            control_focus_width: FiniteNonNeg::new_or(
                self.control_focus_width,
                d.control_focus_width,
            )
            .get(),
            control_hover_alpha: if self.control_hover_alpha.is_finite() {
                self.control_hover_alpha.clamp(0.0, 1.0)
            } else {
                d.control_hover_alpha
            },
            ..*self
        }
    }

    /// Return `(fg, bg, border)` for a tone.
    pub fn tone_colors(&self, tone: ToastTone) -> (Color, Color, Color) {
        match tone {
            ToastTone::Info => (self.info_fg, self.info_bg, self.info_border),
            ToastTone::Success => (self.success_fg, self.success_bg, self.success_border),
            ToastTone::Warning => (self.warning_fg, self.warning_bg, self.warning_border),
            ToastTone::Error => (self.error_fg, self.error_bg, self.error_border),
            ToastTone::Neutral => (self.neutral_fg, self.neutral_bg, self.neutral_border),
        }
    }

    /// Return stored description ink for the tone without contrast adjustment.
    pub fn secondary_fg(&self, tone: ToastTone) -> Color {
        match tone {
            ToastTone::Info => self.info_secondary_fg,
            ToastTone::Success => self.success_secondary_fg,
            ToastTone::Warning => self.warning_secondary_fg,
            ToastTone::Error => self.error_secondary_fg,
            ToastTone::Neutral => self.neutral_secondary_fg,
        }
    }
}

fn with_alpha(mut c: Color, alpha: f32) -> Color {
    c.a = alpha.clamp(0.0, 1.0);
    c
}

pub(crate) const DESCRIPTION_MIN_CONTRAST: f32 = 4.5;

pub(crate) const MIN_HIT_TARGET: f32 = 24.0;

const DESCRIPTION_TARGET_CONTRAST: f32 = DESCRIPTION_MIN_CONTRAST + 0.1;

pub(crate) fn secondary_fg(fg: Color, bg: Color) -> Color {
    use crate::accessibility_metrics::contrast_ratio;
    // Toast contrast assumes an opaque surface; preserve foreground alpha while muting.
    let bg = Color { a: 1.0, ..bg };
    // Prefer the most muted candidate with headroom; otherwise keep the supplied primary ink.
    let mut mix = 0.44_f32;
    while mix > 0.0 {
        let c = blend_to(fg, bg, mix);
        if contrast_ratio(c, bg) >= DESCRIPTION_TARGET_CONTRAST {
            return c;
        }
        mix -= 0.02;
    }
    fg
}
fn blend_to(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a,
    }
}
fn same_u8(a: Color, b: Color) -> bool {
    (a.r - b.r).abs() < 1.0 / 255.0
        && (a.g - b.g).abs() < 1.0 / 255.0
        && (a.b - b.b).abs() < 1.0 / 255.0
        && (a.a - b.a).abs() < 1.0 / 255.0
}
fn tone_pick(role: Color, base_role: Color, literal: Color) -> Color {
    // Keep hand-tuned preset ink while admitting distinct custom palette roles.
    if same_u8(role, base_role) {
        literal
    } else {
        role
    }
}
fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgb8(r, g, b)
}

/// Toast builder.
#[must_use]
pub struct EnzToast<'a, Message> {
    material: Option<enzui_theme::MaterialStyle>,
    text: String,
    description: String,
    tone: ToastTone,
    size: ToastSize,
    show_icon: Option<bool>,
    icon_override: Option<String>,
    dismissible: bool,
    on_dismiss: Option<Message>,
    action_label: String,
    on_action: Option<Message>,
    secondary: Option<(String, Message)>,
    tokens: Option<ToastTokens>,
    id_prefix: String,
    id_override: Option<String>,
    announcement: Option<enzui_core::live_region::Announcement>,
    _lifetime: std::marker::PhantomData<&'a ()>,
}

const TOAST_CLOSE_SIZE: f32 = MIN_HIT_TARGET;
pub(crate) fn enz_toast_undismissible<'a, Message: Clone + 'a>(
    text: impl Into<String>,
) -> EnzToast<'a, Message> {
    let mut toast = enz_toast_with(text, None);
    toast.dismissible = false;
    toast
}

/// Construct an enabled dismiss control with the supplied message.
/// Use dismissible(false) to omit it; lifecycle scheduling belongs to the host.
pub fn enz_toast<'a, Message: Clone + 'a>(
    text: impl Into<String>,
    on_dismiss: Message,
) -> EnzToast<'a, Message> {
    enz_toast_with(text, Some(on_dismiss))
}

fn enz_toast_with<'a, Message: Clone + 'a>(
    text: impl Into<String>,
    on_dismiss: Option<Message>,
) -> EnzToast<'a, Message> {
    EnzToast {
        material: None,
        text: text.into(),
        description: String::new(),
        tone: ToastTone::default(),
        size: ToastSize::default(),
        show_icon: None,
        icon_override: None,
        dismissible: true,
        on_dismiss,
        action_label: String::new(),
        on_action: None,
        secondary: None,
        tokens: None,
        id_prefix: "enz-toast".into(),
        id_override: None,
        announcement: None,
        _lifetime: std::marker::PhantomData,
    }
}

impl<'a, Message: Clone + 'a> EnzToast<'a, Message> {
    /// Set description text; a nonempty string selects card layout and raises the height floor.
    pub fn description(mut self, d: impl Into<String>) -> Self {
        self.description = d.into();
        self
    }

    /// Tone (default `Info`).
    pub fn tone(mut self, t: ToastTone) -> Self {
        self.tone = t;
        self
    }

    /// Size (default `Md`).
    pub fn size(mut self, s: ToastSize) -> Self {
        self.size = s;
        self
    }

    /// Show the leading status icon. By default only Warning and Error toasts
    /// show it, so severity never rests on wording alone; it may be omitted at
    /// narrow widths.
    pub fn show_icon(mut self, v: bool) -> Self {
        self.show_icon = Some(v);
        self
    }

    /// Show a catalog icon override; an empty name uses the tone default.
    pub fn icon(mut self, name: impl Into<String>) -> Self {
        let s = name.into();
        self.icon_override = if s.is_empty() { None } else { Some(s) };
        self.show_icon = Some(true);
        self
    }

    /// Enable / disable the trailing dismiss button (default `true`).
    pub fn dismissible(mut self, v: bool) -> Self {
        self.dismissible = v;
        self
    }

    /// Set a primary text action; empty text omits primary and secondary actions.
    pub fn action(mut self, label: impl Into<String>, on_action: Message) -> Self {
        self.action_label = label.into();
        self.on_action = Some(on_action);
        self
    }

    /// Add an action before the primary; it is shown only with a nonempty primary label.
    /// Its address derives from the toast namespace and the secondary key.
    pub fn secondary_action(mut self, label: impl Into<String>, message: Message) -> Self {
        self.secondary = Some((label.into(), message));
        self
    }

    /// Set the prefix for the default namespace: prefix-message, default enz-toast-message.
    /// The toast uses that namespace as its id; control ids derive from it and the
    /// action, secondary and dismiss keys. Equal messages in one namespace collide,
    /// and changing the message changes every derived address. An explicit id wins.
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }

    /// Pin the toast namespace independently of its message and prefix.
    /// The toast uses this exact id; controls use identity::derive_id with their
    /// action, secondary or dismiss key. Supply a unique stable notification key.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id_override = Some(id.into());
        self
    }

    /// Override the live-region term; otherwise it follows the tone.
    /// Silent publishes explicit Live::Off for nested surfaces whose host owns the
    /// shared announcement, keeping them navigable without inherited politeness.
    pub fn announcement(mut self, term: enzui_core::live_region::Announcement) -> Self {
        self.announcement = Some(term);
        self
    }

    /// Pin palette roles and geometry after sanitizing numeric fields.
    /// The enabled dismiss control keeps shared toolbar styling.
    pub fn tokens(mut self, t: ToastTokens) -> Self {
        self.tokens = Some(t.sanitized());
        self
    }

    /// Wrap the surface in the supplied glass style using its resolved token radius.
    pub fn material(mut self, style: enzui_theme::MaterialStyle) -> Self {
        self.material = Some(style);
        self
    }

    /// Convert into an iced [`Element`].
    pub fn into_element(self) -> Element<'a, Message> {
        let material = self.material;
        let radius = self
            .tokens
            .map_or_else(|| ToastTokens::dark().radius, |t| t.radius);
        crate::glass::wrap(self.build_element(), material, radius)
    }

    fn build_element(self) -> Element<'a, Message> {
        // Geometry resolves at composition; palette ink resolves during painting.
        let geom = self.tokens.unwrap_or_else(ToastTokens::dark);
        let pinned_geometry = self.tokens.is_some();
        let pinned = self.tokens;
        let tone = self.tone;
        let resolve_tokens = move |theme: &iced::Theme| -> ToastTokens {
            crate::theme_lookup::resolve_pinned(
                crate::theme_lookup::PIN_SITE_TOAST_TONE,
                pinned,
                theme,
                ToastTokens::from_palette,
            )
        };
        let resolve_tone = move |theme: &iced::Theme| -> (Color, Color, Color) {
            resolve_tokens(theme).tone_colors(tone)
        };
        let resolve_chrome = move |theme: &iced::Theme| -> (Color, Color, Color) {
            let tok = resolve_tokens(theme);
            let (_, bg, border) = tok.tone_colors(tone);
            (tok.text, bg, border)
        };
        let resolve_icon = move |theme: &iced::Theme| -> Color {
            let tone_c = resolve_tone(theme).0;
            let (_, bg, _) = resolve_chrome(theme);
            // Status icons use the shared legibility adjustment even with pinned ink.
            crate::accessibility_metrics::legible_tone(tone_c, bg, 3.0)
        };
        let resolve_secondary = move |theme: &iced::Theme| -> Color {
            let tok = crate::theme_lookup::resolve_pinned(
                crate::theme_lookup::PIN_SITE_TOAST_SECONDARY,
                pinned,
                theme,
                ToastTokens::from_palette,
            );
            tok.secondary_fg(tone)
        };
        let a11y_text = self.text.clone();
        let a11y_desc = self.description.clone();
        let namespace = self
            .id_override
            .clone()
            .unwrap_or_else(|| format!("{}-{}", self.id_prefix, self.text));
        let toast_id = iced::advanced::widget::Id::from(namespace.clone());
        let action_id = enzui_core::identity::derive_id(&namespace, "action");
        let secondary_id = enzui_core::identity::derive_id(&namespace, "secondary");
        let dismiss_id = enzui_core::identity::derive_id(&namespace, "dismiss");
        let base_h = self.size.base_height();
        let min_h = if self.description.is_empty() {
            base_h
        } else {
            base_h + 18.0
        };
        let title_str = self.text;
        let desc_str = self.description;
        let action_label = self.action_label;
        let on_action = self.on_action;
        let secondary = self.secondary;
        let on_dismiss = self.on_dismiss;
        let dismissible = self.dismissible;
        let show_icon = self
            .show_icon
            .unwrap_or_else(|| self.tone.shows_icon_by_default());
        let icon_name = if show_icon {
            Some(
                self.icon_override
                    .unwrap_or_else(|| self.tone.default_icon().to_owned()),
            )
        } else {
            None
        };
        let has_action = !action_label.is_empty();
        let has_trailing = has_action || dismissible;
        let gap = geom.gap;

        let card_title = title_str.clone();
        let build_text_column = {
            let desc_str = desc_str.clone();
            move || -> Element<'a, Message> {
                let mut text_children: Vec<Element<'a, Message>> = Vec::new();
                text_children.push(
                    text(title_str.clone())
                        .size(geom.title_size)
                        // Glyph breaks keep single oversized words within narrow text columns.
                        .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
                        .style(move |theme: &iced::Theme| iced::widget::text::Style {
                            color: Some(resolve_chrome(theme).0),
                        })
                        .into(),
                );
                if !desc_str.is_empty() {
                    text_children.push(
                        text(desc_str.clone())
                            .size(geom.description_size)
                            .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
                            .font(enzui_theme::text_defaults::font(
                                enzui_theme::FontRole::Ui,
                                enzui_theme::WeightRung::Medium,
                            ))
                            .style(move |theme: &iced::Theme| iced::widget::text::Style {
                                color: Some(resolve_secondary(theme)),
                            })
                            .into(),
                    );
                }
                column(text_children).spacing(2).width(Length::Fill).into()
            }
        };

        let build_icon = {
            let icon_name = icon_name.clone();
            move || -> Option<Element<'a, Message>> {
                icon_name.as_ref().map(|name| {
                    crate::icon::enz_icon(name)
                        .size(geom.icon_size)
                        .tint_resolved(move |theme| resolve_icon(theme))
                        .into()
                })
            }
        };
        let action_button = move |label: String,
                                  message: Option<Message>,
                                  id: iced::advanced::widget::Id|
              -> Element<'a, Message> {
            let action_name = label.clone();
            let action_enabled = message.is_some();
            let b = focus_button_maybe(
                text(label)
                    .size(geom.title_size)
                    .font(enzui_theme::text_defaults::font(
                        enzui_theme::FontRole::Ui,
                        enzui_theme::WeightRung::Semibold,
                    ))
                    .style(move |theme: &iced::Theme| iced::widget::text::Style {
                        color: Some(resolve_chrome(theme).0),
                    }),
                message,
            )
            .id(id)
            .padding(Padding::from([4.0, 10.0]))
            .semantics(
                Semantics::new(Role::Button)
                    .default_action()
                    .label(action_name)
                    .disabled(!action_enabled),
            )
            .style_focus(move |theme: &iced::Theme, status: FocusStatus| {
                let fg = resolve_chrome(theme).0;
                let hover_bg = with_alpha(fg, geom.control_hover_alpha);
                let this_bg = match status {
                    FocusStatus::Hovered | FocusStatus::Pressed => hover_bg,
                    _ => Color::TRANSPARENT,
                };
                let (border_c, border_w) = if matches!(status, FocusStatus::Focused) {
                    (fg, geom.control_focus_width)
                } else {
                    (Color::TRANSPARENT, 0.0)
                };
                ButtonStyle {
                    background: Some(Background::Color(this_bg)),
                    text_color: fg,
                    border: Border {
                        color: border_c,
                        width: border_w,
                        // A text button like every other: the theme's
                        // `shape.button` step unless the tokens pin it.
                        radius: if pinned_geometry {
                            geom.action_radius
                        } else {
                            crate::theme_lookup::shape_radius(theme, |shape| shape.button)
                        }
                        .into(),
                    },
                    ..ButtonStyle::default()
                }
            });
            crate::min_width::min_width(
                crate::min_height::min_height(b, MIN_HIT_TARGET),
                MIN_HIT_TARGET,
            )
            // Preserve control targets under narrow parents; whole controls reflow below.
            .overflowing(true)
            .into()
        };
        let build_action = {
            let on_action = on_action;
            let secondary = secondary;
            move || -> Option<Element<'a, Message>> {
                if action_label.is_empty() {
                    return None;
                }
                let primary =
                    action_button(action_label.clone(), on_action.clone(), action_id.clone());
                Some(match secondary.clone() {
                    Some((label, message)) => row![
                        action_button(label, Some(message), secondary_id.clone()),
                        primary
                    ]
                    .spacing(gap)
                    .align_y(Alignment::Center)
                    .into(),
                    None => primary,
                })
            }
        };
        let build_dismiss = {
            let on_dismiss = on_dismiss;
            move || -> Option<Element<'a, Message>> {
                if !dismissible {
                    return None;
                }
                if let Some(message) = on_dismiss.clone() {
                    let close: Element<'a, Message> =
                        crate::toolbar_icon::enz_toolbar_icon("action.close", message)
                            .size(TOAST_CLOSE_SIZE, geom.dismiss_glyph_size)
                            .aria_label(enzui_core::localization::text("enzui-toast-dismiss-toast"))
                            .id(dismiss_id.clone())
                            .into();
                    return Some(
                        // The dismiss target must survive the same compression as text actions.
                        crate::min_width::min_width(close, MIN_HIT_TARGET)
                            .overflowing(true)
                            .into(),
                    );
                }
                let close_icon = crate::icon::enz_icon("action.close")
                    .size(geom.dismiss_glyph_size)
                    .tint_resolved(move |theme| resolve_chrome(theme).0);
                let dismiss_msg = on_dismiss.clone();
                let dismiss_enabled = dismiss_msg.is_some();
                let b = focus_button_maybe(close_icon, dismiss_msg)
                    .id(dismiss_id.clone())
                    .padding(Padding::from([4.0, 4.0]))
                    .semantics(
                        Semantics::new(Role::Button)
                            .default_action()
                            .label(enzui_core::localization::text("enzui-toast-dismiss-toast"))
                            .disabled(!dismiss_enabled),
                    )
                    .style_focus(move |theme: &iced::Theme, status: FocusStatus| {
                        let fg = resolve_chrome(theme).0;
                        let hover_bg = with_alpha(fg, geom.control_hover_alpha);
                        let this_bg = match status {
                            FocusStatus::Hovered | FocusStatus::Pressed => hover_bg,
                            _ => Color::TRANSPARENT,
                        };
                        let (border_c, border_w) = if matches!(status, FocusStatus::Focused) {
                            (fg, geom.control_focus_width)
                        } else {
                            (Color::TRANSPARENT, 0.0)
                        };
                        ButtonStyle {
                            background: Some(Background::Color(this_bg)),
                            border: Border {
                                color: border_c,
                                width: border_w,
                                radius: (TOAST_CLOSE_SIZE / 2.0).into(),
                            },
                            ..ButtonStyle::default()
                        }
                    });
                Some(
                    crate::min_width::min_width(
                        crate::min_height::min_height(b, MIN_HIT_TARGET),
                        MIN_HIT_TARGET,
                    )
                    .overflowing(true)
                    .into(),
                )
            }
        };
        // Render and breakpoint probes share the same controls and localized labels.
        let build_trailing = {
            let build_action = build_action.clone();
            let build_dismiss = build_dismiss.clone();
            move || -> Element<'a, Message> {
                let mut controls: Vec<Element<'a, Message>> = Vec::new();
                if let Some(a) = build_action() {
                    controls.push(a);
                }
                if let Some(d) = build_dismiss() {
                    controls.push(d);
                }
                row(controls)
                    .spacing(gap)
                    .align_y(Alignment::Center)
                    .wrap()
                    .vertical_spacing(gap)
                    .into()
            }
        };
        let build_wide = {
            let build_icon = build_icon.clone();
            let build_text_column = build_text_column.clone();
            let build_action = build_action.clone();
            let build_dismiss = build_dismiss.clone();
            move || -> Element<'a, Message> {
                let mut row_children: Vec<Element<'a, Message>> = Vec::new();
                if let Some(i) = build_icon() {
                    row_children.push(i);
                }
                row_children.push(build_text_column());
                if let Some(a) = build_action() {
                    row_children.push(a);
                }
                if let Some(d) = build_dismiss() {
                    row_children.push(d);
                }
                row(row_children)
                    .spacing(gap)
                    .align_y(Alignment::Center)
                    .width(Length::Fill)
                    .into()
            }
        };
        let build_compact = {
            let build_icon = build_icon.clone();
            let build_text_column = build_text_column.clone();
            let build_trailing = build_trailing.clone();
            move |with_icon: bool| -> Element<'a, Message> {
                let mut lead_row: Vec<Element<'a, Message>> = Vec::new();
                if let Some(i) = build_icon().filter(|_| with_icon) {
                    lead_row.push(i);
                }
                lead_row.push(build_text_column());
                let top = row(lead_row)
                    .spacing(gap)
                    .align_y(Alignment::Center)
                    .width(Length::Fill);
                column![top, build_trailing()]
                    .spacing(gap)
                    .width(Length::Fill)
                    .into()
            }
        };
        const MIN_SINGLE_ROW_TEXT: f32 = 88.0;
        const MIN_COMPACT_TEXT: f32 = 64.0;
        let padding_x = geom.padding_x;
        let icon_slot = if icon_name.is_some() {
            geom.icon_size + gap
        } else {
            0.0
        };
        let breakpoint = move |trailing_w: f32| -> f32 {
            let mut w = padding_x * 2.0 + MIN_SINGLE_ROW_TEXT + icon_slot;
            if trailing_w > 0.0 {
                w += gap + trailing_w;
            }
            w
        };
        // With a description: the icon, title and dismiss share the first
        // line; the description runs below, aligned with the title, and the
        // actions sit under it at the trailing edge.
        let build_card = {
            let build_icon = build_icon.clone();
            let build_action = build_action.clone();
            let build_dismiss = build_dismiss.clone();
            let title_str = card_title;
            let desc_str = desc_str.clone();
            move || -> Element<'a, Message> {
                let mut top: Vec<Element<'a, Message>> = Vec::new();
                if let Some(icon) = build_icon() {
                    top.push(icon);
                }
                top.push(
                    text(title_str.clone())
                        .size(geom.title_size)
                        .font(enzui_theme::text_defaults::font(
                            enzui_theme::FontRole::Ui,
                            enzui_theme::WeightRung::Semibold,
                        ))
                        .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
                        .width(Length::Fill)
                        .style(move |theme: &iced::Theme| iced::widget::text::Style {
                            color: Some(resolve_chrome(theme).0),
                        })
                        .into(),
                );
                if let Some(dismiss) = build_dismiss() {
                    top.push(dismiss);
                }
                let top = row(top)
                    .spacing(gap)
                    .align_y(Alignment::Center)
                    .width(Length::Fill);
                let description = container(
                    text(desc_str.clone())
                        .size(geom.description_size)
                        .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
                        .font(enzui_theme::text_defaults::font(
                            enzui_theme::FontRole::Ui,
                            enzui_theme::WeightRung::Medium,
                        ))
                        .style(move |theme: &iced::Theme| iced::widget::text::Style {
                            color: Some(resolve_secondary(theme)),
                        }),
                )
                .padding(Padding::ZERO.left(icon_slot))
                .width(Length::Fill);
                // Actions take their own row under the description, at the
                // trailing edge: beside it, a long label ("Start fresh (your
                // file is kept)") squeezed the description into a narrow column.
                let second: Element<'a, Message> = match build_action() {
                    Some(actions) => column![
                        description,
                        container(actions).width(Length::Fill).align_x(
                            if enzui_core::placement::resolve_direction(None)
                                == enzui_core::placement::Direction::RightToLeft
                            {
                                iced::alignment::Horizontal::Left
                            } else {
                                iced::alignment::Horizontal::Right
                            },
                        ),
                    ]
                    .spacing(gap)
                    .width(Length::Fill)
                    .into(),
                    None => description.into(),
                };
                column![top, second].width(Length::Fill).into()
            }
        };

        let inner: Element<'a, Message> = if !desc_str.is_empty() {
            build_card()
        } else {
            crate::responsive_switch::ResponsiveSwitch::new(
                build_wide(),
                crate::responsive::enz_responsive(move |size: iced::Size| {
                    build_compact(size.width >= geom.icon_size + gap + MIN_COMPACT_TEXT)
                })
                .height(Length::Shrink)
                .into(),
                build_trailing(),
                Length::Fill,
                has_trailing,
                breakpoint,
            )
            .into()
        };

        let border_width = geom.border_width;
        let radius = geom.radius;
        let elevation = geom.elevation;
        // Small controls need reduced insets to fit the 32px height floor.
        let padding_y = match self.size {
            ToastSize::Sm => geom.padding_y.min(4.0),
            _ => geom.padding_y,
        };
        // Trailing controls sit as far in from the end as from the top and
        // bottom, so their hover washes line up with the toast's rounded
        // corner instead of floating a text inset away from it.
        let padding_right = if has_trailing {
            padding_y.max(4.0)
        } else {
            geom.padding_x
        };
        let toast = container(inner)
            .padding(
                Padding::new(padding_y)
                    .left(geom.padding_x)
                    .right(padding_right),
            )
            .align_y(iced::alignment::Vertical::Center)
            .width(Length::Fill)
            .height(Length::Shrink)
            .style(move |theme: &iced::Theme| {
                let (_ink, bg, border) = resolve_chrome(theme);
                container::Style {
                    background: Some(Background::Color(bg)),
                    border: Border {
                        color: border,
                        width: border_width,
                        radius: radius.into(),
                    },
                    // The glass layer owns elevation when this surface is on glass.
                    shadow: if crate::theme_lookup::palette(theme).is_on_glass() {
                        iced::Shadow::default()
                    } else {
                        crate::elevation::iced_shadow_for_theme(theme, elevation)
                    },
                    ..container::Style::default()
                }
            });
        let severity = match self.tone {
            ToastTone::Info => enzui_core::localization::text("enzui-toast-info"),
            ToastTone::Success => enzui_core::localization::text("enzui-toast-success"),
            ToastTone::Warning => enzui_core::localization::text("enzui-toast-warning"),
            ToastTone::Error => enzui_core::localization::text("enzui-toast-error"),
            ToastTone::Neutral => enzui_core::localization::text("enzui-toast-notification"),
        };
        let mut group_label = enzui_core::localized!("enzui-toast-description", "severity" => severity, "message" => a11y_text);
        if !a11y_desc.is_empty() {
            group_label.push_str(". ");
            group_label.push_str(&a11y_desc);
        }
        let floored = crate::min_height::min_height(toast, min_h);
        let capped = crate::max_width::max_width(floored, geom.max_width);
        // Open a semantic group so controls remain descendants of the live region.
        let semantics = self
            .announcement
            .unwrap_or_else(|| self.tone.announcement())
            .region(group_label);

        crate::semantics_container::SemanticsContainer::new(capped, semantics)
            .id(toast_id)
            .into()
    }
}

impl<'a, Message: Clone + 'a> From<EnzToast<'a, Message>> for Element<'a, Message> {
    fn from(t: EnzToast<'a, Message>) -> Self {
        t.into_element()
    }
}

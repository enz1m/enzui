//! [`enz_banner`]: inline status banners with a title, body, tone, optional
//! action and optional dismiss, shaped like a desktop infobar.
//!
//! ## Colour roles
//!
//! Every colour resolves from the runtime theme inside the style closures, so
//! a custom theme reaches the surface, text, border and status glyph alike.
//!
//! - Surface: the normal `palette.surface` with a 1 px `palette.border`
//!   border, for every tone.
//! - Glyph: a bare 20 px icon in the tone colour
//!   (`info`/`success`/`warning`/`destructive`, or
//!   [`tone_color`](EnzBanner::tone_color)), lifted only as far as 3:1 on the
//!   surface needs. [`BannerBadgeStyle::Chip`] opts back into a filled chip.
//! - Title: bold `palette.text`. Body: wrapped `Muted` text.
//! - Action: an [`EnzButton`](crate::button::EnzButton) (`Suggested` is the
//!   accent CTA [`Accent`](crate::button::ButtonKind::Accent), `Neutral` is
//!   `Secondary`).
//! - Dismiss: the round "Dismiss banner" X
//!   ([`enz_toolbar_icon`](crate::toolbar_icon::enz_toolbar_icon)) in the
//!   top-right corner.
//!
//! ## Accessibility
//!
//! A banner is a live region, because AT-SPI adapters announce an inserted
//! node only when its politeness is not `Off`: `Info` and `Success` publish
//! [`Status`](Role::Status) with [`Live::Polite`], `Warning` and `Danger`
//! [`Alert`](Role::Alert) with [`Live::Assertive`]. It is not a dialog role:
//! nothing modal opened.
//!
//! The node's label is the whole announcement, severity included
//! (`"Warning: Disk almost full. 12% left."`), since an insertion announces
//! only the name and colour and glyph alone do not convey severity. The
//! rendered title and body do not announce themselves again
//! ([`announce(false)`](crate::enz_text::EnzText::announce)), and no
//! description repeats the label. The action and dismiss buttons are the
//! banner's children in the accessibility tree.
//!
//! [`Live::Polite`]: enzui_core::semantics::Live::Polite
//! [`Live::Assertive`]: enzui_core::semantics::Live::Assertive
//!
//! ## Compact layout
//!
//! Below a content-aware width (icon, minimum text, trailing controls and
//! padding), the action and dismiss stack below the text column instead of
//! sitting beside it.

use crate::button::{ButtonSize, enz_button};
use crate::enz_text::{TextTone, enz_text};
use enzui_core::semantics::{Role, Semantics};
use enzui_theme::Palette;
use iced::advanced::layout::Measure as _;
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::widget::{Space, column, container, row};
use iced::{Border, Color, Element, Event, Length, Padding, Rectangle, Size, Vector, border};

/// Tone variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BannerTone {
    /// Informational (the default) — `info` / `info_soft`, the palette's own
    /// semantic informational pair. Not `accent`/`accent_soft`: see
    /// [`tone_colors`].
    #[default]
    Info,
    /// Positive outcome — `success` / `success_soft`.
    Success,
    /// Needs attention but not an error — `warning` / `warning_soft`.
    Warning,
    /// Error / failure — `danger`/`destructive` fill.
    Danger,
}

impl BannerTone {
    /// The severity word used in the region's accessible label.
    pub fn severity_word(self) -> String {
        match self {
            BannerTone::Info => enzui_core::localization::text("enzui-banner-information"),
            BannerTone::Success => enzui_core::localization::text("enzui-banner-success"),
            BannerTone::Warning => enzui_core::localization::text("enzui-banner-warning"),
            BannerTone::Danger => enzui_core::localization::text("enzui-banner-error"),
        }
    }

    /// The live-region role for this tone: advisory `Info` and `Success`
    /// announce politely as [`Status`](Role::Status); `Warning` and `Danger`
    /// interrupt as [`Alert`](Role::Alert). A banner is never an
    /// `AlertDialog`: it does not demand a response.
    #[must_use]
    pub fn live_region_role(self) -> Role {
        match self {
            BannerTone::Info | BannerTone::Success => Role::Status,
            BannerTone::Warning | BannerTone::Danger => Role::Alert,
        }
    }
}

/// Emphasis of the optional action button. The same
/// [`ActionEmphasis`](crate::action_emphasis::ActionEmphasis) vocabulary as
/// [`enz_status_page`](crate::status_page::enz_status_page), so callers write
/// the same word for both widgets. The `BannerActionEmphasis` alias is kept
/// so existing code compiles.
pub type BannerActionEmphasis = crate::action_emphasis::ActionEmphasis;

/// Presentation of the leading tone glyph.
///
/// [`Bare`](Self::Bare), the default, is the desktop shape: the
/// banner sits on the normal surface and only the glyph carries the tone.
/// [`Chip`](Self::Chip) keeps the older filled tone chip behind the glyph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BannerBadgeStyle {
    /// A tone-inked glyph on the banner surface, with no fill and no chip
    /// radius (AdwBanner / macOS inline alert). The default.
    #[default]
    Bare,
    /// A filled tone chip behind the glyph, resolved from [`tone_colors`], at
    /// the banner's own draw-time radius. Opt in.
    Chip,
}

// ---------------------------------------------------------------------------
// Tone colours — derived from the runtime palette
// ---------------------------------------------------------------------------

/// The `(surface fill, tone colour)` pair for a tone, from the palette: the
/// soft status fill for the surface, the full status colour for the badge.
/// `Info` uses the `info`/`info_soft` pair, not `accent`, so an accent action
/// button still stands out on the banner; toasts use the same pair.
pub fn tone_colors(tone: BannerTone, p: &Palette) -> (Color, Color) {
    match tone {
        BannerTone::Info => (p.info_soft, p.info),
        BannerTone::Success => (p.success_soft, p.success),
        BannerTone::Warning => (p.warning_soft, p.warning),
        BannerTone::Danger => (p.danger_soft, p.destructive),
    }
}

/// The tone badge's glyph ink, chosen for measured contrast against the
/// badge fill rather than assumed white (light amber, for example, needs a
/// dark glyph).
pub fn badge_ink(tone: BannerTone, p: &Palette) -> Color {
    let (_, fill) = tone_colors(tone, p);
    crate::tokens::ink_for(fill, p)
}

/// Geometry overrides for a banner: radius, padding, gap, type sizes and
/// badge size. Colours always resolve from the live theme (see
/// [`tone_colors`]), so a theme switch retints the banner.
#[derive(Debug, Clone, Copy)]
pub struct BannerTokens {
    /// Corner radius of the surface and the badge chip, in px (default 10).
    pub radius: f32,
    /// Horizontal inset from the surface edge to content, in px (default 12).
    pub padding_x: f32,
    /// Vertical inset from the surface edge to content, in px (default 12).
    pub padding_y: f32,
    /// Gap between the badge, text column and trailing controls, and between
    /// the stacked rows in compact layout, in px (default 12).
    pub gap: f32,
    /// Title type size in px (default 13, the Body/Headline role).
    pub title_size: f32,
    /// Body type size in px (default 12, the Footnote role — not 11, which is
    /// below this system's readable minimum for running prose).
    pub body_size: f32,
    /// Edge length of the square leading status badge, in px (default 28).
    pub icon_badge: f32,
    /// Elevation level (0-5) of the drop shadow, on the theme's elevation
    /// ladder (default 1: a banner sits in the page, just lifted off it).
    pub elevation: u8,
    /// How far the border darkens the banner's own surface, `0.0..=1.0`
    /// (default 0.12): a shade of the surface, not a separate colour.
    pub edge_depth: f32,
}

impl Default for BannerTokens {
    fn default() -> Self {
        Self {
            radius: 10.0,
            padding_x: 12.0,
            padding_y: 12.0,
            gap: 12.0,
            // Body/Headline role: 13 px.
            title_size: enzui_theme::text_scale::ui_px(13.0),
            // Footnote role: 12 px. Was 11 px (the Caption role), which is
            // below the smallest size this system considers readable running
            // prose.
            body_size: enzui_theme::text_scale::ui_px(12.0),
            icon_badge: 28.0,
            elevation: 1,
            edge_depth: crate::surface_edge::DEFAULT_EDGE_DEPTH,
        }
    }
}

impl BannerTokens {
    /// The default geometry. Kept as a named constructor for source-compat with
    /// callers written against the old per-theme token sets; this geometry is
    /// not theme-derived today (see the type docs), so it is exactly
    /// [`BannerTokens::default`].
    #[must_use]
    pub fn dark() -> Self {
        Self::default()
    }
    /// The default geometry — see [`BannerTokens::dark`]. Identical; geometry
    /// is theme-independent.
    #[must_use]
    pub fn light() -> Self {
        Self::default()
    }

    /// These tokens with non-finite or negative values replaced by the
    /// defaults. Zero is kept for every field (a borderless or gapless banner).
    /// [`tokens`](EnzBanner::tokens) applies this.
    #[must_use]
    pub fn sanitized(&self) -> Self {
        use enzui_core::metrics::FiniteNonNeg;
        let d = Self::default();
        Self {
            radius: FiniteNonNeg::new_or(self.radius, d.radius).get(),
            padding_x: FiniteNonNeg::new_or(self.padding_x, d.padding_x).get(),
            padding_y: FiniteNonNeg::new_or(self.padding_y, d.padding_y).get(),
            gap: FiniteNonNeg::new_or(self.gap, d.gap).get(),
            title_size: FiniteNonNeg::new_or(self.title_size, d.title_size).get(),
            body_size: FiniteNonNeg::new_or(self.body_size, d.body_size).get(),
            icon_badge: FiniteNonNeg::new_or(self.icon_badge, d.icon_badge).get(),
            elevation: self.elevation.min(5),
            edge_depth: if self.edge_depth.is_finite() {
                self.edge_depth.clamp(0.0, 1.0)
            } else {
                d.edge_depth
            },
        }
    }
}

/// Default glyph alias for a banner tone (semantic aliases resolved by
/// `enzui-icons`), `EnzBanner` `resolved-icon`.
pub fn default_icon_for_tone(tone: BannerTone) -> &'static str {
    match tone {
        BannerTone::Info => "status.info",
        BannerTone::Success => "status.success",
        BannerTone::Warning => "status.warning",
        BannerTone::Danger => "status.error",
    }
}

/// The id kind for banners. It is combined with a host key from
/// [`owner`](EnzBanner::owner); a banner without one publishes no ids.
pub const DEFAULT_ID_PREFIX: &str = "enz-banner";

/// Builder for an EnzBanner element.
#[must_use]
pub struct EnzBanner<'a, Message> {
    /// See [`EnzBanner::material`].
    material: Option<enzui_theme::MaterialStyle>,
    title: String,
    body: Option<String>,
    tone: BannerTone,
    action_text: Option<String>,
    action_emphasis: BannerActionEmphasis,
    on_action: Option<Message>,
    dismissible: bool,
    on_dismiss: Option<Message>,
    banner_visible: bool,
    tokens: BannerTokens,
    /// Whether [`EnzBanner::tokens`] pinned the geometry explicitly. When
    /// `false`, the draw-time `radius` follows the runtime theme's
    /// [`RadiusScale`](enzui_theme::RadiusScale) instead of the
    /// compile-time default. An explicit pin always wins.
    tokens_pinned: bool,
    width: Length,
    /// Whether the neutral outer hairline is visible. Its one-pixel geometry
    /// is retained when transparent so toggling it never reflows the banner.
    bordered: bool,
    show_icon: bool,
    icon_override: Option<String>,
    /// Caller-supplied namespace prefix, or `None`. Not defaulted to
    /// a per-kind constant: a constant distinguishes no instance, and treating
    /// it as identity is what made the contract's equal-title banners collide. See
    /// [`EnzBanner::id_prefix`].
    id_prefix: Option<String>,
    /// Caller-supplied stable instance key. See [`EnzBanner::id`].
    id_override: Option<String>,
    /// Presentation of the leading tone glyph; default [`BannerBadgeStyle::Bare`].
    badge_style: BannerBadgeStyle,
    /// A caller colour for the tone glyph (and chip), replacing the tone's
    /// palette colour; the tone still decides the live-region role.
    tone_color: Option<Color>,
    /// A second, quieter action placed before the primary one.
    secondary_action: Option<(String, Message)>,
    _lifetime: std::marker::PhantomData<&'a ()>,
}

/// Create an EnzBanner with a title.
pub fn enz_banner<'a, Message: 'a>(title: impl Into<String>) -> EnzBanner<'a, Message> {
    EnzBanner {
        material: None,
        title: title.into(),
        body: None,
        tone: BannerTone::default(),
        action_text: None,
        action_emphasis: BannerActionEmphasis::default(),
        on_action: None,
        dismissible: true,
        on_dismiss: None,
        banner_visible: true,
        tokens: BannerTokens::default(),
        tokens_pinned: false,
        width: Length::Fill,
        bordered: true,
        show_icon: true,
        icon_override: None,
        id_prefix: None,
        id_override: None,
        badge_style: BannerBadgeStyle::default(),
        tone_color: None,
        secondary_action: None,
        _lifetime: std::marker::PhantomData,
    }
}

impl<'a, Message: 'a + Clone> EnzBanner<'a, Message> {
    /// Set the secondary body copy below the title. An empty string clears it
    /// (the body is `None` by default); body text wraps rather than eliding.
    pub fn body(mut self, b: impl Into<String>) -> Self {
        let s = b.into();
        self.body = if s.is_empty() { None } else { Some(s) };
        self
    }

    /// Set the severity tone. Defaults to [`BannerTone::Info`].
    pub fn tone(mut self, tone: BannerTone) -> Self {
        self.tone = tone;
        self
    }

    /// Set the action button label. The action control is only rendered
    /// when both a label and [`Self::on_action`] are present, so a labelled
    /// but message-less action can never appear active-but-inert.
    pub fn action_text(mut self, t: impl Into<String>) -> Self {
        let s = t.into();
        self.action_text = if s.is_empty() { None } else { Some(s) };
        self
    }

    /// Set the action button's emphasis. Defaults to
    /// [`BannerActionEmphasis::Neutral`] (a ghost button).
    pub fn action_emphasis(mut self, e: BannerActionEmphasis) -> Self {
        self.action_emphasis = e;
        self
    }

    /// Message emitted when the action button is pressed. Required for the
    /// action to render at all — a label without a message stays hidden.
    pub fn on_action(mut self, msg: Message) -> Self {
        self.on_action = Some(msg);
        self
    }

    /// A second action, such as "Open location" beside "Retry", drawn as a
    /// neutral button before the primary [`Self::action_text`] action, in the
    /// banner's own row. An empty label draws nothing.
    pub fn secondary_action(mut self, label: impl Into<String>, msg: Message) -> Self {
        let label = label.into();
        self.secondary_action = (!label.is_empty()).then_some((label, msg));
        self
    }

    /// Whether the banner offers a dismiss (×) control. Defaults to `true`, but
    /// the control only renders once [`Self::on_dismiss`] is also set.
    pub fn dismissible(mut self, d: bool) -> Self {
        self.dismissible = d;
        self
    }

    /// Message emitted when the dismiss control is pressed. Required for the
    /// dismiss control to render.
    pub fn on_dismiss(mut self, msg: Message) -> Self {
        self.on_dismiss = Some(msg);
        self
    }

    /// Whether the banner is shown. When `false`, [`Self::into_element`]
    /// collapses to a zero-size `Space`. Defaults to `true`.
    pub fn banner_visible(mut self, v: bool) -> Self {
        self.banner_visible = v;
        self
    }

    /// Override the geometry tokens (radius, padding, sizes). Replaces the
    /// whole set; defaults to [`BannerTokens::default`].
    ///
    /// Pinning also opts the banner out of theme-driven radius: an explicit
    /// px value is a stronger statement of intent than a scale, so a pinned
    /// `radius` is honoured verbatim.
    pub fn tokens(mut self, t: BannerTokens) -> Self {
        // Normalize at the public boundary so no non-finite geometry token can
        // reach renderer coordinates.
        self.tokens = t.sanitized();
        self.tokens_pinned = true;
        self
    }

    /// Show the neutral one-pixel outer border. On by default: the banner sits
    /// on the normal surface colour, and the hairline is what separates it.
    pub fn bordered(mut self, bordered: bool) -> Self {
        self.bordered = bordered;
        self
    }

    /// Set the banner's width. Defaults to [`Length::Fill`].
    pub fn width(mut self, w: Length) -> Self {
        self.width = w;
        self
    }

    /// Show or hide the leading status badge. Defaults to `true`.
    pub fn show_icon(mut self, v: bool) -> Self {
        self.show_icon = v;
        self
    }

    /// Override the leading glyph with a specific Lucide/semantic name.
    /// Empty string reverts to the tone default.
    pub fn icon(mut self, name: impl Into<String>) -> Self {
        let s = name.into();
        self.icon_override = if s.is_empty() { None } else { Some(s) };
        self
    }

    /// Presentation of the leading tone glyph.
    ///
    /// Defaults to [`Bare`](BannerBadgeStyle::Bare): the glyph in its tone
    /// colour on the banner's surface. [`Chip`](BannerBadgeStyle::Chip) adds a
    /// filled tone chip behind it.
    pub fn badge_style(mut self, style: BannerBadgeStyle) -> Self {
        self.badge_style = style;
        self
    }

    /// Draw the tone glyph (and chip) in `color` instead of the tone's palette
    /// colour, for a status the built-in tones do not name. The tone still
    /// decides how the banner is announced.
    pub fn tone_color(mut self, color: Color) -> Self {
        self.tone_color = Some(color);
        self
    }

    /// A host-chosen id prefix, so the banner's action and dismiss controls
    /// are routable. Prefer [`id`](Self::id) when the host has a model key.
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = Some(prefix.into());
        self
    }

    /// The banner's stable instance key from the host's model (a queue entry
    /// or record key): unique among mounted banners and stable across
    /// rebuilds, so its controls are routable. Titles are not used: two
    /// banners can share one, and a banner can be retitled.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id_override = Some(id.into());
        self
    }

    /// The id owner for this banner: `.id("queue-7")` gives
    /// `enz-banner#queue-7/region`, `…/action` and `…/dismiss`;
    /// `.id_prefix("p")` gives `p#/region` and so on. With neither, the banner
    /// is anonymous and its controls expose no actions, rather than sharing an
    /// id with another banner.
    pub fn owner(&self) -> Option<enzui_core::identity::OwnerId> {
        enzui_core::identity::OwnerId::resolve(
            DEFAULT_ID_PREFIX,
            self.id_prefix.as_deref(),
            self.id_override.as_deref(),
        )
    }

    /// Put the banner on liquid glass of `style` (see [`crate::glass`]).
    /// Controls inside default to glass.
    pub fn material(mut self, style: enzui_theme::MaterialStyle) -> Self {
        self.material = Some(style);
        self
    }

    /// Consume the builder and produce the widget tree. When
    /// [`Self::banner_visible`] is `false` this is a zero-size `Space`.
    pub fn into_element(self) -> Element<'a, Message> {
        let material = self.material;
        let (tokens, pinned) = (self.tokens, self.tokens_pinned);
        crate::glass::wrap_resolved(self.build_element(), material, move |theme| {
            banner_radius(theme, tokens, pinned)
        })
    }

    fn build_element(self) -> Element<'a, Message> {
        if !self.banner_visible {
            return Space::new()
                .width(Length::Shrink)
                .height(Length::Shrink)
                .into();
        }

        let tokens = self.tokens;
        let tokens_pinned = self.tokens_pinned;
        let tone = self.tone;
        let width = self.width;

        // Titles are mutable and non-unique; only a host-owned key defines identity.
        let owner = self.owner();
        let region_id = owner.as_ref().map(|o| o.id("region"));
        let action_id = owner.as_ref().map(|o| o.id("action"));
        let dismiss_id = owner.as_ref().map(|o| o.id("dismiss"));

        // Resolve everything that a compact/wide layout needs once, up-front,
        // then let `responsive` pick the arrangement. The pieces are rebuilt
        // inside the closure because Element isn't Clone; we capture plain data.
        let title = self.title;
        let body = self.body;
        let icon_name = if self.show_icon {
            Some(
                self.icon_override
                    .unwrap_or_else(|| default_icon_for_tone(tone).to_owned()),
            )
        } else {
            None
        };
        let mut actions: Actions<Message> = Vec::new();
        if let Some((label, message)) = self.secondary_action {
            actions.push((
                label,
                message,
                BannerActionEmphasis::Neutral,
                owner.as_ref().map(|o| o.id("secondary-action")),
            ));
        }
        // An action renders only with both its label and its message.
        if let (Some(label), Some(message)) = (self.action_text, self.on_action) {
            actions.push((label, message, self.action_emphasis, action_id));
        }
        let dismiss = self.on_dismiss.filter(|_| self.dismissible);

        let has_trailing = !actions.is_empty() || dismiss.is_some();

        // Build both arrangements once. BannerResponsive measures the real
        // trailing controls during layout and atomically selects one complete
        // arrangement; it never commits a wide row whose action is only
        // partially visible. Inactive trees are neither drawn nor
        // traversed.
        let badge_style = self.badge_style;
        let wide = build_banner(
            &title,
            body.as_deref(),
            tone,
            icon_name.as_deref(),
            actions.clone(),
            dismiss.clone(),
            region_id.clone(),
            dismiss_id.clone(),
            tokens,
            Arrangement::Wide,
            has_trailing,
            tokens_pinned,
            self.bordered,
            badge_style,
            self.tone_color,
        );
        let compact = build_banner(
            &title,
            body.as_deref(),
            tone,
            icon_name.as_deref(),
            actions.clone(),
            dismiss.clone(),
            region_id,
            dismiss_id.clone(),
            tokens,
            Arrangement::Compact,
            has_trailing,
            tokens_pinned,
            self.bordered,
            badge_style,
            self.tone_color,
        );
        let actions_for_narrow = actions.clone();
        let dismiss_for_narrow = dismiss.clone();
        let trailing_probe = build_trailing(actions, dismiss, dismiss_id.clone(), tokens);

        // The narrow arrangement: no leading glyph, and the dismiss X joins
        // the action row under the text so the text keeps the full width.
        let narrow = build_banner(
            &title,
            body.as_deref(),
            tone,
            None,
            actions_for_narrow,
            dismiss_for_narrow,
            None,
            dismiss_id,
            tokens,
            Arrangement::Narrow,
            has_trailing,
            tokens_pinned,
            self.bordered,
            badge_style,
            self.tone_color,
        );
        BannerResponsive {
            children: vec![wide, compact, trailing_probe, narrow],
            width,
            tokens,
            has_icon: icon_name.is_some(),
            has_trailing,
        }
        .into()
    }
}

/// Content-aware compact breakpoint. `trailing_width` is measured from the
/// actual localized controls, including the gap between action and dismiss.
fn compact_breakpoint(tokens: BannerTokens, has_icon: bool, trailing_width: f32) -> f32 {
    const MIN_SINGLE_ROW_TEXT: f32 = 132.0;
    let mut width = tokens.padding_x * 2.0 + MIN_SINGLE_ROW_TEXT;
    if has_icon {
        width += tokens.icon_badge + tokens.gap;
    }
    if trailing_width > 0.0 {
        width += tokens.gap + trailing_width;
    }
    width
}

/// Width below which a banner takes its narrow arrangement: the insets, the
/// glyph (when there is one), the dismiss X and a 48 px text column.
fn narrow_breakpoint(tokens: BannerTokens, has_icon: bool) -> f32 {
    const MIN_TEXT: f32 = 48.0;
    let glyph = if has_icon {
        tokens.icon_badge + tokens.gap
    } else {
        0.0
    };
    tokens.padding_x * 2.0 + glyph + MIN_TEXT + DISMISS_GAP + DISMISS_SIZE
}

/// Build the exact trailing control row used both for rendering and intrinsic
/// breakpoint measurement.
fn build_trailing<'a, Message: 'a + Clone>(
    actions: Actions<Message>,
    dismiss: Option<Message>,
    // `None` Means the caller supplied no host-owned identity, so the
    // control is published ANONYMOUS rather than under an aliasing default.
    dismiss_id: Option<iced::advanced::widget::Id>,
    tokens: BannerTokens,
) -> Element<'a, Message> {
    let mut controls: Vec<Element<'a, Message>> = Vec::new();
    for (label, message, emphasis, id) in actions {
        let mut b = enz_button(label, message)
            .kind(emphasis.button_kind())
            .size(ButtonSize::Sm);
        if let Some(id) = id {
            b = b.id(id);
        }
        controls.push(b.into());
    }
    if let Some(message) = dismiss {
        controls.push(dismiss_button(message, dismiss_id));
    }
    row(controls)
        .spacing(tokens.gap)
        .align_y(iced::Alignment::Center)
        // Compact banners may be narrower than action + dismiss together.
        // Wrap only between whole controls; each button keeps its intrinsic
        // width, so the fallback is another row rather than a half button.
        .wrap()
        .vertical_spacing(tokens.gap)
        .into()
}

/// The banner's actions in trailing order, secondary first and primary last,
/// each with its label, message, emphasis and published identity.
type Actions<Message> = Vec<(
    String,
    Message,
    BannerActionEmphasis,
    Option<iced::advanced::widget::Id>,
)>;

/// Size of the tone glyph drawn without a chip.
const BARE_GLYPH: f32 = 20.0;

/// The dismiss X's control and glyph size: the toolbar icon's own 28 / 16.
const DISMISS_SIZE: f32 = 28.0;
const DISMISS_GLYPH: f32 = 16.0;

/// Gap between the banner's content and its top-right dismiss X.
const DISMISS_GAP: f32 = 4.0;

/// The dismiss X: the shared round icon action (`toolbar_icon`), a neutral
/// glyph with a circular hover wash, the same control a toast closes with.
fn dismiss_button<'a, Message: 'a + Clone>(
    message: Message,
    id: Option<iced::advanced::widget::Id>,
) -> Element<'a, Message> {
    // A tight 28 px box: the toolbar default adds a 32 px row slot that
    // would drop the X 2 px below the banner's corner padding.
    let mut b = crate::toolbar_icon::enz_toolbar_icon("action.close", message)
        .size(DISMISS_SIZE, DISMISS_GLYPH)
        .aria_label(enzui_core::localization::text(
            "enzui-banner-dismiss-banner",
        ));
    if let Some(id) = id {
        b = b.id(id);
    }
    b.into()
}

/// How [`build_banner`] lays out its parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arrangement {
    /// Glyph, text and action on one row; the X at the top-right.
    Wide,
    /// The action stacked under the text; the X at the top-right.
    Compact,
    /// No glyph; the action and the X share a row under the text.
    Narrow,
}

#[derive(Debug, Default)]
struct BannerResponsiveState {
    /// Index of the arrangement in force: 0 wide, 1 compact, 3 narrow.
    active: usize,
}

/// Selects a complete wide or compact banner after measuring the real trailing
/// controls. The third child is a layout-only intrinsic measurement probe.
struct BannerResponsive<'a, Message> {
    /// Wide, compact, the trailing-controls probe, then narrow.
    children: Vec<Element<'a, Message>>,
    width: Length,
    tokens: BannerTokens,
    has_icon: bool,
    has_trailing: bool,
}

impl<'a, Message: 'a> BannerResponsive<'a, Message> {
    fn active(tree: &Tree) -> usize {
        tree.state.downcast_ref::<BannerResponsiveState>().active
    }
}

impl<'a, Message: 'a> Widget<Message, iced::Theme, iced::Renderer>
    for BannerResponsive<'a, Message>
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        iced::advanced::widget::tree::Tag::of::<BannerResponsiveState>()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        iced::advanced::widget::tree::State::new(BannerResponsiveState::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(&mut self.children);
    }

    fn size(&self) -> Size<Length> {
        Size::new(self.width, Length::Shrink)
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let probe_limits = layout::Limits::new(Size::ZERO, Size::new(f32::INFINITY, f32::INFINITY));
        let probe = self.children[2].as_widget_mut().measure(
            &mut tree.children[2],
            renderer,
            &probe_limits,
        );
        let breakpoint = compact_breakpoint(self.tokens, self.has_icon, probe.size().width);
        let limits = limits.width(self.width).height(Length::Shrink);
        let available = limits.max().width;
        let compact = self.has_trailing
            && available.is_finite()
            && available > 0.0
            && available + 0.01 < breakpoint;
        // Below the narrow breakpoint the glyph leaves the row and the X moves
        // under the text, so the text keeps a readable column.
        let narrow = (self.has_icon || self.has_trailing)
            && available.is_finite()
            && available > 0.0
            && available + 0.01 < narrow_breakpoint(self.tokens, self.has_icon);
        let active = if narrow { 3 } else { usize::from(compact) };
        tree.state.downcast_mut::<BannerResponsiveState>().active = active;
        let node = self.children[active].as_widget_mut().measure(
            &mut tree.children[active],
            renderer,
            &limits,
        );
        // Only the active branch is placed; the others stay laid out.
        layout::Node::with_children(node.size(), vec![node])
            .with_placed([active])
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
        let active = Self::active(tree);
        let child_layout = layout.child(&tree.children, Self::active(tree));
        self.children[active].as_widget_mut().operate(
            &mut tree.children[active],
            child_layout,
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
        let active = Self::active(tree);
        let child_layout = layout.child(&tree.children, Self::active(tree));
        self.children[active].as_widget_mut().update(
            &mut tree.children[active],
            event,
            child_layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        crate::frame_routing::refresh_opaque_child(&self.children[active], tree, active);
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        let active = Self::active(tree);
        tree.children
            .get(active)
            .is_some_and(Tree::has_frame_interest)
    }

    fn update_frame(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let active = Self::active(tree);
        let child_layout = layout.child(&tree.children, Self::active(tree));
        crate::frame_routing::forward_frame_opaque(
            &mut self.children[active],
            tree,
            active,
            event,
            child_layout,
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
        let active = Self::active(tree);
        let child_layout = layout.child(&tree.children, Self::active(tree));
        self.children[active].as_widget().draw(
            &tree.children[active],
            renderer,
            theme,
            style,
            child_layout,
            cursor,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let active = Self::active(tree);
        let child_layout = layout.child(&tree.children, Self::active(tree));
        self.children[active].as_widget().mouse_interaction(
            &tree.children[active],
            child_layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        let active = Self::active(tree);
        let child_layout = layout.child(&tree.children, Self::active(tree));
        self.children[active].as_widget_mut().overlay(
            &mut tree.children[active],
            child_layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message: 'a> From<BannerResponsive<'a, Message>> for Element<'a, Message> {
    fn from(value: BannerResponsive<'a, Message>) -> Self {
        Element::new(value)
    }
}

/// Build the private responsive wrapper directly for frame-routing contract
/// tests. This does not expose its layout policy as supported API.
#[doc(hidden)]
pub fn __frame_routing_fixture<'a, Message: 'a>(
    child: Element<'a, Message>,
) -> Element<'a, Message> {
    BannerResponsive {
        children: vec![
            child,
            iced::widget::Space::new().into(),
            iced::widget::Space::new().into(),
            iced::widget::Space::new().into(),
        ],
        width: Length::Fill,
        tokens: BannerTokens::default(),
        has_icon: false,
        has_trailing: false,
    }
    .into()
}

/// The banner's corner radius: the caller's tokens when they pinned the
/// geometry, otherwise the runtime theme's `lg` rung. Reading the theme at
/// draw time is fine here because the radius only feeds
/// `container::Style`; the padding and gap fields feed layout, which cannot
/// see the theme. The default is the `lg` rung (10 px), so no special case is
/// needed.
fn banner_radius(theme: &iced::Theme, tokens: BannerTokens, pinned: bool) -> f32 {
    if pinned {
        tokens.radius
    } else {
        crate::theme_lookup::radius(theme).lg
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "every banner part, resolved by the builder"
)]
fn build_banner<'a, Message: 'a + Clone>(
    title: &str,
    body: Option<&str>,
    tone: BannerTone,
    icon_name: Option<&str>,
    actions: Actions<Message>,
    dismiss: Option<Message>,
    region_id: Option<iced::advanced::widget::Id>,
    dismiss_id: Option<iced::advanced::widget::Id>,
    tokens: BannerTokens,
    arrangement: Arrangement,
    has_trailing: bool,
    // When false, the surface/badge corner radius resolves from the runtime
    // theme's RadiusScale at draw time instead of `tokens.radius`.
    radius_pinned: bool,
    bordered: bool,
    badge_style: BannerBadgeStyle,
    tone_color: Option<Color>,
) -> Element<'a, Message> {
    // Title and body render with `announce(false)`: the region node already
    // speaks them in one aggregate string, and they would otherwise be read
    // twice.
    let mut text_children: Vec<Element<'a, Message>> = Vec::new();
    // An empty title takes no line: a body-only banner centres its body on
    // the glyph instead of leaving a blank row above it.
    if !title.trim().is_empty() {
        text_children.push(
            enz_text(title.to_owned())
                .size_px(tokens.title_size)
                .weight_rung(enzui_theme::WeightRung::Bold)
                // A word wider than a narrow column breaks rather than
                // painting past the banner's frame.
                .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
                .announce(false)
                .into(),
        );
    }
    if let Some(b) = body {
        text_children.push(
            enz_text(b.to_owned())
                .size_px(tokens.body_size)
                .tone(TextTone::Muted)
                // Body copy reflows instead of truncating to its first
                // line. This is `EnzText`'s default too, but it is set
                // explicitly so the banner does not depend on the default
                // staying put.
                .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
                .width(Length::Fill)
                .announce(false)
                .into(),
        );
    }
    let text_column: Element<'a, Message> =
        column(text_children).spacing(4).width(Length::Fill).into();

    // --- Leading tone glyph ---
    //
    // The default [`BannerBadgeStyle::Bare`] inks the glyph on the surface;
    // [`BannerBadgeStyle::Chip`] paints a filled tone chip behind it.
    let leading: Option<Element<'a, Message>> = icon_name.map(|name| {
        let badge = tokens.icon_badge;
        match badge_style {
            BannerBadgeStyle::Bare => {
                // Tone-inked glyph, measured against the banner surface. Same
                // box (icon_badge) as the Chip variant so switching the setter
                // never re-flows the banner.
                let glyph = crate::icon::enz_icon(name).size(BARE_GLYPH).tint_resolved(
                    move |theme: &iced::Theme| {
                        let palette = crate::theme_lookup::palette(theme);
                        let surface = palette.surface;
                        let tone_c = tone_color.unwrap_or_else(|| tone_colors(tone, &palette).1);
                        // The tone colour itself, lightened or darkened only
                        // as far as the 3:1 non-text contrast floor needs, so
                        // a pale custom colour or a low-contrast palette keeps
                        // its hue rather than turning neutral.
                        crate::accessibility_metrics::legible_tone(tone_c, surface, 3.0)
                    },
                );
                container(
                    container(glyph)
                        .center_x(Length::Fill)
                        .center_y(Length::Fill),
                )
                .width(Length::Fixed(badge))
                .height(Length::Fixed(badge))
                .into()
            }
            BannerBadgeStyle::Chip => {
                let glyph = crate::icon::enz_icon(name)
                    .size(enzui_theme::text_scale::ui_px(16.0))
                    .tint_resolved(move |theme: &iced::Theme| {
                        badge_ink(tone, &crate::theme_lookup::palette(theme))
                    });
                container(
                    container(glyph)
                        .center_x(Length::Fill)
                        .center_y(Length::Fill),
                )
                .width(Length::Fixed(badge))
                .height(Length::Fixed(badge))
                .style(move |theme: &iced::Theme| {
                    let tone_c = tone_color.unwrap_or_else(|| {
                        tone_colors(tone, &crate::theme_lookup::palette(theme)).1
                    });
                    container::Style {
                        background: Some(iced::Background::Color(tone_c)),
                        border: Border {
                            color: Color::TRANSPARENT,
                            width: 0.0,
                            radius: border::Radius::new(banner_radius(
                                theme,
                                tokens,
                                radius_pinned,
                            )),
                        },
                        ..container::Style::default()
                    }
                })
                .into()
            }
        }
    });

    // The same helper constructs the rendered row and the intrinsic probe, so
    // breakpoint measurement cannot drift from localized button geometry.
    let compact = arrangement != Arrangement::Wide;
    let (action_row, close) = if arrangement == Arrangement::Narrow {
        let row = (!actions.is_empty() || dismiss.is_some())
            .then(|| build_trailing(actions, dismiss, dismiss_id, tokens));
        (row, None)
    } else {
        (
            (!actions.is_empty()).then(|| build_trailing(actions, None, None, tokens)),
            dismiss.map(|message| dismiss_button(message, dismiss_id)),
        )
    };
    let _ = has_trailing;

    // --- Arrange ---
    //
    // The glyph, text and action are centred on each other; the dismiss X is
    // pinned to the top-right corner, outside that row. Compact
    // banners stack the action under both.
    let mut lead_row: Vec<Element<'a, Message>> = Vec::new();
    if let Some(l) = leading {
        lead_row.push(l);
    }
    lead_row.push(text_column);
    let (inline_action, stacked_action) = if compact {
        (None, action_row)
    } else {
        (action_row, None)
    };
    if let Some(action) = inline_action {
        lead_row.push(action);
    }
    let main: Element<'a, Message> = row(lead_row)
        .spacing(tokens.gap)
        .align_y(iced::Alignment::Center)
        .width(Length::Fill)
        .into();
    let header: Element<'a, Message> = match close {
        // Centred on the row like the glyph and the action beside it; a
        // top-aligned X reads as misplaced next to a button.
        Some(close) => row![main, close]
            .spacing(DISMISS_GAP)
            .align_y(iced::Alignment::Center)
            .width(Length::Fill)
            .into(),
        None => main,
    };
    // A stacked action spans the banner's full width, under the X too, so a
    // long localized label keeps its intrinsic width.
    let content: Element<'a, Message> = match stacked_action {
        Some(action) => column![header, action]
            .spacing(tokens.gap)
            .width(Length::Fill)
            .into(),
        None => header,
    };

    // --- Surface: tone bg + NEUTRAL border, group semantics ---
    let surface = container(content)
        .padding(
            Padding::new(tokens.padding_y)
                .left(tokens.padding_x)
                .right(tokens.padding_x),
        )
        .width(Length::Fill)
        // Correctly rectangular: padding_x/padding_y (12 px) exceeds
        // the radius (10 px), so a full-bleed child is inset past the
        // ~2.9 px arc depth on every side.  The banner builds its own
        // children (no caller slot for a full-bleed element).  Guard:
        // `rounded_clip_widget_sweep::banner_shape_padding_clears_arc_
        // full_bleed_child`. Not `clip(true)`: a clipping container clips its
        // own background quad, and with it the drop shadow.
        .style(move |theme: &iced::Theme| {
            let palette = crate::theme_lookup::palette(theme);
            // Info and success sit on the normal surface with the tone in
            // the glyph. Warning and danger take their soft status fill: a
            // "this disk will be erased" banner on a plain white card read
            // as one more card.
            let bg = match tone {
                BannerTone::Warning => palette.warning_soft,
                BannerTone::Danger => palette.danger_soft,
                BannerTone::Info | BannerTone::Success => palette.surface,
            };
            container::Style {
                background: Some(iced::Background::Color(bg)),
                border: Border {
                    color: if bordered {
                        crate::surface_edge::edge_color(bg, tokens.edge_depth)
                    } else {
                        Color::TRANSPARENT
                    },
                    width: 1.0,
                    radius: border::Radius::new(banner_radius(theme, tokens, radius_pinned)),
                },
                // On glass the glass casts the shadow.
                shadow: if palette.is_on_glass() {
                    iced::Shadow::default()
                } else {
                    crate::elevation::iced_shadow_for_theme(theme, tokens.elevation)
                },
                ..container::Style::default()
            }
        });

    // The surface is a live region (`Status` or `Alert`, not `Group`), so a
    // newly inserted banner is announced. It is built with `new()`, not
    // `leaf()`, so the action and dismiss buttons nest under it.
    let mut region = crate::semantics_container::SemanticsContainer::new(
        surface,
        banner_semantics(tone, title, body),
    );
    if let Some(id) = region_id {
        region = region.id(id);
    }
    // Interaction stays bounded to the banner's box; paint is not clipped
    // here, so the drop shadow outside the box survives.
    crate::inert_clip::InertClip::interaction_only(region).into()
}

/// The banner's live-region node: [`Status`](Role::Status) (polite) for
/// `Info` and `Success`, [`Alert`](Role::Alert) (assertive) for `Warning` and
/// `Danger`, so screen readers announce it when it is inserted.
///
/// It speaks one aggregate label, severity then title then body ("Warning:
/// Disk almost full. 12% left."), with no description: the label is what is
/// announced on insertion, and a description would repeat it on focus. The
/// visible title and body are built with `announce(false)` so they are not
/// read again.
pub fn banner_semantics(tone: BannerTone, title: &str, body: Option<&str>) -> Semantics {
    Semantics::new(tone.live_region_role()).label(aggregate_announcement(tone, title, body))
}

/// The single string a banner speaks: severity, then title, then body. An
/// empty title or body is dropped without leaving stray punctuation.
fn aggregate_announcement(tone: BannerTone, title: &str, body: Option<&str>) -> String {
    let severity = tone.severity_word();
    let title = title.trim();
    let body = body.map(str::trim).filter(|b| !b.is_empty());
    match (title.is_empty(), body) {
        (false, Some(b)) => {
            enzui_core::localized!("enzui-banner-description", "severity" => severity, "title" => title, "body" => b)
        }
        (false, None) => {
            enzui_core::localized!("enzui-banner-title-description", "severity" => severity, "title" => title)
        }
        (true, Some(b)) => {
            enzui_core::localized!("enzui-banner-body-description", "severity" => severity, "body" => b)
        }
        // Nothing to say but the severity. Still published: the region exists,
        // it is the caller who supplied no prose.
        (true, None) => severity,
    }
}

impl<'a, Message: 'a + Clone> From<EnzBanner<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzBanner<'a, Message>) -> Self {
        w.into_element()
    }
}

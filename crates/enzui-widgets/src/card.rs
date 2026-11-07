//! EnzCard: a styled container/card surface for enzUI-ICED.
//!
//! Rounded-rectangle container with optional title, description, footer. Five
//! tones (Default, Elevated, Flat, Outlined, Sunken); configurable padding,
//! gap, radius, border.
//!
//! # Where this belongs
//!
//! [`group_box`](crate::group_box) titles through this; [`preferences_group`](crate::preferences_group)
//! borrows card tokens for settings containers. Elsewhere: reach for `card`
//! wherever rounded-surface + title + description + footer fits. Plus: **per-
//! corner radius** ([`CardCorners`] + [`radius_top`](EnzCard::radius_top) /
//! [`radius_bottom`](EnzCard::radius_bottom) grouped-list helpers), trailing
//! header action slots ([`header_action`](EnzCard::header_action)),
//! bounded content — clipped caps ([`content_max_height`](EnzCard::content_max_height))
//! or scrollable caps ([`content_scrollable`](EnzCard::content_scrollable)
//! over [`EnzScrollView`](crate::scroll_view::EnzScrollView)). Shadows via
//! shared [`iced_shadow_for_level`](crate::elevation::iced_shadow_for_level).
//!
//! # Related
//!
//! Three jobs, three answers:
//!
//! * A discrete entity to act on — a card, a real tone. This module.
//! * A top-level page section — still this module, but [`Flat`](CardTone::Flat):
//!   the same 17 px bold [`title`](EnzCard::title) with NO chrome, inheriting
//!   its surface. Titled Flat auto-promotes to [`Section`](HeadingLevel::Section)
//!   (aria-level 2): heading navigation finds the prominent section header.
//!   Non-Flat stays ordinary labels (entity cards are not outline sections);
//!   `header_decorative(true)` callers (e.g. [`preferences_group`](crate::preferences_group))
//!   republish as `Role::Group` names — headings suppressed there against
//!   double-announcement.
//! * A caption above a form/settings group (11 px muted `APPEARANCE` band) —
//!   [`section_header`](crate::section_header): deliberately SUB-body-text;
//!   page-heading use inverts hierarchy.
//!
//! Settings panes: [`preferences_group`](crate::preferences_group) is the ready-made
//! flat-by-default specialisation (settings rhythm pre-applied).
//!
//! # Identity
//!
//! Header actions activate for real: they need routable identity. Set
//! [`id`](EnzCard::id) (or [`owner`](EnzCard::owner)) to publish
//! [`card_header_action_id`]; AT can focus + click it. Ownerless cards leave
//! actions anonymous — the collector strips undeliverable actions.

use crate::enz_text::{EnzText, TextSize, enz_text};
use enzui_core::identity::OwnerId;
use enzui_core::semantics::HeadingLevel;
use enzui_theme::{Palette, TypographyScale};
use iced::widget::column;
use iced::widget::text::LineHeight;
use iced::{Color, Element, Fill, Padding};

// ---------------------------------------------------------------------------
// Tone enum
// ---------------------------------------------------------------------------

/// Visual tone variant for the card surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CardTone {
    /// Neutral tonal card (surface-elevated fill, transparent 1px stroke, no shadow).
    ///
    /// This is the flat-by-default neutral entity surface: its one-pixel
    /// stroke is transparent and its shadow is off, preserving geometry while
    /// avoiding an automatic outlined, lifted box.
    #[default]
    Default,
    /// Popout emphasis (same bg, heavier shadow).
    Elevated,
    /// Inline grouping — paints no fill at all, so it inherits whatever
    /// surface it sits on, and has no border and no shadow.
    ///
    /// The page-section tone: `enz_card(body).title("Repository").tone(Flat)`
    /// = 17 px bold heading, zero box. ≡ [`transparent(true)`](EnzCard::transparent)
    /// (which forces the same on any other tone). Paints nothing — never
    /// `palette.background` (the WINDOW background: matching parents only when
    /// the parent is the window; on other panes it rectangles visibly — the
    /// name's broken promise).
    Flat,
    /// Grid tiles / bordered groups (surface-elevated bg, 1px border, no shadow).
    Outlined,
    /// Inset / recessed area (surface-alt bg).
    Sunken,
}

// ---------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------

/// Colour and geometry overrides for cards.
#[derive(Debug, Clone, Copy)]
pub struct CardTokens {
    /// Background for Default/Elevated/Outlined tones.
    pub surface: Color,
    /// Background for Sunken tone.
    pub surface_sunken: Color,
    /// The app/window background (`palette.background`).
    ///
    /// No tone resolves to this — [`Flat`](CardTone::Flat) paints nothing.
    /// That is what parent-inheritance actually requires. Retained: public API
    /// and legitimate runtime-palette readings for own-fill composers — never
    /// Flat's paint.
    pub background: Color,
    /// Edge colour (Elevated and Sunken tones): a shade of the card surface.
    pub border: Color,
    /// The Outlined tone's outline, its whole identity: the
    /// [`Palette::border_strong`] line by default, not a 3:1 outline. Pin a
    /// stronger colour for an outline that must carry contrast on its own.
    pub control_border: Color,
    /// Title text color.
    pub title_color: Color,
    /// Description/footer text color.
    pub description_color: Color,
    /// Default corner radius.
    pub radius: f32,
    /// Default border width (hairline).
    pub border_width: f32,
    /// Default padding X.
    pub padding_x: f32,
    /// Default padding Y.
    pub padding_y: f32,
    /// Default gap between sections.
    pub gap: f32,
    /// Title font size.
    pub title_size: f32,
    /// Description/subtitle font size.
    pub description_size: f32,
    /// Footer caption font size (smaller than the description).
    pub footer_size: f32,
    /// Gap within the header block (title <-> description).
    pub header_gap: f32,
}

impl CardTokens {
    /// Derive colours from a palette with fixed geometry presets.
    ///
    /// Passing these tokens to a card pins its radius. An unpinned card instead
    /// follows the theme's `shape.card` step (`radius.lg` by default).
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            surface: p.surface_elevated,
            surface_sunken: p.surface_alt,
            background: p.background,
            // A shade of the card surface, not a line in another colour.
            border: crate::surface_edge::edge_color(
                p.surface_elevated,
                crate::surface_edge::DEFAULT_EDGE_DEPTH,
            ),
            // The Outlined tone's outline: `border_strong`, a visible line
            // that stays clear of a light page without the 3:1 grey.
            control_border: p.border_strong,
            title_color: p.text,
            description_color: p.text_muted,
            radius: 8.0,
            border_width: 1.0,
            padding_x: 16.0,
            padding_y: 16.0,
            gap: 12.0,
            // Title 17 px (`title-size`), description 13 px
            // (`body-size`), footer 11 px, 2 px between title and description.
            title_size: 17.0,
            description_size: 13.0,
            footer_size: 11.0,
            header_gap: 2.0,
        }
    }

    /// Convenience: dark-theme tokens.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }

    /// Convenience: light-theme tokens.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }

    /// The border colour for a given tone.
    ///
    /// `Outlined` uses [`Self::control_border`], its outline, because the
    /// border is that tone's whole identity. Elevated and Sunken use the
    /// [`Self::border`] edge, a shade of the card surface.
    pub fn border_for_tone(&self, tone: CardTone) -> Color {
        match tone {
            CardTone::Default | CardTone::Flat => Color::TRANSPARENT,
            CardTone::Outlined => self.control_border,
            CardTone::Elevated | CardTone::Sunken => self.border,
        }
    }

    /// Returns (background, border_width) for a tone.
    fn tone_style(&self, tone: CardTone) -> (Color, f32) {
        match tone {
            CardTone::Default | CardTone::Elevated => (self.surface, self.border_width),
            // Flat paints nothing. `self.background` is the *window*
            // background, which equals the parent's colour only when the
            // parent is the window; on any other pane an opaque fill of it is
            // a visible rectangle. Transparency is the only fill that
            // genuinely inherits, and it is what `.transparent(true)` and
            // `preferences_group`'s flat path already did.
            CardTone::Flat => (Color::TRANSPARENT, 0.0),
            CardTone::Outlined => (self.surface, self.border_width),
            CardTone::Sunken => (self.surface_sunken, self.border_width),
        }
    }
}

// The neutral default is flat; Elevated's level-4 lift is what visually
// distinguishes it from Default.
fn shadow_level_for_tone(tone: CardTone) -> u8 {
    match tone {
        CardTone::Elevated => 4,
        CardTone::Default | CardTone::Flat | CardTone::Outlined | CardTone::Sunken => 0,
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Independent per-corner radius overrides. Each corner
/// defaults to the card's uniform radius; set one to square off an edge — a
/// card flush against a window edge, or a segmented card in a grouped-list
/// stack (first row rounds the top corners, last row the bottom, middle rows
/// none). Order matches iced's `border::Radius`: TL, TR, BR, BL.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CardCorners {
    /// Top-left corner radius override.
    pub top_left: Option<f32>,
    /// Top-right corner radius override.
    pub top_right: Option<f32>,
    /// Bottom-right corner radius override.
    pub bottom_right: Option<f32>,
    /// Bottom-left corner radius override.
    pub bottom_left: Option<f32>,
}

/// Horizontal gap between the header text block and a trailing header action
/// (`has-header-suffix` spacing). The semantic `sm` rung (8px), NAME-resolved
/// from shared [`SpacingScale`](enzui_theme::SpacingScale) (never bare
/// literals): rescaling themes flow through; tests pin the NAMED token (dodging
/// the equal-value trap where rungs share values). The 12px `md` section gap
/// is the wrong rung here.
fn header_action_gap() -> f32 {
    enzui_theme::SpacingScale::default().sm()
}

/// Clamp a requested corner radius to a safe, finite, non-negative value.
/// Radius APIs must not accept negative/non-finite values because they would
/// poison `iced::border::Radius` and the layout pass. A negative or
/// non-finite radius degrades to `0.0` (a squared corner).
fn sanitize_radius(r: f32) -> f32 {
    if r.is_finite() && r > 0.0 { r } else { 0.0 }
}

/// Clamp a [`CardRhythm`] metric (padding / gap / font size) to a finite,
/// non-negative value, falling back to `own_default` otherwise.
///
/// Fallback = the field's own previous value (`layout.padding_x` for
/// `padding_x`, …), never siblings: cross-metric fallback makes build
/// ORDER-dependent (`.padding_x(NaN).gap(20)` ≠ reverse). Own-token defaults
/// keep `.rhythm(..)` order-free. `NaN`/`±inf`/negatives are INEXPRESSIBLE
/// geometry (degrade to `own_default`); **`0.0` is expressible intent**
/// (zero padding/gap means exactly that) — honoured literally (the
/// [`sane_dim`](crate::chart_tooltip) + shared `FiniteNonNeg` policy).
fn sane_metric(v: f32, own_default: f32) -> f32 {
    if v.is_finite() && v >= 0.0 {
        v
    } else {
        own_default
    }
}

fn card_text(
    text: String,
    role: TextSize,
    pin: Option<f32>,
    typography: Option<TypographyScale>,
) -> EnzText {
    let text = enz_text(text);
    let text = match pin {
        Some(px) => text.size_px(px),
        None => text.size(role),
    };
    let text = match typography {
        Some(scale) => text.typography(scale),
        None => text,
    };
    text.line_height(LineHeight::Relative(1.3))
        .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
}

impl CardCorners {
    /// Resolve to an iced `iced::border::Radius`, filling unset corners with
    /// `uniform`.
    fn resolve(self, uniform: f32) -> iced::border::Radius {
        // `uniform` is defensively clamped so even a poisoned token default
        // can't produce a non-finite radius. Per-corner overrides are already
        // sanitized at their setters.
        let uniform = sanitize_radius(uniform);
        iced::border::Radius {
            top_left: self.top_left.unwrap_or(uniform),
            top_right: self.top_right.unwrap_or(uniform),
            bottom_right: self.bottom_right.unwrap_or(uniform),
            bottom_left: self.bottom_left.unwrap_or(uniform),
        }
    }

    /// True when at least one corner is overridden.
    fn any(self) -> bool {
        self.top_left.is_some()
            || self.top_right.is_some()
            || self.bottom_right.is_some()
            || self.bottom_left.is_some()
    }
}

/// Numeric-only rhythm overrides for an [`EnzCard`] (title/description size,
/// radius, padding, section gap). Unlike [`tokens`](EnzCard::tokens) (whole-set
/// pins freezing colours): metrics-only, colours still runtime-resolving from
/// `iced::Theme`. What lets thin specialisations (e.g. `EnzPreferencesGroup` —
/// `header-title-size`/`-description-size`/`card-radius`/`padding` overrides)
/// reuse runtime-aware chrome instead of duplicating
/// `Container`/`Column`/`Text` trees.
#[derive(Debug, Clone, Copy, Default)]
pub struct CardRhythm {
    /// Override the uniform corner radius (still overridden by [`EnzCard::radius`]/corners).
    pub radius: Option<f32>,
    /// Override the outer horizontal padding.
    pub padding_x: Option<f32>,
    /// Override the outer vertical padding.
    pub padding_y: Option<f32>,
    /// Override the section gap (header block <-> content <-> footer).
    pub gap: Option<f32>,
    /// Override the header title font size.
    pub title_size: Option<f32>,
    /// Override the header description font size.
    pub description_size: Option<f32>,
    /// Override the gap between the header text column and the trailing header
    /// action.
    ///
    /// Stock = `SpacingScale::sm`. Sans this field, rescaled ladders forwarded
    /// through every other `CardRhythm` metric still hit a STOCK header gap
    /// (`header_action_gap()` eagerly resolves `SpacingScale::default()`; the
    /// value dies into `Row::spacing` in themeless `layout()`). Invisible until
    /// rescaled + stock cards sit side by side.
    pub header_action_gap: Option<f32>,
    /// Inset the header (title, description, action) from the leading edge,
    /// independently of the content. A flat settings group uses it to put its
    /// title on the same edge as the labels of the inset rows below it.
    pub header_inset_x: Option<f32>,
}

/// Default identity namespace for a card given a bare instance key through
/// [`EnzCard::id`].
pub const DEFAULT_ID_PREFIX: &str = "enz-card";

// Fixed component: the header action keeps one identity while its label, the
// card title, and the card's position all change.
const HEADER_ACTION_ROLE: &str = "header-action";

/// The routable widget `iced::advanced::widget::Id` the header action of
/// the card owned by `owner` publishes.
///
/// Use it to aim a routed accessibility action, a focus request, or a test at
/// that control instead of re-deriving the component name. Returns the id the
/// card would publish whether or not such a card is mounted.
#[must_use]
pub fn card_header_action_id(owner: &OwnerId) -> iced::advanced::widget::Id {
    owner.id(HEADER_ACTION_ROLE)
}

/// Builder for an EnzCard element.
#[must_use]
pub struct EnzCard<'a, Message> {
    content: Element<'a, Message>,
    title: Option<String>,
    title_weight: enzui_theme::WeightRung,
    description: Option<String>,
    footer: Option<String>,
    tone: CardTone,
    tokens: Option<CardTokens>,
    rhythm: CardRhythm,
    typography: Option<TypographyScale>,
    force_transparent: bool,
    /// Narrow per-field pin for the surface fill. `None` = resolve the
    /// tone's fill from the runtime theme every render. Deliberately *not*
    /// [`EnzCard::tokens`]: pinning one colour must not freeze every other
    /// runtime role (the failure mode). Modelled on [`crate::status_bar`]'s
    /// `StatusBarStyle`.
    surface_pin: Option<Color>,
    /// Narrow per-field pin for the border colour. `None` = runtime
    /// `border` (or the tone-derived hairline).
    border_pin: Option<Color>,
    edge_depth: Option<f32>,
    radius: Option<f32>,
    /// The whole-struct corner base, set by [`EnzCard::corners`]. Per-corner
    /// setters do not write here; they write [`Self::corner_overrides`], a
    /// sidecar this field cannot clobber (the same failure mode).
    corners: CardCorners,
    /// Per-corner sidecar written by the narrow radius setters
    /// ([`radius_top_left`](EnzCard::radius_top_left) … [`radius_bottom`](EnzCard::radius_bottom)),
    /// overlaid on [`corners`](Self::corners) at read time by
    /// `effective_corners`.
    ///
    /// Narrow setters must never write through to `self.corners.<corner>` while
    /// [`corners`](EnzCard::corners) replaces whole structs:
    /// `.radius_top_left(20).corners(c)` silently dropped the 20 (and
    /// `.radius_top(20)` dropped FOUR subfields); reverse order kept it.
    /// A sidecar `corners()` never touches ⇒ per-corner wins in either order
    /// (specificity wins — the shell-`split_axis`-over-`tokens` shape).
    corner_overrides: CardCorners,
    /// See [`EnzCard::material`].
    material: Option<enzui_theme::MaterialStyle>,
    /// See [`EnzCard::grain`].
    grain: enzui_theme::GrainSpec,
    padding: Option<Padding>,
    header_action: Option<(String, Option<Message>)>,
    /// Arbitrary trailing header control. Takes precedence over the
    /// [`Self::header_action`] label/message convenience form, so callers can
    /// host an icon button, a toggle, a menu trigger, or any composed control
    /// instead of being forced into a Secondary/Sm text button.
    header_action_slot: Option<Element<'a, Message>>,
    /// Host-owned identity namespace for the controls this card builds itself.
    /// `None` leaves the header action anonymous, so the collector strips the
    /// actions no request could have reached.
    owner: Option<OwnerId>,
    content_max_height: Option<f32>,
    content_scrollable: bool,
    width: iced::Length,
    height: iced::Length,
    /// Explicit elevation shadow override: `Some((level, transition,
    /// reduced_motion))` renders `elevation-N`'s shadow directly over the
    /// tone-derived one. `transition = Some((from_level, progress))`
    /// interpolates; `reduced_motion` = the draw-time-theme-resolved
    /// per-instance override snapping transitions to targets. Powers
    /// [`EnzElevation`](crate::elevation::EnzElevation): thin card
    /// specialisation, never duplicated chrome.
    elevation_override: Option<ElevationOverride>,
    /// Live transition source. When set, the elevation override's
    /// static transition `(from, t)` is REPLACED at draw time by `from`/
    /// `progress` from these cells: [`EnzElevation`](crate::elevation::EnzElevation)'s
    /// internal `Animation` wrapper advances shadows sans caller-pinned static
    /// `progress`. `None` keeps static caller-driven progress.
    elevation_live: Option<ElevationLive>,
    /// When `true`, the header title + description are drawn but wrapped in
    /// [`decorative`](crate::decorative::decorative): zero `Role::Label`s in the
    /// AT tree. For title-republishing specialisations (e.g.
    /// `EnzPreferencesGroup`'s `Role::Group`): sans this the card's own title
    /// leaf double-announces. Default `false`: stand-alone titles announce as
    /// ordinary labels.
    header_decorative: bool,
}

/// Shared live transition state for the animated elevation path: the
/// `from` level being left behind and the eased progress `0..=1` toward the
/// override's target level, both updated per frame by the animating wrapper.
#[derive(Debug, Clone)]
pub(crate) struct ElevationLive {
    /// The level the transition is animating *from* (the previously-settled
    /// level). Updated by the wrapper when a new level change is detected.
    pub(crate) from: std::rc::Rc<std::cell::Cell<u8>>,
    /// Eased progress `0..=1` toward the override's target level.
    pub(crate) progress: std::rc::Rc<std::cell::Cell<f32>>,
}

/// `(level, transition, reduced_motion)` for [`EnzCard::elevation_override_rm`]
/// `transition = Some((from_level, progress))` interpolates the shadow;
/// `reduced_motion` is the per-instance override resolved against the theme at
/// draw time to snap the transition to its target.
type ElevationOverride = (u8, Option<(u8, f32)>, Option<bool>);

/// Create an EnzCard wrapping some content.
pub fn enz_card<'a, Message: 'a>(content: impl Into<Element<'a, Message>>) -> EnzCard<'a, Message> {
    EnzCard {
        content: content.into(),
        title: None,
        title_weight: enzui_theme::WeightRung::Bold,
        description: None,
        footer: None,
        tone: CardTone::default(),
        tokens: None,
        rhythm: CardRhythm::default(),
        typography: None,
        force_transparent: false,
        surface_pin: None,
        border_pin: None,
        edge_depth: None,
        radius: None,
        corners: CardCorners::default(),
        corner_overrides: CardCorners::default(),
        material: None,
        grain: enzui_theme::GrainSpec::None,
        padding: None,
        header_action: None,
        header_action_slot: None,
        owner: None,
        content_max_height: None,
        content_scrollable: false,
        width: Fill,
        height: iced::Length::Shrink,
        elevation_override: None,
        elevation_live: None,
        header_decorative: false,
    }
}

impl<'a, Message: 'a> EnzCard<'a, Message> {
    /// Lay grain on the card: a [`Grain`](enzui_theme::Grain), or
    /// [`GrainSpec::Theme`](enzui_theme::GrainSpec::Theme) for the theme's.
    /// See [`crate::grain`].
    pub fn grain(mut self, grain: impl Into<enzui_theme::GrainSpec>) -> Self {
        self.grain = grain.into();
        self
    }

    /// Make the card liquid glass of `style`: the content behind it shows
    /// through and bends at its rim, and controls inside default to glass.
    pub fn material(mut self, style: enzui_theme::MaterialStyle) -> Self {
        self.material = Some(style);
        self
    }

    /// Set the title; an empty string removes it.
    /// Defaults to Title3/Bold text. Flat cards publish a Section heading;
    /// other tones publish a label. Decorative headers suppress both so a
    /// parent can publish the title once as its accessible name.
    pub fn title(mut self, title: &str) -> Self {
        if title.is_empty() {
            self.title = None;
        } else {
            self.title = Some(title.to_owned());
        }
        self
    }

    /// Set a header description/subtitle.
    pub fn description(mut self, desc: &str) -> Self {
        if desc.is_empty() {
            self.description = None;
        } else {
            self.description = Some(desc.to_owned());
        }
        self
    }

    /// Set a footer caption.
    pub fn footer(mut self, footer: &str) -> Self {
        if footer.is_empty() {
            self.footer = None;
        } else {
            self.footer = Some(footer.to_owned());
        }
        self
    }

    /// Set the tone variant.
    pub fn tone(mut self, tone: CardTone) -> Self {
        self.tone = tone;
        self
    }

    /// Override the token set.
    pub fn tokens(mut self, tokens: CardTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Hide title and description semantics while retaining their painted text.
    /// Defaults to false. Use when a parent republishes them as its accessible name;
    /// Flat section-heading promotion is also suppressed.
    pub fn header_decorative(mut self, decorative: bool) -> Self {
        self.header_decorative = decorative;
        self
    }

    /// Select the header title's semantic weight; defaults to Bold.
    /// A forwarded typography ramp supplies the weight for this rung.
    pub fn title_weight(mut self, weight: enzui_theme::WeightRung) -> Self {
        self.title_weight = weight;
        self
    }

    /// Forward the type ramp used by the title, description, and footer.
    ///
    /// `Title3`/`Body`/`Caption` resolve when the element is built; rebuild after
    /// a scale change. Without a forwarded ramp, they follow the desktop text
    /// scale. Positive token and rhythm size pins win; body content is unchanged.
    pub fn typography(mut self, scale: TypographyScale) -> Self {
        self.typography = Some(scale);
        self
    }

    /// Override numeric rhythm (sizes/radius/padding/gap) while keeping colours
    /// runtime-theme-resolved. See [`CardRhythm`].
    pub fn rhythm(mut self, rhythm: CardRhythm) -> Self {
        self.rhythm = rhythm;
        self
    }

    /// Force the card background to fully transparent regardless of tone (so it
    /// inherits whatever surface it sits on — the flat
    /// `card-background: transparent`). Colours for text/border still resolve
    /// from the runtime theme.
    pub fn transparent(mut self, transparent: bool) -> Self {
        self.force_transparent = transparent;
        self
    }

    /// Pin the surface fill for this instance only, overriding the tone's
    /// runtime-resolved background.
    ///
    /// NARROW by design: borders, title/description ink, shadows keep tracking
    /// live themes. Prefer over [`tokens`](Self::tokens) for single-colour fixes:
    /// full snapshots freeze every runtime role (the failure mode).
    /// [`transparent`](Self::transparent) still wins: forced-transparent paints
    /// no fill under any pin.
    pub fn surface_color(mut self, color: Color) -> Self {
        self.surface_pin = Some(color);
        self
    }

    /// Pin the border colour for this instance only, overriding the runtime
    /// `border`. The fill and text still track the live theme unless
    /// separately pinned.
    pub fn border_color(mut self, color: Color) -> Self {
        self.border_pin = Some(color);
        self
    }

    /// Border the card with a shade of its own fill, `depth` of the way to
    /// black (`0.0..=1.0`), resolved from the live fill at draw time. For a
    /// raised card with a shadow: a hairline in its own hue instead of the
    /// palette's general border colour. [`border_color`](Self::border_color)
    /// wins over it.
    pub fn edge(mut self, depth: f32) -> Self {
        self.edge_depth = Some(if depth.is_finite() {
            depth.clamp(0.0, 1.0)
        } else {
            crate::surface_edge::DEFAULT_EDGE_DEPTH
        });
        self
    }

    /// Override the uniform corner radius. Negative/non-finite values are
    /// clamped to `0.0`.
    pub fn radius(mut self, r: f32) -> Self {
        self.radius = Some(sanitize_radius(r));
        self
    }

    /// Override all four corner radii independently. Unset corners fall
    /// back to the uniform [`Self::radius`]. Use for grouped-list rows / cards
    /// flush against an edge.
    ///
    /// # Order-independent: a base, not a clobber
    ///
    /// Whole-struct BASES; narrow per-corner setters
    /// ([`radius_top_left`](Self::radius_top_left) … [`radius_bottom`](Self::radius_bottom))
    /// record into an untouched-by-this sidecar (`EnzCard::corner_overrides`);
    /// `effective_corners` overlays at read time. So
    /// `.radius_top_left(20).corners(c)` ≡ `.corners(c).radius_top_left(20)`:
    /// specificity wins either order (the shell-`split_axis`-over-`tokens`
    /// shape). Sans sidecars, narrow setters wrote `self.corners` through —
    /// later `corners()` silently discarded them.
    pub fn corners(mut self, corners: CardCorners) -> Self {
        // Sanitize each overridden corner so a raw struct can't smuggle in a
        // negative/non-finite radius.
        self.corners = CardCorners {
            top_left: corners.top_left.map(sanitize_radius),
            top_right: corners.top_right.map(sanitize_radius),
            bottom_right: corners.bottom_right.map(sanitize_radius),
            bottom_left: corners.bottom_left.map(sanitize_radius),
        };
        self
    }

    /// Override the top-left corner radius only. Clamped to `>= 0`/finite.
    ///
    /// records into the `EnzCard::corner_overrides` sidecar, so it wins
    /// over a [`Self::corners`] call in either order.
    pub fn radius_top_left(mut self, r: f32) -> Self {
        self.corner_overrides.top_left = Some(sanitize_radius(r));
        self
    }

    /// Override the top-right corner radius only. Clamped to `>= 0`/finite.
    ///
    /// records into the `EnzCard::corner_overrides` sidecar.
    pub fn radius_top_right(mut self, r: f32) -> Self {
        self.corner_overrides.top_right = Some(sanitize_radius(r));
        self
    }

    /// Override the bottom-right corner radius only. Clamped to `>= 0`/finite.
    ///
    /// records into the `EnzCard::corner_overrides` sidecar.
    pub fn radius_bottom_right(mut self, r: f32) -> Self {
        self.corner_overrides.bottom_right = Some(sanitize_radius(r));
        self
    }

    /// Override the bottom-left corner radius only. Clamped to `>= 0`/finite.
    ///
    /// records into the `EnzCard::corner_overrides` sidecar.
    pub fn radius_bottom_left(mut self, r: f32) -> Self {
        self.corner_overrides.bottom_left = Some(sanitize_radius(r));
        self
    }

    /// Round only the top corners (top-left + top-right) to `r`, squaring the
    /// bottom. Convenience for the *first* row of a grouped-list stack.
    ///
    /// writes all four subfields into the `EnzCard::corner_overrides`
    /// sidecar (not `self.corners`), so a later [`Self::corners`] cannot
    /// silently discard the four values this sets.
    pub fn radius_top(mut self, r: f32) -> Self {
        let r = sanitize_radius(r);
        self.corner_overrides.top_left = Some(r);
        self.corner_overrides.top_right = Some(r);
        self.corner_overrides.bottom_left = Some(0.0);
        self.corner_overrides.bottom_right = Some(0.0);
        self
    }

    /// Round only the bottom corners (bottom-left + bottom-right) to `r`,
    /// squaring the top. Convenience for the *last* row of a grouped-list stack.
    ///
    /// writes all four subfields into the `EnzCard::corner_overrides`
    /// sidecar (not `self.corners`).
    pub fn radius_bottom(mut self, r: f32) -> Self {
        let r = sanitize_radius(r);
        self.corner_overrides.bottom_left = Some(r);
        self.corner_overrides.bottom_right = Some(r);
        self.corner_overrides.top_left = Some(0.0);
        self.corner_overrides.top_right = Some(0.0);
        self
    }

    /// The corners this card resolves, with the per-corner sidecar overlaid on
    /// the [`Self::corners`] base.
    ///
    /// A corner set individually beats the whole-struct value for that
    /// corner; corners only set as a whole stand. Both are sanitised by their
    /// setters. Resolving here rather than writing through at set time is
    /// what makes the setters order-independent.
    fn effective_corners(&self) -> CardCorners {
        let base = self.corners;
        let ovr = self.corner_overrides;
        CardCorners {
            top_left: ovr.top_left.or(base.top_left),
            top_right: ovr.top_right.or(base.top_right),
            bottom_right: ovr.bottom_right.or(base.bottom_right),
            bottom_left: ovr.bottom_left.or(base.bottom_left),
        }
    }

    /// Set this card's host-owned identity namespace.
    ///
    /// Names self-built controls: header actions publish
    /// [`card_header_action_id`]`(&owner)`. Pass host-model-owned owners:
    /// unique per window-mounted card, stable across rebuilds/retitles/
    /// reorders.
    ///
    /// # Accessibility
    ///
    /// Ownerless header actions go anonymous: absent from routing maps, the
    /// collector publishes read-only announcements (no `Click`, no focus)
    /// rather than advertising unreachable operations. Shared owners collide
    /// onto single nodes (later cards dropped): keys must DISTINGUISH.
    pub fn owner(mut self, owner: OwnerId) -> Self {
        self.owner = Some(owner);
        self
    }

    /// Set this card's host-owned stable instance key, namespaced under
    /// [`DEFAULT_ID_PREFIX`].
    ///
    /// The one-call form of [`Self::owner`]. Pass a key the host's model
    /// already uses (a record id, a route name) — never the visible title or a
    /// position, which change while the control stays the same.
    pub fn id(mut self, key: impl Into<String>) -> Self {
        self.owner = Some(OwnerId::of_kind(DEFAULT_ID_PREFIX, &key.into()));
        self
    }

    /// Add a trailing action control to the header row.
    /// Only rendered when the
    /// card also has a title or description. `on_press = None` renders it
    /// disabled.
    ///
    /// # Accessibility
    ///
    /// Routable only under card identity ([`id`](Self::id) / [`owner`](Self::owner));
    /// anonymous-card publications per those docs.
    pub fn header_action(mut self, label: &str, on_press: Message) -> Self {
        self.header_action = Some((label.to_owned(), Some(on_press)));
        self
    }

    /// Header action that may be disabled: `None` renders a disabled control.
    ///
    /// The disabled control keeps the same identity as the enabled one, so an
    /// assistive technology tracking the node does not lose it when the action
    /// becomes unavailable. It advertises no action in either case while it is
    /// disabled.
    pub fn header_action_maybe(mut self, label: &str, on_press: Option<Message>) -> Self {
        if label.is_empty() {
            self.header_action = None;
        } else {
            self.header_action = Some((label.to_owned(), on_press));
        }
        self
    }

    /// Host an arbitrary trailing header control: an icon button, a
    /// toggle, a menu trigger, a configured `enz_button`, etc. This is the
    /// general slot for arbitrary controls — unlike [`Self::header_action`] it is
    /// not restricted to a Secondary/Sm text button. It takes precedence over
    /// any label/message convenience action, and is only rendered when the
    /// card also has header text (title or description).
    ///
    /// # Accessibility
    ///
    /// Unbuilt-here controls keep caller-given identity: operable ones need
    /// own routable ids (e.g. [`id`](crate::button::EnzButton::id)). Card
    /// [`owner`](Self::owner)s name only label/message forms.
    pub fn header_action_element(mut self, action: impl Into<Element<'a, Message>>) -> Self {
        self.header_action_slot = Some(action.into());
        self
    }

    /// Cap the content region's height, clipping it past `max` while the card's
    /// chrome/header/footer keep their natural size. For real scrolling,
    /// compose an `EnzScrollView` inside the card.
    pub fn content_max_height(mut self, max: f32) -> Self {
        // Require a finite, positive cap: `0`/negative means "no cap", and a
        // non-finite value (NaN/Infinity) would poison the layout pass, so it
        // is rejected too.
        self.content_max_height = if max.is_finite() && max > 0.0 {
            Some(max)
        } else {
            None
        };
        // This selects the *clip* mode: clear any previously-set scrollable
        // flag so `.content_scrollable(n).content_max_height(m)` is
        // deterministic (last mode wins) and never leaves scrolling on while
        // rendering the clip path; builder order must not leave scrolling
        // enabled unexpectedly.
        self.content_scrollable = false;
        self
    }

    /// Bound the content region's height to `max` and make it scroll when
    /// the content exceeds the cap (the "bounded / scrollable content
    /// region" half). The richer [`content_max_height`](Self::content_max_height)
    /// variant: overflow scrolls inside via composed
    /// [`EnzScrollView`](crate::scroll_view::EnzScrollView) instead of hard
    /// clipping. Chrome/header/footer stay natural-sized. Non-finite/non-
    /// positive `max` disables.
    ///
    /// `max` is a MAXIMUM never a fixed height (`max-content-height`): short
    /// content hugs measured heights reserving nothing; only taller-than-`max`
    /// clamps to `max` + scrolls.
    ///
    /// [`content_max_height`]: EnzCard::content_max_height
    pub fn content_scrollable(mut self, max: f32) -> Self {
        self.content_max_height = if max.is_finite() && max > 0.0 {
            Some(max)
        } else {
            None
        };
        self.content_scrollable = self.content_max_height.is_some();
        self
    }

    /// Override the padding.
    pub fn padding(mut self, padding: impl Into<Padding>) -> Self {
        self.padding = Some(padding.into());
        self
    }

    /// Override the outer width (default [`Fill`]). Lets an
    /// [`crate::elevation::EnzElevation`] pass through `Length::Shrink` /
    /// fixed sizing while still inheriting card chrome.
    pub fn width_length(mut self, width: iced::Length) -> Self {
        self.width = width;
        self
    }

    /// Override the outer height (default `Length::Shrink`).
    pub fn height_length(mut self, height: iced::Length) -> Self {
        self.height = height;
        self
    }

    /// Render an explicit `elevation-N` shadow instead of the tone-derived
    /// one. `transition = Some((from_level, progress))` interpolates
    /// the shadow between levels. This is how
    /// [`EnzElevation`](crate::elevation::EnzElevation) specialises the card without
    /// duplicating its chrome.
    ///
    /// ≡ [`elevation_override_rm`](Self::elevation_override_rm) sans reduced-motion
    /// override (defers fully to draw-time theme policy).
    pub fn elevation_override(mut self, level: u8, transition: Option<(u8, f32)>) -> Self {
        self.elevation_override = Some((level.min(5), transition, None));
        self
    }

    /// [`Self::elevation_override`] with an explicit per-instance
    /// reduced-motion override. When reduced motion is in effect
    /// (this override, else the theme policy) the transition snaps to its
    /// target level at draw time instead of interpolating.
    pub fn elevation_override_rm(
        mut self,
        level: u8,
        transition: Option<(u8, f32)>,
        reduced_motion: Option<bool>,
    ) -> Self {
        self.elevation_override = Some((level.min(5), transition, reduced_motion));
        self
    }

    /// Supply a live transition source. When set, the `(from, t)` in
    /// [`elevation_override_rm`](Self::elevation_override_rm)'s `transition` is replaced at *draw
    /// time* by the `from`/`progress` read from these cells, letting
    /// [`EnzElevation`](crate::elevation::EnzElevation)'s internal `Animation`-driven
    /// wrapper advance the shadow every frame. The static `(from, t)` becomes
    /// only the initial value; the cells win on every draw. This is how the
    /// animated path threads its state through without a caller-pinned
    /// `progress`.
    pub(crate) fn elevation_live(mut self, live: ElevationLive) -> Self {
        self.elevation_live = Some(live);
        self
    }

    /// Build the element with live theme colours and the `shape.card` corner step.
    ///
    /// Token, rhythm, and explicit radius pins retain their pixel values.
    /// Unpinned text uses semantic type roles; padding and gaps use the fixed preset.
    ///
    /// `Message: Clone` is required because the optional header action is a
    /// canonical `enz_button` (which composes `FocusButton`).
    pub fn into_element(self) -> Element<'a, Message>
    where
        Message: Clone,
    {
        let pinned_tokens = self.tokens;
        let tone = self.tone;
        let radius_override = self.radius;
        // overlay the per-corner sidecar on the whole-struct base so both
        // `.radius_*().corners()` orderings resolve identically.
        let corners = self.effective_corners();
        let elevation_override = self.elevation_override;
        let elevation_live = self.elevation_live;
        let out_width = self.width;
        let out_height = self.height;

        // Layout has no theme argument; spacing and pinned sizes use the builder's preset.
        let layout = pinned_tokens.unwrap_or_else(CardTokens::dark);
        let rhythm = self.rhythm;
        let force_transparent = self.force_transparent;
        let surface_pin = self.surface_pin;
        let border_pin = self.border_pin;
        let edge_depth = self.edge_depth;
        let radius_pin = radius_override
            .or(rhythm.radius)
            .or_else(|| pinned_tokens.map(|t| t.radius));
        // Public-`Option<f32>` `CardRhythm` fields bypass setters entirely
        // (`Some(NaN)`/`Some(-1.0)` straight in); `unwrap_or(layout.*)` guards
        // UNSET (`None`) only — poisoned `Some(..)` flowed raw into `Padding` /
        // spacing / text geometry. Sanitize per-metric against own token
        // defaults (siblings ⇒ order-dependence); `0.0` literal-honoured.
        let padding = self.padding.unwrap_or_else(|| {
            let px = sane_metric(
                rhythm.padding_x.unwrap_or(layout.padding_x),
                layout.padding_x,
            );
            let py = sane_metric(
                rhythm.padding_y.unwrap_or(layout.padding_y),
                layout.padding_y,
            );
            Padding::new(py).left(px).right(px)
        });
        let gap = sane_metric(rhythm.gap.unwrap_or(layout.gap), layout.gap);
        let title_size = rhythm
            .title_size
            .or_else(|| pinned_tokens.map(|t| t.title_size))
            .map(|px| sane_metric(px, layout.title_size));
        let description_size = rhythm
            .description_size
            .or_else(|| pinned_tokens.map(|t| t.description_size))
            .map(|px| sane_metric(px, layout.description_size));
        let typography = self.typography;
        // same `sane_metric` contract as the other rhythm metrics --
        // sanitised against its own stock token, never a sibling, so the
        // builder stays order-independent and `0.0` is honoured literally.
        let header_action_gap_px = sane_metric(
            rhythm.header_action_gap.unwrap_or_else(header_action_gap),
            header_action_gap(),
        );
        let footer_size = pinned_tokens.map(|t| t.footer_size);
        let header_gap = layout.header_gap;
        let header_decorative = self.header_decorative;
        let action_id = self.owner.as_ref().map(card_header_action_id);

        // optionally suppress the title/description from AT (they still
        // draw). A specialisation that republishes the title as a grouping
        // node's name sets this so the string is not announced twice.
        let maybe_decorative = |el: Element<'a, Message>| -> Element<'a, Message> {
            if header_decorative {
                crate::decorative::decorative(el).into()
            } else {
                el
            }
        };

        let resolve = move |theme: &iced::Theme| -> CardTokens {
            crate::theme_lookup::resolve_pinned(
                crate::theme_lookup::PIN_SITE_CARD,
                pinned_tokens,
                theme,
                CardTokens::from_palette,
            )
        };

        // Keep title and description in one header block so the outer gap does not separate them.
        let mut inner = column![].spacing(gap);

        let has_header_text = self.title.is_some() || self.description.is_some();
        if has_header_text {
            let mut header = column![].spacing(header_gap);
            if let Some(title_text) = self.title {
                let resolve_title = resolve;
                // Flat section titles enter heading navigation unless their parent republishes them.
                let title_heading: Option<HeadingLevel> =
                    if tone == CardTone::Flat && !header_decorative {
                        Some(HeadingLevel::Section)
                    } else {
                        None
                    };
                let mut title_widget =
                    card_text(title_text, TextSize::Title3, title_size, typography)
                        .weight_rung(self.title_weight)
                        .color_resolved(move |theme| resolve_title(theme).title_color);
                if let Some(level) = title_heading {
                    title_widget = title_widget.heading(level);
                }
                header = header.push(maybe_decorative(title_widget.into()));
            }
            if let Some(desc_text) = self.description {
                let resolve_desc = resolve;
                header = header.push(maybe_decorative(
                    card_text(desc_text, TextSize::Body, description_size, typography)
                        .color_resolved(move |theme| resolve_desc(theme).description_color)
                        .into(),
                ));
            }

            // Trailing header actions need header text. Arbitrary
            // `header_action_element` slots beat label/message convenience forms
            // (canonical Secondary/Sm buttons — disabled + inert + traversal-
            // dropped at `on_press = None`). Text columns take remainders:
            // actions pin trailing. Owner ids enter routing maps: routed Clicks
            // arm pointer-release-identical activation lines.
            let action_el: Option<Element<'a, Message>> = self.header_action_slot.or_else(|| {
                self.header_action.map(|(action_label, action_msg)| {
                    let builder = match action_msg {
                        Some(msg) => crate::button::enz_button(action_label, msg),
                        None => crate::button::enz_inert_button(
                            action_label,
                            crate::button::ButtonKind::Secondary,
                        ),
                    }
                    .kind(crate::button::ButtonKind::Secondary)
                    .size(crate::button::ButtonSize::Sm);
                    match action_id {
                        Some(id) => builder.id(id).into(),
                        None => builder.into(),
                    }
                })
            });
            // In right-to-left text the header runs from the right edge.
            let rtl = enzui_core::placement::ambient_direction()
                == enzui_core::placement::Direction::RightToLeft;
            let header = if rtl {
                header.width(Fill).align_x(iced::Alignment::End)
            } else {
                header
            };
            let header_inset = rhythm.header_inset_x.filter(|x| x.is_finite() && *x > 0.0);
            let header: Element<'a, Message> = match header_inset {
                Some(x) => iced::widget::container(header)
                    .padding(if rtl {
                        iced::Padding::ZERO.right(x)
                    } else {
                        iced::Padding::ZERO.left(x)
                    })
                    .into(),
                None => header.into(),
            };
            let action_el = action_el.map(|el| -> Element<'a, Message> {
                match header_inset {
                    Some(x) => iced::widget::container(el)
                        .padding(if rtl {
                            iced::Padding::ZERO.left(x)
                        } else {
                            iced::Padding::ZERO.right(x)
                        })
                        .into(),
                    None => el,
                }
            });
            if let Some(action_el) = action_el {
                inner = inner.push(
                    iced::widget::row![iced::widget::container(header).width(Fill), action_el]
                        .mirrored(rtl)
                        // 8px (`SpacingScale::sm`), not the 12px section gap.
                        // resolved above so a forwarded ladder reaches it.
                        .spacing(header_action_gap_px)
                        .align_y(iced::Alignment::Center),
                );
            } else {
                inner = inner.push(header);
            }
        }

        // Height-bounded content, two modes on shared `MaxHeight`
        // shrink-until-cap wrappers: SHORT children hug content, TALL children
        // cap (`max-content-height` semantics; `Fixed(max)` would inflate shorts
        // to caps — maximum-height-contract violations).
        //   * `content_scrollable`: capped `EnzScrollView`s — overflow SCROLLS
        //     in-card (bounded/scrollable regions).
        //   * `content_max_height`-only: capped `.clip(true)` containers —
        //     overflow CLIPPED (lightweight non-scroll viewports).
        // Chrome/header/footer stay natural-sized regardless. Mutually exclusive
        // (`content_scrollable` implies caps + flags): builder orders can never
        // strand scrolling on after cap-clears.
        //
        // Scrollable arms need explicit `Shrink` `EnzScrollView` heights:
        // `MaxHeight` clamps incoming maxima then reports child measures — right
        // shape, but default-`Fill` `EnzScrollView`s resolve via
        // `Limits::resolve_height` straight to capped maxima (2-line content +
        // `content_scrollable(400.0)` measured 432px over true 94px). `Shrink`
        // compresses: content-hugging below caps, caps biting above
        // (`MaxHeight` never hands children more than `cap`). `MaxHeight`
        // itself stood correct (`max_height_wrapper_itself_hugs_a_shrink_child`).
        let content_el: Element<'a, Message> = match self.content_max_height {
            Some(max_h) if self.content_scrollable => crate::max_height::MaxHeight::new(
                crate::scroll_view::enz_scroll_view(self.content).height(iced::Length::Shrink),
                max_h,
            )
            .into(),
            // Clip-without-scroll arms strand past-cap content UNREACHABLY:
            // `clip_interaction(true)` clips focus traversal + AT trees + pointer
            // events with drawings — keyboard focus can never vanish into
            // unrevealable content. Scrollable arms deliberately OMIT it:
            // scrolled-to content stays reachable.
            Some(max_h) => crate::max_height::MaxHeight::new(
                iced::widget::container(self.content).clip(true),
                max_h,
            )
            .clip_interaction(true)
            .into(),
            None => self.content,
        };
        inner = inner.push(content_el);

        if let Some(footer_text) = self.footer {
            let resolve_footer = resolve;
            inner = inner.push(
                card_text(footer_text, TextSize::Caption, footer_size, typography)
                    .color_resolved(move |theme| resolve_footer(theme).description_color),
            );
        }

        let resolve_radius = move |theme: &iced::Theme| {
            let radius =
                sanitize_radius(radius_pin.unwrap_or_else(|| {
                    crate::theme_lookup::shape_radius(theme, |shape| shape.card)
                }));
            if corners.any() {
                corners.resolve(radius)
            } else {
                radius.into()
            }
        };

        let (_, border_w_tone) = layout.tone_style(tone);
        // Border-width gating: the outline shows only at elevation level > 0.
        // When an explicit elevation override is set the
        // gate follows the override's level; otherwise the tone owns the
        // outline. Theme-independent, so resolved outside the closure.
        let border_w = match elevation_override {
            Some((level, _, _)) => {
                if level > 0 {
                    border_w_tone
                } else {
                    0.0
                }
            }
            None => border_w_tone,
        };

        // Drop shadows route shared-elevation-source-of-truth (never
        // hand-rolled): explicit overrides contribute levels (+ transitions),
        // else tone-derived shadows. CONVERTIBLE: draw-time resolvers read live
        // `EnzTheme.elevation` ladders (`iced_shadow_*_for_theme`) over
        // compile-time tables — custom scales reach paints.
        let shadow_resolver = move |theme: &iced::Theme| -> iced::Shadow {
            match elevation_override {
                Some((level, Some((from, t)), reduced)) => {
                    // an internal animation may drive the transition live
                    // via shared cells; when present they *replace* the static
                    // `(from, t)` so the shadow advances every frame toward the
                    // override's target `level`.
                    let (from, t) = elevation_live
                        .as_ref()
                        .map_or((from, t), |l| (l.from.get(), l.progress.get()));
                    // resolve reduced-motion (override, else theme) at draw
                    // time and snap the transition when it is on.
                    crate::elevation::iced_shadow_transition_for_theme_rm(
                        theme, from, level, t, reduced,
                    )
                }
                Some((level, None, _)) => crate::elevation::iced_shadow_for_theme(theme, level),
                None => crate::elevation::iced_shadow_for_theme(theme, shadow_level_for_tone(tone)),
            }
        };

        // EnzSurface owns the single rounded child clip, leaving the outer shadow unmasked.
        let material = self.material;
        let surface = crate::surface::enz_surface(inner)
            .grain(self.grain)
            .width(out_width)
            .height(out_height)
            .clip(true)
            .padding(padding)
            .radius_resolved(resolve_radius)
            .border_width(border_w)
            // Fill: narrow per-field pins sit between the tone's runtime
            // fill and the transparent override. `transparent(true)` wins; an
            // unpinned field keeps resolving from `theme`.
            .fill_resolved(move |theme: &iced::Theme| {
                if force_transparent {
                    Color::TRANSPARENT
                } else {
                    let (bg, _) = resolve(theme).tone_style(tone);
                    surface_pin.unwrap_or(bg)
                }
            })
            .border_color_resolved(move |theme: &iced::Theme| {
                let tokens = resolve(theme);
                if let Some(pin) = border_pin {
                    return pin;
                }
                // A raised card's outline is a shade of its own fill; a
                // transparent one would only eat the card's outer pixel.
                let raised = matches!(elevation_override, Some((level, _, _)) if level > 0);
                match edge_depth {
                    Some(depth) => {
                        let (bg, _) = tokens.tone_style(tone);
                        crate::surface_edge::edge_color(surface_pin.unwrap_or(bg), depth)
                    }
                    None if raised && tokens.border_for_tone(tone).a == 0.0 => {
                        let (bg, _) = tokens.tone_style(tone);
                        crate::surface_edge::edge_color(
                            surface_pin.unwrap_or(bg),
                            crate::surface_edge::DEFAULT_EDGE_DEPTH,
                        )
                    }
                    None => tokens.border_for_tone(tone),
                }
            })
            .shadow_resolved(shadow_resolver);
        match material {
            Some(style) => surface.material(style).into(),
            None => surface.into(),
        }
    }
}

impl<'a, Message: Clone + 'a> From<EnzCard<'a, Message>> for Element<'a, Message> {
    fn from(card: EnzCard<'a, Message>) -> Self {
        card.into_element()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

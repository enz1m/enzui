//! EnzToolbarIcon: circular icon-only button for titlebars / toolbars.
//!
//! In a titlebar this is the *leaf*: group several of them inside
//! [`crate::titlebar_icon_group`], which sits in the content slot of
//! [`crate::title_bar`]. See `# Composing a titlebar` there for the whole
//! stack, including [`crate::titlebar_actions`] for the narrow-screen fold
//!
//!
//! An `EnzButton` with icon-only position, fully-circular radius, no border, and a
//! two-state colour scheme:
//!
//! - Inactive (ghost): transparent fill, `text-muted` glyph,
//!   `titlebar-icon-hover-background` on hover.
//! - Active (secondary): `surface-selected` fill, `text` glyph;
//!   used to indicate a persistently-pressed / toggled state.
//!
//! Icons resolve via `enzui-icons` — canonical Lucide names
//! or semantic aliases (e.g. `"action.settings"`, `"navigation.forward"`).
//!
//! # Two kinds of toolbar icon
//!
//! [`EnzToolbarIcon::active`] is what distinguishes them, and the distinction
//! is carried by an `Option`, not a `bool`:
//!
//! | call | paint | published semantics |
//! |---|---|---|
//! | (never calls `active`) | ghost | `Role::Button`, no toggle state |
//! | `.active(false)` | ghost | `Role::Button` + `Toggle::False` |
//! | `.active(true)` | `surface-selected` | `Role::Button` + `Toggle::True` |
//!
//! An action icon is an ordinary push button and must announce as one; a
//! Bold / Pin / Lock control is a toggle whose persistent state must be
//! published — `.active(false)` is as load-bearing as `.active(true)`,
//! since "off" is a state AT has to read.
//!
//! # Example
//!
//! ```
//! use enzui_widgets::toolbar_icon::enz_toolbar_icon;
//!
//! #[derive(Debug, Clone)]
//! enum Msg { Settings, Bold }
//!
//! // An ordinary action: no toggle state is published.
//! // `.aria_label(..)` is required — without it an `assert!` panics.
//! let action: iced::Element<'_, Msg> =
//!     enz_toolbar_icon("action.settings", Msg::Settings)
//!         .aria_label("Settings")
//!         .into();
//!
//! // A toggle: the state is published either way round.
//! let bold_off: iced::Element<'_, Msg> =
//!     enz_toolbar_icon("bold", Msg::Bold)
//!         .aria_label("Bold")
//!         .active(false).into();
//! let bold_on: iced::Element<'_, Msg> =
//!     enz_toolbar_icon("bold", Msg::Bold)
//!         .aria_label("Bold")
//!         .active(true).into();
//! ```

use enzui_theme::Palette;
use iced::{Color, Element};

// ---------------------------------------------------------------------------
// Tokens and variants
// ---------------------------------------------------------------------------

/// Theme-reactive visual policy: applied after the live theme (or an
/// explicit token override) resolves, so choosing one never pins the
/// control to the build-time palette.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ToolbarIconVariant {
    /// Standard toolbar chrome: translucent interaction fills and stable muted
    /// glyph ink.
    #[default]
    Standard,
    /// Chromeless feedback where the box abuts a seam or window edge:
    /// transparent background, glyph moving muted-to-primary on hover/press.
    InkOnly,
    /// Standard fills with the glyph in primary ink at rest: controls that
    /// are the content rather than chrome, such as media transport.
    Primary,
}

impl ToolbarIconVariant {
    fn apply(self, mut tokens: ToolbarIconTokens) -> ToolbarIconTokens {
        if self == Self::InkOnly {
            // Reuse the set's primary enabled ink, so explicit overrides stay
            // coherent and the unpinned path stays theme-reactive.
            tokens.hover_color = tokens.active_color;
            tokens.pressed_color = tokens.active_color;
            tokens.hover_background = Color::TRANSPARENT;
            tokens.pressed_background = Color::TRANSPARENT;
        }
        if self == Self::Primary {
            tokens.icon_color = tokens.active_color;
            tokens.hover_color = tokens.active_color;
            tokens.pressed_color = tokens.active_color;
        }
        tokens
    }
}

/// Shared geometry authority for the titlebar row's icon-only affordances:
/// collapsed search trigger, toolbar actions, sidebar disclosure, window
/// controls. Preferred box + glyph size derive here; the a11y hit-target
/// floor applies afterwards ([`effective_control_size`](Self::effective_control_size)),
/// keeping row size separate from target minimum.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TitlebarAffordanceTokens {
    /// Preferred square control-box side before the hit-target floor.
    pub control_size: f32,
    /// Nominal SVG glyph size shared by the row's icon actions.
    pub glyph_size: f32,
    /// Minimum toolbar slot height in which the control is centred.
    pub slot_height: f32,
}

impl TitlebarAffordanceTokens {
    /// Stock titlebar geometry: a 28 px box carrying a 16 px glyph on a 32 px
    /// toolbar track.
    pub const DEFAULT: Self = Self {
        control_size: 28.0,
        glyph_size: crate::icon::DEFAULT_ICON_SIZE,
        slot_height: 32.0,
    };

    /// Preferred size against the global interaction floor: larger visual
    /// wins; the floor never becomes glyph size nor changes row geometry.
    #[must_use]
    pub fn effective_control_size(self) -> f32 {
        self.control_size
            .max(enzui_theme::SizingTokens::default().min_hit_target)
    }
}

impl Default for TitlebarAffordanceTokens {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Colour + geometry tokens for a toolbar icon button.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToolbarIconTokens {
    /// Glyph colour when inactive + enabled.
    pub icon_color: Color,
    /// Glyph colour on hover (inactive + enabled); defaults to `icon_color`
    /// (existing callers pixel-identical). Set brighter for the flat-at-rest
    /// idiom without a background fill. Mirrors `BreadcrumbTokens::hover_color`
    /// and [`SidebarToggleButtonTokens`](crate::sidebar_toggle_button)'s hover fg.
    pub hover_color: Color,
    /// Glyph colour on press (inactive + enabled); defaults to `icon_color`
    /// (a strict no-op).
    pub pressed_color: Color,
    /// Glyph colour when active + enabled.
    pub active_color: Color,
    /// Full-strength source glyph colour when disabled, before the widget's
    /// disabled opacity is composited over its backdrop.
    ///
    /// [`from_palette`](Self::from_palette) uses [`Palette::text`]: opacity
    /// is the complete disabled treatment, so this source must not be
    /// pre-dimmed — overriding callers supply enabled-strength sources too.
    pub disabled_color: Color,
    /// Fill when active.
    pub active_background: Color,
    /// Fill on hover (inactive state). Translucent
    /// `titlebar-icon-hover-background` (white/black alpha overlay).
    pub hover_background: Color,
    /// Inset in px drawing the painted interaction fill *inside* the box;
    /// box, focus ring and hit rect unchanged. Defaults to `0.0`
    /// (pixel-identical: the fill is coextensive with the box, which is why
    /// hover lands flush against an abutting divider or window edge).
    /// `> 0.0` gives breathing room without moving the box (fill circle
    /// diameter `button_size - 2 * inset`); hit rect and ring untouched, so
    /// only the painted fill moves inward.
    pub hover_fill_inset: f32,
    /// Fill on press (inactive state). Translucent
    /// `titlebar-icon-pressed-background`.
    pub pressed_background: Color,
    /// Circular control diameter (`button-height - 4px` = 28px on the
    /// default 32px control track).
    pub button_size: f32,
    /// Minimum slot height the control centres inside:
    /// `titlebar-control-height = max(button-height - 4, field-md-height)`
    /// (32px default). Taller layout rows stretch it further.
    pub slot_height: f32,
    /// Icon glyph size (`icon-sm` = 16px).
    pub icon_size: f32,
    /// Focus-ring colour from `Palette::focus_ring` (dark `#0A84FF`, light
    /// `#006CDC`): a dedicated token rather than `active_color`, so the ring
    /// stays distinguishable from the active-state glyph.
    pub focus_ring: Color,
    /// Focus-ring border width in px (shared convention: `2.0`).
    /// Matches [`ButtonTokens::focus_ring_width`](crate::tokens::ButtonTokens).
    pub focus_ring_width: f32,
}

impl ToolbarIconTokens {
    /// Derive from an enzUI palette. Geometry follows
    /// `button-height - 4 px` (≈32 px chrome) and `icon-sm` (16 px).
    ///
    /// Palette mapping: `icon-color` →
    /// `text-muted`; `active-color` → `text`; `disabled-color` → `text` as
    /// the full-strength source for the single disabled-alpha treatment;
    /// `active-background` → `surface-selected`; `hover-background` → the
    /// shared `overlay_hover()` wash, like every other composited-hover
    /// widget; `pressed-background` → the shared `overlay_pressed()` wash.
    pub fn from_palette(p: &Palette) -> Self {
        let geometry = TitlebarAffordanceTokens::DEFAULT;
        Self {
            icon_color: p.text_muted,
            // Default interaction tints to `icon_color` (existing callers
            // pixel-identical); the flat/brighten idiom sets these and zeroes
            // `hover_background`, and the resolver switches ink instead of
            // painting a pill.
            hover_color: p.text_muted,
            pressed_color: p.text_muted,
            active_color: p.text,
            // Alpha is the complete disabled treatment: feeding the
            // already-weakened `p.disabled` through it applied the treatment
            // twice (the framework's worst contrast).
            disabled_color: p.text,
            // The toggled state is the
            // persistent-selection role, so read `surface_selected`, not the
            // raw `selected` alias (identical by default; distinct once a
            // theme overrides one).
            active_background: p.surface_selected,
            hover_background: p.overlay_hover(),
            // 0.0 default: fill coextensive with the box, the historical
            // behaviour every caller depends on.
            hover_fill_inset: 0.0,
            pressed_background: p.overlay_pressed(),
            // One geometry authority; consumers apply the hit-target floor
            // after this preferred size.
            button_size: geometry.control_size,
            slot_height: geometry.slot_height,
            icon_size: geometry.glyph_size,
            // Dedicated focus-ring token, matching the shared
            // `ButtonTokens::focus_ring` / `focus_ring_width` convention.
            focus_ring: p.focus_ring,
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

    /// Normalize raw `f32` geometry so no non-finite or negative value
    /// reaches renderer coordinates — the single normalization point (the
    /// [`tokens`](EnzToolbarIcon::tokens) setter runs every override through
    /// it). Every field is [`FiniteNonNeg`](enzui_core::metrics::FiniteNonNeg):
    /// zero/sub-pixel are meaningful intent, preserved not floored; no field
    /// disables on zero, so no `FinitePositive`. Fallbacks are the
    /// `from_palette` values; colours pass through.
    #[must_use]
    pub fn sanitized(&self) -> Self {
        use enzui_core::metrics::FiniteNonNeg;
        let geometry = TitlebarAffordanceTokens::DEFAULT;
        Self {
            button_size: FiniteNonNeg::new_or(self.button_size, geometry.control_size).get(),
            slot_height: FiniteNonNeg::new_or(self.slot_height, geometry.slot_height).get(),
            icon_size: FiniteNonNeg::new_or(self.icon_size, geometry.glyph_size).get(),
            focus_ring_width: FiniteNonNeg::new_or(self.focus_ring_width, 2.0).get(),
            // Zero inset is legitimate (the default no-op): preserve like the
            // other geometry; garbage falls back to `0.0`.
            hover_fill_inset: FiniteNonNeg::new_or(self.hover_fill_inset, 0.0).get(),
            ..*self
        }
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Toolbar-icon builder.
#[must_use]
pub struct EnzToolbarIcon<'a, Message> {
    /// See [`EnzToolbarIcon::material`].
    material: Option<enzui_theme::MaterialStyle>,
    icon: crate::icon::IconSource,
    on_activated: Option<Message>,
    /// Event-time activation for controlled menu hosts, separate from the
    /// eager constructor so a view rebuild never invokes the host mapper.
    on_activated_lazy: Option<Box<dyn Fn() -> Message + 'a>>,
    tokens: Option<ToolbarIconTokens>,
    variant: ToolbarIconVariant,
    /// Configured toggle state, or `None` for an ordinary action button —
    /// the `Option` is what separates "toggle currently off" from "not a
    /// toggle", which a plain `bool` cannot express.
    active: Option<bool>,
    /// Configured disclosure state, or `None` for a control that reveals
    /// nothing. Orthogonal to [`Self::active`]: `active` is `aria-pressed`
    /// (two-state control is on), this is `aria-expanded` (revealed region
    /// is open). A sidebar toggle opens an otherwise-absent region; it turns
    /// nothing on.
    expanded: Option<bool>,
    enabled: bool,
    aria_label: Option<String>,
    /// Hover tooltip via the canonical enzUI [`FadeTooltip`](crate::tooltip::FadeTooltip),
    /// above the control.
    tooltip: Option<String>,
    tooltip_delay: Option<std::time::Duration>,
    /// Stable focus / test id for the inner `FocusButton`: address the icon
    /// deterministically (focus targeting, `iced_test` selectors), not by
    /// position.
    id: Option<iced::advanced::widget::Id>,
    /// Override for [`ToolbarIconTokens::hover_fill_inset`] without pinning
    /// the whole token set: rides on the geometry read while `tokens` stays
    /// `None`, so the inset takes effect and colours stay theme-reactive
    /// (the shell's collapse toggle needs per-theme colours).
    hover_fill_inset: Option<f32>,
    /// `.size(control, glyph)` override; see [`EnzToolbarIcon::size`].
    size_override: Option<(f32, f32)>,
    /// Focus-return reclaim token for the inner `FocusButton` (`None`
    /// leaves the control out of the reclaim path).
    reclaim_focus_on_return: Option<iced::advanced::widget::Id>,
    /// Release half of a focus transfer for the inner `FocusButton`
    /// (`false` leaves it out of the path).
    relinquish_focus_while_open: bool,
    /// See [`EnzToolbarIcon::busy`].
    busy: bool,
    _phantom: std::marker::PhantomData<&'a ()>,
}

impl<'a, Message: Clone + 'a> EnzToolbarIcon<'a, Message> {
    /// Override the glyph source. Bundled names follow toolbar ink; app-owned
    /// SVGs and rasters retain their colours inside the glyph box.
    /// An explicit [`Self::aria_label`] remains required for every source.
    pub fn icon_source(mut self, source: impl Into<crate::icon::IconSource>) -> Self {
        self.icon = source.into();
        self
    }

    /// Override the tokens.
    pub fn tokens(mut self, t: ToolbarIconTokens) -> Self {
        // Normalize at the public boundary so no non-finite geometry token can
        // reach renderer coordinates.
        self.tokens = Some(t.sanitized());
        self
    }

    /// Select a named, theme-reactive visual policy ([`ToolbarIconVariant::InkOnly`]
    /// keeps the background transparent for seam-flush chrome and brightens
    /// ink on hover/press) without freezing runtime-theme resolution like a
    /// full [`ToolbarIconTokens`] would.
    pub fn variant(mut self, variant: ToolbarIconVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Inset the painted interaction fill by `px` without pinning token
    /// colours (unlike [`Self::tokens`], which pins the whole set). `0.0`
    /// is a strict no-op (fill coextensive with the box); `> 0.0` moves the
    /// fill onto an inner container, leaving box, hit rect and focus ring.
    /// Non-finite/negative floor to `0.0` at consumption.
    pub fn hover_fill_inset(mut self, px: f32) -> Self {
        self.hover_fill_inset = Some(px);
        self
    }

    /// Set the control's box and glyph size without pinning its colours
    /// (full [`ToolbarIconTokens`] would freeze them to one palette). A dense
    /// surface such as a toast closes with a 24 px control. Non-finite or
    /// negative sizes keep the default.
    pub fn size(mut self, control: f32, glyph: f32) -> Self {
        self.size_override = Some((control, glyph));
        self
    }

    /// Declare this icon a toggle and set its state: calling this at all
    /// is the declaration, the argument the state. `.active(false)` is not a
    /// no-op — it publishes [`Toggle::False`](enzui_core::semantics::Toggle::False),
    /// mapping the node to a toggle button in AT-SPI; never calling this
    /// leaves an ordinary action button. `true` paints persistently-pressed
    /// (`surface-selected` + `text`); `false` paints ghost, looking exactly
    /// like an action — which is why the state has to be spoken.
    pub fn active(mut self, on: bool) -> Self {
        self.active = Some(on);
        self
    }

    /// Declare this icon a disclosure for a collapsible region and set
    /// its open/closed state. Publishes `aria-expanded`: the region is absent
    /// from the tree while closed (structural visibility, not an oversight),
    /// so this control is the only evidence it exists — without it the
    /// collapsed state is unrepresented and undiscoverable.
    ///
    /// # Why not [`Self::active`]
    ///
    /// `active` is `aria-pressed` (control is *on*); this is `aria-expanded`
    /// (control *reveals* something). AT phrases the two differently, and a
    /// sidebar toggle is unambiguously the second — the sidebar is not "on".
    ///
    /// # The advertised action follows the id, deliberately
    ///
    /// A routed action looks its node id back up to a widget, so an
    /// anonymous node routes nothing (advertised-but-dead). Hence with an
    /// [`id`](Self::id) the state goes through `Semantics::expanded` (action
    /// advertised; inner button takes the id as `expand_alias`, reaching the
    /// click handler), without one through `Semantics::expanded_state_only`
    /// (announced, no action). Resolved at build time: builder order does
    /// not matter.
    pub fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = Some(expanded);
        self
    }

    /// Show the spinner in place of the glyph while the control's work runs.
    ///
    /// A refresh button stays where it is and keeps its name, tooltip and
    /// action; only the glyph turns. Put the work's details in the
    /// [`tooltip`](Self::tooltip) rather than in a banner.
    pub fn busy(mut self, on: bool) -> Self {
        self.busy = on;
        self
    }

    /// Enable / disable the button.
    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    /// Accessible name — required for icon-only buttons. Unset falls
    /// back to the glyph name (artwork metadata, not the action): an
    /// `assert!` panics and a `MissingName` audit finding is emitted.
    pub fn aria_label(mut self, l: impl Into<String>) -> Self {
        self.aria_label = Some(l.into());
        self
    }

    /// Override the app's hover delay for this control; zero starts revealing immediately.
    pub fn tooltip_delay(mut self, delay: std::time::Duration) -> Self {
        self.tooltip_delay = Some(delay);
        self
    }

    /// Override the delayed hover hint, which otherwise uses the accessible name.
    /// Empty text suppresses it. Set [`Self::aria_label`] to change the name.
    pub fn tooltip(mut self, tip: impl Into<String>) -> Self {
        self.tooltip = Some(tip.into());
        self
    }

    /// Name the action once: the accessible name and the hover tooltip. The
    /// usual form for an icon-only titlebar or toolbar button; use
    /// [`Self::aria_label`] and [`Self::tooltip`] only when they must differ.
    pub fn label(self, label: impl Into<String>) -> Self {
        let label = label.into();
        self.aria_label(label.clone()).tooltip(label)
    }

    /// Stable focus / test id for the inner `FocusButton`.
    pub fn id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Opt in to reclaiming keyboard focus when a transient menu this
    /// trigger opened closes. Pass the token the popup owner arms
    /// ([`crate::menu::EnzMenu::focus_return_to`]), set **only while the menu
    /// is CLOSED** (`None` removes the control from the path). Unlike
    /// [`Self::id`] this is a matching key only, never the public widget id —
    /// no second addressable node.
    pub fn reclaim_focus_on_return(mut self, token: Option<iced::advanced::widget::Id>) -> Self {
        self.reclaim_focus_on_return = token;
        self
    }

    /// Opt in to releasing keyboard focus while this trigger's transient
    /// menu is open. Pass `true` for exactly the open frames, paired with
    /// [`Self::reclaim_focus_on_return`] set only while closed: the popup's
    /// mount grab solely owns focus, the trigger reclaims on close.
    pub fn relinquish_focus_while(mut self, open: bool) -> Self {
        self.relinquish_focus_while_open = open;
        self
    }

    /// Put the icon button on liquid glass of `style` (see [`crate::glass`]).
    /// Controls inside default to glass.
    pub fn material(mut self, style: enzui_theme::MaterialStyle) -> Self {
        self.material = Some(style);
        self
    }

    /// Convert into an iced [`Element`] via the shared round icon-action
    /// leaf (the `icon_button` engine) with the toolbar preset: the builder
    /// owns defaults, token/variant policy and the missing-name diagnostic;
    /// the leaf owns geometry, paint, activation, semantics, tooltip, id and
    /// the scan probe.
    pub fn into_element(self) -> Element<'a, Message> {
        let material = self.material;
        let radius: f32 = self
            .size_override
            .map_or(14.0, |(control, _)| control / 2.0);
        crate::glass::wrap(self.build_element(), material, radius)
    }

    fn build_element(self) -> Element<'a, Message> {
        // Geometry is theme-independent, so read it from whichever tokens
        // resolve; colours resolve lazily per runtime theme in the leaf.
        let geom = self
            .tokens
            .unwrap_or_else(|| ToolbarIconTokens::from_palette(&Palette::dark()));

        // The `.hover_fill_inset(px)` override wins and rides the geometry
        // read without pinning colours (`tokens` stays `None`, so the leaf
        // stays theme-reactive). Garbage floors to `0.0` in the leaf, the
        // same discipline `ToolbarIconTokens::sanitized` applies.
        let fill_inset = self.hover_fill_inset.unwrap_or(geom.hover_fill_inset);

        let has_explicit_aria_label = self.aria_label.is_some();
        // A missing name fails loudly while developing; a shipped app falls
        // back to the icon's name rather than crashing.
        debug_assert!(
            has_explicit_aria_label,
            "enz_toolbar_icon({:?}): icon-only button has no aria_label — \
             artwork metadata {:?} cannot name the action. Set .aria_label(\"...\") to \
             describe what the button does.",
            self.icon, self.icon,
        );
        let label = self.aria_label.clone().unwrap_or_else(|| {
            log::warn!("enz_toolbar_icon({:?}) has no aria_label", self.icon);
            match &self.icon {
                crate::icon::IconSource::Named(name) => name.replace(['-', '_'], " "),
                _ => String::from("Button"),
            }
        });

        let spec = crate::icon_button::IconButtonSpec {
            icon: match self.icon {
                _ if self.busy => crate::icon_button::IconSource::Spinner,
                crate::icon::IconSource::Named(name) => crate::icon_button::IconSource::Name(name),
                crate::icon::IconSource::Svg(handle) => {
                    crate::icon_button::IconSource::UntintedSvg(handle)
                }
                crate::icon::IconSource::SymbolicSvg(handle) => {
                    crate::icon_button::IconSource::Handle(handle)
                }
                crate::icon::IconSource::Image(handle) => {
                    crate::icon_button::IconSource::Image(handle)
                }
            },
            on_activated: self.on_activated,
            enabled: self.enabled,
            style: crate::icon_button::IconButtonStyle::Toolbar {
                pinned: self.tokens,
                variant: self.variant,
            },
            box_size: self.size_override.map_or(geom.button_size, |(control, _)| {
                enzui_core::metrics::FiniteNonNeg::new_or(control, geom.button_size).get()
            }),
            icon_size: self.size_override.map_or(geom.icon_size, |(_, glyph)| {
                enzui_core::metrics::FiniteNonNeg::new_or(glyph, geom.icon_size).get()
            }),
            // 28px circle centred in a >=32px slot: mixed controls share one
            // vertical centreline.
            // An explicit `.size()` hugs its own box: the slot exists to line
            // titlebar controls up, not to pad a toast's close control.
            slot_height: if self.size_override.is_some() {
                None
            } else {
                Some(geom.slot_height)
            },
            fill_inset,
            // Alpha is the COMPLETE disabled treatment on the
            // enabled-strength source ink; another dim after this point
            // would apply the treatment twice.
            disabled_glyph_opacity: Some(TOOLBAR_ICON_DISABLED_OPACITY),
            active: self.active,
            expanded: self.expanded,
            name: crate::icon_button::IconButtonName {
                label: Some(label),
                audit_missing: !has_explicit_aria_label,
            },
            publish_disabled: true,
            // Tooltip above the icon: toolbar convention.
            tooltip_delay: self.tooltip_delay,
            tooltip: self
                .tooltip
                .map(|tip| (tip, iced::widget::tooltip::Position::Top)),
            id: self.id,
            reclaim_focus_on_return: self.reclaim_focus_on_return,
            relinquish_focus_while_open: self.relinquish_focus_while_open,
        };
        match self.on_activated_lazy {
            Some(factory) => crate::icon_button::build_lazy(spec, Some(factory)),
            None => crate::icon_button::build(spec),
        }
    }
}

impl<'a, Message: Clone + 'a> From<EnzToolbarIcon<'a, Message>> for Element<'a, Message> {
    fn from(t: EnzToolbarIcon<'a, Message>) -> Self {
        t.into_element()
    }
}

/// Construct a toolbar icon button, defaulting to an ordinary action
/// button: no toggle state reaches the AT tree until [`EnzToolbarIcon::active`]
/// declares one.
pub fn enz_toolbar_icon<'a, Message: Clone + 'a>(
    icon: impl Into<String>,
    on_activated: Message,
) -> EnzToolbarIcon<'a, Message> {
    EnzToolbarIcon {
        material: None,
        icon: crate::icon::IconSource::Named(icon.into()),
        on_activated: Some(on_activated),
        on_activated_lazy: None,
        tokens: None,
        variant: ToolbarIconVariant::default(),
        active: None,
        expanded: None,
        enabled: true,
        aria_label: None,
        tooltip: None,
        tooltip_delay: None,
        id: None,
        // No inset by default: fill coextensive with the box, painted by
        // the button quad, exactly as before.
        hover_fill_inset: None,
        size_override: None,
        reclaim_focus_on_return: None,
        relinquish_focus_while_open: false,
        busy: false,
        _phantom: std::marker::PhantomData,
    }
}

/// Crate-internal lazy constructor for controlled menu hosts: the same
/// builder and leaf as [`enz_toolbar_icon`], but the message is created at
/// activation time, preserving menu-button's exactly-once factory contract.
pub(crate) fn enz_toolbar_icon_maybe_with<'a, Message: Clone + 'a>(
    icon: impl Into<String>,
    on_activated: Option<Box<dyn Fn() -> Message + 'a>>,
) -> EnzToolbarIcon<'a, Message> {
    EnzToolbarIcon {
        material: None,
        icon: crate::icon::IconSource::Named(icon.into()),
        on_activated: None,
        on_activated_lazy: on_activated,
        tokens: None,
        variant: ToolbarIconVariant::default(),
        active: None,
        expanded: None,
        enabled: true,
        aria_label: None,
        tooltip: None,
        tooltip_delay: None,
        id: None,
        hover_fill_inset: None,
        size_override: None,
        reclaim_focus_on_return: None,
        relinquish_focus_while_open: false,
        busy: false,
        _phantom: std::marker::PhantomData,
    }
}

// Whole-control alpha affordance (not an opaque disabled ink), observed
// through the real paint path by the rendered-pixel gate: never multiply
// another disabled alpha into it.
const TOOLBAR_ICON_DISABLED_OPACITY: f32 = 0.59;

pub(crate) fn resolve_toolbar_icon_tokens(
    theme: &iced::Theme,
    pinned: Option<ToolbarIconTokens>,
    variant: ToolbarIconVariant,
) -> ToolbarIconTokens {
    variant.apply(
        pinned.unwrap_or_else(|| {
            crate::theme_lookup::resolve(theme, ToolbarIconTokens::from_palette)
        }),
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod sanitized_geometry_reaches_layout;

// ---------------------------------------------------------------------------
// Focus ring is the SHARED icon-action ring (contrast-aware ink, 2 px),
// not raw `tokens.focus_ring` at `tokens.focus_ring_width`.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod focus_ring_style;

// ---------------------------------------------------------------------------
// Missing aria_label — assert! + a11y audit finding
// ---------------------------------------------------------------------------

#[cfg(test)]
mod missing_accessible_name;

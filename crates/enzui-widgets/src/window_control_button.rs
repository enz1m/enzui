//! EnzWindowControlButton: icon-only round chip for window chrome.
//!
//! A small, borderless, round button used for
//! close/minimize/maximize icons in the titlebar. Uses a surface-hover
//! fill by default and a surface-selected fill on hover — a subtle
//! Chrome-style pill that lives on the titlebar without competing
//! with the main EnzButton family.
//!
//! Accessibility: the button is icon-only, so its glyph
//! carries no accessible name. `aria_label` defaults to empty and, when unset
//! (or all-whitespace), the `Role::Button` node would reach AT-SPI nameless
//! — a close / minimise / maximise control announced as nothing. Always set
//! [`aria_label`](EnzWindowControlButton::aria_label) with the action
//! ("Close", "Minimize", …). The composed [`crate::window_controls`] chrome
//! sets these; direct callers must supply their own. When it is
//! omitted, an `assert!` panics and a
//! [`MissingNameScanProbe`](crate::accessibility_scan_widgets::MissingNameScanProbe)
//! records an
//! [`AccessibilityCheck::MissingName`](enzui_core::accessibility_scan::AccessibilityCheck::MissingName)
//! finding for the a11y audit — mirroring `toolbar_icon`.
//!
//! # Example
//!
//! ```
//! use enzui_widgets::window_control_button::enz_window_control_button;
//!
//! #[derive(Debug, Clone)]
//! enum Msg { Close }
//! // `.aria_label(..)` is required — without it an `assert!` panics and the
//! // The Role::Button node would otherwise reach AT-SPI nameless.
//! let el: iced::Element<'_, Msg> =
//!     enz_window_control_button("action.close", Msg::Close)
//!         .aria_label("Close")
//!         .into();
//! ```
//!
//! # Deferred
//!
//! - Hover animation — iced buttons style per Status; deferred.

use iced::{Color, Element};

/// This widget's documented ink-pin precedence, stated once.
///
/// `false` means [`EnzWindowControlButton::foreground`] is the rest-state
/// ink only: hover keeps [`EnzWindowControlButton::hover_foreground`] and the
/// inert state keeps `Palette::disabled`. The sidebar toggle's `.icon_color(..)`
/// deliberately differs (it holds in every state) — see
/// [`ChipPins::ink_pin_holds_in_every_state`](crate::icon_button::ChipPins::ink_pin_holds_in_every_state)
/// and [`sidebar_toggle_button::INK_PIN_HOLDS_IN_EVERY_STATE`](crate::sidebar_toggle_button::INK_PIN_HOLDS_IN_EVERY_STATE).
pub(crate) const INK_PIN_HOLDS_IN_EVERY_STATE: bool = false;

/// Default control diameter in px, derived from the shared titlebar-row
/// authority (`button-height ~32 - 4`).
pub const DEFAULT_CONTROL_SIZE: f32 =
    crate::toolbar_icon::TitlebarAffordanceTokens::DEFAULT.control_size;
/// Default glyph size in px, derived from the shared titlebar-row authority
/// (`icon-sm`).
pub const DEFAULT_ICON_SIZE: f32 =
    crate::toolbar_icon::TitlebarAffordanceTokens::DEFAULT.glyph_size;

/// A single icon-only window-control button (close / minimize / maximize).
/// Circular by default, with configurable rest and hover colours. Built via
/// [`enz_window_control_button`]. Because it is icon-only, callers should
/// set [`aria_label`](Self::aria_label) for accessibility.
#[must_use]
pub struct EnzWindowControlButton<'a, Message> {
    icon: String,
    on_activate: Message,
    aria_label: String,
    tooltip: Option<String>,
    tooltip_delay: Option<std::time::Duration>,
    control_size: f32,
    icon_size: f32,
    foreground: Option<Color>,
    button_background: Option<Color>,
    hover_background: Option<Color>,
    hover_foreground: Option<Color>,
    fill_inset: f32,
    hover_emphasis: f32,
    enabled: bool,
    /// Stable widget id forwarded to the composed [`crate::focus_button`], so
    /// the control is addressable over the AT bus. `None` leaves the
    /// button anonymous — collected and announced, but absent from
    /// `NodeIdMap`, so a routed Focus/Click resolves to nothing. The composed
    /// [`crate::window_controls`] cluster supplies a per-action id;
    /// direct callers who want addressability must set their own.
    id: Option<iced::advanced::widget::Id>,
    _lifetime: std::marker::PhantomData<&'a ()>,
}

/// Create an [`EnzWindowControlButton`] rendering `icon`, emitting
/// `on_activate` when pressed.
pub fn enz_window_control_button<'a, Message: Clone + 'a>(
    icon: impl Into<String>,
    on_activate: Message,
) -> EnzWindowControlButton<'a, Message> {
    EnzWindowControlButton {
        icon: icon.into(),
        on_activate,
        aria_label: String::new(),
        tooltip: None,
        tooltip_delay: None,
        control_size: DEFAULT_CONTROL_SIZE, // button-height ~32 - 4
        icon_size: DEFAULT_ICON_SIZE,       // icon-sm
        foreground: None,
        button_background: None,
        hover_background: None,
        hover_foreground: None,
        fill_inset: 0.0,
        hover_emphasis: 1.0,
        enabled: true,
        id: None,
        _lifetime: std::marker::PhantomData,
    }
}

impl<'a, Message: Clone + 'a> EnzWindowControlButton<'a, Message> {
    /// Override the app's hover delay for this control; zero starts revealing immediately.
    pub fn tooltip_delay(mut self, delay: std::time::Duration) -> Self {
        self.tooltip_delay = Some(delay);
        self
    }

    /// Override the hover hint; an empty string disables it while retaining its name.
    pub fn tooltip(mut self, tip: impl Into<String>) -> Self {
        self.tooltip = Some(tip.into());
        self
    }

    /// Set the button's diameter in px (the hit target); the corner radius
    /// is half this (see [`radius`](Self::radius)).
    ///
    /// Non-finite (`NaN`, `±inf`) and negative inputs are not expressible and
    /// fall back to the default `28.0`. `0.0` is expressible intent (a
    /// zero-size control) and is honoured literally.
    pub fn control_size(mut self, s: f32) -> Self {
        self.control_size =
            enzui_core::metrics::FiniteNonNeg::new_or(s, DEFAULT_CONTROL_SIZE).get();
        self
    }
    /// Set the glyph size in px.
    ///
    /// Non-finite (`NaN`, `±inf`) and negative inputs are not expressible and
    /// fall back to the default `16.0`. `0.0` is honoured literally.
    pub fn icon_size(mut self, s: f32) -> Self {
        self.icon_size = enzui_core::metrics::FiniteNonNeg::new_or(s, DEFAULT_ICON_SIZE).get();
        self
    }
    /// Override the rest-state icon colour (defaults to the theme's muted text).
    pub fn foreground(mut self, c: Color) -> Self {
        self.foreground = Some(c);
        self
    }
    /// Override the rest-state background colour.
    pub fn button_background(mut self, c: Color) -> Self {
        self.button_background = Some(c);
        self
    }
    /// Override the hover-state background colour.
    pub fn hover_background(mut self, c: Color) -> Self {
        self.hover_background = Some(c);
        self
    }
    /// Draw the hover and pressed fill `inset` px inside the hit target, as a
    /// control sitting in a shared pill does. The target keeps its full size.
    pub fn fill_inset(mut self, inset: f32) -> Self {
        self.fill_inset = enzui_core::metrics::FiniteNonNeg::new_or(inset, 0.0).get();
        self
    }
    /// Step the theme's hover fill further from the resting fill, by `factor`
    /// times the `surface_hover` to `surface_selected` step (1.0 by default).
    pub(crate) fn hover_emphasis(mut self, factor: f32) -> Self {
        self.hover_emphasis = enzui_core::metrics::FiniteNonNeg::new_or(factor, 1.0).get();
        self
    }
    /// Override the hover-state icon colour.
    pub fn hover_foreground(mut self, c: Color) -> Self {
        self.hover_foreground = Some(c);
        self
    }
    /// Accessible name for the AT tree. Required — this is an
    /// icon-only interactive control (never presentational), so with no
    /// `aria_label` the `Role::Button` node reaches AT-SPI nameless. Pass the
    /// action, e.g. `"Close"` / `"Minimize"`.
    ///
    /// An empty or all-whitespace name is treated identically to an unset
    /// one: a blank name is useless to a screen reader and, on a window
    /// control, is always an omission rather than deliberate intent. Omitting
    /// it fires an `assert!` and emits an
    /// [`AccessibilityCheck::MissingName`](enzui_core::accessibility_scan::AccessibilityCheck::MissingName)
    /// finding.
    pub fn aria_label(mut self, s: impl Into<String>) -> Self {
        self.aria_label = s.into();
        self
    }
    /// Enable or disable the button; disabled buttons are muted and
    /// non-interactive.
    pub fn enabled(mut self, e: bool) -> Self {
        self.enabled = e;
        self
    }
    /// Assign a stable widget `Id` so the control
    /// is addressable over the AT bus — programmatic `focus(id)` and a routed
    /// AccessKit `Click` both resolve through it. An icon-only button
    /// with no id reaches AT-SPI announced but anonymous: its `NodeId`
    /// falls back to collection order, so a traversal-order change silently
    /// repoints it. Forwarded verbatim to the composed
    /// [`crate::focus_button::FocusButton::id`], which owns the
    /// `focusable`/activation registration.
    ///
    /// This is a *setter only*: the widget never auto-derives an id (an
    /// icon-only button has no stable logical key of its own), so callers — or
    /// the composing [`crate::window_controls`] cluster — supply one.
    pub fn id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }

    /// The corner radius (half the control size) — a fully-rounded circle.
    pub fn radius(&self) -> f32 {
        self.control_size / 2.0
    }

    /// The chip paint pins this builder hands the shared leaf.
    ///
    /// Runtime-theme resolution: every unpinned colour must defer its token
    /// lookup to the leaf's per-render style closures. Reading from a build-time
    /// `Palette::dark()` would make a light theme draw dark-palette ink and fills.
    ///
    /// `ink_pin_holds_in_every_state` is `false` here, which is this widget's
    /// documented precedence: [`Self::foreground`] is the rest-state ink,
    /// while hover keeps [`Self::hover_foreground`] and the inert state keeps
    /// `Palette::disabled`. (The sidebar toggle deliberately differs; see
    /// [`ChipPins::ink_pin_holds_in_every_state`](crate::icon_button::ChipPins::ink_pin_holds_in_every_state).)
    fn chip_pins(&self) -> crate::icon_button::ChipPins {
        crate::icon_button::ChipPins {
            ink_rest: self.foreground,
            ink_hover: self.hover_foreground,
            fill_rest: self.button_background,
            fill_hover: self.hover_background,
            ink_pin_holds_in_every_state: INK_PIN_HOLDS_IN_EVERY_STATE,
            hover_emphasis: self.hover_emphasis,
        }
    }

    /// Convert into an iced `iced::Element`.
    ///
    /// Delegates to the shared round icon-action leaf
    /// (the crate-internal `icon_button` engine) with the window-control
    /// preset: this builder owns the chrome defaults, the `FiniteNonNeg`
    /// geometry guards and
    /// the required-name diagnostic; the leaf owns the square box,
    /// status-aware paint, activation, semantics, id and the scan probe.
    ///
    /// # Panics
    ///
    /// When the control has no accessible name.
    pub fn into_element(self) -> Element<'a, Message> {
        // An unnamed window control publishes a `Role::Button` node with
        // NO accessible name at all — a close / minimise / maximise control
        // announced as nothing. Unlike `toolbar_icon` there is no glyph
        // fallback here, so the node is genuinely nameless, which is the worse
        // of the two failure modes. This is latent today (every call site names
        // its children — the composite `enz_window_controls` hardcodes
        // "Close"/"Minimize"/… and the probe sets each explicitly), so the trap
        // only fires for a *future* direct use of the leaf. An `assert!`
        // catches it and the leaf's `MissingNameScanProbe`
        // wrapper makes the omission a regression-testable a11y-audit finding.
        //
        // An empty *or all-whitespace* label is treated identically to an unset
        // one: this is an interactive window control, never a presentational
        // element, so a blank name is always an omission, never intent. A
        // whitespace-only name is equally useless to a screen reader, so the
        // guard trims — mirroring `tooltip_is_meaningful`.
        let has_name = !self.aria_label.trim().is_empty();
        assert!(
            has_name,
            "enz_window_control_button({:?}): icon-only window control has no \
             aria_label — the Role::Button node reaches AT-SPI with NO name at \
             all, so a screen reader announces nothing. Set \
             .aria_label(\"Close\"/\"Minimize\"/…) to describe the action.",
            self.icon,
        );
        let pins = self.chip_pins();
        let tooltip = self.tooltip.unwrap_or_else(|| self.aria_label.clone());
        crate::icon_button::build(crate::icon_button::IconButtonSpec {
            icon: crate::icon_button::IconSource::Name(self.icon),
            on_activated: Some(self.on_activate),
            enabled: self.enabled,
            style: crate::icon_button::IconButtonStyle::Chip(pins),
            box_size: self.control_size,
            icon_size: self.icon_size,
            // The chip *is* the control: window chrome centres it in the band
            // itself (see `window_controls`), so there is no taller slot here.
            slot_height: None,
            fill_inset: self.fill_inset,
            // No opacity treatment: an inert window control dims its glyph to
            // `Palette::disabled` and keeps its resting pill.
            disabled_glyph_opacity: None,
            // Window chrome is neutral: a control is never a toggle and never a
            // disclosure, so neither axis is exposed.
            active: None,
            expanded: None,
            name: crate::icon_button::IconButtonName {
                label: has_name.then_some(self.aria_label),
                audit_missing: !has_name,
            },
            publish_disabled: false,
            tooltip: Some((tooltip, iced::widget::tooltip::Position::Bottom)),
            tooltip_delay: self.tooltip_delay,
            // Forward the caller/cluster-supplied stable id so the control is
            // routable over the AT bus. A disabled leaf carries no
            // activation callback and so publishes no routable carrier
            // regardless, but it still registers the id for programmatic focus.
            id: self.id,
            // Window chrome triggers no transient focus-owning menu.
            reclaim_focus_on_return: None,
            relinquish_focus_while_open: false,
        })
    }
}

impl<'a, Message: Clone + 'a> From<EnzWindowControlButton<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzWindowControlButton<'a, Message>) -> Self {
        w.into_element()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Missing aria_label — assert! + a11y audit finding
// ---------------------------------------------------------------------------
//
// Mirrors the mechanism used by `toolbar_icon`. The window control leaf is
// icon-only and has NO glyph fallback, so an unnamed control publishes a
// `Role::Button` node with no name at all. Latent today (every call site names
// its children), so these guard a future *direct* use of the leaf.

#[cfg(test)]
mod missing_accessible_name {}

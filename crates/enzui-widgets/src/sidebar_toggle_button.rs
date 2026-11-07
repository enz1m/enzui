//! EnzSidebarToggleButton: circular icon-only chip that toggles the sidebar.
//!
//! A themed, circular, icon-only button used in the header bar to open/close
//! the primary sidebar (GNOME / macOS convention). Same footprint as
//! `EnzToolbarIcon` so it sits on the same vertical centreline; the
//! pill-radius background is `surface_hover` at rest and `surface_selected`
//! on hover — same aliases used by `EnzWindowControlButton` (see palette).
//!
//! # Icon and hover foreground
//!
//! The glyph is Lucide `panel-left` (semantic alias
//! `navigation.sidebar_left`) by default and is configurable via
//! [`EnzSidebarToggleButton::icon`] — callers place the toggle on the
//! left or the right and point it open or closed. The glyph re-tints
//! with interaction status through the icon's `tint_with_status`
//! resolver, sharing a status cell with the button's `style_focus` (the
//! Pattern), so the icon and its background move together: rest
//! `text_muted`, hover/pressed `text` (or the caller's
//! [`EnzSidebarToggleButton::hover_foreground`]). An explicit
//! [`EnzSidebarToggleButton::icon_color`] pins the tint across every
//! state.
//!
//! # Deferred
//!
//! - Pressed-state colour — the shared `FocusStatus` cell reports `Pressed`,
//!   which this widget maps to the hover fill and hover foreground for
//!   parity.

use iced::{Color, Element};

/// Default semantic icon alias — Lucide `panel-left` via `navigation.sidebar_left`.
pub(crate) const DEFAULT_ICON: &str = "navigation.sidebar_left";

/// The icon-name a builder with `icon` set to `name` will actually render.
///
/// Single source of truth for the glyph (mirrors `document_tab::favicon_handle`):
/// [`EnzSidebarToggleButton::into_element`] resolves the icon from exactly this
/// name, so a test asserting the resolved `iced::widget::svg::Handle` `id()`
/// proves which glyph paints — not a stored field.
///
/// An empty string falls back to [`DEFAULT_ICON`] rather than rendering an
/// empty glyph: a sidebar toggle is meaningless without an icon, so "no icon"
/// is not an expressible intent here. This deliberately differs from
/// `button.rs`'s `.icon("")`, which omits the icon entirely — a labelled button
/// stays legible without its optional glyph, this icon-only chip does not.
pub(crate) fn resolved_icon_name(name: &str) -> &str {
    if name.is_empty() { DEFAULT_ICON } else { name }
}

/// This widget's documented ink-pin precedence, as one value both the render
/// path and its tests read.
///
/// `true` means an explicit [`EnzSidebarToggleButton::icon_color`] pin holds in
/// every interaction state and outranks
/// [`EnzSidebarToggleButton::hover_foreground`]. The window control's
/// `.foreground(..)` deliberately differs (rest-state only); see
/// [`ChipPins::ink_pin_holds_in_every_state`](crate::icon_button::ChipPins::ink_pin_holds_in_every_state).
///
/// A single constant, because `EnzSidebarToggleButton::chip_pins` (what the leaf
/// paints from)
/// and `icon_fg` (what tests measure) must not be able to disagree — two
/// literals could, and a policy flip would then be invisible to the tests.
pub(crate) const INK_PIN_HOLDS_IN_EVERY_STATE: bool = true;

/// Builder for a sidebar toggle button. See [`enz_sidebar_toggle_button`].
#[must_use]
pub struct EnzSidebarToggleButton<'a, Message> {
    on_press: Message,
    tooltip: Option<String>,
    tooltip_delay: Option<std::time::Duration>,
    icon: String,
    button_size: f32,
    icon_size: f32,
    icon_color: Option<Color>,
    hover_foreground: Option<Color>,
    background: Option<Color>,
    hover_background: Option<Color>,
    aria_label: String,
    id: Option<iced::advanced::widget::Id>,
    _phantom: std::marker::PhantomData<&'a ()>,
}

/// Construct the button with a required press message. Defaults: 28px button,
/// 16px icon, localized tooltip and label, theme-resolved colours.
pub fn enz_sidebar_toggle_button<Message>(
    on_press: Message,
) -> EnzSidebarToggleButton<'static, Message> {
    let geometry = crate::toolbar_icon::TitlebarAffordanceTokens::DEFAULT;
    EnzSidebarToggleButton {
        on_press,
        tooltip: None,
        tooltip_delay: None,
        icon: DEFAULT_ICON.into(),
        button_size: geometry.effective_control_size(),
        icon_size: geometry.glyph_size,
        icon_color: None,
        hover_foreground: None,
        background: None,
        hover_background: None,
        aria_label: enzui_core::localization::text("enzui-sidebar-toggle-button-toggle-sidebar"),
        id: None,
        _phantom: std::marker::PhantomData,
    }
}

impl<'a, Message: Clone + 'a> EnzSidebarToggleButton<'a, Message> {
    /// Override the app's hover delay for this control; zero starts revealing immediately.
    pub fn tooltip_delay(mut self, delay: std::time::Duration) -> Self {
        self.tooltip_delay = Some(delay);
        self
    }

    /// Override the hover hint, which otherwise follows the accessible name.
    /// Empty text suppresses the hint.
    pub fn tooltip(mut self, t: impl Into<String>) -> Self {
        self.tooltip = Some(t.into());
        self
    }
    /// The icon glyph (Lucide canonical name or semantic alias resolved by
    /// `enzui-icons`). Defaults to `"navigation.sidebar_left"`.
    ///
    /// A sidebar toggle is placed on the left *or* the right and points *open*
    /// or *closed*; a single hard-coded glyph cannot express that, so callers
    /// compute the name (hence `String`, not `&'static str`).
    ///
    /// An empty string falls back to the default rather than rendering an
    /// empty glyph — this icon-only chip is meaningless without an icon (see
    /// `resolved_icon_name`). This deliberately differs from `button.rs`'s
    /// `.icon("")`, which omits the optional glyph from a labelled button.
    pub fn icon(mut self, name: impl Into<String>) -> Self {
        self.icon = name.into();
        self
    }
    /// Square button (hit-target) edge length in px. Non-finite or non-positive
    /// input falls back to 28.
    pub fn button_size(mut self, s: f32) -> Self {
        self.button_size = enzui_core::metrics::FinitePositive::new_or(
            s,
            crate::toolbar_icon::TitlebarAffordanceTokens::DEFAULT.effective_control_size(),
        )
        .get();
        self
    }
    /// Icon glyph size in px. Non-finite or non-positive input falls back to 16.
    pub fn icon_size(mut self, s: f32) -> Self {
        self.icon_size = enzui_core::metrics::FinitePositive::new_or(
            s,
            crate::toolbar_icon::TitlebarAffordanceTokens::DEFAULT.glyph_size,
        )
        .get();
        self
    }
    /// Pin the icon tint in every interaction state, opting out of both
    /// runtime-theme resolution and the rest→hover foreground move. Unset by
    /// default (rest is `text_muted`, hover/press moves to `text`).
    ///
    /// This pin takes precedence over [`Self::hover_foreground`] (a caller who
    /// pins the tint wants that exact colour, not a hover transition) — see
    /// `icon_fg`.
    pub fn icon_color(mut self, c: Color) -> Self {
        self.icon_color = Some(c);
        self
    }
    /// Pin the hover/pressed icon foreground, opting out of the default `text`
    /// hover tint. Unset by default (hover moves the glyph to the palette's
    /// primary `text`). Overridden by an explicit [`Self::icon_color`] pin,
    /// which fixes the tint across all states.
    pub fn hover_foreground(mut self, c: Color) -> Self {
        self.hover_foreground = Some(c);
        self
    }
    /// Pin the rest-state background, opting out of runtime-theme resolution.
    /// Unset by default (follows the live theme).
    pub fn background(mut self, c: Color) -> Self {
        self.background = Some(c);
        self
    }
    /// Pin the hover background, opting out of runtime-theme resolution. Unset
    /// by default (follows the live theme).
    pub fn hover_background(mut self, c: Color) -> Self {
        self.hover_background = Some(c);
        self
    }
    /// Overrides the localized accessible name.
    pub fn aria_label(mut self, l: impl Into<String>) -> Self {
        self.aria_label = l.into();
        self
    }

    /// Assign a stable widget identity, forwarded to the composed
    /// [`crate::focus_button::FocusButton`] so programmatic `focus(id)` and
    /// AccessKit action routing can target this toggle.
    pub fn id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }

    /// The `iced::widget::svg::Handle` this toggle will actually render for
    /// its current `.icon(..)` selection, resolved through the icon pipeline.
    ///
    /// Single source of truth for the glyph: [`Self::into_element`]
    /// builds the icon from *exactly* this handle, so a test asserting the
    /// returned `id()` proves which glyph paints — not the stored `icon`
    /// field. An empty selection falls back to [`DEFAULT_ICON`]; an unknown
    /// name resolves to the fallback glyph (never silently blank).
    pub(crate) fn rendered_icon_handle(&self) -> iced::widget::svg::Handle {
        crate::icon::handle_for(resolved_icon_name(&self.icon))
    }

    /// The chip paint pins this builder hands the shared leaf.
    ///
    /// Single source of truth for the toggle's colour policy: both
    /// `icon_fg` (which tests measure directly) and
    /// [`Self::into_element`] resolve their colours from this value, so a test
    /// on the resolved ink observes what the glyph actually paints.
    ///
    /// `ink_pin_holds_in_every_state` is `true` here, which is this widget's
    /// documented precedence: an explicit [`Self::icon_color`] pin fixes the
    /// tint across every interaction state and outranks
    /// [`Self::hover_foreground`].
    fn chip_pins(&self) -> crate::icon_button::ChipPins {
        crate::icon_button::ChipPins {
            ink_rest: self.icon_color,
            ink_hover: self.hover_foreground,
            fill_rest: self.background,
            fill_hover: self.hover_background,
            ink_pin_holds_in_every_state: INK_PIN_HOLDS_IN_EVERY_STATE,
            hover_emphasis: 1.0,
        }
    }

    /// Consume the builder and produce the widget tree.
    ///
    /// Delegates to the shared round icon-action leaf
    /// (the crate-internal `icon_button` engine) with the
    /// sidebar-disclosure preset: this builder owns the default glyph, tooltip,
    /// accessible name and the
    /// `FinitePositive` geometry guards; the leaf owns the square box,
    /// status-aware paint, activation, semantics, tooltip placement and id.
    pub fn into_element(mut self) -> Element<'a, Message> {
        // Colours resolve from the runtime theme unless the caller pins them.
        // A hard-coded `Palette::dark()` would keep dark-theme chrome in a light
        // theme.
        let pins = self.chip_pins();
        // The localized default supplies a name; an
        // empty one publishes no name rather than a blank one.
        let label = Some(std::mem::take(&mut self.aria_label)).filter(|l| !l.is_empty());
        let icon = crate::icon_button::IconSource::Handle(self.rendered_icon_handle());
        let tooltip = self.tooltip.or_else(|| label.clone());
        crate::icon_button::build(crate::icon_button::IconButtonSpec {
            // Built from `rendered_icon_handle()` — the single source of truth a
            // test asserts by `id()`, so a test cannot pass against a build path
            // that quietly resolves a different glyph (the defect). An
            // empty name falls back to the default (see `resolved_icon_name`).
            icon,
            on_activated: Some(self.on_press),
            // A sidebar disclosure has no disabled state: the region it reveals
            // is always openable, so the preset does not expose one.
            enabled: true,
            style: crate::icon_button::IconButtonStyle::Chip(pins),
            box_size: self.button_size,
            icon_size: self.icon_size,
            // The chip *is* the control: no taller slot.
            slot_height: None,
            fill_inset: 0.0,
            disabled_glyph_opacity: None,
            // This is a disclosure, not a toggle, and the shell's real
            // disclosure state lives on `enz_toolbar_icon(..).expanded(..)`;
            // this preset publishes neither axis.
            active: None,
            expanded: None,
            name: crate::icon_button::IconButtonName {
                label,
                // The localized default supplies a name, so there is no
                // missing-name case to audit.
                audit_missing: false,
            },
            publish_disabled: false,
            // The tooltip is rendered through the canonical enzUI fade
            // tooltip, below the chip (header-bar convention). Empty or
            // whitespace-only text means "no tooltip", enforced centrally
            // by the leaf.
            tooltip_delay: self.tooltip_delay,
            tooltip: tooltip.map(|tip| (tip, iced::widget::tooltip::Position::Bottom)),
            // Forward the caller's stable id so `focus(id)` (and any other
            // Operation) actually reaches it. Stored on the builder alone would
            // be indistinguishable from no id at all.
            id: self.id,
            // This preset triggers no transient focus-owning menu.
            reclaim_focus_on_return: None,
            relinquish_focus_while_open: false,
        })
    }
}

impl<'a, Message: Clone + 'a> From<EnzSidebarToggleButton<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzSidebarToggleButton<'a, Message>) -> Self {
        w.into_element()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

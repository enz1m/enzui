//! Crate-internal engine behind the three icon-button wrappers, which differ
//! only in preset + defaults:
//!
//! | wrapper | preset | context |
//! |---|---|---|
//! | [`crate::toolbar_icon::enz_toolbar_icon`] | [`IconButtonStyle::Toolbar`] | toolbar / titlebar action, toggle (`aria-pressed`) or disclosure (`aria-expanded`) |
//! | [`crate::sidebar_toggle_button::enz_sidebar_toggle_button`] | [`IconButtonStyle::Chip`] | sidebar disclosure chip, default icon + tooltip + name |
//! | [`crate::window_control_button::enz_window_control_button`] | [`IconButtonStyle::Chip`] | window chrome, required action name, cluster-supplied id |
//!
//! Sharing one implementation keeps focus-ring, hover-tint, disabled-contrast
//! and runtime-theme policy from drifting. The two chip ink policies stay
//! distinct because normalising them would repaint one wrapper: see
//! [`ChipPins::ink_pin_holds_in_every_state`].

use crate::focus_button::{FocusStatus, focus_button_maybe};
use crate::toolbar_icon::{ToolbarIconTokens, ToolbarIconVariant};
use enzui_core::semantics::{Role, Semantics, Toggle};
use enzui_theme::Palette;
use iced::widget::button::Style as ButtonStyle;
use iced::widget::container;
use iced::{Alignment, Background, Border, Color, Element, Length, Padding};

// ---------------------------------------------------------------------------
// Paint: one resolved colour per interaction state
// ---------------------------------------------------------------------------

/// The fill + glyph ink for one interaction state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct StatePaint {
    pub fill: Color,
    pub ink: Color,
}

/// Every state's paint, resolved once per frame: the glyph tint resolver and
/// the button style closure read the same value, so the two cannot disagree.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct IconButtonPaint {
    /// Resting (and focused) appearance.
    pub rest: StatePaint,
    pub hover: StatePaint,
    pub pressed: StatePaint,
    /// Toggle-on paint; `None` when the preset has no toggle axis, rather
    /// than an unreachable colour.
    pub active: Option<StatePaint>,
    /// Disabled appearance, reached exactly when no activation message is
    /// carried — the same condition reporting [`FocusStatus::Disabled`] — so
    /// arm and status agree by construction.
    pub disabled: StatePaint,
}

impl IconButtonPaint {
    /// Precedence is disabled → active → status: a disabled toggle paints
    /// disabled, and an active control keeps its fill under the pointer.
    pub(crate) fn state(self, status: FocusStatus, active: bool, enabled: bool) -> StatePaint {
        if !enabled {
            return self.disabled;
        }
        if active && let Some(a) = self.active {
            return a;
        }
        match status {
            FocusStatus::Hovered => self.hover,
            FocusStatus::Pressed => self.pressed,
            _ => self.rest,
        }
    }
}

/// Caller pins for the [`IconButtonStyle::Chip`] preset. `None` means "resolve
/// from the runtime theme", which is the default for every field.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct ChipPins {
    /// Resting glyph ink; defaults to `Palette::text_muted`.
    pub ink_rest: Option<Color>,
    /// Hover / pressed glyph ink; defaults to `Palette::text`.
    pub ink_hover: Option<Color>,
    /// Resting fill; defaults to `Palette::surface_hover`.
    pub fill_rest: Option<Color>,
    /// Hover / pressed fill; defaults to `Palette::surface_selected`.
    pub fill_hover: Option<Color>,
    /// Whether an explicit [`Self::ink_rest`] pin holds in every state.
    ///
    /// The sidebar toggle pins across all states and outranks hover ink;
    /// the window control's pin seeds rest ink only. Normalising either
    /// repaints its wrapper, so the policy is a field.
    pub ink_pin_holds_in_every_state: bool,
    /// How far the unpinned hover fill steps from the resting fill, in units
    /// of the `surface_hover` to `surface_selected` step: `1.0` is that step.
    /// A control inside a shared pill steps further, so its highlight reads
    /// on light palettes, whose step is small.
    pub hover_emphasis: f32,
}

/// The paint policy a preset applies, resolved per frame from the live theme.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum IconButtonStyle {
    /// Toolbar / titlebar chrome: transparent resting box, translucent
    /// interaction overlays, `surface_selected` toggle fill. `pinned`
    /// freezes one palette; `None` resolves per theme.
    Toolbar {
        /// Caller-pinned token set, or `None` for live-theme resolution.
        pinned: Option<ToolbarIconTokens>,
        /// Named theme-reactive visual policy applied after resolution.
        variant: ToolbarIconVariant,
    },
    /// Sidebar-disclosure / window-control chip: opaque resting pill,
    /// `surface_selected` + `text` under the pointer, `Palette::disabled`
    /// glyph when inert. No toggle axis.
    Chip(ChipPins),
}

impl IconButtonStyle {
    pub(crate) fn paint(self, theme: &iced::Theme) -> IconButtonPaint {
        match self {
            Self::Toolbar { pinned, variant } => toolbar_paint(
                &crate::toolbar_icon::resolve_toolbar_icon_tokens(theme, pinned, variant),
            ),
            Self::Chip(pins) => chip_paint(&crate::theme_lookup::palette(theme), &pins),
        }
    }
}

/// The state mapping for a resolved token set; kept here (not in
/// [`crate::toolbar_icon`]) so it exists exactly once.
pub(crate) fn toolbar_paint(t: &ToolbarIconTokens) -> IconButtonPaint {
    IconButtonPaint {
        rest: StatePaint {
            fill: Color::TRANSPARENT,
            ink: t.icon_color,
        },
        hover: StatePaint {
            fill: t.hover_background,
            ink: t.hover_color,
        },
        pressed: StatePaint {
            fill: t.pressed_background,
            ink: t.pressed_color,
        },
        active: Some(StatePaint {
            fill: t.active_background,
            ink: t.active_color,
        }),
        // The glyph opacity is the complete treatment; `disabled_color` is
        // enabled-strength so it applies exactly once.
        disabled: StatePaint {
            fill: Color::TRANSPARENT,
            ink: t.disabled_color,
        },
    }
}

pub(crate) fn chip_paint(p: &Palette, pins: &ChipPins) -> IconButtonPaint {
    // A holding pin wins over hover and disabled ink; a rest-only pin seeds
    // rest ink alone.
    let sticky = pins.ink_rest.filter(|_| pins.ink_pin_holds_in_every_state);
    let rest_ink = pins.ink_rest.unwrap_or(p.text_muted);
    let hover_ink = sticky.or(pins.ink_hover).unwrap_or(p.text);
    let disabled_ink = sticky.unwrap_or(p.disabled);
    let rest_fill = pins.fill_rest.unwrap_or(p.surface_hover);
    let hover_fill = pins.fill_hover.unwrap_or_else(|| {
        let step = |from: f32, to: f32| from + (to - from) * pins.hover_emphasis;
        Color {
            r: step(p.surface_hover.r, p.surface_selected.r).clamp(0.0, 1.0),
            g: step(p.surface_hover.g, p.surface_selected.g).clamp(0.0, 1.0),
            b: step(p.surface_hover.b, p.surface_selected.b).clamp(0.0, 1.0),
            a: p.surface_selected.a,
        }
    });
    IconButtonPaint {
        rest: StatePaint {
            fill: rest_fill,
            ink: rest_ink,
        },
        hover: StatePaint {
            fill: hover_fill,
            ink: hover_ink,
        },
        // The chip preset uses the hover fill for pressed too.
        pressed: StatePaint {
            fill: hover_fill,
            ink: hover_ink,
        },
        // A chip has no toggle axis: `None` rather than an unreachable state.
        active: None,
        disabled: StatePaint {
            fill: rest_fill,
            ink: disabled_ink,
        },
    }
}

/// Paint for one resolved state. `inset_fill` moves the interaction fill
/// onto an inner container, so the button quad paints no fill and the focus
/// ring's contrast is resolved against what it actually overlaps.
pub(crate) fn leaf_style(
    theme: &iced::Theme,
    paint: IconButtonPaint,
    status: FocusStatus,
    active: bool,
    enabled: bool,
    radius: f32,
    inset_fill: bool,
) -> ButtonStyle {
    let state = paint.state(status, active, enabled);
    let bg = if inset_fill {
        Color::TRANSPARENT
    } else {
        state.fill
    };
    let border = if matches!(status, FocusStatus::Focused) {
        // Ring contrast is checked against the fill it is painted over.
        crate::focus_button::icon_action_focus_ring(theme, bg, radius)
    } else {
        Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: radius.into(),
        }
    };
    ButtonStyle {
        background: Some(Background::Color(bg)),
        text_color: state.ink,
        border,
        ..ButtonStyle::default()
    }
}

// ---------------------------------------------------------------------------
// Spec
// ---------------------------------------------------------------------------

/// Where the glyph comes from.
pub(crate) enum IconSource {
    /// A bundled Lucide name or semantic alias, resolved by `enzui-icons`.
    Name(String),
    /// A resolved handle, for a wrapper publishing the handle as its own
    /// source of truth.
    Handle(iced::widget::svg::Handle),
    UntintedSvg(iced::widget::svg::Handle),
    Image(iced::widget::image::Handle),
    /// The self-animating spinner in place of the glyph, for a control whose
    /// work is running.
    Spinner,
}

/// The accessible name, resolved by the wrapper — which owns the missing-name
/// diagnostic, because the message names the wrapper.
pub(crate) struct IconButtonName {
    /// The name, or `None` for no name.
    pub label: Option<String>,
    /// Report omission in accessibility scans.
    pub audit_missing: bool,
}

/// Everything the leaf needs, with presets expressed as values — never as a
/// branch in wrapper paint code.
pub(crate) struct IconButtonSpec<Message> {
    /// The glyph.
    pub icon: IconSource,
    /// The message, or `None` for no handler (distinct from `enabled = false`).
    pub on_activated: Option<Message>,
    /// `false` drops the activation message up front, suppressing pointer,
    /// keyboard and routed-AT activation together.
    pub enabled: bool,
    pub style: IconButtonStyle,
    /// Square box side in px; the corner radius is exactly `box_size / 2`.
    pub box_size: f32,
    /// Glyph size in px.
    pub icon_size: f32,
    /// Taller slot centring the box (e.g. the toolbar 32 px track); `None`
    /// means the box is the control.
    pub slot_height: Option<f32>,
    /// Draws the interaction fill inside the box; box, hit rect and focus
    /// ring stay put.
    pub fill_inset: f32,
    /// Disabled glyph opacity; `None` relies on the disabled ink alone. A
    /// `Some` treatment needs enabled-strength ink (applied exactly once).
    pub disabled_glyph_opacity: Option<f32>,
    /// Toggle state; `None` publishes no toggle state (a push button, not an
    /// off toggle).
    pub active: Option<bool>,
    /// Disclosure state; `None` reveals nothing.
    pub expanded: Option<bool>,
    pub name: IconButtonName,
    /// Pins the build-time disabled descriptor; the focus button reflects the
    /// live state.
    pub publish_disabled: bool,
    /// `None` derives a hint from a verified accessible name; empty text suppresses it.
    pub tooltip: Option<(String, iced::widget::tooltip::Position)>,
    pub tooltip_delay: Option<std::time::Duration>,
    /// Stable focus / test id forwarded to the focus button.
    pub id: Option<iced::advanced::widget::Id>,
    /// Reclaim token for menu-trigger focus return; `None` opts out. See
    /// [`crate::focus_button::FocusButton::reclaim_focus_on_return`].
    pub reclaim_focus_on_return: Option<iced::advanced::widget::Id>,
    /// Yields focus while the triggered menu is open, so its on-mount grab
    /// owns focus; `false` opts out.
    pub relinquish_focus_while_open: bool,
}

pub(crate) fn build<'a, Message: Clone + 'a>(
    spec: IconButtonSpec<Message>,
) -> Element<'a, Message> {
    build_with_activation(spec, None)
}

/// Defers message construction to activation time, so building a menu view
/// never invokes the event mapper. Paint, geometry, semantics and routing
/// stay here.
pub(crate) fn build_lazy<'a, Message: Clone + 'a>(
    mut spec: IconButtonSpec<Message>,
    on_activated: Option<Box<dyn Fn() -> Message + 'a>>,
) -> Element<'a, Message> {
    debug_assert!(
        spec.on_activated.is_none(),
        "a canonical icon leaf must have exactly one activation source"
    );
    spec.on_activated = None;
    build_with_activation(spec, on_activated)
}

fn build_with_activation<'a, Message: Clone + 'a>(
    spec: IconButtonSpec<Message>,
    lazy_activation: Option<Box<dyn Fn() -> Message + 'a>>,
) -> Element<'a, Message> {
    let IconButtonSpec {
        icon,
        on_activated,
        enabled,
        style,
        box_size,
        icon_size,
        slot_height,
        fill_inset,
        disabled_glyph_opacity,
        active,
        expanded,
        name,
        publish_disabled,
        tooltip,
        tooltip_delay,
        id,
        reclaim_focus_on_return,
        relinquish_focus_while_open,
    } = spec;

    let radius = box_size / 2.0;
    // An over-large inset must not invert the inner box.
    let fill_inset = enzui_core::metrics::FiniteNonNeg::new_or(fill_inset, 0.0)
        .get()
        .clamp(0.0, box_size / 2.0);
    let inset_fill = fill_inset > 0.0;
    let toggled_on = active.unwrap_or(false);

    // Shared status cell: the style closure writes each frame and the glyph
    // tint reads it, so ink and fill resolve the same state (never the
    // container-hover split, where a theme-only glyph cannot show hover).
    let status_cell = std::rc::Rc::new(std::cell::Cell::new(if enabled {
        FocusStatus::Active
    } else {
        FocusStatus::Disabled
    }));

    let sized_svg = |glyph: crate::icon::EnzIcon<'a>| {
        glyph.size(icon_size).opacity(if enabled {
            1.0
        } else {
            disabled_glyph_opacity.unwrap_or(1.0)
        })
    };
    let tinted = |glyph: crate::icon::EnzIcon<'a>| -> Element<'a, Message> {
        sized_svg(glyph)
            .tint_with_status(
                std::rc::Rc::clone(&status_cell),
                move |theme: &iced::Theme, status| {
                    style.paint(theme).state(status, toggled_on, enabled).ink
                },
            )
            .into()
    };
    let glyph: Element<'a, Message> = match icon {
        IconSource::Name(name) => tinted(crate::icon::enz_icon(&name)),
        IconSource::Handle(handle) => tinted(crate::icon::enz_icon_from_handle(handle)),
        IconSource::UntintedSvg(handle) => {
            sized_svg(crate::icon::enz_icon_from_handle(handle).no_tint()).into()
        }
        IconSource::Image(handle) => crate::icon::enz_icon_from_image(
            handle,
            icon_size,
            if enabled {
                1.0
            } else {
                disabled_glyph_opacity.unwrap_or(1.0)
            },
        ),
        IconSource::Spinner => crate::spinner::enz_spinner().size(icon_size).into(),
    };

    // At a nonzero inset the fill moves to an inner container; box, hit rect
    // and ring stay put.
    let glyph: Element<'a, Message> = if inset_fill {
        let inner_size = box_size - 2.0 * fill_inset;
        let inner_radius = inner_size / 2.0;
        let fill_cell = std::rc::Rc::clone(&status_cell);
        container(glyph)
            .width(Length::Fixed(inner_size))
            .height(Length::Fixed(inner_size))
            .center_x(Length::Fixed(inner_size))
            .center_y(Length::Fixed(inner_size))
            .style(move |theme: &iced::Theme| container::Style {
                background: Some(Background::Color(
                    style
                        .paint(theme)
                        .state(fill_cell.get(), toggled_on, enabled)
                        .fill,
                )),
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: inner_radius.into(),
                },
                ..container::Style::default()
            })
            .into()
    } else {
        glyph
    };

    let box_el = container(glyph)
        .width(Length::Fixed(box_size))
        .height(Length::Fixed(box_size))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center);

    // One `FocusButton` per preset: every leaf Tab-focusable with one
    // `Role::Button` node.
    let mut semantics = Semantics::new(Role::Button).default_action();
    let tooltip = tooltip.or_else(|| {
        if name.audit_missing {
            return None;
        }
        name.label
            .clone()
            .map(|label| (label, iced::widget::tooltip::Position::Top))
    });
    if let Some(label) = name.label {
        semantics = semantics.label(label);
    }
    if publish_disabled {
        semantics = semantics.disabled(!enabled);
    }
    // `None` publishes no toggle state; `Some(false)` is as load-bearing as
    // `Some(true)`.
    if let Some(on) = active {
        semantics = semantics.toggled(if on { Toggle::True } else { Toggle::False });
    }
    // Only an identified node can route the action, so an anonymous disclosure
    // announces state without advertising a dead action.
    if let Some(on) = expanded {
        semantics = if id.is_some() {
            semantics.expanded(on)
        } else {
            semantics.expanded_state_only(on)
        };
    }

    // Dropping the message is the single switch suppressing pointer,
    // keyboard and routed-AT activation at once.
    let on_activated = enabled.then_some(on_activated).flatten();
    let lazy_activation = enabled.then_some(lazy_activation).flatten();
    let mut button = if lazy_activation.is_some() {
        crate::focus_button::focus_button_maybe_with(box_el, lazy_activation)
    } else {
        focus_button_maybe(box_el, on_activated)
    }
    .padding(Padding::ZERO)
    .relinquish_focus_while(relinquish_focus_while_open)
    .reclaim_focus_on_return(reclaim_focus_on_return)
    .semantics(semantics);
    if let Some(id) = id {
        // Routes advertised Expand/Collapse to the click handler; without
        // the alias the action would be advertised but dead.
        if expanded.is_some() {
            button = button.expand_alias(id.clone());
        }
        button = button.id(id);
    }
    let button = button.style_focus(move |theme: &iced::Theme, status: FocusStatus| {
        status_cell.set(status);
        leaf_style(
            theme,
            style.paint(theme),
            status,
            toggled_on,
            enabled,
            radius,
            // Build-time value: a builder-overridden inset leaves `0.0` in the
            // resolved set, and re-reading it would paint the fill twice.
            inset_fill,
        )
    });

    // Fixed height (not `Fill`) keeps the hit rect deterministic inside a
    // `Shrink` parent.
    let control: Element<'a, Message> = match slot_height {
        Some(slot) => container(button)
            .width(Length::Fixed(box_size))
            .height(Length::Fixed(slot))
            .center_y(Length::Fixed(slot))
            .into(),
        None => button.into(),
    };

    // A meaningless tip is no tooltip, not an empty bubble.
    let element = match tooltip {
        Some((tip, position)) if crate::tooltip::tooltip_is_meaningful(&tip) => {
            crate::tooltip::enz_tooltip_with_delay(
                control,
                tip,
                position,
                tooltip_delay.unwrap_or_else(crate::tooltip::default_tooltip_delay),
            )
        }
        _ => control,
    };

    // An unnamed icon-only control is an audit finding, not a silent omission.
    if name.audit_missing {
        crate::accessibility_scan_widgets::MissingNameScanProbe::new(element).into()
    } else {
        element
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

//! EnzTreeDisclosure: expand/collapse chevron for tree rows.
//!
//! Shows a right-pointing chevron when collapsed and a down-pointing
//! chevron when expanded; the whole slot is a hit target that fires
//! `on_toggle`.
//!
//! Composes `enz_icon_slot` inside the shared
//! [`FocusButton`](crate::focus_button) primitive with a transparent
//! background — the slot is invisible chrome, only the chevron glyph shows.
//!
//! # Framework consumers
//!
//! The one framework consumer of this chevron is [`crate::tree_view`], which
//! builds it into every branch row automatically — reach for the chevron
//! directly only when laying out a row the view cannot host (a custom
//! disclosure column, an outline in another surface). A reader arriving at
//! `tree_disclosure` learns first that its home is the tree view.
//!
//! # Accessibility
//!
//! Routes through [`FocusButton`](crate::focus_button) with a
//! `Role::Disclosure` node: publishes `.expanded(state)`, consumes the
//! `aria_label` (collapsed/expanded) as the accessible name, and
//! `.disabled(!enabled)`. Gains Tab focus + Enter/Space activation and a
//! keyboard-only focus ring.

use crate::focus_button::FocusStatus;
use crate::icon_slot::enz_icon_slot;
use enzui_core::semantics::{Role, Semantics};
use iced::widget::button::Style as ButtonStyle;
use iced::{Background, Border, Color, Element, Length};

/// Builder for a tree disclosure chevron (expand/collapse control). See
/// [`enz_tree_disclosure`] and [`enz_tree_disclosure_lazy`].
#[must_use]
pub struct EnzTreeDisclosure<'a, Message> {
    expanded: bool,
    /// Lazy activation. Built from the constructor's eager `Message` so the
    /// public signature is unchanged, but stored as a factory so a composite
    /// can supply a per-row closure without pre-computing N messages.
    on_toggle: Option<Box<dyn Fn() -> Message + 'a>>,
    button_size: f32,
    row_height: f32,
    icon_size: f32,
    icon_color: Option<Color>,
    enabled: bool,
    aria_label_collapsed: String,
    aria_label_expanded: String,
    collapsed_icon: String,
    expanded_icon: String,
    /// Writing direction override; `None` follows the locale.
    direction: Option<enzui_core::placement::Direction>,
    embedded: bool,
    /// The owning row's id, under which this chevron answers a routed
    /// expand/collapse. See [`EnzTreeDisclosure::expand_alias`].
    expand_alias: Option<iced::advanced::widget::Id>,
    /// Caller-supplied stable identity for a standalone chevron.
    /// See [`EnzTreeDisclosure::id`].
    id: Option<iced::advanced::widget::Id>,
    _lifetime: std::marker::PhantomData<&'a ()>,
}

/// Construct a disclosure chevron whose activation emits `on_toggle`.
/// `expanded` selects the down (expanded) vs forward (collapsed) glyph.
/// Defaults: 20px button, 36px row height, 10px icon, enabled, standalone
/// (non-embedded).
pub fn enz_tree_disclosure<'a, Message: Clone + 'a>(
    expanded: bool,
    on_toggle: Message,
) -> EnzTreeDisclosure<'a, Message> {
    EnzTreeDisclosure {
        expanded,
        on_toggle: Some(Box::new(move || on_toggle.clone())),
        button_size: 20.0, // icon-md
        row_height: 36.0,  // list-row-height
        icon_size: 10.0,   // icon-xxs
        icon_color: None,
        enabled: true,
        aria_label_collapsed: enzui_core::localization::text("enzui-tree-disclosure-expand"),
        aria_label_expanded: enzui_core::localization::text("enzui-tree-disclosure-collapse"),
        collapsed_icon: "navigation.forward".into(),
        expanded_icon: "navigation.down".into(),
        direction: None,
        embedded: false,
        expand_alias: None,
        id: None,
        _lifetime: std::marker::PhantomData,
    }
}

/// A disclosure with no eager toggle message, for a composite that supplies
/// one lazily per row.
///
/// [`enz_tree_disclosure`] takes a `Message` by value, which a tree of N rows
/// cannot satisfy without building N messages every frame. Pair this
/// with [`EnzTreeDisclosure::on_toggle_maybe_with`].
pub fn enz_tree_disclosure_lazy<'a, Message: Clone + 'a>(
    expanded: bool,
) -> EnzTreeDisclosure<'a, Message> {
    EnzTreeDisclosure {
        expanded,
        on_toggle: None,
        button_size: 20.0,
        row_height: 36.0,
        icon_size: 10.0,
        icon_color: None,
        enabled: true,
        aria_label_collapsed: enzui_core::localization::text("enzui-tree-disclosure-expand"),
        aria_label_expanded: enzui_core::localization::text("enzui-tree-disclosure-collapse"),
        collapsed_icon: "navigation.forward".into(),
        expanded_icon: "navigation.down".into(),
        direction: None,
        embedded: false,
        expand_alias: None,
        id: None,
        _lifetime: std::marker::PhantomData,
    }
}

impl<'a, Message: Clone + 'a> EnzTreeDisclosure<'a, Message> {
    /// Square button (hit-target) edge length in px (default 20).
    pub fn button_size(mut self, s: f32) -> Self {
        self.button_size = s;
        self
    }
    /// Height the chevron aligns to within its row, in px (default 36).
    pub fn row_height(mut self, h: f32) -> Self {
        self.row_height = h;
        self
    }
    /// Chevron glyph size in px (default 10).
    pub fn icon_size(mut self, s: f32) -> Self {
        self.icon_size = s;
        self
    }
    /// Override the chevron tint. Unset by default (resolved from the theme).
    pub fn icon_color(mut self, c: Color) -> Self {
        self.icon_color = Some(c);
        self
    }
    /// Enable or disable the chevron. Defaults to `true`.
    pub fn enabled(mut self, e: bool) -> Self {
        self.enabled = e;
        self
    }

    /// Replace the activation with a lazy factory, run once per real
    /// activation rather than pre-computed per row. `None` makes the
    /// chevron inert without dimming it.
    pub fn on_toggle_maybe_with(mut self, f: Option<Box<dyn Fn() -> Message + 'a>>) -> Self {
        self.on_toggle = f;
        self
    }
    /// Set the accessible names for the collapsed and expanded states
    /// (defaults `"Expand"` / `"Collapse"`); the current one is announced by
    /// state.
    pub fn aria_label(mut self, collapsed: impl Into<String>, expanded: impl Into<String>) -> Self {
        self.aria_label_collapsed = collapsed.into();
        self.aria_label_expanded = expanded.into();
        self
    }
    /// Glyph name shown when collapsed (default `"navigation.forward"`).
    pub fn collapsed_icon(mut self, name: impl Into<String>) -> Self {
        self.collapsed_icon = name.into();
        self
    }
    /// Glyph name shown when expanded (default `"navigation.down"`).
    pub fn expanded_icon(mut self, name: impl Into<String>) -> Self {
        self.expanded_icon = name.into();
        self
    }

    /// Embedded mode: this chevron sits inside a roving-focus
    /// composite (a tree row) that owns the single Tab stop.
    ///
    /// A standalone disclosure is its own Tab stop and its own Enter/Space
    /// target, which is right for an expander header. Inside a tree it is
    /// wrong twice over: a nine-row tree would offer nine extra Tab stops, and
    /// Enter on the focused row would be ambiguous between "activate the row"
    /// and "toggle the chevron". Embedding hands focus and the keyboard to the
    /// owner while keeping the chevron a live pointer target with its own
    /// hover/pressed feedback.
    ///
    /// It also stops publishing its own `Role::Disclosure` node: the owning
    /// row already advertises `expanded` plus the expand/collapse action, and
    /// a second expandable node over the same state announces the row twice.
    ///
    /// See [`crate::focus_button::FocusButton::embedded`].
    pub fn embedded(mut self, embedded: bool) -> Self {
        self.embedded = embedded;
        self
    }

    /// Answer a routed expand/collapse aimed at `alias` — the owning row's
    /// id.
    ///
    /// A tree row publishes one `Role::TreeItem` node carrying the row's id and
    /// its `expanded` state, so that id is the only handle an AT has for both
    /// "select this row" and "expand this row". They are different handlers:
    /// selection is the row surface's `on_activate`, the toggle is this
    /// chevron's `on_toggle`.
    ///
    /// Setting this makes the chevron answer only
    /// [`activation::expand`](crate::activation::expand) for `alias`, never
    /// [`activation::activate`](crate::activation::activate) — so the row keeps
    /// selection and the chevron gets the toggle. See
    /// [`FocusButton::expand_alias`](crate::focus_button::FocusButton::expand_alias)
    /// for why this is an alias rather than an `.id()`.
    pub fn expand_alias(mut self, alias: iced::advanced::widget::Id) -> Self {
        self.expand_alias = Some(alias);
        self
    }

    /// Stable identity for a standalone chevron, so an AT `Focus`/`Click`
    /// and any `Operation` can name it.
    ///
    /// # Identity is caller-supplied
    ///
    /// Identity can instead be derived from a stable logical key where
    /// one exists (`breadcrumb` from the crumb, `button_group` from
    /// `item.id`). A bare chevron has none. Its only candidate keys are the
    /// `aria_label` pair — which defaults to the literal `"Expand"`/
    /// `"Collapse"` on *every* disclosure in the product — and its position,
    /// which the identity rule forbids outright: an id that shifts on reorder is
    /// worse than no id, because focus then follows a slot rather than a
    /// thing. Deriving from the label would give one `Id` to every chevron on
    /// screen, and duplicate ids are the same defect one step along: release
    /// collection keeps the first and drops the rest from the published
    /// tree, so the extra controls become unreachable while an id-count still
    /// looks healthy.
    ///
    /// So identity here is caller-supplied: *intrinsically unique keys within
    /// stable owner namespaces;
    /// otherwise caller-supplied stable identity.* The owner knows which row
    /// this chevron belongs to; the chevron does not.
    ///
    /// # Distinct from [`expand_alias`](Self::expand_alias)
    ///
    /// They answer different questions and can coexist. An `id` is *this*
    /// control's own name, used for focus and for a Click aimed at the
    /// chevron. An `expand_alias` makes the chevron answer an Expand/Collapse
    /// aimed at the owning row's id, so the row keeps selection while the
    /// chevron takes the toggle.
    ///
    /// # Embedded chevrons
    ///
    /// An [`embedded`](Self::embedded) chevron publishes no semantics node of
    /// its own — the owning row publishes the expandable node — so setting an
    /// id there names a control that is deliberately absent from the AT tree.
    /// Use [`expand_alias`](Self::expand_alias) for that shape instead.
    pub fn id(mut self, id: impl Into<iced::advanced::widget::Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Override the writing direction. By default the collapsed chevron
    /// follows the locale, pointing left in a right-to-left one.
    pub fn direction(mut self, direction: enzui_core::placement::Direction) -> Self {
        self.direction = Some(direction);
        self
    }

    /// The glyph name for the current state (expanded vs collapsed).
    pub fn current_icon(&self) -> &str {
        if self.expanded {
            &self.expanded_icon
        } else {
            &self.collapsed_icon
        }
    }

    /// The accessible label for the current state (expanded vs collapsed).
    pub fn current_aria_label(&self) -> &str {
        if self.expanded {
            &self.aria_label_expanded
        } else {
            &self.aria_label_collapsed
        }
    }

    /// Consume the builder and produce the widget tree.
    pub fn into_element(self) -> Element<'a, Message> {
        let expanded = self.expanded;
        let enabled = self.enabled;
        let a11y_label = self.current_aria_label().to_owned();
        let direction = enzui_core::placement::resolve_direction(self.direction);
        // The stock chevron turns a quarter to point down as the node opens,
        // like a disclosure's; custom glyphs swap.
        let turns =
            self.collapsed_icon == "navigation.forward" && self.expanded_icon == "navigation.down";
        let slot = enz_icon_slot(crate::icon::icon_name_for_direction(
            self.current_icon(),
            direction,
        ))
        .icon_size(self.icon_size)
        .slot_width(self.button_size)
        .slot_height(self.row_height);
        // Only pin a tint when the caller set one. Left unset, `EnzIconSlot`
        // keeps `enz_icon`'s default `ThemeMuted` tone, which resolves
        // `text_muted` from the *runtime* theme every render. Passing
        // `Palette::dark().text_muted` here would defeat that and freeze the
        // chevron to the dark palette in light themes.
        //
        // When disabled and no explicit colour is set, use
        // `IconTone::Disabled` so the chevron visually ghosts to
        // `palette.disabled`. This stays theme-reactive (resolved every
        // render) and does not pin a colour.
        let slot: Element<'a, Message> = if turns {
            let icon = crate::icon::enz_icon_for_direction("navigation.forward", direction)
                .size(self.icon_size);
            let icon = match (self.icon_color, enabled) {
                (Some(c), _) => icon.tint(c),
                (None, false) => icon.tint_theme_tone(crate::icon::IconTone::Disabled),
                (None, true) => icon,
            };
            iced::widget::container(icon.quarter_turn_in(expanded, direction))
                .center_x(Length::Fixed(self.button_size))
                .center_y(Length::Fixed(self.row_height))
                .into()
        } else {
            match (self.icon_color, enabled) {
                (Some(c), _) => slot.icon_color(c),
                (None, false) => slot.icon_tone(crate::icon::IconTone::Disabled),
                (None, true) => slot,
            }
            .into_element()
        };

        let embedded = self.embedded;
        let semantics = Semantics::new(Role::Disclosure)
            .default_action()
            .label(a11y_label)
            .expanded(expanded)
            .disabled(!enabled);

        let mut button =
            crate::focus_button::focus_button_maybe_with(slot, self.on_toggle.filter(|_| enabled))
                .padding(0)
                .embedded(embedded);
        // Embedded: the owning row publishes the expandable node, so a second
        // one here would announce the same state twice.
        if !embedded {
            button = button.semantics(semantics);
        }
        // Answer a routed expand/collapse aimed at the owning row's id.
        // Not an `.id()`: the row already answers that id for activation, and
        // two widgets answering one id for the *same* operation would be a
        // coin-flip between selecting and toggling.
        if let Some(alias) = self.expand_alias {
            button = button.expand_alias(alias);
        }
        // Forward the caller-supplied id into the composed button, which
        // is what actually publishes the node. An id stored on the builder and
        // dropped here is indistinguishable from no id at all -- and worse,
        // because the API then advertises routing it does not do.
        //
        // Embedded is excluded on purpose: `embedded` above skips
        // `.semantics(..)`, so an embedded chevron publishes no node for an id
        // to land on, and naming it would put an `Id` in `NodeIdMap` pointing
        // at a node an AT can never see.
        if !embedded && let Some(id) = self.id {
            button = button.id(id);
        }
        button
            .style_focus(move |theme: &iced::Theme, status: FocusStatus| {
                let border = if matches!(status, FocusStatus::Focused) {
                    Border {
                        radius: 0.0.into(),
                        width: 1.5,
                        color: crate::theme_lookup::palette(theme).focus_ring,
                    }
                } else {
                    Border {
                        radius: 0.0.into(),
                        width: 0.0,
                        color: Color::TRANSPARENT,
                    }
                };
                ButtonStyle {
                    background: None::<Background>,
                    text_color: Color::TRANSPARENT,
                    border,
                    ..ButtonStyle::default()
                }
            })
            .into()
    }
}

impl<'a, Message: Clone + 'a> From<EnzTreeDisclosure<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzTreeDisclosure<'a, Message>) -> Self {
        w.into_element()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

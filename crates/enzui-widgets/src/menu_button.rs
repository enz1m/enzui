//! Controlled menu-only and split-action buttons with anchored popups.
//!
//! Applications own [`MenuModel`] and [`MenuState`] and apply emitted events
//! with [`apply_event`]. Split primaries act independently of the menu segment.
//! Disabled triggers and models without focusable entries show no popup.
//!
//! [`MenuButtonKind`] selects trigger styling; [`EnzMenuButton::menu_id`] and
//! [`focus_restore_operation`] support application-driven focus restoration.
//! Outside presses dismiss even after a sibling captures them; presses first
//! handled by the menu are consumed.
//!
//! Submenus: a [`MenuEntryModel::submenu`](enzui_core::menu_model::MenuEntryModel::submenu)
//! row whose entries the model holds ([`MenuModel::with_submenu`]) opens them
//! beside itself on a click, <kbd>Enter</kbd> or <kbd>Right</kbd>, its first
//! row highlighted. While it is open the
//! arrows, <kbd>Enter</kbd> and the pointer act on it; <kbd>Left</kbd> or
//! <kbd>Escape</kbd> close only the submenu, and choosing one of its commands
//! closes the whole menu. One level opens: a submenu's own submenus do not.

use crate::anchored_overlay::anchored_overlay;
use crate::focus_button::{FocusStatus, focus_button_maybe, focus_button_maybe_with};
use crate::menu::{MenuTokens, enz_menu};
use crate::menu_row::{MenuRowTokens, MenuRowTone, enz_menu_row_maybe_with, enz_menu_separator};
use enzui_core::context_menu::MenuMove;
use enzui_core::dismissal::{self, DismissPolicy, DismissRegions};
use enzui_core::menu_model::{
    Activation, CheckState, CloseCause, EntryId, EntryKind, MenuKey, MenuModel, MenuState,
    OpenCause, Segment, key_for_open_menu, menu_semantics, open_cause_for_key, trigger_semantics,
};
use enzui_core::metrics::{FiniteNonNeg, FinitePositive};
use enzui_core::placement::{Alignment, Direction, Placement, Side, resolve_direction};
use enzui_theme::Palette;
use iced::widget::button::Style as ButtonStyle;
use iced::widget::{Space, container, row, text};
use iced::{Background, Border, Color, Element, Padding};

/// Corner radius of the trigger, matching `EnzButton` (`button-radius`).
pub const TRIGGER_RADIUS: f32 = 6.0;
/// Default horizontal padding in logical pixels; Quiet uses its size tier instead.
pub const TRIGGER_PADDING_X: f32 = 14.0;
/// Chevron glyph size.
pub const CHEVRON_ICON_SIZE: f32 = 10.0;
/// Gap between the trigger and the popup. Placement accounts for the
/// trigger height, so this value is the gap on its own.
pub const POPUP_GAP: f32 = 4.0;

/// Default empty-label trigger icon, overridable with [`EnzMenuButton::trigger_icon`].
pub const ICON_ONLY_TRIGGER_ICON: &str = "ellipsis-vertical";

/// Empty-label trigger glyph size in logical pixels; Toolbar uses its own icon sizing.
pub const ICON_ONLY_TRIGGER_SIZE: f32 = 16.0;

/// Events for the application to apply to its owned model and state.
/// Activation identifies entries by stable id rather than list position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuButtonEvent {
    /// The primary (non-menu) segment fired. Split mode only.
    PrimaryActivated,
    /// The menu should open, highlighting per the cause.
    Open(OpenCause),
    /// The menu should close for this reason.
    Close(CloseCause),
    /// The highlight should move.
    Navigate(MenuMove),
    /// The highlight should jump to this row (hover).
    Highlight(EntryId),
    /// A row requested activation, with its proposed check state.
    Activated {
        /// Which row.
        id: EntryId,
        /// Proposed post-toggle state; `None` for a command. The application applies it.
        check: CheckState,
    },
    /// A submenu row was clicked or hovered: its submenu opens with no row
    /// highlighted, as a pointer leaves it.
    SubmenuOpened(EntryId),
    /// The keyboard entered a submenu (Right, or Enter on its row): it
    /// opens with its first row highlighted.
    SubmenuEntered(EntryId),
    /// The open submenu should close, leaving its menu open.
    SubmenuClosed,
}

/// Trigger presentation and, for size-derived kinds, geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MenuButtonKind {
    /// Filled trigger (the default). `background` is `palette.button_primary_bg`.
    #[default]
    Default,
    /// Transparent in every status; label and glyph use palette text ink.
    Flat,
    /// Use the given button size and Secondary button colours.
    ActionBar(crate::button::ButtonSize),
    /// Transparent at rest with shared Quiet hover/pressed washes and size-tier geometry.
    Quiet(crate::button::ButtonSize),
    /// Use the shared toolbar-icon trigger, bypassing menu-button colours and geometry.
    /// The popup and controlled event handling remain menu-button owned.
    Toolbar,
}
fn resolve_tokens(
    pinned: Option<MenuButtonTokens>,
    kind: MenuButtonKind,
    theme: &iced::Theme,
) -> MenuButtonTokens {
    pinned.unwrap_or_else(|| {
        let palette = crate::theme_lookup::palette(theme);
        MenuButtonTokens::from_palette_kind(&palette, kind)
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TriggerGeometry {
    pub height: f32,
    pub radius: f32,
    pub label_size: f32,
    pub padding_x: f32,
}

pub(crate) fn resolve_geometry(
    kind: MenuButtonKind,
    height: f32,
    radius: f32,
    label_size: f32,
) -> TriggerGeometry {
    match kind {
        MenuButtonKind::ActionBar(size) => {
            let st = size.tokens();
            TriggerGeometry {
                height: st.height,
                radius: st.radius,
                label_size: st.text_size,
                padding_x: TRIGGER_PADDING_X,
            }
        }
        MenuButtonKind::Quiet(size) => {
            let st = size.tokens();
            TriggerGeometry {
                height: st.height,
                radius: st.radius,
                label_size: st.text_size,
                padding_x: st.padding_x,
            }
        }
        _ => TriggerGeometry {
            height,
            radius,
            label_size,
            padding_x: TRIGGER_PADDING_X,
        },
    }
}

/// Trigger colour overrides; unpinned colours follow the live palette.
#[derive(Debug, Clone, Copy)]
pub struct MenuButtonTokens {
    /// Trigger fill.
    pub background: Color,
    /// Trigger fill while hovered.
    pub hover_background: Color,
    /// Trigger fill while pressed; Quiet defaults to the stronger pressed wash.
    pub pressed_background: Color,
    /// Trigger fill when inert.
    pub disabled_background: Color,
    /// Label / chevron colour.
    pub foreground: Color,
    /// Label / chevron colour when inert.
    pub disabled_foreground: Color,
    /// Hairline between the split segments.
    pub divider: Color,
    /// Focus ring.
    pub focus_ring: Color,
}
fn disabled_foreground(
    p: &Palette,
    tokens: &MenuButtonTokens,
    parent_backdrop: Option<Color>,
) -> Color {
    let backdrop = if tokens.disabled_background.a >= 0.999 {
        Some(tokens.disabled_background)
    } else {
        parent_backdrop
            .map(|parent| crate::surface_interaction::composite(parent, tokens.disabled_background))
    };
    backdrop.map_or(tokens.disabled_foreground, |backdrop| {
        p.disabled_on_with_preferred(tokens.disabled_foreground, backdrop)
    })
}

impl MenuButtonTokens {
    /// Resolve from a runtime palette.
    #[must_use]
    pub fn from_palette(palette: &Palette) -> Self {
        Self {
            background: palette.button_primary_bg,
            hover_background: palette.button_primary_hover_bg,
            pressed_background: palette.button_primary_hover_bg,
            disabled_background: palette.surface_alt,
            foreground: palette.text,
            disabled_foreground: palette.disabled,
            divider: palette.border,
            focus_ring: palette.focus_ring,
        }
    }

    /// Resolve the selected kind from a runtime palette.
    /// Flat clears all four fills. Toolbar returns Default tokens, although its
    /// trigger does not consume these tokens.
    #[must_use]
    pub fn from_palette_kind(palette: &Palette, kind: MenuButtonKind) -> Self {
        match kind {
            MenuButtonKind::Default => Self::from_palette(palette),
            MenuButtonKind::Flat => Self {
                background: Color::TRANSPARENT,
                hover_background: Color::TRANSPARENT,
                pressed_background: Color::TRANSPARENT,
                disabled_background: Color::TRANSPARENT,
                ..Self::from_palette(palette)
            },
            MenuButtonKind::ActionBar(_size) => {
                let bt = crate::tokens::ButtonTokens::from_palette(palette);
                Self {
                    background: bt.secondary_bg,
                    hover_background: bt.secondary_hover_bg,
                    pressed_background: bt.secondary_hover_bg,
                    disabled_background: bt.disabled_bg,
                    foreground: bt.secondary_fg,
                    disabled_foreground: bt.disabled_fg,
                    divider: palette.border,
                    focus_ring: palette.focus_ring,
                }
            }
            MenuButtonKind::Quiet(_size) => {
                let bt = crate::tokens::ButtonTokens::from_palette(palette);
                Self {
                    background: Color::TRANSPARENT,
                    hover_background: bt.ghost_hover_bg,
                    pressed_background: bt.ghost_pressed_bg,
                    disabled_background: Color::TRANSPARENT,
                    foreground: bt.secondary_fg,
                    disabled_foreground: bt.disabled_fg,
                    divider: palette.border,
                    focus_ring: palette.focus_ring,
                }
            }
            MenuButtonKind::Toolbar => Self::from_palette(palette),
        }
    }

    /// Dark-theme tokens.
    #[must_use]
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }

    /// Light-theme tokens.
    #[must_use]
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }
}

/// Segment paint availability, accounting for enabled state and focusable menu rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisualLiveness {
    /// Whether the primary segment is painted as live.
    pub primary: bool,
    /// Whether the menu segment (chevron, or the whole menu-only surface) is.
    pub menu: bool,
}

/// A controlled menu button, optionally split.
#[must_use]
pub struct EnzMenuButton<'a, Message> {
    menu_material: Option<enzui_theme::MaterialStyle>,
    label: String,
    model: &'a MenuModel,
    state: &'a MenuState,
    on_event: std::rc::Rc<dyn Fn(MenuButtonEvent) -> Message + 'a>,
    on_activated: Option<Message>,
    split: bool,
    split_divider: bool,
    enabled: bool,
    height: f32,
    radius: f32,
    /// Whether `.radius(..)` pinned the corners; otherwise they take the
    /// theme's `shape.button` step, as buttons do.
    radius_pinned: bool,
    arrow_width: f32,
    label_size: f32,
    popup_width: Option<f32>,
    side: Side,
    alignment: Alignment,
    direction: Option<Direction>,
    tokens: Option<MenuButtonTokens>,
    menu_id: Option<iced::advanced::widget::Id>,
    primary_id: Option<iced::advanced::widget::Id>,
    trigger_icon: Option<String>,
    kind: MenuButtonKind,
    disabled_backdrop: Option<Color>,
    aria_label: Option<String>,
    tooltip: Option<String>,
    tooltip_delay: Option<std::time::Duration>,
}

/// Build a menu-only button: the whole surface opens the menu.
pub fn enz_menu_button<'a, Message: Clone + 'a>(
    label: impl Into<String>,
    model: &'a MenuModel,
    state: &'a MenuState,
    on_event: impl Fn(MenuButtonEvent) -> Message + 'a,
) -> EnzMenuButton<'a, Message> {
    EnzMenuButton {
        label: label.into(),
        model,
        state,
        on_event: std::rc::Rc::new(on_event),
        on_activated: None,
        split: false,
        split_divider: false,
        enabled: true,
        height: 30.0,
        radius: TRIGGER_RADIUS,
        radius_pinned: false,
        arrow_width: 26.0,
        label_size: enzui_theme::text_scale::ui_px(13.0),
        popup_width: None,
        side: Side::Bottom,
        alignment: Alignment::Start,
        direction: None,
        tokens: None,
        menu_id: None,
        primary_id: None,
        trigger_icon: None,
        kind: MenuButtonKind::Default,
        disabled_backdrop: None,
        aria_label: None,
        tooltip: None,
        tooltip_delay: None,
        menu_material: None,
    }
}

/// Build a split button: a primary action plus a menu chevron.
///
/// The two segments activate independently — `on_activated` never opens the
/// menu, and opening the menu never dispatches `on_activated`.
pub fn enz_split_menu_button<'a, Message: Clone + 'a>(
    label: impl Into<String>,
    model: &'a MenuModel,
    state: &'a MenuState,
    on_activated: Message,
    on_event: impl Fn(MenuButtonEvent) -> Message + 'a,
) -> EnzMenuButton<'a, Message> {
    EnzMenuButton {
        on_activated: Some(on_activated),
        split: true,
        ..enz_menu_button(label, model, state, on_event)
    }
}

/// Default accessible name shared with titlebar overflow triggers.
pub const KEBAB_TRIGGER_LABEL: &str = crate::action_bar::TRIGGER_LABEL;

/// Build a controlled kebab menu using the shared toolbar-icon trigger.
/// The interactive control is 28 px square in a 32 px tall slot. Its default
/// icon is vertical ellipsis and its name is [`KEBAB_TRIGGER_LABEL`].
/// Name, icon and menu id remain overridable.
pub fn enz_kebab_menu_button<'a, Message: Clone + 'a>(
    model: &'a MenuModel,
    state: &'a MenuState,
    on_event: impl Fn(MenuButtonEvent) -> Message + 'a,
) -> EnzMenuButton<'a, Message> {
    enz_menu_button("", model, state, on_event)
        .kind(MenuButtonKind::Toolbar)
        .aria_label(enzui_core::localization::text("enzui-actions-more"))
}
type EmittedRow = (EntryId, String, Option<String>);
type BuiltPopup<'a, Message> = (Element<'a, Message>, Vec<EmittedRow>);

impl<'a, Message: Clone + 'a> EnzMenuButton<'a, Message> {
    /// Show the internal rule between split-trigger segments. Off by default.
    pub fn split_divider(mut self, visible: bool) -> Self {
        self.split_divider = visible;
        self
    }

    /// Enable or disable the whole control.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Override the app's hover delay for this control; zero starts revealing immediately.
    pub fn tooltip_delay(mut self, delay: std::time::Duration) -> Self {
        self.tooltip_delay = Some(delay);
        self
    }

    /// Override the trigger's hover hint. Empty text suppresses it without changing its name.
    pub fn tooltip(mut self, tip: impl Into<String>) -> Self {
        self.tooltip = Some(tip.into());
        self
    }

    /// Override the trigger name.
    /// A nonblank name gives non-toolbar triggers ComboBox semantics with the
    /// visible label as value. Blank names use the derived Button name.
    /// Toolbar retains Button semantics.
    pub fn aria_label(mut self, label: impl Into<String>) -> Self {
        self.aria_label = Some(label.into());
        self
    }

    /// Select trigger styling and geometry.
    /// Explicit tokens override colours for non-toolbar kinds. ActionBar and Quiet
    /// still derive geometry from their size; Toolbar bypasses these tokens entirely.
    pub fn kind(mut self, kind: MenuButtonKind) -> Self {
        self.kind = kind;
        self
    }

    /// Resolve disabled ink against the supplied parent fill when the trigger fill is translucent.
    /// An opaque disabled fill resolves against itself. Without a parent fill,
    /// translucent triggers retain their preferred disabled ink. Toolbar bypasses this setting.
    pub fn disabled_on(mut self, backdrop: Color) -> Self {
        self.disabled_backdrop = Some(backdrop);
        self
    }

    /// Copy height, radius and label size from a button size tier.
    /// Later setters override these fields. ActionBar and Quiet derive geometry
    /// at render time instead, and Toolbar uses its own geometry.
    pub fn size(mut self, size: crate::button::ButtonSize) -> Self {
        let tokens = size.tokens();
        self.height = tokens.height;
        self.radius = tokens.radius;
        self.label_size = tokens.text_size;
        self
    }

    /// Trigger height.
    pub fn height(mut self, height: f32) -> Self {
        self.height = FinitePositive::new_or(height, 30.0).get();
        self
    }

    /// Pin the trigger corner radius; unpinned corners follow the theme's
    /// `shape.button` step.
    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = FiniteNonNeg::new_or(radius, TRIGGER_RADIUS).get();
        self.radius_pinned = true;
        self
    }

    /// Width of the chevron cell in split mode.
    pub fn arrow_width(mut self, width: f32) -> Self {
        self.arrow_width = FinitePositive::new_or(width, 26.0).get();
        self
    }

    /// Label type size.
    pub fn label_size(mut self, size: f32) -> Self {
        self.label_size = FinitePositive::new_or(size, 13.0).get();
        self
    }

    /// Put the popup menu on liquid glass of `style` (see
    /// [`EnzMenu::material`](crate::menu::EnzMenu::material)).
    pub fn menu_material(mut self, style: enzui_theme::MaterialStyle) -> Self {
        self.menu_material = Some(style);
        self
    }

    /// Pin popup width in logical pixels; invalid or nonpositive values use 240.
    /// Unset uses the menu's intrinsic width.
    pub fn popup_width(mut self, width: f32) -> Self {
        self.popup_width = Some(FinitePositive::new_or(width, 240.0).get());
        self
    }

    /// Preferred popup side. Placement flips it when it does not fit.
    pub fn side(mut self, side: Side) -> Self {
        self.side = side;
        self
    }

    /// Which edge of the trigger the popup lines up with. Defaults to the
    /// leading edge ([`Alignment::Start`], see [`menu_placement`]); a trigger
    /// at the trailing edge of a field or panel passes [`Alignment::End`] so
    /// the popup opens back over its container instead of past it.
    pub fn alignment(mut self, alignment: Alignment) -> Self {
        self.alignment = alignment;
        self
    }

    /// Pin leading-edge placement direction; otherwise follow ambient locale direction.
    pub fn direction(mut self, direction: Direction) -> Self {
        self.direction = Some(direction);
        self
    }

    /// Pin trigger colours for non-toolbar kinds; Toolbar bypasses these tokens.
    pub fn tokens(mut self, tokens: MenuButtonTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Attach a widget id to the menu trigger segment (the chevron in split
    /// mode, the whole surface in menu-only mode) so an application can
    /// restore focus to it when the menu closes.
    ///
    /// Pair with [`focus_restore_operation`] in `update`: on `Close(...)` (or
    /// `Activated`, which also closes) run the returned `Operation`. Split
    /// primaries are separate controls with separate tab stops — untouched by
    /// this id.
    pub fn menu_id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.menu_id = Some(id);
        self
    }

    /// Name the independent split primary for focus and accessibility action routing.
    /// Ignored in menu-only mode. No id is derived from the presentation label.
    pub fn primary_id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.primary_id = Some(id);
        self
    }

    /// Override the empty-label or Toolbar trigger icon.
    /// Other labelled non-toolbar buttons retain their chevron.
    pub fn trigger_icon(mut self, icon: impl Into<String>) -> Self {
        self.trigger_icon = Some(icon.into());
        self
    }

    /// Return the explicit trigger icon or [`ICON_ONLY_TRIGGER_ICON`].
    #[must_use]
    pub fn resolved_trigger_icon(&self) -> &str {
        self.trigger_icon
            .as_deref()
            .unwrap_or(ICON_ONLY_TRIGGER_ICON)
    }

    /// Return whether the control is enabled and its model has focusable rows.
    #[must_use]
    pub fn menu_is_available(&self) -> bool {
        self.enabled && self.model.has_focusable()
    }

    /// Whether the primary segment can fire. Split mode only, and independent
    /// of whether the menu has any rows.
    #[must_use]
    pub fn primary_is_available(&self) -> bool {
        self.split && self.enabled && self.on_activated.is_some()
    }

    /// Which segments are painted as live. See [`VisualLiveness`].
    #[must_use]
    pub fn visual_liveness(&self) -> VisualLiveness {
        let menu = self.menu_is_available();
        VisualLiveness {
            primary: if self.split { self.enabled } else { menu },
            menu,
        }
    }

    /// Whether this renders as a split button.
    #[must_use]
    pub const fn is_split(&self) -> bool {
        self.split
    }

    /// Semantics for the popup surface, for a host that wraps it.
    pub fn popup_semantics(&self) -> enzui_core::semantics::Semantics {
        let label = (!self.label.trim().is_empty()).then_some(self.label.as_str());
        menu_semantics(label, self.model)
    }

    /// Return the available menu segment's pointer toggle: open when closed, close when open.
    #[must_use]
    pub fn menu_press_event(&self) -> Option<MenuButtonEvent> {
        if !self.menu_is_available() {
            return None;
        }
        Some(if self.state.is_open() {
            MenuButtonEvent::Close(CloseCause::TriggerToggle)
        } else {
            MenuButtonEvent::Open(OpenCause::Pointer)
        })
    }
    fn popup(&self) -> Option<Element<'a, Message>> {
        // Popup padding and separator gaps must capture presses before the outside guard.
        Some(crate::swallow_press::SwallowPress::new(self.build_popup()?.0).into())
    }
    fn build_popup(&self) -> Option<BuiltPopup<'a, Message>> {
        if !self.state.is_open() || !self.menu_is_available() {
            return None;
        }
        // Validate ids only when building rows; closed triggers avoid this full scan.
        debug_assert!(
            self.model.duplicate_ids().is_empty(),
            "menu entries must have unique ids; duplicates: {:?}",
            self.model.duplicate_ids()
        );
        let mut menu = enz_menu();
        if let Some(style) = self.menu_material {
            menu = menu.material(style);
        }
        let popup_name = if matches!(self.kind, MenuButtonKind::Toolbar) {
            self.aria_label
                .as_deref()
                .filter(|name| !name.trim().is_empty())
                .or_else(|| (!self.label.trim().is_empty()).then_some(self.label.trim()))
                .map_or_else(
                    || enzui_core::localization::text("enzui-actions-more"),
                    str::to_owned,
                )
        } else {
            match self.label.trim() {
                "" => enzui_core::localization::text("enzui-menu-button-menu"),
                trimmed => {
                    enzui_core::localized!("enzui-menu-button-menu-detail", "trimmed" => trimmed)
                }
            }
        };
        // Rows are keyed under the menu's id, or else its name, so routed
        // accessibility activation can find the same row after a rebuild.
        let row_prefix = match &self.menu_id {
            Some(id) => {
                use std::hash::{Hash, Hasher};
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                id.hash(&mut hasher);
                format!("menu-button-{:016x}", hasher.finish())
            }
            None => format!("menu-button:{popup_name}"),
        };
        menu = menu.label(popup_name);
        if let Some(width) = self.popup_width {
            menu = menu.width(width);
        }

        let mut emitted: Vec<EmittedRow> = Vec::new();
        let menu = self.push_rows(menu, self.model, &row_prefix, &mut emitted, true);
        Some((menu.into_element(), emitted))
    }

    /// The message a row the pointer enters publishes: a highlight, which
    /// on a submenu row also opens its submenu.
    fn hover_event(&self) -> std::rc::Rc<dyn Fn(EntryId) -> Message + 'a> {
        let on_event = std::rc::Rc::clone(&self.on_event);
        std::rc::Rc::new(move |id| on_event(MenuButtonEvent::Highlight(id)))
    }

    /// Push `level`'s rows onto `menu`. With `nested`, the row whose submenu
    /// is open carries that submenu's popup beside it.
    fn push_rows(
        &self,
        mut menu: crate::menu::EnzMenu<'a, Message>,
        level: &'a MenuModel,
        row_prefix: &str,
        emitted: &mut Vec<EmittedRow>,
        nested: bool,
    ) -> crate::menu::EnzMenu<'a, Message> {
        let reserve_leading = level
            .entries()
            .iter()
            .any(|entry| entry.icon_name().is_some() || entry.check_state().is_checkable());
        let dedup = level.dedup_entries();
        let focusable_total = dedup.iter().filter(|e| e.is_focusable()).count();
        let mut focusable_seen: usize = 0;
        for entry in dedup {
            if entry.kind() == EntryKind::Separator {
                menu = menu.push(enz_menu_separator::<Message>());
                continue;
            }
            // Resolve the held entry to avoid a model scan per row; defer the app mapper until activation.
            let event = match entry.activation() {
                Activation::Dispatch { id, check } => MenuButtonEvent::Activated { id, check },
                Activation::OpenSubmenu(id) => MenuButtonEvent::SubmenuOpened(id),
                Activation::None => MenuButtonEvent::Highlight(entry.id().clone()),
            };

            let factory: Box<dyn Fn() -> Message + 'a> = {
                let on_event = std::rc::Rc::clone(&self.on_event);
                Box::new(move || on_event(event.clone()))
            };

            let position = entry.is_focusable().then(|| {
                focusable_seen += 1;
                enzui_core::semantics::SetPosition {
                    index: focusable_seen,
                    count: focusable_total,
                }
            });
            let row = model_row(
                entry,
                level.active() == Some(entry.id()),
                reserve_leading,
                row_prefix,
                position,
                factory,
            );
            emitted.push((
                entry.id().clone(),
                entry.label().to_owned(),
                entry.icon_name().map(str::to_owned),
            ));

            let submenu = nested
                .then(|| self.state.open_submenu())
                .flatten()
                .filter(|open| *open == entry.id())
                .and_then(|open| level.submenu(open));
            let row: Element<'a, Message> = match submenu {
                Some(child) => {
                    let mut popup = enz_menu().label(entry.label().to_owned());
                    if let Some(style) = self.menu_material {
                        popup = popup.material(style);
                    }
                    let popup = self.push_rows(
                        popup,
                        child,
                        &format!("{row_prefix}-{}", entry.id().as_str()),
                        emitted,
                        false,
                    );
                    let direction = resolve_direction(self.direction);
                    anchored_overlay(
                        row,
                        Some(crate::swallow_press::SwallowPress::new(popup.into_element()).into()),
                    )
                    .placement(
                        Placement::new(Side::Right)
                            .alignment(Alignment::Start)
                            .direction(direction),
                    )
                    .into()
                }
                None => row.into(),
            };
            // Rows are pushed as children, so each reports the pointer
            // entering it itself: a highlight, which opens a submenu.
            let row = if entry.is_focusable() {
                crate::menu::report_hover(row, entry.id().clone(), self.hover_event())
            } else {
                row
            };
            menu = menu.push(row);
        }
        menu
    }

    /// Build the trigger and controlled popup with live palette resolution unless colours are pinned.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        let pinned = self.tokens;
        let enabled = self.enabled;
        let split_divider = self.split_divider;
        let menu_id = self.menu_id.clone();
        let primary_id = self.primary_id.clone();
        let VisualLiveness {
            primary: primary_live,
            menu: menu_live,
        } = self.visual_liveness();
        let TriggerGeometry {
            height,
            radius,
            label_size,
            padding_x: trigger_padding_x,
        } = resolve_geometry(self.kind, self.height, self.radius, self.label_size);

        let kind = self.kind;
        let radius_pinned = self.radius_pinned;
        let disabled_backdrop = self.disabled_backdrop;
        let resolve =
            move |theme: &iced::Theme| -> MenuButtonTokens { resolve_tokens(pinned, kind, theme) };
        let menu_event = self.menu_press_event();
        let menu_message: Option<Box<dyn Fn() -> Message + 'a>> = menu_event.map(|event| {
            let on_event = std::rc::Rc::clone(&self.on_event);
            let factory: Box<dyn Fn() -> Message + 'a> = Box::new(move || on_event(event.clone()));
            factory
        });
        let primary_message = self
            .primary_is_available()
            .then(|| self.on_activated.clone())
            .flatten();

        let label = (!self.label.trim().is_empty()).then(|| self.label.clone());
        let aria_label = self.aria_label.clone();
        let icon_only_trigger = self.resolved_trigger_icon().to_owned();
        let open = self.state.is_open();
        let popup = self.popup();
        let split = self.split;
        let arrow_width = self.arrow_width;

        let chevron = crate::icon::enz_icon("navigation.down")
            .size(CHEVRON_ICON_SIZE)
            .tint_resolved(move |theme: &iced::Theme| {
                let t = resolve(theme);
                if menu_live {
                    t.foreground
                } else {
                    disabled_foreground(&crate::theme_lookup::palette(theme), &t, disabled_backdrop)
                }
            });

        let style_for = move |leading: bool, trailing: bool| {
            move |theme: &iced::Theme, status: FocusStatus| -> ButtonStyle {
                let t = resolve(theme);
                let radius = match kind {
                    MenuButtonKind::ActionBar(size) | MenuButtonKind::Quiet(size) => {
                        size.resolve_radius(theme)
                    }
                    _ if radius_pinned => radius,
                    _ => crate::theme_lookup::shape_radius(theme, |shape| shape.button),
                };
                let background = match status {
                    FocusStatus::Disabled => t.disabled_background,
                    FocusStatus::Pressed => t.pressed_background,
                    FocusStatus::Hovered => t.hover_background,
                    _ => t.background,
                };
                let text_color = if matches!(status, FocusStatus::Disabled) {
                    disabled_foreground(&crate::theme_lookup::palette(theme), &t, disabled_backdrop)
                } else {
                    t.foreground
                };
                ButtonStyle {
                    background: Some(Background::Color(background)),
                    text_color,
                    border: Border {
                        color: Color::TRANSPARENT,
                        width: 0.0,
                        radius: iced::border::Radius::default()
                            .top_left(if leading { radius } else { 0.0 })
                            .bottom_left(if leading { radius } else { 0.0 })
                            .top_right(if trailing { radius } else { 0.0 })
                            .bottom_right(if trailing { radius } else { 0.0 }),
                    },
                    ..ButtonStyle::default()
                }
            }
        };

        let tooltip = self.tooltip.clone().or_else(|| {
            (matches!(kind, MenuButtonKind::Toolbar) || (!split && label.is_none())).then(|| {
                aria_label
                    .clone()
                    .or_else(|| label.clone())
                    .unwrap_or_else(|| enzui_core::localization::text("enzui-actions-more"))
            })
        });
        let trigger: Element<'a, Message> = if matches!(kind, MenuButtonKind::Toolbar) {
            let accessible_name = aria_label
                .or_else(|| label.clone())
                .unwrap_or_else(|| enzui_core::localization::text("enzui-actions-more"));
            let mut button =
                crate::toolbar_icon::enz_toolbar_icon_maybe_with(icon_only_trigger, menu_message)
                    .enabled(menu_live)
                    .aria_label(accessible_name)
                    .tooltip("")
                    .expanded(open);
            if let Some(id) = menu_id {
                button = button.id(id);
            }
            button.into()
        } else if split {
            let primary_label = text(self.label.clone())
                .size(label_size)
                .font(enzui_theme::text_defaults::font(
                    enzui_theme::FontRole::Ui,
                    enzui_theme::WeightRung::Semibold,
                ))
                .style(move |theme: &iced::Theme| {
                    let t = resolve(theme);
                    iced::widget::text::Style {
                        color: Some(if primary_live {
                            t.foreground
                        } else {
                            disabled_foreground(
                                &crate::theme_lookup::palette(theme),
                                &t,
                                disabled_backdrop,
                            )
                        }),
                    }
                });

            let mut primary = focus_button_maybe(
                container(primary_label)
                    .padding(
                        Padding::ZERO
                            .left(trigger_padding_x)
                            .right(trigger_padding_x),
                    )
                    .height(height)
                    .center_y(height),
                primary_message,
            )
            .semantics(
                enzui_core::semantics::Semantics::new(enzui_core::semantics::Role::Button)
                    .label_opt(label.clone())
                    .default_action()
                    .disabled(!primary_live),
            )
            .outset_focus_ring(true)
            .style_focus(style_for(true, false));
            if let Some(id) = primary_id {
                primary = primary.id(id);
            }

            let divider = container(Space::new())
                .width(1.0)
                .height(height - 8.0)
                .style(move |theme: &iced::Theme| {
                    let t = resolve(theme);
                    container::Style {
                        background: Some(Background::Color(if enabled && split_divider {
                            t.divider
                        } else {
                            Color::TRANSPARENT
                        })),
                        ..container::Style::default()
                    }
                });

            let mut chevron_button = focus_button_maybe_with(
                container(chevron.into_element::<Message>())
                    .width(arrow_width)
                    .height(height)
                    .center_x(arrow_width)
                    .center_y(height),
                menu_message,
            )
            .semantics(trigger_semantics(
                label.as_deref(),
                aria_label.as_deref(),
                open,
                menu_live,
            ))
            .outset_focus_ring(true)
            .style_focus(style_for(false, true));
            if let Some(id) = menu_id {
                chevron_button = chevron_button.expand_alias(id.clone()).id(id);
            }

            row![
                primary,
                container(divider).height(height).center_y(height),
                chevron_button
            ]
            .into()
        } else if label.is_some() {
            let menu_label = text(self.label.clone())
                .size(label_size)
                .font(enzui_theme::text_defaults::font(
                    enzui_theme::FontRole::Ui,
                    enzui_theme::WeightRung::Semibold,
                ))
                .style(move |theme: &iced::Theme| {
                    let t = resolve(theme);
                    iced::widget::text::Style {
                        color: Some(if menu_live {
                            t.foreground
                        } else {
                            disabled_foreground(
                                &crate::theme_lookup::palette(theme),
                                &t,
                                disabled_backdrop,
                            )
                        }),
                    }
                });

            let mut btn = focus_button_maybe_with(
                container(
                    row![
                        menu_label,
                        Space::new().width(8.0),
                        chevron.into_element::<Message>()
                    ]
                    .align_y(iced::Alignment::Center),
                )
                .padding(
                    Padding::ZERO
                        .left(trigger_padding_x)
                        .right(trigger_padding_x),
                )
                .height(height)
                .center_y(height),
                menu_message,
            )
            .semantics(trigger_semantics(
                label.as_deref(),
                aria_label.as_deref(),
                open,
                menu_live,
            ))
            .outset_focus_ring(true)
            .style_focus(style_for(true, true));
            if let Some(id) = menu_id {
                btn = btn.expand_alias(id.clone()).id(id);
            }
            btn.into()
        } else {
            let icon_el = crate::icon::enz_icon(&icon_only_trigger)
                .size(ICON_ONLY_TRIGGER_SIZE)
                .tint_resolved(move |theme: &iced::Theme| {
                    let t = resolve(theme);
                    if menu_live {
                        t.foreground
                    } else {
                        disabled_foreground(
                            &crate::theme_lookup::palette(theme),
                            &t,
                            disabled_backdrop,
                        )
                    }
                });

            let mut btn = focus_button_maybe_with(
                container(icon_el.into_element::<Message>())
                    .height(height)
                    .center_x(height)
                    .center_y(height),
                menu_message,
            )
            .semantics(trigger_semantics(
                label.as_deref(),
                aria_label.as_deref(),
                open,
                menu_live,
            ))
            .outset_focus_ring(true)
            .style_focus(style_for(true, true));
            if let Some(id) = menu_id {
                btn = btn.expand_alias(id.clone()).id(id);
            }
            btn.into()
        };
        let trigger = match tooltip {
            Some(tip) => crate::tooltip::enz_tooltip_with_delay(
                trigger,
                tip,
                iced::widget::tooltip::Position::Top,
                self.tooltip_delay
                    .unwrap_or_else(crate::tooltip::default_tooltip_delay),
            ),
            None => trigger,
        };
        let popup_live = popup.is_some();
        let anchored: Element<'a, Message> = anchored_overlay(trigger, popup)
            .placement(
                menu_placement(self.side, resolve_direction(self.direction))
                    .alignment(self.alignment),
            )
            .into();
        let on_event = std::rc::Rc::clone(&self.on_event);
        dismiss_on_outside_press(anchored, self.model, self.state, popup_live, move |event| {
            on_event(event)
        })
    }
}

impl<'a, Message: Clone + 'a> From<EnzMenuButton<'a, Message>> for Element<'a, Message> {
    fn from(button: EnzMenuButton<'a, Message>) -> Self {
        button.into_element()
    }
}

/// One menu row for `entry`: its label, state, icon, shortcut, description,
/// check mark and submenu chevron, published under `row_prefix` and at
/// `position` among the focusable rows. Shared by the menu button and the
/// menu bar.
pub(crate) fn model_row<'a, Message: Clone + 'a>(
    entry: &enzui_core::menu_model::MenuEntryModel,
    highlighted: bool,
    reserve_leading: bool,
    row_prefix: &str,
    position: Option<enzui_core::semantics::SetPosition>,
    on_activate: Box<dyn Fn() -> Message + 'a>,
) -> crate::menu_row::EnzMenuRow<'a, Message> {
    let mut row = enz_menu_row_maybe_with(entry.label().to_owned(), Some(on_activate))
        .enabled(entry.is_enabled())
        .highlighted(highlighted)
        .reserve_leading(reserve_leading)
        .id(enzui_core::identity::derive_id(
            row_prefix,
            entry.id().as_str(),
        ));
    if let Some(position) = position {
        row = row.set_position(position);
    }
    if entry.is_destructive() {
        row = row.tone(MenuRowTone::Destructive);
    }
    if let Some(icon) = entry.icon_name() {
        row = row.icon(icon.to_owned());
    }
    if let Some(color) = entry.icon_tint_color() {
        row = row.icon_tint(color);
    }
    if let Some(shortcut) = entry.shortcut_hint() {
        row = row.trailing(shortcut.to_owned());
    }
    if let Some(description) = entry.description_text() {
        row = row.description(description.to_owned());
    }
    if entry.check_state().is_checkable() {
        row = row
            .checkable(true)
            .radio(entry.is_radio())
            .selected(entry.check_state().is_checked());
    }
    if entry.kind() == EntryKind::Submenu {
        row = row.submenu_indicator(true);
    }
    row
}

/// Apply navigation and lifecycle events, returning a revalidated dispatched entry id.
/// Activation rechecks the current model and closes the menu; it ignores the
/// event's check payload and does not toggle model check states. Applications
/// execute commands and update check values. This reducer does not deduplicate
/// repeated activation events. With a submenu open, navigation and activation
/// act on its model.
pub fn apply_event(
    model: &mut MenuModel,
    state: &mut MenuState,
    event: &MenuButtonEvent,
) -> Option<EntryId> {
    match event {
        MenuButtonEvent::PrimaryActivated => None,
        MenuButtonEvent::Open(cause) => {
            if state.open()
                && let Some(movement) = cause.initial_move()
            {
                model.navigate(movement);
            }
            None
        }
        MenuButtonEvent::Close(cause) => {
            if state.close(*cause) {
                model.clear_active();
            }
            None
        }
        MenuButtonEvent::Navigate(movement) => {
            match open_submenu_mut(model, state) {
                Some(submenu) => submenu.navigate(*movement),
                None => model.navigate(*movement),
            };
            None
        }
        MenuButtonEvent::Highlight(id) => {
            if let Some(submenu) = open_submenu_mut(model, state)
                && submenu.set_active(id)
            {
                return None;
            }
            // Moving onto another row of the menu closes its submenu.
            if state.open_submenu().is_some_and(|open| open != id) {
                close_submenu(model, state);
            }
            model.set_active(id);
            // Resting on a submenu row opens its submenu, as desktop menus
            // do, with none of its rows highlighted yet.
            if model.submenu(id).is_some() {
                state.open_submenu_for(id.clone());
            }
            None
        }
        MenuButtonEvent::Activated { id, .. } => {
            let activation = match open_submenu_mut(model, state) {
                Some(submenu) if submenu.entry(id).is_some() => submenu.activate(id),
                _ => model.activate(id),
            };
            match activation {
                Activation::Dispatch { id, .. } => {
                    state.close(CloseCause::Activated);
                    model.clear_active();
                    Some(id)
                }
                _ => None,
            }
        }
        MenuButtonEvent::SubmenuOpened(id) => {
            if state.open_submenu() != Some(id) {
                close_submenu(model, state);
                state.open_submenu_for(id.clone());
            }
            model.set_active(id);
            None
        }
        MenuButtonEvent::SubmenuEntered(id) => {
            if state.open_submenu() != Some(id) {
                close_submenu(model, state);
                state.open_submenu_for(id.clone());
            }
            model.set_active(id);
            if let Some(submenu) = model.submenu_mut(id) {
                submenu.navigate(MenuMove::First);
            }
            None
        }
        MenuButtonEvent::SubmenuClosed => {
            close_submenu(model, state);
            None
        }
    }
}

/// The open submenu's model, when the model holds its entries.
fn open_submenu_mut<'m>(model: &'m mut MenuModel, state: &MenuState) -> Option<&'m mut MenuModel> {
    model.submenu_mut(state.open_submenu()?)
}

fn close_submenu(model: &mut MenuModel, state: &mut MenuState) {
    if let Some(submenu) = open_submenu_mut(model, state) {
        submenu.clear_active();
    }
    state.close_submenu();
}

/// Decode menu keys when available; return `None` for unclaimed keys.
/// Closed menus accept opening keys; open menus accept navigation and activation.
/// Right opens only submenus; with a submenu open, keys act on it, and Left
/// or Escape close it alone. Without the submenu's entries in the model, Left
/// closes the whole menu when a submenu id is set.
#[must_use]
pub fn event_for_key(
    model: &MenuModel,
    state: &MenuState,
    available: bool,
    event: &iced::keyboard::Event,
) -> Option<MenuButtonEvent> {
    if !available {
        return None;
    }

    if !state.is_open() {
        return open_cause_for_key(event).map(MenuButtonEvent::Open);
    }

    if let Some(submenu) = state.open_submenu().and_then(|id| model.submenu(id)) {
        return match key_for_open_menu(event)? {
            MenuKey::Move(movement) => Some(MenuButtonEvent::Navigate(movement)),
            MenuKey::Dismiss | MenuKey::CloseSubmenu => Some(MenuButtonEvent::SubmenuClosed),
            MenuKey::Activate => match submenu.activate_active() {
                Activation::Dispatch { id, check } => {
                    Some(MenuButtonEvent::Activated { id, check })
                }
                _ => None,
            },
            MenuKey::OpenSubmenu => None,
        };
    }

    match key_for_open_menu(event)? {
        MenuKey::Move(movement) => Some(MenuButtonEvent::Navigate(movement)),
        MenuKey::Dismiss => dismissal::on_escape(menu_dismiss_policy())
            .closes()
            .then_some(MenuButtonEvent::Close(CloseCause::Escape)),
        MenuKey::Activate => match model.activate_active() {
            Activation::Dispatch { id, check } => Some(MenuButtonEvent::Activated { id, check }),
            Activation::OpenSubmenu(id) => Some(MenuButtonEvent::SubmenuEntered(id)),
            Activation::None => None,
        },
        MenuKey::OpenSubmenu => match model.activate_active() {
            Activation::OpenSubmenu(id) => Some(MenuButtonEvent::SubmenuEntered(id)),
            _ => None,
        },
        MenuKey::CloseSubmenu => state
            .open_submenu()
            .map(|_| MenuButtonEvent::Close(CloseCause::Escape))
            .or(None),
    }
}

/// Place a popup at the trigger's leading edge with [`POPUP_GAP`], respecting writing direction.
/// Placement may flip and shift it within the viewport.
#[must_use]
pub fn menu_placement(side: Side, direction: Direction) -> Placement {
    Placement::new(side)
        .gap(POPUP_GAP)
        .alignment(Alignment::Start)
        .direction(direction)
}

/// Dismiss on outside press or Escape, consuming an unclaimed closing press.
/// The policy's modal input setting does not add a scrim, background inertness
/// or modal accessibility semantics.
#[must_use]
pub const fn menu_dismiss_policy() -> DismissPolicy {
    DismissPolicy::popover().modality(enzui_core::dismissal::Modality::Modal)
}

/// Return an outside-click close event for an open menu when the policy dismisses.
/// Trigger presses remain owned by the toggle. This helper does not capture input.
#[must_use]
pub fn event_for_press(
    state: &MenuState,
    regions: &DismissRegions,
    point: iced::Point,
) -> Option<MenuButtonEvent> {
    if !state.is_open() {
        return None;
    }
    dismissal::on_press(menu_dismiss_policy(), regions, point)
        .closes()
        .then_some(MenuButtonEvent::Close(CloseCause::OutsideClick))
}

/// Register the trigger and any supplied popup or submenu bounds as inside regions.
/// All bounds and queried press points must share a coordinate space. The
/// menu-button wrapper supplies only trigger bounds because its popup captures presses.
#[must_use]
pub fn dismiss_regions(
    trigger: iced::Rectangle,
    popup: Option<iced::Rectangle>,
    submenu: Option<iced::Rectangle>,
) -> DismissRegions {
    let mut regions = match popup {
        Some(popup) => DismissRegions::new(popup),
        None => DismissRegions::empty(),
    };
    regions = regions.trigger(trigger);
    if let Some(submenu) = submenu {
        regions = regions.with(submenu);
    }
    regions
}

use iced::advanced::{Widget, layout as adv_layout, overlay as adv_overlay, widget as adv_widget};
use iced::{Event, Length, Rectangle, Size, Vector, mouse};

/// Wrap a menu subtree with outside-press dismissal and open-menu keyboard handling.
/// `open` means a popup is available, accounting for enabled state and model rows.
/// Outside mouse and touch presses dismiss even if a sibling already captured
/// them; unclaimed closing presses are captured. Open-menu keys run before the
/// trigger to prevent its focused toggle from intercepting activation. Other keys
/// remain available to the application.
#[must_use]
pub fn dismiss_on_outside_press<'a, Message: Clone + 'a>(
    trigger: impl Into<Element<'a, Message>>,
    model: &'a MenuModel,
    state: &'a MenuState,
    open: bool,
    on_dismiss: impl Fn(MenuButtonEvent) -> Message + 'a,
) -> Element<'a, Message> {
    Element::new(DismissOnOutsidePress {
        trigger: trigger.into(),
        model,
        state,
        open,
        on_dismiss: Box::new(on_dismiss),
    })
}

struct DismissOnOutsidePress<'a, Message> {
    trigger: Element<'a, Message>,
    model: &'a MenuModel,
    state: &'a MenuState,
    open: bool,
    on_dismiss: Box<dyn Fn(MenuButtonEvent) -> Message + 'a>,
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for DismissOnOutsidePress<'_, Message>
where
    Message: Clone,
{
    fn tag(&self) -> adv_widget::tree::Tag {
        self.trigger.as_widget().tag()
    }

    fn state(&self) -> adv_widget::tree::State {
        self.trigger.as_widget().state()
    }

    fn diff(&mut self, tree: &mut adv_widget::Tree) {
        self.trigger.as_widget_mut().diff(tree);
    }

    fn size(&self) -> Size<Length> {
        self.trigger.as_widget().size()
    }

    fn is_void(&self) -> bool {
        self.trigger.as_widget().is_void()
    }

    fn layout(
        &mut self,
        tree: &mut adv_widget::Tree,
        renderer: &iced::Renderer,
        limits: &adv_layout::Limits,
    ) {
        self.trigger.as_widget_mut().layout(tree, renderer, limits);
    }

    fn draw(
        &self,
        tree: &adv_widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &iced::advanced::renderer::Style,
        layout: adv_layout::Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.trigger
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn mouse_interaction(
        &self,
        tree: &adv_widget::Tree,
        layout: adv_layout::Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.trigger
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn operate(
        &mut self,
        tree: &mut adv_widget::Tree,
        layout: adv_layout::Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn adv_widget::Operation,
    ) {
        self.trigger
            .as_widget_mut()
            .operate(tree, layout, viewport, renderer, operation);
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut adv_widget::Tree,
        layout: adv_layout::Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: iced::Size,
    ) -> Vec<adv_overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.trigger
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }

    fn update(
        &mut self,
        tree: &mut adv_widget::Tree,
        event: &Event,
        layout: adv_layout::Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        // Open-menu activation precedes the focused trigger toggle.
        if self.open
            && let iced::Event::Keyboard(key_event) = event
            && !shell.is_event_captured()
            && let Some(menu_event) = event_for_key(self.model, self.state, true, key_event)
        {
            shell.publish((self.on_dismiss)(menu_event));
            shell.capture_event();
        }
        self.trigger
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);

        if !self.open {
            return;
        }
        // Sibling capture must not suppress dismissal; only unclaimed closing presses are captured.
        let already_captured = shell.is_event_captured();
        if matches!(event, iced::Event::Keyboard(_)) {
            return;
        }
        if !crate::swallow_press::is_press(event) {
            return;
        }
        let Some(position) = crate::swallow_press::press_position(event, cursor) else {
            return;
        };
        // Overlay presses are captured by the popup before reaching this base-tree guard.
        let regions = dismiss_regions(layout.bounds(), None, None);
        let outcome = dismissal::on_press(menu_dismiss_policy(), &regions, position);

        if let Some(event) = event_for_press(self.state, &regions, position) {
            shell.publish((self.on_dismiss)(event));
        }
        if outcome.consumes() && !already_captured {
            shell.capture_event();
        }
    }
}

impl<Message> std::fmt::Debug for DismissOnOutsidePress<'_, Message> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DismissOnOutsidePress")
            .field("open", &self.open)
            .finish_non_exhaustive()
    }
}

/// Classify a split-trigger x coordinate; the trailing arrow-width region belongs to Menu.
/// Menu-only controls always return Menu. Invalid total and arrow widths
/// normalize to zero; points outside the trigger are still classified.
#[must_use]
pub fn segment_at(x: f32, total: f32, arrow_width: f32, split: bool) -> Segment {
    if !split {
        return Segment::Menu;
    }
    let arrow = FiniteNonNeg::new_or(arrow_width, 0.0).get();
    let total = FiniteNonNeg::new_or(total, 0.0).get();
    if x >= (total - arrow).max(0.0) {
        Segment::Menu
    } else {
        Segment::Primary
    }
}

/// Row tokens used by the popup, resolved from the runtime palette.
#[must_use]
pub fn popup_row_tokens(palette: &Palette) -> MenuRowTokens {
    MenuRowTokens::from_palette(palette)
}

/// Popup surface tokens, resolved from the runtime palette.
#[must_use]
pub fn popup_surface_tokens(palette: &Palette) -> MenuTokens {
    MenuTokens::from_palette(palette)
}

/// Build a probe-then-focus operation for close causes that restore focus and for activation.
/// No operation is returned without a trigger id; a missing target leaves current
/// focus untouched. This helper inspects the event independently of activation
/// validation, so rejected activation events can still request restoration.
#[must_use]
pub fn focus_restore_operation<T>(
    event: &MenuButtonEvent,
    trigger_id: Option<&iced::advanced::widget::Id>,
) -> Option<Box<dyn iced::advanced::widget::Operation<T>>>
where
    T: Send + 'static,
{
    let restores = match event {
        MenuButtonEvent::Close(cause) => cause.restores_focus(),
        MenuButtonEvent::Activated { .. } => CloseCause::Activated.restores_focus(),
        _ => false,
    };
    if !restores {
        return None;
    }
    let target = trigger_id?.clone();
    Some(Box::new(enzui_core::focus::RestoreFocus::new(target)))
}

/// Deprecated alias for [`enzui_core::focus::RestoreFocus`].
#[deprecated(since = "0.1.0", note = "use `enzui_core::focus::RestoreFocus`")]
pub type RestoreTriggerFocus = enzui_core::focus::RestoreFocus;

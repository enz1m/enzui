//! Measured commands with a caller-controlled overflow menu.
//!
//! Folding uses the assigned width, command priorities and pinned/overflow-only
//! policies. Authored order is preserved within inline and overflow groups.
//! [`ActionItem::command`](crate::action_bar::ActionItem::command) builds both presentations from an [`Action`];
//! [`ActionItem::with_inline`](crate::action_bar::ActionItem::with_inline) supplies custom inline content, while
//! [`ActionItem::new`](crate::action_bar::ActionItem::new) keeps the caller's opaque widget in overflow as well.
//! Builder closures may run repeatedly during measurement and layout and must
//! not change application state.
//!
//! Feed the caller's menu state through [`EnzActionBar::overflow_open`](crate::action_bar::EnzActionBar::overflow_open). Trigger
//! activation publishes the toggle message. [`EnzActionBar::on_dismiss`](crate::action_bar::EnzActionBar::on_dismiss) adds an
//! idempotent close message after activation and handles Escape/outside presses;
//! without it, only Escape/outside presses fall back to toggle. Changes in
//! descriptor order, IDs, visibility or policy suppress a stale open menu until
//! the caller supplies `false`.
//!
//! When every overflow row has command metadata, the menu uses roving focus
//! and returns keyboard focus through [`crate::focus_return`]. Opaque overflow
//! content keeps its own keyboard behavior. Explicit trigger and row IDs remain
//! caller-owned; anonymous triggers have distinct retained focus-return tokens.
//!
//! Measured folding is the default. [`EnzActionBar::slot_width`](crate::action_bar::EnzActionBar::slot_width) reserves uniform
//! slots and centres their contents; it does not add a paint clip. Pinned
//! actions remain inline even when their geometry exceeds the available width.
//! A fluid action height is guarded to intrinsic height; fixed height and
//! padding still apply. In [`CollectorPlacement::TrailingCluster`], a fluid-width
//! action with non-shrink height owns the residual width instead of sharing it
//! with a synthetic spacer.
//!
//! [`crate::titlebar_actions`] supplies the trailing kebab preset.

use enzui_core::Action;
use enzui_core::action_overflow::{Descriptor, Plan, resolve};
use enzui_core::action_overflow::{OverflowPolicy, Presentation};
use enzui_core::menu_model::{CheckState, EntryId};
use enzui_core::metrics::{FiniteNonNeg, FinitePositive};
use enzui_core::placement::{Alignment as PlacementAlignment, Placement, Side};
use enzui_theme::{BreakpointScale, EnzSizeClass};
use iced::advanced::layout::Measure as _;
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::alignment::Horizontal;
use iced::widget::row;
use iced::{Alignment, Element, Event, Length, Rectangle, Size, Vector};

use crate::anchored_overlay::dismissible_anchored_menu;
use crate::button::{ButtonIconPosition, ButtonKind, ButtonSize, enz_button};
use crate::cross_fluid_guard::{Axis, hug_cross_fluid};
use crate::menu::{EnzMenuItem, enz_menu};
use crate::surface_stack::SpaceToken;
use crate::toolbar_icon::{ToolbarIconTokens, enz_toolbar_icon};
mod widget_impls;

/// Build one inline action for a resolved [`Presentation`].
///
/// The engine may call this repeatedly for measurement and rendering; avoid
/// application-state changes in the closure.
pub type ActionFn<'a, Message> = Box<dyn Fn(Presentation) -> Element<'a, Message> + 'a>;

/// Builds a framework-made inline command with the routable id the bar
/// gives it.
type IdentifiedFn<'a, Message> =
    Box<dyn Fn(Presentation, iced::advanced::widget::Id) -> Element<'a, Message> + 'a>;

/// How inline capacity is derived.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FoldPolicy {
    /// Measure every command's intrinsic width. This is the default and the
    /// right policy for unequal text buttons.
    Measured,
    /// Give every command the same fixed-width slot. This is appropriate for
    /// a toolbar whose icon controls are uniform by construction.
    Uniform(f32),
}

/// Framework-owned overflow trigger chrome with a shared accessible name and menu relationship.
#[derive(Debug, Clone)]
pub enum Collector {
    /// A labelled secondary button, the action-bar preset.
    Labelled {
        /// Visible label; the accessible name is the bar's
        /// [`trigger_label`](EnzActionBar::trigger_label).
        label: String,
    },
    /// A standard circular toolbar-icon button, the titlebar preset.
    Kebab {
        /// Bundled icon name, normally `ellipsis-vertical`.
        icon: String,
        /// Optional surface-aware toolbar tokens.
        tokens: Option<ToolbarIconTokens>,
    },
    /// A labelled button above the transported compact size class, and a
    /// kebab in that class.
    LabelledUntilCompact {
        /// Label shown outside the compact size class.
        label: String,
        /// Bundled icon shown in the compact size class.
        icon: String,
        /// Optional surface-aware toolbar tokens for the compact form.
        tokens: Option<ToolbarIconTokens>,
    },
}

impl Default for Collector {
    fn default() -> Self {
        Self::Labelled {
            label: enzui_core::localization::text("enzui-actions-more"),
        }
    }
}

/// Where the collector sits relative to inline commands.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum CollectorPlacement {
    /// Immediately after the last inline command.
    #[default]
    Inline,
    /// Pin only the collector to the assigned slot's trailing edge. Inline
    /// commands remain at the leading edge.
    Trailing,
    /// Keep the collector and every surviving inline command as one cluster at
    /// the assigned slot's trailing edge. Folding never leaves the survivors
    /// stranded at the leading edge; the collector itself remains stationary.
    TrailingCluster,
}

/// When framework-owned collector chrome is shown.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum CollectorVisibility {
    /// Show the collector exactly when the resolved plan contains overflow.
    /// This includes [`OverflowPolicy::AlwaysOverflow`] commands at every
    /// width, even when every foldable command fits inline.
    #[default]
    Automatic,
    /// Keep the collector visible even when the current plan has no overflow.
    /// Useful for a stable command-menu affordance whose inventory may be
    /// populated conditionally by the caller.
    Always,
}

#[derive(Clone)]
struct CommandMetadata<Message> {
    label: String,
    message: Message,
    enabled: bool,
    icon: Option<&'static str>,
    shortcut: Option<String>,
    check: Option<CheckState>,
    destructive: bool,
    group: Option<String>,
}

/// Default width budgeted per inline action slot, in px.
pub const DEFAULT_SLOT_WIDTH: f32 = 96.0;

/// Default logical-pixel width for an empty-label collector.
///
/// Nonempty labels use natural width by default. Kebab geometry comes from
/// its toolbar-icon tokens.
pub const DEFAULT_TRIGGER_WIDTH: f32 = 40.0;

/// Default overflow-popup width in logical pixels.
pub const DEFAULT_MENU_WIDTH: f32 = 240.0;

/// The widest an unpinned overflow menu grows to fit its labels.
pub const MAX_AUTO_MENU_WIDTH: f32 = 400.0;

/// Default trigger-to-popup gap in logical pixels.
pub const DEFAULT_POPUP_GAP: f32 = 4.0;

/// The owner kind of inline command ids: a command `new` in a bar with
/// menu id prefix `main` is `enz-action-bar#main/new`.
pub const COMMAND_ID_KIND: &str = "enz-action-bar";

/// Shared accessible name of the overflow trigger.
pub const TRIGGER_LABEL: &str = "More actions";

/// Default namespace for anonymous triggers' focus-return tokens.
///
/// Tokens include a retained instance ordinal and do not assign a widget ID.
pub const DEFAULT_FOCUS_RETURN_PREFIX: &str = "enz-action-bar-overflow-trigger";

/// Restore focus to an identified overflow trigger when closing caller-owned state.
///
/// Returns `None` when opening or when no trigger ID is supplied. The operation
/// checks that the target is still mounted. Command menus normally return focus
/// automatically after keyboard dismissal through [`crate::focus_return`]; this
/// operation also supports unconditional restoration on other close routes.
#[must_use]
pub fn overflow_focus_restore_operation<T: 'static>(
    was_open: bool,
    trigger_id: Option<&iced::advanced::widget::Id>,
) -> Option<Box<dyn Operation<T>>> {
    if !was_open {
        return None;
    }
    Some(Box::new(enzui_core::focus::RestoreFocus::new(
        trigger_id?.clone(),
    )))
}

/// The inline control of a [`ActionItem::command`].
fn command_inline<'a, Message: Clone + 'a>(
    action: &Action<Message>,
    presentation: Presentation,
    id: Option<iced::advanced::widget::Id>,
) -> Element<'a, Message> {
    match (action.icon(), presentation) {
        (Some(icon), Presentation::Compact) => {
            let mut button = enz_toolbar_icon(icon.name(), action.message().clone())
                .label(action.label())
                .enabled(action.is_enabled());
            if let Some(id) = id {
                button = button.id(id);
            }
            button.into()
        }
        (icon, _) => {
            let mut button = enz_button(action.label(), action.message().clone())
                .kind(ButtonKind::Secondary)
                .size(ButtonSize::Sm)
                .enabled(action.is_enabled());
            if let Some(icon) = icon {
                button = button.icon(icon.name());
            }
            if let Some(id) = id {
                button = button.id(id);
            }
            button.into()
        }
    }
}

/// One descriptor with an inline builder and optional overflow-command metadata.
///
/// [`Self::command`] builds both forms from one [`Action`]. [`Self::with_inline`]
/// uses the action for the menu row and passes it to a custom inline builder.
/// [`Self::new`] keeps its opaque content when folded.
pub struct ActionItem<'a, Message> {
    descriptor: Descriptor,
    build: ActionFn<'a, Message>,
    command: Option<CommandMetadata<Message>>,
    compact_inline: bool,
    /// For [`Self::command`] items: the inline control with the id the bar
    /// derives from the descriptor id, so assistive technology can invoke it.
    identified: Option<IdentifiedFn<'a, Message>>,
}

impl<'a, Message> ActionItem<'a, Message> {
    pub(crate) fn declares_layout_extent(&self) -> bool {
        let element = (self.build)(self.descriptor.presentation);
        let size = element.as_widget().size();
        let declares = |length| match length {
            Length::Shrink | Length::Fit => false,
            Length::Fixed(value) => {
                value.is_finite() && value >= crate::bounds_probe::SPILL_EPSILON
            }
            _ => true,
        };
        declares(size.width) || declares(size.height)
    }

    /// Pair a descriptor with opaque content used inline or in overflow.
    ///
    /// Fluid height is guarded to intrinsic height; fixed height and padding still apply.
    pub fn new(descriptor: Descriptor, build: impl Fn() -> Element<'a, Message> + 'a) -> Self {
        Self {
            descriptor,
            build: Box::new(move |_| build()),
            command: None,
            compact_inline: false,
            identified: None,
        }
    }

    /// Build inline and menu controls from the same [`Action`].
    ///
    /// Compact presentation uses an icon-only toolbar button when the action has
    /// an icon; otherwise it uses a labelled button. Both forms use its enabled state.
    ///
    /// The inline control is routable: its id is the descriptor id inside the
    /// bar's [`EnzActionBar::menu_id_prefix`] namespace, so bars that share a
    /// window and a command id need distinct prefixes.
    ///
    /// # Panics
    ///
    /// Panics when descriptor and action IDs differ.
    pub fn command(descriptor: Descriptor, action: Action<Message>) -> Self
    where
        Message: Clone + 'a,
    {
        let identified = action.clone();
        let mut item = Self::with_inline(descriptor, action, |action, presentation| {
            command_inline(action, presentation, None)
        });
        item.identified = Some(Box::new(move |presentation, id| {
            command_inline(&identified, presentation, Some(id))
        }));
        item
    }

    /// An icon command, the usual titlebar action: an icon-only toolbar
    /// button inline, named and tooltipped with `label`, and a labelled row
    /// with the icon once folded into the overflow menu.
    ///
    /// Shorthand for [`Self::command`] with a compact descriptor. For a
    /// pinned or prioritised icon command, build the [`Descriptor`] and use
    /// [`Self::command`].
    pub fn icon(
        id: &'static str,
        icon: &'static str,
        label: impl Into<String>,
        message: Message,
    ) -> Self
    where
        Message: Clone + 'a,
    {
        Self::command(
            Descriptor::new(id).with_presentation(Presentation::Compact),
            Action::new(id, message)
                .with_label(label)
                .with_icon(enzui_core::Icon::named(icon)),
        )
    }

    /// Use an [`Action`] for the folded menu row and a custom inline builder.
    ///
    /// The builder receives the action and presentation and must keep its command
    /// and enabled state consistent with the action.
    ///
    /// # Panics
    ///
    /// Panics when descriptor and action IDs differ.
    pub fn with_inline(
        descriptor: Descriptor,
        action: Action<Message>,
        build: impl Fn(&Action<Message>, Presentation) -> Element<'a, Message> + 'a,
    ) -> Self
    where
        Message: Clone + 'a,
    {
        assert_eq!(
            descriptor.id,
            action.id(),
            "action-bar Descriptor and Action ids must match"
        );
        let inline_action = action.clone();
        Self {
            descriptor,
            build: Box::new(move |presentation| build(&inline_action, presentation)),
            command: Some(CommandMetadata {
                label: action.label().to_owned(),
                message: action.message().clone(),
                enabled: action.is_enabled(),
                icon: action.icon().map(|icon| icon.name()),
                shortcut: action.shortcut().map(ToString::to_string),
                check: None,
                destructive: false,
                group: None,
            }),
            compact_inline: false,
            identified: None,
        }
    }

    /// Offer a compact inline variant before folding in measured trailing-collector layouts.
    ///
    /// Both variants are measured. At a given inline capacity, lower-priority
    /// commands compact first. Inline collector layout and uniform slots do not
    /// use this intermediate compact-width selection.
    #[must_use]
    pub fn compact_inline(mut self) -> Self {
        self.compact_inline = true;
        self
    }

    pub(crate) fn with_menu_command(
        descriptor: Descriptor,
        label: String,
        activate: Message,
        build: impl Fn(Presentation) -> Element<'a, Message> + 'a,
    ) -> Self {
        Self {
            descriptor,
            build: Box::new(build),
            command: Some(CommandMetadata {
                label,
                message: activate,
                enabled: true,
                icon: None,
                shortcut: None,
                check: None,
                destructive: false,
                group: None,
            }),
            compact_inline: false,
            identified: None,
        }
    }

    /// Set checked menu state for commands; opaque items are unchanged.
    #[must_use]
    pub fn check(mut self, state: CheckState) -> Self {
        if let Some(command) = &mut self.command {
            command.check = Some(state);
        }
        self
    }

    /// Set destructive menu styling for commands; opaque items are unchanged.
    #[must_use]
    pub fn destructive(mut self, destructive: bool) -> Self {
        if let Some(command) = &mut self.command {
            command.destructive = destructive;
        }
        self
    }

    /// Set a named overflow-menu group for commands; opaque items are unchanged.
    ///
    /// A transition after a named group inserts a separator when the next group
    /// differs. Inline separators remain part of the caller's builder.
    #[must_use]
    pub fn group(mut self, group: impl Into<String>) -> Self {
        if let Some(command) = &mut self.command {
            command.group = Some(group.into());
        }
        self
    }

    /// The descriptor this item carries.
    pub fn descriptor(&self) -> &Descriptor {
        &self.descriptor
    }
}

/// Reserve one overflow-trigger slot only when `slots < n`.
///
/// Returns all slots when the requested actions fit, otherwise `slots - 1`
/// saturating at zero. This helper does not inspect pinned or overflow-only policy.
#[must_use]
pub fn inline_budget(slots: usize, n: usize) -> usize {
    if slots >= n {
        slots
    } else {
        slots.saturating_sub(1)
    }
}

/// Count uniform slots fitting a width.
///
/// Non-finite or non-positive width or slot width returns zero. The result is
/// capped below `usize::MAX`, which the resolver reserves for an unlimited budget.
#[must_use]
pub fn slots_for_width(width: f32, slot_width: f32) -> usize {
    if !width.is_finite() || width <= 0.0 || !slot_width.is_finite() || slot_width <= 0.0 {
        return 0;
    }
    // Saturation must stay below the resolver's unlimited-budget sentinel.
    ((width / slot_width).floor().max(0.0) as usize).min(usize::MAX - 1)
}

/// Count the authored prefix of measured widths that fits, reserving a trigger when needed.
///
/// Gaps separate items and the trigger. If every item fits without a trigger,
/// return the full count; otherwise subtract the trigger and its gap and
/// recompute. Fitting allows a 0.5 logical-pixel tolerance.
///
/// Non-finite or negative available width returns zero. Invalid item width ends
/// the prefix; invalid gap or trigger width is treated as zero. Zero-width
/// items can fit a zero-width allocation.
#[must_use]
pub fn measured_budget(widths: &[f32], available: f32, gap: f32, trigger: f32) -> usize {
    measured_budget_with_trigger_gap(widths, available, gap, trigger, gap)
}

fn measured_budget_with_trigger_gap(
    widths: &[f32],
    available: f32,
    gap: f32,
    trigger: f32,
    trigger_gap: f32,
) -> usize {
    if !available.is_finite() || available < 0.0 {
        return 0;
    }
    let gap = if gap.is_finite() && gap >= 0.0 {
        gap
    } else {
        0.0
    };
    let trigger_gap = if trigger_gap.is_finite() && trigger_gap >= 0.0 {
        trigger_gap
    } else {
        0.0
    };

    let fit_within = |budget: f32| -> usize {
        let mut used = 0.0_f32;
        let mut n = 0_usize;
        for (i, w) in widths.iter().enumerate() {
            if !w.is_finite() || *w < 0.0 {
                break;
            }
            let step = if i == 0 { *w } else { gap + *w };
            // Match the shared fit policy's subpixel tolerance.
            if used + step > budget + 0.5 {
                break;
            }
            used += step;
            n += 1;
        }
        n
    };

    if fit_within(available) == widths.len() {
        return widths.len();
    }
    let trigger = if trigger.is_finite() && trigger >= 0.0 {
        trigger
    } else {
        0.0
    };
    fit_within((available - trigger - trigger_gap).max(0.0))
}

fn resolve_fitting(
    descriptors: &[Descriptor],
    widths: &[(String, f32, Option<f32>)],
    available: f32,
    gap: f32,
    trigger: f32,
    trigger_gap: f32,
    collector_always: bool,
) -> Plan {
    let foldable = descriptors
        .iter()
        .filter(|d| d.visible && d.policy != OverflowPolicy::AlwaysOverflow)
        .count();
    for budget in (0..=foldable).rev() {
        let mut plan = resolve(descriptors, budget);
        let used = |plan: &Plan| {
            let mut used = 0.0;
            for (index, descriptor) in plan.inline.iter().enumerate() {
                if index > 0 {
                    used += gap;
                }
                used += widths
                    .iter()
                    .find(|(id, _, _)| id == &descriptor.id)
                    .map_or(f32::INFINITY, |(_, expanded, compact)| {
                        if descriptor.presentation == Presentation::Compact {
                            compact.unwrap_or(*expanded)
                        } else {
                            *expanded
                        }
                    });
            }
            if collector_always || !plan.overflow.is_empty() {
                if !plan.inline.is_empty() {
                    used += trigger_gap;
                }
                used += trigger;
            }
            used
        };
        if used(&plan) <= available + 0.5 {
            return plan;
        }

        // Compact lower-priority variants before reducing inline command count.
        let mut compactable: Vec<_> = plan
            .inline
            .iter()
            .filter(|descriptor| {
                widths
                    .iter()
                    .any(|(id, _, compact)| id == &descriptor.id && compact.is_some())
            })
            .map(|descriptor| (descriptor.priority, descriptor.id.clone()))
            .collect();
        compactable.sort_by_key(|(priority, _)| *priority);
        for (_, id) in compactable {
            if let Some(descriptor) = plan.inline.iter_mut().find(|d| d.id == id) {
                descriptor.presentation = Presentation::Compact;
            }
            if used(&plan) <= available + 0.5 {
                return plan;
            }
        }
    }
    resolve(descriptors, 0)
}

/// Builder returned by [`enz_action_bar`].
#[must_use]
pub struct EnzActionBar<'a, Message> {
    actions: Vec<ActionItem<'a, Message>>,
    on_toggle_overflow: Message,
    overflow_open: bool,
    slot_width: Option<f32>,
    forced_budget: Option<usize>,
    trigger_width: Option<f32>,
    trigger_label: Option<String>,
    trigger_tooltip: Option<String>,
    tooltip_delay: Option<std::time::Duration>,
    collector: Collector,
    collector_placement: CollectorPlacement,
    collector_visibility: CollectorVisibility,
    max_inline_actions: Option<usize>,
    menu_width: f32,
    menu_width_pinned: bool,
    menu_material: Option<enzui_theme::MaterialStyle>,
    popup_gap: f32,
    menu_id_prefix: Option<String>,
    on_dismiss: Option<Message>,
    spacing: SpaceToken,
    custom_gap: Option<f32>,
    breakpoints: BreakpointScale,
    overflow_trigger_id: Option<iced::advanced::widget::Id>,
    align_x: Horizontal,
    fit_content: bool,
}

/// Build an action bar with caller-owned overflow state.
///
/// Trigger activation publishes `on_toggle_overflow`; feed the resulting state
/// back through [`EnzActionBar::overflow_open`].
pub fn enz_action_bar<'a, Message: Clone + 'a>(
    actions: Vec<ActionItem<'a, Message>>,
    on_toggle_overflow: Message,
) -> EnzActionBar<'a, Message> {
    EnzActionBar {
        actions,
        on_toggle_overflow,
        overflow_open: false,
        slot_width: None,
        forced_budget: None,
        trigger_width: None,
        trigger_label: None,
        trigger_tooltip: None,
        tooltip_delay: None,
        collector: Collector::default(),
        collector_placement: CollectorPlacement::Inline,
        collector_visibility: CollectorVisibility::Automatic,
        max_inline_actions: None,
        menu_width: DEFAULT_MENU_WIDTH,
        menu_width_pinned: false,
        menu_material: None,
        popup_gap: DEFAULT_POPUP_GAP,
        menu_id_prefix: None,
        on_dismiss: None,
        spacing: SpaceToken::Sm,
        custom_gap: None,
        breakpoints: BreakpointScale::default(),
        overflow_trigger_id: None,
        align_x: Horizontal::Left,
        fit_content: false,
    }
}

impl<'a, Message: Clone + 'a> EnzActionBar<'a, Message> {
    /// Reserve uniform inline slots of `px` logical pixels.
    ///
    /// Contents are centred in fixed-width containers. Non-finite or non-positive
    /// input uses [`DEFAULT_SLOT_WIDTH`] and remains in uniform mode. Leaving this
    /// unset uses measured command widths.
    pub fn slot_width(mut self, px: f32) -> Self {
        self.slot_width = Some(FinitePositive::new_or(px, DEFAULT_SLOT_WIDTH).get());
        self
    }

    /// Select measured or uniform-slot folding explicitly.
    pub fn fold_policy(mut self, policy: FoldPolicy) -> Self {
        self.slot_width = match policy {
            FoldPolicy::Measured => None,
            FoldPolicy::Uniform(px) => Some(FinitePositive::new_or(px, DEFAULT_SLOT_WIDTH).get()),
        };
        self
    }

    pub(crate) fn force_all(mut self, force: bool) -> Self {
        self.forced_budget = force.then_some(0);
        self
    }

    /// Set a numeric inline gap in logical pixels, overriding [`Self::spacing`].
    ///
    /// Non-finite or negative input becomes zero.
    pub fn gap(mut self, px: impl Into<f64>) -> Self {
        self.custom_gap = Some(FiniteNonNeg::new_or(px.into() as f32, 0.0).get());
        self
    }

    /// Set labelled-collector width in logical pixels.
    ///
    /// Non-finite or non-positive input uses [`DEFAULT_TRIGGER_WIDTH`]. When unset,
    /// a nonempty label uses natural width; an empty label uses the default.
    /// Kebab collectors use toolbar-icon geometry and ignore this setting.
    pub fn trigger_width(mut self, px: f32) -> Self {
        self.trigger_width = Some(FinitePositive::new_or(px, DEFAULT_TRIGGER_WIDTH).get());
        self
    }

    /// Name the overflow trigger and its menu for assistive technology, in
    /// every collector form (default: the localized "More actions"). Set it
    /// when the collector stands for a specific menu, such as an import
    /// icon, so it is not read as a second generic overflow.
    pub fn trigger_label(mut self, label: impl Into<String>) -> Self {
        self.trigger_label = Some(label.into());
        self
    }

    /// Override the overflow trigger's hover delay; zero starts revealing immediately.
    pub fn tooltip_delay(mut self, delay: std::time::Duration) -> Self {
        self.tooltip_delay = Some(delay);
        self
    }

    /// Hover tooltip for the overflow trigger, in every collector form
    /// (default: its accessible name for an icon-only trigger, none when labelled).
    /// Empty text suppresses the hint. It does not change the accessible name; set
    /// [`Self::trigger_label`] for that.
    pub fn trigger_tooltip(mut self, tip: impl Into<String>) -> Self {
        self.trigger_tooltip = Some(tip.into());
        self
    }

    /// Select framework-owned collector chrome.
    pub fn collector(mut self, collector: Collector) -> Self {
        self.collector = collector;
        self
    }

    /// Place the collector inline, pin it alone, or pin the whole surviving
    /// action cluster to the slot's trailing edge.
    pub fn collector_placement(mut self, placement: CollectorPlacement) -> Self {
        self.collector_placement = placement;
        self
    }

    /// Select whether collector visibility follows overflow automatically or
    /// remains stable even when the current plan has no overflow.
    pub fn collector_visibility(mut self, visibility: CollectorVisibility) -> Self {
        self.collector_visibility = visibility;
        self
    }

    /// Cap the number of foldable commands that may remain inline. Pinned
    /// commands remain inline and overflow-only commands remain in the menu;
    /// priority still decides which foldable commands survive.
    pub fn max_inline_actions(mut self, limit: usize) -> Self {
        self.max_inline_actions = Some(limit);
        self
    }

    /// Put the overflow menu on liquid glass of `style` (see
    /// [`EnzMenu::material`](crate::menu::EnzMenu::material)).
    pub fn menu_material(mut self, style: enzui_theme::MaterialStyle) -> Self {
        self.menu_material = Some(style);
        self
    }

    /// Set popup width in logical pixels (default [`DEFAULT_MENU_WIDTH`]).
    ///
    /// Non-finite or non-positive input uses the default.
    pub fn menu_width(mut self, px: impl Into<f64>) -> Self {
        self.menu_width = FinitePositive::new_or(px.into() as f32, DEFAULT_MENU_WIDTH).get();
        self.menu_width_pinned = true;
        self
    }

    /// Set trigger-to-popup gap in logical pixels (default [`DEFAULT_POPUP_GAP`]).
    ///
    /// Non-finite or negative input uses the default; zero is allowed.
    pub fn popup_gap(mut self, px: impl Into<f64>) -> Self {
        self.popup_gap = FiniteNonNeg::new_or(px.into() as f32, DEFAULT_POPUP_GAP).get();
        self
    }

    /// Namespace keyed overflow rows.
    pub fn menu_id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.menu_id_prefix = Some(prefix.into());
        self
    }

    /// Sets the idempotent message used to close an open overflow menu.
    ///
    /// Outside press, Escape and row activation publish it — activation after
    /// its row command, so this must set `false`, never toggle. Unset: row
    /// activation publishes only its command; outside/Escape use toggle.
    pub fn on_dismiss(mut self, message: Message) -> Self {
        self.on_dismiss = Some(message);
        self
    }

    /// Whether the overflow menu is currently presented. Caller-owned.
    pub fn overflow_open(mut self, open: bool) -> Self {
        self.overflow_open = open;
        self
    }

    /// Assign the overflow trigger a stable widget ID for routed focus and activation.
    ///
    /// No widget ID is assigned by default. This ID also serves as its focus-return
    /// token; distinct bars need distinct identities.
    pub fn overflow_trigger_id(mut self, id: impl Into<iced::advanced::widget::Id>) -> Self {
        self.overflow_trigger_id = Some(id.into());
        self
    }

    /// Set the inline gap token (default [`SpaceToken::Sm`]), clearing a numeric gap override.
    ///
    /// The token uses its default pixel ladder; runtime theme changes do not rescale it.
    pub fn spacing(mut self, token: SpaceToken) -> Self {
        self.spacing = token;
        self.custom_gap = None;
        self
    }

    /// Align content within the bar's assigned width (default [`Horizontal::Left`]).
    ///
    /// Alignment is applied after folding and does not change the measured budget.
    pub fn align_x(mut self, align: Horizontal) -> Self {
        self.align_x = align;
        self
    }

    /// Take only the content's width while no action can fold by width.
    ///
    /// When every visible action always overflows, the bar is just its
    /// collector and has no fold to decide, so it hugs it: a kebab beside a
    /// refresh button sits next to it instead of at the far end of an empty
    /// `Fill` slot. A bar with an action that can show inline keeps the
    /// `Fill` slot its folding needs.
    pub fn fit_content(mut self) -> Self {
        self.fit_content = true;
        self
    }

    /// Classify a candidate assigned width through this bar's transported
    /// breakpoint scale. This is introspection only; folding remains driven by
    /// measured command geometry.
    #[must_use]
    pub fn size_class(&self, width: f32) -> EnzSizeClass {
        self.breakpoints.classify(width)
    }

    /// Supply the scale used by [`Self::size_class`] and responsive collector chrome.
    ///
    /// Defaults to [`BreakpointScale::default`]. The caller must pass updated values
    /// when its theme scale changes; command folding still uses measured widths.
    pub fn breakpoints(mut self, scale: BreakpointScale) -> Self {
        self.breakpoints = scale;
        self
    }

    /// Consume the builder and produce the widget tree, published as a
    /// toolbar.
    pub fn into_element(self) -> Element<'a, Message> {
        let bar = Element::new(ActionBarWidget {
            actions: self.actions,
            on_toggle_overflow: self.on_toggle_overflow,
            overflow_open: self.overflow_open,
            slot_width: self.slot_width,
            forced_budget: self.forced_budget,
            trigger_width: self.trigger_width,
            trigger_label: self.trigger_label,
            trigger_tooltip: self.trigger_tooltip,
            tooltip_delay: self.tooltip_delay,
            collector: self.collector,
            collector_placement: self.collector_placement,
            collector_visibility: self.collector_visibility,
            max_inline_actions: self.max_inline_actions,
            menu_width: self.menu_width,
            menu_width_pinned: self.menu_width_pinned,
            menu_material: self.menu_material,
            popup_gap: self.popup_gap,
            menu_id_prefix: self.menu_id_prefix,
            on_dismiss: self.on_dismiss,
            gap: self.custom_gap.unwrap_or_else(|| self.spacing.px()),
            breakpoints: self.breakpoints,
            overflow_trigger_id: self.overflow_trigger_id,
            align_x: self.align_x,
            fit_content: self.fit_content,
            content: Element::new(iced::widget::Space::new()),
        });
        crate::semantics_container::SemanticsContainer::new(
            bar,
            enzui_core::semantics::Semantics::new(enzui_core::semantics::Role::Toolbar),
        )
        .into()
    }
}

/// Layout-driven widget produced by [`EnzActionBar::into_element`].
///
/// It measures through the renderer and rebuilds content during layout while
/// retaining presentation and child state in the widget tree.
pub struct ActionBarWidget<'a, Message> {
    actions: Vec<ActionItem<'a, Message>>,
    on_toggle_overflow: Message,
    overflow_open: bool,
    slot_width: Option<f32>,
    forced_budget: Option<usize>,
    trigger_width: Option<f32>,
    trigger_label: Option<String>,
    trigger_tooltip: Option<String>,
    tooltip_delay: Option<std::time::Duration>,
    collector: Collector,
    collector_placement: CollectorPlacement,
    collector_visibility: CollectorVisibility,
    max_inline_actions: Option<usize>,
    menu_width: f32,
    menu_width_pinned: bool,
    menu_material: Option<enzui_theme::MaterialStyle>,
    popup_gap: f32,
    menu_id_prefix: Option<String>,
    on_dismiss: Option<Message>,
    gap: f32,
    breakpoints: BreakpointScale,
    overflow_trigger_id: Option<iced::advanced::widget::Id>,
    align_x: Horizontal,
    fit_content: bool,
    content: Element<'a, Message>,
}

static NEXT_ACTION_BAR_INSTANCE: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(1);

#[derive(Debug, Default)]
struct ActionBarState {
    signature: Vec<String>,
    suppress_stale_open: bool,
    instance: u64,
}

impl<'a, Message> ActionBarWidget<'a, Message> {
    fn content_signature(&self) -> Vec<String> {
        self.actions
            .iter()
            .map(|a| {
                format!(
                    "{}:{}:{:?}",
                    a.descriptor.id, a.descriptor.visible, a.descriptor.policy
                )
            })
            .collect()
    }

    fn width(&self) -> Length {
        let nothing_folds = self.actions.iter().all(|item| {
            !item.descriptor.visible || item.descriptor.policy == OverflowPolicy::AlwaysOverflow
        });
        if self.fit_content && nothing_folds {
            Length::Shrink
        } else {
            Length::Fill
        }
    }
}

impl<'a, Message: Clone + 'a> ActionBarWidget<'a, Message> {
    fn measure(el: &mut Element<'a, Message>, renderer: &iced::Renderer) -> f32 {
        let mut tree = Tree::new(&*el);
        // Populate nested children before measuring flex content.
        tree.diff(&mut *el);
        // Finite probe limits would clamp oversized content and conceal overflow.
        let limits = layout::Limits::new(Size::ZERO, Size::INFINITE)
            .width(Length::Shrink)
            .height(Length::Shrink);
        el.as_widget_mut()
            .measure(&mut tree, renderer, &limits)
            .size()
            .width
    }

    fn focus_return_token(&self, instance: u64) -> iced::advanced::widget::Id {
        if let Some(id) = &self.overflow_trigger_id {
            return id.clone();
        }
        let prefix = self
            .menu_id_prefix
            .as_deref()
            .unwrap_or(DEFAULT_FOCUS_RETURN_PREFIX);
        enzui_core::identity::derive_id(prefix, &format!("{TRIGGER_LABEL}#{instance}"))
    }

    /// The trigger's and its menu's accessible name.
    fn trigger_name(&self) -> String {
        self.trigger_label
            .clone()
            .unwrap_or_else(|| enzui_core::localization::text("enzui-actions-more"))
    }

    fn trigger(
        &self,
        available: f32,
        open: bool,
        focus_return: Option<&iced::advanced::widget::Id>,
    ) -> Element<'a, Message> {
        let compact = match &self.collector {
            Collector::Kebab { .. } => true,
            Collector::LabelledUntilCompact { .. } => {
                self.breakpoints.classify(available) == EnzSizeClass::Compact
            }
            Collector::Labelled { .. } => false,
        };
        if compact {
            let (icon, tokens) = match &self.collector {
                Collector::Kebab { icon, tokens }
                | Collector::LabelledUntilCompact { icon, tokens, .. } => (icon.as_str(), *tokens),
                Collector::Labelled { .. } => unreachable!(),
            };
            let mut button = enz_toolbar_icon(icon, self.on_toggle_overflow.clone())
                .aria_label(self.trigger_name())
                .expanded(open);
            if let Some(delay) = self.tooltip_delay {
                button = button.tooltip_delay(delay);
            }
            if let Some(tip) = self.trigger_tooltip.clone() {
                button = button.tooltip(tip);
            }
            if let Some(tokens) = tokens {
                let mut tokens = tokens.sanitized();
                let floor = enzui_theme::SizingTokens::default().min_hit_target;
                tokens.button_size = tokens.button_size.max(tokens.icon_size).max(floor);
                tokens.slot_height = tokens.slot_height.max(tokens.button_size);
                button = button.tokens(tokens);
            }
            if let Some(id) = self.overflow_trigger_id.clone() {
                button = button.id(id);
            }
            // Measurement passes omit the token to avoid consuming live focus-return records.
            if let Some(token) = focus_return {
                button = button
                    .relinquish_focus_while(open)
                    .reclaim_focus_on_return((!open).then(|| token.clone()));
            }
            return button.into();
        }

        let label = match &self.collector {
            Collector::Labelled { label } | Collector::LabelledUntilCompact { label, .. } => {
                label.as_str()
            }
            Collector::Kebab { .. } => unreachable!(),
        };
        let mut button = enz_button(label, self.on_toggle_overflow.clone())
            .kind(ButtonKind::Secondary)
            .size(ButtonSize::Sm)
            .aria_label(self.trigger_name());
        match (self.trigger_width, label.is_empty()) {
            (Some(px), _) => button = button.width(Length::Fixed(px)),
            (None, true) => button = button.width(Length::Fixed(DEFAULT_TRIGGER_WIDTH)),
            (None, false) => {}
        }
        if label.is_empty() {
            button = button
                .icon("ellipsis")
                .icon_position(ButtonIconPosition::Only);
        }
        if let Some(id) = self.overflow_trigger_id.clone() {
            button = button.id(id);
        }
        // Measurement passes omit the token to avoid consuming live focus-return records.
        if let Some(token) = focus_return {
            button = button
                .relinquish_focus_while(open)
                .reclaim_focus_on_return((!open).then(|| token.clone()));
        }
        if let Some(delay) = self.tooltip_delay {
            button = button.tooltip_delay(delay);
        }
        if let Some(tip) = self.trigger_tooltip.clone() {
            button = button.tooltip(tip);
        }
        button.into()
    }

    fn overflow_popup(
        &self,
        plan: &Plan,
        menu_open: bool,
        uses_return_seam: bool,
        return_token: &iced::advanced::widget::Id,
    ) -> Option<Element<'a, Message>> {
        let mut menu = enz_menu()
            .label(self.trigger_name())
            .width(self.overflow_menu_width(plan));
        if let Some(style) = self.menu_material {
            menu = menu.material(style);
        }
        if let Some(prefix) = &self.menu_id_prefix {
            menu = menu.id_prefix(prefix.clone());
        }
        if menu_open && uses_return_seam {
            menu = menu.roving_self(true).focus_return_to(return_token.clone());
        }
        if menu_open {
            let mut previous_group: Option<&str> = None;
            for d in &plan.overflow {
                if let Some(item) = self.actions.iter().find(|a| a.descriptor.id == d.id) {
                    if let Some(command) = &item.command {
                        let group = command.group.as_deref();
                        if previous_group.is_some() && group != previous_group {
                            menu = menu.push_item(EnzMenuItem::separator(EntryId::from(format!(
                                "{}-group",
                                d.id
                            ))));
                        }
                        previous_group = group;
                        let mut row = EnzMenuItem::command(
                            EntryId::from(d.id.clone()),
                            command.label.clone(),
                            command.message.clone(),
                        )
                        .enabled(command.enabled)
                        .destructive(command.destructive);
                        if let Some(icon) = command.icon {
                            row = row.icon(icon);
                        }
                        if let Some(shortcut) = &command.shortcut {
                            row = row.shortcut(shortcut.clone());
                        }
                        if let Some(check) = command.check {
                            row = row.check(check);
                        }
                        menu = menu.push_item(row);
                    } else {
                        menu = menu.push((item.build)(d.presentation));
                    }
                }
            }
        }
        let activation_dismiss = self.on_dismiss.clone();
        menu_open.then(|| match activation_dismiss {
            Some(close) => dismiss_on_child_activation(menu.into_element(), close),
            None => menu.into_element(),
        })
    }

    /// The overflow menu's width: the pinned width, or wide enough for its
    /// longest label and shortcut, from [`DEFAULT_MENU_WIDTH`] up to
    /// [`MAX_AUTO_MENU_WIDTH`]. A fixed 240 px cut "Open network stream
    /// Ctrl+U" to "Open network s…".
    fn overflow_menu_width(&self, plan: &Plan) -> f32 {
        if self.menu_width_pinned {
            return self.menu_width;
        }
        let typo = crate::text_measure::MeasureTypography::new(crate::text::TextSize::Body);
        let measure = |t: &str| crate::text_measure::enz_text_measure_width(t, typo);
        // Icon slot, check slot, row padding and the label-to-shortcut gap.
        const ROW_CHROME: f32 = 20.0 + 20.0 + 2.0 * 14.0 + 24.0;
        plan.overflow
            .iter()
            .filter_map(|d| self.actions.iter().find(|a| a.descriptor.id == d.id))
            .filter_map(|item| item.command.as_ref())
            .map(|c| measure(&c.label) + c.shortcut.as_deref().map_or(0.0, measure) + ROW_CHROME)
            .fold(DEFAULT_MENU_WIDTH, f32::max)
            .min(MAX_AUTO_MENU_WIDTH)
            .ceil()
    }

    fn collector_always(&self) -> bool {
        self.collector_visibility == CollectorVisibility::Always
    }

    fn limit_plan(&self, descriptors: &[Descriptor], plan: Plan) -> Plan {
        let Some(limit) = self.max_inline_actions else {
            return plan;
        };
        let foldable_inline = plan
            .inline
            .iter()
            .filter(|d| d.policy != OverflowPolicy::Pinned)
            .count();
        if foldable_inline <= limit {
            plan
        } else {
            let pinned = plan
                .inline
                .iter()
                .filter(|d| d.policy == OverflowPolicy::Pinned)
                .count();
            resolve(descriptors, pinned + limit)
        }
    }

    fn plan_measured(&self, renderer: &iced::Renderer, available: f32) -> Plan {
        let descriptors: Vec<Descriptor> =
            self.actions.iter().map(|a| a.descriptor.clone()).collect();

        if let Some(budget) = self.forced_budget {
            let plan = resolve(&descriptors, budget);
            return self.limit_plan(&descriptors, plan);
        }

        if let Some(slot) = self.slot_width {
            let n = self
                .actions
                .iter()
                .filter(|a| {
                    a.descriptor.visible && a.descriptor.policy != OverflowPolicy::AlwaysOverflow
                })
                .count();
            if self.collector_placement == CollectorPlacement::Inline {
                let slots = slots_for_width(available + self.gap, slot + self.gap);
                let budget = if self.collector_always() {
                    slots.saturating_sub(1).min(n)
                } else {
                    inline_budget(slots, n)
                };
                let plan = resolve(&descriptors, budget);
                return self.limit_plan(&descriptors, plan);
            }
            let widths: Vec<_> = self
                .actions
                .iter()
                .filter(|item| {
                    item.descriptor.visible
                        && item.descriptor.policy != OverflowPolicy::AlwaysOverflow
                })
                .map(|item| (item.descriptor.id.clone(), slot, None))
                .collect();
            debug_assert_eq!(widths.len(), n, "one measured width per action");
            let mut trig = self.trigger(available, false, None);
            let trigger_w = Self::measure(&mut trig, renderer);
            let plan = resolve_fitting(
                &descriptors,
                &widths,
                available,
                self.gap,
                trigger_w,
                0.0,
                self.collector_always(),
            );
            return self.limit_plan(&descriptors, plan);
        }

        let mut widths: Vec<(String, f32, Option<f32>)> = Vec::new();
        for a in &self.actions {
            if a.descriptor.visible && a.descriptor.policy != OverflowPolicy::AlwaysOverflow {
                let expanded_presentation = if a.compact_inline {
                    Presentation::Expanded
                } else {
                    a.descriptor.presentation
                };
                let mut expanded = (a.build)(expanded_presentation);
                let expanded = Self::measure(&mut expanded, renderer);
                let compact = if a.compact_inline {
                    let mut compact = (a.build)(Presentation::Compact);
                    Some(Self::measure(&mut compact, renderer))
                } else {
                    None
                };
                widths.push((a.descriptor.id.clone(), expanded, compact));
            }
        }

        let mut trig = self.trigger(available, false, None);
        let trigger_w = Self::measure(&mut trig, renderer);

        if self.collector_placement == CollectorPlacement::Inline {
            let plain_widths: Vec<_> = widths.iter().map(|(_, width, _)| *width).collect();
            let budget = if self.collector_always() {
                measured_budget(
                    &plain_widths,
                    (available - trigger_w - self.gap).max(0.0),
                    self.gap,
                    0.0,
                )
            } else {
                measured_budget(&plain_widths, available, self.gap, trigger_w)
            };
            let plan = resolve(&descriptors, budget);
            return self.limit_plan(&descriptors, plan);
        }
        let plan = resolve_fitting(
            &descriptors,
            &widths,
            available,
            self.gap,
            trigger_w,
            0.0,
            self.collector_always(),
        );
        self.limit_plan(&descriptors, plan)
    }

    /// The routable id of the inline command `descriptor_id`.
    fn command_id(&self, descriptor_id: &str) -> iced::advanced::widget::Id {
        enzui_core::identity::OwnerId::of_kind(
            COMMAND_ID_KIND,
            self.menu_id_prefix.as_deref().unwrap_or(""),
        )
        .id(descriptor_id)
    }

    fn build_inline(
        &self,
        item: &ActionItem<'a, Message>,
        presentation: Presentation,
    ) -> (Element<'a, Message>, bool) {
        let element = match &item.identified {
            Some(identified) => identified(presentation, self.command_id(&item.descriptor.id)),
            None => (item.build)(presentation),
        };
        let declared = element.as_widget().size();
        let owns_cross_axis = match declared.height {
            Length::Shrink | Length::Fit => false,
            Length::Fixed(height) => {
                height.is_finite() && height >= crate::bounds_probe::SPILL_EPSILON
            }
            _ => true,
        };
        // Full-band composites own residual width; shrink-height inputs remain ordinary controls.
        let owns_residual =
            self.slot_width.is_none() && declared.width.fill_factor() > 0 && owns_cross_axis;
        let element = match self.slot_width {
            Some(slot) => iced::widget::container(element)
                .width(Length::Fixed(slot))
                .center_x(Length::Fixed(slot))
                .into(),
            None => element,
        };
        (hug_cross_fluid(element, Axis::Horizontal), owns_residual)
    }

    fn build_owned(
        &self,
        available: f32,
        overflow_open: bool,
        instance: u64,
        plan: &Plan,
    ) -> Option<Element<'a, Message>> {
        use crate::item_strip::{Key, Row};
        use layout::action_strip as owned;
        // Responsive collector chrome uses the row path rather than a fixed material inventory.
        if matches!(&self.collector, Collector::LabelledUntilCompact { .. }) {
            return None;
        }
        let mut ids = std::collections::BTreeSet::new();
        if self
            .actions
            .iter()
            .any(|item| !ids.insert(item.descriptor.id.as_str()))
        {
            return None;
        }
        let commands: Vec<_> = self
            .actions
            .iter()
            .map(|item| owned::Command::from(&item.descriptor))
            .collect();
        let mut variants = Vec::with_capacity(self.actions.len());
        let mut material = Vec::new();
        let mut keys = Vec::new();
        for item in &self.actions {
            if !item.descriptor.visible || item.descriptor.policy == OverflowPolicy::AlwaysOverflow
            {
                variants.push(None);
                continue;
            }
            let presentation = if item.compact_inline {
                Presentation::Expanded
            } else {
                item.descriptor.presentation
            };
            let (element, owns_residual) = self.build_inline(item, presentation);
            let expanded = material.len();
            material.push(element);
            keys.push(Key::Action(
                item.descriptor.id.clone(),
                presentation == Presentation::Compact,
            ));
            let compact = if item.compact_inline {
                let (element, _) = self.build_inline(item, Presentation::Compact);
                let index = material.len();
                material.push(element);
                keys.push(Key::Action(item.descriptor.id.clone(), true));
                Some(index)
            } else {
                None
            };
            variants.push(Some(owned::Variants {
                expanded,
                compact,
                owns_residual,
            }));
        }
        let needs_collector = self.collector_always()
            || commands
                .iter()
                .any(|command| command.visible && command.placement != owned::Placement::Pinned);
        let collector = if needs_collector {
            let visible = !plan.overflow.is_empty() || self.collector_always();
            // Opaque rows keep their own keyboard activation instead of keyed-menu roving focus.
            let uses_return_seam = plan.overflow.iter().all(|descriptor| {
                self.actions
                    .iter()
                    .find(|item| item.descriptor.id == descriptor.id)
                    .is_some_and(|item| item.command.is_some())
            });
            let token = self.focus_return_token(instance);
            let menu_open = overflow_open && !plan.overflow.is_empty();
            let trigger = self.trigger(
                available,
                menu_open,
                (visible && uses_return_seam).then_some(&token),
            );
            let popup = self.overflow_popup(plan, menu_open, uses_return_seam, &token);
            let dismiss = self
                .on_dismiss
                .clone()
                .unwrap_or_else(|| self.on_toggle_overflow.clone());
            let trigger = dismissible_anchored_menu(
                trigger,
                popup,
                Placement::new(Side::Bottom)
                    .gap(self.popup_gap)
                    .alignment(PlacementAlignment::End),
                move || dismiss.clone(),
            );
            let index = material.len();
            material.push(trigger);
            keys.push(Key::ActionCollector);
            Some(index)
        } else {
            None
        };
        let policy = owned::Specification {
            commands,
            variants,
            collector,
            sizing: material
                .iter()
                .map(|child| child.as_widget().size())
                .collect(),
            gap: self.gap,
            slot_width: self.slot_width,
            forced_budget: self.forced_budget,
            inline_limit: self.max_inline_actions,
            collector_always: self.collector_always(),
            placement: match self.collector_placement {
                CollectorPlacement::Inline => owned::CollectorPlacement::Inline,
                CollectorPlacement::Trailing => owned::CollectorPlacement::Trailing,
                CollectorPlacement::TrailingCluster => owned::CollectorPlacement::TrailingCluster,
            },
            align: Alignment::from(self.align_x),
        };
        policy.valid().then(|| {
            Element::new(Row::keyed(
                layout::item_strip::Policy::Actions(policy),
                keys,
                material,
            ))
        })
    }

    fn build(
        &self,
        available: f32,
        overflow_open: bool,
        instance: u64,
        plan: &Plan,
    ) -> Element<'a, Message> {
        let mut inline_row: Vec<Element<'a, Message>> = Vec::new();
        let mut inline_owns_residual = false;
        for d in &plan.inline {
            if let Some(item) = self.actions.iter().find(|a| a.descriptor.id == d.id) {
                let (element, owns_residual) = self.build_inline(item, d.presentation);
                inline_owns_residual |= owns_residual;
                inline_row.push(element);
            }
        }

        // The gap separates surviving inline commands from the collector; a
        // collector alone has nothing to keep apart. The empty row stays so
        // the collector keeps its place in the tree as commands fold.
        let gap = if inline_row.is_empty() { 0.0 } else { self.gap };
        let mut children: Vec<Element<'a, Message>> = Vec::new();
        let mut inline = row(inline_row).spacing(self.gap).align_y(Alignment::Center);
        if self.collector_placement == CollectorPlacement::TrailingCluster && inline_owns_residual {
            inline = inline.width(Length::Fill);
        }
        children.push(inline.into());
        let has_overflow = !plan.overflow.is_empty();
        let show_collector = has_overflow || self.collector_always();
        let menu_open = overflow_open && has_overflow;

        if show_collector {
            // Opaque rows keep their own keyboard activation instead of keyed-menu roving focus.
            let uses_return_seam = plan.overflow.iter().all(|d| {
                self.actions
                    .iter()
                    .find(|a| a.descriptor.id == d.id)
                    .is_some_and(|item| item.command.is_some())
            });
            let return_token = self.focus_return_token(instance);
            let popup = self.overflow_popup(plan, menu_open, uses_return_seam, &return_token);
            let dismiss = self
                .on_dismiss
                .clone()
                .unwrap_or_else(|| self.on_toggle_overflow.clone());
            children.push(dismissible_anchored_menu(
                self.trigger(
                    available,
                    menu_open,
                    uses_return_seam.then_some(&return_token),
                ),
                popup,
                Placement::new(Side::Bottom)
                    .gap(self.popup_gap)
                    .alignment(PlacementAlignment::End),
                move || dismiss.clone(),
            ));
        }

        match self.collector_placement {
            CollectorPlacement::Inline => {
                row(children).spacing(gap).align_y(Alignment::Center).into()
            }
            CollectorPlacement::Trailing if children.len() > 1 => {
                let collector = children.pop().expect("collector exists");
                let inline = children.pop().expect("inline row exists");
                row![
                    inline,
                    iced::widget::Space::new().width(Length::Fill),
                    collector
                ]
                .align_y(Alignment::Center)
                .width(Length::Fill)
                .into()
            }
            CollectorPlacement::Trailing => {
                row(children).spacing(gap).align_y(Alignment::Center).into()
            }
            CollectorPlacement::TrailingCluster if inline_owns_residual => {
                // A second Fill owner would divide the composite's residual width.
                row(children)
                    .spacing(gap)
                    .align_y(Alignment::Center)
                    .width(Length::Fill)
                    .into()
            }
            CollectorPlacement::TrailingCluster => row![
                iced::widget::Space::new().width(Length::Fill),
                row(children).spacing(gap).align_y(Alignment::Center)
            ]
            .align_y(Alignment::Center)
            .width(Length::Fill)
            .into(),
        }
    }
}

impl<'a, Message: Clone + 'a> From<EnzActionBar<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzActionBar<'a, Message>) -> Self {
        w.into_element()
    }
}

// Observe child messages to append dismissal only after an activation publishes.
fn dismiss_on_child_activation<'a, Message: Clone + 'a>(
    child: Element<'a, Message>,
    on_dismiss: Message,
) -> Element<'a, Message> {
    Element::new(DismissOnChildActivation {
        child,
        on_dismiss,
        routed: false,
    })
}

fn is_activation_event(event: &Event) -> bool {
    match event {
        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
        | Event::Touch(iced::touch::Event::FingerLifted { .. }) => true,
        Event::Keyboard(kb @ iced::keyboard::Event::KeyPressed { key, .. }) => {
            matches!(
                key,
                iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter),
            ) && enzui_core::focus::activation_from_key(kb)
        }
        Event::Keyboard(iced::keyboard::Event::KeyReleased { key, .. }) => matches!(
            key,
            iced::keyboard::Key::Named(iced::keyboard::key::Named::Space),
        ),
        _ => false,
    }
}

struct DismissOnChildActivation<'a, Message> {
    child: Element<'a, Message>,
    on_dismiss: Message,
    /// A routed activation armed a row; the row publishes on a later pass,
    /// which is then an activation too.
    routed: bool,
}

/// Forwards an operation, noting whether it armed an [`ActivationRequest`]
/// below it.
///
/// [`ActivationRequest`]: crate::activation::ActivationRequest
struct ArmWatch<'a, T> {
    inner: &'a mut dyn Operation<T>,
    armed: &'a mut bool,
}

impl<T> Operation<T> for ArmWatch<'_, T> {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        let armed = &mut *self.armed;
        self.inner.traverse(&mut |inner| {
            operate(&mut ArmWatch {
                inner,
                armed: &mut *armed,
            });
        });
    }
    fn is_native_reconciliation(&self) -> bool {
        self.inner.is_native_reconciliation()
    }
    fn is_docked_clock_rebase(&self) -> bool {
        self.inner.is_docked_clock_rebase()
    }
    fn docked_clock(&mut self, clock: &mut dyn iced::advanced::widget::operation::DockedClock) {
        self.inner.docked_clock(clock);
    }
    fn projection_applied(&mut self, root: iced::advanced::renderer::NodeId) {
        self.inner.projection_applied(root);
    }
    fn needs_editor_reconciliation(&self, node: iced::advanced::renderer::NodeId) -> bool {
        self.inner.needs_editor_reconciliation(node)
    }
    fn editor_reconciled(&mut self, node: iced::advanced::renderer::NodeId, success: bool) {
        self.inner.editor_reconciled(node, success);
    }
    fn needs_scroll_reconciliation(&self, node: iced::advanced::renderer::NodeId) -> bool {
        self.inner.needs_scroll_reconciliation(node)
    }
    fn scroll_reconciled(&mut self, node: iced::advanced::renderer::NodeId, success: bool) {
        self.inner.scroll_reconciled(node, success);
    }
    fn container(
        &mut self,
        id: Option<&iced::advanced::widget::Id>,
        bounds: Rectangle,
        viewport: &Rectangle,
    ) {
        self.inner.container(id, bounds, viewport);
    }
    fn scrollable(
        &mut self,
        id: Option<&iced::advanced::widget::Id>,
        bounds: Rectangle,
        content: Size,
        translation: Vector,
        state: &mut dyn iced::advanced::widget::operation::Scrollable,
    ) {
        self.inner
            .scrollable(id, bounds, content, translation, state);
    }
    fn focusable(
        &mut self,
        id: Option<&iced::advanced::widget::Id>,
        bounds: Rectangle,
        state: &mut dyn iced::advanced::widget::operation::Focusable,
    ) {
        self.inner.focusable(id, bounds, state);
    }
    fn text_input(
        &mut self,
        id: Option<&iced::advanced::widget::Id>,
        bounds: Rectangle,
        state: &mut dyn iced::advanced::widget::operation::TextInput,
    ) {
        self.inner.text_input(id, bounds, state);
    }
    fn text(&mut self, id: Option<&iced::advanced::widget::Id>, bounds: Rectangle, text: &str) {
        self.inner.text(id, bounds, text);
    }
    fn selectable_text(
        &mut self,
        id: Option<&iced::advanced::widget::Id>,
        bounds: Rectangle,
        text: &str,
        layout: &dyn iced::advanced::widget::operation::SelectableTextLayout,
    ) {
        self.inner.selectable_text(id, bounds, text, layout);
    }
    fn custom(
        &mut self,
        id: Option<&iced::advanced::widget::Id>,
        bounds: Rectangle,
        state: &mut dyn std::any::Any,
    ) {
        let was = state
            .downcast_ref::<crate::activation::ActivationRequest>()
            .map(crate::activation::ActivationRequest::armed);
        self.inner.custom(id, bounds, state);
        if was == Some(false)
            && state
                .downcast_ref::<crate::activation::ActivationRequest>()
                .is_some_and(crate::activation::ActivationRequest::armed)
        {
            *self.armed = true;
        }
    }
    fn finish(&self) -> iced::advanced::widget::operation::Outcome<T> {
        self.inner.finish()
    }
}

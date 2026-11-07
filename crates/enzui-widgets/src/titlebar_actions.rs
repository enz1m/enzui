//! Measured titlebar commands with a trailing kebab overflow menu.
//!
//! Inline actions stay leading and are centred on the cross axis. Folding,
//! priority, pinned actions and menu dismissal use the shared
//! [`crate::action_bar`] engine. Closed overflow rows are absent from the tree;
//! the focusable trigger provides access to them.
//!
//! The caller owns the menu state via [`EnzTitlebarActions::overflow_open`].
//! [`EnzTitlebarActions::on_dismiss`] supplies an idempotent close message;
//! without it, Escape and outside presses use the toggle message and menu
//! activation publishes only the command.
//!
//! [`EnzTitlebarActions::into_element`] adds no enclosing band chrome. Wrap it
//! in [`crate::titlebar_icon_group`] when composing a standalone cluster, and
//! allocate or clip children taller than the band. Inside [`crate::title_bar`],
//! use [`EnzTitleBar::actions`](crate::title_bar::EnzTitleBar::actions) so folding
//! accounts for window controls and leading chrome. A generic `push` child
//! does not receive that residual width and can overlap the controls.

use enzui_core::Action;
use enzui_core::action_overflow::{Descriptor, Plan, resolve};
use enzui_core::menu_model::CheckState;
use iced::Element;

/// Accessible name of the overflow trigger.
pub const TRIGGER_LABEL: &str = "More actions";

/// Default kebab glyph, shared with the icon-only menu-button trigger.
pub const DEFAULT_TRIGGER_ICON: &str = crate::menu_button::ICON_ONLY_TRIGGER_ICON;

/// Default width of the popover menu.
pub const DEFAULT_MENU_WIDTH: f32 = 220.0;

/// Default gap between the trigger and the popup surface, in px.
pub const DEFAULT_POPUP_GAP: f32 = 4.0;

/// Build an inline action for measurement and rendering.
///
/// The engine may invoke the closure repeatedly; it must not change application state.
pub type ActionFn<'a, Message> = Box<dyn Fn() -> Element<'a, Message> + 'a>;

/// One command with a fold descriptor and inline builder.
///
/// The label and activation message feed the folded menu row.
/// [`ActionItem::with_inline`] takes menu metadata from an [`Action`]; the
/// caller must keep its custom inline builder consistent with that action.
pub struct ActionItem<'a, Message> {
    descriptor: Descriptor,
    label: String,
    build: ActionFn<'a, Message>,
    activate: Message,
    action: Option<Action<Message>>,
    check: Option<CheckState>,
    destructive: bool,
}

impl<'a, Message> ActionItem<'a, Message> {
    /// Create an inline command with a separate menu label and activation message.
    pub fn new(
        descriptor: Descriptor,
        label: impl Into<String>,
        build: impl Fn() -> Element<'a, Message> + 'a,
        activate: Message,
    ) -> Self {
        Self {
            descriptor,
            label: label.into(),
            build: Box::new(build),
            activate,
            action: None,
            check: None,
            destructive: false,
        }
    }

    /// Use an [`Action`] for menu metadata and a custom builder for inline content.
    ///
    /// The builder must use the same command and enabled state; this adapter does
    /// not enforce what its returned widget dispatches.
    ///
    /// # Panics
    ///
    /// Panics when the descriptor ID differs from the action ID.
    pub fn with_inline(
        descriptor: Descriptor,
        action: Action<Message>,
        build: impl Fn() -> Element<'a, Message> + 'a,
    ) -> Self
    where
        Message: Clone,
    {
        assert_eq!(descriptor.id, action.id(), "descriptor/action id mismatch");
        Self {
            descriptor,
            label: action.label().to_owned(),
            build: Box::new(build),
            activate: action.message().clone(),
            action: Some(action),
            check: None,
            destructive: false,
        }
    }

    /// Give the command its checked menu state.
    #[must_use]
    pub fn check(mut self, check: CheckState) -> Self {
        self.check = Some(check);
        self
    }

    /// Mark the command destructive in its menu presentation.
    #[must_use]
    pub fn destructive(mut self, destructive: bool) -> Self {
        self.destructive = destructive;
        self
    }

    /// The descriptor this item carries.
    pub fn descriptor(&self) -> &Descriptor {
        &self.descriptor
    }
}

/// Builder for [`enz_titlebar_actions`].
pub struct EnzTitlebarActions<'a, Message> {
    actions: Vec<ActionItem<'a, Message>>,
    on_toggle_overflow: Message,
    on_dismiss: Option<Message>,
    overflow_open: bool,
    force_compact: bool,
    trigger_icon: String,
    tooltip_delay: Option<std::time::Duration>,
    trigger_tokens: Option<crate::toolbar_icon::ToolbarIconTokens>,
    trigger_id: Option<iced::advanced::widget::Id>,
    menu_width: f32,
    menu_width_pinned: bool,
    menu_id_prefix: Option<String>,
    gap: f32,
    popup_gap: f32,
}

/// Build a command cluster with caller-owned overflow state.
///
/// Trigger activation publishes `on_toggle_overflow`; feed the resulting state
/// back through [`EnzTitlebarActions::overflow_open`].
pub fn enz_titlebar_actions<'a, Message: Clone + 'a>(
    actions: Vec<ActionItem<'a, Message>>,
    on_toggle_overflow: Message,
) -> EnzTitlebarActions<'a, Message> {
    EnzTitlebarActions {
        actions,
        on_toggle_overflow,
        on_dismiss: None,
        overflow_open: false,
        force_compact: false,
        trigger_icon: DEFAULT_TRIGGER_ICON.to_owned(),
        tooltip_delay: None,
        trigger_tokens: None,
        trigger_id: None,
        menu_width: DEFAULT_MENU_WIDTH,
        menu_width_pinned: false,
        menu_id_prefix: None,
        gap: 4.0,
        popup_gap: DEFAULT_POPUP_GAP,
    }
}

impl<'a, Message: Clone + 'a> EnzTitlebarActions<'a, Message> {
    /// Override the overflow trigger's hover delay; zero starts revealing immediately.
    #[must_use]
    pub fn tooltip_delay(mut self, delay: std::time::Duration) -> Self {
        self.tooltip_delay = Some(delay);
        self
    }

    /// Whether the overflow menu is currently open. Caller-owned.
    #[must_use]
    pub fn overflow_open(mut self, open: bool) -> Self {
        self.overflow_open = open;
        self
    }

    /// Force all foldable commands into overflow; pinned commands remain inline.
    ///
    /// Defaults to false, allowing the engine to fold according to measured width.
    #[must_use]
    pub fn force_compact(mut self, compact: bool) -> Self {
        self.force_compact = compact;
        self
    }

    /// Override the kebab glyph (default [`DEFAULT_TRIGGER_ICON`]).
    #[must_use]
    pub fn trigger_icon(mut self, name: impl Into<String>) -> Self {
        self.trigger_icon = name.into();
        self
    }

    /// Sets the idempotent message used to close the overflow menu.
    ///
    /// Row activation publishes its command followed by this message. Escape
    /// and outside press also publish it. When unset, row activation publishes
    /// only its command and the other dismissal routes use the toggle message.
    #[must_use]
    pub fn on_dismiss(mut self, message: Message) -> Self {
        self.on_dismiss = Some(message);
        self
    }

    /// Override toolbar-icon colours and geometry for the overflow trigger.
    ///
    /// Tokens are sanitized. Its button size is at least the icon size and the
    /// default minimum hit target; its slot height is at least that button size.
    #[must_use]
    pub fn trigger_tokens(mut self, tokens: crate::toolbar_icon::ToolbarIconTokens) -> Self {
        self.trigger_tokens = Some(tokens);
        self
    }

    /// Give the trigger a stable widget id.
    #[must_use]
    pub fn trigger_id(mut self, id: impl Into<iced::advanced::widget::Id>) -> Self {
        self.trigger_id = Some(id.into());
        self
    }

    /// Set menu width in logical pixels (default [`DEFAULT_MENU_WIDTH`]).
    ///
    /// Non-finite or non-positive values use the default.
    #[must_use]
    pub fn menu_width(mut self, px: f32) -> Self {
        self.menu_width = enzui_core::metrics::FinitePositive::new_or(px, DEFAULT_MENU_WIDTH).get();
        self.menu_width_pinned = true;
        self
    }

    /// Set the id-prefix for the menu's keyed rows. A menu built without
    /// one is anonymous.
    #[must_use]
    pub fn menu_id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.menu_id_prefix = Some(prefix.into());
        self
    }

    /// Set the inline gap in logical pixels (default 4).
    ///
    /// Non-finite or negative values use the default; zero is allowed.
    #[must_use]
    pub fn gap(mut self, px: f32) -> Self {
        self.gap = enzui_core::metrics::FiniteNonNeg::new_or(px, 4.0).get();
        self
    }

    /// Set the trigger-to-popup gap in logical pixels (default [`DEFAULT_POPUP_GAP`]).
    ///
    /// Non-finite or negative values use the default; zero is allowed.
    #[must_use]
    pub fn popup_gap(mut self, px: f32) -> Self {
        self.popup_gap = enzui_core::metrics::FiniteNonNeg::new_or(px, DEFAULT_POPUP_GAP).get();
        self
    }

    /// Preview the descriptor-only plan without measuring a width.
    ///
    /// With `force_compact`, the inline budget is zero; otherwise it is unlimited.
    /// The rendered element measures its assigned width independently, so this
    /// preview can keep actions inline that the actual bar folds.
    #[must_use]
    pub fn plan(&self) -> Plan {
        let descriptors: Vec<Descriptor> =
            self.actions.iter().map(|a| a.descriptor.clone()).collect();
        let budget = if self.force_compact { 0 } else { usize::MAX };
        resolve(&descriptors, budget)
    }

    /// Whether the descriptor-only [`Self::plan`] has overflow.
    ///
    /// The rendered trigger can also appear when measured width folds an action.
    #[must_use]
    pub fn trigger_visible(&self) -> bool {
        !self.plan().overflow.is_empty()
    }

    /// Build the shared measured action bar with a trailing kebab collector.
    pub fn into_element(self) -> Element<'a, Message> {
        let actions = self
            .actions
            .into_iter()
            .map(|item| {
                let build = item.build;
                let mut adapted = if let Some(action) = item.action {
                    crate::action_bar::ActionItem::with_inline(
                        item.descriptor,
                        action,
                        move |_action, _presentation| build(),
                    )
                } else {
                    crate::action_bar::ActionItem::with_menu_command(
                        item.descriptor,
                        item.label,
                        item.activate,
                        move |_presentation| build(),
                    )
                };
                if let Some(check) = item.check {
                    adapted = adapted.check(check);
                }
                adapted.destructive(item.destructive)
            })
            .collect();

        let collector = crate::action_bar::Collector::Kebab {
            icon: self.trigger_icon,
            tokens: self.trigger_tokens,
        };
        let mut bar = crate::action_bar::enz_action_bar(actions, self.on_toggle_overflow)
            .collector(collector)
            .collector_placement(crate::action_bar::CollectorPlacement::TrailingCluster)
            .overflow_open(self.overflow_open)
            .popup_gap(self.popup_gap)
            .gap(self.gap)
            .force_all(self.force_compact);
        // Only a pinned width is forwarded; otherwise the bar sizes the menu
        // to its labels (a fixed 220 px cut "Open network stream  Ctrl+U").
        if self.menu_width_pinned {
            bar = bar.menu_width(self.menu_width);
        }
        if let Some(delay) = self.tooltip_delay {
            bar = bar.tooltip_delay(delay);
        }
        if let Some(id) = self.trigger_id {
            bar = bar.overflow_trigger_id(id);
        }
        if let Some(prefix) = self.menu_id_prefix {
            bar = bar.menu_id_prefix(prefix);
        }
        if let Some(dismiss) = self.on_dismiss {
            bar = bar.on_dismiss(dismiss);
        }
        bar.into_element()
    }
}

impl<'a, Message: Clone + 'a> From<EnzTitlebarActions<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzTitlebarActions<'a, Message>) -> Self {
        w.into_element()
    }
}

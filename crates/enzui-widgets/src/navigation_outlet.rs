//! Active-route content with a header, controlled back navigation and page transitions.
//!
//! The caller builds the active page and owns its [`RoutePath`] and
//! [`NavigationTransition`]. Back actions publish [`EnzNavigationOutlet::on_pop`];
//! the caller decides whether to change the path. Escape is off by default,
//! and uncaptured edge swipes follow the ambient writing direction.
//!
//! Page child state is retained while the route key is unchanged and reset
//! when it changes. Give simultaneously mounted outlets distinct scopes.

use std::cell::Cell;
use std::time::{Duration, Instant};

use enzui_core::metrics::FinitePositive;
use enzui_core::placement::{Direction, ambient_direction};
use enzui_core::route::{Route, RouteKey, RoutePath, Transition};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size, Vector, touch};

use crate::header_bar::{HeaderAction, HeaderBarStyle, enz_header_bar};
use crate::swipe::{SwipeAxis, SwipeGesture, SwipePhase, SwipePolicy};
mod back_swipe;
mod events;
mod focus;
mod key_capture;
mod tokens;
mod transition;

pub use back_swipe::*;
pub use events::*;
pub use focus::*;
pub(crate) use key_capture::*;
pub use tokens::*;
pub use transition::*;

/// Projects the active route of a [`RoutePath`] with a header and back
/// navigation.
pub struct EnzNavigationOutlet<'a, Message> {
    path: &'a RoutePath,
    content: Element<'a, Message>,
    show_headerbar: bool,
    show_title: bool,
    headerbar_style: HeaderBarStyle,
    can_pop: bool,
    pop_on_escape: bool,
    on_pop: Option<Message>,
    start_actions: Vec<HeaderAction<Message>>,
    end_actions: Vec<HeaderAction<Message>>,
    transition: Option<(&'a NavigationTransition, Instant)>,
    tokens: NavigationOutletTokens,
    scope: String,
}

/// Build an outlet projecting `path`'s active route as `content`.
pub fn enz_navigation_outlet<'a, Message: 'a + Clone>(
    path: &'a RoutePath,
    content: impl Into<Element<'a, Message>>,
) -> EnzNavigationOutlet<'a, Message> {
    EnzNavigationOutlet {
        path,
        content: content.into(),
        show_headerbar: true,
        show_title: true,
        headerbar_style: HeaderBarStyle::Flat,
        can_pop: true,
        pop_on_escape: false,
        on_pop: None,
        start_actions: Vec::new(),
        end_actions: Vec::new(),
        transition: None,
        tokens: NavigationOutletTokens::DEFAULT,
        scope: DEFAULT_OUTLET_SCOPE.to_owned(),
    }
}

impl<'a, Message: 'a + Clone> EnzNavigationOutlet<'a, Message> {
    /// Set the prefix for content and back-control identities.
    /// Defaults to [`DEFAULT_OUTLET_SCOPE`]; callers must choose distinct prefixes
    /// for simultaneously mounted outlets.
    #[must_use]
    pub fn scope(mut self, scope: impl Into<String>) -> Self {
        self.scope = scope.into();
        self
    }

    /// Show the header bar, including its title, actions and back control; defaults to true.
    /// Hiding it leaves content, Escape and edge-swipe handling mounted.
    #[must_use]
    pub fn show_headerbar(mut self, show: bool) -> Self {
        self.show_headerbar = show;
        self
    }

    /// Show the header title; defaults to true. The subtitle is independent.
    #[must_use]
    pub fn show_title(mut self, show: bool) -> Self {
        self.show_title = show;
        self
    }

    /// Choose the header appearance; defaults to [`HeaderBarStyle::Flat`].
    #[must_use]
    pub fn headerbar_style(mut self, style: HeaderBarStyle) -> Self {
        self.headerbar_style = style;
        self
    }

    /// Allow back navigation when the path has a parent; defaults to true.
    #[must_use]
    pub fn can_pop(mut self, can_pop: bool) -> Self {
        self.can_pop = can_pop;
        self
    }

    /// Opt into popping on uncaptured Escape presses; defaults to false.
    /// Repeats are ignored; keyboard modifiers do not suppress this action.
    #[must_use]
    pub fn pop_on_escape(mut self, pop_on_escape: bool) -> Self {
        self.pop_on_escape = pop_on_escape;
        self
    }

    /// Set the message published by back activation; the host must update the path.
    #[must_use]
    pub fn on_pop(mut self, message: Message) -> Self {
        self.on_pop = Some(message);
        self
    }

    /// Actions for the leading edge of the header.
    #[must_use]
    pub fn start_actions(mut self, actions: Vec<HeaderAction<Message>>) -> Self {
        self.start_actions = actions;
        self
    }

    /// Actions for the trailing edge of the header.
    #[must_use]
    pub fn end_actions(mut self, actions: Vec<HeaderAction<Message>>) -> Self {
        self.end_actions = actions;
        self
    }

    /// Sample the host-owned entering-page transition at `now`.
    /// Rebuild the view with later timestamps to advance it; redraws alone do not
    /// resample the transition. Synchronize reduced motion on the transition before
    /// building the view to suppress the initial translated layout.
    #[must_use]
    pub fn transition(mut self, transition: &'a NavigationTransition, now: Instant) -> Self {
        self.transition = Some((transition, now));
        self
    }

    /// Override logical-pixel geometry, replacing negative or non-finite fields
    /// with defaults. Zero is preserved; the gesture adapter uses a 72 px threshold
    /// when the supplied threshold is zero.
    #[must_use]
    pub fn tokens(mut self, tokens: NavigationOutletTokens) -> Self {
        self.tokens = tokens.sanitized();
        self
    }

    /// The active route being projected.
    #[must_use]
    pub fn active(&self) -> &Route {
        self.path.active()
    }

    /// Whether the path and host policy allow a back affordance.
    /// This does not account for a hidden header or a missing pop message.
    #[must_use]
    pub fn back_enabled(&self) -> bool {
        back_enabled(self.path, self.can_pop)
    }

    /// Whether the path, host policy and pop message permit back activation.
    /// This remains true with a hidden header, allowing Escape and edge swipes.
    #[must_use]
    pub fn back_activatable(&self) -> bool {
        self.back_enabled() && self.on_pop.is_some()
    }

    /// Whether uncaptured, non-repeating Escape presses are eligible to publish pop.
    /// Requires the opt-in and [`Self::back_activatable`]; children may capture Escape.
    #[must_use]
    pub fn escape_pops(&self) -> bool {
        self.pop_on_escape && self.back_activatable()
    }

    /// Return the active title, or empty text when title display is disabled.
    /// The header may itself be hidden.
    #[must_use]
    pub fn header_title(&self) -> &str {
        if self.show_title {
            self.path.active().title()
        } else {
            ""
        }
    }

    /// Return the scoped identity attached to the active page's focusable content region.
    #[must_use]
    pub fn content_widget_id(&self) -> iced::advanced::widget::Id {
        route_content_widget_id_in(&self.scope, &self.path.active_key())
    }

    /// This outlet's scope.
    #[must_use]
    pub fn scope_name(&self) -> &str {
        &self.scope
    }

    /// Return the scoped back identity when back is activatable.
    /// The header carries this identity only when [`Self::show_headerbar`] is true;
    /// with a hidden header, the returned identity has no mounted back control.
    #[must_use]
    pub fn back_widget_id(&self) -> Option<iced::advanced::widget::Id> {
        self.back_activatable()
            .then(|| back_widget_id_in(&self.scope, &self.path.active_key()))
    }

    /// Sample the entering-page offset in logical pixels, applying `theme` reduction.
    /// Returns zero without a transition or with reduced motion. This query does not
    /// change the mounted widget; its theme backstop latches during draw and affects
    /// a later layout.
    #[must_use]
    pub fn content_offset(&self, theme: &iced::Theme) -> f32 {
        if crate::theme_lookup::reduced_motion(theme) {
            return 0.0;
        }
        self.raw_content_offset()
    }

    fn raw_content_offset(&self) -> f32 {
        self.transition.map_or(0.0, |(transition, now)| {
            transition.offset(now, self.tokens.transition_distance)
        })
    }

    /// Compose the header, keyed content region and back-event adapters.
    pub fn into_element(self) -> Element<'a, Message> {
        let back_enabled = self.back_enabled();
        let back_activatable = self.back_activatable();
        let title = self.header_title().to_owned();
        let subtitle = self
            .path
            .active()
            .subtitle_text()
            .unwrap_or_default()
            .to_owned();
        let offset = self.raw_content_offset();
        let escape_pops = self.escape_pops();
        let content_id = self.content_widget_id();
        let back_id = self.back_widget_id();
        let route_key = self.path.active_key().key_string();
        let page_name = self.path.active().title().to_owned();
        let back_swipe_edge_width = self.tokens.back_swipe_edge_width;
        let back_swipe_threshold = self.tokens.back_swipe_threshold;

        let Self {
            content,
            show_headerbar,
            show_title,
            headerbar_style,
            pop_on_escape,
            on_pop,
            start_actions,
            end_actions,
            ..
        } = self;
        // Keep the wrapper mounted at rest so transitions do not replace the page tree.
        let page: Element<'a, Message> = Slide {
            child: content,
            offset,
            route_key: Some(route_key),
            id: Some(content_id),
            accessible_name: page_name,
        }
        .into();

        let page: Element<'a, Message> = iced::widget::container(page)
            .width(Length::Fill)
            .height(Length::Fill)
            .clip(true)
            .into();

        let mut column = iced::widget::column![].spacing(0);

        if show_headerbar {
            let mut bar = enz_header_bar(title)
                .show_back_button(back_enabled)
                .show_title(show_title)
                .style(headerbar_style)
                .start_actions(start_actions)
                .end_actions(end_actions);
            if !subtitle.is_empty() {
                bar = bar.subtitle(subtitle);
            }
            if let Some(message) = on_pop.clone() {
                bar = bar.on_back(message);
            }
            if let Some(id) = back_id {
                bar = bar.back_id(id);
            }
            column = column.push(bar.into_element());
        }

        column = column.push(page);

        let body = iced::widget::container(column)
            .width(Length::Fill)
            .height(Length::Fill)
            .clip(true)
            .style(|theme: &iced::Theme| iced::widget::container::Style {
                background: Some(iced::Background::Color(
                    crate::theme_lookup::palette(theme).background,
                )),
                ..iced::widget::container::Style::default()
            });
        let body_with_swipe: Element<'a, Message> = BackSwipeCapture {
            child: body.into(),
            on_pop: on_pop.clone(),
            back_enabled: back_activatable,
            edge_width: back_swipe_edge_width,
            threshold: back_swipe_threshold,
        }
        .into();
        BackKeyCapture {
            child: body_with_swipe,
            on_pop,
            pop_on_escape,
            back_enabled: escape_pops || back_activatable,
        }
        .into()
    }
}

impl<'a, Message: 'a + Clone> From<EnzNavigationOutlet<'a, Message>> for Element<'a, Message> {
    fn from(outlet: EnzNavigationOutlet<'a, Message>) -> Self {
        outlet.into_element()
    }
}

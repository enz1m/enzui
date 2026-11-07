//! Anchored find/search popover with one navigation model.
//!
//! `find_surface` is the reusable composition around [`crate::search_field`]:
//! a query, one match count, previous/next controls, standard keyboard routing,
//! and dismissal through [`crate::anchored_overlay::dismissible_anchored_menu`].
//! The popup is an iced overlay, so opening it never inserts a row into the
//! host layout.
//!
//! # Ownership and highlighting
//!
//! The caller owns the ordered match collection, including each match's paint
//! coordinates. [`FindSurfaceState`] deliberately stores no `Vec<Match>`;
//! it stores only the active match's stable key. Pass the same ordered key
//! slice to [`enz_find_surface`] and to
//! [`FindSurfaceState::active_match`]. The former derives `N of M`; the latter
//! tells the host which of its own coordinates to paint as active. Appending
//! streaming content therefore cannot move the selection to another match.
//!
//! The composition does not search content and does not own replace, regular
//! expressions, case sensitivity, or whole-word policy. Those change how a
//! host produces its match collection; they are not universally appropriate
//! presentation controls. A host may put such policy outside this focused
//! cluster without forking navigation or dismissal.
//!
//! # Reducer and focus contract
//!
//! Feed each [`FindSurfaceEvent`] to [`FindSurfaceState::apply`]. After a
//! [`FindSurfaceEvent::Dismissed`], schedule the operation returned by
//! [`focus_restore_operation`] against the same id carried by the invoker.
//! This uses the framework's shared probe-then-focus primitive rather than a
//! document fallback. The field focuses itself on the opening transition.
//!
//! # Announcing the count
//!
//! The count publishes as an
//! [`Announcement::Status`]
//! region — [`Role::Status`](enzui_core::semantics::Role::Status) +
//! [`Live::Polite`](enzui_core::semantics::Live::Polite) — so a
//! screen-reader user hears result-count, navigation-index and
//! `still searching` transitions when the visible text changes.
//!
//! An empty query publishes no name on the region, so opening the popup with
//! nothing typed is silent while the surface stays mounted and navigable.

use crate::anchored_overlay::dismissible_anchored_menu;
use crate::button::{ButtonKind, ButtonSize, enz_button_with};
use crate::card::enz_card;
use crate::keyboard_capture::keyboard_capture_unscoped;
use crate::search_field::enz_search_field;
use crate::semantics_container::SemanticsContainer;
use enzui_core::focus::RestoreFocus;
use enzui_core::live_region::Announcement;
use enzui_core::placement::{Alignment, Placement, Side};
use enzui_core::shortcut::{Modifier, Shortcut};
use iced::advanced::widget::{Id, Operation};
use iced::keyboard::{Key, Modifiers, key::Named};
use iced::widget::{column, row, text};
use iced::{Element, Length};
use std::rc::Rc;

/// Messages emitted by a find surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FindSurfaceEvent {
    /// The standard Ctrl+F shortcut requested the surface.
    Open,
    /// Escape or a completed outside click dismissed the surface.
    Dismissed,
    /// The query changed. The caller should rebuild its one match collection.
    QueryChanged(String),
    /// Select the next match, wrapping at the end.
    Next,
    /// Select the previous match, wrapping at the beginning.
    Previous,
}

/// Controlled find state with stable-key navigation and no duplicate matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindSurfaceState<K> {
    open: bool,
    query: String,
    active: Option<K>,
    searching: bool,
}

impl<K> Default for FindSurfaceState<K> {
    fn default() -> Self {
        Self {
            open: false,
            query: String::new(),
            active: None,
            searching: false,
        }
    }
}

impl<K: Clone + Eq> FindSurfaceState<K> {
    /// Create a closed, empty find surface.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the anchored popup is open.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open
    }

    /// Current query text.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Whether the caller is still producing matches for this query.
    #[must_use]
    pub const fn is_searching(&self) -> bool {
        self.searching
    }

    /// Mark asynchronous matching in progress or complete.
    pub fn set_searching(&mut self, searching: bool) {
        self.searching = searching;
    }

    /// Clear query and navigation state without closing the surface.
    pub fn clear(&mut self) {
        self.query.clear();
        self.active = None;
        self.searching = false;
    }

    /// Apply one emitted event against the caller-owned ordered match keys.
    ///
    /// `QueryChanged` starts the "still searching" announcement. A synchronous
    /// host should rebuild its matches and immediately call
    /// `set_searching(false)`; an asynchronous host does so when its final
    /// result batch arrives.
    pub fn apply(&mut self, event: &FindSurfaceEvent, matches: &[K]) {
        match event {
            FindSurfaceEvent::Open => self.open = true,
            FindSurfaceEvent::Dismissed => self.open = false,
            FindSurfaceEvent::QueryChanged(query) => {
                self.query.clone_from(query);
                self.active = None;
                self.searching = true;
            }
            FindSurfaceEvent::Next => self.select_step(matches, true),
            FindSurfaceEvent::Previous => self.select_step(matches, false),
        }
    }

    /// Reconcile after the caller replaces its match set.
    ///
    /// The current stable key wins wherever it moved. If it disappeared, the
    /// first remaining key becomes active; an empty set clears selection.
    pub fn reconcile(&mut self, matches: &[K]) {
        self.active = self.active_match(matches).cloned();
    }

    /// The active key from the caller-owned slice.
    ///
    /// If no explicit key has been selected yet (or it disappeared), the first
    /// match is the effective selection. The count and host highlighting both
    /// use this same function, so they cannot name different matches.
    #[must_use]
    pub fn active_match<'m>(&self, matches: &'m [K]) -> Option<&'m K> {
        self.active
            .as_ref()
            .and_then(|active| matches.iter().find(|key| *key == active))
            .or_else(|| matches.first())
    }

    /// Zero-based active position in the caller-owned ordered slice.
    #[must_use]
    pub fn active_index(&self, matches: &[K]) -> Option<usize> {
        let active = self.active_match(matches)?;
        matches.iter().position(|key| key == active)
    }

    /// Whether `key` is the active match the host should paint distinctly.
    #[must_use]
    pub fn is_active(&self, matches: &[K], key: &K) -> bool {
        self.active_match(matches)
            .is_some_and(|active| active == key)
    }

    /// User-facing and accessible count text, derived from the same match set.
    #[must_use]
    pub fn count_text(&self, matches: &[K]) -> String {
        let current = self.active_index(matches).map_or(0, |index| index + 1);
        let key = if self.searching {
            "enzui-find-count-searching"
        } else {
            "enzui-find-count"
        };
        enzui_core::localized!(key, "current" => current, "total" => matches.len())
    }

    fn select_step(&mut self, matches: &[K], forwards: bool) {
        if matches.is_empty() {
            self.active = None;
            return;
        }
        let current = self.active_index(matches).unwrap_or(0);
        let next = if forwards {
            (current + 1) % matches.len()
        } else if current == 0 {
            matches.len() - 1
        } else {
            current - 1
        };
        self.active = Some(matches[next].clone());
    }
}

/// Build the anchored find popover around `invoker`.
///
/// `matches` contains only stable keys from the caller's one authoritative
/// match collection. The host keeps coordinates and paints every visible
/// match, using [`FindSurfaceState::is_active`] for the distinct active paint.
pub fn enz_find_surface<'a, K, Message>(
    invoker: impl Into<Element<'a, Message>>,
    state: &'a FindSurfaceState<K>,
    matches: &'a [K],
    on_event: impl Fn(FindSurfaceEvent) -> Message + 'a,
) -> EnzFindSurface<'a, K, Message>
where
    K: Clone + Eq + 'a,
    Message: Clone + 'a,
{
    EnzFindSurface {
        invoker: invoker.into(),
        state,
        matches,
        on_event: Rc::new(on_event),
        placement: Placement::new(Side::Bottom)
            .alignment(Alignment::End)
            .gap(6.0),
        field_id: None,
        placeholder: enzui_core::localization::text("enzui-find-surface-find"),
        width: 340.0,
    }
}

/// Builder returned by [`enz_find_surface`].
pub struct EnzFindSurface<'a, K, Message> {
    invoker: Element<'a, Message>,
    state: &'a FindSurfaceState<K>,
    matches: &'a [K],
    on_event: Rc<dyn Fn(FindSurfaceEvent) -> Message + 'a>,
    placement: Placement,
    field_id: Option<Id>,
    placeholder: String,
    width: f32,
}

impl<'a, K, Message> EnzFindSurface<'a, K, Message>
where
    K: Clone + Eq + 'a,
    Message: Clone + 'a,
{
    /// Override popup placement relative to the invoker.
    #[must_use]
    pub fn placement(mut self, placement: Placement) -> Self {
        self.placement = placement;
        self
    }

    /// Give the query field a stable routing id.
    #[must_use]
    pub fn field_id(mut self, id: Id) -> Self {
        self.field_id = Some(id);
        self
    }

    /// Override the field's visible and accessible name (default `"Find"`).
    #[must_use]
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Build the overlay composition.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        let event = Rc::clone(&self.on_event);
        let popup = self.state.is_open().then(|| {
            let query_event = Rc::clone(&event);
            let mut field = enz_search_field(self.state.query(), move |query| {
                query_event(FindSurfaceEvent::QueryChanged(query))
            })
            .placeholder(self.placeholder)
            .responsive(false)
            .focus_on_mount(true)
            .width(Length::Fill);
            if let Some(id) = self.field_id {
                field = field.id(id);
            }

            // Empty query passes an empty name so `Announcement::region`
            // publishes no label and the adapter's insertion gate stays silent.
            let count = self.state.count_text(self.matches);
            let announcement = if self.state.query().is_empty() {
                String::new()
            } else {
                count.clone()
            };
            let count_node: Element<'a, Message> =
                SemanticsContainer::leaf(text(count), Announcement::Status.region(announcement))
                    .into();
            let previous_event = Rc::clone(&event);
            let previous = enz_button_with(
                enzui_core::localization::text("enzui-find-surface-previous"),
                move || previous_event(FindSurfaceEvent::Previous),
            )
            .kind(ButtonKind::Secondary)
            .size(ButtonSize::Sm)
            .enabled(!self.matches.is_empty());
            let next_event = Rc::clone(&event);
            let next = enz_button_with(
                enzui_core::localization::text("enzui-find-surface-next"),
                move || next_event(FindSurfaceEvent::Next),
            )
            .kind(ButtonKind::Secondary)
            .size(ButtonSize::Sm)
            .enabled(!self.matches.is_empty());
            let controls: Element<'a, Message> = row![count_node, previous, next]
                .spacing(8)
                .align_y(iced::Alignment::Center)
                .into();
            let body: Element<'a, Message> = column![field, controls].spacing(8).into();
            let card: Element<'a, Message> = enz_card(body)
                .padding(12)
                .width_length(Length::Fixed(self.width))
                .into();

            let keys = Rc::clone(&event);
            // Open find owns Enter/Shift+Enter globally, just as its Escape
            // dismissal seam is global. A text input is not a FocusButton, so
            // the scoped keyboard wrapper cannot infer its focus from that
            // private state; the popup's open lifetime is the correct scope.
            keyboard_capture_unscoped(card, move |key, modifiers| {
                find_step_for_key(key, modifiers).map(|step| keys(step))
            })
        });

        let dismiss = Rc::clone(&event);
        let anchored = dismissible_anchored_menu(self.invoker, popup, self.placement, move || {
            dismiss(FindSurfaceEvent::Dismissed)
        });
        keyboard_capture_unscoped(anchored, move |key, modifiers| {
            ctrl_f_matches(key, modifiers).then(|| event(FindSurfaceEvent::Open))
        })
    }
}

impl<'a, K, Message> From<EnzFindSurface<'a, K, Message>> for Element<'a, Message>
where
    K: Clone + Eq + 'a,
    Message: Clone + 'a,
{
    fn from(surface: EnzFindSurface<'a, K, Message>) -> Self {
        surface.into_element()
    }
}

/// Standard find keyboard routing: Enter advances, Shift+Enter reverses.
#[must_use]
pub fn find_step_for_key(key: &Key, modifiers: Modifiers) -> Option<FindSurfaceEvent> {
    if !matches!(key, Key::Named(Named::Enter)) {
        return None;
    }
    if modifiers == Modifiers::SHIFT {
        Some(FindSurfaceEvent::Previous)
    } else if modifiers.is_empty() {
        Some(FindSurfaceEvent::Next)
    } else {
        None
    }
}

fn ctrl_f_matches(key: &Key, modifiers: Modifiers) -> bool {
    Shortcut::new(Modifier::CTRL, "f").matches(key, modifiers)
}

/// Focus operation for a dismissal, targeting the exact invoker id.
///
/// Queue this operation after applying the dismissal and rebuilding the closed
/// surface. Non-dismissal events and a missing invoker produce no operation.
pub fn focus_restore_operation<T: 'static>(
    event: &FindSurfaceEvent,
    invoker: Option<&Id>,
) -> Option<Box<dyn Operation<T>>> {
    if !matches!(event, FindSurfaceEvent::Dismissed) {
        return None;
    }
    Some(Box::new(RestoreFocus::new(invoker?.clone())))
}

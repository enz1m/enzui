//! Controlled find/replace fields with wrapping action bands and live status.
//!
//! The host owns query, replacement, disclosure and search-policy state. Buttons,
//! field edits and submissions emit [`FindReplaceEvent`] values. Both action bands
//! wrap controls using [`crate::flow`] rather than a window-width breakpoint.
//!
//! Navigation and replacement buttons start enabled; the query requests focus on
//! mount. Button enable flags do not gate field submissions: query Enter emits
//! Next and replacement Enter emits Replace. The host must validate those requests.
//! Status defaults to a polite live region; blank status leaves its name unset.
//! Replacement width defaults to 260 logical pixels with standard text-field
//! chrome. Use [`EnzFindReplaceBar::replacement_width`] with [`Length::Fill`] and
//! [`EnzFindReplaceBar::replacement_chrome`] with [`ReplacementFieldChrome::Search`]
//! for a full-width input matching Find's fill and border roles.
//!
//! Conditionally mount the bar while open. Its unscoped keyboard handler emits
//! Close for unmodified Escape when children leave the event uncaptured.

use crate::button::{ButtonKind, ButtonSize, enz_button};
use crate::enz_text::{TextSize, enz_text};
use crate::flow::enz_flow;
use crate::keyboard_capture::keyboard_capture_unscoped;
use crate::search_field::enz_search_field;
use crate::semantics_container::SemanticsContainer;
use crate::surface::{RadiusSize, SurfaceTone, enz_surface};
use crate::surface_stack::SpaceToken;
use crate::tokens::FieldVariant;
use crate::toolbar_icon::enz_toolbar_icon;
use enzui_core::live_region::Announcement;
use iced::advanced::widget::Id;
use iced::keyboard::{Key, key::Named};
use iced::widget::{column, container, row};
use iced::{Element, Length};
use std::rc::Rc;

/// Fill and border treatment for the replacement input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReplacementFieldChrome {
    /// Standard filled text-field treatment, preserving the existing default.
    #[default]
    Standard,
    /// Match Find's surface fill and field-border roles, retaining input semantics.
    Search,
}

/// Every user action emitted by [`EnzFindReplaceBar`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FindReplaceEvent {
    /// The query field changed.
    QueryChanged(String),
    /// The replacement field changed.
    ReplacementChanged(String),
    /// Select the previous match.
    Previous,
    /// Select the next match.
    Next,
    /// Reveal or hide the replace controls.
    ToggleReplace,
    /// Replace the active match.
    Replace,
    /// Replace every match in the caller-selected scope.
    ReplaceAll,
    /// Close the complete find/replace surface.
    Close,
}

/// Borrow controlled field values and own the caller’s match-summary text.
/// Navigation and replacement buttons start enabled, query mount focus is on,
/// and status uses [`Announcement::Status`]. Blank status has no accessible name.
/// Enable flags affect buttons; hosts still validate field-submit events.
/// Built-in action messages are created during composition; the callback should
/// map events to messages without mutating search state.
pub fn enz_find_replace_bar<'a, Message>(
    query: &'a str,
    replacement: &'a str,
    status: impl Into<String>,
    replace_open: bool,
    on_event: impl Fn(FindReplaceEvent) -> Message + 'a,
) -> EnzFindReplaceBar<'a, Message>
where
    Message: Clone + 'a,
{
    EnzFindReplaceBar {
        query,
        replacement,
        status: status.into(),
        replace_open,
        on_event: Rc::new(on_event),
        option_controls: Vec::new(),
        replace_scope: None,
        query_id: None,
        replacement_id: None,
        replacement_width: Length::Fixed(260.0),
        replacement_chrome: ReplacementFieldChrome::Standard,
        close_id: None,
        can_navigate: true,
        can_replace: true,
        focus_query_on_mount: true,
        announcement: Announcement::Status,
    }
}

/// Build an icon toggle with `label` as its tooltip and accessible name.
/// Active state selects persistent styling and is published as a toggle state.
///
/// The stock icons are `case-sensitive` (Match case), `whole-word` (Whole
/// word) and `regex` (Regular expression).
pub fn enz_find_option<'a, Message: Clone + 'a>(
    icon: &str,
    label: &str,
    active: bool,
    on_toggle: Message,
) -> Element<'a, Message> {
    enz_toolbar_icon(icon, on_toggle)
        .aria_label(label)
        .tooltip(label)
        .active(active)
        .into()
}

/// Builder returned by [`enz_find_replace_bar`].
pub struct EnzFindReplaceBar<'a, Message> {
    query: &'a str,
    replacement: &'a str,
    status: String,
    replace_open: bool,
    on_event: Rc<dyn Fn(FindReplaceEvent) -> Message + 'a>,
    option_controls: Vec<Element<'a, Message>>,
    replace_scope: Option<Element<'a, Message>>,
    query_id: Option<Id>,
    replacement_id: Option<Id>,
    replacement_width: Length,
    replacement_chrome: ReplacementFieldChrome,
    close_id: Option<Id>,
    can_navigate: bool,
    can_replace: bool,
    focus_query_on_mount: bool,
    announcement: Announcement,
}

impl<'a, Message> EnzFindReplaceBar<'a, Message>
where
    Message: Clone + 'a,
{
    /// Append one host-owned search-policy control (case, whole-word, regex),
    /// usually an [`enz_find_option`]. Each control is an independent flow
    /// item and therefore wraps whole.
    #[must_use]
    pub fn option(mut self, control: impl Into<Element<'a, Message>>) -> Self {
        self.option_controls.push(control.into());
        self
    }

    /// Add the host-owned replacement-scope selector.
    #[must_use]
    pub fn replace_scope(mut self, scope: impl Into<Element<'a, Message>>) -> Self {
        self.replace_scope = Some(scope.into());
        self
    }

    /// Stable id for the query input.
    #[must_use]
    pub fn query_id(mut self, id: Id) -> Self {
        self.query_id = Some(id);
        self
    }

    /// Stable id for the replacement input.
    #[must_use]
    pub fn replacement_id(mut self, id: Id) -> Self {
        self.replacement_id = Some(id);
        self
    }

    /// Allocate the replacement input; defaults to 260 logical pixels.
    ///
    /// [`Length::Fill`] uses the available width, wrapping action controls below it.
    #[must_use]
    pub fn replacement_width(mut self, width: Length) -> Self {
        self.replacement_width = width;
        self
    }

    /// Select replacement field chrome; defaults to [`ReplacementFieldChrome::Standard`].
    #[must_use]
    pub fn replacement_chrome(mut self, chrome: ReplacementFieldChrome) -> Self {
        self.replacement_chrome = chrome;
        self
    }

    /// Set a stable identity for close-button focus and action routing.
    #[must_use]
    pub fn close_id(mut self, id: Id) -> Self {
        self.close_id = Some(id);
        self
    }

    /// Enable previous/next buttons; true by default.
    /// Query submission still emits Next while these buttons are disabled.
    #[must_use]
    pub fn can_navigate(mut self, enabled: bool) -> Self {
        self.can_navigate = enabled;
        self
    }

    /// Enable replace and replace-all buttons; true by default.
    /// Replacement submission still emits Replace while these buttons are disabled.
    #[must_use]
    pub fn can_replace(mut self, enabled: bool) -> Self {
        self.can_replace = enabled;
        self
    }

    /// Whether the query claims focus on the opening mount (default `true`).
    #[must_use]
    pub fn focus_query_on_mount(mut self, enabled: bool) -> Self {
        self.focus_query_on_mount = enabled;
        self
    }

    /// Set the status live-region policy; defaults to polite Status.
    /// Alert selects assertive announcements and Silent disables automatic announcements.
    #[must_use]
    pub fn announcement(mut self, announcement: Announcement) -> Self {
        self.announcement = announcement;
        self
    }

    /// Build the responsive surface.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        let emit = Rc::clone(&self.on_event);
        let query_changed = Rc::clone(&emit);
        let mut query = enz_search_field(self.query, move |value| {
            query_changed(FindReplaceEvent::QueryChanged(value))
        })
        .placeholder(enzui_core::localization::text(
            "enzui-find-replace-bar-find",
        ))
        .responsive(false)
        .focus_on_mount(self.focus_query_on_mount)
        .on_submit(emit(FindReplaceEvent::Next))
        .on_clear(emit(FindReplaceEvent::QueryChanged(String::new())))
        .width(Length::Fill);
        if let Some(id) = self.query_id {
            query = query.id(id);
        }

        let mut close = enz_toolbar_icon("action.close", emit(FindReplaceEvent::Close))
            .aria_label(enzui_core::localization::text(
                "enzui-find-replace-bar-close-find-and-replace",
            ))
            .tooltip(enzui_core::localization::text(
                "enzui-find-replace-bar-close",
            ));
        if let Some(id) = self.close_id {
            close = close.id(id);
        }
        let query_line = row![query, close]
            .spacing(SpaceToken::Xs.px())
            .align_y(iced::Alignment::Center)
            .width(Length::Fill);

        let previous = enz_toolbar_icon("navigation.back", emit(FindReplaceEvent::Previous))
            .aria_label(enzui_core::localization::text(
                "enzui-find-replace-bar-previous-match",
            ))
            .tooltip(enzui_core::localization::text(
                "enzui-find-replace-bar-previous-match",
            ))
            .enabled(self.can_navigate);
        let next = enz_toolbar_icon("navigation.forward", emit(FindReplaceEvent::Next))
            .aria_label(enzui_core::localization::text(
                "enzui-find-replace-bar-next-match",
            ))
            .tooltip(enzui_core::localization::text(
                "enzui-find-replace-bar-next-match",
            ))
            .enabled(self.can_navigate);
        let replace_toggle = enz_toolbar_icon("replace", emit(FindReplaceEvent::ToggleReplace))
            .aria_label(if self.replace_open {
                enzui_core::localization::text("enzui-find-replace-bar-hide-replace")
            } else {
                enzui_core::localization::text("enzui-find-replace-bar-show-replace")
            })
            .tooltip(if self.replace_open {
                enzui_core::localization::text("enzui-find-replace-bar-hide-replace")
            } else {
                enzui_core::localization::text("enzui-find-replace-bar-show-replace")
            })
            .expanded(self.replace_open);
        // The leaf region owns the status name and suppresses the text’s semantic child.
        let status_leaf: Element<'a, Message> = SemanticsContainer::leaf(
            enz_text(self.status.clone()).size(TextSize::Footnote),
            self.announcement.region(self.status),
        )
        .into();
        let mut navigation = enz_flow::<Message>()
            .gap(SpaceToken::Xs)
            .cross_gap(SpaceToken::Xs)
            .align_items(iced::Alignment::Center)
            .fill(true)
            .push(status_leaf)
            .push(previous)
            .push(next)
            .push(replace_toggle);
        for option in self.option_controls {
            navigation = navigation.push(option);
        }

        let mut body = column![query_line, navigation]
            .spacing(SpaceToken::Xs.px())
            .width(Length::Fill);

        if self.replace_open {
            let replacement_changed = Rc::clone(&emit);
            let mut replacement = crate::text_field::enz_text_field_owned(
                "",
                enzui_core::localization::text("enzui-find-replace-bar-replace-with"),
                self.replacement,
                move |value| replacement_changed(FindReplaceEvent::ReplacementChanged(value)),
            )
            .aria_label(enzui_core::localization::text(
                "enzui-find-replace-bar-replace-with",
            ))
            .on_submit(emit(FindReplaceEvent::Replace));
            if self.replacement_chrome == ReplacementFieldChrome::Search {
                replacement = replacement
                    .variant(FieldVariant::Outlined)
                    .border_variant(FieldVariant::Filled);
            }
            if let Some(id) = self.replacement_id {
                replacement = replacement.id(id);
            }
            let replacement: Element<'a, Message> =
                container(replacement).width(self.replacement_width).into();
            let replace = enz_button(
                enzui_core::localization::text("enzui-find-replace-bar-replace"),
                emit(FindReplaceEvent::Replace),
            )
            .kind(ButtonKind::Secondary)
            .size(ButtonSize::Sm)
            .enabled(self.can_replace);
            let replace_all = enz_button(
                enzui_core::localization::text("enzui-find-replace-bar-replace-all"),
                emit(FindReplaceEvent::ReplaceAll),
            )
            .kind(ButtonKind::Secondary)
            .size(ButtonSize::Sm)
            .enabled(self.can_replace);
            let mut replace_line = enz_flow::<Message>()
                .gap(SpaceToken::Xs)
                .cross_gap(SpaceToken::Xs)
                .align_items(iced::Alignment::Center)
                .fill(true)
                .push(replacement)
                .push(replace)
                .push(replace_all);
            if let Some(scope) = self.replace_scope {
                replace_line = replace_line.push(scope);
            }
            body = body.push(replace_line);
        }

        let surface: Element<'a, Message> = enz_surface(body)
            .tone(SurfaceTone::SurfaceAlt)
            .radius_size(RadiusSize::None)
            .padding(SpaceToken::Sm.px())
            .width(Length::Fill)
            .into();
        // Native text-input focus is outside FocusButton’s scoped focus markers.
        keyboard_capture_unscoped(surface, move |key, modifiers| {
            (modifiers.is_empty() && matches!(key, Key::Named(Named::Escape)))
                .then(|| emit(FindReplaceEvent::Close))
        })
    }
}

impl<'a, Message> From<EnzFindReplaceBar<'a, Message>> for Element<'a, Message>
where
    Message: Clone + 'a,
{
    fn from(bar: EnzFindReplaceBar<'a, Message>) -> Self {
        bar.into_element()
    }
}

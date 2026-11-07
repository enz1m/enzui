//! A header bar above a caller-owned content element.
//!
//! The header is shown by default; its back button is hidden until enabled.
//! Content fills the remaining area over the live palette background. The
//! transparent-background option changes only that fill. This composite adds
//! neither scrolling nor paint clipping; supply those in the content or parent.
//! Use an ID prefix when sibling views expose the same action IDs.
//!
//! A scrolling body remains caller-owned.

use crate::header_bar::{HeaderAction, HeaderBarStyle, enz_header_bar};
use crate::theme_lookup;
use iced::widget::{column, container};
use iced::{Element, Length};
fn content_background(theme: &iced::Theme, transparent: bool) -> iced::Color {
    if transparent {
        iced::Color::TRANSPARENT
    } else {
        theme_lookup::palette(theme).background
    }
}

/// A screen shell composite: an [`enz_header_bar`] (title, subtitle, and
/// start/end actions) stacked above a content region. Built via
/// [`enz_toolbar_view`].
#[must_use]
pub struct EnzToolbarView<'a, Message> {
    title: String,
    subtitle: Option<String>,
    content: Element<'a, Message>,
    on_back: Message,
    start_actions: Vec<HeaderAction<Message>>,
    end_actions: Vec<HeaderAction<Message>>,
    reveal_top: bool,
    transparent_content_background: bool,
    top_style: HeaderBarStyle,
    show_back_button: bool,
    id_prefix: Option<String>,
}

/// Create an [`EnzToolbarView`] with the given `title` over `content`,
/// emitting `on_back` when the (optional) back button is activated.
pub fn enz_toolbar_view<'a, Message: Clone + 'a>(
    title: impl Into<String>,
    content: impl Into<Element<'a, Message>>,
    on_back: Message,
) -> EnzToolbarView<'a, Message> {
    EnzToolbarView {
        title: title.into(),
        subtitle: None,
        content: content.into(),
        on_back,
        start_actions: Vec::new(),
        end_actions: Vec::new(),
        reveal_top: true,
        transparent_content_background: false,
        top_style: HeaderBarStyle::default(),
        show_back_button: false,
        id_prefix: None,
    }
}

impl<'a, Message: Clone + 'a> EnzToolbarView<'a, Message> {
    /// Set the subtitle shown beneath the title. An empty string clears it.
    pub fn subtitle(mut self, s: impl Into<String>) -> Self {
        let s = s.into();
        self.subtitle = if s.is_empty() { None } else { Some(s) };
        self
    }
    /// Set the header actions shown at the leading (start) edge.
    pub fn start_actions(mut self, actions: Vec<HeaderAction<Message>>) -> Self {
        self.start_actions = actions;
        self
    }
    /// Set the header actions shown at the trailing (end) edge.
    pub fn end_actions(mut self, actions: Vec<HeaderAction<Message>>) -> Self {
        self.end_actions = actions;
        self
    }
    /// Show (`true`, default) or hide the top header bar entirely.
    pub fn reveal_top(mut self, r: bool) -> Self {
        self.reveal_top = r;
        self
    }
    /// Use a transparent body fill when enabled; defaults to the live palette background.
    /// The header remains above the body; this option does not move or overlap content.
    pub fn transparent_content_background(mut self, transparent: bool) -> Self {
        self.transparent_content_background = transparent;
        self
    }
    /// Set the visual style of the top header bar.
    pub fn top_style(mut self, s: HeaderBarStyle) -> Self {
        self.top_style = s;
        self
    }
    /// Show or hide the header back button; defaults to hidden.
    /// When enabled, activation emits the constructor's `on_back` message.
    pub fn show_back_button(mut self, b: bool) -> Self {
        self.show_back_button = b;
        self
    }

    /// Forward an action-ID namespace to the composed header bar.
    /// Use distinct prefixes when views share actions to prevent ID collisions.
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = Some(prefix.into());
        self
    }
    /// Convert into an iced `iced::Element`.
    pub fn into_element(self) -> Element<'a, Message> {
        let mut cols = column![];
        if self.reveal_top {
            let mut header = enz_header_bar(self.title)
                .start_actions(self.start_actions)
                .end_actions(self.end_actions)
                .style(self.top_style)
                .show_back_button(self.show_back_button);
            if let Some(sub) = self.subtitle {
                header = header.subtitle(sub);
            }
            if self.show_back_button {
                header = header.on_back(self.on_back);
            }
            if let Some(p) = self.id_prefix {
                header = header.id_prefix(p);
            }
            cols = cols.push(header);
        } else {
            let _ = self.on_back;
        }
        let transparent = self.transparent_content_background;
        cols = cols.push(
            container(self.content)
                .width(Length::Fill)
                .height(Length::Fill)
                .style(move |theme: &iced::Theme| container::Style {
                    background: Some(content_background(theme, transparent).into()),
                    ..Default::default()
                }),
        );

        container(cols)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

impl<'a, Message: Clone + 'a> From<EnzToolbarView<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzToolbarView<'a, Message>) -> Self {
        w.into_element()
    }
}

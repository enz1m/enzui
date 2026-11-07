//! Stack application-state notices with one live region in a nonempty list.
//!
//! The first error is the live notice, or the first notice when none is an
//! error; the rest are silent. This selects accessibility metadata without
//! retaining announcement history or guaranteeing one spoken utterance.
//! Applications own notice lifetime and removal. Dismiss and action messages
//! do not remove notices automatically. Supply distinct, stable IDs.
//! For transient events, use [`crate::toast_host::ToastHost`].

use crate::toast::{ToastTone, enz_toast, enz_toast_undismissible};
use enzui_core::live_region::Announcement;
use iced::widget::column;
use iced::{Element, Length};

/// One notice in a stack.
#[derive(Debug, Clone)]
pub struct EnzNotice<Message> {
    id: String,
    title: String,
    description: Option<String>,
    tone: ToastTone,
    on_dismiss: Option<Message>,
    action: Option<(String, Message)>,
    secondary: Option<(String, Message)>,
}

impl<Message> EnzNotice<Message> {
    /// Create a notice without a close control; the application owns its lifetime.
    /// Use [`Self::on_dismiss`] to add a close control that publishes a message.
    pub fn new(id: impl Into<String>, title: impl Into<String>, tone: ToastTone) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            description: None,
            tone,
            on_dismiss: None,
            action: None,
            secondary: None,
        }
    }

    /// A muted second line.
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Shows a close control that publishes `message`.
    #[must_use]
    pub fn on_dismiss(mut self, message: Message) -> Self {
        self.on_dismiss = Some(message);
        self
    }

    /// The primary action button.
    #[must_use]
    pub fn action(mut self, label: impl Into<String>, message: Message) -> Self {
        self.action = Some((label.into(), message));
        self
    }

    /// The secondary action button.
    #[must_use]
    pub fn secondary_action(mut self, label: impl Into<String>, message: Message) -> Self {
        self.secondary = Some((label.into(), message));
        self
    }
}

/// A column that designates at most one notice as a live region.
pub struct EnzNoticeStack<Message> {
    notices: Vec<EnzNotice<Message>>,
    spacing: f32,
    width: Length,
}

/// The notices, in order, as a column of toasts.
pub fn enz_notice_stack<Message>(notices: Vec<EnzNotice<Message>>) -> EnzNoticeStack<Message> {
    EnzNoticeStack {
        notices,
        spacing: enzui_theme::SpacingScale::default().xs(),
        width: Length::Shrink,
    }
}

impl<Message> EnzNoticeStack<Message> {
    /// The gap between notices, in logical pixels. A non-finite or negative
    /// gap keeps the current one.
    #[must_use]
    pub fn spacing(mut self, spacing: f32) -> Self {
        self.spacing = enzui_core::metrics::FiniteNonNeg::new_or(spacing, self.spacing).get();
        self
    }

    /// The column's width.
    #[must_use]
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Whether the stack holds no notice.
    pub fn is_empty(&self) -> bool {
        self.notices.is_empty()
    }

    /// The stack as an element.
    pub fn into_element<'a>(self) -> Element<'a, Message>
    where
        Message: Clone + 'a,
    {
        let spoken = self
            .notices
            .iter()
            .position(|notice| notice.tone == ToastTone::Error)
            .unwrap_or(0);
        let toasts = self.notices.into_iter().enumerate().map(|(index, notice)| {
            let mut toast = match notice.on_dismiss {
                Some(message) => enz_toast(notice.title, message),
                None => enz_toast_undismissible(notice.title),
            }
            .tone(notice.tone)
            .show_icon(true)
            .id(notice.id);
            if let Some(description) = notice.description {
                toast = toast.description(description);
            }
            if let Some((label, message)) = notice.action {
                toast = toast.action(label, message);
            }
            if let Some((label, message)) = notice.secondary {
                toast = toast.secondary_action(label, message);
            }
            if index != spoken {
                toast = toast.announcement(Announcement::Silent);
            }
            toast.into_element()
        });
        column(toasts)
            .spacing(self.spacing)
            .width(self.width)
            .into()
    }
}

impl<'a, Message: Clone + 'a> From<EnzNoticeStack<Message>> for Element<'a, Message> {
    fn from(stack: EnzNoticeStack<Message>) -> Self {
        stack.into_element()
    }
}

//! The application's About dialog: icon, name, version, developer, links,
//! release notes, credits and legal text.
//!
//! [`EnzAboutDialog::into_host_with`] presents the dialog over the
//! application's view and reports dismissal, focus moves and the links a user
//! opens ([`AboutDialogEvent::OpenUrl`]); the application closes the dialog and
//! opens the link (for example with `enzui_core::portal::open_uri`). The header
//! (icon, name, developer, version) and the Website and Report-an-Issue buttons
//! stay in view; release notes, credits and legal text scroll below them. Every
//! label is localized.

use crate::button::ButtonKind;
use crate::scroll_view::{enz_scroll_view, key_scroll_forward};
use crate::text::{TextSize, TextTone, enz_text};
use enzui_core::dismissal::DismissPolicy;
use enzui_core::modal::DismissCause;
use iced::widget::{column, container, row};
use iced::{Element, Length};
use std::rc::Rc;

/// What a hosted About dialog reports.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AboutDialogEvent {
    /// Close the dialog, for this cause.
    Dismissed(DismissCause),
    /// Move focus forward inside the dialog.
    FocusNext,
    /// Move focus backward inside the dialog.
    FocusPrevious,
    /// Open this link (the website or the issue tracker).
    OpenUrl(String),
}

impl AboutDialogEvent {
    fn from_host(event: crate::modal_host::ModalHostEvent) -> Self {
        use crate::modal_host::ModalHostEvent;
        match event {
            ModalHostEvent::Dismissed(cause) => Self::Dismissed(cause),
            ModalHostEvent::FocusNext => Self::FocusNext,
            ModalHostEvent::FocusPrevious => Self::FocusPrevious,
        }
    }

    /// The focus move this event asks for, or `None` for other events.
    #[must_use]
    pub fn focus_operation<T>(&self) -> Option<Box<dyn iced::advanced::widget::Operation<T>>>
    where
        T: Send + 'static,
    {
        use crate::modal_host::ModalHostEvent;
        let host_event = match self {
            Self::FocusNext => ModalHostEvent::FocusNext,
            Self::FocusPrevious => ModalHostEvent::FocusPrevious,
            Self::Dismissed(_) | Self::OpenUrl(_) => return None,
        };
        crate::modal_host::focus_wrap_operation(host_event)
    }

    /// The focus move this event asks for, restricted to the presentation
    /// `scope` (configure the same [`EnzAboutDialog::scope_key`]).
    #[must_use]
    pub fn scoped_focus_operation<T>(
        &self,
        scope: iced::advanced::widget::Id,
    ) -> Option<Box<dyn iced::advanced::widget::Operation<T>>>
    where
        T: Send + 'static,
    {
        use crate::modal_host::ModalHostEvent;
        let host_event = match self {
            Self::FocusNext => ModalHostEvent::FocusNext,
            Self::FocusPrevious => ModalHostEvent::FocusPrevious,
            Self::Dismissed(_) | Self::OpenUrl(_) => return None,
        };
        crate::modal_host::scoped_focus_operation(host_event, scope)
    }

    /// Focus back on `invoker` after a keyboard or Close dismissal; `None`
    /// for other events.
    #[must_use]
    pub fn restore_focus_operation<T>(
        &self,
        invoker: Option<&iced::advanced::widget::Id>,
    ) -> Option<Box<dyn iced::advanced::widget::Operation<T>>>
    where
        T: Send + 'static,
    {
        match self {
            Self::Dismissed(cause) => crate::modal_host::restore_focus_operation(
                crate::modal_host::ModalHostEvent::Dismissed(*cause),
                invoker,
            ),
            _ => None,
        }
    }
}

/// Builder for [`enz_about_dialog`].
#[must_use]
pub struct EnzAboutDialog {
    open: bool,
    name: String,
    icon: Option<String>,
    version: Option<String>,
    developer: Option<String>,
    website: Option<String>,
    issue_url: Option<String>,
    release_notes: Option<String>,
    credits: Vec<(String, Vec<String>)>,
    copyright: Option<String>,
    license: Option<String>,
    license_text: Option<String>,
    width: f32,
    max_height: f32,
    scope_key: Option<String>,
}

/// An About dialog for the application called `name`, shown while `open`.
pub fn enz_about_dialog(open: bool, name: impl Into<String>) -> EnzAboutDialog {
    EnzAboutDialog {
        open,
        name: name.into(),
        icon: None,
        version: None,
        developer: None,
        website: None,
        issue_url: None,
        release_notes: None,
        credits: Vec::new(),
        copyright: None,
        license: None,
        license_text: None,
        width: 420.0,
        max_height: 600.0,
        scope_key: None,
    }
}

impl EnzAboutDialog {
    /// The application icon, by icon name.
    pub fn icon(mut self, name: impl Into<String>) -> Self {
        self.icon = Some(name.into());
        self
    }

    /// The version shown under the name.
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    /// The developer or organisation, shown under the name.
    pub fn developer(mut self, developer: impl Into<String>) -> Self {
        self.developer = Some(developer.into());
        self
    }

    /// The website the Website button opens.
    pub fn website(mut self, url: impl Into<String>) -> Self {
        self.website = Some(url.into());
        self
    }

    /// The issue tracker the Report an Issue button opens.
    pub fn issue_url(mut self, url: impl Into<String>) -> Self {
        self.issue_url = Some(url.into());
        self
    }

    /// What is new in this version.
    pub fn release_notes(mut self, notes: impl Into<String>) -> Self {
        self.release_notes = Some(notes.into());
        self
    }

    /// Credit `names` under the localized "Developers" heading.
    pub fn developers(self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let title = enzui_core::localization::text("enzui-about-dialog-developers");
        self.credit_section(title, names)
    }

    /// Credit `names` under the localized "Designers" heading.
    pub fn designers(self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let title = enzui_core::localization::text("enzui-about-dialog-designers");
        self.credit_section(title, names)
    }

    /// Credit `names` under the localized "Artists" heading.
    pub fn artists(self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let title = enzui_core::localization::text("enzui-about-dialog-artists");
        self.credit_section(title, names)
    }

    /// Credit `names` under the localized "Translators" heading.
    pub fn translators(self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let title = enzui_core::localization::text("enzui-about-dialog-translators");
        self.credit_section(title, names)
    }

    /// Credit `names` under the localized "Documenters" heading.
    pub fn documenters(self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let title = enzui_core::localization::text("enzui-about-dialog-documenters");
        self.credit_section(title, names)
    }

    /// Credit `names` under a heading of the application's own.
    pub fn credit_section(
        mut self,
        title: impl Into<String>,
        names: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        let names: Vec<String> = names.into_iter().map(Into::into).collect();
        if !names.is_empty() {
            self.credits.push((title.into(), names));
        }
        self
    }

    /// The copyright line.
    pub fn copyright(mut self, copyright: impl Into<String>) -> Self {
        self.copyright = Some(copyright.into());
        self
    }

    /// The licence's name (an SPDX identifier, for example `GPL-3.0-or-later`).
    pub fn license(mut self, license: impl Into<String>) -> Self {
        self.license = Some(license.into());
        self
    }

    /// The licence's full text, shown under Legal.
    pub fn license_text(mut self, text: impl Into<String>) -> Self {
        self.license_text = Some(text.into());
        self
    }

    /// The dialog's width (default 420 px).
    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// The tallest the dialog grows before its body scrolls (default 600 px).
    pub fn max_height(mut self, height: f32) -> Self {
        self.max_height = height;
        self
    }

    /// Scope the dialog's presentation, so
    /// [`AboutDialogEvent::scoped_focus_operation`] keeps Tab inside it.
    pub fn scope_key(mut self, key: impl Into<String>) -> Self {
        self.scope_key = Some(key.into());
        self
    }

    /// The dialog's accessible name: "About Name".
    #[must_use]
    pub fn title(&self) -> String {
        enzui_core::localized!("enzui-about-dialog-about", "name" => self.name.clone())
    }

    /// Present the dialog over `background`; every event goes through
    /// `on_event`. Escape, the scrim and Close dismiss.
    pub fn into_host_with<'a, Message: Clone + 'a>(
        self,
        background: impl Into<Element<'a, Message>>,
        on_event: impl Fn(AboutDialogEvent) -> Message + 'a,
    ) -> Element<'a, Message> {
        let on_event: Rc<dyn Fn(AboutDialogEvent) -> Message + 'a> = Rc::new(on_event);
        let label = self.title();
        let scope = self
            .scope_key
            .as_deref()
            .map(crate::modal_host::presentation_scope_id);
        let surface = self.open.then(|| self.into_surface(Rc::clone(&on_event)));
        let mut host = crate::modal_host::enz_modal_host(background, surface, move |event| {
            on_event(AboutDialogEvent::from_host(event))
        })
        .policy(DismissPolicy::modal())
        .label(label);
        if let Some(scope) = scope {
            host = host.scope(scope);
        }
        host.into_element()
    }

    /// The dialog card alone: header, links, scrolling sections and Close.
    pub fn into_surface<'a, Message: Clone + 'a>(
        self,
        on_event: Rc<dyn Fn(AboutDialogEvent) -> Message + 'a>,
    ) -> Element<'a, Message> {
        let padding = crate::modal_surface::DEFAULT_DIALOG_PADDING.px();
        let mut header = column![].spacing(6.0).align_x(iced::Alignment::Center);
        if let Some(icon) = &self.icon {
            header = header.push(
                crate::icon::enz_icon(icon)
                    .size(64.0)
                    .into_element::<Message>(),
            );
        }
        header = header.push(
            enz_text(self.name.clone())
                .size(TextSize::Title1)
                .into_element(),
        );
        if let Some(developer) = &self.developer {
            header = header.push(
                enz_text(developer.clone())
                    .tone(TextTone::Muted)
                    .into_element(),
            );
        }
        if let Some(version) = &self.version {
            header = header.push(
                enz_text(enzui_core::localized!(
                    "enzui-about-dialog-version",
                    "version" => version.clone()
                ))
                .size(TextSize::Caption)
                .tone(TextTone::Muted)
                .into_element(),
            );
        }
        let mut links = row![].spacing(8.0);
        for (label, url) in [
            ("enzui-about-dialog-website", &self.website),
            ("enzui-about-dialog-report-issue", &self.issue_url),
        ] {
            if let Some(url) = url {
                let on_event = Rc::clone(&on_event);
                let url = url.clone();
                links = links.push(
                    crate::button::enz_button_with(
                        enzui_core::localization::text(label),
                        move || on_event(AboutDialogEvent::OpenUrl(url.clone())),
                    )
                    .kind(ButtonKind::Secondary),
                );
            }
        }
        let header: Element<'a, Message> = column![
            container(header).width(Length::Fill).center_x(Length::Fill),
            container(links).width(Length::Fill).center_x(Length::Fill),
        ]
        .spacing(16.0)
        .into();

        let section = |title: String, body: Element<'a, Message>| -> Element<'a, Message> {
            column![
                enz_text(title)
                    .size(TextSize::Headline)
                    .heading(enzui_core::semantics::HeadingLevel::Section)
                    .into_element(),
                body,
            ]
            .spacing(6.0)
            .into()
        };
        let mut sections = column![].spacing(20.0).width(Length::Fill);
        if let Some(notes) = self.release_notes {
            sections = sections.push(section(
                enzui_core::localization::text("enzui-about-dialog-whats-new"),
                enz_text(notes).into_element(),
            ));
        }
        if !self.credits.is_empty() {
            let mut credits = column![].spacing(12.0);
            for (title, names) in self.credits {
                credits = credits.push(
                    column![
                        enz_text(title)
                            .tone(TextTone::Muted)
                            .size(TextSize::Caption)
                            .into_element(),
                        enz_text(names.join("\n")).into_element(),
                    ]
                    .spacing(2.0),
                );
            }
            sections = sections.push(section(
                enzui_core::localization::text("enzui-about-dialog-credits"),
                credits.into(),
            ));
        }
        if self.copyright.is_some() || self.license.is_some() || self.license_text.is_some() {
            let mut legal = column![].spacing(6.0);
            for line in [self.copyright, self.license, self.license_text]
                .into_iter()
                .flatten()
            {
                legal = legal.push(enz_text(line).into_element());
            }
            sections = sections.push(section(
                enzui_core::localization::text("enzui-about-dialog-legal"),
                legal.into(),
            ));
        }
        let body: Element<'a, Message> = enz_scroll_view(sections).height(Length::Fill).into();

        let close_label = enzui_core::localization::text("enzui-about-dialog-close");
        let close: Element<'a, Message> = {
            let on_event = Rc::clone(&on_event);
            container(
                crate::button::enz_button_with(close_label, move || {
                    on_event(AboutDialogEvent::Dismissed(DismissCause::CloseButton))
                })
                .kind(ButtonKind::Primary),
            )
            .width(Length::Fill)
            .align_x(iced::alignment::Horizontal::Right)
            .into()
        };
        let content: Element<'a, Message> = crate::capped_dialog_body::CappedDialogBody::new(
            vec![header],
            body,
            vec![close],
            20.0,
            (self.max_height - padding * 2.0).max(f32::MIN_POSITIVE),
        )
        .into();
        crate::modal_surface::enz_modal_surface(key_scroll_forward(content))
            .width(Length::Fixed(self.width))
            .padding(padding)
            .into_element()
    }
}

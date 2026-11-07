use iced::Element;

/// Pane body with an optional decorative, draggable title and interactive controls.
/// Titles elide within the lane left after controls and retain their full accessible name.
pub struct EnzPaneContent<'a, Message> {
    body: Element<'a, Message>,
    title: Option<String>,
    controls: Option<Element<'a, Message>>,
}

impl<'a, Message: 'a> EnzPaneContent<'a, Message> {
    /// Construct a body-only pane, which has no header drag region.
    pub fn new(body: impl Into<Element<'a, Message>>) -> Self {
        Self {
            body: body.into(),
            title: None,
            controls: None,
        }
    }

    /// Add a decorative title that can pick the pane when drag handling is enabled.
    /// Interactive widgets belong in [`Self::controls`], outside the drag region.
    #[must_use]
    pub fn header(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Add always-visible header controls; their width is reserved before laying out the title.
    /// Controls are shown only when a header is present.
    #[must_use]
    pub fn controls(mut self, controls: impl Into<Element<'a, Message>>) -> Self {
        self.controls = Some(controls.into());
        self
    }

    /// The same pane with its body replaced by `wrap(body)`.
    pub(super) fn map_body(
        mut self,
        wrap: impl FnOnce(Element<'a, Message>) -> Element<'a, Message>,
    ) -> Self {
        self.body = wrap(self.body);
        self
    }

    pub(super) fn native(self) -> iced::widget::pane_grid::Content<'a, Message> {
        let mut content = iced::widget::pane_grid::Content::new(self.body);
        if let Some(title) = self.title {
            let label: Element<'a, Message> = crate::enz_pane_header(title).into();
            let mut header = iced::widget::pane_grid::TitleBar::new(label)
                .draggable_title(true)
                .reserve_controls()
                .padding(8)
                .always_show_controls()
                .style(|theme: &iced::Theme| {
                    let p = crate::theme_lookup::palette(theme);
                    iced::widget::container::Style {
                        background: Some(p.surface.into()),
                        text_color: Some(p.text),
                        ..Default::default()
                    }
                });
            if let Some(controls) = self.controls {
                header = header.controls(controls);
            }
            content = content.title_bar(header);
        }
        content
    }
}

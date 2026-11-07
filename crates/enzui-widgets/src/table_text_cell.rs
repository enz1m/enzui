//! Single-line table values with configurable alignment and overflow markers.
//!
//! Cells fill their allocated width and height with 12px side padding. Text
//! captures a scaled 13px size at construction and resolves live palette ink
//! unless pinned. End ellipsis is the default; other modes retain no wrapping.
//!
//! Announcement is disabled by default so a host can provide the row's name.
//! Enabling it exposes one stock text label. Suppression blocks all operation
//! traversal into the cell; layout, painting, input and frame forwarding remain
//! delegated to the composed child.

use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, layout, mouse, overlay, renderer};
use iced::widget::text::{Ellipsis, Wrapping};
use iced::widget::{container, text};
use iced::{Color, Element, Event, Length, Rectangle, Size, Vector, alignment::Horizontal};
/// Horizontal placement of text within the allocated cell width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EnzTableTextAlign {
    /// Align left; this is the default.
    #[default]
    Left,
    /// Centre text horizontally.
    Center,
    /// Align right.
    Right,
}

impl EnzTableTextAlign {
    /// Parse exact lowercase center or right; every other string selects Left.
    pub fn from_name(s: &str) -> Self {
        match s {
            "center" => Self::Center,
            "right" => Self::Right,
            _ => Self::Left,
        }
    }
    /// Return the corresponding iced horizontal alignment.
    pub fn to_iced(self) -> Horizontal {
        match self {
            Self::Left => Horizontal::Left,
            Self::Center => Horizontal::Center,
            Self::Right => Horizontal::Right,
        }
    }
}
/// Table-value builder with optional text announcement.
#[must_use]
pub struct EnzTableTextCell<'a, Message> {
    text: String,
    text_color: Option<Color>,
    body_size: f32,
    align: EnzTableTextAlign,
    cell_padding_x: f32,
    ellipsis: Ellipsis,
    announce: bool,
    _phantom: std::marker::PhantomData<&'a Message>,
}
/// Own a value with scaled 13px text, left alignment, 12px side padding and suppressed announcement.
pub fn enz_table_text_cell<Message>(text: impl Into<String>) -> EnzTableTextCell<'static, Message> {
    EnzTableTextCell {
        text: text.into(),
        text_color: None,
        body_size: enzui_theme::text_scale::ui_px(13.0),
        align: EnzTableTextAlign::Left,
        cell_padding_x: 12.0,
        ellipsis: Ellipsis::End,
        announce: false,
        _phantom: std::marker::PhantomData,
    }
}

impl<'a, Message: 'a> EnzTableTextCell<'a, Message> {
    /// Pin text ink instead of resolving the live palette text colour.
    pub fn text_color(mut self, c: Color) -> Self {
        self.text_color = Some(c);
        self
    }
    /// Set text size in logical pixels. Non-finite or nonpositive values reset to stock 13.
    /// The construction default follows UI scaling, so the fallback can differ.
    pub fn body_size(mut self, sz: f32) -> Self {
        self.body_size = enzui_core::metrics::FinitePositive::new_or(sz, 13.0).get();
        self
    }
    /// Select horizontal placement; defaults to Left.
    pub fn align(mut self, a: EnzTableTextAlign) -> Self {
        self.align = a;
        self
    }
    /// Set side padding in logical pixels; negative or non-finite values reset to 12.
    pub fn cell_padding_x(mut self, px: f32) -> Self {
        self.cell_padding_x = enzui_core::metrics::FiniteNonNeg::new_or(px, 12.0).get();
        self
    }
    /// Expose the stock text label when true; defaults to false.
    /// False blocks all operation traversal into the composed cell.
    pub fn announce(mut self, announce: bool) -> Self {
        self.announce = announce;
        self
    }
    /// Return whether text announcement is enabled.
    pub fn is_announced(&self) -> bool {
        self.announce
    }
    /// Select the overflow marker; defaults to End. None removes the marker.
    /// The text remains single-line in every mode.
    pub fn ellipsis(mut self, e: Ellipsis) -> Self {
        self.ellipsis = e;
        self
    }
    fn run_config(&self) -> (Wrapping, Ellipsis) {
        (Wrapping::None, self.ellipsis)
    }
    /// Return the configured overflow marker; defaults to End.
    pub fn ellipsis_mode(&self) -> Ellipsis {
        self.run_config().1
    }
    /// Return None: table values do not wrap.
    pub fn wrapping_mode(&self) -> Wrapping {
        self.run_config().0
    }
    /// Compose a cell filling its allocated width and height.
    pub fn into_element(self) -> Element<'a, Message> {
        let pinned = self.text_color;
        // Raw iced text bypasses the framework text fallback setup.
        crate::sans_serif_fallback::ensure_sans_serif_fallback();
        let (wrapping, ellipsis) = self.run_config();
        let mut t = text(self.text)
            .size(self.body_size)
            .wrapping(wrapping)
            .ellipsis(ellipsis)
            .style(move |theme: &iced::Theme| text::Style {
                color: Some(pinned.unwrap_or_else(|| crate::theme_lookup::palette(theme).text)),
            });
        // Text needs allocated width for centre and right alignment to affect painting.
        t = t.width(Length::Fill).align_x(self.align.to_iced());
        let element: Element<'a, Message> = container(t)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(
                iced::Padding::default()
                    .left(self.cell_padding_x)
                    .right(self.cell_padding_x),
            )
            .center_y(Length::Fill)
            .into();

        if self.announce {
            element
        } else {
            SuppressAnnounce { child: element }.into()
        }
    }
}

impl<'a, Message: 'a> From<EnzTableTextCell<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzTableTextCell<'a, Message>) -> Self {
        w.into_element()
    }
}

struct SuppressAnnounce<'a, Message> {
    child: Element<'a, Message>,
}

impl<Message> iced::advanced::Widget<Message, iced::Theme, iced::Renderer>
    for SuppressAnnounce<'_, Message>
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        self.child.as_widget().tag()
    }
    fn state(&self) -> iced::advanced::widget::tree::State {
        self.child.as_widget().state()
    }
    fn diff(&mut self, tree: &mut Tree) {
        self.child.as_widget_mut().diff(tree);
    }
    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }
    // This transparent wrapper shares the child tree node and its frame interest.
    fn frame_interest(&self, tree: &Tree) -> bool {
        crate::frame_routing::interest_transparent(&self.child, tree)
    }
    fn update_frame(
        &mut self,
        tree: &mut Tree,
        event: &iced::Event,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &iced::Rectangle,
    ) {
        crate::frame_routing::forward_frame_transparent(
            &mut self.child,
            tree,
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
    }
    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.child.as_widget_mut().layout(tree, renderer, limits);
    }
    fn operate(
        &mut self,
        _tree: &mut Tree,
        _layout: Layout,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
        _operation: &mut dyn Operation,
    ) {
        // Stop the walk so stock text cannot publish a duplicate of its host row name.
    }
    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);
    }
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.child
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }
    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.child
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }
    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.child
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
}

impl<'a, Message: 'a> From<SuppressAnnounce<'a, Message>> for Element<'a, Message> {
    fn from(w: SuppressAnnounce<'a, Message>) -> Self {
        Element::new(w)
    }
}

#[cfg(test)]
mod tests {}

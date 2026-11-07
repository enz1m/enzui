//! The seam between two segmented tab cells.

use iced::advanced::widget::Tree;
use iced::advanced::{Layout, Widget, layout, mouse, renderer};
use iced::{Color, Length, Rectangle, Size, Theme};

/// Resolves a frame's track and hairline colours from the theme.
type TrackPaint<'a> = Box<dyn Fn(&Theme) -> (Color, Color) + 'a>;

/// The seam between two segmented cells: a track-coloured column as tall as
/// the cells, with a hairline across its middle half. One leaf paints both;
/// a container around a container would cost three tree nodes per seam.
pub(super) struct Seam<'a> {
    height: f32,
    /// The track and hairline colours for this frame.
    paint: TrackPaint<'a>,
}

impl<'a> Seam<'a> {
    /// A seam `height` tall, painted with the `(track, hairline)` colours
    /// `paint` resolves each frame.
    pub(super) fn new(height: f32, paint: impl Fn(&Theme) -> (Color, Color) + 'a) -> Self {
        Self {
            height,
            paint: Box::new(paint),
        }
    }
}

impl<Message> Widget<Message, Theme, iced::Renderer> for Seam<'_> {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(1.0), Length::Fixed(self.height))
    }

    fn layout(&mut self, tree: &mut Tree, _renderer: &iced::Renderer, limits: &layout::Limits) {
        layout::paint_box(limits, Length::Fixed(1.0), Length::Fixed(self.height)).apply(tree);
    }

    fn draw(
        &self,
        _tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &Theme,
        _style: &renderer::Style,
        layout: Layout,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        let bounds = layout.bounds();
        let (track, hairline) = (self.paint)(theme);
        let quad = |bounds| renderer::Quad {
            bounds,
            snap: renderer::CRISP,
            ..renderer::Quad::default()
        };
        renderer.fill_quad(quad(bounds), track);
        let line = bounds.height / 2.0;
        renderer.fill_quad(
            quad(Rectangle {
                y: bounds.y + (bounds.height - line) / 2.0,
                height: line,
                ..bounds
            }),
            hairline,
        );
    }
}

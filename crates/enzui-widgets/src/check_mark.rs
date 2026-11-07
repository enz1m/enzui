//! The tick and dash a checkbox draws inside its box.
//!
//! Stroked from the box's own geometry rather than taken from an icon font:
//! a font glyph nearly fills its viewbox and carries side bearings, so it ran
//! into a round box's edge and sat off centre. The strokes here keep a
//! margin from the edge, use round caps and joins, and the dash is centred
//! exactly.

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Tree, Widget};
use iced::advanced::{mouse, renderer};
use iced::widget::canvas::{Frame, LineCap, LineJoin, Path, Stroke};
use iced::{Color, Element, Length, Point, Rectangle, Size, Vector};

/// Which mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mark {
    /// Checked.
    Tick,
    /// Mixed: some children on.
    Dash,
}

/// A mark `size` px square, inked by `ink` for the live theme.
pub(crate) struct CheckMark<'a> {
    mark: Mark,
    size: f32,
    ink: Box<dyn Fn(&iced::Theme) -> Color + 'a>,
}

impl<'a> CheckMark<'a> {
    pub(crate) fn new(mark: Mark, size: f32, ink: impl Fn(&iced::Theme) -> Color + 'a) -> Self {
        Self {
            mark,
            size,
            ink: Box::new(ink),
        }
    }
}

/// The mark's points as fractions of the box side. The tick's short arm
/// starts left of centre and its long arm ends short of the top-right edge,
/// so it sits optically centred in a round box as well as a square one.
const TICK: [(f32, f32); 3] = [(0.28, 0.52), (0.44, 0.67), (0.72, 0.36)];
const DASH: [(f32, f32); 2] = [(0.29, 0.5), (0.71, 0.5)];

/// The stroke width for a box `size` px across: about an eighth of it, and
/// never thinner than 1.5 px.
pub(crate) fn stroke_width(size: f32) -> f32 {
    (size * 0.125).max(1.5)
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for CheckMark<'_> {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(self.size), Length::Fixed(self.size))
    }

    fn layout(&mut self, tree: &mut Tree, _renderer: &iced::Renderer, _limits: &layout::Limits) {
        layout::Node::new(Size::new(self.size, self.size)).apply(tree);
    }

    fn draw(
        &self,
        _tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        _style: &renderer::Style,
        layout: Layout,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        use iced::advanced::graphics::geometry::Renderer as _;
        let bounds = layout.bounds();
        let side = bounds.width.min(bounds.height);
        if side <= 0.0 {
            return;
        }
        let at = |(x, y): (f32, f32)| Point::new(x * side, y * side);
        let path = Path::new(|builder| {
            let points: &[(f32, f32)] = match self.mark {
                Mark::Tick => &TICK,
                Mark::Dash => &DASH,
            };
            builder.move_to(at(points[0]));
            for point in &points[1..] {
                builder.line_to(at(*point));
            }
        });
        let mut frame = Frame::new(renderer, Size::new(side, side));
        frame.stroke(
            &path,
            Stroke::default()
                .with_color((self.ink)(theme))
                .with_width(stroke_width(side))
                .with_line_cap(LineCap::Round)
                .with_line_join(LineJoin::Round),
        );
        let geometry = frame.into_geometry();
        renderer.with_translation(Vector::new(bounds.x, bounds.y), |renderer| {
            renderer.draw_geometry(geometry);
        });
    }
}

impl<'a, Message: 'a> From<CheckMark<'a>> for Element<'a, Message> {
    fn from(mark: CheckMark<'a>) -> Self {
        Element::new(mark)
    }
}

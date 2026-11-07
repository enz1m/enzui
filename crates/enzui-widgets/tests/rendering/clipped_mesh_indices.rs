//! A mesh clipped out entirely does not shift the next mesh's indices: the
//! visible canvas after an off-screen one still paints its own shape.

use iced::advanced::renderer::Headless as _;
use iced::widget::canvas::{self, Canvas, Frame, Geometry, Path};
use iced::widget::pin;
use iced::{Color, Element, Point, Rectangle, Size, Theme, mouse};
use iced_test::runtime::user_interface::{Cache, UserInterface};

type Renderer = iced_test::renderer::Renderer;

/// Draws a large clipped-away shape first (its own mesh, outside every
/// viewport), then fills its bounds red.
struct ClippedThenFilled;

impl canvas::Program<()> for ClippedThenFilled {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &iced::Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        // Far outside the window: a mesh with many indices that is clipped out.
        frame.with_clip(
            Rectangle::new(Point::new(5000.0, 5000.0), Size::new(400.0, 400.0)),
            |clipped| {
                let circle = Path::circle(Point::new(200.0, 200.0), 190.0);
                clipped.fill(&circle, Color::from_rgb(0.0, 0.0, 1.0));
            },
        );
        let square = Path::rectangle(Point::ORIGIN, bounds.size());
        frame.fill(&square, Color::from_rgb(1.0, 0.0, 0.0));
        vec![frame.into_geometry()]
    }
}

#[test]
fn a_mesh_after_a_clipped_one_paints_its_own_shape() {
    let Some(mut renderer) = iced_test::futures::futures::executor::block_on(Renderer::new(
        iced::advanced::renderer::Settings::default(),
        Some("wgpu"),
    )) else {
        println!("SKIPPED: no wgpu adapter, meshes cannot draw");
        return;
    };
    let view: Element<'_, (), Theme, Renderer> =
        pin(Canvas::new(ClippedThenFilled).width(64).height(64))
            .x(8.0)
            .y(8.0)
            .into();
    let mut ui = UserInterface::build(view, Size::new(96.0, 96.0), Cache::default(), &mut renderer);
    ui.draw(
        &mut renderer,
        &Theme::Light,
        &iced::advanced::renderer::Style {
            text_color: Color::BLACK,
        },
        mouse::Cursor::Unavailable,
    );
    let pixels = renderer.screenshot(Size::new(96, 96), 1.0, Color::WHITE);
    let at = |x: usize, y: usize| {
        let i = (y * 96 + x) * 4;
        [pixels[i], pixels[i + 1], pixels[i + 2]]
    };
    for (x, y) in [(10, 10), (40, 40), (70, 70), (12, 68), (68, 12)] {
        assert!(
            at(x, y)[0] > 200 && at(x, y)[2] < 60,
            "red at ({x}, {y}): {:?}",
            at(x, y)
        );
    }
}

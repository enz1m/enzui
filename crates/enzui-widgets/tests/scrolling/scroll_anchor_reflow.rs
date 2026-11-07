//! A scroll view's anchor keeps a document page in place while the content
//! reflows every frame (#3653): a fit-width document narrowing as a sidebar
//! slides in keeps the page the reader was on at the same painted position on
//! every frame, instead of drifting and settling back.

use enzui_widgets::scroll_view::enz_scroll_view;
use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::Headless as _;
use iced::advanced::widget::{Tree, Widget};
use iced::advanced::{Renderer as _, renderer};
use iced::widget::column;
use iced::{Color, Element, Length, Rectangle, Size, mouse};
use iced_test::runtime::user_interface::{Cache, UserInterface};

const PAGES: usize = 10;
const GAP: f32 = 8.0;
const ASPECT: f32 = 1.4;
const READING: usize = 5;
const HEIGHT: f32 = 400.0;
/// Where the page being read is held, so its top edge is visible and any
/// drift either way shows.
const HELD_AT: f32 = 50.0;

/// A page as wide as it is given and `ASPECT` times as tall; the page the
/// reader is on is red.
struct Page(bool);

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for Page {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Shrink)
    }

    fn layout(&mut self, tree: &mut Tree, _: &iced::Renderer, limits: &layout::Limits) {
        let width = limits.max().width;
        layout::Node::new(Size::new(width, width * ASPECT)).apply(tree);
    }

    fn draw(
        &self,
        _: &Tree,
        renderer: &mut iced::Renderer,
        _: &iced::Theme,
        _: &renderer::Style,
        layout: Layout,
        _: mouse::Cursor,
        _: &Rectangle,
    ) {
        renderer.fill_quad(
            renderer::Quad {
                bounds: layout.bounds(),
                ..renderer::Quad::default()
            },
            if self.0 {
                Color::from_rgb(1.0, 0.0, 0.0)
            } else {
                Color::from_rgb(0.5, 0.5, 0.5)
            },
        );
    }
}

/// The offset that shows the page the reader is on `HELD_AT` below the top,
/// at the laid-out content width.
fn page_top(content: Size) -> f32 {
    READING as f32 * (content.width * ASPECT + GAP) - HELD_AT
}

fn view(anchored: bool) -> Element<'static, ()> {
    let pages = column((0..PAGES).map(|index| Element::new(Page(index == READING)))).spacing(GAP);
    let view = enz_scroll_view(pages).height(Length::Fixed(HEIGHT));
    if anchored {
        view.anchor(|content, _viewport| Some(page_top(content)))
            .into()
    } else {
        view.initial_offset(iced::widget::scrollable::AbsoluteOffset {
            x: 0.0,
            y: page_top(Size::new(600.0, 0.0)),
        })
        .into()
    }
}

/// The topmost red row in `pixels`.
fn red_top(pixels: &[u8], width: u32) -> Option<usize> {
    pixels
        .as_chunks::<4>()
        .0
        .iter()
        .position(|px| px[0] > 240 && px[1] < 16 && px[2] < 16)
        .map(|index| index / width as usize)
}

/// Where the page the reader is on is painted on each frame as the window
/// narrows from 600 to 450 px, each frame laid out after its update as the
/// runtime does.
fn painted_tops(anchored: bool) -> Vec<Option<usize>> {
    let mut renderer = crate::honest_frame_driver::new_renderer();
    let mut cache = Cache::default();
    let mut tops = Vec::new();
    let mut previous = 600.0;
    for width in [600.0, 600.0, 570.0, 540.0, 510.0, 480.0, 450.0, 450.0] {
        let mut ui = UserInterface::build(
            view(anchored),
            Size::new(previous, HEIGHT),
            std::mem::take(&mut cache),
            &mut renderer,
        );
        let _ = ui.update(
            &iced::window::Headless,
            &iced_test::core::shell::Waker::noop(),
            &[iced::Event::Window(iced::window::Event::RedrawRequested(
                iced::time::Instant::now(),
            ))],
            mouse::Cursor::Unavailable,
            &mut renderer,
            &mut iced::advanced::shell::Bus::new(),
        );
        let mut ui = UserInterface::build(
            view(anchored),
            Size::new(width, HEIGHT),
            ui.into_cache(),
            &mut renderer,
        );
        ui.draw(
            &mut renderer,
            &iced::Theme::Dark,
            &renderer::Style::default(),
            mouse::Cursor::Unavailable,
        );
        let pixels = renderer.screenshot(Size::new(width as u32, HEIGHT as u32), 1.0, Color::BLACK);
        tops.push(red_top(&pixels, width as u32));
        cache = ui.into_cache();
        previous = width;
    }
    tops
}

#[test]
fn the_page_being_read_stays_put_while_the_document_narrows() {
    let anchored = painted_tops(true);
    assert!(
        anchored.iter().all(|top| *top == Some(HELD_AT as usize)),
        "the page stays put on every frame: {anchored:?}"
    );
    let unanchored = painted_tops(false);
    assert!(
        unanchored.iter().any(|top| *top != Some(HELD_AT as usize)),
        "without the anchor the page drifts, so the check can fail: {unanchored:?}"
    );
}

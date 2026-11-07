use enzui_widgets::inert::Inert;
use iced::advanced::widget::{self, Widget};
use iced::advanced::{Layout, Shell, layout, mouse, renderer};
use iced::{Element, Event, Length, Rectangle, Size};
use iced_test::runtime::user_interface::{Cache, UserInterface};
use std::cell::Cell;
use std::rc::Rc;

type Renderer = iced_test::renderer::Renderer;

#[derive(Debug, Clone)]
enum Msg {}

struct InterestLeaf {
    active: Rc<Cell<bool>>,
    frames: Rc<Cell<u32>>,
    ordinary_updates: Rc<Cell<u32>>,
    frame_cursor: Rc<Cell<Option<iced::Point>>>,
    keep_animating: bool,
}

impl Widget<Msg, iced::Theme, Renderer> for InterestLeaf {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(80.0), Length::Fixed(48.0))
    }

    fn layout(&mut self, tree: &mut widget::Tree, _renderer: &Renderer, limits: &layout::Limits) {
        layout::Node::new(limits.resolve(
            Length::Fixed(80.0),
            Length::Fixed(48.0),
            Size::new(80.0, 48.0),
        ))
        .apply(tree);
    }

    fn frame_interest(&self, _tree: &widget::Tree) -> bool {
        self.active.get()
    }

    fn update(
        &mut self,
        _tree: &mut widget::Tree,
        event: &Event,
        _layout: Layout,
        _cursor: mouse::Cursor,
        _renderer: &Renderer,
        shell: &mut Shell<'_, Msg>,
        _viewport: &Rectangle,
    ) {
        self.ordinary_updates.set(self.ordinary_updates.get() + 1);
        if matches!(event, Event::Mouse(mouse::Event::ButtonPressed(_))) {
            self.active.set(true);
            shell.request_redraw();
        }
    }

    fn update_frame(
        &mut self,
        _tree: &mut widget::Tree,
        _event: &Event,
        _layout: Layout,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
        shell: &mut Shell<'_, Msg>,
        _viewport: &Rectangle,
    ) {
        self.frames.set(self.frames.get() + 1);
        self.frame_cursor.set(cursor.position());
        if self.keep_animating {
            shell.request_redraw();
        } else {
            self.active.set(false);
        }
    }

    fn draw(
        &self,
        _tree: &widget::Tree,
        _renderer: &mut Renderer,
        _theme: &iced::Theme,
        _style: &renderer::Style,
        _layout: Layout,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
    }
}

fn renderer() -> Renderer {
    iced_test::futures::futures::executor::block_on(
        <Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer")
}

#[test]
fn an_inert_wrapper_refuses_input_but_still_delivers_time() {
    let active = Rc::new(Cell::new(true));
    let frames = Rc::new(Cell::new(0));
    let ordinary_updates = Rc::new(Cell::new(0));
    let leaf = Element::new(InterestLeaf {
        active: Rc::clone(&active),
        frames: Rc::clone(&frames),
        ordinary_updates: Rc::clone(&ordinary_updates),
        frame_cursor: Rc::new(Cell::new(None)),
        keep_animating: false,
    });
    let mut renderer = renderer();
    let inert: Element<'static, Msg> = Inert::new(leaf, true).into();
    let mut ui = UserInterface::build(
        inert,
        Size::new(200.0, 100.0),
        Cache::default(),
        &mut renderer,
    );
    let mut messages = iced::advanced::shell::Bus::new();
    fn frame(
        ui: &mut UserInterface<'static, Msg, iced::Theme, iced::Renderer>,
        renderer: &mut iced::Renderer,
        messages: &mut iced::advanced::shell::Bus<Msg>,
    ) {
        let _ = ui.update(
            &iced::window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            &[Event::Window(iced::window::Event::RedrawRequested(
                iced::time::Instant::now(),
            ))],
            mouse::Cursor::Unavailable,
            renderer,
            messages,
        );
    }

    frame(&mut ui, &mut renderer, &mut messages);
    assert_eq!(
        frames.get(),
        1,
        "an inert branch that is still animating must still be told that time \
         passed, or its animation freezes and it can never reach the lane"
    );
    let before = ordinary_updates.get();
    let _ = ui.update(
        &iced::window::Headless,
        &iced::advanced::graphics::core::shell::Waker::noop(),
        &[Event::Mouse(mouse::Event::ButtonPressed(
            mouse::Button::Left,
        ))],
        mouse::Cursor::Available(iced::Point::new(10.0, 10.0)),
        &mut renderer,
        &mut messages,
    );
    assert_eq!(
        ordinary_updates.get(),
        before,
        "an inert branch must still refuse ordinary input"
    );
    active.set(false);
    let quiet = frames.get();
    frame(&mut ui, &mut renderer, &mut messages);
    frame(&mut ui, &mut renderer, &mut messages);
    assert_eq!(
        frames.get(),
        quiet,
        "a descendant that declares no frame interest must still never be \
         walked by a moving frame"
    );
}

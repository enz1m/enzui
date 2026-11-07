//! A code block's text selects like prose: a drag selects part of it and
//! Ctrl+C copies that part. Inside a selectable document the block steps
//! aside, so one drag selects from the code into the text around it.

use enzui_widgets::code_block::enz_code_block;
use enzui_widgets::enz_text::enz_text;
use enzui_widgets::selection_scope::enz_selection_scope;
use iced::advanced::clipboard::Content;
use iced::advanced::mouse;
use iced::advanced::renderer::Headless;
use iced::advanced::widget::{self, Operation};
use iced::widget::column;
use iced::{Element, Event, Point, Rectangle, Size};
use iced_test::runtime::user_interface::{Cache, State, UserInterface};

type Renderer = iced_test::renderer::Renderer;

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Copied(String),
}

const CODE: &str = "let alpha = 1;\nlet beta = 2;";
const WINDOW: Size = Size::new(600.0, 400.0);

fn block() -> Element<'static, Msg> {
    enz_code_block(CODE).language("rust").into_element()
}

/// The block under a line of prose, both in one selection scope.
fn document() -> Element<'static, Msg> {
    enz_selection_scope(column![enz_text("Intro line"), block()].spacing(12))
        .on_copy(Msg::Copied)
        .into_element()
}

fn renderer() -> Renderer {
    iced_test::futures::futures::executor::block_on(<Renderer as Headless>::new(
        iced::advanced::renderer::Settings::default(),
        std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
    ))
    .expect("headless renderer")
}

/// The bounds of the selectable text region whose text contains `needle`.
fn region(view: fn() -> Element<'static, Msg>, needle: &str) -> Rectangle {
    struct Find<'n> {
        needle: &'n str,
        found: Option<Rectangle>,
    }
    impl Operation for Find<'_> {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }
        fn selectable_text(
            &mut self,
            _id: Option<&widget::Id>,
            bounds: Rectangle,
            text: &str,
            _shaped: &dyn widget::operation::SelectableTextLayout,
        ) {
            if self.found.is_none() && text.contains(self.needle) {
                self.found = Some(bounds);
            }
        }
        fn finish(&self) -> widget::operation::Outcome<()> {
            widget::operation::Outcome::None
        }
    }
    let mut renderer = renderer();
    let mut ui = UserInterface::build(view(), WINDOW, Cache::default(), &mut renderer);
    let mut find = Find {
        needle,
        found: None,
    };
    ui.operate(&renderer, &mut find as &mut dyn Operation);
    find.found
        .unwrap_or_else(|| panic!("no selectable text holds {needle:?}"))
}

/// Press at `from`, drag to `to`, release and press Ctrl+C: the messages
/// published and the text written to the clipboard.
fn drag_and_copy(
    view: fn() -> Element<'static, Msg>,
    from: Point,
    to: Point,
) -> (Vec<Msg>, Option<String>) {
    let c = iced::keyboard::Key::Character("c".into());
    let mut events = vec![
        (
            Event::Mouse(mouse::Event::CursorMoved { position: from }),
            from,
        ),
        (
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            from,
        ),
    ];
    for step in 1..=6 {
        let t = step as f32 / 6.0;
        let at = Point::new(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
        events.push((Event::Mouse(mouse::Event::CursorMoved { position: at }), at));
    }
    events.push((
        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
        to,
    ));
    events.push((
        Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key: c.clone(),
            modified_key: c,
            physical_key: iced::keyboard::key::Physical::Unidentified(
                iced::keyboard::key::NativeCode::Unidentified,
            ),
            location: iced::keyboard::Location::Standard,
            modifiers: iced::keyboard::Modifiers::COMMAND,
            text: None,
            repeat: false,
        }),
        to,
    ));

    let mut renderer = renderer();
    let mut messages = iced::advanced::shell::Bus::new();
    let waker = iced::advanced::graphics::core::shell::Waker::noop();
    let window = iced::advanced::graphics::core::window::Headless;
    let mut cache = Cache::default();
    let mut written = None;
    for (event, at) in events {
        let mut ui = UserInterface::build(view(), WINDOW, cache, &mut renderer);
        let (state, _) = ui.update(
            &window,
            &waker,
            &[event],
            mouse::Cursor::Available(at),
            &mut renderer,
            &mut messages,
        );
        if let State::Updated { clipboard, .. } = state
            && let Some(Content::Text(text)) = clipboard.write
        {
            written = Some(text);
        }
        cache = ui.into_cache();
    }
    (messages.into_iter().collect(), written)
}

/// A point on the first line of `bounds`, `x` in from its left edge.
fn first_line(bounds: Rectangle, x: f32) -> Point {
    Point::new(bounds.x + x, bounds.y + 6.0)
}

#[test]
fn a_drag_in_a_code_block_copies_the_part_it_selects() {
    let code = region(block, "alpha");
    let (_, written) = drag_and_copy(block, first_line(code, 1.0), first_line(code, 60.0));
    let written = written.expect("Ctrl+C after a drag writes the clipboard");
    assert!(
        !written.is_empty() && CODE.starts_with(&written) && written.len() < CODE.len(),
        "copied {written:?}, not the start of the first line"
    );
}

#[test]
fn a_drag_from_code_into_the_prose_above_selects_both() {
    let code = region(document, "alpha");
    let intro = region(document, "Intro");
    let (messages, written) =
        drag_and_copy(document, first_line(code, 60.0), first_line(intro, 1.0));
    assert_eq!(written, None, "the document's own copy handler owns Ctrl+C");
    let Some(Msg::Copied(text)) = messages.first() else {
        panic!("the document published no copy: {messages:?}");
    };
    assert!(
        text.starts_with("Intro") && text.contains("let"),
        "copied {text:?}, not from the prose into the code"
    );
}

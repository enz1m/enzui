//! Shared label layout, finite width bounds, row grouping, and retained field carets.

use super::*;
use iced::widget::text_input;
use iced::{Element, Size};

#[derive(Debug, Clone, PartialEq)]
#[allow(
    dead_code,
    reason = "the widget needs these messages to exist; this test never sends them"
)]
enum Msg {
    Edit(String),
    Toggle(bool),
}

fn renderer() -> iced_test::renderer::Renderer {
    iced_test::futures::futures::executor::block_on(
        <iced_test::renderer::Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer")
}

#[test]
fn caret_survives_a_resize_under_focus() {
    use iced::advanced::widget::{Id, Operation};
    use iced::{Point, mouse, window};
    use iced_test::runtime::user_interface::{Cache, UserInterface};

    let field_id = Id::new("form-name-field");

    let view = |value: &str| -> Element<'_, Msg> {
        enz_form(vec![form_field(
            "Name",
            text_input("", value)
                .id(field_id.clone())
                .on_input(Msg::Edit),
        )])
        .breakpoint(320.0)
        .into()
    };

    let mut renderer = renderer();
    let mut value = String::new();
    let mut size = Size::new(600.0, 400.0);
    let mut cache = Cache::default();

    {
        let mut ui = UserInterface::build(view(&value), size, cache, &mut renderer);
        let mut op: Box<dyn Operation<()>> =
            enzui_core::focus::traversal_operation::<()>(enzui_core::focus::Traversal::Next);
        loop {
            ui.operate(&renderer, &mut *op);
            match op.finish() {
                iced::advanced::widget::operation::Outcome::Chain(next) => op = next,
                _ => break,
            }
        }
        cache = ui.into_cache();
    }

    let pump = |value: &mut String,
                size: Size,
                cache: Cache,
                events: Vec<iced::Event>,
                renderer: &mut iced_test::renderer::Renderer|
     -> Cache {
        let mut ui = UserInterface::build(view(value), size, cache, renderer);
        let mut messages: iced::advanced::shell::Bus<Msg> = iced::advanced::shell::Bus::new();
        let _ = ui.update(
            &window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            &events,
            mouse::Cursor::Available(Point::new(5000.0, 5000.0)),
            renderer,
            &mut messages,
        );
        for m in messages {
            if let Msg::Edit(v) = m {
                *value = v;
            }
        }
        ui.into_cache()
    };

    let ch = |c: char| -> Vec<iced::Event> {
        use iced::keyboard;
        let s = iced::advanced::graphics::core::SmolStr::new_inline(&c.to_string());
        vec![iced::Event::Keyboard(keyboard::Event::KeyPressed {
            key: keyboard::Key::Character(s.clone()),
            modified_key: keyboard::Key::Character(s.clone()),
            physical_key: keyboard::key::Physical::Unidentified(
                keyboard::key::NativeCode::Unidentified,
            ),
            location: keyboard::Location::Standard,
            modifiers: keyboard::Modifiers::default(),
            repeat: false,
            text: Some(s),
        })]
    };
    let left = || -> Vec<iced::Event> {
        use iced::keyboard;
        vec![iced::Event::Keyboard(keyboard::Event::KeyPressed {
            key: keyboard::Key::Named(keyboard::key::Named::ArrowLeft),
            modified_key: keyboard::Key::Named(keyboard::key::Named::ArrowLeft),
            physical_key: keyboard::key::Physical::Unidentified(
                keyboard::key::NativeCode::Unidentified,
            ),
            location: keyboard::Location::Standard,
            modifiers: keyboard::Modifiers::default(),
            repeat: false,
            text: None,
        })]
    };

    for c in ['a', 'b', 'c', 'd'] {
        cache = pump(&mut value, size, cache, ch(c), &mut renderer);
    }
    assert_eq!(value, "abcd", "typing accumulates: {value:?}");
    cache = pump(&mut value, size, cache, left(), &mut renderer);
    cache = pump(&mut value, size, cache, left(), &mut renderer);

    size = Size::new(280.0, 400.0);
    cache = pump(&mut value, size, cache, ch('X'), &mut renderer);

    assert_eq!(
        value, "abXcd",
        "the caret must survive the resize: got {value:?} (a rebuild that reset \
         the caret would give \"abcdX\" or \"Xabcd\")"
    );

    // With no caret movement, the same resize sequence must append at the end.
    let mut value2 = String::new();
    let mut cache2 = Cache::default();
    {
        let mut ui = UserInterface::build(
            view(&value2),
            Size::new(600.0, 400.0),
            cache2,
            &mut renderer,
        );
        let mut op: Box<dyn Operation<()>> =
            enzui_core::focus::traversal_operation::<()>(enzui_core::focus::Traversal::Next);
        loop {
            ui.operate(&renderer, &mut *op);
            match op.finish() {
                iced::advanced::widget::operation::Outcome::Chain(next) => op = next,
                _ => break,
            }
        }
        cache2 = ui.into_cache();
    }
    for c in ['a', 'b', 'c', 'd'] {
        cache2 = pump(
            &mut value2,
            Size::new(600.0, 400.0),
            cache2,
            ch(c),
            &mut renderer,
        );
    }
    cache2 = pump(
        &mut value2,
        Size::new(280.0, 400.0),
        cache2,
        ch('X'),
        &mut renderer,
    );
    let _ = cache2;
    assert_eq!(
        value2, "abcdX",
        "control: with no caret move, typing after resize appends at the end"
    );
    let _ = cache;
}

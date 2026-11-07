//! A press inside a selection and a drag hand the selected text to the
//! selection-drag handler, in single-line inputs and in editors; a press that
//! does not move is an ordinary click; a declined drag selects as usual.
//! One test: the handler is process-wide.

#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::missing_assert_message,
    reason = "a test crate reports by panicking"
)]
use iced::advanced::text::selection_drag::{self, SelectionDrag};
use iced::advanced::widget::operation::TextInput;
use iced::advanced::widget::operation::text_input::AccessibleText;
use iced::advanced::widget::{Id, Operation};
use iced::widget::{text_editor, text_input};
use iced::{Element, Event, Point, Rectangle, Size, mouse, window};
use iced_test::runtime::user_interface::{Cache, UserInterface};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
enum Msg {
    Input(String),
    Edit(text_editor::Action),
}

/// Selects everything in the input `id`, or reads its text.
struct Probe {
    select: bool,
    bounds: Option<Rectangle>,
    text: Option<AccessibleText>,
}

impl Operation for Probe {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn text_input(&mut self, _id: Option<&Id>, bounds: Rectangle, state: &mut dyn TextInput) {
        if self.select {
            state.select_all();
        }
        self.bounds = Some(bounds);
        self.text = state.accessible_text();
    }
}

struct Doc {
    value: String,
    content: text_editor::Content,
    editor: bool,
}

fn view(doc: &Doc) -> Element<'_, Msg> {
    if doc.editor {
        text_editor(&doc.content).on_action(Msg::Edit).into()
    } else {
        text_input("", &doc.value).on_input(Msg::Input).into()
    }
}

struct Host {
    renderer: iced_test::renderer::Renderer,
    cache: Cache,
    doc: Doc,
}

impl Host {
    fn probe(&mut self, select: bool) -> Probe {
        let mut probe = Probe {
            select,
            bounds: None,
            text: None,
        };
        let mut ui = UserInterface::build(
            view(&self.doc),
            Size::new(400.0, 200.0),
            std::mem::take(&mut self.cache),
            &mut self.renderer,
        );
        ui.operate(&self.renderer, &mut probe);
        self.cache = ui.into_cache();
        probe
    }

    fn events(&mut self, events: &[Event], at: Point) -> Vec<Msg> {
        let mut ui = UserInterface::build(
            view(&self.doc),
            Size::new(400.0, 200.0),
            std::mem::take(&mut self.cache),
            &mut self.renderer,
        );
        let mut messages = iced::advanced::shell::Bus::new();
        let _ = ui.update(
            &window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            events,
            mouse::Cursor::Available(at),
            &mut self.renderer,
            &mut messages,
        );
        self.cache = ui.into_cache();
        let messages: Vec<Msg> = messages.into_iter().collect();
        for message in &messages {
            match message {
                Msg::Input(value) => self.doc.value.clone_from(value),
                Msg::Edit(action) => {
                    let _ = self.doc.content.perform(action.clone());
                }
            }
        }
        messages
    }

    /// A press after the double-click interval, so it counts as one click.
    fn press(&mut self, at: Point) -> Vec<Msg> {
        std::thread::sleep(std::time::Duration::from_millis(450));
        self.events(
            &[Event::Mouse(mouse::Event::ButtonPressed(
                mouse::Button::Left,
            ))],
            at,
        )
    }

    fn release(&mut self, at: Point) -> Vec<Msg> {
        self.events(
            &[Event::Mouse(mouse::Event::ButtonReleased(
                mouse::Button::Left,
            ))],
            at,
        )
    }

    fn move_to(&mut self, at: Point) -> Vec<Msg> {
        self.events(
            &[Event::Mouse(mouse::Event::CursorMoved { position: at })],
            at,
        )
    }

    fn selection(&mut self) -> Option<(usize, usize)> {
        let text = self.probe(false).text?;
        text.selection.map(|(a, b)| (a.index, b.index))
    }
}

fn host(editor: bool) -> Host {
    let renderer = iced::futures::executor::block_on(
        <iced_test::renderer::Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            None,
        ),
    )
    .unwrap();
    Host {
        renderer,
        cache: Cache::default(),
        doc: Doc {
            value: "hello drag world".to_owned(),
            content: text_editor::Content::with_text("hello drag world"),
            editor,
        },
    }
}

#[test]
fn selections_drag_out_of_inputs_and_editors() {
    let dragged: Arc<Mutex<Vec<String>>> = Arc::default();
    let accept = Arc::new(Mutex::new(true));
    {
        let (dragged, accept) = (Arc::clone(&dragged), Arc::clone(&accept));
        selection_drag::set_handler(Some(Arc::new(move |drag: SelectionDrag| {
            dragged.lock().unwrap().push(drag.text);
            *accept.lock().unwrap()
        })));
    }

    for editor in [false, true] {
        let what = if editor { "editor" } else { "input" };
        // Focus, then select everything.
        let mut host = host(editor);
        let bounds = host.probe(false).bounds.unwrap();
        let inside = Point::new(bounds.x + 30.0, bounds.center_y());
        let _ = host.press(inside);
        let _ = host.release(inside);
        let _ = host.probe(true);
        assert_eq!(host.selection(), Some((0, 16)), "{what}: all selected");

        // A press inside the selection and a drag: the text goes out, the
        // selection stays.
        dragged.lock().unwrap().clear();
        let _ = host.press(inside);
        let _ = host.move_to(Point::new(inside.x + 2.0, inside.y));
        assert!(
            dragged.lock().unwrap().is_empty(),
            "{what}: under the threshold"
        );
        let _ = host.move_to(Point::new(inside.x + 40.0, inside.y));
        let _ = host.move_to(Point::new(inside.x + 80.0, inside.y));
        assert_eq!(
            *dragged.lock().unwrap(),
            ["hello drag world"],
            "{what}: once"
        );
        let _ = host.release(Point::new(inside.x + 80.0, inside.y));
        assert_eq!(
            host.selection(),
            Some((0, 16)),
            "{what}: the selection stays"
        );

        // A press inside the selection that does not move is a click.
        let _ = host.press(inside);
        let _ = host.release(inside);
        let caret = host.selection().unwrap();
        assert_eq!(caret.0, caret.1, "{what}: a caret, not a selection");

        // A declined drag selects from the press.
        let _ = host.probe(true);
        *accept.lock().unwrap() = false;
        let _ = host.press(inside);
        let _ = host.move_to(Point::new(inside.x + 80.0, inside.y));
        let _ = host.release(Point::new(inside.x + 80.0, inside.y));
        let selection = host.selection().unwrap();
        assert_ne!(selection, (0, 16), "{what}: selected anew");
        assert_ne!(selection.0, selection.1, "{what}: a selection");
        *accept.lock().unwrap() = true;
    }

    // A double-click's word selection reads as the word, not as a caret.
    let mut editor = host(true);
    let bounds = editor.probe(false).bounds.unwrap();
    let on_drag = Point::new(bounds.x + 62.0, bounds.center_y());
    let click = [
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
    ];
    let _ = editor.events(&click, on_drag);
    let _ = editor.events(&click, on_drag);
    assert_eq!(editor.doc.content.selection().as_deref(), Some("drag"));
    assert_eq!(editor.selection(), Some((6, 10)), "\"drag\" selected");

    // Without a handler a press inside a selection is a click at once.
    selection_drag::set_handler(None);
    let mut host = host(false);
    let bounds = host.probe(false).bounds.unwrap();
    let inside = Point::new(bounds.x + 30.0, bounds.center_y());
    let _ = host.press(inside);
    let _ = host.release(inside);
    let _ = host.probe(true);
    let _ = host.press(inside);
    let caret = host.selection().unwrap();
    assert_eq!(caret.0, caret.1, "no handler: the press places the caret");
}

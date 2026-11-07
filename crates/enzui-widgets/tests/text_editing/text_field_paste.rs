use std::sync::Arc;

use enzui_widgets::enz_text_field;
use iced::advanced::clipboard::{self, Kind};
use iced::advanced::widget::{Id, Operation};
use iced::keyboard::{self, Key, Modifiers, key};
use iced::{Element, Event, Size, mouse, window};
use iced_test::renderer::Renderer;
use iced_test::runtime::user_interface::{Cache, State, UserInterface};

struct Host {
    renderer: Renderer,
    cache: Cache,
    value: String,
    themed: bool,
}

impl Host {
    fn new(themed: bool) -> Self {
        enzui_widgets::focus_retention::clear_claims();
        Self {
            renderer: crate::honest_frame_driver::new_renderer(),
            cache: Cache::default(),
            value: String::new(),
            themed,
        }
    }

    /// One frame: the field's messages applied to the value, and the
    /// clipboard reads it asked for.
    fn frame(&mut self, focus: bool, events: &[Event]) -> Vec<Kind> {
        let id = Id::new("paste-field");
        let field: Element<'_, String> = if self.themed {
            enz_text_field("Paste", "", &self.value, |value| value)
                .id(id.clone())
                .into()
        } else {
            iced::widget::text_input("Paste", &self.value)
                .on_input(|value| value)
                .id(id.clone())
                .into()
        };
        let mut ui = UserInterface::build(
            field,
            Size::new(400.0, 100.0),
            std::mem::take(&mut self.cache),
            &mut self.renderer,
        );
        if focus {
            ui.operate(
                &self.renderer,
                &mut iced::advanced::widget::operation::focusable::focus::<()>(id)
                    as &mut dyn Operation,
            );
        }
        let mut messages = iced::advanced::shell::Bus::new();
        let (state, _) = ui.update(
            &window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            events,
            mouse::Cursor::Unavailable,
            &mut self.renderer,
            &mut messages,
        );
        self.cache = ui.into_cache();
        if let Some(value) = messages.into_iter().last() {
            self.value = value;
        }
        match state {
            State::Updated { clipboard, .. } => clipboard.reads,
            _ => Vec::new(),
        }
    }
}

fn v(pressed: bool) -> Event {
    let key = Key::Character("v".into());
    let physical_key = key::Physical::Code(key::Code::KeyV);
    Event::Keyboard(if pressed {
        keyboard::Event::KeyPressed {
            modified_key: key.clone(),
            key,
            physical_key,
            location: keyboard::Location::Standard,
            modifiers: Modifiers::CTRL,
            text: None,
            repeat: false,
        }
    } else {
        keyboard::Event::KeyReleased {
            modified_key: key.clone(),
            key,
            physical_key,
            location: keyboard::Location::Standard,
            modifiers: Modifiers::CTRL,
        }
    })
}

fn reply(text: Option<&str>) -> Event {
    Event::Clipboard(clipboard::Event::Read(match text {
        Some(text) => Ok(Arc::new(clipboard::Content::Text(text.into()))),
        None => Err(clipboard::Error::ContentNotAvailable),
    }))
}

#[test]
fn a_quick_paste_lands_once_when_the_clipboard_answers_after_the_release() {
    for themed in [false, true] {
        let mut host = Host::new(themed);
        let ctrl = Event::Keyboard(keyboard::Event::ModifiersChanged(Modifiers::CTRL));
        host.frame(true, &[]);
        assert_eq!(host.frame(false, &[ctrl, v(true), v(false)]), [Kind::Text]);
        host.frame(false, &[reply(Some("one"))]);
        assert_eq!(host.value, "one", "themed={themed}: the paste landed");
        host.frame(false, &[reply(Some("one"))]);
        assert_eq!(host.value, "one", "themed={themed}: once");

        // The next press reads the clipboard again rather than repeating
        // what the last one pasted.
        assert_eq!(host.frame(false, &[v(true)]), [Kind::Text]);
        host.frame(false, &[reply(Some(" two")), v(false)]);
        assert_eq!(host.value, "one two", "themed={themed}");

        // A failed read ends that paste: the next press asks again.
        assert_eq!(host.frame(false, &[v(true), v(false)]), [Kind::Text]);
        host.frame(false, &[reply(None)]);
        assert_eq!(host.frame(false, &[v(true)]), [Kind::Text]);
    }
}

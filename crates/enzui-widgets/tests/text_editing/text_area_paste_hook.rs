use enzui_widgets::text_area::enz_text_area;
use iced::advanced::clipboard::{self, Kind};
use iced::advanced::widget::{Id, Operation};
use iced::keyboard::{Key, Modifiers, key::Named};
use iced::widget::text_editor::{Action, Content, Edit};
use iced::{Event, Size, mouse, window};
use iced_test::renderer::Renderer;
use iced_test::runtime::user_interface::{Cache, State, UserInterface};

const TEXT: &str = "Pasted once: café\nsecond line";

#[derive(Debug, Clone)]
enum Msg {
    Edit(Action),
    Paste,
    Submit,
}

struct Host {
    renderer: Renderer,
    cache: Cache,
    content: Content,
    hook: bool,
    submit: bool,
    enabled: bool,
    read_only: bool,
}

impl Host {
    fn new(hook: bool) -> Self {
        Self {
            renderer: crate::honest_frame_driver::new_renderer(),
            cache: Cache::default(),
            content: Content::new(),
            hook,
            submit: false,
            enabled: true,
            read_only: false,
        }
    }

    fn frame(&mut self, focus: bool, events: &[Event]) -> (Vec<Msg>, Vec<Kind>) {
        let mut area = enz_text_area(&self.content, Msg::Edit)
            .id("paste-editor")
            .enabled(self.enabled)
            .read_only(self.read_only);
        if self.hook {
            area = area.on_paste(Msg::Paste);
        }
        if self.submit {
            area = area.on_submit(Msg::Submit);
        }
        let mut ui = UserInterface::build(
            area.into_element(),
            Size::new(500.0, 180.0),
            std::mem::take(&mut self.cache),
            &mut self.renderer,
        );
        if focus {
            ui.operate(
                &self.renderer,
                &mut iced::advanced::widget::operation::focusable::focus::<()>(Id::new(
                    "paste-editor",
                )) as &mut dyn Operation,
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
        for message in &messages {
            if let Msg::Edit(action) = message {
                self.content.perform(action.clone());
            }
        }
        let reads = match state {
            State::Updated { clipboard, .. } => clipboard.reads,
            _ => Vec::new(),
        };
        (messages.into_iter().collect(), reads)
    }

    fn paste_reply(&mut self) -> (Vec<Msg>, Vec<Kind>) {
        self.frame(
            false,
            &[Event::Clipboard(clipboard::Event::Read(Ok(
                std::sync::Arc::new(clipboard::Content::Text(TEXT.into())),
            )))],
        )
    }
}

fn key(key: Key, modifiers: Modifiers) -> Event {
    Event::Keyboard(iced::keyboard::Event::KeyPressed {
        modified_key: key.clone(),
        key,
        physical_key: iced::keyboard::key::Physical::Unidentified(
            iced::keyboard::key::NativeCode::Unidentified,
        ),
        location: iced::keyboard::Location::Standard,
        modifiers,
        text: None,
        repeat: false,
    })
}

#[test]
fn paste_hook_composes_with_submit_and_shift_enter() {
    let mut host = Host::new(true);
    host.submit = true;
    let (messages, reads) = host.frame(true, &[key(Key::Named(Named::Enter), Modifiers::empty())]);
    assert!(matches!(messages.as_slice(), [Msg::Submit]));
    assert!(reads.is_empty());
    assert_eq!(host.content.text(), "");
    let (messages, reads) = host.frame(false, &[key(Key::Named(Named::Enter), Modifiers::SHIFT)]);
    assert!(matches!(
        messages.as_slice(),
        [Msg::Edit(Action::Edit(Edit::Enter))]
    ));
    assert!(reads.is_empty());
    assert_eq!(host.content.text(), "\n");
    let (messages, reads) = host.frame(
        false,
        &[key(Key::Character("v".into()), Modifiers::COMMAND)],
    );
    assert!(matches!(messages.as_slice(), [Msg::Paste]));
    assert_eq!(reads, vec![Kind::Text]);
    host.paste_reply();
    assert_eq!(host.content.text(), format!("\n{TEXT}"));
}

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

/// A composer hands its paste hook to its input, so a chat can read a pasted
/// picture while the text paste goes on as before.
#[test]
fn composer_forwards_its_paste_hook() {
    let content = Content::new();
    let mut renderer = crate::honest_frame_driver::new_renderer();
    let mut cache = Cache::default();
    let mut frame = |hook: bool, focus: bool, events: &[Event]| {
        let mut composer = enzui_widgets::composer::enz_composer(&content, Msg::Edit)
            .on_send(Msg::Submit)
            .id("paste-composer");
        if hook {
            composer = composer.on_paste(Msg::Paste);
        }
        let mut ui = UserInterface::build(
            composer.into_element(),
            Size::new(500.0, 180.0),
            std::mem::take(&mut cache),
            &mut renderer,
        );
        if focus {
            ui.operate(
                &renderer,
                &mut iced::advanced::widget::operation::focusable::focus::<()>(Id::new(
                    "paste-composer",
                )) as &mut dyn Operation,
            );
        }
        let mut messages = iced::advanced::shell::Bus::new();
        let _ = ui.update(
            &window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            events,
            mouse::Cursor::Unavailable,
            &mut renderer,
            &mut messages,
        );
        cache = ui.into_cache();
        messages.into_iter().collect::<Vec<_>>()
    };
    let paste = [key(Key::Character("v".into()), Modifiers::COMMAND)];
    frame(true, true, &[]);
    assert!(
        matches!(frame(true, false, &paste).as_slice(), [Msg::Paste]),
        "the hook is told"
    );
    frame(false, true, &[]);
    assert!(
        !frame(false, false, &paste)
            .iter()
            .any(|m| matches!(m, Msg::Paste)),
        "no hook, no message"
    );
}

/// A card composer draws its field around the input and the rail: both sit
/// inside its edge, where a plain composer's reach the sides.
#[test]
fn a_card_composer_frames_its_input_and_rail() {
    use enzui_core::collect::SemanticsCollector;
    use enzui_core::semantics::Role;
    let content = Content::new();
    let mut renderer = crate::honest_frame_driver::new_renderer();
    let mut bounds = |card: bool| {
        let composer = enzui_widgets::composer::enz_composer(&content, Msg::Edit)
            .on_send(Msg::Submit)
            .id("card-composer")
            .card(card);
        let mut ui = UserInterface::build(
            composer.into_element(),
            Size::new(500.0, 180.0),
            Cache::default(),
            &mut renderer,
        );
        let mut c = SemanticsCollector::new();
        ui.operate(&renderer, &mut c as &mut dyn Operation);
        let nodes = c.into_collected();
        let find = |role: Role| {
            nodes
                .iter()
                .find(|n| n.semantics.role == role)
                .expect("published")
                .bounds
        };
        (find(Role::TextInput), find(Role::Button))
    };
    let (input, send) = bounds(false);
    assert!(
        input.x < 0.5 && send.x + send.width > 499.5,
        "sanity: a plain composer reaches the sides"
    );
    let (input, send) = bounds(true);
    assert!(input.x >= 1.0, "the input is inside the card: {input:?}");
    assert!(
        send.x + send.width <= 499.0,
        "the rail is inside the card: {send:?}"
    );
}

/// With wrapped rows, a long single line grows the input as it wraps; by
/// logical lines it stays one row.
#[test]
fn a_wrapping_line_grows_a_wrapped_rows_composer() {
    use enzui_core::collect::SemanticsCollector;
    use enzui_core::semantics::Role;
    let content = Content::with_text(&"many words that wrap ".repeat(12));
    let mut renderer = crate::honest_frame_driver::new_renderer();
    let mut height = |wrapped: bool| {
        let composer = enzui_widgets::composer::enz_composer(&content, Msg::Edit)
            .on_send(Msg::Submit)
            .id("wrap-composer")
            .max_rows(8)
            .wrapped_rows(wrapped);
        let mut ui = UserInterface::build(
            composer.into_element(),
            Size::new(220.0, 400.0),
            Cache::default(),
            &mut renderer,
        );
        // Wrapped height settles once the editor has laid out its text.
        ui.draw(
            &mut renderer,
            &iced::Theme::Dark,
            &iced::advanced::renderer::Style::default(),
            mouse::Cursor::Unavailable,
        );
        let mut ui = UserInterface::build(
            enzui_widgets::composer::enz_composer(&content, Msg::Edit)
                .on_send(Msg::Submit)
                .id("wrap-composer")
                .max_rows(8)
                .wrapped_rows(wrapped)
                .into_element(),
            Size::new(220.0, 400.0),
            ui.into_cache(),
            &mut renderer,
        );
        let mut c = SemanticsCollector::new();
        ui.operate(&renderer, &mut c as &mut dyn Operation);
        c.into_collected()
            .into_iter()
            .find(|n| n.semantics.role == Role::TextInput)
            .expect("published")
            .bounds
            .height
    };
    let one = height(false);
    let grown = height(true);
    assert!(
        grown > one * 2.0,
        "the wrapped line grows the input: {one} -> {grown}"
    );
}

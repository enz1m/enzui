//! Search-field remounts retain keystrokes and exact caret without relying on shell recovery.

use enzui_widgets::app_shell::{ShellLayout, enz_app_shell};
use enzui_widgets::button::enz_button;
use enzui_widgets::scroll_column::enz_scroll_column;
use enzui_widgets::search_field::enz_search_field;
use iced::advanced::widget::Operation;
use iced::advanced::widget::operation::Outcome;
use iced::keyboard::key::{NativeCode, Physical};
use iced::keyboard::{Key, Modifiers};
use iced::widget::{Space, column, container, stack};
use iced::{Element, Event, Size, Theme};
use iced_test::runtime::user_interface::{Cache, UserInterface};

type Renderer = iced_test::renderer::Renderer;

const PREFIX: &str = "retention";
const PLACEHOLDER: &str = "Search…";
const OTHER_ID: &str = "retention-1394.other";

fn target() -> iced::advanced::widget::Id {
    enzui_core::identity::derive_id(PREFIX, PLACEHOLDER)
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(
    dead_code,
    reason = "the widget needs these messages to exist; this test never sends them"
)]
enum Shape {
    WrapperAdded,
    BranchSwap,
    SiblingOnly,
    MaterialToggle,
}

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Input(String),
    Other,
}

struct Harness {
    value: String,
    shape: Shape,
    retain: bool,
    // Omitting the setter distinguishes the shipped default from explicit opt-in.
    use_default_retention: bool,
    shell: bool,
    show_field: bool,
    renderer: Renderer,
    cache: Cache,
}

impl Harness {
    fn new(shape: Shape) -> Self {
        enzui_widgets::focus_retention::clear_claims();
        let renderer = iced_test::futures::futures::executor::block_on(
            <Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
            ),
        )
        .expect("headless renderer");
        Self {
            value: String::new(),
            shape,
            retain: true,
            use_default_retention: false,
            shell: false,
            show_field: true,
            renderer,
            cache: Cache::default(),
        }
    }

    fn view(&self) -> Element<'static, Msg> {
        let value: &'static str = Box::leak(self.value.clone().into_boxed_str());
        let non_empty = !value.is_empty();

        let built = enz_search_field(value, Msg::Input).id_prefix(PREFIX);
        let built = if self.shape == Shape::MaterialToggle && value.len() % 2 == 1 {
            built.material(enzui_theme::MaterialStyle::Regular)
        } else {
            built
        };
        let field: Element<'static, Msg> = if self.use_default_retention {
            built.into()
        } else {
            built.retain_focus(self.retain).into()
        };

        let dynamic: Element<'static, Msg> = if !self.show_field {
            Space::new().height(32).into()
        } else {
            match self.shape {
                Shape::WrapperAdded if non_empty => enz_scroll_column(vec![field]).into(),
                Shape::WrapperAdded => field,
                Shape::BranchSwap if non_empty => stack![field].into(),
                Shape::BranchSwap => enz_scroll_column(vec![field]).into(),
                Shape::SiblingOnly | Shape::MaterialToggle => field,
            }
        };

        let sibling: Element<'static, Msg> = match (self.shape, non_empty) {
            (Shape::SiblingOnly, true) => container(Space::new().height(8)).into(),
            (Shape::SiblingOnly, false) => stack![Space::new().height(8)].into(),
            _ => Space::new().height(8).into(),
        };

        let body: Element<'static, Msg> = column![
            dynamic,
            sibling,
            enz_button("Other", Msg::Other)
                .id(iced::advanced::widget::Id::new(OTHER_ID))
                .into_element()
        ]
        .spacing(8)
        .into();

        if !self.shell {
            return body;
        }
        let cell = std::cell::RefCell::new(Some(body));
        enz_app_shell(move || cell.borrow_mut().take().expect("one build"))
            .layout(ShellLayout::TitlebarContent)
            .titlebar(|| Space::new().width(iced::Length::Fill).height(24).into())
            .into()
    }

    fn build(&mut self) -> UserInterface<'static, Msg, Theme, Renderer> {
        UserInterface::build(
            self.view(),
            Size::new(700.0, 420.0),
            std::mem::take(&mut self.cache),
            &mut self.renderer,
        )
    }

    fn operate(&mut self, operation: &mut dyn Operation<()>) {
        let mut ui = self.build();
        ui.operate(&self.renderer, operation);
        self.cache = ui.into_cache();
    }

    fn focus(&mut self, id: iced::advanced::widget::Id) {
        let mut operation = iced::advanced::widget::operation::focusable::focus::<()>(id);
        self.operate(&mut operation);
    }

    fn focused(&mut self) -> Option<String> {
        let mut find = iced::advanced::widget::operation::focusable::find_focused();
        let mut boxed: Box<dyn Operation<()>> =
            Box::new(iced::advanced::widget::operation::black_box(&mut find));
        self.operate(&mut boxed);
        drop(boxed);
        match find.finish() {
            Outcome::Some(id) => Some(format!("{id:?}")),
            _ => None,
        }
    }

    fn rebuild(&mut self) {
        let ui = self.build();
        self.cache = ui.into_cache();
    }

    fn type_char(&mut self, c: char) {
        let key = Key::Character(iced_test::core::SmolStr::new(c.to_string()));
        let events = [
            Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: key.clone(),
                modified_key: key.clone(),
                physical_key: Physical::Unidentified(NativeCode::Unidentified),
                location: iced::keyboard::Location::Standard,
                modifiers: Modifiers::default(),
                repeat: false,
                text: Some(iced_test::core::SmolStr::new(c.to_string())),
            }),
            Event::Keyboard(iced::keyboard::Event::KeyReleased {
                key: key.clone(),
                modified_key: key,
                physical_key: Physical::Unidentified(NativeCode::Unidentified),
                location: iced::keyboard::Location::Standard,
                modifiers: Modifiers::default(),
            }),
        ];
        let mut messages = iced::advanced::shell::Bus::new();
        let mut ui = self.build();
        ui.update(
            &iced::advanced::graphics::core::window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            &events,
            iced::advanced::mouse::Cursor::Unavailable,
            &mut self.renderer,
            &mut messages,
        );
        self.cache = ui.into_cache();
        for message in messages {
            if let Msg::Input(value) = message {
                self.value = value;
            }
        }
    }

    fn type_str(&mut self, s: &str) {
        for c in s.chars() {
            self.type_char(c);
        }
    }
}

#[test]
fn a_removed_field_does_not_come_back_focused() {
    let mut h = Harness::new(Shape::WrapperAdded);
    h.focus(target());
    h.type_str("mi");

    h.show_field = false;
    h.rebuild();
    assert_eq!(h.focused(), None, "precondition: the field left the tree");

    std::thread::sleep(enzui_widgets::focus_retention::RETENTION_WINDOW * 2);

    h.show_field = true;
    h.rebuild();
    h.type_char('x');

    assert_eq!(
        h.value, "mi",
        "a field the user navigated away from and back must not silently own \
         the keyboard"
    );
}

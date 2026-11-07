use enzui_core::collect::SemanticsCollector;
use enzui_core::semantics::Role;
use enzui_widgets::{enz_glass, enz_tab_list, enz_titlebar_search};
use iced::advanced::widget::Id;
use iced::{Element, Event, Point, Size, Theme, mouse};
use iced_test::runtime::user_interface::{Cache, UserInterface};

type Renderer = iced_test::renderer::Renderer;
#[derive(Debug, Clone, PartialEq)]
enum Message {
    Search(String),
    Select(usize),
}
fn view(value: &str, selected: usize, reversed: bool) -> Element<'static, Message> {
    let value: &'static str = Box::leak(value.to_owned().into_boxed_str());
    let search: Element<'static, Message> = enz_glass(
        enz_titlebar_search(value, Message::Search)
            .id(Id::new("handoff-search"))
            .placeholder("Search photos")
            .width(340.0),
    )
    .radius(18.0)
    .into();
    let tabs: Element<'static, Message> = enz_glass(
        enz_tab_list(["Years", "Months", "Days"], selected, Message::Select)
            .id(Id::new("handoff-tabs"))
            .aria_label("Library grouping"),
    )
    .radius(18.0)
    .into();
    let children = if reversed {
        vec![search, tabs]
    } else {
        vec![tabs, search]
    };
    iced::widget::container(iced::widget::row(children).spacing(20))
        .padding(20)
        .into()
}
struct Harness {
    value: String,
    selected: usize,
    reversed: bool,
    cache: Cache,
    renderer: Renderer,
    messages: Vec<Message>,
}
impl Harness {
    fn new(reversed: bool) -> Self {
        Self {
            value: String::new(),
            selected: 0,
            reversed,
            cache: Cache::default(),
            renderer: crate::honest_frame_driver::new_renderer(),
            messages: Vec::new(),
        }
    }
    fn build(&mut self) -> UserInterface<'static, Message, Theme, Renderer> {
        UserInterface::build(
            view(&self.value, self.selected, self.reversed),
            Size::new(1280.0, 300.0),
            std::mem::take(&mut self.cache),
            &mut self.renderer,
        )
    }
    fn nodes(&mut self) -> Vec<enzui_core::collect::CollectedNode> {
        let mut ui = self.build();
        let mut collector = SemanticsCollector::new();
        ui.operate(&self.renderer, &mut collector);
        self.cache = ui.into_cache();
        collector.into_collected()
    }
    fn feed(&mut self, event: Event, cursor: mouse::Cursor) {
        let mut ui = self.build();
        let mut bus = iced::advanced::shell::Bus::new();
        ui.update(
            &iced::window::Headless,
            &iced_test::core::shell::Waker::noop(),
            &[event],
            cursor,
            &mut self.renderer,
            &mut bus,
        );
        self.cache = ui.into_cache();
        for message in bus {
            match &message {
                Message::Search(v) => self.value.clone_from(v),
                Message::Select(i) => self.selected = *i,
            }
            self.messages.push(message);
        }
    }
    fn press(&mut self, point: Point, touch: bool) {
        let events = if touch {
            vec![
                Event::Touch(iced::touch::Event::FingerPressed {
                    id: iced::touch::Finger(1),
                    position: point,
                }),
                Event::Touch(iced::touch::Event::FingerLifted {
                    id: iced::touch::Finger(1),
                    position: point,
                }),
            ]
        } else {
            vec![
                Event::Mouse(mouse::Event::CursorMoved { position: point }),
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
            ]
        };
        for event in events {
            self.feed(event, mouse::Cursor::Available(point));
        }
    }
    fn key(&mut self, key: iced::keyboard::Key, text: Option<&str>) {
        self.feed(
            Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: key.clone(),
                modified_key: key,
                physical_key: iced::keyboard::key::Physical::Unidentified(
                    iced::keyboard::key::NativeCode::Unidentified,
                ),
                location: iced::keyboard::Location::Standard,
                modifiers: Default::default(),
                text: text.map(Into::into),
                repeat: false,
            }),
            mouse::Cursor::Unavailable,
        );
    }
    fn assert_owner(&mut self, role: Role) {
        let nodes = self.nodes();
        let owners: Vec<_> = nodes.iter().filter(|n| n.semantics.focused).collect();
        assert_eq!(owners.len(), 1, "focus owners: {owners:?}");
        assert_eq!(owners[0].semantics.role, role);
    }
}
#[test]
fn titlebar_search_glass_tabs_exchange_one_keyboard_focus_owner() {
    for (reversed, touch) in [(false, false), (true, false), (false, true), (true, true)] {
        let _scope = enzui_widgets::focus_retention::FocusScope::new();
        let mut h = Harness::new(reversed);
        let nodes = h.nodes();
        let search = nodes
            .iter()
            .find(|n| n.semantics.role == Role::TextInput)
            .unwrap()
            .bounds
            .center();
        let months = nodes
            .iter()
            .find(|n| {
                n.semantics.role == Role::Tab && n.semantics.label.as_deref() == Some("Months")
            })
            .unwrap()
            .bounds
            .center();
        h.press(search, touch);
        h.assert_owner(Role::TextInput);
        h.press(months, touch);
        assert_eq!(h.selected, 1, "tab click must select Months");
        h.messages.clear();
        h.key(iced::keyboard::Key::Character("q".into()), Some("q"));
        assert!(
            h.value.is_empty(),
            "typing after a tab click must not edit search: {:?}",
            h.messages
        );
        h.assert_owner(Role::TabList);
        h.key(
            iced::keyboard::Key::Named(iced::keyboard::key::Named::ArrowRight),
            None,
        );
        assert_eq!(h.selected, 2, "the tab strip must own arrow navigation");
        h.press(search, touch);
        h.assert_owner(Role::TextInput);
        h.key(iced::keyboard::Key::Character("q".into()), Some("q"));
        assert_eq!(h.value, "q", "search typing positive control");
        h.key(
            iced::keyboard::Key::Named(iced::keyboard::key::Named::ArrowLeft),
            None,
        );
        assert_eq!(h.selected, 2, "search arrows must not navigate tabs");
    }
}

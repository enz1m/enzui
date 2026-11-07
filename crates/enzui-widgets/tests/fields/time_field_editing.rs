//! `enz_time_field` driven as a user and as an assistive technology drives it:
//! real key, clipboard and input-method events through `UserInterface::update`,
//! real AccessKit requests through `ActionRouter`, and real pixels out of the
//! headless renderer.
//!
//! The model underneath is the one a caller writes — a draft string and a
//! committed time, updated only by `TimeFieldEvent` — so every assertion here
//! is about what actually reached a reducer, not about a builder field.

use enzui_core::time_of_day::{HourCycle, TimeFormat, TimeOfDay};
use enzui_widgets::time_field::{TimeFieldEvent, enz_time_field};
use iced::advanced::widget::Id;
use iced::{Element, Event, Point, Size, keyboard, mouse, window};
use iced_test::runtime::user_interface::{Cache, UserInterface};

type Renderer = iced_test::renderer::Renderer;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Msg {
    Time(TimeFieldEvent),
}

const FIELD_ID: &str = "start-time";
const VIEWPORT: Size = Size::new(400.0, 90.0);

// The cursor parked outside every widget, so a hover can never be mistaken for
// the keyboard or the routed path under test.
const AWAY: Point = Point::new(5000.0, 5000.0);

fn at(hour: u32, minute: u32) -> TimeOfDay {
    TimeOfDay::hm(hour, minute).expect("fixture time is valid")
}

fn field_id() -> Id {
    Id::new(FIELD_ID)
}

// The caller's state: what is on screen, what has been committed, and every
// outcome the field has reported.
struct App {
    value: TimeOfDay,
    draft: String,
    error: Option<String>,
    format: TimeFormat,
    enabled: bool,
    routable: bool,
    pinned_palette: bool,
    commits: Vec<TimeOfDay>,
    refusals: usize,
}

impl App {
    fn new(value: TimeOfDay) -> Self {
        let format = TimeFormat::new(HourCycle::H12);
        Self {
            draft: format.format(value),
            value,
            error: None,
            format,
            enabled: true,
            routable: true,
            pinned_palette: false,
            commits: Vec::new(),
            refusals: 0,
        }
    }

    // The reducer the module documentation prescribes, written out once here so
    // the tests exercise the adoption path rather than a private shortcut.
    fn apply(&mut self, message: Msg) {
        let Msg::Time(event) = message;
        match event {
            TimeFieldEvent::Drafted(text) => self.draft = text,
            TimeFieldEvent::Committed(time) => {
                self.value = time;
                self.draft = self.format.format(time);
                self.error = None;
                self.commits.push(time);
            }
            TimeFieldEvent::Refused(_) => {
                self.error = Some("Enter a time".to_owned());
                self.refusals += 1;
            }
        }
    }
}

fn build<'a>(app: &'a App, draft: &'a str) -> Element<'a, Msg, iced::Theme, Renderer> {
    let mut field = enz_time_field(draft, app.value, Msg::Time)
        .format(app.format.clone())
        .aria_label("Start")
        // Pinned, so the pixel band the invalid-state gate reads stays below a
        // known field height rather than below whatever the default resolves to.
        .size(enzui_widgets::text_field::TextFieldSize::Md)
        .enabled(app.enabled);
    if app.routable {
        field = field.id(field_id());
    }
    if app.pinned_palette {
        field = field.palette(enzui_theme::Palette::dark());
    }
    if let Some(error) = app.error.clone() {
        field = field.error(error);
    }
    field.into()
}

// A live tree rebuilt from the model after every event, the way a real
// application loop rebuilds it — so a keystroke sees the draft the previous
// keystroke produced.
struct Driver {
    app: App,
    cache: Cache,
    renderer: Renderer,
    size: Size,
    statuses: Vec<iced::event::Status>,
}

impl Driver {
    fn new(app: App) -> Self {
        Self::with_size(app, VIEWPORT)
    }

    fn with_size(app: App, size: Size) -> Self {
        let renderer = iced_test::futures::futures::executor::block_on(
            <Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
            ),
        )
        .expect("headless renderer");
        Self {
            app,
            cache: Cache::default(),
            renderer,
            size,
            statuses: Vec::new(),
        }
    }

    fn feed(&mut self, events: &[Event], cursor: Point) {
        let Self {
            app,
            cache,
            renderer,
            size,
            statuses,
        } = self;
        let draft = app.draft.clone();
        let element = build(app, &draft);
        let mut ui = UserInterface::build(element, *size, std::mem::take(cache), renderer);
        let mut messages = iced::advanced::shell::Bus::new();
        let (_, produced) = ui.update(
            &window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            events,
            mouse::Cursor::Available(cursor),
            renderer,
            &mut messages,
        );
        *statuses = produced;
        *cache = ui.into_cache();
        drop(draft);
        for message in messages {
            app.apply(message);
        }
    }

    fn click_at(&mut self, point: Point) {
        for event in [
            Event::Mouse(mouse::Event::CursorMoved { position: point }),
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
        ] {
            self.feed(&[event], point);
        }
    }

    fn focus_field(&mut self) {
        self.click_at(Point::new(30.0, 16.0));
    }

    fn click_away(&mut self) {
        self.click_at(Point::new(self.size.width - 4.0, self.size.height - 4.0));
    }

    // Types `text` one character at a time, each in its own frame, the way a
    // keyboard delivers it.
    fn type_text(&mut self, text: &str) {
        for character in text.chars() {
            let key = keyboard::Key::Character(character.to_string().into());
            self.feed(
                &[key_event(
                    key,
                    Some(character.to_string().as_str()),
                    keyboard::Modifiers::default(),
                )],
                AWAY,
            );
        }
    }

    fn select_all(&mut self) {
        let command = keyboard::Modifiers::COMMAND;
        self.feed(
            &[Event::Keyboard(keyboard::Event::ModifiersChanged(command))],
            AWAY,
        );
        self.feed(
            &[key_event(
                keyboard::Key::Character("a".into()),
                None,
                command,
            )],
            AWAY,
        );
        self.feed(
            &[Event::Keyboard(keyboard::Event::ModifiersChanged(
                keyboard::Modifiers::default(),
            ))],
            AWAY,
        );
    }
}

fn key_event(key: keyboard::Key, text: Option<&str>, modifiers: keyboard::Modifiers) -> Event {
    Event::Keyboard(keyboard::Event::KeyPressed {
        key: key.clone(),
        modified_key: key,
        physical_key: keyboard::key::Physical::Unidentified(
            keyboard::key::NativeCode::Unidentified,
        ),
        location: keyboard::Location::Standard,
        modifiers,
        text: text.map(Into::into),
        repeat: false,
    })
}

// --- typing ------------------------------------------------------------------

// --- stepping ----------------------------------------------------------------

// --- cancelling and leaving --------------------------------------------------

#[test]
fn losing_focus_commits_a_readable_draft() {
    let mut driver = Driver::new(App::new(at(9, 0)));
    driver.focus_field();
    driver.select_all();
    driver.type_text("2:00 PM");
    assert!(driver.app.commits.is_empty());
    driver.click_away();
    assert_eq!(
        driver.app.commits,
        vec![at(14, 0)],
        "leaving a field with a readable time must keep it"
    );
    assert_eq!(driver.app.value, at(14, 0));
}

// --- accessibility -----------------------------------------------------------

// --- routed assistive-technology actions -------------------------------------

// --- painted state -----------------------------------------------------------

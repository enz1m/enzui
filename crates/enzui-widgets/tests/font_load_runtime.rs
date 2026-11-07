//! Font loads driven through the real runtime task, in this target's own
//! process.
//!
//! `FontSystem` is process-global, so a load here would invalidate paragraphs
//! that a concurrently running test holds warm. A separate integration target
//! is a separate process, which is what makes the warmth assertions below
//! meaningful.
//!
//! Truth travels on the single public path: `iced::font::load` resolves to
//! `Result<(), font::Error>`, and the only consequence of a real admission is
//! that the process-global font generation advances. Nothing here reads a
//! bespoke "load report"; a rejected or duplicate load must be invisible to
//! the generation, the font database and any paragraph shaped before it.

#![allow(
    missing_docs,
    unreachable_pub,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::panic,
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    clippy::print_stdout,
    clippy::string_slice,
    clippy::missing_assert_message,
    reason = "a test crate reports by panicking and printing, and none of it is public API"
)]

use enzui_widgets::revision_text::font_generation;

use iced::advanced::graphics::text::cosmic_text::fontdb;
use iced::advanced::graphics::text::{Paragraph, cosmic_text, font_system};
use iced::advanced::text::{self, Difference, LineHeight, Paragraph as _};
use iced::alignment;
use iced::font;
use iced::{Element, Font, Pixels, Size, Task, Theme};
use iced_test::emulator::{Emulator, Event, Mode};
use iced_test::futures::futures::channel::mpsc;
use iced_test::program::Program;

use std::borrow::Cow;
use std::time::{Duration, Instant};

/// Arsenal, redistributed beside this target with its OFL text. Its single
/// face registers under exactly one family.
const FIXTURE: &[u8] = include_bytes!("row_count_generation_font.ttf");
const FAMILY: &str = "Enz Row Count Arsenal";
const TRUNCATED: usize = 200;
const MARKER: &[u8] = b"MARKER-b8f1-not-a-font";
const EMPTY: &[u8] = b"";

const SIZE: Size = Size {
    width: 320.0,
    height: 120.0,
};
const TEXT: &str = "Warm paragraph";
const TEXT_SIZE: f32 = 24.0;

/// A load that never resolves fails the test instead of hanging it.
const DEADLINE: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
enum Message {
    Load(Cow<'static, [u8]>),
    Loaded(Result<(), font::Error>),
}

#[derive(Debug, Default)]
struct State {
    results: Vec<Result<(), font::Error>>,
}

struct FontApp;

impl Program for FontApp {
    type State = State;
    type Message = Message;
    type Theme = Theme;
    type Renderer = iced_test::renderer::Renderer;
    type Executor = iced_test::futures::backend::native::thread_pool::Executor;

    fn name() -> &'static str {
        "font_load_runtime"
    }

    fn settings(&self) -> iced::Settings {
        iced::Settings::default()
    }

    fn window(&self) -> Option<iced::window::Settings> {
        None
    }

    fn boot(&self) -> (State, Task<Message>) {
        (State::default(), Task::none())
    }

    fn update(&self, state: &mut State, message: Message) -> Task<Message> {
        match message {
            Message::Load(bytes) => font::load(bytes).map(Message::Loaded),
            Message::Loaded(result) => {
                state.results.push(result);
                Task::none()
            }
        }
    }

    fn view<'a>(
        &self,
        _state: &'a State,
        _window: iced::window::Id,
    ) -> Element<'a, Message, Theme, Self::Renderer> {
        iced::widget::text(TEXT)
            .font(Font::DEFAULT)
            .size(TEXT_SIZE)
            .into()
    }
}

struct Harness {
    program: FontApp,
    emulator: Emulator<FontApp>,
    events: mpsc::Receiver<Event<FontApp>>,
}

impl Harness {
    fn new() -> Self {
        let program = FontApp;
        let (sender, events) = mpsc::channel(64);
        let emulator = Emulator::new(sender, &program, Mode::Immediate, SIZE);

        Self {
            program,
            emulator,
            events,
        }
    }

    /// Runs one message and pumps the runtime until the program has recorded
    /// `expected` results, or the deadline expires.
    fn run(&mut self, message: Message, expected: usize) {
        self.emulator.update(&self.program, message);

        let started = Instant::now();

        while self.emulator.state().results.len() < expected {
            match self.events.try_recv() {
                Ok(Event::Action(action)) => self.emulator.perform(&self.program, action),
                Ok(Event::Failed(instruction)) => {
                    panic!("the emulator refused {instruction}")
                }
                Ok(Event::Ready) | Err(mpsc::TryRecvError::Empty) => {
                    assert!(
                        started.elapsed() < DEADLINE,
                        "the load task never resolved within {DEADLINE:?}"
                    );

                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(mpsc::TryRecvError::Closed) => {
                    panic!("the emulator closed its event channel")
                }
            }
        }
    }

    fn load(&mut self, bytes: Cow<'static, [u8]>) -> Result<(), font::Error> {
        let expected = self.emulator.state().results.len() + 1;
        self.run(Message::Load(bytes), expected);

        *self.emulator.state().results.last().expect("a load result")
    }
}

/// Whether the font database exposes `family` for selection.
fn family_present(family: &str) -> bool {
    font_system()
        .read()
        .expect("read font system")
        .families()
        .any(|name| name == family)
}

/// The number of faces registered under `family`.
fn faces_in(family: &str) -> usize {
    let mut system = font_system().write().expect("write font system");

    system
        .raw()
        .db()
        .faces()
        .filter(|face| face.families.iter().any(|(name, _)| name == family))
        .count()
}

/// The face `fontdb` resolves for a family name, which is what a widget asking
/// for [`Font::with_family`] actually gets.
fn selected(family: &str) -> Option<fontdb::ID> {
    let mut system = font_system().write().expect("write font system");

    system.raw().db().query(&fontdb::Query {
        families: &[fontdb::Family::Name(family)],
        ..fontdb::Query::default()
    })
}

/// The descriptor a warm paragraph is compared against. `Paragraph::compare`
/// folds the global font generation into its verdict, which is the cache this
/// load path can wrongly invalidate.
fn spec<Content>(content: Content) -> text::Text<Content> {
    text::Text {
        content,
        bounds: SIZE,
        size: Pixels(TEXT_SIZE),
        line_height: LineHeight::Absolute(Pixels(TEXT_SIZE * 1.25)),
        font: Font::DEFAULT,
        align_x: text::Alignment::Left,
        align_y: alignment::Vertical::Top,
        shaping: text::Shaping::Advanced,
        wrapping: text::Wrapping::Word,
        ellipsis: text::Ellipsis::None,
        hint_factor: None,
    }
}

/// The faces the real shaper picks for text in `family`.
fn shaped_faces(family: &str) -> Vec<fontdb::ID> {
    let mut system = font_system().write().expect("write font system");
    let raw = system.raw();

    let mut buffer =
        cosmic_text::Buffer::new(raw, cosmic_text::Metrics::new(TEXT_SIZE, TEXT_SIZE * 1.25));
    buffer.set_size(Some(SIZE.width), Some(SIZE.height));
    buffer.set_text(
        TEXT,
        &cosmic_text::Attrs::new().family(cosmic_text::Family::Name(family)),
        cosmic_text::Shaping::Advanced,
        None,
    );
    buffer.shape_until_scroll(raw, false);

    buffer
        .layout_runs()
        .flat_map(|run| run.glyphs.iter().map(|glyph| glyph.font_id))
        .collect()
}

#[test]
fn font_loads_report_truthfully_and_only_a_new_face_reshapes() {
    let mut harness = Harness::new();

    assert!(
        !family_present(FAMILY),
        "the private family must begin unresolved"
    );

    let mut generation = font_generation();
    let warm_paragraph = Paragraph::with_text(spec(TEXT));
    assert_eq!(
        warm_paragraph.compare(spec(())),
        Difference::None,
        "the paragraph is warm before any load"
    );

    // Empty, truncated and non-font bytes are all rejected, and each must leave
    // the generation, the warm paragraph and the family table untouched.
    for (bytes, offered) in [
        (Cow::Borrowed(EMPTY), 0),
        (Cow::Owned(FIXTURE[..TRUNCATED].to_vec()), TRUNCATED),
        (Cow::Borrowed(MARKER), MARKER.len()),
    ] {
        assert_eq!(
            harness.load(bytes),
            Err(font::Error::InvalidData { bytes: offered }),
            "invalid bytes must report failure, not a silent success"
        );
        assert_eq!(
            font_generation(),
            generation,
            "a rejected load must not advance the generation"
        );
        assert_eq!(
            warm_paragraph.compare(spec(())),
            Difference::None,
            "a rejected load must leave the paragraph warm"
        );
        assert!(
            !family_present(FAMILY),
            "a rejected load must not register the family"
        );
    }

    // The valid TTF is admitted through the same borrowed pointer the duplicate
    // load reuses below.
    assert_eq!(
        harness.load(Cow::Borrowed(FIXTURE)),
        Ok(()),
        "a real font is admitted"
    );

    assert_ne!(
        font_generation(),
        generation,
        "a new face must advance the generation"
    );
    generation = font_generation();

    assert!(family_present(FAMILY), "the admitted family is selectable");
    assert_eq!(faces_in(FAMILY), 1, "the fixture registers a single face");

    let face = selected(FAMILY).expect("the family resolves to a face");
    let faces = shaped_faces(FAMILY);
    assert!(!faces.is_empty(), "the loaded family must shape glyphs");
    assert!(
        faces.iter().all(|shaped| *shaped == face),
        "every glyph must come from the newly admitted face"
    );

    assert_eq!(
        warm_paragraph.compare(spec(())),
        Difference::Shape,
        "a new face must retire paragraphs shaped before it"
    );

    // A paragraph shaped after the admission is itself warm, and the duplicate
    // load must not disturb it.
    let reshaped = Paragraph::with_text(spec(TEXT));

    assert_eq!(
        harness.load(Cow::Borrowed(FIXTURE)),
        Ok(()),
        "loading the same borrowed bytes again is not a failure"
    );
    assert_eq!(
        font_generation(),
        generation,
        "a duplicate borrowed load mutates nothing, so the generation holds"
    );
    assert_eq!(
        reshaped.compare(spec(())),
        Difference::None,
        "a duplicate load must leave the paragraph warm"
    );
}

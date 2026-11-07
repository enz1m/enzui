//! `EnzTextField::quiet` — the opt-in inline value editor: no resting
//! outline, every other painted state and every behaviour intact.
//!
//! Three claims are calibrated separately, because one "it looks quiet"
//! assertion would also pass for a field that had lost its focus ring, its
//! error edge or its editability:
//!
//! 1. Resting paint: no edge at rest on all three render paths the widget has
//!    (plain `text_input`, slotted `FieldChrome`, inert display), in both
//!    shipped palettes, while the value still paints.
//! 2. Surviving states: hover, the 2 px keyboard focus ring and the error
//!    border still paint, each read from the live palette.
//! 3. The identification guard: a field that is unnamed, or paints no text of
//!    its own, keeps the ordinary resting outline and is byte-identical to the
//!    same field without `.quiet(true)`. A leading or trailing slot does not
//!    satisfy the guard, because it identifies its own affordance.
//!
//! The accessibility and interaction half runs through a live
//! `UserInterface`: published node, typing, selection and copy, read-only,
//! disabled, the trailing in-field action, and the numeric contract.

use enzui_core::semantics::{Role as SemRole, Semantics};
use enzui_theme::Palette;
use enzui_widgets::focus_button::focus_button;
use enzui_widgets::icon::enz_icon;
use enzui_widgets::text_field::enz_text_field;
use enzui_widgets::tokens::FieldVariant;
use iced::advanced::renderer::Headless;
use iced::advanced::widget::Id;
use iced::keyboard::key::{NativeCode, Physical};
use iced::keyboard::{Key, Modifiers};
use iced::widget::container;
use iced::{Element, Event, Length, Point, Size, mouse, window};
use iced_test::runtime::user_interface::{Cache, State, UserInterface};

type Renderer = iced_test::renderer::Renderer;

const VIEW_W: u32 = 400;
const VIEW_H: u32 = 200;
const FIELD_W: f32 = 240.0;
const FIELD_ID: &str = "quiet-value";
// The edited value. Wide enough to paint ink across the left half of the
// field, which is where `value_ink_is_painted` looks.
const VALUE: &str = "1.25 cm";
// The accessible name, supplied by `aria_label` because the visible caption
// of an inline value editor lives on the sibling row, not above the box.
const NAME: &str = "Display scale";

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Draft(String),
    Apply,
}

#[derive(Clone, Copy)]
struct Spec {
    quiet: bool,
    named: bool,
    value: &'static str,
    placeholder: &'static str,
    leading: bool,
    trailing: bool,
    variant: FieldVariant,
    error: bool,
    enabled: bool,
    read_only: bool,
}

impl Spec {
    // An ordinary named field holding a value: the shipped default paint.
    const fn ordinary() -> Self {
        Self {
            quiet: false,
            named: true,
            value: VALUE,
            placeholder: "",
            leading: false,
            trailing: false,
            variant: FieldVariant::Filled,
            error: false,
            enabled: true,
            read_only: false,
        }
    }

    // The same field as an inline value editor.
    const fn quiet() -> Self {
        Self {
            quiet: true,
            ..Self::ordinary()
        }
    }
}

fn element(palette: Palette, spec: Spec) -> Element<'static, Msg, iced::Theme, Renderer> {
    let mut field = enz_text_field::<Msg>("", spec.placeholder, spec.value, Msg::Draft)
        .id(Id::new(FIELD_ID))
        .quiet(spec.quiet)
        .variant(spec.variant)
        .enabled(spec.enabled)
        .read_only(spec.read_only)
        // A known-theme pin so the expected channel values are exact; see the
        // `.palette()` docs for why this is not a per-widget restyle.
        .palette(palette);
    if spec.named {
        field = field.aria_label(NAME);
    }
    if spec.error {
        field = field.error("Out of range");
    }
    if spec.leading {
        field = field.leading(enz_icon("action.search").size(16.0));
    }
    if spec.trailing {
        field = field.trailing(
            focus_button(enz_icon("action.confirm").size(16.0), Msg::Apply)
                .semantics(Semantics::new(SemRole::Button).label("Apply")),
        );
    }
    field.into()
}

// The field pinned at the view origin on a `surface`-coloured pane, so its
// top border is physical row 0 and the pane is readable below it.
fn view(palette: Palette, spec: Spec) -> Element<'static, Msg, iced::Theme, Renderer> {
    let pane_fill = palette.surface;
    let boxed = container(element(palette, spec)).width(Length::Fixed(FIELD_W));
    container(boxed)
        .padding(0)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_t: &iced::Theme| container::Style {
            background: Some(iced::Background::Color(pane_fill)),
            ..container::Style::default()
        })
        .into()
}

fn make_renderer() -> Renderer {
    iced_test::futures::futures::executor::block_on(<Renderer as Headless>::new(
        iced::advanced::renderer::Settings::default(),
        std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
    ))
    .expect("headless renderer")
}

const INSIDE: Point = Point { x: 60.0, y: 16.0 };

struct App<'a> {
    ui: UserInterface<'a, Msg, iced::Theme, Renderer>,
    renderer: Renderer,
    messages: iced::advanced::shell::Bus<Msg>,
    cursor: mouse::Cursor,
    clipboard_write: Option<String>,
}

impl<'a> App<'a> {
    fn new(view: Element<'a, Msg, iced::Theme, Renderer>) -> Self {
        let mut renderer = make_renderer();
        let ui = UserInterface::build(
            view,
            Size::new(VIEW_W as f32, VIEW_H as f32),
            Cache::default(),
            &mut renderer,
        );
        Self {
            ui,
            renderer,
            messages: iced::advanced::shell::Bus::new(),
            cursor: mouse::Cursor::Unavailable,
            clipboard_write: None,
        }
    }

    fn feed(&mut self, events: &[Event]) {
        let (state, _) = self.ui.update(
            &window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            events,
            self.cursor,
            &mut self.renderer,
            &mut self.messages,
        );
        if let State::Updated { clipboard, .. } = state
            && let Some(iced::advanced::clipboard::Content::Text(text)) = clipboard.write
        {
            self.clipboard_write = Some(text);
        }
    }

    fn click_at(&mut self, at: Point) {
        self.cursor = mouse::Cursor::Available(at);
        self.feed(&[
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
        ]);
    }

    // `modifiers` must be announced first: `text_input` only reads them from
    // a `ModifiersChanged` event, so `Ctrl+C` otherwise arrives as a bare `c`.
    fn press(&mut self, key: Key, text: Option<&str>, modifiers: Modifiers) {
        let text = text.map(iced_test::core::SmolStr::new);
        self.feed(&[
            Event::Keyboard(iced::keyboard::Event::ModifiersChanged(modifiers)),
            Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: key.clone(),
                modified_key: key.clone(),
                physical_key: Physical::Unidentified(NativeCode::Unidentified),
                location: iced::keyboard::Location::Standard,
                modifiers,
                repeat: false,
                text,
            }),
            Event::Keyboard(iced::keyboard::Event::KeyReleased {
                key: key.clone(),
                modified_key: key,
                physical_key: Physical::Unidentified(NativeCode::Unidentified),
                location: iced::keyboard::Location::Standard,
                modifiers,
            }),
        ]);
    }

    fn typewrite(&mut self, s: &str) {
        for c in s.chars() {
            self.press(
                Key::Character(iced_test::core::SmolStr::new(c.to_string())),
                Some(&c.to_string()),
                Modifiers::default(),
            );
        }
    }
}

fn app(spec: Spec) -> App<'static> {
    App::new(view(Palette::dark(), spec))
}

#[test]
fn a_quiet_field_still_edits_selects_and_copies() {
    let mut app = app(Spec::quiet());
    app.click_at(INSIDE);
    app.typewrite("9");
    assert!(
        app.messages
            .iter()
            .any(|m| matches!(m, Msg::Draft(text) if text.contains('9'))),
        "a quiet field must publish edits, got {:?}",
        app.messages
    );
    app.press(Key::Character("a".into()), Some("a"), Modifiers::CTRL);
    app.press(Key::Character("c".into()), Some("c"), Modifiers::CTRL);
    assert!(
        app.clipboard_write
            .as_deref()
            .is_some_and(|text| text.contains("1.25")),
        "select-all + copy must reach the caret and selection, got {:?}",
        app.clipboard_write
    );
    // A command chord's printable text is a shortcut, never typing (#3677;
    // sabotage: drop the `chord` guard in iced's text_input); AltGr, which
    // arrives as Ctrl+Alt, still types.
    let before = app.messages.len();
    app.press(Key::Character("0".into()), Some("0"), Modifiers::CTRL);
    app.press(Key::Character("=".into()), Some("="), Modifiers::LOGO);
    assert_eq!(
        app.messages.len(),
        before,
        "Ctrl+0 and Super+= must not edit: {:?}",
        app.messages
    );
    app.press(
        Key::Character("@".into()),
        Some("@"),
        Modifiers::CTRL | Modifiers::ALT,
    );
    assert!(
        app.messages
            .iter()
            .skip(before)
            .any(|m| matches!(m, Msg::Draft(text) if text.contains('@'))),
        "AltGr text must still type, got {:?}",
        app.messages
    );
}

/// An address-bar field selects its whole value on the click that focuses
/// it, so typing replaces it; the next click places the caret (#3678).
///
/// Sabotage: drop the `select_all_on_focus` arm of the single click in
/// iced's `text_input`; the first keystroke is inserted into the value.
#[test]
fn an_address_field_selects_all_on_the_focusing_click() {
    let field = enz_text_field("Address", "", VALUE, Msg::Draft)
        .id(Id::new(FIELD_ID))
        .select_all_on_focus(true)
        .subtle_focus(true);
    let mut app = App::new(container(field).width(Length::Fixed(FIELD_W)).into());
    app.click_at(Point::new(40.0, 30.0));
    app.typewrite("x");
    assert_eq!(
        app.messages.iter().last(),
        Some(&Msg::Draft("x".into())),
        "typing replaces the whole value"
    );
    // Far enough and late enough not to be a double click.
    std::thread::sleep(std::time::Duration::from_millis(600));
    app.click_at(Point::new(120.0, 30.0));
    app.typewrite("y");
    assert!(
        matches!(app.messages.iter().last(), Some(Msg::Draft(text)) if text != "y"),
        "the next click places the caret: {:?}",
        app.messages
    );
}

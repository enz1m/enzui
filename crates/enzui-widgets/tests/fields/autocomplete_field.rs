//! `EnzAutocompleteField` — an editable combo box with custom-value commit.

use enzui_core::collect::{CollectedNode, SemanticsCollector};
use enzui_core::semantics::Role;
use enzui_widgets::autocomplete_field::{AutocompleteOption, enz_autocomplete_field};
use iced::advanced::widget::Operation;
use iced::advanced::widget::operation::Outcome;
use iced::{Element, Event, Point, Size, Theme};

use iced_test::runtime::user_interface::{Cache, UserInterface};

const SURFACE_W: f32 = 400.0;
const SURFACE_H: f32 = 600.0;

/// Somewhere far from every widget — see the module docs.
fn parked() -> Point {
    Point::new(5000.0, 5000.0)
}

/// What the host's autocomplete field publishes.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Msg {
    /// An edit. Carries the new text.
    Input(String),
    /// A committed suggestion, carrying its payload — never its label.
    Picked(u32),
    /// A committed custom value.
    Custom(String),
}

fn mouse_press() -> Event {
    Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left))
}

fn mouse_release() -> Event {
    Event::Mouse(iced::mouse::Event::ButtonReleased(
        iced::mouse::Button::Left,
    ))
}

/// Two contacts sharing one display label, plus two distinct ones.
fn duplicate_label_options() -> Vec<AutocompleteOption<u32>> {
    vec![
        AutocompleteOption::new("alex@corp", "Alex Morgan", 1_u32),
        AutocompleteOption::new("alex@home", "Alex Morgan", 2_u32),
        AutocompleteOption::new("alice@corp", "Alice Chen", 3_u32),
    ]
}

struct Host {
    renderer: iced_test::renderer::Renderer,
    cache: Cache,
    /// The caller-owned input text. Real applications set this from `Input`;
    /// tests set it directly to model a refilter.
    value: String,
    /// The suggestions to show. Replaced wholesale to model a refilter.
    options: Vec<AutocompleteOption<u32>>,
    custom_values: bool,
    read_only: bool,
    enabled: bool,
    loading: bool,
    /// An optional custom in-flight notice, exercising `loading_text`.
    loading_text: Option<&'static str>,
    /// When set, `Input` messages rewrite `value` — i.e. the app actually
    /// applies the edit, as a real one does.
    echo_input: bool,
    messages: Vec<Msg>,
    cursor: iced::advanced::mouse::Cursor,
}

impl Host {
    fn new(value: &str, options: Vec<AutocompleteOption<u32>>) -> Self {
        let renderer = iced_test::futures::futures::executor::block_on(
            <iced_test::renderer::Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
            ),
        )
        .expect("headless renderer");
        Self {
            renderer,
            cache: Cache::default(),
            value: value.to_owned(),
            options,
            custom_values: false,
            read_only: false,
            enabled: true,
            loading: false,
            loading_text: None,
            echo_input: true,
            messages: Vec::new(),
            cursor: iced::advanced::mouse::Cursor::Available(parked()),
        }
    }

    fn view(&self) -> Element<'static, Msg> {
        let mut ta = enz_autocomplete_field(
            "To",
            Box::leak(self.value.clone().into_boxed_str()),
            self.options.clone(),
            Msg::Input,
            |id: &u32| Msg::Picked(*id),
        )
        .placeholder("name@example.com")
        .enabled(self.enabled)
        .read_only(self.read_only)
        .loading(self.loading);
        if let Some(t) = self.loading_text {
            ta = ta.loading_text(t);
        }
        if self.custom_values {
            ta = ta.on_custom_value(Msg::Custom);
        }
        ta.into()
    }

    fn build(&mut self) -> UserInterface<'static, Msg, Theme, iced_test::renderer::Renderer> {
        let view = self.view();
        UserInterface::build(
            view,
            Size::new(SURFACE_W, SURFACE_H),
            std::mem::take(&mut self.cache),
            &mut self.renderer,
        )
    }

    /// Push events through one frame, keeping the cache (and therefore the
    /// widget's highlight / dismissal state) alive. Applies `Input` back to the
    /// model when `echo_input`, so the app behaves like a real one.
    fn frame(&mut self, events: &[Event]) -> Vec<Msg> {
        let mut messages = iced::advanced::shell::Bus::new();
        let cursor = self.cursor;
        let mut ui = self.build();
        ui.update(
            &iced::advanced::graphics::core::window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            events,
            cursor,
            &mut self.renderer,
            &mut messages,
        );
        self.cache = ui.into_cache();
        if self.echo_input {
            for m in &messages {
                if let Msg::Input(text) = m {
                    self.value.clone_from(text);
                }
            }
        }
        self.messages.extend(messages.iter().cloned());
        messages.into_iter().collect()
    }

    /// Give the text input keyboard focus, the way Tab would.
    fn focus(&mut self) {
        let mut ui = self.build();
        let mut op = iced::advanced::widget::operation::focusable::focus_next::<()>();
        ui.operate(&self.renderer, &mut op as &mut dyn Operation<()>);
        let mut outcome = op.finish();
        for _ in 0..8 {
            match outcome {
                Outcome::Chain(mut next) => {
                    ui.operate(&self.renderer, &mut next);
                    outcome = next.finish();
                }
                _ => break,
            }
        }
        self.cache = ui.into_cache();
    }

    fn collect(&mut self) -> Vec<CollectedNode> {
        let mut ui = self.build();
        let mut c = SemanticsCollector::new();
        ui.operate(&self.renderer, &mut c as &mut dyn Operation);
        self.cache = ui.into_cache();
        c.into_collected()
    }

    /// The screen-space centre of suggestion row `index`, from its published
    /// node's bounds.
    fn row_center(&mut self, index: usize) -> Option<Point> {
        let b = self
            .collect()
            .into_iter()
            .filter(|n| n.semantics.role == Role::ListBoxOption)
            .nth(index)?
            .bounds;
        Some(Point::new(b.x + b.width / 2.0, b.y + b.height / 2.0))
    }

    /// Press at `press`, move, then release at `release` — a drag.
    fn drag(&mut self, press: Point, release: Point) -> Vec<Msg> {
        self.cursor = iced::advanced::mouse::Cursor::Available(press);
        let _ = self.frame(&[mouse_press()]);
        self.cursor = iced::advanced::mouse::Cursor::Available(release);
        let out = self.frame(&[
            Event::Mouse(iced::mouse::Event::CursorMoved { position: release }),
            mouse_release(),
        ]);
        self.cursor = iced::advanced::mouse::Cursor::Available(parked());
        out
    }
}

/// A release in the popup's padding — off every row — commits nothing either.
#[test]
fn releasing_off_every_row_commits_nothing() {
    let mut h = Host::new("al", duplicate_label_options());
    h.focus();
    let first = h.row_center(0).expect("row 0 published");
    // Two rows' worth above the first row is the field itself / the gap — in
    // any case not a suggestion row.
    let off = Point::new(first.x, first.y - 200.0);
    let out = h.drag(first, off);
    assert!(
        out.is_empty(),
        "release off every row: no commit, got {out:?}"
    );
}

/// A click on a suggestion commits it: the press on the popup above does
/// not reach the field below as a press elsewhere that would close it.
#[test]
fn clicking_a_suggestion_commits_it() {
    let mut h = Host::new("al", duplicate_label_options());
    h.focus();
    let row = h.row_center(0).expect("row 0 published");
    let out = h.drag(row, row);
    assert_eq!(out.first(), Some(&Msg::Picked(1)), "the row commits");
}

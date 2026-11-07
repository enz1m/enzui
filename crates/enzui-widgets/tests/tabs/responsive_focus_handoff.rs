use enzui_core::focus::count_focused;
use enzui_widgets::focus_button::focus_button_maybe;
use enzui_widgets::view_switcher::{SwitcherPage, enz_view_switcher};
use iced::advanced::widget::{Id, Operation, operation};
use iced::{Element, Event, Rectangle, Size, keyboard, mouse, window};
use iced_test::runtime::user_interface::{Cache, UserInterface};

const WIDE: f32 = 700.0;
const SWITCHER_NARROW: f32 = 200.0;
const HEIGHT: f32 = 240.0;
const OUTSIDE: &str = "responsive-focus-outside";

type Renderer = iced_test::renderer::Renderer;

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(
    dead_code,
    reason = "the widget needs these messages to exist; this test never sends them"
)]
enum Msg {
    Selected(usize),
    Outside,
    ToastAction,
    ToastDismiss,
    Anonymous,
    Named,
}

fn renderer() -> Renderer {
    iced_test::futures::futures::executor::block_on(
        <Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer")
}

fn key_press(named: keyboard::key::Named) -> Event {
    let key = keyboard::Key::Named(named);
    Event::Keyboard(keyboard::Event::KeyPressed {
        key: key.clone(),
        modified_key: key,
        physical_key: keyboard::key::Physical::Unidentified(
            keyboard::key::NativeCode::Unidentified,
        ),
        location: keyboard::Location::Standard,
        modifiers: keyboard::Modifiers::default(),
        text: None,
        repeat: false,
    })
}

fn switcher_scene() -> Element<'static, Msg> {
    let pages = vec![
        SwitcherPage::new("inbox", "Inbox").with_icon("inbox"),
        SwitcherPage::new("sent", "Sent").with_icon("send"),
        SwitcherPage::new("archive", "Archive").with_icon("archive"),
    ];
    let switcher: Element<'static, Msg> = enz_view_switcher(pages, 0, Msg::Selected).into();
    let outside: Element<'static, Msg> =
        focus_button_maybe(iced::widget::text("Outside"), Some(Msg::Outside))
            .id(Id::new(OUTSIDE))
            .into();
    iced::widget::column![switcher, outside].into()
}

#[derive(Debug, Clone)]
#[allow(dead_code, reason = "fixture fields are written but never read")]
struct FocusRecord {
    id: Option<Id>,
    bounds: Rectangle,
    focused: bool,
}

#[derive(Default)]
struct FocusSnapshot(Vec<FocusRecord>);

impl Operation for FocusSnapshot {
    fn focusable(
        &mut self,
        id: Option<&Id>,
        bounds: Rectangle,
        state: &mut dyn operation::Focusable,
    ) {
        self.0.push(FocusRecord {
            id: id.cloned(),
            bounds,
            focused: state.is_focused(),
        });
    }

    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }
}

// A shared cache retains inactive branch state across width changes.
struct RetainedHarness {
    cache: Option<Cache>,
    renderer: Renderer,
}

impl RetainedHarness {
    fn new() -> Self {
        Self {
            cache: Some(Cache::default()),
            renderer: renderer(),
        }
    }

    fn with_view<R>(
        &mut self,
        width: f32,
        view: Element<'static, Msg>,
        f: impl FnOnce(&mut UserInterface<'_, Msg, iced::Theme, Renderer>, &mut Renderer) -> R,
    ) -> R {
        let cache = self.cache.take().expect("retained cache");
        let mut ui =
            UserInterface::build(view, Size::new(width, HEIGHT), cache, &mut self.renderer);
        let result = f(&mut ui, &mut self.renderer);
        self.cache = Some(ui.into_cache());
        result
    }

    fn run_operation(
        &mut self,
        width: f32,
        view: Element<'static, Msg>,
        mut operation: Box<dyn Operation>,
    ) {
        self.with_view(width, view, |ui, renderer| {
            loop {
                ui.operate(renderer, operation.as_mut());
                match operation.finish() {
                    operation::Outcome::Chain(next) => operation = next,
                    _ => break,
                }
            }
        });
    }

    fn focus_first(&mut self, width: f32, view: Element<'static, Msg>) {
        self.run_operation(width, view, Box::new(operation::focusable::focus_next()));
    }

    fn focus_id(&mut self, width: f32, view: Element<'static, Msg>, id: Id) {
        self.run_operation(width, view, Box::new(operation::focusable::focus(id)));
    }

    fn snapshot(&mut self, width: f32, view: Element<'static, Msg>) -> Vec<FocusRecord> {
        self.with_view(width, view, |ui, renderer| {
            let mut snapshot = FocusSnapshot::default();
            ui.operate(renderer, &mut snapshot);
            snapshot.0
        })
    }

    fn focused_count(&mut self, width: f32, view: Element<'static, Msg>) -> usize {
        self.with_view(width, view, |ui, renderer| {
            let mut count = count_focused();
            let mut boxed = operation::black_box(&mut count);
            ui.operate(renderer, &mut boxed);
            drop(boxed);
            match count.finish() {
                operation::Outcome::Some(count) => count,
                other => panic!("focus count did not finish: {other:?}"),
            }
        })
    }

    fn press_key(
        &mut self,
        width: f32,
        view: Element<'static, Msg>,
        key: keyboard::key::Named,
    ) -> Vec<Msg> {
        self.with_view(width, view, |ui, renderer| {
            let mut messages = iced::advanced::shell::Bus::new();
            ui.update(
                &window::Headless,
                &iced::advanced::graphics::core::shell::Waker::noop(),
                &[key_press(key)],
                mouse::Cursor::Unavailable,
                renderer,
                &mut messages,
            );
            messages.into_iter().collect()
        })
    }
}

fn anonymous_switcher(records: &[FocusRecord]) -> &FocusRecord {
    records
        .iter()
        .find(|record| record.id.is_none())
        .expect("the tab-list focus owner")
}

#[test]
fn hidden_switcher_focus_cannot_resurrect_after_outside_focus() {
    let mut harness = RetainedHarness::new();
    harness.focus_first(WIDE, switcher_scene());
    let compact = harness.snapshot(SWITCHER_NARROW, switcher_scene());
    assert!(
        anonymous_switcher(&compact).focused,
        "calibration: compact received focus before the outside move"
    );
    harness.focus_id(SWITCHER_NARROW, switcher_scene(), Id::new(OUTSIDE));
    assert_eq!(
        harness.focused_count(SWITCHER_NARROW, switcher_scene()),
        1,
        "outside focus must leave one owner while compact is visible"
    );
    let wide = harness.snapshot(WIDE, switcher_scene());
    let switcher = anonymous_switcher(&wide);
    let outside = wide
        .iter()
        .find(|record| record.id.as_ref() == Some(&Id::new(OUTSIDE)))
        .expect("outside button focus record");
    assert!(!switcher.focused, "wide focus must not resurrect: {wide:?}");
    assert!(
        outside.focused,
        "outside button must remain focused: {wide:?}"
    );
    assert_eq!(
        harness.focused_count(WIDE, switcher_scene()),
        1,
        "only the outside button may own focus after returning wide"
    );
    assert!(
        harness
            .press_key(WIDE, switcher_scene(), keyboard::key::Named::ArrowRight)
            .is_empty(),
        "an unfocused switcher must ignore Right Arrow"
    );
}

// overlay-normalization: modal-settings
use enzui_widgets::modal_host::{ModalFocusLifecycle, ModalHostEvent};
use iced::Element;
use iced::advanced::widget::operation::focusable::Focusable;
use iced::advanced::widget::{Id, Operation};
use iced_test::runtime::user_interface::{Cache, UserInterface};

#[derive(Debug, Clone, PartialEq)]
#[allow(
    dead_code,
    reason = "the widget needs these messages to exist; this test never sends them"
)]
enum Msg {
    Bg,
    Host(ModalHostEvent),
}

#[derive(Default)]
struct FocusedIds {
    focused: Vec<Option<Id>>,
}

impl Operation for FocusedIds {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }
    fn focusable(&mut self, id: Option<&Id>, _bounds: iced::Rectangle, state: &mut dyn Focusable) {
        if state.is_focused() {
            self.focused.push(id.cloned());
        }
    }
}

fn headless() -> iced_test::renderer::Renderer {
    iced_test::futures::futures::executor::block_on(
        <iced_test::renderer::Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer")
}

fn run<M: 'static>(
    ui: &mut UserInterface<'_, M, iced::Theme, iced_test::renderer::Renderer>,
    renderer: &iced_test::renderer::Renderer,
    op: Box<dyn Operation<()>>,
) {
    let mut current = op;
    enzui_test_support::chained_operation::operate_chained(
        |o| ui.operate(renderer, o),
        current.as_mut(),
    );
}

fn focused<M: 'static>(
    ui: &mut UserInterface<'_, M, iced::Theme, iced_test::renderer::Renderer>,
    renderer: &iced_test::renderer::Renderer,
) -> Vec<Option<Id>> {
    let mut f = FocusedIds::default();
    ui.operate(renderer, &mut f as &mut dyn Operation);
    f.focused
}

fn focus_btn(label: &'static str, id: Id) -> Element<'static, Msg> {
    enzui_widgets::focus_button::focus_button(iced::widget::text(label), Msg::Bg)
        .id(id)
        .into()
}

fn build<'a>(
    element: Element<'a, Msg>,
    renderer: &mut iced_test::renderer::Renderer,
) -> UserInterface<'a, Msg, iced::Theme, iced_test::renderer::Renderer> {
    build_with(element, Cache::default(), renderer)
}

/// Rebuild carrying `cache`, which preserves widget state — crucially focus —
/// across the view rebuild the way iced does between frames. A fresh
/// `Cache::default()` would silently drop focus, so the exit-retention test
/// must thread the cache to make its "focus persisted into retention" claim
/// honest.
fn build_with<'a>(
    element: Element<'a, Msg>,
    cache: Cache,
    renderer: &mut iced_test::renderer::Renderer,
) -> UserInterface<'a, Msg, iced::Theme, iced_test::renderer::Renderer> {
    UserInterface::build(element, iced::Size::new(800.0, 600.0), cache, renderer)
}

#[test]
fn host_driven_closure_steals_no_focus() {
    let bg = [Id::unique(), Id::unique()];
    let invoker = bg[0].clone();
    let lifecycle = ModalFocusLifecycle::open::<()>("lc-host", Some(invoker)).0;

    let element: Element<'_, Msg> =
        iced::widget::column![focus_btn("a", bg[0].clone()), focus_btn("b", bg[1].clone())].into();
    let mut renderer = headless();
    let mut ui = build(element, &mut renderer);

    // Focus the SECOND control — somewhere other than the invoker.
    run(
        &mut ui,
        &renderer,
        Box::new(iced::advanced::widget::operation::focusable::focus(
            bg[1].clone(),
        )),
    );
    assert_eq!(focused(&mut ui, &renderer), vec![Some(bg[1].clone())]);

    let outcome = lifecycle.close_for_host::<()>();
    assert_eq!(
        outcome.dismissed(),
        None,
        "a host-driven close is not a user dismissal"
    );
    assert!(
        outcome.into_operation().is_none(),
        "a host-driven close must produce NO focus operation — nothing to run"
    );
    // Focus is untouched: still on the second control, not yanked to the invoker.
    assert_eq!(
        focused(&mut ui, &renderer),
        vec![Some(bg[1].clone())],
        "host-driven closure must not steal focus back to the invoker"
    );
}

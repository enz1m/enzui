//! One shared [`enzui_core::focus::RestoreFocus`], two consumers.

use enzui_core::menu_model::{CloseCause, MenuEntryModel, MenuModel, MenuState};
use iced::Element;
use iced::advanced::widget::{Id, Operation, operation};

type Renderer = iced_test::renderer::Renderer;
use iced_test::runtime::user_interface::{Cache, UserInterface};

const FIRST_DECOY: &str = "restore-shared-first-decoy";
const TARGET_ID: &str = "restore-shared-target";
const THIRD_DECOY: &str = "restore-shared-third-decoy";

#[derive(Debug, Clone, PartialEq)]
#[allow(
    dead_code,
    reason = "the widget needs these messages to exist; this test never sends them"
)]
enum Msg {
    First,
    Third,
    Menu(enzui_widgets::menu_button::MenuButtonEvent),
    Ctx(enzui_widgets::context_menu_host::ContextMenuEvent),
    Ignored,
}

fn model() -> MenuModel {
    MenuModel::new(vec![
        MenuEntryModel::command("save", "Save"),
        MenuEntryModel::command("quit", "Quit"),
    ])
}

/// A [`UserInterface`] holding an [`Element`] and the borrows the element
/// needs to live for. `UserInterface<'a>` borrows the view, so we keep the
/// view's backing storage alive alongside it.
struct Harness {
    ui: UserInterface<'static, Msg, iced::Theme, Renderer>,
    renderer: Renderer,
    _leaked_model: &'static MenuModel,
    _leaked_menu_state: &'static MenuState,
    _leaked_ctx_state: &'static enzui_widgets::context_menu_host::ContextMenuState,
}

impl Harness {
    fn new_with_view<F>(build_view: F) -> Self
    where
        F: for<'a> FnOnce(
            &'a MenuModel,
            &'a MenuState,
            &'a enzui_widgets::context_menu_host::ContextMenuState,
        ) -> Element<'a, Msg>,
    {
        let leaked_model: &'static MenuModel = Box::leak(Box::new(model()));
        let leaked_menu_state: &'static MenuState = Box::leak(Box::new(MenuState::closed()));
        let leaked_ctx_state: &'static enzui_widgets::context_menu_host::ContextMenuState =
            Box::leak(Box::new(
                enzui_widgets::context_menu_host::ContextMenuState::default(),
            ));

        let view = build_view(leaked_model, leaked_menu_state, leaked_ctx_state);

        let mut renderer = iced_test::futures::futures::executor::block_on(
            <Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
            ),
        )
        .expect("headless renderer");

        let ui = UserInterface::build(
            view,
            iced::Size::new(1024.0, 768.0),
            Cache::default(),
            &mut renderer,
        );

        Self {
            ui,
            renderer,
            _leaked_model: leaked_model,
            _leaked_menu_state: leaked_menu_state,
            _leaked_ctx_state: leaked_ctx_state,
        }
    }

    /// Focus `id` up-front, so a restore that fails silently ends up leaving
    /// the *decoy* focused rather than nothing focused — a null result cannot
    /// masquerade as a pass.
    fn focus(&mut self, id: &Id) {
        let mut op: Box<dyn Operation<()>> = Box::new(operation::focusable::focus(id.clone()));
        loop {
            self.ui.operate(&self.renderer, &mut *op);
            match op.finish() {
                operation::Outcome::Chain(next) => op = next,
                _ => break,
            }
        }
    }

    /// Drive `op` to completion (chain through every `Outcome::Chain`), the
    /// same shape every existing restore test uses.
    fn run(&mut self, op: Box<dyn Operation<()>>) {
        let mut op = op;
        loop {
            self.ui.operate(&self.renderer, &mut *op);
            match op.finish() {
                operation::Outcome::Chain(next) => op = next,
                _ => break,
            }
        }
    }

    fn focused_id_debug(&mut self) -> Option<String> {
        let mut find = operation::focusable::find_focused();
        let mut boxed: Box<dyn Operation<()>> = Box::new(operation::black_box(&mut find));
        loop {
            self.ui.operate(&self.renderer, &mut *boxed);
            match boxed.finish() {
                operation::Outcome::Chain(next) => boxed = next,
                _ => break,
            }
        }
        drop(boxed);
        match find.finish() {
            operation::Outcome::Some(id) => Some(format!("{id:?}")),
            _ => None,
        }
    }
}

fn decoy_row_with_menu_button<'a>(
    m: &'a MenuModel,
    menu_state: &'a MenuState,
    trigger_id: Id,
) -> Element<'a, Msg> {
    let first = enzui_widgets::focus_button::focus_button(iced::widget::text("First"), Msg::First)
        .id(Id::new(FIRST_DECOY))
        .width(iced::Length::Fixed(80.0))
        .height(iced::Length::Fixed(30.0));

    let button = enzui_widgets::menu_button::enz_menu_button("File", m, menu_state, Msg::Menu)
        .menu_id(trigger_id);

    let third = enzui_widgets::focus_button::focus_button(iced::widget::text("Third"), Msg::Third)
        .id(Id::new(THIRD_DECOY))
        .width(iced::Length::Fixed(80.0))
        .height(iced::Length::Fixed(30.0));

    let first_e: Element<'a, Msg> = first.into();
    let button_e: Element<'a, Msg> = button.into();
    let third_e: Element<'a, Msg> = third.into();

    iced::widget::row![first_e, button_e, third_e]
        .spacing(12.0)
        .into()
}

/// Consumer: menu_button. Trigger sits *between* two decoys; a restore
/// aimed at it must land on it and not on the first decoy.
#[test]
fn menu_button_restore_lands_on_a_late_trigger_not_the_first_focusable() {
    let trigger_id = Id::new(TARGET_ID);
    let trigger_for_build = trigger_id.clone();
    let mut h = Harness::new_with_view(move |m, menu_state, _ctx| {
        decoy_row_with_menu_button(m, menu_state, trigger_for_build)
    });

    // Park focus on `First` up-front. If the restore silently no-ops, the
    // failure message names `First` rather than reporting a null result.
    h.focus(&Id::new(FIRST_DECOY));
    assert_eq!(
        h.focused_id_debug().as_deref(),
        Some(format!("{:?}", Id::new(FIRST_DECOY)).as_str()),
        "precondition: First must hold focus before the restore is attempted",
    );

    // Now run the restore, routed through the *host's* helper, at the LATE
    // trigger.
    let op = enzui_widgets::menu_button::focus_restore_operation::<()>(
        &enzui_widgets::menu_button::MenuButtonEvent::Close(CloseCause::Escape),
        Some(&trigger_id),
    )
    .expect("Escape must produce a restore operation");
    h.run(op);

    let expected = format!("{trigger_id:?}");
    let actual = h.focused_id_debug();
    assert_eq!(
        actual.as_deref(),
        Some(expected.as_str()),
        "menu_button restore must land on the LATE trigger id {expected}, not on {actual:?} \
         (if this shows `{first:?}`, the shared RestoreFocus stopped chaining `focus(id)` \
         and the fallback landed on the first focusable)",
        first = Id::new(FIRST_DECOY),
    );
}

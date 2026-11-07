//! `DialogStack` keeps the dialogs an app has open: Escape closes the
//! topmost one only, closing a dialog closes the ones over it, and a hand-off
//! replaces one dialog with the next. A dialog's own buttons close it with
//! focus returned, and a busy dialog refuses Escape.

use enzui_widgets::modal_host::{DialogEvent, DialogStack};
use iced::widget::{button, text};
use iced::{Element, Length, Size};
use iced_test::Simulator;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Dialog {
    Settings,
    Confirm,
    Edit,
}

#[derive(Debug, Clone)]
enum Msg {
    Dialog(DialogEvent<Dialog>),
    Pressed,
}

const SIZE: Size = Size::new(640.0, 480.0);

fn view(dialogs: &DialogStack<Dialog>) -> Element<'_, Msg> {
    dialogs.view(
        button("page")
            .on_press(Msg::Pressed)
            .width(Length::Fill)
            .height(Length::Fill),
        |key| text(format!("{key:?} dialog")).into(),
        Msg::Dialog,
    )
}

/// Press Escape over `dialogs`' view and apply what the hosts report.
fn escape(dialogs: &mut DialogStack<Dialog>) -> Vec<Dialog> {
    let events = escape_events(dialogs);
    let mut closed = Vec::new();
    for event in events {
        let (gone, _focus) = dialogs.update::<Msg>(event);
        closed.extend(gone);
    }
    closed
}

/// The events Escape over `dialogs`' view produces, not yet applied.
fn escape_events(dialogs: &DialogStack<Dialog>) -> Vec<DialogEvent<Dialog>> {
    {
        let mut ui = Simulator::with_size(iced::Settings::default(), SIZE, view(dialogs));
        let _ = ui.tap_key(iced::keyboard::key::Named::Escape);
        ui.into_messages()
            .filter_map(|message| match message {
                Msg::Dialog(event) => Some(event),
                Msg::Pressed => None,
            })
            .collect()
    }
}

#[test]
fn dialog_stack_owns_dismissal_replacement_and_focus_1() {
    // an_event_from_a_closed_showing_does_not_close_the_next_one
    {
        let mut dialogs = DialogStack::new();
        let _ = dialogs.open::<Msg>(Dialog::Settings, None);
        let stale = escape_events(&dialogs);
        assert!(!stale.is_empty(), "Escape produced an event");

        // The first showing closes another way, and the key opens again.
        let _ = dialogs.close(&Dialog::Settings);
        let _ = dialogs.open::<Msg>(Dialog::Settings, None);
        for event in stale {
            let (gone, _focus) = dialogs.update::<Msg>(event);
            assert!(gone.is_empty(), "the old showing's Escape closed {gone:?}");
        }
        assert!(dialogs.is_open(&Dialog::Settings));
        assert_eq!(
            escape(&mut dialogs),
            vec![Dialog::Settings],
            "a fresh Escape still closes it"
        );
    }

    // escape_closes_only_the_topmost_dialog
    {
        let mut dialogs = DialogStack::new();
        let _ = dialogs.open::<Msg>(Dialog::Settings, None);
        let _ = dialogs.open::<Msg>(Dialog::Confirm, None);
        assert_eq!(dialogs.topmost(), Some(&Dialog::Confirm));

        assert_eq!(escape(&mut dialogs), vec![Dialog::Confirm]);
        assert!(
            dialogs.is_open(&Dialog::Settings),
            "the covered dialog stays"
        );
        assert_eq!(dialogs.topmost(), Some(&Dialog::Settings));

        assert_eq!(escape(&mut dialogs), vec![Dialog::Settings]);
        assert!(dialogs.is_empty());
    }

    // closing_a_dialog_closes_the_ones_over_it
    {
        let mut dialogs = DialogStack::new();
        let _ = dialogs.open::<Msg>(Dialog::Settings, None);
        let _ = dialogs.open::<Msg>(Dialog::Confirm, None);
        assert_eq!(
            dialogs.close(&Dialog::Settings),
            vec![Dialog::Confirm, Dialog::Settings]
        );
        assert!(dialogs.is_empty());
    }

    // a_hand_off_replaces_one_dialog_with_the_next
    {
        let mut dialogs = DialogStack::new();
        let _ = dialogs.open::<Msg>(Dialog::Settings, None);
        let _ = dialogs.hand_off::<Msg>(&Dialog::Settings, Dialog::Edit);
        assert!(!dialogs.is_open(&Dialog::Settings));
        assert_eq!(dialogs.topmost(), Some(&Dialog::Edit));
        assert_eq!(escape(&mut dialogs), vec![Dialog::Edit]);
    }
}

/// Runs the widget operations `task` carries over `view`, then reports which
/// widget holds focus.
fn focus_after(
    task: iced::Task<Msg>,
    view: Element<'_, Msg>,
) -> Option<iced::advanced::widget::Id> {
    use iced::advanced::widget::operation::{Outcome, focusable};
    use iced_test::runtime::Action;
    use iced_test::runtime::user_interface::{Cache, UserInterface};
    let mut renderer = iced_test::futures::futures::executor::block_on(
        <iced_test::renderer::Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            None,
        ),
    )
    .expect("headless renderer");
    let mut ui = UserInterface::build(view, SIZE, Cache::default(), &mut renderer);
    let mut stream = iced_test::runtime::task::into_stream(task)?;
    let waker = iced::futures::task::noop_waker();
    let mut context = std::task::Context::from_waker(&waker);
    while let std::task::Poll::Ready(Some(action)) =
        iced::futures::StreamExt::poll_next_unpin(&mut stream, &mut context)
    {
        let (Action::Widget(mut operation) | Action::WidgetIn { mut operation, .. }) = action
        else {
            continue;
        };
        loop {
            ui.operate(&renderer, &mut operation);
            match operation.finish() {
                Outcome::Chain(next) => operation = next,
                _ => break,
            }
        }
    }
    struct Focused(Option<iced::advanced::widget::Id>);
    impl iced::advanced::widget::Operation for Focused {
        fn traverse(
            &mut self,
            operate: &mut dyn FnMut(&mut dyn iced::advanced::widget::Operation),
        ) {
            operate(self);
        }
        fn focusable(
            &mut self,
            id: Option<&iced::advanced::widget::Id>,
            _bounds: iced::Rectangle,
            state: &mut dyn focusable::Focusable,
        ) {
            if state.is_focused() {
                self.0 = id.cloned();
            }
        }
    }
    let mut focused = Focused(None);
    ui.operate(&renderer, &mut focused);
    focused.0
}

const INVOKER: &str = "open-settings";

fn page_with_invoker(dialogs: &DialogStack<Dialog>) -> Element<'_, Msg> {
    dialogs.view(
        enzui_widgets::button::enz_button("Settings", Msg::Pressed)
            .id(iced::advanced::widget::Id::new(INVOKER)),
        |key| text(format!("{key:?} dialog")).into(),
        Msg::Dialog,
    )
}

#[test]
fn dialog_stack_owns_dismissal_replacement_and_focus_2() {
    // a_dialog_button_closes_it_and_returns_focus_to_its_invoker
    {
        use enzui_core::modal::DismissCause;
        let invoker = iced::advanced::widget::Id::new(INVOKER);
        let mut dialogs = DialogStack::new();
        let _ = dialogs.open::<Msg>(Dialog::Settings, Some(invoker.clone()));
        let _ = dialogs.open::<Msg>(Dialog::Confirm, None);

        let (closed, focus) = dialogs.dismiss::<Msg>(&Dialog::Settings, DismissCause::CancelAction);
        assert_eq!(closed, vec![Dialog::Confirm, Dialog::Settings]);
        assert!(dialogs.is_empty());
        assert_eq!(
            focus_after(focus, page_with_invoker(&dialogs)),
            Some(invoker)
        );
    }

    // a_busy_dialog_refuses_escape_until_its_policy_returns
    {
        use enzui_widgets::modal_host::{alert_dismiss_policy, modal_dismiss_policy};
        let mut dialogs = DialogStack::new();
        let _ = dialogs.open::<Msg>(Dialog::Edit, None);

        assert!(dialogs.set_policy(&Dialog::Edit, alert_dismiss_policy()));
        assert_eq!(escape(&mut dialogs), vec![], "busy: Escape is refused");
        assert!(dialogs.is_open(&Dialog::Edit));

        assert!(dialogs.set_policy(&Dialog::Edit, modal_dismiss_policy()));
        assert_eq!(escape(&mut dialogs), vec![Dialog::Edit]);
    }

    // view_with_configures_each_dialogs_host
    {
        use enzui_widgets::modal_host::SurfacePlacement;
        let mut dialogs = DialogStack::new();
        let _ = dialogs.open::<Msg>(Dialog::Settings, None);
        let bottom_of = |placement: SurfacePlacement| {
            let view = dialogs.view_with(
                text("page"),
                |key| text(format!("{key:?} dialog")).into(),
                Msg::Dialog,
                move |_, host| host.placement(placement),
            );
            let node = enzui_core::collect::collect_frame(view, SIZE)
                .collected()
                .iter()
                .find(|node| node.semantics.label.as_deref() == Some("Settings dialog"))
                .map(|node| node.bounds)
                .expect("the dialog is published");
            node.y + node.height
        };
        assert!(bottom_of(SurfacePlacement::Centred) < SIZE.height - 100.0);
        assert!(bottom_of(SurfacePlacement::BottomEdge) > SIZE.height - 1.0);
    }
}

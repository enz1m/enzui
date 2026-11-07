//! A popover: a named non-modal panel beside its trigger that an outside
//! press or Escape dismisses, whose trigger stays live, and that takes focus
//! when asked.

use enzui_core::collect::{CollectedNode, collect_frame};
use enzui_core::placement::Side;
use enzui_core::semantics::Role;
use enzui_widgets::popover::{POPOVER_GAP, enz_popover, focus_operation};
use iced::advanced::widget::Id;
use iced::widget::{column, container, text, text_input};
use iced::{Element, Length, Point, Rectangle, Size};
use iced_test::Simulator;
use iced_test::simulator::click;

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Toggle,
    Dismiss,
    Typed(String),
    Background,
}

fn view(open: bool, side: Side) -> Element<'static, Msg> {
    let trigger = enzui_widgets::button::enz_button("Share", Msg::Toggle).into_element();
    let content: Option<Element<'static, Msg>> = open.then(|| {
        column![
            text("Share with"),
            text_input("Name", "").id("share-name").on_input(Msg::Typed),
        ]
        .spacing(8)
        .width(200)
        .into()
    });
    let popover = enz_popover(trigger, content, || Msg::Dismiss)
        .side(side)
        .label("Share")
        .scope(Id::new("share-popover"));
    // The page under the panel answers presses too.
    iced::widget::mouse_area(container(popover.into_element()).center(Length::Fill))
        .on_press(Msg::Background)
        .into()
}

const WINDOW: Size = Size::new(640.0, 480.0);

fn nodes(open: bool, side: Side) -> Vec<CollectedNode> {
    collect_frame(view(open, side), WINDOW).collected().to_vec()
}

fn bounds(nodes: &[CollectedNode], role: Role, label: &str) -> Rectangle {
    nodes
        .iter()
        .find(|n| n.semantics.role == role && n.semantics.label.as_deref() == Some(label))
        .unwrap_or_else(|| panic!("{role:?} {label}"))
        .bounds
}

#[test]
fn popover_publishes_routes_and_restores_exclusive_focus() {
    // an_open_popover_is_a_named_non_modal_dialog_beside_its_trigger
    {
        let closed = nodes(false, Side::Bottom);
        assert!(
            !closed.iter().any(|n| n.semantics.role == Role::Dialog),
            "closed: no panel"
        );
        for side in [Side::Bottom, Side::Top] {
            let open = nodes(true, side);
            let panel = open
                .iter()
                .find(|n| n.semantics.role == Role::Dialog)
                .expect("the panel");
            assert_eq!(panel.semantics.label.as_deref(), Some("Share"));
            assert!(!panel.semantics.modal, "a popover is not modal");
            let panel = panel.bounds;
            let trigger = bounds(&open, Role::Button, "Share");
            match side {
                Side::Bottom => assert!(
                    (panel.y - (trigger.y + trigger.height + POPOVER_GAP)).abs() < 1.0,
                    "below the trigger: {trigger:?} {panel:?}"
                ),
                _ => assert!(
                    (panel.y + panel.height + POPOVER_GAP - trigger.y).abs() < 1.0,
                    "above the trigger: {trigger:?} {panel:?}"
                ),
            }
            assert!(
                (panel.x - trigger.x).abs() < 1.0,
                "leading edge on the trigger's: {trigger:?} {panel:?}"
            );
        }
    }

    // an_outside_press_or_escape_dismisses_and_the_trigger_stays_live
    {
        let press_at = |point: Point| {
            let mut ui = Simulator::with_size(Default::default(), WINDOW, view(true, Side::Bottom));
            ui.point_at(point);
            let _ = ui.simulate(click());
            ui.into_messages().collect::<Vec<Msg>>()
        };
        let open = nodes(true, Side::Bottom);
        let trigger = bounds(&open, Role::Button, "Share");
        let panel = open
            .iter()
            .find(|n| n.semantics.role == Role::Dialog)
            .expect("the panel")
            .bounds;
        let outside = press_at(Point::new(20.0, 20.0));
        assert!(outside.contains(&Msg::Dismiss), "{outside:?}");
        assert_eq!(
            press_at(trigger.center()),
            [Msg::Toggle],
            "the trigger toggles"
        );
        assert_eq!(
            press_at(Point::new(panel.x + 4.0, panel.y + 4.0)),
            [],
            "a press on the panel stays in it, off the page beneath"
        );

        let mut ui = Simulator::with_size(Default::default(), WINDOW, view(true, Side::Bottom));
        let _ = ui.tap_key(iced::keyboard::key::Named::Escape);
        assert_eq!(ui.into_messages().collect::<Vec<Msg>>(), [Msg::Dismiss]);
        let mut ui = Simulator::with_size(Default::default(), WINDOW, view(false, Side::Bottom));
        let _ = ui.tap_key(iced::keyboard::key::Named::Escape);
        assert_eq!(
            ui.into_messages().collect::<Vec<Msg>>(),
            [],
            "a closed popover leaves Escape alone"
        );
    }

    // the_focus_operation_moves_focus_into_the_popover
    {
        let mut ui = Simulator::with_size(Default::default(), WINDOW, view(true, Side::Bottom));
        // Run the operation and every operation it chains, as the runtime does.
        let mut operation = focus_operation::<()>(Id::new("share-popover"));
        loop {
            ui.operate(operation.as_mut());
            match operation.finish() {
                iced::advanced::widget::operation::Outcome::Chain(next) => operation = next,
                _ => break,
            }
        }
        let _ = ui.typewrite("Ada");
        let typed: Vec<Msg> = ui.into_messages().collect();
        assert_eq!(
            typed.last(),
            Some(&Msg::Typed("Ada".to_owned())),
            "{typed:?}"
        );
    }

    // focusing_the_popover_takes_focus_off_the_trigger
    {
        let mut ui = Simulator::with_size(Default::default(), WINDOW, view(true, Side::Bottom));
        // The trigger has focus, as after a keyboard activation.
        run(
            &mut ui,
            Box::new(iced::advanced::widget::operation::focusable::focus_next()),
        );
        let mut focused = Focused(0);
        ui.operate(&mut focused);
        assert_eq!(focused.0, 1, "calibration: the trigger is focused");
        run(&mut ui, focus_operation(Id::new("share-popover")));
        let mut focused = Focused(0);
        ui.operate(&mut focused);
        assert_eq!(focused.0, 1, "only the field is focused");
        let _ = ui.typewrite("x");
        assert_eq!(
            ui.into_messages().last(),
            Some(Msg::Typed("x".to_owned())),
            "and it is the field"
        );
    }
}

/// Counts focused widgets.
struct Focused(usize);

impl iced::advanced::widget::Operation for Focused {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn iced::advanced::widget::Operation)) {
        operate(self);
    }

    fn focusable(
        &mut self,
        _id: Option<&Id>,
        _bounds: Rectangle,
        state: &mut dyn iced::advanced::widget::operation::Focusable,
    ) {
        self.0 += usize::from(state.is_focused());
    }
}

fn run(ui: &mut Simulator<'_, Msg>, mut operation: Box<dyn iced::advanced::widget::Operation>) {
    loop {
        ui.operate(operation.as_mut());
        match operation.finish() {
            iced::advanced::widget::operation::Outcome::Chain(next) => operation = next,
            _ => break,
        }
    }
}

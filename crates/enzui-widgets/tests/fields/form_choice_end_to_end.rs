//! A choice field driven through a rendered dialog all the way into the
//! session, plus the `Simulator` end-to-end path for the field kinds Simulator
//! can reach (every field kind, including option changes).

use enzui_core::form::{ChoiceOption, FieldId, FieldValue, FormField, FormModel, FormSession};
use enzui_widgets::form_dialog::{
    FormDialogEvent, FormPresenter, enz_form_dialog, field_widget_id,
};
use iced::{Element, Point};
use iced_test::runtime::user_interface::{Cache, UserInterface};

/// The message the dialog reports through. A newtype rather than a closure
/// returning `unreachable!()`: `enz_form_dialog` invokes `on_submit(on_event(..))`
/// EAGERLY while building the view, so a panicking callback would blow up at
/// construction time, long before any click.
#[derive(Debug, Clone, PartialEq)]
struct Msg(FormDialogEvent);

type Renderer = iced_test::renderer::Renderer;

fn new_renderer() -> Renderer {
    iced_test::futures::futures::executor::block_on(
        <Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer")
}

const VIEW_SIZE: iced::Size = iced::Size {
    width: 900.0,
    height: 1400.0,
};

const PARKED: Point = Point {
    x: 5000.0,
    y: 5000.0,
};

/// A form whose Choice field starts unset with three distinguishable
/// options, so any applied selection is a visible change rather than a no-op,
/// plus a toggle field (the kind `Simulator` can drive with no overlay).
fn form_model() -> FormModel {
    let choice = FormField::choice(
        "city",
        "City",
        vec![
            ChoiceOption::new("paris-fr", "Paris"),
            ChoiceOption::new("berlin-de", "Berlin"),
            ChoiceOption::new("lisbon-pt", "Lisbon"),
        ],
    )
    .expect("non-empty options");
    let toggle = FormField::toggle("pinned", "Pin to top", false);
    FormModel::new(vec![choice, toggle]).expect("valid model")
}

fn city() -> FieldId {
    FieldId::new("city")
}

fn pinned() -> FieldId {
    FieldId::new("pinned")
}

fn dialog(session: &FormSession) -> Element<'_, Msg> {
    enz_form_dialog("Pick a city", session, Msg).into_surface()
}

/// Drives a form dialog the way the runtime does: the `UserInterface` is
/// rebuilt from the current session on every frame, carrying widget state
/// across in a `Cache`. That rebuild is what makes an overlay (the combo
/// popup) appear on the frame after the state that opened it, which is exactly
/// what `Simulator` cannot do.
struct FormHost {
    presenter: FormPresenter,
    renderer: Renderer,
    cache: Cache,
    cursor: iced::advanced::mouse::Cursor,
}

impl FormHost {
    fn new(model: FormModel) -> Self {
        Self {
            presenter: FormPresenter::new(model),
            renderer: new_renderer(),
            cache: Cache::default(),
            cursor: iced::advanced::mouse::Cursor::Unavailable,
        }
    }

    fn point_at(&mut self, p: Point) {
        self.cursor = iced::advanced::mouse::Cursor::Available(p);
    }

    /// One frame: rebuild the view over the live session, deliver `events`,
    /// then APPLY every resulting event through the real presenter.
    fn frame(&mut self, events: &[iced::Event]) -> Vec<FormDialogEvent> {
        let mut messages: iced::advanced::shell::Bus<Msg> = iced::advanced::shell::Bus::new();
        let cache = std::mem::take(&mut self.cache);
        {
            let element = dialog(self.presenter.session());
            let mut ui = UserInterface::build(element, VIEW_SIZE, cache, &mut self.renderer);
            let _ = ui.update(
                &iced::advanced::graphics::core::window::Headless,
                &iced::advanced::graphics::core::shell::Waker::noop(),
                events,
                self.cursor,
                &mut self.renderer,
                &mut messages,
            );
            self.cache = ui.into_cache();
        }
        let events: Vec<FormDialogEvent> = messages.into_iter().map(|Msg(e)| e).collect();
        for event in events.clone() {
            let (_outcome, _task) = self.presenter.update::<Msg>(event, None);
        }
        events
    }

    /// A press+release at the current cursor position.
    fn click(&mut self) -> Vec<FormDialogEvent> {
        self.frame(&[
            iced::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)),
            iced::Event::Mouse(iced::mouse::Event::ButtonReleased(
                iced::mouse::Button::Left,
            )),
        ])
    }

    /// The screen-space centre of option row `index`, taken from the live
    /// overlay layout. Returns `None` when the popup is closed.
    fn row_center(&mut self, index: usize) -> Option<Point> {
        let cache = std::mem::take(&mut self.cache);
        let element = dialog(self.presenter.session());
        let mut ui = UserInterface::build(element, VIEW_SIZE, cache, &mut self.renderer);
        let mut collector = enzui_core::collect::SemanticsCollector::new();
        ui.operate(
            &self.renderer,
            &mut collector as &mut dyn iced::advanced::widget::Operation,
        );
        let center = collector
            .collected()
            .iter()
            .filter(|n| n.semantics.role == enzui_core::semantics::Role::ListBoxOption)
            .nth(index)
            .map(|n| n.bounds.center());
        self.cache = ui.into_cache();
        center
    }

    fn value(&self, id: &FieldId) -> Option<&FieldValue> {
        self.presenter.session().value(id)
    }
}

/// The screen-space centre of the choice field's trigger, read from the live
/// layout by the id the dialog actually renders it with.
fn trigger_center(host: &mut FormHost) -> Option<Point> {
    let cache = std::mem::take(&mut host.cache);
    let element = dialog(host.presenter.session());
    let mut ui = UserInterface::build(element, VIEW_SIZE, cache, &mut host.renderer);
    let mut collector = enzui_core::collect::SemanticsCollector::new();
    ui.operate(
        &host.renderer,
        &mut collector as &mut dyn iced::advanced::widget::Operation,
    );
    let target = field_widget_id(&city());
    let center = collector
        .collected()
        .iter()
        .find(|n| n.id.as_ref() == Some(&target))
        .map(|n| n.bounds.center());
    host.cache = ui.into_cache();
    center
}

/// A choice selection must not disturb a neighbouring field's draft. This is
/// the keyed-draft property at the rendered level rather than the model level.
#[test]
fn selecting_a_choice_leaves_the_other_fields_draft_alone() {
    let mut host = FormHost::new(form_model());
    let before = host.value(&pinned()).cloned();

    let trigger = trigger_center(&mut host).expect("trigger laid out");
    host.point_at(trigger);
    let _ = host.click();
    let row = host.row_center(2).expect("Lisbon row");
    host.point_at(row);
    let _ = host.click();
    host.point_at(PARKED);

    assert_eq!(
        host.value(&pinned()).cloned(),
        before,
        "selecting a choice must not touch the toggle field's draft (keyed \
         drafts are per-field)"
    );
    assert_eq!(
        host.value(&city()),
        Some(&FieldValue::Choice(Some("lisbon-pt".to_owned()))),
        "...while the choice itself did change"
    );
}

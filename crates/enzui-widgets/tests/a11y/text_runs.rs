//! Text fields and areas publish their text, caret and selection to AT-SPI.
//!
//! Each test lays out a real field, publishes the collected frame through the
//! vendored AT-SPI adapter the product uses, and asks it what a screen reader
//! would: the text, the caret offset, the selection and character extents.

use accesskit_atspi_common::{
    Adapter, AdapterCallback, AppContext, CoordType, Event, InterfaceSet, NodeId as AtspiNodeId,
    WindowBounds,
};
use enzui_core::a11y_action::{AccessibilityAction, resolve_action};
use enzui_core::collect::{NodeIdMap, SemanticsCollector};
use enzui_widgets::text_area::enz_text_area;
use enzui_widgets::text_field::enz_text_field;
use iced::advanced::text::Position;
use iced::advanced::widget::operation::text_input::select_range;
use iced::advanced::widget::{Id, Operation};
use iced::widget::text_editor::Content;
use iced::{Element, Size, window};
use iced_test::runtime::user_interface::{Cache, UserInterface};
use std::sync::{Arc, Mutex};

type Renderer = iced_test::renderer::Renderer;

#[derive(Debug, Clone)]
enum Msg {
    Edited,
}

/// The node that registered AT-SPI's Text interface, and the action requests
/// the adapter sent.
#[derive(Clone, Default)]
struct Probe {
    text_nodes: Arc<Mutex<Vec<AtspiNodeId>>>,
    editable_nodes: Arc<Mutex<Vec<AtspiNodeId>>>,
    all_nodes: Arc<Mutex<Vec<AtspiNodeId>>>,
    requests: Arc<Mutex<Vec<accesskit::ActionRequest>>>,
}

impl AdapterCallback for Probe {
    fn register_interfaces(&self, _: &Adapter, node: AtspiNodeId, interfaces: InterfaceSet) {
        if interfaces.contains(atspi_common::Interface::Text) {
            self.text_nodes.lock().expect("probe").push(node);
        }
        if interfaces.contains(atspi_common::Interface::EditableText) {
            self.editable_nodes.lock().expect("probe").push(node);
        }
        self.all_nodes.lock().expect("probe").push(node);
    }

    fn unregister_interfaces(&self, _: &Adapter, _: AtspiNodeId, _: InterfaceSet) {}

    fn emit_event(&self, _: &Adapter, _: Event) {}
}

impl accesskit::ActionHandler for Probe {
    fn do_action(&mut self, request: accesskit::ActionRequest) {
        self.requests.lock().expect("probe").push(request);
    }
}

/// Lays `view` out, runs `select` on it, and publishes the collected frame.
fn publish(
    view: Element<'_, Msg>,
    select: Option<(Id, Position, Position)>,
) -> (Adapter, Probe, NodeIdMap) {
    let mut renderer = iced_test::futures::futures::executor::block_on(
        <Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer");
    let mut ui = UserInterface::build(
        view,
        Size::new(480.0, 200.0),
        Cache::default(),
        &mut renderer,
    );
    if let Some((id, anchor, focus)) = select {
        let mut operation = select_range::<()>(id, anchor, focus);
        ui.operate(&renderer, &mut operation as &mut dyn Operation);
    }
    let mut collector = SemanticsCollector::new();
    ui.operate(&renderer, &mut collector as &mut dyn Operation);
    let (update, map) = collector.to_tree_update_for_window(window::Id::unique(), Some("Text"));
    let probe = Probe::default();
    let adapter = Adapter::with_id(
        accesskit_atspi_common::next_adapter_id(),
        &AppContext::new(None),
        probe.clone(),
        update,
        true,
        WindowBounds::default(),
        probe.clone(),
    );
    (adapter, probe, map)
}

fn at(index: usize) -> Position {
    Position { line: 0, index }
}

#[test]
fn text_accessibility_reports_selection_direction_and_editability_without_secrets_1() {
    // a_text_field_reports_its_text_caret_and_selection
    {
        let id = Id::new("name-field");
        let field = enz_text_field("Name", "", "Hello wörld", |_| Msg::Edited).id(id.clone());
        // Bytes 7..10 are "ör": the ö takes two bytes and one character.
        let (adapter, probe, map) = publish(field.into(), Some((id.clone(), at(7), at(10))));

        let nodes = probe.text_nodes.lock().expect("probe").clone();
        assert_eq!(nodes.len(), 1, "one text node: the field");
        let field = adapter.platform_node(nodes[0]);
        assert_eq!(field.character_count().expect("count"), 11);
        assert_eq!(field.text(0, -1).expect("text"), "Hello wörld");
        assert_eq!(field.caret_offset().expect("caret"), 9);
        assert_eq!(field.n_selections().expect("selections"), 1);
        assert_eq!(field.selection(0).expect("selection"), (7, 9));

        let first = field
            .character_extents(0, CoordType::Window)
            .expect("first character");
        let last = field
            .character_extents(10, CoordType::Window)
            .expect("last character");
        assert!(first.width > 0 && last.width > 0, "{first:?} {last:?}");
        assert!(
            last.x > first.x,
            "characters run left to right: {first:?} {last:?}"
        );

        // A screen reader moves the selection; the request resolves to the
        // field's byte positions, past the two-byte ö.
        assert!(field.set_selection(0, 7, 9).expect("set selection"));
        let request = probe
            .requests
            .lock()
            .expect("probe")
            .pop()
            .expect("request");
        assert_eq!(
            resolve_action(
                request.action,
                request.target_node,
                request.data.as_ref(),
                &map
            ),
            Some(AccessibilityAction::SetTextSelection {
                id,
                anchor: at(7),
                focus: at(10),
            })
        );
    }

    // a_password_field_publishes_no_text
    {
        let id = Id::new("secret-field");
        let field = enz_text_field("Password", "", "hunter2", |_| Msg::Edited)
            .secure(true)
            .id(id.clone());
        let (_adapter, probe, _map) = publish(field.into(), Some((id, at(0), at(3))));
        assert!(probe.text_nodes.lock().expect("probe").is_empty());
    }

    // a_text_area_reports_its_lines
    {
        let content = Content::with_text("first line\nsecond");
        let id = Id::new("notes");
        let area = enz_text_area(&content, |_| Msg::Edited)
            .label("Notes")
            .id(id.clone());
        let (adapter, probe, _map) = publish(
            area.into(),
            Some((
                id,
                Position { line: 1, index: 0 },
                Position { line: 1, index: 3 },
            )),
        );
        let nodes = probe.text_nodes.lock().expect("probe").clone();
        assert_eq!(nodes.len(), 1, "one text node: the area");
        let area = adapter.platform_node(nodes[0]);
        assert_eq!(area.text(0, -1).expect("text"), "first line\nsecond");
        assert_eq!(area.selection(0).expect("selection"), (11, 14));
        assert_eq!(area.caret_offset().expect("caret"), 14);
    }

    // right_to_left_characters_run_leftwards
    {
        let id = Id::new("arabic-field");
        let field = enz_text_field("Name", "", "اليوم", |_| Msg::Edited).id(id.clone());
        let (adapter, probe, _map) = publish(field.into(), Some((id, at(0), at(0))));
        let nodes = probe.text_nodes.lock().expect("probe").clone();
        let field = adapter.platform_node(nodes[0]);
        let first = field
            .character_extents(0, CoordType::Window)
            .expect("first character");
        let last = field
            .character_extents(4, CoordType::Window)
            .expect("last character");
        assert!(first.width > 0 && last.width > 0, "{first:?} {last:?}");
        assert!(
            first.x > last.x,
            "the first character is rightmost: {first:?} {last:?}"
        );
    }
}

#[test]
fn text_accessibility_reports_selection_direction_and_editability_without_secrets_2() {
    // an_editable_field_takes_its_text_from_assistive_technology
    {
        let editable = Id::new("editable-probe");
        let view: Element<'_, Msg> = iced::widget::column![
            enz_text_field("Editable", "", "12345", |_| Msg::Edited).id(editable.clone()),
            enz_text_field("Read only", "", "67890", |_| Msg::Edited)
                .read_only(true)
                .id(Id::new("readonly-probe")),
        ]
        .into();
        let (adapter, probe, map) = publish(view, None);

        let editable_nodes = probe.editable_nodes.lock().expect("probe").clone();
        assert_eq!(editable_nodes.len(), 1, "only the editable field");
        let text_nodes = probe.text_nodes.lock().expect("probe").clone();
        let read_only = text_nodes
            .iter()
            .copied()
            .find(|node| !editable_nodes.contains(node))
            .expect("the read-only field still publishes its text");
        assert!(
            adapter
                .platform_node(read_only)
                .set_text_contents("1")
                .is_err(),
            "a read-only field refuses"
        );

        adapter
            .platform_node(editable_nodes[0])
            .set_text_contents("888")
            .expect("set text contents");
        let request = probe
            .requests
            .lock()
            .expect("probe")
            .pop()
            .expect("request");
        assert_eq!(
            resolve_action(
                request.action,
                request.target_node,
                request.data.as_ref(),
                &map
            ),
            Some(AccessibilityAction::SetValue {
                id: editable,
                value: "888".to_owned(),
            })
        );
    }

    // a_disabled_control_reports_neither_enabled_nor_sensitive
    {
        use atspi_common::State;
        let view: Element<'_, Msg> = iced::widget::column![
            enzui_widgets::button::enz_button("Enabled probe", Msg::Edited),
            enzui_widgets::button::enz_button("Disabled probe", Msg::Edited).enabled(false),
            enz_text_field("Read-only probe", "", "text", |_| Msg::Edited).read_only(true),
        ]
        .into();
        let (adapter, probe, _map) = publish(view, None);
        let nodes = probe.all_nodes.lock().expect("probe").clone();
        let state_of = |name: &str| {
            let node = nodes
                .iter()
                .map(|node| adapter.platform_node(*node))
                .find(|node| {
                    node.name().is_ok_and(|n| n == name)
                        && node
                            .role()
                            .is_ok_and(|role| role != atspi_common::Role::Label)
                })
                .unwrap_or_else(|| panic!("{name} is published"));
            node.state()
        };
        let enabled = state_of("Enabled probe");
        assert!(enabled.contains(State::Enabled) && enabled.contains(State::Sensitive));
        let disabled = state_of("Disabled probe");
        assert!(
            !disabled.contains(State::Enabled) && !disabled.contains(State::Sensitive),
            "disabled: {disabled:?}"
        );
        let read_only = state_of("Read-only probe");
        assert!(
            read_only.contains(State::ReadOnly) && read_only.contains(State::Enabled),
            "read-only: {read_only:?}"
        );
    }
}

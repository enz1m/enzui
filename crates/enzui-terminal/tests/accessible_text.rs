//! The terminal is readable by a screen reader: its rows are its text, the
//! caret is on the cursor, and new output reaches the reader as inserted
//! text in a polite live region.

#![allow(
    missing_docs,
    unreachable_pub,
    clippy::missing_panics_doc,
    clippy::unwrap_used,
    clippy::panic,
    clippy::missing_assert_message,
    reason = "a test crate reports by panicking, and none of it is public API"
)]

use accesskit_atspi_common::{
    Adapter, AdapterCallback, AppContext, Event, InterfaceSet, NodeId as AtspiNodeId, ObjectEvent,
    WindowBounds,
};
use enzui_core::collect::SemanticsCollector;
use enzui_terminal::{Cell, Cursor, CursorShape, Rgba, Selection, TerminalScreen, enz_terminal};
use iced::advanced::widget::{Id, Operation};
use iced::{Size, window};
use iced_test::runtime::user_interface::{Cache, UserInterface};
use std::sync::{Arc, Mutex};

type Renderer = iced_test::renderer::Renderer;

const WHITE: Rgba = Rgba::rgb(255, 255, 255);

#[derive(Clone, Default)]
struct Probe {
    text_nodes: Arc<Mutex<Vec<AtspiNodeId>>>,
    inserted: Arc<Mutex<Vec<String>>>,
}

impl AdapterCallback for Probe {
    fn register_interfaces(&self, _: &Adapter, node: AtspiNodeId, interfaces: InterfaceSet) {
        if interfaces.contains(atspi_common::Interface::Text) {
            self.text_nodes.lock().unwrap().push(node);
        }
    }

    fn unregister_interfaces(&self, _: &Adapter, _: AtspiNodeId, _: InterfaceSet) {}

    fn emit_event(&self, _: &Adapter, event: Event) {
        if let Event::Object {
            event: ObjectEvent::TextInserted { content, .. },
            ..
        } = event
        {
            self.inserted.lock().unwrap().push(content);
        }
    }
}

struct NoActions;

impl accesskit::ActionHandler for NoActions {
    fn do_action(&mut self, _: accesskit::ActionRequest) {}
}

fn write(screen: &TerminalScreen, row: u16, text: &str, cursor_col: u16) {
    screen.update(|w| {
        w.set_row(row, text.chars().map(|ch| Cell::new(ch, WHITE)));
        w.set_cursor(Cursor {
            col: cursor_col,
            row,
            shape: CursorShape::Block,
            visible: true,
        });
    });
}

fn frame(screen: &TerminalScreen, renderer: &mut Renderer) -> accesskit::TreeUpdate {
    frame_above(screen, 0, renderer)
}

/// [`frame`] of a view drawing `above` rows above its bounds.
fn frame_above(
    screen: &TerminalScreen,
    above: u16,
    renderer: &mut Renderer,
) -> accesskit::TreeUpdate {
    let element: iced::Element<'_, ()> = enz_terminal(screen)
        .id(Id::new("shell"))
        .label("Terminal")
        .rows_above(above)
        .into();
    let mut ui = UserInterface::build(element, Size::new(400.0, 200.0), Cache::default(), renderer);
    let mut collector = SemanticsCollector::new();
    ui.operate(renderer, &mut collector as &mut dyn Operation);
    collector
        .to_tree_update_for_window(window::Id::unique(), Some("Terminal"))
        .0
}

#[test]
fn the_terminal_is_text_with_a_caret_and_announced_output() {
    let mut renderer = iced_test::futures::futures::executor::block_on(
        <Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .unwrap();
    let screen = TerminalScreen::new(20, 3, WHITE);
    write(&screen, 0, "$ ls", 4);

    let probe = Probe::default();
    let mut adapter = Adapter::with_id(
        accesskit_atspi_common::next_adapter_id(),
        &AppContext::new(None),
        probe.clone(),
        frame(&screen, &mut renderer),
        true,
        WindowBounds::default(),
        NoActions,
    );
    let nodes = probe.text_nodes.lock().unwrap().clone();
    assert_eq!(nodes.len(), 1, "one text node: the terminal");
    let terminal = adapter.platform_node(nodes[0]);
    assert_eq!(terminal.role().unwrap(), atspi_common::Role::Terminal);
    assert_eq!(terminal.text(0, -1).unwrap(), "$ ls\n\n");
    assert_eq!(
        terminal.caret_offset().unwrap(),
        4,
        "the caret is on the cursor"
    );

    // Output arrives on the next row; the reader is told what was inserted.
    write(&screen, 1, "Cargo.toml  src", 0);
    adapter.update(frame(&screen, &mut renderer));
    assert_eq!(terminal.text(0, -1).unwrap(), "$ ls\nCargo.toml  src\n");
    assert_eq!(
        terminal.caret_offset().unwrap(),
        5,
        "the caret moved to the new row"
    );
    let inserted = probe.inserted.lock().unwrap().clone();
    assert!(
        inserted.iter().any(|text| text.contains("Cargo.toml  src")),
        "{inserted:?}"
    );

    // A drag selection of "rgo." (cells 2..=5 of row 1) is the selection the
    // reader is told, not a caret on the cursor.
    screen.update(|w| {
        w.set_selection(Some(Selection {
            start: (2, 1),
            end: (5, 1),
            block: false,
        }));
    });
    adapter.update(frame(&screen, &mut renderer));
    assert_eq!(terminal.n_selections().unwrap(), 1);
    let (start, end) = terminal.selection(0).unwrap();
    assert_eq!(terminal.text(start, end).unwrap(), "rgo.");
}

/// The rows drawn above the bounds are not read: the text starts with the
/// first row in the bounds, and the caret counts from it.
///
/// Sabotage: publish every screen row in `operate` (`above = 0`). The text
/// starts with the older row.
#[test]
fn rows_above_the_bounds_are_not_read() {
    let mut renderer = iced_test::futures::futures::executor::block_on(
        <Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .unwrap();
    let screen = TerminalScreen::new(20, 3, WHITE);
    write(&screen, 0, "older", 0);
    write(&screen, 1, "$ ls", 4);
    let probe = Probe::default();
    let adapter = Adapter::with_id(
        accesskit_atspi_common::next_adapter_id(),
        &AppContext::new(None),
        probe.clone(),
        frame_above(&screen, 1, &mut renderer),
        true,
        WindowBounds::default(),
        NoActions,
    );
    let nodes = probe.text_nodes.lock().unwrap().clone();
    let terminal = adapter.platform_node(nodes[0]);
    assert_eq!(terminal.text(0, -1).unwrap(), "$ ls\n");
    assert_eq!(terminal.caret_offset().unwrap(), 4);
}

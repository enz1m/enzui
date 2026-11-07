//! Motion contract: virtual lists and grids share the keyboard scrolling contract of
//! `enz_scroll_view`.
//!
//! The exact key set is PageUp, PageDown, Home and End. Ctrl / Alt / Logo mark
//! application-owned chords, so Ctrl+Home/Ctrl+End are left
//! unclaimed — no scroll, `Ignored` status — for the application's own bindings;
//! Shift stays widget-owned. Space, Shift+Space and arrow keys are deliberately
//! not scroll keys. Paging changes only the viewport; it does not move keyboard
//! focus or selection.

use enzui_core::virtual_grid::TileProvider;
use enzui_core::virtual_sequence::{ItemProvider, KeyedProvider, SequenceKey};
use enzui_widgets::scroll_view::enz_scroll_view;
use enzui_widgets::virtual_list::enz_virtual_list;
use iced::widget::{Space, text};
use iced::{Element, Length, Point, keyboard};
use iced_test::Simulator;
use std::cell::RefCell;

const ROW_H: f32 = 20.0;
const ROWS: u64 = 213;

#[derive(Debug, Clone)]
// Callback payloads are constructed by widget callbacks, never read back in
// these assertions (matches the local convention for harness message enums).
#[allow(
    dead_code,
    reason = "the widget needs these messages to exist; this test never sends them"
)]
enum Msg {
    Scrolled(f32),
    Edited(String),
}

#[derive(Default)]
struct Rows {
    asked: RefCell<Vec<u64>>,
}

impl Rows {}

impl ItemProvider<String> for Rows {
    fn len(&self) -> u64 {
        ROWS
    }

    fn item(&self, index: u64) -> Option<String> {
        if index >= ROWS {
            return None;
        }
        self.asked.borrow_mut().push(index);
        Some(format!("row {index}"))
    }
}

impl KeyedProvider<String> for Rows {
    fn key(&self, index: u64) -> Option<SequenceKey> {
        (index < ROWS).then(|| SequenceKey::new(format!("row-{index}")))
    }
}

impl TileProvider<String> for Rows {}

fn list(rows: &'static Rows, height: f32, offset: f32) -> Element<'static, Msg> {
    enz_virtual_list(rows, ROW_H, |_index, item| text(item.clone()).into())
        .overscan(0)
        .viewport_height(height)
        .scroll_offset(offset)
        .height(Length::Fixed(height))
        .on_scroll(|viewport| Msg::Scrolled(viewport.absolute_offset().y))
        .into()
}

fn canonical(height: f32) -> Element<'static, Msg> {
    enz_scroll_view(
        Space::new()
            .width(Length::Fixed(80.0))
            .height(Length::Fixed(ROWS as f32 * ROW_H)),
    )
    .width(Length::Fixed(80.0))
    .height(Length::Fixed(height))
    .on_scroll(|viewport| Msg::Scrolled(viewport.absolute_offset().y))
    .into()
}

fn key_events(named: keyboard::key::Named) -> Vec<iced::Event> {
    iced_test::simulator::tap_key(keyboard::Key::Named(named), None).collect()
}

fn press(ui: &mut Simulator<'static, Msg>, height: f32, named: keyboard::key::Named) {
    ui.point_at(Point::new(30.0, height / 2.0));
    let _ = ui.simulate(key_events(named));
}

fn offsets(ui: Simulator<'static, Msg>) -> Vec<f32> {
    ui.into_messages()
        .filter_map(|message| match message {
            Msg::Scrolled(y) => Some(y),
            Msg::Edited(_) => None,
        })
        .collect()
}

fn offset_after(view: Element<'static, Msg>, height: f32, keys: &[keyboard::key::Named]) -> f32 {
    let mut ui = Simulator::new(view);
    for key in keys {
        press(&mut ui, height, *key);
    }
    *offsets(ui)
        .last()
        .expect("a moving key must report an offset")
}

#[test]
fn virtual_list_and_scroll_view_have_key_for_key_movement_parity() {
    let rows: &'static Rows = Box::leak(Box::new(Rows::default()));
    let height = 200.0;
    for key in [keyboard::key::Named::PageDown, keyboard::key::Named::End] {
        assert_eq!(
            offset_after(list(rows, height, 0.0), height, &[key]),
            offset_after(canonical(height), height, &[key]),
            "{key:?} must produce the same movement in virtual and canonical scrolling"
        );
    }
    for key in [keyboard::key::Named::PageUp, keyboard::key::Named::Home] {
        let sequence = [keyboard::key::Named::End, key];
        assert_eq!(
            offset_after(list(rows, height, 0.0), height, &sequence),
            offset_after(canonical(height), height, &sequence),
            "{key:?} must produce the same reverse/start movement"
        );
    }

    // Application-chord parity: Ctrl+Home is an application-owned chord,
    // so both the virtual list and the canonical scroll view
    // must leave it unclaimed — no scroll back to the top, `Ignored` status —
    // even after End has scrolled the viewport to the bottom. A boundary
    // control (bare Home right after, below) proves the offset *can* still
    // move, so "unchanged" is genuine ownership, not an inability to scroll.
    let ctrl_home = iced::Event::Keyboard(keyboard::Event::KeyPressed {
        key: keyboard::Key::Named(keyboard::key::Named::Home),
        modified_key: keyboard::Key::Named(keyboard::key::Named::Home),
        physical_key: keyboard::key::Physical::Unidentified(
            keyboard::key::NativeCode::Unidentified,
        ),
        location: keyboard::Location::Standard,
        modifiers: keyboard::Modifiers::CTRL,
        text: None,
        repeat: false,
    });
    let bare_home = iced::Event::Keyboard(keyboard::Event::KeyPressed {
        key: keyboard::Key::Named(keyboard::key::Named::Home),
        modified_key: keyboard::Key::Named(keyboard::key::Named::Home),
        physical_key: keyboard::key::Physical::Unidentified(
            keyboard::key::NativeCode::Unidentified,
        ),
        location: keyboard::Location::Standard,
        modifiers: keyboard::Modifiers::default(),
        text: None,
        repeat: false,
    });
    for view in [list(rows, height, 0.0), canonical(height)] {
        let mut ui = Simulator::new(view);
        press(&mut ui, height, keyboard::key::Named::End);
        let ctrl_status = ui.simulate([ctrl_home.clone()]);
        assert!(
            ctrl_status
                .iter()
                .all(|s| *s == iced::event::Status::Ignored),
            "Ctrl+Home is an application chord and must stay Ignored: {ctrl_status:?}"
        );
        // Boundary control: bare Home from the same bottom position *does*
        // capture and scroll, so "Ctrl+Home did not move" is genuine ownership
        // rather than an inability to scroll.
        let bare_status = ui.simulate([bare_home.clone()]);
        assert!(
            bare_status.contains(&iced::event::Status::Captured),
            "bare Home must still capture and scroll: {bare_status:?}"
        );
        // End published one bottom offset; Ctrl+Home published nothing; bare
        // Home published exactly the return to 0. So a chord that scrolled
        // would show up as a third offset even if its status were wrong.
        let scrolled = offsets(ui);
        assert_eq!(
            scrolled.len(),
            2,
            "only End and bare Home may scroll; Ctrl+Home must add no offset: {scrolled:?}"
        );
        assert!(
            scrolled[0] > 0.0,
            "End must scroll to a positive bottom: {}",
            scrolled[0]
        );
        assert_eq!(scrolled[1], 0.0, "bare Home must return to exactly 0");
    }

    for key in [
        keyboard::key::Named::Space,
        keyboard::key::Named::ArrowDown,
        keyboard::key::Named::ArrowUp,
    ] {
        let mut virtual_ui = Simulator::new(list(rows, height, 0.0));
        virtual_ui.point_at(Point::new(30.0, 50.0));
        let virtual_status = virtual_ui.simulate(key_events(key));
        let mut canonical_ui = Simulator::new(canonical(height));
        canonical_ui.point_at(Point::new(30.0, 50.0));
        let canonical_status = canonical_ui.simulate(key_events(key));
        assert_eq!(virtual_status, canonical_status, "{key:?} parity");
        assert!(
            virtual_status
                .iter()
                .all(|s| *s == iced::event::Status::Ignored)
        );
    }
}

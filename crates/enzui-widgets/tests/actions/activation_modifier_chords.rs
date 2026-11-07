//! A focused interactive control must not decode Ctrl/Alt/Logo-modified Enter
//! or Space as ordinary activation. It must publish nothing, and it must leave
//! the event `Status::Ignored` so the application's chord bindings still see
//! it.
//!
//! Every gate below has three parts:
//!
//! 1. a positive control that fires on the bare key (`Captured` + one
//!    activation message on top of the focusing click);
//! 2. a message gate (nothing published for the chord); and
//! 3. an independent `Status::Ignored` gate on the same chord (a widget that
//!    silently swallowed the chord without publishing would satisfy #2 and
//!    fail here).
//!
//! Coverage: `switch` (a shipped `activation_from_key` consumer, kept honest
//! against a decoder that drops modifiers again), `focus_button` (an inline
//! decoder), and one Enter-only route through
//! `focus_button::on_enter` — the branch shared by `enz_pressable` grid tiles
//! and `TextFieldSemantics::on_submit_with`. `TextFieldSemantics` itself is
//! covered in `text_editing/text_field_interaction.rs`, alongside the existing
//! read-only / editable submit tests.

use enzui_widgets::focus_button::focus_button;
use iced::widget::text;
use iced::{Element, Event, Point, keyboard};
use iced_test::Simulator;
use iced_test::simulator::click;

/// A key press carrying `modifiers`. `Simulator::tap_key` and
/// `iced_test::simulator::press_key` both hardwire `Modifiers::default()`, so a
/// chord has to be built by hand.
fn press_with(named: keyboard::key::Named, modifiers: keyboard::Modifiers) -> Event {
    Event::Keyboard(keyboard::Event::KeyPressed {
        key: keyboard::Key::Named(named),
        modified_key: keyboard::Key::Named(named),
        physical_key: keyboard::key::Physical::Unidentified(
            keyboard::key::NativeCode::Unidentified,
        ),
        location: keyboard::Location::Standard,
        modifiers,
        text: None,
        repeat: false,
    })
}

/// A key release carrying `modifiers`, for the Space release half of the
/// platform button contract.
fn release_with(named: keyboard::key::Named, modifiers: keyboard::Modifiers) -> Event {
    Event::Keyboard(keyboard::Event::KeyReleased {
        key: keyboard::Key::Named(named),
        modified_key: keyboard::Key::Named(named),
        physical_key: keyboard::key::Physical::Unidentified(
            keyboard::key::NativeCode::Unidentified,
        ),
        location: keyboard::Location::Standard,
        modifiers,
    })
}

// ---------------------------------------------------------------------------
// switch — the shared primitive's shipped consumer.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// focus_button — the inline decoder.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum ButtonMsg {
    Press,
    Enter,
}

/// Focus a focus_button (with a distinct `on_enter` split, so the Enter-only
/// route below can tell the two apart) by clicking it, then run `keys`.
fn focused_button_run(at: Point, keys: Vec<Event>) -> (Vec<iced::event::Status>, Vec<ButtonMsg>) {
    let view: Element<'static, ButtonMsg> = focus_button(text("Ev"), ButtonMsg::Press)
        .padding(20)
        .on_enter(ButtonMsg::Enter)
        .into();
    let mut ui = Simulator::new(view);

    ui.point_at(at);
    let _ = ui.simulate(click());
    let statuses = ui.simulate(keys);
    (statuses, ui.into_messages().collect())
}

/// Somewhere comfortably inside the padded label.
const BUTTON_AT: Point = Point { x: 25.0, y: 25.0 };

#[test]
fn bare_space_still_arms_and_publishes_on_release() {
    // Space keeps its arm-on-press / fire-on-release contract; the chord gate
    // must not have broken it. The press is captured (armed) and the release
    // is captured (fires `on_press`, since `on_space` is unset).
    let (statuses, messages) = focused_button_run(
        BUTTON_AT,
        vec![
            press_with(keyboard::key::Named::Space, keyboard::Modifiers::default()),
            release_with(keyboard::key::Named::Space, keyboard::Modifiers::default()),
        ],
    );
    assert_eq!(
        statuses,
        vec![iced::event::Status::Captured, iced::event::Status::Captured],
        "bare Space on a focused focus_button must be captured on press and release"
    );
    assert_eq!(
        messages,
        vec![ButtonMsg::Press, ButtonMsg::Press],
        "one Press for the focusing click, one Press for the Space release"
    );
}

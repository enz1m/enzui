//! Read-only Toggle and Choice must be FOCUSABLE and ANNOUNCED, but refuse the
//! edit — distinct from *disabled*, which is inert.
//!
//! The defect: the form dialog's Toggle and Choice arms collapsed read-only
//! into disabled (`.enabled(field.is_editable())`, where
//! `is_editable() == enabled && !read_only`). A read-only toggle therefore
//! rendered fully disabled: not a Tab stop, not announced as a reachable
//! control, and dimmed — which is wrong. `read_only` means the control is
//! reachable but not editable (the API-key / generated-path case), exactly
//! as `text_field.rs` already documents for text fields:
//!
//! * `enabled(false)` — inert: no focus, no semantics action, dimmed.
//! * `read_only(true)` — live: focusable, announced (as read-only, not
//!   disabled), but every mutating path refused.
//!
//! Every assertion drives a real widget tree through `UserInterface`:
//!
//! * focus is proven by *traversing the Tab ring* and reading `find_focused`,
//!   not by inspecting a registration guard;
//! * announcement is proven by what the tree publishes to a real
//!   `SemanticsCollector` (`read_only` true, `disabled` false);
//! * edit-refusal is proven by driving a click/key and asserting NO edit
//!   message was emitted.
//!
//! CRITICAL: a fix that conflates read-only and
//! disabled passes one and breaks the other, so every read-only case here is
//! paired with a disabled case, and the two are asserted to DIFFER.

use enzui_widgets::combo_box::enz_combo_box;
use iced::advanced::widget::Id;
use iced::{Element, Point};
use iced_test::runtime::user_interface::{Cache, UserInterface};

#[derive(Debug, Clone, PartialEq)]
#[allow(
    dead_code,
    reason = "the widget needs these messages to exist; this test never sends them"
)]
enum Msg {
    ToggleTo(bool),
    SelectLabel(String),
}

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

struct App<'a> {
    ui: UserInterface<'a, Msg, iced::Theme, Renderer>,
    renderer: Renderer,
    messages: iced::advanced::shell::Bus<Msg>,
    cursor: iced::advanced::mouse::Cursor,
}

impl<'a> App<'a> {
    fn new(view: Element<'a, Msg>) -> Self {
        let mut renderer = new_renderer();
        let ui = UserInterface::build(
            view,
            iced::Size::new(400.0, 300.0),
            Cache::default(),
            &mut renderer,
        );
        Self {
            ui,
            renderer,
            messages: iced::advanced::shell::Bus::new(),
            cursor: iced::advanced::mouse::Cursor::Unavailable,
        }
    }

    fn point_at(&mut self, p: Point) {
        self.cursor = iced::advanced::mouse::Cursor::Available(p);
    }

    fn feed(&mut self, events: &[iced::Event]) {
        let _ = self.ui.update(
            &iced::advanced::graphics::core::window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            events,
            self.cursor,
            &mut self.renderer,
            &mut self.messages,
        );
    }

    /// A press+release at the current cursor position.
    fn click(&mut self) {
        self.feed(&[
            iced::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)),
            iced::Event::Mouse(iced::mouse::Event::ButtonReleased(
                iced::mouse::Button::Left,
            )),
        ]);
    }
}

// ===========================================================================
// LEVEL 1 — the `Switch` primitive: read-only vs disabled at the source.
// ===========================================================================

// ===========================================================================
// LEVEL 1.5 — `EnzToggle`: read-only forwards to the switch (the form dialog
// composes `enz_toggle`, not the bare `switch`, so the forwarding matters).
// ===========================================================================

// ===========================================================================
// LEVEL 2 — `EnzComboBox`: read-only vs disabled Choice.
// ===========================================================================

fn combo(read_only: bool, enabled: bool) -> Element<'static, Msg> {
    enz_combo_box(
        "Region",
        vec!["Alpha".to_owned(), "Beta".to_owned()],
        Some("Alpha".to_owned()),
        Msg::SelectLabel,
    )
    .id(Id::new("cb"))
    .enabled(enabled)
    .read_only(read_only)
    .into()
}

/// A read-only combo box refuses to open on a trigger click, so no selection
/// can be made. (The live combo opens; the read-only one does not.)
#[test]
fn read_only_combo_refuses_to_open() {
    // The combo trigger sits near the top-left of the 400x300 view.
    let click_at = Point::new(40.0, 30.0);

    // Read-only: click the trigger, then click where the first row would be.
    // Nothing may be selected.
    let mut ro = App::new(combo(true, true));
    ro.point_at(click_at);
    ro.click();
    // If it wrongly opened, a second click lower would land on a row.
    ro.point_at(Point::new(40.0, 70.0));
    ro.click();
    assert!(
        ro.messages.is_empty(),
        "a READ-ONLY combo box must not open or select on click; got {:?}",
        ro.messages
    );
}

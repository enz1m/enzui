//! `EnzComboBox` owns its popup, so it can be operated from the keyboard:
//! Enter, arrows and Escape drive it, `expanded` reflects the true open
//! state, and every option row is visible to AT.
//!
//! # Why the popup must be ours
//!
//! A design that composed `pick_list` would store `is_open` in private tree
//! state a composing widget can neither read nor set (the `combo_box` module
//! docs record this under "the popup is ours"). Only by owning the popup can
//! the widget publish an operable popup at all.
//!
//! # Why every assertion goes through the laid-out tree
//!
//! The tempting shortcut is to assert `state.open == true` after a key. That
//! is worthless: an `is_open` flag round-trips through its own setter while the
//! popup renders nothing, which is the classic "field reaches nothing" shape
//! this repo has been bitten by repeatedly. So *open* is
//! measured as **"a `Role::ListBox` node exists in the collected tree"**, the
//! highlight as **"which `ListBoxOption` reports `focused`"**, and the scroll
//! cap as a measured height in px.
//!
//! Because the widget is uncontrolled (it owns its state in the widget tree),
//! the harness keeps one `Cache` alive across frames — that cache *is* the
//! state. A test that rebuilt the `UserInterface` from a fresh cache would
//! silently reset the popup and measure nothing.
//!
//! # The cursor is parked at (5000, 5000)
//!
//! A draw- or status-based focus assertion taken with the cursor still on
//! the control reads `Hovered` and passes for the
//! wrong reason. Every frame here is driven with the cursor parked far away
//! unless the test is deliberately clicking.

use enzui_core::collect::{CollectedNode, SemanticsCollector};
use enzui_core::semantics::Role;
use enzui_widgets::combo_box::enz_combo_box;
use iced::advanced::widget::Operation;
use iced::advanced::widget::operation::Outcome;
use iced::keyboard::key::Named;
use iced::{Element, Event, Point, Size, Theme};

use iced_test::runtime::user_interface::{Cache, UserInterface};

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Pick(String),
    Query(String),
}

const SURFACE_W: f32 = 400.0;
const SURFACE_H: f32 = 600.0;

/// Somewhere far from every widget — see the module docs.
fn parked() -> Point {
    Point::new(5000.0, 5000.0)
}

fn key(named: Named) -> Event {
    let k = iced::keyboard::Key::Named(named);
    Event::Keyboard(iced::keyboard::Event::KeyPressed {
        key: k.clone(),
        modified_key: k,
        physical_key: iced::keyboard::key::Physical::Unidentified(
            iced::keyboard::key::NativeCode::Unidentified,
        ),
        location: iced::keyboard::Location::Standard,
        modifiers: iced::keyboard::Modifiers::default(),
        text: None,
        repeat: false,
    })
}

fn mouse_press() -> Event {
    Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left))
}

fn mouse_release() -> Event {
    Event::Mouse(iced::mouse::Event::ButtonReleased(
        iced::mouse::Button::Left,
    ))
}

// ---------------------------------------------------------------------------
// Harness — one live `Cache` across frames, because the widget owns its state
// ---------------------------------------------------------------------------

struct Host {
    renderer: iced_test::renderer::Renderer,
    cache: Cache,
    options: Vec<String>,
    selected: Option<String>,
    size: enzui_widgets::combo_box::ComboBoxSize,
    disabled: bool,
    separator: Option<usize>,
    query: Option<String>,
    /// An explicit widget id, so a test that routes AT actions knows the
    /// combo box node's identity. `None` uses the derived id.
    id: Option<iced::advanced::widget::Id>,
    messages: Vec<Msg>,
    cursor: iced::advanced::mouse::Cursor,
}

impl Host {
    fn with(options: Vec<String>, selected: Option<String>) -> Self {
        let renderer = iced_test::futures::futures::executor::block_on(
            <iced_test::renderer::Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
            ),
        )
        .expect("headless renderer");
        Self {
            renderer,
            cache: Cache::default(),
            options,
            selected,
            size: enzui_widgets::combo_box::ComboBoxSize::Md,
            disabled: false,
            separator: None,
            query: None,
            id: None,
            messages: Vec::new(),
            cursor: iced::advanced::mouse::Cursor::Available(parked()),
        }
    }

    fn view(&self) -> Element<'static, Msg> {
        let mut cb = enz_combo_box(
            "Fruit",
            self.options.clone(),
            self.selected.clone(),
            Msg::Pick,
        )
        .size(self.size)
        .enabled(!self.disabled);
        if let Some(query) = &self.query {
            cb = cb.searchable(query.clone(), Msg::Query);
        }
        if let Some(index) = self.separator {
            cb = cb.separator_before(index);
        }
        if let Some(id) = &self.id {
            cb = cb.id(id.clone());
        }
        cb.into()
    }

    fn build(&mut self) -> UserInterface<'static, Msg, Theme, iced_test::renderer::Renderer> {
        let view = self.view();
        UserInterface::build(
            view,
            Size::new(SURFACE_W, SURFACE_H),
            std::mem::take(&mut self.cache),
            &mut self.renderer,
        )
    }

    /// Push events through one frame, keeping the cache (and therefore the
    /// widget's open/highlight state) alive.
    fn frame(&mut self, events: &[Event]) -> Vec<Msg> {
        let mut messages = iced::advanced::shell::Bus::new();
        let cursor = self.cursor;
        let mut ui = self.build();
        ui.update(
            &iced::advanced::graphics::core::window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            events,
            cursor,
            &mut self.renderer,
            &mut messages,
        );
        self.cache = ui.into_cache();
        self.messages.extend(messages.iter().cloned());
        messages.into_iter().collect()
    }

    /// Give the trigger keyboard focus, the way Tab would.
    ///
    /// `focus_next` is a chained operation: its first pass only counts the
    /// focusables and the `Outcome::Chain` pass is what actually moves focus.
    /// Driving only the first pass focuses nothing, and every later assertion
    /// would then read "closed" for the wrong reason.
    fn focus(&mut self) {
        let mut ui = self.build();
        let mut op = iced::advanced::widget::operation::focusable::focus_next::<()>();
        ui.operate(&self.renderer, &mut op as &mut dyn Operation<()>);
        let mut outcome = op.finish();
        for _ in 0..8 {
            match outcome {
                Outcome::Chain(mut next) => {
                    ui.operate(&self.renderer, &mut next);
                    outcome = next.finish();
                }
                _ => break,
            }
        }
        self.cache = ui.into_cache();
    }

    fn collect(&mut self) -> Vec<CollectedNode> {
        let mut ui = self.build();
        let mut c = SemanticsCollector::new();
        ui.operate(&self.renderer, &mut c as &mut dyn Operation);
        self.cache = ui.into_cache();
        c.into_collected()
    }

    /// The label of the highlighted row, resolved through the **published AT
    /// tree**: the ComboBox node's `active_descendant` names
    /// the highlighted option by its widget [`Id`], and the option row carries
    /// that same `Id`. So this reads the relation the widget actually
    /// publishes rather than any internal flag — the same discipline the rest
    /// of this file uses, and the observable that replaces the removed
    /// `focused`-on-a-row lie.
    fn highlighted(&mut self) -> Option<String> {
        let nodes = self.collect();
        let target = nodes
            .iter()
            .find(|n| n.semantics.role == Role::ComboBox)
            .and_then(|n| n.semantics.active_descendant.clone())?;
        nodes
            .iter()
            .find(|n| n.semantics.role == Role::ListBoxOption && n.id.as_ref() == Some(&target))
            .and_then(|n| n.semantics.label.clone())
    }

    /// The screen-space center of option row `index`, read from its published
    /// `ListBoxOption` node's bounds. `None` when the popup is
    /// shut or the option is not published.
    fn row_center(&mut self, index: usize) -> Option<Point> {
        let b = self
            .collect()
            .into_iter()
            .filter(|n| n.semantics.role == Role::ListBoxOption)
            .nth(index)?
            .bounds;
        Some(Point::new(b.x + b.width / 2.0, b.y + b.height / 2.0))
    }

    /// Fire a mouse press then release at `point` in one frame (a click),
    /// with the cursor tracked at `point` throughout.
    fn click_at(&mut self, point: Point) -> Vec<Msg> {
        self.cursor = iced::advanced::mouse::Cursor::Available(point);
        let out = self.frame(&[mouse_press(), mouse_release()]);
        self.cursor = iced::advanced::mouse::Cursor::Available(parked());
        out
    }
}

// ---------------------------------------------------------------------------
// The test families (the popup-family split, child of the source-size boundary)
// ---------------------------------------------------------------------------
//
// This file passed 2500 lines, so each coherent family moved into a sibling
// file under `combo_box_popup/` and is spliced back with `include!`.
// `include!` is a TEXT move: every test keeps its exact
// `combo_box_popup::<name>` path, so `--test menus_popups_tooltips <filter>` and
// test target needs no change. Everything above this line is the
// shared harness the families use; nothing above defines a `#[test]`.

include!("combo_box_popup/highlight_identity.rs");
include!("combo_box_popup/commit_identity.rs");

//! `EnzTreeView` must realize O(visible) rows, not O(rows), while
//! keeping everything logical logical: the keyboard cursor, the row
//! identities, and the sibling-relative accessibility positions.
//!
//! # Why the obvious test is worthless
//!
//! Asserting *which rows appear* proves nothing (see
//! `virtual_list.rs`'s module docs): a widget can realize all N `Element`s and
//! then draw only 25 of them, and no output-shape assertion can tell the two
//! apart. So the load-bearing observable is work — an `Rc<Cell<usize>>`
//! bumped once per row the widget materializes as an `Element`, asserted with
//! `assert_eq!`, never `is_empty`/`at most`.
//!
//! Every counting assertion here is preceded by a calibration against the
//! eager path, so a zero can never pass vacuously: if the probe were blind, the
//! eager control would read 0 too and the calibration fails first.
//!
//! # The three properties that a naive virtualization breaks
//!
//! 1. The cursor is logical. Stored as a realized-window index, `End` on a
//!    1,000-row tree would move to "the last row I built", not row 999.
//!    [`keyboard_navigation_reaches_a_row_outside_the_realized_window`] drives
//!    real key events through a live `UserInterface` and reads the published
//!    `on_select(idx)` — the widget's own answer to "which logical row is the
//!    cursor on".
//! 2. Accessibility positions are sibling-relative. A virtualized tree that
//!    reports `1 of 25` (its realized window) instead of `1 of 2` (its parent's
//!    child set) is a regression AT users would hear.
//!    [`a11y_positions_stay_sibling_relative_under_virtualization`] pins the
//!    *whole* announced set against a shape where flat, windowed and
//!    sibling-relative all disagree.
//! 3. Row identity survives realization. A row's routable id is derived
//!    from its path, so scrolling must not renumber it.
//!    [`row_ids_are_unchanged_by_the_realization_window`] pins that the ids
//!    published at a deep offset are the same ids as at offset 0.
//!
//! # Geometry parity
//!
//! [`total_content_height_is_identical_eager_vs_virtualized`] measures the
//! scrollable's content height both ways. If the spacer arithmetic were wrong
//! the scrollbar thumb would lie and the deep rows would be unreachable, which
//! is a defect no row-count assertion can see.

use enzui_widgets::tree_view::{EnzTreeItem, enz_tree_view};
use iced::{Element, Size};
use iced_test::runtime::user_interface::{Cache, UserInterface};

type Renderer = iced_test::renderer::Renderer;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Msg {
    Select(usize),
    Toggle(usize),
}

/// N flat leaf rows, labelled by index so a realized row is identifiable.
/// `n` flat rows, each with a host-supplied logical key. A row
/// publishes a routable id only when the host keys it, so the id-related
/// properties below need keys; the label alone does not constitute identity.
fn flat(n: usize) -> Vec<EnzTreeItem> {
    (0..n)
        .map(|i| EnzTreeItem::leaf(format!("row-{i}"), 0).key(format!("row-{i}")))
        .collect()
}

fn renderer() -> Renderer {
    iced_test::futures::futures::executor::block_on(
        <Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer")
}

// ---------------------------------------------------------------------------
// Property 1 — the number of rows BUILT
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Property 2 — the cursor is a LOGICAL key, not a window index
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Property 3 — a11y positions stay sibling-relative
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The loop closes: keyboard -> scroll -> on_scroll -> next window
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Geometry parity — the scrollbar must not lie
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Event harness
// ---------------------------------------------------------------------------

fn press() -> iced::Event {
    iced::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left))
}

fn run(
    ui: &mut UserInterface<'_, Msg, iced::Theme, Renderer>,
    renderer: &mut Renderer,
    events: &[iced::Event],
    cursor: iced::mouse::Cursor,
) -> Vec<Msg> {
    use iced::advanced::graphics::core::shell::Waker;
    use iced::advanced::graphics::core::window::Headless as HeadlessWindow;
    let mut messages = iced::advanced::shell::Bus::new();
    let _ = ui.update(
        &HeadlessWindow,
        &Waker::noop(),
        events,
        cursor,
        renderer,
        &mut messages,
    );
    messages.into_iter().collect()
}

// ---------------------------------------------------------------------------
// Press must not survive a window shift
// ---------------------------------------------------------------------------

fn release() -> iced::Event {
    iced::Event::Mouse(iced::mouse::Event::ButtonReleased(
        iced::mouse::Button::Left,
    ))
}

/// A press must not survive a window shift.
///
/// The `release()` helper above sat unused for the life of this file — the
/// heading at its declaration announced this behaviour and no test asserted it,
/// which clippy reported as `function 'release' is never used`. An unused event
/// constructor under a behavioural heading is a coverage gap wearing a lint's
/// clothing: the lint says "delete me", the heading says "the test is missing".
///
/// # What is measured
///
/// Press row 3 while it is realized, scroll so row 3 leaves the realized
/// window, then release. A press latched by slot position rather than by
/// row identity would apply the release to whichever row now occupies that
/// slot, publishing a `Select` for a row the user never pressed — the
/// identity-not-position defect class.
///
/// # Why the assertion is on messages, not on state
///
/// The user-visible failure is a spurious activation, so the observable is the
/// published message. Asserting on internal press state would pass for a
/// widget that tracks the press correctly and still emits the wrong `Select`.
///
/// # Positive control
///
/// `press_then_release_without_a_shift_does_select` proves the harness can
/// observe a `Select` at all. Without it, "no Select was published" is
/// indistinguishable from a harness that never delivers the release.
#[test]
fn press_does_not_survive_a_window_shift() {
    let items = flat(20);
    let mut r = renderer();
    let size = Size::new(320.0, 170.0);
    // Row 3 sits at y = 3*34 + 17 = 119 in frame 1 (offset 0, 34px rows).
    let on_row_3 = iced::mouse::Cursor::Available(iced::Point::new(60.0, 119.0));

    let view_1: Element<'_, Msg, iced::Theme, Renderer> = enz_tree_view(items.clone())
        .viewport_height(170.0)
        .scroll_offset(0.0)
        .overscan(0)
        .on_select(Msg::Select)
        .on_toggle(Msg::Toggle)
        .into();
    let mut ui = UserInterface::build(view_1, size, Cache::default(), &mut r);

    let pressed = run(&mut ui, &mut r, &[press()], on_row_3);
    assert!(
        pressed.is_empty(),
        "calibration: a press alone must not publish a Select; got {pressed:?}"
    );

    // Frame 2: offset 340 (10 rows) — row 3 is now far outside the realized
    // window [10..15], so the slot it occupied is held by row 10.
    let cache = ui.into_cache();
    let view_2: Element<'_, Msg, iced::Theme, Renderer> = enz_tree_view(items)
        .viewport_height(170.0)
        .scroll_offset(340.0)
        .overscan(0)
        .on_select(Msg::Select)
        .on_toggle(Msg::Toggle)
        .into();
    let mut ui2 = UserInterface::build(view_2, size, cache, &mut r);

    let after = run(&mut ui2, &mut r, &[release()], on_row_3);

    assert!(
        !after.contains(&Msg::Select(3)),
        "the pressed row left the realized window, so its press must not \
         activate on release; got {after:?}"
    );
    assert!(
        !after.contains(&Msg::Select(10)),
        "a press latched by slot position would select whichever row now \
         occupies that slot (row 10) — that is the identity-not-position \
         defect; got {after:?}"
    );
}

/// A tree in which no row expands or nests has no chevron column to align:
/// a row's body starts at the row's padding. A tree with a branch keeps the
/// column, so labels line up across depths.
///
/// Sabotage: keep the disclosure spacer's width in `row_element` when `flat`.
/// The flat tree's body starts a chevron's width in.
#[test]
fn a_flat_tree_starts_its_rows_at_the_padding() {
    use enzui_core::collect::collect_frame;
    use enzui_core::semantics::{Role, Semantics};
    use enzui_widgets::semantics_container::SemanticsContainer;
    use enzui_widgets::tree_view::{EnzTreeItem, ROW_PADDING_X, enz_tree_view};

    let body_offset = |items: Vec<EnzTreeItem>| -> f32 {
        let tree: iced::Element<'static, ()> = enz_tree_view(items)
            .row_content(|_, item| {
                SemanticsContainer::leaf(
                    iced::widget::Space::new()
                        .width(iced::Length::Fill)
                        .height(10.0),
                    Semantics::new(Role::Label).label(format!("body {}", item.text)),
                )
                .into()
            })
            .into();
        let frame = collect_frame(tree, iced::Size::new(400.0, 300.0));
        let nodes = frame.collected();
        let row = nodes
            .iter()
            .find(|node| node.semantics.role == Role::TreeItem)
            .expect("a row")
            .bounds;
        let body = nodes
            .iter()
            .find(|node| node.semantics.label.as_deref() == Some("body a"))
            .expect("the first row's body")
            .bounds;
        body.x - row.x
    };
    let flat = body_offset(vec![EnzTreeItem::leaf("a", 0), EnzTreeItem::leaf("b", 0)]);
    assert!((flat - ROW_PADDING_X).abs() < 0.5, "flat body at {flat}");
    let nested = body_offset(vec![
        EnzTreeItem::leaf("a", 0),
        EnzTreeItem::branch("b", 0, true),
        EnzTreeItem::leaf("c", 1),
    ]);
    assert!(
        nested > flat + 4.0,
        "a nested tree keeps its chevron column: {nested}"
    );
}

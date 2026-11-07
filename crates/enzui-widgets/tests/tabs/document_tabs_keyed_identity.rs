//! `EnzDocumentTabs` must reconcile its cells by document key, not by slot.
//!
//! # The failure mode
//!
//! The model is keyed all the way down, and the *widget* can throw that away
//! at the last step: `into_element` building plain `Row::with_children` /
//! `Column::with_children` vectors leaves iced diffing children
//! positionally. Iced core's `Tree::diff_children_custom` reconciler
//! truncates or extends the retained child-state vector and then `zip`s old
//! state against new widgets by index, never consulting an `Id`. A widget `Id`
//! makes a cell *addressable*; it does nothing for *reconciliation*.
//!
//! So inserting a document at the front, or reordering the strip, hands slot
//! *k*'s retained widget state — pressed, hover, animation, AT identity — to
//! whichever tab moved into slot *k*. That is the same class already closed for
//! tab strips and button groups, in the one collection whose entire reason to
//! exist is stable keyed identity.
//!
//! # Why the test must mutate the list between frames
//!
//! A single-frame test cannot observe this at all, which is how it survives
//! a large green widget suite: build one strip, press a tab, read the result,
//! and the answer is right whether reconciliation is keyed or positional. The
//! divergence exists only in the *transition* between two frames
//! sharing one `Cache`. Every case here therefore builds a strip, presses a
//! tab, rebuilds with a mutated list, and releases.
//!
//! # Why press, and not focus
//!
//! The obvious probe — focus a cell, mutate, read focus back — is not
//! observable here by construction, and it is a recorded trap. In a
//! strip the cells are built `embedded`, which deliberately withdraws per-cell
//! focus registration so the collection keeps one roving stop. Per-cell focus
//! would read `None` in every frame and every "focus did not jump" assertion
//! would pass vacuously.
//!
//! `FocusButton::is_pressed` is the per-cell retained state that *does* exist.
//! It lives in the child's own `Tree` — so a positional diff hands it to
//! whichever cell lands in the slot — and it has a clean behavioural
//! observable: a press is completed into a message only by the cell that
//! *owns* it. Press, mutate, release; if reconciliation is positional the
//! activation names the wrong document.
//!
//! # The fixture
//!
//! Keys descend while positions ascend (`e d c b a`), because a fixture
//! whose keys ascend with position cannot distinguish a key-based
//! implementation from an index-based one: both pass. Titles are non-monotonic
//! too, so "picked the first tab" and "picked the right tab" name different
//! strings.

use enzui_core::document_tabs::{DocumentItem, DocumentTabs};
use enzui_widgets::document_tab::enz_document_tab;
use enzui_widgets::document_tabs::enz_document_tabs;
use iced::advanced::widget::Operation;
use iced::{Element, Size};
use iced_test::runtime::user_interface::{Cache, UserInterface};

type Renderer = iced_test::renderer::Renderer;

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    /// Activation, carrying the DOCUMENT KEY — never an index. An index-carrying
    /// message could not distinguish the two rules at all, because the whole
    /// question is which document a slot belongs to.
    Activate(String),
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

const STRIP_W: f32 = 1_200.0;
const TAB_W: f32 = 160.0;

/// Build a model from `(key, title)` pairs, none pinned.
fn model(items: &[(&str, &str)]) -> DocumentTabs {
    DocumentTabs::from_items(
        items
            .iter()
            .map(|(k, t)| DocumentItem::new(*k, *t))
            .collect(),
    )
    .expect("unique keys")
}

/// One strip over `m`. Each cell's activation message carries its own document
/// key, so a message identifies the document, not the slot.
fn strip(m: &DocumentTabs) -> Element<'_, Msg> {
    enz_document_tabs(
        m,
        "keyed-identity",
        |_| TAB_W,
        |item, _ctx| {
            enz_document_tab(
                item.title.clone(),
                Msg::Activate(item.key.as_str().to_owned()),
            )
            .into()
        },
    )
    .extent(STRIP_W)
    .width(iced::Length::Fill)
    .into()
}

/// Drive one frame over `m`, feeding `events` at `cursor`, collecting messages.
///
/// The `Cache` is threaded through by the caller, which is what makes `diff`
/// actually run against last frame's tree. A fresh cache per frame would
/// reconcile nothing and every assertion here would be vacuous.
fn frame(
    m: &DocumentTabs,
    cache: Cache,
    r: &mut Renderer,
    events: &[iced::Event],
    cursor: iced::advanced::mouse::Cursor,
    msgs: &mut iced::advanced::shell::Bus<Msg>,
) -> Cache {
    let mut ui = UserInterface::build(strip(m), Size::new(1_400.0, 300.0), cache, r);
    let _ = ui.update(
        &iced::advanced::graphics::core::window::Headless,
        &iced::advanced::graphics::core::shell::Waker::noop(),
        events,
        cursor,
        r,
        msgs,
    );
    ui.into_cache()
}

/// The centre of the cell whose published `Role::Tab` node is labelled `title`.
///
/// Located by label, out of the laid-out tree, rather than by index: the
/// point of the whole test is that indices move, so an index-addressed probe
/// would be measuring with the ruler under test.
fn centre_of(m: &DocumentTabs, title: &str, r: &mut Renderer) -> iced::Point {
    let mut ui = UserInterface::build(strip(m), Size::new(1_400.0, 300.0), Cache::default(), r);
    let mut c = enzui_core::collect::SemanticsCollector::new();
    ui.operate(r, &mut c as &mut dyn Operation);
    let node = c
        .into_collected()
        .into_iter()
        .find(|n| {
            n.semantics.role == enzui_core::semantics::Role::Tab
                && n.semantics.label.as_deref() == Some(title)
        })
        .unwrap_or_else(|| panic!("a Role::Tab node labelled {title:?} must be laid out"));
    iced::Point::new(
        node.bounds.x + node.bounds.width / 2.0,
        node.bounds.y + node.bounds.height / 2.0,
    )
}

fn press() -> iced::Event {
    iced::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left))
}
fn release() -> iced::Event {
    iced::Event::Mouse(iced::mouse::Event::ButtonReleased(
        iced::mouse::Button::Left,
    ))
}

/// Keys DESCEND while positions ascend, so no assertion below can be satisfied
/// by an index that happens to equal a key's ordinal.
const FIXTURE: [(&str, &str); 5] = [
    ("e", "mu.rs"),
    ("d", "xi.rs"),
    ("c", "delta.rs"),
    ("b", "chi.rs"),
    ("a", "rho.rs"),
];

/// The same, for a pure REORDER at constant length.
///
/// This is the case iced's own `diff_children_custom_with_search` misses
/// entirely — it splices only on a length change (iced's `Tree::diff_children`) and an
/// equal-length permutation falls through to the positional `zip` at `:183`.
/// So a strip could "be keyed" and still fail this.
///
/// The permutation is a full reversal of 5 elements, which has a **palindromic
/// fixed point at position 2**, which is exactly the trap a centre-pressed
/// specimen falls into. The pressed document is therefore `b` (position 3 → 1),
/// deliberately away from the centre, so a positional reconcile and a keyed one
/// disagree.
#[test]
fn press_state_follows_the_document_key_across_a_reorder() {
    let before = model(&FIXTURE);
    let mut reversed: Vec<(&str, &str)> = FIXTURE.to_vec();
    reversed.reverse();
    let after = model(&reversed);

    let mut r = renderer();
    let mut msgs = iced::advanced::shell::Bus::new();

    // Press document 'b' — position 3 before, position 1 after. Not the fixed
    // point.
    let at_before = iced::advanced::mouse::Cursor::Available(centre_of(&before, "chi.rs", &mut r));
    let cache = frame(
        &before,
        Cache::default(),
        &mut r,
        &[press()],
        at_before,
        &mut msgs,
    );
    assert!(msgs.is_empty(), "precondition: press does not activate");

    let at_after = iced::advanced::mouse::Cursor::Available(centre_of(&after, "chi.rs", &mut r));
    let _ = frame(&after, cache, &mut r, &[release()], at_after, &mut msgs);

    assert_eq!(
        msgs,
        vec![Msg::Activate("b".to_owned())],
        "the retained press must follow DOCUMENT 'b' from position 3 to 1 \
         across a reversal"
    );
}

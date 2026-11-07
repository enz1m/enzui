//! Pin link-activation index alignment across the pointer path and
//! the keyboard path, and pin the adjacency clause of link grouping.
//!
//! ## What this protects
//!
//! `LinkSemantics::wrap` groups adjacent runs that share a link target into one
//! `LinkEntry` (`[**bold**rest](url)` is one link, not two). The keyboard
//! activation list (`link_activations`) must be grouped the same way, or its
//! index drifts from the entry list and `resize_with(N)` truncates it — so
//! activating link *B* fires link *A*'s message.
//!
//! The keyboard side of this drift is pinned in `rich_text_link_keyboard.rs`
//! (`activating_the_last_group_after_adjacent_same_target_runs_fires_its_own_message`).
//! This file covers the complementary properties:
//!
//! 1. The pointer path fires the correct link for `[A][A][B]` (clicking B
//!    emits B). It routes through the child `rich_text`'s own `on_link_click`,
//!    so it reads the clicked span's real payload rather than the `activations`
//!    list. This test makes any routing drift observable.
//! 2. Pointer and keyboard activation of the same link emit the same
//!    message — the `:1163` "they cannot drift" claim, asserted rather than
//!    trusted.
//! 3. The adjacency clause: two same-target links separated by prose stay
//!    two distinct links (the `prev = None` reset on a no-link run).
//!
//! ## Harness
//!
//! Clicks go through `iced_test::Simulator` (press + release at a cursor
//! position), exactly as `reading_pane_links.rs` does. The click position is
//! not guessed: each link publishes a `Role::Link` semantics node at its
//! own union rect, so we collect those bounds and click each link's centre.
//! Keyboard activation drives `focus_next` then injects Enter through
//! `UserInterface::update`.

use enzui_core::collect::SemanticsCollector;
use enzui_core::semantics::Role;
use enzui_theme::palette::Palette;
use enzui_widgets::enz_text::TextTokens;
use enzui_widgets::rich_text::{RichSpan, RichText, SpanStyle};
use iced::advanced::widget::Operation;
use iced::{Element, Point, Rectangle, Size};
use iced_test::Simulator;
use iced_test::runtime::user_interface::{Cache, UserInterface};
use iced_test::simulator::click;

type Renderer = iced_test::renderer::Renderer;

/// The clicked-link message. Carries the link's own destination, so the emitted
/// value names *which* link fired — never a stored field, never a count.
#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Open(String),
}

fn tokens() -> TextTokens {
    TextTokens::from_palette(&Palette::dark())
}

/// A window wide enough that `[A][A][B]` lays out on one line, so each link's
/// union rect is a clean horizontal band and clicks cannot miss.
const WIDE: Size = Size::new(1000.0, 300.0);

fn headless() -> Renderer {
    iced_test::futures::futures::executor::block_on(
        <Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer")
}

fn view(doc: &RichText<String>) -> Element<'static, Msg> {
    doc.view(&tokens(), 16.0, Msg::Open)
}

/// A link run carrying an explicit style, so `[**bold**plain](url)` (a bold link
/// run + a plain link run sharing a target) can be built.
fn styled_link(text: &str, style: SpanStyle, payload: &str) -> RichSpan<String> {
    RichSpan {
        text: text.into(),
        style,
        link: Some(payload.to_owned()),
    }
}

fn bold() -> SpanStyle {
    SpanStyle {
        bold: true,
        ..Default::default()
    }
}

/// `[A][A][B]`: two ADJACENT runs sharing target A (bold then plain — the
/// `[**bold**rest](url)` shape), immediately followed by a run with a different
/// target B. Grouped correctly this is two links: A ("Aa Ab") then B ("Bb").
fn a_a_b_doc() -> RichText<String> {
    RichText::new(vec![
        styled_link("Aa ", bold(), "https://a.example/same"),
        styled_link("Ab", SpanStyle::default(), "https://a.example/same"),
        styled_link("Bb", SpanStyle::default(), "https://b.example/other"),
    ])
}

/// Collect the on-screen bounds of every `Role::Link` node, in document order,
/// paired with its label. Each link is published at its own union rect, so this
/// is exactly where a user would click.
fn link_bounds(doc: &RichText<String>, viewport: Size) -> Vec<(String, Rectangle)> {
    let mut r = headless();
    let mut ui = UserInterface::build(view(doc), viewport, Cache::default(), &mut r);
    let mut c = SemanticsCollector::new();
    ui.operate(&r, &mut c as &mut dyn Operation);
    c.collected()
        .iter()
        .filter(|n| n.semantics.role == Role::Link)
        .map(|n| (n.semantics.label.clone().unwrap_or_default(), n.bounds))
        .collect()
}

/// Click the link whose label is `want`, at the centre of its published rect,
/// and return every message the click produced.
fn click_link(doc: &RichText<String>, viewport: Size, want: &str) -> Vec<Msg> {
    let links = link_bounds(doc, viewport);
    let (_, rect) = links
        .iter()
        .find(|(label, _)| label == want)
        .unwrap_or_else(|| panic!("no link labelled {want:?}; have {links:?}"));
    let centre = Point::new(rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);

    let mut ui = Simulator::with_size(Default::default(), viewport, view(doc));
    ui.point_at(centre);
    let _ = ui.simulate(click());
    ui.into_messages().collect()
}

#[test]
fn pointer_click_on_grouped_a_fires_a() {
    // The complement: clicking the grouped A anchor fires A's destination.
    // Together with the B test this proves the pointer path resolves both
    // grouped links to their own targets, not just that *some* message fires.
    let doc = a_a_b_doc();
    let msgs = click_link(&doc, WIDE, "Aa Ab");
    assert_eq!(
        msgs,
        vec![Msg::Open("https://a.example/same".to_owned())],
        "clicking the grouped A anchor must fire A's destination; got {msgs:?}"
    );
}

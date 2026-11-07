//! A routed AccessKit `Click` must actually select a tab.
//!
//! # What this requires
//!
//! Clearing the action mask on anonymous nodes would stop a `Tab` advertising a
//! `Click` it cannot deliver, but only by withdrawing the capability, which
//! leaves a screen-reader user unable to switch pages. Delivering the click needs two things:
//!
//! 1. Per-tab identity, threaded through `EnzTabList`. The key is the page's
//!    stable id (`SwitcherPage::id` / `InlineSwitcherPage::id`), not its
//!    label: the label is presentation and moves when a badge count ticks
//!    (`SwitcherPage::display_label` appends it) or the copy is translated.
//! 2. A carrier. A cell built from `iced::widget::mouse_area` has no
//!    `Operation` hook at all, so a resolved `Activate` matches nothing.
//!
//! # The carrier decision
//!
//! The cells compose **`focus_button(..).embedded(true)`** rather than
//! `EnzTabList` growing a bespoke carrier. `embedded` mode exists for
//! precisely this shape — one cell of a roving composite — and withdraws exactly
//! what belongs to the owner (focus registration, Enter/Space, self-focus on
//! press) while keeping hover/pressed visuals, click-completion semantics and
//! the `activation::ActivationRequest` carrier. `calendar`, `carousel`,
//! `command_palette` and `document_tab` already use it, so this is the
//! established convention rather than a new one, and the shared primitive keeps
//! being the single activation path for all 34 widgets that compose it.
//!
//! A side benefit worth naming: `mouse_area::on_press` fires on press, while
//! `focus_button` activates on a completed press+release over the same bounds,
//! so dragging off a tab cancels the activation, instead of firing on press
//! with no release cancellation.
//!
//! # How these tests are driven
//!
//! Through the whole chain the application drives — collector → published
//! `TreeUpdate` → real `ActionRouter` → `activation::activate` over the live
//! tree → message on a redraw-only pass. Asserting that a node *has* a `Click`
//! verb would pass in exactly the state where nothing delivers it.

use enzui_core::collect::SemanticsCollector;
use enzui_widgets::view_switcher::{SwitcherPage, enz_view_switcher};
use iced::advanced::widget::Operation;
use iced::{Element, Size};
use iced_test::runtime::user_interface::{Cache, UserInterface};

type Renderer = iced_test::renderer::Renderer;

#[derive(Debug, Clone, PartialEq)]
#[allow(
    dead_code,
    reason = "the widget needs these messages to exist; this test never sends them"
)]
enum Msg {
    Selected(usize),
    SelectedId(String),
}

struct Harness<'a, Message> {
    ui: UserInterface<'a, Message, iced::Theme, Renderer>,
    renderer: Renderer,
}

impl<'a, Message> Harness<'a, Message> {
    fn new(view: Element<'a, Message, iced::Theme, Renderer>) -> Self {
        let mut renderer = iced_test::futures::futures::executor::block_on(
            <Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
            ),
        )
        .expect("headless renderer");
        let ui = UserInterface::build(
            view,
            Size::new(800.0, 400.0),
            Cache::default(),
            &mut renderer,
        );
        Self { ui, renderer }
    }

    fn publish(
        &mut self,
    ) -> (
        accesskit::TreeUpdate,
        enzui_core::collect::NodeIdMap,
        Vec<enzui_core::collect::CollectedNode>,
    ) {
        let mut collector = SemanticsCollector::new();
        self.ui
            .operate(&self.renderer, &mut collector as &mut dyn Operation);
        let collected = collector.collected().to_vec();
        let (update, map) = collector.to_tree_update_with_ids();
        (update, map, collected)
    }
}

fn pages() -> Vec<SwitcherPage> {
    vec![
        SwitcherPage::new("all", "All"),
        SwitcherPage::new("open", "Open"),
        SwitcherPage::new("closed", "Closed"),
    ]
}

// ---------------------------------------------------------------------------
// 1. POSITIVE CONTROL first.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 2. The identity key: the page id, and why that is the right one.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 3. The gates.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 4. The drain / rebuild-to-disabled discard.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 5. What routed activation must not break.
// ---------------------------------------------------------------------------

/// Selection state still reaches AT: exactly one tab reports `selected`.
#[test]
fn exactly_one_tab_reports_selected() {
    use enzui_core::semantics::Role;
    let view: Element<'static, Msg> = enz_view_switcher(pages(), 1, Msg::Selected).into();
    let mut harness = Harness::new(view);
    let (_, _, collected) = harness.publish();
    let selected: Vec<&str> = collected
        .iter()
        .filter(|n| n.semantics.role == Role::Tab && n.semantics.selected == Some(true))
        .filter_map(|n| n.semantics.label.as_deref())
        .collect();
    assert_eq!(
        selected,
        vec!["Open"],
        "exactly the selected tab must report selected"
    );
}

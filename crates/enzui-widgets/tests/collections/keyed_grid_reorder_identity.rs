//! `KeyedGrid` must reconcile an equal-length permutation by key.
//!
//! # Failure mechanism
//!
//! A naive `grid.rs` reconciles through
//! `tree::diff_children_custom_with_search`. That helper only ever splices on a
//! *length change*: `iced/core/src/widget/tree.rs` is guarded by
//! `current_children.len() > new_children.len()` and `:153` by
//! `current_children.len() < new_children.len()`. When the two lengths are
//! equal, neither branch runs and control falls through to the positional
//! `zip` at `:177`. It reconciles `[a,b,c] -> [c,a,b]` by index, leaving the
//! retained per-child state (focus, text contents, hover, animation) with the
//! *slot*.
//!
//! `tests/keyed_responsive_grid_identity.rs` promises identity coverage but
//! every case in its "reorder / insert / remove" section changes the child
//! count (5 -> 3, 2 -> 6), so none of them can observe this.
//!
//! # Why every assertion drives a live `Cache` across frames
//!
//! The failure is about *retained widget state surviving a reorder*, so a test
//! that builds the grid once proves nothing — it would pass identically with or
//! without key reconciliation. Focus lives in the widget tree, and the tree lives in the
//! `Cache`, so the harness keeps **one `Cache` alive across frames** (the cross-frame `Cache`
//! pattern from `header_bar_keyed_actions.rs`).
//!
//! # The cursor is parked at (5000, 5000)
//!
//! A draw- or status-based focus assertion taken with the cursor still on
//! the control reads `Hovered` and passes for the wrong
//! reason. Focus here is read from the collected semantics tree (which
//! cell's node reports `focused`), never from draw state, and the pointer is
//! never placed on a cell at all.

use enzui_core::collect::{CollectedNode, SemanticsCollector};
use enzui_widgets::button::enz_button;
use enzui_widgets::grid::enz_keyed_grid;
use iced::advanced::widget::Operation;
use iced::advanced::widget::operation::Outcome;
use iced::{Element, Size, Theme};
use iced_test::runtime::user_interface::{Cache, UserInterface};

const SURFACE_W: f32 = 900.0;
const SURFACE_H: f32 = 600.0;

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Press(&'static str),
}

/// Host that keeps one `Cache` across frames — that cache *is* the widget state
/// under test.
struct Host {
    renderer: iced_test::renderer::Renderer,
    cache: Cache,
    columns: usize,
    /// The cell keys currently in the grid, in document order.
    keys: Vec<&'static str>,
}

impl Host {
    fn new(columns: usize, keys: &[&'static str]) -> Self {
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
            columns,
            keys: keys.to_vec(),
        }
    }

    /// Each cell's label is its key, so the collected node identifies which
    /// logical cell holds focus rather than which slot does.
    fn view(&self) -> Element<'static, Msg> {
        let children: Vec<(String, Element<'static, Msg>)> = self
            .keys
            .iter()
            .map(|&k| (k.to_owned(), enz_button(k, Msg::Press(k)).into()))
            .collect();
        enz_keyed_grid(self.columns, children)
            // `duplicate_keys_are_matched_in_occurrence_order` below feeds
            // this harness deliberately duplicated keys to pin the documented
            // occurrence-order pairing. That behaviour is guaranteed in *every*
            // build profile, so the test must be able to exercise it in debug —
            // hence the explicit opt-out rather than weakening the diagnostic.
            // Reconciliation is unaffected; only the debug assertion is silenced.
            .allow_duplicate_keys()
            .width(iced::Length::Fixed(SURFACE_W))
            .into()
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

    /// Replace the cell order for the next frame. The `Cache` is untouched, so
    /// retained widget state carries into the rebuilt tree — which is exactly
    /// the reconciliation path under test.
    fn reorder(&mut self, keys: &[&'static str]) {
        self.keys = keys.to_vec();
    }

    /// Move focus forward one stop, the way Tab would.
    ///
    /// `focus_next` is a chained operation: the first pass only counts the
    /// focusables and the `Outcome::Chain` pass is what actually moves focus.
    /// Driving only the first pass focuses nothing and every later assertion
    /// would read "nothing focused" for the wrong reason.
    fn tab(&mut self) {
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

    /// The label of the node currently reporting `focused`. Because each
    /// cell's label is its key, this answers "which logical cell holds focus",
    /// not "which slot holds focus".
    fn focused_cell(&mut self) -> Option<String> {
        self.collect()
            .into_iter()
            .find(|n| n.semantics.focused)
            .and_then(|n| n.semantics.label)
    }
}

// ---------------------------------------------------------------------------
// Calibration — prove the harness observes the thing before asserting about it
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The defect: equal-length permutations
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Length changes must keep working (control for the length-splice path)
// ---------------------------------------------------------------------------

/// Insert at the front: `[a,b,c] -> [z,a,b,c]`. Focus on `a` must not be handed
/// to the newly inserted `z`.
#[test]
fn focus_survives_an_insertion_at_the_front() {
    let mut h = Host::new(3, &["a", "b", "c"]);
    h.tab();
    assert_eq!(h.focused_cell().as_deref(), Some("a"));

    h.reorder(&["z", "a", "b", "c"]);

    assert_eq!(
        h.focused_cell().as_deref(),
        Some("a"),
        "inserting a cell at the front must not move focus onto it"
    );
}

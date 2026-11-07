//! Header-bar action rows must reconcile by identity, not by slot.
//!
//! # The half that was already fixed, and the half that was not
//!
//! `header_bar.rs` already forwards each action's canonical `Action::id()` onto
//! the rendered button (`Id::new(a.action.id())`). That makes a control
//! addressable — focus targeting, AT routing and tests can name it.
//!
//! It does not make it reconciled by identity. iced's child
//! reconciliation is positional: `iced/core/src/widget/tree.rs`
//! (`diff_children_custom`) truncates/extends `self.children` to the new length
//! and then `zip`s old state against new widgets by index. `Id` is never
//! consulted. So inserting an action at the front, or swapping two, hands slot
//! *k*'s retained widget state — focus, pressed, hover — to whatever action now
//! occupies slot *k*. The user sees the focus ring jump to a different command.
//!
//! Same keyed-reconciliation defect class as `EnzMenu` and other dynamic collections.
//!
//! # Why every assertion drives real interaction across a live `Cache`
//!
//! The failure is about *retained widget state surviving a reorder*, so a test
//! that builds the row once proves nothing — it would pass identically before
//! and after an insert. Focus lives in the widget tree, and the tree lives in the
//! `Cache`, so the harness keeps **one `Cache` alive across frames**. A test
//! that rebuilt from a fresh cache would reset focus and measure nothing.
//!
//! # The cursor is parked at (5000, 5000)
//!
//! A draw- or status-based focus assertion taken with the cursor still on
//! the control reads `Hovered` and passes for the wrong
//! reason. Focus here is read from the collected semantics tree (which
//! action's node reports `focused`), and every frame runs with the cursor
//! parked far away.

use enzui_core::Action;
use enzui_core::collect::{CollectedNode, SemanticsCollector};
use enzui_widgets::header_bar::{HeaderAction, enz_header_bar};
use iced::advanced::widget::Operation;
use iced::advanced::widget::operation::Outcome;
use iced::{Element, Size, Theme};
use iced_test::runtime::user_interface::{Cache, UserInterface};

const SURFACE_W: f32 = 1280.0;
const SURFACE_H: f32 = 200.0;

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Act(&'static str),
}

/// One action whose canonical id *and* visible label are `id`, so the collected
/// node's label identifies which command a node belongs to.
fn act(id: &'static str) -> HeaderAction<Msg> {
    HeaderAction::new(Action::new(id, Msg::Act(id)).with_label(id))
}

/// Host that keeps one `Cache` across frames — that cache *is* the widget
/// state under test.
struct Host {
    renderer: iced_test::renderer::Renderer,
    cache: Cache,
    /// The action ids currently in the end zone, in document order.
    actions: Vec<&'static str>,
}

impl Host {
    fn new(actions: &[&'static str]) -> Self {
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
            actions: actions.to_vec(),
        }
    }

    fn view(&self) -> Element<'static, Msg> {
        enz_header_bar("Docs")
            .end_actions(self.actions.iter().copied().map(act).collect())
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

    /// Replace the action order for the next frame. The `Cache` is untouched,
    /// so retained widget state carries into the rebuilt tree — which is
    /// exactly the reconciliation path under test.
    fn reorder(&mut self, actions: &[&'static str]) {
        self.actions = actions.to_vec();
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
    /// action's label is its canonical id, this answers "which command holds
    /// focus", not "which slot holds focus".
    fn focused_action(&mut self) -> Option<String> {
        self.collect()
            .into_iter()
            .find(|n| n.semantics.focused)
            .and_then(|n| n.semantics.label)
    }
}

/// Insert at the front: focus `alpha`, then unshift a new
/// action ahead of it. Everything shifts one slot right.
#[test]
fn focus_survives_an_insertion_at_the_front() {
    let mut h = Host::new(&["alpha", "beta", "gamma"]);
    h.tab();
    assert_eq!(h.focused_action().as_deref(), Some("alpha"));

    h.reorder(&["inserted", "alpha", "beta", "gamma"]);

    assert_eq!(
        h.focused_action().as_deref(),
        Some("alpha"),
        "inserting an action at the front must not move focus onto it"
    );
}

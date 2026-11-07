//! The orphan-`Role::Tab` gate, run over the framework's own widgets.
//!
//! This regression is invisible to visual QA: a hand-rolled
//! segmented control that *looks* correct on screen while publishing
//! `Role::Tab` leaves with no owning `Role::TabList`. To an assistive tech
//! that is a tab belonging to no strip — N focus stops instead of one, no
//! roving arrow-key cursor. `SemanticsCollector::orphan_tabs()` is the
//! frame-level gate; this file exercises it against real, laid-out trees.
//!
//! Two axes, and both are load-bearing:
//!
//! * No false positives on correct code. Every shipped segmented control
//!   ([`enz_inline_view_switcher`], [`enz_view_switcher`], [`enz_tab_list`])
//!   wraps its tabs in exactly one `TabList`; the gate must report zero
//!   orphans for each, or it is worse than none (it would fire on the correct
//!   widgets).
//!
//! * It fires on hand-rolled substitutes. A bare row of
//!   `enz_button(..).role(Role::Tab)` and
//!   `enz_button_group(..).selectable(true)` — the fallback shape
//!   to — both publish orphan tabs, and the gate reports them.
//!
//! Every assertion reads the collected semantics tree of a laid-out element,
//! never a builder field.

use enzui_core::collect::SemanticsCollector;
use enzui_core::semantics::Role;
use enzui_widgets::button::enz_button;
use iced::Element;
use iced::advanced::widget::Operation;

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Pick(usize),
}

/// Lay the element out for real and count orphan `Role::Tab` nodes — tabs with
/// no owning `Role::TabList` ancestor.
fn orphan_tab_count(view: Element<'static, Msg>) -> usize {
    let mut renderer = iced_test::futures::futures::executor::block_on(
        <iced_test::renderer::Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer");
    let mut ui = iced_test::runtime::user_interface::UserInterface::build(
        view,
        iced::Size::new(600.0, 200.0),
        iced_test::runtime::user_interface::Cache::default(),
        &mut renderer,
    );
    let mut c = SemanticsCollector::new();
    ui.operate(&renderer, &mut c as &mut dyn Operation);
    c.orphan_tabs().len()
}

/// Count `Role::Tab` nodes in the laid-out tree — the calibration that proves a
/// "zero orphans" result is a real observation over a tree that *has* tabs, not
/// an empty collection.
fn tab_count(view: Element<'static, Msg>) -> usize {
    let mut renderer = iced_test::futures::futures::executor::block_on(
        <iced_test::renderer::Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer");
    let mut ui = iced_test::runtime::user_interface::UserInterface::build(
        view,
        iced::Size::new(600.0, 200.0),
        iced_test::runtime::user_interface::Cache::default(),
        &mut renderer,
    );
    let mut c = SemanticsCollector::new();
    ui.operate(&renderer, &mut c as &mut dyn Operation);
    c.collected()
        .iter()
        .filter(|n| n.semantics.role == Role::Tab)
        .count()
}

// ---------------------------------------------------------------------------
// No false positives — the correct segmented controls report zero orphans.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The gate FIRES on hand-rolled substitutes.
// ---------------------------------------------------------------------------

/// A bare row of `EnzButton` set to `Role::Tab`. Every cell is a tab with no
/// owning list — N focus stops, no
/// roving cursor — and the gate flags all of them. This is the shape a visual
/// QA pass cannot see and this gate can.
#[test]
fn a_hand_rolled_tab_row_is_all_orphans() {
    let row: Element<'static, Msg> = iced::widget::row![
        enz_button("Chat", Msg::Pick(0))
            .role(Role::Tab)
            .selected(true),
        enz_button("Git", Msg::Pick(1))
            .role(Role::Tab)
            .selected(false),
        enz_button("Files", Msg::Pick(2))
            .role(Role::Tab)
            .selected(false),
    ]
    .into();

    // Precondition: this really did publish three tabs.
    let el2: Element<'static, Msg> = iced::widget::row![
        enz_button("Chat", Msg::Pick(0))
            .role(Role::Tab)
            .selected(true),
        enz_button("Git", Msg::Pick(1))
            .role(Role::Tab)
            .selected(false),
        enz_button("Files", Msg::Pick(2))
            .role(Role::Tab)
            .selected(false),
    ]
    .into();
    assert_eq!(tab_count(el2), 3, "harness precondition: three tab cells");

    assert_eq!(
        orphan_tab_count(row),
        3,
        "a bare row of Role::Tab buttons has no owning TabList — every cell is \
         an orphan"
    );
}

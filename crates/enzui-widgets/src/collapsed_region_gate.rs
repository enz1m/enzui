//! Check disclosure semantics for framework-owned collapsed shell regions.
//!
//! Registered specimens are collected as real frames with the sidebar closed and
//! open at narrow widths. Closed frames omit sidebar headings and must publish a
//! named disclosure with `expanded = false` and a routable ID. Open frames restore
//! headings and publish `expanded = true`. A separate check verifies that a routed
//! expand operation arms a handler; it does not dispatch the resulting message.
//!
//! The registry covers shell-owned sidebar toggles. It excludes caller-owned
//! disclosures in [`crate::navigation_split_view`], app-owned panels and whether
//! app content needs headings. Apps supply those checks and their own region keys.

use crate::app_shell::{ShellLayout, ShellTokens, SidebarHeader, enz_app_shell};
use crate::nav_split::NavSplitState;
use crate::section_header::enz_section_header;
use enzui_core::collect::CollectedNode;
use iced::widget::{column, text};
use iced::{Element, Length};

/// Sidebar heading used to distinguish closed and revealed frames.
pub const REGION_CAPTION: &str = "PLACES";

/// Whether the registry specimen is built with its region shut or open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionState {
    /// The region is collapsed out of the tree.
    Collapsed,
    /// The region is on screen, so its nodes are published.
    Revealed,
}

/// State-change messages advertised by the specimen’s sidebar toggle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateMessage {
    /// The shell's flipped sidebar state (`EnzAppShell::on_sidebar_state`).
    SidebarState(NavSplitState),
}

/// One shell specimen the gate walks: a shape that collapses a structural
/// region out of the accessibility tree.
///
/// See [`registry`] for the shipped entries and the module docs for scope.
pub struct CollapsedRegionCase {
    /// Shell configuration name used in failure reports.
    pub widget: &'static str,
    /// Collapsing region name used in failure reports.
    pub region: &'static str,
    /// Logical-pixel width used for the collapsed frame.
    pub collapsed_width: f32,
    /// Logical-pixel width used for the revealed frame.
    ///
    /// May equal [`Self::collapsed_width`] when revealing a drawer at narrow width.
    pub revealed_width: f32,
    /// Build the specimen in the given state.
    pub build: fn(RegionState) -> Element<'static, GateMessage>,
}

// The disappearing caption is a heading so closure is observable in the outline.
fn sidebar_slot() -> Element<'static, GateMessage> {
    column![
        enz_section_header(REGION_CAPTION).into_element(),
        text("Home"),
    ]
    .width(Length::Fill)
    .into()
}

// Content stays headingless to distinguish sidebar structure from the main pane.
fn content_slot() -> Element<'static, GateMessage> {
    column![text("a file")]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn shell(
    layout: ShellLayout,
    tokens: ShellTokens,
    state: RegionState,
) -> Element<'static, GateMessage> {
    let nav = match state {
        RegionState::Collapsed => NavSplitState::new(),
        RegionState::Revealed => NavSplitState::for_sidebar_first(true),
    };
    let mut builder = enz_app_shell(content_slot)
        .layout(layout)
        .tokens(tokens)
        .state(nav)
        .sidebar(sidebar_slot)
        .on_sidebar_state(GateMessage::SidebarState);
    if layout.has_titlebar() {
        builder = builder.titlebar(|| text("Title").into());
    }
    builder.into()
}

/// Shell specimens whose sidebars leave the accessibility tree when collapsed.
///
/// Each entry must omit the sidebar heading when closed and restore it when open.
/// Add newly supported collapsing shell configurations here to extend coverage.
#[must_use]
pub fn registry() -> Vec<CollapsedRegionCase> {
    // Both states use narrow width so opening a drawer cannot pass through inline layout.
    const NARROW: f32 = 420.0;

    vec![
        CollapsedRegionCase {
            widget: "enz_app_shell TitlebarSidebarContent / SidebarHeader::Inside",
            region: "sidebar",
            collapsed_width: NARROW,
            revealed_width: NARROW,
            build: |s| {
                shell(
                    ShellLayout::TitlebarSidebarContent,
                    ShellTokens::default().sidebar_header(SidebarHeader::Inside),
                    s,
                )
            },
        },
        CollapsedRegionCase {
            widget: "enz_app_shell TitlebarSidebarContent / SidebarHeader::SplitBand",
            region: "sidebar",
            collapsed_width: NARROW,
            revealed_width: NARROW,
            build: |s| {
                shell(
                    ShellLayout::TitlebarSidebarContent,
                    ShellTokens::default().sidebar_header(SidebarHeader::SplitBand),
                    s,
                )
            },
        },
        CollapsedRegionCase {
            widget: "enz_app_shell TitlebarSidebarContent / SidebarHeader::SpanningBand",
            region: "sidebar",
            collapsed_width: NARROW,
            revealed_width: NARROW,
            build: |s| {
                shell(
                    ShellLayout::TitlebarSidebarContent,
                    ShellTokens::default().sidebar_header(SidebarHeader::SpanningBand),
                    s,
                )
            },
        },
        CollapsedRegionCase {
            widget: "enz_app_shell TitlebarSidebarContent / full_width_drawer",
            region: "sidebar",
            collapsed_width: NARROW,
            revealed_width: NARROW,
            build: |s| {
                shell(
                    ShellLayout::TitlebarSidebarContent,
                    ShellTokens::default().full_width_drawer(true),
                    s,
                )
            },
        },
        CollapsedRegionCase {
            widget: "enz_app_shell FloatingTitlebarSidebar",
            region: "sidebar",
            collapsed_width: NARROW,
            revealed_width: NARROW,
            build: |s| {
                shell(
                    ShellLayout::FloatingTitlebarSidebar,
                    ShellTokens::default(),
                    s,
                )
            },
        },
        CollapsedRegionCase {
            widget: "enz_app_shell FloatingSidebar",
            region: "sidebar",
            collapsed_width: NARROW,
            revealed_width: NARROW,
            build: |s| shell(ShellLayout::FloatingSidebar, ShellTokens::default(), s),
        },
    ]
}

/// Extract disclosure names, expansion states and widget-ID presence.
///
/// ID presence is a routing precondition; it does not prove a handler is registered.
#[must_use]
pub fn disclosures(nodes: &[CollectedNode]) -> Vec<(Option<String>, bool, bool)> {
    nodes
        .iter()
        .filter_map(|n| {
            n.semantics
                .expanded
                .map(|e| (n.semantics.label.clone(), e, n.id.is_some()))
        })
        .collect()
}

/// Count nodes satisfying [`CollectedNode::is_heading`].
#[must_use]
pub fn heading_count(nodes: &[CollectedNode]) -> usize {
    nodes.iter().filter(|n| n.is_heading()).count()
}

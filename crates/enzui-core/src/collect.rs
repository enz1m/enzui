//! Renderer-neutral collection of widget semantics, text, bounds, and traversal depth for accessibility adapters.

use crate::scroll_stack::ScrollStack;
use crate::semantics::Semantics;
use iced_core::Rectangle;
use iced_core::widget::Id;

/// Carries accessibility semantics through `Operation::custom`; collectors ignore other payload types.
#[derive(Debug, Clone)]
#[must_use]
pub struct SemanticsNode {
    /// The control's semantics.
    pub semantics: Semantics,
    /// Whether this node parents subsequently collected nodes at greater depth.
    pub is_group: bool,
    /// Visible text to suppress when it is redundant with this control’s accessible representation.
    pub content_text: Option<String>,
}

/// Delimits a subtree that operations traverse but accessibility collection ignores.
///
/// Wrappers send [`Hide`](Self::Hide), operate on their child, and then send
/// [`Restore`](Self::Restore) within one traversal callback. Other operations
/// ignore these markers, so focus, action, and shaped-text discovery still
/// reach the child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticsCollectionBoundary {
    /// Begin suppressing semantic nodes and text.
    Hide,
    /// End the innermost suppression region.
    Restore,
}

/// Text that describes the first control in the subtree it is sent for: a
/// tooltip's text, which becomes that control's accessible description
/// unless it has one.
///
/// A wrapper sends it through `Operation::custom` and then traverses its
/// child, so the description applies inside the wrapper only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescribesContent(pub String);

impl SemanticsNode {
    /// Carrier for a leaf control.
    pub fn new(semantics: Semantics) -> Self {
        Self {
            semantics,
            is_group: false,
            content_text: None,
        }
    }

    /// Creates a carrier that parents subsequently collected nodes at greater depth.
    pub fn group(semantics: Semantics) -> Self {
        Self {
            semantics,
            is_group: true,
            content_text: None,
        }
    }

    /// Sets visible text that should not become a separate accessibility label.
    pub fn with_content_text(mut self, text: impl Into<String>) -> Self {
        self.content_text = Some(text.into());
        self
    }
}

/// One semantics node collected during a tree walk, with the geometry and
/// nesting depth the AT tree needs.
#[derive(Debug, Clone)]
pub struct CollectedNode {
    /// Stable widget identity used for cross-frame node identity and action routing.
    pub id: Option<Id>,
    /// Screen-space bounds at collection time.
    pub bounds: Rectangle,
    /// Traversal depth, with `0` at the root, used to reconstruct containment.
    pub depth: usize,
    /// Whether this node opens a group scope for deeper nodes.
    pub is_group: bool,
    /// The control's semantics.
    pub semantics: Semantics,
}

impl CollectedNode {
    /// Returns whether this node is an explicit or promoted heading.
    pub fn is_heading(&self) -> bool {
        self.semantics.is_heading()
    }
}

/// Collects widget semantics in document order with bounds and traversal depth.
#[derive(Debug, Default)]
pub struct SemanticsCollector {
    nodes: Vec<CollectedNode>,
    depth: usize,
    // Open `(depth, name, description, content_text)` scopes used to suppress only nested duplicate labels.
    interactive_ancestors: Vec<(usize, String, Option<String>, Option<String>)>,
    // Open `(depth, node_index)` candidates whose first nonblank descendant text supplies their name.
    name_from_content: Vec<(usize, usize)>,
    // Depths of open labelled live regions. The region's name is what an
    // assistive client announces; loose text inside it stays navigable but
    // silent, or the client would announce the same content twice.
    live_regions: Vec<usize>,
    semantics_suppression_depth: usize,
    // `(depth, names)` of the link leaves just collected at one depth. An
    // inline-link paragraph publishes its links before its own text; text that
    // is only those links is the same words announced twice.
    preceding_links: Option<(usize, String)>,
    // Actionable nodes that remained nameless during this collector’s frame.
    nameless_actionable: Vec<nameless::NamelessActionable>,
    // Tracks enclosing scroll translations so collected bounds remain screen-space.
    scroll: ScrollStack,
    // For each collected node, the index one past its last descendant.
    subtree_ends: Vec<usize>,
    // Open `(depth, node_index)` subtrees, closed when traversal leaves them.
    open_subtrees: Vec<(usize, usize)>,
    // Labelled groups whose first control already took their label relation.
    labelled_groups_used: Vec<usize>,
    // Open `(depth, text, used)` descriptions for the first control inside.
    pending_descriptions: Vec<(usize, String, bool)>,
}

impl SemanticsCollector {
    /// A fresh, empty collector.
    pub fn new() -> Self {
        Self::default()
    }

    /// The nodes collected so far, in document order.
    pub fn collected(&self) -> &[CollectedNode] {
        &self.nodes
    }

    /// Consume the collector, returning the collected nodes.
    pub fn into_collected(self) -> Vec<CollectedNode> {
        self.nodes
    }

    /// The collected nodes with each node's subtree extent: `ends[i]` is one
    /// past the last node collected inside node `i`'s traversal, so
    /// `i + 1..ends[i]` are exactly its descendants.
    ///
    /// `depth` alone cannot delimit a subtree when a sibling's own container
    /// publishes no node: the sibling's deeper descendants then look nested
    /// in the preceding node. The extent is recorded when the traversal that
    /// produced the node returns, so it is exact.
    #[must_use]
    pub fn into_collected_with_subtree_ends(mut self) -> (Vec<CollectedNode>, Vec<usize>) {
        self.close_subtrees(0);
        (self.nodes, self.subtree_ends)
    }

    /// Returns duplicated widget IDs once each, ordered by their first repeated occurrence.
    ///
    /// Anonymous nodes are ignored.
    pub fn duplicate_ids(&self) -> Vec<Id> {
        crate::identity::duplicates(self.nodes.iter().filter_map(|n| n.id.as_ref()))
    }

    /// Returns roles of actionable nodes without widget IDs in this frame.
    ///
    /// Such nodes are published without actions because they cannot be routed.
    #[must_use]
    pub fn anonymous_actionable(&self) -> Vec<crate::semantics::Role> {
        self.nodes
            .iter()
            .filter(|n| n.id.is_none() && n.semantics.advertises_action())
            .map(|n| n.semantics.role)
            .collect()
    }

    /// Returns bounds of tabs that have no owning tab-list ancestor in this frame.
    #[must_use]
    pub fn orphan_tabs(&self) -> Vec<Rectangle> {
        use crate::semantics::Role;
        // Pop scopes at the same or shallower depth; surviving group scopes are the true ancestors.
        let mut ancestors: Vec<(usize, bool)> = Vec::new();
        let mut orphans = Vec::new();
        for n in &self.nodes {
            while let Some(&(d, _)) = ancestors.last() {
                if d >= n.depth {
                    ancestors.pop();
                } else {
                    break;
                }
            }
            if n.semantics.role == Role::Tab {
                let owned = ancestors.iter().any(|&(_, is_list)| is_list);
                if !owned {
                    orphans.push(n.bounds);
                }
            }
            if n.is_group {
                ancestors.push((n.depth, n.semantics.role == Role::TabList));
            }
        }
        orphans
    }

    /// Returns `true` only when neither semantic nodes nor static text were collected.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

/// Marks the root of an app-owned widget subtree for structural census.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AppSubtreeBoundary;

/// Returns the widget-tree tag recognized as an app-owned subtree boundary.
#[must_use]
pub fn app_subtree_boundary_tag() -> iced_core::widget::tree::Tag {
    iced_core::widget::tree::Tag::of::<AppSubtreeBoundary>()
}

/// Structural census of the app-owned subtree(s) inside one realised frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppSubtreeCensus {
    /// Number of boundary markers, including markers nested inside another boundary.
    ///
    /// Zero means `subtree_nodes` is not an app-subtree measurement.
    pub boundaries: usize,
    /// Nodes in outermost marked subtrees, counting each node once and summing disjoint boundaries.
    pub subtree_nodes: usize,
}

fn subtree_size(tree: &iced_core::widget::Tree) -> usize {
    1 + tree.children.iter().map(subtree_size).sum::<usize>()
}

/// Counts all boundary markers and all nodes in outermost marked subtrees.
///
/// The walk reads only the realized tree’s tags and children and performs no allocation.
#[must_use]
pub fn app_subtree_census(tree: &iced_core::widget::Tree) -> AppSubtreeCensus {
    let tag = app_subtree_boundary_tag();
    let mut census = AppSubtreeCensus {
        boundaries: 0,
        subtree_nodes: 0,
    };
    // Count nested markers, but add nodes only for outermost marked subtrees.
    fn walk(
        tree: &iced_core::widget::Tree,
        tag: iced_core::widget::tree::Tag,
        in_boundary: bool,
        census: &mut AppSubtreeCensus,
    ) {
        let is_boundary = tree.tag == tag;
        if is_boundary {
            census.boundaries += 1;
            if !in_boundary {
                census.subtree_nodes += subtree_size(tree);
            }
        }
        let inside = in_boundary || is_boundary;
        for child in &tree.children {
            walk(child, tag, inside, census);
        }
    }
    walk(tree, tag, false, &mut census);
    census
}

#[cfg(feature = "accesskit")]
mod ak;
// The renderer-neutral boundary marker ships without `test-frame` because applications wrap production views with it.
mod boundary;
mod containment;
// The shared headless collection frame is gated behind `test-frame`
// because driving the collector needs `iced_test` (a concrete renderer), which
// this otherwise renderer-neutral crate must not pull into its default build.
#[cfg(feature = "test-frame")]
mod frame;
mod nameless;
/// Pixel-level visibility checks for semantic nodes.
pub mod visibility_probe;
mod visit;

#[cfg(feature = "accesskit")]
pub use ak::{NodeIdMap, NodeRouting, ROOT_ID, node_id_of_widget};
pub use boundary::{AppSubtreeBoundaryElement, app_subtree_boundary};
#[cfg(feature = "test-frame")]
pub use frame::{
    FrameAppSubtreeCensus, FrameTreeCensus, collect_frame, collect_frame_with_app_subtree_census,
    collect_frame_with_tree_census,
};
pub use nameless::NamelessActionable;

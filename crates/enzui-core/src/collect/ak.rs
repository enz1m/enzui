//! Conversion from collected semantics to AccessKit tree updates and action-routing maps.
use super::{CollectedNode, SemanticsCollector};
use accesskit::{Node, NodeId, Rect, Role as AkRole, Tree, TreeId, TreeUpdate};
use iced_core::widget::Id;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Stable root node id for the enzUI-ICED AT tree.
pub const ROOT_ID: NodeId = NodeId(0);

/// The action-routing capabilities copied from a collected semantic node.
///
/// These fields retain the state and capabilities used to gate activation, mutation, expansion, and context-menu requests without recollecting the tree.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodeRouting {
    /// Whether the control is disabled; activation, mutation, and context actions are refused, while focus and viewport reveal remain routable.
    pub disabled: bool,
    /// Whether the control is read-only — refuses `SetValue` even when it
    /// exposes a text value.
    pub read_only: bool,
    /// The numeric contract for `Increment` / `Decrement`, if any.
    pub numeric: Option<crate::semantics::NumericValue>,
    /// Whether the control declares numeric adjustability.
    pub adjustable: bool,
    /// Whether the control has the writable text-like shape for string `SetValue`, before disabled-state gating.
    pub text_editable: bool,
    /// Whether the control has the writable spin-button or adjustable-slider
    /// shape for numeric `SetValue`, before disabled-state gating.
    pub numeric_settable: bool,
    /// Whether the control declares context-menu capability, before disabled-state gating.
    pub context_menu: bool,
    /// Published expanded or collapsed state, if any.
    pub expanded: Option<bool>,
    /// Whether the control declares expand/collapse capability; state and disabled status are gated separately.
    pub expandable_action: bool,
    /// Whether the control declares default activation capability, before disabled-state gating.
    pub supports_default_action: bool,
}

impl NodeRouting {
    pub(super) fn from_semantics(s: &crate::semantics::Semantics) -> Self {
        // Keep stored mutation, activation, and context-menu gates aligned with `Semantics::to_accesskit_node`; declared capabilities and required state must match advertised actions.
        let text_editable = s.editable
            && !s.read_only
            && matches!(
                s.role,
                crate::semantics::Role::TextInput
                    | crate::semantics::Role::PasswordInput
                    // Editable combo boxes route string `SetValue` requests through the text-editable gate.
                    | crate::semantics::Role::EditableComboBox
            );
        let numeric_settable = s.numeric_settable();
        Self {
            disabled: s.disabled,
            read_only: s.read_only,
            numeric: s.numeric,
            adjustable: s.adjustable,
            text_editable,
            numeric_settable,
            context_menu: s.supports_context_menu,
            expanded: s.expanded,
            expandable_action: s.expandable_action,
            supports_default_action: s.supports_default_action,
        }
    }
}

/// Maps routable AccessKit node ids back to widget ids and action capabilities.
///
/// Anonymous nodes are intentionally absent, so their action requests cannot be misrouted.
#[derive(Debug, Clone, Default)]
pub struct NodeIdMap {
    entries: Vec<(NodeId, Id, NodeRouting)>,
    /// Each editable text node's lines, as the UTF-8 length of every
    /// character, to map text positions back from its runs.
    text: Vec<(NodeId, Vec<Vec<usize>>)>,
}

impl NodeIdMap {
    /// The widget [`Id`] a given AccessKit node maps back to, if any.
    pub fn widget_id(&self, node: NodeId) -> Option<&Id> {
        self.entries
            .iter()
            .find(|(n, _, _)| *n == node)
            .map(|(_, id, _)| id)
    }

    /// Returns action-routing capabilities for an AccessKit node, if the node is routable.
    pub fn routing(&self, node: NodeId) -> Option<&NodeRouting> {
        self.entries
            .iter()
            .find(|(n, _, _)| *n == node)
            .map(|(_, _, r)| r)
    }

    /// The AccessKit node a given widget [`Id`] resolves to, if it was
    /// collected (inverse of [`Self::widget_id`]).
    pub fn node_of(&self, id: &Id) -> Option<NodeId> {
        self.entries
            .iter()
            .find(|(_, wid, _)| wid == id)
            .map(|(n, _, _)| *n)
    }

    /// The character lengths of each line of `node`'s text, when it
    /// publishes text runs.
    pub fn text_lines(&self, node: NodeId) -> Option<&[Vec<usize>]> {
        self.text
            .iter()
            .find(|(n, _)| *n == node)
            .map(|(_, lines)| lines.as_slice())
    }

    /// The text position (line and byte index) that `position` names in
    /// `node`'s text, when its run belongs to `node`.
    pub fn text_position(
        &self,
        node: NodeId,
        position: accesskit::TextPosition,
    ) -> Option<iced_core::text::Position> {
        let lines = self.text_lines(node)?;
        let line = (0..lines.len()).find(|line| text_run_id(node, *line) == position.node)?;
        let characters = &lines[line];
        let index = position.character_index.min(characters.len());
        Some(iced_core::text::Position {
            line,
            index: characters.iter().take(index).sum(),
        })
    }

    /// Iterates routable node and widget-id pairs in collection order.
    pub fn iter_ids(&self) -> impl Iterator<Item = (NodeId, &Id)> + '_ {
        self.entries.iter().map(|(node, id, _)| (*node, id))
    }

    /// Number of routable (id-bearing) nodes.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no node carries a stable widget id.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Derives an AccessKit node id solely from a widget id.
///
/// The result does not prove that the widget is present in a collected tree; relation publishers must check presence separately.
pub fn node_id_of_widget(id: &Id) -> NodeId {
    let mut hasher = DefaultHasher::new();
    id.hash(&mut hasher);
    // Reserve zero and the low half for the root and anonymous per-index ids; widget hashes occupy the high half.
    NodeId(hasher.finish() | (1 << 63))
}

// Process-global widget ids whose duplicate semantic nodes were dropped, in first-seen order.
static DROPPED_DUPLICATES: std::sync::Mutex<Vec<Id>> = std::sync::Mutex::new(Vec::new());

// Record each dropped duplicate id and print it once; release drops the shadow instead of publishing a malformed tree or crashing the event loop.
pub(super) fn report_dropped_duplicate(id: &Id, role: crate::semantics::Role) {
    let Ok(mut seen) = DROPPED_DUPLICATES.lock() else {
        // Diagnostic reporting must not escalate a mutex-poisoning panic.
        return;
    };
    if seen.contains(id) {
        return;
    }
    seen.push(id.clone());
    log::warn!(
        target: "enzui::a11y",
        "enzui a11y: dropped a duplicate semantic node id {id:?} \
         (role {role:?}). Two live controls published the SAME widget Id, \
         so both hash to one AccessKit NodeId. The first in document order \
         is kept and published; this one is dropped, which keeps the AT \
         tree well-formed but leaves the shadowed control unreachable by \
         assistive technology. Give the instances distinct host-owned \
         identities (`enzui_core::identity::OwnerId`, or the widget's \
         `.id()` / `.id_prefix()`)."
    );
}

// Process-global roles whose anonymous actionable nodes were degraded, in first-seen order.
static ANONYMOUS_ACTIONABLE: std::sync::Mutex<Vec<crate::semantics::Role>> =
    std::sync::Mutex::new(Vec::new());

// Record each anonymous actionable role and print it once; its unroutable actions are removed without aborting the event loop.
pub(super) fn report_anonymous_actionable(role: crate::semantics::Role) {
    let Ok(mut seen) = ANONYMOUS_ACTIONABLE.lock() else {
        // Diagnostic reporting must not escalate a mutex-poisoning panic.
        return;
    };
    if seen.contains(&role) {
        return;
    }
    seen.push(role);
    log::warn!(
        target: "enzui::a11y",
        "enzui a11y: an ACTIONABLE semantic node (role {role:?}) was \
         published with NO widget Id. An anonymous node never enters the \
         NodeIdMap, so every action it advertises (Focus / Click / \
         SetValue / Increment / Expand / ShowContextMenu) resolves to \
         nothing: assistive technology is told the control is operable and \
         it is not. The published tree degrades it to a read-only \
         announcement to stay honest; give the widget a stable identity \
         (`.id(...)` / `.id_prefix(...)`, or \
         `enzui_core::identity::OwnerId`) to make the actions \
         routable again."
    );
}

// Derive stable ids from widget identity; anonymous nodes use collection-order fallback ids.
fn stable_node_id(cn: &CollectedNode, index: usize) -> NodeId {
    match &cn.id {
        Some(id) => node_id_of_widget(id),
        // Anonymous ids use the low range and reserve zero for the root.
        None => NodeId((index as u64) + 1),
    }
}

impl SemanticsCollector {
    /// Returns the process-wide, first-seen widget ids whose duplicate nodes were dropped.
    ///
    /// The log is cumulative for the process lifetime rather than frame-local.
    pub fn dropped_duplicate_ids() -> Vec<Id> {
        DROPPED_DUPLICATES
            .lock()
            .map(|v| v.clone())
            .unwrap_or_default()
    }

    /// Returns the process-wide, first-seen roles whose anonymous actionable nodes were degraded.
    ///
    /// The log is cumulative for the process lifetime rather than frame-local.
    pub fn anonymous_actionable_roles() -> Vec<crate::semantics::Role> {
        ANONYMOUS_ACTIONABLE
            .lock()
            .map(|v| v.clone())
            .unwrap_or_default()
    }

    /// Build an `accesskit::TreeUpdate` from the collected nodes, discarding
    /// the id map. Convenience wrapper over [`Self::to_tree_update_with_ids`].
    pub fn to_tree_update(&self) -> TreeUpdate {
        self.to_tree_update_with_ids().0
    }

    /// Builds an AccessKit tree update and its reverse action-routing map.
    ///
    /// Widget ids produce stable node ids; anonymous nodes receive collection-order ids and advertise no actions. Only group scopes become semantic parents, absent active-descendant targets are omitted, duplicate ids are dropped first-wins, and focus defaults to the root.
    pub fn to_tree_update_with_ids(&self) -> (TreeUpdate, NodeIdMap) {
        self.to_tree_update_with_ids_in_window(None, None)
    }

    /// Builds an AccessKit tree for one window with an optional root label and window-namespaced AT-SPI accessible ids.
    pub fn to_tree_update_for_window(
        &self,
        window: iced_core::window::Id,
        title: Option<&str>,
    ) -> (TreeUpdate, NodeIdMap) {
        self.to_tree_update_with_ids_in_window(Some(window.as_u64()), title)
    }

    fn to_tree_update_with_ids_in_window(
        &self,
        window: Option<u64>,
        title: Option<&str>,
    ) -> (TreeUpdate, NodeIdMap) {
        let mut nodes: Vec<(NodeId, Node)> = Vec::with_capacity(self.nodes.len() + 1);
        let mut id_map = NodeIdMap::default();

        let mut root = Node::new(AkRole::Window);

        let ids: Vec<NodeId> = self
            .nodes
            .iter()
            .enumerate()
            .map(|(i, cn)| stable_node_id(cn, i))
            .collect();

        // Duplicate widget ids collapse to one AccessKit id, so publish only the first collected node and suppress every later shadow.
        let mut suppressed = vec![false; self.nodes.len()];
        {
            let mut seen: std::collections::HashSet<NodeId> =
                std::collections::HashSet::with_capacity(self.nodes.len());
            for (i, cn) in self.nodes.iter().enumerate() {
                if cn.id.is_none() {
                    continue;
                }
                if !seen.insert(ids[i]) {
                    suppressed[i] = true;
                    // Record suppressed duplicates in release because the debug assertion is unavailable there.
                    report_dropped_duplicate(
                        cn.id.as_ref().expect("checked is_none above"),
                        cn.semantics.role,
                    );
                }
            }
        }

        // A node's descendants are exactly `i + 1..ends[i]`; subtrees still
        // open (a collection not yet finished) run to the end.
        let mut ends = self.subtree_ends.clone();
        for &(_, index) in &self.open_subtrees {
            ends[index] = self.nodes.len();
        }
        // Track only group ancestors, by exact extent: depth alone would keep
        // a closed group open across a sibling whose container published no node.
        let mut group_stack: Vec<(usize, usize)> = Vec::new();
        let mut children: Vec<Vec<NodeId>> = vec![Vec::new(); self.nodes.len()];
        let mut root_children: Vec<NodeId> = Vec::new();
        let mut focus = ROOT_ID;
        let mut first_focused_index: Option<usize> = None;

        for (i, cn) in self.nodes.iter().enumerate() {
            if suppressed[i] {
                // Suppressed duplicates must not participate in hierarchy, focus, routing, or node emission.
                continue;
            }
            while group_stack.last().is_some_and(|&(end, _)| end <= i) {
                group_stack.pop();
            }
            match group_stack.last() {
                Some(&(_, parent_i)) => children[parent_i].push(ids[i]),
                None => root_children.push(ids[i]),
            }
            if cn.is_group {
                group_stack.push((ends[i], i));
            }

            if cn.semantics.focused {
                // AccessKit accepts one focused id; a second focus claim is invalid, and release degrades deterministically to the first.
                debug_assert!(
                    focus == ROOT_ID,
                    "two nodes claim focus in one tree: {:?} and {:?} \
                     ({:?} / {:?}). AccessKit can only report one, so the \
                     second is unreachable by assistive tech -- a widget \
                     set focus without clearing its siblings.",
                    focus,
                    ids[i],
                    self.nodes[first_focused_index.unwrap_or(i)].semantics.role,
                    cn.semantics.role,
                );
                if focus == ROOT_ID {
                    focus = ids[i];
                    first_focused_index = Some(i);
                }
            }

            if let Some(id) = &cn.id {
                id_map.entries.push((
                    ids[i],
                    id.clone(),
                    NodeRouting::from_semantics(&cn.semantics),
                ));
            }
        }

        // Debug builds reject duplicate live widget ids after release-safe suppression has kept the published tree well formed.
        debug_assert!(
            self.duplicate_ids().is_empty(),
            "{} widget Id(s) are carried by more than one collected node: \
             {:?}. Each pair hashes to ONE AccessKit NodeId; in release the \
             second and later duplicates are dropped from the tree (see \
             `suppressed` above), so the AT sees a well-formed tree but \
             the shadowed control(s) are unreachable. Two instances of one \
             widget sharing a default id namespace is the usual cause -- \
             give one a distinct .id_prefix()/.id().",
            self.duplicate_ids().len(),
            self.duplicate_ids(),
        );

        // Active-descendant relations may target only widget ids collected in this frame.
        let present: std::collections::HashSet<&Id> =
            self.nodes.iter().filter_map(|cn| cn.id.as_ref()).collect();

        for (i, cn) in self.nodes.iter().enumerate() {
            if suppressed[i] {
                // Suppressed duplicates have no valid linkage and therefore emit no node.
                continue;
            }
            let mut node = cn.semantics.to_accesskit_node();
            node.set_bounds(rect_of(cn));
            let mut node_children = std::mem::take(&mut children[i]);
            if let Some((input, text)) = cn.semantics.text.as_deref() {
                let runs = text_runs(ids[i], *input, text);
                node_children.extend(runs.iter().map(|(id, _)| *id));
                if let Some((anchor, focus)) = text.selection {
                    let at = |position: iced_core::text::Position| {
                        text.lines
                            .get(position.line)
                            .map(|line| accesskit::TextPosition {
                                node: text_run_id(ids[i], position.line),
                                character_index: line.character_at_byte(position.index),
                            })
                    };
                    if let (Some(anchor), Some(focus)) = (at(anchor), at(focus)) {
                        node.set_text_selection(accesskit::TextSelection { anchor, focus });
                    }
                }
                if cn.id.is_some() {
                    node.add_action(accesskit::Action::SetTextSelection);
                    id_map.text.push((
                        ids[i],
                        text.lines
                            .iter()
                            .map(|line| line.characters.clone())
                            .collect(),
                    ));
                }
                nodes.extend(runs);
            }
            if !node_children.is_empty() {
                node.set_children(node_children);
            }
            // Validate active-descendant targets here because only this stage knows the complete collected-id set.
            if let Some(target) = &cn.semantics.active_descendant
                && !present.contains(target)
            {
                node.clear_active_descendant();
            }
            // Relations keep only targets collected in this frame.
            let collected = |ids: &[Id]| {
                ids.iter()
                    .filter(|id| present.contains(*id))
                    .map(node_id_of_widget)
                    .collect::<Vec<_>>()
            };
            for (relation, set, clear) in [
                (
                    &cn.semantics.labelled_by,
                    Node::set_labelled_by as fn(&mut Node, Vec<NodeId>),
                    Node::clear_labelled_by as fn(&mut Node),
                ),
                (
                    &cn.semantics.described_by,
                    Node::set_described_by,
                    Node::clear_described_by,
                ),
                (
                    &cn.semantics.controls,
                    Node::set_controls,
                    Node::clear_controls,
                ),
            ] {
                let targets = collected(relation);
                if targets.is_empty() {
                    clear(&mut node);
                } else {
                    set(&mut node, targets);
                }
            }
            if let Some(target) = &cn.semantics.error_message
                && !present.contains(target)
            {
                node.clear_error_message();
            }
            // Namespace AT-SPI accessible ids by window because the same local widget id may appear in sibling windows.
            if cn.id.is_some()
                && let Some(window) = window
            {
                node.set_author_id(format!("enzui:target:{window:016x}:{:016x}", ids[i].0));
            }
            // Anonymous nodes have no reverse-map entry, so clear every advertised action while preserving readable semantics.
            if cn.id.is_none() {
                // Report only anonymous nodes whose action mask is about to be cleared; anonymous non-actionable content is valid.
                if cn.semantics.advertises_action() {
                    report_anonymous_actionable(cn.semantics.role);
                }
                node.clear_actions();
            }
            nodes.push((ids[i], node));
        }

        root.set_children(root_children);
        // AT-SPI exposes the root label as the frame’s accessible name.
        if let Some(title) = title {
            root.set_label(title.to_owned());
        }
        if let Some(window) = window {
            root.set_author_id(format!("enzui:frame:{window:016x}"));
        }
        nodes.push((ROOT_ID, root));

        (
            TreeUpdate {
                nodes,
                tree: Some(Tree::new(ROOT_ID)),
                tree_id: TreeId::ROOT,
                focus,
            },
            id_map,
        )
    }
}

/// The AccessKit id of line `line`'s text run under the editable text node
/// `parent`.
pub(crate) fn text_run_id(parent: NodeId, line: usize) -> NodeId {
    let mut hasher = DefaultHasher::new();
    ("enzui-text-run", parent.0, line).hash(&mut hasher);
    // The high range: anonymous nodes count up from 1.
    NodeId(hasher.finish() | (1 << 63))
}

/// One `TextRun` per line of `text`, placed from `input`, the box the line
/// positions start from. Lines but the last end with their line break, so
/// the runs read as the whole text.
fn text_runs(
    parent: NodeId,
    input: iced_core::Rectangle,
    text: &crate::semantics::TextLayout,
) -> Vec<(NodeId, Node)> {
    let last = text.lines.len().saturating_sub(1);
    text.lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let mut node = Node::new(AkRole::TextRun);
            let mut value = line.text.clone();
            let mut lengths: Vec<u8> = line
                .characters
                .iter()
                .map(|length| u8::try_from(*length).unwrap_or(u8::MAX))
                .collect();
            let mut positions = line.positions.clone();
            // A right-to-left line's first character sits right of its last.
            let rtl = positions.as_ref().is_some_and(|positions| {
                positions.len() > 1
                    && positions.first().map(|(x, _)| *x) > positions.last().map(|(x, _)| *x)
            });
            node.set_text_direction(if rtl {
                accesskit::TextDirection::RightToLeft
            } else {
                accesskit::TextDirection::LeftToRight
            });
            if index < last {
                value.push('\n');
                lengths.push(1);
                if let Some(positions) = &mut positions {
                    let end = positions.last().map_or(0.0, |(x, width)| x + width);
                    positions.push((end, 0.0));
                }
            }
            if let Some(positions) = positions {
                // AccessKit measures from the edge the line starts at.
                let width = line.bounds.map_or_else(
                    || positions.iter().map(|(x, w)| x + w).fold(0.0, f32::max),
                    |bounds| bounds.width,
                );
                node.set_character_positions(
                    positions
                        .iter()
                        .map(|(x, w)| if rtl { width - (x + w) } else { *x })
                        .collect::<Vec<_>>(),
                );
                node.set_character_widths(positions.iter().map(|(_, w)| *w).collect::<Vec<_>>());
            }
            node.set_word_starts(word_starts(&value, &lengths));
            node.set_character_lengths(lengths);
            node.set_value(value);
            let bounds = match line.bounds {
                // A single line sits in the middle of its input.
                Some(line_bounds) if text.lines.len() == 1 => iced_core::Rectangle {
                    x: input.x + line_bounds.x,
                    y: input.y + ((input.height - line_bounds.height) / 2.0).max(0.0),
                    ..line_bounds
                },
                Some(line_bounds) => iced_core::Rectangle {
                    x: input.x + line_bounds.x,
                    y: input.y + line_bounds.y,
                    ..line_bounds
                },
                None => input,
            };
            node.set_bounds(Rect {
                x0: f64::from(bounds.x),
                y0: f64::from(bounds.y),
                x1: f64::from(bounds.x + bounds.width),
                y1: f64::from(bounds.y + bounds.height),
            });
            (text_run_id(parent, index), node)
        })
        .collect()
}

/// The characters that start a word in `text`, split into characters of
/// `lengths` bytes: each one that is not a space but follows a space or the
/// start. AccessKit counts them in a byte, so starts past 255 are left out.
fn word_starts(text: &str, lengths: &[u8]) -> Vec<u8> {
    let mut starts = Vec::new();
    let mut byte = 0;
    let mut after_space = true;
    for (index, length) in lengths.iter().enumerate() {
        let character = text.get(byte..byte + usize::from(*length)).unwrap_or("");
        byte += usize::from(*length);
        let space = character.chars().all(char::is_whitespace);
        if !space
            && after_space
            && let Ok(index) = u8::try_from(index)
        {
            starts.push(index);
        }
        after_space = space;
    }
    starts
}

fn rect_of(cn: &CollectedNode) -> Rect {
    let b = cn.bounds;
    Rect {
        x0: f64::from(b.x),
        y0: f64::from(b.y),
        x1: f64::from(b.x) + f64::from(b.width),
        y1: f64::from(b.y) + f64::from(b.height),
    }
}

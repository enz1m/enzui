//! Depth-first containment helpers for collector ancestor and name-candidate stacks.
//!
//! An entry at depth `d` owns subsequent nodes with greater depth and closes at the first node with depth at most `d`.

use super::SemanticsCollector;

impl SemanticsCollector {
    pub(super) fn close_descriptions(&mut self, depth: usize) {
        while self
            .pending_descriptions
            .last()
            .is_some_and(|(d, _, _)| *d >= depth)
        {
            self.pending_descriptions.pop();
        }
    }

    pub(super) fn close_live_regions(&mut self, depth: usize) {
        while self.live_regions.last().is_some_and(|d| *d >= depth) {
            self.live_regions.pop();
        }
    }

    pub(super) fn close_interactive_ancestors(&mut self, depth: usize) {
        while let Some((d, _, _, _)) = self.interactive_ancestors.last() {
            if *d >= depth {
                self.interactive_ancestors.pop();
            } else {
                break;
            }
        }
    }

    /// Record the node just pushed as an open subtree at the current depth.
    pub(super) fn open_subtree(&mut self) {
        let index = self.nodes.len() - 1;
        self.subtree_ends.push(index + 1);
        self.open_subtrees.push((self.depth, index));
    }

    /// Close every open subtree at `depth` or deeper at the current node count.
    pub(super) fn close_subtrees(&mut self, depth: usize) {
        while let Some(&(d, index)) = self.open_subtrees.last() {
            if d < depth {
                break;
            }
            self.open_subtrees.pop();
            self.subtree_ends[index] = self.nodes.len();
        }
    }

    pub(super) fn close_name_candidates(&mut self, depth: usize) {
        while let Some(&(d, index)) = self.name_from_content.last() {
            if d >= depth {
                self.name_from_content.pop();
                let node = &self.nodes[index];
                if node.semantics.advertises_action() {
                    let role = node.semantics.role;
                    let id = node.id.clone();
                    self.report_nameless_actionable(role, id.as_ref());
                }
            } else {
                break;
            }
        }
    }
}

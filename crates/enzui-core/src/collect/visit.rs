//! [`SemanticsCollector`] traversal callbacks for collecting nodes and static text.

use super::{CollectedNode, SemanticsCollectionBoundary, SemanticsCollector, SemanticsNode};
use crate::semantics::Semantics;
use iced_core::widget::Id;
use iced_core::widget::operation::scrollable::Scrollable;
use iced_core::widget::operation::{Operation, Outcome};
use iced_core::{Rectangle, Size, Vector};

impl<T> Operation<T> for SemanticsCollector {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        self.depth += 1;
        self.scroll.traverse_enter();
        operate(self);
        self.scroll.traverse_exit();
        self.depth -= 1;
        // Restore suppression scope at subtree exit so later siblings cannot inherit this subtree’s ancestors.
        self.close_interactive_ancestors(self.depth);
        self.close_live_regions(self.depth);
        self.close_descriptions(self.depth);
        // Close candidates at subtree exit; an icon-only control may have no later callback to trigger its nameless-actionable report.
        self.close_name_candidates(self.depth);
        self.close_subtrees(self.depth);
    }

    fn scrollable(
        &mut self,
        id: Option<&Id>,
        bounds: Rectangle,
        _content_bounds: Size,
        translation: Vector,
        _state: &mut dyn Scrollable,
    ) {
        // Translate descendant bounds, but retain offscreen nodes for keyboard accessibility navigation.
        self.scroll.note_scrollable(id, bounds, translation);
    }

    fn custom(&mut self, id: Option<&Id>, bounds: Rectangle, state: &mut dyn std::any::Any) {
        if let Some(boundary) = state.downcast_ref::<SemanticsCollectionBoundary>() {
            match boundary {
                SemanticsCollectionBoundary::Hide => self.semantics_suppression_depth += 1,
                SemanticsCollectionBoundary::Restore => {
                    self.semantics_suppression_depth =
                        self.semantics_suppression_depth.saturating_sub(1);
                }
            }
            return;
        }
        if self.semantics_suppression_depth > 0 {
            return;
        }
        if let Some(super::DescribesContent(text)) = state.downcast_ref::<super::DescribesContent>()
        {
            self.close_descriptions(self.depth);
            self.pending_descriptions
                .push((self.depth, text.clone(), false));
            return;
        }
        if let Some(node) = state.downcast_ref::<SemanticsNode>() {
            self.note_link_run(node);
            self.close_interactive_ancestors(self.depth);
            self.close_name_candidates(self.depth);
            self.close_live_regions(self.depth);
            if node.is_group
                && matches!(
                    node.semantics.live,
                    Some(crate::semantics::Live::Polite | crate::semantics::Live::Assertive)
                )
                && node
                    .semantics
                    .label
                    .as_deref()
                    .is_some_and(|label| !label.trim().is_empty())
            {
                self.live_regions.push(self.depth);
            }
            // Name-owning structural roles also suppress nested copies; interactivity is too narrow.
            if node.semantics.owns_duplicable_name()
                && let Some(name) = node.semantics.label.as_deref()
            {
                // Retain description and drawn content so either duplicate can be suppressed within this subtree.
                self.interactive_ancestors.push((
                    self.depth,
                    name.to_owned(),
                    node.semantics.description.clone(),
                    node.content_text.clone(),
                ));
            }
            // Eligible unnamed controls take their first nonblank descendant text as their accessible name.
            if node.semantics.takes_name_from_content()
                && node
                    .semantics
                    .label
                    .as_deref()
                    .is_none_or(|l| l.trim().is_empty())
            {
                self.name_from_content.push((self.depth, self.nodes.len()));
            }
            self.close_subtrees(self.depth);
            let mut semantics = node.semantics.clone();
            // A tooltip describes the first control inside it.
            if semantics.is_interactive()
                && let Some((_, text, used)) = self
                    .pending_descriptions
                    .iter_mut()
                    .rev()
                    .find(|(depth, _, used)| *depth < self.depth && !*used)
            {
                *used = true;
                // A tooltip repeating the control's name adds nothing to hear.
                if semantics.description.is_none() && semantics.label.as_deref() != Some(text) {
                    semantics.description = Some(text.clone());
                }
            }
            // A group labelled by a caption (a form row) labels the first
            // control in it too, as a label-for relation does.
            if semantics.is_interactive() && semantics.labelled_by.is_empty() {
                let donor = self.open_subtrees.iter().rev().find_map(|&(_, index)| {
                    let group = &self.nodes[index];
                    (group.is_group
                        && group.semantics.role == crate::semantics::Role::Group
                        && !group.semantics.labelled_by.is_empty())
                    .then_some(index)
                });
                if let Some(donor) = donor
                    && !self.labelled_groups_used.contains(&donor)
                {
                    semantics
                        .labelled_by
                        .clone_from(&self.nodes[donor].semantics.labelled_by);
                    self.labelled_groups_used.push(donor);
                }
            }
            self.nodes.push(CollectedNode {
                id: id.cloned(),
                bounds: self.scroll.translate(bounds),
                depth: self.depth,
                is_group: node.is_group,
                semantics,
            });
            self.open_subtree();
        }
    }

    fn text_input(
        &mut self,
        id: Option<&Id>,
        bounds: Rectangle,
        state: &mut dyn iced_core::widget::operation::TextInput,
    ) {
        if self.semantics_suppression_depth > 0 {
            return;
        }
        // In document order the last node shallower than this input is the
        // node enclosing it: the field whose text this is.
        let Some(node) = self
            .nodes
            .iter_mut()
            .rev()
            .find(|node| node.depth < self.depth)
        else {
            return;
        };
        let semantics = &node.semantics;
        // A password's characters never reach the tree.
        if !semantics.editable
            || semantics.role == crate::semantics::Role::PasswordInput
            || semantics.text.is_some()
            || node
                .id
                .as_ref()
                .zip(id)
                .is_some_and(|(node, input)| node != input)
        {
            return;
        }
        // Past this a document is too large to republish each frame; it
        // belongs in a virtual text surface.
        const MAX_TEXT_BYTES: usize = 64 * 1024;
        if let Some(text) = state.accessible_text()
            && text.lines.iter().map(|line| line.text.len()).sum::<usize>() <= MAX_TEXT_BYTES
        {
            node.semantics.text = Some(Box::new((self.scroll.translate(bounds), text)));
        }
    }

    fn text(&mut self, id: Option<&Id>, bounds: Rectangle, text: &str) {
        if self.semantics_suppression_depth > 0 || text.trim().is_empty() {
            return;
        }
        if self.text_repeats_preceding_links(text) {
            return;
        }
        // Close exited scopes before matching so equal text in a later sibling is not suppressed.
        self.close_interactive_ancestors(self.depth);
        self.close_live_regions(self.depth);
        // Before duplicate checks, the innermost unnamed candidate claims its first nonblank text and the loose label is omitted.
        self.close_name_candidates(self.depth);
        if let Some(&(candidate_depth, index)) = self.name_from_content.last() {
            self.name_from_content.pop();
            self.nodes[index].semantics.label = Some(text.to_owned());
            // Insert by depth because ancestor cleanup assumes this stack is ascending.
            let at = self
                .interactive_ancestors
                .iter()
                .position(|(d, _, _, _)| *d > candidate_depth)
                .unwrap_or(self.interactive_ancestors.len());
            self.interactive_ancestors.insert(
                at,
                (
                    candidate_depth,
                    text.to_owned(),
                    self.nodes[index].semantics.description.clone(),
                    None,
                ),
            );
            return;
        }
        if self
            .interactive_ancestors
            .iter()
            .any(|(_, name, desc, content)| {
                name == text || desc.as_deref() == Some(text) || content.as_deref() == Some(text)
            })
        {
            return;
        }
        let mut semantics = Semantics::new(crate::semantics::Role::Label).label(text);
        if !self.live_regions.is_empty() {
            // Inside a labelled live region the region's name is the
            // announcement; this copy stays readable but must not speak again.
            semantics = semantics.live(Some(crate::semantics::Live::Off));
        }
        self.close_subtrees(self.depth);
        self.nodes.push(CollectedNode {
            id: id.cloned(),
            bounds: self.scroll.translate(bounds),
            depth: self.depth,
            is_group: false,
            semantics,
        });
        self.open_subtree();
    }

    fn finish(&self) -> Outcome<T> {
        Outcome::None
    }
}

impl SemanticsCollector {
    // Extends the run of link leaves at this depth, or ends it for any other node.
    fn note_link_run(&mut self, node: &SemanticsNode) {
        let name = node.semantics.label.as_deref().unwrap_or_default();
        if node.is_group || node.semantics.role != crate::semantics::Role::Link || name.is_empty() {
            self.preceding_links = None;
            return;
        }
        match &mut self.preceding_links {
            Some((depth, names)) if *depth == self.depth => names.push_str(name),
            _ => self.preceding_links = Some((self.depth, name.to_owned())),
        }
    }

    // Whether `text` at this depth is exactly the link run just collected,
    // ignoring whitespace; the run ends either way.
    fn text_repeats_preceding_links(&mut self, text: &str) -> bool {
        let Some((depth, names)) = self.preceding_links.take() else {
            return false;
        };
        let compact = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
        depth == self.depth && compact(text) == compact(&names)
    }
}

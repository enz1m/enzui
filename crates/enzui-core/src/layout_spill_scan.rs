//! Renderer-neutral collection of layout spills published by widget-tree probes.

use crate::scroll_stack::ScrollStack;
use iced_core::widget::Id;
use iced_core::widget::operation::scrollable::Scrollable;
use iced_core::widget::operation::{Operation, Outcome};
use iced_core::{Rectangle, Size, Vector};

/// Layout data published by a spill probe during a widget-tree operation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutSpillProbeNode {
    /// The child's bounds in the current widget-operation coordinate space.
    pub child: Rectangle,
    /// The container bounds the child is compared against.
    pub container: Rectangle,
    /// Whether the spill function should treat overflow as intentional.
    pub allow_spill: bool,
    /// Display scale used when the probe was published.
    pub scale: f32,
}

/// Computes the worst spill amount for a child and container, or `None` when accepted.
pub type SpillFn = dyn Fn(Rectangle, Rectangle, bool, f32) -> Option<f32> + Send + Sync;

/// A layout spill found during a widget-tree scan.
#[derive(Debug, Clone)]
pub struct LayoutSpillFinding {
    /// Stable id of the probe that raised the finding (its `report-id` analogue).
    pub id: Option<Id>,
    /// The child's screen-space bounds.
    pub child: Rectangle,
    /// The container bounds the child spilled out of.
    pub container: Rectangle,
    /// The worst single-edge spill in pixels.
    pub spill_amount: f32,
    /// Nesting depth at collection time (for parent/child correlation).
    pub depth: usize,
}

/// Results from one layout-spill scan.
#[derive(Debug, Clone)]
pub struct LayoutSpillScanResult {
    /// Findings retained before the reporting cap was reached.
    pub findings: Vec<LayoutSpillFinding>,
    /// Findings discovered but omitted after the cap was reached.
    pub overflowed: usize,
    /// Findings omitted because their containers were outside every ancestor scrollable's viewport.
    pub clipped_out_of_view: usize,
    /// Number of registered spill subjects examined by the scan.
    pub subjects_examined: usize,
}

impl LayoutSpillScanResult {
    /// Returns all discovered findings, including omitted and clipped findings.
    pub fn total(&self) -> usize {
        self.findings.len() + self.overflowed + self.clipped_out_of_view
    }

    /// Returns whether the scan discovered no spill, including omitted findings.
    pub fn is_clean(&self) -> bool {
        self.total() == 0
    }

    /// Returns whether the scan examined no registered spill subjects.
    pub fn examined_nothing(&self) -> bool {
        self.subjects_examined == 0
    }

    /// True when the retained finding list is incomplete (either the cap
    /// was hit or findings were scrolled out of view).
    pub fn is_truncated(&self) -> bool {
        self.overflowed > 0 || self.clipped_out_of_view > 0
    }
}

/// Collects bounded layout-spill findings from a widget-tree operation.
pub struct LayoutSpillScanCollector<'f> {
    findings: Vec<LayoutSpillFinding>,
    depth: usize,
    spill: &'f SpillFn,
    max_findings: usize,
    overflowed: usize,
    // Tracks translations and clips contributed by ancestor scrollables.
    scroll: ScrollStack,
    // Counts spills discarded after translation because they lie outside the effective clip.
    clipped: usize,
    // Counts recognized spill probes even when they produce no retained finding.
    subjects: usize,
}

/// Default reporting cap (throttle) for a single tree walk.
pub const DEFAULT_MAX_FINDINGS: usize = 256;

impl<'f> LayoutSpillScanCollector<'f> {
    /// A collector using the default reporting cap.
    pub fn new(spill: &'f SpillFn) -> Self {
        Self::with_cap(spill, DEFAULT_MAX_FINDINGS)
    }

    /// Creates a collector with an explicit maximum number of retained findings.
    pub fn with_cap(spill: &'f SpillFn, max_findings: usize) -> Self {
        Self {
            findings: Vec::new(),
            depth: 0,
            spill,
            max_findings,
            overflowed: 0,
            scroll: ScrollStack::new(),
            clipped: 0,
            subjects: 0,
        }
    }

    /// Returns the number of registered spill subjects examined so far.
    pub fn subjects_examined(&self) -> usize {
        self.subjects
    }

    /// The findings collected so far, in document order.
    pub fn findings(&self) -> &[LayoutSpillFinding] {
        &self.findings
    }

    /// Consumes the collector and returns retained findings without scan metadata.
    pub fn into_findings(self) -> Vec<LayoutSpillFinding> {
        self.findings
    }

    /// Consumes the collector and returns findings with scan metadata.
    pub fn into_result(self) -> LayoutSpillScanResult {
        LayoutSpillScanResult {
            findings: self.findings,
            overflowed: self.overflowed,
            clipped_out_of_view: self.clipped,
            subjects_examined: self.subjects,
        }
    }

    /// Returns whether no spill was discovered, including capped or clipped spills.
    pub fn is_clean(&self) -> bool {
        self.findings.is_empty() && self.overflowed == 0 && self.clipped == 0
    }

    /// How many findings were dropped after hitting `max_findings`.
    pub fn overflowed(&self) -> usize {
        self.overflowed
    }

    /// Returns spills discarded because they were outside the effective scroll clip.
    pub fn clipped_out_of_view(&self) -> usize {
        self.clipped
    }
}

impl<T> Operation<T> for LayoutSpillScanCollector<'_> {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        self.depth += 1;
        self.scroll.traverse_enter();
        operate(self);
        self.scroll.traverse_exit();
        self.depth -= 1;
    }

    fn scrollable(
        &mut self,
        id: Option<&Id>,
        bounds: Rectangle,
        _content_bounds: Size,
        translation: Vector,
        _state: &mut dyn Scrollable,
    ) {
        // Promote each noted scrollable on the next descent so descendant probes inherit its translation and clip.
        self.scroll.note_scrollable(id, bounds, translation);
    }

    fn custom(&mut self, id: Option<&Id>, _bounds: Rectangle, state: &mut dyn std::any::Any) {
        // Count every `custom()` visit for the scan-cost gate.
        crate::diagnostics_host::NODE_VISITS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let Some(node) = state.downcast_ref::<LayoutSpillProbeNode>() else {
            return;
        };
        self.subjects += 1;
        // Compute spill before translation because translation preserves its magnitude; only reported bounds move to screen space.
        let Some(amount) = (self.spill)(node.child, node.container, node.allow_spill, node.scale)
        else {
            return;
        };
        // Apply all ancestor translations, then discard findings whose container is outside the effective clip.
        let screen_child = self.scroll.translate(node.child);
        let screen_container = self.scroll.translate(node.container);
        if !self.scroll.intersects_viewport(screen_container) {
            self.clipped += 1;
            return;
        }
        if self.findings.len() >= self.max_findings {
            self.overflowed += 1;
            return;
        }
        self.findings.push(LayoutSpillFinding {
            id: id.cloned(),
            child: screen_child,
            container: screen_container,
            spill_amount: amount,
            depth: self.depth,
        });
    }

    fn finish(&self) -> Outcome<T> {
        Outcome::None
    }
}

/// Reports a completed [`LayoutSpillScanCollector`] through an operation outcome.
pub struct LayoutSpillScanOperation<'f> {
    collector: LayoutSpillScanCollector<'f>,
}

impl<'f> LayoutSpillScanOperation<'f> {
    /// A reporting scan with the default throttle.
    pub fn new(spill: &'f SpillFn) -> Self {
        Self {
            collector: LayoutSpillScanCollector::new(spill),
        }
    }

    /// Creates a reporting scan with an explicit finding cap.
    pub fn with_cap(spill: &'f SpillFn, max_findings: usize) -> Self {
        Self {
            collector: LayoutSpillScanCollector::with_cap(spill, max_findings),
        }
    }

    /// How many findings were dropped after hitting the cap during this walk.
    pub fn overflowed(&self) -> usize {
        self.collector.overflowed()
    }
}

impl Operation<LayoutSpillScanResult> for LayoutSpillScanOperation<'_> {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<LayoutSpillScanResult>)) {
        // Traverse as the collector so its depth bookkeeping encloses every descendant callback.
        Operation::traverse(&mut self.collector, operate);
    }

    fn container(&mut self, id: Option<&Id>, bounds: Rectangle, viewport: &Rectangle) {
        Operation::<LayoutSpillScanResult>::container(&mut self.collector, id, bounds, viewport);
    }

    fn scrollable(
        &mut self,
        id: Option<&Id>,
        bounds: Rectangle,
        content_bounds: Size,
        translation: Vector,
        state: &mut dyn Scrollable,
    ) {
        // Forward scrollable visits so descendant probes use the collector’s complete scroll stack.
        Operation::<LayoutSpillScanResult>::scrollable(
            &mut self.collector,
            id,
            bounds,
            content_bounds,
            translation,
            state,
        );
    }

    fn custom(&mut self, id: Option<&Id>, bounds: Rectangle, state: &mut dyn std::any::Any) {
        Operation::<LayoutSpillScanResult>::custom(&mut self.collector, id, bounds, state);
    }

    fn finish(&self) -> Outcome<LayoutSpillScanResult> {
        Outcome::Some(LayoutSpillScanResult {
            findings: self.collector.findings().to_vec(),
            overflowed: self.collector.overflowed(),
            clipped_out_of_view: self.collector.clipped_out_of_view(),
            subjects_examined: self.collector.subjects_examined(),
        })
    }
}

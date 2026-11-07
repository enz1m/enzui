//! Pairwise overlap detection for widget regions registered through `Operation::custom`.
//!
//! Callers must publish `RegisteredOverlapProbeNode` values; a scan with no
//! recognized regions is not clean. Rectangle intersection does not prove
//! visual occlusion.

use crate::scroll_stack::ScrollStack;
use iced_core::widget::Id;
use iced_core::widget::operation::scrollable::Scrollable;
use iced_core::widget::operation::{Operation, Outcome};
use iced_core::{Rectangle, Size, Vector};

/// Region claim supplied through `Operation::custom`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegisteredOverlapProbeNode {
    /// Bounds in the current operation traversal's coordinate space; ancestor scroll translations are applied during collection.
    pub bounds: Rectangle,
    /// Whether intersections with this region are intentional and suppressed.
    pub allow_overlap: bool,
}

/// One overlap defect: two registered regions whose screen-space bounds
/// intersect.
#[derive(Debug, Clone)]
pub struct RegisteredOverlapFinding {
    /// Id of the first overlapping region.
    pub id_a: Option<Id>,
    /// Id of the second overlapping region.
    pub id_b: Option<Id>,
    /// Bounds of the first region (screen-space).
    pub bounds_a: Rectangle,
    /// Bounds of the second region (screen-space).
    pub bounds_b: Rectangle,
    /// The intersection rectangle.
    pub intersection: Rectangle,
    /// Nesting depth of the second (later) region at collection time.
    pub depth: usize,
}

/// The complete result of one registered-overlap scan walk.
#[derive(Debug, Clone)]
pub struct RegisteredOverlapScanResult {
    /// Overlapping pairs retained before the cap was reached.
    pub findings: Vec<RegisteredOverlapFinding>,
    /// Pairs discovered but omitted after the cap.
    pub overflowed: usize,
    /// Regions dropped because they were scrolled fully out of view.
    pub clipped_out_of_view: usize,
    /// Number of registered regions examined before viewport clipping.
    pub subjects_examined: usize,
}

impl RegisteredOverlapScanResult {
    /// Returns retained findings, overflowed findings, and clipped regions as one total.
    pub fn total(&self) -> usize {
        self.findings.len() + self.overflowed + self.clipped_out_of_view
    }

    /// Returns true only after examining a region with no findings, overflow, or clipping.
    pub fn is_clean(&self) -> bool {
        self.subjects_examined > 0 && self.total() == 0
    }

    /// True when no registered region was examined at all
    pub fn examined_nothing(&self) -> bool {
        self.subjects_examined == 0
    }

    /// True when the retained list is incomplete.
    pub fn is_truncated(&self) -> bool {
        self.overflowed > 0 || self.clipped_out_of_view > 0
    }
}

/// Default cap for overlap findings.
pub const DEFAULT_MAX_FINDINGS: usize = 256;

#[derive(Debug, Clone)]
struct RegisteredRegion {
    id: Option<Id>,
    bounds: Rectangle,
    allow_overlap: bool,
}

/// Operation that collects registered regions and reports pairwise intersections.
pub struct RegisteredOverlapScanCollector {
    regions: Vec<RegisteredRegion>,
    findings: Vec<RegisteredOverlapFinding>,
    depth: usize,
    max_findings: usize,
    overflowed: usize,
    scroll: ScrollStack,
    clipped: usize,
    // Count recognized regions before clipping so a clipped-only walk is distinguishable from an empty walk.
    subjects: usize,
}

impl RegisteredOverlapScanCollector {
    /// A collector using the default reporting cap.
    pub fn new() -> Self {
        Self::with_cap(DEFAULT_MAX_FINDINGS)
    }

    /// Creates a collector with an explicit cap; zero retains no findings but counts overflow.
    pub fn with_cap(max_findings: usize) -> Self {
        Self {
            regions: Vec::new(),
            findings: Vec::new(),
            depth: 0,
            max_findings,
            overflowed: 0,
            scroll: ScrollStack::new(),
            clipped: 0,
            subjects: 0,
        }
    }

    /// Returns the number of recognized regions, including those later clipped.
    pub fn subjects_examined(&self) -> usize {
        self.subjects
    }

    /// Findings collected so far.
    pub fn findings(&self) -> &[RegisteredOverlapFinding] {
        &self.findings
    }

    /// Consume the collector, returning findings.
    pub fn into_findings(self) -> Vec<RegisteredOverlapFinding> {
        self.findings
    }

    /// Consume the collector, returning the full result.
    pub fn into_result(self) -> RegisteredOverlapScanResult {
        RegisteredOverlapScanResult {
            findings: self.findings,
            overflowed: self.overflowed,
            clipped_out_of_view: self.clipped,
            subjects_examined: self.subjects,
        }
    }

    /// Returns true only after examining a region with no findings, overflow, or clipping.
    pub fn is_clean(&self) -> bool {
        self.subjects > 0 && self.findings.is_empty() && self.overflowed == 0 && self.clipped == 0
    }

    /// True when no registered region was examined at all
    pub fn examined_nothing(&self) -> bool {
        self.subjects == 0
    }

    /// How many findings were dropped after hitting the cap.
    pub fn overflowed(&self) -> usize {
        self.overflowed
    }

    /// How many regions were clipped out of view.
    pub fn clipped_out_of_view(&self) -> usize {
        self.clipped
    }

    fn push(&mut self, finding: RegisteredOverlapFinding) {
        if self.findings.len() >= self.max_findings {
            self.overflowed += 1;
            return;
        }
        self.findings.push(finding);
    }
}

impl Default for RegisteredOverlapScanCollector {
    fn default() -> Self {
        Self::new()
    }
}

fn rect_intersection(a: Rectangle, b: Rectangle) -> Option<Rectangle> {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let right = (a.x + a.width).min(b.x + b.width);
    let bottom = (a.y + a.height).min(b.y + b.height);
    if right > x && bottom > y {
        Some(Rectangle::new(
            [x, y].into(),
            [right - x, bottom - y].into(),
        ))
    } else {
        None
    }
}

impl<T> Operation<T> for RegisteredOverlapScanCollector {
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
        self.scroll.note_scrollable(id, bounds, translation);
    }

    fn custom(&mut self, id: Option<&Id>, _bounds: Rectangle, state: &mut dyn std::any::Any) {
        crate::diagnostics_host::NODE_VISITS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let Some(node) = state.downcast_ref::<RegisteredOverlapProbeNode>() else {
            return;
        };
        self.subjects += 1;
        let screen_bounds = self.scroll.translate(node.bounds);

        if !self.scroll.intersects_viewport(screen_bounds) {
            self.clipped += 1;
            return;
        }

        if !node.allow_overlap {
            let depth = self.depth;
            let new_findings: Vec<RegisteredOverlapFinding> = self
                .regions
                .iter()
                .filter(|existing| !existing.allow_overlap)
                .filter_map(|existing| {
                    rect_intersection(screen_bounds, existing.bounds).map(|intersection| {
                        RegisteredOverlapFinding {
                            id_a: existing.id.clone(),
                            id_b: id.cloned(),
                            bounds_a: existing.bounds,
                            bounds_b: screen_bounds,
                            intersection,
                            depth,
                        }
                    })
                })
                .collect();
            for finding in new_findings {
                self.push(finding);
            }
        }

        self.regions.push(RegisteredRegion {
            id: id.cloned(),
            bounds: screen_bounds,
            allow_overlap: node.allow_overlap,
        });
    }

    fn finish(&self) -> Outcome<T> {
        Outcome::None
    }
}

/// Adapter that returns overlap results through `Operation::finish`.
pub struct RegisteredOverlapScanOperation {
    collector: RegisteredOverlapScanCollector,
}

impl RegisteredOverlapScanOperation {
    /// Creates a reporting scan with the default finding cap.
    pub fn new() -> Self {
        Self {
            collector: RegisteredOverlapScanCollector::new(),
        }
    }

    /// Creates a reporting scan with an explicit cap; zero retains no findings but counts overflow.
    pub fn with_cap(max_findings: usize) -> Self {
        Self {
            collector: RegisteredOverlapScanCollector::with_cap(max_findings),
        }
    }
}

impl Default for RegisteredOverlapScanOperation {
    fn default() -> Self {
        Self::new()
    }
}

impl Operation<RegisteredOverlapScanResult> for RegisteredOverlapScanOperation {
    fn traverse(
        &mut self,
        operate: &mut dyn FnMut(&mut dyn Operation<RegisteredOverlapScanResult>),
    ) {
        Operation::traverse(&mut self.collector, operate);
    }

    fn container(&mut self, id: Option<&Id>, bounds: Rectangle, viewport: &Rectangle) {
        Operation::<RegisteredOverlapScanResult>::container(
            &mut self.collector,
            id,
            bounds,
            viewport,
        );
    }

    fn scrollable(
        &mut self,
        id: Option<&Id>,
        bounds: Rectangle,
        content_bounds: Size,
        translation: Vector,
        state: &mut dyn Scrollable,
    ) {
        Operation::<RegisteredOverlapScanResult>::scrollable(
            &mut self.collector,
            id,
            bounds,
            content_bounds,
            translation,
            state,
        );
    }

    fn custom(&mut self, id: Option<&Id>, bounds: Rectangle, state: &mut dyn std::any::Any) {
        Operation::<RegisteredOverlapScanResult>::custom(&mut self.collector, id, bounds, state);
    }

    fn finish(&self) -> Outcome<RegisteredOverlapScanResult> {
        Outcome::Some(RegisteredOverlapScanResult {
            findings: self.collector.findings().to_vec(),
            overflowed: self.collector.overflowed(),
            clipped_out_of_view: self.collector.clipped_out_of_view(),
            subjects_examined: self.collector.subjects_examined(),
        })
    }
}

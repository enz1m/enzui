//! Renderer-neutral collection of accessibility defects from live widget probes.
//!
//! Probe metadata, rather than magic geometry values, controls exclusions. A
//! visible zero-sized target remains a finding, and unresolved paint is reported
//! rather than treated as compliant.

use crate::scroll_stack::ScrollStack;
use iced_core::widget::Id;
use iced_core::widget::operation::scrollable::Scrollable;
use iced_core::widget::operation::{Operation, Outcome};
use iced_core::{Color, Rectangle, Size, Vector};

/// Interaction state whose resolved paint is being checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InteractionStatus {
    /// Resting appearance with no pointer or focus interaction.
    #[default]
    Resting,
    /// Pointer-hovered paint.
    Hovered,
    /// Pressed (pointer down / Space-Enter held) paint.
    Pressed,
    /// Keyboard-focused paint.
    Focused,
    /// Disabled or inert appearance.
    Disabled,
}

impl InteractionStatus {
    /// A short human label for reporters (`"hover"`, `"disabled"`, ...).
    pub fn label(self) -> &'static str {
        match self {
            InteractionStatus::Resting => "resting",
            InteractionStatus::Hovered => "hover",
            InteractionStatus::Pressed => "pressed",
            InteractionStatus::Focused => "focus",
            InteractionStatus::Disabled => "disabled",
        }
    }
}

/// What an [`AccessibilityProbeNode`] is checking.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AccessibilityCheck {
    /// Checks whether rendered text is shorter than its full content.
    ///
    /// Both lengths use the caller-selected unit, and `allow` suppresses intentional clamping.
    Elision {
        /// Full content length in the same caller-selected unit as `displayed_len`.
        full_len: u32,
        /// Amount of the full content that was rendered.
        displayed_len: u32,
        /// Whether intentional clamping is exempt from reporting.
        allow: bool,
    },
    /// Checks tap-target dimensions against a logical-pixel minimum unless exempted.
    HitTarget {
        /// Minimum edge length in logical pixels; either smaller axis fails.
        min_size: f32,
        /// Whether intentional undersizing is exempt from reporting.
        allow: bool,
        /// Whether the control is not presented and must be skipped.
        ///
        /// A visible zero-sized control is not hidden and remains a finding.
        hidden: bool,
    },
    /// Checks resolved foreground and background paint against a minimum contrast ratio.
    Contrast {
        /// Foreground color after theme and interaction-state resolution.
        foreground: Color,
        /// Resolved color painted behind the foreground.
        background: Color,
        /// Required contrast ratio, commonly 4.5 for normal text or 3.0 for large text.
        min_ratio: f32,
        /// Whether intentional low contrast is exempt from reporting.
        allow: bool,
        /// Whether the colors came from live resolved paint; unresolved paint is reported separately.
        resolved: bool,
        /// Interaction state for which these colors were resolved.
        status: InteractionStatus,
    },
    /// Checks an icon-only control that fell back to a glyph name instead of an explicit accessible name.
    MissingName {
        /// When `true`, the glyph-name fallback is intentional and must
        /// not be reported.
        allow: bool,
    },
}

/// Accessibility probe payload published through `Operation::custom`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AccessibilityProbeNode {
    /// Audited control bounds after layout.
    pub bounds: Rectangle,
    /// Which check to run and its resolved parameters.
    pub kind: AccessibilityCheck,
}

/// The type of accessibility defect found.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AccessibilityDefectKind {
    /// Reports truncated content and the positive amount hidden.
    TextElision {
        /// Positive hidden amount, computed as `full_len - displayed_len`.
        clamped_by: u32,
    },
    /// Reports an undersized target with its worst-axis deficit in logical pixels.
    UndersizedTarget {
        /// Positive shortfall of the smaller axis from the configured minimum.
        deficit: f32,
    },
    /// Reports an unrounded contrast ratio below its required threshold.
    LowContrast {
        /// Unrounded measured ratio, at least `1.0`.
        ratio: f32,
        /// Required ratio that the measurement failed to reach.
        min_ratio: f32,
        /// Interaction state in which the ratio was measured.
        status: InteractionStatus,
    },
    /// Reports that no resolved paint was available for comparison.
    UnresolvedPaint {
        /// Interaction state whose paint was unavailable.
        status: InteractionStatus,
    },
    /// Reports an icon-only control using a glyph-name fallback.
    MissingName,
}

/// One accessibility defect discovered during a tree walk.
#[derive(Debug, Clone)]
pub struct AccessibilityDefect {
    /// Stable identifier of the probe, when provided.
    pub id: Option<Id>,
    /// The audited control's bounds.
    pub bounds: Rectangle,
    /// The defect and its measured magnitude.
    pub kind: AccessibilityDefectKind,
    /// Nesting depth at collection time.
    pub depth: usize,
}

/// Result of one throttled accessibility scan, including omitted and examined counts.
#[derive(Debug, Clone)]
pub struct AccessibilityScanResult {
    /// Findings retained before the reporting cap was reached.
    pub findings: Vec<AccessibilityDefect>,
    /// Findings discovered but omitted after the cap was reached.
    pub overflowed: usize,
    /// Findings omitted because they did not intersect the combined viewport of their ancestor scrollables.
    pub clipped_out_of_view: usize,
    /// Number of registered accessibility subjects examined by the walk.
    pub subjects_examined: usize,
}

impl AccessibilityScanResult {
    /// Returns the total number of defects, including omitted and clipped findings.
    pub fn total(&self) -> usize {
        self.findings.len() + self.overflowed + self.clipped_out_of_view
    }

    /// Returns whether no defect was discovered, including clipped and overflowed defects.
    pub fn is_clean(&self) -> bool {
        self.total() == 0
    }

    /// Returns whether the walk examined no registered accessibility subject.
    pub fn examined_nothing(&self) -> bool {
        self.subjects_examined == 0
    }

    /// Returns whether findings were omitted by the cap or viewport clipping.
    pub fn is_truncated(&self) -> bool {
        self.overflowed > 0 || self.clipped_out_of_view > 0
    }
}

/// Hit-target check returning the worst-axis deficit for `(width, height, minimum, allow)`.
pub type HitTargetFn = dyn Fn(f32, f32, f32, bool) -> Option<f32> + Send + Sync;

/// Contrast check returning the measured ratio on failure for `(foreground, background, minimum, allow)`.
pub type ContrastFn = dyn Fn(Color, Color, f32, bool) -> Option<f32> + Send + Sync;

/// Elision check returning the hidden amount for `(full, displayed, allow)` in caller-selected units.
pub type ElisionFn = dyn Fn(u32, u32, bool) -> Option<u32> + Send + Sync;

/// Default reporting cap (throttle) for one tree walk.
pub const DEFAULT_MAX_FINDINGS: usize = 256;

/// Tree operation that records accessibility defects from failing probe nodes.
pub struct AccessibilityScanCollector<'f> {
    findings: Vec<AccessibilityDefect>,
    depth: usize,
    hit_target: &'f HitTargetFn,
    contrast: &'f ContrastFn,
    elision: &'f ElisionFn,
    max_findings: usize,
    overflowed: usize,
    // Translates content-space probe bounds and clips them through ancestor scrollables.
    scroll: ScrollStack,
    // Clipped defects still count against cleanliness even though no finding record is retained.
    clipped: usize,
    // Count recognized probes before filtering so coverage is independent of their verdict.
    subjects: usize,
}

impl<'f> AccessibilityScanCollector<'f> {
    /// Creates a collector using `DEFAULT_MAX_FINDINGS`.
    pub fn new(
        hit_target: &'f HitTargetFn,
        contrast: &'f ContrastFn,
        elision: &'f ElisionFn,
    ) -> Self {
        Self::with_cap(hit_target, contrast, elision, DEFAULT_MAX_FINDINGS)
    }

    /// Creates a collector retaining at most `max_findings`.
    ///
    /// A zero cap retains no details but counts every defect as overflow, so a
    /// discovery still makes the result non-clean.
    pub fn with_cap(
        hit_target: &'f HitTargetFn,
        contrast: &'f ContrastFn,
        elision: &'f ElisionFn,
        max_findings: usize,
    ) -> Self {
        Self {
            findings: Vec::new(),
            depth: 0,
            hit_target,
            contrast,
            elision,
            max_findings,
            overflowed: 0,
            scroll: ScrollStack::new(),
            clipped: 0,
            subjects: 0,
        }
    }

    /// Returns the number of recognized accessibility subjects examined.
    pub fn subjects_examined(&self) -> usize {
        self.subjects
    }

    /// Findings collected so far, in document order.
    pub fn findings(&self) -> &[AccessibilityDefect] {
        &self.findings
    }

    /// Consume the collector, returning its findings.
    pub fn into_findings(self) -> Vec<AccessibilityDefect> {
        self.findings
    }

    /// Returns whether no defect was discovered, including overflowed or clipped defects.
    pub fn is_clean(&self) -> bool {
        self.findings.is_empty() && self.overflowed == 0 && self.clipped == 0
    }

    /// How many findings were dropped after hitting `max_findings`.
    pub fn overflowed(&self) -> usize {
        self.overflowed
    }

    /// Returns the number of defects outside the combined viewport of their ancestor scrollables.
    pub fn clipped_out_of_view(&self) -> usize {
        self.clipped
    }

    fn push(&mut self, finding: AccessibilityDefect) {
        if self.findings.len() >= self.max_findings {
            self.overflowed += 1;
            return;
        }
        self.findings.push(finding);
    }
}

impl<T> Operation<T> for AccessibilityScanCollector<'_> {
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
        let Some(node) = state.downcast_ref::<AccessibilityProbeNode>() else {
            return;
        };
        self.subjects += 1;
        let depth = self.depth;
        // Keep check inputs in content space; use translated bounds only for reporting and viewport clipping.
        let screen_bounds = self.scroll.translate(node.bounds);
        match node.kind {
            AccessibilityCheck::Elision {
                full_len,
                displayed_len,
                allow,
            } => {
                if let Some(clamped_by) = (self.elision)(full_len, displayed_len, allow) {
                    if !self.scroll.intersects_viewport(screen_bounds) {
                        self.clipped += 1;
                        return;
                    }
                    self.push(AccessibilityDefect {
                        id: id.cloned(),
                        bounds: screen_bounds,
                        kind: AccessibilityDefectKind::TextElision { clamped_by },
                        depth: self.depth,
                    });
                }
            }
            AccessibilityCheck::HitTarget {
                min_size,
                allow,
                hidden,
            } => {
                if hidden {
                    return;
                }
                if let Some(deficit) =
                    (self.hit_target)(node.bounds.width, node.bounds.height, min_size, allow)
                {
                    if !self.scroll.intersects_viewport(screen_bounds) {
                        self.clipped += 1;
                        return;
                    }
                    self.push(AccessibilityDefect {
                        id: id.cloned(),
                        bounds: screen_bounds,
                        kind: AccessibilityDefectKind::UndersizedTarget { deficit },
                        depth,
                    });
                }
            }
            AccessibilityCheck::Contrast {
                foreground,
                background,
                min_ratio,
                allow,
                resolved,
                status,
            } => {
                if !resolved {
                    if !allow {
                        if !self.scroll.intersects_viewport(screen_bounds) {
                            self.clipped += 1;
                            return;
                        }
                        self.push(AccessibilityDefect {
                            id: id.cloned(),
                            bounds: screen_bounds,
                            kind: AccessibilityDefectKind::UnresolvedPaint { status },
                            depth,
                        });
                    }
                    return;
                }
                if let Some(ratio) = (self.contrast)(foreground, background, min_ratio, allow) {
                    if !self.scroll.intersects_viewport(screen_bounds) {
                        self.clipped += 1;
                        return;
                    }
                    self.push(AccessibilityDefect {
                        id: id.cloned(),
                        bounds: screen_bounds,
                        kind: AccessibilityDefectKind::LowContrast {
                            ratio,
                            min_ratio,
                            status,
                        },
                        depth,
                    });
                }
            }
            AccessibilityCheck::MissingName { allow } => {
                if allow {
                    return;
                }
                if !self.scroll.intersects_viewport(screen_bounds) {
                    self.clipped += 1;
                    return;
                }
                self.push(AccessibilityDefect {
                    id: id.cloned(),
                    bounds: screen_bounds,
                    kind: AccessibilityDefectKind::MissingName,
                    depth,
                });
            }
        }
    }

    fn finish(&self) -> Outcome<T> {
        Outcome::None
    }
}

/// Reporting operation that returns an `AccessibilityScanResult` through `Operation::finish`.
pub struct AccessibilityScanOperation<'f> {
    collector: AccessibilityScanCollector<'f>,
}

impl<'f> AccessibilityScanOperation<'f> {
    /// Creates a reporting scan using `DEFAULT_MAX_FINDINGS`.
    pub fn new(
        hit_target: &'f HitTargetFn,
        contrast: &'f ContrastFn,
        elision: &'f ElisionFn,
    ) -> Self {
        Self {
            collector: AccessibilityScanCollector::new(hit_target, contrast, elision),
        }
    }

    /// Creates a reporting scan retaining at most `max_findings`.
    pub fn with_cap(
        hit_target: &'f HitTargetFn,
        contrast: &'f ContrastFn,
        elision: &'f ElisionFn,
        max_findings: usize,
    ) -> Self {
        Self {
            collector: AccessibilityScanCollector::with_cap(
                hit_target,
                contrast,
                elision,
                max_findings,
            ),
        }
    }
}

impl Operation<AccessibilityScanResult> for AccessibilityScanOperation<'_> {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<AccessibilityScanResult>)) {
        // Traverse as the collector so its depth bookkeeping remains authoritative.
        Operation::traverse(&mut self.collector, operate);
    }

    fn container(&mut self, id: Option<&Id>, bounds: Rectangle, viewport: &Rectangle) {
        Operation::<AccessibilityScanResult>::container(&mut self.collector, id, bounds, viewport);
    }

    fn text(&mut self, id: Option<&Id>, bounds: Rectangle, text: &str) {
        Operation::<AccessibilityScanResult>::text(&mut self.collector, id, bounds, text);
    }

    fn scrollable(
        &mut self,
        id: Option<&Id>,
        bounds: Rectangle,
        content_bounds: Size,
        translation: Vector,
        state: &mut dyn Scrollable,
    ) {
        // Forward scrollable visits so the collector can translate and clip descendant probes.
        Operation::<AccessibilityScanResult>::scrollable(
            &mut self.collector,
            id,
            bounds,
            content_bounds,
            translation,
            state,
        );
    }

    fn custom(&mut self, id: Option<&Id>, bounds: Rectangle, state: &mut dyn std::any::Any) {
        Operation::<AccessibilityScanResult>::custom(&mut self.collector, id, bounds, state);
    }

    fn finish(&self) -> Outcome<AccessibilityScanResult> {
        // Preserve overflow, clipping, and coverage metadata across the operation boundary.
        Outcome::Some(AccessibilityScanResult {
            findings: self.collector.findings().to_vec(),
            overflowed: self.collector.overflowed(),
            clipped_out_of_view: self.collector.clipped_out_of_view(),
            subjects_examined: self.collector.subjects_examined(),
        })
    }
}

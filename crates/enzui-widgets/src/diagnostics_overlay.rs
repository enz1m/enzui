//! A paint-only diagnostics overlay.
//!
//! Wraps an inner widget and, when enabled, paints diagnostic rectangles on
//! top of it after the child has drawn. The overlay is topmost (drawn
//! last), paint-only (never affects layout, input, focus or semantics),
//! and zero-cost when disabled (the child is the only thing that exists).
//!
//! # The four inertness properties
//!
//! 1. Layout: the overlay's `layout()` delegates entirely to the child.
//!    The returned `layout::Node` is identical with or without the overlay.
//! 2. Input: `update()` delegates to the child with the cursor unchanged.
//!    The overlay never captures or modifies events.
//! 3. Focus: `operate()` delegates to the child unchanged. The overlay
//!    contributes no focusable state.
//! 4. Semantics: `operate()` delegates to the child unchanged. The
//!    overlay contributes no semantic nodes (no `custom` call, no `text`
//!    call of its own).
//!
//! Each property is tested by comparing a widget tree *with* the overlay
//! against the *same* tree without it, and asserting equality.
//!
//! # Precedent
//!
//! This crate's internal `InertClip` paint-only clip is the prior art for
//! paint-only layers. This overlay matches its shape (no state, full child
//! delegation) and its test discipline (headless renderer proof).

use iced::advanced::layout::Measure as _;
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Color, Element, Event, Length, Rectangle, Size, Vector};

/// Run the three cooperative audits over one already-laid-out widget tree.
///
/// This lives at the widget layer because the collectors need the real widget
/// implementations of the hit-target, contrast, elision, and spill policies.
/// Keeping the walk here also lets the framework-owned root wrappers drive it:
/// applications should not have to copy the probe's `Task::widget` plumbing
/// merely to make `ENZUI_DIAGNOSTICS=1` mean what it says.
struct TreeAuditResults {
    a11y: enzui_core::accessibility_scan::AccessibilityScanResult,
    bounds: enzui_core::layout_spill_scan::LayoutSpillScanResult,
    overlap: enzui_core::registered_overlap_scan::RegisteredOverlapScanResult,
}

fn tree_audit_results<Message, Theme, Renderer>(
    child: &mut Element<'_, Message, Theme, Renderer>,
    tree: &mut Tree,
    layout: Layout,
    renderer: &Renderer,
) -> TreeAuditResults
where
    Renderer: iced::advanced::Renderer,
{
    use enzui_core::accessibility_scan::{AccessibilityScanCollector, AccessibilityScanResult};
    use enzui_core::layout_spill_scan::LayoutSpillScanCollector;
    use enzui_core::registered_overlap_scan::RegisteredOverlapScanCollector;

    let mut a11y = AccessibilityScanCollector::new(
        &crate::hit_target_deficit,
        &crate::contrast_deficit,
        &crate::elision_clamp,
    );
    child
        .as_widget_mut()
        .operate(tree, layout, &layout.bounds(), renderer, &mut a11y);
    let a11y_result = AccessibilityScanResult {
        findings: a11y.findings().to_vec(),
        overflowed: a11y.overflowed(),
        clipped_out_of_view: a11y.clipped_out_of_view(),
        // Carry how many registered probes the walk actually saw, so
        // the report can tell "found nothing" from "looked at nothing".
        subjects_examined: a11y.subjects_examined(),
    };

    let mut bounds = LayoutSpillScanCollector::new(&crate::spill_amount);
    child
        .as_widget_mut()
        .operate(tree, layout, &layout.bounds(), renderer, &mut bounds);
    let bounds_result = bounds.into_result();

    let mut overlap = RegisteredOverlapScanCollector::new();
    child
        .as_widget_mut()
        .operate(tree, layout, &layout.bounds(), renderer, &mut overlap);
    let overlap_result = overlap.into_result();

    TreeAuditResults {
        a11y: a11y_result,
        bounds: bounds_result,
        overlap: overlap_result,
    }
}

fn report_tree_audits(
    sink: &enzui_core::diagnostics_sink::DiagnosticsSink,
    results: &TreeAuditResults,
) {
    sink.report_a11y(&results.a11y);
    sink.report_bounds(&results.bounds);
    sink.report_overlap(&results.overlap);
}

/// Emit exactly one complete three-report audit for this process when the
/// diagnostics environment contract is enabled.
///
/// Capture starts a fresh process for every frame, so process-once is
/// frame-once there. It also keeps ordinary relayouts from flooding a bounded
/// log buffer and pushing one of the three report kinds out of it.
pub(crate) fn emit_tree_audit_once<Message, Theme, Renderer>(
    child: &mut Element<'_, Message, Theme, Renderer>,
    tree: &mut Tree,
    layout: Layout,
    renderer: &Renderer,
) where
    Renderer: iced::advanced::Renderer,
{
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, Ordering};

    static ENABLED: OnceLock<bool> = OnceLock::new();
    static EMITTED: AtomicBool = AtomicBool::new(false);

    let enabled = *ENABLED.get_or_init(|| std::env::var("ENZUI_DIAGNOSTICS").as_deref() == Ok("1"));
    if !enabled || EMITTED.swap(true, Ordering::AcqRel) {
        return;
    }

    let sink = enzui_core::diagnostics_sink::DiagnosticsSink::to_stderr();
    let results = tree_audit_results(child, tree, layout, renderer);
    report_tree_audits(&sink, &results);
}

/// A diagnostic finding to paint: a rectangle with a colour.
#[derive(Debug, Clone, Copy)]
pub struct DiagnosticRect {
    /// The rectangle to highlight: screen-space for `findings`, or child-relative
    /// for `local_findings`.
    pub bounds: Rectangle,
    /// The colour of the highlight.
    pub color: Color,
    /// Line width of the outline. The overlay paints outlines, not fills,
    /// so it does not obscure the content it is diagnosing.
    pub stroke: f32,
}

/// A paint-only diagnostics overlay.
///
/// When `findings` is empty or `enabled` is false, this is a transparent
/// pass-through: the child is drawn, nothing is painted on top, and no
/// extra state exists.
#[must_use]
pub struct DiagnosticsOverlay<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    child: Element<'a, Message, Theme, Renderer>,
    findings: Vec<DiagnosticRect>,
    local: bool,
    enabled: bool,
}

impl<'a, Message, Theme, Renderer> DiagnosticsOverlay<'a, Message, Theme, Renderer> {
    /// Wrap `child` with a diagnostics overlay. Nothing is painted until
    /// `findings` is called with a non-empty list.
    pub fn new(child: impl Into<Element<'a, Message, Theme, Renderer>>) -> Self {
        Self {
            child: child.into(),
            findings: Vec::new(),
            local: false,
            enabled: false,
        }
    }

    /// Enable screen-space findings; rectangles may extend beyond the child.
    pub fn findings(mut self, rects: Vec<DiagnosticRect>) -> Self {
        self.enabled = !rects.is_empty();
        self.findings = rects;
        self.local = false;
        self
    }

    /// Enable findings whose bounds are relative to the child's layout origin.
    ///
    /// Resolves the origin during drawing so highlights follow placement and
    /// scrolling. Findings may extend beyond the child; use [`Self::findings`]
    /// for rectangles already expressed in screen coordinates.
    pub fn local_findings(mut self, rects: Vec<DiagnosticRect>) -> Self {
        self.enabled = !rects.is_empty();
        self.findings = rects;
        self.local = true;
        self
    }

    /// Enable or disable the overlay.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for DiagnosticsOverlay<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    // ---- Tree delegation (stateless wrapper) ----

    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        self.child.as_widget().tag()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        self.child.as_widget().state()
    }

    fn diff(&mut self, tree: &mut Tree) {
        self.child.as_widget_mut().diff(tree);
    }

    // ---- Layout: pure delegation, identical Node ----

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        let node = self.child.as_widget_mut().measure(tree, renderer, limits);
        emit_tree_audit_once(&mut self.child, tree, Layout::of(&node), renderer);
        node.apply(tree);
    }

    // ---- Operations: pure delegation, no extra nodes ----

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.child
            .as_widget_mut()
            .operate(tree, layout, viewport, renderer, operation);
    }

    // ---- Input: pure delegation, cursor unchanged ----

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.child
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    // ---- Draw: child first, then overlay on top ----

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.child
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);

        if self.enabled {
            // Findings may identify spill outside the child, so do not clip here.
            for rect in &self.findings {
                let mut rect = *rect;
                if self.local {
                    rect.bounds.x += layout.bounds().x;
                    rect.bounds.y += layout.bounds().y;
                }
                paint_outline(renderer, &rect);
            }
        }
    }

    // ---- Overlays: pure delegation ----

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, Theme, Renderer>> {
        self.child
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
}

impl<'a, Message, Theme, Renderer> From<DiagnosticsOverlay<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: 'a + iced::advanced::Renderer,
{
    fn from(value: DiagnosticsOverlay<'a, Message, Theme, Renderer>) -> Self {
        Element::new(value)
    }
}

/// Paint one diagnostic outline as four edge quads.
///
/// The stroke width is clamped to `min(width, height) / 2` so that the
/// left/right side heights never go negative (which would produce degenerate
/// quads). Non-finite or negative stroke values are treated as zero.
fn paint_outline<Renderer: iced::advanced::Renderer>(
    renderer: &mut Renderer,
    rect: &DiagnosticRect,
) {
    let b = rect.bounds;
    let color = rect.color;

    // Sanitise: NaN, negative and infinite strokes become 0.
    let raw = if rect.stroke.is_finite() && rect.stroke > 0.0 {
        rect.stroke
    } else {
        0.0
    };
    // Clamp so left/right height (`b.height - 2*s`) and top/bottom width
    // remain non-negative. At the limit the outline becomes a solid fill.
    let s = raw.min(b.width * 0.5).min(b.height * 0.5);

    if s <= 0.0 {
        return; // nothing visible to paint
    }

    // Top edge
    renderer.fill_quad(
        renderer::Quad {
            bounds: Rectangle {
                x: b.x,
                y: b.y,
                width: b.width,
                height: s,
            },
            ..Default::default()
        },
        color,
    );
    // Bottom edge
    renderer.fill_quad(
        renderer::Quad {
            bounds: Rectangle {
                x: b.x,
                y: b.y + b.height - s,
                width: b.width,
                height: s,
            },
            ..Default::default()
        },
        color,
    );

    let side_height = b.height - 2.0 * s;
    if side_height > 0.0 {
        // Left edge
        renderer.fill_quad(
            renderer::Quad {
                bounds: Rectangle {
                    x: b.x,
                    y: b.y + s,
                    width: s,
                    height: side_height,
                },
                ..Default::default()
            },
            color,
        );
        // Right edge
        renderer.fill_quad(
            renderer::Quad {
                bounds: Rectangle {
                    x: b.x + b.width - s,
                    y: b.y + s,
                    width: s,
                    height: side_height,
                },
                ..Default::default()
            },
            color,
        );
    }
    // When side_height == 0 (s == b.height/2), the top and bottom edges
    // meet and form a solid fill — which is the correct degenerate form.
}

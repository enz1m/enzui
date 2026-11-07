//! Thread-local shell build, reconciliation, focus, and motion counters.
//!
//! Read counters on the thread that performs the measured work. Counts wrap,
//! and [`reset`] clears counters, the transition edge, width, and isolation
//! override. Instrumentation remains active outside test builds so integration
//! tests and diagnostics observe the running implementation.

use std::cell::Cell;

thread_local! {
    static BUILD_CALLS: Cell<u64> = const { Cell::new(0) };
    static LAST_WIDTH_BITS: Cell<u32> = const { Cell::new(0x7fc0_0000) }; // qNaN
    static LAYOUT_SKIPS: Cell<u64> = const { Cell::new(0) };
    static MOTION_EDGE: Cell<Option<iced::time::Instant>> = const { Cell::new(None) };
    static FOCUS_PROBES: Cell<u64> = const { Cell::new(0) };
    static LAYOUT_INVALIDATIONS: Cell<u64> = const { Cell::new(0) };
    static ROOT_DIFFS: Cell<u64> = const { Cell::new(0) };
    static CONTENT_DIFFS: Cell<u64> = const { Cell::new(0) };
    static TOP_LEVEL_LAYOUTS: Cell<u64> = const { Cell::new(0) };
    static RESPONSIVE_ISLAND_LAYOUTS: Cell<u64> = const { Cell::new(0) };
    static RESPONSIVE_WIDTH_LAYOUTS: Cell<u64> = const { Cell::new(0) };
    static PANEL_CHILD_LAYOUTS: Cell<u64> = const { Cell::new(0) };
    static PANEL_LAYOUT_REPLAYS: Cell<u64> = const { Cell::new(0) };
    static ISOLATION_OVERRIDE: Cell<Option<bool>> = const { Cell::new(None) };
}

/// Record the transition edge on this thread for a later take-once read.
///
/// Anchor synthetic frame timestamps to this edge to exclude build and draw time.
#[inline]
pub fn note_motion_edge(at: iced::time::Instant) {
    MOTION_EDGE.with(|edge| edge.set(Some(at)));
}

/// Take the instant of the most recent sidebar transition
/// on this thread, clearing it so a later read cannot
/// silently reuse it.
#[must_use]
pub fn take_motion_edge() -> Option<iced::time::Instant> {
    MOTION_EDGE.with(|edge| edge.replace(None))
}

/// Record a shell construction and its assigned width on this thread.
#[inline]
pub fn note_build(available: f32) {
    LAST_WIDTH_BITS.with(|w| w.set(available.to_bits()));
    BUILD_CALLS.with(|c| c.set(c.get().wrapping_add(1)));
}

/// Note that `layout()` skipped the rebuild because the content
/// was still valid at this width.
#[inline]
pub fn note_layout_skip() {
    LAYOUT_SKIPS.with(|c| c.set(c.get().wrapping_add(1)));
}

/// Count a full content-tree focus probe on this thread.
#[inline]
pub fn note_focus_probe() {
    FOCUS_PROBES.with(|c| c.set(c.get().wrapping_add(1)));
}

/// Count a root layout invalidation requested by shell motion on this thread.
#[inline]
pub fn note_layout_invalidation() {
    LAYOUT_INVALIDATIONS.with(|c| c.set(c.get().wrapping_add(1)));
}

#[inline]
pub(crate) fn note_root_diff() {
    ROOT_DIFFS.with(|c| c.set(c.get().wrapping_add(1)));
}

#[inline]
pub(crate) fn note_content_diff() {
    CONTENT_DIFFS.with(|c| c.set(c.get().wrapping_add(1)));
}

#[inline]
pub(crate) fn note_top_level_layout() {
    TOP_LEVEL_LAYOUTS.with(|c| c.set(c.get().wrapping_add(1)));
}

#[inline]
pub(crate) fn note_responsive_island_layout() {
    RESPONSIVE_ISLAND_LAYOUTS.with(|c| c.set(c.get().wrapping_add(1)));
}

#[inline]
pub(crate) fn note_responsive_width_layout() {
    RESPONSIVE_WIDTH_LAYOUTS.with(|c| c.set(c.get().wrapping_add(1)));
}

#[inline]
pub(crate) fn note_panel_child_layout() {
    PANEL_CHILD_LAYOUTS.with(|c| c.set(c.get().wrapping_add(1)));
}

#[inline]
pub(crate) fn note_panel_layout_replay() {
    PANEL_LAYOUT_REPLAYS.with(|c| c.set(c.get().wrapping_add(1)));
}

/// Read this thread's `build` call count.
#[must_use]
pub fn snapshot() -> u64 {
    BUILD_CALLS.with(Cell::get)
}

/// Read this thread's layout-skip count.
#[must_use]
pub fn layout_skips() -> u64 {
    LAYOUT_SKIPS.with(Cell::get)
}

/// Read this thread's cumulative focus-probe walk count.
#[must_use]
pub fn focus_probes() -> u64 {
    FOCUS_PROBES.with(Cell::get)
}

/// Read this thread's cumulative moving-seam invalidation count.
#[must_use]
pub fn layout_invalidations() -> u64 {
    LAYOUT_INVALIDATIONS.with(Cell::get)
}

/// Root `Widget::diff` calls observed by the shell.
#[must_use]
pub fn root_diffs() -> u64 {
    ROOT_DIFFS.with(Cell::get)
}

/// Descendant-tree reconciliations performed by the shell.
#[must_use]
pub fn content_diffs() -> u64 {
    CONTENT_DIFFS.with(Cell::get)
}

/// Ordinary top-level `AppShellWidget::layout` passes.
#[must_use]
pub fn top_level_layouts() -> u64 {
    TOP_LEVEL_LAYOUTS.with(Cell::get)
}

/// Local retained-island layout passes performed during docked motion.
#[must_use]
pub fn responsive_island_layouts() -> u64 {
    RESPONSIVE_ISLAND_LAYOUTS.with(Cell::get)
}

/// Responsive-width layout calls below the shell's retained content boundary.
#[must_use]
pub fn responsive_width_layouts() -> u64 {
    RESPONSIVE_WIDTH_LAYOUTS.with(Cell::get)
}

/// Real sidebar-subtree layout calls below [`crate::motion::PanelReveal`].
#[must_use]
pub fn panel_child_layouts() -> u64 {
    PANEL_CHILD_LAYOUTS.with(Cell::get)
}

/// Cached sidebar-layout replays by [`crate::motion::PanelReveal`].
#[must_use]
pub fn panel_layout_replays() -> u64 {
    PANEL_LAYOUT_REPLAYS.with(Cell::get)
}

/// Assigned width of this thread’s latest shell construction; NaN before the
/// first construction or after [`reset`].
#[must_use]
pub fn last_width() -> f32 {
    f32::from_bits(LAST_WIDTH_BITS.with(Cell::get))
}

/// Clear this thread’s counters, transition edge, width, and isolation override.
///
/// Returns the previous build count. Scoped docked-layout observations are unaffected.
pub fn reset() -> u64 {
    LAST_WIDTH_BITS.with(|w| w.set(f32::NAN.to_bits()));
    LAYOUT_SKIPS.with(|c| c.set(0));
    MOTION_EDGE.with(|edge| edge.set(None));
    FOCUS_PROBES.with(|c| c.set(0));
    LAYOUT_INVALIDATIONS.with(|c| c.set(0));
    ROOT_DIFFS.with(|c| c.set(0));
    CONTENT_DIFFS.with(|c| c.set(0));
    TOP_LEVEL_LAYOUTS.with(|c| c.set(0));
    RESPONSIVE_ISLAND_LAYOUTS.with(|c| c.set(0));
    RESPONSIVE_WIDTH_LAYOUTS.with(|c| c.set(0));
    PANEL_CHILD_LAYOUTS.with(|c| c.set(0));
    PANEL_LAYOUT_REPLAYS.with(|c| c.set(0));
    ISOLATION_OVERRIDE.with(|c| c.set(None));
    BUILD_CALLS.with(|c| c.replace(0))
}

/// Override the process isolation setting on this thread.
///
/// Some(true) forces root invalidation for local docked motion; Some(false) enables
/// isolation. None uses the cached `ENZUI_DISABLE_RESPONSIVE_ISLAND` setting.
#[doc(hidden)]
pub fn set_isolation_override(value: Option<bool>) {
    ISOLATION_OVERRIDE.with(|c| c.set(value));
}

/// Current thread’s override; None delegates to the process isolation setting.
#[doc(hidden)]
pub fn isolation_override() -> Option<bool> {
    ISOLATION_OVERRIDE.with(Cell::get)
}

/// Builds the shell's real inset wrapper with a caller-owned progress cell.
///
/// `None` deliberately selects callback-only native layout; it must not gain owned admission.
#[doc(hidden)]
pub fn seam_inset<'a, Message: 'a>(
    child: impl Into<iced::Element<'a, Message>>,
    policy: iced::advanced::layout::inset::Policy,
    progress: std::rc::Rc<std::cell::Cell<f32>>,
    driver: Option<iced::advanced::renderer::NodeId>,
    trailing: bool,
) -> iced::Element<'a, Message> {
    let pad = if let Some(driver) = driver {
        super::seam::LiveSeamPad::driven(child, policy, progress, driver)
    } else {
        super::seam::LiveSeamPad::new(child, move || policy.amount(progress.get()))
    };
    pad.trailing_edge(trailing).into()
}

/// Wraps a layout-driven seam child using the same rider as the default shell.
#[doc(hidden)]
pub fn seam_rider<'a, Message: 'a>(
    child: impl Into<iced::Element<'a, Message>>,
    policy: iced::advanced::layout::inset::Policy,
    progress: std::rc::Rc<std::cell::Cell<f32>>,
    trailing: bool,
) -> iced::Element<'a, Message> {
    let laid_out = std::rc::Rc::clone(&progress);
    super::seam_rider::SeamRider::new(
        child,
        crate::motion::RevealClock {
            layout_driver: Some(iced::advanced::renderer::NodeId(900)),
            spatial: None,
            progress,
            travel: None,
            pin: None,
            responsive_reserve: true,
            retained_panel: None,
        },
        move || laid_out.get(),
        move |value| (if trailing { -1.0 } else { 1.0 }) * policy.amount(value),
    )
    .into()
}

/// Identity published by the actual shell-content layout, not inferred from lowering success.
#[derive(Debug, Clone, Copy, Default)]
pub struct DockedLayoutObservation {
    /// Number of shell-content layouts in this observation scope.
    pub captures: usize,
    /// Complete content’s presentation identity; None for an untyped root.
    pub node: Option<iced::advanced::renderer::NodeId>,
    /// Shell-owned driver shared by docked layout and its live seam.
    pub driver: Option<iced::advanced::renderer::NodeId>,
    /// Whether the docked sidebar was mounted in the latest observed layout.
    pub inline_mounted: bool,
    /// Native docked fraction in the latest layout, before renderer sampling.
    pub inline_progress: f32,
}

thread_local! {
    static DOCKED_LAYOUT: Cell<Option<DockedLayoutObservation>> = const { Cell::new(None) };
}

/// Observes shell-content identities during `layout`, restoring the prior scope even on unwind.
/// Unobserved production layouts retain no probe state.
pub fn observe_docked_layout<T>(layout: impl FnOnce() -> T) -> (T, DockedLayoutObservation) {
    struct Restore(Option<DockedLayoutObservation>);
    impl Drop for Restore {
        fn drop(&mut self) {
            DOCKED_LAYOUT.with(|slot| slot.set(self.0));
        }
    }
    let previous =
        DOCKED_LAYOUT.with(|slot| slot.replace(Some(DockedLayoutObservation::default())));
    let restore = Restore(previous);
    let result = layout();
    let observed = DOCKED_LAYOUT.with(Cell::get).unwrap_or_default();
    drop(restore);
    (result, observed)
}

pub(crate) fn note_docked_layout(
    content: &iced::advanced::widget::Tree,
    driver: iced::advanced::renderer::NodeId,
    inline_mounted: bool,
    inline_progress: f32,
) {
    DOCKED_LAYOUT.with(|slot| {
        if let Some(mut observed) = slot.get() {
            observed.captures = observed.captures.saturating_add(1);
            observed.node = content.presentation_node();
            observed.driver = Some(driver);
            observed.inline_mounted = inline_mounted;
            observed.inline_progress = inline_progress;
            slot.set(Some(observed));
        }
    });
}

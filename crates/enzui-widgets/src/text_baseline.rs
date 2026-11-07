//! Seat a text run on the painted baseline of a differently-weighted neighbour.
//!
//! # Purpose
//!
//! A row that mixes weights — a bold current segment beside regular ancestors,
//! a semibold value beside a regular label — does not share a baseline just
//! because every run is vertically centred. Centring aligns *line boxes*, and
//! the baseline inside a line box comes from the face's own ascent/descent, so
//! the heavier run rides one pixel lower or higher than its neighbours. Both
//! runs report identical bounds while doing it, which is why layout-rectangle
//! assertions cannot see the defect.
//!
//! [`seat_on_baseline_of`] removes the difference: the run keeps its weight,
//! size, colour and line-box height, and paints on the baseline the
//! `reference` font would occupy at the same size.
//!
//! # Hazards
//!
//! * The reference font must be measured at the same size and line box as
//!   the run; a different type rung is a different problem and this module
//!   will not fake it.
//! * The seat is width-transparent and height-pinned to the run's own line
//!   box, so a `Fill` run keeps absorbing its row's residual.
//! * Where both faces already share a baseline the run is returned untouched,
//!   so adopting this on a host whose weights agree changes no pixel.
//!
//! # Related
//!
//! * [`crate::enz_text`](fn@crate::enz_text) — the run this seats, and the source of its
//!   [`committed_font`](crate::text::EnzText::committed_font).
//! * [`crate::text_measure`] — width/height measurement through the same
//!   shaper.

use crate::text::EnzText;
use iced::advanced::layout::Measure as _;
use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::tree::{self, Tree};
use iced::advanced::{Widget, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size};

/// Distance from a run's line-box top to the baseline it paints on, in px.
///
/// Measured through the process-global font system the renderer paints with,
/// so it reports the face this host actually resolves for `font` — including
/// a weight that escapes to another family. Returns `line_height_px / 2.0`
/// when the host shapes no glyph at all.
///
/// `line_height_px` is the absolute line box, matching
/// [`EnzText::effective_line_height_px`](crate::text::EnzText::effective_line_height_px).
///
/// Results are memoized per exact `(font, size_px, line_height_px)` tuple
/// under the current backend font generation, bounded at
/// [`METRIC_CACHE_CAPACITY`] entries with LRU eviction (a size hit every
/// layout stays warm through a burst of one-shot sizes), and every prior
/// entry is retired on a `FontSystem::load_font` bump before the next hit.
/// Sizes are stored as exact `f32` bit patterns and are never quantized.
#[must_use]
pub fn baseline_offset_px(font: iced::Font, size_px: f32, line_height_px: f32) -> f32 {
    let size_px = crate::text::clamp_line_box(size_px);
    let line_height_px = crate::text::clamp_line_box(line_height_px);
    let key: MetricKey = (font, size_px.to_bits(), line_height_px.to_bits());

    let cache = metric_cache();
    let observed_generation = {
        // Advance-only under the cache lock: the observation the insert path
        // races against is the same generation the hit path uses to prune,
        // so an older reader cannot roll the tracked generation backward.
        let mut guard = cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.retire_if_stale(crate::revision_text::font_generation());
        if let Some(hit) = guard.get_and_promote(&key) {
            return hit;
        }
        guard.generation
    };

    METRIC_SHAPE_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let offset =
        shape_baseline_offset(font, size_px, line_height_px).unwrap_or(line_height_px / 2.0);

    #[cfg(any(test, feature = "internal-test-seams"))]
    __test_seams::wait_post_shape();

    let mut guard = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.try_seat(key, offset, observed_generation);
    offset
}

/// Test-only hooks the shape-race integration target uses to drive an
/// in-flight shape across a font-system mutation and to obtain an uncached
/// shape reference. Gated behind `cfg(test)` (lib unit tests) or the
/// `internal-test-seams` feature (integration); the module is not exposed
/// in a normal build, and no `pub` mutation entry point ships in one.
#[cfg(any(test, feature = "internal-test-seams"))]
pub mod __test_seams {
    use std::sync::mpsc::{Receiver, SyncSender};
    use std::sync::{Mutex, OnceLock};
    use std::time::Duration;

    // Only the next completed shape pauses; later lookups remain available.
    struct Gate {
        arrived: SyncSender<()>,
        release: Receiver<()>,
        release_timeout: Duration,
    }
    static POST_SHAPE_GATE: OnceLock<Mutex<Option<Gate>>> = OnceLock::new();

    fn gate() -> &'static Mutex<Option<Gate>> {
        POST_SHAPE_GATE.get_or_init(|| Mutex::new(None))
    }

    /// Installs the one-shot post-shape handshake. The waiter sends on
    /// `arrived` when it reaches the gate, then blocks on `release` for at
    /// most `release_timeout` before panicking.
    pub fn install_post_shape_gate(
        arrived: SyncSender<()>,
        release: Receiver<()>,
        release_timeout: Duration,
    ) {
        *gate()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Gate {
            arrived,
            release,
            release_timeout,
        });
    }

    #[allow(
        clippy::panic,
        reason = "a test gate: the release wait timing out is the failure it reports"
    )]
    pub(super) fn wait_post_shape() {
        let taken = gate()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(gate) = taken {
            // A closed `arrived` channel means the driver has hung up; the
            // eventual release wait will time out and surface the failure.
            let _ = gate.arrived.send(());
            if let Err(err) = gate.release.recv_timeout(gate.release_timeout) {
                panic!(
                    "post-shape release wait exceeded {:?}: {err:?}",
                    gate.release_timeout,
                );
            }
        }
    }

    /// Runs one shape through the process-global font system without
    /// consulting or mutating the metric cache; used as an independently-
    /// uncached reference in the race target.
    #[must_use]
    pub fn shape_uncached(font: iced::Font, size_px: f32, line_height_px: f32) -> Option<f32> {
        let size_px = crate::text::clamp_line_box(size_px);
        let line_height_px = crate::text::clamp_line_box(line_height_px);
        super::shape_baseline_offset(font, size_px, line_height_px)
    }
}

/// Maximum entries the metric cache holds within a single font generation.
///
/// Sized to cover a real app's typography working set with headroom for
/// live-resize probes; on overflow the least-recently-used entry leaves,
/// and on a font-generation change every entry leaves at once.
pub const METRIC_CACHE_CAPACITY: usize = 256;

type MetricKey = (iced::Font, u32, u32);
type Generation = iced::advanced::graphics::text::Version;

struct MetricCache<G: Copy + Eq + Ord> {
    generation: G,
    entries: std::collections::HashMap<MetricKey, f32>,
    // Front is LRU (evicted first); back is MRU.
    order: std::collections::VecDeque<MetricKey>,
}

impl<G: Copy + Eq + Ord> MetricCache<G> {
    fn new(generation: G) -> Self {
        Self {
            generation,
            entries: std::collections::HashMap::with_capacity(METRIC_CACHE_CAPACITY),
            order: std::collections::VecDeque::with_capacity(METRIC_CACHE_CAPACITY),
        }
    }

    // Monotonic: only advance. A caller whose observation predates the
    // tracked generation must not roll it back or clear entries a fresher
    // reader already inserted.
    fn retire_if_stale(&mut self, current: G) {
        if current > self.generation {
            self.entries.clear();
            self.order.clear();
            self.generation = current;
        }
    }

    fn get_and_promote(&mut self, key: &MetricKey) -> Option<f32> {
        let hit = *self.entries.get(key)?;
        promote_to_mru(&mut self.order, key);
        Some(hit)
    }

    fn insert(&mut self, key: MetricKey, offset: f32) {
        if self.entries.insert(key, offset).is_some() {
            promote_to_mru(&mut self.order, &key);
            return;
        }
        if self.entries.len() > METRIC_CACHE_CAPACITY
            && let Some(evict) = self.order.pop_front()
        {
            self.entries.remove(&evict);
        }
        self.order.push_back(key);
    }

    // Refuses inserts whose observation predates the tracked generation, so
    // a shape started against an obsolete generation cannot be seated in
    // the post-bump cache and answer a future lookup as fresh.
    fn try_seat(&mut self, key: MetricKey, offset: f32, observed_generation: G) -> bool {
        if self.generation == observed_generation {
            self.insert(key, offset);
            true
        } else {
            false
        }
    }
}

// Scan from the back because a hot key is at or near MRU; over a working set
// bounded by `METRIC_CACHE_CAPACITY` this bounds the promotion cost. The
// scan runs on every hit inside `baseline_offset_px`, which is a layout-time
// path.
fn promote_to_mru(order: &mut std::collections::VecDeque<MetricKey>, key: &MetricKey) {
    let Some(from_back) = order.iter().rev().position(|k| k == key) else {
        return;
    };
    let pos = order.len() - 1 - from_back;
    let k = order.remove(pos).expect("indexed position");
    order.push_back(k);
}

fn metric_cache() -> &'static std::sync::Mutex<MetricCache<Generation>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<MetricCache<Generation>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| {
        std::sync::Mutex::new(MetricCache::new(crate::revision_text::font_generation()))
    })
}

static METRIC_SHAPE_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Non-mutating retention gauge: entries under the currently tracked font
/// generation. `#[doc(hidden)]` test-observation seam; consumers should not
/// use it.
#[doc(hidden)]
#[must_use]
pub fn __metric_cache_len() -> usize {
    metric_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entries
        .len()
}

/// Non-mutating generation gauge: the backend generation the cache is
/// currently keyed against. `#[doc(hidden)]` test-observation seam.
#[doc(hidden)]
#[must_use]
pub fn __metric_cache_generation() -> iced::advanced::graphics::text::Version {
    metric_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .generation
}

/// Non-mutating shape-call counter: how many times the shaper ran since
/// process start. `#[doc(hidden)]` test-observation seam so no-reshape-on-hit
/// gates read a mechanism counter rather than inferring one.
#[doc(hidden)]
#[must_use]
pub fn __metric_shape_calls() -> usize {
    METRIC_SHAPE_CALLS.load(std::sync::atomic::Ordering::Relaxed)
}

/// The probe string. Latin letters with an ascender, an x-height body and a
/// descender, so the shaped line reports the face's full vertical extent
/// rather than the extent of one caller's particular string.
const PROBE_TEXT: &str = "Hxpg";

fn shape_baseline_offset(font: iced::Font, size_px: f32, line_height_px: f32) -> Option<f32> {
    use iced::advanced::graphics::text::cosmic_text::{Buffer, Metrics, Shaping};
    use iced::advanced::graphics::text::{font_system, to_attributes};

    // Every framework run resolves its family through this installer, so the
    // measurement must observe the same generic the renderer will.
    crate::sans_serif_fallback::ensure_sans_serif_fallback();

    let mut guard = font_system().write().ok()?;
    let system = guard.raw();
    let mut buffer = Buffer::new(system, Metrics::new(size_px, line_height_px));
    buffer.set_text(PROBE_TEXT, &to_attributes(font), Shaping::Advanced, None);
    buffer.shape_until_scroll(system, false);
    let run = buffer.layout_runs().next()?;
    run.glyphs.first()?;
    Some(run.line_y - run.line_top)
}

/// Seat `run` on the painted baseline that `reference` occupies at the run's
/// own size and line box.
///
/// The returned element keeps the run's width sizing and occupies exactly the
/// run's own line-box height, so it can replace the run anywhere without
/// moving its neighbours. When both faces already paint on the same baseline
/// — the common case of one family covering both weights — the run is
/// converted unchanged.
///
/// The correction is applied to the run's line box, which is the only input
/// the shaper's baseline placement responds to at sub-pixel precision: a
/// translated line box is quantised back to whole pixels by both renderer
/// backends. A correction that would need to distort the line box beyond half
/// its height is refused rather than applied approximately.
#[must_use]
pub fn seat_on_baseline_of<'a, Message: 'a>(
    run: EnzText,
    reference: iced::Font,
) -> Element<'a, Message> {
    let line_box = run.effective_line_height_px();
    let size_px = run.effective_size_px();
    let own = baseline_offset_px(run.committed_font(), size_px, line_box);
    let wanted = baseline_offset_px(reference, size_px, line_box);
    let drift = own - wanted;
    // `line_y` moves by half of a line-box change, so the compensation is
    // twice the drift.
    let compensated = line_box - 2.0 * drift;
    if drift == 0.0 || compensated < line_box / 2.0 || compensated > line_box * 1.5 {
        return run.into();
    }
    let seated: Element<'a, Message> = run
        .line_height(iced::widget::text::LineHeight::Absolute(compensated.into()))
        .into();
    Element::new(BaselineSeat {
        child: seated,
        line_box,
    })
}

/// Pins a compensated run back to the line box its neighbours occupy.
///
/// The compensation changes the run's intrinsic height, and a parent that
/// centres its children would spend half of that change undoing the
/// correction. This widget therefore reports the neighbours' line box as its
/// own height and seats the child at its top edge, leaving width entirely to
/// the child.
struct BaselineSeat<'a, Message> {
    child: Element<'a, Message>,
    line_box: f32,
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for BaselineSeat<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::stateless()
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: self.child.as_widget().size().width,
            height: Length::Fixed(self.line_box),
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let child = self.child.as_widget_mut().measure(
            &mut tree.children[0],
            renderer,
            &limits.height(Length::Shrink),
        );
        layout::Node::with_children(Size::new(child.size().width, self.line_box), vec![child])
            .apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        operation.container(None, layout.bounds(), viewport);
        operation.traverse(&mut |operation| {
            let child_layout = layout
                .children(&tree.children)
                .next()
                .expect("seated child");
            self.child.as_widget_mut().operate(
                &mut tree.children[0],
                child_layout,
                viewport,
                renderer,
                operation,
            );
        });
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: iced::advanced::mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let child_layout = layout
            .children(&tree.children)
            .next()
            .expect("seated child");
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            child_layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        crate::frame_routing::refresh_opaque_child(&self.child, tree, 0);
    }

    // The seat owns its child's tree node, so frame interest and frame
    // dispatch come from that child; without these the default interest keeps
    // every seated run on the frame route.
    fn frame_interest(&self, tree: &Tree) -> bool {
        crate::frame_routing::interest_opaque(tree)
    }

    fn update_frame(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: iced::advanced::mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        crate::frame_routing::forward_frame_opaque(
            &mut self.child,
            tree,
            0,
            event,
            layout
                .children(&tree.children)
                .next()
                .expect("seated child"),
            cursor,
            renderer,
            shell,
            viewport,
        );
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: iced::advanced::mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.child.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout
                .children(&tree.children)
                .next()
                .expect("seated child"),
            cursor,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: iced::advanced::mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> iced::advanced::mouse::Interaction {
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            layout
                .children(&tree.children)
                .next()
                .expect("seated child"),
            cursor,
            viewport,
            renderer,
        )
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: iced::Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        let child_layout = layout
            .children(&tree.children)
            .next()
            .expect("seated child");
        self.child.as_widget_mut().overlay(
            &mut tree.children[0],
            child_layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

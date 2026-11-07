//! Recursive tiled panes with live-theme splitters, keyboard resizing and semantics.
//!
//! Build with [`enz_pane_grid`] and pass its events to [`EnzPaneGridState::update`].
//! Ratios give the first child its share of the parent region. Recorded split
//! extents narrow keyboard and semantic ranges against pixel minimums, but
//! descendant minimums are not included in those announced ranges.
//!
//! Colours change immediately on hover and focus. [`enz_pane_grid_with_content`]
//! adds draggable headers and current pane allocations; [`EnzPaneGrid::on_drag`]
//! publishes keyed gestures for [`EnzPaneGridState::drop_pane`]. Multiple grids need distinct
//! [`EnzPaneGrid::id_base`] values to avoid splitter ID collisions.
//! For a sidebar that collapses into a drawer, use [`crate::navigation_split_view`].

use iced::advanced::layout::Measure as _;
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_WIDGET_EPOCH: AtomicUsize = AtomicUsize::new(0);

use enzui_core::collect::SemanticsNode;
use enzui_core::metrics::{OrderedRangeF32, UnitInterval};
use enzui_core::semantics::{NumericValue, Orientation as SemOrientation, Role, Semantics};
use enzui_theme::Palette;
use iced::advanced::Widget;
use iced::{Color, Element, Length, Rectangle, Size};

mod content;
pub use content::EnzPaneContent;

mod keyed;
pub use keyed::{PaneDragEvent, PaneDropTarget, PaneEdge, PaneKeyError, PaneRegion};

mod band;
mod rects;
mod seam;
mod semantics;
pub use semantics::{PaneGridSemantics, pane_grid_semantics};

pub use seam::{HANDLE_INDICATOR_LENGTH, HANDLE_INDICATOR_THICKNESS};
use seam::{collect_split_ids, collect_splits, hit_bounds, paint_handle_indicator};

/// Seam thickness (1 px).
pub const SEAM_THICKNESS: f32 = 1.0;

/// Extra grab room on each side in logical pixels: 4.5, giving a 10 px default band.
pub const HANDLE_SLOP: f32 = 4.5;

/// Total width of the grab target, seam included (10 px).
pub const HANDLE_TOTAL: f32 = SEAM_THICKNESS + 2.0 * HANDLE_SLOP;

/// Focus-ring corner radius (2 px).
pub const FOCUS_RING_RADIUS: f32 = 2.0;

/// Minimum pixel extent of either pane on either axis, handed to native
/// `PaneGrid::min_size`.
pub const MIN_PANE_PX: f32 = 120.0;

/// Smallest fraction of its parent a pane may occupy (0.05).
pub const MIN_RATIO: f32 = 0.05;

/// Suggested content minimum in logical pixels: 52.
/// Resolve it with [`min_ratio_for_content_px`]; it does not replace
/// [`PaneGridTokens::min_pane_px`] automatically.
pub const CONTENT_MIN_PANE_PX: f32 = 52.0;

/// Default arrow-key increment as a fraction of the split extent: 0.02.
pub const KEYBOARD_STEP: f32 = 0.02;

/// Whether a requested pixel minimum can fit on both sides of a seam.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ContentMinFit {
    /// The derived floor, reconciled with the caller's ratio-band floor.
    /// Its complement is the upper bound, subject to native rounding.
    Fits {
        /// The derived lower floor, reconciled with the supplied ratio band.
        min_ratio: f32,
    },
    /// The available extent cannot fit both pixel minimums and the seam.
    TooNarrow,
}

impl ContentMinFit {
    /// Return the derived floor, or `None` when the minimums cannot fit.
    #[must_use]
    pub fn min_ratio(self) -> Option<f32> {
        match self {
            ContentMinFit::Fits { min_ratio } => Some(min_ratio),
            ContentMinFit::TooNarrow => None,
        }
    }

    /// Whether both panes fit at `min_px`.
    #[must_use]
    pub fn fits(self) -> bool {
        matches!(self, ContentMinFit::Fits { .. })
    }
}

/// Resolve a symmetric content minimum into a ratio floor for an along-axis extent.
///
/// The floor is `max((min_px + seam / 2) / extent, repaired_min_ratio)`.
/// Negative or non-finite dimensions become zero; [`PaneGridTokens::ratio_range`]
/// repairs the ratio band. Returns [`ContentMinFit::TooNarrow`] below
/// `2 * min_px + seam`. At zero extent, a positive minimum is too narrow,
/// while a zero minimum returns the ratio-band floor even with a nonzero seam.
///
/// This helper does not change widget minimums or collapse a split; the caller
/// handles `TooNarrow`. Native layout rounding still applies to the result.
#[must_use]
pub fn min_ratio_for_content_px(
    min_px: f32,
    extent: f32,
    seam: f32,
    min_ratio: f32,
) -> ContentMinFit {
    use enzui_core::metrics::FiniteNonNeg;
    let min_px = FiniteNonNeg::new_or(min_px, 0.0).get();
    let extent = FiniteNonNeg::new_or(extent, 0.0).get();
    let seam = FiniteNonNeg::new_or(seam, 0.0).get();
    let ratio_band = PaneGridTokens {
        min_ratio,
        ..PaneGridTokens::default()
    }
    .ratio_range();
    let band_floor = ratio_band.lo();
    if extent <= 0.0 {
        return if min_px > 0.0 {
            ContentMinFit::TooNarrow
        } else {
            ContentMinFit::Fits {
                min_ratio: band_floor,
            }
        };
    }
    if extent < 2.0 * min_px + seam {
        return ContentMinFit::TooNarrow;
    }

    let floor_px = (min_px + seam * 0.5) / extent;
    let floor = floor_px.max(band_floor);
    ContentMinFit::Fits { min_ratio: floor }
}

/// Whether Control, Alt or Logo reserves a key press for the application.
/// Shift alone remains available for separator resizing and roving.
#[must_use]
pub fn is_application_chord(modifiers: iced::keyboard::Modifiers) -> bool {
    modifiers.control() || modifiers.alt() || modifiers.logo()
}

/// Direction of the separator line, following native iced axis names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Orientation {
    /// A vertical seam; panes sit side by side (left | right).
    Vertical,
    /// A horizontal seam; panes stack (top / bottom).
    Horizontal,
}

impl Orientation {
    /// The matching `iced::widget::pane_grid::Axis`.
    #[must_use]
    pub fn axis(self) -> iced::widget::pane_grid::Axis {
        match self {
            Orientation::Vertical => iced::widget::pane_grid::Axis::Vertical,
            Orientation::Horizontal => iced::widget::pane_grid::Axis::Horizontal,
        }
    }

    /// Accessibility orientation of the separator line, perpendicular to its movement.
    #[must_use]
    pub fn semantic(self) -> SemOrientation {
        match self {
            Orientation::Vertical => SemOrientation::Vertical,
            Orientation::Horizontal => SemOrientation::Horizontal,
        }
    }

    /// The two arrow keys that decrease / increase this split's ratio.
    #[must_use]
    pub fn arrow_keys(self) -> (iced::keyboard::key::Named, iced::keyboard::key::Named) {
        use iced::keyboard::key::Named;
        match self {
            Orientation::Vertical => (Named::ArrowLeft, Named::ArrowRight),
            Orientation::Horizontal => (Named::ArrowUp, Named::ArrowDown),
        }
    }

    /// Previous/next focus arrows, perpendicular to the pair from [`Self::arrow_keys`].
    #[must_use]
    pub fn cross_axis_arrow_keys(self) -> (iced::keyboard::key::Named, iced::keyboard::key::Named) {
        use iced::keyboard::key::Named;
        match self {
            Orientation::Vertical => (Named::ArrowUp, Named::ArrowDown),
            Orientation::Horizontal => (Named::ArrowLeft, Named::ArrowRight),
        }
    }
}

/// Seam / handle / focus tokens for `EnzPaneGrid`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaneGridTokens {
    /// Seam thickness in px.
    pub seam_thickness: f32,
    /// Seam colour at rest.
    pub seam: Color,
    /// Compact handle ink during dragging or visible keyboard focus; defaults to muted text.
    pub seam_active: Color,
    /// Compact handle ink on hover; defaults to the palette strong border.
    pub handle_ink: Color,
    /// Grab slop on each side of the seam.
    pub handle_slop: f32,
    /// Focus-ring colour.
    pub focus_ring: Color,
    /// Focus-ring thickness.
    pub focus_ring_width: f32,
    /// Focus-ring corner radius.
    pub focus_ring_radius: f32,
    /// Native pane minimum in logical pixels on both axes; defaults to 120.
    /// An undersized parent may still produce negative native dimensions;
    /// [`EnzPaneGridState::pane_rects`] clamps its returned rectangles.
    pub min_pane_px: f32,
    /// Smallest fraction of its parent a pane may occupy.
    pub min_ratio: f32,
    /// Arrow-key step as a fraction of the split extent.
    pub keyboard_step: f32,
}

impl PaneGridTokens {
    /// Resolve palette colours with the standard numeric defaults.
    #[must_use]
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            seam_thickness: SEAM_THICKNESS,
            seam: p.border,
            seam_active: p.text_muted,
            handle_ink: p.border_strong,
            handle_slop: HANDLE_SLOP,
            focus_ring: p.focus_ring,
            focus_ring_width: enzui_theme::text_defaults::focus_ring_width(),
            focus_ring_radius: FOCUS_RING_RADIUS,
            min_pane_px: MIN_PANE_PX,
            min_ratio: MIN_RATIO,
            keyboard_step: KEYBOARD_STEP,
        }
    }

    /// Dark preset.
    #[must_use]
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }

    /// Light preset.
    #[must_use]
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }

    /// The `leeway` to hand `PaneGrid::on_resize`, so that the grabbable band
    /// is `seam_thickness + 2 * handle_slop` wide overall.
    ///
    /// Native `PaneGrid` grabs over `spacing + leeway` centred on the line, and
    /// this `spacing` is the seam, so `leeway = 2 * handle_slop`.
    #[must_use]
    pub fn leeway(&self) -> f32 {
        2.0 * self.handle_slop.max(0.0)
    }

    /// The clamped ratio range implied by [`Self::min_ratio`].
    ///
    /// Repairs a nonsensical `min_ratio` (negative, NaN, or ≥ 0.5, which would
    /// invert the range) into `0.0..=0.49`, including non-finite input.
    #[must_use]
    pub fn ratio_range(&self) -> OrderedRangeF32 {
        let m = if self.min_ratio.is_finite() {
            self.min_ratio.clamp(0.0, 0.49)
        } else {
            MIN_RATIO
        };
        OrderedRangeF32::new(m, 1.0 - m).unwrap_or(
            const {
                OrderedRangeF32::new(MIN_RATIO, 1.0 - MIN_RATIO).expect("static bounds are ordered")
            },
        )
    }

    /// Clamp an arbitrary ratio into the legal range, repairing non-finite
    /// input to the midpoint rather than propagating NaN into layout.
    #[must_use]
    pub fn clamp_ratio(&self, ratio: f32) -> f32 {
        let range = self.ratio_range();
        if ratio.is_finite() {
            range.clamp(ratio)
        } else {
            range.lerp(UnitInterval::clamped(0.5))
        }
    }

    /// Resolve a content minimum using this token set's seam and ratio band.
    /// See [`min_ratio_for_content_px`] for normalization and fit rules.
    #[must_use]
    pub fn content_min_fit(&self, min_px: f32, extent: f32) -> ContentMinFit {
        min_ratio_for_content_px(min_px, extent, self.seam_thickness, self.min_ratio)
    }

    /// Repair invalid numeric geometry to the standard defaults, leaving colours unchanged.
    ///
    /// Zero is allowed except for `keyboard_step`, which must be finite and positive.
    /// `min_ratio` values at or above 0.5 are retained here and clamped later by
    /// [`Self::ratio_range`]. Non-finite colour components are not repaired.
    #[must_use]
    pub fn sanitized(&self) -> Self {
        use enzui_core::metrics::{FiniteNonNeg, FinitePositive};
        Self {
            seam_thickness: FiniteNonNeg::new_or(self.seam_thickness, SEAM_THICKNESS).get(),
            handle_slop: FiniteNonNeg::new_or(self.handle_slop, HANDLE_SLOP).get(),
            focus_ring_width: FiniteNonNeg::new_or(
                self.focus_ring_width,
                enzui_theme::text_defaults::focus_ring_width(),
            )
            .get(),
            focus_ring_radius: FiniteNonNeg::new_or(self.focus_ring_radius, FOCUS_RING_RADIUS)
                .get(),
            min_pane_px: FiniteNonNeg::new_or(self.min_pane_px, MIN_PANE_PX).get(),
            min_ratio: FiniteNonNeg::new_or(self.min_ratio, MIN_RATIO).get(),
            keyboard_step: FinitePositive::new_or(self.keyboard_step, KEYBOARD_STEP).get(),
            ..*self
        }
    }
}

impl Default for PaneGridTokens {
    fn default() -> Self {
        Self::dark()
    }
}

/// An indexed resize request shared by pointer and keyboard paths.
/// Apply it through [`SeparatorModel::apply`] or a native-state resize helper.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResizeRequest {
    /// Index of the separator within the [`SeparatorModel`].
    pub index: usize,
    /// Ratio supplied by the caller; model methods clamp it at their boundaries.
    pub ratio: f32,
}

/// One separator's description — everything the a11y tree and the keyboard handler
/// need, independent of rendering.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Separator {
    /// Seam direction.
    pub orientation: Orientation,
    /// Ratio supplied by the caller; model methods clamp it at their boundaries.
    pub ratio: f32,
    /// Native split identity when mirrored from a live tree; absent on detached models.
    pub split: Option<iced::widget::pane_grid::Split>,
}

impl Separator {
    /// A detached separator with no native `Split` behind it.
    #[must_use]
    pub fn new(orientation: Orientation, ratio: f32) -> Self {
        Self {
            orientation,
            ratio,
            split: None,
        }
    }
}

/// Owned separator ratios, focus and cached extents without rendering.
/// Mirror a matching native tree, then refresh extents at layout for pixel-aware ranges.
#[derive(Debug, Clone, PartialEq)]
pub struct SeparatorModel {
    separators: Vec<Separator>,
    tokens: PaneGridTokens,
    focused: Option<usize>,
    // Extents follow native split IDs across syncs between layout and keyboard events.
    extents: Vec<Cell<Option<f32>>>,
}

impl SeparatorModel {
    /// Build a model from an explicit separator list, clamping every ratio.
    #[must_use]
    pub fn new(separators: impl IntoIterator<Item = Separator>, tokens: PaneGridTokens) -> Self {
        let tokens = tokens.sanitized();
        let separators: Vec<Separator> = separators
            .into_iter()
            .map(|s| Separator {
                ratio: tokens.clamp_ratio(s.ratio),
                ..s
            })
            .collect();
        let extents = (0..separators.len()).map(|_| Cell::new(None)).collect();
        Self {
            separators,
            tokens,
            focused: None,
            extents,
        }
    }

    /// Mirror splits in depth-first order, visiting each parent before its children.
    /// First-child splits precede second-child splits; structural edits can change indices.
    #[must_use]
    pub fn from_node(node: &iced::widget::pane_grid::Node, tokens: PaneGridTokens) -> Self {
        let tokens = tokens.sanitized();
        let mut out = Vec::new();
        collect_splits(node, &mut out);
        Self::new(out, tokens)
    }

    /// Re-mirror the tree while reusing the separator and extent vector capacities.
    /// Cached extents follow native split IDs. Focus retains its ordinal if it
    /// still exists, even if that ordinal now describes a different split.
    /// This method still allocates a temporary map for cached extents.
    pub fn sync_from_node(&mut self, node: &iced::widget::pane_grid::Node) {
        // Preserve the last layout range until the next extent refresh.
        let old_extents: std::collections::BTreeMap<iced::widget::pane_grid::Split, f32> = self
            .separators
            .iter()
            .zip(self.extents.iter())
            .filter_map(|(s, e)| Some((s.split?, e.get()?)))
            .collect();
        self.separators.clear();
        collect_splits(node, &mut self.separators);
        let tokens = self.tokens;
        self.extents.clear();
        for s in &mut self.separators {
            s.ratio = tokens.clamp_ratio(s.ratio);
            let carried = s.split.and_then(|id| old_extents.get(&id).copied());
            self.extents.push(Cell::new(carried));
        }
        if let Some(f) = self.focused
            && f >= self.separators.len()
        {
            self.focused = None;
        }
    }

    /// Cache each split's along-axis extent from native regions at the supplied grid size.
    /// Missing native IDs clear their slots. Detached separators match by reading
    /// order; the model and node must describe the same tree.
    pub fn refresh_extents(&self, node: &iced::widget::pane_grid::Node, size: iced::Size) {
        let spacing = self.tokens.seam_thickness;
        let regions = node.split_regions(spacing, self.tokens.min_pane_px, size);
        let mut ordered_splits: Vec<iced::widget::pane_grid::Split> = Vec::new();
        collect_split_ids(node, &mut ordered_splits);
        for (i, sep) in self.separators.iter().enumerate() {
            let region = if let Some(id) = sep.split {
                regions.get(&id).map(|(axis, region, _)| (*axis, *region))
            } else {
                ordered_splits
                    .get(i)
                    .and_then(|id| regions.get(id).map(|(axis, region, _)| (*axis, *region)))
            };
            let extent = region.map(|(axis, r)| match axis {
                iced::widget::pane_grid::Axis::Vertical => r.width,
                iced::widget::pane_grid::Axis::Horizontal => r.height,
            });
            if let Some(cell) = self.extents.get(i) {
                cell.set(extent);
            }
        }
    }

    /// Return the ratio band intersected with immediate pixel minimums at the cached extent.
    ///
    /// Missing, non-finite or nonpositive extents use the ratio band. If the bands
    /// do not intersect, the result collapses to the native trailing-limit point,
    /// clamped into the ratio band. At very small sizes that point can differ from
    /// native layout. Nested descendant minimums are not included.
    /// Returns `None` for an out-of-range index.
    #[must_use]
    pub fn binding_ratio_range(&self, index: usize) -> Option<OrderedRangeF32> {
        if index >= self.separators.len() {
            return None;
        }
        Some(self.binding_ratio_range_at(index))
    }

    fn binding_ratio_range_at(&self, index: usize) -> OrderedRangeF32 {
        let ratio_range = self.tokens.ratio_range();
        let Some(extent) = self.extents.get(index).and_then(Cell::get) else {
            return ratio_range;
        };
        if !extent.is_finite() || extent <= 0.0 {
            return ratio_range;
        }
        let m_px = self.tokens.min_pane_px.max(0.0);
        let s = self.tokens.seam_thickness.max(0.0);
        let px_edge = (m_px + s * 0.5) / extent;
        let lo_px = px_edge;
        let hi_px = 1.0 - px_edge;
        let lo = ratio_range.lo().max(lo_px);
        let hi = ratio_range.hi().min(hi_px);
        if let Some(r) = OrderedRangeF32::new(lo, hi) {
            r
        } else {
            // Native Axis::split chooses the trailing limit when minimums cross.
            let point = hi_px.clamp(ratio_range.lo(), ratio_range.hi());
            OrderedRangeF32::new(point, point).unwrap_or(ratio_range)
        }
    }

    /// Number of separators.
    #[must_use]
    pub fn len(&self) -> usize {
        self.separators.len()
    }

    /// Whether there are no separators (a single-pane grid).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.separators.is_empty()
    }

    /// The separators, in focus order.
    #[must_use]
    pub fn separators(&self) -> &[Separator] {
        &self.separators
    }

    /// The tokens this model clamps with.
    #[must_use]
    pub fn tokens(&self) -> PaneGridTokens {
        self.tokens
    }

    /// Which separator currently holds focus, if any.
    #[must_use]
    pub fn focused(&self) -> Option<usize> {
        self.focused
    }

    /// Focus a separator by index. An out-of-range index clears focus
    /// rather than panicking — panes can disappear between the event and its
    /// delivery; stale keys are always ignored.
    pub fn focus(&mut self, index: usize) {
        self.focused = (index < self.separators.len()).then_some(index);
    }

    /// Drop focus.
    pub fn blur(&mut self) {
        self.focused = None;
    }

    /// Move focus to the next / previous separator, saturating at the ends
    /// (Tab order is owned by the app; this is the roving-arrow fallback).
    /// Returns the newly focused index, if any.
    pub fn focus_shift(&mut self, delta: i32) -> Option<usize> {
        if self.separators.is_empty() {
            self.focused = None;
            return None;
        }
        let last = self.separators.len() - 1;
        let next = match self.focused {
            None => {
                if delta >= 0 {
                    0
                } else {
                    last
                }
            }
            Some(cur) => {
                let raw = cur as i64 + i64::from(delta);
                raw.clamp(0, last as i64) as usize
            }
        };
        self.focused = Some(next);
        Some(next)
    }

    /// Read one separator, tolerating a stale index.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<Separator> {
        self.separators.get(index).copied()
    }

    /// Find the separator index mirroring a native `Split`.
    ///
    /// Returns `None` for a split that is no longer in the tree — the stale-key
    /// rule for the pointer path, since a `ResizeEvent` can outlive the pane it
    /// addressed.
    #[must_use]
    pub fn index_of(&self, split: iced::widget::pane_grid::Split) -> Option<usize> {
        self.separators.iter().position(|s| s.split == Some(split))
    }

    /// Convert a native resize event into a request for its mirrored split.
    /// Returns `None` for an unknown split.
    #[must_use]
    pub fn on_resize_event(
        &self,
        event: &iced::widget::pane_grid::ResizeEvent,
    ) -> Option<ResizeRequest> {
        let index = self.index_of(event.split)?;
        self.request(index, event.ratio)
    }

    /// Clamp a requested ratio to [`Self::binding_ratio_range`].
    /// Non-finite ratios become that range's midpoint; a stale index returns `None`.
    /// Creating a request does not change the model or native state.
    #[must_use]
    pub fn request(&self, index: usize, ratio: f32) -> Option<ResizeRequest> {
        if index >= self.separators.len() {
            return None;
        }
        let range = self.binding_ratio_range_at(index);
        let clamped = if ratio.is_finite() {
            range.clamp(ratio)
        } else {
            range.lerp(enzui_core::metrics::UnitInterval::clamped(0.5))
        };
        Some(ResizeRequest {
            index,
            ratio: clamped,
        })
    }

    /// Step a separator by `steps` keyboard increments (negative shrinks the
    /// first pane). Clamped, and `None` for a stale index.
    #[must_use]
    pub fn step(&self, index: usize, steps: i32) -> Option<ResizeRequest> {
        let sep = self.separators.get(index)?;
        let delta = self.tokens.keyboard_step * steps as f32;
        self.request(index, sep.ratio + delta)
    }

    /// Translate an unmodified key press on the focused separator into a resize
    /// request.
    ///
    /// This compatibility entry point represents a bare press. Event routes
    /// that have modifier information must use [`Self::on_key_with_modifiers`].
    #[must_use]
    pub fn on_key(&self, key: &iced::keyboard::Key) -> Option<ResizeRequest> {
        self.on_key_with_modifiers(key, iced::keyboard::Modifiers::empty())
    }

    /// Translate a key press and its modifiers into a resize request.
    ///
    /// Only the two arrows that match the seam's orientation are consumed, so
    /// the cross-axis arrows stay available for app navigation. Home/End jump
    /// to the extremes. Ctrl, Alt, and Logo mark application-owned chords and
    /// therefore return `None`; Shift remains separator-owned.
    #[must_use]
    pub fn on_key_with_modifiers(
        &self,
        key: &iced::keyboard::Key,
        modifiers: iced::keyboard::Modifiers,
    ) -> Option<ResizeRequest> {
        use iced::keyboard::key::Named;
        if is_application_chord(modifiers) {
            return None;
        }
        let index = self.focused?;
        let sep = self.separators.get(index)?;
        let iced::keyboard::Key::Named(named) = key else {
            return None;
        };
        let (dec, inc) = sep.orientation.arrow_keys();
        let range = self.binding_ratio_range_at(index);
        match *named {
            n if n == dec => self.step(index, -1),
            n if n == inc => self.step(index, 1),
            Named::Home => self.request(index, range.lo()),
            Named::End => self.request(index, range.hi()),
            _ => None,
        }
    }

    /// Decode focused cross-axis arrows as previous (`-1`) or next (`1`).
    /// Application chords, missing focus and unrelated keys return `None`.
    /// This method does not move focus itself.
    #[must_use]
    pub fn rove_from_key(
        &self,
        key: &iced::keyboard::Key,
        modifiers: iced::keyboard::Modifiers,
    ) -> Option<i32> {
        if is_application_chord(modifiers) {
            return None;
        }
        let index = self.focused?;
        let sep = self.separators.get(index)?;
        let iced::keyboard::Key::Named(named) = key else {
            return None;
        };
        let (prev, next) = sep.orientation.cross_axis_arrow_keys();
        match *named {
            n if n == prev => Some(-1),
            n if n == next => Some(1),
            _ => None,
        }
    }

    /// Update the mirrored ratio after clamping against the current binding range.
    /// Non-finite ratios become its midpoint; stale indices are ignored.
    /// This does not resize the native pane state.
    pub fn apply(&mut self, req: ResizeRequest) {
        if req.index >= self.separators.len() {
            return;
        }
        let range = self.binding_ratio_range_at(req.index);
        if let Some(sep) = self.separators.get_mut(req.index) {
            sep.ratio = if req.ratio.is_finite() {
                range.clamp(req.ratio)
            } else {
                range.lerp(enzui_core::metrics::UnitInterval::clamped(0.5))
            };
        }
    }

    /// Describe one adjustable splitter with its stored ratio, focus and binding range.
    /// Returns `None` for a stale index. The stored ratio is not reclamped when
    /// layout changes the cached range; it may lie outside that range until updated.
    #[must_use]
    pub fn semantics(&self, index: usize, label: Option<String>) -> Option<Semantics> {
        let sep = self.separators.get(index)?;
        let range = self.binding_ratio_range_at(index);
        Some(
            Semantics::new(Role::Splitter)
                .label_opt(label)
                .orientation(sep.orientation.semantic())
                .focused(self.focused == Some(index))
                .adjustable(true)
                .numeric(NumericValue {
                    value: f64::from(sep.ratio),
                    min: f64::from(range.lo()),
                    max: f64::from(range.hi()),
                    step: Some(f64::from(self.tokens.keyboard_step)),
                }),
        )
    }

    /// Mirror the tree, clamp the pointer request, and resize the native split.
    /// An unknown split is forwarded to native state with its raw ratio.
    pub fn pointer_resize<T>(
        &mut self,
        state: &mut iced::widget::pane_grid::State<T>,
        event: &iced::widget::pane_grid::ResizeEvent,
    ) {
        self.sync_from_node(state.layout());
        let ratio = match self.on_resize_event(event) {
            Some(req) => {
                self.apply(req);
                req.ratio
            }
            None => event.ratio,
        };
        state.resize(event.split, ratio);
    }

    /// Drive a native `State` from an unmodified key press on the focused
    /// separator.
    ///
    /// This compatibility entry point represents a bare press. Event routes
    /// that have modifier information must use
    /// [`Self::key_resize_with_modifiers`].
    pub fn key_resize<T>(
        &mut self,
        state: &mut iced::widget::pane_grid::State<T>,
        key: &iced::keyboard::Key,
    ) -> bool {
        self.key_resize_with_modifiers(state, key, iced::keyboard::Modifiers::empty())
    }

    /// Mirror the tree and apply a focused resize key to native state and model.
    /// Returns whether the key was consumed; application chords are rejected.
    /// Mirroring first resolves native IDs for initially detached separators.
    pub fn key_resize_with_modifiers<T>(
        &mut self,
        state: &mut iced::widget::pane_grid::State<T>,
        key: &iced::keyboard::Key,
        modifiers: iced::keyboard::Modifiers,
    ) -> bool {
        self.sync_from_node(state.layout());
        let Some(req) = self.on_key_with_modifiers(key, modifiers) else {
            return false;
        };
        if let Some(split) = self.get(req.index).and_then(|s| s.split) {
            state.resize(split, req.ratio);
        }
        self.apply(req);
        true
    }
}

fn collect_seam_rects(
    node: &iced::widget::pane_grid::Node,
    regions: &std::collections::BTreeMap<
        iced::widget::pane_grid::Split,
        (iced::widget::pane_grid::Axis, Rectangle, f32),
    >,
    spacing: f32,
    offset: iced::Point,
    out: &mut Vec<Rectangle>,
) {
    if let iced::widget::pane_grid::Node::Split { id, a, b, .. } = node {
        if let Some((axis, region, ratio)) = regions.get(id) {
            let line = axis.split_line_bounds(*region, *ratio, spacing);
            out.push(Rectangle {
                x: line.x + offset.x,
                y: line.y + offset.y,
                width: line.width,
                height: line.height,
            });
        }
        collect_seam_rects(a, regions, spacing, offset, out);
        collect_seam_rects(b, regions, spacing, offset, out);
    }
}

/// A two-pane separator model — the `EnzSplitView` case.
#[must_use]
pub fn two_pane(orientation: Orientation, ratio: f32) -> SeparatorModel {
    SeparatorModel::new(
        [Separator::new(orientation, ratio)],
        PaneGridTokens::default(),
    )
}

/// A three-pane separator model — the `EnzThreePaneView` case
/// (leading | content | inspector), as two vertical seams.
///
/// Both seams are keyboard-adjustable.
#[must_use]
pub fn three_pane(leading: f32, inspector: f32) -> SeparatorModel {
    SeparatorModel::new(
        [
            Separator::new(Orientation::Vertical, leading),
            Separator::new(Orientation::Vertical, inspector),
        ],
        PaneGridTokens::default(),
    )
}

/// Build the native `Configuration` for a two-pane split.
#[must_use]
pub fn two_pane_configuration<T>(
    orientation: Orientation,
    ratio: f32,
    first: T,
    second: T,
) -> iced::widget::pane_grid::Configuration<T> {
    iced::widget::pane_grid::Configuration::Split {
        axis: orientation.axis(),
        ratio: PaneGridTokens::default().clamp_ratio(ratio),
        a: Box::new(iced::widget::pane_grid::Configuration::Pane(first)),
        b: Box::new(iced::widget::pane_grid::Configuration::Pane(second)),
    }
}

/// Build the native `Configuration` for leading | content | inspector.
///
/// Nested as `leading | (content | inspector)` so the leading seam is the
/// outer split — that makes it separator index 0, matching reading order.
#[must_use]
pub fn three_pane_configuration<T>(
    leading_ratio: f32,
    inspector_ratio: f32,
    leading: T,
    content: T,
    inspector: T,
) -> iced::widget::pane_grid::Configuration<T> {
    use iced::widget::pane_grid::{Axis, Configuration};
    let t = PaneGridTokens::default();
    Configuration::Split {
        axis: Axis::Vertical,
        ratio: t.clamp_ratio(leading_ratio),
        a: Box::new(Configuration::Pane(leading)),
        b: Box::new(Configuration::Split {
            axis: Axis::Vertical,
            ratio: t.clamp_ratio(inspector_ratio),
            a: Box::new(Configuration::Pane(content)),
            b: Box::new(Configuration::Pane(inspector)),
        }),
    }
}

/// Construct native two-pane state and its mirrored separator model from one ratio.
/// The model carries native split IDs immediately; layout may still clamp
/// the configured ratio against pixel minimums.
#[must_use]
pub fn two_pane_state<T>(
    orientation: Orientation,
    ratio: f32,
    first: T,
    second: T,
) -> (iced::widget::pane_grid::State<T>, SeparatorModel) {
    let state = iced::widget::pane_grid::State::with_configuration(two_pane_configuration(
        orientation,
        ratio,
        first,
        second,
    ));
    let model = SeparatorModel::from_node(state.layout(), PaneGridTokens::default());
    (state, model)
}

/// Construct native `leading | (content | inspector)` state and its separator model.
/// `leading` is the outer first-child ratio; `inspector` assigns the inner
/// first child to content. Both ratios use the default ratio clamp independently.
/// Pixel minimums couple them when layout resolves their respective extents.
#[must_use]
pub fn three_pane_state<T>(
    leading: f32,
    inspector: f32,
    leading_pane: T,
    content: T,
    inspector_pane: T,
) -> (iced::widget::pane_grid::State<T>, SeparatorModel) {
    let state = iced::widget::pane_grid::State::with_configuration(three_pane_configuration(
        leading,
        inspector,
        leading_pane,
        content,
        inspector_pane,
    ));
    let model = SeparatorModel::from_node(state.layout(), PaneGridTokens::default());
    (state, model)
}

/// An app-owned recursive pane tree consumed by [`EnzPaneGridState`].
///
/// This keeps the native iced configuration type out of application code while
/// preserving arbitrary nesting. Ratios use the same leading/top convention as
/// [`Orientation`].
#[derive(Debug, Clone, PartialEq)]
pub enum PaneTree<T> {
    /// One leaf pane containing app state `T`.
    Pane(T),
    /// Two recursively nested regions separated by one adjustable splitter.
    Split {
        /// Direction of the splitter line.
        orientation: Orientation,
        /// Fraction assigned to `first` (left or top).
        ratio: f32,
        /// Left or top subtree.
        first: Box<PaneTree<T>>,
        /// Right or bottom subtree.
        second: Box<PaneTree<T>>,
    },
}

impl<T> PaneTree<T> {
    /// Construct one leaf pane.
    #[must_use]
    pub fn pane(value: T) -> Self {
        Self::Pane(value)
    }

    /// Split two subtrees recursively.
    #[must_use]
    pub fn split(
        orientation: Orientation,
        ratio: f32,
        first: PaneTree<T>,
        second: PaneTree<T>,
    ) -> Self {
        Self::Split {
            orientation,
            ratio,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    fn into_configuration(
        self,
        tokens: PaneGridTokens,
    ) -> iced::widget::pane_grid::Configuration<T> {
        match self {
            Self::Pane(value) => iced::widget::pane_grid::Configuration::Pane(value),
            Self::Split {
                orientation,
                ratio,
                first,
                second,
            } => iced::widget::pane_grid::Configuration::Split {
                axis: orientation.axis(),
                ratio: tokens.clamp_ratio(ratio),
                a: Box::new(first.into_configuration(tokens)),
                b: Box::new(second.into_configuration(tokens)),
            },
        }
    }
}

/// An interaction emitted by [`EnzPaneGrid`].
///
/// Feed it directly to [`EnzPaneGridState::update`] in the application's
/// update arm. Both pointer and keyboard resizing then pass through the same
/// framework clamp before changing the pane tree.
#[derive(Debug, Clone)]
pub enum EnzPaneGridEvent {
    /// Native pointer drag of a splitter. This updates layout immediately but
    /// is not a signal to reflow expensive pane content yet.
    PointerResize(iced::widget::pane_grid::ResizeEvent),
    /// The pointer-owned resize gesture ended. Expensive consumers should
    /// commit/reflow once on this boundary instead of once per move.
    PointerResizeFinished,
    /// A pointer press moved keyboard focus to this splitter.
    Focus(usize),
    /// A key was pressed while a splitter held focus.
    Key(iced::keyboard::Key),
}

/// App-owned native pane state and synchronized separator model.
#[derive(Debug, Clone)]
pub struct EnzPaneGridState<T> {
    panes: iced::widget::pane_grid::State<T>,
    separators: SeparatorModel,
    tokens: PaneGridTokens,
    pointer_resizing: Cell<bool>,
    focus_visible: Cell<bool>,
    widget_epoch: usize,
}

impl<T> EnzPaneGridState<T> {
    /// Build state for an arbitrary recursive pane tree with theme-derived
    /// colours and the standard geometry defaults.
    #[must_use]
    pub fn new(tree: PaneTree<T>) -> Self {
        Self::with_tokens(tree, PaneGridTokens::default())
    }

    /// Build state with explicit geometry tokens.
    ///
    /// The paint colours — resting seam, hovered handle ink, active ink and
    /// focus ring — still come from the live theme; the supplied numeric
    /// geometry is authoritative. Pin the resting seam with
    /// [`EnzPaneGrid::seam_color`].
    #[must_use]
    pub fn with_tokens(tree: PaneTree<T>, tokens: PaneGridTokens) -> Self {
        let tokens = tokens.sanitized();
        let panes =
            iced::widget::pane_grid::State::with_configuration(tree.into_configuration(tokens));
        let separators = SeparatorModel::from_node(panes.layout(), tokens);
        Self {
            panes,
            separators,
            tokens,
            pointer_resizing: Cell::new(false),
            focus_visible: Cell::new(false),
            widget_epoch: NEXT_WIDGET_EPOCH.fetch_add(1, Ordering::Relaxed),
        }
    }

    /// Replace all pane state and discard widget gestures, focus and cached extents.
    /// Use [`Self::update`] for pointer and keyboard resizing.
    pub fn replace_tree(&mut self, tree: PaneTree<T>) {
        self.widget_epoch = NEXT_WIDGET_EPOCH.fetch_add(1, Ordering::Relaxed);
        self.panes = iced::widget::pane_grid::State::with_configuration(
            tree.into_configuration(self.tokens),
        );
        self.separators = SeparatorModel::from_node(self.panes.layout(), self.tokens);
        self.pointer_resizing.set(false);
        self.focus_visible.set(false);
    }

    /// Apply an event emitted by [`enz_pane_grid`].
    pub fn update(&mut self, event: EnzPaneGridEvent) {
        match event {
            EnzPaneGridEvent::PointerResize(event) => {
                self.separators.pointer_resize(&mut self.panes, &event);
            }
            EnzPaneGridEvent::PointerResizeFinished => {
                self.pointer_resizing.set(false);
            }
            EnzPaneGridEvent::Focus(index) => {
                self.focus_visible.set(false);
                self.separators.focus(index);
            }
            EnzPaneGridEvent::Key(key) => {
                self.focus_visible.set(true);
                let _ = self.separators.key_resize(&mut self.panes, &key);
            }
        }
    }

    /// The separator currently focused for keyboard adjustment.
    #[must_use]
    pub fn focused_separator(&self) -> Option<usize> {
        self.separators.focused()
    }

    /// Whether a pointer-owned splitter gesture is still active.
    #[must_use]
    pub fn pointer_resize_active(&self) -> bool {
        self.pointer_resizing.get()
    }

    /// Whether the persistent separator ring is in keyboard-visible mode.
    #[must_use]
    pub fn separator_focus_visible(&self) -> bool {
        self.focus_visible.get() && self.separators.focused().is_some()
    }

    /// Read stored split ratios in depth-first order for persistence.
    /// These are requested ratios; native pixel constraints may place seams elsewhere.
    #[must_use]
    pub fn ratios(&self) -> Vec<f32> {
        self.separators
            .separators()
            .iter()
            .map(|separator| separator.ratio)
            .collect()
    }
}

type PaneView<'a, T, Message> =
    std::rc::Rc<dyn Fn(&'a T, bool, Option<Rectangle>) -> EnzPaneContent<'a, Message> + 'a>;

/// Builder combining recursive native layout, live-theme paint, resize events and splitter semantics.
pub struct EnzPaneGrid<'a, T, Message> {
    state: &'a EnzPaneGridState<T>,
    view: PaneView<'a, T, Message>,
    allocated_content: bool,
    on_activate: Option<std::rc::Rc<dyn Fn(iced::widget::pane_grid::Pane) -> Message + 'a>>,
    on_drag: Option<std::rc::Rc<dyn Fn(iced::widget::pane_grid::DragEvent) -> Message + 'a>>,
    on_event: std::rc::Rc<dyn Fn(EnzPaneGridEvent) -> Message + 'a>,
    labels: Vec<String>,
    id_base: String,
    seam_color: Option<Color>,
}

impl<'a, T, Message> EnzPaneGrid<'a, T, Message> {
    /// Emit the application key on a pane press, including presses captured by its children.
    /// Child selection and control callbacks remain active; splitter presses do not activate panes.
    #[allow(
        clippy::missing_panics_doc,
        reason = "panics only on a broken invariant: the grid activates only panes it holds \
                  state for"
    )]
    #[must_use]
    pub fn on_activate(mut self, callback: impl Fn(T) -> Message + 'a) -> Self
    where
        T: Clone + 'a,
        Message: 'a,
    {
        let state = self.state;
        self.on_activate = Some(std::rc::Rc::new(move |pane| {
            callback(
                state
                    .panes
                    .get(pane)
                    .expect("activated pane has state")
                    .clone(),
            )
        }));
        self
    }

    /// Emit keyed header gestures and paint the live-theme drop region.
    /// The tree changes only when the host applies a dropped event with
    /// [`EnzPaneGridState::drop_pane`]. Pointer previews track directly without tweening.
    #[must_use]
    pub fn on_drag(mut self, callback: impl Fn(PaneDragEvent<T>) -> Message + 'a) -> Self
    where
        T: Clone + 'a,
        Message: 'a,
    {
        let state = self.state;
        self.on_drag = Some(std::rc::Rc::new(move |event| {
            callback(state.drag_event(event))
        }));
        self
    }

    /// Set splitter names in depth-first order.
    /// Missing labels default to `Pane splitter N`; excess labels are unused.
    #[must_use]
    pub fn labels(mut self, labels: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.labels = labels.into_iter().map(Into::into).collect();
        self
    }

    /// Set the splitter ID prefix; defaults to `enz-pane-grid`.
    /// Use a distinct prefix for each grid in one view.
    #[must_use]
    pub fn id_base(mut self, id: impl Into<String>) -> Self {
        self.id_base = id.into();
        self
    }

    /// Override the resting gap/seam colour while retaining live-theme hover,
    /// focus, and picked-divider colours. Useful when pane cards should reveal
    /// their containing shell background between them.
    #[must_use]
    pub fn seam_color(mut self, color: Color) -> Self {
        self.seam_color = Some(color);
        self
    }

    /// Convert this builder into an iced element.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message>
    where
        T: 'a,
        Message: 'a,
    {
        self.into()
    }
}

/// Build a recursive resizable pane tree.
/// Pass emitted events to [`EnzPaneGridState::update`]. The content callback
/// receives each leaf value and the native maximized flag.
#[must_use]
pub fn enz_pane_grid<'a, T, Message>(
    state: &'a EnzPaneGridState<T>,
    view: impl Fn(&'a T, bool) -> Element<'a, Message> + 'a,
    on_event: impl Fn(EnzPaneGridEvent) -> Message + 'a,
) -> EnzPaneGrid<'a, T, Message>
where
    Message: 'a,
{
    EnzPaneGrid {
        state,
        view: std::rc::Rc::new(move |value, maximized, _| {
            EnzPaneContent::new(view(value, maximized))
        }),
        allocated_content: false,
        on_activate: None,
        on_drag: None,
        on_event: std::rc::Rc::new(on_event),
        labels: Vec::new(),
        id_base: "enz-pane-grid".to_owned(),
        seam_color: None,
    }
}
/// Build panes with draggable decorative headers and their current native allocations.
/// The callback runs during layout and receives grid-local bounds, including spacing
/// and native minimum-size rounding. Undersized grids may allocate outside their bounds.
/// Headers reserve controls automatically; the body receives the remaining height.
#[allow(
    clippy::missing_panics_doc,
    reason = "panics only on a broken invariant: an allocated pane view has bounds"
)]
#[must_use]
pub fn enz_pane_grid_with_content<'a, T, Message>(
    state: &'a EnzPaneGridState<T>,
    view: impl Fn(&'a T, bool, Rectangle) -> EnzPaneContent<'a, Message> + 'a,
    on_event: impl Fn(EnzPaneGridEvent) -> Message + 'a,
) -> EnzPaneGrid<'a, T, Message>
where
    Message: 'a,
{
    EnzPaneGrid {
        state,
        view: std::rc::Rc::new(move |value, maximized, bounds| {
            view(
                value,
                maximized,
                bounds.expect("allocated pane view has bounds"),
            )
        }),
        allocated_content: true,
        on_activate: None,
        on_drag: None,
        on_event: std::rc::Rc::new(on_event),
        labels: Vec::new(),
        id_base: "enz-pane-grid".to_owned(),
        seam_color: None,
    }
}

// Assign every field explicitly so added tokens must choose live paint or owned geometry.
fn theme_tokens(theme: &iced::Theme, geometry: PaneGridTokens) -> PaneGridTokens {
    let live = PaneGridTokens::from_palette(&crate::theme_lookup::palette(theme));
    PaneGridTokens {
        seam: live.seam,
        seam_active: live.seam_active,
        handle_ink: live.handle_ink,
        focus_ring: live.focus_ring,
        seam_thickness: geometry.seam_thickness,
        handle_slop: geometry.handle_slop,
        focus_ring_width: geometry.focus_ring_width,
        focus_ring_radius: geometry.focus_ring_radius,
        min_pane_px: geometry.min_pane_px,
        min_ratio: geometry.min_ratio,
        keyboard_step: geometry.keyboard_step,
    }
}

impl<'a, T: 'a, Message: 'a> From<EnzPaneGrid<'a, T, Message>> for Element<'a, Message> {
    fn from(builder: EnzPaneGrid<'a, T, Message>) -> Self {
        if builder.allocated_content {
            iced::widget::Responsive::new(move |size| builder.build(Some(size))).into()
        } else {
            builder.build(None)
        }
    }
}

impl<'a, T: 'a, Message: 'a> EnzPaneGrid<'a, T, Message> {
    fn build(&self, size: Option<Size>) -> Element<'a, Message> {
        let state = self.state;
        let view = std::rc::Rc::clone(&self.view);
        let on_event = std::rc::Rc::clone(&self.on_event);
        let labels = self.labels.clone();
        let id_base = self.id_base.clone();
        let seam_color = self.seam_color;
        let regions = size.map(|size| {
            state.panes.layout().pane_regions(
                state.tokens.seam_thickness,
                state.tokens.min_pane_px,
                size,
            )
        });
        let geometry = state.tokens;
        // Which pane edges carry a splitter is a matter of the tree's shape,
        // so a nominal size answers it before the grid is laid out.
        let nominal = Size::new(1.0e5, 1.0e5);
        let interiors = state.panes.layout().pane_regions(
            geometry.seam_thickness,
            geometry.min_pane_px,
            nominal,
        );
        let resize = std::rc::Rc::clone(&on_event);
        let mut grid = iced::widget::PaneGrid::new(&state.panes, move |pane, value, maximized| {
            let bounds = if maximized {
                size.map(Rectangle::with_size)
            } else {
                regions
                    .as_ref()
                    .and_then(|regions| regions.get(&pane).copied())
            };
            let interior = if maximized {
                band::Interior::default()
            } else {
                interiors
                    .get(&pane)
                    .map(|region| band::Interior::of(*region, nominal))
                    .unwrap_or_default()
            };
            view(value, maximized, bounds)
                .map_body(|body| {
                    band::SplitterBand::new(body, interior, geometry.leeway() / 2.0).into()
                })
                .native()
        })
        .spacing(geometry.seam_thickness)
        .min_size(geometry.min_pane_px)
        .width(Length::Fill)
        .height(Length::Fill)
        .on_resize(geometry.leeway(), move |event| {
            resize(EnzPaneGridEvent::PointerResize(event))
        })
        .style({
            let dragging = self.on_drag.is_some();
            move |theme: &iced::Theme| {
                let mut style = style_from_tokens(theme_tokens(theme, geometry));
                if dragging {
                    let p = crate::theme_lookup::palette(theme);
                    style.hovered_region.background = Color {
                        a: 0.18,
                        ..p.accent
                    }
                    .into();
                    style.hovered_region.border.color = p.accent;
                }
                style
            }
        });
        if let Some(callback) = self.on_activate.clone() {
            grid = grid.on_click(move |pane| callback(pane));
        }
        if let Some(callback) = self.on_drag.clone() {
            grid = grid.on_drag(move |event| callback(event));
        }
        let grid = crate::keyed_row::keyed_column(vec![(
            state.widget_epoch.to_string(),
            Element::from(grid),
        )])
        .width(Length::Fill)
        .height(Length::Fill);
        // Native spacing exposes this backdrop as the resting seam.
        let backed = iced::widget::container(grid)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |theme: &iced::Theme| iced::widget::container::Style {
                background: Some(
                    seam_color
                        .unwrap_or_else(|| theme_tokens(theme, geometry).seam)
                        .into(),
                ),
                ..Default::default()
            });

        let mut labels = labels.into_iter().map(Some).collect::<Vec<_>>();
        let provided = labels.len();
        labels.extend((provided..state.separators.len()).map(|index| {
            Some(enzui_core::localized!("enzui-pane-grid-pane-splitter", "splitter" => index + 1))
        }));
        let focus = std::rc::Rc::clone(&on_event);
        let key = std::rc::Rc::clone(&on_event);
        PaneGridSemantics::new(backed, &state.separators, state.panes.layout())
            .id_base(id_base)
            .labels(labels)
            .on_focus(move |index| focus(EnzPaneGridEvent::Focus(index)))
            .on_resize_finished(&state.pointer_resizing, &state.focus_visible, move || {
                on_event(EnzPaneGridEvent::PointerResizeFinished)
            })
            .on_key(move |pressed| key(EnzPaneGridEvent::Key(pressed)))
            .tokens_resolved(move |theme| theme_tokens(theme, geometry))
            .into()
    }
}

/// Disable native splitter lines and drop-target paint with sanitized border geometry.
/// [`PaneGridSemantics`] paints the compact solid handle and focus ring.
/// The resting seam needs a background behind native spacing gaps.
#[must_use]
pub fn style_from_tokens(t: PaneGridTokens) -> iced::widget::pane_grid::Style {
    use iced::widget::pane_grid::{Highlight, Line, Style};
    let t = t.sanitized();
    Style {
        hovered_region: Highlight {
            background: iced::Background::Color(Color::TRANSPARENT),
            border: iced::Border {
                width: t.focus_ring_width,
                color: Color::TRANSPARENT,
                radius: t.focus_ring_radius.into(),
            },
        },
        picked_split: Line {
            color: Color::TRANSPARENT,
            width: 0.0,
        },
        hovered_split: Line {
            color: Color::TRANSPARENT,
            width: 0.0,
        },
    }
}

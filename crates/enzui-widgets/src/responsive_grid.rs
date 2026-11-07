//! EnzResponsiveGrid: own-width responsive grid with size-class column ramp.
//!
//! A self-measuring grid that reads its own laid-out width, classifies it
//! into a size class, picks a per-tier column count (wide/medium/narrow/compact
//! ramp), computes an even cell width, and wraps its children into rows with
//! tokenised gaps and per-edge insets.
//!
//! # Native primitive over fixed-width item alias
//!
//! [`crate::responsive_grid_item`] provides only a
//! fixed-`Container` cell and requires the app to compute the "responsive"
//! width from a resize handler. iced already supplies the missing piece:
//! `iced::widget::responsive` hands the closure the component's own
//! available size during layout, so the grid reflows without any global
//! window-resize plumbing. This module builds the real parent on top of that
//! primitive and reuses [`crate::grid::enz_grid`] for the equal-cell row math.
//!
//! # Size classes (canonical breakpoints)
//!
//! The canonical viewport size classes (the theme breakpoint tokens reuse the
//! same values):
//!
//! | class    | width range        | column knob        | default |
//! |----------|--------------------|--------------------|---------|
//! | Compact  | `< 480`            | `columns_compact`  | 1 |
//! | Regular  | `< 720`            | `columns_narrow`   | 2 |
//! | Medium   | `< 1024`           | `columns_medium`   | 3 |
//! | Expanded | `< 1440`           | `columns_wide`     | 4 |
//! | Large    | `>= 1440`          | `columns_wide`     | 4 |
//!
//! Expanded and Large share `columns_wide`: the column ramp tops out at the
//! wide tier. The active column count is
//! clamped to `>= 1`, so a caller that (mis)sets a knob to 0 degenerates to a
//! single column instead of dividing the cell width by zero.
//!
//! # Rebuild-on-reflow: children are builder closures
//!
//! iced's `responsive` closure is called on every layout pass (see
//! `iced/widget/src/responsive.rs`), and `Element` is not `Clone`, so
//! the children must be *regenerated* each pass. Callers therefore pass a
//! `Vec` of builder closures (`impl Fn() -> Element`), one per cell, rather
//! than owned elements. This is the idiomatic iced responsive pattern and is
//! what makes live reflow (not a rebuild-only snapshot) work.
//!
//! # Height
//!
//! Natural (content) height by default (`fill == false`). `.fill(true)` claims
//! the parent's height for panes that own their vertical space.

use crate::enz_responsive::enz_responsive;
use crate::grid::{enz_grid, enz_keyed_grid};
use crate::surface_stack::SpaceToken;
use enzui_core::identity::KeyedIds;
use enzui_theme::{BreakpointScale, SpacingScale};
use iced::{Element, Length};

// ---------------------------------------------------------------------------
// Size classes (single classifier in the theme crate)
// ---------------------------------------------------------------------------

/// Re-export of the framework's single size-class type.
///
/// This crate must not carry its own `EnzSizeClass` and
/// `class_for_width` over the same 360/480/720/1024/1440 thresholds as the
/// theme crate's `BreakpointScale::classify`, because a duplicate classifier
/// could name its bands by opposite edges and return answers offset by one
/// for the same width (400 could be `Compact` by one, `Sm` by the other).
/// One classifier: this type is [`enzui_theme::EnzSizeClass`] and
/// [`class_for_width`] delegates to [`BreakpointScale::classify`].
pub use enzui_theme::EnzSizeClass;

/// Classify a width in logical pixels into an [`EnzSizeClass`].
///
/// Delegates to [`BreakpointScale::classify`] on the default scale — the single
/// classifier. A non-finite or non-positive width (`<= 0.0`) is
/// treated as the widest tier (`Large`) — the "assume widest before the real
/// width is known" guard that avoids a flash of the collapsed layout.
pub fn class_for_width(width: f32) -> EnzSizeClass {
    BreakpointScale::default().classify(width)
}

// ---------------------------------------------------------------------------
// Column configuration
// ---------------------------------------------------------------------------

/// Per-size-class target column counts (desktop → phone ramp). Defaults match
/// (`columns-wide: 4`, `columns-medium: 3`, `columns-narrow: 2`,
/// `columns-compact: 1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColumnRamp {
    /// Expanded + Large tier.
    pub wide: usize,
    /// Medium tier.
    pub medium: usize,
    /// Regular tier.
    pub narrow: usize,
    /// Compact tier.
    pub compact: usize,
}

impl Default for ColumnRamp {
    fn default() -> Self {
        Self {
            wide: 4,
            medium: 3,
            narrow: 2,
            compact: 1,
        }
    }
}

impl ColumnRamp {
    /// Active (unclamped) column count for a size class.
    fn raw_for(&self, class: EnzSizeClass) -> usize {
        match class {
            EnzSizeClass::Compact => self.compact,
            EnzSizeClass::Regular => self.narrow,
            EnzSizeClass::Medium => self.medium,
            EnzSizeClass::Expanded | EnzSizeClass::Large => self.wide,
        }
    }

    /// Column count for a laid-out width, clamped to `>= 1` so a bad knob can
    /// never produce a zero/NaN cell width.
    pub fn columns_for_width(&self, width: f32) -> usize {
        self.raw_for(class_for_width(width)).max(1)
    }
}

/// Per-tier sidecar for [`EnzResponsiveGrid`]: the column counts set by
/// the narrow `columns_*` setters, overlaid on the whole-ramp base at read time
/// so a later [`EnzResponsiveGrid::ramp`] cannot silently discard them. `None`
/// per tier = defer to [`ColumnRamp`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct RampOverrides {
    wide: Option<usize>,
    medium: Option<usize>,
    narrow: Option<usize>,
    compact: Option<usize>,
}

impl RampOverrides {
    /// Overlay these per-tier overrides on a whole-ramp `base`. A tier set by a
    /// narrow setter wins; a tier only `base` set is kept.
    fn apply(self, base: ColumnRamp) -> ColumnRamp {
        ColumnRamp {
            wide: self.wide.unwrap_or(base.wide),
            medium: self.medium.unwrap_or(base.medium),
            narrow: self.narrow.unwrap_or(base.narrow),
            compact: self.compact.unwrap_or(base.compact),
        }
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// A cell builder closure — rebuilt on every reflow (see module docs).
pub type CellFn<'a, Message> = Box<dyn Fn() -> Element<'a, Message> + 'a>;

/// Convenience constructor: `enz_responsive_grid(cells)`.
///
/// `cells` are builder closures, one per cell, each producing a fresh
/// [`Element`] (the responsive closure re-runs them on every layout pass).
///
/// Positional reconciliation. This constructor uses positional (index-based)
/// tree diffing. When the column count changes during a responsive reflow,
/// widget state (focus, text cursor, scroll offset) may be reconciled into the
/// wrong child or destroyed, because the `Column<Row<...>>` tree structure
/// shifts. When cells carry stateful children, use
/// [`enz_responsive_grid_keyed`] instead.
pub fn enz_responsive_grid<'a, Message>(
    cells: Vec<CellFn<'a, Message>>,
) -> EnzResponsiveGrid<'a, Message>
where
    Message: 'a,
{
    EnzResponsiveGrid {
        cells,
        keys: None,
        ramp: ColumnRamp::default(),
        ramp_overrides: RampOverrides::default(),
        gap_x: SpaceToken::Sm,
        gap_y: SpaceToken::Sm,
        inset_x: SpaceToken::None,
        inset_y: SpaceToken::None,
        fill: false,
        spacing_scale: None,
    }
}

/// Keyed responsive grid constructor: `enz_responsive_grid_keyed(cells)`.
///
/// Each cell is a `(key, closure)` pair. The key is a stable logical identifier
/// (e.g. a model id, a path segment) that does not depend on the cell's
/// position. The grid derives a stable `iced::advanced::widget::Id` for each
/// cell via [`KeyedIds`], and uses a flat [`KeyedGrid`](crate::grid::KeyedGrid)
/// layout that reconciles children by key rather than position.
///
/// When the column count changes during a responsive reflow, widget state
/// (focus, text cursor, scroll offset) follows the key, not the grid slot.
///
/// # Id derivation happens outside the closure
///
/// The stable ids are derived once from the key list and captured by the
/// `responsive` closure by value. They are not re-derived on each layout
/// pass. This is critical: `responsive` re-invokes its closure on every layout
/// pass, and re-deriving would produce fresh ids each time, defeating the
/// purpose of stable identity.
pub fn enz_responsive_grid_keyed<'a, Message>(
    cells: Vec<(String, CellFn<'a, Message>)>,
) -> EnzResponsiveGrid<'a, Message>
where
    Message: 'a,
{
    let keys: Vec<String> = cells.iter().map(|(k, _)| k.clone()).collect();
    let fns: Vec<CellFn<'a, Message>> = cells.into_iter().map(|(_, f)| f).collect();
    EnzResponsiveGrid {
        cells: fns,
        keys: Some(keys),
        ramp: ColumnRamp::default(),
        ramp_overrides: RampOverrides::default(),
        gap_x: SpaceToken::Sm,
        gap_y: SpaceToken::Sm,
        inset_x: SpaceToken::None,
        inset_y: SpaceToken::None,
        fill: false,
        spacing_scale: None,
    }
}

/// Builder returned by [`enz_responsive_grid`] and
/// [`enz_responsive_grid_keyed`].
#[must_use]
pub struct EnzResponsiveGrid<'a, Message> {
    cells: Vec<CellFn<'a, Message>>,
    /// When `Some`, each cell carries a stable logical key; the grid uses
    /// [`KeyedGrid`](crate::grid::KeyedGrid) for identity-preserving reflow.
    /// When `None`, the grid uses positional [`enz_grid`] (`Column<Row>`).
    keys: Option<Vec<String>>,
    /// The whole-ramp base, set by [`EnzResponsiveGrid::ramp`]. The per-tier
    /// `columns_*` setters do not write here; they write
    /// [`Self::ramp_overrides`], a sidecar this field cannot clobber.
    ramp: ColumnRamp,
    /// Per-tier sidecar written by [`EnzResponsiveGrid::columns_wide`] …
    /// [`EnzResponsiveGrid::columns_compact`], overlaid on [`Self::ramp`] at
    /// read time by [`Self::effective_ramp`].
    ///
    /// The `columns_*` setters write to this sidecar rather than
    /// `self.ramp.<tier>`. Because [`EnzResponsiveGrid::ramp`] never touches the
    /// sidecar, per-tier overrides win in either order.
    ramp_overrides: RampOverrides,
    gap_x: SpaceToken,
    gap_y: SpaceToken,
    inset_x: SpaceToken,
    inset_y: SpaceToken,
    fill: bool,
    /// Explicit spacing-scale override for layout-time consumers.
    spacing_scale: Option<SpacingScale>,
}

impl<'a, Message> EnzResponsiveGrid<'a, Message>
where
    Message: 'a,
{
    /// Set the Expanded/Large-tier column count (`columns-wide`, def 4).
    ///
    /// Records into the `Self::ramp_overrides` sidecar, so it wins over
    /// a [`Self::ramp`] call in either order.
    pub fn columns_wide(mut self, n: usize) -> Self {
        self.ramp_overrides.wide = Some(n);
        self
    }

    /// Set the Medium-tier column count (`columns-medium`, def 3).
    ///
    /// Records into the `Self::ramp_overrides` sidecar.
    pub fn columns_medium(mut self, n: usize) -> Self {
        self.ramp_overrides.medium = Some(n);
        self
    }

    /// Set the Regular-tier column count (`columns-narrow`, def 2).
    ///
    /// Records into the `Self::ramp_overrides` sidecar.
    pub fn columns_narrow(mut self, n: usize) -> Self {
        self.ramp_overrides.narrow = Some(n);
        self
    }

    /// Set the Compact-tier column count (`columns-compact`, def 1).
    ///
    /// Records into the `Self::ramp_overrides` sidecar.
    pub fn columns_compact(mut self, n: usize) -> Self {
        self.ramp_overrides.compact = Some(n);
        self
    }

    /// Set the whole column ramp at once.
    ///
    /// # Order-independent: a base, not a clobber
    ///
    /// This sets a whole-ramp base; the per-tier `columns_*` setters record
    /// into a sidecar (`Self::ramp_overrides`) this method does not touch, and
    /// `Self::effective_ramp` overlays it at read time. So
    /// `.columns_wide(2).ramp(r)` and `.ramp(r).columns_wide(2)` both keep the
    /// 2 — the more specific per-tier setter wins in either order. A narrow
    /// setter never writes through to `self.ramp`, so a later
    /// `ramp()` cannot discard it.
    pub fn ramp(mut self, ramp: ColumnRamp) -> Self {
        self.ramp = ramp;
        self
    }

    /// The ramp this grid resolves, with the per-tier sidecar overlaid on the
    /// [`Self::ramp`] base. A tier set by a narrow `columns_*` setter
    /// wins; a tier only [`Self::ramp`] set is kept. Resolving here — rather
    /// than writing through at set time — is what makes the builder
    /// order-independent.
    fn effective_ramp(&self) -> ColumnRamp {
        self.ramp_overrides.apply(self.ramp)
    }

    /// Set both inter-cell gaps .
    pub fn gap(mut self, token: SpaceToken) -> Self {
        self.gap_x = token;
        self.gap_y = token;
        self
    }

    /// Set the horizontal (cell-to-cell) gap .
    pub fn gap_x(mut self, token: SpaceToken) -> Self {
        self.gap_x = token;
        self
    }

    /// Set the vertical (row-to-row) gap .
    pub fn gap_y(mut self, token: SpaceToken) -> Self {
        self.gap_y = token;
        self
    }

    /// Set both edge insets .
    pub fn inset(mut self, token: SpaceToken) -> Self {
        self.inset_x = token;
        self.inset_y = token;
        self
    }

    /// Set the horizontal (left+right) inset .
    pub fn inset_x(mut self, token: SpaceToken) -> Self {
        self.inset_x = token;
        self
    }

    /// Set the vertical (top+bottom) inset .
    pub fn inset_y(mut self, token: SpaceToken) -> Self {
        self.inset_y = token;
        self
    }

    /// Claim the parent's height (`fill: true` →
    /// `vertical-stretch: 1`). Default `false` = natural content height.
    pub fn fill(mut self, on: bool) -> Self {
        self.fill = on;
        self
    }

    /// Forward a custom [`SpacingScale`] so gap/inset token resolution uses
    /// it instead of the stock ladder.
    ///
    /// `Widget::layout` has no `&Theme` parameter, so the spacing scale must
    /// be threaded in explicitly at build time. Pass `theme.spacing` from
    /// the app's `EnzTheme` to make the grid follow a custom density.
    pub fn spacing_scale(mut self, spacing: SpacingScale) -> Self {
        self.spacing_scale = Some(spacing);
        self
    }
}

impl<'a, Message> EnzResponsiveGrid<'a, Message>
where
    Message: 'a + 'static,
{
    /// Finish the builder and hand back the [`Element`], which is step 3 of
    /// the build pattern every widget in this crate follows (`docs/widgets.md`).
    ///
    /// This forwards to the `From<Self> for Element` impl below, so the two
    /// spellings are one implementation and cannot drift apart. Prefer this
    /// one inside `row![]` / `column![]`, where `.into()` has no target type
    /// to infer.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        Element::from(self)
    }
}

impl<'a, Message> From<EnzResponsiveGrid<'a, Message>> for Element<'a, Message>
where
    Message: 'a + 'static,
{
    fn from(g: EnzResponsiveGrid<'a, Message>) -> Self {
        // Resolve the per-tier sidecar over the whole-ramp base before the
        // destructure moves the struct apart, so both `.columns_*().ramp()`
        // orderings pick the same column counts.
        let ramp = g.effective_ramp();
        let EnzResponsiveGrid {
            cells,
            keys,
            ramp: _,
            ramp_overrides: _,
            gap_x,
            gap_y,
            inset_x,
            inset_y,
            fill,
            spacing_scale,
        } = g;

        // Layout-time consumers (`enz_grid` spacing, wrapper padding) are read
        // in `layout()` where iced supplies no `&Theme`. The only way to
        // receive a custom density is explicit forwarding via
        // `.spacing_scale()`. Without it, the stock ladder applies.
        let scale = spacing_scale.unwrap_or_default();
        let gap_x_px = gap_x.resolve(&scale);
        let gap_y_px = gap_y.resolve(&scale);
        let inset_x_px = inset_x.resolve(&scale);
        let inset_y_px = inset_y.resolve(&scale);

        let height = if fill { Length::Fill } else { Length::Shrink };

        // Derive stable ids outside the responsive closure so they are
        // computed once and captured by move. The closure re-runs on every
        // layout pass (iced's `responsive`); re-deriving inside would produce
        // fresh ids each time, defeating identity preservation.
        let cell_ids: Option<Vec<iced::advanced::widget::Id>> = keys.as_ref().map(|ks| {
            let mut id_deriver = KeyedIds::new("enz-responsive-grid");
            ks.iter().map(|k| id_deriver.id_for(k)).collect()
        });

        // The `enz_responsive` closure runs at most once per distinct width
        // per frame; it classifies its own available width, picks the
        // column count, rebuilds the cells, and defers the equal-width row
        // math to `enz_grid` (positional) or `enz_keyed_grid` (keyed,
        // identity-preserving).
        let element: Element<'a, Message> = enz_responsive(move |size| {
            let columns = ramp.columns_for_width(size.width);

            if let (Some(keys), Some(ids)) = (&keys, &cell_ids) {
                // Keyed path: use flat KeyedGrid for identity-preserving reflow.
                let keyed_children: Vec<(String, Element<'a, Message>)> = cells
                    .iter()
                    .zip(keys.iter())
                    .zip(ids.iter())
                    .map(|((f, key), id)| {
                        let cell = iced::widget::container(f())
                            .id(id.clone())
                            .width(Length::Fill)
                            .into();
                        (key.clone(), cell)
                    })
                    .collect();
                let grid: Element<'a, Message> = enz_keyed_grid(columns, keyed_children)
                    .col_gap(gap_x_px)
                    .row_gap(gap_y_px)
                    .padding_x(inset_x_px)
                    .padding_y(inset_y_px)
                    .width(Length::Fill)
                    .height(Length::Shrink)
                    .into();
                grid
            } else {
                // Positional path: original Column<Row> layout via enz_grid.
                let children: Vec<Element<'a, Message>> = cells.iter().map(|f| f()).collect();
                let grid: Element<'a, Message> = enz_grid(columns, children)
                    .col_gap(gap_x_px)
                    .row_gap(gap_y_px)
                    .padding_x(inset_x_px)
                    .padding_y(inset_y_px)
                    .stretch_cells(true)
                    .width(Length::Fill)
                    .height(Length::Shrink)
                    .into();
                grid
            }
        })
        .width(Length::Fill)
        .height(height)
        .into();

        element
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

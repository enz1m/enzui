//! Responsive column priority and the row-detail fold destination for
//! [`EnzTable`](super::EnzTable).
//!
//! # The four parts of a complete fold
//!
//! A compact table must not simply drop columns below an app-authored width.
//! Responsive folding requires four coordinated parts: a column set that
//! shrinks with the allocation, a destination for each value it removes,
//! semantics on that destination, and a keyboard route to it. Each part can
//! be correct in isolation while the overall table still makes data
//! unreachable.
//!
//! This module makes the complete outcome the default of one builder call:
//!
//! * [`ColumnFold`] cannot be constructed without a value projection, so the
//!   fold destination is part of the model rather than an afterthought a
//!   consumer might forget;
//! * [`plan_columns`] yields columns in declared priority order, measured
//!   from the row's own allocation rather than from an app-authored
//!   breakpoint;
//! * every folded value is published as a keyboard-focusable `Role::Button`
//!   carrying the value in its accessible name, so hidden cell values remain
//!   in the tree; when [`ColumnFold::on_activate`] is wired, the button is
//!   also operable, because presence in the tree alone is not enough.
//!
//! # Conservation by construction
//!
//! [`plan_columns`] returns a partition: every candidate column is either
//! visible or folded, never neither. The row widget builds one fold entry
//! per folded column from the same partition, so the set of destinations at
//! a narrow rung is a superset of the wide rung's by construction — not by
//! each app remembering to check.
//!
//! # Cost
//!
//! The fold decision runs in `layout()`, which fires on every hover-induced
//! `invalidate_layout` and every frame of a resize. The plan is therefore
//! cached in tree state keyed by the measured width: a second layout pass
//! at the same width reuses it and [`fold_probe`] counts nothing, while a
//! genuine width change recomputes so the plan still matches the current
//! allocation. Element subtrees are never rebuilt here: children are built
//! once per `view()` and the plan only chooses which of them participate.

use enzui_core::semantics::{Role, Semantics};
use enzui_core::table::ColumnKey;
use iced::advanced::layout::Measure as _;
use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Shell, Widget, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

/// The default height of one folded-value row, in logical px.
pub const FOLD_ENTRY_HEIGHT: f32 = 22.0;

/// The default left indent of the fold destination under its row, in px.
pub const FOLD_INDENT: f32 = 12.0;

// ---------------------------------------------------------------------------
// FoldedField — what a fold entry publishes on activation
// ---------------------------------------------------------------------------

/// One folded cell: which row, which column, and the projected value.
///
/// Handed to [`ColumnFold::on_activate`] when a fold entry is activated
/// (pointer, Enter or Space). `row` is the model row index and `column` the
/// stable [`ColumnKey`] — the durable identity pair, as with
/// [`CellCoord`](super::CellCoord). `value` is what
/// [`ColumnFold::new`]'s projection produced for that cell this frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoldedField {
    /// The model row index.
    pub row: u64,
    /// The stable key of the folded column.
    pub column: ColumnKey,
    /// The projected value of the folded cell.
    pub value: String,
}

// ---------------------------------------------------------------------------
// ColumnFold — the declaration
// ---------------------------------------------------------------------------

/// Projects a `(row_id, column_index, &Cell)` triple to the text a fold
/// destination publishes. Named so [`ColumnFold::value`] does not trip
/// `clippy::type_complexity`.
type CellValueFn<'a, Cell> = std::rc::Rc<dyn Fn(u64, usize, &Cell) -> String + 'a>;

/// A declared column priority order plus its fold destination.
///
/// Hand one to [`EnzTable::column_fold`](super::EnzTable::column_fold).
/// Without a `ColumnFold`, priority is opt-in and the table behaves as if
/// no column ever folds.
///
/// # Why the projection is a constructor argument
///
/// A fold destination that a consumer can forget is the whole point of
/// requiring the value projection at declaration time — a caller that
/// declares priority cannot express the "nowhere to fold to" case.
/// The action on a folded value ([`on_activate`](Self::on_activate)) is
/// optional: without it the entry is still published and still
/// keyboard-reachable, just inert.
pub struct ColumnFold<'a, Message, Cell> {
    /// Columns in declared order, most important first. Columns absent
    /// from this list fold before any listed column.
    pub(super) priority: Vec<ColumnKey>,
    /// Projects a cell to the text the fold destination publishes.
    pub(super) value: CellValueFn<'a, Cell>,
    /// What a folded value's activation publishes, if anything.
    pub(super) on_activate: Option<std::rc::Rc<dyn Fn(FoldedField) -> Message + 'a>>,
    /// Namespace for the derived, field-keyed entry ids.
    pub(super) id_prefix: String,
    /// Height of one folded-value row.
    pub(super) entry_height: f32,
}

impl<Message, Cell> Clone for ColumnFold<'_, Message, Cell> {
    fn clone(&self) -> Self {
        Self {
            priority: self.priority.clone(),
            value: std::rc::Rc::clone(&self.value),
            on_activate: self.on_activate.clone(),
            id_prefix: self.id_prefix.clone(),
            entry_height: self.entry_height,
        }
    }
}

impl<'a, Message, Cell> ColumnFold<'a, Message, Cell> {
    /// Declare the priority order (most important first) and how a cell
    /// projects to the text its fold destination publishes.
    ///
    /// Columns not named in `priority` are treated as *least* important, in
    /// column order, so naming only the relevant columns is enough.
    #[must_use]
    pub fn new<I, K>(priority: I, value: impl Fn(u64, usize, &Cell) -> String + 'a) -> Self
    where
        I: IntoIterator<Item = K>,
        K: Into<ColumnKey>,
    {
        ColumnFold {
            priority: priority.into_iter().map(Into::into).collect(),
            value: std::rc::Rc::new(value),
            on_activate: None,
            id_prefix: "enz-table-fold".to_owned(),
            entry_height: FOLD_ENTRY_HEIGHT,
        }
    }

    /// Make each folded value operable: activating it (pointer, Enter or
    /// Space) publishes this message.
    ///
    /// Fold entries are copy actions, and a value behind a control nobody
    /// can operate is not reachable. Without this the entry is focusable
    /// and named but inert.
    #[must_use]
    pub fn on_activate(mut self, f: impl Fn(FoldedField) -> Message + 'a) -> Self {
        self.on_activate = Some(std::rc::Rc::new(f));
        self
    }

    /// Namespace the derived fold-entry ids.
    ///
    /// Ids are derived from the row key and column key — never from the
    /// sampled value — so a table that refreshes its data every second
    /// does not throw keyboard focus away when a number changes. The
    /// `folded_inspector_focus_survives_a_sample_value_change` test pins
    /// this.
    #[must_use]
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }

    /// Height of one folded-value row, in px. Defaults to
    /// [`FOLD_ENTRY_HEIGHT`].
    #[must_use]
    pub fn entry_height(mut self, h: f32) -> Self {
        self.entry_height = enzui_core::metrics::FiniteNonNeg::new_or(h, FOLD_ENTRY_HEIGHT).get();
        self
    }
}

/// The accessible name a folded value publishes: `"<column>: <value>"`.
///
/// One function so the drawn text, the `content_text` the collector
/// suppresses, and the semantics label cannot drift apart — and so a test can
/// ask for the exact string the tree should contain rather than re-typing the
/// format.
#[must_use]
pub fn fold_entry_label(column: &ColumnKey, value: &str) -> String {
    format!("{}: {}", column.as_str(), value)
}

/// The semantics a fold entry publishes: a `Role::Button` named by the
/// folded value, described by its column.
///
/// `Role::Button` rather than a bare label because the destination has to
/// be operable, not merely present.
///
/// # Why the name is the value verbatim and the column is the description
///
/// A conservation check compares the destination strings a rung
/// publishes. A visible cell publishes its value as its `GridCell` name
/// (name-from-content), so a fold entry named `"User: root"` would lose
/// `"root"` from the narrow rung's set even though the value is right
/// there on screen. Naming the entry with the value verbatim makes
/// conservation hold as string equality, the comparison consumers
/// actually write, and `aria-description` keeps the field name announced.
///
/// A cell whose projection is empty has no value to name, so the column
/// label is used instead — a nameless actionable node has no accessible
/// identity.
pub fn fold_entry_semantics(column: &ColumnKey, value: &str) -> Semantics {
    if value.trim().is_empty() {
        Semantics::new(Role::Button).label(column.as_str().to_owned())
    } else {
        Semantics::new(Role::Button)
            .label(value.to_owned())
            .description(column.as_str().to_owned())
    }
}

// ---------------------------------------------------------------------------
// The plan
// ---------------------------------------------------------------------------

/// A candidate column for the fold decision: its model index and its track
/// width.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FoldCandidate {
    /// Model column index.
    pub column: usize,
    /// The column's track width in px.
    pub width: f32,
}

/// The partition [`plan_columns`] produces: every candidate is in exactly one
/// of the two sets, both in ascending column order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FoldPlan {
    /// Columns that keep their track, in column order.
    pub visible: Vec<usize>,
    /// Columns whose value moved to the fold destination, in column order.
    pub folded: Vec<usize>,
}

/// Decide which columns keep their track at `available` px.
///
/// `candidates` are the foldable columns in column order; `order` lists
/// model column indices most important first (as produced by
/// [`priority_order`]).
///
/// Rules:
///
/// 1. Columns yield in the declared order: walking `order` from the most
///    important, a column keeps its track while the running total of
///    track widths fits in `available`. The first column that does not
///    fit yields, and so does every less important column — a narrower
///    low-priority column is not allowed to jump the queue, because
///    "declared priority" is the contract and best-fit packing would
///    silently violate it.
/// 2. The most important column always stands, even if it alone exceeds
///    `available`: a table with no columns is not a table.
/// 3. A non-finite or non-positive `available` means "unmeasured" — nothing
///    folds, because folding on an unknown width would hide data for a
///    layout that never happened.
#[must_use]
pub fn plan_columns(candidates: &[FoldCandidate], order: &[usize], available: f32) -> FoldPlan {
    let mut plan = FoldPlan::default();
    if candidates.is_empty() {
        return plan;
    }
    if !available.is_finite() || available <= 0.0 {
        plan.visible = candidates.iter().map(|c| c.column).collect();
        return plan;
    }

    let width_of = |column: usize| -> f32 {
        candidates
            .iter()
            .find(|c| c.column == column)
            .map_or(0.0, |c| c.width)
    };

    let mut keep: Vec<usize> = Vec::new();
    let mut used = 0.0f32;
    for (rank, column) in order
        .iter()
        .copied()
        .filter(|c| candidates.iter().any(|k| k.column == *c))
        .enumerate()
    {
        let w = width_of(column);
        if rank == 0 {
            // Rule 2: the most important column always stands.
            keep.push(column);
            used += w;
            continue;
        }
        if used + w <= available {
            keep.push(column);
            used += w;
        } else {
            // Rule 1: everything less important yields too.
            break;
        }
    }

    for candidate in candidates {
        if keep.contains(&candidate.column) {
            plan.visible.push(candidate.column);
        } else {
            plan.folded.push(candidate.column);
        }
    }
    plan
}

/// Model column indices in declared priority order, most important first.
///
/// `declared` names columns by key; `resolve` maps a key to its model index.
/// Every candidate column absent from `declared` is appended in column order,
/// so it folds before any declared column — declaring one column important is
/// enough, and an unknown key is skipped rather than shifting the order.
#[must_use]
pub fn priority_order(
    declared: &[ColumnKey],
    candidates: &[FoldCandidate],
    resolve: impl Fn(&ColumnKey) -> Option<usize>,
) -> Vec<usize> {
    let mut order: Vec<usize> = Vec::new();
    for key in declared {
        if let Some(index) = resolve(key)
            && candidates.iter().any(|c| c.column == index)
            && !order.contains(&index)
        {
            order.push(index);
        }
    }
    for candidate in candidates {
        if !order.contains(&candidate.column) {
            order.push(candidate.column);
        }
    }
    order
}

// ---------------------------------------------------------------------------
// fold_probe — the fold-plan computation counter
// ---------------------------------------------------------------------------

/// Test-only probe counting fold-plan computations.
///
/// A responsive table must not redo fold planning on every layout pass. This
/// counter provides deterministic evidence of reuse without relying on timing:
/// `note_plan` fires only when a folding row computes a plan, while a layout
/// pass served from the width-keyed cache does not touch it.
///
/// Thread-local for the same reason `app_shell::build_probe` is: a
/// process-global counter attributes another test thread's layout to this one.
#[doc(hidden)]
pub mod fold_probe {
    use std::cell::Cell;

    thread_local! {
        static PLANS: Cell<u64> = const { Cell::new(0) };
        static LAST_WIDTH_BITS: Cell<u32> = const { Cell::new(0x7fc0_0000) }; // qNaN
        static CACHE_HITS: Cell<u64> = const { Cell::new(0) };
    }

    /// Note a fold-plan computation at `available` px.
    #[inline]
    pub fn note_plan(available: f32) {
        LAST_WIDTH_BITS.with(|w| w.set(available.to_bits()));
        PLANS.with(|c| c.set(c.get().wrapping_add(1)));
    }

    /// Note a layout pass served from the width-keyed cache.
    #[inline]
    pub fn note_cache_hit() {
        CACHE_HITS.with(|c| c.set(c.get().wrapping_add(1)));
    }

    /// This thread's fold-plan computation count.
    #[must_use]
    pub fn plans() -> u64 {
        PLANS.with(Cell::get)
    }

    /// This thread's cache-hit count.
    #[must_use]
    pub fn cache_hits() -> u64 {
        CACHE_HITS.with(Cell::get)
    }

    /// The width the most recent plan was computed at; `NaN` before the first.
    #[must_use]
    pub fn last_width() -> f32 {
        f32::from_bits(LAST_WIDTH_BITS.with(Cell::get))
    }

    /// Reset this thread's counters, returning the previous plan count.
    pub fn reset() -> u64 {
        LAST_WIDTH_BITS.with(|w| w.set(f32::NAN.to_bits()));
        CACHE_HITS.with(|c| c.set(0));
        PLANS.with(|c| c.replace(0))
    }
}

// ---------------------------------------------------------------------------
// FoldRow — the widget that measures, partitions and renders
// ---------------------------------------------------------------------------

/// Retained state: the plan, keyed by the width it was measured at.
#[derive(Debug, Default)]
struct FoldRowState {
    width_bits: Option<u32>,
    /// Fingerprint of the inputs the plan was computed from. The width
    /// alone is not a sufficient key: an app that changes a column's
    /// track width or re-declares its priority between frames would
    /// otherwise be served a stale plan at the same width, so the key
    /// covers everything `plan_columns` reads.
    inputs: u64,
    /// Per-child participation, in child order (cells then entries).
    active: Vec<bool>,
}

/// FNV-1a over the plan inputs. A hash, not equality, so the retained state
/// stays a fixed size no matter how many columns a table has.
fn fingerprint(cells: &[FoldCandidate], entries: &[usize], order: &[usize]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |value: u64| {
        for byte in value.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x1000_0000_01b3);
        }
    };
    for cell in cells {
        eat(cell.column as u64);
        eat(u64::from(cell.width.to_bits()));
    }
    eat(u64::MAX);
    for entry in entries {
        eat(*entry as u64);
    }
    eat(u64::MAX);
    for column in order {
        eat(*column as u64);
    }
    hash
}

/// One row of a folding table: the cell tracks it can show, plus the fold
/// destination for the ones it cannot.
///
/// Children are `cells` (in column order) followed by `entries` (one per
/// foldable column, in column order). Both are built by
/// [`EnzTable`](super::EnzTable) once per `view()`; this widget only decides
/// which participate, and a non-participating child is inert, not merely
/// unpainted: `draw`, `operate`, `update` and `mouse_interaction` all skip it,
/// so a hidden column's cell is out of the semantics tree and out of the focus
/// ring while its fold entry participates in both; clipping cannot make a
/// child semantically inert by itself.
pub(super) struct FoldRow<'a, Message> {
    children: Vec<Element<'a, Message>>,
    /// Candidates in column order; `children[i]` is the cell of `cells[i]`.
    cells: Vec<FoldCandidate>,
    /// Model column index of each entry child, in column order.
    entries: Vec<usize>,
    /// Priority order, most important first.
    order: Vec<usize>,
    row_height: f32,
    entry_height: f32,
    /// Subtracted from the measured allocation before planning, so a header
    /// row outside the scrollbar gutter plans from the same number its body
    /// does.
    width_adjust: f32,
}

impl<'a, Message> FoldRow<'a, Message> {
    pub(super) fn new(
        cells: Vec<(FoldCandidate, Element<'a, Message>)>,
        entries: Vec<(usize, Element<'a, Message>)>,
        order: Vec<usize>,
        row_height: f32,
        entry_height: f32,
        width_adjust: f32,
    ) -> Self {
        let mut children: Vec<Element<'a, Message>> =
            Vec::with_capacity(cells.len() + entries.len());
        let mut candidates = Vec::with_capacity(cells.len());
        for (candidate, element) in cells {
            candidates.push(candidate);
            children.push(element);
        }
        let mut entry_columns = Vec::with_capacity(entries.len());
        for (column, element) in entries {
            entry_columns.push(column);
            children.push(element);
        }
        FoldRow {
            children,
            cells: candidates,
            entries: entry_columns,
            order,
            row_height,
            entry_height,
            width_adjust,
        }
    }

    /// The measured allocation this row plans from.
    fn budget(&self, limits: &layout::Limits) -> f32 {
        let max = limits.max().width;
        if max.is_finite() {
            (max - self.width_adjust).max(0.0)
        } else {
            max
        }
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for FoldRow<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<FoldRowState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(FoldRowState::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(&mut self.children);
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: Length::Shrink,
            height: Length::Shrink,
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let available = self.budget(limits);
        let width_bits = available.to_bits();

        // Recompute only when the measured width genuinely changed; a
        // stale plan at a different width must not be served.
        let inputs = fingerprint(&self.cells, &self.entries, &self.order);
        let cached = {
            let state = tree.state.downcast_ref::<FoldRowState>();
            (state.width_bits == Some(width_bits)
                && state.inputs == inputs
                && state.active.len() == self.children.len())
            .then(|| state.active.clone())
        };

        let active = match cached {
            Some(active) => {
                fold_probe::note_cache_hit();
                active
            }
            None => {
                fold_probe::note_plan(available);
                let plan = plan_columns(&self.cells, &self.order, available);
                let mut active = vec![false; self.children.len()];
                for (i, candidate) in self.cells.iter().enumerate() {
                    active[i] = plan.visible.contains(&candidate.column);
                }
                for (j, column) in self.entries.iter().enumerate() {
                    active[self.cells.len() + j] = plan.folded.contains(column);
                }
                let state = tree.state.downcast_mut::<FoldRowState>();
                state.width_bits = Some(width_bits);
                state.inputs = inputs;
                state.active.clone_from(&active);
                active
            }
        };

        let mut nodes: Vec<layout::Node> = Vec::with_capacity(self.children.len());
        let mut x = 0.0f32;
        for (i, candidate) in self.cells.iter().enumerate() {
            if !active[i] {
                nodes.push(layout::Node::new(Size::ZERO));
                continue;
            }
            // An intrinsic floor is clamped to what the parent actually
            // offers. The most important column always stands
            // (`plan_columns` rule 2) even when its track alone exceeds
            // the allocation, so without this clamp a 120 px cell would
            // paint over its neighbour in an 80 px region.
            let track = if available.is_finite() {
                candidate.width.min((available - x).max(0.0))
            } else {
                candidate.width
            };
            let child_limits =
                layout::Limits::new(Size::ZERO, Size::new(track, self.row_height.max(0.0)));
            let node = self.children[i].as_widget_mut().measure(
                &mut tree.children[i],
                renderer,
                &child_limits,
            );
            // Centre the cell in the row band. A cell shorter than the row
            // — a single-line text cell in a 28 px row, the ordinary case
            // — otherwise sits at y=0 while the row's own selection/hover
            // pill stays centred. The row's height is the band this widget
            // was given; a cell taller than it still starts at 0.
            let y = ((self.row_height.max(0.0) - node.size().height) / 2.0).max(0.0);
            nodes.push(node.move_to(iced::Point::new(x, y)));
            x += track;
        }

        let entry_width = if available.is_finite() {
            (available - FOLD_INDENT).max(0.0)
        } else {
            x
        };
        let mut y = self.row_height.max(0.0);
        for (j, _column) in self.entries.iter().enumerate() {
            let index = self.cells.len() + j;
            if !active[index] {
                nodes.push(layout::Node::new(Size::ZERO));
                continue;
            }
            let child_limits =
                layout::Limits::new(Size::ZERO, Size::new(entry_width, self.entry_height));
            let node = self.children[index].as_widget_mut().measure(
                &mut tree.children[index],
                renderer,
                &child_limits,
            );
            nodes.push(node.move_to(iced::Point::new(FOLD_INDENT, y)));
            y += self.entry_height;
        }

        let mut width = x.max(if y > self.row_height.max(0.0) {
            entry_width + FOLD_INDENT
        } else {
            0.0
        });
        if available.is_finite() {
            width = width.min(available);
        }
        // The height is deliberately not clamped: a data row is as tall as
        // its row height plus the destinations it is holding, and a row
        // shorter than its own content would clip its folded values away.
        // Recorded as a `RigidMinimum` Y exception in the bounds sweep.
        layout::Node::with_children(Size::new(width, y), nodes).apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let active = tree.state.downcast_ref::<FoldRowState>().active.clone();
        operation.container(None, layout.bounds(), viewport);
        operation.traverse(&mut |operation| {
            for (i, (child, (child_layout, state))) in self
                .children
                .iter_mut()
                .zip(layout.iter_mut(&mut tree.children))
                .enumerate()
            {
                if !active.get(i).copied().unwrap_or(true) {
                    continue;
                }
                child
                    .as_widget_mut()
                    .operate(state, child_layout, viewport, renderer, operation);
            }
        });
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let active = tree.state.downcast_ref::<FoldRowState>().active.clone();
        for (i, (child, (child_layout, state))) in self
            .children
            .iter_mut()
            .zip(layout.iter_mut(&mut tree.children))
            .enumerate()
        {
            if !active.get(i).copied().unwrap_or(true) {
                continue;
            }
            child.as_widget_mut().update(
                state,
                event,
                child_layout,
                cursor,
                renderer,
                shell,
                viewport,
            );
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let active = &tree.state.downcast_ref::<FoldRowState>().active;
        self.children
            .iter()
            .zip(layout.iter(&tree.children))
            .enumerate()
            .filter(|(i, _)| active.get(*i).copied().unwrap_or(true))
            .map(|(_, (child, (child_layout, state)))| {
                child
                    .as_widget()
                    .mouse_interaction(state, child_layout, cursor, viewport, renderer)
            })
            .max()
            .unwrap_or_default()
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let active = &tree.state.downcast_ref::<FoldRowState>().active;
        for (_i, (child, (child_layout, state))) in self
            .children
            .iter()
            .zip(layout.iter(&tree.children))
            .enumerate()
            .filter(|(i, _)| active.get(*i).copied().unwrap_or(true))
        {
            child.as_widget().draw(
                state,
                renderer,
                theme,
                style,
                child_layout,
                cursor,
                viewport,
            );
        }
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        // Not `overlay::from_children`: a non-participating child has a
        // childless zero node, and handing that to a `container`'s overlay
        // (which unwraps `layout.children().next()`) panics. Skipping is
        // the same rule the other four methods follow — an inert child is
        // inert on every path.
        let active = tree.state.downcast_ref::<FoldRowState>().active.clone();
        let overlays: Vec<_> = self
            .children
            .iter_mut()
            .zip(layout.iter_mut(&mut tree.children))
            .enumerate()
            .filter(|(index, _)| active.get(*index).copied().unwrap_or(true))
            .flat_map(|(_, (child, (child_layout, state)))| {
                child.as_widget_mut().overlay(
                    state,
                    child_layout,
                    renderer,
                    viewport,
                    translation,
                    window,
                )
            })
            .collect();
        overlays
    }
}

impl<'a, Message: 'a> From<FoldRow<'a, Message>> for Element<'a, Message> {
    fn from(row: FoldRow<'a, Message>) -> Self {
        Element::new(row)
    }
}

// ---------------------------------------------------------------------------
//  bounds-sweep seam
// ---------------------------------------------------------------------------

/// Test-only constructor putting a bare [`FoldRow`] at the root of an
/// `Element`, for the custom-`Widget` bounds sweep.
///
/// `FoldRow` originates geometry (it sizes every track and stacks the
/// fold destination), so it is swept rather than recorded uncovered. In
/// its natural home it sits under `EnzTable`'s outer
/// `container(..).width(..).height(..)`, which clamps whatever node it
/// returns; rooting it here means the sweep measures this widget's own
/// `layout()` output.
///
/// The row is deliberately built narrower than its tracks want, so the
/// sweep exercises the folded shape (a hidden zero-size cell and a
/// laid-out fold entry) rather than the trivial everything-fits one.
#[doc(hidden)]
#[must_use]
pub fn __test_only_fold_row<'a, Message: 'a>(
    content: Element<'a, Message>,
) -> Element<'a, Message> {
    use iced::widget::Space;
    let cells = vec![
        (
            FoldCandidate {
                column: 0,
                width: 120.0,
            },
            content,
        ),
        (
            FoldCandidate {
                column: 1,
                width: 120.0,
            },
            Space::new().width(120.0).height(20.0).into(),
        ),
    ];
    let entries = vec![(
        1usize,
        Space::new()
            .width(Length::Fill)
            .height(FOLD_ENTRY_HEIGHT)
            .into(),
    )];
    FoldRow::new(cells, entries, vec![0, 1], 20.0, FOLD_ENTRY_HEIGHT, 0.0).into()
}

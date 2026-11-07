//! `EnzReorderList`: a keyed list with drag-to-reorder. Simplest genuine
//! [`drag`](enzui_core::drag) consumers: keyed-child columns (or
//! [`.horizontal()`](EnzReorderList::horizontal) rows) with press-and-drag-past-
//! deadband reorder gestures. Clickable rows opt in via
//! [`.drag_through_children(true)`](EnzReorderList::drag_through_children):
//! deadband-inside releases stay child clicks; real drags suppress clicks,
//! publishing only reorders. Composite rows drag by a part of themselves:
//! [`.drag_handles(true)`](EnzReorderList::drag_handles) with the part wrapped
//! in [`reorder_handle`]. Releases publish single stable-ID-only
//! [`DropRequest`]s (no payloads); application `update`s resolve current
//! payloads exactly once.
//!
//! # Orientation and writing direction
//!
//! [`Horizontal`](Orientation::Horizontal) lists slot along x via
//! [`slot_for_position_1d_with`] (writing [`Direction`] threaded: RTL mirrors
//! horizontal midpoint rules — this render path makes that RTL geometry
//! reachable). Vertical axes stay top-to-bottom, writing-insensitive.
//!
//! # Architecture
//!
//! - Pointer press on children → origins recorded; [`Pending`](DragPhase::Pending)
//!   [`DragSession`]s begin.
//! - Deadband-crossing moves → [`Active`](DragPhase::Active) sessions; per-move
//!   [`slot_for_position_1d_with`] insertion slots (horizontal writing-[`Direction`]s
//!   threaded) + [`hover`](DragSession::hover)-negotiated operations.
//! - Active releases → [`release`](DragSession::release)s produce [`Dropped`](SessionOutcome::Dropped)
//!   single-[`DropRequest`]s.
//! - Escape / pointer lost → [`cancel`](DragSession::cancel)s, zero requests.
//!
//! Collection IDs default `"reorder-list"`; [`.collection_id()`](EnzReorderList::collection_id)
//! overrides.
//!
//! # Laziness contract
//!
//! Drag states advertise stable IDs + payload KINDS, never materialised
//! payloads. Accepted drops publish exactly one [`DropRequest`]; applications
//! resolve model payloads in `update`. Nothing here constructs payloads.

use crate::drag_drop_host::{
    CollectionAxis, CollectionRegistration, DragFeedbackTokens, DragHostChannel, LiveDrag,
};
use enzui_core::collect::SemanticsNode;
use enzui_core::drag::{
    CollectionId, DestinationProposal, DragOperation, DragPhase, DragSession, DropRequest,
    InsertionSlot, ItemId, PayloadKinds, SessionOutcome, SourceCapabilities,
    slot_for_position_1d_with,
};
use enzui_core::list_selection::{ClickModel, ClickModifiers, ListSelection};
use enzui_core::placement::{Direction, resolve_direction};
use enzui_core::semantics::{Role, Semantics};

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Shell, Widget, renderer};
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector, mouse};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default collection ID.
const DEFAULT_COLLECTION_ID: &str = "reorder-list";

mod feedback;
mod handle;
mod list_widget;

pub use handle::{ReorderHandle, reorder_handle};

// ---------------------------------------------------------------------------
// Orientation
// ---------------------------------------------------------------------------

/// The axis along which a [`EnzReorderList`] stacks its children and decides
/// the drag insertion slot.
///
/// The vertical axis is always top-to-bottom; only the horizontal axis is
/// writing-direction sensitive (see [`.direction()`](EnzReorderList::direction)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Orientation {
    /// Children stacked in a column; slot decided along y (top-to-bottom).
    #[default]
    Vertical,
    /// Children laid out in a row; slot decided along x, honouring the
    /// writing [`Direction`] (RTL mirrors the midpoint rule).
    Horizontal,
}

// ---------------------------------------------------------------------------
// Pipeline observation
// ---------------------------------------------------------------------------

/// An observable milestone of the [`DragSession`] pipeline. Values read from
/// sessions themselves per step (reported phases, negotiated operations,
/// produced outcomes): observers (consumers/tests) see pipeline-own states,
/// never widget guesses. Pipeline-severing trivial-state replacements change
/// carried values: properties stay directly testable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DragObservation {
    /// A press began a session; the session reports [`DragPhase::Pending`].
    Began(DragPhase),
    /// A move crossed the deadband; the session reports [`DragPhase::Active`].
    Activated(DragPhase),
    /// A hover re-negotiated the operation the session now advertises.
    Negotiated(DragOperation),
    /// The session terminated. `Some` carries an accepted drop's negotiated
    /// operation; `None` means the session cancelled (refused / over nothing).
    Outcome(Option<DragOperation>),
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// A keyed vertical list with drag-to-reorder.
#[must_use]
pub struct EnzReorderList<'a, Message> {
    /// The reconciliation key of each row, parallel to [`Self::children`].
    /// BESIDE-element storage (never zipped) keeps `self.children` plain
    /// `[Element]` slices: standard child-overlay paths
    /// (`iced::advanced::overlay::from_children`) demand that, matching sibling
    /// keyed collections (`KeyedRow`, `KeyedGrid`). `Vec<(String, Element)>`
    /// shapes can't reach those functions at all: load-bearing, not cosmetic.
    keys: Vec<String>,
    /// The row elements, in list order; `keys[i]` is the key of `children[i]`.
    children: Vec<Element<'a, Message, iced::Theme, iced::Renderer>>,
    /// Called when a drag-and-drop reorder completes.
    on_drop: Box<dyn Fn(DropRequest) -> Message + 'a>,
    /// Optional pipeline observer: fired at each [`DragSession`]
    /// milestone with values read from the session itself.
    on_drag_event: Option<Box<dyn Fn(DragObservation) -> Message + 'a>>,
    on_selection_change: Option<Box<dyn Fn(Vec<String>) -> Message + 'a>>,
    /// What the drag source permits. Routed into [`DragSession`] so a
    /// read-only source can genuinely refuse a drop via [`negotiate`].
    source_capabilities: SourceCapabilities,
    /// The operations the drop destination proposes, most-preferred first
    /// Negotiated against `source_capabilities` inside
    /// [`DragSession::hover`]; a proposal the source cannot satisfy resolves to
    /// [`DragOperation::Forbidden`] and the drop is refused.
    destination_operations: Vec<DragOperation>,
    /// Deadband in px.
    deadband: f32,
    /// Collection ID for the drag session.
    collection_id: String,
    /// The [`PayloadKind`](enzui_core::drag::PayloadKind)s a drag out of
    /// this list offers. Only consulted on the hosted path — a
    /// standalone list's source and destination are the same collection, so
    /// asking "what kind" of a self-reorder is meaningless (the reason
    /// [`DragSession::begin`] defaults to universal).
    offering: PayloadKinds,
    /// The kinds this list accepts as a drop destination. This is
    /// the half that can refuse: it is only reachable when some *other*
    /// collection is the source.
    accepting: PayloadKinds,
    /// The shared [`DragHostChannel`], when this list participates in an
    /// [`EnzDragDropHost`](crate::drag_drop_host::EnzDragDropHost). `None` —
    /// or a channel no host claimed this pass — means the standalone path,
    /// unchanged.
    drag_host: Option<DragHostChannel>,
    /// Widget width.
    width: Length,
    /// Widget height.
    height: Length,
    /// Gap between items.
    spacing: f32,
    /// The axis along which children stack and the slot is decided.
    orientation: Orientation,
    /// Columns of a wrapped grid, or `None` for a single row or column.
    columns: Option<usize>,
    /// The writing direction used for the horizontal axis. `None`
    /// reads the ambient locale direction via
    /// [`resolve_direction`](enzui_core::placement::resolve_direction); a
    /// builder override wins. Ignored for [`Orientation::Vertical`], whose axis
    /// is always top-to-bottom.
    direction: Option<Direction>,
    /// Opt out of the duplicate-key `debug_assert`. A reorder list
    /// keys its rows by `ItemId`, and reconciliation, `DropRequest.items` and
    /// selection all assume those keys are unique; a duplicate reconciles
    /// positionally among its occurrences, which is the bug keying removes.
    /// Set only when duplicate keys are deliberate.
    allow_duplicate_keys: bool,
    /// Allow a drag to begin from a child that captured the initial press.
    ///
    /// This is opt-in because an arbitrary interactive child may own a richer
    /// drag gesture. It is the intended composition for clickable list rows:
    /// a release inside the deadband remains the child's click, while crossing
    /// the deadband transfers the gesture to this reorder owner and suppresses
    /// the child's release activation.
    drag_through_children: bool,
    /// Only a press on a [`reorder_handle`] inside a row selects or drags it.
    drag_handles: bool,
    /// Treat a key that disappears from `children` as filtered out rather
    /// than deleted (default `false` = deleted). [`reconcile`](ListSelection::reconcile)s
    /// are filter-vs-delete FORGETTING halves; models can't distinguish —
    /// callers know domain-removals from filter-hides. Deletion defaults fit
    /// reorder-list drop routes (real-model splices; silently-returning deleted
    /// selections prevented). Search-box-driven same-list callers set this:
    /// filter-cycled rows stay selected.
    selection_persists_when_filtered: bool,
    /// The rows' corner radius, followed by the focus ring and the lifted
    /// row's shadow.
    row_radius: f32,
    /// How far each row's visible pill stands in from its sides.
    row_inset: f32,
    /// Insertion bar, empty-target outline and lift strength.
    feedback: DragFeedbackTokens,
}

/// Construct a reorderable vertical list.
///
/// `items` is an iterable of `(key, element)` pairs; `on_drop` is called with
/// the [`DropRequest`] when a reorder drag completes.
pub fn enz_reorder_list<'a, Message>(
    items: impl IntoIterator<
        Item = (
            impl Into<String>,
            Element<'a, Message, iced::Theme, iced::Renderer>,
        ),
    >,
    on_drop: impl Fn(DropRequest) -> Message + 'a,
) -> EnzReorderList<'a, Message> {
    let (keys, children): (
        Vec<String>,
        Vec<Element<'a, Message, iced::Theme, iced::Renderer>>,
    ) = items.into_iter().map(|(k, e)| (k.into(), e)).unzip();

    EnzReorderList {
        keys,
        children,
        on_drop: Box::new(on_drop),
        on_drag_event: None,
        on_selection_change: None,
        source_capabilities: SourceCapabilities::move_only(),
        destination_operations: vec![DragOperation::Move],
        deadband: enzui_core::desktop_settings::input().drag_threshold,
        collection_id: DEFAULT_COLLECTION_ID.to_owned(),
        offering: PayloadKinds::universal(),
        accepting: PayloadKinds::universal(),
        drag_host: None,
        width: Length::Fill,
        height: Length::Shrink,
        spacing: 0.0,
        orientation: Orientation::Vertical,
        columns: None,
        direction: None,
        allow_duplicate_keys: false,
        drag_through_children: false,
        drag_handles: false,
        selection_persists_when_filtered: false,
        row_radius: enzui_theme::text_defaults::shape_radius(|s| s.row),
        row_inset: 0.0,
        feedback: DragFeedbackTokens::default(),
    }
}

impl<'a, Message> EnzReorderList<'a, Message> {
    /// The rows' corner radius (default: the theme's `row` radius). The keyboard focus ring and a
    /// lifted row's shadow follow it, so a list of pill-shaped rows passes
    /// the pills' radius.
    pub fn row_radius(mut self, radius: f32) -> Self {
        self.row_radius = enzui_core::metrics::FiniteNonNeg::new_or(radius, 0.0).get();
        self
    }

    /// How far each row's visible pill stands in from the row's sides: the
    /// outer gutter of an [`enz_list_row_surface`](crate::list_row_surface)
    /// or sidebar row. The focus ring and a lifted row's plate stand in by
    /// the same amount, so they hug the pill instead of showing a band either
    /// side of it (default 0).
    pub fn row_inset(mut self, inset: f32) -> Self {
        self.row_inset = enzui_core::metrics::FiniteNonNeg::new_or(inset, 0.0).get();
        self
    }

    /// The drag feedback's insertion-bar and outline width, empty-target
    /// outline radius and wash, and lifted-row elevation, as a fixed
    /// snapshot; the colours keep following the live theme. Pass a hosted
    /// list the same value as its host. See [`DragFeedbackTokens`].
    pub fn feedback_tokens(mut self, tokens: DragFeedbackTokens) -> Self {
        self.feedback = tokens.sanitized();
        self
    }

    /// Set the deadband (minimum drag distance in px before a press becomes a
    /// drag).
    pub fn deadband(mut self, px: f32) -> Self {
        self.deadband = px;
        self
    }

    /// Override the collection ID (default `"reorder-list"`).
    pub fn collection_id(mut self, id: impl Into<String>) -> Self {
        self.collection_id = id.into();
        self
    }

    /// Observe the [`DragSession`] pipeline. `f` fires per pipeline milestone
    /// (begin/activation/negotiation/termination) with session-read
    /// [`DragObservation`]s: pipeline-own states observable to `update`
    /// (live-preview/drop-cue UIs; session-reaching wiring pins in tests).
    pub fn on_drag_event(mut self, f: impl Fn(DragObservation) -> Message + 'a) -> Self {
        self.on_drag_event = Some(Box::new(f));
        self
    }

    /// Allow reorder gestures to begin on interactive child rows.
    ///
    /// A child still owns an ordinary click. Once pointer travel crosses this
    /// list's deadband, the list owns the gesture and consumes the terminal
    /// release so the child's navigation/action is not fired after a reorder.
    /// The default is `false` for backward compatibility.
    pub fn drag_through_children(mut self, enabled: bool) -> Self {
        self.drag_through_children = enabled;
        self
    }

    /// Drag rows only by their handles, for composite rows: a group whose
    /// header drags the group while its nested rows, buttons and drag sources
    /// keep their own gestures.
    ///
    /// Wrap the dragging part of each row in [`reorder_handle`]. A press
    /// there works as with [`drag_through_children`](Self::drag_through_children):
    /// a release inside the deadband is the handle's own click, travel past
    /// it drags the whole row with the usual lift, gap and settle. A press
    /// anywhere else in the row is left to the row's content; the list
    /// neither selects nor drags. The default is `false`: the whole row drags.
    pub fn drag_handles(mut self, enabled: bool) -> Self {
        self.drag_handles = enabled;
        self
    }

    /// Set what the drag source permits (default
    /// [`SourceCapabilities::move_only`]).
    ///
    /// A source that permits nothing (or not the operation the destination
    /// proposes) makes [`DragSession`] negotiate [`DragOperation::Forbidden`]
    /// and refuse the drop — no [`DropRequest`] is published. This is the
    /// negotiation path a trivial "always drop" implementation cannot express.
    pub fn source_capabilities(mut self, caps: SourceCapabilities) -> Self {
        self.source_capabilities = caps;
        self
    }

    /// Set the operations the drop destination proposes, most-preferred first
    /// (default `[DragOperation::Move]`).
    ///
    /// Negotiated against [`source_capabilities`](Self::source_capabilities)
    /// inside [`DragSession::hover`].
    pub fn destination_operations(mut self, ops: Vec<DragOperation>) -> Self {
        self.destination_operations = ops;
        self
    }

    /// Join an [`EnzDragDropHost`](crate::drag_drop_host::EnzDragDropHost)
    /// through `channel`. Hosted lists REGISTER collection ids + row geometry
    /// per event pass then DEFER: zero own sessions/requests (cross-collection
    /// moves = one atomic request = exactly one publisher: hosts). Same
    /// channels to hosts + every participating list. Deferral gates on host-
    /// claimed channels this pass: hostless-channeled lists keep standalone
    /// paths untouched.
    pub fn drag_host(mut self, channel: &DragHostChannel) -> Self {
        self.drag_host = Some(channel.clone());
        self
    }

    /// Declare the payload kinds a drag out of this list offers (default
    /// [`PayloadKinds::universal`]). Hosted path only.
    pub fn offering(mut self, kinds: PayloadKinds) -> Self {
        self.offering = kinds;
        self
    }

    /// Declare the payload kinds this list accepts as a destination
    /// (default [`PayloadKinds::universal`]).
    ///
    /// An empty set accepts nothing and refuses every incoming drag —
    /// expressible intent honoured literally. Hosted path only: within a
    /// single collection the source and destination are the same widget, so
    /// the gate had nothing to refuse.
    pub fn accepting(mut self, kinds: PayloadKinds) -> Self {
        self.accepting = kinds;
        self
    }

    /// Set the widget width (default [`Length::Fill`]).
    pub fn width(mut self, w: Length) -> Self {
        self.width = w;
        self
    }

    /// Set the widget height (default [`Length::Shrink`]).
    pub fn height(mut self, h: Length) -> Self {
        self.height = h;
        self
    }

    /// Vertical spacing between items in px (default 0).
    ///
    /// Normalized to finite non-negative at the boundary: the spacing is
    /// summed into the layout `main` axis (`main += self.spacing`) and the
    /// position cursor, so a NaN / ±inf / negative value would poison the whole
    /// intrinsic `Size`. `0` is expressible intent ("no gap") and is honoured
    /// literally; only inexpressible input degrades to the `0` default.
    pub fn spacing(mut self, px: f32) -> Self {
        self.spacing = enzui_core::metrics::FiniteNonNeg::new_or(px, 0.0).get();
        self
    }

    /// Opt out of the duplicate-key `debug_assert`.
    ///
    /// Opt out of the duplicate-key `debug_assert`. Reorder lists key rows by
    /// `ItemId`; keyed reconciliation preserves retained row states across
    /// reorders only on unique keys (duplicate keys reconcile positionally —
    /// exactly keying-removed bugs, runtime-invisible). `diff()`
    /// `debug_assert`s uniqueness. Call on deliberate duplications; release
    /// behaviour unchanged (defined occurrence orders — see [`keyed_diff`](crate::keyed_diff)).
    pub fn allow_duplicate_keys(mut self) -> Self {
        self.allow_duplicate_keys = true;
        self
    }

    /// Keep the selection for rows that vanish from `children`. Default
    /// DELETION: vanished keys drop from selections (+ range-anchor clears),
    /// so later-reinserted same ids start UNSELECTED. Otherwise removed ids
    /// hide inside [`selected_in_order`](ListSelection::selected_in_order) (filtering vs
    /// handed orders) while `len()`/`contains()` keep counting — silent
    /// reinsertion-resurrections + never-set stale-anchor revivals. Set for
    /// FILTERED (not deleted) rows — narrowing search boxes etc.: model-alive
    /// items; forgetting loses real user intent on filter-clears.
    /// [`ListSelection`] can't distinguish; callers declare here.
    pub fn selection_persists_when_filtered(mut self) -> Self {
        self.selection_persists_when_filtered = true;
        self
    }

    /// Set the stacking / reorder axis (default
    /// [`Orientation::Vertical`]).
    pub fn orientation(mut self, orientation: Orientation) -> Self {
        self.orientation = orientation;
        self
    }

    /// Shorthand for [`.orientation(Orientation::Horizontal)`](Self::orientation).
    ///
    /// A horizontal list lays its children out in a row and decides the drag
    /// insertion slot along the x axis, honouring the writing
    /// [`Direction`] — under RTL the midpoint rule is mirrored, so dragging an
    /// item left of a neighbour's centre inserts it *after* that neighbour in
    /// layout order.
    pub fn horizontal(mut self) -> Self {
        self.orientation = Orientation::Horizontal;
        self
    }

    /// Report the selected keys, in list order, whenever a click, a
    /// Ctrl-click or a Shift-click changes them, or a removed item leaves
    /// the selection. What the app needs to act on the selection: delete or
    /// extract the selected pages, say.
    pub fn on_selection_change(mut self, f: impl Fn(Vec<String>) -> Message + 'a) -> Self {
        self.on_selection_change = Some(Box::new(f));
        self
    }

    /// Lay the items out as a grid of `columns` equal columns, filled row by
    /// row, with [`spacing`](Self::spacing) between cells both ways: a page
    /// sorter, a photo strip that wraps. A drag opens its gap cell by cell,
    /// the dragged items follow the pointer freely, and the keyboard moves
    /// along a row with Left and Right and between rows with Up and Down.
    /// Each item is laid out at the column's width and its own height; a row
    /// is as tall as its tallest item.
    pub fn grid(mut self, columns: usize) -> Self {
        self.columns = Some(columns.max(1));
        self
    }

    /// Override the writing [`Direction`] used for the horizontal axis.
    ///
    /// Defaults to the ambient locale direction
    /// ([`resolve_direction(None)`](enzui_core::placement::resolve_direction)).
    /// Passing `Direction::RightToLeft` mirrors the horizontal midpoint rule;
    /// ignored for a vertical list.
    pub fn direction(mut self, direction: Direction) -> Self {
        self.direction = Some(direction);
        self
    }

    // -- internal helpers ------------------------------------------------

    /// The ordered [`ItemId`]s of the current children.
    fn order(&self) -> Vec<ItemId> {
        self.keys.iter().map(|k| ItemId::new(k.clone())).collect()
    }

    /// The index of the row whose full-extent hit rect contains `position`.
    ///
    /// The hit area spans the list on the cross axis because children may
    /// be smaller than the list on that axis (a shrink-to-fit label in a wide
    /// list is grabbed anywhere on its row).
    /// In a grid only the item's own bounds take the press.
    fn row_at(
        &self,
        position: Point,
        bounds: Rectangle,
        child_layouts: impl IntoIterator<Item = Layout>,
    ) -> Option<usize> {
        child_layouts.into_iter().position(|cl| {
            let cb = cl.bounds();
            // A grid cell is grabbed only on itself: its row holds others.
            if self.columns.is_some() {
                return cb.contains(position);
            }
            let row_hit = match self.orientation {
                Orientation::Vertical => Rectangle {
                    x: bounds.x,
                    y: cb.y,
                    width: bounds.width,
                    height: cb.height,
                },
                Orientation::Horizontal => Rectangle {
                    x: cb.x,
                    y: bounds.y,
                    width: cb.width,
                    height: bounds.height,
                },
            };
            row_hit.contains(position)
        })
    }

    /// Re-negotiate the drop destination from the current cursor position
    /// and the current row geometry.
    ///
    /// # Why this takes the `cursor`, not a `CursorMoved` payload
    ///
    /// Coordinate-space fixes (actual stale-slot-defect causes, not missing
    /// per-frame calls). `CursorMoved` `position`s are untranslated
    /// window/screen coordinates; `child_layouts` bounds are CONTENT-space
    /// (enclosing `scrollable`s lay contents at scrolled positions, shifting
    /// *cursor arguments* — `iced/widget/src/scrollable.rs` —
    /// leaving raw event payloads untouched). Untranslated-payload vs
    /// translated-bounds comparisons mix coordinate systems: slots wrong by
    /// exactly scroll offsets, unfixable by recompute frequency. Measured on
    /// `dragautoscroll` routes at 149px offsets: payloads `y = 309.6` vs cursor
    /// `y = 458.6` (= 309.6 + 149), row 0 at `y = 131.6` — payload-recomputes
    /// slot 4 pre/post-scroll; cursor-recomputes point at pointer rows. Press
    /// paths already `cursor.position_over(bounds)`-used: asymmetries removed
    /// (same-space press+move resolution). Construction-idempotent
    /// ([`hover`](DragSession::hover) scratch-re-negotiates per call): pointer+frame-path
    /// calls never double-apply.
    fn refresh_destination(
        &self,
        session: &mut DragSession,
        cursor: mouse::Cursor,
        child_layouts: &[Layout],
        shell: &mut Shell<'_, Message>,
    ) where
        Message: Clone,
    {
        // No cursor (it left the window, or the runtime has none) means no
        // destination to compute. Leave the last negotiated proposal alone
        // rather than inventing one from a position we do not have.
        let Some(position) = cursor.position() else {
            return;
        };

        if self.columns.is_some() {
            let cells: Vec<(f32, f32, f32, f32)> = child_layouts
                .iter()
                .map(|cl| {
                    let b = cl.bounds();
                    (b.x, b.y, b.width, b.height)
                })
                .collect();
            let slot = enzui_core::drag::slot_for_position_2d_with(
                (position.x, position.y),
                &cells,
                resolve_direction(self.direction),
            );
            let proposal = DestinationProposal::new(
                CollectionId::new(&self.collection_id),
                slot,
                self.destination_operations.clone(),
            );
            let op = session.hover(Some(proposal));
            if let Some(observe) = self.on_drag_event.as_ref() {
                shell.publish(observe(DragObservation::Negotiated(op)));
            }
            return;
        }

        // Compute the insertion slot from child layout geometry along the
        // stacking axis. Children are already in layout order, which is the
        // order `slot_for_position_1d_with` expects under either writing
        // direction.
        let (axis_pos, items): (f32, Vec<(f32, f32)>) = match self.orientation {
            Orientation::Vertical => (
                position.y,
                child_layouts
                    .iter()
                    .map(|cl| {
                        let b = cl.bounds();
                        (b.y, b.height)
                    })
                    .collect(),
            ),
            Orientation::Horizontal => (
                position.x,
                child_layouts
                    .iter()
                    .map(|cl| {
                        let b = cl.bounds();
                        (b.x, b.width)
                    })
                    .collect(),
            ),
        };

        // The vertical axis is always LTR (top-to-bottom); only the horizontal
        // axis is writing-direction sensitive.
        let direction = match self.orientation {
            Orientation::Vertical => Direction::LeftToRight,
            Orientation::Horizontal => resolve_direction(self.direction),
        };

        let slot = slot_for_position_1d_with(axis_pos, &items, direction);

        // Propose the configured operations into this collection at the
        // computed slot; the session negotiates them against the source
        // capabilities.
        let proposal = DestinationProposal::new(
            CollectionId::new(&self.collection_id),
            slot,
            self.destination_operations.clone(),
        );
        let op = session.hover(Some(proposal));

        // Observe the negotiated operation the session now advertises after
        // this hover.
        if let Some(observe) = self.on_drag_event.as_ref() {
            shell.publish(observe(DragObservation::Negotiated(op)));
        }
    }

    /// Apply a press on row `i` to the selection and return the ids a drag
    /// begun from that press should carry, in list order. Standalone+hosted
    /// shared paths: identical selection semantics from one implementation
    /// (hosted divergences once made `.drag_host(...)` multi-item-drag
    /// unproducible). Two rules: plain presses on already-selected rows don't
    /// click-apply (press-time collapses destroy pre-drag multi-item payloads;
    /// `pending_collapse`-recorded, release-performed iff dragless); payloads =
    /// whole selections on selected-row presses, single pressed rows otherwise.
    fn apply_press_to_selection(&self, i: usize, state: &mut ReorderState) -> Vec<ItemId> {
        let pressed_id = ItemId::new(self.keys[i].clone());
        let order = self.order();

        // Reconstruct the click's meaning from the tracked modifiers. Cmd on
        // macOS / Ctrl elsewhere is the toggle modifier; Shift is range-extend.
        let mods = ClickModifiers {
            toggle: state.modifiers.command(),
            range: state.modifiers.shift(),
        };
        let is_plain = !mods.toggle && !mods.range;
        let already_selected = state.selection.contains(&pressed_id);

        if is_plain && already_selected {
            // Defer the collapse to release. Record which row, so a
            // release can collapse to exactly the row the user pressed.
            state.pending_collapse = Some(pressed_id.clone());
        } else {
            state.pending_collapse = None;
            let model = ClickModel::new(order.clone());
            state.selection.click(&model, &pressed_id, mods);
        }

        if state.selection.contains(&pressed_id) {
            state.selection.selected_in_order(&order)
        } else {
            vec![pressed_id]
        }
    }

    // Rectangles come from this pass's `child_layouts`, the space the host
    // hit-tests in. Press (drag source), move (destination) and release
    // (revalidation) all consult the registry, so all three must register.
    fn register_hosted(
        &self,
        channel: &DragHostChannel,
        bounds: Rectangle,
        child_layouts: &[Layout],
        state: &ReorderState,
        handles: Option<&[Vec<Rectangle>]>,
    ) {
        channel.register(CollectionRegistration {
            collection: CollectionId::new(&self.collection_id),
            bounds,
            items: self
                .keys
                .iter()
                .zip(child_layouts.iter())
                .map(|(key, cl)| {
                    let cb = cl.bounds();
                    // Full-extent hit row on the cross axis, matching the
                    // standalone path.
                    (
                        ItemId::new(key.clone()),
                        match self.orientation {
                            Orientation::Vertical => Rectangle {
                                x: bounds.x,
                                y: cb.y,
                                width: bounds.width,
                                height: cb.height,
                            },
                            Orientation::Horizontal => Rectangle {
                                x: cb.x,
                                y: bounds.y,
                                width: cb.width,
                                height: bounds.height,
                            },
                        },
                    )
                })
                .collect(),
            axis: match self.orientation {
                Orientation::Vertical => CollectionAxis::Vertical,
                Orientation::Horizontal => CollectionAxis::Horizontal,
            },
            direction: match self.orientation {
                Orientation::Vertical => Direction::LeftToRight,
                Orientation::Horizontal => resolve_direction(self.direction),
            },
            source_capabilities: self.source_capabilities,
            offering: self.offering.clone(),
            accepting: self.accepting.clone(),
            destination_operations: self.destination_operations.clone(),
            // the selection, in list order, so the host can begin a multi-item
            // session instead of `vec![item]`.
            selected: state.selection.selected_in_order(&self.order()),
            grab_regions: handles.map(<[Vec<Rectangle>]>::to_vec),
        });
    }

    /// Handle the events that must end a drag, returning whether the event
    /// was terminal (and therefore fully consumed by this function).
    ///
    /// Called above the `is_event_captured()` gate and above the hosted defer.
    /// A drag begun by this widget must be ended by this widget even
    /// when a child consumes the event that ends it, and even when the pointer
    /// or the window is lost with no such event at all.
    fn handle_terminal_event(
        &self,
        event: &Event,
        state: &mut ReorderState,
        shell: &mut Shell<'_, Message>,
        hosted: bool,
    ) -> bool
    where
        Message: Clone,
    {
        match event {
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                // Releases discharge pending collapses + live sessions whether
                // children consumed events or not. "Did drags happen?" has two
                // answers (two session owners). Standalone: own left-`Pending`
                // sessions. Hosted: zero held sessions (hosts own cross-sibling
                // singles) — `state.session` reads always-`false` there, so
                // reading it would make every hosted release look
                // click-without-drag, discharging deferred collapses and
                // destroying just-dragged multi-selections. Hosts
                // publish own phases into channels at `begin_pass` (pre-us runs):
                // truthful both-path answers.
                let dragged = if hosted {
                    self.drag_host
                        .as_ref()
                        .is_some_and(DragHostChannel::host_dragged)
                } else {
                    state
                        .session
                        .as_ref()
                        .is_some_and(|s| s.phase() != DragPhase::Pending)
                };

                if let Some(session) = state.session.as_mut() {
                    // Hosted lists run no session of their own; the host owns
                    // it. (Defensive: `state.session` is None on that path.)
                    if let Some(outcome) = session.release() {
                        match outcome {
                            SessionOutcome::Dropped(req) => {
                                if let Some(observe) = self.on_drag_event.as_ref() {
                                    shell.publish(observe(DragObservation::Outcome(Some(
                                        req.operation,
                                    ))));
                                }
                                shell.publish((self.on_drop)(*req));
                            }
                            SessionOutcome::Cancelled => {
                                if let Some(observe) = self.on_drag_event.as_ref() {
                                    shell.publish(observe(DragObservation::Outcome(None)));
                                }
                            }
                        }
                    }
                    state.session = None;
                    state.dragged_index = None;
                    shell.request_redraw();
                }

                // DEFERRED COLLAPSE rationale. Presses preserve multi-selections
                // for potential drags; dragless gestures were plain clicks on
                // selected rows (every file manager collapses to exactly those).
                // Press-time collapsing destroys drag payloads; post-real-drag
                // firing clears just-moved selections. So: releases, only when
                // sessions never left `Pending`.
                if let Some(pending) = state.pending_collapse.take()
                    && !dragged
                {
                    let order = self.order();
                    let model = ClickModel::new(order);
                    if state
                        .selection
                        .click(&model, &pending, ClickModifiers::PLAIN)
                    {
                        shell.request_redraw();
                    }
                }
                true
            }

            Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                ..
            }) => {
                // Escape abandons the gesture entirely: no drop, and no
                // deferred collapse either (the user retracted the click).
                state.pending_collapse = None;
                if let Some(session) = state.session.as_mut()
                    && session.is_active()
                {
                    let outcome = session.cancel();
                    if matches!(outcome, Some(SessionOutcome::Cancelled))
                        && let Some(observe) = self.on_drag_event.as_ref()
                    {
                        shell.publish(observe(DragObservation::Outcome(None)));
                    }
                    state.session = None;
                    state.dragged_index = None;
                    shell.capture_event();
                    shell.request_redraw();
                    return true;
                }
                // Not our Escape: let the rest of the widget (and siblings)
                // see it.
                false
            }

            // Pointer/window-mid-drag losses end sessions. Normal-ending
            // releases deliver to outside-window pointer owners: armless sessions
            // go immortal (last negotiated proposals kept, apps believing pending
            // drops, second sessions stacking on next presses). CANCEL (never
            // commit): `image_viewer`-committing pans apply incrementally with
            // app-owned states; reorders publish nothing pre-release — zero
            // keepable committed states; completing user-aborted-by-leaving
            // drops invents intent.
            Event::Mouse(mouse::Event::CursorLeft)
            | Event::Window(iced::window::Event::Unfocused) => {
                state.pending_collapse = None;
                if let Some(session) = state.session.as_mut() {
                    let was_active = session.is_active();
                    let outcome = session.cancel();
                    if was_active
                        && matches!(outcome, Some(SessionOutcome::Cancelled))
                        && let Some(observe) = self.on_drag_event.as_ref()
                    {
                        shell.publish(observe(DragObservation::Outcome(None)));
                    }
                    state.session = None;
                    state.dragged_index = None;
                    shell.request_redraw();
                }
                // Not "consumed": a hosted list must still fall through so the
                // host sees the same loss, and siblings need it too. Returning
                // false is safe because no arm below acts on these events.
                let _ = hosted;
                false
            }

            _ => false,
        }
    }

    /// Handle the keyboard reorder path: `Ctrl+Up` / `Ctrl+Down`
    /// moves the focused item one position, routed through the *same*
    /// [`DragSession`]/[`negotiate`](enzui_core::drag::negotiate) pipeline
    /// the pointer path uses. Returns `true` iff the event was a reorder key
    /// this widget consumed.
    ///
    /// # The pipeline is the policy
    ///
    /// Never direct order mutations. Sessions begin → activate → hover
    /// destination slots → release: pointer-path-identical four steps. Refused
    /// [`negotiate`](enzui_core::drag::negotiate)s (incompatible source/destination) cancel
    /// releases publishing nothing: keyboard refusals == pointer refusals.
    /// Independently-mutating dual routes let policies diverge.
    ///
    /// # Focus retention
    ///
    /// [`focused`](ReorderState::focused)s are [`ItemId`]s: moved items keep focus
    /// across reorders (next `Ctrl+Up`/`Ctrl+Down` acts on same rows).
    /// Pure-index focus stays at old slots: unusable past first presses.
    ///
    /// # Boundary is a refusal, not a clamp
    ///
    /// First-`Ctrl+Up`/last-`Ctrl+Down`s have no destination slots:
    /// nothing-hovering sessions ([`hover(None)`](DragSession::hover) negotiates
    /// [`Forbidden`](DragOperation::Forbidden)) cancel releases. Refusals observable in
    /// identical `DragObservation` phases as all refused drags: items unmoved,
    /// zero [`DropRequest`]s.
    fn handle_keyboard_reorder(
        &self,
        event: &Event,
        state: &mut ReorderState,
        shell: &mut Shell<'_, Message>,
    ) -> bool
    where
        Message: Clone,
    {
        // Only Ctrl+Up / Ctrl+Down are reorder keys. Plain arrows keep moving
        // focus (below), never items.
        let Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key: iced::keyboard::Key::Named(named),
            modifiers,
            ..
        }) = event
        else {
            return false;
        };
        let named = *named;

        // An empty or single-item list is a real, expressible case: the
        // focus arrows and the reorder keys simply have nothing to do. Honour
        // it literally rather than degrading.
        if self.children.is_empty() {
            return false;
        }

        let order = self.order();

        // Plain (unmodified) Up/Down move FOCUS, not items. This is the
        // affordance the reorder keys build on: the focused item is what
        // Ctrl+Up/Down acts on, and it must be movable without moving anything.
        let is_plain =
            !modifiers.command() && !modifiers.control() && !modifiers.shift() && !modifiers.alt();
        let step = self.key_step(named);
        if is_plain && let Some(step) = step {
            let cur = self.focused_index(state, &order);
            let next = cur.saturating_add_signed(step).min(order.len() - 1);
            state.focused = Some(order[next].clone());
            shell.capture_event();
            shell.request_redraw();
            return true;
        }

        // Ctrl (Cmd on macOS) + Up/Down is the reorder gesture.
        let is_reorder_mod = modifiers.command() || modifiers.control();
        if !is_reorder_mod {
            return false;
        }
        let Some(delta) = step else {
            return false;
        };

        let from = self.focused_index(state, &order);
        let focused_id = order[from].clone();
        // Keep focus on the item regardless of whether the move succeeds.
        state.focused = Some(focused_id.clone());

        // The destination slot, in the `slot_for_position_1d_with` convention:
        // a slot is a gap `0..=len`. Moving an item at index `from` up by one
        // targets slot `from - 1`; down by one targets slot `from + 2` (the gap
        // *after* the successor, because the item itself still occupies a slot
        // when the gap is counted). A boundary move has no in-range slot.
        // A grid moves by a whole row too: the item lands at index `to`,
        // which is slot `to` going back and `to + 1` going forward. Past
        // either end there is no slot: a boundary refusal.
        let to = from
            .checked_add_signed(delta)
            .filter(|&to| to < order.len());
        let target_slot: Option<usize> = to.map(|to| if to < from { to } else { to + 1 });

        shell.capture_event();
        shell.request_redraw();

        // Route through the same pipeline the pointer path uses. Origin and a
        // synthetic activation move keep the session honest: it must cross the
        // deadband to become Active, exactly like a pointer drag, before hover
        // is honoured.
        let mut session = DragSession::begin(
            CollectionId::new(&self.collection_id),
            vec![focused_id.clone()],
            self.source_capabilities,
            (0.0, 0.0),
            0.0,
        );
        if let Some(observe) = self.on_drag_event.as_ref() {
            shell.publish(observe(DragObservation::Began(session.phase())));
        }
        // Activate: a finite non-zero move past a zero deadband. This is the
        // keyboard analogue of crossing the pointer deadband.
        let became_active = session.pointer_moved((1.0, 0.0));
        if became_active && let Some(observe) = self.on_drag_event.as_ref() {
            shell.publish(observe(DragObservation::Activated(session.phase())));
        }

        // Hover the destination — `Some(slot)` for an in-range move, `None` at a
        // boundary. `hover(None)` negotiates `Forbidden`, so a boundary move
        // refuses through the pipeline rather than being clamped silently.
        let proposal = target_slot.map(|slot| {
            DestinationProposal::new(
                CollectionId::new(&self.collection_id),
                InsertionSlot(slot),
                self.destination_operations.clone(),
            )
        });
        let op = session.hover(proposal);
        if let Some(observe) = self.on_drag_event.as_ref() {
            shell.publish(observe(DragObservation::Negotiated(op)));
        }

        // Release: the pipeline decides. A `Dropped` publishes the request and
        // announces; a `Cancelled` (boundary, or a refused negotiation) does
        // neither — the item does not move.
        match session.release() {
            Some(SessionOutcome::Dropped(req)) => {
                if let Some(observe) = self.on_drag_event.as_ref() {
                    shell.publish(observe(DragObservation::Outcome(Some(req.operation))));
                }
                // Announce the new position for a screen-reader user.
                // The move is not yet applied to `self.children` (the app
                // resolves the `DropRequest` in `update`), so compute the
                // destination index the drop will land the item at.
                let dest_index = to.unwrap_or(from);
                state.announcement =
                    Some(reorder_announcement(&focused_id, dest_index, order.len()));
                shell.publish((self.on_drop)(*req));
            }
            Some(SessionOutcome::Cancelled) | None => {
                if let Some(observe) = self.on_drag_event.as_ref() {
                    shell.publish(observe(DragObservation::Outcome(None)));
                }
            }
        }
        true
    }

    /// The index of the currently-focused item, seeding focus if it is unset or
    /// stale (its id no longer present). Never panics: the caller guarantees a
    /// non-empty list.
    /// How far an arrow key moves the focus or the focused item: one item
    /// along the list, and in a grid one row for Up and Down. `None` for a
    /// key that does not move along this list.
    fn key_step(&self, named: iced::keyboard::key::Named) -> Option<isize> {
        use iced::keyboard::key::Named;
        let row = self
            .columns
            .map_or(1, |columns| isize::try_from(columns).unwrap_or(isize::MAX));
        let forward = match resolve_direction(self.direction) {
            Direction::LeftToRight => 1,
            Direction::RightToLeft => -1,
        };
        match (named, self.columns) {
            (Named::ArrowUp, _) => Some(-row),
            (Named::ArrowDown, _) => Some(row),
            (Named::ArrowLeft, Some(_)) => Some(-forward),
            (Named::ArrowRight, Some(_)) => Some(forward),
            _ => None,
        }
    }

    /// Lay the items out in `columns` equal columns, row by row.
    fn layout_grid(
        &mut self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
        columns: usize,
    ) {
        let limits = limits.width(self.width).height(self.height);
        let max = limits.max();
        let spacing = self.spacing.max(0.0);
        let gaps = spacing * (columns - 1) as f32;
        let cell_width = if max.width.is_finite() {
            ((max.width - gaps) / columns as f32).max(0.0)
        } else {
            f32::INFINITY
        };
        let rtl = resolve_direction(self.direction) == Direction::RightToLeft;
        let cell_limits =
            layout::Limits::new(iced::Size::ZERO, iced::Size::new(cell_width, f32::INFINITY));
        let mut nodes: Vec<layout::Node> = self
            .children
            .iter_mut()
            .zip(tree.children.iter_mut())
            .map(|(child, child_tree)| {
                use iced::advanced::layout::Measure as _;
                child
                    .as_widget_mut()
                    .measure(child_tree, renderer, &cell_limits)
            })
            .collect();
        // An unbounded parent gives each column its widest item.
        let pitch = if cell_width.is_finite() {
            cell_width
        } else {
            nodes
                .iter()
                .map(|node| node.size().width)
                .fold(0.0, f32::max)
        };
        let width = pitch * columns as f32 + gaps;
        let mut y = 0.0;
        for row in nodes.chunks_mut(columns) {
            let height = row
                .iter()
                .map(|node| node.size().height)
                .fold(0.0, f32::max);
            for (column, node) in row.iter_mut().enumerate() {
                let column = if rtl { columns - 1 - column } else { column };
                node.move_to_mut(iced::Point::new(column as f32 * (pitch + spacing), y));
            }
            y += height + spacing;
        }
        let height = (y - spacing).max(0.0);
        let size = limits.resolve(
            Length::Shrink,
            Length::Shrink,
            iced::Size::new(width, height),
        );
        layout::Node::with_children(size, nodes).apply(tree);
    }

    /// The keyboard focus ring around the focused row, shown `offset` from
    /// its laid-out place. It follows the row's radius at the theme's ring
    /// width and outset, like every other control's ring.
    #[allow(
        clippy::too_many_arguments,
        reason = "the draw arguments iced passes, plus the ring's state"
    )]
    fn draw_focus_ring(
        &self,
        state: &ReorderState,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        layout: Layout,
        tree: &Tree,
        viewport: &Rectangle,
        offset: impl Fn(usize) -> Vector,
    ) {
        use crate::focus_button::FocusRingTheme as _;
        if !state.keyboard_owner.focus.is_focused()
            || self.children.is_empty()
            || state.pointer_input
        {
            return;
        }
        let index = self.focused_index(state, &self.order());
        let Some(row) = layout
            .children(&tree.children)
            .nth(index)
            .map(|l| l.bounds())
        else {
            return;
        };
        if row.intersection(viewport).is_none() {
            return;
        }
        let width = theme.enz_focus_ring_width();
        crate::focus_button::paint_focus_ring(
            renderer,
            self.pill(row) + offset(index),
            crate::focus_button::FocusRingQuad {
                ink: theme.enz_focus_ring_color(None),
                width,
                radius: (self.row_radius + width).into(),
                outset: width,
                snap: true,
            },
        );
    }

    /// A row's visible pill: its bounds less the row inset on the
    /// cross-axis sides.
    pub(crate) fn pill(&self, row: Rectangle) -> Rectangle {
        let inset = self.row_inset.min(row.width / 2.0).min(row.height / 2.0);
        match self.orientation {
            Orientation::Vertical => Rectangle {
                x: row.x + inset,
                width: row.width - 2.0 * inset,
                ..row
            },
            Orientation::Horizontal => Rectangle {
                y: row.y + inset,
                height: row.height - 2.0 * inset,
                ..row
            },
        }
    }

    fn focused_index(&self, state: &ReorderState, order: &[ItemId]) -> usize {
        if let Some(id) = state.focused.as_ref()
            && let Some(i) = order.iter().position(|o| o == id)
        {
            return i;
        }
        // Seed to the selection's first row, else the first row.
        let selected = state.selection.selected_in_order(order);
        selected
            .first()
            .and_then(|id| order.iter().position(|o| o == id))
            .unwrap_or(0)
    }
}

/// Build the live-region announcement string for a keyboard reorder.
///
/// One-based position for the screen reader, mirroring `set_position` / ARIA
/// `aria-posinset` conventions — a user hears "moved to position 2 of 3", not
/// "index 1".
/// The extent along this list's axis of the rows `live` drags in from
/// another collection, when that collection stacks the same way. Carried by
/// the drag itself, so no paint-time reader needs the per-pass registry.
pub(crate) fn incoming_extent(live: &LiveDrag, vertical: bool) -> Option<f32> {
    let (axis, size) = live.extent?;
    let same_axis = matches!(
        (axis, vertical),
        (CollectionAxis::Vertical, true) | (CollectionAxis::Horizontal, false)
    );
    let extent = if vertical { size.height } else { size.width };
    (same_axis && extent > 0.0).then_some(extent)
}

fn reorder_announcement(id: &ItemId, dest_index: usize, len: usize) -> String {
    enzui_core::localized!("enzui-reorder-list-moved-to-position-of", "item" => id.as_str(), "position" => dest_index + 1, "count" => len)
}

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct KeyboardOwner {
    id: iced::advanced::widget::Id,
    focus: enzui_core::focus::FocusState,
}

impl Default for KeyboardOwner {
    fn default() -> Self {
        Self {
            id: iced::advanced::widget::Id::unique(),
            focus: Default::default(),
        }
    }
}

#[derive(Default)]
struct FocusProbe {
    focused: bool,
    /// Whether the probed subtree exposes any focusable at all.
    any: bool,
}

impl Operation for FocusProbe {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn focusable(
        &mut self,
        _id: Option<&iced::advanced::widget::Id>,
        _bounds: Rectangle,
        state: &mut dyn iced::advanced::widget::operation::Focusable,
    ) {
        self.focused |= state.is_focused();
        self.any = true;
    }
}

struct FocusFirst {
    take: bool,
}

impl Operation for FocusFirst {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn focusable(
        &mut self,
        _id: Option<&iced::advanced::widget::Id>,
        _bounds: Rectangle,
        state: &mut dyn iced::advanced::widget::operation::Focusable,
    ) {
        if self.take {
            state.focus();
            self.take = false;
        } else {
            state.unfocus();
        }
    }
}

#[derive(Debug, Default)]
struct ReorderState {
    keyboard_owner: KeyboardOwner,
    /// The active drag session, if any.
    session: Option<DragSession>,
    /// Index of the child being dragged.
    dragged_index: Option<usize>,
    /// The keys at the time the drag started (for building the DropRequest).
    keys: Vec<String>,
    /// Multi-item selection state. A press feeds into this with the
    /// modifiers observed from the event stream, and the drag payload is the
    /// selection (when the pressed row is part of it) rather than a single
    /// hard-coded item — which is what makes multi-item drag *producible* at
    /// runtime instead of merely expressible in the type.
    selection: ListSelection,
    /// The last keyboard modifier state seen on the event stream. Mouse
    /// `ButtonPressed`s carry no modifiers: out-of-band tracking from
    /// `ModifiersChanged` (+ modifier-carrying keyboard press/releases).
    /// Focus-loss-sound: winit/iced emits `ModifiersChanged(empty)` on window
    /// focus loss — focus-lost-held Ctrls never linger as next-click phantom
    /// modifiers.
    modifiers: iced::keyboard::Modifiers,
    /// The row a *plain* press deferred its selection-collapse for. Press-time
    /// collapses on already-multi-selected rows would shrink drags to single
    /// items (vs user-visible selections): releases perform, dragless-only —
    /// the entire deferral point. Every-terminal-event-cleared: never later-
    /// gesture-leaking.
    pending_collapse: Option<ItemId>,
    /// The selection the app was last told about through
    /// [`EnzReorderList::on_selection_change`].
    published_selection: Vec<ItemId>,
    /// The row keys, in order, as of the last `diff()` — the identities the
    /// child `Tree`s in `Tree::children` are currently attached to. Keyed (never
    /// slot-indexed) old→new `Tree` reconciliation: retained row states (focus,
    /// editors, overlays, animations) follow ITEMS across reorders, never fixed
    /// positions. Pre-first-`diff()` empty.
    child_keys: Vec<String>,
    /// Where the first lifted row was laid out when this list's drag began:
    /// a collection reflowing above this one (a destination opening its gap)
    /// moves the row's layout, not where the user grabbed it (#3675).
    lift_anchor: std::cell::RefCell<Option<(String, iced::Point)>>,
    // Item identity survives keyed reordering; it is not permission to receive keyboard input.
    focused: Option<ItemId>,
    /// The most recent keyboard-reorder announcement, read in
    /// `operate()` to emit a `Role::Status` live region so a screen-reader
    /// user hears the new position of a silently-reordered item. Set only by
    /// the keyboard path; the pointer path already gives visual feedback.
    announcement: Option<String>,
    /// Live drag feedback: where each row is shown while it glides.
    motion: feedback::RowMotion,
    /// Whether the last input this list saw was a pointer press: its focus
    /// ring then stays hidden, as focus-visible does on the web.
    pointer_input: bool,
}

// ---------------------------------------------------------------------------
// Widget impl
// ---------------------------------------------------------------------------

impl<'a, Message> EnzReorderList<'a, Message>
where
    Message: Clone + 'a,
{
    /// Tell the app the selection when it differs from what it was last
    /// told, in list order.
    fn publish_selection(&self, tree: &mut Tree, shell: &mut Shell<'_, Message>) {
        let Some(on_change) = self.on_selection_change.as_ref() else {
            return;
        };
        let state = tree.state.downcast_mut::<ReorderState>();
        let selected = state.selection.selected_in_order(&self.order());
        if selected != state.published_selection {
            shell.publish(on_change(
                selected.iter().map(|id| id.as_str().to_owned()).collect(),
            ));
            state.published_selection = selected;
        }
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the widget-event arguments iced passes"
    )]
    fn handle_event(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let hosted = self
            .drag_host
            .as_ref()
            .is_some_and(DragHostChannel::is_hosting);

        // Focus-visible follows the input modality: a key shows the ring, a
        // pointer press (a click, or a drag and drop) hides it.
        match event {
            Event::Keyboard(iced::keyboard::Event::KeyPressed { .. }) => {
                tree.state.downcast_mut::<ReorderState>().pointer_input = false;
            }
            Event::Mouse(mouse::Event::ButtonPressed(_)) | Event::Touch(_) => {
                tree.state.downcast_mut::<ReorderState>().pointer_input = true;
            }
            _ => {}
        }

        // Rows still gliding after a pointer stops, or after a drag ends,
        // need the next frame; nothing else here asks for one. A drag that
        // ended since the last draw has not started its settle yet (`draw`
        // starts it), so that frame asks for the one after it too.
        if let Event::Window(iced::window::Event::RedrawRequested(at)) = event {
            let state = tree.state.downcast_ref::<ReorderState>();
            let live = match self
                .drag_host
                .as_ref()
                .filter(|channel| channel.is_hosting())
            {
                Some(channel) => channel.live_drag().is_some(),
                None => state
                    .session
                    .as_ref()
                    .and_then(crate::drag_drop_host::LiveDrag::of)
                    .is_some(),
            };
            state.motion.note_frame(*at);
            if state.motion.in_motion(*at) || state.motion.ended_since_drawn(live) {
                shell.request_redraw();
            }
        }

        // An opted-in interactive row receives the initial press so an
        // in-deadband gesture remains its ordinary click. Once our session is
        // active, however, the release belongs to the reorder gesture. Consume
        // it before forwarding to the child; otherwise a dragged sidebar row
        // both reorders and navigates when the pointer comes up.
        if (self.drag_through_children || self.drag_handles)
            && !hosted
            && matches!(
                event,
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
            )
            && tree
                .state
                .downcast_ref::<ReorderState>()
                .session
                .as_ref()
                .is_some_and(DragSession::is_active)
        {
            let state = tree.state.downcast_mut::<ReorderState>();
            let _ = self.handle_terminal_event(event, state, shell, hosted);
            shell.capture_event();
            return;
        }

        // Forward events to children without collecting `layout.children()`
        // into a Vec. A `Vec<Layout>` at pointer-motion frequency
        // is one allocation per event proportional to list length, on the UI
        // thread. The iterator zip below hands each child its layout in one
        // pass; the indexed vec is only built later, on the arms that actually
        // need random access.
        for (child, (child_layout, child_tree)) in self
            .children
            .iter_mut()
            .zip(layout.iter_mut(&mut tree.children))
        {
            child.as_widget_mut().update(
                child_tree,
                event,
                child_layout,
                cursor,
                renderer,
                shell,
                viewport,
            );
        }

        let press = enzui_core::focus::pointer_press_position(event, cursor);
        let mut focused_child = None;
        if press.is_some()
            || matches!(
                event,
                Event::Keyboard(iced::keyboard::Event::KeyPressed { .. })
            )
        {
            for (slot, (child, (child_layout, child_tree))) in self
                .children
                .iter_mut()
                .zip(layout.iter_mut(&mut tree.children))
                .enumerate()
            {
                let mut probe = FocusProbe::default();
                child.as_widget_mut().operate(
                    child_tree,
                    child_layout,
                    &child_layout.bounds(),
                    renderer,
                    &mut probe,
                );
                if probe.focused {
                    focused_child = Some(slot);
                }
            }
        }
        // Each row's handle rects, on the events that start a drag or
        // register the rows with a host.
        let handles: Option<Vec<Vec<Rectangle>>> = (self.drag_handles
            && matches!(
                event,
                Event::Mouse(
                    mouse::Event::ButtonPressed(mouse::Button::Left)
                        | mouse::Event::ButtonReleased(mouse::Button::Left)
                        | mouse::Event::CursorMoved { .. }
                )
            ))
        .then(|| {
            self.children
                .iter_mut()
                .zip(layout.iter_mut(&mut tree.children))
                .map(|(child, (child_layout, child_tree))| {
                    let mut probe = handle::HandleProbe::default();
                    child.as_widget_mut().operate(
                        child_tree,
                        child_layout,
                        &child_layout.bounds(),
                        renderer,
                        &mut probe,
                    );
                    probe.rects
                })
                .collect()
        });
        let on_handle = |row: usize, position: Point| {
            handles
                .as_ref()
                .is_none_or(|handles| handles[row].iter().any(|rect| rect.contains(position)))
        };
        let state = tree.state.downcast_mut::<ReorderState>();
        if let Some(position) = press {
            use enzui_core::focus::{PressFocus, press_focus};
            let was_focused = state.keyboard_owner.focus.is_focused();
            match press_focus(position, bounds.intersection(viewport).unwrap_or_default()) {
                PressFocus::Take if focused_child.is_none() && !self.children.is_empty() => {
                    state.keyboard_owner.focus.focus();
                }
                PressFocus::Take | PressFocus::Drop => state.keyboard_owner.focus.unfocus(),
                PressFocus::Leave => {}
            }
            if state.keyboard_owner.focus.is_focused() != was_focused {
                shell.request_redraw();
            }
        }
        if matches!(event, Event::Window(iced::window::Event::Unfocused)) {
            state.keyboard_owner.focus.unfocus();
        }
        if let Some(slot) = focused_child {
            state.focused = Some(ItemId::new(self.keys[slot].clone()));
        }

        // ------------------------------------------------------------------
        // ------------------------------------------------------------------
        // ------------------------------------------------------------------
        // (A) Ambient modifier state — tracked before any early return.
        // ------------------------------------------------------------------
        //
        // Modifierless mouse presses need click meanings (plain/toggle/range)
        // reconstructed from last-seen modifier states. `ModifiersChanged`
        // canonical sources; modifier-carrying key press/releases update same
        // fields (intervening-`ModifiersChanged`-less changes still caught).
        // Deliberately above capture gates + hosted defers: modifiers are
        // AMBIENT, not gestures. Event-consuming children (capturing-keystroke
        // text inputs) or session-owning hosts must never blind Ctrl-down facts
        // — next presses misread as plain clicks silently replacing whole user
        // selections otherwise.
        match event {
            Event::Keyboard(iced::keyboard::Event::ModifiersChanged(m)) => {
                state.modifiers = *m;
            }
            Event::Keyboard(
                iced::keyboard::Event::KeyPressed { modifiers, .. }
                | iced::keyboard::Event::KeyReleased { modifiers, .. },
            ) => {
                state.modifiers = *modifiers;
            }
            _ => {}
        }

        // ------------------------------------------------------------------
        // ------------------------------------------------------------------
        // (B) TERMINAL events — handled before the capture gate and before the
        //     hosted defer.
        // ------------------------------------------------------------------
        //
        // This must run before the `is_event_captured()` gate. Below it, a
        // child that consumes the Release or Escape would leave the session
        // live: the proposal held, the app expecting a drop, and a second
        // session on the next press. A drag the widget began, the widget
        // ends, whoever consumes the terminal event. `CursorLeft` and
        // `Unfocused` have the same hole without a consumer (a release
        // outside the window), which would leave the session immortal. They
        // cancel rather than commit, like `image_viewer` and `marquee_area`:
        // nothing was published on release, so there is no committed state
        // to keep. Hosts revalidate destinations published from release-pass
        // geometry, since scrolling under a held pointer moves rows silently.
        if hosted
            && matches!(
                event,
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
            )
            && let Some(channel) = self.drag_host.as_ref()
        {
            let release_layouts: Vec<Layout> = layout.children(&tree.children).collect();
            self.register_hosted(channel, bounds, &release_layouts, state, handles.as_deref());
        }

        if self.handle_terminal_event(event, state, shell, hosted) {
            return;
        }

        // Terminal drag cleanup ignores capture, but new keyboard commands belong to the live focus owner.
        if !hosted
            && !shell.is_event_captured()
            && (state.keyboard_owner.focus.is_focused() || focused_child.is_some())
            && self.handle_keyboard_reorder(event, state, shell)
        {
            if focused_child.is_some() {
                let destination = state
                    .focused
                    .as_ref()
                    .and_then(|id| self.keys.iter().position(|key| key == id.as_str()));
                for (slot, (child, (child_layout, child_tree))) in self
                    .children
                    .iter_mut()
                    .zip(layout.iter_mut(&mut tree.children))
                    .enumerate()
                {
                    let mut focus = FocusFirst {
                        take: destination == Some(slot),
                    };
                    child.as_widget_mut().operate(
                        child_tree,
                        child_layout,
                        &child_layout.bounds(),
                        renderer,
                        &mut focus,
                    );
                }
            }
            return;
        }

        // Post-terminal/modifier-arm events alone need indexed row-layout
        // access. Up-front `Vec` collection allocates per-event × list-length on
        // hot paths. Above iterates once needing zero random access; only below
        // does, for bounded event sets. So: allocation skipped for never-
        // `row_at`/`refresh_destination`-reaching events (Modifiers, non-Escape
        // keys, wheel scrolls, non-`RedrawRequested` window events, all
        // already-returned terminal events).
        let needs_child_layouts = matches!(
            event,
            Event::Mouse(
                mouse::Event::ButtonPressed(mouse::Button::Left) | mouse::Event::CursorMoved { .. }
            ) | Event::Window(iced::window::Event::RedrawRequested(_))
        );
        let child_layouts: Vec<Layout> = if needs_child_layouts {
            layout.children(&tree.children).collect()
        } else {
            Vec::new()
        };

        // ------------------------------------------------------------------
        // ------------------------------------------------------------------
        // (C) Hosted defer — but selection first.
        // ------------------------------------------------------------------
        //
        // Channel-claiming `EnzDragDropHost`s own cross-sibling single sessions:
        // lists must not run own (re-`source == destination`s). But lists alone
        // know SELECTIONS: own-`ListSelection` clicks applied + resulting id
        // sets registration-published — host multi-item-drag beginners. Pre-fix
        // `.drag_host(...)` returned untouched here; hosts began `vec![item]`:
        // present-but-host-unreachable whole selection features.
        if hosted {
            let channel = self
                .drag_host
                .as_ref()
                .expect("hosted implies a channel is present");

            if let Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event
                && !shell.is_event_captured()
                && let Some(position) = cursor.position_over(bounds)
                && let Some(i) = self.row_at(position, bounds, child_layouts.iter().copied())
                && on_handle(i, position)
            {
                self.apply_press_to_selection(i, state);
            }

            // Registration clones every item id and rect, so it runs only on
            // the events the host consults it on: press, move, and the release
            // pass above. Redraws and key events never read the registry.
            if matches!(
                event,
                Event::Mouse(
                    mouse::Event::ButtonPressed(mouse::Button::Left)
                        | mouse::Event::CursorMoved { .. }
                )
            ) {
                self.register_hosted(channel, bounds, &child_layouts, state, handles.as_deref());
            }
            return;
        }

        // If a child captured the event, don't START a drag from it unless the
        // caller explicitly opted clickable rows into deadband arbitration.
        // Terminal events were already handled above, so this gate can no
        // longer strand a session.
        let captured_child_press = (self.drag_through_children || self.drag_handles)
            && matches!(
                event,
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
            );
        if shell.is_event_captured() && !captured_child_press {
            return;
        }

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(position) = cursor.position_over(bounds)
                    && let Some(i) = self.row_at(position, bounds, child_layouts.iter().copied())
                    && on_handle(i, position)
                {
                    // Apply the click to the selection first, then begin the
                    // session from whatever that selection now says.
                    let items = self.apply_press_to_selection(i, state);

                    let session = DragSession::begin(
                        CollectionId::new(&self.collection_id),
                        items,
                        self.source_capabilities,
                        (position.x, position.y),
                        self.deadband,
                    );
                    // Observe the phase the session reports at begin.
                    if let Some(observe) = self.on_drag_event.as_ref() {
                        shell.publish(observe(DragObservation::Began(session.phase())));
                    }
                    state.session = Some(session);
                    state.dragged_index = Some(i);
                    state.keys.clone_from(&self.keys);
                }
            }

            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                if let Some(session) = state.session.as_mut() {
                    if session.phase() == DragPhase::Finished {
                        return;
                    }

                    // The DEADBAND is measured in the same space the press
                    // origin was recorded in. The press stored
                    // `cursor.position_over(bounds)` — already translated by any
                    // enclosing scrollable — so the travel test must use the
                    // translated cursor too, or a scroll between press and move
                    // would register as pointer travel the user never made.
                    let travel_pos = cursor.position().unwrap_or(*position);
                    let became_active = session.pointer_moved((travel_pos.x, travel_pos.y));

                    // Observe the activation transition, reading the phase the
                    // session now reports.
                    if became_active && let Some(observe) = self.on_drag_event.as_ref() {
                        shell.publish(observe(DragObservation::Activated(session.phase())));
                    }

                    if session.is_active() {
                        self.refresh_destination(session, cursor, &child_layouts, shell);
                        shell.capture_event();
                        shell.request_redraw();
                    }

                    // Only capture once we're past the deadband, so clicks
                    // still work on children.
                    if became_active {
                        shell.capture_event();
                    }
                }
            }

            // Destinations are a function of the current geometry, not of
            // the last pointer event. `EnzDragAutoScrollHost` scrolls under a
            // stationary pointer, so the row under the cursor changes with no
            // `CursorMoved`; destinations refreshed only on pointer motion
            // would commit the pre-scroll slot on release, exactly in the
            // gestures auto-scroll serves (dragging to the far end of a long
            // list, holding at an edge). Frames are the only events delivered
            // while the pointer is still and the content moves, so refresh
            // here against this pass's freshly laid-out `child_layouts`. It
            // is idempotent ([`hover`](DragSession::hover) re-negotiates from
            // scratch on each call), so running on both paths never applies
            // twice. Only while a session is active, so idle lists do no work
            // per frame; it neither captures the event nor requests a redraw,
            // since the host owns the frame loop and asking here would create
            // an idle redraw loop.
            Event::Window(iced::window::Event::RedrawRequested(_)) => {
                if let Some(session) = state.session.as_mut()
                    && session.is_active()
                {
                    self.refresh_destination(session, cursor, &child_layouts, shell);
                }
            }

            // Release / Escape / CursorLeft / Unfocused are TERMINAL and are
            // handled in `handle_terminal_event` above the capture gate
            // — deliberately not here.
            _ => {}
        }
    }
}

impl<'a, Message> EnzReorderList<'a, Message>
where
    Message: Clone + 'a,
{
    /// Finish the builder and hand back the [`Element`], which is step 3 of
    /// the build pattern every widget in this crate follows (`docs/widgets.md`).
    ///
    /// This forwards to the `From<Self> for Element` impl below, so the two
    /// spellings are one implementation and cannot drift apart. Prefer this
    /// one inside `row![]` / `column![]`, where `.into()` has no target type
    /// to infer.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message, iced::Theme, iced::Renderer> {
        Element::from(self)
    }
}

impl<'a, Message> From<EnzReorderList<'a, Message>>
    for Element<'a, Message, iced::Theme, iced::Renderer>
where
    Message: Clone + 'a,
{
    fn from(list: EnzReorderList<'a, Message>) -> Self {
        Self::new(list)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

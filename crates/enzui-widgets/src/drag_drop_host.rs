//! Share one drag session across reorder lists and keyed item surfaces.
//!
//! Create a [`DragHostChannel`] in the view and pass it to the host and every
//! participant. The session lives in retained widget state, so rebuilding the
//! channel does not interrupt a drag. Participants register current geometry
//! during each event pass; the host publishes accepted drops through its own
//! callbacks. A channel without a claiming host leaves lists in standalone mode.
//!
//! Each collection needs a unique id. Duplicate registrations are rejected in
//! registration order. Hit tests also use registration order, independently of
//! visual stacking; avoid overlapping participants. Registrations and cursor
//! positions must share the host's coordinate space. Scrollables may enclose
//! the host, but a scroll boundary between host and participant is unsupported.
//!
//! The activation threshold defaults to 4 logical pixels. Escape, cursor leave,
//! and window unfocus cancel the session. Release rechecks current geometry;
//! when event routing withholds the negotiated destination's registration, it
//! retains the last proposal. Release with no cursor position cancels.
//!
//! Item surfaces support onto-item and collection-background targets.
//! Control or Command promotes Copy when the item destination allows it.
//! Active drags expose a status announcement and a grabbing cursor; feedback
//! colours follow the live theme. Touch arbitration and tab tear-out are not
//! supported here. Keyboard commands can use
//! [`keyboard_drop`](enzui_core::drag::keyboard_drop).

use enzui_core::drag::{
    CollectionId, DestinationProposal, DragOperation, DragPhase, DragSession, DropRequest,
    DropTarget, ItemDropRequest, ItemId, PayloadKinds, SessionOutcome, SourceCapabilities,
    slot_for_position_1d_with,
};
use enzui_core::placement::Direction;

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Renderer as _, Shell, Widget, overlay, renderer};
use iced::{
    Background, Border, Element, Event, Length, Point, Rectangle, Shadow, Size, Vector, mouse,
};

use std::cell::RefCell;
use std::rc::Rc;

/// Confine one drag-feedback rectangle to its registering surface: a row's
/// hit rectangle may exceed what it draws, and the surface bounds are the
/// outer paint limit. `paint` disjoint from `surface` returns a zero-size
/// rectangle, skipped rather than pinned to the edge.
#[must_use]
pub fn feedback_bounds_within(paint: Rectangle, surface: Rectangle) -> Rectangle {
    paint.intersection(&surface).unwrap_or(Rectangle {
        x: paint.x,
        y: paint.y,
        width: 0.0,
        height: 0.0,
    })
}

fn is_paintable(bounds: Rectangle) -> bool {
    bounds.width > 0.0 && bounds.height > 0.0
}

/// The stacking axis a registered collection uses for slot geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionAxis {
    /// Items advance top-to-bottom and ignore writing direction.
    Vertical,
    /// Items advance on x and use the registration's writing direction.
    Horizontal,
}

/// Geometry and drag policy for one collection in the current event pass.
///
/// Registrations carry ids and rectangles, without resolving payloads.
#[derive(Debug, Clone)]
pub struct CollectionRegistration {
    /// The registering collection's stable id.
    pub collection: CollectionId,
    /// Bounds in the same space as [`mouse::Cursor::position`].
    pub bounds: Rectangle,
    /// One `(item id, row rect)` per item, in layout order — the hit row,
    /// not necessarily the child's intrinsic bounds (a narrow label in a wide
    /// list grabs anywhere on its row).
    pub items: Vec<(ItemId, Rectangle)>,
    /// Which coordinate and extent produce insertion slots.
    pub axis: CollectionAxis,
    /// Writing direction for a horizontal collection. Vertical collections
    /// register [`Direction::LeftToRight`] because top-to-bottom is invariant.
    pub direction: Direction,
    /// What a drag out of this collection permits.
    pub source_capabilities: SourceCapabilities,
    /// The [`PayloadKinds`] a drag out of this collection offers.
    pub offering: PayloadKinds,
    /// Payload kinds accepted by this destination.
    pub accepting: PayloadKinds,
    /// The operations this collection proposes as a destination,
    /// most-preferred first.
    pub destination_operations: Vec<DragOperation>,
    /// Selected ids in layout order. Pressing a selected item drags this
    /// selection; otherwise the host drags only the pressed item.
    pub selected: Vec<ItemId>,
    /// Where each item can be grabbed, one entry per item in `items` order;
    /// `None` grabs anywhere on the item's row.
    pub grab_regions: Option<Vec<Vec<Rectangle>>>,
}

/// One realised keyed item in a virtual list/grid drag surface.
#[derive(Debug, Clone)]
pub struct ItemTargetRegistration {
    /// Stable source/target identity.
    ///
    /// Target labels and generic item highlights are matched by id across all
    /// registered surfaces; shared ids can produce ambiguous feedback.
    pub id: ItemId,
    /// Realised hit rectangle in the host cursor coordinate space.
    pub bounds: Rectangle,
    /// Paint rectangle in the host cursor coordinate space,
    /// already surface-confined by [`feedback_bounds_within`]: narrower than
    /// [`Self::bounds`] when grabbed wider than drawn, identical when drawn
    /// edge to edge.
    pub feedback_bounds: Rectangle,
    /// Whether a press on this item may begin a drag.
    ///
    /// Drop-only regions set this `false`, or an unrelated selectable child
    /// sharing the rectangle could accidentally become a drag source.
    pub drag_source: bool,
    /// Whether this item accepts an onto-item drop.
    pub accepts_drop: bool,
    /// Label used for the active drop target's accessibility announcement.
    pub label: String,
}

/// Bounded registration for a virtual list or grid: the realised window
/// only. Scrolling replaces these rectangles next pass; stable ids, not
/// indices, keep source/target identity across reflow and view switches.
#[derive(Debug, Clone)]
pub struct ItemSurfaceRegistration {
    /// Stable identity of the current collection/location.
    pub collection: CollectionId,
    /// Viewport bounds. Empty-area drops resolve onto this collection.
    pub bounds: Rectangle,
    /// Realised items only.
    pub items: Vec<ItemTargetRegistration>,
    /// Selected source ids in model order.
    pub selected: Vec<ItemId>,
    /// Whether empty space represents an accepting current location.
    pub accepts_background: bool,
    /// Source operations allowed by this surface.
    pub source_capabilities: SourceCapabilities,
    /// Offered payload kinds.
    pub offering: PayloadKinds,
    /// Accepted payload kinds.
    pub accepting: PayloadKinds,
    /// Destination operation preference.
    pub destination_operations: Vec<DragOperation>,
    /// The surface paints its own active-target affordance; the host
    /// suppresses its generic bordered item rectangle for it.
    pub custom_target_highlight: bool,
    /// Whether an explicitly draggable item may take over after its child
    /// captured pointer-down. Opt-in for row surfaces where child capture only
    /// establishes focus; selectable content stays `false`.
    pub allows_captured_press: bool,
}

/// The live drag as a collection paints it: what is dragged, from where,
/// and the destination under the pointer. A paint-time snapshot of the
/// session, never a second drag state.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveDrag {
    /// The collection the drag began in.
    pub source: CollectionId,
    /// The dragged items, in source order.
    pub items: Vec<ItemId>,
    /// Where the press that began the drag happened.
    pub origin: Point,
    /// The collection under the pointer and the target there, if any.
    pub destination: Option<(CollectionId, DropTarget)>,
    /// The dragged rows' axis in their source and their summed width and
    /// height there, captured when the drag left its deadband, while every
    /// collection's geometry was registered. A destination sizes its gap from
    /// it.
    pub extent: Option<(CollectionAxis, iced::Size)>,
}

impl LiveDrag {
    /// Snapshot an active session, returning `None` otherwise.
    ///
    /// The snapshot has no extent; the host supplies it from registered geometry.
    #[must_use]
    pub fn of(session: &DragSession) -> Option<Self> {
        if !session.is_active() {
            return None;
        }
        let (x, y) = session.origin();
        Some(Self {
            source: session.source().clone(),
            items: session.items().to_vec(),
            origin: Point::new(x, y),
            destination: session
                .destination()
                .cloned()
                .zip(session.target().cloned()),
            extent: None,
        })
    }
}

#[derive(Debug, Default)]
struct ChannelInner {
    hosting: bool,

    host_dragged: bool,

    // Reader callbacks may reenter the channel; shared snapshots avoid holding a borrow.
    registry: Rc<Vec<CollectionRegistration>>,
    item_surfaces: Rc<Vec<ItemSurfaceRegistration>>,

    rejected_duplicates: Vec<CollectionId>,

    // Draw mirrors are derived from retained state and cleared after descendant painting.
    active_target: Option<(CollectionId, DropTarget)>,

    live: Option<LiveDrag>,

    layout_live: Option<LiveDrag>,
}

/// Share per-pass geometry and live-drag snapshots between a host and participants.
///
/// Clones share the same channel. The durable session belongs to the host's
/// retained widget state; create this conduit in the view.
#[derive(Debug, Clone, Default)]
pub struct DragHostChannel {
    inner: Rc<RefCell<ChannelInner>>,
}

impl DragHostChannel {
    /// A fresh channel.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a host is currently updating or drawing through this channel.
    ///
    /// A collection with an unclaimed channel keeps its standalone drag handling.
    #[must_use]
    pub fn is_hosting(&self) -> bool {
        self.inner.borrow().hosting
    }

    fn begin_pass(&self, host_dragged: bool) {
        let mut inner = self.inner.borrow_mut();
        inner.hosting = true;
        inner.host_dragged = host_dragged;

        Rc::make_mut(&mut inner.registry).clear();
        Rc::make_mut(&mut inner.item_surfaces).clear();
        inner.rejected_duplicates.clear();
    }

    fn end_pass(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.hosting = false;
        inner.host_dragged = false;
    }

    fn begin_draw(
        &self,
        host_dragged: bool,
        active_target: Option<(CollectionId, DropTarget)>,
        live: Option<LiveDrag>,
    ) {
        let mut inner = self.inner.borrow_mut();
        inner.hosting = true;
        inner.host_dragged = host_dragged;
        inner.active_target = active_target;
        inner.live = live;
    }

    fn end_draw(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.hosting = false;
        inner.host_dragged = false;
        inner.active_target = None;
        inner.live = None;
    }

    /// The live drag as of the host's last layout, for a collection that
    /// grows to fit rows dragged into it.
    #[must_use]
    pub fn layout_drag(&self) -> Option<LiveDrag> {
        self.inner.borrow().layout_live.clone()
    }

    fn set_layout_drag(&self, live: Option<LiveDrag>) {
        self.inner.borrow_mut().layout_live = live;
    }

    /// The live drag while the claiming host draws its content; `None`
    /// outside that window or with no drag past the deadband.
    #[must_use]
    pub fn live_drag(&self) -> Option<LiveDrag> {
        self.inner.borrow().live.clone()
    }

    /// Whether the claiming host had a drag past the deadband at this pass's
    /// start. A hosted list consults it to decide whether a release discharges
    /// its deferred selection collapse; `false` with no claiming host, so
    /// unhosted lists are unaffected.
    #[must_use]
    pub fn host_dragged(&self) -> bool {
        self.inner.borrow().host_dragged
    }

    /// Whether the live drag is currently negotiated onto `item` in
    /// `collection`. Read-only paint seam for the host's draw pass: a surface
    /// renders a directional target without a second drag state.
    #[must_use]
    pub fn is_active_item_target(&self, collection: &CollectionId, item: &ItemId) -> bool {
        let inner = self.inner.borrow();
        matches!(
            &inner.active_target,
            Some((c, DropTarget::OntoItem(target))) if c == collection && target == item
        )
    }

    /// Publish collection geometry and policy for the current event pass.
    ///
    /// Duplicate collection ids across lists and item surfaces are rejected; the
    /// first registration wins. Rejections appear in [`Self::rejected_duplicates`].
    pub fn register(&self, registration: CollectionRegistration) {
        let mut inner = self.inner.borrow_mut();
        let duplicate = inner
            .registry
            .iter()
            .any(|r| r.collection == registration.collection)
            || inner
                .item_surfaces
                .iter()
                .any(|r| r.collection == registration.collection);
        if duplicate {
            log::warn!(
                target: "enzui::drag_drop",
                "EnzDragDropHost: duplicate collection id {:?} rejected — two \
                 collections under one host claim the same identity, which \
                 would make every routed drop ambiguous. Give each list a \
                 distinct .collection_id(); the default is a shared constant.",
                registration.collection.as_str(),
            );
            inner.rejected_duplicates.push(registration.collection);
            return;
        }
        Rc::make_mut(&mut inner.registry).push(registration);
    }

    /// Publish a realised item surface for the current event pass.
    ///
    /// Duplicate collection ids across lists and item surfaces are rejected; the
    /// first registration wins. Rejections appear in [`Self::rejected_duplicates`].
    pub fn register_item_surface(&self, registration: ItemSurfaceRegistration) {
        let mut inner = self.inner.borrow_mut();
        let duplicate = inner
            .item_surfaces
            .iter()
            .any(|r| r.collection == registration.collection)
            || inner
                .registry
                .iter()
                .any(|r| r.collection == registration.collection);
        if duplicate {
            inner.rejected_duplicates.push(registration.collection);
        } else {
            Rc::make_mut(&mut inner.item_surfaces).push(registration);
        }
    }

    /// Read the realised keyed surfaces without cloning their item vectors.
    /// `f` gets the surfaces so far, in registration order, with no interior
    /// borrow held: it may call any channel method, nest readers arbitrarily;
    /// a registration during `f` leaves the held slice unchanged and lands on
    /// the next reader. Taking the snapshot does not allocate.
    pub fn with_item_surfaces<R>(&self, f: impl FnOnce(&[ItemSurfaceRegistration]) -> R) -> R {
        let surfaces = Rc::clone(&self.inner.borrow().item_surfaces);
        f(&surfaces)
    }

    /// Clone the collections registered in the current or most recent event pass.
    ///
    /// Cost is proportional to registered items and metadata. Use
    /// [`Self::with_registry`] to read a shared snapshot without deep cloning.
    #[must_use]
    pub fn registry(&self) -> Vec<CollectionRegistration> {
        (*self.inner.borrow().registry).clone()
    }

    /// Read a shared registry snapshot without cloning collection data.
    ///
    /// `f` sees registrations so far in registration order, with no interior
    /// borrow held. Nested reads and registration are allowed; new registrations
    /// appear on subsequent reads without changing the slice held by `f`. Taking
    /// the snapshot does not allocate.
    pub fn with_registry<R>(&self, f: impl FnOnce(&[CollectionRegistration]) -> R) -> R {
        let registry = Rc::clone(&self.inner.borrow().registry);
        f(&registry)
    }

    /// Collection ids rejected this pass for duplicating an earlier
    /// registration: test/diagnostic surface.
    #[must_use]
    pub fn rejected_duplicates(&self) -> Vec<CollectionId> {
        self.inner.borrow().rejected_duplicates.clone()
    }
}

/// The geometry and strength of drag feedback, in logical pixels and
/// alpha multipliers. A fixed snapshot: the colours stay the live theme's
/// (`primary.weak` for dragged items, `primary.strong` for a target,
/// `danger` for a refused one).
///
/// [`EnzDragDropHost::feedback_tokens`] uses the fill, border and radius
/// fields for item surfaces. [`EnzReorderList::feedback_tokens`](crate::reorder_list::EnzReorderList::feedback_tokens)
/// uses `highlight_border_width` for its insertion bar, the `area_*` fields
/// and that width for an empty target list, and `dragging_elevation` for a
/// lifted row. Pass the same value to both so hosted lists match their host.
///
/// Negative or non-finite radii and border widths use defaults. Finite
/// alphas clamp to `0..=1`; non-finite alphas use defaults. Elevation caps at 5.
/// A zero border width hides target outlines and the insertion bar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DragFeedbackTokens {
    /// Corner radius of a dragged or target item's fill (default 6).
    pub item_radius: f32,
    /// Corner radius of a whole-collection target (default 8).
    pub area_radius: f32,
    /// Width of a target's outline and of an insertion bar (default 2).
    pub highlight_border_width: f32,
    /// Alpha of the fill over dragged items (default 0.32).
    pub drag_fill_alpha: f32,
    /// Alpha of the fill over a target item (default 0.16).
    pub highlight_fill_alpha: f32,
    /// Alpha of the fill over a whole-collection target (default 0.08).
    pub area_fill_alpha: f32,
    /// Elevation level of a lifted row, 0 to 5 (default 3).
    pub dragging_elevation: u8,
}

impl Default for DragFeedbackTokens {
    fn default() -> Self {
        Self {
            item_radius: 6.0,
            area_radius: 8.0,
            highlight_border_width: 2.0,
            drag_fill_alpha: 0.32,
            highlight_fill_alpha: 0.16,
            area_fill_alpha: 0.08,
            dragging_elevation: 3,
        }
    }
}

impl DragFeedbackTokens {
    /// Normalize radii, widths, alphas and elevation as documented on the type.
    #[must_use]
    pub fn sanitized(self) -> Self {
        use enzui_core::metrics::FiniteNonNeg;
        let d = Self::default();
        let alpha = |v: f32, default: f32| {
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                default
            }
        };
        Self {
            item_radius: FiniteNonNeg::new_or(self.item_radius, d.item_radius).get(),
            area_radius: FiniteNonNeg::new_or(self.area_radius, d.area_radius).get(),
            highlight_border_width: FiniteNonNeg::new_or(
                self.highlight_border_width,
                d.highlight_border_width,
            )
            .get(),
            drag_fill_alpha: alpha(self.drag_fill_alpha, d.drag_fill_alpha),
            highlight_fill_alpha: alpha(self.highlight_fill_alpha, d.highlight_fill_alpha),
            area_fill_alpha: alpha(self.area_fill_alpha, d.area_fill_alpha),
            dragging_elevation: self.dragging_elevation.min(5),
        }
    }
}

#[derive(Debug, Default)]
struct EnzDragDropHostState {
    session: Option<DragSession>,
    modifiers: iced::keyboard::Modifiers,

    // Event capture can withhold destination registration on release; retain its negotiated id.
    last_destination: Option<CollectionId>,

    dragged_extent: Option<(CollectionAxis, iced::Size)>,
}

impl EnzDragDropHostState {
    fn live(&self) -> Option<LiveDrag> {
        self.session
            .as_ref()
            .and_then(LiveDrag::of)
            .map(|mut live| {
                live.extent = self.dragged_extent;
                live
            })
    }

    fn end_session(&mut self) -> bool {
        let grown = self.grows(self.last_destination.as_ref());
        self.session = None;
        self.last_destination = None;
        self.dragged_extent = None;
        grown
    }

    fn grows(&self, destination: Option<&CollectionId>) -> bool {
        self.dragged_extent.is_some()
            && destination.is_some_and(|destination| {
                self.session
                    .as_ref()
                    .is_some_and(|session| session.source() != destination)
            })
    }
}

fn dragged_extent(
    registry: &[CollectionRegistration],
    source: &CollectionId,
    items: &[ItemId],
) -> Option<(CollectionAxis, iced::Size)> {
    let registration = registry.iter().find(|r| &r.collection == source)?;
    let size = registration
        .items
        .iter()
        .filter(|(id, _)| items.contains(id))
        .fold(iced::Size::ZERO, |sum, (_, rect)| {
            iced::Size::new(sum.width + rect.width, sum.height + rect.height)
        });
    (size.width > 0.0 || size.height > 0.0).then_some((registration.axis, size))
}

/// Transparent wrapper owning one [`DragSession`] across the collections
/// beneath it, enabling cross-collection drops.
#[must_use]
pub struct EnzDragDropHost<'a, Message> {
    channel: DragHostChannel,
    content: Element<'a, Message, iced::Theme, iced::Renderer>,
    on_drop: Option<Box<dyn Fn(DropRequest) -> Message + 'a>>,
    on_item_drop: Option<Box<dyn Fn(ItemDropRequest) -> Message + 'a>>,
    deadband: f32,
    feedback: DragFeedbackTokens,
}

/// Wrap `content` in a host bound to `channel` — the same `channel` for
/// every participating collection.
pub fn enz_drag_drop_host<'a, Message>(
    channel: &DragHostChannel,
    content: impl Into<Element<'a, Message, iced::Theme, iced::Renderer>>,
) -> EnzDragDropHost<'a, Message> {
    EnzDragDropHost {
        channel: channel.clone(),
        content: content.into(),
        on_drop: None,
        on_item_drop: None,
        deadband: enzui_core::desktop_settings::input().drag_threshold,
        feedback: DragFeedbackTokens::default(),
    }
}

impl<'a, Message> EnzDragDropHost<'a, Message> {
    /// Publish one request for an accepted drop.
    ///
    /// Hosted lists do not invoke their own drop callbacks. Typed item drops
    /// also use this callback when [`Self::on_item_drop`] is unset. Without a
    /// matching callback, the session ends without publishing a message.
    pub fn on_drop(mut self, f: impl Fn(DropRequest) -> Message + 'a) -> Self {
        self.on_drop = Some(Box::new(f));
        self
    }

    /// Publish typed onto-item or collection-background drops.
    ///
    /// This takes precedence over [`Self::on_drop`] for item surfaces; reorder
    /// insertion drops use [`Self::on_drop`].
    pub fn on_item_drop(mut self, f: impl Fn(ItemDropRequest) -> Message + 'a) -> Self {
        self.on_item_drop = Some(Box::new(f));
        self
    }

    /// Travel threshold in logical pixels (default 4).
    ///
    /// A drag activates only after exceeding this distance. Negative and
    /// non-finite thresholds become zero when the session begins.
    pub fn deadband(mut self, px: f32) -> Self {
        self.deadband = px;
        self
    }

    /// The feedback geometry and wash strengths, as a fixed snapshot; the
    /// colours keep following the live theme. See [`DragFeedbackTokens`].
    pub fn feedback_tokens(mut self, tokens: DragFeedbackTokens) -> Self {
        self.feedback = tokens.sanitized();
        self
    }
}

/// The item a press at `position` grabs: the one under it, when the press is
/// inside one of the item's grab regions.
fn item_at(
    registry: &[CollectionRegistration],
    position: Point,
) -> Option<(&CollectionRegistration, ItemId)> {
    registry.iter().find_map(|reg| {
        reg.items
            .iter()
            .enumerate()
            .find(|(index, (_, rect))| {
                rect.contains(position)
                    && reg.grab_regions.as_ref().is_none_or(|regions| {
                        regions
                            .get(*index)
                            .is_some_and(|rects| rects.iter().any(|r| r.contains(position)))
                    })
            })
            .map(|(_, (id, _))| (reg, id.clone()))
    })
}

fn proposal_at(
    registry: &[CollectionRegistration],
    position: Point,
) -> Option<DestinationProposal> {
    let reg = registry.iter().find(|reg| reg.bounds.contains(position))?;
    let (axis_position, items): (f32, Vec<(f32, f32)>) = match reg.axis {
        CollectionAxis::Vertical => (
            position.y,
            reg.items
                .iter()
                .map(|(_, rect)| (rect.y, rect.height))
                .collect(),
        ),
        CollectionAxis::Horizontal => (
            position.x,
            reg.items
                .iter()
                .map(|(_, rect)| (rect.x, rect.width))
                .collect(),
        ),
    };
    let slot = slot_for_position_1d_with(axis_position, &items, reg.direction);
    Some(
        DestinationProposal::new(
            reg.collection.clone(),
            slot,
            reg.destination_operations.clone(),
        )
        .accepting(reg.accepting.clone()),
    )
}

fn item_proposal_at(
    registry: &[ItemSurfaceRegistration],
    position: Point,
    dragged: &[ItemId],
    copy_modifier: bool,
) -> Option<DestinationProposal> {
    let reg = registry.iter().find(|reg| reg.bounds.contains(position))?;
    let mut preferred = reg.destination_operations.clone();
    if copy_modifier && preferred.contains(&DragOperation::Copy) {
        preferred.retain(|op| *op != DragOperation::Copy);
        preferred.insert(0, DragOperation::Copy);
    }
    if let Some(item) = reg.items.iter().find(|item| item.bounds.contains(position)) {
        if !item.accepts_drop || dragged.contains(&item.id) {
            return None;
        }
        return Some(
            DestinationProposal::onto_item(reg.collection.clone(), item.id.clone(), preferred)
                .accepting(reg.accepting.clone()),
        );
    }
    reg.accepts_background.then(|| {
        DestinationProposal::onto_collection(reg.collection.clone(), preferred)
            .accepting(reg.accepting.clone())
    })
}

fn item_source_at(
    registry: &[ItemSurfaceRegistration],
    position: Point,
    child_captured: bool,
) -> Option<(CollectionId, Vec<ItemId>, SourceCapabilities, PayloadKinds)> {
    let reg = registry.iter().find(|reg| {
        reg.bounds.contains(position) && (!child_captured || reg.allows_captured_press)
    })?;
    let item = reg
        .items
        .iter()
        .find(|item| item.drag_source && item.bounds.contains(position))?;
    let items = if reg.selected.contains(&item.id) {
        reg.selected.clone()
    } else {
        vec![item.id.clone()]
    };
    Some((
        reg.collection.clone(),
        items,
        reg.source_capabilities,
        reg.offering.clone(),
    ))
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for EnzDragDropHost<'_, Message>
where
    Message: Clone,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<EnzDragDropHostState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(EnzDragDropHostState::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let live = tree.state.downcast_ref::<EnzDragDropHostState>().live();
        self.channel.set_layout_drag(live);
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let host_state = tree.state.downcast_ref::<EnzDragDropHostState>();
        if let Some(session) = host_state
            .session
            .as_ref()
            .filter(|session| session.is_active())
        {
            let destination = match session.target() {
                Some(DropTarget::OntoItem(id)) => {
                    let label = self.channel.with_item_surfaces(|surfaces| {
                        surfaces
                            .iter()
                            .flat_map(|surface| &surface.items)
                            .find(|item| &item.id == id)
                            .map(|item| item.label.clone())
                    });
                    enzui_core::localized!("enzui-drag-target", "target" => label.as_deref().unwrap_or(id.as_str()))
                }
                Some(DropTarget::OntoCollection) => {
                    enzui_core::localization::text("enzui-drag-current-location")
                }
                Some(DropTarget::Insertion(slot)) => {
                    enzui_core::localized!("enzui-drag-position", "position" => slot.index())
                }
                None => enzui_core::localization::text("enzui-drag-no-target"),
            };
            let verb = match session.operation() {
                DragOperation::Copy => enzui_core::localization::text("enzui-drag-copy"),
                DragOperation::Move => enzui_core::localization::text("enzui-drag-move"),
                DragOperation::Delete => enzui_core::localization::text("enzui-drag-delete"),
                DragOperation::Forbidden => enzui_core::localization::text("enzui-drag-forbidden"),
            };
            let label = enzui_core::localized!("enzui-drag-announcement", "count" => session.items().len(), "destination" => destination, "operation" => verb);
            let mut node = enzui_core::collect::SemanticsNode::new(
                enzui_core::semantics::Semantics::new(enzui_core::semantics::Role::Status)
                    .label(label),
            );
            operation.custom(None, layout.bounds(), &mut node);
        }
        self.content.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            viewport,
            renderer,
            operation,
        );
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
        // Children need the prior drag phase to distinguish selection clicks from drag releases.
        let host_dragged = tree
            .state
            .downcast_ref::<EnzDragDropHostState>()
            .session
            .as_ref()
            .is_some_and(|s| s.phase() != DragPhase::Pending);
        self.channel.begin_pass(host_dragged);

        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );

        let state = tree.state.downcast_mut::<EnzDragDropHostState>();
        if crate::text_drag::file_source_active() {
            if state.end_session() {
                shell.invalidate_layout();
            }
            self.channel.begin_pass(false);
            shell.request_redraw();
            return;
        }

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if state.session.is_none()
                    && let Some(position) = cursor.position()
                {
                    let begin: Option<(
                        CollectionId,
                        Vec<ItemId>,
                        SourceCapabilities,
                        PayloadKinds,
                    )> = self
                        .channel
                        .with_item_surfaces(|reg| {
                            item_source_at(reg, position, shell.is_event_captured())
                        })
                        .or_else(|| {
                            self.channel.with_registry(|reg| {
                                let (r, item) = item_at(reg, position)?;
                                let items = if r.selected.contains(&item) {
                                    r.selected.clone()
                                } else {
                                    vec![item]
                                };
                                Some((
                                    r.collection.clone(),
                                    items,
                                    r.source_capabilities,
                                    r.offering.clone(),
                                ))
                            })
                        });
                    if let Some((collection, items, capabilities, offering)) = begin {
                        state.session = Some(
                            DragSession::begin(
                                collection,
                                items,
                                capabilities,
                                (position.x, position.y),
                                self.deadband,
                            )
                            .offering(offering),
                        );
                    }
                }
            }

            // Use the translated cursor: the event payload remains in raw window coordinates.
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let copy_modifier = state.modifiers.command() || state.modifiers.control();
                let adjusted = cursor.position();
                let destination = match (state.session.as_mut(), adjusted) {
                    (Some(session), Some(position)) => {
                        session.pointer_moved((position.x, position.y));
                        if session.is_active() && state.dragged_extent.is_none() {
                            state.dragged_extent = self.channel.with_registry(|reg| {
                                dragged_extent(reg, session.source(), session.items())
                            });
                        }
                        if session.is_active() {
                            let proposal = self.channel.with_item_surfaces(|reg| {
                                item_proposal_at(reg, position, session.items(), copy_modifier)
                            });
                            let proposal = proposal.or_else(|| {
                                self.channel.with_registry(|reg| proposal_at(reg, position))
                            });
                            let destination = proposal.as_ref().map(|p| p.collection.clone());
                            session.hover(proposal);
                            shell.capture_event();
                            shell.request_redraw();
                            destination
                        } else {
                            None
                        }
                    }

                    (Some(session), None) => {
                        if session.is_active() {
                            session.hover(None);
                            shell.request_redraw();
                        }
                        None
                    }
                    (None, _) => None,
                };

                if state.last_destination != destination
                    && (state.grows(state.last_destination.as_ref())
                        || state.grows(destination.as_ref()))
                {
                    shell.invalidate_layout();
                }
                state.last_destination = destination;
            }

            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let negotiated = state.last_destination.clone();
                let copy_modifier = state.modifiers.command() || state.modifiers.control();
                if let Some(session) = state.session.as_mut() {
                    // Autoscroll can move targets under a stationary pointer before release.
                    if session.is_active() {
                        if let Some(position) = cursor.position() {
                            let proposal = self.channel.with_item_surfaces(|reg| {
                                item_proposal_at(reg, position, session.items(), copy_modifier)
                            });
                            let proposal = proposal.or_else(|| {
                                self.channel.with_registry(|reg| proposal_at(reg, position))
                            });

                            let has_geometry =
                                self.channel.with_item_surfaces(|reg| !reg.is_empty())
                                    || self.channel.with_registry(|reg| !reg.is_empty());

                            let destination_present =
                                negotiated.as_ref().is_none_or(|collection| {
                                    self.channel.with_item_surfaces(|reg| {
                                        reg.iter().any(|r| &r.collection == collection)
                                    }) || self.channel.with_registry(|reg| {
                                        reg.iter().any(|r| &r.collection == collection)
                                    })
                                });
                            // Preserve the proposal when capture withheld destination geometry.
                            if has_geometry && (proposal.is_some() || destination_present) {
                                session.hover(proposal);
                            }
                        } else {
                            session.hover(None);
                        }
                    }
                    let target = session.target().cloned();
                    if let Some(SessionOutcome::Dropped(request)) = session.release() {
                        match (target, self.on_item_drop.as_ref(), self.on_drop.as_ref()) {
                            (
                                Some(
                                    target @ (DropTarget::OntoItem(_) | DropTarget::OntoCollection),
                                ),
                                Some(on_item_drop),
                                _,
                            ) => shell.publish(on_item_drop(ItemDropRequest {
                                request: *request,
                                target,
                            })),
                            (_, _, Some(on_drop)) => shell.publish(on_drop(*request)),
                            _ => {}
                        }
                    }
                    if state.end_session() {
                        shell.invalidate_layout();
                    }
                    shell.request_redraw();
                }
            }

            Event::Keyboard(iced::keyboard::Event::ModifiersChanged(modifiers)) => {
                state.modifiers = *modifiers;
            }

            Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                ..
            }) => {
                if let Some(session) = state.session.as_mut() {
                    let was_active = session.is_active();
                    let _ = session.cancel();
                    if state.end_session() {
                        shell.invalidate_layout();
                    }
                    if was_active {
                        shell.capture_event();
                    }
                    shell.request_redraw();
                }
            }

            Event::Mouse(mouse::Event::CursorLeft)
            | Event::Window(iced::window::Event::Unfocused) => {
                if let Some(session) = state.session.as_mut() {
                    let _ = session.cancel();
                    if state.end_session() {
                        shell.invalidate_layout();
                    }
                    shell.request_redraw();
                }
            }

            _ => {}
        }

        // Hosting is scoped to this traversal; a later standalone traversal must not defer.
        self.channel.end_pass();
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
        let state = tree.state.downcast_ref::<EnzDragDropHostState>();
        let active = state
            .session
            .as_ref()
            .is_some_and(enzui_core::drag::DragSession::is_active);
        let active_target = state
            .session
            .as_ref()
            .filter(|session| session.is_active())
            .and_then(|session| {
                state.last_destination.as_ref().and_then(|collection| {
                    session
                        .target()
                        .cloned()
                        .map(|target| (collection.clone(), target))
                })
            });
        let live = state.live();
        self.channel.begin_draw(active, active_target, live);
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
        self.channel.end_draw();

        let Some(session) = state.session.as_ref().filter(|session| session.is_active()) else {
            return;
        };
        let palette = theme.palette();
        let with_alpha = |mut color: iced::Color, alpha: f32| {
            color.a *= alpha;
            color
        };
        let tokens = self.feedback;
        let source_color = with_alpha(palette.primary.weak.color, tokens.drag_fill_alpha);
        let target_color = if session.operation() == DragOperation::Forbidden {
            palette.danger.base.color
        } else {
            palette.primary.strong.color
        };

        // A separate layer keeps feedback above descendant layers on wgpu.
        iced::advanced::Renderer::with_layer(renderer, layout.bounds(), |renderer| {
            self.channel.with_item_surfaces(|surfaces| {
                for surface in surfaces {
                    for item in &surface.items {

                        // Hit bounds may exceed the surface; paint only its confined feedback bounds.
                        if !is_paintable(item.feedback_bounds) {
                            continue;
                        }
                        if session.source() == &surface.collection
                            && session.items().contains(&item.id)
                        {
                            renderer.fill_quad(
                                renderer::Quad {
                                    bounds: item.feedback_bounds,
                                    border: Border {
                                        radius: tokens.item_radius.into(),
                                        ..Border::default()
                                    },
                                    shadow: Shadow::default(),
                                    snap: true,
                                },
                                Background::Color(source_color),
                            );
                        }
                        if !surface.custom_target_highlight
                            && matches!(session.target(), Some(DropTarget::OntoItem(id)) if id == &item.id)
                        {
                            renderer.fill_quad(
                                renderer::Quad {
                                    bounds: item.feedback_bounds,
                                    border: Border {
                                        color: target_color,
                                        width: tokens.highlight_border_width,
                                        radius: tokens.item_radius.into(),
                                    },
                                    shadow: Shadow::default(),
                                    snap: true,
                                },
                                Background::Color(with_alpha(
                                    target_color,
                                    tokens.highlight_fill_alpha,
                                )),
                            );
                        }
                    }
                    if matches!(session.target(), Some(DropTarget::OntoCollection)) {
                        renderer.fill_quad(
                            renderer::Quad {
                                bounds: surface.bounds,
                                border: Border {
                                    color: target_color,
                                    width: tokens.highlight_border_width,
                                    radius: tokens.area_radius.into(),
                                },
                                shadow: Shadow::default(),
                                snap: true,
                            },
                            Background::Color(with_alpha(
                                target_color,
                                tokens.area_fill_alpha,
                            )),
                        );
                    }
                }
            });
        });
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let state = tree.state.downcast_ref::<EnzDragDropHostState>();
        if state.session.as_ref().is_some_and(DragSession::is_active) {
            return mouse::Interaction::Grabbing;
        }

        let from_content = self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        );
        if from_content != mouse::Interaction::None {
            return from_content;
        }

        let Some(position) = cursor.position() else {
            return mouse::Interaction::None;
        };

        let over_source = self.channel.with_item_surfaces(|surfaces| {
            surfaces.iter().any(|surface| {
                surface.bounds.contains(position)
                    && surface
                        .items
                        .iter()
                        .any(|item| item.drag_source && item.bounds.contains(position))
            })
        }) || self
            .channel
            .with_registry(|registry| item_at(registry, position).is_some());

        if over_source {
            mouse::Interaction::Grab
        } else {
            mouse::Interaction::None
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
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message> EnzDragDropHost<'a, Message>
where
    Message: Clone + 'a,
{
    /// Convert the builder into an [`Element`] without relying on type inference.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message, iced::Theme, iced::Renderer> {
        Element::from(self)
    }
}

impl<'a, Message> From<EnzDragDropHost<'a, Message>>
    for Element<'a, Message, iced::Theme, iced::Renderer>
where
    Message: Clone + 'a,
{
    fn from(host: EnzDragDropHost<'a, Message>) -> Self {
        Self::new(host)
    }
}

#[cfg(test)]
mod tests;

//! Renderer-neutral drag-session state, payload-kind negotiation, drop requests, and reorder geometry.
//!
//! The core stores stable ids and payload-kind tags, never materialized payloads or renderer objects. Accepted drops publish one typed request for application update to resolve.

use crate::metrics::FiniteNonNeg;
use crate::placement::Direction;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

/// A stable application-supplied item key, unique within its collection.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemId(pub String);

impl ItemId {
    /// Build an id from anything string-like.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Builds a stable id from a native path without losing platform bytes or code units.
    ///
    /// Domain, version, and platform tags prevent collisions with display text or literal encoded spellings.
    #[must_use]
    pub fn from_path(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();

        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;

            Self(encode_path_units(
                "enzui:item:path:v1:unix:bytes:",
                path.as_os_str().as_bytes().iter().copied().map(u16::from),
                2,
            ))
        }

        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;

            Self(encode_path_units(
                "enzui:item:path:v1:windows:wchar:",
                path.as_os_str().encode_wide(),
                4,
            ))
        }

        // `OsStr` exposes no target-specific native-unit extension on other
        // platforms. Its encoded bytes are nevertheless an injective encoding
        // for that target and are the representation the standard library
        // supports for lossless OS-string transport.
        #[cfg(not(any(unix, windows)))]
        Self(encode_path_units(
            "enzui:item:path:v1:rust-os:bytes:",
            path.as_os_str()
                .as_encoded_bytes()
                .iter()
                .copied()
                .map(u16::from),
            2,
        ))
    }

    /// The underlying key.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn encode_path_units(prefix: &str, units: impl IntoIterator<Item = u16>, width: usize) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let mut encoded = String::from(prefix);
    for unit in units {
        for shift in (0..width).rev() {
            let nibble = usize::from((unit >> (shift * 4)) & 0xf);
            encoded.push(char::from(HEX[nibble]));
        }
    }
    encoded
}

impl From<&str> for ItemId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

/// A stable key identifying one reorderable collection (list, tab strip, grid).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CollectionId(pub String);

impl CollectionId {
    /// Build a collection id.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The underlying key.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for CollectionId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

/// A gap between items: index `i` is before item `i`, and index `n` appends to an `n`-item collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct InsertionSlot(pub usize);

impl InsertionSlot {
    /// The slot index.
    #[must_use]
    pub fn index(self) -> usize {
        self.0
    }
}

/// A negotiated drag operation.
///
/// [`Self::Forbidden`] explicitly refuses a destination and is distinct from having no destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DragOperation {
    /// Duplicate the items, leaving the source untouched.
    Copy,
    /// Move the items out of the source into the destination.
    Move,
    /// Remove the items (drop on a trash affordance).
    Delete,
    /// Explicitly refused.
    Forbidden,
}

/// Operations a drag source permits; allowing none makes every negotiation forbidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceCapabilities {
    /// The source can be copied from.
    pub copy: bool,
    /// The source can be moved out of.
    pub move_: bool,
    /// The source supports delete-on-drop.
    pub delete: bool,
}

impl SourceCapabilities {
    /// Move-only, the common list-reorder case.
    #[must_use]
    pub fn move_only() -> Self {
        Self {
            copy: false,
            move_: true,
            delete: false,
        }
    }

    /// Copy and move, no delete.
    #[must_use]
    pub fn copy_or_move() -> Self {
        Self {
            copy: true,
            move_: true,
            delete: false,
        }
    }

    /// Nothing is permitted.
    #[must_use]
    pub fn none() -> Self {
        Self {
            copy: false,
            move_: false,
            delete: false,
        }
    }

    /// Whether `op` is permitted by this source.
    #[must_use]
    pub fn allows(self, op: DragOperation) -> bool {
        match op {
            DragOperation::Copy => self.copy,
            DragOperation::Move => self.move_,
            DragOperation::Delete => self.delete,
            DragOperation::Forbidden => false,
        }
    }
}

/// A tag describing payload kind without containing payload data.
///
/// Applications resolve actual data only after an accepted [`DropRequest`]; [`Self::Application`] stores only a kind name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PayloadKind {
    /// Plain text.
    Text,
    /// A URI reference, not the referenced resource.
    Uri,
    /// An image kind, not image pixels.
    Image,
    /// An application-defined kind named by an owned MIME-like string and compared by equality.
    Application(Arc<str>),
}

impl PayloadKind {
    /// Build an [`Self::Application`] kind from anything string-like.
    pub fn application(name: impl Into<Arc<str>>) -> Self {
        Self::Application(name.into())
    }
}

/// A set of payload kinds offered by a source or accepted by a destination.
///
/// The default universal set means every kind. Explicit sets may be empty, which means no kinds rather than universal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadKinds {
    universal: bool,
    kinds: HashSet<PayloadKind>,
}

impl Default for PayloadKinds {
    fn default() -> Self {
        Self::universal()
    }
}

impl PayloadKinds {
    /// Returns the universal set containing every payload kind.
    #[must_use]
    pub fn universal() -> Self {
        Self {
            universal: true,
            kinds: HashSet::new(),
        }
    }

    /// Builds an explicit set; an empty iterator produces a set containing no kinds.
    pub fn from_kinds(kinds: impl IntoIterator<Item = PayloadKind>) -> Self {
        Self {
            universal: false,
            kinds: kinds.into_iter().collect(),
        }
    }

    /// Builds an explicit set from a slice of payload kinds.
    #[must_use]
    pub fn of(kinds: &[PayloadKind]) -> Self {
        Self::from_kinds(kinds.iter().cloned())
    }

    /// Whether this is the universal ("every kind") set.
    #[must_use]
    pub fn is_universal(&self) -> bool {
        self.universal
    }

    /// Returns whether this is an explicit empty set; the universal set is not empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self.universal && self.kinds.is_empty()
    }

    /// Whether `kind` is a member.
    #[must_use]
    pub fn contains(&self, kind: &PayloadKind) -> bool {
        self.universal || self.kinds.contains(kind)
    }

    /// Returns whether offered and accepted sets share any payload kind.
    ///
    /// Universal intersects every set except an explicit empty set; two explicit sets intersect only when they share a member.
    #[must_use]
    pub fn intersects(&self, other: &PayloadKinds) -> bool {
        if self.is_empty() || other.is_empty() {
            return false;
        }
        if self.universal || other.universal {
            return true;
        }
        self.kinds.iter().any(|k| other.kinds.contains(k))
    }
}

/// A typed drop destination: insertion gap, keyed item, or collection background.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropTarget {
    /// A gap between ordered items.
    Insertion(InsertionSlot),
    /// Onto one keyed item (for example a directory).
    OntoItem(ItemId),
    /// Onto the collection itself (for example the current directory).
    OntoCollection,
}

/// A destination target, accepted kinds, and operations ordered by preference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DestinationProposal {
    /// The destination collection.
    pub collection: CollectionId,
    /// Typed destination geometry. This is never inferred from a sentinel slot.
    pub target: DropTarget,
    /// Compatibility accessor for insertion-oriented consumers. Item and
    /// collection targets carry slot zero; callers must inspect [`Self::target`]
    /// before treating it as an insertion position.
    pub slot: InsertionSlot,
    /// Acceptable operations, most-preferred first.
    pub preferred: Vec<DragOperation>,
    /// Payload kinds accepted by this destination; defaults to [`PayloadKinds::universal`].
    pub accepts: PayloadKinds,
}

impl DestinationProposal {
    /// Proposes a move into `collection` at `slot`, accepting every payload kind.
    pub fn move_into(collection: impl Into<CollectionId>, slot: InsertionSlot) -> Self {
        Self {
            collection: collection.into(),
            target: DropTarget::Insertion(slot),
            slot,
            preferred: vec![DragOperation::Move],
            accepts: PayloadKinds::universal(),
        }
    }

    /// Proposes ordered operations into `collection` at `slot`, accepting every payload kind.
    pub fn new(
        collection: impl Into<CollectionId>,
        slot: InsertionSlot,
        preferred: Vec<DragOperation>,
    ) -> Self {
        Self {
            collection: collection.into(),
            target: DropTarget::Insertion(slot),
            slot,
            preferred,
            accepts: PayloadKinds::universal(),
        }
    }

    /// Propose a drop onto one keyed item.
    #[must_use]
    pub fn onto_item(
        collection: impl Into<CollectionId>,
        item: impl Into<ItemId>,
        preferred: Vec<DragOperation>,
    ) -> Self {
        Self {
            collection: collection.into(),
            target: DropTarget::OntoItem(item.into()),
            slot: InsertionSlot(0),
            preferred,
            accepts: PayloadKinds::universal(),
        }
    }

    /// Propose a drop onto the collection's background/current location.
    #[must_use]
    pub fn onto_collection(
        collection: impl Into<CollectionId>,
        preferred: Vec<DragOperation>,
    ) -> Self {
        Self {
            collection: collection.into(),
            target: DropTarget::OntoCollection,
            slot: InsertionSlot(0),
            preferred,
            accepts: PayloadKinds::universal(),
        }
    }

    /// Restricts accepted payload kinds; an empty set refuses every drag.
    #[must_use]
    pub fn accepting(mut self, accepts: PayloadKinds) -> Self {
        self.accepts = accepts;
        self
    }
}

/// Negotiates a mutually supported operation.
///
/// Disjoint payload kinds return [`DragOperation::Forbidden`] before operation preferences are considered; otherwise the first destination preference allowed by the source wins.
#[must_use]
pub fn negotiate(
    source: SourceCapabilities,
    offered: &PayloadKinds,
    proposal: &DestinationProposal,
) -> DragOperation {
    if !offered.intersects(&proposal.accepts) {
        return DragOperation::Forbidden;
    }
    proposal
        .preferred
        .iter()
        .copied()
        .find(|op| source.allows(*op))
        .unwrap_or(DragOperation::Forbidden)
}

/// Builds a keyboard-accessible drop through [`DragSession`] using the same kind and operation negotiation as pointer dragging.
#[must_use]
pub fn keyboard_drop(
    source: impl Into<CollectionId>,
    items: Vec<ItemId>,
    capabilities: SourceCapabilities,
    offered: PayloadKinds,
    proposal: DestinationProposal,
) -> Option<ItemDropRequest> {
    let mut session =
        DragSession::begin(source, items, capabilities, (0.0, 0.0), 0.0).offering(offered);
    let _ = session.pointer_moved((1.0, 0.0));
    let _ = session.hover(Some(proposal));
    let target = session.target()?.clone();
    match session.release() {
        Some(SessionOutcome::Dropped(request)) => Some(ItemDropRequest {
            request: *request,
            target,
        }),
        Some(SessionOutcome::Cancelled) | None => None,
    }
}

/// A file-style item or collection drop paired with its shared drag request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemDropRequest {
    /// Shared drag request fields.
    pub request: DropRequest,
    /// Typed destination geometry.
    pub target: DropTarget,
}

/// The atomic ids-only request emitted by an accepted drop.
///
/// Application update resolves current payloads and applies cross-collection changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropRequest {
    /// Where the items came from.
    pub source: CollectionId,
    /// Where they are going.
    pub destination: CollectionId,
    /// The dragged items, in their source order.
    pub items: Vec<ItemId>,
    /// Insertion position for reorder-oriented consumers. File-style item
    /// targets are published separately as [`ItemDropRequest`].
    pub slot: InsertionSlot,
    /// The negotiated operation.
    pub operation: DragOperation,
}

/// Why a [`DragSession`] ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionOutcome {
    /// The session finished with an accepted drop request.
    Dropped(Box<DropRequest>),
    /// The session ended without a drop because it was cancelled, lost capture, or released without an accepting destination.
    Cancelled,
}

/// The lifecycle phase of a drag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragPhase {
    /// The session has not yet crossed its activation deadband.
    Pending,
    /// The deadband was exceeded; the drag is live.
    Active,
    /// The session finished (dropped or cancelled) and is inert.
    Finished,
}

/// Transient drag gesture state containing stable ids and negotiation state but no payload or renderer objects.
#[derive(Debug, Clone, PartialEq)]
pub struct DragSession {
    source: CollectionId,
    items: Vec<ItemId>,
    capabilities: SourceCapabilities,
    // Offered payload kinds default to `PayloadKinds::universal`.
    offered: PayloadKinds,
    origin: (f32, f32),
    deadband: f32,
    phase: DragPhase,
    proposal: Option<DestinationProposal>,
    operation: DragOperation,
}

impl DragSession {
    /// Begins a pending drag with normalized origin, non-negative deadband, and source-ordered item ids.
    ///
    /// Duplicate item ids collapse to their first occurrence. Empty item lists are retained but can never produce a dropped outcome.
    pub fn begin(
        source: impl Into<CollectionId>,
        items: Vec<ItemId>,
        capabilities: SourceCapabilities,
        origin: (f32, f32),
        deadband: f32,
    ) -> Self {
        Self {
            source: source.into(),
            items: dedup_preserving_order(items),
            capabilities,
            offered: PayloadKinds::universal(),
            origin: (
                if origin.0.is_finite() { origin.0 } else { 0.0 },
                if origin.1.is_finite() { origin.1 } else { 0.0 },
            ),
            deadband: FiniteNonNeg::new_or(deadband, 0.0).get(),
            phase: DragPhase::Pending,
            proposal: None,
            operation: DragOperation::Forbidden,
        }
    }

    /// Sets the payload kinds offered by this drag; an empty set can satisfy no destination.
    #[must_use]
    pub fn offering(mut self, offered: PayloadKinds) -> Self {
        self.offered = offered;
        self
    }

    /// The [`PayloadKind`]s this drag offers.
    #[must_use]
    pub fn offered(&self) -> &PayloadKinds {
        &self.offered
    }

    /// The current lifecycle phase.
    #[must_use]
    pub fn phase(&self) -> DragPhase {
        self.phase
    }

    /// The source collection's stable id.
    #[must_use]
    pub fn source(&self) -> &CollectionId {
        &self.source
    }

    /// The dragged item ids.
    #[must_use]
    pub fn items(&self) -> &[ItemId] {
        &self.items
    }

    /// Where the press that began this session happened.
    #[must_use]
    pub fn origin(&self) -> (f32, f32) {
        self.origin
    }

    /// The collection currently under the pointer, if any.
    #[must_use]
    pub fn destination(&self) -> Option<&CollectionId> {
        self.proposal.as_ref().map(|proposal| &proposal.collection)
    }

    /// The typed destination currently under the pointer.
    #[must_use]
    pub fn target(&self) -> Option<&DropTarget> {
        self.proposal.as_ref().map(|proposal| &proposal.target)
    }

    /// The currently negotiated operation (`Forbidden` when over nothing).
    #[must_use]
    pub fn operation(&self) -> DragOperation {
        self.operation
    }

    /// Whether the drag has passed the deadband and is live.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.phase == DragPhase::Active
    }

    /// Updates pointer position and returns whether the session is active.
    ///
    /// Non-finite positions cannot activate the drag and clear any current proposal so release cannot use a stale destination.
    pub fn pointer_moved(&mut self, position: (f32, f32)) -> bool {
        if self.phase == DragPhase::Finished {
            return false;
        }
        // Treat non-finite coordinates as loss of position and clear the current proposal before distance arithmetic.
        if !position.0.is_finite() || !position.1.is_finite() {
            self.proposal = None;
            self.operation = DragOperation::Forbidden;
            return self.is_active();
        }
        if self.phase == DragPhase::Pending {
            let dx = position.0 - self.origin.0;
            let dy = position.1 - self.origin.1;
            if (dx * dx + dy * dy) > self.deadband * self.deadband {
                self.phase = DragPhase::Active;
            }
        }
        self.is_active()
    }

    /// Record the destination currently under the pointer (or `None`), and
    /// re-negotiate the operation. Ignored unless the drag is active.
    pub fn hover(&mut self, proposal: Option<DestinationProposal>) -> DragOperation {
        if !self.is_active() {
            return DragOperation::Forbidden;
        }
        self.operation = proposal.as_ref().map_or(DragOperation::Forbidden, |p| {
            negotiate(self.capabilities, &self.offered, p)
        });
        self.proposal = proposal;
        self.operation
    }

    /// Finishes the session on pointer release.
    ///
    /// Produces a drop only for an active non-empty drag over an accepting destination. Other first releases cancel; calls after the session is finished return `None`.
    pub fn release(&mut self) -> Option<SessionOutcome> {
        if self.phase == DragPhase::Finished {
            return None;
        }
        let outcome = match (&self.phase, &self.proposal) {
            (DragPhase::Active, Some(p))
                if self.operation != DragOperation::Forbidden && !self.items.is_empty() =>
            {
                SessionOutcome::Dropped(Box::new(DropRequest {
                    source: self.source.clone(),
                    destination: p.collection.clone(),
                    items: self.items.clone(),
                    slot: p.slot,
                    operation: self.operation,
                }))
            }
            _ => SessionOutcome::Cancelled,
        };
        self.finish();
        Some(outcome)
    }

    /// Cancels an unfinished session, returning [`SessionOutcome::Cancelled`]; finished sessions return `None`.
    pub fn cancel(&mut self) -> Option<SessionOutcome> {
        if self.phase == DragPhase::Finished {
            return None;
        }
        self.finish();
        Some(SessionOutcome::Cancelled)
    }

    fn finish(&mut self) {
        self.phase = DragPhase::Finished;
        self.proposal = None;
        self.operation = DragOperation::Forbidden;
    }
}

/// Removes duplicate item ids while preserving each first occurrence and its
/// relative order.
#[must_use]
pub fn dedup_preserving_order(items: Vec<ItemId>) -> Vec<ItemId> {
    let mut seen = HashSet::with_capacity(items.len());
    items
        .into_iter()
        .filter(|id| seen.insert(id.clone()))
        .collect()
}

/// Errors from [`reorder`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReorderError {
    /// The collection contains the same id twice, so reconciliation, selection
    /// and drag identity would be ambiguous
    DuplicateId(ItemId),
    /// A dragged id is not present in the collection.
    UnknownId(ItemId),
    /// The slot is beyond `len` (valid slots are `0..=len`).
    SlotOutOfRange {
        /// The offending slot.
        slot: usize,
        /// The collection length.
        len: usize,
    },
}

/// The result of moving `moved` into `slot` within a single collection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReorderDiff {
    /// The new full ordering.
    pub order: Vec<ItemId>,
    /// Whether the resulting order differs from the input.
    pub changed: bool,
}

/// Moves `moved` ids, preserving source order, into an insertion slot measured against the original ordering.
///
/// Returns [`ReorderError`] for duplicate collection ids, unknown moved ids, or slots outside `0..=order.len()`.
///
/// # Errors
///
/// [`ReorderError`] for duplicate collection ids, unknown moved ids, or a slot outside
/// `0..=order.len()`.
pub fn reorder(
    order: &[ItemId],
    moved: &[ItemId],
    slot: InsertionSlot,
) -> Result<ReorderDiff, ReorderError> {
    let mut seen = HashSet::with_capacity(order.len());
    for id in order {
        if !seen.insert(id) {
            return Err(ReorderError::DuplicateId(id.clone()));
        }
    }
    if slot.index() > order.len() {
        return Err(ReorderError::SlotOutOfRange {
            slot: slot.index(),
            len: order.len(),
        });
    }
    let moving: HashSet<&ItemId> = moved.iter().collect();
    for id in moved {
        if !seen.contains(id) {
            return Err(ReorderError::UnknownId(id.clone()));
        }
    }

    let before = order[..slot.index()]
        .iter()
        .filter(|id| moving.contains(id))
        .count();

    let mut kept: Vec<ItemId> = order
        .iter()
        .filter(|id| !moving.contains(id))
        .cloned()
        .collect();

    // Collect moved ids in source collection order, regardless of the caller’s selection order.
    let mut payload: Vec<ItemId> = order
        .iter()
        .filter(|id| moving.contains(id))
        .cloned()
        .collect();

    let at = slot.index() - before;
    let tail = kept.split_off(at);
    kept.append(&mut payload);
    kept.extend(tail);

    let changed = kept != order;
    Ok(ReorderDiff {
        order: kept,
        changed,
    })
}

/// Returns the insertion slot for a one-dimensional pointer position and layout-ordered `(start, size)` items.
///
/// Crossing an item midpoint advances the slot. Pointer and starts must share a coordinate space; clipping is caller-controlled. The default direction is left-to-right. Non-finite pointer positions return slot zero; non-finite starts and negative or non-finite sizes normalize to zero. Empty and zero-size inputs are accepted literally.
#[must_use]
pub fn slot_for_position_1d(position: f32, items: &[(f32, f32)]) -> InsertionSlot {
    slot_for_position_1d_with(position, items, Direction::LeftToRight)
}

/// Applies [`slot_for_position_1d`] with an explicit writing direction.
///
/// Right-to-left items remain in visual layout order, and crossing left of a midpoint advances the slot.
#[must_use]
pub fn slot_for_position_1d_with(
    position: f32,
    items: &[(f32, f32)],
    direction: Direction,
) -> InsertionSlot {
    if !position.is_finite() {
        return InsertionSlot(0);
    }
    let mut slot = 0;
    for (start, size) in items {
        let size = FiniteNonNeg::new_or(*size, 0.0).get();
        let start = if start.is_finite() { *start } else { 0.0 };
        let mid = start + size / 2.0;
        let past = match direction {
            Direction::LeftToRight => position >= mid,
            Direction::RightToLeft => position <= mid,
        };
        if past {
            slot += 1;
        } else {
            break;
        }
    }
    InsertionSlot(slot)
}

/// Returns the insertion slot for a two-dimensional pointer and layout-ordered item rectangles.
///
/// Rows are resolved top-to-bottom before the horizontal midpoint. Coordinate and malformed-input rules match [`slot_for_position_1d`]; horizontal direction defaults to left-to-right.
#[must_use]
pub fn slot_for_position_2d(position: (f32, f32), items: &[(f32, f32, f32, f32)]) -> InsertionSlot {
    slot_for_position_2d_with(position, items, Direction::LeftToRight)
}

/// Applies [`slot_for_position_2d`] with explicit horizontal direction; rows remain top-to-bottom.
#[must_use]
pub fn slot_for_position_2d_with(
    position: (f32, f32),
    items: &[(f32, f32, f32, f32)],
    direction: Direction,
) -> InsertionSlot {
    if !position.0.is_finite() || !position.1.is_finite() {
        return InsertionSlot(0);
    }
    let mut slot = 0;
    for (x, y, w, h) in items {
        let (x, y) = (
            if x.is_finite() { *x } else { 0.0 },
            if y.is_finite() { *y } else { 0.0 },
        );
        let w = FiniteNonNeg::new_or(*w, 0.0).get();
        let h = FiniteNonNeg::new_or(*h, 0.0).get();
        let past_row = position.1 >= y + h / 2.0;
        let on_row = position.1 >= y && position.1 < y + h;
        let x_mid = x + w / 2.0;
        let past_x_on_row = match direction {
            Direction::LeftToRight => position.0 >= x_mid,
            Direction::RightToLeft => position.0 <= x_mid,
        };
        if (past_row && !on_row) || (on_row && past_x_on_row) {
            slot += 1;
        } else {
            break;
        }
    }
    InsertionSlot(slot)
}

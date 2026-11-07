//! Caller-owned schedule events and overlap-layout results.

/// A stable, caller-owned event identity that is never derived from position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EventId(pub u64);

impl EventId {
    /// The raw key.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// A caller-owned event expressed as minute offsets and a zero-based column.
///
/// The payload is carried through layout unchanged, and the engine applies no wall-clock or timezone interpretation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduleEvent<P> {
    /// The event's stable identity.
    pub id: EventId,
    /// Which day column this event occupies (`0`-based).
    pub column: usize,
    /// Start, in minutes from the caller's origin.
    pub start_minute: i32,
    /// The end minute; reversed ranges collapse to a zero-duration event at `start_minute` during layout.
    pub end_minute: i32,
    /// Whether this event belongs to the separate all-day lane and is excluded from timed overlap layout.
    pub all_day: bool,
    /// Opaque caller payload, carried through layout untouched.
    pub payload: P,
}

impl<P> ScheduleEvent<P> {
    #[must_use]
    pub(crate) fn interval(&self) -> (i32, i32) {
        (self.start_minute, self.end_minute.max(self.start_minute))
    }
}

/// The lane allocation for one timed event.
///
/// `lane` is zero-based within its overlap cluster, and width is proportional to `lane_span / lane_count`.
///
/// `lane_span` is resolved per event: an event grows into the adjacent lanes
/// that stay free for its whole interval, so an event with no concurrent
/// neighbour is full width inside a cluster that a busier moment widened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaidOutEvent<P> {
    /// The event's stable identity, carried through from the input.
    pub id: EventId,
    /// Which day column the event occupies, carried through from the input.
    pub column: usize,
    /// 0-based lane index within the column's overlap cluster.
    pub lane: usize,
    /// Adjacent lanes occupied by this event: its own plus every following
    /// lane that stays empty for the event's whole interval. At least `1`,
    /// never more than `lane_count - lane`.
    pub lane_span: usize,
    /// Total lanes the event's overlap cluster needs.
    pub lane_count: usize,
    /// Start minute, carried through from the input.
    pub start_minute: i32,
    /// End minute (clamped to `>= start`), carried through from the input.
    pub end_minute: i32,
    /// Opaque caller payload, carried through untouched.
    pub payload: P,
}

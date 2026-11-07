//! Trust-boundary limits for queued accessibility actions and value payloads.

/// Bounds queue growth and `SetValue` text received across the accessibility trust boundary.
///
/// A queue-growing request is rejected when the queued count has reached
/// `max_pending`; adjacent same-target `SetValue` requests may still coalesce.
/// Eligible text payloads exactly at `max_payload_bytes` pass the size check,
/// and a zero payload cap permits only empty text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActionRouterBudget {
    /// Maximum queued-action count; reaching it rejects queue growth but still permits adjacent same-target `SetValue` coalescing.
    pub max_pending: usize,
    /// Maximum bytes checked for a live, writable `SetValue` text payload; longer text is refused, zero permits only empty text, and numeric payloads bypass the check.
    pub max_payload_bytes: usize,
}

impl ActionRouterBudget {
    /// Uses a 1,024-action queue and a 64 KiB `SetValue` payload limit.
    pub const DEFAULT: Self = Self {
        max_pending: 1024,
        max_payload_bytes: 64 * 1024,
    };
}

impl Default for ActionRouterBudget {
    fn default() -> Self {
        Self::DEFAULT
    }
}

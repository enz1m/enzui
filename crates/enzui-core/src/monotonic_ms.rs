//! Overflow-safe arithmetic for monotonic millisecond and [`std::time::Instant`] deadlines.
//!
//! Arithmetic uses caller-supplied snapshots; Linux clock sampling supports native presentation joins.

use std::time::{Duration, Instant};

/// Samples Linux `CLOCK_MONOTONIC` (clock id 1), without converting another clock domain.
///
/// # Errors
/// Refuses unsupported platforms, clock read failures, or malformed native timestamp fields.
pub fn presentation_clock_ns() -> std::io::Result<(i32, u128)> {
    presentation_clock_ns_for(1)
}

/// Samples the advertised Linux monotonic presentation clock, preserving its id and epoch.
///
/// Accepts `CLOCK_MONOTONIC` (1) and `CLOCK_MONOTONIC_RAW` (4); performs no epoch conversion.
///
/// # Errors
/// Refuses other clock ids, unsupported platforms, read failures and malformed timestamps.
#[allow(
    unsafe_code,
    reason = "`clock_gettime` on a caller-chosen clock has no safe wrapper"
)]
pub fn presentation_clock_ns_for(clock_id: i32) -> std::io::Result<(i32, u128)> {
    #[cfg(target_os = "linux")]
    {
        if !matches!(clock_id, libc::CLOCK_MONOTONIC | libc::CLOCK_MONOTONIC_RAW) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "unsupported presentation clock",
            ));
        }
        let mut value = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: value is a live, aligned timespec and the call writes only through that exclusive pointer.
        if unsafe { libc::clock_gettime(clock_id, &mut value) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        if value.tv_sec < 0 || !(0..1_000_000_000).contains(&value.tv_nsec) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid monotonic timestamp",
            ));
        }
        Ok((
            clock_id,
            value.tv_sec as u128 * 1_000_000_000 + value.tv_nsec as u128,
        ))
    }
    #[cfg(not(target_os = "linux"))]
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "native presentation clock is unavailable",
    ))
}

/// Milliseconds on the monotonic clock, as deadline arithmetic sees them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MonotonicMs(pub u64);

impl MonotonicMs {
    /// The clock origin.
    pub const ZERO: MonotonicMs = MonotonicMs(0);
    /// The latest representable instant.
    pub const MAX: MonotonicMs = MonotonicMs(u64::MAX);

    /// The milliseconds.
    #[must_use]
    #[inline]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// `delta` in whole milliseconds, saturating at `u64::MAX`.
    #[must_use]
    pub fn from_duration_saturating(delta: Duration) -> u64 {
        u64::try_from(delta.as_millis()).unwrap_or(u64::MAX)
    }

    /// `self + delta_ms`, or `None` on overflow.
    #[must_use]
    #[inline]
    pub const fn checked_add_ms(self, delta_ms: u64) -> Option<MonotonicMs> {
        match self.0.checked_add(delta_ms) {
            Some(v) => Some(MonotonicMs(v)),
            None => None,
        }
    }

    /// `self + delta_ms`, saturating at [`MonotonicMs::MAX`].
    #[must_use]
    #[inline]
    pub const fn saturating_add_ms(self, delta_ms: u64) -> MonotonicMs {
        match self.checked_add_ms(delta_ms) {
            Some(value) => value,
            None => MonotonicMs::MAX,
        }
    }

    /// The deadline `delta_ms` after `self`, named by `policy` when it overflows.
    #[must_use]
    #[inline]
    pub const fn deadline_from(self, delta_ms: u64, policy: Policy) -> MonotonicMs {
        match policy {
            Policy::Clamp => self.saturating_add_ms(delta_ms),
            Policy::Expire => match self.checked_add_ms(delta_ms) {
                Some(value) => value,
                None => MonotonicMs::ZERO,
            },
        }
    }

    /// Millisecond ticks until expiration under `boundary`, saturating at `u64::MAX`.
    ///
    /// Exclusive deadlines include one tick at equality. No clock rollback is
    /// detected; callers comparing snapshots must enforce their clock origin.
    #[must_use]
    pub const fn remaining_ms(self, deadline: MonotonicMs, boundary: Boundary) -> u64 {
        match boundary {
            Boundary::Inclusive => deadline.0.saturating_sub(self.0),
            Boundary::Exclusive => {
                if self.0 > deadline.0 {
                    0
                } else {
                    (deadline.0 - self.0).saturating_add(1)
                }
            }
        }
    }

    /// Whether `deadline` has been reached, with `boundary` deciding equality.
    #[must_use]
    #[inline]
    pub const fn is_expired(self, deadline: MonotonicMs, boundary: Boundary) -> bool {
        match boundary {
            Boundary::Inclusive => self.0 >= deadline.0,
            Boundary::Exclusive => self.0 > deadline.0,
        }
    }
}

/// Chooses whether equality counts as reaching a deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Boundary {
    /// Equality counts as reaching the deadline.
    Inclusive,
    /// Equality remains pending for one tick.
    Exclusive,
}

/// How to name a deadline whose arithmetic cannot fit into `u64` or
/// [`Instant`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// Clamps millisecond overflow to `u64::MAX` and represents `Instant` overflow as no deadline.
    Clamp,
    /// Treats overflow as already expired: zero milliseconds or `Some(now)` for an `Instant`.
    Expire,
}

/// Computes `now + delta` without panicking.
///
/// On overflow, [`Policy::Clamp`] returns `None` and [`Policy::Expire`] returns `Some(now)`.
#[must_use]
pub fn checked_instant_deadline(now: Instant, delta: Duration, policy: Policy) -> Option<Instant> {
    if let Some(deadline) = now.checked_add(delta) {
        return Some(deadline);
    }
    match policy {
        Policy::Clamp => None,
        Policy::Expire => Some(now),
    }
}

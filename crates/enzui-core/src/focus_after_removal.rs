//! Chooses a valid focus target after removing a row from a list.

/// Returns the successor at `removed_pos`, or the preceding row after tail removal.
/// Returns `None` when `new_len` is zero.
#[must_use]
pub fn focus_pos_after_removal(removed_pos: usize, new_len: usize) -> Option<usize> {
    if new_len == 0 {
        return None;
    }
    Some(removed_pos.min(new_len - 1))
}

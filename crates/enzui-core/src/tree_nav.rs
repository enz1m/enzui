//! Keyboard navigation for pre-flattened trees.
//!
//! Each row supplies depth and expansion state; parent relationships are inferred from the nearest preceding shallower row.

/// One row of a pre-flattened tree, reduced to what navigation needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeRow {
    /// Zero-based indentation depth.
    pub level: u8,
    /// Whether this row can be expanded (it is a branch, not a leaf).
    pub has_children: bool,
    /// Whether this row is currently expanded.
    pub expanded: bool,
}

impl TreeRow {
    /// A branch row at `level`, expanded or not.
    #[must_use]
    pub const fn branch(level: u8, expanded: bool) -> Self {
        Self {
            level,
            has_children: true,
            expanded,
        }
    }

    /// A leaf row at `level`.
    #[must_use]
    pub const fn leaf(level: u8) -> Self {
        Self {
            level,
            has_children: false,
            expanded: false,
        }
    }
}

/// A keyboard-navigation intent decoded from a key press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeKey {
    /// `ArrowDown` — next visible row.
    Down,
    /// `ArrowUp` — previous visible row.
    Up,
    /// `Home` — first row.
    Home,
    /// `End` — last row.
    End,
    /// `ArrowRight` — expand, or descend into the first child.
    Right,
    /// `ArrowLeft` — collapse, or ascend to the parent.
    Left,
    /// `Enter` or `Space` — activate the cursor row.
    Activate,
}

/// An action proposed to the host in response to a [`TreeKey`]; the host owns expansion state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeAction {
    /// Move the cursor to this row and select it.
    Move(usize),
    /// Toggle this row's expansion.
    Toggle(usize),
    /// Re-activate this row without moving (Enter / Space).
    Activate(usize),
}

/// Returns the nearest preceding row at a shallower level, or `None` for a root or invalid index.
#[must_use]
pub fn parent_of(rows: &[TreeRow], index: usize) -> Option<usize> {
    let level = rows.get(index)?.level;
    if level == 0 {
        return None;
    }
    rows[..index].iter().rposition(|r| r.level < level)
}

/// Returns the row’s 1-based `(position, set_size)` among siblings, matching ARIA `posinset` and `setsize`.
///
/// Returns `None` for an invalid index.
#[must_use]
pub fn sibling_position_of(rows: &[TreeRow], index: usize) -> Option<(usize, usize)> {
    let level = rows.get(index)?.level;
    let mut position = 1usize;
    for r in rows[..index].iter().rev() {
        if r.level < level {
            break;
        }
        if r.level == level {
            position += 1;
        }
    }
    let mut count = position;
    for r in &rows[index + 1..] {
        if r.level < level {
            break;
        }
        if r.level == level {
            count += 1;
        }
    }
    Some((position, count))
}

/// Resolves a keyed cursor after rows change.
///
/// An existing key follows its first exact match. A missing key uses `old_index` clamped to the final row, or the final row when `old_index` is absent. An empty tree returns `None`.
#[must_use]
pub fn reconcile_cursor(
    old_key: Option<&str>,
    old_index: Option<usize>,
    new_keys: &[&str],
) -> Option<usize> {
    let old_key = old_key?;
    if new_keys.is_empty() {
        return None;
    }
    if let Some(pos) = new_keys.iter().position(|k| *k == old_key) {
        return Some(pos);
    }
    let last = new_keys.len() - 1;
    Some(old_index.unwrap_or(last).min(last))
}

/// Returns the proposed action for a key press on a pre-flattened tree.
///
/// Movement does not wrap. Right expands or enters the first child; Left collapses or ascends. With no cursor, Down, Home, and Right select the first row; Up and End select the last; Left and Activate do nothing.
#[must_use]
pub fn tree_key_action(
    rows: &[TreeRow],
    cursor: Option<usize>,
    key: TreeKey,
) -> Option<TreeAction> {
    if rows.is_empty() {
        return None;
    }
    let last = rows.len() - 1;
    // A cursor past the end (the host shrank the tree) is treated as absent
    // rather than clamped silently onto a row the user never selected.
    let cursor = cursor.filter(|&i| i <= last);

    let Some(current) = cursor else {
        return match key {
            TreeKey::Down | TreeKey::Home | TreeKey::Right => Some(TreeAction::Move(0)),
            TreeKey::Up | TreeKey::End => Some(TreeAction::Move(last)),
            TreeKey::Left | TreeKey::Activate => None,
        };
    };

    match key {
        TreeKey::Down => (current < last).then(|| TreeAction::Move(current + 1)),
        TreeKey::Up => (current > 0).then(|| TreeAction::Move(current - 1)),
        TreeKey::Home => (current != 0).then_some(TreeAction::Move(0)),
        TreeKey::End => (current != last).then_some(TreeAction::Move(last)),
        TreeKey::Right => {
            let row = rows[current];
            if row.has_children && !row.expanded {
                Some(TreeAction::Toggle(current))
            } else if row.has_children && row.expanded {
                // Descend only when the next flattened row is deeper than the expanded branch.
                rows.get(current + 1)
                    .filter(|next| next.level > row.level)
                    .map(|_| TreeAction::Move(current + 1))
            } else {
                None
            }
        }
        TreeKey::Left => {
            let row = rows[current];
            if row.has_children && row.expanded {
                Some(TreeAction::Toggle(current))
            } else {
                parent_of(rows, current).map(TreeAction::Move)
            }
        }
        TreeKey::Activate => Some(TreeAction::Activate(current)),
    }
}

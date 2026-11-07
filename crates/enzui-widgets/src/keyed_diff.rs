//! Child-state reconciliation by key and compatible widget type.
//!
//! Existing keys retain state through insertion, removal, and equal-length
//! reorder; missing keys and type changes create fresh state. Duplicate keys
//! pair by occurrence order, so their individual identities remain positional.
//! Use [`enzui_core::identity::KeyedIds`] when repeated keys are legitimate.
//!
//! [`crate::keyed_row::KeyedRow`] and [`crate::grid::KeyedGrid`] check duplicate
//! keys during widget diff in debug builds unless their opt-out is enabled.

use iced::advanced::widget::tree::{Tag, Tree};
use std::collections::{HashMap, VecDeque};

/// Reports duplicated keys once each, in order of first repetition.
///
/// ```
/// use enzui_widgets::keyed_diff::duplicate_keys;
///
/// let unique = ["a".to_string(), "b".to_string()];
/// assert!(duplicate_keys(&unique).is_empty());
///
/// let repeated = ["a".to_string(), "b".to_string(), "a".to_string(), "a".to_string()];
/// assert_eq!(duplicate_keys(&repeated), vec!["a"]);
/// ```
pub fn duplicate_keys(keys: &[String]) -> Vec<&str> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    let mut dupes = Vec::new();
    for key in keys {
        let n = counts.entry(key.as_str()).or_insert(0);
        *n += 1;
        if *n == 2 {
            dupes.push(key.as_str());
        }
    }
    dupes
}

/// Reconciles retained children against new children by key and matching widget tag.
///
/// Supply one previous key per retained child and one new key per new child, in
/// corresponding order. The caller must store the new keys for the next diff.
/// Equal key lists and compatible tags reuse positional state without allocating
/// reconciliation collections; supplied callbacks may still allocate.
///
/// The general path consumes duplicate slots in occurrence order. Missing keys
/// or mismatched tags use `new_state`; removed state is dropped. Each new child
/// receives one `diff` call after reconciliation. `tag_of` must describe the state
/// type `new_state` constructs and `diff` accepts.
///
/// Key-list length mismatches are not validated. On the general path, a missing
/// new key creates fresh state and surplus previous keys have no retained slot.
/// Equal incomplete key lists can take the positional fast path, so callers must
/// keep keys and children aligned rather than relying on mismatch recovery.
pub fn reconcile_children_by_key<T>(
    current_children: &mut Vec<Tree>,
    previous_keys: &[String],
    new_keys: &[String],
    new_children: &mut [T],
    diff: impl Fn(&mut Tree, &mut T),
    tag_of: impl Fn(&T) -> Tag,
    new_state: impl Fn(&T) -> Tree,
) {
    // Equal keys still require compatible tags before any retained state is reused.
    if previous_keys == new_keys && current_children.len() == new_children.len() {
        let compatible = current_children
            .iter()
            .zip(new_children.iter())
            .all(|(tree, child)| tree.tag == tag_of(child));

        if compatible {
            for (child_state, new) in current_children.iter_mut().zip(new_children.iter_mut()) {
                diff(child_state, new);
            }
            return;
        }
    }

    let mut available: HashMap<&str, VecDeque<usize>> = HashMap::new();
    for (i, k) in previous_keys.iter().enumerate() {
        if i < current_children.len() {
            available.entry(k.as_str()).or_default().push_back(i);
        }
    }

    let mut retained: Vec<Option<Tree>> = std::mem::take(current_children)
        .into_iter()
        .map(Some)
        .collect();

    let mut rebuilt: Vec<Tree> = Vec::with_capacity(new_children.len());
    for (new_index, child) in new_children.iter().enumerate() {
        let claimed = new_keys
            .get(new_index)
            .and_then(|key| available.get_mut(key.as_str()))
            .and_then(std::collections::VecDeque::pop_front)
            .and_then(|old_index| retained.get_mut(old_index).and_then(Option::take))
            // Reject after consuming the occurrence so its slot cannot pass to a later duplicate.
            .filter(|existing| existing.tag == tag_of(child));

        rebuilt.push(match claimed {
            Some(existing) => existing,
            None => new_state(child),
        });
    }

    *current_children = rebuilt;

    for (child_state, new) in current_children.iter_mut().zip(new_children.iter_mut()) {
        diff(child_state, new);
    }
}

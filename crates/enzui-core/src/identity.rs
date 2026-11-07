//! Derives stable widget identifiers and injective owner-scoped namespaces.

use iced_core::widget::Id;
use std::collections::HashMap;

const SEP: &str = "-";

const OCCURRENCE: char = '#';

fn escape_occurrence(key: &str) -> String {
    if !key.contains(OCCURRENCE) {
        return key.to_owned();
    }
    key.replace(OCCURRENCE, "##")
}

/// Derive a stable widget [`Id`] from a namespace prefix and a logical key.
pub fn derive_id(prefix: &str, key: &str) -> Id {
    Id::from(format!("{prefix}{SEP}{key}"))
}

/// A stable [`Id`] for a part of the widget `id` (its error line, its
/// caption), so relations can name the part. Only meaningful for a stable
/// `id`: a unique one gives a new part id every frame.
#[must_use]
pub fn part_id(id: &Id, part: &str) -> Id {
    derive_id(&format!("{id:?}"), part)
}

/// The stable widget [`Id`] for an embedded web page node.
#[must_use]
pub fn web_page_node_id(view_slot: u64, node_key: &str) -> Id {
    derive_id("webpage", &format!("{view_slot}:{node_key}"))
}

/// Per-instance id deriver that keeps keys unique within one widget.
#[derive(Debug, Clone)]
pub struct KeyedIds {
    prefix: String,
    seen: HashMap<String, usize>,
}

impl KeyedIds {
    /// A deriver namespaced by `prefix`.
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            seen: HashMap::new(),
        }
    }

    /// The prefix this deriver namespaces with.
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// The [`Id`] for `key`, unique within this deriver.
    pub fn id_for(&mut self, key: &str) -> Id {
        let disambiguated = self.key_for(key);
        derive_id(&self.prefix, &disambiguated)
    }

    /// The disambiguated key [`id_for`](Self::id_for) would derive an [`Id`] from —
    /// escaped, and carrying its occurrence suffix — without the prefix.
    pub fn key_for(&mut self, key: &str) -> String {
        let count = self.seen.entry(key.to_owned()).or_insert(0);
        *count += 1;
        let escaped = escape_occurrence(key);
        if *count == 1 {
            escaped
        } else {
            format!("{escaped}{OCCURRENCE}{count}")
        }
    }
}

const LEN_SEP: char = ':';

// Length-prefix each component so distinct component sequences cannot share an
// encoding.
fn push_component(out: &mut String, component: &str) {
    use std::fmt::Write as _;
    // Formatting into a `String` is infallible.
    let _ = write!(out, "{}{LEN_SEP}{component}", component.len());
}

/// An injective namespace for the stable identity of one widget instance.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OwnerId {
    encoded: String,
    display: String,
}

impl OwnerId {
    /// An owner namespace for one instance of widget kind `kind`, distinguished by the
    /// host-supplied stable `key`.
    pub fn of_kind(kind: &str, key: &str) -> Self {
        let mut encoded = String::with_capacity(kind.len() + key.len() + 8);
        push_component(&mut encoded, kind);
        push_component(&mut encoded, key);
        Self {
            display: format!("{kind}#{key}"),
            encoded,
        }
    }

    /// A nested owner namespace under this one, for a sub-scope the host also names (a tree
    /// node's path segment, a nested group).
    #[must_use]
    pub fn child(&self, segment: &str) -> Self {
        let mut encoded = String::with_capacity(self.encoded.len() + segment.len() + 4);
        encoded.push_str(&self.encoded);
        push_component(&mut encoded, segment);
        Self {
            display: format!("{}/{segment}", self.display),
            encoded,
        }
    }

    /// The routable widget [`Id`] for the item named `role` inside this owner.
    pub fn id(&self, role: &str) -> Id {
        let mut encoded = String::with_capacity(self.encoded.len() + role.len() + 4);
        encoded.push_str(&self.encoded);
        push_component(&mut encoded, role);
        // Keep the readable tag diagnostic-only; the following length-prefixed encoding
        // determines identity.
        Id::from(format!("{}/{role} ‹{encoded}›", self.display))
    }

    /// The human-readable rendering of this owner (`"enz-banner#queue-7"`).
    pub fn display(&self) -> &str {
        &self.display
    }

    /// Resolves an optional prefix and key into an owner namespace.
    ///
    /// A key without a prefix uses `default_kind`; a prefix without a key uses an
    /// empty key. Returns `None` when both are absent.
    pub fn resolve(default_kind: &str, prefix: Option<&str>, key: Option<&str>) -> Option<Self> {
        match (prefix, key) {
            (Some(p), Some(k)) => Some(Self::of_kind(p, k)),
            (None, Some(k)) => Some(Self::of_kind(default_kind, k)),
            (Some(p), None) => Some(Self::of_kind(p, "")),
            (None, None) => None,
        }
    }
}

/// The [`Id`]s that appear more than once in `ids`, each reported once, in order of the
/// first repeat.
pub fn duplicates<'a>(ids: impl IntoIterator<Item = &'a Id>) -> Vec<Id> {
    let mut counts: HashMap<&Id, usize> = HashMap::new();
    let mut dupes = Vec::new();
    for id in ids {
        let n = counts.entry(id).or_insert(0);
        *n += 1;
        if *n == 2 {
            dupes.push(id.clone());
        }
    }
    dupes
}

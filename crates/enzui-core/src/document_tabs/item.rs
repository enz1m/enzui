//! Document-tab keys, item data, runtime state, and reconciliation errors.

/// Stable string identity for a document tab; keys must not be derived from positions.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DocumentKey(pub String);

impl DocumentKey {
    /// Build a key from anything string-like.
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    /// The underlying key.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for DocumentKey {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<String> for DocumentKey {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl std::fmt::Display for DocumentKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Application-owned data and flags for one document tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentItem {
    /// Stable identity; duplicate keys are rejected by
    /// [`DocumentTabs::reconcile`](super::DocumentTabs::reconcile).
    pub key: DocumentKey,
    /// Title displayed by the tab.
    pub title: String,
    /// Icon name; an empty string means no icon.
    pub icon: String,
    /// Show the loading spinner in place of the favicon.
    pub loading: bool,
    /// Places the tab in the fixed pinned cluster without changing its keyed runtime state.
    pub pinned: bool,
    /// Whether selection and focus navigation may target this tab.
    pub enabled: bool,
    /// Whether this tab offers a close affordance.
    pub closable: bool,
}

impl DocumentItem {
    /// Creates an enabled, closable, unpinned, idle tab with no icon.
    pub fn new(key: impl Into<DocumentKey>, title: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            title: title.into(),
            icon: String::new(),
            loading: false,
            pinned: false,
            enabled: true,
            closable: true,
        }
    }

    /// Set the favicon / icon name.
    #[must_use]
    pub fn icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = icon.into();
        self
    }

    /// Set the loading flag.
    #[must_use]
    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    /// Set the pinned flag.
    #[must_use]
    pub fn pinned(mut self, pinned: bool) -> Self {
        self.pinned = pinned;
        self
    }

    /// Set the enabled flag.
    #[must_use]
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Set whether the tab offers a close affordance.
    #[must_use]
    pub fn closable(mut self, closable: bool) -> Self {
        self.closable = closable;
        self
    }
}

/// Collection-owned transient state preserved by key across reconciliation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TabRuntime {
    /// Monotonic sequence value assigned when this key first appears and preserved while the
    /// key remains present.
    pub first_seen: u64,
    /// Number of times this key became selected; selecting it again while active does not
    /// increment the count.
    pub activations: u32,
}

/// An invalid document-tab collection update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentTabsError {
    /// The input contains the same [`DocumentKey`] more than once.
    DuplicateKey(DocumentKey),
}

impl std::fmt::Display for DocumentTabsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateKey(k) => write!(f, "duplicate document key: {k}"),
        }
    }
}

impl std::error::Error for DocumentTabsError {}

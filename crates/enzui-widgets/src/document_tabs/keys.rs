//! Document-tab reconciliation keys and realization limit.

use super::*;

/// Maximum session tabs realized in one frame, including overscan.
///
/// Pinned tabs are uncapped. Very narrow tabs or a wide viewport can exceed this
/// limit with finite, positive dimensions; the remaining cells are omitted
/// without an overflow indicator. Inspect [`EnzDocumentTabs::realized_was_capped`].
pub const MAX_REALIZED_SESSION: usize = 512;

/// Reserved reconciliation key for the leading session spacer.
///
/// Stable spacer keys prevent insertion or removal of a gap from shifting tab state.
pub const LEADING_SPACER_KEY: &str = "\u{0}spacer:leading";

/// Reconciliation key of the trailing spacer. See [`LEADING_SPACER_KEY`].
pub const TRAILING_SPACER_KEY: &str = "\u{0}spacer:trailing";

/// Reserved reconciliation key that preserves scrollable state as pinned tabs change.
pub const SESSION_STRIP_KEY: &str = "\u{0}session-strip";

/// Reserved reconciliation key for the fixed new-tab affordance.
pub const NEW_TAB_KEY: &str = "\u{0}new-tab";

/// Reserved reconciliation key for the vertical pinned-cluster wrapper.
pub const PINNED_CLUSTER_KEY: &str = "\u{0}pinned-cluster";

/// Derive a tab reconciliation key within its pinned or session partition.
///
/// The fixed prefix keeps document keys distinct from structural keys, even if
/// the document text contains NUL. Moving between partitions rebuilds widget
/// state; model [`TabRuntime`](enzui_core::document_tabs::TabRuntime) remains
/// keyed by the original document key.
#[must_use]
pub fn tab_key(key: &DocumentKey, pinned: bool) -> String {
    let ns = if pinned { "pinned" } else { "session" };
    format!("\u{0}{ns}\u{0}{}", key.as_str())
}

/// List pinned tab keys followed by the session-scrollable key.
///
/// This describes the horizontal outer collection without a new-tab affordance.
/// Vertical strips wrap the pinned cluster under [`PINNED_CLUSTER_KEY`] instead.
#[must_use]
pub fn outer_keys(model: &DocumentTabs) -> Vec<String> {
    let mut keys: Vec<String> = model
        .pinned()
        .map(|item| tab_key(&item.key, true))
        .collect();
    keys.push(SESSION_STRIP_KEY.to_owned());
    keys
}

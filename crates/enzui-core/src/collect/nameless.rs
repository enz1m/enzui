//! Frame-local diagnostics for actionable semantic nodes with neither an accessible name nor visible text.
//!
//! Every occurrence is retained on its collector, while console warnings are throttled once per role per process.

use super::SemanticsCollector;
use crate::semantics::Role;

/// One actionable node lacking both an accessible name and derivable visible text.
///
/// The optional debug-form widget id identifies the offender; `None` also indicates anonymous identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamelessActionable {
    /// The role reported to AT with no name to describe it.
    pub role: Role,
    /// The offending node's widget id rendered via `Debug`, or `None` when the
    /// node is also anonymous.
    pub id: Option<String>,
}

// Roles already warned on stderr; this process-wide state throttles printing only, not frame-local diagnostic records.
static PRINTED_ROLES: std::sync::Mutex<Vec<Role>> = std::sync::Mutex::new(Vec::new());

pub(in crate::collect) fn warn_nameless_actionable_once(role: Role) -> bool {
    let Ok(mut printed) = PRINTED_ROLES.lock() else {
        // Diagnostic warning throttling must not escalate a mutex-poisoning panic.
        return false;
    };
    if printed.contains(&role) {
        return false;
    }
    printed.push(role);
    log::warn!(
        target: "enzui::a11y",
        "enzui a11y: an ACTIONABLE semantic node (role {role:?}) was \
         published with NO accessible name, and it draws no visible text for \
         one to be derived from. An AT-SPI client reads such a node as a bare \
         role word, so a screen-reader user hears \"push button\" and cannot \
         tell what it does. Name-from-content cannot help an icon-only \
         control: give it an explicit name (`.aria_label(..)`, or \
         `Semantics::label(..)`)."
    );
    true
}

impl SemanticsCollector {
    pub(super) fn report_nameless_actionable(
        &mut self,
        role: Role,
        id: Option<&iced_core::widget::Id>,
    ) {
        self.nameless_actionable.push(NamelessActionable {
            role,
            id: id.map(|i| format!("{i:?}")),
        });
        let _printed = warn_nameless_actionable_once(role);
    }

    /// Returns every actionable nameless node observed in this frame, in first-seen order.
    ///
    /// Entries include repeated roles and node ids; the slice is frame-local rather than process-global.
    #[must_use]
    pub fn nameless_actionable(&self) -> &[NamelessActionable] {
        &self.nameless_actionable
    }

    /// Returns distinct actionable-but-nameless roles observed in this frame, in first-seen order.
    #[must_use]
    pub fn nameless_actionable_roles(&self) -> Vec<Role> {
        let mut roles: Vec<Role> = Vec::new();
        for n in &self.nameless_actionable {
            if !roles.contains(&n.role) {
                roles.push(n.role);
            }
        }
        roles
    }
}

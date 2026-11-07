//! Scoped focus operations and host-owned route focus bookkeeping.

use super::*;

/// The default outlet scope, used when a caller does not name one.
///
/// See [`route_content_widget_id_in`] for why the scope exists.
pub const DEFAULT_OUTLET_SCOPE: &str = "enzui-route-content";

/// Derive a content-region identity from outlet scope, route ID and depth.
/// The caller must use the same scope as the mounted outlet and keep live
/// outlet scopes distinct.
#[must_use]
pub fn route_content_widget_id_in(scope: &str, key: &RouteKey) -> iced::advanced::widget::Id {
    enzui_core::identity::derive_id(scope, &key.key_string())
}

/// Derive the content identity in [`DEFAULT_OUTLET_SCOPE`].
/// Use [`route_content_widget_id_in`] for simultaneously mounted outlets.
#[must_use]
pub fn route_content_widget_id(key: &RouteKey) -> iced::advanced::widget::Id {
    route_content_widget_id_in(DEFAULT_OUTLET_SCOPE, key)
}

/// Create an operation targeting the active route's scoped content region.
/// The host must run it against a mounted outlet using the same scope and path.
#[must_use]
pub fn focus_content_operation_in<T>(
    scope: &str,
    path: &RoutePath,
) -> Box<dyn iced::advanced::widget::Operation<T>>
where
    T: Send + 'static,
{
    Box::new(iced::advanced::widget::operation::focusable::focus(
        route_content_widget_id_in(scope, &path.active_key()),
    ))
}

/// [`focus_content_operation_in`] in the default scope.
#[must_use]
pub fn focus_content_operation<T>(path: &RoutePath) -> Box<dyn iced::advanced::widget::Operation<T>>
where
    T: Send + 'static,
{
    focus_content_operation_in(DEFAULT_OUTLET_SCOPE, path)
}

/// Derive a scoped back-control identity from route ID and depth.
/// This helper does not check whether an activatable header control is mounted.
#[must_use]
pub fn back_widget_id_in(scope: &str, key: &RouteKey) -> iced::advanced::widget::Id {
    enzui_core::identity::derive_id(scope, &format!("back:{}", key.key_string()))
}

/// A requested focus destination returned to the host after navigation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FocusTarget {
    /// Request the destination content region on forward or same-depth navigation,
    /// or on backward navigation without remembered focus.
    Content(iced::advanced::widget::Id),
    /// Request a remembered control on backward navigation; the host must ensure
    /// the control still exists in the destination tree.
    Restore(iced::advanced::widget::Id),
}

impl FocusTarget {
    /// The id to focus, whichever kind this is.
    #[must_use]
    pub fn id(&self) -> &iced::advanced::widget::Id {
        match self {
            Self::Content(id) | Self::Restore(id) => id,
        }
    }

    /// Create a focus operation; the host must run it against the destination tree.
    #[must_use]
    pub fn operation<T>(&self) -> Box<dyn iced::advanced::widget::Operation<T>>
    where
        T: Send + 'static,
    {
        Box::new(iced::advanced::widget::operation::focusable::focus(
            self.id().clone(),
        ))
    }
}

/// Collect the last named, focused control visited by an operation.
/// Always produces `Some(..)` so a host task continuation runs even without a
/// named target. Anonymous focused controls yield `Some(None)` unless another
/// named focused control is visited.
#[derive(Debug, Default)]
pub struct FindFocusedAlways {
    focused: Option<iced::advanced::widget::Id>,
}

impl iced::advanced::widget::Operation<Option<iced::advanced::widget::Id>> for FindFocusedAlways {
    fn focusable(
        &mut self,
        id: Option<&iced::advanced::widget::Id>,
        _bounds: Rectangle,
        state: &mut dyn iced::advanced::widget::operation::Focusable,
    ) {
        if state.is_focused()
            && let Some(id) = id
        {
            self.focused = Some(id.clone());
        }
    }

    fn traverse(
        &mut self,
        operate: &mut dyn FnMut(
            &mut dyn iced::advanced::widget::Operation<Option<iced::advanced::widget::Id>>,
        ),
    ) {
        operate(self);
    }

    fn finish(
        &self,
    ) -> iced::advanced::widget::operation::Outcome<Option<iced::advanced::widget::Id>> {
        iced::advanced::widget::operation::Outcome::Some(self.focused.clone())
    }
}

/// Host-owned capture and restoration metadata for one navigation outlet.
/// Use one instance per outlet: remembered route keys do not include the scope.
/// Navigation returns a requested target; it does not perform focus or verify
/// that a remembered control remains mounted. Entries at or deeper than the
/// destination are pruned after every navigation.
#[derive(Debug, Clone, Default)]
pub struct RouteFocus {
    remembered: std::collections::HashMap<String, iced::advanced::widget::Id>,
    current: Option<iced::advanced::widget::Id>,
}

impl RouteFocus {
    /// Empty bookkeeping.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the pending outgoing-page focus capture. `None` clears it.
    /// Supply the result of [`FindFocusedAlways`] before navigating.
    pub fn capture(&mut self, focused: Option<iced::advanced::widget::Id>) {
        self.current = focused;
    }

    /// What is currently captured, if anything.
    #[must_use]
    pub fn captured(&self) -> Option<&iced::advanced::widget::Id> {
        self.current.as_ref()
    }

    /// The remembered focus for a route key, if any.
    #[must_use]
    pub fn remembered_for(&self, key: &RouteKey) -> Option<&iced::advanced::widget::Id> {
        self.remembered.get(&key.key_string())
    }

    /// How many routes have a remembered focus. Bounded by live depth.
    #[must_use]
    pub fn remembered_len(&self) -> usize {
        self.remembered.len()
    }

    /// Record outgoing capture and return the destination focus request.
    /// Call once per path change. Backward navigation restores a remembered target
    /// when available; other moves request destination content. This clears the
    /// pending capture and prunes entries at or deeper than the destination. The
    /// host must apply the returned operation after mounting the destination.
    pub fn navigated(&mut self, scope: &str, from: &RoutePath, to: &RoutePath) -> FocusTarget {
        let from_key = from.active_key();
        let to_key = to.active_key();
        if from_key != to_key
            && let Some(id) = self.current.clone()
        {
            self.remembered.insert(from_key.key_string(), id);
        }

        let target = match from.transition_to(to) {
            Transition::Backward => self
                .remembered
                .get(&to_key.key_string())
                .cloned()
                .map_or_else(
                    || FocusTarget::Content(route_content_widget_id_in(scope, &to_key)),
                    FocusTarget::Restore,
                ),
            Transition::Forward | Transition::None => {
                FocusTarget::Content(route_content_widget_id_in(scope, &to_key))
            }
        };
        let keep_above = to.depth();
        self.remembered.retain(|k, _| {
            k.rsplit_once('@')
                .and_then(|(_, d)| d.parse::<usize>().ok())
                .is_some_and(|d| d < keep_above)
        });
        self.current = None;

        target
    }
}

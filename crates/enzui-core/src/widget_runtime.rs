//! Scoped ownership for widget state that must survive a tree replacement.
//!
//! [`WidgetRuntimeScope`] isolates unavoidable out-of-tree handoffs per runtime or fixture, and [`with_state`] accesses them by type. Dropping a scope restores its outer owner and drops its retained values.

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

#[derive(Default)]
struct RuntimeState {
    values: RefCell<HashMap<TypeId, Box<dyn Any>>>,
    active: RefCell<HashSet<TypeId>>,
}

thread_local! {
    static CURRENT: RefCell<Rc<RuntimeState>> = RefCell::new(Rc::new(RuntimeState::default()));
}

// Clears the reentrancy marker during normal return and unwinding.
struct ActiveType {
    owner: Rc<RuntimeState>,
    type_id: TypeId,
}

impl Drop for ActiveType {
    fn drop(&mut self) {
        self.owner.active.borrow_mut().remove(&self.type_id);
    }
}

struct StateLease<T: 'static> {
    active: ActiveType,
    value: Option<T>,
}

impl<T: 'static> Drop for StateLease<T> {
    fn drop(&mut self) {
        let value = self
            .value
            .take()
            .expect("widget runtime state lease must retain its value");
        let replaced = self
            .active
            .owner
            .values
            .borrow_mut()
            .insert(self.active.type_id, Box::new(value));
        debug_assert!(
            replaced.is_none(),
            "an active widget runtime state type must not be replaced"
        );
    }
}

/// Runs `f` with the active runtime’s state for `T`, isolated by [`TypeId`].
///
/// State is leased without holding a `RefCell` borrow, so callbacks may access other state types.
///
/// # Panics
/// Panics on recursive access to the same `T` in one runtime.
pub fn with_state<T, R>(f: impl FnOnce(&mut T) -> R) -> R
where
    T: Default + 'static,
{
    let owner = CURRENT.with(|current| Rc::clone(&current.borrow()));
    let type_id = TypeId::of::<T>();

    {
        let mut active = owner.active.borrow_mut();
        assert!(
            active.insert(type_id),
            "recursive access to the same widget runtime state type"
        );
    }
    // Installed before `T::default` can run, so even a panicking constructor
    // removes the active marker during unwinding.
    let active = ActiveType {
        owner: Rc::clone(&owner),
        type_id,
    };

    let stored = { owner.values.borrow_mut().remove(&type_id) };
    let value = stored
        .map(|value| {
            *value
                .downcast::<T>()
                .expect("widget runtime state type must match its TypeId")
        })
        .unwrap_or_default();
    let mut lease = StateLease {
        active,
        value: Some(value),
    };
    f(lease
        .value
        .as_mut()
        .expect("widget runtime state lease must retain its value"))
}

/// A scoped owner for out-of-tree widget handoffs; dropping it restores the previous scope, including during unwinding.
pub struct WidgetRuntimeScope {
    previous: Rc<RuntimeState>,
}

impl std::fmt::Debug for WidgetRuntimeScope {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WidgetRuntimeScope")
            .finish_non_exhaustive()
    }
}

impl WidgetRuntimeScope {
    /// Enter a fresh widget-runtime scope on the current thread.
    #[must_use]
    pub fn new() -> Self {
        let previous = CURRENT.with(|current| current.replace(Rc::new(RuntimeState::default())));
        Self { previous }
    }
}

impl Default for WidgetRuntimeScope {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for WidgetRuntimeScope {
    fn drop(&mut self) {
        CURRENT.with(|current| {
            current.replace(Rc::clone(&self.previous));
        });
    }
}

/// Runs an event loop inside one widget-state scope that is dropped when the loop returns.
///
/// An application enters this scope through [`crate::app_runtime::run`], which installs accessibility as well; this entry point is for a host that owns accessibility registration itself.
///
/// ```ignore
/// enzui_core::widget_runtime::run(|| {
///     iced::application(App::boot, App::update, App::view).run()
/// })
/// ```
pub fn run<R>(event_loop: impl FnOnce() -> R) -> R {
    let _scope = WidgetRuntimeScope::new();
    event_loop()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct First(u32);

    fn first() -> u32 {
        with_state::<First, _>(|state| state.0)
    }

    #[test]
    fn unwinding_restores_the_outer_owner() {
        let _test = WidgetRuntimeScope::new();
        with_state::<First, _>(|state| state.0 = 23);

        let result = std::panic::catch_unwind(|| {
            let _inner = WidgetRuntimeScope::new();
            with_state::<First, _>(|state| state.0 = 29);
            panic!("fixture panic");
        });

        assert!(result.is_err(), "positive control: the fixture must unwind");
        assert_eq!(first(), 23, "unwinding must restore the outer owner");
    }
}

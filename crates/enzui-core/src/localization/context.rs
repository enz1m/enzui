use super::{Arguments, Localizer};
use std::cell::RefCell;

thread_local! {
    static CURRENT: RefCell<Option<Localizer>> = const { RefCell::new(None) };
    static DEFAULT: Localizer = Localizer::default();
}

struct Restore(Option<Localizer>);

impl Drop for Restore {
    fn drop(&mut self) {
        CURRENT.with(|current| {
            current.replace(self.0.take());
        });
    }
}

impl Localizer {
    /// Applies this localizer to synchronous widget construction and runtime calls.
    ///
    /// Scope the whole event loop so lazily built overlays, accessibility and
    /// event handlers see the same locale. Nested scopes restore their predecessor
    /// during return or unwinding. Other threads and async tasks do not inherit it;
    /// pass a localizer explicitly to background work.
    pub fn scope<R>(&self, operation: impl FnOnce() -> R) -> R {
        let _restore = Restore(CURRENT.with(|current| current.replace(Some(self.clone()))));
        operation()
    }
}

/// The thread's scoped localizer, or a lazily initialized environment-based default.
#[must_use]
pub fn current() -> Localizer {
    CURRENT
        .with(|current| current.borrow().as_ref().cloned())
        .unwrap_or_else(|| DEFAULT.with(Clone::clone))
}

/// Returns the explicit scope, leaving environment defaults uninitialized when no scope exists.
#[must_use]
pub fn scoped() -> Option<Localizer> {
    CURRENT.with(|current| current.borrow().as_ref().cloned())
}

pub(crate) fn scoped_locale() -> Option<super::Locale> {
    CURRENT.with(|current| current.borrow().as_ref().map(Localizer::locale))
}

// `read` must not enter a new scope on this thread.
fn with_current<R>(read: impl FnOnce(&Localizer) -> R) -> R {
    CURRENT.with(|current| match current.borrow().as_ref() {
        Some(localizer) => read(localizer),
        None => DEFAULT.with(read),
    })
}

/// Resolves framework or app text through the current thread's localizer.
#[must_use]
pub fn text(key: &str) -> String {
    with_current(|localizer| localizer.text(key))
}

/// Resolves parameterized text through the current thread's localizer.
#[must_use]
pub fn text_with(key: &str, arguments: &Arguments<'_>) -> String {
    with_current(|localizer| localizer.text_with(key, arguments))
}

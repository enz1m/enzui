//! Routes AccessKit requests through per-window node maps to queued widget actions.
//!
//! [`ActionRouter`] shares maps and its queue across threads and never falls back
//! to another window when a target window has no published tree.

mod budget;
mod resolve;
mod router;

pub use budget::*;
pub use resolve::*;
pub use router::*;

/// Window token for unkeyed single-window hosts; zero is an ordinary key, not a wildcard.
pub const A11Y_UNKEYED_WINDOW: u64 = 0;

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) mod interleave {
    use std::cell::RefCell;

    type Hook = Box<dyn FnMut() + 'static>;

    thread_local! {
        static BEFORE_ENQUEUE: RefCell<Option<Hook>> = const { RefCell::new(None) };
    }

    pub(crate) fn before_enqueue() {
        let hook = BEFORE_ENQUEUE.with(|slot| slot.borrow_mut().take());
        if let Some(mut hook) = hook {
            hook();
            BEFORE_ENQUEUE.with(|slot| {
                *slot.borrow_mut() = Some(hook);
            });
        }
    }
}

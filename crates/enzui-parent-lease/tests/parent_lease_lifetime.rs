//! Lease-lifetime and surface-replacement rules every parented portal adopter
//! obeys, exercised against a witness lease that records its own destruction.
//!
//! These cover the adopter's ownership rules, not the Wayland protocol: the
//! xdg-foreign round trip and the exported object's teardown are covered by the
//! protocol fixture in `enzui-wayland`.

#![allow(
    missing_docs,
    unreachable_pub,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::panic,
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    clippy::print_stdout,
    clippy::string_slice,
    clippy::missing_assert_message,
    reason = "a test crate reports by panicking and printing, and none of it is public API"
)]

use std::cell::RefCell;
use std::rc::Rc;

use enzui_parent_lease::{
    ParentLease, ReleaseCause, RequestGeneration, ResponseVerdict, SurfaceIncarnation,
};

// Stand-in for the exported protocol object; records every destruction so a
// lease released twice or never released is visible.
#[derive(Debug)]
struct Witness {
    name: &'static str,
    log: Rc<RefCell<Vec<(&'static str, ReleaseCause)>>>,
    dropped_undestroyed: Rc<RefCell<Vec<&'static str>>>,
    destroyed: bool,
}

impl Witness {
    fn new(
        name: &'static str,
        log: &Rc<RefCell<Vec<(&'static str, ReleaseCause)>>>,
        dropped_undestroyed: &Rc<RefCell<Vec<&'static str>>>,
    ) -> Self {
        Self {
            name,
            log: Rc::clone(log),
            dropped_undestroyed: Rc::clone(dropped_undestroyed),
            destroyed: false,
        }
    }

    fn destroy(mut self, cause: ReleaseCause) {
        self.log.borrow_mut().push((self.name, cause));
        self.destroyed = true;
    }
}

impl Drop for Witness {
    fn drop(&mut self) {
        if !self.destroyed {
            self.dropped_undestroyed.borrow_mut().push(self.name);
        }
    }
}

struct Harness {
    log: Rc<RefCell<Vec<(&'static str, ReleaseCause)>>>,
    leaked: Rc<RefCell<Vec<&'static str>>>,
}

impl Harness {
    fn new() -> Self {
        Self {
            log: Rc::new(RefCell::new(Vec::new())),
            leaked: Rc::new(RefCell::new(Vec::new())),
        }
    }

    fn lease(&self, name: &'static str) -> Witness {
        Witness::new(name, &self.log, &self.leaked)
    }

    fn leaked(&self) -> Vec<&'static str> {
        self.leaked.borrow().clone()
    }
}

const S1: SurfaceIncarnation = SurfaceIncarnation(1);
const G1: RequestGeneration = RequestGeneration(1);
const G2: RequestGeneration = RequestGeneration(2);

#[test]
fn a_response_from_a_superseded_request_is_refused() {
    let h = Harness::new();
    let mut slot = ParentLease::new();
    let _ = slot.install(h.lease("first"), S1, G1);
    let superseded = slot.install(h.lease("second"), S1, G2).expect("held");
    superseded.lease.destroy(superseded.cause);

    assert_eq!(
        slot.verdict(G1, S1),
        ResponseVerdict::StaleGeneration {
            held: G2,
            arrived: G1
        }
    );
    assert_eq!(slot.verdict(G2, S1), ResponseVerdict::Accept);

    let released = slot.settle().expect("held");
    released.lease.destroy(released.cause);
    assert!(h.leaked().is_empty());
}

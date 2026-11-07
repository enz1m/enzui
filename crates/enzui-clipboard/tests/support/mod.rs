//! Fixtures for the ordinary-clipboard acceptance: a private compositor, the
//! peer application driver, and the runner this `harness = false` target needs.

pub mod compositor;
pub mod peer;
pub mod runner;

// Include only the scratch owner to avoid the renderer dependency graph.
#[path = "../../../enzui-test-support/src/test_scratch.rs"]
pub mod test_scratch;

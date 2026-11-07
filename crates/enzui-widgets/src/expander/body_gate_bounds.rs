//! `BodyGate` must be layout-transparent on both length
//! axes. It forwards the child's `size()` and `layout()` under the parent's
//! limits and introduces no length of its own; the only node it synthesizes
//! is the suppressed branch, `layout::Node::new(cached_size)` — a concrete,
//! finite `Size`, never a `Fill`.
//!
//! This is the `Fill`-under-bound check. It probes the private widget
//! directly rather than through the whole expander, so the assertion is
//! about BodyGate's own layout, not the composite.

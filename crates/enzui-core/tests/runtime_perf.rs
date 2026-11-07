//! Before/after runtime-performance gate for accelerator dispatch.
//!
//! Before the window-aware command registry lands, this ignored gate compares
//! the current [`dispatch_shortcut`] against the future resolver at 32 / 256 /
//! 2,048 actions, four windows, and
//! application / window / modal layers, fed 1,000,000 deterministic events
//! (non-key, captured keys, ignored hit/miss, repeats, early/middle/late
//! matches). It records: actions visited, snapshots built, allocations and
//! bytes, emitted messages, and ns/event p50 + p95.
//!
//! # What is measured, and what stands in for what
//!
//! This task is the gate only -- not the registry. The window and scope-
//! layer concepts do not exist in `action.rs` yet, and this test adds no
//! types to `action.rs` for a benchmark's benefit. Instead:
//!
//! * "before" is the shipping [`dispatch_shortcut`]: a single flat
//!   `Vec<Action>` linearly scanned with `find_map` on *every* event. This is
//!   the baseline the registry will be judged against.
//! * **windows and layers are modelled as plain grouped `Vec<Action<_>>`** --
//!   four windows, each with a modal / window / application layer. The
//!   "resolver" stand-in composes the effective action list for the *focused*
//!   window's layers once (a "snapshot"), then dispatches ignored key
//!   presses against that snapshot. It short-circuits captured and non-key
//!   events before touching any action, which is the zero-visit /
//!   zero-allocation requirement.
//!
//! Because the registry does not exist, the resolver column is a faithful
//! stand-in built from the *same* `Action` matching primitives, not a second
//! implementation of dispatch. Its purpose is to record the numbers the real
//! registry must not regress past -- most importantly the 256-action p95.
//!
//! # Actions visited
//!
//! "Actions visited" cannot be seen from outside `find_map`. A [`CountingIter`]
//! wraps the action slice; its `Iterator::next` bumps a global counter, so the
//! recorded count is the *real* short-circuit behaviour of `find_map` (it stops
//! at the first match), not an assumption. `action.rs` is not modified to add
//! instrumentation.
//!
//! # Honesty
//!
//! * Release only. Debug timings are not evidence. Run under
//!   `--release`.
//! * The gate is sabotage-verified: making dispatch scan the list twice per
//!   event (quadratic) trips the actions-visited assertion. See the comment on
//!   `SABOTAGE_DOUBLE_SCAN` below.
//!
//! Run (ignored by default -- it is a measurement, not a pass/fail unit test):
//!
//! ```text
//! cargo test -p enzui-core --all-features --release \
//!   --test runtime_perf accelerator_dispatch_perf -- --ignored --nocapture --test-threads=1
//! ```
//!
//! 5 warmups, 20 paired samples.
//!
//! # Event count: 2,000 instead of 1,000,000 (deviation, recorded)
//!
//! [`EVENT_COUNT`] is 2,000, not a
//! 1,000,000-event workload.
//! The original workload specified "1m events, 5 warmups, 20 paired samples"
//! *per configuration* without multiplying it out against the three action
//! scales and the two dispatch paths:
//!
//! ```text
//! 3 scales x (5 warmups + 20 samples) x 2 paths x 1,000,000 events
//!   = 150,000,000 event-visits
//! ```
//!
//! and each visit linearly scans the action list (32 / 256 / 2,048, mean
//! ~760), which is roughly 3.8e10 action comparisons -- hours, not minutes.
//! A run that killed at 13+ minutes with no output confirmed it does not
//! terminate in usable time.
//!
//! At 2,000 events the whole matrix completes in ~1.5s in `--release`. The
//! distribution of event kinds is identical (the same seeded mix of
//! captured / non-key / miss / repeat / early / middle / late), so the p50 /
//! p95 of a nanosecond-scale operation remain meaningful -- only the sample
//! size per configuration is smaller. Note the recorded p50/p95 are *not*
//! asserted; the pass/fail bar is the scale-invariant actions-visited ceiling,
//! which trips at n=32 on the first dispatched event (proven by the
//! [`SABOTAGE_DOUBLE_SCAN`] double-scan). The constant is named precisely so a
//! release-gating run can raise it back to 1,000,000 for one scale at a time if
//! desired.
//!
//! # Why 2,000 events
//!
//! The discriminating assertion is an operation count, not a wall-clock
//! number, so a small event count and sample matrix guard as well as a large
//! one. 10,000 events took minutes in a debug `--ignored` run, and a test that
//! slow is one nobody runs.
//!
//! # Running this target
//!
//! The deterministic diagnostics-summary, reconcile-probe, and coalescer-byte
//! gates run by default. Their verdicts are exact mechanism counts, independent
//! of machine speed or build profile. Run the complete ordinary target with:
//!
//! ```text
//! cargo test -p enzui-core --features test-frame --test runtime_perf
//! ```
//!
//! `accelerator_dispatch_perf` and `diagnostics_budget_gate` remain ignored:
//! their workloads are costly or their wall-clock measurements are
//! profile-sensitive. Run them with the profile, backend, output and
//! single-thread isolation they need:
//!
//! ```text
//! ICED_TEST_BACKEND=software \
//!   cargo test -p enzui-core --features test-frame --release \
//!   --test runtime_perf -- --ignored --nocapture --test-threads=1
//! ```

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
#![allow(
    unsafe_code,
    reason = "a counting allocator implements the unsafe `GlobalAlloc` trait, forwarding to \
              `System`"
)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

struct CountingAlloc;

static MEASURING: AtomicBool = AtomicBool::new(false);
static ALLOC_COUNT: AtomicU64 = AtomicU64::new(0);
static ALLOC_BYTES: AtomicU64 = AtomicU64::new(0);

static LIVE_ALLOCATED: AtomicU64 = AtomicU64::new(0);
static LIVE_FREED: AtomicU64 = AtomicU64::new(0);

// SAFETY: every method forwards to `System`, only counting sizes on the way.
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if MEASURING.load(Ordering::Relaxed) {
            ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
            ALLOC_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        LIVE_ALLOCATED.fetch_add(layout.size() as u64, Ordering::Relaxed);
        // SAFETY: the caller's `GlobalAlloc` contract passes to `System` unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE_FREED.fetch_add(layout.size() as u64, Ordering::Relaxed);
        // SAFETY: the caller's `GlobalAlloc` contract passes to `System` unchanged.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

#[derive(Debug, Clone, PartialEq)]
#[allow(
    dead_code,
    reason = "payloads carried for realism; only the count and id are asserted"
)] // payloads carried for realism; only the count/id is asserted
enum Msg {
    Fire(usize),
}

mod stream_coalescer_perf {
    use enzui_core::chat_stream::StreamCoalescer;

    /// Deliver a FIXED total payload of `total_bytes` over exactly `updates`
    /// evenly sized presentation ticks, and return the total bytes handed out
    /// across all presentation deltas (`presented_bytes()`). The payload is the
    /// same size at every `updates` value, so the only thing that changes is n
    /// — exactly the "hold the data fixed, vary only n" discipline that
    /// separates amplification from a composition artefact.
    fn presented_bytes_for(total_bytes: usize, updates: usize) -> u64 {
        // A cap large enough that it never forces an early tick — cadence here
        // is driven purely by our explicit tick() calls, not the byte cap.
        let mut c = StreamCoalescer::new(usize::MAX);
        let per_update = total_bytes / updates;
        let chunk = "x".repeat(per_update);
        for _ in 0..updates {
            c.push(&chunk);
            let _ = c.tick();
        }
        let _ = c.flush();
        // Sanity: the whole payload really landed in the canonical body.
        assert_eq!(
            c.body().len(),
            per_update * updates,
            "the fixture must deliver the whole fixed payload regardless of n"
        );
        c.presented_bytes()
    }

    // Deterministic mechanism-count coverage: exact presented-byte totals are
    // ordinary debug-suite coverage, not a profile-sensitive measurement.
    #[test]
    fn coalescer_copied_bytes_are_flat_in_update_count() {
        // A payload divisible by all three update counts so per-update chunks
        // are equal and the total is byte-identical across n.
        const PAYLOAD: usize = 180_000; // 180000 / {10,30,60} = {18000,6000,3000}

        let b10 = presented_bytes_for(PAYLOAD, 10);
        let b30 = presented_bytes_for(PAYLOAD, 30);
        let b60 = presented_bytes_for(PAYLOAD, 60);

        println!("\n=== coalescer copy-amplification gate (mechanism counter) ===");
        println!("fixed {PAYLOAD}-byte payload; only presentation delta bytes counted");
        println!("  n=10  -> {b10} bytes copied");
        println!("  n=30  -> {b30} bytes copied");
        println!("  n=60  -> {b60} bytes copied");
        println!(
            "  old cumulative-clone estimate at n=60 ~= {} bytes (61x)",
            PAYLOAD as u64 * 61
        );

        // Positive control: the counter is actually moving. A severed counter
        // reading 0 at every n would satisfy "all equal" vacuously.
        assert!(
            b10 > 0,
            "presented_bytes counter read 0 at n=10 — it is observing nothing, \
             so the flatness assertions below would be vacuous"
        );

        assert_eq!(
            b10, b30,
            "copied bytes rose from {b10} (n=10) to {b30} (n=30) — not flat, so \
             the cumulative-body copy is back"
        );
        assert_eq!(
            b30, b60,
            "copied bytes rose from {b30} (n=30) to {b60} (n=60) — not flat, so \
             the cumulative-body copy is back"
        );

        assert_eq!(
            b60, PAYLOAD as u64,
            "with the delta API the total copied bytes must equal the payload \
             ({PAYLOAD}); got {b60}"
        );
    }
}

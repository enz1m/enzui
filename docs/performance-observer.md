# Input-to-frame performance observer

The enzICED fork exposes an opt-in timing spine behind iced's
`performance-observer` Cargo feature. Compiling the feature does not start
collection. Until `iced::performance::enable(Config { .. })` is called, every
runtime hook is a cold atomic branch and allocates nothing.

```rust
use iced::performance::{self, Config};

performance::enable(Config::default());
// Run the interaction.
print!("{}", performance::to_ndjson());
performance::disable();
```

The winit runtime timestamps input arrival, event-queue wait, input dispatch,
invalidation, view rebuild, tree diff, layout, draw/primitive preparation, GPU
submission and the strongest available frame milestone against one monotonic
origin and iced's stable window id. `to_ndjson()` emits one JSON object per
observed window.

For one window, use `snapshot(WindowId(window_id.as_u64()))` with the actual
`iced::window::Id` returned by the runtime. Collection is process-wide;
`enable` starts a new session, and `disable` stops collection without clearing
the retained results.

## Timestamp quality is part of the result

`input_to_milestone` is deliberately not named `input_to_visible`. Read
`presentation_quality` with every result. It records the strongest milestone
seen, so use `input_to_milestone_by_quality` for latency comparisons when a
window has samples of different qualities:

- `Submitted`: commands reached the GPU queue, but there is no surface or
  compositor result. In plain words, `Submitted` is **not** "the user saw it":
  the frame may still be waiting on the surface, the compositor, or vsync.
  Offscreen benchmarks report this.
- `SurfacePresented`: the backend returned from its surface presentation call.
  For wgpu this means `SurfaceTexture::present()` returned; it still does **not**
  prove the compositor displayed the frame.
- `CompositorPresented`: an optional platform integration supplied actual
  compositor feedback. The common path never invents this value.

Use an external display/photodiode/capture clock for visible-response claims.
When compositor feedback is unavailable, publish the external measurement and
the observer's submit/surface milestone side by side, not as interchangeable
numbers.

## Bounds and allocation behavior

`Config` bounds both dimensions that retain memory:

- `trace_capacity` defaults to 4,096 events. It is preallocated at `enable`;
  ring wrap increments `dropped_trace_events` and never silently changes the
  histogram population.
- `max_windows` defaults to 64. Window state and all histograms are preallocated
  at `enable`; events for additional windows increment
  `dropped_window_events`.
- Every duration uses a fixed 257-bucket logarithmic HDR histogram (four
  buckets per power of two through `u64::MAX`). Histograms never evict samples;
  snapshots expose count, p50, p95, p99 and exact max.

`Config::payload_memory_cap_bytes()` reports the architecture-specific payload
cap and each JSON snapshot repeats it. Allocator bookkeeping around the two
backing buffers is implementation-specific. Collection performs no steady
per-event or per-frame allocation; snapshots/JSON output allocate because they
are explicitly requested.

## Release measurement entry point

Representative workloads include shell label clicks, modal typing, a
10,000-row virtual-list scroll, a 1 MiB reading pane and a ten-second 60 Hz
animation. Visible-response claims require an external capture harness in
addition to this observer. Do not use a
headless layout duration or the offscreen `Submitted` milestone as that external
baseline. Record machine/GPU/backend, release profile, resolution, scale,
sample count, CPU/RSS/VRAM and allocations with every published run.

## Repeatable workload reports

`enzui_core::perf_report` (feature `perf-report`, separate from `test-frame`
so enabling `test-frame` alone does not compile the observer) drives scripted
page switches, animations, scrolling and idle holds. It layers report generation
on the `perf_frame` harness and uses the observer's
`HdrHistogram`/`HistogramSnapshot` types.

The following fragment assumes `app` implements `perf_frame::PerfSubject` and
its message type accepts `Msg::Scroll(u16)`:

```rust
use enzui_core::perf_report::{CaptureProvenance, Workload, WorkloadDriver};
use iced_core::Size;

let capture = CaptureProvenance::from_env().expect("runner supplied complete provenance");
let mut driver = WorkloadDriver::new(app, Size::new(1200.0, 800.0), 1.0, capture);
let report = driver
    .run_observed(&Workload::busy_scroll(
        "virtual list",
        "5000 rows, 300 realised",
        (0..64u16).map(Msg::Scroll),
    ))
    .expect("a non-empty workload reports");
println!("{}", report.to_json());
```

Each `WorkloadReport` records **workload identity** (app + interaction kind +
dataset), **provenance** (app name/version/revision, framework and vendored-iced
revisions, lock SHA-256, build config, enabled features, exact binary path/mtime
and SHA-256, profile, backend, GPU, resolution and scale), **frame count**,
**p50/p95/p99/max** paint and layout
histograms, the worst per-frame routed-page rebuild count, **CPU/RSS** (from
`/proc`, `VRAM` where a GPU backend reports it), the milestone quality, dropped
events, and an optional **external capture** cadence. `run_observed` wraps the
same offscreen paint path in the runtime observer and folds its `WindowSnapshot` in
through `ObserverSummary::from_snapshot`, adding requested/drawn/milestone
counts, frame-interval cadence, latency from injected input to submission and p50/p95/p99/
max histograms for view rebuild, tree diff, layout, draw preparation and submit.
Both methods report `Submitted` milestones; neither measures native window
presentation. Plain `run` remains available when only offscreen processing
histograms are needed.

Report and comparison APIs reject invalid measurement inputs:

* an **empty workload** (no measured frame) is `PerfError::EmptyWorkload`, never
  a report of zero;
* a **`Submitted`-only** milestone asked to speak for what the user saw is
  `PerfError::SubmittedClaimedVisible`. The observer keeps separate exact-
  quality histograms; `visible_input_latency()` accepts one homogeneous
  `SurfacePresented` or `CompositorPresented` histogram and fails closed with
  `MixedMilestoneQualities` if any different qualities are mixed. A
  single stronger sample therefore cannot relabel an aggregate; `with_observer`
  also does not promote the report's own paint quality;
* a comparison touching a run that **dropped** trace/window events is
  `PerfError::DroppedTraces`;
* an **underpowered** (fewer than `MIN_COMPARABLE_FRAMES`) or **cross-backend /
  cross-profile / cross-GPU / cross-workload identity / cross-build-config /
  cross-feature / cross-lock / cross-presentation-quality / cross-resolution /
  cross-scale** comparison is `PerfError::Underpowered` /
  `PerfError::Incomparable`. A `paint.count` / `frame_count` disagreement is
  corruption (`PerfError::CorruptSamples`), not underpowered evidence.

Despite its name, `visible_input_latency()` accepting `SurfacePresented` does
not prove the frame was visible. It validates sample quality and consistency;
the surface milestone still has the limitation described above. `compare`
compares paint-time histograms; it is not an external visibility measurement.

Reporting is test-only, as is the headless collection frame: a shipping
dependency table must not enable `enzui-core/test-frame`, `perf-report` or
`a11y-hosted-test`, or depend on a harness crate (`iced_test`, `iced_tester`,
`enzui-test-support`). An application repository that publishes reports
validates them, and that its shipping tables stay free of these features, with
its own tooling.

### Capture identity

`CaptureProvenance::from_env` requires `ENZUI_PERF_APP_NAME`,
`ENZUI_PERF_APP_VERSION`, `ENZUI_PERF_APP_REV`, `ENZUI_PERF_FRAMEWORK_REV`,
`ENZUI_PERF_VENDOR_REV`, `ENZUI_PERF_LOCK_SHA256`, `ENZUI_PERF_CONFIG`,
`ENZUI_PERF_FEATURES`, and `ENZUI_PERF_GPU`. Empty or `unknown` values are
rejected. The lock digest must be a SHA-256 hex value. Use the actual release
binary and a representative workload for published application measurements;
record the exact command and capture environment with the result.

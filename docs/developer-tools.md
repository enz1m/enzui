# Developer tools

Tools for inspecting a running enzUI application. Enable capture and tracing
explicitly through the features and environment switches below. Development
checks and runtime diagnostics have their own defaults.

## iced's developer tools and Comet

iced's devtools add F12 to an application: it opens
[Comet](https://github.com/iced-rs/comet), iced's companion debugger for
frame timings, message rates and the update and view costs behind them (and,
with `time-travel`, stepping back through messages). The first time, the
devtools offer to install Comet with `cargo install`.

enzUI's `app_setup::run` starts the event loop itself, so attach the devtools
between its two halves, behind a feature of your application:

```toml
[features]
devtools = ["iced/debug", "dep:iced_devtools"]

[dependencies]
iced_devtools = { git = "https://github.com/enz1m/enzim-iced.git", rev = "67f991b7442cbd79821141069c3b864cc12413f2", optional = true }
```

```rust
use enzui_widgets::app_setup::{self, AppServices};

fn main() -> iced::Result {
    let program = app_setup::with_services(
        iced::application(App::new, App::update, App::view),
        AppServices::default(),
    );
    #[cfg(feature = "devtools")]
    let program = iced_devtools::attach(program);
    app_setup::run_program(program)
}
```

`cargo run --features devtools`, then F12. Keep it out of release builds:
`iced/debug` also turns on iced's timing instrumentation and the Comet
connection. The message type must implement `Debug`.

## Environment switches

| variable | what it does |
|---|---|
| `RUST_LOG` | which diagnostics the default logger prints; see [logging.md](logging.md) |
| `ENZUI_DIAGNOSTICS=1` | paint the accessibility (elided text, and controls wrapped in hit-target or contrast probes), layout-spill and overlap audit findings over the window, and log one audit report under `enzui::trace::diagnostics` |
| `ENZUI_FRAME_COST=1` | log shell layout durations and frame intervals under `enzui::trace::frame_cost` |
| `ENZUI_TEXT_DIRECTION=rtl` | lay the application out right to left, whatever the locale |
| `ENZUI_SSD=1` | let the desktop draw the titlebar of shell windows where it decorates windows (KDE, sway), instead of the shell's own titlebar everywhere |
| `ICED_BACKEND=tiny-skia` | render in software instead of on the GPU (`wgpu`) |
| `ICED_PRESENT_MODE=fifo` | the GPU present mode: `vsync`, `no_vsync`, `immediate`, `fifo`, `fifo_relaxed` or `mailbox` |
| `ICED_OVERLAY_POPUPS=0` | draw menus, lists, tooltips and popovers inside the window on Wayland too, instead of in popup surfaces |
| `ENZUI_MOTION_SCALE=4` | multiply every animation's duration, to watch motion slowly |
| `ENZUI_RENDER_PASS_TRACE=1` | print each presented frame's render passes and command buffers |
| `ENZUI_DISABLE_LAYOUT_MEMO=1` | turn off layout reuse, to rule it out when something lays out wrong |
| `ENZUI_CACHED_SUBTREE_BYPASS=1` | turn off cached subtrees, to rule them out when something does not update |
| `ENZUI_CACHED_SUBTREE_NESTING_CHECK=0` | skip the nested cached-subtree check |
| `ENZUI_DISABLE_RESPONSIVE_ISLAND` | route docked shell motion through a full relayout, for comparison with `ENZUI_FRAME_COST` |
| `ICED_PRESENTATION_TRACE=<file>` | append one JSON record per frame the compositor presented (owner, scene, tracks, timestamp) and per UI frame (wake and submit time) |
| `ICED_SPAN_TRACE=<file>` | append `stage start_us duration_us` for every update, view, layout, draw, replay and present; the first line gives the origin on the presentation clock |
| `ICED_FRAME_DUMP=<dir>` | write every frame handed to the surface, by the UI thread or the render lane, as raw pixels; the readback stalls the GPU, so take timings from a separate run |
| `ICED_REPLAY_VERIFY=1` or `=<dir>` | draw every frame the UI thread replays in full as well, and report each that differs (writing both renderings to the directory); with `ICED_DISABLE_ANIMATION_LANE=1` every replay is checked |
| `ICED_DISABLE_ANIMATION_LANE=1` | animate on the UI thread instead of the render lane |
| `ICED_LANE_DAMAGE_CHECK=<file>` | paint every render-lane frame again in full and append, for each frame the lane painted only where it changed, how many pixels differ and where |
| `ICED_DISABLE_LANE_DAMAGE=1` | paint every render-lane frame in full instead of only where it changed |
| `ICED_UI_DAMAGE_CHECK=<file>` | paint every UI-thread frame painted only where it changed again in full, and append how many pixels differ and where |
| `ICED_DISABLE_UI_DAMAGE=1` | paint every UI-thread frame in full instead of only where it changed |
| `ICED_DEBUG_DEVICE_LOSS_AFTER_MS=<ms>` | lose the graphics device that long after it is created, to watch the window recover on a new one |
| `ICED_LANE_TRACE=<file>` | append one JSON record per frame the render lane presented (sampled values, wake and present time) and per lane stop |

## Accessibility

`ENZUI_DIAGNOSTICS=1` is the quickest look: silent elision, and undersized
targets or low contrast in the controls wrapped in scan probes, are outlined
on screen. Logged warnings
under `enzui::a11y` (on by default) name controls that share an id or have
no name. For the tree itself, any AT-SPI inspector (Accerciser, or Orca's
own flat review) shows what a screen reader reads; the same tree is what
[testing.md](testing.md) asserts on.

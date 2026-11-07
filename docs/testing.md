# Testing an enzUI application

enzUI applications test their real views headlessly: no window, no display,
no compositor. The input, focus and accessibility examples are exercised in
[`crates/enzui-widgets/tests/foundations/testing_guide.rs`](../crates/enzui-widgets/tests/foundations/testing_guide.rs),
The animation fragment below needs a GPU adapter; native platform operations
need separate integration checks.

## Set up

```toml
[dev-dependencies]
iced_test = { git = "https://github.com/enz1m/enzim-iced.git", rev = "67f991b7442cbd79821141069c3b864cc12413f2" }
enzui-core = { path = "../enzui/crates/enzui-core", features = ["test-frame"] }
```

Copy the [consumer overrides](dependencies.md#consumer-workspace-overrides)
into the application workspace root. Use the same `enzui-core` source in normal
and development dependencies; mixing a Git dependency with a local path creates
distinct Rust crate types.

`iced_test` drives a view: it lays it out, delivers events and collects the
messages your `update` would receive. The `test-frame` feature of
`enzui-core` adds the headless frame, the semantics helpers and the
operation runner used below; it belongs in dev-dependencies only (see
[features.md](features.md)).

## Simulate input

Build the view, act on it, read the messages:

```rust
let mut ui = Simulator::with_size(Default::default(), Size::new(400.0, 300.0), form(false));
let _ = ui.click("Save").expect("a Save button");
assert_eq!(ui.into_messages().collect::<Vec<_>>(), [Message::Save]);
```

`click` finds a control by the text it shows. `point_at` with
`simulate(click())` presses at a position; `typewrite` and `tap_key` type.
The simulator does not run `update`: assert on the messages, or apply them
to your state and rebuild with `ui.rebuild(view(&state))`, which keeps
widget state (focus, scroll offsets) as a real frame would.

## Focus and keys

Focus moves through widget operations, and some (`focus_next`,
`focus_previous`) work in two passes. Run them the way the runtime does:

```rust
let mut focus = focusable::focus::<()>(Id::new("name"));
let _ = enzui_core::chained_operation::operate_chained(|op| ui.operate(op), &mut focus);
let _ = ui.typewrite("Ada");
```

Running only the first pass of a chained operation focuses nothing, and
the keys that follow go nowhere.

To check that Tab and Shift+Tab move through a whole screen in order,
implement `enzui_core::keyboard_traversal_validation::KeyboardTraversalHost`
for your application (its real `update`, `view` and `subscription`) and call
`assert_tab_moves_focus(&mut host, size)`.

## What assistive technology sees

`collect_frame` lays a view out and returns the accessibility tree a screen
reader would get:

```rust
let frame = collect_frame(form(true), Size::new(400.0, 300.0));
let checkbox = frame.collected().iter()
    .find(|node| node.semantics.role == Role::CheckBox)
    .expect("a checkbox");
assert_eq!(checkbox.semantics.label.as_deref(), Some("Remember me"));
assert_eq!(checkbox.semantics.toggled, Some(Toggle::True));
```

The same `SemanticsCollector` runs over a live simulator
(`ui.operate(&mut collector)`), so you can check the tree after input too.
Assert roles, names (`label`), values, and states such as `toggled`,
`expanded`, `selected` and `disabled`; names from Fluent messages carry
Unicode isolation marks (U+2068, U+2069) around their arguments.

## Audit accessibility

The audit a running application performs with `ENZUI_DIAGNOSTICS=1` runs as a
test too. It examines what publishes a probe: text that elides without an
accessible full value (every `enz_text` does), and the controls you wrap in
`HitTargetScanProbe` (targets under 24 px) or `ContrastScanProbe` (text below
its contrast ratio):

```rust
let toolbar = row![
    HitTargetScanProbe::new(save_button),
    HitTargetScanProbe::new(close_button), // 12 px: too small to hit reliably
];
let mut audit = AccessibilityScanCollector::new(
    &enzui_widgets::hit_target_deficit,
    &enzui_widgets::contrast_deficit,
    &enzui_widgets::elision_clamp,
);
ui.operate(&mut audit);
assert!(audit.subjects_examined() >= 2);
assert_eq!(audit.findings().len(), 1); // the close button, UndersizedTarget
```

Check `subjects_examined`: an audit that looked at nothing also finds
nothing.

## Pixels

For what only paint shows (a colour, a corner, a focus ring), build a
`iced_test::runtime::user_interface::UserInterface` with a headless renderer,
draw it with your theme, and read `renderer.screenshot(..)`.
`Simulator::snapshot` with `matches_hash` keeps a whole-frame golden. Set
`ICED_TEST_BACKEND=tiny-skia` for deterministic software rendering.

## Animation, frame by frame

Most animation frames are not drawn: the runtime replays the frame it
recorded and moves the recorded paint by the tracks widgets publish, on the
UI thread or on the render lane. A wrong recording or track shows up only in
those frames, so a test that only draws cannot see it.
`enzui_frame_check::FrameCheck` (a dev-dependency) runs a view twice in
lockstep on the GPU, one copy replaying the way the runtime does and one
drawing every frame, and compares each simulated frame at the whole-view size.
It is a headless GPU check, not compositor-presentation evidence:

```rust,ignore
let mut check = FrameCheck::new(size, theme, || view(false))
    .expect("GPU adapter required for this qualification");
check.frames(30);
check.view(|| view(true)); // what the app does after a message
check.scroll(Point::new(400.0, 300.0), 200.0);
check.frames(120);
check.assert_replays_match();
assert!(check.replays() > 60, "the motion reaches replay");
assert!(check.changed() > 30, "it moved, rather than jumped");
assert_eq!(check.content_frames(), 0, "nothing redraws once settled");
```

Build the view as the app does: inside its shell, sidebar and scrolled
page. A check of a lone widget misses whatever the surrounding clips and
scroll offsets do to a replay. `ENZUI_FRAME_CHECK_DUMP=<dir>` writes both
renderings of every differing frame.

## Desktop preferences, locale and theme

Scope them around the code under test rather than changing the process:

```rust
let reduced = iced_core::preferences::Preferences { reduced_motion: true, ..iced_core::preferences::get() };
iced_core::preferences::scoped(reduced, || { /* build and drive the view */ });

german_localizer.scope(|| { /* text resolves in German */ });
```

`enzui_core::desktop_settings::with_settings(settings, || ..)` does the same
for desktop settings (button layout, accent, contrast). Pass a theme to the
renderer or to your view directly: `EnzTheme::dark().iced_theme()`.

## Hold the app to the framework's patterns

`enzui-test-support` (a dev-dependency) scans an app's own sources.
`titlebar_adoption::assert_titlebar_adoption(src, &exceptions)` fails on a
titlebar height or padding set by hand, `.show_close(false)`, an icon button
labelled twice, or commands pushed into a titlebar instead of its
`.actions(..)`; each deliberate exception names its file, rule and reason, and
an exception that no longer matches anything fails too. See
[the app shell](building-an-app.md#build-the-whole-window-with-the-app-shell).

## Native platform checks

Headless input and pixel tests do not exercise compositor drag grabs, native
MIME negotiation, portal descriptor transfer or actual frame presentation.
Check those on a private desktop with independent peer applications; see
[native drag and drop](native-drag-and-drop.md#verification-boundary). Keep
source/destination identity, completion, bytes and cancellation in the evidence.

## Make each test able to fail

A test that passes the first time it runs has not shown it can catch
anything. Break the code it covers (invert a condition, drop a message,
skip the call) and watch it fail before trusting it.

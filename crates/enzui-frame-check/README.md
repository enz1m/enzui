# enzui-frame-check

Checks frame by frame that an animated enzUI view replays what a full redraw
draws. Animations that only move, fade, scale or clip are replayed from a
recording rather than drawn again; a widget whose recording or tracks are
wrong shows a frame no draw would, and tests that only draw never see it.

`FrameCheck` runs a view twice in lockstep on the GPU, one copy replaying and
one drawing every frame, and compares simulated frames at the whole-view size.
This is a headless GPU harness, not native compositor-presentation evidence.
It is a dev-dependency; see the crate documentation for an example.

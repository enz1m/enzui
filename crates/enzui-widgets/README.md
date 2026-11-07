# enzui-widgets

The widget library - a large collection of styled, token-driven components built
on iced's native widget architecture. This is the crate app authors spend almost
all their time in. The exact count of tier-1 modules is derived from the source
(see below); no hand-maintained number in this README can stay honest against a
tree that changes.

## What to reach for first

- **Building an app?** Read [`docs/building-an-app.md`](../../docs/building-an-app.md)
  first. It shows the `Message` round-trip, the adaptive `enz_app_shell` (which
  places the standalone `enz_window_controls` cluster for you - an app never
  composes those controls directly), and the component-width breakpoint rule -
  the frame every widget hangs in.
- **Finding a widget?** Every builder is `enz_<thing>` (`enz_button`, `enz_badge`,
  `enz_navigation_split_view`). To list the current set:

  ```sh
  grep '^pub mod' crates/enzui-widgets/src/lib.rs
  ```

  The task-grouped catalog in `docs/widgets.md` is the friendlier entry point
  once you do not yet know the name.
- **Turning a builder into an `Element`?** Use `.into_element()`. A few types
  (e.g. `EnzWindowControls`) require it specifically rather than a bare `.into()`,
  because `row!`/`column!` cannot always infer the target type.

## The export tiers

`lib.rs` defines a three-tier export policy:

- **Tier 1** - the supported public API (the widgets you compose).
- **Tier 2** - internal composition helpers; re-exported so widgets can build on
  each other, not intended as an app author's first reach.
- **Tier 3** - audit/probe support.

Prefer tier-1 builders. If a symbol is only reachable via a deep module path and
is not re-exported at the crate root, treat it as internal.

## What this crate is **not** for

- Not a place for application state or I/O - widgets never spawn work or perform
  hidden I/O. State is app-owned and passed in; effectful loading lives in
  `enzui-resources`.
- Not the token layer - spacing, colour, sizing and breakpoint values come from
  `enzui-theme`. Reach for a theme token before typing a literal like
  `16.0`.

## Docs coverage

`#![deny(missing_docs)]` is active: every module carries a `//!` block. Coverage
is enforced, so a new public item without docs fails the build.

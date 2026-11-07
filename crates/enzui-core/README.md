# enzui-core

The framework-level primitives that sit **between application code and iced's
widget layer**. This crate is architecture, not widgets: its default build
depends on iced's renderer- and window-backend-free crates (`iced_core`,
`iced_runtime`, `iced_futures`) plus the token, icon and security layers
(`enzui-theme`, `enzui-icons`, `enzui-security`), never on iced's
renderer or window backend, so its types are usable in pure logic and unit
tests without a GPU or a window system. Everything else - AccessKit mapping,
the D-Bus portal and application backends, freedesktop notifications, the font
portal and the shared headless collect frame - is off by default behind
explicit Cargo features. Media, preview and credential services live in
`enzui-services`.

## What to reach for first

- **Semantics** (`semantics`) - the `Role`/accessibility vocabulary every widget
  publishes.
- **Pure lifecycle reducers** - e.g. `async_image` (the policy + pure reducer half
  of remote image loading; the effectful half is `enzui-resources`).
- **Shared traversal / focus primitives** used by the widgets crate
  so both bind the *same* rule rather than each hand-rolling one.

## What this crate is **not** for

- It is not where you find widgets - those live in `enzui-widgets`.
- It renders nothing, and its reducers and models do no I/O, which is what
  lets them be tested in isolation. The I/O it does own sits at named edges:
  saved state (`persist`, `atomic_state`, `recovery`, `store_watch`), the
  thumbnail cache, and the D-Bus portal, tray and activation clients behind
  their features. Core also supplies bounded blocking dispatch, cancellation and temporary-file
  custody (`blocking_task`, `cancellation`, `drop_files`). Network transports,
  media and credentials live in `enzui-resources` and `enzui-services`.

## Architecture

enzUI uses an Elm-style loop: a plain-struct **Model**, an app-defined **Message**
enum, an **`update`** state transition returning a `Task`, and a **`view`** that
builds the widget tree. The renderer lays out and paints that tree. This crate
holds the types the model shares across widgets.

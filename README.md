# enzUI

enzUI is a Linux GUI framework for Rust, built on
[our iced fork](https://github.com/enz1m/enzim-iced). It combines themed widgets,
an application shell, keyboard navigation, accessibility integration and optional
desktop services. Rendering uses wgpu with a tiny-skia software fallback.

**Alpha 0.1 — `v0.1.0-alpha.1`.** Wayland is the primary platform; X11 has a reduced
feature set. APIs may change. This first source snapshot is distributed through
Git, not crates.io.

## Try it

Install the pinned Rust **1.99.1** toolchain, Git, a C toolchain, `pkg-config` and
the desktop libraries
listed in [platform support](docs/platform-support.md). Rust **1.95** remains
the minimum supported compiler and is checked separately in CI. The repository pins its
public fork dependencies; no private checkout, global helper or SDK installation
is required.

```sh
git clone --branch v0.1.0-alpha.1 https://github.com/enz1m/enzui.git
cd enzui
cargo run --locked --example app
```

The app is a small notes workspace with a sidebar, editable title and body,
System/Light/Dark appearance controls, and enzUI's own titlebar, window controls
and resize edges. Add, select and remove notes; state lasts for the session.
Use Tab / Shift+Tab to move focus and Enter to activate a button.
The release tag pins this source snapshot and its public fork dependencies.

To build an application, start with [Building an app](docs/building-an-app.md)
and copy the pinned dependency setup from [Dependencies](docs/dependencies.md).
Use `enzui_widgets::app_setup::run` to install the framework runtime,
accessibility bridge and default keyboard traversal. Custom event loops need
the equivalent setup.

## What it provides

- Themed controls, forms, editors, menus, dialogs, navigation, lists, tables,
  charts and media views, with searchable [widget documentation](docs/widgets.md).
- An adaptive application shell with client-side window controls, sidebars,
  rounded clipping, glass materials, motion and reduced-motion preferences.
- Keyboard traversal and AccessKit/AT-SPI semantics for supported controls.
  Applications remain responsible for meaningful names and their composed flows.
- Runtime light/dark themes, shared [theme files](docs/theme-file.md),
  [localization](docs/localization.md) and typography preferences.
- Optional portals, media, clipboard, notifications, credentials, background
  activity and other [core capabilities](docs/core-capabilities.md).

## Examples

The repository has two runnable examples:

| Example | Purpose | Run |
| --- | --- | --- |
| [App](crates/enzui-widgets/examples/app.rs) | Complete application wiring: notes, sidebar, editing, theme controls and an enzUI window frame | `cargo run --locked --example app` |
| [Gallery](crates/enzui-widgets/examples/gallery/main.rs) | Browse the widget library and its variations | `cargo run --locked --example gallery --features enzui-widgets/emoji-picker` |

Start with the app to learn how a window, state, messages and services fit
together. Use the Gallery and [widget catalog](docs/widgets.md) to choose
components for your own application.

## Build and test

```sh
cargo wbuild --locked                       # workspace default features
cargo wcheck --locked                       # all targets and all features
cargo wt --locked --test fields search_field
cargo wclippy
cargo fmt --all -- --check
```

The aliases are ordinary Cargo commands. They do not install global tools,
change other checkouts, prune build caches or impose a memory limit. The pinned
Rust toolchain already uses bundled LLD by default on x86_64 Linux.

`wcheck`, `wt` and `wclippy` enable development and test features, including
fault-injection and observation seams, and require the optional native build
libraries. **Do not use `--all-features` for a production application.** Enable
only the backends and services it needs. Headless test baselines use the software
renderer by default; explicit GPU checks can override `ICED_TEST_BACKEND`.

See [CONTRIBUTING.md](CONTRIBUTING.md) and [validation](docs/release-review.md)
for the reviewed build and test scope. [Build performance](docs/build-performance.md)
covers fast consumer profiles, cache reuse and release optimization.

## Crates

```
crates/
├── enzui-core - actions, shortcuts, semantics, focus, portals,
│                             media and other runtime models
├── enzui-theme - the enzUI Nebula theme in dark and light, the
│                             spacing / radius / typography scales, iced::Theme adapter
├── enzui-widgets - the widget library and application shell
├── enzui-icons - bundled Lucide icons and the icon registry
├── enzui-terminal - a GPU terminal widget
├── enzui-services - app services: media (GStreamer, MPRIS), process
│                             transport and activation, credential store,
│                             document preview, Git status, OAuth, DNS, mail;
│                             each heavy backend behind a feature
├── enzui-resources - remote resource loading and image decoding
├── enzui-external-image - DMA-BUF frames from another process, drawn on the
│                             renderer's own GPU device without a CPU copy
├── enzui-clipboard - the Wayland clipboard on the application's own
│                             connection and seat
├── enzui-clipboard-runtime - installs that clipboard into an iced runtime
├── enzui-wayland - portal parent export, compositor glass, the system
│                             bell and surface hints
├── enzui-parent-lease - lifetime rules for an XDG portal parent lease
├── enzui-activity - background activity publishing
├── enzui-center-protocol - the agent center wire protocol
├── enzui-agent - app side of the optional agent center
├── enzui-agent-app - agent-process glue: single instance, background mode
├── enzui-security - process-level security primitives
├── enzui-frame-check - checks that animated views replay what a full
│                             redraw draws (dev-dependency)
└── enzui-test-support - source scanners and test helpers (dev-dependency only)
```

## Optional integrations

Agent integration is opt-in through `enzui-agent` and `enzui-agent-app`.
Widgets do not depend on those crates. Applications should expose agent-only
controls only while the service is available; the normal application flow
must remain usable without it.

Web content lives in the separate `enzui-webkit` project. Its WPE engine and
`EnzWebView` adapter are not prerequisites for building this framework.

## Platform limits

- X11 overlays stay inside the window. Wayland can use popup surfaces where
  compositor support and the application's overflow policy allow them.
- Pointer lock, graphics-tablet events, touchpad gestures and native outward
  drag-and-drop use Wayland protocols and are not available through the X11 path.
- Window shadows, glass, session restore, icons, clipboard persistence and
  surface hints depend on compositor protocols. Unsupported calls report that
  state; the supported subset is documented in [platform support](docs/platform-support.md).
- No other operating systems are supported by enzUI, even though upstream iced
  supports additional platforms.

## Documentation and license

[Documentation index](docs/README.md) · [Contributing](CONTRIBUTING.md) ·
[Changelog](CHANGELOG.md) · [Contributor conventions](AGENTS.md)

The framework is MIT licensed. Bundled icons, fonts, fixtures and native source
retain their own notices; see [LICENSE](LICENSE) and
[THIRD_PARTY.md](THIRD_PARTY.md).

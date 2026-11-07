# Building an app

Start with the complete [App example](../crates/enzui-widgets/examples/app.rs).
It is a small notes application with a sidebar, an editable title and body,
System/Light/Dark appearance controls, and enzUI's own window frame. It is the
canonical source for the wiring described on this page.

```sh
cargo run --locked --example app
```

The notes live in memory for the session. Closing the app discards them; the
example does not claim persistence or a production document format.

The [Gallery](../crates/enzui-widgets/examples/gallery/main.rs) is the other
runnable example. Use it to explore individual components and their variants:

```sh
cargo run --locked --example gallery --features enzui-widgets/emoji-picker
```

## Set up a consumer workspace

The framework is distributed through Git. Follow [Dependencies](dependencies.md)
for the pinned sources and the consumer workspace overrides. Cargo fetches the
iced and native windowing forks; no sibling source checkout is required.

Keep every enzUI dependency on the same URL and revision. When developing an
application beside a local checkout, use that same checkout for its normal and
dev dependencies. Mixing a Git copy and a local copy creates distinct Rust
types even when their version numbers agree.

Use the app's selected features as a starting point, then enable only the
backends and services your application needs. `--all-features` enables internal
development seams and is not a shipping configuration.

## Follow the application loop

iced uses an Elm-style loop:

- Application state owns the notes, selected note, editor state and theme host.
- A `Message` identifies an edit, navigation action, theme event or window gesture.
- `update` changes the state and returns the tasks needed by the runtime.
- `view` builds the current widget tree from that state.
- `subscription` supplies asynchronous events, including desktop theme changes.

The editor is controlled by the application. An edit changes the selected note's
title or body in `update`; the next `view` reflects that change. Sidebar rows
select a note. The titlebar's **New note** action inserts a note and selects it;
the editor's **Delete note** button removes the selected note only while another
note remains. An empty title appears as **Untitled** in the sidebar and titlebar.

The titlebar action can move into its overflow menu as space narrows. The app
owns whether that menu is open and closes it after an action or dismissal.
The shell publishes responsive sidebar state through `on_sidebar_state`; the app
retains it, and selecting a note dismisses an overlay sidebar.

Keep state and effects outside the view. Building a widget tree should not read
a file, start a network request or decide which note will be saved.

## Build the whole window with the app shell

The app mounts its content through `enz_app_shell`, using one layout for the
sidebar, titlebar and editor. The shell owns its chrome, rounded frame, window
controls and resize edges. Do not add a second titlebar or compose window
controls around it.

The native window must use the matching settings:

`DesktopApplicationIdentity::apply_to_window_settings(enz_window_settings(LAYOUT))`

`enz_window_settings` requests the shell's client-side frame rather than the
desktop's default titlebar. The app requests an initial size of 860 by 600 logical
pixels; the compositor can choose a different size. A bare
`iced::window::Settings::default()` would skip this setup and is not the example's
entry point.

Shell window gestures are messages too. The app forwards them through
`WindowCommand::into_task`. This single-window app resolves its current native
window with `iced::window::latest` before executing drag, resize or window-control
operations. A multi-window application should retain and route the originating
window id instead. Painting a Close button without routing its command would
leave a visible control that does nothing.

Call `enzui_widgets::app_setup::run` for the runtime services and default
Tab / Shift+Tab traversal. It also installs the accessibility action route.
Applications must still give controls meaningful names and test their composed
keyboard and accessibility flows. A custom event loop needs the equivalent
runtime setup.

## Follow the shared theme

`ThemeIntegration` owns appearance state. Boot it once, route its asynchronous
events back through `update`, include its subscription, and return its iced theme
from the application's theme callback. The app retains that host for the entire
session rather than rebuilding it in `view`.

The app's three appearance choices have different scopes:

- **System** follows the shared theme and desktop preference through the host's
  subscription.
- **Light** and **Dark** set an application-local mode override.
- Returning to **System** clears the override.

These controls do not rewrite the cross-application theme file. The shared
file, token precedence, desktop fonts and live updates are documented in
[Theme files](theme-file.md), [Styling](styling.md) and
[Localization](localization.md).

## Keep the application identity consistent

The example uses `org.enzui.AppExample` for its package identity and `app` for
the executable name. The same identity reaches the native window, desktop entry,
icon and AppStream metadata. Choose an id for a domain you control in a real app.

The example can export those package files without opening a window:

```sh
cargo build --release --locked --example app
target/release/examples/app --write-package-files /path/to/staging /usr
```

The staging root controls where files are written; `/usr` describes where the
installed application will live. See [Packaging](packaging.md) before installing
those files on a desktop or building a sandboxed package.

## Add capabilities to this structure

Use [Widgets](widgets.md) to find components and
[Core capabilities](core-capabilities.md) for models, reducers and platform
operations. The Gallery demonstrates component variants; the app demonstrates
how to host them in one application.

For durable state, choose an explicit model and storage policy rather than
assuming the session-only note example saves it. Platform operations such as
file dialogs, activation, notifications and media have opt-in backends; use their
typed unavailable outcomes when the desktop does not provide a service.

Follow [Testing](testing.md) for simulated input, focus, semantics and pixel
checks. Test the composed application path as well as the individual controls.

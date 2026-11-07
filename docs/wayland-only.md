# Wayland defaults and optional X11 support

enzUI enables Wayland by default in its widgets, clipboard runtime and terminal
crates. X11 remains an additive `x11` feature. A consumer must also disable
iced's own mixed-backend defaults, as the examples below show; Cargo unions
features from every dependency edge.

This is a build choice, not a runtime switch. A Wayland-only executable needs
a Wayland session. Build with the optional X11 features to run in an X11
session. XWayland applications elsewhere on a Wayland desktop are unaffected.

This configuration changes only client dependencies. It does not configure a
compositor, session, portal service, or distribution, and it does not require
X11 packages to be removed from the host.

Paths below assume the framework checkout is `../enzui`; adjust them to match
your workspace. Use the pinned fork and application patch table described in
[Dependencies and consumer setup](dependencies.md).

## Facade features

Disable iced's defaults and request the default renderer/runtime capabilities
explicitly, omitting `x11`:

```toml
iced = { git = "https://github.com/enz1m/enzim-iced.git", rev = "67f991b7442cbd79821141069c3b864cc12413f2", default-features = false, features = [
    "wgpu",
    "tiny-skia",
    "crisp",
    "web-colors",
    "thread-pool",
    "linux-theme-detection",
    "wayland",
    "canvas",
    "svg",
    "a11y",
    "tokio",
] }
```

The Git URL and revision select the enzUI fork. A `version` constraint alone
selects crates.io; it does not select our fork. Keep this revision aligned with
the [pinned foundations](dependencies.md#pinned-foundations).

The first seven entries reproduce iced's current default renderer, runtime,
theme-detection, and Wayland capabilities except for `x11`. The remaining
entries are application choices; omit any widget or executor feature the client
does not use.

Core accessibility can be enabled independently:

```toml
enzui-core = { path = "../enzui/crates/enzui-core", features = [
    "accesskit",
    "portal-dbus",
] }
```

`enzui-core/accesskit` adds `iced_winit/a11y`, not a window backend. The
application's iced facade therefore remains the sole owner of its winit backend
selection. Core's own test graph supplies X11 and Wayland on a dev-dependency
edge; Cargo does not unify dependency-crate dev features into a consuming
application's normal graph.

Select the same backend on the widget and clipboard-runtime crates:

```toml
enzui-widgets = { path = "../enzui/crates/enzui-widgets", default-features = false, features = [
    "wayland",
    "desktop-portal",
    "portal-dbus",
] }
enzui-clipboard-runtime = { path = "../enzui/crates/enzui-clipboard-runtime", default-features = false, features = [
    "wayland",
    "image",
] }
```

The `x11` and `wayland` features on both crates are additive. Only Wayland is on by
default; `enzui-widgets` also
turns on `desktop-portal` by default (desktop settings and the UI font from the
Settings portal), so list it again after disabling defaults. An app that uses
`enzui-terminal` selects its backend the same way:

```toml
enzui-terminal = { path = "../enzui/crates/enzui-terminal", default-features = false, features = [
    "wayland",
] }
```

Disabling defaults therefore requires selecting at least one supported backend. The
widgets crate keeps iced's renderer, accessibility, theme-detection, canvas,
SVG, image-without-codecs, markdown, and QR capabilities independently of that
choice.

`enzui-clipboard-runtime/wayland` forwards to `iced_winit/wayland`, while
its `image` feature forwards the existing clipboard image support. The
lower-level `enzui-clipboard` crate continues to use `wl_data_device` on
the application's Wayland connection without depending on iced's X11 backend.

## Optional X11 compatibility

The shared workspace iced entry spells out renderer, runtime and theme features
without choosing a native backend. The widgets, clipboard-runtime and terminal
crates select Wayland by default. For an application that also supports X11:

```toml
[features]
x11 = ["iced/x11", "enzui-widgets/x11"]
```

Add `enzui-terminal/x11` or `enzui-clipboard-runtime/x11` when the application
uses those crates directly. `cargo build --features x11` then includes both
backends. A direct iced edge with its own defaults enabled also selects X11;
check every direct dependency and the resolved normal dependency graph.
Framework all-feature checks and native-backend tests intentionally include X11.

The expected benefit is modest: less backend code to compile and link. Once an
application is running on Wayland, removing its unused X11 backend does not
remove a second renderer or a second GPU context. Measure each application's
shipping graph before promising runtime performance gains.

## Portals and toolkit boundaries

enzUI's application-side portal code is a D-Bus client and does not require GTK,
GDK, libadwaita, or WebKitGTK. The host's portal service may be implemented by
GTK, GNOME, KDE, or another toolkit; that is supported interoperability, not an
enzUI client dependency. Service absence remains a typed unavailable result.
Applications should not add a direct toolkit dialog as a fallback.

`wayland-csd-adwaita` and `sctk-adwaita` provide Rust-drawn client-side window
decoration. Their names do not imply a GTK dependency. The web engine uses WPE
WebKit rather than WebKitGTK; GLib and GIO are shared non-GUI libraries and do
not imply an X11 client backend.

## Clipboard residual

`iced_winit/wayland` enables `arboard/wayland-data-control`. The locked arboard
version also makes `x11rb` reachable on Linux independently of the winit, renderer, and
softbuffer X11 feature paths. A graph check must therefore distinguish:

- absent: `iced/x11`, `iced_winit/x11`, `iced_renderer/x11`,
  `iced_tiny_skia/x11`, `winit/x11`, `softbuffer/x11`, `x11-dl`, and
  `tiny-xlib`;
- present: `arboard/wayland-data-control` and the current arboard-to-`x11rb`
  residual.

Removing the arboard residual belongs to the clipboard backend work; it is not
evidence that winit or the renderer selected X11.

## Verification shape

The resolved graph must come from a standalone consumer workspace. Resolving it
as a member of the framework workspace can unify iced features with development
and all-feature checks that deliberately include both backends, so it cannot
prove the standalone shipping graph.

A valid gate asks Cargo to filter metadata for the Linux target under test and
walks only normal dependency edges from the consumer. It rejects empty or
partial metadata, duplicate backend-sensitive package IDs, and dev/build edges
that could contaminate a checked package's feature set. Feature and parent
checks use Cargo package IDs, not dependency aliases or package-name greps. The
gate checks Wayland/renderer/accessibility/portal/clipboard features on iced,
widgets, and clipboard runtime, checks the X11 exclusions above, and requires
the sole `x11rb` package ID to have exactly one normal parent edge from the sole
arboard package ID.

No gate in this repository currently validates that standalone graph. A consumer
must carry the required patches from [Building an app](building-an-app.md),
generate its own lockfile, and resolve its own normal dependency graph. Use
`cargo metadata --offline --locked --filter-platform <target-triple>` after
dependencies have been fetched to make subsequent verification reproducible.

Graph verification does not prove the Wayland-only client compiles. That is a
separate check under that consumer's selected feature set.

Native verification is separate from graph verification. Under an existing
Wayland compositor with `DISPLAY` unset, exercise a window, clipboard, IME, and
the portal chooser, including the typed unavailable result when no portal
service is present.

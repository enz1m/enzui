# Dependencies and consumer setup

This is a Git-source release. The enzUI crates and their development iced
versions are not published on crates.io. Clone this repository to work on it;
Cargo fetches the external forks from their public URLs automatically.

## Pinned foundations

| Source | Revision |
| --- | --- |
| [iced](https://github.com/enz1m/enzim-iced) | `67f991b7442cbd79821141069c3b864cc12413f2` |
| [wgpu family](https://github.com/enz1m/enzim-wgpu) | `7f040c8501f6838262c90ede4edfa40940fe2de8` |
| [cosmic-text](https://github.com/enz1m/enzim-cosmic-text) | `ad474131e52c0fde5979f8def2889489b50a0480` |
| [cryoglyph](https://github.com/enz1m/enzim-cryoglyph) | `eeaaa5585fbb1bac004ca398cfad957fec6be5f3` |
| [winit](https://github.com/enz1m/enzim-winit) | `f6ce16941bf815a438db7e628ab1126ce48e31dd` |
| [AccessKit family](https://github.com/enz1m/enzim-accesskit) | `e9da4a8da1825e65356f35c94325fb9438cf90d0` |
| [zbus family](https://github.com/enz1m/enzim-zbus) | `815a9ca7b64aa24e14ccb6ea3f7eda97ef7bbc8a` |

The patched softbuffer source is bundled by the iced fork. The bounded native
media source is bundled here under `vendor/gst-curl`, with input hashes and
licenses. Neither needs a separate checkout.

## An application using Alpha 0.1

Use the `v0.1.0-alpha.1` release tag for this snapshot. Keep all enzUI
dependencies on the same Git URL and tag, and commit the application's Cargo.lock.
The lockfile records the resolved commit so subsequent builds reuse that snapshot.

```toml
[dependencies]
enzui-core = { git = "https://github.com/enz1m/enzui.git", tag = "v0.1.0-alpha.1", features = ["accesskit"] }
enzui-theme = { git = "https://github.com/enz1m/enzui.git", tag = "v0.1.0-alpha.1" }
enzui-widgets = { git = "https://github.com/enz1m/enzui.git", tag = "v0.1.0-alpha.1" }
iced = { git = "https://github.com/enz1m/enzim-iced.git", rev = "67f991b7442cbd79821141069c3b864cc12413f2", default-features = false, features = ["wgpu", "tokio", "advanced", "a11y", "wayland"] }
```

A local application can instead use paths such as
`enzui-widgets = { path = "../enzui/crates/enzui-widgets" }`.

Wayland is enabled by default. An application that also supports an X11
session exposes an additive feature:

```toml
[features]
x11 = ["iced/x11", "enzui-widgets/x11"]
```

Forward this feature to `enzui-terminal/x11` and
`enzui-clipboard-runtime/x11` too if they are direct dependencies.
Build that variant with `cargo build --features x11`. Keep the backend feature
selection consistent across the application's direct dependencies; Cargo unions
the requested features. See [backend selection](wayland-only.md).

## Consumer workspace overrides

Cargo reads patch tables only from the root being built. Copy this table into
the application's root Cargo.toml (merge with an existing table):

```toml
[patch.crates-io]
winit = { git = "https://github.com/enz1m/enzim-winit.git", rev = "f6ce16941bf815a438db7e628ab1126ce48e31dd" }
zbus = { git = "https://github.com/enz1m/enzim-zbus.git", rev = "815a9ca7b64aa24e14ccb6ea3f7eda97ef7bbc8a" }
zbus_macros = { git = "https://github.com/enz1m/enzim-zbus.git", rev = "815a9ca7b64aa24e14ccb6ea3f7eda97ef7bbc8a" }
zbus_names = { git = "https://github.com/enz1m/enzim-zbus.git", rev = "815a9ca7b64aa24e14ccb6ea3f7eda97ef7bbc8a" }
zvariant = { git = "https://github.com/enz1m/enzim-zbus.git", rev = "815a9ca7b64aa24e14ccb6ea3f7eda97ef7bbc8a" }
zvariant_derive = { git = "https://github.com/enz1m/enzim-zbus.git", rev = "815a9ca7b64aa24e14ccb6ea3f7eda97ef7bbc8a" }
zvariant_utils = { git = "https://github.com/enz1m/enzim-zbus.git", rev = "815a9ca7b64aa24e14ccb6ea3f7eda97ef7bbc8a" }
```

The winit override keeps AccessKit and iced on one `Window` type. The zbus
family overrides keep portal and accessibility transport types coherent.
The SDK constrains the compatible zbus, zbus_names and zvariant versions so a
fresh consumer also resolves this transport family consistently. Update a
migrated application's lockfile before using `--locked`; a direct application
dependency requiring a newer transport family needs an explicit migration.
Check the resulting graph with `cargo tree -d`; the same fork family should
not appear from both Git and the registry. Do not substitute stock iced:
its development APIs differ from the framework's requirements.

A default-feature, renderer-neutral `enzui-core` localization consumer needs
none of these windowing/accessibility overrides. Enable services explicitly;
`--all-features` is a development check, not a shipping configuration.

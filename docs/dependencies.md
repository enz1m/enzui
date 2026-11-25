# Dependencies and consumer setup

This is a Git-source release. The enzUI crates and their development iced
versions are not published on crates.io. Clone this repository to work on it;
Cargo fetches the pinned sources automatically.

## Pinned foundations

These revisions describe the current `main` branch. Tagged releases keep
the foundation revisions recorded in their own dependency guides.

| Source | Revision |
| --- | --- |
| [iced](https://github.com/enz1m/enzim-iced) | `523c1755479c55ba4030dafb2be9c05bf8a1def4` |
| [wgpu family](https://github.com/enz1m/enzim-wgpu) | `798421e7ac9091704835a8a733adfa2d7134dafb` |
| [cosmic-text](https://github.com/enz1m/enzim-cosmic-text) | `739e9d9e469e6f943a3eea233b6dc77ae3832ab5` |
| [cryoglyph](https://github.com/enz1m/enzim-cryoglyph) | `bd2494fbd903f9cfac4c5b3065ff216850c0c5d3` |
| [winit](https://github.com/enz1m/enzim-winit) | `40eeb41f8637ef1620afa377ba006874224b6d65` |
| [AccessKit family](https://github.com/enz1m/enzim-accesskit) | `7da7ac7b800fabf0bd2ba62eef23fa9117a684f4` |
| [zbus family](https://github.com/enz1m/enzim-zbus) | `e182645b56d15e637bb6c8144f99ebfa7f900716` |

The patched softbuffer source is bundled by the iced fork. The bounded native
media source is bundled here under `vendor/gst-curl`, with input hashes and
licenses. Neither needs a separate checkout.

Applications tracking `main` should use `main` for every enzUI crate and the
iced revision listed above, with a committed application lockfile. Keep the
SDK and iced sources aligned with the same SDK revision.

## An application using Alpha 0.3

Keep all enzUI dependencies on the same Git URL and the `v0.3.0-alpha.1` tag,
and commit the application's Cargo.lock. The lockfile pins the resolved source.
For the original `v0.1.0-alpha.1` release, use that tag's dependency guide;
its older foundation pins must remain together.

```toml
[dependencies]
enzui-core = { git = "https://github.com/enz1m/enzui.git", tag = "v0.3.0-alpha.1", features = ["accesskit"] }
enzui-theme = { git = "https://github.com/enz1m/enzui.git", tag = "v0.3.0-alpha.1" }
enzui-widgets = { git = "https://github.com/enz1m/enzui.git", tag = "v0.3.0-alpha.1" }
iced = { git = "https://github.com/enz1m/enzim-iced.git", rev = "e36b07b591661fdb666088fc943cc0b927a2367b", default-features = false, features = ["wgpu", "tokio", "advanced", "a11y", "wayland", "wayland-dlopen", "wayland-decorations"] }
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
winit = { git = "https://github.com/enz1m/enzim-winit.git", rev = "40eeb41f8637ef1620afa377ba006874224b6d65" }
zbus = { git = "https://github.com/enz1m/enzim-zbus.git", rev = "e182645b56d15e637bb6c8144f99ebfa7f900716" }
zbus_macros = { git = "https://github.com/enz1m/enzim-zbus.git", rev = "e182645b56d15e637bb6c8144f99ebfa7f900716" }
zbus_names = { git = "https://github.com/enz1m/enzim-zbus.git", rev = "e182645b56d15e637bb6c8144f99ebfa7f900716" }
zvariant = { git = "https://github.com/enz1m/enzim-zbus.git", rev = "e182645b56d15e637bb6c8144f99ebfa7f900716" }
zvariant_derive = { git = "https://github.com/enz1m/enzim-zbus.git", rev = "e182645b56d15e637bb6c8144f99ebfa7f900716" }
zvariant_utils = { git = "https://github.com/enz1m/enzim-zbus.git", rev = "e182645b56d15e637bb6c8144f99ebfa7f900716" }
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

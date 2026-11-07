# Packaging an enzUI application

A Linux desktop knows an application through files that must all carry the
same application id: the desktop entry that launchers read, the AppStream
metainfo that software centres read, the icon, the D-Bus service file that
starts a D-Bus activatable application, and the GNOME Shell search provider
file. The window's Wayland app id has to match the desktop entry too, or the
compositor shows a generic icon and groups the window under the wrong entry.

enzUI keeps that identity in one value,
[`enzui_core::packaging::AppPackage`](../crates/enzui-core/src/packaging.rs),
and generates every file from it.

## 1. Choose the id

Use reverse-DNS: a domain you control, reversed, then the application name.
`org.example.Notes`, `io.github.someone.Notes`. The id must have at least two
dot-separated elements of ASCII letters, digits, `_` and `-`, none starting
with a digit (`enzui_core::packaging::validate_app_id`). Flathub also wants
at least three elements and the domain proven, so pick the final id early:
it is in user data paths and settings, and changing it later loses them.

## 2. Describe the application once

```rust
use enzui_core::packaging::{AppPackage, Release};

fn package() -> AppPackage {
    AppPackage::new("org.example.Notes", "Notes", "Write things down", "notes")
        .expect("a valid identity")
        .description(["Notes keeps short formatted notes and finds them again."])
        .developer("org.example", "The Example Team")
        .categories(["Office"])
        .keywords(["memo"])
        .mime_types(["text/markdown"])
        .project_license("GPL-3.0-or-later")
        .homepage("https://example.org/notes")
        .bug_tracker("https://example.org/notes/issues")
        .release(Release {
            version: "1.0.0".into(),
            date: "2026-10-21".into(),
            notes: vec!["First release.".into()],
        })
        .translation("de", "Notizen", "Dinge aufschreiben")
        .icon_svg(include_str!("../data/org.example.Notes.svg"))
}
```

The command (`notes` above) is the executable's name in the install
prefix's `bin`. Add `.mime_types(..)` and the desktop entry passes opened
files on the command line (`Exec=notes %U`).

## 3. Give windows the same id

```rust
use enzui_widgets::app_shell::{DesktopApplicationIdentity, ShellLayout, enz_window_settings};

let identity = DesktopApplicationIdentity::for_package(&package());
iced::application(App::new, App::update, App::view)
    .window(identity.apply_to_window_settings(
        enz_window_settings(ShellLayout::TitlebarSidebarContent),
    ));
```

Every window should use the same identity and the window settings for its
shell layout. `enz_window_settings` selects enzUI's own titlebar, window controls
and resize edges. Passing bare iced defaults instead would request a different
frame setup. The [App example](../crates/enzui-widgets/examples/app.rs) applies
this identity before opening its shell window.

## 4. Activation, notifications and search (optional)

A single-instance application serves `org.freedesktop.Application` on the
session bus under its id with
`enzui_core::activation::dbus::ApplicationService::connect(id)`. Mark the
package `.dbus_activatable(true)`: the desktop entry then says
`DBusActivatable=true` and a service file lets the bus start the application
when a launcher, a notification action or another application asks for it.
An application that serves search
(`ApplicationService::serve_search`, answered over GNOME Shell's and KRunner's
interfaces as well as enzUI's) adds `.search_provider(true)`, which also
installs the GNOME Shell provider file and the KRunner plugin. Desktop notifications sent through the
portal are attributed to the same id.

## 5. Install the files

`AppPackage::files(prefix)` lists every file with its path and contents;
`AppPackage::write(root, prefix)` writes them under a staging root:

| file | path under the prefix |
|---|---|
| desktop entry | `share/applications/<id>.desktop` |
| metainfo | `share/metainfo/<id>.metainfo.xml` |
| icon | `share/icons/hicolor/scalable/apps/<id>.svg` |
| D-Bus service (activatable only) | `share/dbus-1/services/<id>.service` |
| GNOME Shell search provider (search only) | `share/gnome-shell/search-providers/<id>.search-provider.ini` |
| KRunner plugin (search only) | `share/krunner/dbusplugins/<id>.desktop` |

The service file names the executable by its full path (`<prefix>/bin/<command>`),
so write it for the prefix the package installs to: `/usr` for a distribution
package, `/app` in a Flatpak.

The simplest build step is the application itself: the
[`app`](../crates/enzui-widgets/examples/app.rs)
example writes its files when started as
`app --write-package-files ROOT PREFIX`, so the identity the
window uses and the identity the files carry cannot differ. An `xtask` or a
`build.rs` that calls `write` works the same way.

For the App example's distribution package:

```sh
cargo build --release --locked --example app
install -Dm755 target/release/examples/app "$DESTDIR/usr/bin/app"
target/release/examples/app --write-package-files "$DESTDIR" /usr
```

The example's application id is `org.enzui.AppExample`. Its notes remain
session-only; exporting package files does not add document persistence.

Distribution packaging tools refresh the desktop database and icon cache
after installing; a manual install runs `update-desktop-database` and
`gtk-update-icon-cache` itself.

## 6. Check what is installed

`AppPackage::check(root, prefix)` reads an installed tree back and reports
every file that is missing or disagrees with the identity: the desktop
entry's command, icon, window class and activation key, the metainfo's id
and launchable, the service file's name and command, and an icon in some
size. Run it in a test against the staged tree, next to the reference
validators:

```sh
desktop-file-validate "$DESTDIR/usr/share/applications/org.example.Notes.desktop"
appstreamcli validate --no-net "$DESTDIR/usr/share/metainfo/org.example.Notes.metainfo.xml"
```

`crates/enzui-core/tests/packaging.rs` runs both on generated files
(`cargo wt --test packaging -- --include-ignored`).

## 7. Flatpak

[`docs/packaging/org.enzui.AppExample.yml`](packaging/org.enzui.AppExample.yml)
is a local-build manifest for the complete `app` example:

```sh
flatpak-builder --user --install --force-clean build-dir docs/packaging/org.enzui.AppExample.yml
flatpak run org.enzui.AppExample
```

Install the listed Flatpak runtime, SDK and Rust extension first. The SDK must
provide the repository's required Rust version. This manifest is a packaging
starting point; a Flatpak build and installed run have not been qualified in
this source review.

What it does, and what an application's own manifest changes:

- **Runtime.** `org.freedesktop.Platform` with the `rust-stable` SDK
  extension; enzUI needs no GNOME or KDE runtime.
- **Permissions.** The default example uses `--socket=wayland` and
  `--device=dri`. An application built with the optional X11 features can add
  `--socket=fallback-x11` and `--share=ipc` for X11 shared-memory support.
  File choosers, opening links, notifications and settings use portals;
  add network access only when the application needs it. See the
  [Flatpak permission reference](https://docs.flatpak.org/en/latest/sandbox-permissions.html).
- **D-Bus.** Inside the sandbox an application may own its own id on the
  session bus, which is what `ApplicationService::connect` needs; no
  `--own-name` is required.
- **Files.** The build installs the binary to `/app/bin` and runs it with
  `--write-package-files / /app`; Flatpak exports the desktop entry, icon,
  metainfo and service file from `/app/share`.
- **Offline builds.** The example gives cargo network access during the
  build, which is fine locally. Flathub builds offline: generate a
  `cargo-sources.json` with `flatpak-cargo-generator.py Cargo.lock`, add it
  to the module's sources, and build with `cargo --offline`.

## Licence notices

A binary built on enzUI carries third-party code and fonts whose licences ask
for their notices to travel with it. Install them beside the binary, for
example under `/usr/share/licenses/<app id>/`:

- enzUI's own `LICENSE` and the relevant bundled notices listed in
  [THIRD_PARTY.md](../THIRD_PARTY.md), including the Lucide icon licenses.
- iced's MIT `LICENSE` from the pinned [iced source](https://github.com/enz1m/enzim-iced): it
  covers iced and its `Iced-Icons.ttf`, which every build embeds.
- With iced's `fira-sans` feature enabled, `FiraSans-Regular.OFL.txt` from
  the iced source's `graphics/fonts/`: Fira Sans is under the SIL Open Font
  License, which must ship with the font.
  The iced source's `graphics/fonts/README.md` records the fonts' provenance.
- The licences of the crates.io dependencies, as `cargo about` or a similar
  tool collects them from `Cargo.lock`.

## Checklist

- One id, everywhere: desktop entry file name, `Icon`, `StartupWMClass`,
  the window app id, metainfo `<id>` and `<launchable>`, service `Name`,
  search provider `BusName`, the D-Bus name the application owns, the
  Flatpak app id.
- The desktop entry's `Exec` and the service file's `Exec` run the installed
  binary.
- A scalable icon, or PNGs at 48, 64 and 128 pixels at least.
- The metainfo has a description, a licence, a developer, a release and,
  for a software centre listing, screenshots.
- The licence notices above are installed with the binary.

## Source distribution and future registry publication

This release is consumed from Git. Every crate has `publish = false`.
Publishing fork repositories on GitHub does not publish `iced_* 0.15.0-dev`
on crates.io. Consumers need the [pinned source setup](dependencies.md).

A future registry release needs a separate review of dependency availability,
feature combinations, licenses and package contents. In particular,
`enzui-services/build.rs` compiles source under `vendor/gst-curl` outside its
crate directory; an individual crate archive must carry that input before it
can be published. Git checkouts include it today.

Build documentation locally with
`cargo doc --locked --workspace --all-features --no-deps` after installing
native build prerequisites. Hosted docs.rs builds have not been qualified.

# Platform support and prerequisites

enzUI targets Linux desktops. Wayland is the primary platform; X11 runs with
the reduced feature set listed below. Other operating systems are not
supported.

## Build prerequisites

The default desktop widget build needs:

- Rust 1.99.1 for the pinned development toolchain, Rust 1.95 minimum for
  consumers (edition 2024), and Git;
- a C toolchain and `pkg-config`;
- the Wayland client library, `libxkbcommon`, and a Vulkan loader with Mesa (or
  vendor) drivers. OpenGL/EGL works when Vulkan is missing, and the software
  renderer needs neither (see [Renderer selection](#renderer-selection)).

The pinned toolchain uses bundled LLD by default on
`x86_64-unknown-linux-gnu`. Keep linker flags consistent across build entry
points to reuse artifacts. Other targets may use their system linker.
Renderer-neutral crates do not require the full desktop graphics stack.

Optional features add system libraries:

| feature | crate | build needs | run needs |
|---|---|---|---|
| `gstreamer` (audio and video) | `enzui-services` | GStreamer 1.x and gst-plugins-base development files, libcurl | GStreamer plugins for the formats you play (good, bad, libav) |
| `mail-transport` | `enzui-services` | libcurl, OpenSSL | - |
| `curl-transport` (default) | `enzui-resources` | libcurl, OpenSSL, zlib | - |
| `pdf-preview` | `enzui-services` | - | a full `libpdfium.so` on the library path |
| `archive` | `enzui-services` | - | `libarchive.so.13` |
| `portal-dbus` and the portal services | `enzui-core` | - | a D-Bus session bus and `xdg-desktop-portal` with a backend for your desktop |
| `spell-check` | `enzui-services` | - | Hunspell dictionaries for the languages checked; Enchant (`libenchant-2.so.2`) when present, loaded at run time |
| `secret-portal` | `enzui-services` | - | inside Flatpak, `xdg-desktop-portal` with a Secret backend (GNOME Keyring, KWallet) |
| `gstreamer` event sounds (`event_sound`) | `enzui-services` | as `gstreamer` | a sound theme (`sound-theme-freedesktop`); with a PulseAudio or PipeWire server the sound plays at the event-sound volume |
| `shell-integration` (tray, launcher count) | `enzui-core` | - | a StatusNotifierItem host (KDE, the GNOME AppIndicator extension, waybar) for the tray |

`cargo wcheck` and `cargo wt` build every feature, so contributors need all of
the build column.

### Package names

| distribution | packages |
|---|---|
| Arch Linux | `base-devel git pkgconf wayland libxkbcommon vulkan-icd-loader mesa gstreamer gst-plugins-base gst-plugins-good curl openssl zlib libarchive` |
| Fedora | `gcc git pkgconf-pkg-config wayland-devel libxkbcommon-devel vulkan-loader mesa-vulkan-drivers gstreamer1-devel gstreamer1-plugins-base-devel libcurl-devel openssl-devel zlib-devel libarchive` |
| Debian, Ubuntu | `build-essential git pkg-config libwayland-dev libxkbcommon-dev libvulkan1 mesa-vulkan-drivers libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev libcurl4-openssl-dev libssl-dev zlib1g-dev libarchive13` |

PDF preview needs a compatible `libpdfium.so` at runtime. Use a distribution
package where available or a matching build from the
[pdfium-binaries project](https://github.com/bblanchon/pdfium-binaries). Without
it, `pdf-preview` reports a typed "libpdfium is not available" error.

## Wayland and X11

| capability | Wayland | X11 |
|---|---|---|
| windows, input, IME, fractional scaling | yes | yes (through winit) |
| clipboard | enzUI transport on the app's own connection: text, HTML, images, file lists | the runtime's default clipboard |
| native drag and drop to and from other applications | yes ([guide](native-drag-and-drop.md)) | files dropped from other applications arrive as `window::Event::FileDropped`; dragging out is not available |
| portal dialogs parented to the window | xdg-foreign export | `x11:` window id |
| launch activation tokens | `XDG_ACTIVATION_TOKEN` | `DESKTOP_STARTUP_ID` |
| desktop glass behind the window | where the compositor offers `enzui-material-v1` | no |
| menus, lists, tooltips and popovers that do not fit in the window | past its edge in `xdg_popup` surfaces | drawn inside the window and clipped |
| dialog windows kept above their parent | `xdg_toplevel.set_parent`, `xdg_wm_dialog_v1` for modality | `WM_TRANSIENT_FOR` |
| touchpad pinch, rotate and swipe gestures | yes | no |
| drawing tablets (`Event::Stylus`) | `zwp_tablet_v2` | no; the pen arrives as a mouse |
| pointer lock (`set_cursor_grab`) | lock and confine | confine only |
| image cursors | yes | yes |
| drag icon, dragging a window with a drag | `wl_data_device` icon, `xdg_toplevel_drag_v1` | no |
| the clipboard after the app exits | `ext_data_control_v1` | no |
| system bell, window icon | `xdg_system_bell_v1`, `xdg_toplevel_icon_v1` | no bell; the window icon through winit |
| window session restore | `xdg-session-management` | no |
| content type and tearing hints | `wp_content_type_v1`, `wp_tearing_control_v1` | no |

Overlays stay inside the window whenever they fit there, flipped or
shifted as needed, because a menu or tooltip hanging past the window looks
out of place on tiling window managers and over other windows. They open
past the edge only when they do not fit inside, or where the app allows it:
`anchored_overlay::set_default_popup_overflow(PopupOverflow::Allowed)` for
every overlay, or `.popup_overflow(..)` on an anchored overlay or tooltip
(`PopupOverflow::Never` keeps one inside always). Menus opened by a click
take the input grab in their popup; tooltips never do.

The app shell draws its own titlebar, window controls and resize edges and
asks the desktop for no decorations. A compositor may decorate the window
anyway (sway does for tiled windows); the shell then drops its own controls
and resize edges, so the window never shows two sets. A window that draws its own
frame is transparent, so its rounded corners show the desktop on any
compositing desktop (GNOME, KDE, sway, Weston); on X11 without a compositor
the corners show black.
`enz_window_settings_with_frame(.., ShellFrame::Desktop)` or `ENZUI_SSD=1`
asks for the desktop's decorations from the start.

Each Wayland row needs the compositor to offer that protocol; where it does
not, the call reports it (an `Unsupported` or `Unavailable` result) and the
window works as before.

Wayland is the default backend. [Optional X11 features](wayland-only.md) retain
the X11 window/renderer path; the Wayland-only graph still contains arboard's
`x11rb` clipboard dependency.

Layer-shell surfaces (panels, docks, lock screens) are not part of the
framework; they belong to the desktop shell built on top of it.

## Renderer selection

The default renderer tries the hardware backend (wgpu: Vulkan, then OpenGL)
and falls back to the tiny-skia software renderer when no adapter is usable.
It also uses the software renderer when the only adapter emulates a GPU on the
CPU (Mesa's llvmpipe or lavapipe, as in a virtual machine without GPU
passthrough): such an adapter rasterises every frame in full, and an animating
window then costs several CPU cores where the software renderer, redrawing
only what changed, needs a fraction of one. A virtual GPU (virtio-gpu with
virgl or venus) is real hardware acceleration and stays on wgpu.

Standard widgets remain usable under the software renderer. GPU-specific
effects use their fallbacks: glass draws as an opaque tinted surface, DMA-BUF external
images show their CPU fallback, and the terminal draws plain quads.

Set `ICED_BACKEND` to force one (`wgpu` keeps an emulated GPU, with its glass):

```sh
ICED_BACKEND=tiny-skia cargo run --locked --example app
ICED_BACKEND=wgpu cargo run --locked --example app
```

`WGPU_BACKEND=gl` or `WGPU_BACKEND=vulkan` narrows the hardware backend. Use
the software renderer to isolate GPU-specific problems. Performance depends on
the workload and available adapter. Tests render with the software backend
unless `ICED_TEST_BACKEND` says otherwise.

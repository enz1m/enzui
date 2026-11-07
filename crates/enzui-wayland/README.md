# enzui-wayland

Wayland platform integration that the iced runtime owns, on each window's own
connection and surface.

## Portal parent export

An XDG portal dialog (file chooser, print, screenshot) is parented to an app
window through a `parent_window` token. On Wayland that token is the reply to an
asynchronous [xdg-foreign v2](https://wayland.app/protocols/xdg-foreign-unstable-v2)
`export_toplevel` round trip, and the exported object must stay alive as long as
the portal request wants the relationship.

- `export_toplevel` performs the round trip and returns an `ExportLease` that
  owns the exported object.
- `adopt_parent` takes a runtime action payload; `ParentExport::open` takes an
  `OwnedWaylandWindow`.
- `ExportError` reports each failure distinctly, never as a silent unparented
  fallback.

The exporter never opens another display connection and never rebuilds proxies
from raw pointers. Keep the export alive until the portal request settles or is
cancelled. Parenting grants neither activation nor keyboard focus. The lease
rules themselves live in `enzui-parent-lease`.

## Desktop glass

`desktop_glass` asks the compositor for liquid glass over what lies behind a
window, where the compositor offers the `enzui-material-v1` protocol.

## System bell and surface hints

- `system_bell` rings the desktop's bell for a window (`xdg_system_bell_v1`),
  played as the user set it up. `enzui_widgets::system_bell::ring` is the task
  an app calls.
- `surface_hints` tells the compositor what a window shows (`wp_content_type_v1`:
  photo, video, game) and whether it may tear (`wp_tearing_control_v1`), for a
  game that wants the lowest latency. `enzui_widgets::surface_hints` wraps them
  as tasks.

Each reports `Unsupported` where the compositor does not offer the protocol.
Desktop glass and the bell share one registry round trip per connection.

# Changelog

## 0.1.0-alpha.1 — 2026-11-08

Initial public source snapshot. The API is alpha and distributed through Git;
no crates.io release is implied.

- Native file sources and MIME-aware drop regions share the default runtime,
  target-specific Copy/Move negotiation, clipped hit-testing, hover tints and
  deferred receipts. Held file gestures can leave scrollable viewports when starting a transfer. Optional document-portal file transfer supports sandbox peers.
- Temporary attachment exports carry a receiver hint; bounded owned snapshots
  keep queued media and imported documents usable after source cleanup.
- Shared theme roles drive rating, presence, badge and table-focus colours,
  plus default titlebar, shell and unpinned row metrics.

- Themed widgets, application shell, menus, dialogs, forms, collections and editors.
- Runtime appearance, typography, localization and reduced-motion preferences.
- Keyboard navigation, focus management and AccessKit accessibility integration.
- GPU and software rendering through the pinned iced fork, with materials,
  rounded clipping, retained presentation and frame diagnostics.
- Wayland clients by default, with additive X11 backend features.
- Optional portals, clipboard, media, resources, credentials and desktop services.
- Examples, public guides and behavior, rendering and security regression tests.

Linux desktops are supported, with Wayland as the primary platform and a reduced
X11 path. See [platform support](docs/platform-support.md) for requirements and
[the release review](docs/release-review.md) for validation and remaining gaps.

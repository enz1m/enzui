# enzui-icons

The bundled icon catalog contains a pinned Lucide set and original enzUI artwork.

## What to reach for first

- `bundled_icon(name)` / `bundled_icons()` - look up a single glyph or enumerate
  the catalog.
- Reference an icon by its **canonical name** (`search`, `enzim-ai`) or by a stable
  **semantic alias** (`action.search`, `ai.assistant`). Prefer the
  semantic alias in framework code: it survives a Lucide rename.

## What this crate is **not** for

- It is asset data + a registry, not rendering. The iced-side integration that
  actually draws an icon lives in the `icon` module of `enzui-widgets`.
- Do not add host-icon-theme lookups here - the whole point is a pinned catalog
  that renders identically on every machine.

# Icon Manifest

## Source

- **Repository**: https://github.com/lucide-icons/lucide
- **Commit**: 423afc6d03c1fb1b86090aa14b13f7f2fa6296e4
- **License**: ISC; Feather-derived icons are MIT. See `LICENSES/ISC.txt` and `LICENSES/MIT.txt`.

The files in `assets/lucide/` are copied from the pinned upstream commit above. `assets/fallback.svg` is an enzUI fallback asset.

## License split (SPDX: `MIT AND ISC`)

This crate's Cargo `license` field is **`MIT AND ISC`** - the union of:

| Portion | License | Text |
|---|---|---|
| Framework code (`src/**`, `build.rs`), generated `icon-manifest.toml`, `assets/fallback.svg` | **MIT** (matches the workspace license) | `LICENSES/MIT-enzUI.txt` |
| Bundled Lucide SVGs (`assets/lucide/*.svg`) | **ISC** | `LICENSES/ISC.txt` |
| Feather-derived Lucide subset | **MIT** | `LICENSES/MIT.txt` |

The framework portion is MIT licensed; the bundled assets retain the
original ISC/MIT notices listed above.

## Framework Policy

enzUI bundles Lucide as the default framework icon pack. Components should use stable semantic names such as `action.search` when the intent matters, or direct Lucide names such as `search` when an app wants a specific glyph. The framework does not depend on host Linux icon themes for built-in components.

## Catalog

- Canonical Lucide icons bundled: 1714
- Semantic aliases generated: 70

The canonical icon list is generated into `icon-manifest.toml` under `[icons]`.

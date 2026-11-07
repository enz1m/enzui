# enzui-theme

The design tokens every enzUI widget reads: palette, spacing, sizing, radius,
stroke, typography, motion, opacity, scrollbar, breakpoints, materials and font
roles. It ships one theme family, **enzUI Nebula**, in a dark and a light preset.

## What to reach for first

- `EnzTheme::dark()` / `EnzTheme::light()`: the full token set for a preset.
- `EnzTheme::iced_theme`: an `iced::Theme` that carries the tokens and projects
  the palette for stock iced widgets. Widgets in `enzui-widgets` read their
  unpinned tokens from the current theme. Rebuild pinned token snapshots and
  app-owned theme-dependent geometry when the theme generation changes.
- `system_theme_subscription()`: follow the desktop's light/dark preference.
- `Palette::high_contrast()`: the palette for a desktop that asks for more
  contrast; the shared theme file applies it on its own (see
  [`docs/styling.md`](../../docs/styling.md)).

```rust
use enzui_theme::EnzTheme;

let theme = EnzTheme::dark();
let background = theme.palette.background;
let gap = theme.spacing.space4;
```

## Features

| feature | default | what |
|---|---|---|
| `system-theme` | yes | colour-scheme tasks and subscriptions from the desktop |
| `system-reduced-motion` | yes | the desktop's reduced-motion preference |
| `desktop-fonts` | yes | the desktop's font preferences |
| `desktop-materials` | yes | the desktop's material (glass) preferences |
| `material-tuning` | no | process-wide material preset overrides |

The crate never depends on a renderer: `iced_core`, plus `iced_runtime` and
`iced_futures` for the desktop feeds. With default features off it is pure
data. A test (`tests/renderer_neutral_dependencies.rs`) keeps it that way.

## What this crate is **not** for

- Not widgets. Token structs per widget (`*Tokens`) live beside each widget in
  `enzui-widgets`; see [`docs/tokens.md`](../../docs/tokens.md).
- Not the user theme file. Loading `enzui/theme.toml` is `enzui-core`'s
  `theme_file`; see [`docs/theme-file.md`](../../docs/theme-file.md).

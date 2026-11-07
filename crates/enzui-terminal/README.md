# enzui-terminal

A fast terminal **view** for enzUI apps. The app keeps its own emulator
(parser, scrollback, modes, PTY); this crate draws the screen and turns
keyboard, mouse and selection input into `TerminalEvent`s.

## How it draws

- Glyphs are rasterised once into texture atlases (a mask atlas and a colour
  atlas for emoji) and drawn with one instanced draw per frame.
- Cell backgrounds come from one full-widget pass over a per-cell colour
  buffer, with no geometry per cell.
- Only rows the emulator changed are rebuilt and uploaded. An idle terminal
  does no work.
- Box drawing, block elements, braille and Powerline separators are drawn to
  the cell's exact pixels, so borders meet without gaps.
- Cells are whole device pixels on a device-pixel grid, so glyphs, backgrounds
  and the cursor agree to the pixel.

Under the software renderer the view falls back to the renderer's own quads and
text: much slower, but it still draws.

## What to reach for first

- `TerminalScreen`: the shared screen model. Write into it from the emulator
  thread with `TerminalScreen::update`, and map `TerminalScreen::wakes` to a
  message so the window redraws.
- `enz_terminal(&screen)`: the widget, styled with `.style(..)` and reporting
  input through `.on_event(..)`.

```rust,ignore
let screen = TerminalScreen::new(80, 24, style.foreground);
// Emulator thread, after parsing output:
screen.update(|w| {
    w.resize(cols, rows);
    for y in damaged_rows { w.set_row(y, cells_of(y)); }
    w.set_cursor(cursor);
});
// View:
enz_terminal(&screen).style(style.clone()).on_event(Message::Terminal)
```

## Features

`wayland` is enabled by default. The additive `x11` feature also enables
iced's X11 backend; forward it from an application's matching backend feature.
See [backend selection](../../docs/wayland-only.md).

## What this crate is **not** for

- Not a terminal emulator: no VT parser, no PTY, no scrollback storage.

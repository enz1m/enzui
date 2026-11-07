//! A GPU terminal view for enzUI apps.
//!
//! The emulator (parser, scrollback, modes, PTY) stays the app's; this crate
//! draws its screen and turns input into [`TerminalEvent`]s. The design
//! follows the terminals that are fast at this:
//!
//! - Glyphs are rasterised once into texture atlases (an 8-bit mask atlas and
//!   a colour atlas for emoji) and drawn as one instanced draw per frame, as
//!   in Alacritty, kitty and Ghostty.
//! - Cell backgrounds come from one full-widget pass that reads a per-cell
//!   colour buffer, as in Ghostty: no geometry per cell.
//! - Only rows the emulator changed are rebuilt and uploaded; an unchanged
//!   frame reuses the GPU buffers as they are, and an idle terminal does no
//!   work at all.
//! - Box drawing, block elements, braille and Powerline separators are drawn
//!   to the cell's exact pixels, so borders meet without gaps.
//! - Cell sizes are whole device pixels and the grid starts on a device
//!   pixel, so glyphs, backgrounds and the cursor agree to the pixel.
//!
//! An app keeps a [`TerminalScreen`], writes into it from its emulator (on
//! any thread), maps [`TerminalScreen::wakes`] to a no-op message so the
//! window redraws, and shows it with [`enz_terminal`]:
//!
//! ```ignore
//! let screen = TerminalScreen::new(80, 24, style.foreground);
//! // Emulator thread, after parsing output:
//! screen.update(|w| {
//!     w.resize(cols, rows);
//!     for y in damaged_rows { w.set_row(y, cells_of(y)); }
//!     w.set_cursor(cursor);
//! });
//! // View:
//! enz_terminal(&screen).style(style.clone()).on_event(Message::Terminal)
//! ```
//!
//! Under the software renderer the view falls back to the renderer's own quads
//! and text: far slower than the GPU pipeline, but it still draws.

#![cfg_attr(docsrs, feature(doc_cfg))]
mod boxdraw;
mod font;
mod palette;
mod render;
mod screen;
mod software;
mod widget;

pub use font::{CellMetrics, TerminalFont};
pub use palette::TerminalColors;
pub use screen::{
    Cell, CellFlags, Cursor, CursorShape, Modes, Rgba, ScreenState, ScreenWriter, Selection,
    TerminalScreen,
};
pub use widget::{EnzTerminal, MouseAction, TerminalEvent, enz_terminal};

/// How a terminal looks.
#[derive(Debug, Clone, PartialEq)]
pub struct TerminalStyle {
    /// The font every cell is drawn in.
    pub font: TerminalFont,
    /// The default background, shown where a cell has none and around the grid.
    pub background: Rgba,
    /// The default foreground, for blank cells.
    pub foreground: Rgba,
    /// The cursor colour.
    pub cursor: Rgba,
    /// The glyph colour under a block cursor.
    pub cursor_text: Rgba,
    /// The background of selected cells.
    pub selection_background: Rgba,
    /// The foreground of selected cells; `None` keeps each cell's own.
    pub selection_foreground: Option<Rgba>,
    /// Space between the widget's edge and the grid, in logical pixels.
    pub padding: f32,
    /// How long the focused cursor stays on, then off; `None` keeps it solid.
    /// Blinking stops after 10 seconds without input or cursor movement, as
    /// GNOME's `cursor-blink-timeout` does, so an idle terminal stays idle.
    pub cursor_blink: Option<std::time::Duration>,
    /// Whether the space right of and below the grid (the part of a cell
    /// that does not fit, and the padding) takes the background of the
    /// nearest edge cell, so a full-screen program's own background reaches
    /// the window's edge. Cells with the default background leave it as
    /// `background`.
    pub extend_background: bool,
}

impl Default for TerminalStyle {
    fn default() -> Self {
        Self::from_colors(&TerminalColors::default())
    }
}

impl TerminalStyle {
    /// A style drawn from `colors`.
    #[must_use]
    pub fn from_colors(colors: &TerminalColors) -> Self {
        Self {
            font: TerminalFont::default(),
            background: colors.background,
            foreground: colors.foreground,
            cursor: colors.foreground,
            cursor_text: colors.background,
            selection_background: colors.selection,
            selection_foreground: None,
            padding: 6.0,
            cursor_blink: Some(std::time::Duration::from_millis(600)),
            extend_background: true,
        }
    }
}

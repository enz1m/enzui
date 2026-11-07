//! The colour table an emulator resolves indexed colours against.

use crate::screen::Rgba;

/// A terminal's colours: the 16 ANSI colours and the defaults.
///
/// The default is the GNOME Console (libadwaita) dark palette.
#[derive(Debug, Clone, PartialEq)]
pub struct TerminalColors {
    /// Black, red, green, yellow, blue, magenta, cyan, white, then their
    /// bright variants.
    pub ansi: [Rgba; 16],
    /// The default foreground.
    pub foreground: Rgba,
    /// The default background.
    pub background: Rgba,
    /// The selection background.
    pub selection: Rgba,
}

impl Default for TerminalColors {
    fn default() -> Self {
        let c = |hex: u32| Rgba::rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8);
        Self {
            ansi: [
                c(0x241f31),
                c(0xc01c28),
                c(0x2ec27e),
                c(0xf5c211),
                c(0x1e78e4),
                c(0x9841bb),
                c(0x0ab9dc),
                c(0xc0bfbc),
                c(0x5e5c64),
                c(0xed333b),
                c(0x57e389),
                c(0xf8e45c),
                c(0x51a1ff),
                c(0xc061cb),
                c(0x4fd2fd),
                c(0xf6f5f4),
            ],
            foreground: c(0xffffff),
            background: c(0x1e1e1e),
            selection: c(0x3d5a80),
        }
    }
}

impl TerminalColors {
    /// The colour of 256-colour index `index`: the ANSI 16, then the xterm
    /// 6 x 6 x 6 cube, then the 24 greys.
    #[must_use]
    pub fn indexed(&self, index: u8) -> Rgba {
        match index {
            0..=15 => self.ansi[usize::from(index)],
            16..=231 => {
                let i = index - 16;
                let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                Rgba::rgb(level(i / 36), level((i / 6) % 6), level(i % 6))
            }
            _ => {
                let v = 8 + (index - 232) * 10;
                Rgba::rgb(v, v, v)
            }
        }
    }
}

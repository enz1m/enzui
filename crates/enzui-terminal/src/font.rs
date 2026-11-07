//! Cell metrics and glyph rasterisation.
//!
//! Every grapheme is shaped on its own (no ligatures) through the process
//! font system, so fallback fonts and colour emoji resolve exactly as in the
//! rest of the UI. Cell sizes are whole device pixels, as in other terminals:
//! the grid, backgrounds, cursor and glyphs then agree to the pixel.

use iced_graphics::text::cosmic_text::{
    self, Attrs, Buffer, Family, Metrics, Shaping, Style, SwashCache, SwashContent, Weight,
};

/// The font a terminal draws with.
#[derive(Debug, Clone, PartialEq)]
pub struct TerminalFont {
    /// The family name; `None` follows the desktop's monospace font (the
    /// enzUI monospace role, fed from the desktop settings).
    ///
    /// `None` is resolved again every time the view measures, so a desktop
    /// font change reaches it. An app that also measures cells itself, to
    /// size its PTY before the view reports a size, should pass the same
    /// [`concrete`](Self::concrete) font to both.
    pub family: Option<String>,
    /// The size in logical pixels; 11 pt by default, as desktop terminals use.
    pub size: f32,
    /// Line height as a multiple of the font's own ascent + descent + gap.
    pub line_height: f32,
}

impl Default for TerminalFont {
    fn default() -> Self {
        Self {
            family: None,
            size: 11.0 * 96.0 / 72.0,
            line_height: 1.0,
        }
    }
}

impl TerminalFont {
    /// This font with `family` filled in from the desktop's monospace font
    /// when unset, so a desktop font change reaches the terminal.
    #[must_use]
    pub fn resolved(&self) -> Self {
        if self.family.is_some() {
            return self.clone();
        }
        let family = match enzui_theme::text_defaults::family(enzui_theme::FontRole::Monospace) {
            enzui_theme::FontFamily::Named(name) => Some(name),
            enzui_theme::FontFamily::Generic(_) => None,
        };
        Self {
            family,
            ..self.clone()
        }
    }

    /// This font with `family` set to the name of the installed family it
    /// draws with now, so measuring it later gives the same cells even if
    /// the desktop's monospace font or the generic family changes meanwhile.
    #[must_use]
    pub fn concrete(&self) -> Self {
        let resolved = self.resolved();
        if resolved.family.is_some() {
            return resolved;
        }
        let mut system = iced_graphics::text::font_system()
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let system = system.raw();
        let mut buffer = Buffer::new(system, Metrics::new(self.size, self.size * 2.0));
        buffer.set_text("M", &resolved.attrs(false, false), Shaping::Advanced, None);
        buffer.shape_until_scroll(system, false);
        let family = buffer
            .layout_runs()
            .next()
            .and_then(|run| run.glyphs.first().map(|glyph| glyph.font_id))
            .and_then(|id| system.db().face(id))
            .and_then(|face| face.families.first().map(|(name, _)| name.clone()));
        Self { family, ..resolved }
    }

    fn attrs(&self, bold: bool, italic: bool) -> Attrs<'_> {
        let family = self
            .family
            .as_deref()
            .map_or(Family::Monospace, Family::Name);
        Attrs::new()
            .family(family)
            .weight(if bold { Weight::BOLD } else { Weight::NORMAL })
            .style(if italic { Style::Italic } else { Style::Normal })
    }
}

/// One cell's size and the lines within it, in device pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellMetrics {
    /// Cell width.
    pub width: u32,
    /// Cell height.
    pub height: u32,
    /// Baseline, from the cell's top.
    pub baseline: u32,
    /// Underline top, from the cell's top.
    pub underline_top: u32,
    /// Underline and strikethrough thickness.
    pub stroke: u32,
    /// Strikethrough top, from the cell's top.
    pub strike_top: u32,
    /// The font size glyphs are rasterised at.
    pub(crate) font_px: f32,
}

impl CellMetrics {
    /// Measures `font` at `scale` device pixels per logical pixel.
    #[must_use]
    pub fn measure(font: &TerminalFont, scale: f32) -> Self {
        let px = (font.size * scale).max(1.0);
        let mut system = iced_graphics::text::font_system()
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let system = system.raw();
        let mut buffer = Buffer::new(system, Metrics::new(px, px * 2.0));
        buffer.set_text("M", &font.attrs(false, false), Shaping::Advanced, None);
        buffer.shape_until_scroll(system, false);
        let glyph = buffer
            .layout_runs()
            .next()
            .and_then(|run| run.glyphs.first().map(|g| (g.w, g.font_id)));
        let (advance, font_id) = glyph.map_or((px * 0.6, None), |(w, id)| (w, Some(id)));
        let metrics = font_id
            .and_then(|id| system.get_font(id, cosmic_text::fontdb::Weight::NORMAL))
            .map(|face| face.as_swash().metrics(&[]).scale(px));

        let (ascent, descent, gap, underline, stroke, strike) = metrics.map_or(
            (px * 0.8, px * 0.2, 0.0, -px * 0.1, px / 14.0, px * 0.3),
            |m| {
                (
                    m.ascent,
                    m.descent.abs(),
                    m.leading,
                    m.underline_offset,
                    m.stroke_size,
                    m.strikeout_offset,
                )
            },
        );
        let natural = ascent + descent + gap;
        let height = (natural * font.line_height.max(0.5)).round().max(1.0);
        let width = advance.round().max(1.0);
        let baseline = ((height - (ascent + descent)) / 2.0 + ascent).round();
        let stroke = stroke.round().max(1.0);
        // Underline offset is below the baseline (negative in font units).
        let underline_top = (baseline - underline - stroke / 2.0)
            .round()
            .clamp(0.0, height - stroke);
        let strike_top = (baseline - strike - stroke / 2.0)
            .round()
            .clamp(0.0, height - stroke);
        Self {
            width: width as u32,
            height: height as u32,
            baseline: baseline as u32,
            underline_top: underline_top as u32,
            stroke: stroke as u32,
            strike_top: strike_top as u32,
            font_px: px,
        }
    }
}

/// A rasterised glyph image placed relative to its cell's top-left.
#[derive(Debug, Clone)]
pub(crate) struct RasterGlyph {
    /// Left edge from the cell's left edge.
    pub left: i32,
    /// Top edge from the cell's top edge.
    pub top: i32,
    pub width: u32,
    pub height: u32,
    /// Whether `data` is RGBA (a colour glyph) rather than an 8-bit mask.
    pub color: bool,
    pub data: Vec<u8>,
}

/// Shapes and rasterises graphemes; keeps its swash scaler between calls.
pub(crate) struct Rasterizer {
    swash: SwashCache,
}

impl Rasterizer {
    pub(crate) fn new() -> Self {
        Self {
            swash: SwashCache::new(),
        }
    }

    /// The glyph images for `text`, in `span` cells, placed in a cell of
    /// `metrics`. Empty when the text draws nothing.
    pub(crate) fn rasterize(
        &mut self,
        font: &TerminalFont,
        metrics: &CellMetrics,
        text: &str,
        bold: bool,
        italic: bool,
        span: u32,
    ) -> Vec<RasterGlyph> {
        let mut system = iced_graphics::text::font_system()
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let system = system.raw();
        let mut glyphs =
            self.rasterize_at(system, font, metrics, text, bold, italic, metrics.font_px);
        // A colour glyph (emoji) drawn from a font at its natural size often
        // overflows the cell; shrink it to fit the cells it owns.
        let limit_w = (metrics.width * span) as f32;
        let limit_h = metrics.height as f32;
        if let Some(big) = glyphs.iter().find(|g| g.color) {
            let fit = (limit_w / big.width.max(1) as f32).min(limit_h / big.height.max(1) as f32);
            if fit < 1.0 {
                glyphs = self.rasterize_at(
                    system,
                    font,
                    metrics,
                    text,
                    bold,
                    italic,
                    metrics.font_px * fit,
                );
            }
        }
        // Centre a colour glyph in its cells; text glyphs keep the font's
        // own bearings.
        for glyph in &mut glyphs {
            if glyph.color {
                glyph.left = ((limit_w - glyph.width as f32) / 2.0).round() as i32;
                glyph.top = ((limit_h - glyph.height as f32) / 2.0).round() as i32;
            }
        }
        glyphs
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the font system, the cache and the glyph key"
    )]
    fn rasterize_at(
        &mut self,
        system: &mut cosmic_text::FontSystem,
        font: &TerminalFont,
        metrics: &CellMetrics,
        text: &str,
        bold: bool,
        italic: bool,
        px: f32,
    ) -> Vec<RasterGlyph> {
        let mut buffer = Buffer::new(system, Metrics::new(px, metrics.height as f32));
        buffer.set_text(text, &font.attrs(bold, italic), Shaping::Advanced, None);
        buffer.shape_until_scroll(system, false);
        let mut out = Vec::new();
        for run in buffer.layout_runs() {
            for glyph in run.glyphs {
                let physical = glyph.physical((0.0, 0.0), 1.0);
                let Some(image) = self.swash.get_image_uncached(system, physical.cache_key) else {
                    continue;
                };
                if image.placement.width == 0 || image.placement.height == 0 {
                    continue;
                }
                let (color, data) = match image.content {
                    SwashContent::Mask => (false, image.data),
                    SwashContent::Color => (true, image.data),
                    // Subpixel masks are averaged to coverage: terminals
                    // draw on arbitrary backgrounds, as the UI text does.
                    SwashContent::SubpixelMask => (
                        false,
                        image
                            .data
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .map(|p| {
                                ((u16::from(p[0]) + u16::from(p[1]) + u16::from(p[2])) / 3) as u8
                            })
                            .collect(),
                    ),
                };
                out.push(RasterGlyph {
                    left: physical.x + image.placement.left,
                    top: metrics.baseline as i32 + physical.y - image.placement.top,
                    width: image.placement.width,
                    height: image.placement.height,
                    color,
                    data,
                });
            }
        }
        out
    }
}

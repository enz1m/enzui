//! Text selection over positioned glyphs: the model behind selecting text on
//! a page that is a picture (a PDF raster, a scanned page with an OCR layer).
//!
//! A document viewer knows where each character sits but has no text widget
//! to select in. Selecting by rectangle (a rubber band picking whole lines)
//! is not text selection: it cannot start mid-word, cannot end mid-line, and
//! across a column break it sweeps up lines the reader never meant. Text
//! selection runs from one caret to another in reading order, whatever
//! shape that leaves on the page.
//!
//! [`GlyphText`] holds the glyphs in reading order. It answers the three
//! questions a selection gesture asks:
//!
//! * where the caret lands for a pointer position ([`GlyphText::caret_at`]);
//! * what a double or triple click selects ([`GlyphText::word_at`],
//!   [`GlyphText::line_at`]);
//! * what to paint and copy for a range ([`GlyphText::range_rects`],
//!   [`GlyphText::text`]).
//!
//! Coordinates are the caller's, origin top-left, y down; nothing here knows
//! about PDF user space.
//!
//! ```
//! use enzui_core::glyph_selection::{Glyph, GlyphText};
//!
//! let text = GlyphText::new(
//!     "Hi you"
//!         .chars()
//!         .enumerate()
//!         .map(|(i, ch)| Glyph::new(ch, [i as f32 * 10.0, 0.0, 10.0, 12.0], 0))
//!         .collect(),
//! );
//! let start = text.caret_at(17.0, 6.0); // past the middle of "i": after it
//! assert_eq!(start, 2);
//! assert_eq!(text.text(text.word_at(4)), "you");
//! ```

use std::ops::Range;

/// One character and the box it occupies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glyph {
    /// The character, used for word boundaries and for copying.
    pub ch: char,
    /// `[x, y, width, height]`, origin top-left.
    pub rect: [f32; 4],
    /// Which line of the page it belongs to, in reading order. Lines start
    /// a new row of selection rectangles and are joined by a newline when
    /// copied.
    pub line: u32,
}

impl Glyph {
    /// A glyph `ch` at `rect` on line `line`.
    pub const fn new(ch: char, rect: [f32; 4], line: u32) -> Self {
        Self { ch, rect, line }
    }

    fn left(&self) -> f32 {
        self.rect[0]
    }

    fn right(&self) -> f32 {
        self.rect[0] + self.rect[2]
    }

    fn top(&self) -> f32 {
        self.rect[1]
    }

    fn bottom(&self) -> f32 {
        self.rect[1] + self.rect[3]
    }

    fn centre_x(&self) -> f32 {
        self.rect[0] + self.rect[2] / 2.0
    }
}

/// The glyphs of a page in reading order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GlyphText {
    glyphs: Vec<Glyph>,
    /// Each line's glyph range, in order.
    lines: Vec<Range<usize>>,
}

impl GlyphText {
    /// Glyphs in reading order. Consecutive glyphs with the same `line` form
    /// one line.
    pub fn new(glyphs: Vec<Glyph>) -> Self {
        let mut lines = Vec::new();
        let mut start = 0;
        for i in 1..=glyphs.len() {
            if i == glyphs.len() || glyphs[i].line != glyphs[start].line {
                lines.push(start..i);
                start = i;
            }
        }
        Self { glyphs, lines }
    }

    /// The glyphs, in reading order.
    pub fn glyphs(&self) -> &[Glyph] {
        &self.glyphs
    }

    /// Whether there is any text to select.
    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }

    /// The vertical extent of a line: the union of its glyph boxes.
    fn line_span(&self, line: &Range<usize>) -> (f32, f32) {
        self.glyphs[line.clone()]
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(top, bottom), g| {
                (top.min(g.top()), bottom.max(g.bottom()))
            })
    }

    /// The caret position, `0..=len`, for a pointer at `(x, y)`.
    ///
    /// Above the first line the caret is at the start of the text, below
    /// the last line at its end, so a drag that leaves the text selects to
    /// its start or end and a drag in the empty space under it selects
    /// nothing. Otherwise the line is the one whose vertical extent holds
    /// `y`, or the nearest; within it the caret goes before the first glyph
    /// whose centre is right of `x`, or after the line's last glyph.
    pub fn caret_at(&self, x: f32, y: f32) -> usize {
        if let (Some(first), Some(last)) = (self.lines.first(), self.lines.last()) {
            if y < self.line_span(first).0 {
                return 0;
            }
            if y > self.line_span(last).1 {
                return self.glyphs.len();
            }
        }
        let Some(line) = self
            .lines
            .iter()
            .min_by(|a, b| {
                let distance = |line: &Range<usize>| {
                    let (top, bottom) = self.line_span(line);
                    if y < top {
                        top - y
                    } else if y > bottom {
                        y - bottom
                    } else {
                        0.0
                    }
                };
                distance(a).total_cmp(&distance(b))
            })
            .cloned()
        else {
            return 0;
        };
        self.glyphs[line.clone()]
            .iter()
            .position(|g| g.centre_x() > x)
            .map_or(line.end, |offset| line.start + offset)
    }

    /// The word around the glyph at `index`: a run of letters, digits and
    /// connector punctuation, or the single glyph when it is neither.
    pub fn word_at(&self, index: usize) -> Range<usize> {
        let Some(glyph) = self.glyphs.get(index) else {
            return index..index;
        };
        let wordish = |g: &Glyph| g.ch.is_alphanumeric() || g.ch == '_' || g.ch == '\'';
        if !wordish(glyph) {
            return index..index + 1;
        }
        let line = glyph.line;
        let same = |g: &Glyph| g.line == line && wordish(g);
        let mut start = index;
        while start > 0 && same(&self.glyphs[start - 1]) {
            start -= 1;
        }
        let mut end = index + 1;
        while end < self.glyphs.len() && same(&self.glyphs[end]) {
            end += 1;
        }
        start..end
    }

    /// The whole line holding the glyph at `index`.
    pub fn line_at(&self, index: usize) -> Range<usize> {
        self.lines
            .iter()
            .find(|line| line.contains(&index))
            .cloned()
            .unwrap_or(index..index)
    }

    /// The rectangles that paint `range`: one per line it touches, from its
    /// first selected glyph's left edge to its last one's right edge, as tall
    /// as the line.
    pub fn range_rects(&self, range: Range<usize>) -> Vec<[f32; 4]> {
        let range = range.start.min(self.glyphs.len())..range.end.min(self.glyphs.len());
        self.lines
            .iter()
            .filter_map(|line| {
                let start = line.start.max(range.start);
                let end = line.end.min(range.end);
                (start < end).then(|| {
                    let (top, bottom) = self.line_span(line);
                    let left = self.glyphs[start..end]
                        .iter()
                        .map(Glyph::left)
                        .fold(f32::INFINITY, f32::min);
                    let right = self.glyphs[start..end]
                        .iter()
                        .map(Glyph::right)
                        .fold(f32::NEG_INFINITY, f32::max);
                    [left, top, right - left, bottom - top]
                })
            })
            .collect()
    }

    /// The text of `range`, with a newline between lines.
    pub fn text(&self, range: Range<usize>) -> String {
        let range = range.start.min(self.glyphs.len())..range.end.min(self.glyphs.len());
        let mut out = String::new();
        let mut line = None;
        for glyph in &self.glyphs[range] {
            if line.is_some_and(|l| l != glyph.line) {
                // A line that ended in a hyphen continues the word.
                if out.ends_with('-') {
                    out.pop();
                } else {
                    out.push('\n');
                }
            }
            line = Some(glyph.line);
            out.push(glyph.ch);
        }
        out
    }
}

/// A selection from an anchor caret to a moving caret, either way round.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GlyphSelection {
    /// Where the gesture started.
    pub anchor: usize,
    /// Where it is now.
    pub focus: usize,
}

impl GlyphSelection {
    /// A collapsed selection at `caret`.
    pub const fn at(caret: usize) -> Self {
        Self {
            anchor: caret,
            focus: caret,
        }
    }

    /// The selected glyph range, start before end.
    pub fn range(&self) -> Range<usize> {
        self.anchor.min(self.focus)..self.anchor.max(self.focus)
    }

    /// Whether nothing is selected.
    pub fn is_collapsed(&self) -> bool {
        self.anchor == self.focus
    }
}

#[cfg(test)]
#[path = "glyph_selection/tests.rs"]
mod tests;

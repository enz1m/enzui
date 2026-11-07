#![allow(
    clippy::string_slice,
    reason = "document ranges always sit on char boundaries"
)]

use super::*;

/// A coalesced styled run: a byte range plus its shared style.
#[derive(Debug, Clone, PartialEq)]
pub struct StyledRun {
    /// Byte range (half-open) this run covers in the backing text.
    pub range: Range<usize>,
    /// The style shared across every byte in `range`.
    pub style: SpanStyle,
}

/// How a paragraph's lines sit across the editor's column.
///
/// Presentational, not direction-relative: the model has no text direction, so
/// `Left` and `Right` name edges of the column rather than the start and end of
/// the reading order. Each variant is one paint and one HTML `text-align`
/// keyword, so a document has one way to say a given alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParagraphAlign {
    /// Lines sit against the left edge. The default, emitted as no attribute.
    #[default]
    Left,
    /// Lines are centred in the column.
    Center,
    /// Lines sit against the right edge.
    Right,
    /// Lines are stretched to both edges, except the last of the paragraph.
    Justify,
}

impl ParagraphAlign {
    /// The renderer alignment this paints as.
    ///
    /// `Left` maps to the renderer's explicit `Left` rather than its
    /// direction-resolved default, because the model's alignment is a column
    /// edge the document chose, not one the direction picks.
    pub fn to_text_alignment(self) -> iced::advanced::text::Alignment {
        match self {
            ParagraphAlign::Left => iced::advanced::text::Alignment::Left,
            ParagraphAlign::Center => iced::advanced::text::Alignment::Center,
            ParagraphAlign::Right => iced::advanced::text::Alignment::Right,
            ParagraphAlign::Justify => iced::advanced::text::Alignment::Justified,
        }
    }
}

/// Block-level formatting carried by one paragraph.
///
/// A struct rather than a bare [`ParagraphAlign`] so a second block property
/// (indent, space-before) does not change the shape of every caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ParagraphFormat {
    /// How this paragraph's lines sit across the column.
    pub align: ParagraphAlign,
    /// The list this paragraph is an item of, if any. Its marker is ordinary
    /// text at the start of the body, so the caret and selection treat it as
    /// text; see [`ListKind::marker_len`].
    pub list: Option<ListKind>,
}

/// The kind of list a paragraph is an item of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListKind {
    /// A bulleted list: `• ` markers, written as `<ul>`.
    Bullet,
    /// A numbered list: `1. ` markers, written as `<ol>`.
    Numbered,
}

impl ListKind {
    /// The marker text for item `number` (1-based) of a list of this kind.
    pub fn marker(self, number: usize) -> String {
        match self {
            ListKind::Bullet => "\u{2022} ".to_owned(),
            ListKind::Numbered => format!("{number}. "),
        }
    }

    /// The byte length of this kind's marker at the start of `body`, or
    /// `None` when the body does not start with one.
    pub fn marker_len(self, body: &str) -> Option<usize> {
        match self {
            ListKind::Bullet => body.starts_with("\u{2022} ").then_some("\u{2022} ".len()),
            ListKind::Numbered => {
                let digits = body.bytes().take_while(u8::is_ascii_digit).count();
                (digits > 0 && body[digits..].starts_with(". ")).then_some(digits + 2)
            }
        }
    }

    /// The number of a numbered item's marker at the start of `body`.
    pub(crate) fn number_in(body: &str) -> Option<usize> {
        let digits = body.bytes().take_while(u8::is_ascii_digit).count();
        body[..digits].parse().ok()
    }
}

/// The body ranges of `text`'s paragraphs, excluding the line break that ends
/// each one.
///
/// `\r\n` and `\n\r` are one break, a lone `\r` or `\n` is one break, and the
/// text after the last break is a paragraph even when it is empty — so a text
/// ending in a break owns a final empty paragraph. Never empty: an empty text
/// is one empty paragraph.
///
/// The one splitter: the paragraph vector, the HTML emitter and the editor
/// highlighter all count paragraphs through this function, so none of them can
/// disagree about where one ends.
pub(crate) fn line_body_ranges(text: &str) -> Vec<Range<usize>> {
    line_bodies(text).collect()
}

/// [`line_body_ranges`] one at a time, for a lookup that stops early.
fn line_bodies(text: &str) -> impl Iterator<Item = Range<usize>> + '_ {
    let bytes = text.as_bytes();
    let mut line_start = 0;
    let mut cursor = 0;
    let mut done = false;
    std::iter::from_fn(move || {
        while cursor < bytes.len() {
            if let Some(len) = break_len(bytes, cursor) {
                let body = line_start..cursor;
                cursor += len;
                line_start = cursor;
                return Some(body);
            }
            cursor += 1;
        }
        (!std::mem::replace(&mut done, true)).then_some(line_start..text.len())
    })
}

// The one statement of the pairing rule: every reading and count of a break
// goes through it.
fn break_len(bytes: &[u8], cursor: usize) -> Option<usize> {
    let first = *bytes.get(cursor)?;
    if !matches!(first, b'\r' | b'\n') {
        return None;
    }
    let paired = matches!(
        (first, bytes.get(cursor + 1).copied()),
        (b'\r', Some(b'\n')) | (b'\n', Some(b'\r'))
    );
    Some(if paired { 2 } else { 1 })
}

fn count_breaks(bytes: &[u8]) -> usize {
    let mut breaks = 0;
    let mut cursor = 0;
    while cursor < bytes.len() {
        match break_len(bytes, cursor) {
            Some(len) => {
                breaks += 1;
                cursor += len;
            }
            None => cursor += 1,
        }
    }
    breaks
}

fn index_in(ranges: &[Range<usize>], at: usize) -> usize {
    ranges
        .iter()
        .position(|range| at <= range.end)
        .unwrap_or(ranges.len() - 1)
}

/// A mutable rich-text editing document: plain text, per-byte inline
/// formatting, and per-paragraph block formatting.
#[derive(Debug, Clone, PartialEq)]
#[must_use]
pub struct RichEditDocument {
    pub(crate) text: String,
    /// One [`EditFormat`] per byte of `text`.
    pub(crate) formats: Vec<EditFormat>,
    /// One [`ParagraphFormat`] per paragraph of `text`, in order. Always as
    /// long as the text's own paragraph list, so it is never shorter than 1.
    pub(crate) paragraphs: Vec<ParagraphFormat>,
    /// Link targets, indexed by [`EditFormat`]'s link id. Append-only, so an
    /// undo that restores formats never points at a missing entry.
    pub(crate) links: Vec<String>,
}

impl Default for RichEditDocument {
    fn default() -> Self {
        Self {
            text: String::new(),
            formats: Vec::new(),
            paragraphs: vec![ParagraphFormat::default()],
            links: Vec::new(),
        }
    }
}

impl RichEditDocument {
    /// An empty document.
    pub fn new() -> Self {
        Self::default()
    }

    /// A document seeded with unstyled `text`.
    pub fn from_plain(text: impl Into<String>) -> Self {
        let text = text.into();
        let formats = vec![EditFormat::default(); text.len()];
        let paragraphs = vec![ParagraphFormat::default(); line_body_ranges(&text).len()];
        Self {
            text,
            formats,
            paragraphs,
            links: Vec::new(),
        }
    }

    /// Assemble a document from parts already in document shape, used by the
    /// HTML reader, which builds all three vectors as it parses.
    pub(crate) fn from_parts(
        text: String,
        formats: Vec<EditFormat>,
        paragraphs: Vec<ParagraphFormat>,
    ) -> Self {
        debug_assert_eq!(formats.len(), text.len(), "one format per byte of text");
        debug_assert_eq!(
            paragraphs.len(),
            line_body_ranges(&text).len(),
            "one paragraph per line body"
        );
        Self {
            text,
            formats,
            paragraphs,
            links: Vec::new(),
        }
    }

    /// [`Self::from_parts`] with a link table the formats index into.
    pub(crate) fn from_parts_with_links(
        text: String,
        formats: Vec<EditFormat>,
        paragraphs: Vec<ParagraphFormat>,
        links: Vec<String>,
    ) -> Self {
        let mut doc = Self::from_parts(text, formats, paragraphs);
        doc.links = links;
        doc
    }

    /// The document's plain text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The document length in bytes.
    pub fn len(&self) -> usize {
        self.text.len()
    }

    /// Whether the document has no text.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Clamp `at` to `[0, len]` and snap down to the nearest char boundary.
    fn snap(&self, at: usize) -> usize {
        let mut at = at.min(self.text.len());
        while at > 0 && !self.text.is_char_boundary(at) {
            at -= 1;
        }
        at
    }

    /// Normalize a caller range: clamp + snap both ends, order ascending.
    pub(crate) fn normalize(&self, range: Range<usize>) -> Range<usize> {
        let a = self.snap(range.start);
        let b = self.snap(range.end);
        if a <= b { a..b } else { b..a }
    }

    /// Insert `new_text` at byte offset `at`. New bytes inherit the format of
    /// the char to their left (typing continues the current style), or plain at
    /// the very start. Returns the offset just past the insertion.
    pub fn insert(&mut self, at: usize, new_text: &str) -> usize {
        let at = self.snap(at);
        if new_text.is_empty() {
            return at;
        }
        let mut inherit = if at > 0 {
            self.formats.get(at - 1).copied().unwrap_or_default()
        } else {
            EditFormat::default()
        };
        // Typing at the end of a link does not extend it.
        if inherit.link.is_some() && self.formats.get(at).map(|f| f.link) != Some(inherit.link) {
            inherit.link = None;
        }
        let delta = self.break_delta(at, 0, new_text.as_bytes());
        self.text.insert_str(at, new_text);
        self.formats
            .splice(at..at, std::iter::repeat_n(inherit, new_text.len()));
        self.apply_break_delta(at, delta);
        at + new_text.len()
    }

    /// Insert `new_text` at `at` with an explicit `format` (used for pending
    /// typing attributes — see [`RichEditController`]). Returns the offset past
    /// the insertion.
    pub fn insert_with(&mut self, at: usize, new_text: &str, format: EditFormat) -> usize {
        let at = self.snap(at);
        if new_text.is_empty() {
            return at;
        }
        let delta = self.break_delta(at, 0, new_text.as_bytes());
        self.text.insert_str(at, new_text);
        self.formats
            .splice(at..at, std::iter::repeat_n(format, new_text.len()));
        self.apply_break_delta(at, delta);
        at + new_text.len()
    }

    /// Delete byte `range`. Returns the normalized start (new caret position).
    pub fn delete(&mut self, range: Range<usize>) -> usize {
        let range = self.normalize(range);
        if range.is_empty() {
            return range.start;
        }
        let delta = self.break_delta(range.start, range.len(), &[]);
        self.text.replace_range(range.clone(), "");
        self.formats.drain(range.clone());
        self.apply_break_delta(range.start, delta);
        range.start
    }

    // How many paragraphs an edit at `at` adds (positive) or removes
    // (negative), replacing `removed` bytes with `inserted`. Call before the
    // edit.
    //
    // Counted over the edited bytes and the whole `\r`/`\n` run they touch, not
    // over the document: breaks pair greedily, so a byte landing in a run can
    // retokenize the run around it, while a run is bounded by bytes that can
    // never pair. Counting the typed bytes alone would call a `\r` typed in
    // front of an existing `\n` a new paragraph.
    fn break_delta(&self, at: usize, removed: usize, inserted: &[u8]) -> isize {
        let bytes = self.text.as_bytes();
        let is_break_byte = |byte: u8| matches!(byte, b'\r' | b'\n');

        let mut start = at;
        while start > 0 && is_break_byte(bytes[start - 1]) {
            start -= 1;
        }
        let mut end = at + removed;
        while end < bytes.len() && is_break_byte(bytes[end]) {
            end += 1;
        }

        let before = &bytes[start..end];
        let mut after = Vec::with_capacity(before.len() + inserted.len());
        after.extend_from_slice(&bytes[start..at]);
        after.extend_from_slice(inserted);
        after.extend_from_slice(&bytes[at + removed..end]);

        count_breaks(&after) as isize - count_breaks(before) as isize
    }

    // Bring the paragraph vector back in step after an edit at `at` that
    // changed the break count by `delta`. Call after the edit: the text before
    // `at`, which the edit does not touch, decides which paragraph it was in.
    //
    // A split gives the new paragraphs the format they split from; a merge
    // keeps the format of the first of the paragraphs it joined.
    fn apply_break_delta(&mut self, at: usize, delta: isize) {
        if delta == 0 {
            return;
        }
        let index = count_breaks(&self.text.as_bytes()[..at]);
        if delta > 0 {
            let inherit = self.paragraphs.get(index).copied().unwrap_or_default();
            let split = (index + 1).min(self.paragraphs.len());
            self.paragraphs.splice(
                split..split,
                std::iter::repeat_n(inherit, delta.unsigned_abs()),
            );
        } else {
            let start = (index + 1).min(self.paragraphs.len());
            let end = (start + delta.unsigned_abs()).min(self.paragraphs.len());
            self.paragraphs.drain(start..end);
        }
    }

    /// The body ranges of the document's paragraphs, line breaks excluded.
    /// Never empty.
    pub fn paragraph_ranges(&self) -> Vec<Range<usize>> {
        line_body_ranges(&self.text)
    }

    /// How many paragraphs the document holds. At least one.
    pub fn paragraph_count(&self) -> usize {
        self.paragraphs.len()
    }

    /// Every paragraph's block formatting, in document order.
    pub fn paragraph_formats(&self) -> &[ParagraphFormat] {
        &self.paragraphs
    }

    /// The index of the paragraph holding byte offset `at`.
    pub fn paragraph_index_at(&self, at: usize) -> usize {
        let at = self.snap(at);
        // The last body ends at the text's end, so a snapped offset always lands.
        line_bodies(&self.text)
            .position(|range| at <= range.end)
            .unwrap_or_else(|| self.paragraphs.len() - 1)
    }

    /// The paragraphs `range` touches, as a half-open index range.
    ///
    /// A selection that stops exactly where the next paragraph begins has not
    /// reached into it, so aligning "the first line and its break" leaves the
    /// following paragraph alone. A caret reports the single paragraph it sits
    /// in, including at a paragraph's first offset.
    pub fn paragraph_span(&self, range: Range<usize>) -> Range<usize> {
        let range = self.normalize(range);
        let ranges = self.paragraph_ranges();
        let first = index_in(&ranges, range.start);
        let mut last = index_in(&ranges, range.end);
        if last > first && ranges[last].start >= range.end {
            last -= 1;
        }
        first..last + 1
    }

    /// The block formatting of paragraph `index`, or `None` past the last one.
    pub fn paragraph_format(&self, index: usize) -> Option<ParagraphFormat> {
        self.paragraphs.get(index).copied()
    }

    /// Align every paragraph `range` touches.
    ///
    /// Unlike [`Self::set_style`] this is meaningful with an empty range: a
    /// caret still sits in exactly one paragraph, which is how alignment is
    /// normally applied.
    pub fn set_paragraph_align(&mut self, align: ParagraphAlign, range: Range<usize>) {
        for index in self.paragraph_span(range) {
            if let Some(paragraph) = self.paragraphs.get_mut(index) {
                paragraph.align = align;
            }
        }
    }

    /// The alignment shared by every paragraph `range` touches, or `None` when
    /// they disagree — what an alignment control shows as chosen.
    pub fn active_paragraph_align(&self, range: Range<usize>) -> Option<ParagraphAlign> {
        let span = self.paragraph_span(range);
        let first = self.paragraphs.get(span.start)?.align;
        self.paragraphs[span]
            .iter()
            .all(|paragraph| paragraph.align == first)
            .then_some(first)
    }

    /// Replace `range` with `new_text` (delete then insert). Returns the offset
    /// past the inserted text.
    ///
    /// The replacement keeps the format of what it replaced: the format at
    /// the start of the deleted range is captured *before* the delete and
    /// applied to the inserted text. This matters at offset 0, where a plain
    /// `insert` has no left neighbour to inherit from and would degrade to the
    /// default format — typing over a bold first word must stay bold. (This is handled
    /// here rather than in `insert`: an `insert` at 0 into an existing
    /// document genuinely has no left neighbour, and making it inherit from the
    /// *right* would change unrelated behaviour.)
    ///
    /// If the range is empty (a pure insertion), there is nothing being
    /// replaced, so the ordinary left-inheriting `insert` applies.
    pub fn replace(&mut self, range: Range<usize>, new_text: &str) -> usize {
        let range = self.normalize(range);
        // Capture the replaced style before the delete removes it. Snap the
        // start (already snapped by `normalize`) and read the format there.
        let replaced_format = if range.is_empty() {
            None
        } else {
            self.formats.get(range.start).copied()
        };
        let start = self.delete(range);
        match replaced_format {
            Some(format) => self.insert_with(start, new_text, format),
            None => self.insert(start, new_text),
        }
    }

    /// Whether `style` is active across the entire `range`. An empty range
    /// reports the style of the char to the left (what typing would produce).
    pub fn is_style_active(&self, style: InlineStyle, range: Range<usize>) -> bool {
        let range = self.normalize(range);
        if range.is_empty() {
            return range
                .start
                .checked_sub(1)
                .and_then(|i| self.formats.get(i))
                .is_some_and(|f| f.has(style));
        }
        self.formats[range].iter().all(|f| f.has(style))
    }

    /// The set of styles active across the entire `range`, in [`InlineStyle::ALL`]
    /// order — what a toolbar uses to light its "on" buttons.
    pub fn active_styles(&self, range: Range<usize>) -> Vec<InlineStyle> {
        InlineStyle::ALL
            .into_iter()
            .filter(|&s| self.is_style_active(s, range.clone()))
            .collect()
    }

    /// Apply or remove `style` across `range`. No-op on an empty range (the
    /// controller tracks pending typing attributes for the empty case).
    pub fn set_style(&mut self, style: InlineStyle, on: bool, range: Range<usize>) {
        let range = self.normalize(range);
        for f in &mut self.formats[range] {
            f.set(style, on);
        }
    }

    /// Toggle `style` across `range`: remove if active across the whole
    /// selection, otherwise apply to the whole selection (word-processor
    /// toolbar semantics).
    pub fn toggle_style(&mut self, style: InlineStyle, range: Range<usize>) {
        let active = self.is_style_active(style, range.clone());
        self.set_style(style, !active, range);
    }

    /// Set (or clear with `None`) the semantic tone across `range`.
    pub fn set_tone(&mut self, tone: Option<TextTone>, range: Range<usize>) {
        let range = self.normalize(range);
        for f in &mut self.formats[range] {
            f.tone = tone;
        }
    }

    /// Set (or clear with `None`) the authored ink across `range`.
    pub fn set_color(&mut self, color: Option<iced::Color>, range: Range<usize>) {
        let range = self.normalize(range);
        for f in &mut self.formats[range] {
            f.color = color;
        }
    }

    /// The ink shared by every byte of `range`, or `None` when the range is
    /// mixed, uninked or empty.
    ///
    /// An empty range reports the ink of the char to the left, matching
    /// [`Self::is_style_active`]: that is what typing at the caret would
    /// inherit.
    pub fn active_color(&self, range: Range<usize>) -> Option<iced::Color> {
        let range = self.normalize(range);
        if range.is_empty() {
            return range
                .start
                .checked_sub(1)
                .and_then(|i| self.formats.get(i))
                .and_then(|f| f.color);
        }
        let first = self.formats[range.clone()].first()?.color?;
        self.formats[range]
            .iter()
            .all(|f| f.color == Some(first))
            .then_some(first)
    }

    /// Set (or clear with `None`) the marker colour across `range`.
    pub fn set_highlight(&mut self, color: Option<iced::Color>, range: Range<usize>) {
        let range = self.normalize(range);
        for f in &mut self.formats[range] {
            f.highlight = color;
        }
    }

    /// The marker colour shared by every byte of `range`, or `None` when mixed
    /// or unmarked; an empty range reports the char to the left.
    pub fn active_highlight(&self, range: Range<usize>) -> Option<iced::Color> {
        let range = self.normalize(range);
        if range.is_empty() {
            return range
                .start
                .checked_sub(1)
                .and_then(|i| self.formats.get(i))
                .and_then(|f| f.highlight);
        }
        let first = self.formats[range.clone()].first()?.highlight?;
        self.formats[range]
            .iter()
            .all(|f| f.highlight == Some(first))
            .then_some(first)
    }

    /// Link `range` to `url`, or unlink it with `None`.
    pub fn set_link(&mut self, url: Option<&str>, range: Range<usize>) {
        let range = self.normalize(range);
        let id = url.map(|url| self.link_id(url));
        for f in &mut self.formats[range] {
            f.link = id;
        }
    }

    // The table index for `url`, appending it when new.
    pub(crate) fn link_id(&mut self, url: &str) -> usize {
        match self.links.iter().position(|known| known == url) {
            Some(index) => index,
            None => {
                self.links.push(url.to_owned());
                self.links.len() - 1
            }
        }
    }

    /// The link target of the byte at `at`, if it belongs to a link.
    pub fn link_at(&self, at: usize) -> Option<&str> {
        let id = self.formats.get(at)?.link?;
        self.links.get(id).map(String::as_str)
    }

    /// The byte range of the link holding `at` (or the char left of it), if any.
    pub fn link_range_at(&self, at: usize) -> Option<Range<usize>> {
        let probe = if self.formats.get(at).and_then(|f| f.link).is_some() {
            at
        } else {
            at.checked_sub(1)?
        };
        let id = self.formats.get(probe)?.link?;
        let mut start = probe;
        while start > 0 && self.formats[start - 1].link == Some(id) {
            start -= 1;
        }
        let mut end = probe + 1;
        while end < self.formats.len() && self.formats[end].link == Some(id) {
            end += 1;
        }
        Some(start..end)
    }

    /// The list paragraph `index` is an item of, when its marker is still in
    /// place; a list item whose marker was deleted reads as a plain paragraph.
    pub fn list_item(&self, index: usize) -> Option<(ListKind, usize)> {
        let kind = self.paragraphs.get(index)?.list?;
        let body = self.paragraph_ranges().get(index)?.clone();
        kind.marker_len(&self.text[body]).map(|len| (kind, len))
    }

    /// The list kind every paragraph `range` touches shares, if any.
    pub fn active_list(&self, range: Range<usize>) -> Option<ListKind> {
        let mut span = self.paragraph_span(range);
        let first = self.list_item(span.start)?.0;
        span.all(|index| self.list_item(index).map(|(kind, _)| kind) == Some(first))
            .then_some(first)
    }

    /// Make every paragraph `range` touches an item of a `kind` list, or a
    /// plain paragraph again when all of them already are. Rewrites the
    /// markers and returns `range` moved to follow them.
    pub fn toggle_list(&mut self, kind: ListKind, range: Range<usize>) -> Range<usize> {
        let span = self.paragraph_span(range.clone());
        let off = span
            .clone()
            .all(|index| self.list_item(index).map(|(item, _)| item) == Some(kind));
        // Numbering continues a numbered list just above the span.
        let mut first_number = 1;
        if kind == ListKind::Numbered && span.start > 0 {
            let ranges = self.paragraph_ranges();
            if let Some((ListKind::Numbered, _)) = self.list_item(span.start - 1) {
                first_number = ListKind::number_in(&self.text[ranges[span.start - 1].clone()])
                    .map_or(1, |n| n + 1);
            }
        }
        let mut selection = self.normalize(range);
        let shift = |at: usize, removed: usize, inserted: usize, pos: usize| {
            if pos >= at + removed {
                pos - removed + inserted
            } else if pos > at {
                at + inserted
            } else {
                pos
            }
        };
        for index in span.clone().rev() {
            let body = self.paragraph_ranges()[index].clone();
            let removed = self.list_item(index).map_or(0, |(_, len)| len);
            let marker = if off {
                String::new()
            } else {
                kind.marker(first_number + (index - span.start))
            };
            self.delete(body.start..body.start + removed);
            self.insert_with(body.start, &marker, EditFormat::default());
            self.paragraphs[index].list = (!off).then_some(kind);
            selection = shift(body.start, removed, marker.len(), selection.start)
                ..shift(body.start, removed, marker.len(), selection.end);
        }
        selection
    }

    /// Remove all inline styles, tone and authored ink across `range`.
    pub fn clear_formatting(&mut self, range: Range<usize>) {
        let range = self.normalize(range);
        for f in &mut self.formats[range] {
            *f = EditFormat::default();
        }
    }

    /// Coalesce equal adjacent per-byte formats into runs, emitting boundaries
    /// only on char boundaries. Every byte is covered (plain stretches emit
    /// plain runs), so `runs()` is a faithful, gap-free segmentation.
    pub fn runs(&self) -> Vec<StyledRun> {
        self.runs_styled(EditFormat::to_span_style)
    }

    /// [`Self::runs`] with the styles the HTML writer emits: a link run keeps
    /// its own boundaries but not the underline and accent it paints with.
    pub(crate) fn written_runs(&self) -> Vec<StyledRun> {
        self.runs_styled(EditFormat::to_written_style)
    }

    fn runs_styled(&self, style_of: fn(EditFormat) -> SpanStyle) -> Vec<StyledRun> {
        let mut runs: Vec<StyledRun> = Vec::new();
        let n = self.text.len();
        let mut i = 0usize;
        while i < n {
            let here = self.formats[i];
            let mut j = i + 1;
            while j < n && (!self.text.is_char_boundary(j) || self.formats[j] == here) {
                j += 1;
            }
            runs.push(StyledRun {
                range: i..j,
                style: style_of(here),
            });
            i = j;
        }
        runs
    }

    /// Bridge to the read-only block renderer: one display block per
    /// paragraph, carrying that paragraph's styled runs and its alignment.
    ///
    /// Paragraph boundaries are the document's own, so a `\r\n` break is one
    /// boundary and a text ending in a break still ends in a paragraph; an
    /// empty paragraph becomes an empty block, so a blank line stays a blank
    /// line. Inline styles and authored inks ride along as inline runs.
    ///
    /// Display only: [`crate::block_list::enz_block_list`] renders these, and
    /// nothing here edits.
    pub fn to_display_blocks(&self) -> Vec<crate::block_list::EnzBlock> {
        let runs = self.runs();
        self.paragraph_ranges()
            .into_iter()
            .zip(&self.paragraphs)
            .map(|(body, paragraph)| {
                let block = crate::block_list::EnzBlock::paragraph(&self.text[body.clone()])
                    .with_align_x(paragraph.align.to_text_alignment());
                if body.is_empty() {
                    // An empty paragraph has no runs to carry, and a rich text
                    // of no spans shapes to nothing; the plain path keeps the
                    // blank line the document has.
                    return block;
                }
                let spans = runs
                    .iter()
                    .filter_map(|run| {
                        let clipped = run.range.start.max(body.start)..run.range.end.min(body.end);
                        (clipped.start < clipped.end)
                            .then(|| RichSpan::styled(&self.text[clipped], run.style))
                    })
                    .collect();
                block.with_inline(RichText::new(spans).coalesced())
            })
            .collect()
    }

    /// Bridge to the immutable display model: produce a [`RichText`]
    /// whose spans mirror the document's coalesced runs. This is how an editor
    /// hands its current buffer to the shared rich-text renderer.
    pub fn to_rich_text(&self) -> RichText {
        let spans = self
            .runs()
            .into_iter()
            .map(|run| RichSpan::styled(&self.text[run.range], run.style))
            .collect();
        RichText::new(spans).coalesced()
    }
}

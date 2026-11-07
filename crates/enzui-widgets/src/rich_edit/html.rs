//! HTML persistence for authored rich-text formatting.
//!
//! [`to_html`] and [`rich_text_to_html`] emit escaped text, inline typography,
//! colors, links, lists and paragraph alignment without a document envelope.
//! [`from_html`] reads this restricted vocabulary and rejects unsupported markup.
//! It is intended for saved framework documents; untrusted general HTML needs
//! the sanitizer owned by its host application.

#![allow(
    clippy::string_slice,
    reason = "every offset comes from `find` or `len_utf8` on the same text"
)]

use super::*;
use crate::rich_text::{RichText, SpanStyle};

// Bodies come from the document's own splitter and hold no break bytes, so
// this writes characters only. Breaks are emitted between bodies by
// `emit_flat`, never from inside a run: a run is a span of style, and a `\r\n`
// pair whose two bytes carry different styles is still one separator.
fn escape_body(text: &str, out: &mut String) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
}

/// Which inline tags to open for a given style, in nesting order.
/// The order is deterministic: strong > em > u > s > code.
fn style_tags(style: &SpanStyle) -> Vec<&'static str> {
    let mut tags = Vec::new();
    if style.bold {
        tags.push("strong");
    }
    if style.italic {
        tags.push("em");
    }
    if style.underline {
        tags.push("u");
    }
    if style.strikethrough {
        tags.push("s");
    }
    if style.code {
        tags.push("code");
    }
    tags
}

/// Format an ink as a `#rrggbb` literal, clamping each channel into range.
///
/// Alpha is dropped: `#rrggbb` is what mail clients parse reliably, and a
/// translucent glyph has no defined colour without the recipient's background.
pub(crate) fn css_hex(color: iced::Color) -> String {
    let channel = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        channel(color.r),
        channel(color.g),
        channel(color.b)
    )
}

fn family_css(family: iced::font::Family) -> String {
    use iced::font::Family;
    match family {
        Family::Serif => "serif".into(),
        Family::SansSerif => "sans-serif".into(),
        Family::Cursive => "cursive".into(),
        Family::Fantasy => "fantasy".into(),
        Family::Monospace => "monospace".into(),
        Family::Name(name) => {
            let mut css = String::from("'");
            for character in name.chars() {
                if character == '\'' || character == '\\' || character.is_control() {
                    use std::fmt::Write as _;
                    let _ = write!(css, "\\{:x} ", character as u32);
                } else {
                    css.push(character);
                }
            }
            css.push('\'');
            css
        }
    }
}

fn parse_family(css: &str) -> Option<iced::font::Family> {
    use iced::font::Family;
    Some(match css {
        "serif" => Family::Serif,
        "sans-serif" => Family::SansSerif,
        "cursive" => Family::Cursive,
        "fantasy" => Family::Fantasy,
        "monospace" => Family::Monospace,
        _ => {
            let mut rest = css.strip_prefix('\'')?.strip_suffix('\'')?;
            let mut name = String::new();
            while let Some(character) = rest.chars().next() {
                rest = &rest[character.len_utf8()..];
                if character == '\\' {
                    let end = rest.find(' ')?;
                    if end == 0 || end > 6 {
                        return None;
                    }
                    let character = char::from_u32(u32::from_str_radix(&rest[..end], 16).ok()?)?;
                    name.push(character);
                    rest = &rest[end + 1..];
                } else if character == '\'' || character.is_control() {
                    return None;
                } else {
                    name.push(character);
                }
            }
            if name.is_empty() {
                return None;
            }
            Family::name(&name)
        }
    })
}

fn open_style(style: &SpanStyle, link: Option<&str>, out: &mut String) -> Vec<&'static str> {
    if let Some(href) = link {
        out.push_str("<a href=\"");
        escape_body(href, out);
        out.push_str("\">");
    }
    if let Some(color) = style.color {
        out.push_str("<span style=\"color:");
        out.push_str(&css_hex(color));
        out.push_str("\">");
    }
    if let Some(color) = style.highlight {
        out.push_str("<span style=\"background-color:");
        out.push_str(&css_hex(color));
        out.push_str("\">");
    }
    if let Some(font) = style.font {
        out.push_str("<span style=\"font-family:");
        escape_body(&family_css(font), out);
        out.push_str("\">");
    }
    if let Some(size) = style
        .text_size
        .filter(|size| size.is_finite() && *size > 0.0)
    {
        out.push_str("<span style=\"font-size:");
        out.push_str(&size.to_string());
        out.push_str("px\">");
    }
    let tags = style_tags(style);
    for tag in &tags {
        out.push('<');
        out.push_str(tag);
        out.push('>');
    }
    tags
}

fn close_style(tags: &[&'static str], style: &SpanStyle, link: Option<&str>, out: &mut String) {
    for tag in tags.iter().rev() {
        out.push_str("</");
        out.push_str(tag);
        out.push('>');
    }
    if style
        .text_size
        .is_some_and(|size| size.is_finite() && size > 0.0)
    {
        out.push_str("</span>");
    }
    if style.font.is_some() {
        out.push_str("</span>");
    }
    if style.highlight.is_some() {
        out.push_str("</span>");
    }
    if style.color.is_some() {
        out.push_str("</span>");
    }
    if link.is_some() {
        out.push_str("</a>");
    }
}

// `text` must be break-free: separators are the document's, not a run's.
fn emit_run(text: &str, style: &SpanStyle, link: Option<&str>, out: &mut String) {
    let tags = open_style(style, link, out);
    escape_body(text, out);
    close_style(&tags, style, link, out);
}

// A separator carries a style because the bytes that make it up do; dropping
// it would silently unbold a break inside a bold passage.
fn emit_break(style: &SpanStyle, out: &mut String) {
    let tags = open_style(style, None, out);
    out.push_str("<br>");
    close_style(&tags, style, None, out);
}

struct Piece {
    style: SpanStyle,
    link: Option<String>,
    content: String,
    // A stretch holding a separator may take an adjacent stretch of the same
    // style into its wrapper.
    is_break: bool,
}

// Emit `text` as flat HTML: styled runs inside each paragraph body, one `<br>`
// between bodies.
//
// Separators are counted over the whole text, where a paragraph boundary lives,
// and not inside each run: a `\r\n` whose two bytes carry different styles
// would otherwise split into two breaks and invent a paragraph.
//
// A separator is written with the style of its first byte — one break, one
// style, and the first byte is the one the caret was on when the break was
// typed. Where that style equals its neighbours', the three share one wrapper,
// so a bold passage containing a break stays a single `<strong>`.
fn emit_flat(
    text: &str,
    runs: &[StyledRun],
    link_of: &dyn Fn(usize) -> Option<String>,
    out: &mut String,
) {
    let bodies = super::document::line_body_ranges(text);
    // `runs` covers the text in ascending, non-overlapping order (see
    // `RichEditDocument::runs`), and so do `bodies`; both walks below query
    // ascending positions, so each keeps a cursor instead of re-scanning the
    // run list once per paragraph.
    let mut break_cursor = 0usize;
    let mut style_at = |at: usize| {
        while break_cursor < runs.len() && runs[break_cursor].range.end <= at {
            break_cursor += 1;
        }
        runs.get(break_cursor)
            .filter(|run| run.range.contains(&at))
            .map(|run| run.style)
            .unwrap_or_default()
    };

    let mut pieces: Vec<Piece> = Vec::new();
    let mut push = |style: &SpanStyle, link: Option<String>, content: String, is_break: bool| {
        // Merged only across a separator: two adjacent runs of equal style are
        // the caller's own division and are left as they were written. A link
        // never takes a break into its anchor.
        if let Some(last) = pieces.last_mut()
            && last.style == *style
            && last.link.is_none()
            && link.is_none()
            && (is_break || last.is_break)
        {
            last.content.push_str(&content);
            last.is_break |= is_break;
            return;
        }
        pieces.push(Piece {
            style: *style,
            link,
            content,
            is_break,
        });
    };

    let mut cursor = 0usize;
    for (index, body) in bodies.iter().enumerate() {
        if index > 0 {
            let at = bodies[index - 1].end;
            push(&style_at(at), None, "<br>".to_owned(), true);
        }
        // A run reaching past this body stays under the cursor: it is the same
        // run the next body starts from.
        while cursor < runs.len() && runs[cursor].range.end <= body.start {
            cursor += 1;
        }
        for run in &runs[cursor..] {
            if run.range.start >= body.end {
                break;
            }
            let clipped = run.range.start.max(body.start)..run.range.end.min(body.end);
            if clipped.start < clipped.end {
                let mut escaped = String::new();
                let link = link_of(clipped.start);
                escape_body(&text[clipped], &mut escaped);
                push(&run.style, link, escaped, false);
            }
        }
    }

    for piece in pieces {
        if piece.is_break && piece.content == "<br>" {
            emit_break(&piece.style, out);
            continue;
        }
        let link = piece.link.as_deref();
        let tags = open_style(&piece.style, link, out);
        out.push_str(&piece.content);
        close_style(&tags, &piece.style, link, out);
    }
}

// `None` for the default alignment, which is written as no attribute at all.
fn align_keyword(align: ParagraphAlign) -> Option<&'static str> {
    match align {
        ParagraphAlign::Left => None,
        ParagraphAlign::Center => Some("center"),
        ParagraphAlign::Right => Some("right"),
        ParagraphAlign::Justify => Some("justify"),
    }
}

/// Convert a [`RichEditDocument`] to an HTML string.
///
/// Walks the document's coalesced [`StyledRun`]s and emits inline HTML
/// tags for each styled run. Plain runs emit bare escaped text.
///
/// ```
/// use enzui_widgets::rich_edit::{RichEditDocument, InlineStyle, to_html};
///
/// let mut doc = RichEditDocument::from_plain("hello world");
/// doc.set_style(InlineStyle::Bold, true, 0..5);
/// let html = to_html(&doc);
/// assert_eq!(html, "<strong>hello</strong> world");
/// ```
///
/// # Paragraphs
///
/// A separator is one `<br>` however its bytes are styled: a `\r\n` whose two
/// bytes carry different styles is still one paragraph boundary, and it is
/// written in the style of its first byte.
///
/// A document whose paragraphs are all [`ParagraphAlign::Left`] emits the flat
/// shape above: breaks as `<br>` and no block wrapper. As soon as one paragraph
/// carries another alignment, every paragraph is wrapped in a `<p>` instead,
/// aligned ones carrying `style="text-align:…"`; mixing `<p>` blocks with loose
/// `<br>` runs would leave one document with two shapes.
pub fn to_html(doc: &RichEditDocument) -> String {
    let mut out = String::new();
    let paragraphs = doc.paragraph_formats();
    let link_of = |at: usize| doc.link_at(at).map(str::to_owned);
    let has_list = (0..paragraphs.len()).any(|index| doc.list_item(index).is_some());
    if !has_list
        && paragraphs
            .iter()
            .all(|paragraph| paragraph.align == ParagraphAlign::Left)
    {
        emit_flat(doc.text(), &doc.written_runs(), &link_of, &mut out);
        return out;
    }

    let runs = doc.written_runs();
    let mut cursor = 0usize;
    let mut open_list: Option<ListKind> = None;
    for (index, (body, paragraph)) in doc
        .paragraph_ranges()
        .into_iter()
        .zip(paragraphs)
        .enumerate()
    {
        let item = doc.list_item(index);
        if open_list.is_some() && item.map(|(kind, _)| kind) != open_list {
            out.push_str(list_close(open_list));
            open_list = None;
        }
        // An item's marker is the list's own rendering, not text.
        let body = match item {
            Some((kind, marker_len)) => {
                if open_list.is_none() {
                    out.push_str(list_open(kind));
                    open_list = Some(kind);
                }
                match align_keyword(paragraph.align) {
                    Some(keyword) => {
                        out.push_str("<li style=\"text-align:");
                        out.push_str(keyword);
                        out.push_str("\">");
                    }
                    None => out.push_str("<li>"),
                }
                body.start + marker_len..body.end
            }
            None => {
                match align_keyword(paragraph.align) {
                    Some(keyword) => {
                        out.push_str("<p style=\"text-align:");
                        out.push_str(keyword);
                        out.push_str("\">");
                    }
                    None => out.push_str("<p>"),
                }
                body
            }
        };
        while cursor < runs.len() && runs[cursor].range.end <= body.start {
            cursor += 1;
        }
        for run in &runs[cursor..] {
            if run.range.start >= body.end {
                break;
            }
            let clipped = run.range.start.max(body.start)..run.range.end.min(body.end);
            if clipped.start < clipped.end {
                let link = link_of(clipped.start);
                emit_run(&doc.text()[clipped], &run.style, link.as_deref(), &mut out);
            }
        }
        out.push_str(if item.is_some() { "</li>" } else { "</p>" });
    }
    if open_list.is_some() {
        out.push_str(list_close(open_list));
    }
    out
}

/// The alignment of a `<p>` or `<li>` opening tag: bare is left, otherwise the
/// one `style="text-align:…"` attribute the emitter writes.
fn block_align(tag: &str, name: &str) -> Result<ParagraphAlign, FromHtmlError> {
    if tag == name {
        return Ok(ParagraphAlign::Left);
    }
    let keyword = tag
        .strip_prefix(name)
        .and_then(|rest| rest.strip_prefix(" style=\"text-align:"))
        .and_then(|value| value.strip_suffix('"'))
        .ok_or_else(|| FromHtmlError::UnsupportedTag(tag.to_owned()))?;
    match keyword {
        "left" => Ok(ParagraphAlign::Left),
        "center" => Ok(ParagraphAlign::Center),
        "right" => Ok(ParagraphAlign::Right),
        "justify" => Ok(ParagraphAlign::Justify),
        other => Err(FromHtmlError::UnsupportedAlignment(other.to_owned())),
    }
}

fn list_open(kind: ListKind) -> &'static str {
    match kind {
        ListKind::Bullet => "<ul>",
        ListKind::Numbered => "<ol>",
    }
}

fn list_close(kind: Option<ListKind>) -> &'static str {
    match kind {
        Some(ListKind::Numbered) => "</ol>",
        _ => "</ul>",
    }
}

/// Convert a [`RichText`] (the immutable display model) to an HTML string.
///
/// Walks the spans and emits inline HTML tags for each styled span. Spans
/// divide style, not paragraphs, so a break split across two spans emits one
/// `<br>`, not one per span.
pub fn rich_text_to_html<L: Clone + PartialEq>(rt: &RichText<L>) -> String {
    let mut text = String::new();
    let mut runs = Vec::with_capacity(rt.spans().len());
    for span in rt.spans() {
        let start = text.len();
        text.push_str(&span.text);
        runs.push(StyledRun {
            range: start..text.len(),
            style: span.style,
        });
    }
    let mut out = String::new();
    // Spans divide style, not paragraphs, so a break landing on a span
    // boundary is still one break.
    emit_flat(&text, &runs, &|_| None, &mut out);
    out
}

/// Why a string could not be read back into a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FromHtmlError {
    /// A tag outside the emitter's vocabulary.
    UnsupportedTag(String),
    /// A `text-align` keyword outside the emitter's vocabulary.
    UnsupportedAlignment(String),
    /// A closing tag that does not close the innermost open one.
    MismatchedClose(String),
    /// A tag still open at the end of the input.
    UnclosedTag(String),
    /// An entity outside the five the emitter writes.
    UnknownEntity(String),
    /// Markup shaped in a way the emitter cannot produce.
    Malformed(String),
}

impl std::fmt::Display for FromHtmlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FromHtmlError::UnsupportedTag(tag) => write!(f, "unsupported tag `<{tag}>`"),
            FromHtmlError::UnsupportedAlignment(keyword) => {
                write!(f, "unsupported text-align `{keyword}`")
            }
            FromHtmlError::MismatchedClose(tag) => write!(f, "mismatched closing tag `{tag}`"),
            FromHtmlError::UnclosedTag(tag) => write!(f, "unclosed tag `<{tag}>`"),
            FromHtmlError::UnknownEntity(entity) => write!(f, "unknown entity `{entity}`"),
            FromHtmlError::Malformed(what) => {
                write!(f, "markup this reader cannot produce: {what}")
            }
        }
    }
}

impl std::error::Error for FromHtmlError {}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Inline {
    Strong,
    Em,
    Underline,
    Strike,
    Code,
    Ink(iced::Color),
    Highlight(iced::Color),
    Link(usize),
    Font(iced::font::Family),
    TextSize(f32),
}

impl Inline {
    fn tag(self) -> &'static str {
        match self {
            Inline::Strong => "strong",
            Inline::Em => "em",
            Inline::Underline => "u",
            Inline::Strike => "s",
            Inline::Code => "code",
            Inline::Ink(_) | Inline::Highlight(_) | Inline::Font(_) | Inline::TextSize(_) => "span",
            Inline::Link(_) => "a",
        }
    }

    // A `<span>` is not here: it names an ink only with an attribute.
    fn opened_by(tag: &str) -> Option<Self> {
        match tag {
            "strong" => Some(Inline::Strong),
            "em" => Some(Inline::Em),
            "u" => Some(Inline::Underline),
            "s" => Some(Inline::Strike),
            "code" => Some(Inline::Code),
            _ => None,
        }
    }
}

// Nesting order does not matter, so the reader accepts the emitter's order
// without depending on it.
fn format_from(stack: &[Inline]) -> EditFormat {
    let mut format = EditFormat::default();
    for entry in stack {
        match *entry {
            Inline::Strong => format.set(InlineStyle::Bold, true),
            Inline::Em => format.set(InlineStyle::Italic, true),
            Inline::Underline => format.set(InlineStyle::Underline, true),
            Inline::Strike => format.set(InlineStyle::Strikethrough, true),
            Inline::Code => format.set(InlineStyle::Code, true),
            Inline::Ink(color) => format.color = Some(color),
            Inline::Highlight(color) => format.highlight = Some(color),
            Inline::Link(id) => format.link = Some(id),
            Inline::Font(font) => format.font = Some(font),
            Inline::TextSize(size) => format.text_size = Some(size),
        }
    }
    format
}

// Reverses `escape_body` for the five entities it writes; anything else is
// refused rather than passed through, so a stray `&` cannot become text.
fn unescape(text: &str) -> Result<String, FromHtmlError> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(offset) = rest.find('&') {
        out.push_str(&rest[..offset]);
        let tail = &rest[offset..];
        let end = tail
            .find(';')
            .ok_or_else(|| FromHtmlError::UnknownEntity(tail.to_owned()))?;
        out.push(match &tail[..=end] {
            "&amp;" => '&',
            "&lt;" => '<',
            "&gt;" => '>',
            "&quot;" => '"',
            "&#39;" => '\'',
            other => return Err(FromHtmlError::UnknownEntity(other.to_owned())),
        });
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

// Alpha is opaque: the emitter drops it.
fn parse_hex(literal: &str) -> Option<iced::Color> {
    let digits = literal.strip_prefix('#')?;
    if digits.len() != 6 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |range: Range<usize>| u8::from_str_radix(&digits[range], 16).ok();
    Some(iced::Color::from_rgb8(
        channel(0..2)?,
        channel(2..4)?,
        channel(4..6)?,
    ))
}

// One paragraph under construction while reading.
#[derive(Debug, Default)]
struct ReadParagraph {
    format: ParagraphFormat,
    // The inline format of the break that ended the previous paragraph, when
    // the markup said what it was: a `<br>` sits inside the tags open around
    // it, while a `</p><p>` junction is not a character and carries nothing.
    break_before: Option<EditFormat>,
    text: String,
    formats: Vec<EditFormat>,
}

// The next paragraph inherits the block format of the one it follows, the same
// rule a typed line break follows in the document, and remembers `at_break`,
// the inline format the break itself was written under.
fn break_paragraph(
    done: &mut Vec<ReadParagraph>,
    open: &mut Option<ReadParagraph>,
    at_break: EditFormat,
) -> Result<(), FromHtmlError> {
    let finished = open
        .take()
        .ok_or_else(|| FromHtmlError::Malformed("a line break outside a paragraph".to_owned()))?;
    let inherited = finished.format;
    done.push(finished);
    *open = Some(ReadParagraph {
        format: inherited,
        break_before: Some(at_break),
        ..ReadParagraph::default()
    });
    Ok(())
}

// Between two `<p>` blocks nothing is open, and an inline started there would
// carry its format into the paragraph that opens next: a scope crossing a block
// boundary, which the emitter never writes.
fn owned_by_open_paragraph(open: Option<&ReadParagraph>, tag: &str) -> Result<(), FromHtmlError> {
    if open.is_none() {
        return Err(FromHtmlError::Malformed(format!(
            "a <{tag}> outside a paragraph"
        )));
    }
    Ok(())
}

fn assemble(paragraphs: Vec<ReadParagraph>, links: Vec<String>) -> RichEditDocument {
    let mut text = String::new();
    let mut formats: Vec<EditFormat> = Vec::new();
    let mut blocks = Vec::with_capacity(paragraphs.len());
    for (index, paragraph) in paragraphs.into_iter().enumerate() {
        if index > 0 {
            text.push('\n');
            // A break between two `<p>` blocks is not written anywhere, so it
            // takes the format of the text on its left, the rule typing
            // follows. It paints nothing either way; what this avoids is an
            // unstyled hole splitting one styled run into two.
            let break_format = paragraph
                .break_before
                .unwrap_or_else(|| formats.last().copied().unwrap_or_default());
            formats.push(break_format);
        }
        text.push_str(&paragraph.text);
        formats.extend_from_slice(&paragraph.formats);
        blocks.push(paragraph.format);
    }
    if blocks.is_empty() {
        blocks.push(ParagraphFormat::default());
    }
    RichEditDocument::from_parts_with_links(text, formats, blocks, links)
}

/// Read a saved document in this module's HTML vocabulary.
///
/// Supported tags and attribute forms preserve authored family, pixel size,
/// emphasis, ink, highlight, links, lists and paragraph alignment. Named
/// families use iced's process-wide name cache and remain retained after parsing.
/// Semantic tones and ink alpha are omitted by the emitter; line endings
/// normalize to `\n`, and paragraph-boundary formatting inherits from its left.
///
/// # Errors
///
/// Returns [`FromHtmlError`] for unsupported tags, CSS or entities, invalid
/// sizes, malformed nesting or incomplete markup. This is not a general HTML sanitizer.
pub fn from_html(html: &str) -> Result<RichEditDocument, FromHtmlError> {
    // The emitter writes either `<p>` blocks or a flat inline run, never both,
    // so the first tag decides which shape is being read.
    let blocks = html.starts_with("<p>")
        || html.starts_with("<p ")
        || html.starts_with("<ul>")
        || html.starts_with("<ol>");
    let mut links: Vec<String> = Vec::new();
    let mut list: Option<(ListKind, usize)> = None;
    let mut done: Vec<ReadParagraph> = Vec::new();
    let mut open: Option<ReadParagraph> = (!blocks).then(ReadParagraph::default);
    let mut stack: Vec<Inline> = Vec::new();
    let mut rest = html;

    while !rest.is_empty() {
        let Some(tail) = rest.strip_prefix('<') else {
            let end = rest.find('<').unwrap_or(rest.len());
            let chunk = unescape(&rest[..end])?;
            rest = &rest[end..];
            // A literal break inside the markup is a paragraph break, decided
            // by the document's own splitter so the reader cannot disagree
            // with the model about where a paragraph ends.
            let bodies = super::document::line_body_ranges(&chunk);
            for (index, body) in bodies.into_iter().enumerate() {
                if index > 0 {
                    break_paragraph(&mut done, &mut open, format_from(&stack))?;
                }
                let piece = &chunk[body];
                let paragraph = open.as_mut().ok_or_else(|| {
                    FromHtmlError::Malformed(format!("text outside a paragraph: {piece}"))
                })?;
                let format = format_from(&stack);
                paragraph
                    .formats
                    .extend(std::iter::repeat_n(format, piece.len()));
                paragraph.text.push_str(piece);
            }
            continue;
        };

        let end = tail
            .find('>')
            .ok_or_else(|| FromHtmlError::Malformed(rest.to_owned()))?;
        let tag = &tail[..end];
        rest = &tail[end + 1..];

        if let Some(closing) = tag.strip_prefix('/') {
            if closing == "ul" || closing == "ol" {
                let kind = if closing == "ul" {
                    ListKind::Bullet
                } else {
                    ListKind::Numbered
                };
                if open.is_some() || list.map(|(open_kind, _)| open_kind) != Some(kind) {
                    return Err(FromHtmlError::MismatchedClose(closing.to_owned()));
                }
                list = None;
            } else if closing == "li" {
                if let Some(inline) = stack.last() {
                    return Err(FromHtmlError::UnclosedTag(inline.tag().to_owned()));
                }
                let paragraph = list
                    .and_then(|_| open.take())
                    .ok_or_else(|| FromHtmlError::MismatchedClose("li".to_owned()))?;
                done.push(paragraph);
            } else if closing == "p" {
                if let Some(inline) = stack.last() {
                    return Err(FromHtmlError::UnclosedTag(inline.tag().to_owned()));
                }
                // Flat markup opens no `<p>`: its one paragraph belongs to the
                // run itself, so there is nothing here for a `</p>` to close.
                let paragraph = blocks
                    .then(|| open.take())
                    .flatten()
                    .ok_or_else(|| FromHtmlError::MismatchedClose("p".to_owned()))?;
                done.push(paragraph);
            } else {
                match stack.pop() {
                    Some(inline) if inline.tag() == closing => {}
                    Some(inline) => {
                        return Err(FromHtmlError::MismatchedClose(format!(
                            "{closing} closes {}",
                            inline.tag()
                        )));
                    }
                    None => return Err(FromHtmlError::MismatchedClose(closing.to_owned())),
                }
            }
        } else if let Some(inline) = Inline::opened_by(tag) {
            owned_by_open_paragraph(open.as_ref(), tag)?;
            stack.push(inline);
        } else if tag == "br" {
            if blocks {
                return Err(FromHtmlError::Malformed(
                    "a <br> inside paragraph blocks".to_owned(),
                ));
            }
            break_paragraph(&mut done, &mut open, format_from(&stack))?;
        } else if tag == "ul" || tag == "ol" {
            if !blocks || open.is_some() || list.is_some() {
                return Err(FromHtmlError::Malformed(format!("a misplaced <{tag}>")));
            }
            let kind = if tag == "ul" {
                ListKind::Bullet
            } else {
                ListKind::Numbered
            };
            list = Some((kind, 0));
        } else if tag == "li" || tag.starts_with("li ") {
            let align = block_align(tag, "li")?;
            let Some((kind, count)) = list.as_mut() else {
                return Err(FromHtmlError::Malformed(
                    "an <li> outside a list".to_owned(),
                ));
            };
            if open.is_some() {
                return Err(FromHtmlError::Malformed("a nested <li>".to_owned()));
            }
            *count += 1;
            let marker = kind.marker(*count);
            open = Some(ReadParagraph {
                format: ParagraphFormat {
                    list: Some(*kind),
                    align,
                },
                formats: vec![EditFormat::default(); marker.len()],
                text: marker,
                ..ReadParagraph::default()
            });
        } else if tag == "p" || tag.starts_with("p ") {
            if !blocks || open.is_some() || list.is_some() {
                return Err(FromHtmlError::Malformed(format!("a misplaced <{tag}>")));
            }
            let align = block_align(tag, "p")?;
            open = Some(ReadParagraph {
                format: ParagraphFormat {
                    align,
                    ..ParagraphFormat::default()
                },
                ..ReadParagraph::default()
            });
        } else if let Some(literal) = tag
            .strip_prefix("span style=\"background-color:")
            .and_then(|value| value.strip_suffix('"'))
        {
            let color =
                parse_hex(literal).ok_or_else(|| FromHtmlError::UnsupportedTag(tag.to_owned()))?;
            owned_by_open_paragraph(open.as_ref(), tag)?;
            stack.push(Inline::Highlight(color));
        } else if let Some(href) = tag
            .strip_prefix("a href=\"")
            .and_then(|value| value.strip_suffix('"'))
        {
            owned_by_open_paragraph(open.as_ref(), tag)?;
            let href = unescape(href)?;
            let id = match links.iter().position(|known| *known == href) {
                Some(index) => index,
                None => {
                    links.push(href);
                    links.len() - 1
                }
            };
            stack.push(Inline::Link(id));
        } else if let Some(literal) = tag
            .strip_prefix("span style=\"font-family:")
            .and_then(|value| value.strip_suffix('"'))
        {
            let family = parse_family(&unescape(literal)?)
                .ok_or_else(|| FromHtmlError::UnsupportedTag(tag.to_owned()))?;
            owned_by_open_paragraph(open.as_ref(), tag)?;
            stack.push(Inline::Font(family));
        } else if let Some(literal) = tag
            .strip_prefix("span style=\"font-size:")
            .and_then(|value| value.strip_suffix("px\""))
        {
            let size = literal
                .parse::<f32>()
                .ok()
                .filter(|size| size.is_finite() && *size > 0.0)
                .ok_or_else(|| FromHtmlError::UnsupportedTag(tag.to_owned()))?;
            owned_by_open_paragraph(open.as_ref(), tag)?;
            stack.push(Inline::TextSize(size));
        } else if let Some(literal) = tag
            .strip_prefix("span style=\"color:")
            .and_then(|value| value.strip_suffix('"'))
        {
            let color =
                parse_hex(literal).ok_or_else(|| FromHtmlError::UnsupportedTag(tag.to_owned()))?;
            owned_by_open_paragraph(open.as_ref(), tag)?;
            stack.push(Inline::Ink(color));
        } else {
            return Err(FromHtmlError::UnsupportedTag(tag.to_owned()));
        }
    }

    if let Some(inline) = stack.last() {
        return Err(FromHtmlError::UnclosedTag(inline.tag().to_owned()));
    }
    if list.is_some() {
        return Err(FromHtmlError::UnclosedTag("ul".to_owned()));
    }
    match open {
        Some(paragraph) if !blocks => done.push(paragraph),
        Some(_) => return Err(FromHtmlError::UnclosedTag("p".to_owned())),
        None => {}
    }
    Ok(assemble(done, links))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_round_trip_preserves_alignment_and_escapes_content() {
        // an_aligned_list_item_keeps_its_alignment_through_html
        {
            let mut doc = RichEditDocument::from_plain("one\ntwo");
            let all = 0..doc.text().len();
            let all = doc.toggle_list(ListKind::Bullet, all);
            doc.set_paragraph_align(ParagraphAlign::Center, all.start..all.start);
            let html = to_html(&doc);
            assert!(html.contains("<li style=\"text-align:center\">"), "{html}");
            let back = from_html(&html).expect("the emitter's own markup reads back");
            assert_eq!(back.paragraph_formats(), doc.paragraph_formats(), "{html}");
            assert_eq!(back.text(), doc.text());
        }

        // html_entities_are_escaped
        {
            let doc = RichEditDocument::from_plain("<script>alert('xss')&\"</script>");
            let html = to_html(&doc);
            assert_eq!(
                html,
                "&lt;script&gt;alert(&#39;xss&#39;)&amp;&quot;&lt;/script&gt;"
            );
        }
    }

    // ---- paragraph alignment: emission and the round trip ----
    //
    // These fixtures compare whole documents, so a reader that dropped an
    // alignment, an inline style, an ink or a paragraph fails on the document
    // it produced rather than on a copied expectation table.
}

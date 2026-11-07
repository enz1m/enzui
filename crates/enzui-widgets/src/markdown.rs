//! EnzMarkdown: a token-aware Markdown document renderer.
//!
//! The rendered-document surface for release notes, help text, and chat
//! replies. It leans on `pulldown-cmark`, the same CommonMark parser iced's own
//! `markdown::Content` uses (`iced/widget/Cargo.toml`), and turns its event stream into an
//! immutable document model rendered through this crate's tokens and widgets.
//!
//! # Why not iced's `markdown::view`
//!
//! iced ships a `markdown` widget, but its `Text::spans` only hands back
//! already-styled iced `Span`s keyed to iced's own `markdown::Style`
//! (`iced/widget/src/markdown.rs`); the raw bold / italic / link
//! structure is a private inner enum. There is no way to recover the
//! [`SpanStyle`]/[`TextTone`](crate::enz_text::TextTone) model from it, so a
//! mechanical `markdown::view` call would re-introduce a second typography
//! and colour system — exactly the fork exists to prevent. Instead it
//! consumes the parser directly and builds a [`RichText`], which every other
//! enzUI prose widget already renders. A markdown heading is therefore
//! indistinguishable from a hand-built [`EnzReadingPane`] heading: both call
//! [`EnzReadingPane::heading_size`] and [`EnzReadingPane::heading_weight`].
//!
//! # Scope (this slice)
//!
//! Headings, prose paragraphs, emphasis / strong / inline-code / strikethrough
//! spans, links, bullet and ordered lists (with nesting and numbering), block
//! quotes, thematic breaks (`---`), fenced code blocks, images and GFM tables
//! — all rendered through enzUI tokens and the shared
//! [`rich_text`](crate::rich_text) / [`code_block`](crate::code_block) /
//! [`image`](crate::image) primitives.
//!
//! Images are resolved by the caller
//! ([`image_resolver`](EnzMarkdown::image_resolver)); the framework fetches
//! nothing, and an unresolved image shows its alt text. Tables keep their
//! column alignment and publish as a grid with column headers, bounded by
//! [`MAX_TABLE_COLUMNS`] and [`MAX_TABLE_ROWS`]. A GFM task item
//! (`- [ ]`, `- [x]`) shows a checkbox named by the item's text; with
//! [`on_task_toggled`](EnzMarkdown::on_task_toggled) the reader can tick it,
//! and the report carries the marker's byte range so the caller rewrites its
//! own source ([`TaskToggle::apply`]).
//!
//! Deferred to their own slices: footnotes and the long-document performance
//! gate.
//!
//! # Links carry typed destinations
//!
//! Primary and keyboard activation pass the destination URL, not the visible
//! label, to [`on_link_clicked`](EnzMarkdown::on_link_clicked). Secondary presses
//! pass that URL and a window-space point to
//! [`on_link_context`](EnzMarkdown::on_link_context). Each rendered link is a
//! keyboard stop and exposes a `Role::Link` accessibility node.

use iced::widget::{Space, column, container, text};
use iced::{Element, Length};
use std::collections::HashSet;
use std::rc::Rc;

use crate::code_block::enz_code_block;
use crate::enz_text::TextTokens;
use crate::reading_pane::EnzReadingPane;
use crate::rich_text::{LinkHandlers, RichSpan, RichText, SpanStyle, ThemedTokens};

/// The Markdown link payload: the destination URL exactly as authored.
pub type MdUrl = String;

/// Maximum blockquote / list nesting depth `enz_markdown` will build during
/// parsing and descend into during rendering.
///
/// A hostile Markdown payload can nest `>` or list levels arbitrarily; both
/// building the tree and repainting it traverse Quote / List blocks, so an
/// unbounded document would consume unbounded work. This single ceiling is
/// enforced twice — once at parse time so the tree is bounded, and once again
/// inside the iterative render adapter so a caller-constructed
/// [`enz_markdown_blocks`] tree still cannot descend past it — with no second
/// divergent limit. Content strictly deeper than this bound is preserved as
/// flat blocks introduced by an accessible [`EnzMarkdownBlock::Truncated`]
/// marker, never quietly dropped.
///
/// `64` matches the effective Markdown / CommonMark nesting cap other
/// consumer parsers use (`cmark-gfm`'s `CMARK_OPT_UNSAFE`-independent
/// `document->options->nesting_max`, pandoc's `--maximum-nesting-level`), and
/// is far above what an authored document reaches: the help / release-notes
/// fixtures in the apps top out at depth 3.
pub const MAX_NESTING_DEPTH: u16 = 64;

/// The most columns a table keeps.
pub const MAX_TABLE_COLUMNS: usize = 32;

/// The most body rows a table keeps.
pub const MAX_TABLE_ROWS: usize = 2048;

/// A GFM task list item's box (`- [ ]` or `- [x]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskMarker {
    /// Whether the box is ticked.
    pub checked: bool,
    /// The byte range of the marker (`[ ]` / `[x]`) in the parsed source, so
    /// a caller can rewrite it when the reader ticks the box.
    pub source: std::ops::Range<usize>,
}

/// The reader ticked or cleared a task box. Delivered by
/// [`EnzMarkdown::on_task_toggled`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskToggle {
    /// The marker's byte range in the source the document was parsed from.
    pub source: std::ops::Range<usize>,
    /// The state the reader asked for.
    pub checked: bool,
}

impl TaskToggle {
    /// The source text with this toggle applied, or `None` when `source` no
    /// longer holds a task marker at that range (it changed since parsing).
    #[must_use]
    pub fn apply(&self, source: &str) -> Option<String> {
        let marker = source.get(self.source.clone())?;
        if !matches!(marker, "[ ]" | "[x]" | "[X]") {
            return None;
        }
        let mut out = String::with_capacity(source.len());
        out.push_str(source.get(..self.source.start).unwrap_or_default());
        out.push_str(if self.checked { "[x]" } else { "[ ]" });
        out.push_str(source.get(self.source.end..).unwrap_or_default());
        Some(out)
    }
}

/// A table column's alignment, from its delimiter row (`:--`, `:-:`, `--:`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColumnAlign {
    /// No colon: the text's natural start.
    #[default]
    Default,
    /// `:--`
    Left,
    /// `:-:`
    Center,
    /// `--:`
    Right,
}

/// One rendered Markdown block.
///
/// This is the immutable document model between the `pulldown-cmark` event
/// stream and the enzUI widgets. Inline content is a [`RichText<MdUrl>`] so a
/// heading, paragraph, quote, or list item renders through the shared
/// [`rich_text`](crate::rich_text) adapter with enzUI tones and link semantics.
///
/// # Alpha exhaustiveness and destruction
///
/// This model is an alpha extension point and is deliberately
/// `#[non_exhaustive]`: downstream matches must include a wildcard arm. New
/// block or safety-marker variants may be added without another source break.
/// Caller-built [`Quote`](Self::Quote) / [`List`](Self::List) trees may be much
/// deeper than [`MAX_NESTING_DEPTH`]. Rendering still clips them at that shared
/// parser/render bound, while destruction drains all nested ownership
/// iteratively so even an over-cap tree can be dropped on a GUI thread's small
/// stack without leaking its children.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum EnzMarkdownBlock {
    /// A heading of the given level (1-6) with inline runs.
    Heading {
        /// Heading level, 1 (largest) through 6.
        level: u8,
        /// The heading's inline content.
        inline: RichText<MdUrl>,
    },
    /// A prose paragraph with inline runs.
    Paragraph(RichText<MdUrl>),
    /// A block quote: a nested sequence of blocks.
    Quote(Vec<EnzMarkdownBlock>),
    /// A list. `start` is `Some(n)` for an ordered list beginning at `n`,
    /// `None` for a bullet list. Each entry is one item's block sequence.
    List {
        /// The first ordinal of an ordered list, or `None` for a bullet list.
        start: Option<u64>,
        /// One block sequence per list item.
        items: Vec<Vec<EnzMarkdownBlock>>,
        /// One entry per item: its GFM task marker (`- [ ]`, `- [x]`), if it
        /// has one. Shorter than `items` only for a hand-built list.
        tasks: Vec<Option<TaskMarker>>,
    },
    /// A fenced or indented code block, with an optional language tag.
    CodeBlock {
        /// The fence's info string (language), if any.
        language: Option<String>,
        /// The raw code, trailing newline trimmed.
        code: String,
    },
    /// A thematic break (`---`).
    Rule,
    /// An image (`![alt](url "title")`). The framework fetches nothing: an
    /// [`image_resolver`](EnzMarkdown::image_resolver) turns the URL into a
    /// handle, and without one (or when it declines) the alt text is shown in
    /// the image's place, so the content is never silently lost. An image in
    /// the middle of a paragraph becomes its own block there.
    Image {
        /// The destination exactly as authored.
        url: MdUrl,
        /// The alt text, which also names the image for assistive technology.
        alt: String,
        /// The title, if any.
        title: Option<String>,
    },
    /// A GFM table: a header row, body rows, and one alignment per column.
    /// Rows shorter than the header are padded; cells beyond
    /// [`MAX_TABLE_COLUMNS`] or rows beyond [`MAX_TABLE_ROWS`] are left out,
    /// and a [`Truncated`](Self::Truncated)-style note follows the table.
    Table {
        /// One alignment per column.
        alignments: Vec<ColumnAlign>,
        /// The header row's cells.
        header: Vec<RichText<MdUrl>>,
        /// The body rows' cells.
        rows: Vec<Vec<RichText<MdUrl>>>,
    },
    /// A truncation marker.
    ///
    /// Placed by [`parse`] at the point where a Quote / List Start event
    /// would have taken the tree deeper than [`MAX_NESTING_DEPTH`], and by
    /// the render adapter at the same bound when it declines to descend
    /// further into a manually constructed deep tree. The renderer paints
    /// this as a muted italic paragraph reading `"… nested content truncated
    /// at N levels"`, so a reader and an assistive-tech client see the
    /// truncation instead of a silent drop.
    ///
    /// Content past the parser boundary flows into the same scope after
    /// this marker rather than being nested further, so no visible text is
    /// lost — the flattening is honest.
    Truncated {
        /// Depth at which truncation happened, in Quote / List nesting
        /// levels below the document root. Equals [`MAX_NESTING_DEPTH`] for
        /// parser output; matches the renderer's actual descent for a
        /// manually-constructed tree.
        depth: u16,
    },
}

impl EnzMarkdownBlock {
    // Move this node's direct recursive children into `pending`.
    //
    // Emptying every child container before the node receives its
    // ordinary field drop is the key invariant behind the iterative
    // [`Drop`] impl.
    fn take_children(&mut self, pending: &mut Vec<Self>) {
        match self {
            Self::Quote(blocks) => pending.append(blocks),
            Self::List { items, .. } => {
                for item in items {
                    pending.append(item);
                }
            }
            Self::Heading { .. }
            | Self::Paragraph(_)
            | Self::CodeBlock { .. }
            | Self::Rule
            | Self::Image { .. }
            | Self::Table { .. }
            | Self::Truncated { .. } => {}
        }
    }
}

impl Drop for EnzMarkdownBlock {
    fn drop(&mut self) {
        // Rust's derived field destruction recursively follows Quote/List
        // ownership. Detach every descendant onto a heap-backed work stack
        // first. When each local `block` is dropped below, its recursive
        // containers are already empty, so its re-entered `Drop` is shallow;
        // ordinary fields (RichText/String) still receive their normal drop.
        let mut pending = Vec::new();
        self.take_children(&mut pending);

        while let Some(mut block) = pending.pop() {
            block.take_children(&mut pending);
        }
    }
}

/// Parse a Markdown source string into the enzUI block model.
///
/// The parser is deterministic and never panics: malformed input (unclosed
/// fences, a heading with no text, an empty link target, `#######`) produces the
/// closest well-formed blocks rather than an error. An empty document yields an
/// empty `Vec`.
#[must_use]
pub fn parse(source: &str) -> Vec<EnzMarkdownBlock> {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

    // The inline extensions iced enables (strikethrough), plus GFM tables and
    // task lists.
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_TASKLISTS);

    // A container we are currently filling. Blocks nest (quotes, list items),
    // so we keep a stack of open scopes and fold a completed scope into its
    // parent when its End event arrives.
    enum Scope {
        /// Top-level document, or the body of a quote / list item.
        Blocks(Vec<EnzMarkdownBlock>),
        /// An open list: `start`, and the item bodies completed so far.
        List {
            start: Option<u64>,
            items: Vec<Vec<EnzMarkdownBlock>>,
            tasks: Vec<Option<TaskMarker>>,
        },
    }

    // kinds of Quote / List / Item Start events that were suppressed
    // because they would have taken the parser stack deeper than
    // `MAX_NESTING_DEPTH`. Suppression shadows pulldown-cmark's Start/End
    // pairs so End events past the cap still balance without touching the
    // real `stack`.
    #[derive(Copy, Clone)]
    enum Suppressed {
        Quote,
        List,
        Item,
    }
    let mut suppressed: Vec<Suppressed> = Vec::new();
    // The document frame plus MAX_NESTING_DEPTH real Quote / List frames.
    // A `stack.len()` at this value means the next Quote / List Start would
    // exceed the cap; we suppress it and emit one Truncated marker.
    const MAX_STACK_LEN: usize = MAX_NESTING_DEPTH as usize + 1;

    let mut stack: Vec<Scope> = vec![Scope::Blocks(Vec::new())];
    // Inline runs accumulating for the current paragraph/heading. `None` when
    // no inline block is open.
    let mut runs: Vec<RichSpan<MdUrl>> = Vec::new();
    // Inline style accumulator, toggled by Start/End(Emphasis|Strong|...) events
    // that bracket text runs.
    let mut inline = InlineFlags::default();
    let mut link: Option<MdUrl> = None;
    // Depth of open `Heading`/`Paragraph` inline block, and its heading level.
    let mut heading_level: Option<u8> = None;
    let mut in_paragraph = false;
    // Fenced code capture.
    let mut code_lang: Option<String> = None;
    let mut code_buf: Option<String> = None;
    // An open image: its URL and title, and the alt text gathering from the
    // Text events inside it.
    let mut image: Option<(MdUrl, Option<String>, String)> = None;
    // An open table.
    struct TableState {
        alignments: Vec<ColumnAlign>,
        header: Vec<RichText<MdUrl>>,
        rows: Vec<Vec<RichText<MdUrl>>>,
        row: Vec<RichText<MdUrl>>,
        dropped: bool,
    }
    let mut table: Option<TableState> = None;

    // Push a completed block into the innermost open block sequence. A `List`
    // scope has no direct block sink (blocks belong to its current item), so
    // this walks to the nearest `Blocks`.
    fn push_block(stack: &mut [Scope], block: EnzMarkdownBlock) {
        for scope in stack.iter_mut().rev() {
            match scope {
                Scope::Blocks(v) => {
                    v.push(block);
                    return;
                }
                Scope::List { items, .. } => {
                    if let Some(item) = items.last_mut() {
                        item.push(block);
                        return;
                    }
                }
            }
        }
        // Unreachable in practice: the bottom of the stack is always a
        // `Blocks`. If we ever fall through, drop the block rather than panic.
    }

    let flush_inline = |runs: &mut Vec<RichSpan<MdUrl>>| -> RichText<MdUrl> {
        RichText::new(std::mem::take(runs)).coalesced()
    };
    // A tight list item's text has no Paragraph of its own; its runs wait for
    // End(Item). A nested list, quote or code block inside that item starts
    // first, so the waiting text is flushed into its own paragraph here —
    // otherwise the nested item's text joined it on one line.
    fn flush_pending_runs(stack: &mut [Scope], runs: &mut Vec<RichSpan<MdUrl>>) {
        if runs.is_empty() {
            return;
        }
        let inline = RichText::new(std::mem::take(runs)).coalesced();
        if !inline.is_empty() {
            push_block(stack, EnzMarkdownBlock::Paragraph(inline));
        }
    }

    for (event, range) in Parser::new_ext(source, opts).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                heading_level = Some(level as u8);
                runs.clear();
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some(level) = heading_level.take() {
                    let inline = flush_inline(&mut runs);
                    push_block(&mut stack, EnzMarkdownBlock::Heading { level, inline });
                }
            }
            Event::Start(Tag::Paragraph) => {
                in_paragraph = true;
                runs.clear();
            }
            Event::End(TagEnd::Paragraph) => {
                let handles_event = in_paragraph;
                if handles_event {
                    in_paragraph = false;
                    let inline = flush_inline(&mut runs);
                    // A paragraph consisting only of stripped inline HTML can be
                    // empty; skip it rather than emit a blank block.
                    if !inline.is_empty() {
                        push_block(&mut stack, EnzMarkdownBlock::Paragraph(inline));
                    }
                }
            }
            Event::Start(Tag::BlockQuote(_)) => {
                let handles_event = !in_paragraph && heading_level.is_none();
                if handles_event {
                    flush_pending_runs(&mut stack, &mut runs);
                }
                if !suppressed.is_empty() || stack.len() >= MAX_STACK_LEN {
                    if suppressed.is_empty() {
                        // First scope past the cap — emit a single marker into the
                        // deepest legitimate scope so the truncation is
                        // visible / announced. Deeper Starts add no more
                        // markers; further Text events flatten into the same
                        // scope after this marker (honest flattening ).
                        push_block(
                            &mut stack,
                            EnzMarkdownBlock::Truncated {
                                depth: MAX_NESTING_DEPTH,
                            },
                        );
                    }
                    suppressed.push(Suppressed::Quote);
                } else {
                    stack.push(Scope::Blocks(Vec::new()));
                }
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                if matches!(suppressed.last(), Some(Suppressed::Quote)) {
                    suppressed.pop();
                } else if let Some(Scope::Blocks(blocks)) = stack.pop() {
                    push_block(&mut stack, EnzMarkdownBlock::Quote(blocks));
                }
            }
            Event::Start(Tag::List(start)) => {
                if !in_paragraph && heading_level.is_none() {
                    flush_pending_runs(&mut stack, &mut runs);
                }
                if !suppressed.is_empty() || stack.len() >= MAX_STACK_LEN {
                    if suppressed.is_empty() {
                        push_block(
                            &mut stack,
                            EnzMarkdownBlock::Truncated {
                                depth: MAX_NESTING_DEPTH,
                            },
                        );
                    }
                    suppressed.push(Suppressed::List);
                } else {
                    stack.push(Scope::List {
                        start,
                        items: Vec::new(),
                        tasks: Vec::new(),
                    });
                }
            }
            Event::End(TagEnd::List(_)) => {
                if matches!(suppressed.last(), Some(Suppressed::List)) {
                    suppressed.pop();
                } else if let Some(Scope::List {
                    start,
                    items,
                    tasks,
                }) = stack.pop()
                {
                    push_block(
                        &mut stack,
                        EnzMarkdownBlock::List {
                            start,
                            items,
                            tasks,
                        },
                    );
                }
            }
            Event::Start(Tag::Item) => {
                if !suppressed.is_empty() {
                    // An Item nested inside a suppressed List (or Quote): the
                    // enclosing container was not built, so an item cannot be
                    // pushed into it. Suppress the Item too; its inline text
                    // still flushes into the deepest legit scope through the
                    // ordinary run stream.
                    suppressed.push(Suppressed::Item);
                } else if let Some(Scope::List { items, tasks, .. }) = stack.last_mut() {
                    items.push(Vec::new());
                    tasks.push(None);
                }
            }
            Event::TaskListMarker(checked) => {
                // The marker belongs to the item just opened in the nearest
                // list; a suppressed item has no box to give it.
                if suppressed.is_empty()
                    && let Some(Scope::List { tasks, .. }) = stack
                        .iter_mut()
                        .rev()
                        .find(|scope| matches!(scope, Scope::List { .. }))
                    && let Some(slot) = tasks.last_mut()
                {
                    *slot = Some(TaskMarker {
                        checked,
                        source: range.clone(),
                    });
                }
            }
            Event::End(TagEnd::Item) => {
                if matches!(suppressed.last(), Some(Suppressed::Item)) {
                    suppressed.pop();
                } else if !runs.is_empty() {
                    // A "tight" list item is inline text with no wrapping Paragraph
                    // event, so its runs never flushed. Flush them into a paragraph
                    // now if any remain.
                    let inline = flush_inline(&mut runs);
                    if !inline.is_empty() {
                        push_block(&mut stack, EnzMarkdownBlock::Paragraph(inline));
                    }
                }
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                if !in_paragraph && heading_level.is_none() {
                    flush_pending_runs(&mut stack, &mut runs);
                }
                code_lang = match kind {
                    pulldown_cmark::CodeBlockKind::Fenced(lang) => {
                        let lang = lang.into_string();
                        // Only the first token of the info string is a language.
                        let lang = lang.split_whitespace().next().unwrap_or("").to_owned();
                        if lang.is_empty() { None } else { Some(lang) }
                    }
                    pulldown_cmark::CodeBlockKind::Indented => None,
                };
                code_buf = Some(String::new());
            }
            Event::End(TagEnd::CodeBlock) => {
                let mut code = code_buf.take().unwrap_or_default();
                // pulldown emits a trailing newline after the last line; trim
                // exactly one so the block does not render a blank final row.
                if code.ends_with('\n') {
                    code.pop();
                }
                push_block(
                    &mut stack,
                    EnzMarkdownBlock::CodeBlock {
                        language: code_lang.take(),
                        code,
                    },
                );
            }
            Event::Start(Tag::Image {
                dest_url, title, ..
            }) => {
                // The image becomes a block of its own; text before it in the
                // same paragraph is flushed first so the order is kept.
                if table.is_none() && heading_level.is_none() {
                    flush_pending_runs(&mut stack, &mut runs);
                }
                let title = title.into_string();
                image = Some((
                    dest_url.into_string(),
                    (!title.is_empty()).then_some(title),
                    String::new(),
                ));
            }
            Event::End(TagEnd::Image) => {
                if let Some((url, title, alt)) = image.take() {
                    if table.is_some() || heading_level.is_some() {
                        // A table cell or heading holds inline content only:
                        // the image reads as its alt text there.
                        push_run(&mut runs, &alt, inline, link.as_ref(), false);
                    } else {
                        push_block(&mut stack, EnzMarkdownBlock::Image { url, alt, title });
                    }
                }
            }
            Event::Start(Tag::Table(alignments)) => {
                flush_pending_runs(&mut stack, &mut runs);
                table = Some(TableState {
                    alignments: alignments
                        .iter()
                        .take(MAX_TABLE_COLUMNS)
                        .map(|a| match a {
                            pulldown_cmark::Alignment::None => ColumnAlign::Default,
                            pulldown_cmark::Alignment::Left => ColumnAlign::Left,
                            pulldown_cmark::Alignment::Center => ColumnAlign::Center,
                            pulldown_cmark::Alignment::Right => ColumnAlign::Right,
                        })
                        .collect(),
                    header: Vec::new(),
                    rows: Vec::new(),
                    row: Vec::new(),
                    dropped: alignments.len() > MAX_TABLE_COLUMNS,
                });
            }
            Event::Start(Tag::TableHead | Tag::TableRow) => {
                if let Some(table) = table.as_mut() {
                    table.row.clear();
                }
            }
            Event::Start(Tag::TableCell) => runs.clear(),
            Event::End(TagEnd::TableCell) => {
                let cell = flush_inline(&mut runs);
                if let Some(table) = table.as_mut() {
                    if table.row.len() < table.alignments.len() {
                        table.row.push(cell);
                    } else {
                        table.dropped = true;
                    }
                }
            }
            Event::End(TagEnd::TableHead) => {
                if let Some(table) = table.as_mut() {
                    table.header = std::mem::take(&mut table.row);
                }
            }
            Event::End(TagEnd::TableRow) => {
                if let Some(table) = table.as_mut() {
                    let mut row = std::mem::take(&mut table.row);
                    if table.rows.len() < MAX_TABLE_ROWS {
                        row.resize_with(table.alignments.len(), RichText::default);
                        table.rows.push(row);
                    } else {
                        table.dropped = true;
                    }
                }
            }
            Event::End(TagEnd::Table) => {
                if let Some(mut done) = table.take() {
                    let columns = done.alignments.len();
                    done.header.resize_with(columns, RichText::default);
                    push_block(
                        &mut stack,
                        EnzMarkdownBlock::Table {
                            alignments: done.alignments,
                            header: done.header,
                            rows: done.rows,
                        },
                    );
                    if done.dropped {
                        push_block(
                            &mut stack,
                            EnzMarkdownBlock::Paragraph(RichText::new(vec![RichSpan::styled(
                                enzui_core::localized!("enzui-markdown-table-truncated", "columns" => MAX_TABLE_COLUMNS, "rows" => MAX_TABLE_ROWS),
                                SpanStyle::default(),
                            )])),
                        );
                    }
                }
            }
            Event::Start(Tag::Emphasis) => inline.emphasis = true,
            Event::End(TagEnd::Emphasis) => inline.emphasis = false,
            Event::Start(Tag::Strong) => inline.strong = true,
            Event::End(TagEnd::Strong) => inline.strong = false,
            Event::Start(Tag::Strikethrough) => inline.strikethrough = true,
            Event::End(TagEnd::Strikethrough) => inline.strikethrough = false,
            // A `#` at the wrong place in a heading, or extra hashes, are handled
            // by pulldown itself; `#######` never emits a Heading Start.
            Event::Start(Tag::Link { dest_url, .. }) => {
                // Allow-list the scheme before the URL ever reaches
                // `on_link_clicked`. A rejected URL becomes `None`, so
                // its label text still renders as plain prose — the content is
                // not lost, but nothing is clickable and the caller never
                // receives a `javascript:`/`data:`/`file:` payload to hand to
                // `xdg-open`.
                link = sanitize_link_url(&dest_url);
            }
            Event::End(TagEnd::Link) => {
                link = None;
            }
            Event::Text(t) => {
                if let Some(buf) = code_buf.as_mut() {
                    buf.push_str(&t);
                } else if let Some((_, _, alt)) = image.as_mut() {
                    alt.push_str(&t);
                } else {
                    // Accumulate into the current run stream. A heading/paragraph
                    // is open, or a "tight" list item's bare text (no wrapping
                    // Paragraph event) which `End(Item)` flushes.
                    push_run(&mut runs, &t, inline, link.as_ref(), false);
                }
            }
            Event::Code(t) => {
                if let Some((_, _, alt)) = image.as_mut() {
                    alt.push_str(&t);
                } else {
                    // Inline code span: monospace, regardless of surrounding style.
                    push_run(&mut runs, &t, inline, link.as_ref(), true);
                }
            }
            Event::SoftBreak => {
                // A newline in the source that the author did not mean as a
                // line break. CommonMark re-wraps it, so it renders as a single
                // space and wrapping is the layout's job.
                push_run(&mut runs, " ", inline, link.as_ref(), false);
            }
            Event::HardBreak => {
                // The author explicitly demanded a line break (two
                // trailing spaces, or a backslash). Collapsing it to a
                // space — as sharing SoftBreak's branch would — quietly
                // discards that intent, and it is exactly the construct
                // used for
                // addresses and verse, where the break carries the meaning
                //
                //
                // # Why a `\n` inside the run, and not a break marker
                //
                // The run stream is flat text, so the break has to be expressed
                // either as a character or as a structural marker that splits
                // the paragraph. `\n` is correct here, and both halves of that
                // were verified rather than assumed:
                //
                // * Rendered. The text reaches the shaper verbatim —
                //   `RichText::to_iced_spans` copies `s.text` into
                //   `Span::new` without touching it, and cosmic-text's
                //   `set_rich_text` splits the concatenated span text
                //   into `BufferLine`s with `BidiParagraphs`
                //   (`cosmic-text-0.19.0/src/buffer.rs`), so an
                //   embedded `\n` starts a new line. Measured: one run
                //   of `"line one line two"` lays out 20.8pt tall, and
                //   the same text with a `\n` lays out 41.6pt — exactly
                //   two lines.
                // * Announced. `collect.rs` drops only whitespace-only
                //   text and performs no whitespace normalisation, so a
                //   `\n` survives into the accessible name verbatim.
                //   `code_block` already relies on this: its body
                //   publishes multi-line code as one label and
                //   `code_body_is_published_verbatim_as_a_label` asserts
                //   the newlines byte-for-byte.
                //
                // A structural break marker would have needed both the renderer
                // and the semantics path taught about it separately, and a
                // marker only one of them understood would render correctly
                // while announcing as nothing (or the reverse) — the same defect
                // moved rather than fixed.
                push_run(&mut runs, "\n", inline, link.as_ref(), false);
            }
            Event::Rule => {
                push_block(&mut stack, EnzMarkdownBlock::Rule);
            }
            // Ignored: footnotes, HTML, math. Their text content (where any)
            // still flows through the active run stream above via
            // `Event::Text`.
            _ => {}
        }
    }

    // Close any still-open scope (unclosed quote/list) by folding it out.
    while stack.len() > 1 {
        match stack.pop() {
            Some(Scope::Blocks(blocks)) => {
                push_block(&mut stack, EnzMarkdownBlock::Quote(blocks));
            }
            Some(Scope::List {
                start,
                items,
                tasks,
            }) => {
                push_block(
                    &mut stack,
                    EnzMarkdownBlock::List {
                        start,
                        items,
                        tasks,
                    },
                );
            }
            None => break,
        }
    }

    match stack.pop() {
        Some(Scope::Blocks(blocks)) => blocks,
        _ => Vec::new(),
    }
}

// Scheme allow-list for a Markdown link destination.
//
// The widget itself cannot control what the caller does with the URL it hands
// to [`on_link_clicked`](EnzMarkdown::on_link_clicked) — the obvious thing for
// An application to do is pass it to `xdg-open` or a system browser, so a
// `javascript:alert(1)` or `data:text/html,…` destination is a live injection
// primitive baited by the callback. Raw HTML is already dropped deliberately
// (the `_ => {}` catch-all swallows `Event::Html`); URL schemes were the one
// unguarded path, and this closes it.
//
// This is an allow-list, not a deny-list, and that is deliberate: a
// deny-list of `javascript:`/`data:` is defeated by the next scheme and by the
// evasions HTML parsers historically permit (mixed case, leading whitespace,
// tabs/newlines/NULs embedded inside the scheme, percent-encoding). The
// allow-list assumes the code has not enumerated every escape — anything
// that is not provably one of the permitted forms is rejected. is the local
// cautionary tale: `portal::policy` was inverted deny→allow for exactly this
// reason, and the first allow-list attempt still shipped two escape classes.
//
// # What is permitted
//
// - Absolute URLs with scheme `http`, `https`, or `mailto`
//   (ASCII-case-insensitive per RFC 3986).
// - Relative references with no scheme at all (`./a/b`, `/abs`,
//   `page.md?x=1`), which cannot name an executable scheme.
// - Pure `#fragment` links, including a bare `#`.
//
// # What a rejected link renders as
//
// `None` — the link's visible label text is preserved as plain prose with
// no link attached (the caller receives no payload for it). The content
// is not lost and nothing is clickable. This is the pinned decision
// (asserted in `markdown/tests.rs`); an empty span or a silent drop
// would be wrong.
//
// # How the scheme is detected past evasions
//
// A URL has a scheme iff a `:` appears before any `/`, `?`, or `#`, with the
// characters before that `:` forming a syntactically valid RFC 3986 scheme
// token (`ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )`). To defeat the
// control-character evasions, all ASCII whitespace and C0 control bytes
// (including NUL, TAB, LF, CR) are stripped from the whole string first —
// this is what HTML/URL parsers do, so `java\tscript:` and `java\0script:`
// normalise back to `javascript:` and are then rejected by the allow-list.
// Percent-encoded scheme bytes (`%6a…`) contain `%`, which is not a valid
// scheme character, so the candidate fails the scheme-token test and is
// treated as having no scheme; but such a string still contains a `:` before
// any `/?#`, which a lenient consumer could read as an opaque scheme, so a
// leading colon-terminated token that is not a valid relative-path
// segment is rejected rather than passed as "relative".
#[must_use]
fn sanitize_link_url(raw: &str) -> Option<String> {
    // 1. Strip ASCII whitespace and all C0 control bytes (0x00..=0x1F) and DEL
    //    (0x7F) from the entire string. This is the normalisation a browser's
    //    URL parser performs, and it is what turns `java\nscript:` back into
    //    `javascript:` so the allow-list sees the real scheme.
    let stripped: String = raw
        .chars()
        .filter(|c| !c.is_ascii_whitespace() && !c.is_ascii_control())
        .collect();

    // 2. An empty destination (`[x]()`, or a string that was pure whitespace)
    //  is expressible intent, not a crash: it names no scheme, so
    //    it is a (degenerate) relative reference and is permitted verbatim.
    //    We return the ORIGINAL `raw` for permitted URLs so the payload the
    //    caller receives is exactly what was authored.
    if stripped.is_empty() {
        return Some(raw.to_owned());
    }

    // 3. A pure fragment link (`#`, `#section`) has no scheme and is safe.
    if stripped.starts_with('#') {
        return Some(raw.to_owned());
    }

    // 4. Find the scheme delimiter: the first `:` that is not preceded by a
    //    path/query/fragment separator. If a `/`, `?`, or `#` appears before
    //    any `:`, there is no scheme — it is a relative reference and safe.
    match stripped.find([':', '/', '?', '#']) {
        // A `:` comes first: there is a candidate scheme.
        Some(idx) if stripped.as_bytes()[idx] == b':' => {
            let (scheme, _) = stripped.split_at(idx);
            if is_valid_scheme_token(scheme)
                && (scheme.eq_ignore_ascii_case("http")
                    || scheme.eq_ignore_ascii_case("https")
                    || scheme.eq_ignore_ascii_case("mailto"))
            {
                // Permitted absolute URL. Hand back the original text.
                Some(raw.to_owned())
            } else {
                // Either a disallowed scheme (`javascript`, `data`, `file`,
                // `ftp`, …) or a colon-terminated token that is not even a
                // valid scheme (percent-encoded, `//`-relative confusion). A
                // bare token followed by `:` that a lenient consumer might read
                // as a scheme is rejected — fail closed.
                None
            }
        }
        // A `/`, `?`, or `#` comes before any `:`: no scheme, relative ref.
        // This also covers scheme-relative `//evil.com` — which is a
        // relative reference (it inherits the document's scheme), and
        // since this widget
        // has no base document, it cannot resolve to an executable scheme; the
        // payload the caller gets is the literal `//evil.com`, never a
        // `javascript:`.
        Some(_) => Some(raw.to_owned()),
        // No `:`, `/`, `?`, or `#` at all: a bare relative segment like
        // `page.md`. Safe.
        None => Some(raw.to_owned()),
    }
}

// Whether `s` is a syntactically valid RFC 3986 scheme token:
// `ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )`. Empty is not valid.
//
// This is what distinguishes a genuine scheme (`https`) from a
// percent-encoded or otherwise mangled prefix (`%6a%61%76`, `//evil`),
// which must not be treated as "relative and therefore safe".
fn is_valid_scheme_token(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

// The inline-style flags accumulated while walking a run of inline
// events.
#[derive(Clone, Copy, Default)]
struct InlineFlags {
    strong: bool,
    emphasis: bool,
    strikethrough: bool,
    code: bool,
}

// Append `text` to `runs` as a run carrying the current inline
// style/link.
fn push_run(
    runs: &mut Vec<RichSpan<MdUrl>>,
    text: &str,
    inline: InlineFlags,
    link: Option<&MdUrl>,
    code: bool,
) {
    if text.is_empty() {
        return;
    }
    let style = SpanStyle {
        bold: inline.strong,
        italic: inline.emphasis,
        code: code || inline.code,
        strikethrough: inline.strikethrough,
        ..SpanStyle::default()
    };
    if let Some(url) = link {
        runs.push(RichSpan {
            text: text.to_owned(),
            style,
            link: Some(url.clone()),
        });
    } else {
        runs.push(RichSpan::styled(text, style));
    }
}

// Default rendering metrics shared with [`EnzReadingPane`] so a
// paragraph or heading is metric-identical to the hand-built
// reading-pane equivalent.
const BODY_SIZE: f32 = 14.0;
const LINE_HEIGHT_MULTIPLE: f32 = 1.6;
// Vertical gap between top-level blocks, matching the reading pane's
// default.
const BLOCK_SPACING: f32 = 12.0;
// Horizontal indent applied per nesting level for quotes and lists.
const INDENT_STEP: f32 = 20.0;

/// Turns an image's URL into a handle, or declines (`None`).
type ImageResolver<'a> = Rc<dyn Fn(&MdUrl) -> Option<iced::widget::image::Handle> + 'a>;

/// A rendered Markdown document. Build with [`enz_markdown`].
pub struct EnzMarkdown<'a, Message> {
    blocks: Vec<EnzMarkdownBlock>,
    link_presentation: Option<crate::rich_text::LinkPresentationTokens>,
    body_size: f32,
    heading_scale: Option<f32>,
    image_resolver: Option<ImageResolver<'a>>,
    line_height_multiple: f32,
    block_spacing: f32,
    on_link_clicked: Option<Rc<dyn Fn(MdUrl) -> Message + 'a>>,
    on_link_context: Option<Rc<dyn Fn(MdUrl, iced::Point) -> Message + 'a>>,
    on_task_toggled: Option<TaskHandler<'a, Message>>,
}

/// Construct an [`EnzMarkdown`] by parsing `source`.
#[must_use]
pub fn enz_markdown<'a, Message>(source: &str) -> EnzMarkdown<'a, Message> {
    EnzMarkdown {
        blocks: parse(source),
        on_link_context: None,
        link_presentation: None,
        body_size: BODY_SIZE,
        heading_scale: None,
        image_resolver: None,
        line_height_multiple: LINE_HEIGHT_MULTIPLE,
        block_spacing: BLOCK_SPACING,
        on_link_clicked: None,
        on_task_toggled: None,
    }
}

/// Construct an [`EnzMarkdown`] from an already-parsed block list, e.g. for a
/// test or a caller that parses once and re-renders across theme switches.
///
/// Manually-built trees are clipped at [`MAX_NESTING_DEPTH`] during rendering,
/// with the same visible truncation marker as parser-built trees. The complete
/// caller-owned tree, including the clipped tail, is destroyed iteratively and
/// is therefore safe to drop on a small GUI-thread stack.
#[must_use]
pub fn enz_markdown_blocks<'a, Message>(blocks: Vec<EnzMarkdownBlock>) -> EnzMarkdown<'a, Message> {
    EnzMarkdown {
        blocks,
        on_link_context: None,
        link_presentation: None,
        body_size: BODY_SIZE,
        heading_scale: None,
        image_resolver: None,
        line_height_multiple: LINE_HEIGHT_MULTIPLE,
        block_spacing: BLOCK_SPACING,
        on_link_clicked: None,
        on_task_toggled: None,
    }
}

impl<'a, Message> EnzMarkdown<'a, Message> {
    /// Override shared link tones and underline policy without changing actions or selection.
    #[must_use]
    pub fn link_presentation(mut self, tokens: crate::rich_text::LinkPresentationTokens) -> Self {
        self.link_presentation = Some(tokens);
        self
    }

    /// Body font size in px. Non-finite or non-positive input falls back to the
    /// default (14). Headings follow it: the shared heading ladder
    /// ([`EnzReadingPane::heading_size`], 28/24/20/17/15/13 at the default
    /// body) is scaled by `body / 14`, so a compact 13 px surface gets
    /// proportionally smaller headings. [`heading_scale`](Self::heading_scale)
    /// overrides that.
    #[must_use]
    pub fn body_size(mut self, sz: f32) -> Self {
        self.body_size = enzui_core::metrics::FinitePositive::new_or(sz, BODY_SIZE).get();
        self
    }

    /// Resolves images. The framework fetches nothing itself: `resolve`
    /// receives each image's URL exactly as authored and returns a handle the
    /// caller already holds (a decoded attachment, a cached file), or `None`
    /// to show the alt text in its place. Treat the URL as untrusted input.
    #[must_use]
    pub fn image_resolver(
        mut self,
        resolve: impl Fn(&MdUrl) -> Option<iced::widget::image::Handle> + 'a,
    ) -> Self {
        self.image_resolver = Some(Rc::new(resolve));
        self
    }

    /// Multiplies the heading ladder by `scale` instead of `body_size / 14`.
    /// `1.0` keeps the absolute reading-pane sizes whatever the body size.
    /// Non-finite or non-positive input is ignored.
    #[must_use]
    pub fn heading_scale(mut self, scale: f32) -> Self {
        if scale.is_finite() && scale > 0.0 {
            self.heading_scale = Some(scale);
        }
        self
    }

    /// The factor headings are drawn at, relative to the reading-pane ladder.
    #[must_use]
    pub fn effective_heading_scale(&self) -> f32 {
        self.heading_scale.unwrap_or(self.body_size / BODY_SIZE)
    }

    /// Line height as a multiple of the body size. Non-finite or non-positive
    /// input falls back to the default (1.6).
    #[must_use]
    pub fn line_height_multiple(mut self, m: f32) -> Self {
        self.line_height_multiple =
            enzui_core::metrics::FinitePositive::new_or(m, LINE_HEIGHT_MULTIPLE).get();
        self
    }

    /// Vertical gap between top-level blocks in px.
    ///
    /// Normalized to finite non-negative: `0` is expressible intent
    /// meaning "no gap" — blocks butt together — and is honoured
    /// literally, because `column().spacing(0.0)` is exactly that.
    /// Substituting the 12 px default for `0` would reverse the caller's
    /// meaning. `NaN` / `±inf` / negative name no gap at all and still
    /// fall back to the default.
    #[must_use]
    pub fn block_spacing(mut self, s: f32) -> Self {
        self.block_spacing = enzui_core::metrics::FiniteNonNeg::new_or(s, BLOCK_SPACING).get();
        self
    }

    /// Route a link click to a message. The callback receives the link's
    /// destination URL (the payload), not its visible text. Without a
    /// callback a link click is a silent no-op — never a panic.
    #[must_use]
    pub fn on_link_clicked<F>(mut self, f: F) -> Self
    where
        F: Fn(MdUrl) -> Message + 'a,
    {
        self.on_link_clicked = Some(Rc::new(f));
        self
    }

    /// Let the reader tick task boxes (`- [ ]`). Each press reports the
    /// marker's byte range in the parsed source and the state asked for;
    /// [`TaskToggle::apply`] rewrites the source. Without a handler, task
    /// boxes show their state and cannot be changed.
    #[must_use]
    pub fn on_task_toggled(mut self, handler: impl Fn(TaskToggle) -> Message + 'a) -> Self {
        self.on_task_toggled = Some(Rc::new(handler));
        self
    }

    /// Handle a secondary press on a link with its URL and window-space point.
    ///
    /// The press is captured without opening the link or invoking an enclosing
    /// selection menu. With no handler, secondary presses remain unhandled.
    #[must_use]
    pub fn on_link_context(mut self, handler: impl Fn(MdUrl, iced::Point) -> Message + 'a) -> Self {
        self.on_link_context = Some(Rc::new(handler));
        self
    }

    /// Number of top-level blocks in the parsed document.
    #[must_use]
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// The parsed blocks, for tests and introspection.
    #[must_use]
    pub fn blocks(&self) -> &[EnzMarkdownBlock] {
        &self.blocks
    }
}

impl<'a, Message: Clone + 'a> EnzMarkdown<'a, Message> {
    /// Consume the builder and produce the widget tree.
    ///
    /// `Message: Clone` because a fenced code block renders through
    /// [`EnzCodeBlock`](crate::code_block::EnzCodeBlock), whose `From` impl
    /// requires it (in the `code_block` module).
    pub fn into_element(self) -> Element<'a, Message> {
        let body_size = self.body_size;
        let heading_scale = self.effective_heading_scale();
        let images = self.image_resolver.clone();
        let line_height = self.line_height_multiple;
        let mut handlers = LinkHandlers::new();
        handlers.presentation = self.link_presentation;
        if let Some(click) = self.on_link_clicked {
            handlers = handlers.on_click(move |url| click(url));
        }
        if let Some(context) = self.on_link_context {
            handlers = handlers.on_context(move |url, point| context(url, point));
        }

        let rendered = render_blocks_flat(
            self.blocks,
            BlockMetrics {
                body_size,
                heading_scale,
                line_height,
                block_spacing: self.block_spacing,
            },
            images.as_ref(),
            &handlers,
            self.on_task_toggled.as_ref(),
        );
        let mut col = column![].spacing(0.0);
        for (gap_before, element) in rendered {
            if gap_before > 0.0 {
                col = col.push(Space::new().height(gap_before));
            }
            col = col.push(element);
        }
        col.width(Length::Fill).into()
    }
}

impl<'a, Message: Clone + 'a> From<EnzMarkdown<'a, Message>> for Element<'a, Message> {
    fn from(md: EnzMarkdown<'a, Message>) -> Self {
        md.into_element()
    }
}

// A decoration accumulated while the iterative adapter walks nested
// blocks.
//
// Decorations become siblings in one flat `iced::widget::Row`, rather
// than recursively wrapping the leaf. Thus the number of Markdown
// levels affects row width and child count, but never iced
// widget-tree depth.
#[derive(Clone)]
enum RenderPrefix {
    Quote,
    ListItem {
        item_id: usize,
        marker: String,
        /// A task item's box and the name it is announced by.
        task: Option<(TaskMarker, String)>,
        nested: bool,
        marker_in_quote: bool,
    },
}

struct PendingRender {
    block: EnzMarkdownBlock,
    depth: u16,
    in_quote: bool,
    prefixes: Vec<RenderPrefix>,
    gap_before: f32,
}

// Add a block sequence to the LIFO work stack while preserving
// document order.
fn enqueue_blocks(
    pending: &mut Vec<PendingRender>,
    blocks: Vec<EnzMarkdownBlock>,
    depth: u16,
    in_quote: bool,
    prefixes: &[RenderPrefix],
    first_gap: f32,
    sibling_gap: f32,
) {
    for (index, block) in blocks.into_iter().enumerate().rev() {
        pending.push(PendingRender {
            block,
            depth,
            in_quote,
            prefixes: prefixes.to_vec(),
            gap_before: if index == 0 { first_gap } else { sibling_gap },
        });
    }
}

/// The type sizes and spacing every rendered block shares.
#[derive(Clone, Copy)]
struct BlockMetrics {
    body_size: f32,
    heading_scale: f32,
    line_height: f32,
    block_spacing: f32,
}

// Flatten the recursive Markdown model into leaf rows before creating
// iced widgets.
//
// `MAX_NESTING_DEPTH` still controls which Markdown content is
// reachable, but a 64-level Quote/List path is represented by 64
// fixed-width children in one row—not by 64 recursively nested
// Row/Column/Container widgets. This keeps layout, semantics
// traversal, painting, and widget destruction on a small GUI stack
// while retaining each quote bar, list marker, indent, and
// ambient-ink context.
fn render_blocks_flat<'a, Message: Clone + 'a>(
    blocks: Vec<EnzMarkdownBlock>,
    metrics: BlockMetrics,
    images: Option<&ImageResolver<'a>>,
    handlers: &LinkHandlers<'a, MdUrl, Message>,
    on_task: Option<&TaskHandler<'a, Message>>,
) -> Vec<(f32, Element<'a, Message>)> {
    let BlockMetrics {
        body_size,
        heading_scale,
        line_height,
        block_spacing,
    } = metrics;
    let mut pending = Vec::new();
    enqueue_blocks(&mut pending, blocks, 0, false, &[], 0.0, block_spacing);

    let mut rendered = Vec::new();
    let mut seen_list_items = HashSet::new();
    let mut next_item_id = 0_usize;

    while let Some(mut task) = pending.pop() {
        if task.depth >= MAX_NESTING_DEPTH {
            let marker = truncation_marker_element(task.depth, body_size, line_height);
            rendered.push((
                task.gap_before,
                decorate_flat_leaf(
                    marker,
                    &task.prefixes,
                    &mut seen_list_items,
                    body_size,
                    line_height,
                    on_task,
                ),
            ));
            // `task.block` still owns the clipped tail. Its iterative Drop drains
            // that ownership without following the 10,000-level tree on-stack.
            continue;
        }

        match &mut task.block {
            EnzMarkdownBlock::Quote(blocks) => {
                let mut prefixes = task.prefixes;
                prefixes.push(RenderPrefix::Quote);
                enqueue_blocks(
                    &mut pending,
                    std::mem::take(blocks),
                    task.depth + 1,
                    true,
                    &prefixes,
                    task.gap_before,
                    BLOCK_SPACING,
                );
            }
            EnzMarkdownBlock::List {
                start,
                items,
                tasks,
            } => {
                let start = *start;
                let tasks = std::mem::take(tasks);
                let mut scheduled = Vec::new();
                for (item_index, item) in std::mem::take(items).into_iter().enumerate() {
                    let item_id = next_item_id;
                    next_item_id += 1;
                    let marker = match start {
                        Some(n) => format!("{}.", n + item_index as u64),
                        None => "•".to_owned(),
                    };
                    // A task box is named by its item's first paragraph,
                    // which is what the reader sees beside it.
                    let task_box = tasks.get(item_index).cloned().flatten().map(|marker| {
                        let name = match item.first() {
                            Some(EnzMarkdownBlock::Paragraph(text)) => text.plain_text(),
                            _ => String::new(),
                        };
                        let name = if name.trim().is_empty() {
                            enzui_core::localization::text("enzui-markdown-task")
                        } else {
                            name
                        };
                        (marker, name)
                    });
                    let mut prefixes = task.prefixes.clone();
                    prefixes.push(RenderPrefix::ListItem {
                        item_id,
                        marker,
                        task: task_box,
                        nested: task.depth > 0,
                        marker_in_quote: task.in_quote,
                    });
                    for (block_index, block) in item.into_iter().enumerate() {
                        scheduled.push(PendingRender {
                            block,
                            depth: task.depth + 1,
                            in_quote: task.in_quote,
                            prefixes: prefixes.clone(),
                            gap_before: if item_index == 0 && block_index == 0 {
                                task.gap_before
                            } else {
                                4.0
                            },
                        });
                    }
                }
                pending.extend(scheduled.into_iter().rev());
            }
            EnzMarkdownBlock::Heading { .. }
            | EnzMarkdownBlock::Paragraph(_)
            | EnzMarkdownBlock::CodeBlock { .. }
            | EnzMarkdownBlock::Rule
            | EnzMarkdownBlock::Image { .. }
            | EnzMarkdownBlock::Table { .. }
            | EnzMarkdownBlock::Truncated { .. } => {
                let leaf = render_leaf(
                    &mut task.block,
                    body_size,
                    heading_scale,
                    images,
                    line_height,
                    task.in_quote,
                    handlers,
                );
                rendered.push((
                    task.gap_before,
                    decorate_flat_leaf(
                        leaf,
                        &task.prefixes,
                        &mut seen_list_items,
                        body_size,
                        line_height,
                        on_task,
                    ),
                ));
            }
        }
    }

    rendered
}

// Render a non-container block, resolving tones against the runtime
// palette so a theme switch re-tints unpinned runs.
fn render_leaf<'a, Message: Clone + 'a>(
    block: &mut EnzMarkdownBlock,
    body_size: f32,
    heading_scale: f32,
    images: Option<&ImageResolver<'a>>,
    line_height: f32,
    in_quote: bool,
    handlers: &LinkHandlers<'a, MdUrl, Message>,
) -> Element<'a, Message> {
    let tokens = default_tokens();
    let line_h = iced::widget::text::LineHeight::Relative(line_height);
    let ambient_ink = ambient_ink_for(in_quote);

    // `EnzMarkdownBlock` has a custom `Drop`, so fields cannot be moved out of
    // it directly. Take owned fields through mutable references.
    match block {
        EnzMarkdownBlock::Heading { level, inline } => {
            let level = (*level).clamp(1, 6);
            let inline = std::mem::take(inline);
            let size = EnzReadingPane::<MdUrl, Message>::heading_size(level) * heading_scale;
            let weight = EnzReadingPane::<MdUrl, Message>::heading_weight(level);
            let base_font = iced::Font {
                weight,
                ..iced::Font::default()
            };
            let plain = inline.prose_only_text();
            let el = inline.view_routed_with_handlers(
                &tokens,
                size,
                Some(base_font),
                Some(line_h),
                handlers,
                Some(ambient_ink),
                ThemedTokens::FromRuntimePalette,
            );
            crate::rich_text::ProseSemantics::wrap(el, plain)
        }
        EnzMarkdownBlock::Paragraph(inline) => {
            let inline = std::mem::take(inline);
            let plain = inline.prose_only_text();
            let el = inline.view_routed_with_handlers(
                &tokens,
                body_size,
                None,
                Some(line_h),
                handlers,
                Some(ambient_ink),
                ThemedTokens::FromRuntimePalette,
            );
            crate::rich_text::ProseSemantics::wrap(el, plain)
        }
        EnzMarkdownBlock::CodeBlock { language, code } => {
            let mut cb = enz_code_block::<Message>(std::mem::take(code));
            if let Some(lang) = std::mem::take(language) {
                cb = cb.language(lang);
            }
            cb.into()
        }
        EnzMarkdownBlock::Truncated { depth } => {
            truncation_marker_element(*depth, body_size, line_height)
        }
        EnzMarkdownBlock::Image { url, alt, title } => {
            let alt = std::mem::take(alt);
            match images.and_then(|resolve| resolve(url)) {
                // Natural size, scaled down to the width available.
                Some(handle) => crate::image::enz_image_handle(handle)
                    .fit(crate::image::EnzImageFit::Contain)
                    .alt_text(if alt.is_empty() {
                        title.clone().unwrap_or_default()
                    } else {
                        alt
                    })
                    .width(Length::Shrink)
                    .height(Length::Shrink)
                    .into(),
                None => {
                    let shown = match (alt.trim().is_empty(), title.as_deref()) {
                        (false, _) => {
                            enzui_core::localized!("enzui-markdown-image-alt", "alt" => alt)
                        }
                        (true, Some(title)) => {
                            enzui_core::localized!("enzui-markdown-image-title", "title" => title)
                        }
                        (true, None) => enzui_core::localization::text("enzui-markdown-image"),
                    };
                    image_placeholder_element(shown, body_size, line_height)
                }
            }
        }
        EnzMarkdownBlock::Table {
            alignments,
            header,
            rows,
        } => render_table(
            std::mem::take(alignments),
            std::mem::take(header),
            std::mem::take(rows),
            body_size,
            line_height,
            ambient_ink,
            handlers,
        ),
        EnzMarkdownBlock::Rule => container(Space::new().width(Length::Fill).height(1.0))
            .width(Length::Fill)
            .height(Length::Fixed(1.0))
            .style(|theme: &iced::Theme| {
                let p = crate::theme_lookup::palette(theme);
                container::Style {
                    background: Some(p.border.into()),
                    ..container::Style::default()
                }
            })
            .into(),
        EnzMarkdownBlock::Quote(_) | EnzMarkdownBlock::List { .. } => {
            unreachable!("container blocks are flattened before leaf rendering")
        }
    }
}

// Apply all logical nesting as siblings in a single row. The first
// flattened leaf in each list item receives its marker; later leaves
// retain an equal-width blank cell, matching the former
// marker-beside-item-column geometry.
fn decorate_flat_leaf<'a, Message: Clone + 'a>(
    leaf: Element<'a, Message>,
    prefixes: &[RenderPrefix],
    seen_list_items: &mut HashSet<usize>,
    body_size: f32,
    line_height: f32,
    on_task: Option<&TaskHandler<'a, Message>>,
) -> Element<'a, Message> {
    if prefixes.is_empty() {
        return leaf;
    }

    let line_h = iced::widget::text::LineHeight::Relative(line_height);
    let mut row = iced::widget::Row::new();
    for prefix in prefixes {
        match prefix {
            RenderPrefix::Quote => {
                let bar = container(Space::new().width(3.0).height(Length::Fill))
                    .width(Length::Fixed(3.0))
                    .style(|theme: &iced::Theme| {
                        let p = crate::theme_lookup::palette(theme);
                        container::Style {
                            background: Some(p.text_muted.into()),
                            ..container::Style::default()
                        }
                    });
                row = row.push(bar).push(Space::new().width(INDENT_STEP - 8.0));
            }
            RenderPrefix::ListItem {
                item_id,
                marker,
                task,
                nested,
                marker_in_quote,
            } => {
                if *nested {
                    row = row.push(Space::new().width(INDENT_STEP));
                }
                if let Some((task, name)) = task
                    && seen_list_items.insert(*item_id)
                {
                    row = row.push(task_box(task, name, on_task, body_size * line_height));
                } else if seen_list_items.contains(item_id) && task.is_some() {
                    row = row.push(Space::new().width(INDENT_STEP));
                } else if seen_list_items.insert(*item_id) {
                    let marker_ink = ambient_ink_for(*marker_in_quote);
                    let marker_el: Element<'a, Message> = text(marker.clone())
                        .size(body_size)
                        .line_height(line_h)
                        .style(move |theme: &iced::Theme| iced::widget::text::Style {
                            color: Some(marker_ink(theme)),
                        })
                        .into();
                    row = row.push(container(marker_el).width(Length::Fixed(INDENT_STEP)));
                } else {
                    row = row.push(Space::new().width(INDENT_STEP));
                }
            }
        }
    }
    row.push(leaf).width(Length::Fill).into()
}

/// What a task box reports when the reader ticks it.
type TaskHandler<'a, Message> = Rc<dyn Fn(TaskToggle) -> Message + 'a>;

/// A task item's box, centred on its first line. Without a handler it shows
/// the state and cannot be changed.
fn task_box<'a, Message: Clone + 'a>(
    task: &TaskMarker,
    name: &str,
    on_task: Option<&TaskHandler<'a, Message>>,
    line_px: f32,
) -> Element<'a, Message> {
    let checkbox = match on_task {
        Some(on_task) => {
            let on_task = Rc::clone(on_task);
            let source = task.source.clone();
            crate::checkbox::enz_checkbox(task.checked, "", move |checked| {
                on_task(TaskToggle {
                    source: source.clone(),
                    checked,
                })
            })
        }
        None => crate::checkbox::enz_checkbox(task.checked, "", |_| {
            unreachable!("a disabled task box publishes no toggle")
        })
        .enabled(false),
    }
    .aria_label(name.to_owned())
    .into_element();
    container(checkbox)
        .width(Length::Fixed(INDENT_STEP))
        .height(Length::Fixed(line_px))
        .align_y(iced::alignment::Vertical::Center)
        .into()
}

/// The prose text of a truncation marker, exposed so tests observe the exact
/// label an assistive-tech client would announce.
#[must_use]
pub fn truncation_marker_text(depth: u16) -> String {
    enzui_core::localized!("enzui-markdown-nested-content-truncated-at-levels", "depth" => depth)
}

// Render a truncation marker as a muted, italic paragraph carrying
// prose semantics so the truncation is both visible and announced.
//
// This is deliberately a plain `iced::widget::text` — not another
// `RichText` document — so the marker itself is a leaf that cannot
// recurse. `depth` records how many nesting levels the boundary
// happened at; the parser always passes [`MAX_NESTING_DEPTH`], the
// renderer passes its own descent depth when it declines to go
// deeper.
/// The alt text in an unresolved image's place: muted italic prose, so it
/// reads (and announces) as the image's description.
fn image_placeholder_element<'a, Message: 'a>(
    label: String,
    body_size: f32,
    line_height: f32,
) -> Element<'a, Message> {
    let italic = iced::Font {
        style: iced::font::Style::Italic,
        ..iced::Font::default()
    };
    let el: Element<'a, Message> = text(label.clone())
        .size(body_size)
        .font(italic)
        .line_height(iced::widget::text::LineHeight::Relative(line_height))
        .style(|theme: &iced::Theme| iced::widget::text::Style {
            color: Some(crate::theme_lookup::palette(theme).text_muted),
        })
        .into();
    crate::rich_text::ProseSemantics::wrap(el, label)
}

/// A GFM table: equal columns, a bold header over a hairline, each cell
/// aligned as its column says, published as a grid with column headers.
fn render_table<'a, Message: Clone + 'a>(
    alignments: Vec<ColumnAlign>,
    header: Vec<RichText<MdUrl>>,
    rows: Vec<Vec<RichText<MdUrl>>>,
    body_size: f32,
    line_height: f32,
    ambient_ink: impl Fn(&iced::Theme) -> iced::Color + Clone + 'a,
    handlers: &LinkHandlers<'a, MdUrl, Message>,
) -> Element<'a, Message> {
    use crate::semantics_container::SemanticsContainer;
    use enzui_core::semantics::{Role, Semantics};
    let tokens = default_tokens();
    let line_h = iced::widget::text::LineHeight::Relative(line_height);
    let cell = |inline: RichText<MdUrl>, column: usize, bold: bool| -> Element<'a, Message> {
        let inline = if bold {
            RichText::new(
                inline
                    .spans()
                    .iter()
                    .cloned()
                    .map(|mut span| {
                        span.style.bold = true;
                        span
                    })
                    .collect(),
            )
        } else {
            inline
        };
        let plain = inline.prose_only_text();
        let content = inline.view_routed_with_handlers(
            &tokens,
            body_size,
            None,
            Some(line_h),
            handlers,
            Some(ambient_ink.clone()),
            ThemedTokens::FromRuntimePalette,
        );
        let align = match alignments.get(column).copied().unwrap_or_default() {
            ColumnAlign::Center => iced::alignment::Horizontal::Center,
            ColumnAlign::Right => iced::alignment::Horizontal::Right,
            ColumnAlign::Default | ColumnAlign::Left => iced::alignment::Horizontal::Left,
        };
        let role = if bold {
            Role::ColumnHeader
        } else {
            Role::GridCell
        };
        let padded = container(content)
            .width(Length::FillPortion(1))
            .align_x(align)
            .padding([4.0, 8.0]);
        SemanticsContainer::new(padded, Semantics::new(role).label(plain)).into()
    };
    let row_of = |cells: Vec<RichText<MdUrl>>, bold: bool| -> Element<'a, Message> {
        let mut row = iced::widget::Row::new().width(Length::Fill);
        for (column, inline) in cells.into_iter().enumerate() {
            row = row.push(cell(inline, column, bold));
        }
        SemanticsContainer::new(row, Semantics::new(Role::GridRow)).into()
    };
    let hairline = || {
        container(Space::new().width(Length::Fill).height(1.0))
            .width(Length::Fill)
            .height(Length::Fixed(1.0))
            .style(|theme: &iced::Theme| container::Style {
                background: Some(crate::theme_lookup::palette(theme).border.into()),
                ..container::Style::default()
            })
    };
    let mut grid = column![row_of(header, true), hairline()].width(Length::Fill);
    for row in rows {
        grid = grid.push(row_of(row, false));
    }
    SemanticsContainer::new(grid, Semantics::new(Role::Grid)).into()
}

fn truncation_marker_element<'a, Message: 'a>(
    depth: u16,
    body_size: f32,
    line_height: f32,
) -> Element<'a, Message> {
    let label = truncation_marker_text(depth);
    let line_h = iced::widget::text::LineHeight::Relative(line_height);
    let italic = iced::Font {
        style: iced::font::Style::Italic,
        ..iced::Font::default()
    };
    let el: Element<'a, Message> = text(label.clone())
        .size(body_size)
        .font(italic)
        .line_height(line_h)
        .style(|theme: &iced::Theme| {
            let p = crate::theme_lookup::palette(theme);
            iced::widget::text::Style {
                color: Some(p.text_muted),
            }
        })
        .into();
    // Wrap in ProseSemantics so selection-scope collection sees the marker
    // as a selectable text region alongside the surrounding prose.
    crate::rich_text::ProseSemantics::wrap(el, label)
}

// Build the ambient-ink resolver for a block, through the shared
// precedence ladder.
//
// `EnzMarkdown` sets no per-instance ink, so the [`AmbientInk`] is
// empty and only rung 4 fires: [`Fallback::Muted`] inside a
// blockquote, [`Fallback::Text`] otherwise. An empty ladder resolving
// `Fallback::Text` reproduces `runtime_ink` (`palette.text`) byte for
// byte — that equivalence is the non-quote regression guard. Routing
// through the shared `resolve` keeps the precedence rule identical to
// the sibling widgets rather than reimplemented here.
//
// [`AmbientInk`]: crate::ambient_ink::AmbientInk
// [`Fallback::Muted`]: crate::ambient_ink::Fallback::Muted
// [`Fallback::Text`]: crate::ambient_ink::Fallback::Text
fn ambient_ink_for(in_quote: bool) -> impl Fn(&iced::Theme) -> iced::Color + Clone {
    let fallback = if in_quote {
        crate::ambient_ink::Fallback::Muted
    } else {
        crate::ambient_ink::Fallback::Text
    };
    move |theme: &iced::Theme| {
        crate::ambient_ink::AmbientInk::new().resolve_with_theme(theme, fallback)
    }
}

// Fallback tokens (light-palette-derived); the runtime resolver
// overrides these per draw for tones, and `runtime_ink` overrides
// ambient ink.
fn default_tokens() -> TextTokens {
    TextTokens::from_palette(&enzui_theme::palette::Palette::light())
}

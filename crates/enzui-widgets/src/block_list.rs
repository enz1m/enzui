//! EnzBlockList: sequence of prose blocks (heading / paragraph /
//! blockquote / list-item / ordered-item / rule / spacer).
//!
//! A flowing document container: per-kind styling (heading sizes, bullets,
//! blockquote accent rule, rules, spacers) and configurable rhythm
//! (line-height + block-spacing + indent-step from a `flow` preset).
//!
//! Prefer `EnzReadingPane` for long-form single-kind articles;
//! `EnzBlockList` shines for mixed-content notes (headings + bullets + rules
//! + quotes).
//!
//! # Where this belongs
//!
//! [`chat_bubble`](crate::chat_bubble) renders a finished prose-block reply
//! through this widget (Markdown headings, quotes, lists); the bubble owns
//! avatar + role tint, the list owns flow rhythm.
//!
//! # Image blocks
//!
//! An `Image` block with a source ([`image_path`](EnzBlock::image_path) /
//! [`image_handle`](EnzBlock::image_handle)) renders pixels through the
//! maintained [`EnzImage`](crate::image_widget::EnzImage): decoded images fit
//! a `3 × block_spacing` slot; loading/failing sources show a THEMED
//! placeholder (bordered, `surface_alt`, radius 4) in that box. Sourceless
//! [`image`](EnzBlock::image) renders the placeholder alone. Either way the
//! alt text publishes as the accessible label, never an empty spacer.
//!
//! # Deferred
//!
//! - Code block — mono panel with optional language caption: compose
//!   `EnzCodeBlock` at that position; this widget falls back to a monospace
//!   paragraph.
//!
//! # Inline decorations
//!
//! Bold/italic/code/underline/strike/tone/highlight/links via a
//! [`RichText`](crate::rich_text::RichText) payload on
//! [`with_inline`](EnzBlock::with_inline): present → the shared `rich_text`
//! adapter (the `EnzReadingPane` path); absent → the plain single-colour path
//! unchanged. Covers `inline_decorations`/`styled_runs` display and mid-
//! paragraph emphasis runs; the blockquote `emphasis: true` flag stays for
//! whole-block italic.

use iced::widget::{Space, column, container, row, text};
use iced::{Border, Color, Element, Length};

use enzui_theme::{EnzTheme, FontFamily, FontRole};

use crate::enz_text::{TextTokens, TextTone};

/// The kind of a single rendered block in an [`EnzBlockList`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EnzBlockKind {
    /// Section heading; its size/weight come from the block's `level` (1-6).
    Heading,
    /// Body paragraph (the default).
    #[default]
    Paragraph,
    /// Indented quotation with a leading accent rule; `emphasis` italicises it.
    /// `level` sets nesting depth (0-based, like [`Self::ListItem`]): a nested
    /// `<blockquote>` reply chain steps in one `indent_step` per level.
    Blockquote,
    /// Bulleted list item; `level` sets nesting depth.
    ListItem,
    /// Numbered list item; `number` sets the marker (0 = auto-index).
    OrderedItem,
    /// Horizontal divider rule (no text).
    Rule,
    /// Vertical blank space (no text).
    Spacer,
    /// Image block. With a source ([`image_path`](EnzBlock::image_path) /
    /// [`image_handle`](EnzBlock::image_handle)) pixels render through the
    /// maintained [`EnzImage`](crate::image_widget::EnzImage) (themed
    /// placeholder while loading/on failure); sourceless ([`image`](EnzBlock::image))
    /// renders the placeholder alone. Either way alt-text publishes, never an
    /// empty spacer.
    Image,
    /// Code block — rendered as a mono paragraph fallback;
    /// prefer composing `EnzCodeBlock` at the call site.
    Code,
}

impl EnzBlockKind {
    /// Parse the source block-kind string (`"heading"`, `"blockquote"`, …);
    /// any unrecognised value maps to [`EnzBlockKind::Paragraph`].
    pub fn from_name(s: &str) -> Self {
        match s {
            "heading" => Self::Heading,
            "blockquote" => Self::Blockquote,
            "list-item" => Self::ListItem,
            "ordered-item" => Self::OrderedItem,
            "rule" => Self::Rule,
            "spacer" => Self::Spacer,
            "image" => Self::Image,
            "code" => Self::Code,
            _ => Self::Paragraph,
        }
    }
}

/// One block of rendered content (heading, paragraph, list item, rule, …).
///
/// Generic link payload `L`, defaulting to `()` (link-free sites compile
/// unannotated). Reached only with inline-decoration link runs
/// (`RichSpan::link(text, payload)`): [`with_inline`](EnzBlock::with_inline)
/// binds `L` to the attached [`RichText`](crate::rich_text::RichText) payload
/// type, and [`on_link`](EnzBlockList::on_link) delivers it on click. Without
/// `with_inline`, `L` stays `()` — unannotated.
#[derive(Debug, Clone)]
#[must_use]
pub struct EnzBlock<L = ()> {
    /// What kind of block this is.
    pub kind: EnzBlockKind,
    /// The block's text content; empty for `Rule` / `Spacer` / `Image`.
    /// Ignored when `inline` is `Some` (the styled runs supersede plain text).
    pub text: String,
    /// Heading level (1-6) or list nesting depth.
    pub level: u8,
    /// Explicit list number (0 = auto-index).
    pub number: u32,
    /// True for italic emphasis. On [`EnzBlockKind::Blockquote`] this renders
    /// the text in a true italic font style (`font-italic`).
    pub emphasis: bool,
    /// How this block's lines sit across the column. Honoured on
    /// [`EnzBlockKind::Paragraph`]; every other kind ignores it, because a
    /// heading ladder, a bullet gutter and a quote rule each own their own
    /// horizontal placement. The default leaves the edge to the text
    /// direction. See [`EnzBlock::with_align_x`] for what moves.
    pub align_x: iced::advanced::text::Alignment,
    /// Optional inline decorations: styled runs / highlights within the
    /// block. `Some` renders through the shared
    /// [`RichText`](crate::rich_text::RichText) adapter (bold/italic/code/
    /// underline/strike/tone/highlight), superseding `text`; `None` keeps the
    /// plain single-colour path. Ignored on `Rule`/`Spacer`/`Image`/`Code`.
    ///
    /// A link run's payload is `L` (`RichSpan::link(text, payload)` delivers
    /// through [`on_link`](EnzBlockList::on_link)). Link-free, `L` stays `()`:
    /// destinations were unrepresentable, every link a unit value.
    pub inline: Option<crate::rich_text::RichText<L>>,
    /// Alt text for [`EnzBlockKind::Image`] blocks. Published as the
    /// accessible label on the image's semantics node. When `None`, a generic
    /// `"Image"` fallback is used so the node is still announced. Ignored on
    /// non-image kinds.
    pub alt: Option<String>,
    /// Image source for [`EnzBlockKind::Image`] blocks. `Some` renders pixels
    /// through the maintained [`EnzImage`](crate::image_widget::EnzImage) (file
    /// path or decoded `image::Handle`; theme placeholder while
    /// loading/on-failure). `None` renders the placeholder alone
    /// (source-free). Ignored on non-image kinds.
    pub src: Option<EnzBlockImage>,
}

/// The pixels behind an [`EnzBlockKind::Image`] block.
///
/// Through the maintained [`image_widget`](crate::image_widget), never a
/// second image path: [`Path`](Self::Path) decodes from disk asynchronously
/// (placeholder until arrival); [`Handle`](Self::Handle) is already-decoded
/// (raw RGBA / encoded bytes / cached) rendering synchronously. Bad paths and
/// undecodable handles fall back to the sourceless placeholder — the
/// primitive's own failure behaviour, not a second path here.
#[derive(Debug, Clone)]
pub enum EnzBlockImage {
    /// A file-path source, decoded from disk by the image primitive.
    Path(String),
    /// An already-decoded image handle (raw RGBA / encoded bytes / cached).
    Handle(iced::widget::image::Handle),
}

impl EnzBlock<()> {
    /// A heading block at the given level (1-6).
    pub fn heading(text: impl Into<String>, level: u8) -> Self {
        Self {
            kind: EnzBlockKind::Heading,
            text: text.into(),
            level,
            number: 0,
            emphasis: false,
            align_x: iced::advanced::text::Alignment::Default,
            inline: None,
            alt: None,
            src: None,
        }
    }
    /// A body paragraph block.
    pub fn paragraph(text: impl Into<String>) -> Self {
        Self {
            kind: EnzBlockKind::Paragraph,
            text: text.into(),
            level: 0,
            number: 0,
            emphasis: false,
            align_x: iced::advanced::text::Alignment::Default,
            inline: None,
            alt: None,
            src: None,
        }
    }
    /// A blockquote block.
    pub fn blockquote(text: impl Into<String>) -> Self {
        Self {
            kind: EnzBlockKind::Blockquote,
            text: text.into(),
            level: 0,
            number: 0,
            emphasis: false,
            align_x: iced::advanced::text::Alignment::Default,
            inline: None,
            alt: None,
            src: None,
        }
    }
    /// A bulleted list item at the given nesting level.
    pub fn list_item(text: impl Into<String>, level: u8) -> Self {
        Self {
            kind: EnzBlockKind::ListItem,
            text: text.into(),
            level,
            number: 0,
            emphasis: false,
            align_x: iced::advanced::text::Alignment::Default,
            inline: None,
            alt: None,
            src: None,
        }
    }
    /// A numbered list item at the given level; `number` is 0 for auto-index.
    pub fn ordered_item(text: impl Into<String>, level: u8, number: u32) -> Self {
        Self {
            kind: EnzBlockKind::OrderedItem,
            text: text.into(),
            level,
            number,
            emphasis: false,
            align_x: iced::advanced::text::Alignment::Default,
            inline: None,
            alt: None,
            src: None,
        }
    }
    /// A horizontal divider rule.
    pub fn rule() -> Self {
        Self {
            kind: EnzBlockKind::Rule,
            text: String::new(),
            level: 0,
            number: 0,
            emphasis: false,
            align_x: iced::advanced::text::Alignment::Default,
            inline: None,
            alt: None,
            src: None,
        }
    }
    /// A vertical blank spacer.
    pub fn spacer() -> Self {
        Self {
            kind: EnzBlockKind::Spacer,
            text: String::new(),
            level: 0,
            number: 0,
            emphasis: false,
            align_x: iced::advanced::text::Alignment::Default,
            inline: None,
            alt: None,
            src: None,
        }
    }
    /// A code block (rendered as a monospace paragraph fallback; prefer
    /// composing `EnzCodeBlock` at the call site — see the module docs).
    pub fn code(text: impl Into<String>) -> Self {
        Self {
            kind: EnzBlockKind::Code,
            text: text.into(),
            level: 0,
            number: 0,
            emphasis: false,
            align_x: iced::advanced::text::Alignment::Default,
            inline: None,
            alt: None,
            src: None,
        }
    }

    /// An image block with no pixel source: renders the themed
    /// placeholder alone. `alt` publishes as the accessible label; `None` for
    /// decorative images (a generic `"Image"` fallback still rides the node).
    ///
    /// Real pixels: [`image_path`](EnzBlock::image_path) /
    /// [`image_handle`](EnzBlock::image_handle).
    pub fn image(alt: Option<impl Into<String>>) -> Self {
        Self {
            kind: EnzBlockKind::Image,
            text: String::new(),
            level: 0,
            number: 0,
            emphasis: false,
            align_x: iced::advanced::text::Alignment::Default,
            inline: None,
            alt: alt.map(std::convert::Into::into),
            src: None,
        }
    }

    /// An image block backed by a file-path source. The pixels are
    /// decoded from disk by the maintained image primitive
    /// ([`crate::image_widget::EnzImage`]); the themed placeholder shows while
    /// the source loads or if it fails to decode. The `alt` text is published
    /// as the accessible label (`None` → generic `"Image"` fallback).
    pub fn image_path(path: impl Into<String>, alt: Option<impl Into<String>>) -> Self {
        Self {
            kind: EnzBlockKind::Image,
            text: String::new(),
            level: 0,
            number: 0,
            emphasis: false,
            align_x: iced::advanced::text::Alignment::Default,
            inline: None,
            alt: alt.map(std::convert::Into::into),
            src: Some(EnzBlockImage::Path(path.into())),
        }
    }

    /// An image block backed by an already-decoded
    /// `iced::widget::image::Handle` — raw RGBA,
    /// encoded bytes, or a cached handle — rendered synchronously through the
    /// maintained image primitive. The `alt` text is published as the
    /// accessible label (`None` → generic `"Image"` fallback).
    pub fn image_handle(
        handle: impl Into<iced::widget::image::Handle>,
        alt: Option<impl Into<String>>,
    ) -> Self {
        Self {
            kind: EnzBlockKind::Image,
            text: String::new(),
            level: 0,
            number: 0,
            emphasis: false,
            align_x: iced::advanced::text::Alignment::Default,
            inline: None,
            alt: alt.map(std::convert::Into::into),
            src: Some(EnzBlockImage::Handle(handle.into())),
        }
    }

    /// Attach inline decorations: styled runs / highlights via the shared
    /// [`RichText`](crate::rich_text::RichText) adapter. Set, the block ignores
    /// plain `text` and renders runs. No effect on `Rule`/`Spacer`/`Image`/`Code`
    /// (fixed layouts).
    ///
    /// TYPE-CHANGING: consumes `EnzBlock<()>` (every constructor's type),
    /// returns `EnzBlock<L>` bound to the attached [`RichText<L>`](crate::rich_text::RichText)
    /// payload — what lets a block carry a meaningful link destination
    /// (`RichText<Dest>` → `EnzBlock<Dest>` → [`on_link`](EnzBlockList::on_link)
    /// delivers `Dest`). Link-free `RichText<()>` yields `EnzBlock<()>`: existing
    /// sites unchanged. Constructors live on `EnzBlock<()>` unannotated; only
    /// `with_inline` moves the parameter, only for a caller-chosen payload.
    /// Matches
    /// [`EnzReadingParagraph::with_inline`](crate::reading_pane::EnzReadingParagraph::with_inline).
    pub fn with_inline<L>(self, inline: crate::rich_text::RichText<L>) -> EnzBlock<L> {
        EnzBlock {
            kind: self.kind,
            text: self.text,
            level: self.level,
            number: self.number,
            emphasis: self.emphasis,
            align_x: self.align_x,
            inline: Some(inline),
            alt: self.alt,
            src: self.src,
        }
    }
}

impl<L> EnzBlock<L> {
    /// Align this block's lines across the column.
    ///
    /// Two steps, both geometry-following: lines align inside their own shaped
    /// box (moves a wrapped paragraph's short lines), then the box docks to
    /// the column edge (moves sub-column paragraphs). Widening the text node
    /// and ink-anchoring inside would divorce paint from selection/hit-test
    /// geometry. Centring/right-aligning needs a column: the list then takes
    /// full offered width instead of shrink-to-widest.
    ///
    /// Honoured on [`Paragraph`](EnzBlockKind::Paragraph).
    pub fn with_align_x(mut self, align_x: impl Into<iced::advanced::text::Alignment>) -> Self {
        self.align_x = align_x.into();
        self
    }
}

// Where a block's shaped box sits in the column. `None` leaves the element
// unwrapped, which is how an unaligned block renders.
fn block_placement(
    align_x: iced::advanced::text::Alignment,
) -> Option<iced::alignment::Horizontal> {
    match align_x {
        iced::advanced::text::Alignment::Center => Some(iced::alignment::Horizontal::Center),
        iced::advanced::text::Alignment::Right => Some(iced::alignment::Horizontal::Right),
        // `Default`, `Left` and `Justified` all start at the leading edge,
        // which is where an unwrapped element already is.
        _ => None,
    }
}

/// Rhythm flow preset. Sets leading + block spacing + indent step.
/// Rhythm flow preset. Sets leading + block spacing + indent step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EnzBlockFlow {
    /// Tight rhythm: 1.3× leading, 6px block spacing, 16px indent step.
    Compact,
    /// Balanced rhythm (the default): 1.5× leading, 12px spacing, 20px indent.
    #[default]
    Comfortable,
    /// Long-form rhythm: 1.6× leading, 16px spacing, 24px indent.
    Reading,
    /// Chat rhythm: 1.4× leading, 8px spacing, 18px indent.
    Chat,
}

impl EnzBlockFlow {
    /// Parse the source flow string; unrecognised values map to `Comfortable`.
    pub fn from_name(s: &str) -> Self {
        match s {
            "compact" => Self::Compact,
            "reading" => Self::Reading,
            "chat" => Self::Chat,
            _ => Self::Comfortable,
        }
    }
    /// Line-height multiple (leading) for this flow.
    pub fn line_height_multiple(self) -> f32 {
        match self {
            Self::Compact => 1.3,
            Self::Comfortable => 1.5,
            Self::Reading => 1.6,
            Self::Chat => 1.4,
        }
    }
    /// Vertical gap between blocks, in px, for this flow.
    pub fn block_spacing(self) -> f32 {
        match self {
            Self::Compact => 6.0,
            Self::Comfortable => 12.0,
            Self::Reading => 16.0,
            Self::Chat => 8.0,
        }
    }
    /// Indent increment per nesting level, in px, for this flow.
    pub fn indent_step(self) -> f32 {
        match self {
            Self::Compact => 16.0,
            Self::Comfortable => 20.0,
            Self::Reading => 24.0,
            Self::Chat => 18.0,
        }
    }
}

// resolve each metric override against this flow's preset, validating
// that no non-finite / negative value reaches a layout length or a text metric.
// Each falls back to *its own* preset — never another field's value — so the
// builder is order-independent. Pure and standalone so the degradation is
// directly testable without laying out a widget.

/// Resolve the effective line-height multiple: the override if finite and
/// strictly positive, else the flow preset. A 0 or negative multiplier would
/// collapse every line, so it is not expressible here.
fn resolve_line_height(override_: Option<f32>, flow: EnzBlockFlow) -> f32 {
    let preset = flow.line_height_multiple();
    enzui_core::metrics::FinitePositive::new_or(override_.unwrap_or(preset), preset).get()
}

/// Resolve the effective inter-block spacing (px): the override if finite and
/// non-negative, else the flow preset. `0` is expressible ("no gap") and
/// honoured; NaN/inf/negative degrade. This reaches `Length::Fixed` at the
/// call site, so non-finite lengths are forbidden.
fn resolve_block_spacing(override_: Option<f32>, flow: EnzBlockFlow) -> f32 {
    let preset = flow.block_spacing();
    enzui_core::metrics::FiniteNonNeg::new_or(override_.unwrap_or(preset), preset).get()
}

/// Resolve the effective per-level indent step (px): the override if finite and
/// non-negative, else the flow preset. `0` is expressible ("no indent") and
/// honoured.
fn resolve_indent_step(override_: Option<f32>, flow: EnzBlockFlow) -> f32 {
    let preset = flow.indent_step();
    enzui_core::metrics::FiniteNonNeg::new_or(override_.unwrap_or(preset), preset).get()
}

/// Resolve the effective indent base (px). When the caller set an explicit
/// base, validate it the same way as `indent_step`. Otherwise fall back to
/// the resolved indent step, reproducing the default multiplicative
/// ladder where base == step.
fn resolve_indent_base(override_: Option<f32>, indent_step: f32) -> f32 {
    match override_ {
        Some(v) => enzui_core::metrics::FiniteNonNeg::new_or(v, indent_step).get(),
        None => indent_step,
    }
}

/// Per-level ordered-list numbering, resetting at list boundaries.
///
/// The document is flat [`EnzBlock`]s; ordered lists are STATEFUL (a marker
/// depends on same-level predecessors within its list). Three rules:
///
/// 1. Reset at boundaries. Any non-[`OrderedItem`](EnzBlockKind::OrderedItem)
///    ends the list; the next run restarts at 1 (else two `1. 2. 3.` lists
///    around a paragraph become `1..6` under a global counter).
/// 2. Scope by level. Each `level` counts independently: nested lists run
///    `1, 2, …` alone; returning to the parent CONTINUES it (never inherits
///    the child). Descending never resets; ascending past a level DISCARDS
///    deeper counters (later sibling nests restart).
/// 3. Explicit-number continuation. `number > 0` sets the marker and seeds
///    the counter (`number + 1` next). `number == 0` is auto — honoured
///    expressible intent, but markers are 1-based: the auto path never renders
///    `0.`.
#[derive(Debug, Default, Clone)]
struct OrderedNumbering {
    /// `counters[level]` is the next auto value to emit at that nesting level.
    /// A missing / `None` entry means "no ordered item seen at this level in the
    /// current list yet", so the next auto item there starts at 1.
    counters: Vec<Option<u32>>,
}

impl OrderedNumbering {
    /// A non-ordered block was encountered: the current ordered list ends, so
    /// every level's counter is cleared. The next ordered run restarts from 1.
    fn on_non_ordered(&mut self) {
        self.counters.clear();
    }

    /// Resolve the marker value for an ordered item at `level` carrying an
    /// explicit `number` (`0` = auto), advancing the level's counter.
    ///
    /// Ascending past a level (emitting at a shallower level than the deepest
    /// one seen) discards the deeper counters, so a subsequent sibling nested
    /// list restarts from 1 rather than continuing the previous nested list.
    fn next(&mut self, level: u8, number: u32) -> u32 {
        let level = level as usize;
        if self.counters.len() <= level {
            self.counters.resize(level + 1, None);
        } else {
            // Emitting at `level` closes any deeper open lists.
            self.counters.truncate(level + 1);
        }

        let value = if number > 0 {
            // Explicit number: this item shows `number`; the next auto item at
            // this level continues from `number + 1`.
            number
        } else {
            // Auto: 1-based, continuing the level's counter.
            self.counters[level].unwrap_or(1)
        };
        self.counters[level] = Some(value.saturating_add(1));
        value
    }
}

/// the default heading-size ladder (px) for levels 1..=6, kept as the
/// default so unset [`heading_sizes`](EnzBlockList::heading_sizes) renders
/// byte-identically to the hardcoded `heading_size` match.
///
/// A deliberate DOCUMENT ladder, distinct from
/// [`SizingTokens`](enzui_theme::SizingTokens) chrome rungs: headings are
/// a per-document editorial choice retuned per builder, not a framework rung.
/// Defaults stay raw literals; rung-derived ladders pass in.
pub const DEFAULT_HEADING_SIZES: [f32; 6] = [26.0, 22.0, 18.0, 16.0, 14.0, 13.0];

/// resolve a heading size (px) for a 1-based `level` from a ladder,
/// clamping to 1..=6 (level 0 → H1, level >6 → H6). Free function so the render
/// loop can call it after `self.blocks` has been moved out of `self`.
fn heading_size_from(ladder: &[f32; 6], level: u8) -> f32 {
    let idx = (level.clamp(1, 6) - 1) as usize;
    ladder[idx]
}

/// Builder for a vertical stack of rendered blocks. See [`enz_block_list`].
///
/// Generic over a link payload `L` (defaults to `()`). When a block's
/// inline decorations contain link runs carrying an `L`, register a callback
/// with [`EnzBlockList::on_link`] to receive that payload on click. Without
/// links, `L` stays `()` and no annotation is needed.
#[must_use]
pub struct EnzBlockList<'a, L, Message> {
    blocks: Vec<EnzBlock<L>>,
    flow: EnzBlockFlow,
    line_height_multiple: Option<f32>,
    block_spacing: Option<f32>,
    indent_step: Option<f32>,
    /// independent indent base (px) for the level-0 nesting offset.
    ///
    /// `None` (default): base == `indent_step` — the multiplicative ladder
    /// `indent_step * (level + 1)`. Set: the AFFINE `indent_base + indent_step
    /// * level` — a base differing from the step (an app html_view
    /// `base = 4, step = 12` shape).
    indent_base: Option<f32>,
    /// per-level heading sizes (px) for levels 1..=6. Defaults to the
    /// default ladder `[26, 22, 18, 16, 14, 13]` so unset behaviour is
    /// byte-identical; override via [`EnzBlockList::heading_sizes`] to follow a
    /// caller's own type ladder (e.g. An app's theme chrome ladder).
    heading_sizes: [f32; 6],
    /// the ambient-ink precedence ladder — absolute pin, caller resolver,
    /// semantic tone — shared with [`crate::streaming_text`] rather than
    /// reimplemented here. See [`crate::ambient_ink`] for the rung order and the
    /// reason rung 2 (`text_color_resolved`) cannot be expressed as either of
    /// its neighbours.
    ink: crate::ambient_ink::AmbientInk,
    accent_color: Option<Color>,
    font_role: FontRole,
    font_family: Option<FontFamily>,
    mono_family: Option<FontFamily>,
    /// caller-supplied link callback. A link run's `L` payload maps to
    /// `Message` here on activation and publishes. `None` (default): no
    /// handler — link clicks silently no-op (the pre-threading contract).
    ///
    /// `Rc`: the render loop consumes `self.blocks` while each inline arm's
    /// `view_routed_themed` needs its own callback handle (`Box<dyn Fn>` is
    /// not `Clone`).
    on_link: Option<std::rc::Rc<dyn Fn(L) -> Message + 'a>>,
    _phantom: std::marker::PhantomData<&'a Message>,
}

/// Construct a block list from a sequence of blocks. Defaults to the
/// [`EnzBlockFlow::Comfortable`] rhythm with theme-resolved colours.
///
/// Generic over the link payload `L`, inferred from the `blocks` vec and
/// defaulting to `()` for link-free documents — so existing call sites need no
/// annotation.
pub fn enz_block_list<L, Message>(blocks: Vec<EnzBlock<L>>) -> EnzBlockList<'static, L, Message> {
    EnzBlockList {
        blocks,
        flow: EnzBlockFlow::Comfortable,
        line_height_multiple: None,
        block_spacing: None,
        indent_step: None,
        indent_base: None,
        heading_sizes: DEFAULT_HEADING_SIZES,
        ink: crate::ambient_ink::AmbientInk::new(),
        accent_color: None,
        on_link: None,
        font_role: FontRole::Ui,
        font_family: None,
        mono_family: None,
        _phantom: std::marker::PhantomData,
    }
}

/// A caller-supplied ambient-ink resolver, run against the live `&Theme` on
/// every render.
///
/// `Rc`: plain-text style closure and each inline block's ambient closure need
/// own handles (`Box<dyn Fn>` is not `Clone`). The resolver lives in
/// [`ambient_ink`](crate::ambient_ink), shared with [`streaming_text`](crate::streaming_text).
/// This alias is the THEME-keyed closure handed to `rich_text` — distinct from
/// the palette-keyed rung-2 resolver callers supply.
pub(crate) type InkResolver = std::rc::Rc<dyn Fn(&iced::Theme) -> Color>;

/// the inline (`RichText`) variant of `inline_text_tokens` that honours
/// a semantic [`TextTone`] against a caller-supplied runtime palette.
///
/// The plain path resolves tone in an `iced` style closure (live `&Theme`);
/// the inline path cannot (`rich_text` bakes `Span::color` eagerly at build)
/// — so tone resolves here against the caller's visible palette. `chat_bubble`
/// passes DARK for inline parity, but ink follows the requested ROLE tone, never unconditional `default`. Absolute `ink_pin`
/// still wins outright.
pub fn inline_text_tokens_toned(
    ink_pin: Option<Color>,
    tone: Option<TextTone>,
    palette: &enzui_theme::palette::Palette,
) -> TextTokens {
    let mut t = TextTokens::from_palette(palette);
    if let Some(c) = ink_pin {
        t.default_color = c;
    } else if let Some(tone) = tone {
        t.default_color = t.color_for(tone);
    }
    t
}

/// Blockquote variant: the ambient ink is the muted text tone (so unstyled
/// runs inherit blockquote colour), while explicit semantic tones still
/// resolve through the full palette.
///
/// accepts the palette explicitly so the caller can pass one derived from
/// `theme_lookup::palette(t)` instead of hardcoding `Palette::dark()`.
pub fn inline_text_tokens_muted(
    ink_pin: Option<Color>,
    palette: &enzui_theme::palette::Palette,
) -> TextTokens {
    let mut t = TextTokens::from_palette(palette);
    t.default_color = ink_pin.unwrap_or(palette.text_muted);
    t
}

/// Resolve the muted blockquote ink for the runtime theme `t`.
///
/// Delegates to the shared [`ambient_ink`](crate::ambient_ink) ladder with the
/// blockquote's own [`Muted`](crate::ambient_ink::Fallback::Muted) rung-4
/// default: the precedence rule defined exactly once across this widget and
/// [`streaming_text`](crate::streaming_text). Inline (`RichText`) and
/// plain-text blockquote arms both resolve here — neither updates without the
/// other (the divergence class that let the plain arm ignore
/// `.text_color(..)`/`.text_color_resolved(..)` while inline honoured them).
///
/// [`Fallback::Muted`]: crate::ambient_ink::Fallback::Muted
pub(crate) fn resolve_muted_ink(ink: &crate::ambient_ink::AmbientInk, t: &iced::Theme) -> Color {
    ink.resolve_with_theme(t, crate::ambient_ink::Fallback::Muted)
}

impl<'a, L, Message: 'a> EnzBlockList<'a, L, Message> {
    /// Register the link callback. A
    /// [`link(text, payload)`](crate::rich_text::RichSpan::link) run inside
    /// inline decorations maps `payload` (`L`) to `Message` through `f` on
    /// activation and publishes.
    ///
    /// Without it link clicks silently no-op (no `on_link_click` installed;
    /// iced's `rich_text::update` early-returns) — the pre-threading contract.
    /// Registering is the only typed-destination path to the application.
    pub fn on_link(mut self, f: impl Fn(L) -> Message + 'a) -> Self {
        self.on_link = Some(std::rc::Rc::new(f));
        self
    }

    /// Set the rhythm preset. Defaults to [`EnzBlockFlow::Comfortable`]. The
    /// per-value overrides below take precedence over the preset.
    pub fn flow(mut self, f: EnzBlockFlow) -> Self {
        self.flow = f;
        self
    }
    /// Override the line-height multiple, ignoring the flow preset's value.
    pub fn line_height_multiple(mut self, m: f32) -> Self {
        self.line_height_multiple = Some(m);
        self
    }
    /// Override the inter-block spacing (px), ignoring the flow preset.
    pub fn block_spacing(mut self, s: f32) -> Self {
        self.block_spacing = Some(s);
        self
    }
    /// Override the per-level indent step (px), ignoring the flow preset.
    pub fn indent_step(mut self, i: f32) -> Self {
        self.indent_step = Some(i);
        self
    }
    /// Override the indent base (px) — the level-0 nesting offset.
    ///
    /// By default the base equals `indent_step`, giving
    /// `indent_step × (level + 1)`. Set independently, the indent is
    /// `indent_base + indent_step × level`, so the first level can differ
    /// from the step.
    ///
    /// ```text
    /// // an app blockquote ladder (base=4, step=12):
    /// //   level 0 →  4    (4 + 0×12)
    /// //   level 1 → 16    (4 + 1×12)
    /// //   level 2 → 28    (4 + 2×12)
    /// ```
    pub fn indent_base(mut self, b: f32) -> Self {
        self.indent_base = Some(b);
        self
    }
    /// Override the body text colour. Unset by default (resolved from the theme).
    ///
    /// An ABSOLUTE pin: frozen at build, theme-switch-blind. The documented
    /// escape hatch — right for genuinely fixed colours. Theme-following fills
    /// need [`text_tone`](Self::text_tone) / [`text_color_resolved`](Self::text_color_resolved)
    /// instead: theme-dependent ink takes a reactive form, never this pin.
    pub fn text_color(mut self, c: Color) -> Self {
        self.ink.pin = Some(c);
        self
    }

    /// Set the ambient body ink as a closure over the runtime theme.
    ///
    /// Between [`text_color`](Self::text_color) (absolute, wins) and
    /// [`text_tone`](Self::text_tone) (semantic, loses) in precedence; re-run
    /// from live `&Theme` every render, plain-text and inline-`RichText`
    /// ambient alike — one list's halves never disagree. For theme-FUNCTION
    /// ink, not theme-VALUE ink: `chat_bubble`'s user bubble needs
    /// `ink_for(palette.accent, palette)` (highest-contrast ink vs its actual
    /// fill) — neither a constant nor any [`TextTone`].
    pub fn text_color_resolved(self, resolver: impl Fn(&iced::Theme) -> Color + 'static) -> Self {
        // the shared ladder is palette-keyed (the only form
        // `streaming_text`, generic over `Theme: StreamingTextTheme`, can
        // consume). This setter keeps its existing `&iced::Theme` signature by
        // adapting here, so no caller's closure changes meaning.
        Self {
            ink: self.ink.with_theme_resolver(resolver),
            ..self
        }
    }

    /// Set the ambient body ink as a semantic tone rather than an absolute
    /// colour.
    ///
    /// Prefer over [`text_color`](Self::text_color) for theme-following ink:
    /// resolved in the `iced` style closure vs live `&Theme` — switches
    /// re-tint without rebuild. Absolute `text_color` wins when both set.
    /// Exists because `chat_bubble` baked `ChatBubbleTokens::dark()` ink into
    /// `.text_color(..)`: 1.03:1 light-theme contrast — invisible bodies.
    pub fn text_tone(mut self, tone: TextTone) -> Self {
        self.ink.tone = Some(tone);
        self
    }
    /// Override the accent colour (blockquote rule, ordered markers). Unset by
    /// default (resolved from the theme).
    pub fn accent_color(mut self, c: Color) -> Self {
        self.accent_color = Some(c);
        self
    }

    /// override the per-level heading sizes (px) for levels 1..=6.
    ///
    /// `sizes[0]` = H1 … `sizes[5]` = H6. Out-of-range levels clamp: 0 is H1
    /// and anything above 6 is H6. Unset, the sizes are
    /// [`DEFAULT_HEADING_SIZES`] (`[26, 22, 18, 16, 14, 13]`). A caller with
    /// its own type ladder (an app HTML bodies: H1=22, H2=18, …) passes it
    /// here so its headings keep their sizes.
    pub fn heading_sizes(mut self, sizes: [f32; 6]) -> Self {
        self.heading_sizes = sizes;
        self
    }

    /// Select the unpinned prose role, defaulting to UI. Code stays monospace.
    pub fn font_role(mut self, role: FontRole) -> Self {
        self.font_role = role;
        self
    }

    /// Pin prose and code families from this theme. Semantic weights and
    /// quote italics remain independent of family pins.
    pub fn font_from_theme(mut self, theme: &EnzTheme) -> Self {
        self.font_family = Some(theme.font_catalog.family(self.font_role).clone());
        self.mono_family = Some(theme.font_catalog.family(FontRole::Monospace).clone());
        self
    }

    /// Resolve prose's pinned or application role family and regular weight.
    pub fn committed_body_font(&self) -> iced::Font {
        let mut font =
            enzui_theme::text_defaults::font(self.font_role, enzui_theme::WeightRung::Regular);
        if let Some(family) = &self.font_family {
            font.family = family.to_iced();
        }
        font
    }

    /// Resolve code's pinned or application monospace family and regular weight.
    pub fn committed_code_font(&self) -> iced::Font {
        let mut font =
            enzui_theme::text_defaults::font(FontRole::Monospace, enzui_theme::WeightRung::Regular);
        if let Some(family) = &self.mono_family {
            font.family = family.to_iced();
        }
        font
    }

    fn body_font_with(&self, weight: iced::font::Weight, style: iced::font::Style) -> iced::Font {
        iced::Font {
            weight,
            style,
            ..self.committed_body_font()
        }
    }

    /// The number of blocks in the list.
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    fn heading_weight(level: u8) -> iced::font::Weight {
        match level {
            1 | 2 => enzui_theme::text_defaults::weight(enzui_theme::WeightRung::Bold),
            _ => enzui_theme::text_defaults::weight(enzui_theme::WeightRung::Semibold),
        }
    }

    /// Consume the builder and produce the widget tree.
    ///
    /// The `L: Clone + PartialEq + 'static` bound comes from the shared
    /// `rich_text` adapter ([`view_routed_themed`](crate::rich_text::RichText::view_routed_themed))
    /// behind the inline arms; `()` (the default) satisfies it — link-free
    /// sites unaffected.
    ///
    /// [`RichText::view_routed_themed`]: crate::rich_text::RichText::view_routed_themed
    pub fn into_element(self) -> Element<'a, Message>
    where
        L: Clone + PartialEq + 'static,
    {
        // capture the resolved font descriptors before any field of
        // `self` moves out. Reading through the same accessors the gates
        // observe (`committed_body_font` / `committed_code_font`) so a
        // sabotage that severs the catalog read breaks the widget and the
        // tests — the stored-field trap.
        let body_font_base = self.committed_body_font();
        let code_font = self.committed_code_font();
        let italic_body_font =
            self.body_font_with(body_font_base.weight, iced::font::Style::Italic);
        // Heading font as an array over the ladder's six levels, resolved
        // now so the render loop below does not need to borrow `self`.
        let heading_fonts: [iced::Font; 6] = [
            self.body_font_with(Self::heading_weight(1), iced::font::Style::Normal),
            self.body_font_with(Self::heading_weight(2), iced::font::Style::Normal),
            self.body_font_with(Self::heading_weight(3), iced::font::Style::Normal),
            self.body_font_with(Self::heading_weight(4), iced::font::Style::Normal),
            self.body_font_with(Self::heading_weight(5), iced::font::Style::Normal),
            self.body_font_with(Self::heading_weight(6), iced::font::Style::Normal),
        ];

        //  Tier B: body ink, the muted blockquote tone, the quote rule
        // accent, the horizontal rule and the code-block ground all resolve
        // against the live theme; caller-set colours still win.
        let ink = self.ink;
        // The eager inline-token bake still needs the raw pin (it is a build-time
        // fallback for toned spans, re-resolved per draw by `inline_themed!()`).
        let ink_pin = ink.pin;
        let ink_tone = ink.tone;
        let accent_pin = self.accent_color;
        // the shared link callback. `Rc` so each inline arm can clone its
        // own handle (the render loop below consumes `self.blocks`). A macro
        // produces the per-arm `Option<impl Fn(L) -> Message>` uniformly: when
        // the caller registered no callback the map yields `None` and iced
        // installs no handler (silent no-op click); when they did, the payload
        // is delivered on activation.
        let on_link = self.on_link;
        macro_rules! link_cb {
            () => {{
                let cb = on_link.clone();
                cb.map(|cb| move |l: L| cb(l))
            }};
        }
        // One resolver for plain, inline-ambient and blockquote-ambient paths:
        // no arm updates without the others. Precedence lives in
        // [`ambient_ink`](crate::ambient_ink) (shared with `streaming_text`):
        // the widgets cannot drift. Ladder: absolute pin > caller resolver >
        // semantic tone > `Palette::text`. `Rc<dyn Fn>`: cloned per-block
        // style closures over a consumed `self.blocks` — a capturing
        // `Option<Rc<..>>` is not `Copy`, so each arm takes its own handle.
        let ambient_ink: InkResolver = {
            let ink = ink.clone();
            std::rc::Rc::new(move |t: &iced::Theme| -> Color {
                ink.resolve_with_theme(t, crate::ambient_ink::Fallback::Text)
            })
        };
        /// One `text::Style` closure per plain-text block, each with its own
        /// handle on the shared resolver.
        macro_rules! ink_style {
            () => {{
                let ink = ambient_ink.clone();
                move |t: &iced::Theme| iced::widget::text::Style {
                    color: Some(ink(t)),
                }
            }};
        }
        /// One [`BoundInk`] per inline (`RichText`) block.
        ///
        /// The inline path hands `rich_text` the SHARED ladder itself
        /// (`RichText::view_routed_ink`), not a pre-collapsed closure.
        /// `rich_text`'s ambient channel was once opaque
        /// `Option<impl Fn(&Theme) -> Color>`, forcing this widget to own
        /// precedence on both paths; now only rung 4 (`Fallback::Text`) is
        /// this widget's contribution.
        ///
        /// [`BoundInk`]: crate::ambient_ink::BoundInk
        macro_rules! inline_ambient {
            () => {
                Some(ink.clone().bound(crate::ambient_ink::Fallback::Text))
            };
        }
        // precedence is absolute pin > semantic tone > `Palette::text`.
        // The tone branch resolves inside the closure, against the *runtime*
        // theme, so a light/dark switch re-tints without a rebuild. Baking a
        // resolved colour in here would leave rich blocks in the dark palette's
        // ink on a light page.

        // Inline-decorated blocks (`Some(RichText)`) go through the shared
        // `rich_text` adapter. Untoned runs emit `Span::color == None`, painted
        // as `span.color.or(style.color).unwrap_or(defaults.text_color)`, so
        // without a widget-level `style.color`:
        //
        // * `.text_color(c)` would reach `inline_tokens` as `default_color` and
        //   be silently dropped (`default_color` only reads via
        //   `color_for(tone)`, never called untoned), while the plain path
        //   honoured the same pin;
        // * unpinned untoned runs would fall to `defaults.text_color` (iced
        //   ambient), not the plain path's `ink_style`, making
        //   `.text_tone(Muted)` a no-op inline.
        //
        // `inline_ambient` installs the plain path's resolver AS the rich style
        // closure (consulting the live `&Theme` per render). Toned spans carry
        // their own per-span theme key (see `rich_text::model`), since
        // `style.color` only reads when `span.color` is `None`.
        // Blockquote ambient: the muted tone per render, matching the plain
        // arm below. Absolute pins still win; caller resolvers beat the muted
        // default — surface-computed ink means the quote too.
        let ambient_ink_muted: InkResolver = {
            let ink = ink.clone();
            std::rc::Rc::new(move |t: &iced::Theme| resolve_muted_ink(&ink, t))
        };
        /// One blockquote [`BoundInk`] per inline block — the [`inline_ambient`]
        /// ladder differing only at rung 4 ([`Muted`](Fallback::Muted)): the
        /// whole reason the fallback is a parameter, not a constant.
        ///
        /// [`BoundInk`]: crate::ambient_ink::BoundInk
        /// [`Fallback::Muted`]: crate::ambient_ink::Fallback::Muted
        macro_rules! inline_ambient_muted {
            () => {
                Some(ink.clone().bound(crate::ambient_ink::Fallback::Muted))
            };
        }
        /// One `text::Style` closure per plain blockquote block, resolving
        /// through the same shared muted resolver as the inline arm so both
        /// honour an absolute pin and a caller resolver identically.
        /// Without this the plain arm silently ignored `.text_color(..)` /
        /// `.text_color_resolved(..)` while the inline arm honoured them.
        macro_rules! plain_ambient_muted {
            () => {{
                let ink = ambient_ink_muted.clone();
                move |t: &iced::Theme| iced::widget::text::Style {
                    color: Some(ink(t)),
                }
            }};
        }
        // Eager `Span::color` bake tokens. This palette is FALLBACK ONLY:
        // `view_routed_themed` re-resolves every toned span at draw via
        // `inline_themed!()` → live-`&Theme` `theme_lookup::palette(t)` (the
        // accessor nine other call sites here use). Eager bake and draw-time
        // resolver both take the live palette: a `Palette::dark()`-locked bake
        // is a dead-but-wrong fallback. The `inline_themed` closure below
        // replaces `FromRuntimePalette` so even the fallback resolves right.
        let inline_tokens = inline_text_tokens_toned(
            ink_pin,
            ink_tone,
            &crate::theme_lookup::palette(&iced::Theme::Dark),
        );
        // per-call-site themed resolver macro. Each `view_routed_themed`
        // consumes a `ThemedTokens`, so a fresh one is produced per call.
        // Resolves toned inline spans against `theme_lookup::palette(t)` at
        // draw time.
        macro_rules! inline_themed {
            () => {
                crate::rich_text::ThemedTokens::Custom(Box::new(move |t: &iced::Theme| {
                    inline_text_tokens_toned(ink_pin, ink_tone, &crate::theme_lookup::palette(t))
                }))
            };
        }
        // Metric overrides store RAW (NaN/inf/negative pass setters):
        // `block_spacing` reaches `Length::Fixed` and `× 3.0` image slots;
        // `indent_step` reaches `Fixed(indent_step / 2.0)` and `Padding::left`;
        // `line_height_multiple` scales text metrics. Non-finite in any is
        // exactly what this crate forbids. Validate at this one consumption
        // point, each field to its own flow default (never another field's —
        // order-dependence). `line_height_multiple` is `FinitePositive` (0/neg
        // collapses every line); `block_spacing`/`indent_step` are
        // `FiniteNonNeg` (`0` = "no gap"/"no indent", honoured; only
        // NaN/inf/negative degrade to preset).
        let line_h = resolve_line_height(self.line_height_multiple, self.flow);
        let block_spacing = resolve_block_spacing(self.block_spacing, self.flow);
        let indent_step = resolve_indent_step(self.indent_step, self.flow);
        // resolve the indent base — defaults to `indent_step` so the
        // multiplicative ladder is preserved when the caller does not ask for
        // an affine split.
        let indent_base = resolve_indent_base(self.indent_base, indent_step);
        // capture the heading ladder before the loop consumes `self.blocks`.
        let heading_sizes = self.heading_sizes;
        let base_body_size = 13.0;
        // per-level ordered numbering that resets at list boundaries.
        // Any non-ordered block ends the current ordered list.
        let mut numbering = OrderedNumbering::default();

        let mut col = column![].spacing(block_spacing);
        for block in self.blocks {
            if block.kind != EnzBlockKind::OrderedItem {
                numbering.on_non_ordered();
            }
            let child: Element<'a, Message> = match block.kind {
                EnzBlockKind::Rule => {
                    container(Space::new().width(Length::Fill).height(Length::Fixed(1.0)))
                        .width(Length::Fill)
                        .height(Length::Fixed(1.0))
                        .style(move |t: &iced::Theme| iced::widget::container::Style {
                            background: Some(crate::theme_lookup::palette(t).border_strong.into()),
                            ..Default::default()
                        })
                        .into()
                }
                EnzBlockKind::Spacer => Space::new()
                    .width(Length::Fill)
                    .height(Length::Fixed(block_spacing))
                    .into(),
                EnzBlockKind::Image => {
                    // Alt text wins; a source-less or unnamed image still gets a
                    // generic "Image" label so the AT node is announced.
                    let alt_label =
                        block
                            .alt
                            .filter(|s| !s.trim().is_empty())
                            .unwrap_or_else(|| {
                                enzui_core::localization::text("enzui-block-list-image")
                            });
                    // A fixed slot height keeps the image block hugging content
                    // rather than filling the parent (the same class as the
                    // blockquote rule above): the primitive's slot is bounded so
                    // a decoded image is fit into it and a not-yet-loaded/failed
                    // source shows the themed placeholder in that same box.
                    let slot_h = block_spacing * 3.0;
                    match block.src {
                        Some(src) => {
                            // route real pixels through the maintained
                            // image primitive (`EnzImage`), which owns the
                            // decode, the Cover fit, the themed placeholder
                            // (shown while loading or on a decode failure) and
                            // the Role::Image a11y node carrying the alt label.
                            // No second image path is invented here.
                            let img = match src {
                                EnzBlockImage::Path(p) => crate::image_widget::enz_image(p),
                                EnzBlockImage::Handle(h) => {
                                    crate::image_widget::enz_image_handle(h)
                                }
                            };
                            img.alt_text(alt_label)
                                .radius(4.0)
                                .height(Length::Fixed(slot_h))
                                .into_element()
                        }
                        None => {
                            // No source: the themed placeholder alone, wrapped so
                            // AT users still perceive the (missing) image.
                            let placeholder = container(
                                Space::new()
                                    .width(Length::Fill)
                                    .height(Length::Fixed(slot_h)),
                            )
                            .width(Length::Fill)
                            .height(Length::Fixed(slot_h))
                            .style(|t: &iced::Theme| {
                                let p = crate::theme_lookup::palette(t);
                                iced::widget::container::Style {
                                    background: Some(p.surface_alt.into()),
                                    border: Border {
                                        color: p.border,
                                        width: 1.0,
                                        radius: 4.0.into(),
                                    },
                                    ..Default::default()
                                }
                            });
                            let semantics = enzui_core::semantics::Semantics::new(
                                enzui_core::semantics::Role::Image,
                            )
                            .label(alt_label);
                            crate::semantics_container::SemanticsContainer::leaf(
                                placeholder,
                                semantics,
                            )
                            .into()
                        }
                    }
                }
                EnzBlockKind::Heading => {
                    let level = block.level.max(1);
                    let size = heading_size_from(&heading_sizes, level);
                    // pick the pre-resolved heading font for this
                    // level. The `clamp(1, 6) - 1` idiom matches the sibling
                    // heading-ladder index in this file (registered under
                    // The cast widens a value already bounded to
                    // `0..=5` so it can neither truncate nor overrun the
                    // `[Font; 6]` array below.
                    let heading_idx = (level.clamp(1, 6) - 1) as usize;
                    let heading_f = heading_fonts[heading_idx];
                    if let Some(rich) = block.inline {
                        // Styled runs via the shared adapter. Document base metrics
                        // (heading weight + leading) pass through
                        // `view_routed_themed` (`rt.font(f)` / `rt.line_height(lh)`
                        // in `rich_text`). The inline path once passed `None, None`:
                        // body-weight headings, default leading — unlike the
                        // identical plain path below. `view_routed_themed` with
                        // `inline_themed!()`: toned spans resolve runtime-palette.
                        let el: Element<'a, Message> = rich.view_routed_ink::<Message>(
                            &inline_tokens,
                            size,
                            // the base font carries the resolved role
                            // family and the heading weight — not
                            // `iced::Font::default()` (SansSerif hardcode).
                            Some(heading_f),
                            Some(iced::widget::text::LineHeight::Relative(line_h)),
                            link_cb!(),
                            inline_ambient!(),
                            inline_themed!(),
                        );
                        el
                    } else {
                        let mut t = text(block.text).size(size).style(ink_style!());
                        t = t.line_height(iced::widget::text::LineHeight::Relative(line_h));
                        // heading family follows the role catalog.
                        t = t.font(heading_f);
                        t.into()
                    }
                }
                EnzBlockKind::Paragraph => {
                    let align_x = block.align_x;
                    let body: Element<'a, Message> = if let Some(rich) = block.inline {
                        // base line-height matches the plain path below.
                        // base font carries the resolved role family
                        // (`None` would paint as iced's
                        // default `SansSerif`, ignoring the theme).
                        let rich = rich.align_x(align_x);
                        rich.view_routed_ink::<Message>(
                            &inline_tokens,
                            base_body_size,
                            Some(body_font_base),
                            Some(iced::widget::text::LineHeight::Relative(line_h)),
                            link_cb!(),
                            inline_ambient!(),
                            inline_themed!(),
                        )
                    } else {
                        let mut t = text(block.text).size(base_body_size).style(ink_style!());
                        t = t.line_height(iced::widget::text::LineHeight::Relative(line_h));
                        // prose family follows the role catalog.
                        t = t.font(body_font_base);
                        t.align_x(align_x).into()
                    };
                    // The text node is not widened, so the box the container
                    // moves is the box the lines were shaped and aligned in.
                    match block_placement(align_x) {
                        Some(placement) => container(body)
                            .width(Length::Fill)
                            .align_x(placement)
                            .into(),
                        None => body,
                    }
                }
                EnzBlockKind::Blockquote => {
                    // Blockquote wraps its text in a rule + indent shell;
                    // both the plain and inline paths reuse that shell.
                    let body: Element<'a, Message> = if let Some(rich) = block.inline {
                        // Blockquote muted tone: give the rich tokens a muted default
                        // ink — toneless spans inherit it. Eager fallback is
                        // `Palette::dark()` (dead); the `muted_themed` resolver
                        // below re-resolves toned spans from the runtime palette
                        // via `theme_lookup::palette(t)`.
                        let muted_tokens = inline_text_tokens_muted(
                            ink_pin,
                            &crate::theme_lookup::palette(&iced::Theme::Dark),
                        );
                        let muted_themed = crate::rich_text::ThemedTokens::Custom(Box::new(
                            move |t: &iced::Theme| {
                                inline_text_tokens_muted(ink_pin, &crate::theme_lookup::palette(t))
                            },
                        ));
                        // Blockquote base line-height == plain path below; emphasis
                        // (true italic) rides the same base-font seam. Emphasised or
                        // not, the base carries the resolved ROLE family — the old
                        // non-emphasis `None` (invisible SansSerif fallback) and
                        // emphasis-pinned `Font::default()` (visible hardcode) were
                        // one defect in two clothes.
                        let base_font = Some(if block.emphasis {
                            italic_body_font
                        } else {
                            body_font_base
                        });
                        rich.view_routed_ink::<Message>(
                            &muted_tokens,
                            base_body_size,
                            base_font,
                            Some(iced::widget::text::LineHeight::Relative(line_h)),
                            link_cb!(),
                            inline_ambient_muted!(),
                            muted_themed,
                        )
                    } else {
                        let mut t = text(block.text)
                            .size(base_body_size)
                            .style(plain_ambient_muted!());
                        t = t.line_height(iced::widget::text::LineHeight::Relative(line_h));
                        // `emphasis` is true italic (`font-italic: true`) — once a
                        // no-op colour branch (`if emphasis { muted } else { muted }`).
                        // Either branch: family threads from the role catalog via
                        // `body_font_base` / `italic_body_font`, never
                        // `Font::default()`.
                        if block.emphasis {
                            t = t.font(italic_body_font);
                        } else {
                            t = t.font(body_font_base);
                        }
                        t.into()
                    };
                    // The accent rule holds `Fill` height to span exactly the row's
                    // content height — but row + wrapping container pin `Shrink`,
                    // so the block HUGS measured content instead of greedily
                    // filling the parent. Flex `Shrink`-cross turns on
                    // cross-compression; a `Fill`-height (cross-fluid) child then
                    // lays against accumulated content height, not parent max
                    // (the `layout/flex.rs` CROSS FLUID pass): rule matches prose
                    // height, neither side force-fills. The class: `Fill` under
                    // a hug-content region silently defeats the bound.
                    let rule =
                        container(Space::new().width(Length::Fixed(3.0)).height(Length::Fill))
                            .width(Length::Fixed(3.0))
                            .height(Length::Fill)
                            .style(move |t: &iced::Theme| {
                                let accent = accent_pin
                                    .unwrap_or_else(|| crate::theme_lookup::palette(t).accent);
                                iced::widget::container::Style {
                                    background: Some(accent.into()),
                                    border: Border {
                                        color: accent,
                                        width: 0.0,
                                        radius: 1.5.into(),
                                    },
                                    ..Default::default()
                                }
                            });
                    let padded = container(
                        row![
                            rule,
                            Space::new()
                                .width(Length::Fixed(indent_step / 2.0))
                                .height(Length::Fixed(1.0)),
                            body,
                        ]
                        .height(Length::Shrink),
                    )
                    .height(Length::Shrink)
                    // Affine indent — `indent_base + indent_step × level`. Unset
                    // `indent_base` resolves to `indent_step`: the old
                    // multiplicative `indent_step * (level + 1)` exactly. Set
                    // (an app base=4, step=12): level-0 takes the base, each
                    // deeper level one more step.
                    .padding(
                        iced::Padding::default()
                            .left(indent_base + indent_step * f32::from(block.level)),
                    );
                    padded.into()
                }
                EnzBlockKind::ListItem => {
                    // bullet marker follows the same prose family as
                    // the item body — asymmetric families for the two would
                    // be a jarring visual (and a pass-then-fail sabotage
                    // opportunity).
                    let bullet = text("•")
                        .size(base_body_size)
                        .style(ink_style!())
                        .font(body_font_base);
                    let body: Element<'a, Message> = if let Some(rich) = block.inline {
                        // base line-height matches the plain path below.
                        // base font carries the resolved role family.
                        rich.view_routed_ink::<Message>(
                            &inline_tokens,
                            base_body_size,
                            Some(body_font_base),
                            Some(iced::widget::text::LineHeight::Relative(line_h)),
                            link_cb!(),
                            inline_ambient!(),
                            inline_themed!(),
                        )
                    } else {
                        let mut b = text(block.text).size(base_body_size).style(ink_style!());
                        b = b.line_height(iced::widget::text::LineHeight::Relative(line_h));
                        b = b.font(body_font_base);
                        b.into()
                    };
                    let item_row = row![
                        bullet,
                        Space::new()
                            .width(Length::Fixed(8.0))
                            .height(Length::Fixed(1.0)),
                        body,
                    ]
                    .align_y(iced::alignment::Vertical::Top);
                    container(item_row)
                        .padding(
                            iced::Padding::default()
                                .left(indent_base + indent_step * f32::from(block.level)),
                        )
                        .into()
                }
                EnzBlockKind::OrderedItem => {
                    // reset at list boundaries (handled above), scope by
                    // nesting level, and continue from an explicit number.
                    let n = numbering.next(block.level, block.number);
                    // ordered marker follows the prose family, same
                    // reason as the ListItem bullet.
                    let marker = text(format!("{n}."))
                        .size(base_body_size)
                        .style(ink_style!())
                        .font(body_font_base);
                    let body: Element<'a, Message> = if let Some(rich) = block.inline {
                        // base line-height matches the plain path below.
                        // base font carries the resolved role family.
                        rich.view_routed_ink::<Message>(
                            &inline_tokens,
                            base_body_size,
                            Some(body_font_base),
                            Some(iced::widget::text::LineHeight::Relative(line_h)),
                            link_cb!(),
                            inline_ambient!(),
                            inline_themed!(),
                        )
                    } else {
                        let mut b = text(block.text).size(base_body_size).style(ink_style!());
                        b = b.line_height(iced::widget::text::LineHeight::Relative(line_h));
                        b = b.font(body_font_base);
                        b.into()
                    };
                    let item_row = row![
                        marker,
                        Space::new()
                            .width(Length::Fixed(8.0))
                            .height(Length::Fixed(1.0)),
                        body,
                    ]
                    .align_y(iced::alignment::Vertical::Top);
                    container(item_row)
                        .padding(
                            iced::Padding::default()
                                .left(indent_base + indent_step * f32::from(block.level)),
                        )
                        .into()
                }
                EnzBlockKind::Code => {
                    let mut body = text(block.text)
                        .size(enzui_theme::text_scale::ui_px(12.0))
                        .style(ink_style!());
                    body = body.line_height(iced::widget::text::LineHeight::Relative(line_h));
                    // Upright-face fallback for generic `Family::Monospace`,
                    // installed at first use (no app opt-in). The family comes
                    // from `committed_code_font()` ([`Monospace`](FontRole::Monospace)):
                    // themed monospace (say `Fira Code`) reaches this arm
                    // untouched, while the generic default stays byte-identical
                    // to `monospace_font()`.
                    crate::monospace_fallback::ensure_monospace_fallback();
                    body = body.font(code_font);
                    container(body)
                        .width(Length::Fill)
                        .padding(
                            iced::Padding::default()
                                .left(12.0)
                                .right(12.0)
                                .top(8.0)
                                .bottom(8.0),
                        )
                        .style(move |t: &iced::Theme| {
                            let p = crate::theme_lookup::palette(t);
                            iced::widget::container::Style {
                                background: Some(p.surface_alt.into()),
                                border: Border {
                                    color: p.border,
                                    width: 1.0,
                                    radius: 6.0.into(),
                                },
                                ..Default::default()
                            }
                        })
                        .into()
                }
            };
            col = col.push(child);
        }
        col.into()
    }
}

impl<'a, L, Message: 'a> From<EnzBlockList<'a, L, Message>> for Element<'a, Message>
where
    L: Clone + PartialEq + 'static,
{
    fn from(w: EnzBlockList<'a, L, Message>) -> Self {
        w.into_element()
    }
}

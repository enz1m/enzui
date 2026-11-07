//! The immutable rich-text model: [`RichText`], [`RichSpan`],
//! [`SpanStyle`], [`StyledRange`]/[`RangeError`], and the deterministic
//! adapters that turn a document into iced spans and into a rendered element.
//!
//! This is the data + builder + view core. `to_iced_spans` resolves semantic
//! tones (via [`super::tone`]) and the `view*` methods wrap the built
//! `rich_text` in [`LinkSemantics`](super::link::LinkSemantics) for link
//! accessibility. The `SPAN_CONVERSIONS` counter is incremented by
//! `to_iced_spans_inner`, so it lives with that code.

#![allow(
    clippy::string_slice,
    reason = "ranges are checked against char boundaries before use"
)]

use super::link::{LinkHandlers, LinkSemantics, RichMetrics, link_activations};
use super::presentation::{LinkPresentationTokens, LinkUnderline, apply_presentation};
use super::tone::{CODE_FILL_KEY, ThemedTokens, code_fill, resolve_tone_key, tone_key};
use crate::enz_text::{TextTokens, TextTone};
use iced::advanced::text::Span;
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::widget::rich_text;
use iced::{Color, Element, Event, Font, Length, Rectangle, Size, Vector};

/// Corner radius of an inline `code` chip's fill, in logical pixels.
const CODE_CHIP_RADIUS: f32 = 3.0;
/// How far an inline `code` chip's fill reaches past its glyphs on each side
/// (the `xs` rung).
const CODE_CHIP_PADDING_X: f32 = 4.0;

// ---------------------------------------------------------------------------
// Instrumentation: span-conversion counter
// ---------------------------------------------------------------------------

/// Instrumentation: how many times `RichText::to_iced_spans_inner` has
/// converted a document into a fresh `Vec<Span>`.
///
/// This is the single choke point for span *construction* on this pipeline
/// ([`RichText::to_iced_spans`] and [`RichText::view_routed_themed`] both funnel
/// through it), and span construction is the dominant per-document cost: every
/// run's `String` is cloned. The property bought is that a **theme switch
/// re-resolves colour over an already-built span vector** rather than rebuilding
/// it — so the assertable fact is "this counter did not move across a theme
/// switch".
///
/// A counter is required because the property is about *work not happening*.
/// Rendered output is identical either way (a rebuild produces the same pixels),
/// and an allocation probe outside production cannot see whether production
/// entered this function at all. Counting here makes "production did not rebuild
/// spans" an assertion rather than a code-reading exercise — the same reasoning
/// as `text_area::BUFFER_CHAR_SCANS`.
#[doc(hidden)]
pub static SPAN_CONVERSIONS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Read [`SPAN_CONVERSIONS`].
#[doc(hidden)]
#[must_use]
pub fn span_conversions() -> usize {
    SPAN_CONVERSIONS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Reset [`SPAN_CONVERSIONS`] to zero and return its previous value.
#[doc(hidden)]
pub fn reset_span_conversions() -> usize {
    SPAN_CONVERSIONS.swap(0, std::sync::atomic::Ordering::Relaxed)
}

/// Inline style of a single [`RichSpan`]. `Default` is unstyled body text.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SpanStyle {
    /// Render the run bold.
    pub bold: bool,
    /// Render the run italic.
    pub italic: bool,
    /// Monospace inline code run.
    pub code: bool,
    /// Draw an underline beneath the run.
    pub underline: bool,
    /// Draw a strikethrough line through the run.
    pub strikethrough: bool,
    /// Semantic colour tone, resolved against the runtime palette at render
    /// time. `None` inherits the ambient text colour.
    pub tone: Option<TextTone>,
    /// An author-chosen ink for the run.
    ///
    /// A tone is a role and follows the runtime theme; this is a value the
    /// document carries, so a theme switch must not repaint it. When both are
    /// set the authored value wins.
    pub color: Option<Color>,
    /// A highlight (marker) background behind the run.
    pub highlight: Option<Color>,
    /// Authored family; `None` inherits the surrounding text family.
    pub font: Option<iced::font::Family>,
    /// Authored size in logical pixels; `None` inherits the surrounding size.
    pub text_size: Option<f32>,
}

impl SpanStyle {
    /// Whether two styles are visually identical (coalescing predicate).
    fn same(self, other: SpanStyle) -> bool {
        self == other
    }
}

/// One styled run of text, optionally a link carrying payload `L`.
#[derive(Debug, Clone, PartialEq)]
pub struct RichSpan<L = ()> {
    /// The run's text content.
    pub text: String,
    /// The inline style applied to this run.
    pub style: SpanStyle,
    /// Link payload. `Some(_)` marks the run as an interactive link.
    pub link: Option<L>,
}

impl<L> RichSpan<L> {
    /// A plain, unstyled run.
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: SpanStyle::default(),
            link: None,
        }
    }

    /// A styled run.
    pub fn styled(text: impl Into<String>, style: SpanStyle) -> Self {
        Self {
            text: text.into(),
            style,
            link: None,
        }
    }

    /// A link run carrying `payload`.
    pub fn link(text: impl Into<String>, payload: L) -> Self {
        Self {
            text: text.into(),
            style: SpanStyle::default(),
            link: Some(payload),
        }
    }
}

/// An immutable rich-text document: an ordered run of [`RichSpan`]s.
///
/// Paragraphs wrap at words, falling back to glyph boundaries for a word that
/// cannot fit the available column. Link hit-testing uses the same policy.
#[derive(Debug, Clone, PartialEq, Default)]
#[must_use]
pub struct RichText<L = ()> {
    spans: Vec<RichSpan<L>>,
    align_x: iced::advanced::text::Alignment,
    link_presentation: Option<LinkPresentationTokens>,
}

/// A styled byte range over a backing string, for [`RichText::from_ranges`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StyledRange {
    /// Inclusive start byte offset.
    pub start: usize,
    /// Exclusive end byte offset.
    pub end: usize,
    /// Inline style applied to this byte range.
    pub style: SpanStyle,
}

/// Error from constructing a [`RichText`] over UTF-8 byte ranges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RangeError {
    /// A range boundary did not fall on a UTF-8 char boundary.
    NotCharBoundary(usize),
    /// A range extended past the end of the backing string.
    OutOfBounds {
        /// The offending range's exclusive end offset.
        end: usize,
        /// The backing string's byte length.
        len: usize,
    },
    /// Two ranges overlapped (start < previous end).
    Overlap {
        /// The byte offset at which the overlap was detected.
        at: usize,
    },
    /// `start > end`.
    Inverted {
        /// The inverted range's start offset.
        start: usize,
        /// The inverted range's end offset (less than `start`).
        end: usize,
    },
}

impl<L: Clone + PartialEq> RichText<L> {
    /// Construct from an explicit list of spans (already ordered).
    pub fn new(spans: Vec<RichSpan<L>>) -> Self {
        Self {
            spans,
            align_x: iced::advanced::text::Alignment::Default,
            link_presentation: None,
        }
    }

    /// Override link tones and decoration while retaining authored colours, fonts and actions.
    pub fn link_presentation(mut self, tokens: LinkPresentationTokens) -> Self {
        self.link_presentation = Some(tokens);
        self
    }

    /// How this paragraph's lines sit across the box they are shaped in.
    ///
    /// The renderer's own alignment: it moves the shaped lines, so the ink and
    /// the geometry a selection reads move together. It changes nothing while
    /// the box shrinks to the widest line, so a caller that wants a short line
    /// moved must also give the paragraph a wider box — that is what
    /// [`crate::block_list`] does around an aligned block. The default leaves
    /// the edge to the text direction.
    pub fn align_x(mut self, align_x: impl Into<iced::advanced::text::Alignment>) -> Self {
        self.align_x = align_x.into();
        self
    }

    /// The alignment this paragraph will be shaped with.
    pub fn alignment(&self) -> iced::advanced::text::Alignment {
        self.align_x
    }

    /// The spans, in order.
    pub fn spans(&self) -> &[RichSpan<L>] {
        &self.spans
    }

    /// Whether the document has no text.
    pub fn is_empty(&self) -> bool {
        self.spans.iter().all(|s| s.text.is_empty())
    }

    /// Concatenate all span texts into a single plain-text string.
    ///
    /// This is the full text content of the document: all formatting is
    /// stripped, and link payloads are discarded.
    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        for s in &self.spans {
            out.push_str(&s.text);
        }
        out
    }

    /// Concatenate span text while excluding linked runs.
    pub fn prose_only_text(&self) -> String {
        let mut out = String::new();
        for s in &self.spans {
            if s.link.is_none() {
                out.push_str(&s.text);
            }
        }
        out
    }

    /// Apply `ranges` to `base`, producing an ordered span list where the gaps
    /// between styled ranges become default runs.
    ///
    /// Validates that every boundary lands on a UTF-8 char boundary, that
    /// ranges are in-bounds and non-inverted, and that they do not overlap
    /// (they need not be compatibility sorted — they are sorted here and overlap is
    /// checked deterministically). This is the UTF-8-safety acceptance
    /// requirement: malformed ranges are rejected rather than silently
    /// slicing mid-codepoint.
    ///
    /// # Errors
    ///
    /// A [`RangeError`] for an inverted range, one past the end, one off a char boundary, or
    /// overlapping ranges.
    pub fn from_ranges(base: &str, mut ranges: Vec<StyledRange>) -> Result<Self, RangeError> {
        ranges.sort_by_key(|r| (r.start, r.end));

        let len = base.len();
        let mut spans: Vec<RichSpan<L>> = Vec::new();
        let mut cursor = 0usize;

        for r in &ranges {
            if r.start > r.end {
                return Err(RangeError::Inverted {
                    start: r.start,
                    end: r.end,
                });
            }
            if r.end > len {
                return Err(RangeError::OutOfBounds { end: r.end, len });
            }
            if !base.is_char_boundary(r.start) {
                return Err(RangeError::NotCharBoundary(r.start));
            }
            if !base.is_char_boundary(r.end) {
                return Err(RangeError::NotCharBoundary(r.end));
            }
            if r.start < cursor {
                return Err(RangeError::Overlap { at: r.start });
            }
            // Gap before this styled range → default run.
            if r.start > cursor {
                spans.push(RichSpan::plain(&base[cursor..r.start]));
            }
            if r.end > r.start {
                spans.push(RichSpan::styled(&base[r.start..r.end], r.style));
            }
            cursor = r.end;
        }
        // Trailing default run.
        if cursor < len {
            spans.push(RichSpan::plain(&base[cursor..]));
        }

        Ok(Self {
            spans,
            align_x: iced::advanced::text::Alignment::Default,
            link_presentation: None,
        }
        .coalesced())
    }

    /// Return a copy with adjacent runs of identical style and no link
    /// merged, and empty runs dropped. Deterministic: equal inputs → equal
    /// output span lists.
    pub fn coalesced(&self) -> Self {
        let mut out: Vec<RichSpan<L>> = Vec::with_capacity(self.spans.len());
        for span in &self.spans {
            if span.text.is_empty() {
                continue;
            }
            if let Some(last) = out.last_mut()
                && last.link.is_none()
                && span.link.is_none()
                && last.style.same(span.style)
            {
                last.text.push_str(&span.text);
                continue;
            }
            out.push(span.clone());
        }
        Self {
            spans: out,
            align_x: self.align_x,
            link_presentation: self.link_presentation,
        }
    }

    /// Resolve resting run colours and decoration against the supplied [`TextTokens`].
    /// The returned spans contain a fixed colour snapshot. Routed views install
    /// live theme and interaction resolvers; raw iced consumers must supply their own.
    pub fn to_iced_spans<'a>(&self, tokens: &TextTokens) -> Vec<Span<'a, L>> {
        self.to_iced_spans_inner(tokens, false, self.link_presentation)
    }

    /// [`to_iced_spans`] with the option to leave [`TextTone::Default`] runs
    /// uncoloured.
    ///
    /// `Default` is not a colour, it is the name for *ambient* ink — the same
    /// thing an untoned run asks for. Baking it as a concrete `Span::color`
    /// therefore pins the one tone that by definition should follow whatever the
    /// surrounding widget resolves. Leaving `Span::color` as `None` lets iced
    /// fall through to the widget-level style closure
    /// (`iced/widget/src/text/rich.rs`), which
    /// [`view_routed_with_ambient`] fills with a live-`&Theme` resolver.
    ///
    /// Only used by [`view_routed_with_ambient`], and only when it actually has
    /// an ambient resolver to fall through *to*.
    ///
    /// Every other tone is still resolved eagerly here — but since the
    /// baked colour is only a fallback: the span is also tagged with
    /// [`tone_key`], and the view methods install [`resolve_tone_key`], so the
    /// tone is re-resolved against the live `&Theme` at draw time. The eager
    /// value governs only when no resolver is installed (e.g. a bare
    /// [`to_iced_spans`] call).
    ///
    /// [`tone_key`]: crate::rich_text::tone_key
    /// [`resolve_tone_key`]: crate::rich_text::resolve_tone_key
    ///
    /// [`to_iced_spans`]: RichText::to_iced_spans
    /// [`view_routed_with_ambient`]: RichText::view_routed_with_ambient
    fn to_iced_spans_inner<'a>(
        &self,
        tokens: &TextTokens,
        default_tone_is_ambient: bool,
        presentation: Option<LinkPresentationTokens>,
    ) -> Vec<Span<'a, L>> {
        // the single choke point for span construction on this pipeline.
        // Counted so a test can assert that a theme switch did *not* rebuild
        // spans (the property) — see [`SPAN_CONVERSIONS`].
        SPAN_CONVERSIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.spans
            .iter()
            .map(|s| {
                let mut span = Span::new(s.text.clone());
                if let Some(tone) = s.style.tone
                    && !(default_tone_is_ambient && tone == TextTone::Default)
                {
                    // bake the eager colour *and* tag the span with the
                    // tone it wants. The baked colour is the fallback for any
                    // caller that installs no resolver (`to_iced_spans`, and
                    // every compatibility call site), so this stays additive; when a
                    // resolver *is* installed the live-theme answer wins
                    // (`iced/widget/src/text/rich.rs`).
                    span = span.color(tokens.color_for(tone)).color_key(tone_key(tone));
                }
                // Inline flags compose onto the authored family; code retains its monospace contract.
                if let Some(family) = s.style.font {
                    span = span.font(Font {
                        family,
                        ..Font::DEFAULT
                    });
                }
                if let Some(size) = s
                    .style
                    .text_size
                    .filter(|size| size.is_finite() && *size > 0.0)
                {
                    span = span.size(size);
                }
                if s.style.bold {
                    let base = span.font.unwrap_or_default();
                    span = span.font(Font {
                        weight: iced::font::Weight::Bold,
                        ..base
                    });
                }
                if s.style.italic {
                    let base = span.font.unwrap_or_default();
                    span = span.font(Font {
                        style: iced::font::Style::Italic,
                        ..base
                    });
                }
                if s.style.code {
                    // ensure the upright-face monospace fallback is
                    // installed at first use so inline `code` spans render
                    // upright without any app opt-in.
                    crate::monospace_fallback::ensure_monospace_fallback();
                    let base = span.font.unwrap_or_default();
                    span = span.font(Font {
                        family: iced::font::Family::Monospace,
                        ..base
                    });
                    // A code chip: a faint fill that follows the live theme,
                    // padded past the glyphs (upstream span padding widens
                    // only the fill, never the text's layout). Body ink only:
                    // muted and subtle ink sit barely above 4.5:1 on the light
                    // backdrop and drop below it on any chip, and an authored
                    // ink's contrast is unknown. An authored highlight below
                    // replaces the chip.
                    let body_ink = s.style.tone.is_none_or(|tone| tone == TextTone::Default)
                        && s.style.color.is_none();
                    if body_ink {
                        span = span
                            .background(code_fill(tokens))
                            .highlight_key(CODE_FILL_KEY)
                            .border(iced::Border::default().rounded(CODE_CHIP_RADIUS))
                            .padding([0.0, CODE_CHIP_PADDING_X]);
                    }
                }
                if s.style.underline {
                    span = span.underline(true);
                }
                if s.style.strikethrough {
                    span = span.strikethrough(true);
                }
                // Span::color preserves the tone key; clear it so live-theme
                // resolution cannot overwrite authored ink.
                if let Some(color) = s.style.color {
                    span = span.color(color).color_key_maybe(None);
                }
                if let Some(bg) = s.style.highlight {
                    span = span.background(bg);
                    span.highlight_key = None;
                }
                if let Some(link) = &s.link {
                    span = span.link(link.clone());
                    if s.style.tone.is_none() && s.style.color.is_none() {
                        span = span
                            .color(tokens.color_for(TextTone::Accent))
                            .color_key(tone_key(TextTone::Accent));
                    }
                    if let Some(presentation) = presentation {
                        if s.style.color.is_none() {
                            span = span
                                .color(tokens.color_for(presentation.rest))
                                .color_key(tone_key(presentation.rest));
                        }
                        span = span.underline(
                            s.style.underline || presentation.underline == LinkUnderline::Always,
                        );
                    } else {
                        span = span.underline(true);
                    }
                }
                span
            })
            .collect()
    }
}

impl<L> RichText<L>
where
    L: Clone + PartialEq + 'static,
{
    /// Build a ready-to-use `rich_text` element. `on_link` maps a clicked
    /// link payload to a message; pass a no-op-producing closure when the
    /// document has no links.
    pub fn view<'a, Message: 'a>(
        &self,
        tokens: &TextTokens,
        size: f32,
        on_link: impl Fn(L) -> Message + 'a,
    ) -> Element<'a, Message>
    where
        L: 'a,
    {
        let spans = self.to_iced_spans(tokens);
        // Share the link callback between the child `rich_text` (pointer path)
        // and the per-link keyboard activations, so both fire the identical
        // `on_link(payload)` call.
        let on_link = std::rc::Rc::new(on_link);
        let activations = link_activations(&spans, &on_link);
        let child_cb = std::rc::Rc::clone(&on_link);
        let rich = rich_text(spans.clone())
            .wrapping(iced::advanced::text::Wrapping::WordOrGlyph)
            .size(size)
            .resolve_color(resolve_tone_key)
            .on_link_click(move |payload| child_cb(payload));
        let (rich, link_focus) = apply_presentation(
            rich,
            self.link_presentation,
            self.spans
                .iter()
                .map(|span| span.style.color.is_some())
                .collect(),
            std::rc::Rc::new(ThemedTokens::FromRuntimePalette),
        );
        let element: Element<'a, Message> = rich.into();
        // publish a Role::Link AT node per link span and make each link a
        // keyboard Tab stop.
        LinkSemantics::wrap(
            element,
            &spans,
            RichMetrics {
                size,
                font: None,
                line_height: None,
                align_x: iced::advanced::text::Alignment::Default,
            },
            activations,
            None,
            link_focus,
        )
    }

    /// Build a `rich_text` element carrying an explicit base font, base line
    /// height, and an optional link callback.
    ///
    /// This is the routed, panic-free entry point. Unlike [`view`],
    /// which forces a mandatory link closure (callers pass an `unreachable!`
    /// for link-free documents), here `on_link` is `Option`: when it is
    /// `None` the widget installs no `on_link_click` handler at all, so a
    /// click on a legal [`RichSpan::link`] is a silent no-op rather than a
    /// panic (iced's `rich_text::update` early-returns when no handler is set,
    /// see `iced/widget/src/text/rich.rs`).
    ///
    /// `font` and `line_height` set the document's *base* metrics — this is
    /// how the reading pane routes a heading's weight and a paragraph's line
    /// height into the rich path (individual [`Span`]s carry no line height,
    /// so per-run weight still must be baked into [`SpanStyle`]).
    ///
    /// [`view`]: RichText::view
    pub fn view_routed<'a, Message: 'a>(
        &self,
        tokens: &TextTokens,
        size: f32,
        font: Option<Font>,
        line_height: Option<iced::widget::text::LineHeight>,
        on_link: Option<impl Fn(L) -> Message + 'a>,
    ) -> Element<'a, Message>
    where
        L: 'a,
    {
        self.view_routed_with_ambient(
            tokens,
            size,
            font,
            line_height,
            on_link,
            None::<fn(&iced::Theme) -> Color>,
        )
    }

    /// [`view_routed`] plus a theme-reactive ambient ink resolver.
    ///
    /// # What `ambient` is for
    ///
    /// A span carrying no [`SpanStyle::tone`] is emitted with `Span::color ==
    /// None`, and iced then paints it with
    /// `span.color.or(style.color).unwrap_or(defaults.text_color)`
    /// (`iced/widget/src/text/rich.rs`). `style.color` is the
    /// *widget-level* class. Without it, a caller's ambient ink (e.g.
    /// `EnzBlockList::text_color`, or its `text_tone`) would reach
    /// [`to_iced_spans`] as `tokens.default_color` and be **silently
    /// dropped**, because `default_color` is only read by `color_for(tone)`,
    /// which untoned spans never call.
    ///
    /// Installing `ambient` as the widget-level style closure solves both
    /// halves at once: the ink arrives, and it arrives **per render from the live
    /// `&Theme`**, so it follows a light/dark/custom switch without a rebuild.
    ///
    /// [`TextTone::Default`] counts as ambient and is routed through `ambient`
    /// too (see `to_iced_spans_inner`) — `Default` names the surrounding body
    /// ink, so pinning it is the same defect one level down.
    ///
    /// # Explicitly toned runs follow the theme too
    ///
    /// A span with an explicit **non-`Default`** [`SpanStyle::tone`] cannot
    /// rely on `ambient`: `Span::color` is a baked `Option<Color>`, and iced
    /// only consults `style.color` when `span.color` is `None`. Resolved
    /// eagerly, a `Muted` run built under the dark theme would keep the dark
    /// palette's ink on a light page, far below the 4.5:1 AA floor.
    ///
    /// The enzICED fork's per-span hook (`Span::color_key` +
    /// `Rich::resolve_color`, `iced/core/src/text.rs` and
    /// `iced/widget/src/text/rich.rs`) closes that gap:
    /// `to_iced_spans_inner` tags every toned span with [`tone_key`], and the
    /// element installs [`resolve_tone_key`], which re-resolves the tone
    /// against the live `&Theme`'s palette on every draw. No rebuild is needed
    /// on a theme switch.
    ///
    /// The eager `tokens` colour is still baked underneath as the fallback,
    /// so this is additive: [`to_iced_spans`] callers that install no resolver,
    /// and any span carrying a key the resolver declines, behave exactly as
    /// before.
    ///
    /// [`tone_key`]: crate::rich_text::tone_key
    /// [`resolve_tone_key`]: crate::rich_text::resolve_tone_key
    ///
    /// `to_iced_spans_inner`: RichText::to_iced_spans_inner
    ///
    /// [`view_routed`]: RichText::view_routed
    /// [`to_iced_spans`]: RichText::to_iced_spans
    pub fn view_routed_with_ambient<'a, Message: 'a>(
        &self,
        tokens: &TextTokens,
        size: f32,
        font: Option<Font>,
        line_height: Option<iced::widget::text::LineHeight>,
        on_link: Option<impl Fn(L) -> Message + 'a>,
        ambient: Option<impl Fn(&iced::Theme) -> Color + 'a>,
    ) -> Element<'a, Message>
    where
        L: 'a,
    {
        self.view_routed_themed(
            tokens,
            size,
            font,
            line_height,
            on_link,
            ambient,
            ThemedTokens::FromRuntimePalette,
        )
    }

    /// [`view_routed_with_ambient`] with an explicit token source for the
    /// theme-reactive tone resolution.
    ///
    /// `tokens` still supplies the colour baked into each toned span, which is
    /// what a caller with no resolver sees. `themed` decides what those tones
    /// re-resolve to per draw:
    ///
    /// * [`ThemedTokens::FromRuntimePalette`] — follow the runtime theme. This
    ///   is what fixes the AA defect for `block_list` (a `Muted` run went from
    ///   1.97:1 to 4.94:1 under the light theme).
    /// * [`ThemedTokens::Fixed`] — keep `tokens` on every theme. For ink that
    ///   is deliberately theme-independent, such as `reading_pane`'s paper
    ///   stocks, whose sepia/cream ink is reader-chosen paper rather than app
    ///   chrome (see `reading_colors` in `reading_pane`).
    ///
    /// [`view_routed_with_ambient`]: RichText::view_routed_with_ambient
    ///
    /// # Prefer [`view_routed_ink`] for the ambient channel
    ///
    /// The `ambient` parameter here is an opaque closure: this adapter
    /// cannot see which rung of the ambient-ink ladder produced the colour, so
    /// every caller has to build the pin → resolver → tone → fallback precedence
    /// itself before calling in. That is exactly the duplication removed
    /// from `block_list` and `streaming_text`. [`view_routed_ink`] takes a
    /// [`BoundInk`] instead and resolves the shared ladder here, once.
    ///
    /// Retained because `code_block` and `reading_pane` supply ambient inks that
    /// are genuinely not ladders (a single-rung pin-or-runtime closure, and
    /// the palette-invariant paper stocks respectively — see the scope note),
    /// and because `ambient: None` has a meaning no ladder can express: inherit
    /// the ancestor container's ink.
    ///
    /// [`view_routed_ink`]: RichText::view_routed_ink
    /// [`BoundInk`]: crate::ambient_ink::BoundInk
    #[allow(
        clippy::too_many_arguments,
        reason = "the view's tokens, routing and handlers"
    )]
    pub fn view_routed_themed<'a, Message: 'a>(
        &self,
        tokens: &TextTokens,
        size: f32,
        font: Option<Font>,
        line_height: Option<iced::widget::text::LineHeight>,
        on_link: Option<impl Fn(L) -> Message + 'a>,
        ambient: Option<impl Fn(&iced::Theme) -> Color + 'a>,
        themed: ThemedTokens<'a>,
    ) -> Element<'a, Message>
    where
        L: 'a,
    {
        let mut handlers = LinkHandlers::new();
        if let Some(on_link) = on_link {
            handlers = handlers.on_click(on_link);
        }
        self.view_routed_with_handlers(tokens, size, font, line_height, &handlers, ambient, themed)
    }

    /// Render with optional primary and secondary link actions and theme-reactive ink.
    ///
    /// Handlers receive the original typed payload, even when labels repeat or
    /// links wrap across lines. A handled secondary press is captured before
    /// an enclosing selection scope can open its own context menu.
    #[allow(
        clippy::too_many_arguments,
        reason = "the view's tokens, routing and handlers"
    )]
    pub fn view_routed_with_handlers<'a, Message: 'a>(
        &self,
        tokens: &TextTokens,
        size: f32,
        font: Option<Font>,
        line_height: Option<iced::widget::text::LineHeight>,
        handlers: &LinkHandlers<'a, L, Message>,
        ambient: Option<impl Fn(&iced::Theme) -> Color + 'a>,
        themed: ThemedTokens<'a>,
    ) -> Element<'a, Message>
    where
        L: 'a,
    {
        let presentation = self.link_presentation.or(handlers.presentation);
        let spans = self.to_iced_spans_inner(tokens, ambient.is_some(), presentation);
        // Unconditional: the model's default is the renderer's own, so a
        // paragraph that never set an alignment is unaffected.
        let mut rt = rich_text(spans.clone())
            .wrapping(iced::advanced::text::Wrapping::WordOrGlyph)
            .size(size)
            .align_x(self.align_x);
        if let Some(f) = font {
            rt = rt.font(f);
        }
        if let Some(lh) = line_height {
            rt = rt.line_height(lh);
        }
        // resolve every toned span against the live theme, through the
        // caller's token source. Installed unconditionally — a span with no
        // `color_key` is untouched, so this cannot change the appearance of
        // untoned text.
        let themed = std::rc::Rc::new(themed);
        let color_tokens = std::rc::Rc::clone(&themed);
        rt = rt.resolve_color(move |theme: &iced::Theme, key| color_tokens.resolve(theme, key));
        let activations = match &handlers.click {
            Some(cb) => {
                let acts = link_activations(&spans, cb);
                let child_cb = std::rc::Rc::clone(cb);
                rt = rt.on_link_click(move |payload| child_cb(payload));
                acts
            }
            None => Vec::new(),
        };
        if let Some(ink) = ambient {
            rt = rt.style(move |theme: &iced::Theme| iced::widget::text::Style {
                color: Some(ink(theme)),
            });
        }
        let (rt, link_focus) = apply_presentation(
            rt,
            presentation,
            self.spans
                .iter()
                .map(|span| span.style.color.is_some())
                .collect(),
            themed,
        );
        LinkSemantics::wrap(
            rt.into(),
            &spans,
            RichMetrics {
                size,
                font,
                line_height,
                align_x: self.align_x,
            },
            activations,
            handlers.context.clone(),
            link_focus,
        )
    }

    /// [`view_routed_themed`] with the ambient channel driven by the shared
    /// ambient-ink ladder instead of an opaque caller closure.
    ///
    /// # What this replaces
    ///
    /// `rich_text` was the third widget in the crate carrying its own ambient
    /// mechanism after `block_list` and `streaming_text` were folded onto
    /// [`crate::ambient_ink`]. Its mechanism was the
    /// `Option<impl Fn(&iced::Theme) -> Color>` of [`view_routed_themed`]: a
    /// single opaque rung, which pushed the entire precedence decision back onto
    /// every caller. `block_list` consequently built the ladder itself, wrapped
    /// it in an `Rc`, and cloned one closure per inline block through two
    /// macros. Routing the ladder in *as a ladder* means the precedence rule has
    /// one definition on this path too.
    ///
    /// # `ink: Option<BoundInk>` — and why it is not `AmbientInk`
    ///
    /// `None` here is not an empty ladder. It installs no widget-level
    /// style, so iced paints the run with `defaults.text_color`, i.e. the ink
    /// inherited from the ancestor container — measured green inside a
    /// `container` whose style sets `text_color`, where an empty ladder resolves
    /// [`Fallback::Text`] (`palette.text`). Both are legitimate and they differ,
    /// so the distinction is carried by the `Option` rather than by
    /// [`AmbientInk::is_set`], which reports `false` for both. That is why the
    /// parameter is `Option<BoundInk>` and why [`BoundInk`] pairs the ladder with
    /// its rung-4 fallback: the caller's rungs and the surface's fallback arrive
    /// from different places and both have to cross this boundary.
    ///
    /// [`view_routed_themed`]: RichText::view_routed_themed
    /// [`BoundInk`]: crate::ambient_ink::BoundInk
    /// [`Fallback::Text`]: crate::ambient_ink::Fallback::Text
    /// [`AmbientInk::is_set`]: crate::ambient_ink::AmbientInk::is_set
    #[allow(
        clippy::too_many_arguments,
        reason = "the view's tokens, routing and handlers"
    )]
    pub fn view_routed_ink<'a, Message: 'a>(
        &self,
        tokens: &TextTokens,
        size: f32,
        font: Option<Font>,
        line_height: Option<iced::widget::text::LineHeight>,
        on_link: Option<impl Fn(L) -> Message + 'a>,
        ink: Option<crate::ambient_ink::BoundInk>,
        themed: ThemedTokens<'a>,
    ) -> Element<'a, Message>
    where
        L: 'a,
    {
        self.view_routed_themed(
            tokens,
            size,
            font,
            line_height,
            on_link,
            ink.map(|ink| move |t: &iced::Theme| ink.resolve_with_theme(t)),
            themed,
        )
    }
}

/// Transparent compatibility wrapper that forwards native shaped-text traversal.
pub struct ProseSemantics<'a, Message> {
    child: Element<'a, Message>,
}

impl<'a, Message: 'a> ProseSemantics<'a, Message> {
    /// Wrap nonblank text without adding a second selection or accessibility region.
    /// The native paragraph supplies the text; `text` only decides whether to add the wrapper.
    pub fn wrap(child: Element<'a, Message>, text: String) -> Element<'a, Message> {
        if text.trim().is_empty() {
            return child;
        }
        Element::new(ProseSemantics { child })
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for ProseSemantics<'_, Message> {
    fn tag(&self) -> tree::Tag {
        self.child.as_widget().tag()
    }

    fn state(&self) -> tree::State {
        self.child.as_widget().state()
    }

    fn diff(&mut self, tree: &mut Tree) {
        self.child.as_widget_mut().diff(tree);
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.child.as_widget_mut().layout(tree, renderer, limits);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        // A compatibility text node would duplicate the native paragraph's selection region.
        self.child
            .as_widget_mut()
            .operate(tree, layout, viewport, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.child
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.child
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.child
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
}

impl RichText<()> {
    /// Re-key link payloads from `()` to each link span's own text.
    ///
    /// A `RichText<()>` records *that* a run is a link but nothing about
    /// *which* one — the `()` payload is information-free. This lifts the
    /// link's own text into the payload so a routed callback can identify the
    /// clicked link in practice.
    ///
    /// Non-link runs are copied unchanged (payload stays `None`).
    ///
    /// # No widget in this crate uses this
    ///
    /// `EnzBlockList` and `EnzReadingPane` are generic over the payload and
    /// deliver it verbatim, so neither needs to reconstruct a destination from
    /// a label. This is a public convenience for an application that genuinely wants text-keyed links,
    /// but prefer attaching a typed payload with
    /// [`RichSpan::link(text, payload)`](RichSpan::link): two runs with the same
    /// visible text collapse to the same payload here, which is exactly the
    /// ambiguity described above.
    pub fn relink_by_text(&self) -> RichText<String> {
        RichText::new(
            self.spans()
                .iter()
                .map(|s| RichSpan::<String> {
                    text: s.text.clone(),
                    style: s.style,
                    link: s.link.as_ref().map(|()| s.text.clone()),
                })
                .collect(),
        )
    }
}

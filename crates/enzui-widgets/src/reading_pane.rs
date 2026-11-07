//! Centred long-form prose with a capped width, paper themes and typed inline links.
//!
//! Plain blockquotes use muted italic text; inline blockquotes keep an upright
//! base and take italics from individual runs. App paper follows the live
//! palette, while Light, Sepia and Dark paper have fixed colours.
//! Optional selection callbacks report paragraph indices from explicit-line
//! height estimates, without accounting for wrapping. Highlight bands cover
//! whole host-selected paragraphs; the host owns selection state and copying.

use enzui_theme::{EnzTheme, FontRole, Palette};
use iced::font::Family;
use iced::widget::{column, container, mouse_area, text};
use iced::{Color, Element, Length};

use crate::enz_text::TextTokens;
use crate::revision_text::{TextSnapshot, enz_revision_text};

/// Kind of block in a reading column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EnzReadingKind {
    /// Heading with level-dependent size and weight.
    Heading,
    /// Body text; the default block kind.
    #[default]
    Paragraph,
    /// Muted quotation; italic on the plain text path.
    Blockquote,
}

impl EnzReadingKind {
    /// Parse `"heading"` or `"blockquote"`; other names select a body paragraph.
    pub fn from_name(s: &str) -> Self {
        match s {
            "heading" => Self::Heading,
            "blockquote" => Self::Blockquote,
            _ => Self::Paragraph,
        }
    }
}

/// Owned paragraph with optional inline runs and typed link payloads.
/// The default payload is `()`; inline runs supersede the plain text.
#[derive(Debug, Clone)]
pub struct EnzReadingParagraph<L = ()> {
    /// Block kind used to resolve typography.
    pub kind: EnzReadingKind,
    /// Plain content used when inline runs are absent.
    pub text: String,
    /// Heading level; zero renders as level one and non-heading blocks ignore it.
    pub level: u8,
    /// Styled runs that replace plain content and carry link payloads.
    pub inline: Option<crate::rich_text::RichText<L>>,
}

/// Plain paragraph backed by a lazy immutable text snapshot.
/// Retain and clone snapshots across view rebuilds to reuse unchanged text layout.
#[derive(Debug, Clone)]
pub struct EnzRevisionReadingParagraph {
    /// Block kind used to resolve typography.
    pub kind: EnzReadingKind,
    /// Immutable lazy content for the plain paragraph.
    pub snapshot: TextSnapshot,
    /// Heading level; zero renders as level one and non-heading blocks ignore it.
    pub level: u8,
}

impl EnzRevisionReadingParagraph {
    /// Construct a revision-aware plain paragraph.
    #[must_use]
    pub fn new(kind: EnzReadingKind, snapshot: TextSnapshot, level: u8) -> Self {
        Self {
            kind,
            snapshot,
            level,
        }
    }
}

impl EnzReadingParagraph<()> {
    /// Construct a paragraph with plain text and no inline runs.
    pub fn new(kind: EnzReadingKind, text: impl Into<String>, level: u8) -> Self {
        Self {
            kind,
            text: text.into(),
            level,
            inline: None,
        }
    }

    /// Attach styled runs, changing the paragraph's link-payload type.
    /// Runs supersede `text`; their payloads are delivered through [`EnzReadingPane::on_link_clicked`].
    pub fn with_inline<L>(self, inline: crate::rich_text::RichText<L>) -> EnzReadingParagraph<L> {
        EnzReadingParagraph {
            kind: self.kind,
            text: self.text,
            level: self.level,
            inline: Some(inline),
        }
    }
}

/// Fixed paper colour schemes or runtime app-palette colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EnzReadingTheme {
    /// Warm sepia paper with fixed ink.
    Sepia,
    /// Cream paper with fixed dark ink.
    Light,
    /// Dark paper with fixed light ink.
    Dark,
    /// Live app-palette surface and ink; the default scheme.
    #[default]
    App,
}

impl EnzReadingTheme {
    /// Parse `"light"`, `"sepia"` or `"dark"`; other names select App mode.
    pub fn from_name(s: &str) -> Self {
        match s {
            "light" => Self::Light,
            "sepia" => Self::Sepia,
            "dark" => Self::Dark,
            _ => Self::App,
        }
    }
}
#[inline]
fn resolve_font_style(kind: EnzReadingKind) -> iced::font::Style {
    if kind == EnzReadingKind::Blockquote {
        iced::font::Style::Italic
    } else {
        iced::font::Style::Normal
    }
}
#[inline]
fn resolve_rich_base_font_style(_kind: EnzReadingKind) -> iced::font::Style {
    iced::font::Style::Normal
}
#[inline]
fn body_font(family: Family, weight: iced::font::Weight, style: iced::font::Style) -> iced::Font {
    iced::Font {
        family,
        weight,
        style,
        ..iced::Font::default()
    }
}
fn paragraph_at_y(y: f32, heights: &[f32], spacing: f32) -> Option<usize> {
    if heights.is_empty() {
        return None;
    }
    // Positive infinity clamps below the column; only NaN has no position.
    if y.is_nan() || y <= 0.0 {
        return Some(0);
    }
    let spacing = if spacing.is_finite() && spacing > 0.0 {
        spacing
    } else {
        0.0
    };
    let mut cursor = 0.0f32;
    for (i, h) in heights.iter().enumerate() {
        let h = if h.is_finite() && *h > 0.0 { *h } else { 0.0 };
        // A spacing gutter belongs to the preceding paragraph.
        let end = cursor + h + spacing;
        if y < end {
            return Some(i);
        }
        cursor = end;
    }
    Some(heights.len() - 1)
}
#[inline]
fn resolve_content_width(content_max_width: f32) -> Length {
    if content_max_width.is_finite() && content_max_width > 0.0 {
        Length::Fixed(content_max_width)
    } else {
        Length::Fill
    }
}

/// Builder for a reading column with optional typed links and paragraph selection callbacks.
#[must_use]
pub struct EnzReadingPane<'a, L, Message> {
    paragraphs: Vec<EnzReadingParagraph<L>>,
    theme: EnzReadingTheme,
    font_size: f32,
    line_height_multiple: f32,
    content_max_width: f32,
    paragraph_spacing: f32,
    inset: f32,
    on_link_clicked: Option<std::rc::Rc<dyn Fn(L) -> Message + 'a>>,
    #[allow(
        clippy::type_complexity,
        reason = "one boxed wrapping callback; an alias would only hide it"
    )]
    wrap_selection:
        Option<Box<dyn FnOnce(Element<'a, Message>, Vec<f32>, f32) -> Element<'a, Message> + 'a>>,
    selected_paragraphs: Vec<usize>,
    revisioned_plain: std::collections::HashMap<usize, TextSnapshot>,
    font_family: Family,
    _phantom: std::marker::PhantomData<&'a Message>,
}

/// Construct a reading pane using App paper and a default sans-serif family.
/// Defaults to a scaled 14 px font, 1.6× line height, a 680 px column cap,
/// 12 px paragraph spacing and 24 px padding on all sides. Links and selection
/// callbacks are unset, and no paragraphs are highlighted.
pub fn enz_reading_pane<L, Message>(
    paragraphs: Vec<EnzReadingParagraph<L>>,
) -> EnzReadingPane<'static, L, Message> {
    EnzReadingPane {
        paragraphs,
        theme: EnzReadingTheme::App,
        font_size: enzui_theme::text_scale::ui_px(14.0),
        line_height_multiple: 1.6,
        content_max_width: 680.0,
        paragraph_spacing: 12.0,
        inset: 24.0,
        on_link_clicked: None,
        wrap_selection: None,
        selected_paragraphs: Vec::new(),
        revisioned_plain: std::collections::HashMap::new(),
        font_family: Family::default(),
        _phantom: std::marker::PhantomData,
    }
}

/// Construct a plain reading pane from lazy immutable snapshots.
/// Uses the ordinary pane's styling defaults. Reusing snapshots with unchanged
/// typography and bounds reuses text layout; replacing them abandons that cache.
/// Selection height estimates use the empty plain placeholders, not snapshot text.
pub fn enz_revisioned_reading_pane<Message>(
    paragraphs: Vec<EnzRevisionReadingParagraph>,
) -> EnzReadingPane<'static, (), Message> {
    let mut revisioned_plain = std::collections::HashMap::with_capacity(paragraphs.len());
    let plain = paragraphs
        .into_iter()
        .enumerate()
        .map(|(index, paragraph)| {
            revisioned_plain.insert(index, paragraph.snapshot);
            EnzReadingParagraph::new(paragraph.kind, String::new(), paragraph.level)
        })
        .collect();

    let mut pane = enz_reading_pane(plain);
    pane.revisioned_plain = revisioned_plain;
    pane
}

impl<'a, L, Message: 'a> EnzReadingPane<'a, L, Message> {
    /// Set paper colours; defaults to [`EnzReadingTheme::App`].
    pub fn theme(mut self, t: EnzReadingTheme) -> Self {
        self.theme = t;
        self
    }

    /// Set body font size in logical pixels.
    /// Non-finite or non-positive input falls back to 14; headings use their own scale.
    pub fn font_size(mut self, sz: f32) -> Self {
        self.font_size = enzui_core::metrics::FinitePositive::new_or(sz, 14.0).get();
        self
    }

    /// Set line height relative to font size; defaults to 1.6.
    /// Non-finite or non-positive input falls back to 1.6.
    pub fn line_height_multiple(mut self, m: f32) -> Self {
        self.line_height_multiple = enzui_core::metrics::FinitePositive::new_or(m, 1.6).get();
        self
    }

    /// Set the readable column's width cap in logical pixels; defaults to 680.
    /// Zero fills the available width. Negative or non-finite input falls back to 680.
    pub fn content_max_width(mut self, w: f32) -> Self {
        self.content_max_width = enzui_core::metrics::FiniteNonNeg::new_or(w, 680.0).get();
        self
    }

    /// Set spacing between blocks in logical pixels; defaults to 12.
    /// Negative or non-finite values fall back to 12, while zero is preserved.
    pub fn paragraph_spacing(mut self, s: f32) -> Self {
        self.paragraph_spacing = enzui_core::metrics::FiniteNonNeg::new_or(s, 12.0).get();
        self
    }

    /// Set padding on all four sides in logical pixels; defaults to 24.
    /// Negative or non-finite values fall back to 24, while zero is preserved.
    pub fn inset(mut self, i: f32) -> Self {
        self.inset = enzui_core::metrics::FiniteNonNeg::new_or(i, 24.0).get();
        self
    }

    /// Number of blocks supplied to this pane.
    pub fn paragraph_count(&self) -> usize {
        self.paragraphs.len()
    }

    /// Receive the clicked inline run's typed payload.
    /// Without a callback, link clicks emit no message.
    pub fn on_link_clicked(mut self, f: impl Fn(L) -> Message + 'a) -> Self {
        self.on_link_clicked = Some(std::rc::Rc::new(f));
        self
    }

    /// Report paragraph-level press, held-drag and release gestures to the host.
    /// Indices use estimated heights from explicit line breaks, font size and line
    /// height, without accounting for wrapped lines or styled-run metrics. Gaps
    /// map to the preceding paragraph and outside drags clamp to an end. Empty
    /// content falls back to index zero. The host owns selection state and copying;
    /// these callbacks do not suppress link navigation automatically.
    pub fn on_selection(
        mut self,
        press: impl Fn(usize) -> Message + 'a,
        drag: impl Fn(usize) -> Message + 'a,
        release: Message,
    ) -> Self
    where
        Message: Clone + 'a,
    {
        self.wrap_selection = Some(Box::new(
            move |content: Element<'a, Message>, heights: Vec<f32>, spacing: f32| {
                let at = move |p: iced::Point| paragraph_at_y(p.y, &heights, spacing).unwrap_or(0);
                let at_press = at.clone();
                mouse_area(content)
                    .on_press_at(move |p| press(at_press(p)))
                    .on_drag(move |p| drag(at(p)))
                    .on_release(release)
                    .into()
            },
        ));
        self
    }

    /// Paint full-width bands behind the supplied paragraph indices.
    /// Defaults to no bands. Duplicate indices collapse and out-of-range indices
    /// are ignored. The host keeps highlighted paragraphs consistent with copied text.
    pub fn selected_paragraphs(mut self, indices: impl IntoIterator<Item = usize>) -> Self {
        self.selected_paragraphs = indices.into_iter().collect();
        self
    }

    /// Resolve the Document font family from this theme at construction time.
    /// Plain and inline paths share it while preserving their weights and styles.
    /// Use the same family and typography for pagination; later theme changes
    /// require rebuilding this configuration.
    pub fn font_from_theme(mut self, theme: &EnzTheme) -> Self {
        self.font_family = theme.font_catalog.family(FontRole::Document).to_iced();
        self
    }

    /// Return paper, primary ink and muted ink colours.
    /// App mode uses the dark palette for this accessor; rendering resolves it
    /// against the live palette. Fixed paper modes ignore the app palette.
    pub fn palette(&self) -> (Color, Color, Color) {
        reading_colors(self.theme, &Palette::dark())
    }
}
fn band_paragraph<'a, Message: 'a>(
    element: Element<'a, Message>,
    index: usize,
    highlighted: &std::collections::HashSet<usize>,
    reading_theme: EnzReadingTheme,
) -> Element<'a, Message> {
    if !highlighted.contains(&index) {
        return element;
    }
    container(element)
        .width(Length::Fill)
        .style(move |t: &iced::Theme| iced::widget::container::Style {
            background: Some(
                reading_selection_fill(reading_theme, &crate::theme_lookup::palette(t)).into(),
            ),
            ..Default::default()
        })
        .into()
}

/// Return the supplied palette's selection RGB with [`SELECTION_FILL_ALPHA`].
/// The paper-theme argument does not affect this fill; it composites over the
/// paper resolved by [`reading_colors`].
pub fn reading_selection_fill(_theme: EnzReadingTheme, p: &Palette) -> Color {
    Color {
        a: SELECTION_FILL_ALPHA,
        ..p.surface_selected
    }
}

/// Alpha of the paragraph selection band, replacing the selection token's alpha.
pub const SELECTION_FILL_ALPHA: f32 = 0.18;
pub(crate) fn inline_tokens(theme: EnzReadingTheme, p: &Palette) -> TextTokens {
    let (_, ink, ink_muted) = reading_colors(theme, p);
    TextTokens {
        default_color: ink,
        muted_color: ink_muted,
        subtle_color: ink_muted,
        accent_color: ink,
        success_color: ink,
        warning_color: ink,
        danger_color: ink,
        disabled_color: ink_muted,
    }
}

/// Return paper, primary ink and muted ink for the selected scheme.
/// App uses the supplied palette; fixed paper modes ignore it.
pub fn reading_colors(theme: EnzReadingTheme, p: &Palette) -> (Color, Color, Color) {
    {
        match theme {
            EnzReadingTheme::Light => (
                Color::from_rgb8(0xfb, 0xfa, 0xf7),
                Color::from_rgb8(0x24, 0x24, 0x2a),
                Color::from_rgb8(0x6a, 0x6a, 0x72),
            ),
            EnzReadingTheme::Sepia => (
                Color::from_rgb8(0xf4, 0xec, 0xd8),
                Color::from_rgb8(0x4a, 0x3f, 0x2f),
                Color::from_rgb8(0x7a, 0x6a, 0x52),
            ),
            EnzReadingTheme::Dark => (
                Color::from_rgb8(0x1a, 0x1a, 0x1e),
                Color::from_rgb8(0xd8, 0xd8, 0xd2),
                Color::from_rgb8(0x9a, 0x9a, 0x94),
            ),
            EnzReadingTheme::App => (p.surface, p.text, p.text_muted),
        }
    }
}

impl<'a, L, Message: 'a> EnzReadingPane<'a, L, Message> {
    /// Heading font size in logical pixels.
    /// Levels 1–5 use 28, 24, 20, 17 and 15; other levels use 13. Pane rendering
    /// coerces level zero to one before calling this helper.
    #[must_use]
    pub fn heading_size(level: u8) -> f32 {
        match level {
            1 => 28.0,
            2 => 24.0,
            3 => 20.0,
            4 => 17.0,
            5 => 15.0,
            _ => 13.0,
        }
    }

    /// Use bold for levels one and two, semibold otherwise.
    /// Pane rendering coerces level zero to one before calling this helper.
    #[must_use]
    pub fn heading_weight(level: u8) -> iced::font::Weight {
        match level {
            1 | 2 => iced::font::Weight::Bold,
            _ => iced::font::Weight::Semibold,
        }
    }

    /// Build the paper surface, paragraph column, inline links and optional selection wrapper.
    /// Plain text wraps by word or glyph; inline text uses the shared rich-text adapter.
    pub fn into_element(self) -> Element<'a, Message>
    where
        L: Clone + PartialEq + 'static,
    {
        let reading_theme = self.theme;
        let (_, inline_ink, inline_ink_muted) = self.palette();
        let column_max = self.content_max_width;
        let paragraph_spacing = self.paragraph_spacing;
        let font_size = self.font_size;
        let line_height = self.line_height_multiple;
        let inset = self.inset;
        let font_family = self.font_family;
        let on_link = self.on_link_clicked;
        let wrap_selection = self.wrap_selection;
        let selection_enabled = wrap_selection.is_some();
        let highlighted: std::collections::HashSet<usize> =
            self.selected_paragraphs.iter().copied().collect();
        let mut revisioned_plain = self.revisioned_plain;
        // Estimates count explicit lines, not the wrapped heights stored in the widget tree.
        let selection_heights: Vec<f32> = if selection_enabled {
            self.paragraphs
                .iter()
                .map(|p| {
                    let size = match p.kind {
                        EnzReadingKind::Heading => Self::heading_size(p.level.max(1)),
                        _ => font_size,
                    };
                    let body = match &p.inline {
                        Some(rich) => rich.spans().iter().map(|s| s.text.as_str()).collect(),
                        None => p.text.clone(),
                    };
                    let lines = body.lines().count().max(1) as f32;
                    lines * size * line_height
                })
                .collect()
        } else {
            Vec::new()
        };

        let mut col = column![].spacing(paragraph_spacing);
        for (para_index, para) in self.paragraphs.into_iter().enumerate() {
            let muted_ink = matches!(para.kind, EnzReadingKind::Blockquote);
            let (size, weight) = match para.kind {
                EnzReadingKind::Heading => (
                    Self::heading_size(para.level.max(1)),
                    Self::heading_weight(para.level.max(1)),
                ),
                EnzReadingKind::Paragraph | EnzReadingKind::Blockquote => {
                    (font_size, iced::font::Weight::Normal)
                }
            };
            let plain_font_style = resolve_font_style(para.kind);
            // Inline italics belong to shaped runs so link geometry shares their metrics.
            let rich_base_font_style = resolve_rich_base_font_style(para.kind);
            if let Some(rich) = para.inline {
                let color = if muted_ink {
                    inline_ink_muted
                } else {
                    inline_ink
                };
                let tokens = TextTokens {
                    default_color: color,
                    muted_color: inline_ink_muted,
                    subtle_color: inline_ink_muted,
                    accent_color: color,
                    success_color: color,
                    warning_color: color,
                    danger_color: color,
                    disabled_color: inline_ink_muted,
                };
                let base_font = body_font(font_family, weight, rich_base_font_style);
                let line_h = iced::widget::text::LineHeight::Relative(line_height);
                let ambient = move |t: &iced::Theme| {
                    let (_, ink, ink_muted) =
                        reading_colors(reading_theme, &crate::theme_lookup::palette(t));
                    if muted_ink { ink_muted } else { ink }
                };
                let themed =
                    crate::rich_text::ThemedTokens::Custom(Box::new(move |t: &iced::Theme| {
                        inline_tokens(reading_theme, &crate::theme_lookup::palette(t))
                    }));
                let el: Element<'a, Message> = rich.view_routed_themed(
                    &tokens,
                    size,
                    Some(base_font),
                    Some(line_h),
                    on_link.clone().map(|cb| move |l: L| cb(l)),
                    Some(ambient),
                    themed,
                );
                col = col.push(band_paragraph(el, para_index, &highlighted, reading_theme));
            } else {
                let style = move |t: &iced::Theme| {
                    let (_, ink, ink_muted) =
                        reading_colors(reading_theme, &crate::theme_lookup::palette(t));
                    iced::widget::text::Style {
                        color: Some(if muted_ink { ink_muted } else { ink }),
                    }
                };
                let element: Element<'a, Message> =
                    if let Some(snapshot) = revisioned_plain.remove(&para_index) {
                        enz_revision_text(snapshot)
                            .size(size)
                            .style(style)
                            .line_height(iced::widget::text::LineHeight::Relative(line_height))
                            .font(body_font(font_family, weight, plain_font_style))
                            .wrapping(iced::advanced::text::Wrapping::WordOrGlyph)
                            .into()
                    } else {
                        text(para.text)
                            .size(size)
                            .style(style)
                            .line_height(iced::widget::text::LineHeight::Relative(line_height))
                            .font(body_font(font_family, weight, plain_font_style))
                            .wrapping(iced::advanced::text::Wrapping::WordOrGlyph)
                            .into()
                    };
                col = col.push(band_paragraph(
                    element,
                    para_index,
                    &highlighted,
                    reading_theme,
                ));
            }
        }
        let body: Element<'a, Message> = match wrap_selection {
            Some(wrap) => wrap(col.into(), selection_heights, paragraph_spacing),
            None => col.into(),
        };
        let inner: Element<'a, Message> = container(body)
            .width(resolve_content_width(column_max))
            .into();
        container(inner)
            .width(Length::Fill)
            .clip(true)
            .padding(
                iced::Padding::default()
                    .left(inset)
                    .right(inset)
                    .top(inset)
                    .bottom(inset),
            )
            .center_x(Length::Fill)
            .style(move |t: &iced::Theme| iced::widget::container::Style {
                background: Some(
                    reading_colors(reading_theme, &crate::theme_lookup::palette(t))
                        .0
                        .into(),
                ),
                ..Default::default()
            })
            .into()
    }
}

impl<'a, L, Message: 'a> From<EnzReadingPane<'a, L, Message>> for Element<'a, Message>
where
    L: Clone + PartialEq + 'static,
{
    fn from(w: EnzReadingPane<'a, L, Message>) -> Self {
        w.into_element()
    }
}

use std::ops::Range;
use std::sync::Arc;

use iced::advanced::text::highlighter::{self as text_highlighter};
use iced::advanced::text::parser;
use iced::font::{Family, Style, Weight};

use super::RichEditController;
use super::document::{ParagraphAlign, line_body_ranges};
use crate::rich_text::SpanStyle;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RichEditPaintSettings {
    revision: u64,
    base_font: iced::Font,
    paper: Option<iced::Color>,
    lines: Arc<[Arc<[PaintRun]>]>,
    aligns: Arc<[ParagraphAlign]>,
}

#[derive(Debug, Clone, PartialEq)]
struct PaintRun {
    range: Range<usize>,
    style: SpanStyle,
}

impl RichEditPaintSettings {
    pub(crate) fn from_controller(controller: &RichEditController) -> Self {
        let document = controller.document();
        let document_runs = document.runs();
        let line_ranges = line_body_ranges(document.text());
        let mut run_index = 0;
        let mut lines: Vec<Arc<[PaintRun]>> = Vec::with_capacity(line_ranges.len());

        for line_range in line_ranges {
            while run_index < document_runs.len()
                && document_runs[run_index].range.end <= line_range.start
            {
                run_index += 1;
            }

            let mut line_runs = Vec::new();
            let mut index = run_index;
            while index < document_runs.len() && document_runs[index].range.start < line_range.end {
                let run = &document_runs[index];
                let start = run.range.start.max(line_range.start) - line_range.start;
                let end = run.range.end.min(line_range.end) - line_range.start;
                if start < end && run.style != SpanStyle::default() {
                    line_runs.push(PaintRun {
                        range: start..end,
                        style: run.style,
                    });
                }
                index += 1;
            }
            lines.push(Arc::from(line_runs));
        }

        // One alignment per line, in line order, because the editor addresses
        // its lines by index. The document's paragraph vector is that same
        // sequence: `line_body_ranges` splits on the endings the paragraph
        // model splits on, so paragraph *i* is line *i*.
        let aligns: Vec<ParagraphAlign> = document
            .paragraph_formats()
            .iter()
            .map(|paragraph| paragraph.align)
            .collect();

        Self {
            revision: controller.format_revision,
            base_font: iced::Font::DEFAULT,
            paper: None,
            lines: lines.into(),
            aligns: aligns.into(),
        }
    }

    /// The alignment of the line at `index`, for the editor's own per-line
    /// alignment channel.
    ///
    /// A line the document does not describe — the editor holds a trailing
    /// empty line the paragraph vector may not — takes the default, which is
    /// the reading edge.
    ///
    /// A left-aligned paragraph is reported as [`Alignment::Default`] rather
    /// than [`Alignment::Left`], because the document's default alignment is
    /// indistinguishable from a paragraph nobody has aligned: `Default` is the
    /// answer that leaves a right-to-left line reading from its own edge, which
    /// is what the editor does today.
    ///
    /// [`Alignment::Default`]: iced::advanced::text::Alignment::Default
    /// [`Alignment::Left`]: iced::advanced::text::Alignment::Left
    pub(crate) fn line_alignment(&self, index: usize) -> iced::advanced::text::Alignment {
        use iced::advanced::text::Alignment;

        match self.aligns.get(index).copied().unwrap_or_default() {
            ParagraphAlign::Left => Alignment::Default,
            other => other.to_text_alignment(),
        }
    }

    pub(crate) fn with_base_font(mut self, base_font: iced::Font) -> Self {
        self.base_font = base_font;
        self
    }

    /// Declare the opaque fill the editor text is painted on.
    ///
    /// Formatted ink is otherwise resolved against the ambient palette, which
    /// is the wrong surface once the editor paints its own page.
    pub(crate) fn with_paper(mut self, paper: iced::Color) -> Self {
        self.paper = Some(paper);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RichEditHighlight {
    style: SpanStyle,
    base_font: iced::Font,
    paper: Option<iced::Color>,
}

// Ink already clearing the floor is returned unchanged, so a colour chosen to
// suit the page paints exactly as authored; the rest moves along its own OkLab
// lightness path and keeps its hue.
fn ink_legible_on(ink: iced::Color, paper: iced::Color) -> iced::Color {
    enzui_theme::raise_to_floor(ink, paper, enzui_theme::ContrastFloor::Text).color()
}

pub(crate) fn format_for_rich_edit(
    highlight: &RichEditHighlight,
    theme: &iced::Theme,
) -> text_highlighter::Style {
    let style = highlight.style;
    // An authored ink is a value the document carries and outranks the tone,
    // which is only the role the run would otherwise take.
    let color = style
        .color
        .or_else(|| {
            style.tone.map(|tone| {
                crate::enz_text::TextTokens::from_palette(&crate::theme_lookup::palette(theme))
                    .color_for(tone)
            })
        })
        .map(|ink| match highlight.paper {
            Some(paper) => ink_legible_on(ink, paper),
            None => ink,
        });

    let font = font_for_rich_edit(highlight);

    text_highlighter::Style {
        color,
        font,
        size: style.text_size.map(iced::Pixels),
        // The whole font above already carries the italic style.
        style: None,
        underline: style.underline,
        strikethrough: style.strikethrough,
    }
}

fn font_for_rich_edit(highlight: &RichEditHighlight) -> Option<iced::Font> {
    let style = highlight.style;
    (style.bold || style.italic || style.code || style.font.is_some()).then_some(iced::Font {
        family: if style.code {
            Family::Monospace
        } else {
            style.font.unwrap_or(highlight.base_font.family)
        },
        weight: if style.bold {
            Weight::Bold
        } else {
            highlight.base_font.weight
        },
        style: if style.italic {
            Style::Italic
        } else {
            highlight.base_font.style
        },
        stretch: highlight.base_font.stretch,
        numeric: highlight.base_font.numeric,
    })
}

fn has_inline_typography(settings: &RichEditPaintSettings) -> bool {
    settings
        .lines
        .iter()
        .flat_map(|runs| runs.iter())
        .any(|run| run.style.font.is_some() || run.style.text_size.is_some())
}

#[derive(Debug, Clone)]
pub(crate) struct RichEditHighlighter {
    settings: RichEditPaintSettings,
    current_line: usize,
    layout_style_enabled: bool,
}

impl parser::Parser for RichEditHighlighter {
    type Settings = RichEditPaintSettings;
    type Output = RichEditHighlight;
    type Iterator<'a> = std::vec::IntoIter<(Range<usize>, RichEditHighlight)>;

    fn has_layout_style(&self) -> bool {
        self.layout_style_enabled
    }

    fn layout_style(highlight: &Self::Output) -> text_highlighter::Style {
        text_highlighter::Style {
            font: font_for_rich_edit(highlight),
            size: highlight.style.text_size.map(iced::Pixels),
            ..text_highlighter::Style::default()
        }
    }

    fn new(settings: &Self::Settings) -> Self {
        Self {
            settings: settings.clone(),
            current_line: 0,
            layout_style_enabled: has_inline_typography(settings),
        }
    }

    fn update(&mut self, new_settings: &Self::Settings) {
        // Clearing overrides must remove stale native metrics before intrinsic measurement.
        self.layout_style_enabled |= has_inline_typography(new_settings);
        self.settings = new_settings.clone();
        self.current_line = 0;
    }

    fn change_line(&mut self, line: usize) {
        self.current_line = self.current_line.min(line);
    }

    fn parse_line(&mut self, line: &str) -> Self::Iterator<'_> {
        let runs: Vec<(Range<usize>, RichEditHighlight)> = self
            .settings
            .lines
            .get(self.current_line)
            .map(|runs| {
                runs.iter()
                    .filter_map(|run| {
                        let start = floor_char_boundary(line, run.range.start.min(line.len()));
                        let end = floor_char_boundary(line, run.range.end.min(line.len()));
                        (start < end).then_some((
                            start..end,
                            RichEditHighlight {
                                style: run.style,
                                base_font: self.settings.base_font,
                                paper: self.settings.paper,
                            },
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.current_line += 1;
        runs.into_iter()
    }

    fn current_line(&self) -> usize {
        self.current_line
    }

    fn seek_line(&mut self, line: usize) -> bool {
        self.current_line = line;
        true
    }
}

fn floor_char_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

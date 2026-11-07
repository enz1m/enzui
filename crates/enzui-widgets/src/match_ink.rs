//! Stateful syntax highlighting with caller-supplied find-match glyph emphasis.
//!
//! Hosts own the corresponding background fills from [`FindMatchTokens`]. Matched
//! glyph ink resolves against those fills; unmatched toned spans keep ordinary
//! syntax colours, and unmatched untoned text uses the editor’s ambient ink.
//!
//! Runs use editor-buffer line indices and byte offsets. Offsets clamp to line
//! length and round down to UTF-8 boundaries; a start can expand backward into
//! its character. Empty or reversed normalized runs are ignored. Active emphasis
//! wins overlapping bytes regardless of input order. Emitted spans omit untoned,
//! unmatched text. The highlighter retains lexical checkpoints; settings share match storage.

use std::ops::Range;
use std::sync::Arc;

use enzui_theme::{FindMatchTokens, MatchEmphasis};
use iced::advanced::text::highlighter::{self as text_highlighter};
use iced::advanced::text::parser;

use crate::syntax::{Language, StatefulSyntaxEngine, SyntaxWork, TokenClass, tone_for_class};

/// A match range on one line of the editor buffer.
/// Offsets clamp to length and round down to character boundaries before styling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchRun {
    /// Line index in the supplied editor buffer, including windowed buffers.
    pub line: usize,
    /// Inclusive starting byte offset; rounding down can expand the range backward.
    pub start: usize,
    /// Exclusive ending byte offset; rounding down omits a partially covered final character.
    pub end: usize,
    /// Current or subordinate match emphasis.
    pub emphasis: MatchEmphasis,
}

/// Syntax language and shared match ranges used during parsing.
/// Settings equality compares match content; cloning shares its Arc storage.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchInkSettings {
    /// Source language for syntax classification.
    pub language: Language,
    /// Match ranges in any order, addressed to editor-buffer lines.
    pub matches: Arc<[MatchRun]>,
}

impl MatchInkSettings {
    /// Own a language choice and shared match ranges.
    #[must_use]
    pub fn new(language: Language, matches: impl Into<Arc<[MatchRun]>>) -> Self {
        Self {
            language,
            matches: matches.into(),
        }
    }

    /// Use syntax highlighting with no match ranges.
    #[must_use]
    pub fn plain(language: Language) -> Self {
        Self::new(language, Vec::new())
    }
}

/// Syntax class and optional find-match emphasis for an emitted span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditorInk {
    /// Syntax classification for this span.
    pub class: TokenClass,
    /// Current or subordinate match emphasis.
    pub emphasis: Option<MatchEmphasis>,
}

/// Resolve span ink against the live theme’s syntax colours and match fills.
/// Outside a match, use ordinary syntax formatting. Matched ink is opaque and
/// resolved through [`FindMatchTokens::ink_on`]; untoned matches start from palette
/// body text rather than a custom editor ambient colour. Hosts must use the matching fills.
#[must_use]
pub fn format_for_editor_ink(ink: &EditorInk, theme: &iced::Theme) -> text_highlighter::Style {
    let palette = crate::theme_lookup::palette(theme);
    let tone = tone_for_class(ink.class);

    let Some(emphasis) = ink.emphasis else {
        let color =
            tone.map(|tone| crate::enz_text::TextTokens::from_palette(&palette).color_for(tone));

        return text_highlighter::Style {
            color,
            font: None,
            ..text_highlighter::Style::default()
        };
    };
    let unmatched = tone.map_or(palette.text, |tone| {
        crate::enz_text::TextTokens::from_palette(&palette).color_for(tone)
    });

    text_highlighter::Style {
        color: Some(FindMatchTokens::from_palette(&palette).ink_on(unmatched, emphasis)),
        font: None,
        ..text_highlighter::Style::default()
    }
}

/// Stateful lexical parser with overlapping find-match ranges.
/// Implements [`parser::Parser`] for editor highlighting.
#[derive(Debug, Clone)]
pub struct MatchInkHighlighter {
    settings: MatchInkSettings,
    engine: StatefulSyntaxEngine,
    /// Indices into the match runs, ordered by line, so a line finds its
    /// runs without scanning everyone else's. Rebuilt when the settings change.
    by_line: Vec<usize>,
}

/// The match runs' indices in line order.
fn index_by_line(matches: &[MatchRun]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..matches.len()).collect();
    order.sort_by_key(|&run| matches[run].line);
    order
}

impl MatchInkHighlighter {
    /// Read accumulated lexical-work counters.
    #[must_use]
    pub fn work(&self) -> SyntaxWork {
        self.engine.work()
    }

    /// Count retained lexical checkpoints.
    #[must_use]
    pub fn resident_checkpoints(&self) -> usize {
        self.engine.resident_checkpoints()
    }

    /// Reset lexical-work counters while retaining checkpoints.
    pub fn reset_work(&mut self) {
        self.engine.reset_work();
    }

    fn runs_on(&self, line: &str, index: usize) -> Vec<(Range<usize>, MatchEmphasis)> {
        let matches = &self.settings.matches;
        let first = self
            .by_line
            .partition_point(|&run| matches[run].line < index);
        let last = self
            .by_line
            .partition_point(|&run| matches[run].line <= index);
        let mut runs: Vec<(Range<usize>, MatchEmphasis)> = self.by_line[first..last]
            .iter()
            .map(|&run| &matches[run])
            .filter_map(|run| {
                let start = floor_boundary(line, run.start.min(line.len()));
                let end = floor_boundary(line, run.end.min(line.len()));

                (start < end).then_some((start..end, run.emphasis))
            })
            .collect();
        runs.sort_by_key(|(range, emphasis)| (range.start, emphasis_rank(*emphasis)));

        runs
    }
}
fn emphasis_rank(emphasis: MatchEmphasis) -> u8 {
    match emphasis {
        MatchEmphasis::Active => 0,
        MatchEmphasis::Inactive => 1,
    }
}
fn floor_boundary(line: &str, offset: usize) -> usize {
    let mut offset = offset.min(line.len());

    while offset > 0 && !line.is_char_boundary(offset) {
        offset -= 1;
    }

    offset
}

impl parser::Parser for MatchInkHighlighter {
    type Settings = MatchInkSettings;
    type Output = EditorInk;
    type Iterator<'a> = std::vec::IntoIter<(Range<usize>, EditorInk)>;

    fn new(settings: &Self::Settings) -> Self {
        Self {
            settings: settings.clone(),
            engine: StatefulSyntaxEngine::new(settings.language),
            by_line: index_by_line(&settings.matches),
        }
    }

    fn update(&mut self, new_settings: &Self::Settings) {
        if self.engine.language() == new_settings.language {
            self.engine.rewind();
        } else {
            self.engine.update_language(new_settings.language);
        }
        self.settings = new_settings.clone();
        self.by_line = index_by_line(&self.settings.matches);
    }

    fn change_line(&mut self, line: usize) {
        self.engine.change_line(line);
    }

    fn parse_line(&mut self, line: &str) -> Self::Iterator<'_> {
        let index = self.engine.current_line();
        let runs = self.runs_on(line, index);
        let tokens = self.engine.highlight_line(line);

        let mut spans: Vec<(Range<usize>, EditorInk)> = Vec::with_capacity(tokens.len());

        for token in tokens {
            let mut cursor = token.start;

            while cursor < token.end {
                let emphasis = runs
                    .iter()
                    .filter(|(range, _)| range.start <= cursor && cursor < range.end)
                    .map(|(_, emphasis)| *emphasis)
                    .min_by_key(|emphasis| emphasis_rank(*emphasis));
                // Each overlap edge changes the covering set; a span must end before that change.
                let next = runs
                    .iter()
                    .filter_map(|(range, _)| {
                        if range.start <= cursor && cursor < range.end {
                            Some(range.end)
                        } else if range.start > cursor {
                            Some(range.start)
                        } else {
                            None
                        }
                    })
                    .min()
                    .unwrap_or(token.end)
                    .min(token.end);

                let end = next.max(cursor + 1).min(token.end);
                if emphasis.is_some() || tone_for_class(token.class).is_some() {
                    spans.push((
                        cursor..end,
                        EditorInk {
                            class: token.class,
                            emphasis,
                        },
                    ));
                }

                cursor = end;
            }
        }

        spans.into_iter()
    }

    fn current_line(&self) -> usize {
        self.engine.current_line()
    }

    fn seek_line(&mut self, line: usize) -> bool {
        self.engine.seek_line(line)
    }

    fn note_edits(&mut self, edits: &[parser::LineEdit]) {
        self.engine.note_edits(edits);
    }
}

#![allow(
    clippy::string_slice,
    reason = "byte offsets come from shaping of the same text"
)]

use super::*;

#[derive(Debug, Clone, Copy)]
pub(super) enum KeyboardSelectionMove {
    Horizontal { right: bool, by_word: bool },
    Vertical { down: bool },
    LineEdge { end: bool },
    DocumentEdge { end: bool },
}

pub(super) fn keyboard_selection_move(event: &keyboard::Event) -> Option<KeyboardSelectionMove> {
    let keyboard::Event::KeyPressed {
        key: keyboard::Key::Named(key),
        modifiers,
        ..
    } = event
    else {
        return None;
    };
    if !modifiers.shift() || modifiers.logo() {
        return None;
    }
    let by_word = modifiers.control() || modifiers.alt();
    match key {
        keyboard::key::Named::ArrowLeft => Some(KeyboardSelectionMove::Horizontal {
            right: false,
            by_word,
        }),
        keyboard::key::Named::ArrowRight => Some(KeyboardSelectionMove::Horizontal {
            right: true,
            by_word,
        }),
        keyboard::key::Named::ArrowUp if !by_word => {
            Some(KeyboardSelectionMove::Vertical { down: false })
        }
        keyboard::key::Named::ArrowDown if !by_word => {
            Some(KeyboardSelectionMove::Vertical { down: true })
        }
        keyboard::key::Named::Home if modifiers.control() => {
            Some(KeyboardSelectionMove::DocumentEdge { end: false })
        }
        keyboard::key::Named::End if modifiers.control() => {
            Some(KeyboardSelectionMove::DocumentEdge { end: true })
        }
        keyboard::key::Named::Home if !modifiers.alt() => {
            Some(KeyboardSelectionMove::LineEdge { end: false })
        }
        keyboard::key::Named::End if !modifiers.alt() => {
            Some(KeyboardSelectionMove::LineEdge { end: true })
        }
        _ => None,
    }
}

pub(super) fn visual_caret_lines(regions: &[TextRegion]) -> Vec<Vec<ShapedCaret>> {
    let mut carets: Vec<_> = regions
        .iter()
        .flat_map(|region| region.keyboard_carets.iter().copied())
        .collect();
    carets.sort_by(|a, b| {
        a.point
            .y
            .total_cmp(&b.point.y)
            .then_with(|| a.point.x.total_cmp(&b.point.x))
            .then_with(|| a.position.cmp(&b.position))
            .then_with(|| (a.affinity as u8).cmp(&(b.affinity as u8)))
    });

    let mut lines: Vec<Vec<ShapedCaret>> = Vec::new();
    for caret in carets {
        if let Some(line) = lines.last_mut()
            && let Some(first) = line.first()
            && (caret.point.y - first.point.y).abs()
                <= 1.0_f32.max(caret.line_height.min(first.line_height) * 0.25)
        {
            line.push(caret);
        } else {
            lines.push(vec![caret]);
        }
    }
    for line in &mut lines {
        line.sort_by(|a, b| {
            a.point
                .x
                .total_cmp(&b.point.x)
                .then_with(|| a.position.cmp(&b.position))
                .then_with(|| (a.affinity as u8).cmp(&(b.affinity as u8)))
        });
        line.dedup_by(|a, b| {
            a.position == b.position
                && a.affinity == b.affinity
                && (a.point.x - b.point.x).abs() < 0.25
                && (a.point.y - b.point.y).abs() < 0.25
        });
    }
    lines
}

pub(super) fn same_visual_caret(a: ShapedCaret, b: ShapedCaret) -> bool {
    a.position == b.position
        && a.affinity == b.affinity
        && (a.point.x - b.point.x).abs() < 0.25
        && (a.point.y - b.point.y).abs() < 0.25
}

pub(super) fn word_boundary(text: &str, offset: usize, forward: bool) -> Option<usize> {
    let segments: Vec<(usize, usize, bool)> = text
        .split_word_bound_indices()
        .map(|(byte, segment)| {
            let start = text[..byte].chars().count();
            let end = start + segment.chars().count();
            (start, end, segment.unicode_words().next().is_some())
        })
        .collect();
    if forward {
        segments
            .iter()
            .find(|(start, end, word)| *word && (*start >= offset || *end > offset))
            .map(|(_, end, _)| *end)
    } else {
        segments
            .iter()
            .rev()
            .find(|(start, end, word)| *word && (*end <= offset || *start < offset))
            .map(|(start, _, _)| *start)
    }
}

pub(super) fn closest_caret_at(
    lines: &[Vec<ShapedCaret>],
    position: DocPosition,
    reference: Point,
) -> Option<ShapedCaret> {
    lines
        .iter()
        .flatten()
        .filter(|caret| caret.position == position)
        .min_by(|a, b| {
            let da = (a.point.x - reference.x).abs() + (a.point.y - reference.y).abs();
            let db = (b.point.x - reference.x).abs() + (b.point.y - reference.y).abs();
            da.total_cmp(&db)
        })
        .copied()
}

pub(super) fn move_keyboard_selection(
    state: &mut ScopeState,
    movement: KeyboardSelectionMove,
) -> bool {
    let lines = visual_caret_lines(&state.regions);
    let Some(first) = lines.first().and_then(|line| line.first()).copied() else {
        return false;
    };
    let focus = state
        .model
        .selection()
        .map_or_else(|| DocPosition::new(0, 0), |selection| selection.focus);
    let current = state
        .keyboard_caret
        .filter(|caret| caret.position == focus)
        .and_then(|saved| {
            lines
                .iter()
                .flatten()
                .filter(|caret| {
                    caret.position == saved.position && caret.affinity == saved.affinity
                })
                .min_by(|a, b| {
                    let da = (a.point.x - saved.point.x).abs() + (a.point.y - saved.point.y).abs();
                    let db = (b.point.x - saved.point.x).abs() + (b.point.y - saved.point.y).abs();
                    da.total_cmp(&db)
                })
                .copied()
        })
        .or_else(|| {
            lines
                .iter()
                .flatten()
                .find(|caret| {
                    caret.position == focus && caret.affinity == CaretAffinity::Downstream
                })
                .copied()
        })
        .or_else(|| closest_caret_at(&lines, focus, first.point))
        .unwrap_or(first);

    let (line_index, _) = lines
        .iter()
        .enumerate()
        .find_map(|(line_index, line)| {
            line.iter()
                .position(|caret| same_visual_caret(*caret, current))
                .map(|caret_index| (line_index, caret_index))
        })
        .unwrap_or((0, 0));

    let target = match movement {
        KeyboardSelectionMove::Horizontal { right, by_word } => {
            let mut flattened = Vec::new();
            for line in &lines {
                flattened.extend(line.iter().copied());
            }
            let current_index = flattened
                .iter()
                .position(|caret| same_visual_caret(*caret, current))
                .unwrap_or(0);
            let immediate = if right {
                flattened[current_index + 1..].iter().find(|caret| {
                    (caret.point.x - current.point.x).abs() >= 0.25
                        || (caret.point.y - current.point.y).abs() >= 0.25
                })
            } else {
                flattened[..current_index].iter().rev().find(|caret| {
                    (caret.point.x - current.point.x).abs() >= 0.25
                        || (caret.point.y - current.point.y).abs() >= 0.25
                })
            }
            .copied()
            .unwrap_or(current);
            if !by_word || immediate.position == current.position {
                immediate
            } else if immediate.position.paragraph == current.position.paragraph {
                let text = &state.regions[current.position.paragraph].text;
                let forward = immediate.position.offset > current.position.offset;
                word_boundary(text, current.position.offset, forward)
                    .and_then(|offset| {
                        closest_caret_at(
                            &lines,
                            DocPosition::new(current.position.paragraph, offset),
                            immediate.point,
                        )
                    })
                    .unwrap_or(immediate)
            } else {
                immediate
            }
        }
        KeyboardSelectionMove::Vertical { down } => {
            let target_line = if down {
                (line_index + 1).min(lines.len() - 1)
            } else {
                line_index.saturating_sub(1)
            };
            let preferred_x = state
                .keyboard_caret
                .map_or(current.point.x, |caret| caret.point.x);
            lines[target_line]
                .iter()
                .min_by(|a, b| {
                    (a.point.x - preferred_x)
                        .abs()
                        .total_cmp(&(b.point.x - preferred_x).abs())
                })
                .copied()
                .unwrap_or(current)
        }
        KeyboardSelectionMove::LineEdge { end } => {
            if end {
                lines[line_index].last().copied().unwrap_or(current)
            } else {
                lines[line_index].first().copied().unwrap_or(current)
            }
        }
        KeyboardSelectionMove::DocumentEdge { end } => {
            let paragraph = if end {
                state.regions.len().saturating_sub(1)
            } else {
                0
            };
            let offset = if end {
                state
                    .regions
                    .get(paragraph)
                    .map_or(0, |region| region.text.chars().count())
            } else {
                0
            };
            let reference = if end {
                lines
                    .last()
                    .and_then(|line| line.last())
                    .copied()
                    .unwrap_or(current)
            } else {
                first
            };
            closest_caret_at(&lines, DocPosition::new(paragraph, offset), reference.point)
                .unwrap_or(reference)
        }
    };

    let anchor = state
        .model
        .selection()
        .map_or(current.position, |selection| selection.anchor);
    state.model.press(anchor);
    state.model.drag_to(target.position);
    let _ = state.model.release();
    state.keyboard_caret = Some(target);
    true
}

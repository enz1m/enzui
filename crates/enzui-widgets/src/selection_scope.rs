//! Read-only text selection across descendant text regions.
//!
//! A scope contributes one document focus stop. Unclaimed pointer gestures
//! select across regions; focused keyboard shortcuts extend, select, clear,
//! or copy the range. Copy goes to the application's `on_copy`, or straight
//! to the clipboard when it has none.
//! Shift+Arrows extend by grapheme or visual line; Ctrl/Alt+Shift+Left/Right
//! extend by word. Shift+Home/End select to line edges, or document edges with Ctrl.
//! Shaped text supplies exact glyph geometry; legacy text uses approximate
//! horizontal offsets and whole-region highlight bounds. Drags do not autoscroll.
//! Regions follow top-to-bottom reading bands, ordered left to right within each band.
//!
//! Over a virtual list, give the scope the whole [`SelectionDocument`] and
//! wrap each row in [`enz_selection_row`] with its [`selection_row_key`]. For
//! rows built in `view`, [`EnzSelectionScope::rendered_rows`] takes the keys
//! and the list's row builder and keeps the document in the scope;
//! [`EnzSelectionScope::document`] borrows one the app keeps elsewhere. A selection then keeps endpoints in rows the
//! list unmounts (it paints up to the mounted edge) and copies rows that were
//! never mounted, identified by key rather than by text.

#![allow(
    clippy::string_slice,
    reason = "byte offsets come from shaping and line ranges of the same text"
)]

use enzui_core::click::{ClickCount, ClickDetector, SingleClickPolicy};
use enzui_core::context_menu::{
    InvocationKind as ContextInvocationKind, Point as ContextPoint, Rect as ContextRect,
};
use enzui_core::focus::FocusState;
use enzui_core::text_selection::{DocPosition, SelectionModel};
use enzui_theme::Palette;
use iced::advanced::layout::Measure as _;
use iced::advanced::widget::{self, Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::keyboard;
use iced::time::Instant;
use iced::{Color, Element, Event, Length, Point, Rectangle, Size, Vector};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use unicode_segmentation::UnicodeSegmentation;

mod caret_keys;
mod rendered_rows;
mod scope_widget;
use caret_keys::*;
use enzui_core::scroll_stack::ScrollStack;
pub use rendered_rows::RenderedRows;

/// The live text range published when selection endpoints change.
#[derive(Debug, Clone, PartialEq)]
pub struct ScopeSelection {
    /// Selected text joined across document regions.
    pub text: String,
    /// Ordered range start, with Unicode-scalar offsets within a region.
    pub start: DocPosition,
    /// Exclusive ordered range end.
    pub end: DocPosition,
    /// Union of visible selection geometry after descendant scroll clipping.
    /// A fully clipped range has zero size while retaining its text and endpoints.
    pub bounds: Rectangle,
}

/// A selection context request with placement geometry and focus restoration identity.
#[derive(Debug, Clone, PartialEq)]
pub struct SelectionContextInvocation {
    /// Selection at the time of the request.
    pub selection: ScopeSelection,
    /// Pointer requests carry a press point; keyboard and routed requests carry selection bounds.
    pub invocation: ContextInvocationKind,
    /// Identity of the focused descendant, or the focused scope when no descendant owns focus.
    /// Anonymous owners and an unfocused tree produce `None`; hosts must handle removed ids.
    pub focus_origin: Option<widget::Id>,
}

/// Fixed opacity of the selection highlight.
pub const SELECTION_FILL_ALPHA: f32 = 0.25;

const PARAGRAPH_MERGE_THRESHOLD: f32 = 2.0;

#[derive(Debug, Clone)]
struct TextRegion {
    collection_index: usize,
    compatibility_collection_index: Option<usize>,
    compatibility_semantics: bool,
    shaped_index: Option<usize>,
    text: Arc<str>,
    bounds: Rectangle,
    clip: Option<Rectangle>,
    shaped_hit: Option<usize>,
    query_line_has_text: Option<bool>,
    shaped_selection_bounds: Vec<Rectangle>,
    keyboard_carets: Vec<ShapedCaret>,
    // The logical row and paragraph within it, under an `enz_selection_row`.
    row: Option<(u64, usize)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaretAffinity {
    Upstream,
    Downstream,
}

#[derive(Debug, Clone, Default)]
struct ScopeFocus {
    state: FocusState,
    ring_visible: bool,
}

impl ScopeFocus {
    fn pointer_focus(&mut self) {
        self.state.focus();
        self.ring_visible = false;
    }

    fn is_focused(&self) -> bool {
        self.state.is_focused()
    }

    fn unfocus(&mut self) {
        self.state.unfocus();
        self.ring_visible = false;
    }

    fn operate<T>(
        &mut self,
        operation: &mut dyn Operation<T>,
        id: Option<&widget::Id>,
        bounds: Rectangle,
    ) {
        operation.focusable(id, bounds, self);
    }
}

impl widget::operation::Focusable for ScopeFocus {
    fn is_focused(&self) -> bool {
        self.state.is_focused()
    }

    fn focus(&mut self) {
        self.state.focus();
        self.ring_visible = true;
    }

    fn unfocus(&mut self) {
        ScopeFocus::unfocus(self);
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ShapedCaret {
    position: DocPosition,
    affinity: CaretAffinity,
    point: Point,
    line_height: f32,
}

impl ShapedCaret {
    fn translated(self, by: Vector) -> Self {
        Self {
            point: self.point + by,
            ..self
        }
    }
}

/// Work counters for the latest region refresh, exposed through widget operations.
/// Counts exclude renderer shaping work.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SelectionScopeWork {
    /// Text and custom callbacks visited while collecting regions.
    pub collection_visits: usize,
    /// Shaped regions inserted into the geometry index.
    pub geometry_indexed_regions: usize,
    /// Shaped callbacks visited while collecting selection geometry.
    pub geometry_visits: usize,
    /// Index lookups made while joining selection geometry.
    pub geometry_lookups: usize,
    /// Bounds and text comparisons made while joining selection geometry.
    pub geometry_comparisons: usize,
    /// Source bytes copied when cached text could not be reused or the model was rebuilt.
    pub copied_text_bytes: usize,
}

fn sort_reading_order(regions: &mut [TextRegion]) {
    regions.sort_by(|a, b| {
        a.bounds
            .y
            .total_cmp(&b.bounds.y)
            .then_with(|| a.bounds.x.total_cmp(&b.bounds.x))
    });

    let mut start = 0;
    while start < regions.len() {
        // Anchor each band to its first y; pairwise tolerance is not transitive.
        let anchor = regions[start].bounds.y;
        let mut end = start + 1;
        while end < regions.len() && regions[end].bounds.y - anchor < PARAGRAPH_MERGE_THRESHOLD {
            end += 1;
        }
        regions[start..end].sort_by(|a, b| a.bounds.x.total_cmp(&b.bounds.x));
        start = end;
    }
}

impl TextRegion {
    fn contains(&self, point: Point) -> bool {
        self.bounds.contains(point) && self.clip.is_none_or(|clip| clip.contains(point))
    }

    fn selection_bounds(&self) -> impl Iterator<Item = Rectangle> + '_ {
        std::iter::once(self.bounds)
            .take(usize::from(self.shaped_selection_bounds.is_empty()))
            .chain(self.shaped_selection_bounds.iter().copied())
            .filter_map(|bounds| match self.clip {
                Some(clip) => bounds.intersection(&clip),
                None => Some(bounds),
            })
    }

    fn char_offset_at(&self, point: Point) -> usize {
        if let Some(offset) = self.shaped_hit {
            return offset.min(self.text.chars().count());
        }
        // Legacy text producers expose no shaped hit testing.
        let char_count = self.text.chars().count();
        if char_count == 0 || self.bounds.width <= 0.0 {
            return 0;
        }
        let x_frac = ((point.x - self.bounds.x) / self.bounds.width).clamp(0.0, 1.0);
        ((x_frac * char_count as f32).round() as usize).min(char_count)
    }
}

// Inline link names remain accessibility metadata; their native paragraph owns selection text.
#[derive(Debug, Clone, Copy)]
pub(crate) enum CompatibilityTextBoundary {
    Begin,
    End,
}

struct CollectTextRegions {
    regions: Vec<TextRegion>,
    compatibility_depth: usize,
    excluded_control_bounds: Vec<Rectangle>,
    list_items: Vec<(Option<widget::Id>, Rectangle, bool)>,
    activation_targets: Vec<widget::Id>,
    query_point: Option<Point>,
    collect_keyboard_carets: bool,
    cached_text: Vec<Option<Arc<str>>>,
    next_collection_index: usize,
    next_shaped_index: usize,
    scroll: ScrollStack,
    work: SelectionScopeWork,
    // A row marker seen just before the traversal that enters its content.
    pending_row: Option<u64>,
    // Open rows and the next paragraph index inside each.
    rows: Vec<(u64, usize)>,
}

impl CollectTextRegions {
    fn new(
        query_point: Option<Point>,
        collect_keyboard_carets: bool,
        cached_text: Vec<Option<Arc<str>>>,
    ) -> Self {
        Self {
            regions: Vec::new(),
            compatibility_depth: 0,
            excluded_control_bounds: Vec::new(),
            list_items: Vec::new(),
            activation_targets: Vec::new(),
            query_point,
            collect_keyboard_carets,
            cached_text,
            next_collection_index: 0,
            next_shaped_index: 0,
            scroll: ScrollStack::new(),
            work: SelectionScopeWork::default(),
            pending_row: None,
            rows: Vec::new(),
        }
    }

    /// The row tag for the next region pushed inside the innermost open row.
    fn next_row_tag(&mut self) -> Option<(u64, usize)> {
        let (key, next) = self.rows.last_mut()?;
        let tag = (*key, *next);
        *next += 1;
        Some(tag)
    }

    /// Drops semantics labels that name the container `text` is shown in: a
    /// table cell or labelled row whose accessible name is the text it
    /// shows. The text is collected once, as itself. Only the labels
    /// collected since the last real text are candidates.
    fn drop_labels_shown_as(&mut self, bounds: Rectangle, text: &str) {
        let tail = self
            .regions
            .iter()
            .rev()
            .take_while(|region| region.compatibility_semantics)
            .count();
        let start = self.regions.len() - tail;
        let center = bounds.center();
        let mut index = start;
        while index < self.regions.len() {
            let region = &self.regions[index];
            if region.text.as_ref() == text && region.bounds.contains(center) {
                self.regions.remove(index);
            } else {
                index += 1;
            }
        }
    }

    fn next_collection(&mut self) -> usize {
        let index = self.next_collection_index;
        self.next_collection_index += 1;
        self.work.collection_visits += 1;
        index
    }

    fn reuse_text(&mut self, collection_index: usize, text: &str) -> Arc<str> {
        if let Some(cached) = self
            .cached_text
            .get(collection_index)
            .and_then(Option::as_ref)
            && cached.as_ref() == text
        {
            return Arc::clone(cached);
        }
        self.work.copied_text_bytes += text.len();
        Arc::from(text)
    }
}

fn shaped_carets(
    paragraph: usize,
    text: &str,
    bounds: Rectangle,
    shaped: &dyn iced::advanced::widget::operation::SelectableTextLayout,
) -> Vec<ShapedCaret> {
    if let Some(graphemes) = shaped.grapheme_bounds() {
        return carets_from_graphemes(paragraph, text, bounds, graphemes);
    }
    let mut carets = Vec::new();
    let mut char_offset = 0;

    for (byte_start, grapheme) in text.grapheme_indices(true) {
        let byte_end = byte_start + grapheme.len();
        let char_end = char_offset + grapheme.chars().count();
        for rect in shaped.range_bounds(byte_start..byte_end) {
            if rect.height <= 0.0 {
                continue;
            }
            let probe_y = rect.y + rect.height / 2.0;
            let inset = (rect.width * 0.2).min(1.0);
            let left_hit = shaped.hit_test(Point::new(rect.x + inset, probe_y));
            let right_hit = shaped.hit_test(Point::new(rect.x + rect.width - inset, probe_y));
            let rtl = matches!((left_hit, right_hit), (Some(left), Some(right)) if left > right);
            let left = bounds.x + rect.x;
            let right = left + rect.width;
            let y = bounds.y + probe_y;
            carets.push(ShapedCaret {
                position: DocPosition::new(paragraph, char_offset),
                affinity: CaretAffinity::Downstream,
                point: Point::new(if rtl { right } else { left }, y),
                line_height: rect.height,
            });
            carets.push(ShapedCaret {
                position: DocPosition::new(paragraph, char_end),
                affinity: CaretAffinity::Upstream,
                point: Point::new(if rtl { left } else { right }, y),
                line_height: rect.height,
            });
        }
        char_offset = char_end;
    }

    carets
}

fn carets_from_graphemes(
    paragraph: usize,
    text: &str,
    bounds: Rectangle,
    mut graphemes: Vec<iced::advanced::text::paragraph::GraphemeBounds>,
) -> Vec<ShapedCaret> {
    graphemes.sort_by_key(|grapheme| grapheme.range.start);
    let mut char_at = vec![0; text.len() + 1];
    let mut chars = 0;
    for (byte, _) in text.char_indices() {
        char_at[byte] = chars;
        chars += 1;
    }
    char_at[text.len()] = chars;
    let char_of = |byte: usize| char_at.get(byte).copied().unwrap_or(chars);

    let mut carets = Vec::with_capacity(graphemes.len() * 2);
    for grapheme in graphemes {
        let rect = grapheme.bounds;
        if rect.height <= 0.0 || grapheme.range.end > text.len() {
            continue;
        }
        let left = bounds.x + rect.x;
        let right = left + rect.width;
        let y = bounds.y + rect.y + rect.height / 2.0;
        carets.push(ShapedCaret {
            position: DocPosition::new(paragraph, char_of(grapheme.range.start)),
            affinity: CaretAffinity::Downstream,
            point: Point::new(if grapheme.rtl { right } else { left }, y),
            line_height: rect.height,
        });
        carets.push(ShapedCaret {
            position: DocPosition::new(paragraph, char_of(grapheme.range.end)),
            affinity: CaretAffinity::Upstream,
            point: Point::new(if grapheme.rtl { left } else { right }, y),
            line_height: rect.height,
        });
    }
    carets
}

impl<T> Operation<T> for CollectTextRegions {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        let row = self.pending_row.take();
        if let Some(key) = row {
            self.rows.push((key, 0));
        }
        self.scroll.traverse_enter();
        operate(self);
        self.scroll.traverse_exit();
        if row.is_some() {
            self.rows.pop();
        }
    }

    fn scrollable(
        &mut self,
        id: Option<&widget::Id>,
        bounds: Rectangle,
        _content_bounds: Size,
        translation: Vector,
        _state: &mut dyn widget::operation::Scrollable,
    ) {
        self.scroll.note_scrollable(id, bounds, translation);
    }

    fn text(&mut self, _id: Option<&widget::Id>, bounds: Rectangle, text: &str) {
        let bounds = self.scroll.translate(bounds);
        let collection_index = self.next_collection();
        if text.trim().is_empty() {
            return;
        }
        self.drop_labels_shown_as(bounds, text);
        let text = self.reuse_text(collection_index, text);
        let row = self.next_row_tag();
        self.regions.push(TextRegion {
            collection_index,
            compatibility_collection_index: None,
            compatibility_semantics: false,
            shaped_index: None,
            text,
            bounds,
            clip: self.scroll.viewport(),
            shaped_hit: None,
            query_line_has_text: None,
            shaped_selection_bounds: Vec::new(),
            keyboard_carets: Vec::new(),
            row,
        });
    }

    fn selectable_text(
        &mut self,
        _id: Option<&widget::Id>,
        bounds: Rectangle,
        text: &str,
        shaped: &dyn iced::advanced::widget::operation::SelectableTextLayout,
    ) {
        let bounds = self.scroll.translate(bounds);
        let collection_index = self.next_collection();
        let shaped_index = self.next_shaped_index;
        self.next_shaped_index += 1;
        if text.trim().is_empty() {
            return;
        }
        // Prose wrappers may emit a compatibility node immediately before shaped text.
        let compatibility_collection_index = self
            .regions
            .last()
            .filter(|last| {
                last.compatibility_semantics && last.bounds == bounds && last.text.as_ref() == text
            })
            .map(|last| last.collection_index);
        // The shaped text replaces the compatibility node, keeping its row slot.
        let replaced_row = if compatibility_collection_index.is_some() {
            self.regions.pop().and_then(|region| region.row)
        } else {
            None
        };
        self.drop_labels_shown_as(bounds, text);
        let shaped_hit_bytes = self.query_point.and_then(|point| {
            // Clamp before shaped hit testing so a distant drag picks the nearest wrapped line.
            let clamped = Point::new(
                point.x.clamp(bounds.x, bounds.x + bounds.width),
                point.y.clamp(bounds.y, bounds.y + bounds.height),
            );
            let local = Point::new(clamped.x - bounds.x, clamped.y - bounds.y);
            shaped.hit_test(local).map(|byte| {
                (0..=byte.min(text.len()))
                    .rev()
                    .find(|&candidate| text.is_char_boundary(candidate))
                    .unwrap_or(0)
            })
        });
        let shaped_hit = shaped_hit_bytes.map(|byte| text[..byte].chars().count());
        // Broad paragraph bounds include blank lines that cannot start a selection.
        let query_line_has_text = self.query_point.map(|point| {
            if !bounds.contains(point)
                || !self.scroll.intersects_viewport(bounds)
                || shaped_hit_bytes.is_none()
            {
                return false;
            }
            let local_y = point.y - bounds.y;
            let floor = |byte: usize| {
                (0..=byte.min(text.len()))
                    .rev()
                    .find(|&candidate| text.is_char_boundary(candidate))
                    .unwrap_or(0)
            };
            let ends = [0.0, bounds.width].map(|x| shaped.hit_test(Point::new(x, local_y)));
            let [Some(left), Some(right)] = ends else {
                return false;
            };
            let line = floor(left.min(right))..floor(left.max(right));
            let slice = &text[line.clone()];
            let Some(first) = slice.find(|ch: char| !ch.is_whitespace()) else {
                return false;
            };
            let ink = line.start + first..line.start + slice.trim_end().len();
            shaped
                .range_bounds(ink)
                .into_iter()
                .any(|rect| local_y >= rect.y && local_y < rect.y + rect.height)
        });
        let keyboard_carets = if self.collect_keyboard_carets {
            shaped_carets(self.regions.len(), text, bounds, shaped)
        } else {
            Default::default()
        };
        let text = self.reuse_text(collection_index, text);
        let row = replaced_row.or_else(|| self.next_row_tag());
        self.regions.push(TextRegion {
            collection_index,
            compatibility_collection_index,
            compatibility_semantics: false,
            shaped_index: Some(shaped_index),
            text,
            bounds,
            clip: self.scroll.viewport(),
            shaped_hit,
            query_line_has_text,
            shaped_selection_bounds: Vec::new(),
            keyboard_carets,
            row,
        });
    }

    fn custom(
        &mut self,
        id: Option<&widget::Id>,
        bounds: Rectangle,
        state: &mut dyn std::any::Any,
    ) {
        if let Some(marker) = state.downcast_ref::<SelectionRowMarker>() {
            self.pending_row = Some(marker.0);
            return;
        }
        let bounds = self.scroll.translate(bounds);
        let collection_index = self.next_collection();
        if let Some(boundary) = state.downcast_ref::<CompatibilityTextBoundary>() {
            match boundary {
                CompatibilityTextBoundary::Begin => self.compatibility_depth += 1,
                CompatibilityTextBoundary::End => {
                    self.compatibility_depth = self
                        .compatibility_depth
                        .checked_sub(1)
                        .expect("paired compatibility text boundary");
                }
            }
            return;
        }
        if let Some(request) = state.downcast_ref::<crate::activation::ActivationRequest>() {
            self.activation_targets.push(request.id().clone());
            return;
        }
        if let Some(node) = state.downcast_ref::<enzui_core::collect::SemanticsNode>() {
            use enzui_core::semantics::Role;
            // A control's caption and accessible name are not document text.
            if matches!(
                node.semantics.role,
                Role::Button | Role::Document | Role::CheckBox | Role::Switch | Role::RadioButton
            ) {
                self.excluded_control_bounds.push(bounds);
                return;
            }
            if node.semantics.role == Role::ListItem {
                self.list_items
                    .push((id.cloned(), bounds, node.semantics.supports_default_action));
                return;
            }
            if self.compatibility_depth == 0
                && let Some(label) = node.semantics.label.as_deref()
                && !label.trim().is_empty()
            {
                let text = self.reuse_text(collection_index, label);
                let row = self.next_row_tag();
                self.regions.push(TextRegion {
                    collection_index,
                    compatibility_collection_index: None,
                    compatibility_semantics: true,
                    shaped_index: None,
                    text,
                    bounds,
                    clip: self.scroll.viewport(),
                    shaped_hit: None,
                    query_line_has_text: None,
                    shaped_selection_bounds: Vec::new(),
                    keyboard_carets: Vec::new(),
                    row,
                });
            }
        }
    }
}

fn selected_char_range(
    selection: (DocPosition, DocPosition),
    paragraph: usize,
    char_count: usize,
) -> Option<std::ops::Range<usize>> {
    let (start, end) = selection;
    if paragraph < start.paragraph || paragraph > end.paragraph {
        return None;
    }
    let local_start = if paragraph == start.paragraph {
        start.offset
    } else {
        0
    };
    let local_end = if paragraph == end.paragraph {
        end.offset
    } else {
        char_count
    };
    (local_start < local_end).then(|| local_start.min(char_count)..local_end.min(char_count))
}

struct CollectSelectionGeometry<'a> {
    regions: &'a mut [TextRegion],
    region_by_shaped_index: Vec<Option<usize>>,
    next_shaped_index: usize,
    selection: (DocPosition, DocPosition),
    indexed_regions: usize,
    visits: usize,
    lookups: usize,
    comparisons: usize,
    scroll: ScrollStack,
}

impl<'a> CollectSelectionGeometry<'a> {
    fn new(regions: &'a mut [TextRegion], selection: (DocPosition, DocPosition)) -> Self {
        let shaped_count = regions
            .iter()
            .filter_map(|region| region.shaped_index)
            .max()
            .map_or(0, |index| index + 1);
        let mut region_by_shaped_index = vec![None; shaped_count];
        let mut indexed_regions = 0;
        for (region_index, region) in regions.iter().enumerate() {
            if let Some(shaped_index) = region.shaped_index {
                region_by_shaped_index[shaped_index] = Some(region_index);
                indexed_regions += 1;
            }
        }
        Self {
            regions,
            region_by_shaped_index,
            next_shaped_index: 0,
            selection,
            indexed_regions,
            visits: 0,
            lookups: 0,
            comparisons: 0,
            scroll: ScrollStack::new(),
        }
    }
}

impl<T> Operation<T> for CollectSelectionGeometry<'_> {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        self.scroll.traverse_enter();
        operate(self);
        self.scroll.traverse_exit();
    }

    fn scrollable(
        &mut self,
        id: Option<&widget::Id>,
        bounds: Rectangle,
        _content_bounds: Size,
        translation: Vector,
        _state: &mut dyn widget::operation::Scrollable,
    ) {
        self.scroll.note_scrollable(id, bounds, translation);
    }

    fn selectable_text(
        &mut self,
        _id: Option<&widget::Id>,
        bounds: Rectangle,
        text: &str,
        shaped: &dyn iced::advanced::widget::operation::SelectableTextLayout,
    ) {
        let bounds = self.scroll.translate(bounds);
        let shaped_index = self.next_shaped_index;
        self.next_shaped_index += 1;
        self.visits += 1;
        self.lookups += 1;
        let Some(paragraph) = self
            .region_by_shaped_index
            .get(shaped_index)
            .copied()
            .flatten()
        else {
            return;
        };
        let char_count = text.chars().count();
        let Some(range) = selected_char_range(self.selection, paragraph, char_count) else {
            return;
        };
        let byte_at = |char_offset: usize| {
            text.char_indices()
                .nth(char_offset)
                .map_or(text.len(), |(byte, _)| byte)
        };
        self.regions[paragraph].shaped_selection_bounds = shaped
            .range_bounds(byte_at(range.start)..byte_at(range.end))
            .into_iter()
            .map(|rect| rect + (bounds.position() - Point::ORIGIN))
            .collect();
    }
}

/// The whole document a virtualised scope shows only part of, so a selection
/// can keep endpoints in rows that scroll out of the mounted window and copy
/// text from rows that were never mounted. Rows are identified by the stable
/// key each one's [`enz_selection_row`] carries (see [`selection_row_key`]).
pub trait SelectionDocument {
    /// The current reading-order index of the row with `key`.
    fn row_index(&self, key: u64) -> Option<usize>;
    /// The row's selectable paragraphs at `index`, in the order and with the
    /// text the row renders them; `None` past the end.
    fn row_paragraphs(&self, index: usize) -> Option<Vec<String>>;

    /// The row's paragraphs as `renderer` lays it out at `width`. The scope
    /// asks through this, so a document can build the row and read its
    /// widgets ([`RenderedRows`]); the default answers
    /// [`row_paragraphs`](Self::row_paragraphs).
    fn rendered_row_paragraphs(
        &self,
        index: usize,
        renderer: &iced::Renderer,
        width: f32,
    ) -> Option<Vec<String>> {
        let _ = (renderer, width);
        self.row_paragraphs(index)
    }
}

/// Hashes a row identity, such as a virtual list's sequence key, into the
/// key [`enz_selection_row`] and [`SelectionDocument::row_index`] share.
#[must_use]
pub fn selection_row_key(key: impl Hash) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
}

/// Marks `content` as the logical row `key` of the enclosing scope's
/// [`SelectionDocument`]. Its text regions, in reading order, are the row's
/// paragraphs.
pub fn enz_selection_row<'a, Message: 'a>(
    key: u64,
    content: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    Element::new(SelectionRow {
        key,
        content: content.into(),
    })
}

/// A position in a logical row: its key, paragraph and character offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LogicalPosition {
    key: u64,
    paragraph: usize,
    offset: usize,
}

struct SelectionRowMarker(u64);

struct SelectionRow<'a, Message> {
    key: u64,
    content: Element<'a, Message>,
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for SelectionRow<'_, Message> {
    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let child = self
            .content
            .as_widget_mut()
            .measure(&mut tree.children[0], renderer, limits);
        layout::Node::with_children(child.size(), vec![child]).apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let child = layout.children(&tree.children).next().unwrap_or(layout);
        operation.custom(None, layout.bounds(), &mut SelectionRowMarker(self.key));
        operation.traverse(&mut |operation| {
            self.content.as_widget_mut().operate(
                &mut tree.children[0],
                child,
                viewport,
                renderer,
                operation,
            );
        });
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
        let child = layout.children(&tree.children).next().unwrap_or(layout);
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            child,
            cursor,
            renderer,
            shell,
            viewport,
        );
        crate::frame_routing::refresh_opaque_child(&self.content, tree, 0);
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        crate::frame_routing::interest_opaque(tree)
    }

    fn update_frame(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let child = layout.children(&tree.children).next().unwrap_or(layout);
        crate::frame_routing::forward_frame_opaque(
            &mut self.content,
            tree,
            0,
            event,
            child,
            cursor,
            renderer,
            shell,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let child = layout.children(&tree.children).next().unwrap_or(layout);
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            child,
            cursor,
            viewport,
            renderer,
        )
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
        let child = layout.children(&tree.children).next().unwrap_or(layout);
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            child,
            cursor,
            viewport,
        );
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
        let child = layout.children(&tree.children).next().unwrap_or(layout);
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            child,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

/// Wrap content in a read-only selection scope with no callbacks or explicit identity.
pub fn enz_selection_scope<'a, Message>(
    content: impl Into<Element<'a, Message>>,
) -> EnzSelectionScope<'a, Message>
where
    Message: 'a,
{
    EnzSelectionScope::new(content)
}

/// A document focus stop for cross-widget selection and context actions.
#[must_use]
pub struct EnzSelectionScope<'a, Message> {
    child: Element<'a, Message>,
    id: Option<widget::Id>,
    label: Option<String>,
    on_copy: Option<Box<dyn Fn(String) -> Message + 'a>>,
    on_selection_change: Option<Box<dyn Fn(Option<ScopeSelection>) -> Message + 'a>>,
    on_context: Option<Box<dyn Fn(ScopeSelection) -> Message + 'a>>,
    on_context_invocation: Option<Box<dyn Fn(SelectionContextInvocation) -> Message + 'a>>,
    context_id: Option<widget::Id>,
    context_label: Option<String>,
    document_key: Option<u64>,
    clear_generation: u64,
    document: Option<ScopeDocument<'a>>,
    yields: bool,
}

/// A scope's [`SelectionDocument`], borrowed from the caller or owned by the
/// scope.
enum ScopeDocument<'a> {
    Borrowed(&'a dyn SelectionDocument),
    Owned(Box<dyn SelectionDocument + 'a>),
}

/// The document a scope selects over, if it has one.
fn scope_document<'d>(
    document: Option<&'d ScopeDocument<'_>>,
) -> Option<&'d dyn SelectionDocument> {
    match document? {
        ScopeDocument::Borrowed(document) => Some(*document),
        ScopeDocument::Owned(document) => Some(document.as_ref()),
    }
}

impl<'a, Message> EnzSelectionScope<'a, Message> {
    /// Wrap content with no callbacks, document key, or explicit identity.
    pub fn new(content: impl Into<Element<'a, Message>>) -> Self {
        Self {
            child: content.into(),
            id: None,
            label: None,
            on_copy: None,
            on_selection_change: None,
            on_context: None,
            on_context_invocation: None,
            context_id: None,
            context_label: None,
            document_key: None,
            clear_generation: 0,
            document: None,
            yields: false,
        }
    }
    /// Set the identity used by targeted focus operations and restoration.
    /// The scope participates in Tab traversal even without an identity.
    pub fn id(mut self, id: impl Into<widget::Id>) -> Self {
        self.id = Some(id.into());
        self
    }
    /// Set the document's accessible name; the default is “Selectable document”.
    /// A merged context action's label supplies the fallback when configured.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
    /// Handle the selected text on the focused scope's command-C shortcut.
    /// The application writes the clipboard; without a callback the scope
    /// writes the selected text to the clipboard itself.
    pub fn on_copy(mut self, f: impl Fn(String) -> Message + 'a) -> Self {
        self.on_copy = Some(Box::new(f));
        self
    }
    /// Handle live endpoint changes, including dragging, replacement, and clearing.
    /// Geometry-only changes do not republish an unchanged range; controlled clears are silent.
    pub fn on_selection_change(
        mut self,
        f: impl Fn(Option<ScopeSelection>) -> Message + 'a,
    ) -> Self {
        self.on_selection_change = Some(Box::new(f));
        self
    }
    /// Handle context requests with the live selection.
    /// Secondary presses must be inside the selection; keyboard requests require scope focus.
    /// Routed assistive-technology requests are also supported. Replaces
    /// [`on_context_invocation`](Self::on_context_invocation).
    pub fn on_context(mut self, f: impl Fn(ScopeSelection) -> Message + 'a) -> Self {
        self.on_context = Some(Box::new(f));
        self.on_context_invocation = None;
        self
    }
    /// Handle a selection context request with placement and focus restoration information.
    /// Pointer requests retain focus; keyboard requests use selection bounds.
    /// Replaces [`on_context`](Self::on_context).
    pub fn on_context_invocation(
        mut self,
        f: impl Fn(SelectionContextInvocation) -> Message + 'a,
    ) -> Self {
        self.on_context_invocation = Some(Box::new(f));
        self.on_context = None;
        self
    }
    /// Set the identity for routed assistive-technology context requests.
    /// A callback and nonempty selection are required to advertise the action.
    /// Queued requests publish once on the next update or frame without moving focus.
    pub fn context_id(mut self, id: impl Into<widget::Id>) -> Self {
        self.context_id = Some(id.into());
        self
    }
    /// Set the accessible name of the selection context action.
    pub fn context_label(mut self, label: impl Into<String>) -> Self {
        self.context_label = Some(label.into());
        self
    }
    /// Hash a document identity to preserve selection across unrelated text changes.
    /// An equal key permits reindexing only when both endpoint texts are unique in
    /// the old and new region sets. A changed key clears silently; without a key,
    /// changed region text clears selection.
    pub fn document_key(mut self, key: impl Hash) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut hasher);
        self.document_key = Some(hasher.finish());
        self
    }
    /// The whole document behind a virtualised child, so a selection keeps
    /// endpoints in rows that scroll out of the mounted window and copies
    /// text from rows never mounted. Wrap each row in [`enz_selection_row`]
    /// with the key the document answers for. Without one, a selection spans
    /// only what is mounted.
    pub fn document(mut self, document: &'a dyn SelectionDocument) -> Self {
        self.document = Some(ScopeDocument::Borrowed(document));
        self
    }
    /// [`Self::document`] with a document the scope keeps, for one built in
    /// `view` that the returned element cannot borrow.
    pub fn owned_document(mut self, document: impl SelectionDocument + 'a) -> Self {
        self.document = Some(ScopeDocument::Owned(Box::new(document)));
        self
    }
    /// The scope's document is the rows the child shows: [`RenderedRows`]
    /// over `keys` in reading order, building row `index` with `row`. Pass
    /// the virtual list the same keys and the same builder; `row` can borrow
    /// the application state `view` borrows.
    pub fn rendered_rows(
        self,
        keys: impl IntoIterator<Item = u64>,
        row: impl Fn(usize) -> Element<'a, Message> + 'a,
    ) -> Self
    where
        Message: 'a,
    {
        self.owned_document(RenderedRows::new(keys, row))
    }
    /// Inside another selection scope, act as plain content: no focus stop,
    /// no selection of its own, so the enclosing scope selects across it. A
    /// widget that makes its own text selectable (a code block) sets this,
    /// so a selectable transcript around it still selects in one range.
    pub fn yield_to_enclosing(mut self, yields: bool) -> Self {
        self.yields = yields;
        self
    }
    /// Clear internal selection silently when this value changes on rebuild.
    /// Any unequal value clears, including a decrease; equal values preserve selection.
    pub fn clear_generation(mut self, generation: u64) -> Self {
        self.clear_generation = generation;
        self
    }
}

impl<'a, Message: 'a> From<EnzSelectionScope<'a, Message>> for Element<'a, Message>
where
    Message: Clone,
{
    fn from(scope: EnzSelectionScope<'a, Message>) -> Self {
        Element::new(SelectionScopeWidget {
            child: scope.child,
            id: scope.id,
            label: scope.label,
            on_copy: scope.on_copy,
            on_selection_change: scope.on_selection_change,
            on_context: scope.on_context,
            on_context_invocation: scope.on_context_invocation,
            context_id: scope.context_id,
            context_label: scope.context_label,
            document_key: scope.document_key,
            clear_generation: scope.clear_generation,
            document: scope.document,
            yields: scope.yields,
        })
    }
}

impl<'a, Message: Clone + 'a> EnzSelectionScope<'a, Message> {
    /// Build the selection element, retaining its state across compatible rebuilds.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        self.into()
    }
}

#[derive(Debug, Clone)]
struct ScopeState {
    copy_lifetime: enzui_core::cancellation::CancelToken,
    copy_range: Option<(DocPosition, DocPosition)>,
    focus: ScopeFocus,
    keyboard_caret: Option<ShapedCaret>,
    regions: Vec<TextRegion>,
    model: SelectionModel,
    over_text: bool,
    clicks: ClickDetector,
    published_range: Option<(DocPosition, DocPosition)>,
    clear_generation: u64,
    document_key: Option<u64>,
    work: SelectionScopeWork,
    context_menu_pending: bool,
    translation: Vector,
    // The selection in logical rows, kept while its endpoints are unmounted.
    logical: Option<(LogicalPosition, LogicalPosition)>,
    // The mounted (anchor, focus) the last refresh left in `model`.
    projection: Option<(DocPosition, DocPosition)>,
    // The child's width at the last refresh: unmounted rows are laid out at it.
    width: f32,
}

impl Default for ScopeState {
    fn default() -> Self {
        Self {
            copy_lifetime: enzui_core::cancellation::CancelToken::new(),
            copy_range: None,
            focus: ScopeFocus::default(),
            keyboard_caret: None,
            regions: Vec::new(),
            model: SelectionModel::new(Vec::<String>::new()),
            over_text: false,
            clicks: ClickDetector::new(SingleClickPolicy::Immediate),
            published_range: None,
            clear_generation: 0,
            document_key: None,
            work: SelectionScopeWork::default(),
            context_menu_pending: false,
            translation: Vector::ZERO,
            logical: None,
            projection: None,
            width: 0.0,
        }
    }
}

impl Drop for ScopeState {
    fn drop(&mut self) {
        self.copy_lifetime
            .cancel(enzui_core::cancellation::CancelCause::Cancelled);
    }
}

thread_local! {
    /// How many selection scopes the widget call in progress is inside.
    static SCOPE_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Holds one level of [`SCOPE_DEPTH`] while a scope's layout, update or operation
/// descends into its child.
struct ScopeDepth;

impl ScopeDepth {
    fn enter() -> Self {
        SCOPE_DEPTH.with(|depth| depth.set(depth.get() + 1));
        Self
    }

    fn enclosed() -> bool {
        SCOPE_DEPTH.with(std::cell::Cell::get) > 0
    }
}

impl Drop for ScopeDepth {
    fn drop(&mut self) {
        SCOPE_DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

fn build_scope_selection(
    state: &ScopeState,
    document: Option<&dyn SelectionDocument>,
    renderer: &iced::Renderer,
    offset: Vector,
) -> Option<ScopeSelection> {
    let (model, regions) = (&state.model, &state.regions);
    let sel = model.selection()?;
    // A logical selection scrolled wholly out of view is still a selection.
    if sel.is_empty() && state.logical.is_none() {
        return None;
    }
    let (start, end) = sel.range();
    let text = selected_scope_text(state, document, renderer);
    if text.is_empty() {
        return None;
    }
    let selected_indices = model.selected_paragraphs();
    let bounds = selected_indices
        .iter()
        .filter_map(|&idx| regions.get(idx))
        .flat_map(TextRegion::selection_bounds)
        .reduce(|a, b| {
            let x = a.x.min(b.x);
            let y = a.y.min(b.y);
            let right = (a.x + a.width).max(b.x + b.width);
            let bottom = (a.y + a.height).max(b.y + b.height);
            Rectangle::new(Point::new(x, y), Size::new(right - x, bottom - y))
        })
        .unwrap_or(Rectangle::new(Point::ORIGIN, Size::ZERO));
    Some(ScopeSelection {
        text,
        start,
        end,
        bounds: bounds + offset,
    })
}

fn selection_contains_point(model: &SelectionModel, regions: &[TextRegion], point: Point) -> bool {
    model.selected_paragraphs().into_iter().any(|index| {
        regions.get(index).is_some_and(|region| {
            region
                .selection_bounds()
                .any(|bounds| bounds.contains(point))
        })
    })
}

fn matching_paragraph(old_texts: &[&str], new_texts: &[&str], old_index: usize) -> Option<usize> {
    if old_index >= old_texts.len() {
        return None;
    }
    let text = old_texts[old_index];
    if old_texts
        .iter()
        .filter(|candidate| **candidate == text)
        .count()
        != 1
        || new_texts
            .iter()
            .filter(|candidate| **candidate == text)
            .count()
            != 1
    {
        return None;
    }
    let prefix = old_texts
        .iter()
        .zip(new_texts)
        .take_while(|(old, new)| old == new)
        .count();
    if old_index < prefix {
        return Some(old_index);
    }
    let suffix = old_texts[prefix..]
        .iter()
        .rev()
        .zip(new_texts[prefix..].iter().rev())
        .take_while(|(old, new)| old == new)
        .count();
    if old_index >= old_texts.len().saturating_sub(suffix) {
        return Some(new_texts.len() - (old_texts.len() - old_index));
    }

    new_texts.iter().position(|candidate| *candidate == text)
}

fn rebuild_model(
    old_model: &SelectionModel,
    old_texts: &[&str],
    new_texts: Vec<String>,
    preserve_selection: bool,
) -> SelectionModel {
    let old_selection = preserve_selection.then(|| old_model.selection()).flatten();
    let was_dragging = old_model.is_dragging();
    let new_refs: Vec<&str> = new_texts.iter().map(String::as_str).collect();
    let mapped = old_selection.and_then(|selection| {
        let anchor = matching_paragraph(old_texts, &new_refs, selection.anchor.paragraph)?;
        let focus = matching_paragraph(old_texts, &new_refs, selection.focus.paragraph)?;
        Some((
            DocPosition::new(anchor, selection.anchor.offset),
            DocPosition::new(focus, selection.focus.offset),
        ))
    });
    let mut model = SelectionModel::new(new_texts);
    if let Some((anchor, focus)) = mapped {
        model.press(anchor);
        model.drag_to(focus);
        if !was_dragging {
            let _ = model.release();
        }
    }
    model
}

/// The logical position of a mounted model position, when its region sits in
/// an [`enz_selection_row`].
fn logical_position(regions: &[TextRegion], position: DocPosition) -> Option<LogicalPosition> {
    let (key, paragraph) = regions.get(position.paragraph)?.row?;
    Some(LogicalPosition {
        key,
        paragraph,
        offset: position.offset,
    })
}

/// Records selection changes the last input made in the mounted model as
/// logical endpoints. An endpoint the refresh projected, and that input left
/// alone, keeps its logical position even while its row is unmounted.
fn sync_logical_selection(state: &mut ScopeState, document: Option<&dyn SelectionDocument>) {
    if document.is_none() {
        state.logical = None;
        return;
    }
    let Some(selection) = state.model.selection() else {
        state.logical = None;
        return;
    };
    let (anchor, focus) = (selection.anchor, selection.focus);
    let projected = state.projection;
    let logical_anchor = match (state.logical, projected) {
        (Some((logical, _)), Some((was, _))) if was == anchor => Some(logical),
        _ => logical_position(&state.regions, anchor),
    };
    let logical_focus = match (state.logical, projected) {
        (Some((_, logical)), Some((_, was))) if was == focus => Some(logical),
        _ => logical_position(&state.regions, focus),
    };
    state.logical = logical_anchor.zip(logical_focus);
}

/// Where a logical endpoint falls in the mounted regions: its own region
/// when mounted, else the near edge of the mounted rows.
fn mounted_position(
    regions: &[TextRegion],
    texts: &[String],
    position: LogicalPosition,
    document: &dyn SelectionDocument,
) -> Option<DocPosition> {
    if let Some(index) = regions
        .iter()
        .position(|region| region.row == Some((position.key, position.paragraph)))
    {
        return Some(DocPosition::new(index, position.offset));
    }
    let target = document.row_index(position.key)?;
    let tagged: Vec<(usize, (usize, usize))> = regions
        .iter()
        .enumerate()
        .filter_map(|(index, region)| {
            let (key, paragraph) = region.row?;
            Some((index, (document.row_index(key)?, paragraph)))
        })
        .collect();
    let wanted = (target, position.paragraph);
    let first = tagged.first()?;
    let last = tagged.last()?;
    if wanted < first.1 {
        Some(DocPosition::new(first.0, 0))
    } else if wanted > last.1 {
        Some(DocPosition::new(last.0, texts[last.0].chars().count()))
    } else {
        // Between mounted rows (a paragraph the row no longer has): the next one.
        let (index, _) = tagged.iter().find(|(_, order)| *order > wanted)?;
        Some(DocPosition::new(*index, 0))
    }
}

/// A model over the new mounted regions holding the logical selection, its
/// unmounted endpoints clamped to the edge of what is mounted.
fn project_logical_selection(
    old_model: &SelectionModel,
    regions: &[TextRegion],
    texts: Vec<String>,
    (anchor, focus): (LogicalPosition, LogicalPosition),
    document: &dyn SelectionDocument,
) -> SelectionModel {
    let was_dragging = old_model.is_dragging();
    let mounted = mounted_position(regions, &texts, anchor, document)
        .zip(mounted_position(regions, &texts, focus, document));
    let mut model = SelectionModel::new(texts);
    if let Some((anchor, focus)) = mounted {
        model.press(anchor);
        model.drag_to(focus);
        if !was_dragging {
            let _ = model.release();
        }
    }
    model
}

/// The selected text, across unmounted rows when the scope has a document.
fn selected_scope_text(
    state: &ScopeState,
    document: Option<&dyn SelectionDocument>,
    renderer: &iced::Renderer,
) -> String {
    if let (Some(document), Some((anchor, focus))) = (document, state.logical)
        && let Some(text) = logical_selected_text(document, anchor, focus, renderer, state.width)
    {
        return text;
    }
    state.model.selected_text()
}

fn logical_selected_text(
    document: &dyn SelectionDocument,
    anchor: LogicalPosition,
    focus: LogicalPosition,
    renderer: &iced::Renderer,
    width: f32,
) -> Option<String> {
    let order = |position: LogicalPosition| {
        document
            .row_index(position.key)
            .map(|row| (row, position.paragraph, position.offset))
    };
    let (a, b) = (order(anchor)?, order(focus)?);
    let (start, end) = if a <= b { (a, b) } else { (b, a) };
    // Only the rows between the endpoints are materialised.
    let mut paragraphs = Vec::new();
    let mut start_base = 0;
    let mut end_base = 0;
    for row in start.0..=end.0 {
        if row == start.0 {
            start_base = paragraphs.len();
        }
        if row == end.0 {
            end_base = paragraphs.len();
        }
        paragraphs.extend(document.rendered_row_paragraphs(row, renderer, width)?);
    }
    let mut model = SelectionModel::new(paragraphs);
    model.press(DocPosition::new(start_base + start.1, start.2));
    model.drag_to(DocPosition::new(end_base + end.1, end.2));
    Some(model.selected_text())
}

/// Collects `child`'s selectable text in reading order, without the text of
/// actionable controls. The scope and [`RenderedRows`] share this one
/// traversal, so a row built off screen yields the paragraphs it has when
/// mounted. Each row's paragraphs are numbered in this final order.
fn collect_text_regions<M>(
    child: &mut Element<'_, M>,
    child_tree: &mut Tree,
    child_layout: Layout,
    renderer: &iced::Renderer,
    collect: &mut CollectTextRegions,
) {
    child.as_widget_mut().operate(
        child_tree,
        child_layout,
        &child_layout.bounds(),
        renderer,
        collect,
    );
    // Activation requests arrive after ListItem semantics; resolve row exclusion after traversal.
    for (id, bounds, supports_default_action) in &collect.list_items {
        let actionable = id.as_ref().map_or(*supports_default_action, |id| {
            collect.activation_targets.iter().any(|target| target == id)
        });
        if actionable {
            collect.excluded_control_bounds.push(*bounds);
        }
    }
    let excluded_control_bounds = &collect.excluded_control_bounds;
    collect.regions.retain(|region| {
        !excluded_control_bounds
            .iter()
            .any(|control| control.contains(region.bounds.center()))
    });

    sort_reading_order(&mut collect.regions);

    let mut next = std::collections::HashMap::<u64, usize>::new();
    for region in &mut collect.regions {
        if let Some((key, paragraph)) = &mut region.row {
            let count = next.entry(*key).or_default();
            *paragraph = *count;
            *count += 1;
        }
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "the widget-tree arguments iced passes through"
)]
fn refresh_text_regions(
    child: &mut Element<'_, impl Clone>,
    child_tree: &mut Tree,
    child_layout: Layout,
    renderer: &iced::Renderer,
    state: &mut ScopeState,
    query_point: Option<Point>,
    collect_keyboard_carets: bool,
    document: Option<&dyn SelectionDocument>,
) {
    sync_logical_selection(state, document);
    let cache_len = state
        .regions
        .iter()
        .map(|region| region.collection_index)
        .max()
        .map_or(0, |index| index + 1);
    let mut cached_text = vec![None; cache_len];
    for region in &state.regions {
        cached_text[region.collection_index] = Some(Arc::clone(&region.text));
        if let Some(index) = region.compatibility_collection_index {
            cached_text[index] = Some(Arc::clone(&region.text));
        }
    }
    let mut collect = CollectTextRegions::new(query_point, collect_keyboard_carets, cached_text);
    collect_text_regions(child, child_tree, child_layout, renderer, &mut collect);
    state.width = child_layout.bounds().width;

    let old_texts: Vec<&str> = state
        .regions
        .iter()
        .map(|region| region.text.as_ref())
        .collect();
    let new_texts: Vec<&str> = collect
        .regions
        .iter()
        .map(|region| region.text.as_ref())
        .collect();
    if old_texts != new_texts {
        collect.work.copied_text_bytes += collect
            .regions
            .iter()
            .map(|region| region.text.len())
            .sum::<usize>();
        let texts = collect
            .regions
            .iter()
            .map(|region| region.text.to_string())
            .collect();
        state.model = match (document, state.logical) {
            (Some(document), Some(logical)) => {
                project_logical_selection(&state.model, &collect.regions, texts, logical, document)
            }
            _ => rebuild_model(
                &state.model,
                &old_texts,
                texts,
                state.document_key.is_some(),
            ),
        };
    }
    state.projection = state
        .model
        .selection()
        .map(|selection| (selection.anchor, selection.focus));

    if let Some(selection) = state.model.selection().map(|selection| selection.range()) {
        let (indexed_regions, visits, lookups, comparisons) = {
            let mut geometry = CollectSelectionGeometry::new(&mut collect.regions, selection);
            child.as_widget_mut().operate(
                child_tree,
                child_layout,
                &child_layout.bounds(),
                renderer,
                &mut geometry,
            );
            (
                geometry.indexed_regions,
                geometry.visits,
                geometry.lookups,
                geometry.comparisons,
            )
        };
        collect.work.geometry_indexed_regions = indexed_regions;
        collect.work.geometry_visits = visits;
        collect.work.geometry_lookups = lookups;
        collect.work.geometry_comparisons = comparisons;
    }

    // Cached geometry stays relative to the child origin between layout and draw.
    let to_local = Point::ORIGIN - child_layout.position();
    for (paragraph, region) in collect.regions.iter_mut().enumerate() {
        region.bounds += to_local;
        if let Some(clip) = &mut region.clip {
            *clip += to_local;
        }
        for rect in &mut region.shaped_selection_bounds {
            *rect += to_local;
        }
        for caret in &mut region.keyboard_carets {
            caret.position.paragraph = paragraph;
            *caret = caret.translated(to_local);
        }
    }
    state.regions = collect.regions;
    state.work = collect.work;
}

struct SelectionScopeWidget<'a, Message> {
    child: Element<'a, Message>,
    id: Option<widget::Id>,
    label: Option<String>,
    on_copy: Option<Box<dyn Fn(String) -> Message + 'a>>,
    on_selection_change: Option<Box<dyn Fn(Option<ScopeSelection>) -> Message + 'a>>,
    on_context: Option<Box<dyn Fn(ScopeSelection) -> Message + 'a>>,
    on_context_invocation: Option<Box<dyn Fn(SelectionContextInvocation) -> Message + 'a>>,
    context_id: Option<widget::Id>,
    context_label: Option<String>,
    document_key: Option<u64>,
    clear_generation: u64,
    document: Option<ScopeDocument<'a>>,
    yields: bool,
}

impl<Message> SelectionScopeWidget<'_, Message> {
    /// Whether this scope steps aside for an enclosing one.
    fn yielding(&self) -> bool {
        self.yields && ScopeDepth::enclosed()
    }
    fn handles_context(&self) -> bool {
        self.on_context.is_some() || self.on_context_invocation.is_some()
    }
    fn focus_origin(
        &mut self,
        tree: &mut Tree,
        child_layout: Layout,
        renderer: &iced::Renderer,
    ) -> Option<widget::Id> {
        let mut owner = FocusOwner::default();
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            child_layout,
            &child_layout.bounds(),
            renderer,
            &mut owner,
        );
        let scope_focused = tree.state.downcast_ref::<ScopeState>().focus.is_focused();
        owner.origin(scope_focused, self.id.as_ref())
    }

    fn publish_context(
        &self,
        shell: &mut Shell<'_, Message>,
        selection: ScopeSelection,
        invocation: ContextInvocationKind,
        focus_origin: Option<widget::Id>,
    ) {
        if let Some(on_context_invocation) = &self.on_context_invocation {
            shell.publish(on_context_invocation(SelectionContextInvocation {
                selection,
                invocation,
                focus_origin,
            }));
        } else if let Some(on_context) = &self.on_context {
            shell.publish(on_context(selection));
        }
    }
}

fn context_rect(bounds: Rectangle) -> ContextRect {
    ContextRect::new(bounds.x, bounds.y, bounds.width, bounds.height)
}

fn publishes_context(event: &Event) -> bool {
    match event {
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) => true,
        Event::Keyboard(event) => enzui_core::context_menu::opens_context_menu(event),
        _ => false,
    }
}

#[derive(Debug, Default)]
struct FocusOwner {
    present: bool,
    id: Option<widget::Id>,
}

impl FocusOwner {
    fn origin(&self, scope_focused: bool, scope_id: Option<&widget::Id>) -> Option<widget::Id> {
        // An anonymous focused descendant must not inherit the document's identity.
        if self.present {
            self.id.clone()
        } else if scope_focused {
            scope_id.cloned()
        } else {
            None
        }
    }
}

impl<T> Operation<T> for FocusOwner {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        operate(self);
    }

    fn focusable(
        &mut self,
        id: Option<&widget::Id>,
        _bounds: Rectangle,
        state: &mut dyn widget::operation::Focusable,
    ) {
        if state.is_focused() {
            self.present = true;
            self.id = id.cloned();
        }
    }
}

/// Use the palette accent with its alpha replaced by [`SELECTION_FILL_ALPHA`].
pub fn selection_fill(palette: &Palette) -> Color {
    Color {
        a: SELECTION_FILL_ALPHA,
        ..palette.accent
    }
}
// Fresh gestures require a text hit; only an active drag uses nearest-region fallback.
fn point_in_doc_position(regions: &[TextRegion], point: Point) -> Option<DocPosition> {
    regions.iter().enumerate().find_map(|(para_idx, region)| {
        (region.contains(point) && region.query_line_has_text.unwrap_or(true))
            .then(|| DocPosition::new(para_idx, region.char_offset_at(point)))
    })
}

fn point_to_doc_position(regions: &[TextRegion], point: Point) -> Option<DocPosition> {
    if let Some(position) = point_in_doc_position(regions, point) {
        return Some(position);
    }
    if regions.is_empty() {
        return None;
    }
    let mut best_idx = 0;
    let mut best_dist = f32::MAX;
    for (i, region) in regions.iter().enumerate() {
        let center_y = region.bounds.y + region.bounds.height / 2.0;
        let dist = (point.y - center_y).abs();
        if dist < best_dist {
            best_dist = dist;
            best_idx = i;
        }
    }

    let region = &regions[best_idx];
    let offset = if point.x < region.bounds.x {
        0
    } else if point.x > region.bounds.x + region.bounds.width {
        region.text.chars().count()
    } else {
        region.char_offset_at(point)
    };
    Some(DocPosition::new(best_idx, offset))
}

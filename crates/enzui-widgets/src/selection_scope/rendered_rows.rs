//! A [`SelectionDocument`] read from the rows' own widgets.

use super::{CollectTextRegions, SelectionDocument, collect_text_regions};
use iced::advanced::widget::Tree;
use iced::advanced::{Layout, layout};
use iced::{Element, Size};
use std::cell::RefCell;
use std::collections::HashMap;

/// A [`SelectionDocument`] whose paragraphs are read from the rows' own
/// widgets, for rich virtual rows (Markdown, code, quotes, tables, controls)
/// whose selection traversal an app should not reimplement.
///
/// The app gives the rows' keys in reading order and a builder for one
/// row, the same widget its virtual list mounts. When a selection or copy
/// needs a row that is not mounted, the scope builds just that row off
/// screen at its own width and reads its paragraphs with the traversal it
/// uses for mounted rows: same paragraphs, order and excluded controls, so
/// offsets agree. Only rows between the selection's endpoints are built;
/// each at most once per document value, so rebuild the document in `view`
/// as usual.
///
/// Each mounted row must be wrapped in [`enz_selection_row`](super::enz_selection_row)
/// with the same key. Keys must be unique; the first of a repeated key wins.
pub struct RenderedRows<'a, Message> {
    keys: Vec<u64>,
    index: HashMap<u64, usize>,
    row: Box<dyn Fn(usize) -> Element<'a, Message> + 'a>,
    // Rows already built, by index and width bits.
    built: RefCell<HashMap<(usize, u32), Vec<String>>>,
}

impl<'a, Message: 'a> RenderedRows<'a, Message> {
    /// A document over `keys` in reading order, building row `index` with
    /// `row`.
    pub fn new(
        keys: impl IntoIterator<Item = u64>,
        row: impl Fn(usize) -> Element<'a, Message> + 'a,
    ) -> Self {
        let keys: Vec<u64> = keys.into_iter().collect();
        let mut index = HashMap::with_capacity(keys.len());
        for (position, key) in keys.iter().enumerate() {
            index.entry(*key).or_insert(position);
        }
        Self {
            keys,
            index,
            row: Box::new(row),
            built: RefCell::new(HashMap::new()),
        }
    }
}

impl<Message> RenderedRows<'_, Message> {
    /// How many rows the document has.
    #[must_use]
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Whether the document has no rows.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// How many rows were built off screen so far.
    #[must_use]
    pub fn built_rows(&self) -> usize {
        self.built.borrow().len()
    }

    fn build(&self, index: usize, renderer: &iced::Renderer, width: f32) -> Vec<String> {
        let mut element = (self.row)(index);
        let mut tree = Tree::new(element.as_widget());
        element.as_widget_mut().diff(&mut tree);
        // Unbounded and compressed down the column, as a vertical scrollable
        // lays out its content: a fill-height part takes its natural height.
        let limits = layout::Limits::with_flags(
            Size::ZERO,
            Size::new(width.max(0.0), f32::INFINITY),
            Size::new(false, true),
            Size::new(false, true),
        );
        element.as_widget_mut().layout(&mut tree, renderer, &limits);
        let size = tree.size;
        let mut collect = CollectTextRegions::new(None, false, Vec::new());
        collect_text_regions(
            &mut element,
            &mut tree,
            Layout::new(size),
            renderer,
            &mut collect,
        );
        collect
            .regions
            .iter()
            .map(|region| region.text.to_string())
            .collect()
    }
}

impl<Message> SelectionDocument for RenderedRows<'_, Message> {
    fn row_index(&self, key: u64) -> Option<usize> {
        self.index.get(&key).copied()
    }

    /// Rows are read from their widgets, which needs the renderer: this
    /// answers only rows already built at some width.
    fn row_paragraphs(&self, index: usize) -> Option<Vec<String>> {
        self.built
            .borrow()
            .iter()
            .find(|((built, _), _)| *built == index)
            .map(|(_, paragraphs)| paragraphs.clone())
    }

    fn rendered_row_paragraphs(
        &self,
        index: usize,
        renderer: &iced::Renderer,
        width: f32,
    ) -> Option<Vec<String>> {
        if index >= self.keys.len() {
            return None;
        }
        let slot = (index, width.to_bits());
        if let Some(paragraphs) = self.built.borrow().get(&slot) {
            return Some(paragraphs.clone());
        }
        let paragraphs = self.build(index, renderer, width);
        self.built.borrow_mut().insert(slot, paragraphs.clone());
        Some(paragraphs)
    }
}

//! The custom-`Widget` bounds gate for this crate.

// The scanner half of this shared module is exercised by
// `layout_spill_population_ratchet.rs`; this binary needs only the pinned list.

use iced::advanced::layout;
use iced::advanced::widget::Tree;
use iced::{Element, Length, Rectangle, Size};

/// A concrete, constructible message. Several widgets take a `Message` *value*
/// for their activation callback; those values are only stored, never emitted,
/// in a layout-only test, but they must be constructible for the builder to run.
#[derive(Debug, Clone, PartialEq)]
#[allow(
    dead_code,
    reason = "the widget needs these messages to exist; this test never sends them"
)]
pub(crate) enum Msg {
    Noop,
    Previous,
    More,
    Close,
}

// matches SPILL_EPSILON

struct OversizedWrapper<'a, Message, Theme, Renderer> {
    child: Element<'a, Message, Theme, Renderer>,
}

impl<Message, Theme, Renderer> iced::advanced::Widget<Message, Theme, Renderer>
    for OversizedWrapper<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn size(&self) -> Size<Length> {
        Size::new(Length::Shrink, Length::Shrink)
    }
    fn diff(&mut self, tree: &mut Tree) {
        self.child.as_widget_mut().diff(tree);
    }
    fn layout(&mut self, tree: &mut Tree, _renderer: &Renderer, _limits: &layout::Limits) {
        // Deliberately ignore limits and lay out oversized.
        layout::Node::new(Size::new(700.0, 700.0)).apply(tree);
    }
    fn draw(
        &self,
        _tree: &Tree,
        _renderer: &mut Renderer,
        _theme: &Theme,
        _style: &iced::advanced::renderer::Style,
        _layout: iced::advanced::Layout,
        _cursor: iced::advanced::mouse::Cursor,
        _viewport: &Rectangle,
    ) {
    }
}

impl<'a, Message, Theme, Renderer> From<OversizedWrapper<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Renderer: 'a + iced::advanced::Renderer,
    Message: 'a,
    Theme: 'a,
{
    fn from(x: OversizedWrapper<'a, Message, Theme, Renderer>) -> Self {
        Element::new(x)
    }
}

/// One (box_w, box_h) -> (bounded_w, bounded_h) observation.
#[allow(
    dead_code,
    reason = "the widget needs these messages to exist; this test never sends them"
)]
#[derive(Debug, Clone, Copy)]
struct Observation {
    box_w: f32,
    box_h: f32,
    bounded_w: f32,
    bounded_h: f32,
}

impl Observation {}

/// One catalog row: a name, the population entries it instantiates, and a
/// factory. `covers` is the coverage claim that `sweep_covers_the_population`
/// checks against the pinned inventory.
#[allow(
    dead_code,
    reason = "the widget needs these messages to exist; this test never sends them"
)]
pub(crate) struct CatalogRow {
    pub(crate) name: &'static str,
    pub(crate) covers: &'static [&'static str],
    pub(crate) make: Box<dyn Fn() -> Element<'static, Msg>>,
}

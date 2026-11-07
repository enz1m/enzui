//! Register realised keyed rows and tiles with a shared drag host.
//!
//! Item bounds are surface-local and translated on event updates. The enclosing
//! [`crate::drag_drop_host::EnzDragDropHost`] must share the channel and owns
//! gesture handling, negotiation and drop messages. The caller supplies only
//! realised items and keeps their identities stable across reflow.
//!
//! Nonempty widget-id registrations take precedence over size-derived items,
//! which take precedence over the initial list. Widget bounds refresh on event
//! updates; custom highlights draw from the last located widget geometry.
//! Size-derived geometry is evaluated during registration and custom-highlight
//! drawing. Use [`crate::drag_autoscroll_host::enz_drag_autoscroll_host`] for
//! scrolling during a drag.

use crate::drag_drop_host::{
    DragHostChannel, ItemSurfaceRegistration, ItemTargetRegistration, feedback_bounds_within,
};
use enzui_core::drag::{CollectionId, DragOperation, ItemId, PayloadKinds, SourceCapabilities};
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Renderer as _, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

/// One realised keyed item, in viewport-local coordinates.
#[derive(Debug, Clone)]
pub struct DragSurfaceItem {
    /// Stable identity shared by list/grid and the application model.
    pub id: ItemId,
    /// Local hit rectangle.
    pub bounds: Rectangle,
    /// Local rectangle drag feedback paints into; `None` paints inside
    /// [`Self::bounds`].
    pub feedback_bounds: Option<Rectangle>,
    /// Whether this item may begin a drag on pointer-down.
    pub drag_source: bool,
    /// Whether this item is a writable onto-item destination.
    pub accepts_drop: bool,
    /// Human label for feedback/diagnostics.
    pub label: String,
}

impl DragSurfaceItem {
    /// Construct a source item. Call [`Self::drop_target`] for directories that
    /// are valid destinations.
    #[must_use]
    pub fn new(id: impl Into<ItemId>, bounds: Rectangle, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            bounds,
            feedback_bounds: None,
            drag_source: true,
            accepts_drop: false,
            label: label.into(),
        }
    }

    /// Paint drag feedback inside `bounds` (surface-local) rather than inside
    /// this item's hit rectangle.
    ///
    /// Use it when the row is grabbed by a wider or taller rectangle than it
    /// draws: a pinned grab width across a rail, or a row surface inset inside
    /// its pitch. By default feedback covers the hit rectangle, confined to the
    /// surface's own allocation.
    #[must_use]
    pub fn feedback_bounds(mut self, bounds: Rectangle) -> Self {
        self.feedback_bounds = Some(bounds);
        self
    }

    /// Mark whether this realised item accepts an onto-item drop.
    #[must_use]
    pub fn drop_target(mut self, accepts: bool) -> Self {
        self.accepts_drop = accepts;
        self
    }

    /// Mark whether pointer-down on this item may begin a drag.
    ///
    /// Directional edge zones and other destination-only geometry should pass
    /// `false`; the default remains `true` for file/list rows.
    #[must_use]
    pub fn drag_source(mut self, enabled: bool) -> Self {
        self.drag_source = enabled;
        self
    }
}

struct WidgetBounds {
    wanted: Vec<iced::advanced::widget::Id>,
    found: Vec<(iced::advanced::widget::Id, Rectangle)>,
}

impl WidgetBounds {
    fn note(&mut self, id: Option<&iced::advanced::widget::Id>, bounds: Rectangle) {
        if let Some(id) = id
            && self.wanted.contains(id)
            && !self.found.iter().any(|(found, _)| found == id)
        {
            self.found.push((id.clone(), bounds));
        }
    }
}

impl Operation for WidgetBounds {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<()>)) {
        operate(self);
    }

    fn container(
        &mut self,
        id: Option<&iced::advanced::widget::Id>,
        bounds: Rectangle,
        _viewport: &Rectangle,
    ) {
        self.note(id, bounds);
    }

    fn focusable(
        &mut self,
        id: Option<&iced::advanced::widget::Id>,
        bounds: Rectangle,
        _state: &mut dyn iced::advanced::widget::operation::Focusable,
    ) {
        self.note(id, bounds);
    }
}

fn to_window(local: Rectangle, origin: Rectangle) -> Rectangle {
    Rectangle {
        x: origin.x + local.x,
        y: origin.y + local.y,
        width: local.width,
        height: local.height,
    }
}

// Host feedback and custom highlights must share surface confinement.
fn item_feedback_bounds(item: &DragSurfaceItem, origin: Rectangle) -> Rectangle {
    let painted = to_window(item.feedback_bounds.unwrap_or(item.bounds), origin);
    feedback_bounds_within(painted, origin)
}

/// Transparent keyed virtual-surface registration wrapper.
pub struct EnzDragItemSurface<'a, Message> {
    channel: DragHostChannel,
    content: Element<'a, Message>,
    collection: CollectionId,
    items: Vec<DragSurfaceItem>,
    items_from_size: Option<Box<dyn Fn(Size) -> Vec<DragSurfaceItem> + 'a>>,
    widget_items: Vec<(iced::advanced::widget::Id, DragSurfaceItem)>,
    located: std::cell::RefCell<Vec<DragSurfaceItem>>,
    selected: Vec<ItemId>,
    accepts_background: bool,
    drop_target_highlight: Option<iced::Color>,
    drop_target_outline: Option<(iced::Color, f32)>,
    source_capabilities: SourceCapabilities,
    offering: PayloadKinds,
    accepting: PayloadKinds,
    destination_operations: Vec<DragOperation>,
    allows_captured_press: bool,
}

/// Register a surface on the shared channel with default Copy/Move capabilities.
/// Background drops are accepted, all payload kinds are offered and accepted,
/// and child-captured presses cannot start a drag unless enabled.
#[must_use]
pub fn enz_drag_item_surface<'a, Message>(
    channel: &DragHostChannel,
    content: impl Into<Element<'a, Message>>,
    collection: impl Into<CollectionId>,
    items: Vec<DragSurfaceItem>,
) -> EnzDragItemSurface<'a, Message> {
    EnzDragItemSurface {
        channel: channel.clone(),
        content: content.into(),
        collection: collection.into(),
        items,
        items_from_size: None,
        widget_items: Vec::new(),
        located: std::cell::RefCell::new(Vec::new()),
        selected: Vec::new(),
        accepts_background: true,
        drop_target_highlight: None,
        drop_target_outline: None,
        source_capabilities: SourceCapabilities::copy_or_move(),
        offering: PayloadKinds::universal(),
        accepting: PayloadKinds::universal(),
        destination_operations: vec![DragOperation::Move, DragOperation::Copy],
        allows_captured_press: false,
    }
}

impl<'a, Message> EnzDragItemSurface<'a, Message> {
    /// Derive surface-local item rectangles from the current laid-out size.
    /// Replaces the initial list unless nonempty widget-id registrations take precedence.
    /// The callback runs on event registration and custom-highlight draws; keep it
    /// consistent across those calls.
    #[must_use]
    pub fn items_from_size(mut self, f: impl Fn(Size) -> Vec<DragSurfaceItem> + 'a) -> Self {
        self.items_from_size = Some(Box::new(f));
        self
    }

    /// Paint the negotiated accepting item with the supplied wash.
    /// Replaces the host's generic item border; feedback remains confined to the
    /// surface allocation and viewport.
    #[must_use]
    pub fn drop_target_highlight(mut self, color: iced::Color) -> Self {
        self.drop_target_highlight = Some(color);
        self
    }

    /// Round the [`Self::drop_target_highlight`] wash to `radius` and outline
    /// it in `color`, so a large target (a half pane a dropped item will
    /// fill) reads as a tile over the content rather than a stain on it.
    #[must_use]
    pub fn drop_target_outline(mut self, color: iced::Color, radius: f32) -> Self {
        self.drop_target_outline = Some((color, radius.max(0.0)));
        self
    }

    /// Locate item bounds on child widgets by id during event updates.
    /// A nonempty list overrides initial and size-derived items. Unmounted ids are
    /// skipped; duplicate matches use the first container or focusable bounds found.
    /// Custom highlights use these located bounds until the next event update.
    #[must_use]
    pub fn items_at_widgets(
        mut self,
        items: Vec<(iced::advanced::widget::Id, DragSurfaceItem)>,
    ) -> Self {
        self.widget_items = items;
        self
    }

    fn resolved_items(&self, size: Size) -> Vec<DragSurfaceItem> {
        if !self.widget_items.is_empty() {
            return self.located.borrow().clone();
        }
        self.items_from_size
            .as_ref()
            .map_or_else(|| self.items.clone(), |items| items(size))
    }

    fn locate_widget_items(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
    ) {
        if self.widget_items.is_empty() {
            return;
        }
        let mut finder = WidgetBounds {
            wanted: self.widget_items.iter().map(|(id, _)| id.clone()).collect(),
            found: Vec::new(),
        };
        self.content
            .as_widget_mut()
            .operate(tree, layout, viewport, renderer, &mut finder);
        let origin = layout.bounds();
        let located = self
            .widget_items
            .iter()
            .filter_map(|(id, item)| {
                let (_, bounds) = finder.found.iter().find(|(found, _)| found == id)?;
                let mut item = item.clone();
                item.bounds = Rectangle {
                    x: bounds.x - origin.x,
                    y: bounds.y - origin.y,
                    ..*bounds
                };
                Some(item)
            })
            .collect();
        *self.located.borrow_mut() = located;
    }

    /// Selected ids; pressing one drags the whole selection in this order.
    #[must_use]
    pub fn selected(mut self, ids: Vec<ItemId>) -> Self {
        self.selected = ids;
        self
    }
    /// Set whether empty space accepts a drop onto this collection; defaults to true.
    #[must_use]
    pub fn accepts_background(mut self, accepts: bool) -> Self {
        self.accepts_background = accepts;
        self
    }
    /// Set source operations; defaults to Copy and Move.
    #[must_use]
    pub fn source_capabilities(mut self, value: SourceCapabilities) -> Self {
        self.source_capabilities = value;
        self
    }
    /// Set offered payload kinds; defaults to universal.
    #[must_use]
    pub fn offering(mut self, value: PayloadKinds) -> Self {
        self.offering = value;
        self
    }
    /// Set accepted payload kinds; defaults to universal.
    #[must_use]
    pub fn accepting(mut self, value: PayloadKinds) -> Self {
        self.accepting = value;
        self
    }
    /// Set destination operation preference; defaults to Move followed by Copy.
    /// Ctrl/Command promotes Copy when it is present.
    #[must_use]
    pub fn destination_operations(mut self, value: Vec<DragOperation>) -> Self {
        self.destination_operations = value;
        self
    }

    /// Allow a draggable item to take over after its child captured
    /// pointer-down. Use only when that capture establishes row focus rather
    /// than owning a gesture; selectable text must leave the default `false`.
    #[must_use]
    pub fn allows_captured_press(mut self, allows: bool) -> Self {
        self.allows_captured_press = allows;
        self
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for EnzDragItemSurface<'_, Message> {
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        self.content.as_widget().tag()
    }
    fn state(&self) -> iced::advanced::widget::tree::State {
        self.content.as_widget().state()
    }
    fn diff(&mut self, tree: &mut Tree) {
        self.content.as_widget_mut().diff(tree);
    }
    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }
    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.content.as_widget_mut().layout(tree, renderer, limits);
    }
    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
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
        self.content
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);
        if !self.channel.is_hosting() {
            return;
        }
        self.locate_widget_items(tree, layout, renderer, viewport);
        let origin = layout.bounds();
        let resolved = self.resolved_items(origin.size());
        let items = resolved
            .iter()
            .map(|item| ItemTargetRegistration {
                id: item.id.clone(),
                bounds: to_window(item.bounds, origin),
                feedback_bounds: item_feedback_bounds(item, origin),
                drag_source: item.drag_source,
                accepts_drop: item.accepts_drop,
                label: item.label.clone(),
            })
            .collect();
        self.channel.register_item_surface(ItemSurfaceRegistration {
            collection: self.collection.clone(),
            bounds: origin,
            items,
            selected: self.selected.clone(),
            accepts_background: self.accepts_background,
            source_capabilities: self.source_capabilities,
            offering: self.offering.clone(),
            accepting: self.accepting.clone(),
            destination_operations: self.destination_operations.clone(),
            custom_target_highlight: self.drop_target_highlight.is_some(),
            allows_captured_press: self.allows_captured_press,
        });
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
        self.content
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);

        let Some(color) = self.drop_target_highlight else {
            return;
        };
        if !self.channel.is_hosting() || !self.channel.host_dragged() {
            return;
        }
        let origin = layout.bounds();
        iced::advanced::Renderer::with_layer(renderer, origin, |renderer| {
            for item in self
                .resolved_items(origin.size())
                .into_iter()
                .filter(|item| {
                    item.accepts_drop
                        && self
                            .channel
                            .is_active_item_target(&self.collection, &item.id)
                })
            {
                if let Some(bounds) = item_feedback_bounds(&item, origin)
                    .intersection(viewport)
                    .filter(|bounds| bounds.width > 0.0 && bounds.height > 0.0)
                {
                    let border = self.drop_target_outline.map_or_else(
                        iced::Border::default,
                        |(outline, radius)| iced::Border {
                            color: outline,
                            width: 1.5,
                            radius: radius.into(),
                        },
                    );
                    renderer.fill_quad(
                        renderer::Quad {
                            bounds,
                            border,
                            ..renderer::Quad::default()
                        },
                        iced::Background::Color(color),
                    );
                }
            }
        });
    }
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.content
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
        self.content
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
}

impl<'a, Message: 'a> EnzDragItemSurface<'a, Message> {
    /// Build the registration wrapper as an [`Element`].
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        Element::from(self)
    }
}

impl<'a, Message: 'a> From<EnzDragItemSurface<'a, Message>> for Element<'a, Message> {
    fn from(value: EnzDragItemSurface<'a, Message>) -> Self {
        Element::new(value)
    }
}

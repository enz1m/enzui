//! Unclaimed switch-row presses replay through the toggle’s own event path.

use iced::advanced::layout::Measure as _;
use iced::advanced::widget::{Id, Operation, operation::Focusable};
use iced::advanced::{Widget, layout, mouse, overlay, renderer, widget as widget_advanced};
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector, touch};

pub(super) fn switch_press<'a, Message: 'a>(
    row: impl Into<Element<'a, Message>>,
    radius: f32,
    paint_hover: bool,
) -> Element<'a, Message> {
    Element::new(SwitchPress {
        row: row.into(),
        radius,
        paint_hover,
    })
}

struct SwitchPress<'a, Message> {
    row: Element<'a, Message>,
    radius: f32,
    paint_hover: bool,
}

#[derive(Default)]
struct Hover(bool);

#[derive(Default)]
struct FirstFocusable(Option<Rectangle>);

impl Operation for FirstFocusable {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<()>)) {
        if self.0.is_none() {
            operate(self);
        }
    }

    fn focusable(&mut self, _id: Option<&Id>, bounds: Rectangle, _state: &mut dyn Focusable) {
        self.0.get_or_insert(bounds);
    }
}

fn row_layout(layout: layout::Layout, tree: &widget_advanced::Tree) -> layout::Layout {
    layout
        .children(&tree.children)
        .next()
        .expect("a switch row has its row's layout")
}

impl<Message> SwitchPress<'_, Message> {
    fn switch_bounds(
        &mut self,
        tree: &mut widget_advanced::Tree,
        layout: layout::Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> Option<Rectangle> {
        let mut finder = FirstFocusable::default();
        let child = row_layout(layout, tree);
        self.row.as_widget_mut().operate(
            &mut tree.children[0],
            child,
            viewport,
            renderer,
            &mut finder,
        );
        finder.0
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for SwitchPress<'_, Message> {
    fn tag(&self) -> widget_advanced::tree::Tag {
        widget_advanced::tree::Tag::of::<Hover>()
    }

    fn state(&self) -> widget_advanced::tree::State {
        widget_advanced::tree::State::new(Hover::default())
    }

    fn diff(&mut self, tree: &mut widget_advanced::Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.row));
    }

    fn size(&self) -> Size<Length> {
        self.row.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut widget_advanced::Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) {
        let child = self
            .row
            .as_widget_mut()
            .measure(&mut tree.children[0], renderer, limits);
        layout::Node::with_children(child.size(), vec![child]).apply(tree);
    }

    fn draw(
        &self,
        tree: &widget_advanced::Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: layout::Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        if self.paint_hover && tree.state.downcast_ref::<Hover>().0 {
            use iced::advanced::Renderer as _;
            renderer.fill_quad(
                renderer::Quad {
                    bounds: layout.bounds(),
                    border: iced::Border {
                        radius: self.radius.into(),
                        ..iced::Border::default()
                    },
                    ..renderer::Quad::default()
                },
                crate::theme_lookup::palette(theme).overlay_hover(),
            );
        }
        self.row.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            row_layout(layout, tree),
            cursor,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &widget_advanced::Tree,
        layout: layout::Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let inner = self.row.as_widget().mouse_interaction(
            &tree.children[0],
            row_layout(layout, tree),
            cursor,
            viewport,
            renderer,
        );
        if inner == mouse::Interaction::None && cursor.is_over(layout.bounds()) {
            mouse::Interaction::Pointer
        } else {
            inner
        }
    }

    fn operate(
        &mut self,
        tree: &mut widget_advanced::Tree,
        layout: layout::Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let child = row_layout(layout, tree);
        self.row.as_widget_mut().operate(
            &mut tree.children[0],
            child,
            viewport,
            renderer,
            operation,
        );
    }

    fn overlay<'a>(
        &'a mut self,
        tree: &'a mut widget_advanced::Tree,
        layout: layout::Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'a, Message, iced::Theme, iced::Renderer>> {
        let child = row_layout(layout, tree);
        self.row.as_widget_mut().overlay(
            &mut tree.children[0],
            child,
            renderer,
            viewport,
            translation,
            window,
        )
    }

    fn frame_interest(&self, tree: &widget_advanced::Tree) -> bool {
        tree.children
            .first()
            .is_none_or(|row| self.row.as_widget().frame_interest(row))
    }

    fn update_frame(
        &mut self,
        tree: &mut widget_advanced::Tree,
        event: &Event,
        layout: layout::Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let child = row_layout(layout, tree);
        self.row.as_widget_mut().update_frame(
            &mut tree.children[0],
            event,
            child,
            cursor,
            renderer,
            shell,
            viewport,
        );
    }

    fn update(
        &mut self,
        tree: &mut widget_advanced::Tree,
        event: &Event,
        layout: layout::Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let child = row_layout(layout, tree);
        // Give the toggle first refusal so a direct track press activates only once.
        self.row.as_widget_mut().update(
            &mut tree.children[0],
            event,
            child,
            cursor,
            renderer,
            shell,
            viewport,
        );
        let bounds = layout.bounds();
        if let Event::Mouse(mouse::Event::CursorMoved { .. } | mouse::Event::CursorLeft) = event {
            let over = cursor.is_over(bounds);
            let hover = tree.state.downcast_mut::<Hover>();
            if hover.0 != over {
                hover.0 = over;
                if self.paint_hover {
                    shell.request_redraw();
                }
            }
        }
        if shell.is_event_captured() {
            return;
        }
        let pressed_at = match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                cursor.position_over(bounds)
            }
            Event::Touch(touch::Event::FingerPressed { position, .. }) => {
                bounds.contains(*position).then_some(*position)
            }
            _ => None,
        };
        if pressed_at.is_none() {
            return;
        }
        let Some(switch) = self.switch_bounds(tree, layout, viewport, renderer) else {
            return;
        };
        let centre = Point::new(switch.center_x(), switch.center_y());
        let replayed = match event {
            Event::Touch(touch::Event::FingerPressed { id, .. }) => {
                Event::Touch(touch::Event::FingerPressed {
                    id: *id,
                    position: centre,
                })
            }
            other => other.clone(),
        };
        let child = row_layout(layout, tree);
        self.row.as_widget_mut().update(
            &mut tree.children[0],
            &replayed,
            child,
            mouse::Cursor::Available(centre),
            renderer,
            shell,
            viewport,
        );
    }
}

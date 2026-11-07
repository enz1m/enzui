//! Theme-aware group hover chrome, with optional hover-dependent content.
//!
//! Chrome resolves from the draw cursor over the whole group, behind its child.
//! [`enz_hover_group_with`] rebuilds content at layout from retained hover state;
//! event and frame updates reconcile that state and request layout on a change.
//! Keep hover variants the same size to avoid moving content under the pointer.
//! Colour-only reveals preserve descendant hit testing and accessibility.

use enzui_core::metrics::FiniteNonNeg;
use iced::advanced::layout::Measure as _;
use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Shell, Widget, mouse, overlay, renderer};
use iced::border::Radius;
use iced::{Background, Border, Color, Element, Event, Length, Padding, Rectangle, Size, Vector};
use std::rc::Rc;
/// Group chrome resolved from the runtime theme and pointer hover state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HoverGroupStyle {
    /// Fill painted behind the child.
    pub background: Color,
    /// Colour of a submitted quad's border.
    pub border_color: Color,
    /// Border width in logical pixels; zero omits the border.
    /// Submitted quads replace negative or non-finite values with 1 px.
    pub border_width: f32,
    /// Uniform radius in logical pixels.
    /// Submitted quads replace negative or non-finite values with 6 px.
    pub radius: f32,
}

impl HoverGroupStyle {
    /// Transparent fill with zero border width and square corners.
    pub const NONE: HoverGroupStyle = HoverGroupStyle {
        background: Color::TRANSPARENT,
        border_color: Color::TRANSPARENT,
        border_width: 0.0,
        radius: 0.0,
    };
    const BORDER_WIDTH_FALLBACK: f32 = 1.0;
    const RADIUS_FALLBACK: f32 = 6.0;
    fn quad_border(self) -> Border {
        Border {
            radius: Radius::from(FiniteNonNeg::new_or(self.radius, Self::RADIUS_FALLBACK).get()),
            width: FiniteNonNeg::new_or(self.border_width, Self::BORDER_WIDTH_FALLBACK).get(),
            color: self.border_color,
        }
    }
}
type StyleFn<'a> = Rc<dyn Fn(&iced::Theme, bool) -> HoverGroupStyle + 'a>;
type ContentFn<'a, Message> = Box<dyn Fn(bool) -> Element<'a, Message> + 'a>;
#[derive(Debug, Default, Clone, Copy)]
struct State {
    hovered: bool,
}
/// A padded container with draw-time hover chrome and optional layout-time content rebuilding.
#[must_use]
pub struct EnzHoverGroup<'a, Message> {
    content: Element<'a, Message>,
    builder: Option<ContentFn<'a, Message>>,
    padding: Padding,
    width: Length,
    height: Length,
    style: StyleFn<'a>,
}
fn default_style(theme: &iced::Theme, hovered: bool) -> HoverGroupStyle {
    if !hovered {
        return HoverGroupStyle::NONE;
    }
    HoverGroupStyle {
        background: crate::theme_lookup::resolve(theme, enzui_theme::Palette::overlay_hover),
        border_color: Color::TRANSPARENT,
        border_width: 0.0,
        radius: 6.0,
    }
}

impl<'a, Message: 'a> EnzHoverGroup<'a, Message> {
    /// Set inner padding; the default is zero.
    pub fn padding(mut self, padding: impl Into<Padding>) -> Self {
        self.padding = padding.into();
        self
    }
    /// Set outer width; the default is [`Length::Shrink`].
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }
    /// Set outer height; the default is [`Length::Shrink`].
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }
    /// Fill the parent's available width.
    pub fn fill_width(mut self) -> Self {
        self.width = Length::Fill;
        self
    }
    /// Build the group element, retaining hover state across compatible rebuilds.
    pub fn into_element(self) -> Element<'a, Message> {
        Element::new(self)
    }
    /// Resolve group chrome on each draw from the runtime theme and pointer position.
    /// A quad is submitted only for positive background alpha or positive raw border width.
    pub fn style(mut self, style: impl Fn(&iced::Theme, bool) -> HoverGroupStyle + 'a) -> Self {
        self.style = Rc::new(style);
        self
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for EnzHoverGroup<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        // Builder content must be diffed after layout rebuilds it with retained hover.
        if self.builder.is_none() {
            tree.diff_children(std::slice::from_mut(&mut self.content));
        }
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: self.width,
            height: self.height,
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        if let Some(builder) = &self.builder {
            let hovered = tree.state.downcast_ref::<State>().hovered;
            self.content = builder(hovered);
            tree.diff_children(std::slice::from_mut(&mut self.content));
        }
        layout::padded_node(limits, self.width, self.height, self.padding, |limits| {
            self.content
                .as_widget_mut()
                .measure(&mut tree.children[0], renderer, limits)
        })
        .apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let child = layout
            .children(&tree.children)
            .next()
            .expect("the layout has its one child");
        self.content.as_widget_mut().operate(
            &mut tree.children[0],
            child,
            viewport,
            renderer,
            operation,
        );
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
        let child = layout
            .children(&tree.children)
            .next()
            .expect("the layout has its one child");
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
        // Content observes retained hover at layout; chrome reads the draw cursor directly.
        if self.builder.is_some() {
            let hovered = cursor.is_over(layout.bounds());
            let state = tree.state.downcast_mut::<State>();
            if state.hovered != hovered {
                state.hovered = hovered;
                shell.invalidate_layout();
                shell.request_redraw();
            }
        }
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
        crate::frame_routing::forward_frame_opaque(
            &mut self.content,
            tree,
            0,
            event,
            layout
                .children(&tree.children)
                .next()
                .expect("the layout has its one child"),
            cursor,
            renderer,
            shell,
            viewport,
        );
        if self.builder.is_some() {
            let hovered = cursor.is_over(layout.bounds());
            let state = tree.state.downcast_mut::<State>();
            if state.hovered != hovered {
                state.hovered = hovered;
                shell.invalidate_layout();
                shell.request_redraw();
            }
        }
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
        use iced::advanced::Renderer as _;
        let bounds = layout.bounds();
        let hovered = cursor.is_over(bounds);
        let resolved = (self.style)(theme, hovered);
        if resolved.background.a > 0.0 || resolved.border_width > 0.0 {
            renderer.fill_quad(
                renderer::Quad {
                    bounds,
                    border: resolved.quad_border(),
                    shadow: iced::Shadow::default(),
                    snap: true,
                },
                Background::Color(resolved.background),
            );
        }

        let child = layout
            .children(&tree.children)
            .next()
            .expect("the layout has its one child");
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

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let child = layout
            .children(&tree.children)
            .next()
            .expect("the layout has its one child");
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            child,
            cursor,
            viewport,
            renderer,
        )
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
        let child = layout
            .children(&tree.children)
            .next()
            .expect("the layout has its one child");
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

impl<'a, Message: 'a> From<EnzHoverGroup<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzHoverGroup<'a, Message>) -> Self {
        w.into_element()
    }
}
/// Wrap content with a transparent resting fill and a themed hover overlay.
/// The default hover radius is 6 px, padding is zero, and sizing shrinks to content.
pub fn enz_hover_group<'a, Message: 'a>(
    content: impl Into<Element<'a, Message>>,
) -> EnzHoverGroup<'a, Message> {
    EnzHoverGroup {
        content: content.into(),
        builder: None,
        padding: Padding::from(0),
        width: Length::Shrink,
        height: Length::Shrink,
        style: Rc::new(default_style),
    }
}
/// Build content from the group's retained hover state.
/// Calls `build(false)` during construction and calls it again on every layout.
/// Hover changes request layout and redraw; direct drawing updates only chrome.
/// Reserve the same geometry in both states to avoid shifting content.
pub fn enz_hover_group_with<'a, Message: 'a, E>(
    build: impl Fn(bool) -> E + 'a,
) -> EnzHoverGroup<'a, Message>
where
    E: Into<Element<'a, Message>>,
{
    let builder: ContentFn<'a, Message> = Box::new(move |hovered| build(hovered).into());
    let content = builder(false);
    EnzHoverGroup {
        content,
        builder: Some(builder),
        padding: Padding::from(0),
        width: Length::Shrink,
        height: Length::Shrink,
        style: Rc::new(default_style),
    }
}

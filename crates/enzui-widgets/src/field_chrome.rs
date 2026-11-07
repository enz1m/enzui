//! Live-theme field surface with whole-frame hover and descendant focus paint.
//!
//! [`FieldChrome`] fills available width and defaults to a fixed logical-pixel
//! height with caller-supplied padding. [`FieldChrome::height_length`] changes
//! that policy. The paint callback controls surface colour and border geometry.
//!
//! Focus outranks hover, and text-entry focus paints in both pointer and
//! keyboard modalities. Ordinary events still update this window's modality
//! before child dispatch so sibling controls see the current input source.
//!
//! Focus readers stop at the first matching state in depth-first order.
//! [`focused_field_in`] prefers any matching input state, even an unfocused
//! one, before looking for a PlainText editor. Multiple inputs or editors in
//! custom slots therefore need care: these readers do not aggregate focus.
//! Editors with a different highlighter state type are not recognized.
//!
//! The surface and border fade between states over the theme's
//! `motion-fast` duration (focusing a field eases its ring in);
//! [`FieldChrome::reduced_motion`] or a reduced-motion theme makes them land
//! at once.
//!
//! [`FieldChrome::interactive`] changes paint status only: false forces
//! [`ChromeStatus::Inert`] while child input, operations and overlays continue.
//! Callers own actual disabled/read-only behavior and accessibility semantics.

use iced::advanced::layout::Measure as _;
use iced::advanced::text::parser::PlainText;
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::widget::{text_editor, text_input};
use iced::{Background, Border, Color, Element, Event, Length, Padding, Rectangle, Size, Vector};

/// Interaction state passed to the field surface's paint callback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromeStatus {
    /// Idle and interactive.
    Active,
    /// Cursor over the field (anywhere in it, including the slots).
    Hovered,
    /// The focus reader found a focused input or PlainText editor state.
    Focused,
    /// The wrapper's `interactive` flag forces inert paint.
    Inert,
}

/// Surface and border values supplied by the caller for one draw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChromePaint {
    /// Surface fill.
    pub background: Color,
    /// Border colour.
    pub border_color: Color,
    /// Border width in logical pixels; the callback chooses the focused width.
    pub border_width: f32,
    /// Corner radius in logical px.
    pub radius: f32,
}
impl crate::motion::Lerp for ChromePaint {
    fn lerp(&self, to: &Self, t: f32) -> Self {
        Self {
            background: self.background.lerp(&to.background, t),
            border_color: self.border_color.lerp(&to.border_color, t),
            border_width: self.border_width.lerp(&to.border_width, t),
            radius: self.radius.lerp(&to.radius, t),
        }
    }
}

/// The chrome's paint fading between states, and the status of the last
/// presented frame.
#[derive(Debug, Default)]
struct State {
    fade: crate::motion::StateFade<ChromePaint>,
    painted: Option<ChromeStatus>,
    /// An event changed the status since the last frame.
    frame_owed: bool,
}

type PaintFn<'a> = Box<dyn Fn(&iced::Theme, ChromeStatus) -> ChromePaint + 'a>;

/// Read the first matching text-input state in depth-first order.
/// Returns `Some(false)` for that input even if a later input is focused,
/// or `None` when no state matches this renderer's paragraph type.
pub fn focused_input_in<Renderer>(tree: &Tree) -> Option<bool>
where
    Renderer: iced::advanced::text::Renderer,
{
    if tree.tag == tree::Tag::of::<text_input::State<Renderer::Paragraph>>() {
        return Some(
            tree.state
                .downcast_ref::<text_input::State<Renderer::Paragraph>>()
                .is_focused(),
        );
    }
    tree.children
        .iter()
        .find_map(|child| focused_input_in::<Renderer>(child))
}

/// Read the first PlainText editor state in depth-first order.
/// Returns its focus flag, or `None` when no matching state exists.
/// Editors with another highlighter state type are not recognized.
pub fn focused_editor_in(tree: &Tree) -> Option<bool> {
    if tree.tag == tree::Tag::of::<text_editor::State<PlainText>>() {
        return Some(
            tree.state
                .downcast_ref::<text_editor::State<PlainText>>()
                .is_focused(),
        );
    }
    tree.children.iter().find_map(focused_editor_in)
}

/// Read input focus first, falling back to a PlainText editor only when no input matches.
/// An unfocused input returns `Some(false)` even when an editor is focused.
/// Each reader uses its first matching state; `None` means neither type matched.
pub fn focused_field_in<Renderer>(tree: &Tree) -> Option<bool>
where
    Renderer: iced::advanced::text::Renderer,
{
    match focused_input_in::<Renderer>(tree) {
        Some(focused) => Some(focused),
        None => focused_editor_in(tree),
    }
}

/// Width-filling field surface with caller-controlled height, padding and live paint.
#[must_use]
pub struct FieldChrome<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    child: Element<'a, Message, Theme, Renderer>,
    height: Length,
    padding: Padding,
    paint: PaintFn<'a>,
    interactive: bool,
    reduced_motion: Option<bool>,
}

impl<'a, Message, Renderer> FieldChrome<'a, Message, iced::Theme, Renderer> {
    /// Wrap a child with a fixed logical-pixel height and the supplied padding and paint.
    /// Defaults to interactive paint; child behavior and semantics remain delegated.
    pub fn new(
        child: impl Into<Element<'a, Message, iced::Theme, Renderer>>,
        height: f32,
        padding: Padding,
        paint: impl Fn(&iced::Theme, ChromeStatus) -> ChromePaint + 'a,
    ) -> Self {
        Self {
            child: child.into(),
            height: Length::Fixed(height),
            padding,
            paint: Box::new(paint),
            interactive: true,
            reduced_motion: None,
        }
    }

    /// Change the surface's height policy; defaults to the fixed height from [`Self::new`].
    /// Parent limits and measured child size participate in resolving this policy.
    pub fn height_length(mut self, height: Length) -> Self {
        self.height = height;
        self
    }

    /// Choose interactive paint; defaults to true.
    /// False forces [`ChromeStatus::Inert`] without blocking child events, focus or operations.
    pub fn interactive(mut self, interactive: bool) -> Self {
        self.interactive = interactive;
        self
    }

    /// Override the theme's reduced-motion policy: `true` makes focus and
    /// hover paint land at once, `false` fades it even under a
    /// reduced-motion theme.
    pub fn reduced_motion(mut self, reduced: bool) -> Self {
        self.reduced_motion = Some(reduced);
        self
    }
}

impl<Message, Renderer> FieldChrome<'_, Message, iced::Theme, Renderer>
where
    Renderer: iced::advanced::text::Renderer,
{
    fn status(&self, tree: &Tree, bounds: Rectangle, cursor: mouse::Cursor) -> ChromeStatus {
        if !self.interactive {
            ChromeStatus::Inert
        } else if focused_field_in::<Renderer>(tree).unwrap_or(false) {
            ChromeStatus::Focused
        } else if cursor.is_over(bounds) {
            ChromeStatus::Hovered
        } else {
            ChromeStatus::Active
        }
    }

    /// Follows the status: an event that changes it asks for a frame, and
    /// each frame moves the paint toward what the status asks for.
    fn track<Message2>(
        &self,
        tree: &mut Tree,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
        shell: &mut Shell<'_, Message2>,
    ) {
        let status = self.status(tree, bounds, cursor);
        let state = tree.state.downcast_mut::<State>();
        if let Event::Window(iced::window::Event::RedrawRequested(now)) = event {
            state.painted = Some(status);
            state.frame_owed = false;
            let paint = &self.paint;
            if state
                .fade
                .frame(*now, self.reduced_motion, false, |theme: &iced::Theme| {
                    paint(theme, status)
                })
            {
                shell.request_redraw();
            }
        } else if state.painted.is_some_and(|painted| painted != status) {
            state.frame_owed = true;
            shell.request_redraw();
        }
    }
}

impl<Message, Renderer> Widget<Message, iced::Theme, Renderer>
    for FieldChrome<'_, Message, iced::Theme, Renderer>
where
    Renderer: iced::advanced::text::Renderer,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: Length::Fill,
            height: self.height,
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        layout::field(limits, self.height, self.padding, |limits| {
            self.child
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
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        operation.container(None, layout.bounds(), viewport);
        operation.traverse(&mut |operation| {
            let child_layout = layout
                .children(&tree.children)
                .next()
                .expect("chrome has one child");
            self.child.as_widget_mut().operate(
                &mut tree.children[0],
                child_layout,
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
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        // Record before child capture so sibling controls in this window see the input source.
        enzui_core::focus::note_input_event_for(
            event,
            enzui_core::focus::window_key(shell.window()),
        );

        let child_layout = layout
            .children(&tree.children)
            .next()
            .expect("chrome has one child");
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            child_layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        crate::frame_routing::refresh_opaque_child(&self.child, tree, 0);
        self.track(tree, event, layout.bounds(), cursor, shell);
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        let state = tree.state.downcast_ref::<State>();
        state.frame_owed
            || state.painted.is_none()
            || state.fade.is_moving()
            || crate::frame_routing::interest_opaque(tree)
    }

    fn update_frame(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        crate::frame_routing::forward_frame_opaque(
            &mut self.child,
            tree,
            0,
            event,
            layout
                .children(&tree.children)
                .next()
                .expect("chrome has one child"),
            cursor,
            renderer,
            shell,
            viewport,
        );
        self.track(tree, event, layout.bounds(), cursor, shell);
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let status = self.status(tree, bounds, cursor);
        let paint = tree
            .state
            .downcast_ref::<State>()
            .fade
            .paint(theme, (self.paint)(theme, status));

        layout.fill_presentation_quad(
            tree,
            renderer,
            renderer::Quad {
                bounds,
                border: Border {
                    color: paint.border_color,
                    width: paint.border_width,
                    radius: paint.radius.into(),
                },
                ..Default::default()
            },
            Background::Color(paint.background),
        );

        self.child.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout
                .children(&tree.children)
                .next()
                .expect("chrome has one child"),
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
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            layout
                .children(&tree.children)
                .next()
                .expect("chrome has one child"),
            cursor,
            viewport,
            renderer,
        )
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, Renderer>> {
        let child_layout = layout
            .children(&tree.children)
            .next()
            .expect("chrome has one child");
        self.child.as_widget_mut().overlay(
            &mut tree.children[0],
            child_layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message, Renderer> From<FieldChrome<'a, Message, iced::Theme, Renderer>>
    for Element<'a, Message, iced::Theme, Renderer>
where
    Message: 'a,
    Renderer: 'a + iced::advanced::text::Renderer,
{
    fn from(w: FieldChrome<'a, Message, iced::Theme, Renderer>) -> Self {
        Element::new(w)
    }
}

//! `virtual_text_surface` — the published AT node for a lazily-served editor.
//!
//! # Motivation
//!
//! [`enzui_core::virtual_text`] can answer arbitrary off-screen AT-SPI
//! text reads, selections and replacements for a multi-gigabyte document. But
//! a provider is only reachable if an assistive technology can *find a node to
//! ask*. iced's stock `text_editor` publishes no semantics at all, so an app
//! that owns one has nothing in the AccessKit tree to bind a provider to.
//!
//! Registering a provider against a node id the app made up is not adoption:
//! `accesskit_unix` would answer on an object no client can reach by
//! traversal, and the "adoption" would be invisible to every real AT. This
//! widget is therefore the missing half — it publishes a real, stable,
//! id-bearing editable-text node over the app's existing editor element,
//! without changing a pixel of it.
//!
//! It deliberately does *not* know about AccessKit, providers or AT-SPI. It
//! publishes a widget [`Id`] and editable-text [`Semantics`]; the runtime seam
//! (`enzui_core::virtual_text::install_provider_tree_builder`, which
//! exists only on Linux with core's `accesskit` feature and so is not part of
//! this crate's default documented surface) resolves that same [`Id`] to the
//! actual `accesskit::NodeId` in the actual `accesskit::TreeUpdate`
//! that the window really published, and binds the provider there. Neither
//! the app nor this widget ever guesses an identity.
//!
//! ## The revert control
//!
//! Remove this wrapper and the editor stops publishing a node. The id is then
//! absent from the frame's `NodeIdMap`, the runtime seam binds nothing, and
//! every provider action becomes unreachable. That is exactly what this
//! module's paired gates assert:
//! `adopted_editor_binds_its_provider_to_the_published_node` requires the
//! provider to bind at the node the frame really published, and
//! `reverting_the_surface_makes_the_provider_unreachable` drops the wrapper
//! and requires both that node and the binding to be absent, so an adoption
//! revert would fail the paired gates.

use enzui_core::collect::SemanticsNode;
use enzui_core::semantics::{Role, Semantics};
use iced::advanced::Shell;
use iced::advanced::input_method;
use iced::advanced::widget::{Id, Operation, Tree, tree};
use iced::advanced::{Layout, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

/// A child element published to assistive technology as one editable text
/// document whose contents are served lazily.
pub struct VirtualTextSurface<'a, Message, Theme, Renderer> {
    child: Element<'a, Message, Theme, Renderer>,
    id: Id,
    label: String,
    read_only: bool,
    focus_id: Option<Id>,
    on_input_method: Option<Box<dyn Fn(input_method::Event) -> Message + 'a>>,
}

/// Publish `child` as the editable-text node identified by `id`.
///
/// `id` must be the same [`Id`] the application hands to
/// `enzui_core::virtual_text::bind_provider_to_widget` (Linux, core's
/// `accesskit` feature); that is the only coupling between the visual surface
/// and the lazy provider, and it is an id the application already owns.
///
/// `label` is the accessible name. An editable text node with no name is
/// announced as an anonymous edit box, so it is required rather than optional.
pub fn enz_virtual_text_surface<'a, Message, Theme, Renderer>(
    child: impl Into<Element<'a, Message, Theme, Renderer>>,
    id: Id,
    label: impl Into<String>,
) -> VirtualTextSurface<'a, Message, Theme, Renderer> {
    VirtualTextSurface {
        child: child.into(),
        id,
        label: label.into(),
        read_only: false,
        focus_id: None,
        on_input_method: None,
    }
}

impl<'a, Message, Theme, Renderer> VirtualTextSurface<'a, Message, Theme, Renderer> {
    /// Publish the truthful read-only state (the OOM paged fallback is
    /// read-only and must not advertise `SetValue`).
    #[must_use]
    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    /// Identify the wrapped child editor whose actual widget focus is
    /// published on the virtual document node.
    #[must_use]
    pub fn focus_id(mut self, id: Id) -> Self {
        self.focus_id = Some(id);
        self
    }

    /// Intercept IME commits while the wrapped child editor is focused. Open,
    /// preedit and close still reach the child for native preedit rendering; a
    /// commit is published only through this callback, preventing the child's
    /// ordinary Paste path from applying it a second time.
    #[must_use]
    pub fn on_input_method(
        mut self,
        handler: impl Fn(input_method::Event) -> Message + 'a,
    ) -> Self {
        self.on_input_method = Some(Box::new(handler));
        self
    }

    /// The semantics this surface publishes. Exposed so a test can assert the
    /// published contract without a renderer.
    pub fn semantics(&self) -> Semantics {
        self.semantics_with_focus(false)
    }

    fn semantics_with_focus(&self, focused: bool) -> Semantics {
        // `editable` is the declared capability; `read_only` is what withdraws
        // the mutation verbs. No `default_action`: this node is reached by
        // focus and served by the virtual-text provider, never by a synthetic
        // click that nothing is armed to handle.
        Semantics::new(Role::TextInput)
            .label(self.label.clone())
            .editable(true)
            .read_only(self.read_only)
            .focused(focused)
    }
}

#[derive(Debug)]
struct SurfaceState {
    child: Tree,
    focused: bool,
}

struct FocusProbe<'a> {
    target: Option<&'a Id>,
    focused: bool,
}

impl Operation for FocusProbe<'_> {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn focusable(
        &mut self,
        id: Option<&Id>,
        _bounds: Rectangle,
        state: &mut dyn iced::advanced::widget::operation::Focusable,
    ) {
        if self.target.is_none_or(|target| id == Some(target)) {
            self.focused |= state.is_focused();
        }
    }
}

impl<Message, Theme, Renderer> iced::advanced::Widget<Message, Theme, Renderer>
    for VirtualTextSurface<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<SurfaceState>()
    }

    fn state(&self) -> tree::State {
        tree::State::interactive(SurfaceState {
            child: Tree::new(self.child.as_widget()),
            focused: false,
        })
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.state
            .downcast_mut::<SurfaceState>()
            .child
            .diff(self.child.as_widget_mut());
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        self.child.as_widget_mut().layout(
            &mut tree.state.downcast_mut::<SurfaceState>().child,
            renderer,
            limits,
        );
        tree.size = tree.state.downcast_mut::<SurfaceState>().child.size;
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        let state = tree.state.downcast_mut::<SurfaceState>();
        let mut probe = FocusProbe {
            target: self.focus_id.as_ref(),
            focused: false,
        };
        self.child.as_widget_mut().operate(
            &mut state.child,
            layout,
            viewport,
            renderer,
            &mut probe,
        );
        state.focused = probe.focused;
        let mut node = SemanticsNode::new(self.semantics_with_focus(state.focused));
        operation.custom(Some(&self.id), layout.bounds(), &mut node);
        // Descend so the child keeps its own focus registration and any
        // operation the app runs over it; the node above is additive.
        self.child
            .as_widget_mut()
            .operate(&mut state.child, layout, viewport, renderer, operation);
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        crate::frame_routing::interest_transparent(
            &self.child,
            &tree.state.downcast_ref::<SurfaceState>().child,
        )
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
        let state = tree.state.downcast_mut::<SurfaceState>();
        // Read the child editor's live focus on every input event. Semantics
        // collection also refreshes this flag, but IME routing must not depend
        // on an AT client having requested a tree operation first.
        let mut probe = FocusProbe {
            target: self.focus_id.as_ref(),
            focused: false,
        };
        self.child.as_widget_mut().operate(
            &mut state.child,
            layout,
            &layout.bounds(),
            renderer,
            &mut probe,
        );
        state.focused = probe.focused;
        if state.focused
            && let Event::InputMethod(input) = event
            && let Some(on_input_method) = &self.on_input_method
        {
            if matches!(input, input_method::Event::Commit(_)) {
                shell.publish(on_input_method(input.clone()));
                shell.capture_event();
                return;
            }
            self.child.as_widget_mut().update(
                &mut state.child,
                event,
                layout,
                cursor,
                renderer,
                shell,
                viewport,
            );
            shell.publish(on_input_method(input.clone()));
            return;
        }
        self.child.as_widget_mut().update(
            &mut state.child,
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
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
        crate::frame_routing::forward_frame_transparent(
            &mut self.child,
            &mut tree.state.downcast_mut::<SurfaceState>().child,
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.child.as_widget().draw(
            &tree.state.downcast_ref::<SurfaceState>().child,
            renderer,
            theme,
            style,
            layout,
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
            &tree.state.downcast_ref::<SurfaceState>().child,
            layout,
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
    ) -> Vec<overlay::Element<'b, Message, Theme, Renderer>> {
        self.child.as_widget_mut().overlay(
            &mut tree.state.downcast_mut::<SurfaceState>().child,
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message, Theme, Renderer> VirtualTextSurface<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: 'a + iced::advanced::Renderer,
{
    /// Finish the surface into an [`Element`] — `docs/widgets.md` step 3.
    ///
    /// Routed through the `From` impl below so the two spellings cannot
    /// diverge, and carrying the same renderer/theme bounds so a caller in a
    /// generic view sees no extra constraint.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message, Theme, Renderer> {
        Element::from(self)
    }
}

impl<'a, Message, Theme, Renderer> From<VirtualTextSurface<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: 'a + iced::advanced::Renderer,
{
    fn from(surface: VirtualTextSurface<'a, Message, Theme, Renderer>) -> Self {
        Element::new(surface)
    }
}

impl tree::Interactive for SurfaceState {
    fn cancel_interaction(&mut self) {
        self.child.cancel_interactions();
    }
}

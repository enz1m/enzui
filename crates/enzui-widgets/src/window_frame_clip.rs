//! Clips a window to the rounded frame its shell draws.
//!
//! An [`app_shell`](crate::app_shell) that draws its own frame rounds its
//! window layer, but layers stacked above the shell in the same view (a modal
//! host's scrim, a full-window overlay) cover the whole rectangular surface,
//! filling the transparent corners outside the frame. The shell publishes its
//! frame radius while it is built (zero when maximized, tiled, fullscreen or
//! framed by the compositor), and the root [`host`] that `app_setup` puts
//! around every window masks the whole view to it. Input is not masked, so the
//! corner resize grips keep answering.
//!
//! The same root keeps the window's drop shadow in step with the theme: the
//! platform paints it outside the window (see [`iced::window::Shadow`]), and
//! [`ShadowSync`] sends a window the theme's shadow once its shell is seen to
//! draw its own frame, and again whenever the theme's shadow changes.

use std::cell::{Cell, RefCell};

use iced::Element;

thread_local! {
    /// The frame radius the shell built during the current view, if any.
    static RADIUS: Cell<f32> = const { Cell::new(0.0) };
    /// Whether a shell built during the current view draws its own frame.
    static OWN_FRAME: Cell<bool> = const { Cell::new(false) };
    /// Whether a shell built during the current view wants its frame glass.
    static WANTS_GLASS: Cell<bool> = const { Cell::new(false) };
}

/// Records whether the shell being built wants its frame to be the
/// compositor's glass.
pub(crate) fn publish_glass(wants: bool) {
    WANTS_GLASS.with(|cell| cell.set(wants));
}

/// Whether the view just built has a shell that wants glass.
pub(crate) fn take_wants_glass() -> bool {
    WANTS_GLASS.with(|cell| cell.replace(false))
}

/// A window's glass binding arriving, as a runtime service message.
#[derive(Debug)]
pub(crate) struct GlassBound(iced::window::Id, crate::desktop_glass::WindowGlass);

/// Asks for an update so a window whose view wants glass is bound.
#[derive(Debug)]
pub(crate) struct GlassWanted;

/// Wraps a window's root; while `nudge` is set, the first event the window
/// sees publishes it (once per view build), so a window waiting for its glass
/// binding gets the update that binds it. Transparent to the widget tree.
pub(crate) fn nudge<'a, Message, Theme, Renderer>(
    child: Element<'a, Message, Theme, Renderer>,
    nudge: Option<Box<dyn Fn() -> Message + 'a>>,
) -> Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    Element::new(Nudge {
        child,
        nudge,
        sent: false,
    })
}

struct Nudge<'a, Message, Theme, Renderer> {
    child: Element<'a, Message, Theme, Renderer>,
    nudge: Option<Box<dyn Fn() -> Message + 'a>>,
    sent: bool,
}

impl<Message, Theme, Renderer> Nudge<'_, Message, Theme, Renderer> {
    fn send(&mut self, shell: &mut iced::advanced::Shell<'_, Message>) {
        if !self.sent
            && let Some(nudge) = &self.nudge
        {
            self.sent = true;
            shell.publish(nudge());
        }
    }
}

impl<Message, Theme, Renderer> iced::advanced::Widget<Message, Theme, Renderer>
    for Nudge<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        self.child.as_widget().tag()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        self.child.as_widget().state()
    }

    fn diff(&mut self, tree: &mut iced::advanced::widget::Tree) {
        self.child.as_widget_mut().diff(tree);
    }

    fn size(&self) -> iced::Size<iced::Length> {
        self.child.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        renderer: &Renderer,
        limits: &iced::advanced::layout::Limits,
    ) {
        self.child.as_widget_mut().layout(tree, renderer, limits);
    }

    fn update(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        event: &iced::Event,
        layout: iced::advanced::Layout,
        cursor: iced::mouse::Cursor,
        renderer: &Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &iced::Rectangle,
    ) {
        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);
        self.send(shell);
    }

    fn frame_interest(&self, tree: &iced::advanced::widget::Tree) -> bool {
        self.nudge.is_some() && !self.sent
            || crate::frame_routing::interest_transparent(&self.child, tree)
    }

    fn update_frame(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        event: &iced::Event,
        layout: iced::advanced::Layout,
        cursor: iced::mouse::Cursor,
        renderer: &Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &iced::Rectangle,
    ) {
        crate::frame_routing::forward_frame_transparent(
            &mut self.child,
            tree,
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        self.send(shell);
    }

    fn mouse_interaction(
        &self,
        tree: &iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        cursor: iced::mouse::Cursor,
        viewport: &iced::Rectangle,
        renderer: &Renderer,
    ) -> iced::mouse::Interaction {
        self.child
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn draw(
        &self,
        tree: &iced::advanced::widget::Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &iced::advanced::renderer::Style,
        layout: iced::advanced::Layout,
        cursor: iced::mouse::Cursor,
        viewport: &iced::Rectangle,
    ) {
        self.child
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn operate(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        viewport: &iced::Rectangle,
        renderer: &Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        self.child
            .as_widget_mut()
            .operate(tree, layout, viewport, renderer, operation);
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        renderer: &Renderer,
        viewport: &iced::Rectangle,
        translation: iced::Vector,
        window: iced::Size,
    ) -> Vec<iced::advanced::overlay::Element<'b, Message, Theme, Renderer>> {
        self.child
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
}

/// Each window's compositor glass: bound once its shell first wants a glass
/// frame, and kept for the window's life.
#[derive(Default)]
pub(crate) struct GlassSync {
    windows: RefCell<Vec<GlassWindow>>,
}

struct GlassWindow {
    id: iced::window::Id,
    glass: crate::desktop_glass::WindowGlass,
    asked: bool,
}

impl GlassSync {
    /// Notes whether `window`'s view, just built, wants glass. Returns
    /// whether the window waits for an update to be bound: binding starts in
    /// `update`, and a view that first wants glass outside one (a live theme
    /// change while the window is idle) would otherwise wait, its panes
    /// hidden, until some input arrives.
    pub(crate) fn saw(&self, window: iced::window::Id, wants: bool) -> bool {
        let mut windows = self.windows.borrow_mut();
        if wants && !windows.iter().any(|known| known.id == window) {
            windows.push(GlassWindow {
                id: window,
                glass: crate::desktop_glass::WindowGlass::pending(),
                asked: false,
            });
        }
        windows
            .iter()
            .any(|known| known.id == window && !known.asked)
    }

    /// Takes `message` when it is a [`GlassWanted`] nudge; the update it
    /// starts binds the waiting window.
    pub(crate) fn wanted(&self, message: &enzui_core::app_runtime::ServiceMessage) -> bool {
        message.downcast_ref::<GlassWanted>().is_some()
    }

    /// The glass `window`'s view is drawn with: none for a window that never
    /// wanted any, which keeps in-app glass as before.
    pub(crate) fn glass_of(&self, window: iced::window::Id) -> crate::desktop_glass::WindowGlass {
        self.windows
            .borrow()
            .iter()
            .find(|known| known.id == window)
            .map_or_else(crate::desktop_glass::WindowGlass::unavailable, |known| {
                known.glass.clone()
            })
    }

    /// Binds every window that wants glass and has not been bound yet; each
    /// resolves to a [`GlassBound`] service message.
    pub(crate) fn tasks<T: Send + 'static>(
        &self,
        message: impl Fn(enzui_core::app_runtime::ServiceMessage) -> T + Send + Clone + 'static,
    ) -> iced::Task<T> {
        let mut tasks = Vec::new();
        for known in self
            .windows
            .borrow_mut()
            .iter_mut()
            .filter(|known| !known.asked)
        {
            known.asked = true;
            let id = known.id;
            let message = message.clone();
            tasks.push(crate::desktop_glass::bind(id).map(move |glass| {
                message(enzui_core::app_runtime::ServiceMessage::new(GlassBound(
                    id, glass,
                )))
            }));
        }
        iced::Task::batch(tasks)
    }

    /// Takes `message` when it is a [`GlassBound`], recording the binding.
    pub(crate) fn bound(&self, message: &enzui_core::app_runtime::ServiceMessage) -> bool {
        let Some(GlassBound(id, glass)) = message.downcast_ref::<GlassBound>() else {
            return false;
        };
        if let Some(known) = self
            .windows
            .borrow_mut()
            .iter_mut()
            .find(|known| known.id == *id)
        {
            known.glass = glass.clone();
        }
        true
    }
}

/// Records the corner radius of the frame the shell being built draws, and
/// whether it draws one at all.
pub(crate) fn publish(radius: f32, own_frame: bool) {
    RADIUS.with(|cell| cell.set(radius));
    OWN_FRAME.with(|cell| cell.set(own_frame));
}

/// Whether the view just built has a shell that draws its own frame.
pub(crate) fn take_own_frame() -> bool {
    OWN_FRAME.with(|cell| cell.replace(false))
}

/// The theme's window shadow for a self-drawn frame, as the platform takes it.
pub(crate) fn platform_shadow() -> Option<iced::window::Shadow> {
    let shadow = enzui_theme::text_defaults::window_shadow();
    if shadow.is_none() {
        return None;
    }
    let layers = |active| {
        shadow
            .layers(active)
            .into_iter()
            .map(|layer| iced::window::ShadowLayer {
                color: iced::Color {
                    a: layer.alpha,
                    ..iced::Color::BLACK
                },
                offset: iced::Vector::new(0.0, layer.offset_y),
                blur: layer.blur,
                spread: layer.spread,
            })
            .collect()
    };
    Some(iced::window::Shadow {
        corner_radius: enzui_theme::text_defaults::window_radius(),
        active: layers(true),
        inactive: layers(false),
        resize_border: RESIZE_BORDER,
    })
}

/// How far past a self-drawn frame's edge, into its shadow, a press resizes
/// the window.
const RESIZE_BORDER: f32 = 8.0;

/// The windows whose shell draws its own frame, and the shadow each was last
/// sent.
#[derive(Default)]
pub(crate) struct ShadowSync {
    // A window or two per application: a list in the order they were seen.
    sent: RefCell<Vec<(iced::window::Id, Option<iced::window::Shadow>)>>,
}

impl ShadowSync {
    /// Notes whether `window`'s view, just built, has a shell drawing its own
    /// frame. A window without one gets no shadow from here.
    pub(crate) fn saw(&self, window: iced::window::Id, own_frame: bool) {
        let mut sent = self.sent.borrow_mut();
        let known = sent.iter().position(|(id, _)| *id == window);
        match (own_frame, known) {
            // `None` until the theme's shadow has been sent.
            (true, None) => sent.push((window, None)),
            (false, Some(index)) => {
                let _ = sent.remove(index);
            }
            _ => {}
        }
    }

    /// Sends every self-framed window the theme's shadow where it differs
    /// from what it was last sent.
    pub(crate) fn tasks<T: Send + 'static>(&self) -> iced::Task<T> {
        let shadow = platform_shadow();
        let mut tasks = Vec::new();
        for (window, sent) in self.sent.borrow_mut().iter_mut() {
            if *sent != shadow {
                sent.clone_from(&shadow);
                tasks.push(iced::window::set_shadow(*window, shadow.clone()));
            }
        }
        iced::Task::batch(tasks)
    }
}

/// Wraps a window's view in a paint mask with the radius its shell published
/// while the view was built. The wrapper stays at radius zero, so maximizing
/// a window keeps the tree's shape and every widget's state.
pub(crate) fn host<'a, Message, Theme, Renderer>(
    view: Element<'a, Message, Theme, Renderer>,
) -> Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    let radius = RADIUS.with(|cell| cell.replace(0.0));
    crate::rounded_clip::rounded_clip(view, radius)
        .paint_only()
        .into()
}

#[cfg(test)]
mod tests;

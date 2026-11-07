//! Makes a focus a widget takes on its own exclusive across the window.
//!
//! A widget can focus only inside its own subtree: a search field that
//! focuses itself as it mounts cannot unfocus the titlebar button that
//! opened it, and both would then claim focus. Such a widget calls
//! [`claim`] with the id it focused; the root wrapper [`host`], which
//! `app_setup` installs around every window, then releases every other
//! focus owner once the update finishes.

use std::cell::RefCell;

use enzui_core::focus::release_focus_except;
use iced::advanced::widget::{Id, Operation, Tree, operation};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

thread_local! {
    /// The id a widget focused on its own during the current update.
    static CLAIM: RefCell<Option<Id>> = const { RefCell::new(None) };
}

/// Records that `target` took focus on its own in this update; the window
/// root makes it the only focus owner.
pub(crate) fn claim(target: Id) {
    CLAIM.with(|claim| *claim.borrow_mut() = Some(target));
}

/// Wraps a window's root so focus claims are made exclusive.
pub(crate) fn host<'a, Message, Theme, Renderer>(
    child: Element<'a, Message, Theme, Renderer>,
) -> Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    Element::new(FocusClaimHost { child })
}

struct FocusClaimHost<'a, Message, Theme, Renderer> {
    child: Element<'a, Message, Theme, Renderer>,
}

impl<Message, Theme, Renderer> FocusClaimHost<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    /// Settles a pending claim: every other control holding focus lets go.
    /// The claimed one is left as it is, caret and selection included; a
    /// control that hides its focus while a menu of its own is open (an edit
    /// menu) is still the one that keeps it.
    fn settle(&mut self, tree: &mut Tree, layout: Layout, renderer: &Renderer) -> bool {
        let Some(target) = CLAIM.with(|claim| claim.borrow_mut().take()) else {
            return false;
        };
        let bounds = layout.bounds();
        let mut release = release_focus_except(target);
        self.child.as_widget_mut().operate(
            tree,
            layout,
            &bounds,
            renderer,
            &mut operation::black_box(&mut release),
        );
        matches!(release.finish(), operation::Outcome::Some(released) if released > 0)
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for FocusClaimHost<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        self.child.as_widget().tag()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        self.child.as_widget().state()
    }

    fn diff(&mut self, tree: &mut Tree) {
        self.child.as_widget_mut().diff(tree);
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        self.child.as_widget_mut().layout(tree, renderer, limits);
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
        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);
        if self.settle(tree, layout, renderer) {
            shell.request_redraw();
        }
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        crate::frame_routing::interest_transparent(&self.child, tree)
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
            tree,
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        if self.settle(tree, layout, renderer) {
            shell.request_redraw();
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.child
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
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
        self.child
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.child
            .as_widget_mut()
            .operate(tree, layout, viewport, renderer, operation);
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
        self.child
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
}

#[cfg(test)]
mod tests {
    use enzui_core::collect::SemanticsCollector;
    use iced::advanced::renderer::Headless;
    use iced::widget::column;
    use iced::{Element, Point, Size, mouse};
    use iced_test::runtime::user_interface::{Cache, UserInterface};

    #[derive(Debug, Clone)]
    enum Msg {
        Search,
        Query,
    }

    fn view(open: bool, query: &str) -> Element<'_, Msg> {
        let button = crate::button::enz_button("Search", Msg::Search).into_element();
        let field: Element<'_, Msg> = if open {
            crate::search_field::enz_search_field(query, |_| Msg::Query)
                .focus_on_mount(true)
                .id(iced::advanced::widget::Id::new("claim-field"))
                .into_element()
        } else {
            iced::widget::text("Page").into()
        };
        super::host(column![button, field].spacing(20).into())
    }

    /// Runs `events` over `view` and returns how many nodes claim focus.
    fn run(
        view: Element<'_, Msg>,
        renderer: &mut iced_test::renderer::Renderer,
        cache: &mut Cache,
        events: &[iced::Event],
        at: Point,
    ) -> (usize, Vec<Msg>) {
        let mut ui = UserInterface::build(
            view,
            Size::new(400.0, 300.0),
            std::mem::take(cache),
            renderer,
        );
        let mut bus = iced::advanced::shell::Bus::new();
        let _ = ui.update(
            &iced::advanced::graphics::core::window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            events,
            mouse::Cursor::Available(at),
            renderer,
            &mut bus,
        );
        let mut collector = SemanticsCollector::new();
        ui.operate(renderer, &mut collector);
        let focused = collector
            .collected()
            .iter()
            .filter(|node| node.semantics.focused)
            .count();
        *cache = ui.into_cache();
        (focused, bus.messages().to_vec())
    }

    #[test]
    fn a_field_focusing_itself_on_mount_is_the_only_focus_owner() {
        let mut renderer = iced::futures::executor::block_on(iced_test::renderer::Renderer::new(
            Default::default(),
            None,
        ))
        .expect("renderer");
        let mut cache = Cache::default();
        let at = Point::new(30.0, 14.0);
        let (focused, messages) = run(
            view(false, ""),
            &mut renderer,
            &mut cache,
            &[
                iced::Event::Mouse(mouse::Event::CursorMoved { position: at }),
                iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
                iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
            ],
            at,
        );
        assert_eq!(focused, 1, "the pressed button takes focus");
        assert!(matches!(messages.as_slice(), [Msg::Search]));
        let frame = iced::Event::Window(iced::window::Event::RedrawRequested(
            iced::time::Instant::now(),
        ));
        let (focused, _) = run(view(true, ""), &mut renderer, &mut cache, &[frame], at);
        assert_eq!(
            focused, 1,
            "the mounted field focuses itself and the button lets go"
        );
    }
}

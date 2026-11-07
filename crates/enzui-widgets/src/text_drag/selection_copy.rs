use super::*;
use enzui_core::cancellation::CancelToken;

struct Offer {
    text: String,
    lifetime: CancelToken,
}

#[derive(Default)]
struct Request {
    point: Option<Point>,
    offer: Option<Offer>,
}

/// Offer an existing read-only selection to native text dragging.
///
/// Custom document and terminal widgets call this from `Widget::operate`.
/// `resolve` runs only when a press queries the selection; its point is in the
/// widget operation's coordinate space. Return selected text only when the point
/// hits its visible selection. Retire the token when its document/selection is
/// replaced or unmounted. Native drags are Copy only and bounded to 8 MiB.
/// Normal clicks and unavailable native transport retain ordinary selection.
pub fn offer_read_only_selection(
    operation: &mut dyn Operation,
    id: Option<&Id>,
    bounds: Rectangle,
    resolve: impl FnOnce(Point) -> Option<(String, CancelToken)>,
) {
    let mut request = Request::default();
    operation.custom(id, bounds, &mut request);
    if let Some(point) = request.point
        && let Some((text, lifetime)) = resolve(point)
        && !text.is_empty()
        && text.len() <= 8 * 1024 * 1024
        && !lifetime.is_terminal()
    {
        request.offer = Some(Offer { text, lifetime });
        operation.custom(id, bounds, &mut request);
    }
}

struct Query {
    point: Point,
    found: Option<Offer>,
    scroll: enzui_core::scroll_stack::ScrollStack,
}

impl Operation for Query {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        self.scroll.traverse_enter();
        operate(self);
        self.scroll.traverse_exit();
    }

    fn scrollable(
        &mut self,
        id: Option<&Id>,
        bounds: Rectangle,
        _content: Size,
        translation: Vector,
        _state: &mut dyn iced::advanced::widget::operation::Scrollable,
    ) {
        self.scroll.note_scrollable(id, bounds, translation);
    }

    fn custom(&mut self, _id: Option<&Id>, bounds: Rectangle, value: &mut dyn std::any::Any) {
        let bounds_on_screen = self.scroll.translate(bounds);
        if bounds_on_screen.width <= 0.0
            || bounds_on_screen.height <= 0.0
            || !bounds_on_screen.contains(self.point)
            || self
                .scroll
                .viewport()
                .is_some_and(|clip| !clip.contains(self.point))
        {
            return;
        }
        if let Some(request) = value.downcast_mut::<Request>() {
            if let Some(offer) = request.offer.take() {
                self.found = Some(offer);
            } else {
                request.point =
                    Some(self.point + (bounds.position() - bounds_on_screen.position()));
            }
        }
    }
}

struct Press {
    origin: Point,
    offer: Offer,
}

#[derive(Default)]
pub(super) struct State {
    press: Option<Press>,
}

impl<Message, Theme, Renderer: iced::advanced::Renderer> Host<'_, Message, Theme, Renderer> {
    #[allow(clippy::too_many_arguments, reason = "the widget update context")]
    pub(super) fn copy_selection_event(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) -> bool {
        let pending = tree.state.downcast_mut::<State>().press.take();
        if let Some(press) = pending {
            if press.offer.lifetime.is_terminal() {
                shell.capture_event();
                return true;
            }
            match event {
                Event::Mouse(mouse::Event::CursorMoved { position }) => {
                    if press.origin.distance(*position)
                        < enzui_core::desktop_settings::input().drag_threshold
                    {
                        tree.state.downcast_mut::<State>().press = Some(press);
                    } else if !begin_files(
                        &[],
                        None,
                        Some(Payload::text(&press.offer.text)),
                        false,
                        press.offer.lifetime.share(),
                        None,
                        DragActions::COPY,
                        Arc::new(Mutex::new(None)),
                    ) {
                        self.replay(
                            &mut tree.children[0],
                            &[Event::Mouse(mouse::Event::ButtonPressed(
                                mouse::Button::Left,
                            ))],
                            layout,
                            mouse::Cursor::Available(press.origin),
                            renderer,
                            shell,
                            viewport,
                        );
                        self.replay(
                            &mut tree.children[0],
                            std::slice::from_ref(event),
                            layout,
                            cursor,
                            renderer,
                            shell,
                            viewport,
                        );
                    }
                    shell.capture_event();
                    return true;
                }
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                    self.replay(
                        &mut tree.children[0],
                        &[Event::Mouse(mouse::Event::ButtonPressed(
                            mouse::Button::Left,
                        ))],
                        layout,
                        mouse::Cursor::Available(press.origin),
                        renderer,
                        shell,
                        viewport,
                    );
                    self.replay(
                        &mut tree.children[0],
                        std::slice::from_ref(event),
                        layout,
                        cursor,
                        renderer,
                        shell,
                        viewport,
                    );
                    shell.capture_event();
                    return true;
                }
                Event::Mouse(mouse::Event::CursorLeft)
                | Event::Window(iced::window::Event::Unfocused)
                | Event::Keyboard(_) => {}
                _ => {
                    tree.state.downcast_mut::<State>().press = Some(press);
                }
            }
        }
        if matches!(
            event,
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
        ) && let Some(point) = cursor.position_over(*viewport)
        {
            let ready = {
                let state = shared();
                state.runtime.is_some() && !state.file_source_active && state.source.is_none()
            };
            if ready {
                let mut query = Query {
                    point,
                    found: None,
                    scroll: Default::default(),
                };
                self.operate_at(
                    &mut tree.children[0],
                    layout,
                    renderer,
                    viewport,
                    point,
                    &mut query,
                );
                if let Some(offer) = query.found {
                    tree.state.downcast_mut::<State>().press = Some(Press {
                        origin: point,
                        offer,
                    });
                    shell.capture_event();
                    return true;
                }
            }
        }
        false
    }
}

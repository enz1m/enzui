use iced::advanced::layout::Measure as _;

/// Measure, place and optionally remeasure a popup in window coordinates.
/// `bounds` gives the window-sized constraint at origin zero; `anchor` uses
/// the same coordinate space. With a [`popup_reach`](iced::advanced::overlay::popup_reach)
/// the popup may be placed past the window edges by up to that much, as the
/// [`default_popup_overflow`] allows: by default only when it does not fit
/// inside the window. A width pin applies during both measurements
/// and clamps to resolved width after shrink. Shrink performs one additional
/// measurement using the resolved size; placement is not iterated after reflow.
#[must_use]
pub fn resolve_popup_layout<Message>(
    popup: &mut Element<'_, Message>,
    tree: &mut widget::Tree,
    renderer: &iced::Renderer,
    bounds: Size,
    anchor: Rectangle,
    placement: &Placement,
    pinned_width: Option<f32>,
) -> layout::Node {
    resolve_popup_layout_within(
        popup,
        tree,
        renderer,
        bounds,
        anchor,
        placement,
        pinned_width,
        iced::advanced::overlay::popup_reach(),
        default_popup_overflow(),
    )
}

/// Whether an overlay may be placed past the edge of its window, where the
/// runtime can show it there (in a Wayland popup surface).
///
/// Overlays past the window edge look out of place on tiling window
/// managers and over neighbouring windows, so by default an overlay stays
/// inside and only leaves the window when it cannot fit there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PopupOverflow {
    /// Inside the window, flipped or shifted as needed; past its edge only
    /// when it does not fit inside without being cut or covering its anchor.
    #[default]
    WhenNeeded,
    /// Always inside the window: flipped, shifted or shrunk to fit.
    Never,
    /// Placed as if the window had the screen around it: on its preferred
    /// side even when that is past the window edge.
    Allowed,
}

static DEFAULT_POPUP_OVERFLOW: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Sets the [`PopupOverflow`] of every overlay that does not choose its own:
/// menus, lists, tooltips, popovers and context menus.
pub fn set_default_popup_overflow(overflow: PopupOverflow) {
    let value = match overflow {
        PopupOverflow::WhenNeeded => 0,
        PopupOverflow::Never => 1,
        PopupOverflow::Allowed => 2,
    };
    DEFAULT_POPUP_OVERFLOW.store(value, std::sync::atomic::Ordering::Relaxed);
}

/// The [`PopupOverflow`] set by [`set_default_popup_overflow`].
#[must_use]
pub fn default_popup_overflow() -> PopupOverflow {
    match DEFAULT_POPUP_OVERFLOW.load(std::sync::atomic::Ordering::Relaxed) {
        1 => PopupOverflow::Never,
        2 => PopupOverflow::Allowed,
        _ => PopupOverflow::WhenNeeded,
    }
}

/// Whether `placed` lies within `viewport`, give or take half a pixel.
pub(crate) fn fits_within(viewport: Rectangle, placed: Rectangle) -> bool {
    placed.x >= viewport.x - 0.5
        && placed.y >= viewport.y - 0.5
        && placed.x + placed.width <= viewport.x + viewport.width + 0.5
        && placed.y + placed.height <= viewport.y + viewport.height + 0.5
}

/// Places an overlay with `place(room, viewport)`, which returns its layout
/// and whether it fit. `room` is what to measure against: with a runtime
/// reach, the window and the room around it, so the overlay's own size is
/// known. Under [`PopupOverflow::WhenNeeded`] the window is tried first and
/// the room around it only when the overlay did not fit.
pub(crate) fn place_popup<T>(
    bounds: Size,
    reach: Option<Size>,
    overflow: PopupOverflow,
    mut place: impl FnMut(Size, Rectangle) -> (T, bool),
) -> T {
    let window = Rectangle::with_size(bounds);
    let reach = reach.filter(|_| overflow != PopupOverflow::Never);
    let Some(reach) = reach else {
        return place(bounds, window).0;
    };
    let (room, around) = popup_room(bounds, Some(reach));
    if overflow == PopupOverflow::Allowed {
        return place(room, around).0;
    }
    let (inside, fits) = place(room, window);
    if fits { inside } else { place(room, around).0 }
}

/// The room a popup may measure against and the viewport it is placed in:
/// the window, or, where the runtime shows overlays that leave the window in
/// popup surfaces, the window and the room around it up to `reach` (the
/// compositor keeps the popup on screen).
pub(crate) fn popup_room(bounds: Size, reach: Option<Size>) -> (Size, Rectangle) {
    match reach {
        Some(reach) => (
            Size::new(
                bounds.width.max(reach.width),
                bounds.height.max(reach.height),
            ),
            Rectangle::new(
                Point::new(-reach.width, -reach.height),
                Size::new(
                    bounds.width + 2.0 * reach.width,
                    bounds.height + 2.0 * reach.height,
                ),
            ),
        ),
        None => (bounds, Rectangle::with_size(bounds)),
    }
}

/// [`place_popup`] with the runtime's reach and the default
/// [`PopupOverflow`].
pub(crate) fn place_in_window_first<T>(
    bounds: Size,
    place: impl FnMut(Size, Rectangle) -> (T, bool),
) -> T {
    place_popup(
        bounds,
        iced::advanced::overlay::popup_reach(),
        default_popup_overflow(),
        place,
    )
}

/// [`resolve_popup_layout`] with an explicit popup reach.
#[allow(
    clippy::too_many_arguments,
    reason = "the layout inputs of resolve_popup_layout plus the reach it reads"
)]
pub(crate) fn resolve_popup_layout_within<Message>(
    popup: &mut Element<'_, Message>,
    tree: &mut widget::Tree,
    renderer: &iced::Renderer,
    bounds: Size,
    anchor: Rectangle,
    placement: &Placement,
    pinned_width: Option<f32>,
    reach: Option<Size>,
    overflow: PopupOverflow,
) -> layout::Node {
    place_popup(bounds, reach, overflow, |room, viewport| {
        // Measure against the available room before placement chooses a side.
        let base_limits = layout::Limits::new(Size::ZERO, room);
        let limits = match pinned_width {
            Some(w) => base_limits.width(Length::Fixed(w)),
            None => base_limits,
        };
        let node = popup.as_widget_mut().measure(tree, renderer, &limits);

        let resolved = placement.resolve(anchor, node.size(), viewport);
        let fits = !resolved.shrunk && !resolved.occluded && fits_within(viewport, resolved.bounds);
        let final_node = if resolved.shrunk {
            let resolved_size = resolved.bounds.size();
            let relayout_limits = layout::Limits::new(Size::ZERO, resolved_size);
            let relayout_limits = match pinned_width {
                Some(w) => relayout_limits.width(Length::Fixed(w)),
                None => relayout_limits,
            };
            popup
                .as_widget_mut()
                .measure(tree, renderer, &relayout_limits)
        } else {
            node
        };

        (
            final_node.move_to(Point::new(resolved.bounds.x, resolved.bounds.y)),
            fits,
        )
    })
}
struct PopupOverlay<'a, 'b, Message> {
    popup: &'b mut Element<'a, Message>,
    tree: &'b mut widget::Tree,
    anchor: Rectangle,
    placement: Placement,
    overflow: PopupOverflow,
    index: f32,
    reported_bounds: Option<PopupBounds>,
    /// The opening motion and the frame it is read at.
    reveal: crate::anchored_overlay::PopupReveal,
}

impl<'a, 'b, Message> crate::overlay_adapter::LegacyOverlay<Message, iced::Theme, iced::Renderer>
    for PopupOverlay<'a, 'b, Message>
{
    fn layout(&mut self, renderer: &iced::Renderer, bounds: Size) -> layout::Node {
        let node = resolve_popup_layout_within(
            self.popup,
            self.tree,
            renderer,
            bounds,
            self.anchor,
            &self.placement,
            None,
            iced::advanced::overlay::popup_reach(),
            self.overflow,
        );
        if let Some(slot) = self.reported_bounds.as_ref() {
            slot.set(Some(node.bounds()));
        }
        node
    }

    fn anchor(&self) -> Option<Rectangle> {
        Some(self.anchor)
    }

    fn grabs(&self) -> bool {
        true
    }

    fn draw(
        &self,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: iced::advanced::layout::Placed<'_>,
        cursor: mouse::Cursor,
    ) {
        let layout = layout.to_layout();
        let draw = |renderer: &mut iced::Renderer| {
            self.popup.as_widget().draw(
                self.tree,
                renderer,
                theme,
                style,
                layout,
                cursor,
                &crate::overlay_adapter::UNBOUNDED_VIEWPORT,
            );
        };
        self.reveal
            .draw(renderer, theme, layout.bounds(), self.anchor, draw);
    }

    fn update(
        &mut self,
        event: &Event,
        layout: iced::advanced::layout::Placed<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
    ) {
        self.popup.as_widget_mut().update(
            self.tree,
            event,
            layout.to_layout(),
            cursor,
            renderer,
            shell,
            &crate::overlay_adapter::UNBOUNDED_VIEWPORT,
        );
    }

    fn operate(
        &mut self,
        layout: iced::advanced::layout::Placed<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.popup.as_widget_mut().operate(
            self.tree,
            layout.to_layout(),
            &layout.bounds(),
            renderer,
            operation,
        );
    }

    fn mouse_interaction(
        &self,
        layout: iced::advanced::layout::Placed<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let inner = self.popup.as_widget().mouse_interaction(
            self.tree,
            layout.to_layout(),
            cursor,
            &crate::overlay_adapter::UNBOUNDED_VIEWPORT,
            renderer,
        );
        // Idle inside plain popup content suppresses base hover; outside remains available.
        let over_popup = cursor
            .position()
            .is_some_and(|p| layout.bounds().contains(p));
        if inner == mouse::Interaction::None && over_popup {
            mouse::Interaction::Idle
        } else {
            inner
        }
    }

    fn overlay<'c>(
        &'c mut self,
        layout: iced::advanced::layout::Placed<'_>,
        renderer: &iced::Renderer,
        window: iced::Size,
    ) -> Vec<overlay::Element<'c, Message, iced::Theme, iced::Renderer>> {
        self.popup.as_widget_mut().overlay(
            self.tree,
            layout.to_layout(),
            renderer,
            &crate::overlay_adapter::UNBOUNDED_VIEWPORT,
            Vector::ZERO,
            window,
        )
    }

    fn index(&self) -> f32 {
        self.index
    }
}

fn is_primary_press(event: &Event) -> bool {
    matches!(
        event,
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
            | Event::Touch(iced::touch::Event::FingerPressed { .. })
    )
}

fn is_primary_release(event: &Event) -> bool {
    matches!(
        event,
        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
            | Event::Touch(iced::touch::Event::FingerLifted { .. })
    )
}

fn point_of(event: &Event, cursor: mouse::Cursor) -> Option<Point> {
    match event {
        Event::Touch(
            iced::touch::Event::FingerPressed { position, .. }
            | iced::touch::Event::FingerLifted { position, .. },
        ) => Some(*position),
        _ => cursor.position(),
    }
}
#[must_use]
fn backdrop_step(
    state: &mut BackdropState,
    event: &Event,
    cursor: mouse::Cursor,
    anchor: Rectangle,
    popup: Option<Rectangle>,
) -> bool {
    if state.armed
        && matches!(event, Event::Mouse(mouse::Event::CursorMoved { .. }))
        && cursor
            .position()
            .is_some_and(|p| in_a_hole(p, anchor, popup))
    {
        state.armed = false;
    }
    if is_primary_press(event) {
        // The backdrop dispatches before the popup, so admission must exclude both holes.
        if point_of(event, cursor).is_some_and(|p| !in_a_hole(p, anchor, popup)) {
            state.armed = true;
        }
        return false;
    }
    if is_primary_release(event) {
        let was_armed = state.armed;
        state.armed = false;
        let outside = point_of(event, cursor).is_some_and(|p| !in_a_hole(p, anchor, popup));
        return was_armed && outside;
    }
    false
}
struct DismissBackdrop<'b, Message> {
    anchor: Rectangle,
    popup_bounds: PopupBounds,
    on_dismiss: &'b dyn Fn() -> Message,
    state: &'b mut BackdropState,
    index: f32,
}

impl<'b, Message> crate::overlay_adapter::LegacyOverlay<Message, iced::Theme, iced::Renderer>
    for DismissBackdrop<'b, Message>
{
    fn layout(&mut self, _renderer: &iced::Renderer, bounds: Size) -> layout::Node {
        layout::Node::new(bounds)
    }

    fn draw(
        &self,
        _renderer: &mut iced::Renderer,
        _theme: &iced::Theme,
        _style: &renderer::Style,
        _layout: iced::advanced::layout::Placed<'_>,
        _cursor: mouse::Cursor,
    ) {
    }

    fn update(
        &mut self,
        event: &Event,
        _layout: iced::advanced::layout::Placed<'_>,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
    ) {
        if shell.is_event_captured() {
            return;
        }
        if let Some(rect) = self.popup_bounds.get() {
            self.state.popup = Some(rect);
        }
        let popup = self.state.popup;
        let dismissed = backdrop_step(self.state, event, cursor, self.anchor, popup);
        if is_primary_press(event) && self.state.armed {
            shell.capture_event();
        }
        if dismissed {
            shell.publish((self.on_dismiss)());
            shell.capture_event();
        }
    }

    fn mouse_interaction(
        &self,
        _layout: iced::advanced::layout::Placed<'_>,
        _cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        mouse::Interaction::None
    }

    fn index(&self) -> f32 {
        self.index
    }
}

impl<'a, Message: 'a> From<AnchoredOverlay<'a, Message>> for Element<'a, Message> {
    fn from(w: AnchoredOverlay<'a, Message>) -> Self {
        Element::new(w)
    }
}

//! Leading-edge swipe routing through the shared gesture policy.

use super::*;

/// Measure rightward travel in logical pixels, clamping leftward travel to zero.
/// This numeric helper does not resolve RTL or dispatch the outlet gesture.
#[must_use]
pub fn swipe_distance(pressed_x: f32, current_x: f32) -> f32 {
    (current_x - pressed_x).max(0.0)
}

/// Whether both flags permit a distance at or above the supplied threshold.
/// This numeric predicate does not perform the outlet's gesture admission.
#[must_use]
pub fn back_swipe_commits(
    enabled: bool,
    back_enabled: bool,
    distance: f32,
    threshold: f32,
) -> bool {
    enabled && back_enabled && distance >= threshold
}

pub(crate) struct BackSwipeCapture<'a, Message> {
    pub(crate) child: Element<'a, Message>,
    pub(crate) on_pop: Option<Message>,
    pub(crate) back_enabled: bool,
    pub(crate) edge_width: f32,
    pub(crate) threshold: f32,
}

#[derive(Debug, Default)]
pub(crate) struct BackSwipeState {
    swipe: SwipeGesture,
}

impl<Message: Clone> BackSwipeCapture<'_, Message> {
    fn policy(&self) -> SwipePolicy {
        let mut p = SwipePolicy::new(
            SwipeAxis::Horizontal,
            FinitePositive::new_or(self.threshold, 72.0),
        )
        .edge_strip_leading(self.edge_width);
        // Leading-to-trailing travel has opposite signed x directions under LTR and RTL.
        p = p.direction(match ambient_direction() {
            Direction::LeftToRight => crate::swipe::SwipeDirection::Positive,
            Direction::RightToLeft => crate::swipe::SwipeDirection::Negative,
        });
        p
    }
}

impl<Message: Clone> Widget<Message, iced::Theme, iced::Renderer>
    for BackSwipeCapture<'_, Message>
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<BackSwipeState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(BackSwipeState::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn is_void(&self) -> bool {
        self.child.as_widget().is_void()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.child
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
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
        // Only an unclaimed edge press may start navigation; page controls own their presses.
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        if self.on_pop.is_none() || !self.back_enabled {
            return;
        }

        let bounds = layout.bounds();
        let state = tree.state.downcast_mut::<BackSwipeState>();
        let direction = ambient_direction();
        let policy = self.policy();

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if shell.is_event_captured() {
                    return;
                }
                let Some(position) = cursor.position() else {
                    return;
                };
                let _ = state
                    .swipe
                    .press(&policy, position, None, bounds, direction);
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if !state.swipe.is_active() || state.swipe.owner().is_some() {
                    return;
                }
                let Some(position) = cursor.position() else {
                    return;
                };
                self.resolve(state.swipe.release(&policy, position, None, None), shell);
            }
            Event::Mouse(mouse::Event::CursorLeft) => {
                let _ = state.swipe.cancel(None);
            }
            Event::Touch(touch::Event::FingerPressed { id, position }) => {
                if shell.is_event_captured() {
                    return;
                }
                let _ = state
                    .swipe
                    .press(&policy, *position, Some(*id), bounds, direction);
            }
            Event::Touch(touch::Event::FingerLifted { id, position }) => {
                if state.swipe.owner() != Some(*id) {
                    return;
                }
                self.resolve(
                    state.swipe.release(&policy, *position, Some(*id), None),
                    shell,
                );
            }
            Event::Touch(touch::Event::FingerLost { id, .. }) => {
                let _ = state.swipe.cancel(Some(*id));
            }
            Event::Window(iced::window::Event::Unfocused) => {
                let _ = state.swipe.cancel_any();
            }
            _ => {}
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
        self.child.as_widget().draw(
            &tree.children[0],
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
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
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
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.child.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<Message: Clone> BackSwipeCapture<'_, Message> {
    fn resolve(&self, phase: SwipePhase, shell: &mut Shell<'_, Message>) {
        if !matches!(phase, SwipePhase::Committed { .. }) {
            return;
        }
        let Some(message) = self.on_pop.clone() else {
            return;
        };
        shell.publish(message);
        shell.capture_event();
    }
}

impl<'a, Message: Clone + 'a> From<BackSwipeCapture<'a, Message>> for Element<'a, Message> {
    fn from(widget: BackSwipeCapture<'a, Message>) -> Self {
        Self::new(widget)
    }
}

//! Host-controlled popups positioned beside content or a window-space anchor.
//!
//! Popups occupy an overlay and add no base layout space. Content and popup
//! state reconcile in separate tree slots; popup operations run through its
//! overlay. An anchor outside its translated viewport suppresses the popup
//! and clears pending backdrop admission, even while the host marks it open.
//!
//! Placement defaults below the anchor in the Popup overlay layer. Outside
//! click dismissal is opt-in and requires a completed press/release outside
//! both anchor and popup. The host must apply dismissal messages to close it.
//! Menu helpers also bind uncaptured Escape regardless of modifiers or repeat.
//!
//! A popup that opens after the first frame fades and scales in from the edge
//! facing its anchor; one open from the start, or under reduced motion, draws
//! whole at once.

use enzui_core::placement::{Placement, Side};
use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{self, Widget};
use iced::advanced::{Shell, overlay, renderer};
use iced::mouse;
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector};
use std::cell::Cell;
use std::rc::Rc;

/// Anchor host-supplied popup content without reserving space in the base layout.
pub struct AnchoredOverlay<'a, Message> {
    content: Element<'a, Message>,
    popup: Option<Element<'a, Message>>,
    placement: Placement,
    anchor_bounds: Option<Rectangle>,
    index: f32,
    dismiss: Option<Box<dyn Fn() -> Message + 'a>>,
    overflow: Option<PopupOverflow>,
}

/// Build with bottom placement, Popup layer ordering and no outside dismissal.
/// `Some(popup)` is host-controlled open state; `None` retains only the content.
pub fn anchored_overlay<'a, Message: 'a>(
    content: impl Into<Element<'a, Message>>,
    popup: Option<Element<'a, Message>>,
) -> AnchoredOverlay<'a, Message> {
    AnchoredOverlay {
        content: content.into(),
        popup,
        placement: Placement::new(Side::Bottom),
        anchor_bounds: None,
        index: enzui_core::overlay_layer::OverlayLayer::Popup.z_index(),
        dismiss: None,
        overflow: None,
    }
}

/// Compose anchored popup placement with completed-outside-click and unscoped Escape dismissal.
/// Closed popups claim neither channel. The host must handle the published
/// message; children may capture Escape before the unscoped adapter sees it.
pub fn dismissible_anchored_menu<'a, Message: Clone + 'a>(
    trigger: impl Into<Element<'a, Message>>,
    popup: Option<Element<'a, Message>>,
    placement: Placement,
    on_dismiss: impl Fn() -> Message + Clone + 'a,
) -> Element<'a, Message> {
    let open = popup.is_some();
    let outside = on_dismiss.clone();
    let anchored: Element<'a, Message> = anchored_overlay(trigger, popup)
        .placement(placement)
        .dismiss_on_outside_press(outside)
        .into();
    dismiss_on_escape(anchored, open, on_dismiss)
}

/// Compose dismissal around a popup anchored to a window-space interaction rectangle.
/// The explicit anchor is used without inherited content translation for both
/// placement and the outside-click hole.
pub fn dismissible_anchored_menu_at<'a, Message: Clone + 'a>(
    content: impl Into<Element<'a, Message>>,
    popup: Option<Element<'a, Message>>,
    anchor_bounds: Rectangle,
    placement: Placement,
    on_dismiss: impl Fn() -> Message + Clone + 'a,
) -> Element<'a, Message> {
    let open = popup.is_some();
    let outside = on_dismiss.clone();
    let anchored: Element<'a, Message> = anchored_overlay(content, popup)
        .placement(placement)
        .anchor_bounds(anchor_bounds)
        .dismiss_on_outside_press(outside)
        .into();
    dismiss_on_escape(anchored, open, on_dismiss)
}

/// Publish dismissal for uncaptured Escape presses while open, wherever focus sits.
/// Modifiers and auto-repeat are accepted. The wrapper stays mounted while
/// closed but claims no keys, preserving child state across toggles.
pub fn dismiss_on_escape<'a, Message: Clone + 'a>(
    content: impl Into<Element<'a, Message>>,
    open: bool,
    on_dismiss: impl Fn() -> Message + 'a,
) -> Element<'a, Message> {
    // Keep the wrapper mounted while closed so toggling does not reset trigger state.
    crate::keyboard_capture::keyboard_capture_unscoped(content.into(), move |key, _| {
        (open
            && matches!(
                key,
                iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape)
            ))
        .then(&on_dismiss)
    })
}

impl<'a, Message> AnchoredOverlay<'a, Message> {
    /// Override the shared placement policy; defaults to bottom placement.
    #[must_use]
    pub fn placement(mut self, placement: Placement) -> Self {
        self.placement = placement;
        self
    }

    /// Use an explicit window-space anchor without adding content translation.
    /// Placement and outside-click exclusion use this same rectangle.
    #[must_use]
    pub fn anchor_bounds(mut self, bounds: Rectangle) -> Self {
        self.anchor_bounds = Some(bounds);
        self
    }

    fn resolved_anchor(&self, content_bounds: Rectangle, translation: Vector) -> Rectangle {
        self.anchor_bounds
            .unwrap_or_else(|| content_bounds + translation)
    }

    /// Set overlay ordering verbatim; defaults to the Popup layer above tooltips.
    /// The dismissal backdrop uses `index - 0.5`. Use finite values consistent
    /// with the host's overlay ordering.
    #[must_use]
    pub fn index(mut self, index: f32) -> Self {
        self.index = index;
        self
    }

    /// Whether the popup may leave the window; by default the
    /// [`default_popup_overflow`]: inside unless it does not fit there.
    #[must_use]
    pub fn popup_overflow(mut self, overflow: PopupOverflow) -> Self {
        self.overflow = Some(overflow);
        self
    }

    /// Publish on completed clicks outside both anchor and popup; defaults off.
    /// Presses over either rectangle remain available to their controls. The
    /// backdrop captures admitted outside presses and committing releases, and
    /// mouse movement onto either hole cancels admission. No backdrop is mounted
    /// while closed or when the anchor is outside its viewport.
    #[must_use]
    pub fn dismiss_on_outside_press(mut self, on_dismiss: impl Fn() -> Message + 'a) -> Self {
        self.dismiss = Some(Box::new(on_dismiss));
        self
    }

    /// Whether the host supplied popup content, independent of anchor visibility.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.popup.is_some()
    }
}

impl<'a, Message> AnchoredOverlay<'a, Message> {
    fn child_count(&self) -> usize {
        1 + usize::from(self.popup.is_some())
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct BackdropState {
    armed: bool,
    // Retain the last popup hole when events arrive without a preceding layout.
    popup: Option<Rectangle>,
    /// The popup fading and scaling in as it opens.
    reveal: PopupReveal,
}

/// How long an opening popup takes to unroll, in milliseconds: the
/// `motion-normal` duration, so `ENZUI_MOTION_SCALE` stretches it too.
fn reveal_ms() -> f32 {
    enzui_theme::MotionScale::default().normal_ms
}

/// How far past the popup its shadow can reach; the reveal keeps it in.
const REVEAL_SHADOW_ROOM: f32 = 32.0;

/// A popup unrolling from the edge facing its anchor as it opens, the way
/// a dropdown drops out of its button (WinUI's combo box, Material's
/// menus): the visible part grows away from the anchor, eased out, while
/// the popup fades in. The content does not scale, so text stays sharp.
/// Kept in the owning widget's state, tracked from its `update`, and drawn
/// around the popup by its overlay. Every framework popup (menus, pickers,
/// combo boxes, suggestion lists, context menus) opens this way.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct PopupReveal {
    reveal: crate::motion::Swell,
    frame_at: Option<iced::time::Instant>,
}

impl PopupReveal {
    /// Resting open or closed.
    pub(crate) fn new(open: bool) -> Self {
        Self {
            reveal: crate::motion::Swell::snap(open),
            frame_at: None,
        }
    }

    /// Follows `open` from the owner's `update`: an opening popup starts
    /// its reveal at the next presented frame and asks for frames until it
    /// is whole.
    pub(crate) fn track<Message>(
        &mut self,
        event: &iced::Event,
        open: bool,
        shell: &mut iced::advanced::Shell<'_, Message>,
    ) {
        self.reveal
            .track(&mut self.frame_at, event, open, reveal_ms(), shell);
    }

    /// Whether the last presented frame was mid-reveal, so the owner keeps
    /// asking for frames.
    pub(crate) fn is_revealing(&self) -> bool {
        self.frame_at
            .is_some_and(|now| self.reveal.animating(now, reveal_ms()))
    }

    /// Draws the popup at `bounds` through `draw`, unrolled and faded while
    /// it is revealing. A popup no frame has reached yet draws whole.
    pub(crate) fn draw(
        &self,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        bounds: Rectangle,
        anchor: Rectangle,
        draw: impl FnOnce(&mut iced::Renderer),
    ) {
        use iced::advanced::Renderer as _;
        let shown = if self.reveal.target {
            self.reveal.paint(self.frame_at, reveal_ms(), theme)
        } else {
            1.0
        };
        if shown >= 1.0 {
            draw(renderer);
            return;
        }
        // The composite group is the part revealed so far, with room for the
        // shadow on every side but the one still unrolling.
        let room = REVEAL_SHADOW_ROOM;
        let shown_height = bounds.height * shown;
        let clip = if bounds.center_y() >= anchor.center_y() {
            Rectangle {
                x: bounds.x - room,
                y: bounds.y - room,
                width: bounds.width + 2.0 * room,
                height: room + shown_height,
            }
        } else {
            Rectangle {
                x: bounds.x - room,
                y: bounds.y + bounds.height - shown_height,
                width: bounds.width + 2.0 * room,
                height: shown_height + room,
            }
        };
        let opacity = (shown * 2.0).min(1.0);
        renderer.with_layer_composite(clip, opacity, 1.0, 0.0, clip.center(), draw);
    }
}

type PopupBounds = Rc<Cell<Option<Rectangle>>>;
fn in_a_hole(p: Point, anchor: Rectangle, popup: Option<Rectangle>) -> bool {
    anchor.contains(p) || popup.is_some_and(|r| r.contains(p))
}

impl widget::tree::Interactive for BackdropState {
    fn cancel_interaction(&mut self) {
        self.armed = false;
    }
}

impl<'a, Message> Widget<Message, iced::Theme, iced::Renderer> for AnchoredOverlay<'a, Message> {
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<BackdropState>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::interactive(BackdropState {
            reveal: PopupReveal::new(self.popup.is_some()),
            ..BackdropState::default()
        })
    }

    fn diff(&mut self, tree: &mut widget::Tree) {
        let expected = self.child_count();

        let mut children: Vec<&mut Element<'a, Message>> = match self.popup.as_mut() {
            Some(popup) => vec![&mut self.content, popup],
            None => vec![&mut self.content],
        };

        debug_assert_eq!(
            children.len(),
            expected,
            "the overlay tree has one child per layer"
        );
        tree.diff_children_custom(
            &mut children,
            |state, item| state.diff(item.as_widget_mut()),
            |item| widget::Tree::new(item.as_widget()),
        );
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn is_void(&self) -> bool {
        self.content.as_widget().is_void()
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
    }

    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        // The first frame after a rebuild (the one a popup opens on) arrives
        // here rather than through `update_frame`.
        if let Event::Window(iced::window::Event::RedrawRequested(_)) = event {
            let open = self.popup.is_some();
            tree.state
                .downcast_mut::<BackdropState>()
                .reveal
                .track(event, open, shell);
        }
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        crate::frame_routing::refresh_opaque_child(&self.content, tree, 0);
    }

    fn mouse_interaction(
        &self,
        tree: &widget::Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    // Always on, so every frame of a reveal reaches it.
    fn frame_interest(&self, _tree: &widget::Tree) -> bool {
        true
    }

    fn update_frame(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let open = self.popup.is_some();
        tree.state
            .downcast_mut::<BackdropState>()
            .reveal
            .track(event, open, shell);
        crate::frame_routing::forward_frame_opaque(
            &mut self.content,
            tree,
            0,
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
        tree: &widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        // Popup operations belong to its overlay; visiting both paths duplicates semantics.
        self.content.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            viewport,
            renderer,
            operation,
        );
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut widget::Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        let anchor = self.resolved_anchor(layout.bounds(), translation);
        let widget::Tree {
            state: tree_state,
            children: tree_children,
            ..
        } = tree;
        let backdrop_state = tree_state.downcast_mut::<BackdropState>();

        let (content_tree, popup_tree) = {
            let (first, rest) = tree_children.split_at_mut(1);
            (&mut first[0], rest.first_mut())
        };
        let content_overlay = self.content.as_widget_mut().overlay(
            content_tree,
            layout,
            renderer,
            viewport,
            translation,
            window,
        );
        // Scroll viewports need translation back into the anchor's window coordinates.
        let visible = anchor
            .intersection(&(*viewport + translation))
            .is_some_and(|rect| rect.width > 0.0 && rect.height > 0.0);
        if !visible {
            backdrop_state.armed = false;
            backdrop_state.popup = None;
            return content_overlay;
        }
        let popup_bounds: PopupBounds = Rc::new(Cell::new(None));

        let popup_open = self.popup.is_some();
        let popup_overlay = match (self.popup.as_mut(), popup_tree) {
            (Some(popup), Some(popup_tree)) => {
                vec![crate::overlay_adapter::element(
                    PopupOverlay {
                        popup,
                        tree: popup_tree,
                        anchor,
                        placement: self.placement.clone(),
                        overflow: self.overflow.unwrap_or_else(default_popup_overflow),
                        index: self.index,
                        reported_bounds: Some(PopupBounds::clone(&popup_bounds)),
                        reveal: backdrop_state.reveal,
                    },
                    renderer,
                    window,
                )]
            }
            _ => Vec::new(),
        };
        let backdrop_overlay = match (popup_open, self.dismiss.as_ref()) {
            (true, Some(on_dismiss)) => vec![crate::overlay_adapter::element(
                DismissBackdrop {
                    anchor,
                    popup_bounds,
                    on_dismiss: on_dismiss.as_ref(),
                    state: backdrop_state,
                    index: self.index - 0.5,
                },
                renderer,
                window,
            )],
            _ => Vec::new(),
        };
        let mut children: Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> =
            Vec::new();
        children.extend(content_overlay);
        children.extend(popup_overlay);
        children.extend(backdrop_overlay);
        children
    }
}
include!("anchored_overlay/transport.rs");

#[cfg(test)]
mod tests;

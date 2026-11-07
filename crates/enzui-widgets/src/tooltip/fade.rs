//! Public tooltip constructors and the [`FadeTooltip`] custom widget with its fade overlay.

use super::*;
use iced::advanced::layout::Measure as _;
use iced::advanced::widget::tree::Interactive as _;

/// Wrap content with a hover tooltip that fades over the theme's fast duration.
///
/// Empty or whitespace-only text returns the content unchanged. Text is owned,
/// so dynamically formatted hints can be passed directly. `FollowCursor` follows
/// the last mouse-event position. Reduced-motion policy is observed at draw;
/// before the first draw, the fade is suppressed. Hover waits for the app default
/// delay (500 ms initially); use [`enz_tooltip_with_delay`] to override it.
pub fn enz_tooltip<'a, Message: 'a>(
    content: Element<'a, Message>,
    tip: impl Into<String>,
    position: Position,
) -> Element<'a, Message> {
    let tip = tip.into();
    if !tooltip_is_meaningful(&tip) {
        return content;
    }
    FadeTooltip::new(content, tip, position)
        .delay(super::default_tooltip_delay())
        .into()
}

/// Wrap content with a custom hover delay; zero starts the reveal immediately.
/// Empty text suppresses the tooltip without changing the content.
pub fn enz_tooltip_with_delay<'a, Message: 'a>(
    content: Element<'a, Message>,
    tip: impl Into<String>,
    position: Position,
    delay: Duration,
) -> Element<'a, Message> {
    let tip = tip.into();
    if !tooltip_is_meaningful(&tip) {
        return content;
    }
    FadeTooltip::new(content, tip, position).delay(delay).into()
}

pub(crate) fn tooltip_is_meaningful(tip: &str) -> bool {
    !tip.trim().is_empty()
}

/// Wrap content with a fading tooltip whose appearance is pinned to explicit tokens.
/// Empty or whitespace-only text returns the content unchanged. The theme's
/// motion timing and reduced-motion policy still apply.
pub fn enz_tooltip_with_tokens<'a, Message: 'a>(
    content: Element<'a, Message>,
    tip: impl Into<String>,
    position: Position,
    tokens: TooltipTokens,
) -> Element<'a, Message> {
    let tip = tip.into();
    if !tooltip_is_meaningful(&tip) {
        return content;
    }
    FadeTooltip::new(content, tip, position)
        .tokens(tokens)
        .delay(super::default_tooltip_delay())
        .into()
}

/// Build the standalone tooltip bubble without hover state or positioning.
/// Unpinned colours follow the runtime theme; supplied tokens pin appearance.
/// Unlike [`enz_tooltip`], this accepts empty text and still builds padded chrome.
pub fn enz_tooltip_bubble<'a, Message: 'a>(
    tip: &'a str,
    pinned_tokens: Option<TooltipTokens>,
) -> Element<'a, Message> {
    faded_bubble(tip, pinned_tokens, 1.0)
}

/// Build the standalone bubble with alpha-scaled fill, text and shadow.
/// Finite `opacity` values are clamped to `0.0..=1.0`; supply a finite value.
/// Opacity changes paint without changing layout.
pub fn enz_tooltip_bubble_opacity<'a, Message: 'a>(
    tip: &'a str,
    pinned_tokens: Option<TooltipTokens>,
    opacity: f32,
) -> Element<'a, Message> {
    faded_bubble(tip, pinned_tokens, opacity)
}

use iced::advanced::Shell;
use iced::advanced::layout::{self, Layout};
use iced::advanced::overlay;
use iced::advanced::renderer;
use iced::advanced::widget::{self, Widget};
use iced::mouse;
use iced::window;
use iced::{Event, Length, Point, Rectangle, Size, Vector};

#[cfg(test)]
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct FadeOverlayShapeProbe {
    pub(crate) published_children: Option<usize>,
}

#[cfg(test)]
std::thread_local! {
    static FADE_OVERLAY_SHAPE_PROBE: std::cell::RefCell<FadeOverlayShapeProbe> =
        std::cell::RefCell::new(FadeOverlayShapeProbe::default());
}

#[cfg(test)]
fn record_published_overlay_children(children: usize) {
    FADE_OVERLAY_SHAPE_PROBE.with(|probe| {
        let mut probe = probe.borrow_mut();
        probe.published_children = Some(children);
    });
}

#[derive(Debug, Clone)]
pub(crate) struct FadeState {
    fade: TooltipFade,
    pub(crate) hovered: bool,
    hover_started: Option<Instant>,
    // Child layout coordinates; overlay translation is applied exactly once.
    pub(crate) cursor: Point,
    reduced_motion: bool,
    // Redraws advance the fade; input samples wall time so idle frames cannot skip a delay.
    pub(crate) frame_at: std::cell::Cell<Instant>,
    // Shape changes must invalidate cached overlay layout before the rebuilt tree draws.
    published_visible: std::cell::Cell<bool>,
    // Unknown policy suppresses interpolation until the first theme-bearing draw.
    theme_reduced_motion: std::cell::Cell<Option<bool>>,
    theme_fast_ms: std::cell::Cell<f32>,
}

impl Default for FadeState {
    fn default() -> Self {
        Self {
            fade: TooltipFade::new(true),
            hovered: false,
            hover_started: None,
            cursor: Point::ORIGIN,
            reduced_motion: true,
            frame_at: std::cell::Cell::new(Instant::now()),
            published_visible: std::cell::Cell::new(false),
            theme_reduced_motion: std::cell::Cell::new(None),
            theme_fast_ms: std::cell::Cell::new(f32::NAN),
        }
    }
}

/// A hover tooltip with a persistent bubble tree and theme-controlled reveal fade.
///
/// The theme policy is latched during draw and applied on subsequent updates.
/// Before any draw, the default suppresses interpolation. Mouse events own the
/// hover latch; redraws advance the fade without changing the tracked cursor.
/// Child overlays remain available, and the tooltip uses the layer below
/// popup and menu surfaces. Empty text is filtered by the convenience constructors
/// but not by [`Self::new`].
#[must_use]
pub struct FadeTooltip<'a, Message> {
    content: Element<'a, Message>,
    tip: String,
    position: Position,
    pinned_tokens: Option<TooltipTokens>,
    reduced_motion: Option<bool>,
    delay: Duration,
    overflow: Option<crate::anchored_overlay::PopupOverflow>,
    // Fresh overlay borrows must share the persistent shaped paragraph tree.
    bubble_slot: Option<Element<'a, Message>>,
}

impl<'a, Message: 'a> FadeTooltip<'a, Message> {
    /// Create a fading tooltip around `content`.
    pub fn new(content: Element<'a, Message>, tip: impl Into<String>, position: Position) -> Self {
        Self {
            content,
            tip: tip.into(),
            position,
            pinned_tokens: None,
            reduced_motion: None,
            delay: Duration::ZERO,
            overflow: None,
            bubble_slot: None,
        }
    }

    /// Wait for uninterrupted pointer hover before starting the reveal fade.
    /// Defaults to zero; reduced motion changes the fade, not this delay.
    pub fn delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    /// Pin bubble colours and geometry while retaining theme-controlled motion.
    pub fn tokens(mut self, tokens: TooltipTokens) -> Self {
        self.pinned_tokens = Some(tokens);
        self
    }

    /// Override the theme's reduced-motion policy for this instance.
    /// Both `true` and `false` take precedence; the default follows the policy
    /// latched during draw, using instant reveal before the first draw.
    pub fn reduced_motion(mut self, reduced_motion: bool) -> Self {
        self.reduced_motion = Some(reduced_motion);
        self
    }

    /// Whether the tooltip may leave the window; by default the
    /// [`default_popup_overflow`](crate::anchored_overlay::default_popup_overflow):
    /// inside unless it does not fit there.
    pub fn popup_overflow(mut self, overflow: crate::anchored_overlay::PopupOverflow) -> Self {
        self.overflow = Some(overflow);
        self
    }

    fn gap(&self) -> f32 {
        FiniteNonNeg::new_or(
            self.pinned_tokens.unwrap_or_else(TooltipTokens::dark).gap,
            6.0,
        )
        .get()
    }

    fn effective_reduced_motion(&self, theme_reduced_motion: Option<bool>) -> bool {
        match self.reduced_motion {
            Some(explicit) => explicit,
            None => theme_reduced_motion.unwrap_or(true),
        }
    }
}

/// Compatibility alias for [`enz_tooltip`] with the same blank-text handling.
pub fn enz_fade_tooltip<'a, Message: 'a>(
    content: Element<'a, Message>,
    tip: impl Into<String>,
    position: Position,
) -> Element<'a, Message> {
    let tip = tip.into();
    if !tooltip_is_meaningful(&tip) {
        return content;
    }
    FadeTooltip::new(content, tip, position)
        .delay(super::default_tooltip_delay())
        .into()
}

impl<'a, Message: 'a> Widget<Message, iced::Theme, iced::Renderer> for FadeTooltip<'a, Message> {
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<FadeState>()
    }

    fn state(&self) -> widget::tree::State {
        let initial = self.effective_reduced_motion(None);
        widget::tree::State::interactive(FadeState {
            fade: TooltipFade::new(initial),
            reduced_motion: initial,
            ..FadeState::default()
        })
    }

    fn diff(&mut self, tree: &mut widget::Tree) {
        let state = tree.state.downcast_mut::<FadeState>();
        let effective = self.effective_reduced_motion(state.theme_reduced_motion.get());
        if state.reduced_motion != effective {
            let at = state.frame_at.get();
            state.fade.set_reduced_motion(effective, at);
            state.reduced_motion = effective;
        }
        let mut bubble_placeholder = faded_bubble::<Message>(&self.tip, self.pinned_tokens, 1.0);
        tree.diff_children(&mut [
            self.content.as_widget_mut(),
            bubble_placeholder.as_widget_mut(),
        ]);
    }

    fn is_void(&self) -> bool {
        self.content.as_widget().is_void()
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
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
        let state = tree.state.downcast_mut::<FadeState>();
        let now = match event {
            Event::Window(window::Event::RedrawRequested(now)) => *now,
            _ => Instant::now().max(state.frame_at.get()),
        };
        state.frame_at.set(now);
        let fast_ms = state.theme_fast_ms.get();
        if fast_ms.is_finite() {
            state.fade.sync_motion(
                &enzui_theme::MotionScale::default().with_fast_ms(fast_ms),
                now,
            );
        }
        let effective = self.effective_reduced_motion(state.theme_reduced_motion.get());
        if state.reduced_motion != effective {
            state.fade.set_reduced_motion(effective, now);
            state.reduced_motion = effective;
        }
        match event {
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let over = cursor.position_over(layout.bounds());
                if let Some(point) = over {
                    state.cursor = point;
                    if !state.hovered {
                        state.hover_started = Some(now);
                    }
                    state.hovered = true;
                } else {
                    state.cancel_interaction();
                }
            }
            Event::Mouse(
                mouse::Event::ButtonPressed(_)
                | mouse::Event::WheelScrolled { .. }
                | mouse::Event::CursorLeft,
            )
            | Event::Window(window::Event::Unfocused | window::Event::Closed) => {
                state.cancel_interaction();
                // Interaction suppresses another hint until the pointer leaves its control.
                state.hovered = cursor.is_over(layout.bounds())
                    && !matches!(
                        event,
                        Event::Mouse(mouse::Event::CursorLeft)
                            | Event::Window(window::Event::Unfocused | window::Event::Closed)
                    );
            }
            _ => {}
        }
        if let Some(deadline) = state
            .hover_started
            .and_then(|at| at.checked_add(self.delay))
        {
            if now >= deadline {
                if !state.fade.is_visible(now) {
                    state.fade.show(now);
                }
            } else {
                shell.request_redraw_at(deadline);
            }
        }
        if state.fade.is_visible(now) != state.published_visible.get() {
            shell.invalidate_overlay();
            shell.request_redraw();
        }
        if state.fade.is_animating(now) {
            shell.request_redraw();
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
        let state = tree.state.downcast_ref::<FadeState>();
        let motion = crate::theme_lookup::motion(theme);
        state.theme_reduced_motion.set(Some(motion.reduced_motion));
        state.theme_fast_ms.set(motion.fast_ms);

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
        // The tip describes the control it wraps.
        let mut description = enzui_core::collect::DescribesContent(self.tip.clone());
        operation.custom(None, layout.bounds(), &mut description);
        let content = &mut self.content;
        operation.traverse(&mut |operation| {
            content.as_widget_mut().operate(
                &mut tree.children[0],
                layout,
                viewport,
                renderer,
                operation,
            );
        });
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
        let (opacity, visible, cursor_pos) = {
            let state = tree.state.downcast_ref::<FadeState>();
            let now = state.frame_at.get();
            let visible = state.fade.is_visible(now);
            state.published_visible.set(visible);
            (state.fade.opacity(now), visible, state.cursor)
        };
        let (content_slice, bubble_slice) = tree.children.split_at_mut(1);
        let child_tree = &mut content_slice[0];
        let bubble_tree = &mut bubble_slice[0];
        let tokens = self.pinned_tokens;
        let position = self.position;
        let gap = self.gap();
        let overflow = self
            .overflow
            .unwrap_or_else(crate::anchored_overlay::default_popup_overflow);
        let content = self.content.as_widget_mut().overlay(
            child_tree,
            layout,
            renderer,
            viewport,
            translation,
            window,
        );
        let tooltip = if visible {
            let mut bubble = faded_bubble::<Message>(&self.tip, tokens, opacity);
            bubble.as_widget_mut().diff(bubble_tree);
            let bubble_ref = self.bubble_slot.insert(bubble);
            Some(crate::overlay_adapter::element(
                FadeOverlay {
                    bubble: bubble_ref,
                    bubble_tree,
                    anchor: layout.bounds() + translation,
                    position,
                    gap,
                    overflow,
                    cursor_pos,
                    translation,
                },
                renderer,
                window,
            ))
        } else {
            None
        };

        // Equal-layer overlays retain child-before-tooltip order; named popup/menu layers stay above.
        let children: Vec<_> = content.into_iter().chain(tooltip).collect();
        #[cfg(test)]
        record_published_overlay_children(children.len());
        children
    }
}

pub(crate) fn content_cap(cap: f32, padding_x: f32) -> f32 {
    // Padding consumes the bubble cap; an impossible cap leaves text unconstrained.
    let content = cap - 2.0 * padding_x;
    if content > 0.0 {
        content
    } else {
        f32::INFINITY
    }
}

pub(crate) fn faded_bubble<'a, Message: 'a>(
    tip: &str,
    pinned_tokens: Option<TooltipTokens>,
    opacity: f32,
) -> Element<'a, Message> {
    let layout = pinned_tokens.unwrap_or_else(TooltipTokens::dark);
    let radius = FiniteNonNeg::new_or(layout.radius, 6.0).get();
    let text_size = FiniteNonNeg::new_or(layout.text_size, 12.0).get();
    let padding_x = FiniteNonNeg::new_or(layout.padding_x, 8.0).get();
    let padding_y = FiniteNonNeg::new_or(layout.padding_y, 4.0).get();
    let cap = FiniteNonNeg::new_or(layout.max_width, 240.0).get();
    let tip = tip.to_owned();

    let resolve = move |theme: &iced::Theme| -> TooltipTokens {
        let tokens = crate::theme_lookup::resolve_pinned(
            crate::theme_lookup::PIN_SITE_TOOLTIP,
            pinned_tokens,
            theme,
            TooltipTokens::from_palette,
        );
        if pinned_tokens.is_some() {
            return tokens;
        }
        TooltipTokens {
            radius: crate::theme_lookup::shape_radius(theme, |s| s.tooltip),
            ..tokens
        }
    };
    let tip_text = enz_text(tip)
        .size_px(text_size)
        .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
        .ellipsis(iced::widget::text::Ellipsis::None)
        .color_resolved(move |theme: &iced::Theme| scale_alpha(resolve(theme).text_color, opacity))
        .into_element();
    // The cap includes the container padding outside the text.
    let capped_tip = max_width(tip_text, content_cap(cap, padding_x));

    let bubble = container(capped_tip)
        .padding(Padding {
            top: padding_y,
            bottom: padding_y,
            left: padding_x,
            right: padding_x,
        })
        .style(move |theme: &iced::Theme| bubble_style_opacity(&resolve(theme), radius, opacity));
    // A shown tooltip is a tooltip node holding its text.
    crate::semantics_container::SemanticsContainer::new(
        bubble,
        enzui_core::semantics::Semantics::new(enzui_core::semantics::Role::Tooltip),
    )
    .into()
}

pub(crate) fn tooltip_placement(position: Position, gap: f32) -> Placement {
    let side = match position {
        Position::Top | Position::FollowCursor => Side::Top,
        Position::Bottom => Side::Bottom,
        Position::Left => Side::Left,
        Position::Right => Side::Right,
    };

    Placement::new(side).gap(gap).overflow(Overflow::Clamp)
}

pub(crate) struct FadeOverlay<'a, 'b, Message> {
    pub(crate) bubble: &'b mut Element<'a, Message>,
    pub(crate) bubble_tree: &'b mut widget::Tree,
    pub(crate) anchor: Rectangle,
    pub(crate) position: Position,
    pub(crate) gap: f32,
    pub(crate) overflow: crate::anchored_overlay::PopupOverflow,
    // Child coordinates; anchor is already translated, but FollowCursor needs translation here.
    pub(crate) cursor_pos: Point,
    pub(crate) translation: Vector,
}

impl<'a, 'b, Message> crate::overlay_adapter::LegacyOverlay<Message, iced::Theme, iced::Renderer>
    for FadeOverlay<'a, 'b, Message>
{
    fn layout(&mut self, renderer: &iced::Renderer, bounds: Size) -> layout::Node {
        // The window, or, when the bubble does not fit there, the room
        // around it where tooltips may leave it.
        let follow = self.position == Position::FollowCursor;
        let point = self.cursor_pos + self.translation;
        let (position, gap, anchor) = (self.position, self.gap, self.anchor);
        let bubble = &mut *self.bubble;
        let bubble_tree = &mut *self.bubble_tree;
        crate::anchored_overlay::place_popup(
            bounds,
            iced::advanced::overlay::popup_reach(),
            self.overflow,
            |room, viewport| {
                let node = bubble.as_widget_mut().measure(
                    bubble_tree,
                    renderer,
                    &layout::Limits::new(Size::ZERO, room),
                );
                let (placed, fits) = if follow {
                    let resolved =
                        enzui_core::placement::place_at_point(point, node.size(), viewport, gap);
                    (resolved.bounds, true)
                } else {
                    let resolved =
                        tooltip_placement(position, gap).resolve(anchor, node.size(), viewport);
                    (resolved.bounds, !resolved.occluded && !resolved.shrunk)
                };
                let fits = fits && crate::anchored_overlay::fits_within(viewport, placed);
                (node.move_to(Point::new(placed.x, placed.y)), fits)
            },
        )
    }

    fn anchor(&self) -> Option<Rectangle> {
        Some(if self.position == Position::FollowCursor {
            Rectangle::new(self.cursor_pos + self.translation, Size::new(1.0, 1.0))
        } else {
            self.anchor
        })
    }

    fn operate(
        &mut self,
        layout: iced::advanced::layout::Placed<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.bubble.as_widget_mut().operate(
            self.bubble_tree,
            layout.to_layout(),
            &layout.bounds(),
            renderer,
            operation,
        );
    }

    fn draw(
        &self,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: iced::advanced::layout::Placed<'_>,
        cursor: mouse::Cursor,
    ) {
        self.bubble.as_widget().draw(
            self.bubble_tree,
            renderer,
            theme,
            style,
            layout.to_layout(),
            cursor,
            &crate::overlay_adapter::UNBOUNDED_VIEWPORT,
        );
    }

    fn index(&self) -> f32 {
        enzui_core::overlay_layer::OverlayLayer::Tooltip.z_index()
    }
}

impl<'a, Message: 'a> From<FadeTooltip<'a, Message>> for Element<'a, Message> {
    fn from(t: FadeTooltip<'a, Message>) -> Self {
        Element::new(t)
    }
}

impl widget::tree::Interactive for FadeState {
    fn cancel_interaction(&mut self) {
        self.hovered = false;
        self.hover_started = None;
        self.fade = TooltipFade::with_duration(self.reduced_motion, self.fade.raw_duration());
    }
}

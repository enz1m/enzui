//! Theme-aware elevation shadows and an animated card wrapper.
//!
//! [`EnzElevation`] uses the theme's elevation ladder and shadow colour, with
//! zero padding, a 10 px default radius, and an outline above level zero.
//! Level changes animate over the theme's normal motion duration; explicit
//! [`progress`](EnzElevation::progress) disables the internal clock.
//! Reduced motion snaps to the target, with an instance override taking precedence.

use enzui_theme::ElevationScale;
use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{self, Widget};
use iced::advanced::{Shell, overlay, renderer};
use iced::animation::Animation;
use iced::mouse;
use iced::time::Instant;
use iced::window;
use iced::{Color, Element, Event, Length, Rectangle, Shadow, Size, Vector};
use std::cell::Cell;
use std::rc::Rc;

/// Shadow blur and vertical offset in pixels; colour is supplied separately.
#[derive(Debug, Clone, Copy, PartialEq)]
#[must_use]
pub struct ElevationShadow {
    /// Blur radius in pixels.
    pub blur: f32,
    /// Vertical offset in pixels; negative values move the shadow upward.
    pub y_offset: f32,
}

impl ElevationShadow {
    /// Interpolate shadow geometry, clamping finite progress to `0.0..=1.0`.
    /// Non-finite progress selects the source; shape fields are not validated.
    pub fn lerp(self, to: ElevationShadow, t: f32) -> ElevationShadow {
        let t = if t.is_finite() {
            t.clamp(0.0, 1.0)
        } else {
            0.0
        };
        ElevationShadow {
            blur: self.blur + (to.blur - self.blur) * t,
            y_offset: self.y_offset + (to.y_offset - self.y_offset) * t,
        }
    }
}

/// Resolve geometry from an explicit ladder, clamping levels above five.
/// Returns `None` when both blur and offset are zero, regardless of level.
pub fn shadow_shape_from(scale: &ElevationScale, level: u8) -> Option<ElevationShadow> {
    let tier = scale.tier(level);
    if tier.blur == 0.0 && tier.y_offset == 0.0 {
        return None;
    }
    Some(ElevationShadow {
        blur: tier.blur,
        y_offset: tier.y_offset,
    })
}

/// Resolve geometry from the stock ladder; level zero is flush and levels above five clamp.
pub fn shadow_shape(level: u8) -> Option<ElevationShadow> {
    shadow_shape_from(&ElevationScale::default(), level)
}

/// Compute a padding envelope from every tier's blur and vertical offset.
/// Each side is nonnegative; this reserves room around clipped elevated content.
#[must_use]
pub fn ink_margin(scale: &ElevationScale) -> iced::Padding {
    (0u8..)
        .take(scale.levels())
        .fold(iced::Padding::ZERO, |margin, level| {
            let tier = scale.tier(level);
            iced::Padding {
                top: margin.top.max(tier.blur - tier.y_offset),
                right: margin.right.max(tier.blur),
                bottom: margin.bottom.max(tier.blur + tier.y_offset),
                left: margin.left.max(tier.blur),
            }
        })
}

/// Return the default dark or light palette's shadow colour.
/// Use [`iced_shadow_for_theme`] to follow a runtime theme's colour.
pub fn elevation_shadow_color(is_dark: bool) -> Color {
    if is_dark {
        enzui_theme::Palette::dark().shadow
    } else {
        enzui_theme::Palette::light().shadow
    }
}

/// Resolve a stock shadow for the colour mode; level zero returns the default shadow.
pub fn iced_shadow_for_level(level: u8, is_dark: bool) -> Shadow {
    iced_shadow_from(&ElevationScale::default(), level, is_dark)
}

/// Resolve an explicit ladder using the default shadow colour for the mode.
pub fn iced_shadow_from(scale: &ElevationScale, level: u8, is_dark: bool) -> Shadow {
    iced_shadow_from_color(scale, level, elevation_shadow_color(is_dark))
}

/// Resolve an explicit ladder and colour; zero geometry returns the default shadow.
pub fn iced_shadow_from_color(scale: &ElevationScale, level: u8, color: Color) -> Shadow {
    match shadow_shape_from(scale, level) {
        None => Shadow::default(),
        Some(s) => Shadow {
            color,
            offset: Vector {
                x: 0.0,
                y: s.y_offset,
            },
            blur_radius: s.blur,
        },
    }
}

/// Resolve the runtime theme's elevation geometry and shadow colour.
pub fn iced_shadow_for_theme(theme: &iced::Theme, level: u8) -> Shadow {
    iced_shadow_from_color(
        &crate::theme_lookup::elevation(theme),
        level,
        crate::theme_lookup::shadow_color(theme),
    )
}

/// Interpolate stock shadow geometry with a constant mode-dependent colour.
/// Finite progress clamps to `0.0..=1.0`; non-finite progress selects the source.
pub fn iced_shadow_transition(from: u8, to: u8, t: f32, is_dark: bool) -> Shadow {
    iced_shadow_transition_from(&ElevationScale::default(), from, to, t, is_dark)
}

/// Interpolate the runtime theme's geometry with its shadow colour.
/// Progress follows [`ElevationShadow::lerp`].
pub fn iced_shadow_transition_for_theme(theme: &iced::Theme, from: u8, to: u8, t: f32) -> Shadow {
    iced_shadow_transition_from_color(
        &crate::theme_lookup::elevation(theme),
        from,
        to,
        t,
        crate::theme_lookup::shadow_color(theme),
    )
}

/// Resolve a themed transition, snapping to the target under reduced motion.
/// An explicit override takes precedence over the theme's reduced-motion policy.
pub fn iced_shadow_transition_for_theme_rm(
    theme: &iced::Theme,
    from: u8,
    to: u8,
    t: f32,
    reduced_override: Option<bool>,
) -> Shadow {
    let reduced = reduced_override.unwrap_or_else(|| crate::theme_lookup::reduced_motion(theme));
    if reduced {
        iced_shadow_for_theme(theme, to)
    } else {
        iced_shadow_transition_for_theme(theme, from, to, t)
    }
}

/// Interpolate an explicit ladder using the default shadow colour for the mode.
/// Progress follows [`ElevationShadow::lerp`].
pub fn iced_shadow_transition_from(
    scale: &ElevationScale,
    from: u8,
    to: u8,
    t: f32,
    is_dark: bool,
) -> Shadow {
    iced_shadow_transition_from_color(scale, from, to, t, elevation_shadow_color(is_dark))
}

/// Interpolate an explicit ladder with a constant colour.
/// Progress follows [`ElevationShadow::lerp`]; nonpositive blur and offset together
/// return the default shadow, including a zero-blur upward offset.
pub fn iced_shadow_transition_from_color(
    scale: &ElevationScale,
    from: u8,
    to: u8,
    t: f32,
    color: Color,
) -> Shadow {
    let zero = ElevationShadow {
        blur: 0.0,
        y_offset: 0.0,
    };
    let a = shadow_shape_from(scale, from).unwrap_or(zero);
    let b = shadow_shape_from(scale, to).unwrap_or(zero);
    let s = a.lerp(b, t);
    if s.blur <= 0.0 && s.y_offset <= 0.0 {
        return Shadow::default();
    }
    Shadow {
        color,
        offset: Vector {
            x: 0.0,
            y: s.y_offset,
        },
        blur_radius: s.blur,
    }
}

/// Default corner radius in pixels.
pub const ELEVATION_RADIUS_LG: f32 = 10.0;

/// Wrap content at elevation level one, with a 10 px radius and shrink sizing.
pub fn enz_elevation<'a, Message>(
    child: impl Into<Element<'a, Message>>,
) -> EnzElevation<'a, Message>
where
    Message: 'a,
{
    EnzElevation {
        child: child.into(),
        level: 1,
        radius: ELEVATION_RADIUS_LG,
        width: Length::Shrink,
        height: Length::Shrink,
        transition_from: None,
        progress: None,
        reduced_motion: None,
    }
}

/// A card wrapper with themed elevation and automatic level-change transitions.
/// Uses [`CardTone::Default`](crate::card::CardTone::Default), zero padding,
/// and an outline when the target level is above zero.
#[must_use]
pub struct EnzElevation<'a, Message> {
    child: Element<'a, Message>,
    level: u8,
    radius: f32,
    width: Length,
    height: Length,
    transition_from: Option<u8>,
    progress: Option<f32>,
    reduced_motion: Option<bool>,
}

impl<'a, Message> EnzElevation<'a, Message>
where
    Message: 'a,
{
    /// Set the target level, clamping values above five.
    /// The stock ladder is flush at zero; custom ladders may supply nonzero geometry there.
    pub fn level(mut self, level: u8) -> Self {
        self.level = level.min(5);
        self
    }

    /// Set the radius in pixels; negative or non-finite values become zero.
    pub fn elevation_radius(mut self, r: f32) -> Self {
        self.radius = if r.is_finite() { r.max(0.0) } else { 0.0 };
        self
    }

    /// Set the source level for an explicitly pinned [`progress`](Self::progress).
    /// Levels above five clamp; this alone does not start an animation.
    pub fn transition_from(mut self, from_level: u8) -> Self {
        self.transition_from = Some(from_level.min(5));
        self
    }

    /// Pin transition progress and disable the internal animation clock.
    /// Finite values clamp to `0.0..=1.0`; non-finite values select the target.
    /// Without [`transition_from`](Self::transition_from), source and target coincide.
    pub fn progress(mut self, t: f32) -> Self {
        self.progress = Some(if t.is_finite() {
            t.clamp(0.0, 1.0)
        } else {
            1.0
        });
        self
    }

    /// Override the theme's reduced-motion policy for this instance.
    /// `true` snaps to the target; `false` permits interpolation under a reduced-motion theme.
    pub fn reduced_motion(mut self, reduced: bool) -> Self {
        self.reduced_motion = Some(reduced);
        self
    }

    /// Resolve the target or explicitly pinned transition for introspection.
    /// Honours reduced motion for pinned transitions; it does not read the widget tree
    /// or observe the internal animation's current progress.
    pub fn resolved_shadow(&self, theme: &iced::Theme) -> Shadow {
        match self.transition_from.zip(self.progress) {
            Some((from, t)) => {
                iced_shadow_transition_for_theme_rm(theme, from, self.level, t, self.reduced_motion)
            }
            None => iced_shadow_for_theme(theme, self.level),
        }
    }

    /// Set the outer width; the default is [`Length::Shrink`].
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Set the outer height; the default is [`Length::Shrink`].
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }
}

impl<'a, Message: Clone + 'a> EnzElevation<'a, Message> {
    /// Build the elevation element, including its animation wrapper when progress is unset.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        Element::from(self)
    }
}

impl<'a, Message: Clone + 'a> From<EnzElevation<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzElevation<'a, Message>) -> Self {
        use crate::card::{CardTone, enz_card};

        let level = w.level;
        let radius = w.radius;
        let reduced_override = w.reduced_motion;
        let width = w.width;
        let height = w.height;
        if let Some(t) = w.progress {
            let from = w.transition_from.unwrap_or(level);
            return enz_card(w.child)
                .tone(CardTone::Default)
                .radius(radius)
                .padding(iced::Padding::ZERO)
                .elevation_override_rm(level, Some((from, t)), reduced_override)
                .width_length(width)
                .height_length(height)
                .into();
        }
        let live = crate::card::ElevationLive {
            from: Rc::new(Cell::new(w.transition_from.unwrap_or(level))),
            progress: Rc::new(Cell::new(1.0)),
        };
        let card: Element<'a, Message> = enz_card(w.child)
            .tone(CardTone::Default)
            .radius(radius)
            .padding(iced::Padding::ZERO)
            .elevation_override_rm(level, Some((level, 1.0)), reduced_override)
            .elevation_live(live.clone())
            .width_length(width)
            .height_length(height)
            .into();

        Element::new(ElevationAnim {
            inner: card,
            level,
            reduced_override,
            live,
        })
    }
}

#[derive(Debug)]
struct ElevationAnimState {
    anim: Animation<bool>,
    prev_level: u8,
    motion: crate::motion::MotionLatch,
    // All phase reads in a frame use the compositor's latched timestamp.
    frame_at: Cell<Option<Instant>>,
    pre_frame_endpoint: bool,
}

const DEFAULT_NORMAL_MS: f32 = 180.0;

impl ElevationAnimState {
    fn new(level: u8, reduced_motion: bool) -> Self {
        let applied_ms = if reduced_motion {
            0.0
        } else {
            DEFAULT_NORMAL_MS
        };
        let easing = enzui_theme::MotionEasing::default();
        let anim = Self::make_anim(true, applied_ms, easing);
        Self {
            anim,
            prev_level: level,
            motion: crate::motion::MotionLatch::new(DEFAULT_NORMAL_MS, applied_ms, easing),
            frame_at: Cell::new(None),
            pre_frame_endpoint: true,
        }
    }

    fn make_anim(
        initial: bool,
        effective_ms: f32,
        easing: enzui_theme::MotionEasing,
    ) -> Animation<bool> {
        crate::motion::build_anim(initial, effective_ms, easing)
    }

    fn effective_normal_ms(&self, override_reduced: Option<bool>) -> f32 {
        self.motion.effective_ms(override_reduced)
    }

    fn sync_duration(&mut self, effective_ms: f32) {
        self.motion.sync_animation(&mut self.anim, effective_ms);
    }

    fn note_frame(&self, now: Instant) {
        self.frame_at.set(Some(now));
    }
    fn sampled_progress(&self) -> f32 {
        self.frame_at.get().map_or_else(
            || if self.pre_frame_endpoint { 1.0 } else { 0.0 },
            |now| self.anim.interpolate(0.0, 1.0, now).clamp(0.0, 1.0),
        )
    }

    fn sampled_animating(&self) -> bool {
        self.frame_at.get().map_or_else(
            || self.anim.value() != self.pre_frame_endpoint,
            |now| self.anim.is_animating(now),
        )
    }
}

struct ElevationAnim<'a, Message> {
    inner: Element<'a, Message>,
    level: u8,
    reduced_override: Option<bool>,
    live: crate::card::ElevationLive,
}

impl<'a, Message> ElevationAnim<'a, Message> {
    fn advance_own_frame(
        &self,
        tree: &mut widget::Tree,
        event: &Event,
        shell: &mut Shell<'_, Message>,
    ) {
        if let Event::Window(window::Event::RedrawRequested(now)) = event {
            let state = tree.state.downcast_mut::<ElevationAnimState>();
            state.note_frame(*now);
            let effective_ms = state.effective_normal_ms(self.reduced_override);
            state.sync_duration(effective_ms);
            self.live.progress.set(state.sampled_progress());
            if state.sampled_animating() {
                shell.request_redraw();
            }
        }
    }
}

impl<'a, Message: Clone + 'a> Widget<Message, iced::Theme, iced::Renderer>
    for ElevationAnim<'a, Message>
{
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<ElevationAnimState>()
    }

    fn state(&self) -> widget::tree::State {
        let initial_rm = self.reduced_override.unwrap_or(false);
        widget::tree::State::new(ElevationAnimState::new(self.level, initial_rm))
    }

    fn diff(&mut self, tree: &mut widget::Tree) {
        let state = tree.state.downcast_mut::<ElevationAnimState>();
        let effective_ms = state.effective_normal_ms(self.reduced_override);
        state.sync_duration(effective_ms);

        if self.level != state.prev_level {
            self.live.from.set(state.prev_level);
            if state.frame_at.get().is_none() {
                state.pre_frame_endpoint = false;
            }
            state.prev_level = self.level;
            state.anim = state.motion.build_animation(false, effective_ms);
            // Diff has no frame event, so a level edge needs a wall-clock anchor.
            state.anim.go_mut(true, Instant::now());
            self.live.progress.set(state.sampled_progress());
        }

        tree.diff_children(std::slice::from_mut(&mut self.inner));
    }

    fn size(&self) -> Size<Length> {
        self.inner.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) {
        self.inner
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
        self.advance_own_frame(tree, event, shell);

        self.inner.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        crate::frame_routing::refresh_opaque_child(&self.inner, tree, 0);
    }

    fn frame_interest(&self, tree: &widget::Tree) -> bool {
        tree.state
            .downcast_ref::<ElevationAnimState>()
            .sampled_animating()
            || crate::frame_routing::interest_opaque(tree)
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
        self.advance_own_frame(tree, event, shell);
        crate::frame_routing::forward_frame_opaque(
            &mut self.inner,
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

    fn mouse_interaction(
        &self,
        tree: &widget::Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.inner.as_widget().mouse_interaction(
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
        let state = tree.state.downcast_ref::<ElevationAnimState>();
        let motion = crate::theme_lookup::motion(theme);
        // Update has no theme; draw transports raw timing and policy to the latch.
        state.motion.observe(
            motion.normal_ms,
            motion.reduced_motion,
            motion.easing_or(None),
        );
        self.live.progress.set(state.sampled_progress());

        self.inner.as_widget().draw(
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
        self.inner.as_widget_mut().operate(
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
        translation: iced::Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.inner.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

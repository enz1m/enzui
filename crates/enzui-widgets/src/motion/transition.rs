//! A painted value easing from what was shown to what the state now asks
//! for: the colour fades of controls (a hover wash, a selected fill, a
//! focused field's border).

use crate::focus_button::FocusRingTheme;
use enzui_theme::{MotionEasing, MotionToken};
use iced::advanced::renderer::{NodeId, Track, TrackAnimation};
use iced::time::{Duration, Instant};
use iced::{Background, Border, Color, Shadow, Vector};
use std::any::Any;
use std::cell::{Cell, RefCell};

/// A value that can be painted part of the way to another.
pub(crate) trait Lerp: Clone + PartialEq {
    /// `self` moved `t` (`0..=1`) of the way to `to`.
    fn lerp(&self, to: &Self, t: f32) -> Self;
}

impl Lerp for f32 {
    fn lerp(&self, to: &Self, t: f32) -> Self {
        self + (to - self) * t
    }
}

impl Lerp for Color {
    fn lerp(&self, to: &Self, t: f32) -> Self {
        // A fully clear end fades from the other colour at zero alpha, so a
        // wash appearing on nothing does not pass through black.
        let from = if self.a == 0.0 {
            Color { a: 0.0, ..*to }
        } else {
            *self
        };
        let to = if to.a == 0.0 {
            Color { a: 0.0, ..from }
        } else {
            *to
        };
        Color {
            r: from.r.lerp(&to.r, t),
            g: from.g.lerp(&to.g, t),
            b: from.b.lerp(&to.b, t),
            a: from.a.lerp(&to.a, t),
        }
    }
}

impl<A: Lerp, B: Lerp> Lerp for (A, B) {
    fn lerp(&self, to: &Self, t: f32) -> Self {
        (self.0.lerp(&to.0, t), self.1.lerp(&to.1, t))
    }
}

impl Lerp for Vector {
    fn lerp(&self, to: &Self, t: f32) -> Self {
        Vector::new(self.x.lerp(&to.x, t), self.y.lerp(&to.y, t))
    }
}

impl Lerp for Option<Background> {
    /// Flat colours (and no fill, as clear) blend; a gradient lands at once.
    fn lerp(&self, to: &Self, t: f32) -> Self {
        let flat = |background: &Self| match background {
            None => Some(Color::TRANSPARENT),
            Some(Background::Color(color)) => Some(*color),
            Some(_) => None,
        };
        match (flat(self), flat(to)) {
            (Some(from), Some(target)) if to.is_some() || t < 1.0 => {
                Some(Background::Color(from.lerp(&target, t)))
            }
            _ => *to,
        }
    }
}

impl Lerp for Border {
    fn lerp(&self, to: &Self, t: f32) -> Self {
        let r = |from: f32, to: f32| from.lerp(&to, t);
        Border {
            color: self.color.lerp(&to.color, t),
            width: self.width.lerp(&to.width, t),
            radius: iced::border::Radius {
                top_left: r(self.radius.top_left, to.radius.top_left),
                top_right: r(self.radius.top_right, to.radius.top_right),
                bottom_right: r(self.radius.bottom_right, to.radius.bottom_right),
                bottom_left: r(self.radius.bottom_left, to.radius.bottom_left),
            },
        }
    }
}

impl Lerp for Shadow {
    fn lerp(&self, to: &Self, t: f32) -> Self {
        Shadow {
            color: self.color.lerp(&to.color, t),
            offset: self.offset.lerp(&to.offset, t),
            blur_radius: self.blur_radius.lerp(&to.blur_radius, t),
        }
    }
}

impl Lerp for iced::widget::button::Style {
    fn lerp(&self, to: &Self, t: f32) -> Self {
        Self {
            background: self.background.lerp(&to.background, t),
            text_color: self.text_color.lerp(&to.text_color, t),
            border: self.border.lerp(&to.border, t),
            shadow: self.shadow.lerp(&to.shadow, t),
            snap: to.snap,
        }
    }
}

/// A painted value moving from what was last shown to its target over a
/// theme duration, read at presented-frame instants. A target that changes
/// mid-way starts again from where the paint is, so nothing jumps.
#[derive(Debug, Clone)]
pub(crate) struct Transition<T> {
    from: T,
    to: T,
    started: Option<Instant>,
    ms: f32,
    easing: MotionEasing,
    /// Counts retargets, so the render lane tells a new travel from the same
    /// one published again.
    generation: u64,
}

impl<T: Lerp> Transition<T> {
    /// Resting at `value`.
    pub(crate) fn new(value: T) -> Self {
        Self {
            from: value.clone(),
            to: value,
            started: None,
            ms: 0.0,
            easing: MotionEasing::EaseOut,
            generation: 0,
        }
    }

    /// The painted value at `now`.
    pub(crate) fn value(&self, now: Instant) -> T {
        match self.started {
            Some(started) if self.ms > 0.0 => {
                let t = now.saturating_duration_since(started).as_secs_f32() * 1000.0 / self.ms;
                if t >= 1.0 {
                    self.to.clone()
                } else {
                    self.from.lerp(&self.to, self.easing.apply(t))
                }
            }
            _ => self.to.clone(),
        }
    }

    /// Move toward `target` from the painted value at `now`, over `ms` (zero
    /// lands at once). Returns whether the value is still moving.
    pub(crate) fn retarget(
        &mut self,
        target: T,
        now: Instant,
        ms: f32,
        easing: MotionEasing,
    ) -> bool {
        if target != self.to {
            self.from = self.value(now);
            self.to = target;
            self.ms = ms.max(0.0);
            self.easing = easing;
            self.started = (self.ms > 0.0).then_some(now);
            self.generation = self.generation.wrapping_add(1);
        }
        if !self.animating(now) {
            self.started = None;
        }
        self.started.is_some()
    }

    /// Land on `target` at once.
    pub(crate) fn snap(&mut self, target: T) {
        let generation = self.generation.wrapping_add(1);
        *self = Self::new(target);
        self.generation = generation;
    }

    /// When the moving value lands, while it moves.
    pub(crate) fn lands_at(&self) -> Option<Instant> {
        self.started
            .map(|started| started + Duration::from_secs_f32(self.ms / 1000.0))
    }

    /// The whole travel, eased, as keyframes the render lane samples without
    /// the UI thread; `None` once it has landed at `now`.
    pub(crate) fn track_animation(
        &self,
        node: NodeId,
        now: Instant,
        track: impl Fn(T) -> Track,
    ) -> Option<TrackAnimation> {
        let started = self.started?;
        let duration = Duration::from_secs_f32(self.ms / 1000.0);
        if now.saturating_duration_since(started) >= duration {
            return None;
        }
        let samples =
            ((duration.as_secs_f32() * super::SPRING_TRACK_RATE).ceil() as usize).clamp(8, 480);
        let keyframes = (0..=samples)
            .map(|index| {
                let at = index as f32 / samples as f32;
                (at, track(self.from.lerp(&self.to, self.easing.apply(at))))
            })
            .collect();
        Some(TrackAnimation {
            node,
            start: started,
            duration,
            keyframes,
            generation: self.generation,
        })
    }

    /// Whether the value is moving at `now`.
    pub(crate) fn animating(&self, now: Instant) -> bool {
        self.started.is_some_and(|started| {
            now.saturating_duration_since(started).as_secs_f32() * 1000.0 < self.ms
        })
    }

    /// Whether a motion has started and not yet been seen to finish.
    pub(crate) fn is_moving(&self) -> bool {
        self.started.is_some()
    }

    /// What to paint when the state asks for `target`: the moving value when
    /// this transition is heading there, else `target` itself (a state the
    /// frame clock has not seen yet lands as it is).
    pub(crate) fn paint(&self, target: &T, frame_at: Option<Instant>) -> T {
        match frame_at {
            Some(now) if *target == self.to => self.value(now),
            _ => target.clone(),
        }
    }
}

/// A widget's painted style fading between the states it is drawn in:
/// draw paints through it and keeps the theme, and a presented frame
/// resolves what the state now asks for with that theme and moves toward
/// it. A first draw, a new theme and reduced motion land at once.
pub(crate) struct StateFade<T> {
    fade: RefCell<Option<Transition<T>>>,
    theme: RefCell<Option<Box<dyn Any>>>,
    theme_changes: Cell<u64>,
    faded_theme: u64,
    frame_at: Option<Instant>,
}

impl<T> Default for StateFade<T> {
    fn default() -> Self {
        Self {
            fade: RefCell::new(None),
            theme: RefCell::new(None),
            theme_changes: Cell::new(0),
            faded_theme: 0,
            frame_at: None,
        }
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for StateFade<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StateFade")
            .field("fade", &self.fade)
            .field("frame_at", &self.frame_at)
            .finish_non_exhaustive()
    }
}

impl<T: Lerp> StateFade<T> {
    /// What to paint in `draw` when the state asks for `target`.
    pub(crate) fn paint<Theme: FocusRingTheme>(&self, theme: &Theme, target: T) -> T {
        let had_theme = self.theme.borrow().is_some();
        if theme.enz_latch(&mut self.theme.borrow_mut()) && had_theme {
            self.theme_changes.set(self.theme_changes.get() + 1);
        }
        let mut fade = self.fade.borrow_mut();
        match &*fade {
            Some(fade) => fade.paint(&target, self.frame_at),
            None => {
                *fade = Some(Transition::new(target.clone()));
                target
            }
        }
    }

    /// At the presented frame `now`, move toward what `target` resolves
    /// with the kept theme. `reduced` overrides the theme's reduced-motion
    /// policy; `snap` lands at once (a press). Returns whether the paint is
    /// still moving, so the caller asks for another frame.
    pub(crate) fn frame<Theme: FocusRingTheme>(
        &mut self,
        now: Instant,
        reduced: Option<bool>,
        snap: bool,
        target: impl FnOnce(&Theme) -> T,
    ) -> bool {
        self.frame_over(now, reduced, snap, MotionToken::Fast, target)
    }

    /// [`Self::frame`] over the theme's duration for `token`.
    pub(crate) fn frame_over<Theme: FocusRingTheme>(
        &mut self,
        now: Instant,
        reduced: Option<bool>,
        snap: bool,
        token: MotionToken,
        target: impl FnOnce(&Theme) -> T,
    ) -> bool {
        self.frame_at = Some(now);
        let Self {
            fade,
            theme,
            theme_changes,
            faded_theme,
            ..
        } = self;
        let theme = theme.borrow();
        let (Some(theme), Some(fade)) = (Theme::enz_latched(&theme), fade.get_mut()) else {
            return false;
        };
        let target = target(theme);
        let new_theme = *faded_theme != theme_changes.get();
        *faded_theme = theme_changes.get();
        if snap || new_theme || reduced.unwrap_or_else(|| theme.enz_reduced_motion()) {
            fade.snap(target);
            false
        } else {
            let (ms, easing) = theme.enz_motion(token);
            fade.retarget(target, now, ms, easing)
        }
    }

    /// Whether the painted value is heading somewhere other than `target`,
    /// so a frame is needed to start moving.
    pub(crate) fn needs_frame(&self, target: &T) -> bool {
        self.fade
            .borrow()
            .as_ref()
            .is_some_and(|fade| fade.to != *target)
    }

    /// The painted value at `now`, once a draw has started the fade.
    pub(crate) fn value_at(&self, now: Instant) -> Option<T> {
        self.fade.borrow().as_ref().map(|fade| fade.value(now))
    }

    /// Whether a fade has started and a frame has not yet seen it end.
    pub(crate) fn is_moving(&self) -> bool {
        self.fade
            .borrow()
            .as_ref()
            .is_some_and(Transition::is_moving)
    }

    /// When the fade lands, while it moves.
    pub(crate) fn lands_at(&self) -> Option<Instant> {
        self.fade.borrow().as_ref().and_then(Transition::lands_at)
    }

    /// Hands the rest of the fade to the render lane as `node`'s travel, so
    /// the lane carries it without waking the UI thread; see
    /// [`Transition::track_animation`]. Call on every advancing frame, with
    /// the sampled track for that frame published too.
    pub(crate) fn publish<Message>(
        &self,
        shell: &mut iced::advanced::Shell<'_, Message>,
        node: NodeId,
        now: Instant,
        track: impl Fn(T) -> Track,
    ) {
        if let Some(animation) = self
            .fade
            .borrow()
            .as_ref()
            .and_then(|fade| fade.track_animation(node, now, track))
        {
            shell.publish_track_animation(animation);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Lerp, Transition};
    use enzui_theme::MotionEasing;
    use iced::time::{Duration, Instant};
    use iced::{Background, Color};

    #[test]
    fn a_retarget_mid_way_starts_from_the_painted_value() {
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let mut fade = Transition::new(0.0_f32);
        assert!(fade.retarget(1.0, at(0), 100.0, MotionEasing::Linear));
        assert!((fade.value(at(50)) - 0.5).abs() < 1e-4);
        // Back toward 0 from 0.5, not from 1.
        assert!(fade.retarget(0.0, at(50), 100.0, MotionEasing::Linear));
        assert!((fade.value(at(50)) - 0.5).abs() < 1e-4);
        assert!((fade.value(at(100)) - 0.25).abs() < 1e-4);
        assert!(!fade.retarget(0.0, at(150), 100.0, MotionEasing::Linear));
        assert_eq!(fade.value(at(150)), 0.0);
        // Zero duration lands at once.
        assert!(!fade.retarget(1.0, at(200), 0.0, MotionEasing::Linear));
        assert_eq!(fade.value(at(200)), 1.0);
    }

    #[test]
    fn a_wash_appearing_on_nothing_fades_its_own_colour_in() {
        let wash = Some(Background::Color(Color::from_rgb(0.2, 0.4, 0.8)));
        let half = None.lerp(&wash, 0.5);
        assert_eq!(
            half,
            Some(Background::Color(Color::from_rgba(0.2, 0.4, 0.8, 0.5)))
        );
        assert_eq!(wash.lerp(&None, 1.0), None);
    }
}

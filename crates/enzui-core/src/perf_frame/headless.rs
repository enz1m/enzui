use core::time::Duration;
use std::fmt;
use std::time::Instant;

use iced_core::renderer::Headless;
use iced_core::theme::Base;
use iced_core::{Element, Event, Point, Size, mouse, renderer, window};

use super::{page_builds, reset_page_builds};

/// Concrete renderer selected by the consuming test build.
pub type Renderer = iced_test::renderer::Renderer;

type UserInterface<'a, Message, Theme> =
    iced_test::runtime::user_interface::UserInterface<'a, Message, Theme, Renderer>;
type Cache = iced_test::runtime::user_interface::Cache;

/// Supplies application state, update behavior, theme, and a borrowed view to [`PerfHarness`].
pub trait PerfSubject {
    /// Message applied through [`update`](Self::update) and emitted by the headless interface.
    type Message: Clone;
    /// Theme used for drawing; [`Base`] supplies frame colors.
    type Theme: Base;

    /// Builds the application’s top-level view, borrowing `self`.
    fn view(&self) -> Element<'_, Self::Message, Self::Theme, Renderer>;

    /// Applies one message to the subject model.
    fn update(&mut self, message: Self::Message);

    /// Returns the theme once per paint for clear and text colors.
    fn theme(&self) -> Self::Theme;
}

/// Reports layout-only and full-raster timings after an input has been applied, with routed-page build counts.
#[derive(Debug, Clone, Copy)]
pub struct ClickToPaint {
    /// Wall time to build and lay out the frame, excluding drawing and rasterization.
    pub layout: Duration,
    /// Time to build, lay out, draw, and rasterize the updated frame.
    pub paint: Duration,
    /// Routed-page builds recorded during the paint frame.
    pub builds_per_frame: usize,
    /// Routed-page builds recorded during the layout-only pass.
    pub builds_per_layout: usize,
}

impl fmt::Display for ClickToPaint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "layout {:?} | click→paint {:?} | page builds/frame {} (layout {})",
            self.layout, self.paint, self.builds_per_frame, self.builds_per_layout
        )
    }
}

/// Realized scroll geometry read from the retained widget tree.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollGeometry {
    /// Vertical offset in logical pixels from the top of the content.
    pub offset_y: f32,
    /// The largest vertical offset the content permits (content minus
    /// viewport height, clamped at zero for content shorter than the viewport).
    pub max_offset_y: f32,
}

impl ScrollGeometry {
    /// Returns normalized position in `0.0..=1.0`, or zero when no scrolling is possible.
    #[must_use]
    pub fn fraction(&self) -> f32 {
        if self.max_offset_y > 0.0 {
            (self.offset_y / self.max_offset_y).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }
}

/// Drives layout and full paint passes over one [`PerfSubject`] using retained renderer state.
pub struct PerfHarness<S: PerfSubject> {
    subject: S,
    renderer: Renderer,
    cache: Cache,
    size: Size,
    // Exact redraw disposition from the most recent UI update.
    redraw: RuntimeRedrawRequest,
}

/// Describes the exact redraw disposition from a headless runtime update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeRedrawRequest {
    /// The interface invalidated widgets and must be rebuilt immediately.
    Outdated,
    /// The runtime requested the next available frame.
    NextFrame,
    /// The runtime requested a frame at an exact monotonic deadline.
    At {
        /// The exact process-monotonic deadline returned by the runtime.
        deadline: Instant,
        /// Deadline offset from the delivered `RedrawRequested` timestamp.
        requested_delay: Duration,
        /// Time remaining after painting. Zero means the deadline became due
        /// while rendering and therefore needs immediate service.
        remaining_after_paint: Duration,
    },
    /// No follow-up redraw was requested.
    Wait,
}

impl RuntimeRedrawRequest {
    /// Whether this disposition requires another frame without waiting for a
    /// future deadline.
    #[must_use]
    pub fn is_immediate(self) -> bool {
        matches!(self, Self::Outdated | Self::NextFrame)
            || matches!(self, Self::At { remaining_after_paint, .. } if remaining_after_paint.is_zero())
    }

    /// Stable machine-readable variant name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Outdated => "outdated",
            Self::NextFrame => "next_frame",
            Self::At { .. } => "at",
            Self::Wait => "wait",
        }
    }

    /// Deadline offset from the delivered redraw timestamp, for `At` only.
    #[must_use]
    pub fn deadline_offset_ns(self) -> Option<u64> {
        match self {
            Self::At {
                requested_delay, ..
            } => Some(u64::try_from(requested_delay.as_nanos()).unwrap_or(u64::MAX)),
            _ => None,
        }
    }

    /// Time still remaining on an `At` request after paint completed.
    #[must_use]
    pub fn remaining_after_paint_ns(self) -> Option<u64> {
        match self {
            Self::At {
                remaining_after_paint,
                ..
            } => Some(u64::try_from(remaining_after_paint.as_nanos()).unwrap_or(u64::MAX)),
            _ => None,
        }
    }
}

impl<S: PerfSubject> PerfHarness<S> {
    /// Creates a harness at `size`, honoring `ICED_TEST_BACKEND`.
    ///
    /// # Panics
    /// Panics when no headless renderer can be constructed.
    #[must_use]
    pub fn new(subject: S, size: Size) -> Self {
        let renderer =
            iced_test::futures::futures::executor::block_on(<Renderer as Headless>::new(
                renderer::Settings::default(),
                std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
            ))
            .expect("headless renderer");
        Self {
            subject,
            renderer,
            cache: Cache::default(),
            size,
            redraw: RuntimeRedrawRequest::Wait,
        }
    }

    /// Returns the active raster backend name, such as `tiny-skia` or `wgpu`.
    #[must_use]
    pub fn renderer_name(&self) -> String {
        self.renderer.name()
    }

    /// Immutable access to the subject (to read model state between clicks).
    pub fn subject(&self) -> &S {
        &self.subject
    }

    /// Mutable access to the subject.
    pub fn subject_mut(&mut self) -> &mut S {
        &mut self.subject
    }

    /// Applies a message without measurement to prepare subject state.
    pub fn apply(&mut self, message: S::Message) {
        self.subject.update(message);
    }

    /// Builds and lays out one frame without drawing or rasterizing it.
    pub fn layout_pass(&mut self) {
        let Self {
            subject,
            renderer,
            cache,
            size,
            ..
        } = self;
        let view = subject.view();
        let ui = UserInterface::build(view, *size, std::mem::take(cache), renderer);
        *cache = ui.into_cache();
    }

    /// Builds, lays out, redraws, and rasterizes one frame to RGBA bytes.
    ///
    /// GPU presentation and vertical synchronization are not measured.
    pub fn paint_pass(&mut self) -> Vec<u8> {
        self.paint_pass_inner(None, None).0
    }

    #[cfg(feature = "perf-report")]
    pub(crate) fn paint_pass_measured(&mut self) -> (Vec<u8>, Vec<S::Message>, Duration) {
        self.paint_pass_inner(None, None)
    }

    // iced negates pixel `y`, so a negative value moves the retained scrollable toward the content tail.
    #[cfg(feature = "perf-report")]
    pub(crate) fn scroll_pass_measured(
        &mut self,
        cursor: Point,
        pixels_y: f32,
    ) -> (Vec<u8>, Vec<S::Message>, Duration) {
        self.paint_pass_inner(Some((cursor, pixels_y)), None)
    }

    // Called only while `WorkloadDriver` owns observer collection so collection is disabled after the run.
    #[cfg(feature = "perf-report")]
    pub(crate) fn paint_pass_observed(
        &mut self,
        window: iced_runtime::performance::WindowId,
    ) -> (Vec<u8>, Vec<S::Message>, Duration) {
        self.paint_pass_inner(None, Some(window))
    }

    #[cfg(feature = "perf-report")]
    pub(crate) fn scroll_pass_observed(
        &mut self,
        cursor: Point,
        pixels_y: f32,
        window: iced_runtime::performance::WindowId,
    ) -> (Vec<u8>, Vec<S::Message>, Duration) {
        self.paint_pass_inner(Some((cursor, pixels_y)), Some(window))
    }

    /// Drives a real wheel scroll frame and returns messages emitted by the retained scrollable.
    ///
    /// Negative `pixels_y` advances toward the content tail.
    pub fn scroll(&mut self, cursor: Point, pixels_y: f32) -> Vec<S::Message> {
        self.paint_pass_inner(Some((cursor, pixels_y)), None).1
    }

    /// Reads realized geometry for `target` from retained widget state, or returns `None` when absent.
    pub fn scroll_geometry(&mut self, target: iced_core::widget::Id) -> Option<ScrollGeometry> {
        use iced_core::widget::Operation;
        use iced_core::widget::operation::Scrollable;
        use iced_core::{Rectangle, Vector};

        struct Read {
            target: iced_core::widget::Id,
            found: Option<ScrollGeometry>,
        }
        impl Operation for Read {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
                operate(self);
            }
            fn scrollable(
                &mut self,
                id: Option<&iced_core::widget::Id>,
                bounds: Rectangle,
                content_bounds: Size,
                translation: Vector,
                _state: &mut dyn Scrollable,
            ) {
                if id == Some(&self.target) {
                    self.found = Some(ScrollGeometry {
                        offset_y: translation.y,
                        max_offset_y: (content_bounds.height - bounds.height).max(0.0),
                    });
                }
            }
        }

        let Self {
            subject,
            renderer,
            cache,
            size,
            ..
        } = self;
        let view = subject.view();
        let mut ui = UserInterface::build(view, *size, std::mem::take(cache), renderer);
        let mut op = Read {
            target,
            found: None,
        };
        ui.operate(renderer, &mut op);
        *cache = ui.into_cache();
        op.found
    }

    fn paint_pass_inner(
        &mut self,
        scroll: Option<(Point, f32)>,
        #[cfg(feature = "perf-report")] observed_window: Option<
            iced_runtime::performance::WindowId,
        >,
        #[cfg(not(feature = "perf-report"))] _observed_window: Option<()>,
    ) -> (Vec<u8>, Vec<S::Message>, Duration) {
        let theme = self.subject.theme();
        let base = theme.base();
        let Self {
            subject,
            renderer,
            cache,
            size,
            redraw,
        } = self;
        let layout_started = Instant::now();
        #[cfg(feature = "perf-report")]
        let view_span = observed_window.map(|window| {
            iced_runtime::performance::begin_stage(
                window,
                iced_runtime::performance::Stage::ViewRebuild,
            )
        });
        let view = subject.view();
        #[cfg(feature = "perf-report")]
        drop(view_span);
        #[cfg(feature = "perf-report")]
        let mut ui = if let Some(window) = observed_window {
            UserInterface::build_observed(view, *size, std::mem::take(cache), renderer, window)
        } else {
            UserInterface::build(view, *size, std::mem::take(cache), renderer)
        };
        #[cfg(not(feature = "perf-report"))]
        let mut ui = UserInterface::build(view, *size, std::mem::take(cache), renderer);
        let layout_elapsed = layout_started.elapsed();
        let mut messages = iced_core::shell::Bus::new();
        let frame_at = iced_core::time::Instant::now();
        // Deliver wheel input before redraw in the same update so retained scroll state and emitted messages belong to that frame.
        let (events, update_cursor): (Vec<Event>, mouse::Cursor) = match scroll {
            Some((cursor, pixels_y)) => (
                vec![
                    Event::Mouse(mouse::Event::WheelScrolled {
                        delta: mouse::ScrollDelta::Pixels {
                            x: 0.0,
                            y: pixels_y,
                        },
                    }),
                    Event::Window(window::Event::RedrawRequested(frame_at)),
                ],
                mouse::Cursor::Available(cursor),
            ),
            None => (
                vec![Event::Window(window::Event::RedrawRequested(frame_at))],
                mouse::Cursor::Unavailable,
            ),
        };
        let (state, _) = ui.update(
            &window::Headless,
            &iced_core::shell::Waker::noop(),
            &events,
            update_cursor,
            renderer,
            &mut messages,
        );
        #[cfg(feature = "perf-report")]
        let draw_span = observed_window.map(|window| {
            iced_runtime::performance::begin_stage(
                window,
                iced_runtime::performance::Stage::DrawPrepare,
            )
        });
        ui.draw(
            renderer,
            &theme,
            &renderer::Style {
                text_color: base.text_color,
            },
            mouse::Cursor::Unavailable,
        );
        #[cfg(feature = "perf-report")]
        drop(draw_span);
        let scale = 1.0_f32;
        let physical = Size::new(
            (size.width * scale).round() as u32,
            (size.height * scale).round() as u32,
        );
        #[cfg(feature = "perf-report")]
        let submit_span = observed_window.map(|window| {
            iced_runtime::performance::begin_stage(
                window,
                iced_runtime::performance::Stage::GpuSubmit,
            )
        });
        let pixels = renderer.screenshot(physical, scale, base.background_color);
        #[cfg(feature = "perf-report")]
        drop(submit_span);
        *cache = ui.into_cache();
        // Preserve timed redraws exactly; a future deadline is not an immediate frame.
        *redraw = match state {
            iced_test::runtime::user_interface::State::Outdated => RuntimeRedrawRequest::Outdated,
            iced_test::runtime::user_interface::State::Updated { redraw_request, .. } => {
                match redraw_request {
                    window::RedrawRequest::NextFrame => RuntimeRedrawRequest::NextFrame,
                    window::RedrawRequest::At(deadline) => RuntimeRedrawRequest::At {
                        deadline,
                        requested_delay: deadline
                            .checked_duration_since(frame_at)
                            .unwrap_or_default(),
                        remaining_after_paint: deadline
                            .checked_duration_since(iced_core::time::Instant::now())
                            .unwrap_or_default(),
                    },
                    window::RedrawRequest::Wait => RuntimeRedrawRequest::Wait,
                }
            }
        };
        (pixels, messages.into_iter().collect(), layout_elapsed)
    }

    /// Returns the redraw disposition from the most recent paint pass.
    #[must_use]
    pub fn redraw_request(&self) -> RuntimeRedrawRequest {
        self.redraw
    }

    /// Runs two throwaway paint passes to warm renderer, font atlas, and glyph caches before steady-state measurement.
    pub fn warm(&mut self) {
        let _ = self.paint_pass();
        let _ = self.paint_pass();
    }

    /// Measures the first cold paint and a subsequent warmed paint; call this on a fresh harness before [`warm`](Self::warm).
    pub fn cold_vs_warm_paint(&mut self) -> (Duration, Duration) {
        let t = Instant::now();
        let _ = self.paint_pass();
        let cold = t.elapsed();
        for _ in 0..3 {
            let _ = self.paint_pass();
        }
        let t = Instant::now();
        let _ = self.paint_pass();
        let warmed = t.elapsed();
        (cold, warmed)
    }

    /// Applies `click` without timing the update, then returns layout-only and full-paint timings with per-pass build counts. Warm the harness first.
    pub fn measure(&mut self, click: S::Message) -> ClickToPaint {
        self.subject.update(click);

        reset_page_builds();
        let t = Instant::now();
        self.layout_pass();
        let layout = t.elapsed();
        let builds_per_layout = page_builds();

        reset_page_builds();
        let t = Instant::now();
        let _ = self.paint_pass();
        let paint = t.elapsed();
        let builds_per_frame = page_builds();

        ClickToPaint {
            layout,
            paint,
            builds_per_frame,
            builds_per_layout,
        }
    }
}
